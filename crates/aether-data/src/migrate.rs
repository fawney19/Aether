use std::collections::{HashMap, HashSet};

use sqlx::{
    migrate::{Migrate, MigrateError, Migrator},
    query, query_scalar, Connection, PgConnection, PgPool,
};
use tracing::{error, info, warn};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
static BASELINE_V2_SQL: &str = include_str!("../bootstrap/20260413020000_baseline_v2.sql");
const BASELINE_V2_CUTOFF_VERSION: i64 = 20260415000000;
const MIGRATIONS_TABLE_EXISTS_SQL: &str =
    "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL";
const PUBLIC_BASE_TABLE_COUNT_SQL: &str = r#"
SELECT COUNT(*)::BIGINT
FROM information_schema.tables
WHERE table_schema = 'public'
  AND table_type = 'BASE TABLE'
  AND table_name <> '_sqlx_migrations'
"#;
const AETHER_SCHEMA_FOOTPRINT_TABLE_COUNT_SQL: &str = r#"
SELECT COUNT(*)::BIGINT
FROM information_schema.tables
WHERE table_schema = 'public'
  AND table_type = 'BASE TABLE'
  AND table_name IN (
    'api_key_provider_mappings',
    'auth_modules',
    'gemini_file_mappings',
    'global_models',
    'oauth_providers',
    'provider_api_keys',
    'proxy_nodes',
    'usage_routing_snapshots',
    'usage_settlement_snapshots'
  )
"#;
const INSERT_APPLIED_MIGRATION_SQL: &str = r#"
INSERT INTO _sqlx_migrations (
    version,
    description,
    success,
    checksum,
    execution_time
) VALUES (
    $1,
    $2,
    TRUE,
    $3,
    0
)
ON CONFLICT (version) DO NOTHING
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMigrationInfo {
    pub version: i64,
    pub description: String,
}

/// Run all pending migrations embedded at compile time from `migrations/`.
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    let mut conn = pool.acquire().await?;

    if MIGRATOR.locking {
        conn.lock().await?;
    }

    let result = run_migrations_locked(&mut conn).await;

    if MIGRATOR.locking {
        match conn.unlock().await {
            Ok(()) => {}
            Err(unlock_error) if result.is_ok() => return Err(unlock_error),
            Err(unlock_error) => {
                warn!(
                    error = %unlock_error,
                    "database migration lock release failed after migration error"
                );
            }
        }
    }

    result
}

pub async fn pending_migrations(pool: &PgPool) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    let mut conn = pool.acquire().await?;
    pending_migrations_locked(&mut conn).await
}

pub async fn prepare_database_for_startup(
    pool: &PgPool,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    let mut conn = pool.acquire().await?;

    if MIGRATOR.locking {
        conn.lock().await?;
    }

    let result = prepare_database_for_startup_locked(&mut conn).await;

    if MIGRATOR.locking {
        match conn.unlock().await {
            Ok(()) => {}
            Err(unlock_error) if result.is_ok() => return Err(unlock_error),
            Err(unlock_error) => {
                warn!(
                    error = %unlock_error,
                    "database migration lock release failed after startup preparation error"
                );
            }
        }
    }

    result
}

async fn run_migrations_locked(conn: &mut PgConnection) -> Result<(), MigrateError> {
    conn.ensure_migrations_table().await?;
    bootstrap_empty_database_to_baseline_v2(conn).await?;

    if let Some(version) = conn.dirty_version().await? {
        error!(version, "database migration state is dirty");
        return Err(MigrateError::Dirty(version));
    }

    let applied_migrations = conn.list_applied_migrations().await?;
    validate_applied_migrations(&applied_migrations)?;

    let known_versions: HashSet<_> = MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.version)
        .collect();
    let applied_migrations_by_version: HashMap<_, _> = applied_migrations
        .into_iter()
        .map(|migration| (migration.version, migration))
        .collect();

    let pending_migrations: Vec<_> = MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .filter(|migration| !applied_migrations_by_version.contains_key(&migration.version))
        .collect();

    let total_migrations = known_versions.len();
    let applied_count = total_migrations.saturating_sub(pending_migrations.len());

    if pending_migrations.is_empty() {
        info!(
            total_migrations,
            applied_migrations = applied_count,
            pending_migrations = 0,
            "database migrations already up to date"
        );
        return Ok(());
    }

    info!(
        total_migrations,
        applied_migrations = applied_count,
        pending_migrations = pending_migrations.len(),
        "database migrations pending"
    );

    for (index, migration) in pending_migrations.iter().enumerate() {
        let current = index + 1;
        let total = pending_migrations.len();

        info!(
            current,
            total,
            version = migration.version,
            description = %migration.description,
            "applying database migration"
        );

        let elapsed = conn.apply(migration).await?;

        info!(
            current,
            total,
            version = migration.version,
            description = %migration.description,
            elapsed_ms = elapsed.as_millis() as u64,
            "applied database migration"
        );
    }

    info!(
        total_migrations,
        applied_migrations = total_migrations,
        pending_migrations = 0,
        "database migrations complete"
    );

    Ok(())
}

async fn prepare_database_for_startup_locked(
    conn: &mut PgConnection,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    conn.ensure_migrations_table().await?;
    bootstrap_empty_database_to_baseline_v2(conn).await?;
    pending_migrations_locked(conn).await
}

async fn pending_migrations_locked(
    conn: &mut PgConnection,
) -> Result<Vec<PendingMigrationInfo>, MigrateError> {
    if !migrations_table_exists(conn).await? {
        return Ok(all_up_migrations());
    }

    if let Some(version) = conn.dirty_version().await? {
        error!(version, "database migration state is dirty");
        return Err(MigrateError::Dirty(version));
    }

    let applied_migrations = conn.list_applied_migrations().await?;
    validate_applied_migrations(&applied_migrations)?;
    Ok(pending_migrations_from_applied(&applied_migrations))
}

async fn bootstrap_empty_database_to_baseline_v2(
    conn: &mut PgConnection,
) -> Result<(), MigrateError> {
    if !should_bootstrap_baseline_v2(conn).await? {
        return Ok(());
    }

    let migrations = baseline_v2_migrations()?;
    info!(
        cutoff_version = BASELINE_V2_CUTOFF_VERSION,
        stamped_migrations = migrations.len(),
        "bootstrapping empty database from baseline_v2"
    );

    let mut tx = conn.begin().await?;
    sqlx::raw_sql(BASELINE_V2_SQL).execute(&mut *tx).await?;
    for migration in migrations {
        query(INSERT_APPLIED_MIGRATION_SQL)
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    Ok(())
}

async fn migrations_table_exists(conn: &mut PgConnection) -> Result<bool, MigrateError> {
    let exists: bool = query_scalar(MIGRATIONS_TABLE_EXISTS_SQL)
        .fetch_one(&mut *conn)
        .await?;
    Ok(exists)
}

async fn should_bootstrap_baseline_v2(conn: &mut PgConnection) -> Result<bool, MigrateError> {
    let applied_migrations = conn.list_applied_migrations().await?;
    if !applied_migrations.is_empty() {
        return Ok(false);
    }

    let public_table_count: i64 = query_scalar(PUBLIC_BASE_TABLE_COUNT_SQL)
        .fetch_one(&mut *conn)
        .await?;
    if public_table_count == 0 {
        return Ok(true);
    }

    let aether_footprint_table_count: i64 = query_scalar(AETHER_SCHEMA_FOOTPRINT_TABLE_COUNT_SQL)
        .fetch_one(&mut *conn)
        .await?;
    if aether_footprint_table_count == 0 {
        info!(
            public_table_count,
            "no Aether schema footprint detected; allowing baseline bootstrap despite pre-existing public tables"
        );
        return Ok(true);
    }

    Ok(false)
}

fn baseline_v2_migrations() -> Result<Vec<&'static sqlx::migrate::Migration>, MigrateError> {
    let migrations = MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .filter(|migration| migration.version <= BASELINE_V2_CUTOFF_VERSION)
        .collect::<Vec<_>>();

    if migrations.is_empty() {
        return Err(MigrateError::Source(Box::new(std::io::Error::other(
            "baseline_v2 cutoff does not match any embedded migrations",
        ))));
    }

    Ok(migrations)
}

fn all_up_migrations() -> Vec<PendingMigrationInfo> {
    MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| PendingMigrationInfo {
            version: migration.version,
            description: migration.description.to_string(),
        })
        .collect()
}

fn pending_migrations_from_applied(
    applied_migrations: &[sqlx::migrate::AppliedMigration],
) -> Vec<PendingMigrationInfo> {
    let applied_versions: HashSet<_> = applied_migrations
        .iter()
        .map(|migration| migration.version)
        .collect();

    MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .filter(|migration| !applied_versions.contains(&migration.version))
        .map(|migration| PendingMigrationInfo {
            version: migration.version,
            description: migration.description.to_string(),
        })
        .collect()
}

fn validate_applied_migrations(
    applied_migrations: &[sqlx::migrate::AppliedMigration],
) -> Result<(), MigrateError> {
    if MIGRATOR.ignore_missing {
        return Ok(());
    }

    let known_versions: HashSet<_> = MIGRATOR.iter().map(|migration| migration.version).collect();

    for applied_migration in applied_migrations {
        if !known_versions.contains(&applied_migration.version) {
            error!(
                version = applied_migration.version,
                "applied database migration is missing from embedded migrations"
            );
            return Err(MigrateError::VersionMissing(applied_migration.version));
        }
    }

    // Checksum drift is reported as a warning only. Strict enforcement makes
    // harmless edits (comment tweaks, whitespace, metadata fixes) impossible
    // without also touching every environment's migration history table. We
    // match by version alone — sqlx still skips already-applied migrations so
    // edited files will not re-run, they are merely allowed to exist.
    for migration in MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
    {
        if let Some(applied_migration) = applied_migrations
            .iter()
            .find(|applied_migration| applied_migration.version == migration.version)
        {
            if migration.checksum != applied_migration.checksum {
                warn!(
                    version = migration.version,
                    description = %migration.description,
                    "database migration checksum mismatch (ignored: version-only validation)"
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use sqlx::{migrate::AppliedMigration, query, query_scalar, Connection, PgConnection, PgPool};

    use super::{
        all_up_migrations, baseline_v2_migrations, pending_migrations_from_applied,
        prepare_database_for_startup, BASELINE_V2_SQL, MIGRATOR,
    };

    #[derive(Debug)]
    struct ManagedPostgresServer {
        child: Option<Child>,
        workdir: PathBuf,
        database_url: String,
    }

    impl ManagedPostgresServer {
        async fn try_start() -> Result<Option<Self>, Box<dyn std::error::Error>> {
            let initdb_bin = std::env::var("AETHER_INITDB_BIN")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "initdb".to_string());
            let postgres_bin = std::env::var("AETHER_POSTGRES_BIN")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "postgres".to_string());

            if !command_exists(&initdb_bin) || !command_exists(&postgres_bin) {
                eprintln!(
                    "skipping postgres integration test because required binaries are unavailable: initdb={}, postgres={}",
                    initdb_bin, postgres_bin
                );
                return Ok(None);
            }

            Ok(Some(Self::start(initdb_bin, postgres_bin).await?))
        }

        async fn start(
            initdb_bin: String,
            postgres_bin: String,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let port = reserve_local_port()?;
            let workdir = std::env::temp_dir().join(format!(
                "aether-migrate-tests-{}-{}",
                std::process::id(),
                port
            ));
            let data_dir = workdir.join("data");
            std::fs::create_dir_all(&workdir)?;

            let init_output = Command::new(&initdb_bin)
                .arg("-D")
                .arg(&data_dir)
                .arg("-U")
                .arg("aether")
                .arg("--auth=trust")
                .arg("--encoding=UTF8")
                .arg("--no-instructions")
                .output()?;
            if !init_output.status.success() {
                return Err(std::io::Error::other(format!(
                    "initdb failed: {}",
                    String::from_utf8_lossy(&init_output.stderr)
                ))
                .into());
            }

            let database_url = format!("postgres://aether@127.0.0.1:{port}/postgres");
            let log_path = workdir.join("postgres.log");
            let stdout = std::fs::File::create(&log_path)?;
            let stderr = stdout.try_clone()?;
            let child = Command::new(&postgres_bin)
                .arg("-D")
                .arg(&data_dir)
                .arg("-h")
                .arg("127.0.0.1")
                .arg("-p")
                .arg(port.to_string())
                .arg("-F")
                .arg("-c")
                .arg("fsync=off")
                .arg("-c")
                .arg("synchronous_commit=off")
                .arg("-c")
                .arg("full_page_writes=off")
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()?;

            wait_for_postgres(&database_url).await?;

            Ok(Self {
                child: Some(child),
                workdir,
                database_url,
            })
        }

        fn database_url(&self) -> &str {
            &self.database_url
        }

        fn stop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for ManagedPostgresServer {
        fn drop(&mut self) {
            self.stop();
            let _ = std::fs::remove_dir_all(&self.workdir);
        }
    }

    fn command_exists(bin: &str) -> bool {
        if bin.contains(std::path::MAIN_SEPARATOR) {
            return Path::new(bin).exists();
        }

        let Some(paths) = std::env::var_os("PATH") else {
            return false;
        };

        std::env::split_paths(&paths).any(|path| path.join(bin).exists())
    }

    fn reserve_local_port() -> Result<u16, std::io::Error> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        Ok(port)
    }

    async fn wait_for_postgres(database_url: &str) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match PgConnection::connect(database_url).await {
                Ok(connection) => {
                    connection.close().await?;
                    return Ok(());
                }
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(err) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("timed out waiting for local postgres: {err}"),
                    )
                    .into())
                }
            }
        }
    }

    async fn table_exists(pool: &PgPool, table_name: &str) -> Result<bool, sqlx::Error> {
        query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("public.{table_name}"))
            .fetch_one(pool)
            .await
    }

    #[test]
    fn baseline_migration_restores_search_path_for_sqlx_bookkeeping() {
        let baseline = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260403000000)
            .expect("baseline migration should be embedded");
        let first_empty_search_path = baseline
            .sql
            .find("SELECT pg_catalog.set_config('search_path', '', true);")
            .expect("baseline migration should clear search_path transaction-local");
        let restore_public_search_path = baseline
            .sql
            .rfind("SELECT pg_catalog.set_config('search_path', 'public', true);")
            .expect("baseline migration should restore search_path before sqlx bookkeeping");

        assert!(
            first_empty_search_path < restore_public_search_path,
            "baseline migration must restore search_path after clearing it",
        );
        assert!(
            !baseline
                .sql
                .contains("SELECT pg_catalog.set_config('search_path', '', false);"),
            "baseline migration must not persist an empty search_path at session scope",
        );
        assert!(
            !baseline
                .sql
                .contains("SELECT pg_catalog.set_config('search_path', 'public', false);"),
            "baseline migration must not persist a restored search_path at session scope",
        );
    }

    #[test]
    fn baseline_v2_bootstrap_covers_current_cutoff_versions() {
        let versions = baseline_v2_migrations()
            .expect("baseline_v2 migrations should resolve")
            .into_iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>();

        assert_eq!(
            versions,
            vec![
                20260403000000,
                20260406000000,
                20260410000000,
                20260413020000,
                20260413030000,
                20260415000000,
            ]
        );
    }

    #[test]
    fn baseline_v2_sql_includes_usage_body_blobs() {
        assert!(BASELINE_V2_SQL.contains("CREATE TABLE IF NOT EXISTS public.usage_body_blobs"));
        assert!(BASELINE_V2_SQL.contains("ix_usage_body_blobs_request_id"));
        assert!(BASELINE_V2_SQL.contains("CREATE TABLE IF NOT EXISTS public.usage_http_audits"));
        assert!(
            BASELINE_V2_SQL.contains("CREATE TABLE IF NOT EXISTS public.usage_routing_snapshots")
        );
        assert!(BASELINE_V2_SQL
            .contains("CREATE TABLE IF NOT EXISTS public.usage_settlement_snapshots"));
        assert!(BASELINE_V2_SQL.contains("billing_snapshot_schema_version"));
        assert!(BASELINE_V2_SQL.contains("price_per_request"));
        assert!(BASELINE_V2_SQL.contains("candidate_index integer"));
    }

    #[test]
    fn deprecation_migration_and_baseline_mark_legacy_usage_columns() {
        let settlement_migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260413020000)
            .expect("deprecation migration should be embedded");
        let http_migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20260413030000)
            .expect("http/body deprecation migration should be embedded");

        assert!(settlement_migration
            .sql
            .contains("COMMENT ON COLUMN public.usage.output_price_per_1m"));
        assert!(settlement_migration
            .sql
            .contains("COMMENT ON COLUMN public.usage.wallet_id"));
        assert!(settlement_migration
            .sql
            .contains("COMMENT ON COLUMN public.usage.username"));
        assert!(settlement_migration
            .sql
            .contains("COMMENT ON COLUMN public.usage.api_key_name"));
        assert!(http_migration
            .sql
            .contains("COMMENT ON COLUMN public.usage.request_headers"));
        assert!(http_migration
            .sql
            .contains("COMMENT ON COLUMN public.usage.request_body"));
        assert!(http_migration
            .sql
            .contains("COMMENT ON COLUMN public.usage.billing_status"));
        assert!(http_migration
            .sql
            .contains("COMMENT ON COLUMN public.usage.finalized_at"));
        assert!(BASELINE_V2_SQL.contains("COMMENT ON COLUMN public.usage.output_price_per_1m"));
        assert!(BASELINE_V2_SQL.contains("COMMENT ON COLUMN public.usage.wallet_id"));
        assert!(BASELINE_V2_SQL.contains("COMMENT ON COLUMN public.usage.username"));
        assert!(BASELINE_V2_SQL.contains("COMMENT ON COLUMN public.usage.api_key_name"));
        assert!(BASELINE_V2_SQL.contains("COMMENT ON COLUMN public.usage.request_headers"));
        assert!(BASELINE_V2_SQL.contains("COMMENT ON COLUMN public.usage.request_body"));
        assert!(BASELINE_V2_SQL.contains("COMMENT ON COLUMN public.usage.billing_status"));
        assert!(BASELINE_V2_SQL.contains("COMMENT ON COLUMN public.usage.finalized_at"));
    }

    #[test]
    fn pending_migrations_from_applied_returns_all_versions_when_none_applied() {
        let pending = pending_migrations_from_applied(&[]);
        assert_eq!(pending, all_up_migrations());
    }

    #[test]
    fn pending_migrations_from_applied_skips_versions_already_applied() {
        let applied = vec![
            AppliedMigration {
                version: 20260403000000,
                checksum: Cow::Borrowed(&[]),
            },
            AppliedMigration {
                version: 20260406000000,
                checksum: Cow::Borrowed(&[]),
            },
        ];

        let pending_versions = pending_migrations_from_applied(&applied)
            .into_iter()
            .map(|migration| migration.version)
            .collect::<Vec<_>>();

        assert_eq!(
            pending_versions,
            vec![
                20260410000000,
                20260413020000,
                20260413030000,
                20260415000000
            ]
        );
    }

    #[test]
    fn pending_migrations_from_applied_is_empty_after_baseline_v2_stamp() {
        let applied = baseline_v2_migrations()
            .expect("baseline_v2 migrations should resolve")
            .into_iter()
            .map(|migration| AppliedMigration {
                version: migration.version,
                checksum: migration.checksum.clone(),
            })
            .collect::<Vec<_>>();

        let pending = pending_migrations_from_applied(&applied);

        assert!(
            pending.is_empty(),
            "baseline_v2-stamped empty databases should not require a manual migration before first startup"
        );
    }

    #[tokio::test]
    async fn prepare_database_for_startup_bootstraps_clean_database() {
        let Some(server) = ManagedPostgresServer::try_start()
            .await
            .expect("postgres bootstrap test should start or skip")
        else {
            return;
        };

        let pool = PgPool::connect(server.database_url())
            .await
            .expect("pool should connect");
        let pending = prepare_database_for_startup(&pool)
            .await
            .expect("clean database bootstrap should succeed");

        assert!(
            pending.is_empty(),
            "fresh databases should not report pending migrations after startup preparation"
        );
        assert!(table_exists(&pool, "users")
            .await
            .expect("users lookup should succeed"));
        assert!(table_exists(&pool, "usage")
            .await
            .expect("usage lookup should succeed"));

        let applied_count: i64 =
            query_scalar("SELECT COUNT(*)::BIGINT FROM public._sqlx_migrations")
                .fetch_one(&pool)
                .await
                .expect("migration count query should succeed");
        assert_eq!(
            applied_count,
            baseline_v2_migrations()
                .expect("baseline migrations should resolve")
                .len() as i64
        );
    }

    #[tokio::test]
    async fn prepare_database_for_startup_bootstraps_when_only_unrelated_public_tables_exist() {
        let Some(server) = ManagedPostgresServer::try_start()
            .await
            .expect("postgres bootstrap test should start or skip")
        else {
            return;
        };

        let pool = PgPool::connect(server.database_url())
            .await
            .expect("pool should connect");
        query("CREATE TABLE public.vendor_bootstrap_marker (id integer PRIMARY KEY)")
            .execute(&pool)
            .await
            .expect("fixture table should be created");

        let pending = prepare_database_for_startup(&pool)
            .await
            .expect("startup preparation should tolerate unrelated public tables");

        assert!(
            pending.is_empty(),
            "unrelated public tables should not block baseline bootstrap on first startup"
        );
        assert!(table_exists(&pool, "vendor_bootstrap_marker")
            .await
            .expect("fixture table lookup should succeed"));
        assert!(table_exists(&pool, "oauth_providers")
            .await
            .expect("oauth_providers lookup should succeed"));
    }
}
