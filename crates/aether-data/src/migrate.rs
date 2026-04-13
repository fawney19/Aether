use std::collections::{HashMap, HashSet};

use sqlx::{
    migrate::{Migrate, MigrateError, Migrator},
    query, query_scalar, Connection, PgConnection, PgPool,
};
use tracing::{error, info, warn};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
static BASELINE_V2_SQL: &str = include_str!("../bootstrap/20260413020000_baseline_v2.sql");
const BASELINE_V2_CUTOFF_VERSION: i64 = 20260413030000;
const MIGRATIONS_TABLE_EXISTS_SQL: &str =
    "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL";
const EMPTY_DATABASE_USER_TABLE_COUNT_SQL: &str = r#"
SELECT COUNT(*)::BIGINT
FROM information_schema.tables
WHERE table_schema = 'public'
  AND table_type = 'BASE TABLE'
  AND table_name <> '_sqlx_migrations'
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

    let user_table_count: i64 = query_scalar(EMPTY_DATABASE_USER_TABLE_COUNT_SQL)
        .fetch_one(&mut *conn)
        .await?;
    Ok(user_table_count == 0)
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

    use sqlx::migrate::AppliedMigration;

    use super::{
        all_up_migrations, baseline_v2_migrations, pending_migrations_from_applied,
        BASELINE_V2_SQL, MIGRATOR,
    };

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
            vec![20260410000000, 20260413020000, 20260413030000]
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
}
