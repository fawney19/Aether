use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sqlx::{
    migrate::{AppliedMigration, Migrate},
    query, query_scalar, Connection, PgConnection, PgPool, SqlitePool,
};

use aether_data_contracts::repository::{
    auth::AuthApiKeyWriteRepository,
    usage::{
        UsageCleanupExecutionMode, UsageCleanupTargets, UsageCleanupWindow,
        UsageLeaderboardGroupBy, UsageLeaderboardQuery,
    },
};

use super::{
    postgres::{all_up_migrations, pending_migrations_from_applied, POSTGRES_MIGRATOR},
    prepare_database_for_startup,
};
use crate::lifecycle::bootstrap::postgres::{
    snapshot_migrations as empty_database_snapshot_migrations, EMPTY_DATABASE_SNAPSHOT_SQL,
};

#[derive(Debug)]
struct ManagedPostgresServer {
    child: Option<Child>,
    workdir: PathBuf,
    database_url: String,
}

impl ManagedPostgresServer {
    async fn try_start() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let required = local_postgres_tests_required();
        let initdb_bin = std::env::var("AETHER_INITDB_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "initdb".to_string());
        let postgres_bin = std::env::var("AETHER_POSTGRES_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "postgres".to_string());

        if !command_exists(&initdb_bin) || !command_exists(&postgres_bin) {
            let message = format!(
                "required postgres integration test binaries are unavailable: initdb={initdb_bin}, postgres={postgres_bin}"
            );
            if required {
                return Err(std::io::Error::new(std::io::ErrorKind::NotFound, message).into());
            }
            eprintln!("skipping postgres integration test because {message}");
            return Ok(None);
        }

        match Self::start(initdb_bin, postgres_bin).await {
            Ok(server) => Ok(Some(server)),
            Err(err)
                if !required && postgres_local_startup_unavailable(err.to_string().as_str()) =>
            {
                eprintln!(
                        "skipping postgres integration test because local postgres could not start in this environment: {err}"
                    );
                Ok(None)
            }
            Err(err) => Err(err),
        }
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
        let mut child = Command::new(&postgres_bin)
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
            .arg("-c")
            .arg("shared_buffers=8MB")
            .arg("-c")
            .arg("max_connections=8")
            .arg("-c")
            .arg("dynamic_shared_memory_type=mmap")
            .arg("-c")
            .arg("autovacuum=off")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()?;

        if let Err(err) = wait_for_postgres(&database_url).await {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }

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

fn local_postgres_tests_required() -> bool {
    // CI can opt into failing when the isolated local PostgreSQL fixture is unavailable.
    std::env::var("AETHER_REQUIRE_LOCAL_POSTGRES_TESTS")
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
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

fn postgres_shared_memory_unavailable(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("shared memory")
        && (message.contains("could not create shared memory segment")
            || message.contains("shmget")
            || message.contains("no space left on device"))
}

fn postgres_local_startup_unavailable(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    postgres_shared_memory_unavailable(&message)
        || (message.contains("timed out waiting for local postgres")
            && (message.contains("connection refused")
                || message.contains("os error 61")
                || message.contains("os error 111")))
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

async fn column_exists(
    pool: &PgPool,
    table_name: &str,
    column_name: &str,
) -> Result<bool, sqlx::Error> {
    query_scalar::<_, bool>(
        r#"
SELECT EXISTS (
    SELECT 1
    FROM information_schema.columns
    WHERE table_schema = 'public'
      AND table_name = $1
      AND column_name = $2
)
"#,
    )
    .bind(table_name)
    .bind(column_name)
    .fetch_one(pool)
    .await
}

async fn foreign_key_exists(
    pool: &PgPool,
    table_name: &str,
    constraint_name: &str,
) -> Result<bool, sqlx::Error> {
    query_scalar::<_, bool>(
        r#"
SELECT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_constraint
    WHERE conname = $1
      AND conrelid = to_regclass($2)
      AND contype = 'f'
)
"#,
    )
    .bind(constraint_name)
    .bind(format!("public.{table_name}"))
    .fetch_one(pool)
    .await
}

fn historical_stats_day() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-07-17T00:00:00Z")
        .expect("stats day should parse")
        .with_timezone(&chrono::Utc)
}

fn postgres_backend(database_url: &str) -> crate::PostgresBackend {
    crate::PostgresBackend::from_config(crate::driver::postgres::PostgresPoolConfig {
        database_url: database_url.to_string(),
        min_connections: 1,
        max_connections: 4,
        acquire_timeout_ms: 1_000,
        idle_timeout_ms: 5_000,
        max_lifetime_ms: 30_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })
    .expect("postgres backend should build")
}

#[test]
fn baseline_migration_restores_search_path_for_sqlx_bookkeeping() {
    let baseline = POSTGRES_MIGRATOR
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
fn empty_database_snapshot_covers_current_cutoff_versions() {
    let versions = empty_database_snapshot_migrations(&POSTGRES_MIGRATOR)
        .expect("empty database snapshot migrations should resolve")
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
            20260418000000,
            20260421000000,
            20260422110000,
            20260422120000,
            20260423000000,
            20260424000000,
            20260428000000,
            20260502000000,
            20260505000000,
            20260505130000,
            20260507000000,
            20260507120000,
            20260508000000,
            20260509000000,
            20260509120000,
            20260510000000,
            20260510120000,
            20260511000000,
            20260511120000,
            20260511130000,
            20260512000000,
            20260512090000,
            20260512110000,
            20260515000000,
            20260516000000,
            20260518000000,
            20260519000000,
            20260519120000,
            20260519130000,
            20260520000000,
            20260520010000,
            20260522000000,
            20260524000000,
            20260527000000,
            20260528000000,
            20260528010000,
            20260528020000,
            20260711000000,
            20260715000000,
            20260715130000,
            20260715130100,
            20260716000000,
            20260718000000,
            20260718010000,
        ]
    );
}

#[test]
fn empty_database_snapshot_includes_tables_created_by_stamped_migrations() {
    let snapshot_tables = create_table_names(EMPTY_DATABASE_SNAPSHOT_SQL);
    let missing_tables = empty_database_snapshot_migrations(&POSTGRES_MIGRATOR)
        .expect("empty database snapshot migrations should resolve")
        .into_iter()
        .flat_map(|migration| create_table_names(migration.sql.as_ref()))
        .filter(|table| !snapshot_tables.contains(table.as_str()))
        .collect::<BTreeSet<_>>();

    assert!(
        missing_tables.is_empty(),
        "empty database snapshot is missing tables created by stamped migrations: {missing_tables:?}"
    );
}

#[test]
fn routing_profiles_repair_migration_creates_missing_tables() {
    let migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260528010000)
        .expect("routing profiles repair migration should be embedded");

    assert!(migration
        .sql
        .contains("CREATE TABLE IF NOT EXISTS public.routing_groups"));
    assert!(migration
        .sql
        .contains("CREATE TABLE IF NOT EXISTS public.routing_group_bindings"));
    assert!(migration
        .sql
        .contains("CREATE TABLE IF NOT EXISTS public.routing_group_versions"));
}

fn create_table_names(sql: &str) -> BTreeSet<String> {
    sql.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let table_part = trimmed
                .strip_prefix("CREATE TABLE IF NOT EXISTS public.")
                .or_else(|| trimmed.strip_prefix("CREATE TABLE IF NOT EXISTS "))?;
            let table_name = table_part
                .split(|ch: char| ch.is_ascii_whitespace() || ch == '(')
                .next()?;
            Some(table_name.trim_matches('"').to_string())
        })
        .collect()
}

#[test]
fn empty_database_snapshot_sql_includes_usage_body_blobs_and_audit_admin_role() {
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("'audit_admin'"));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("CREATE TABLE IF NOT EXISTS public.usage_body_blobs")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("ix_usage_body_blobs_request_id"));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("CREATE TABLE IF NOT EXISTS public.usage_http_audits")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("request_body_state character varying(32)"));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("provider_request_body_state character varying(32)")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("response_body_state character varying(32)"));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("client_response_body_state character varying(32)")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.usage_routing_snapshots"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.usage_settlement_snapshots"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("billing_snapshot_schema_version"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("price_per_request"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("settlement_snapshot_schema_version"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("billing_effective_input_tokens"));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("CREATE OR REPLACE VIEW public.usage_billing_facts")
    );
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("usage_settlement_snapshots.billing_total_cost_usd")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("candidate_index integer"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.stats_user_summary"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.stats_user_daily_model"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.stats_hourly_user_model"));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("CREATE TABLE IF NOT EXISTS public.schema_backfills")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_schema_backfills_applied_at"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("ALTER TABLE public.stats_hourly"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("response_time_sum_ms double precision"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("CREATE TABLE IF NOT EXISTS public.api_keys"));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("total_tokens bigint DEFAULT '0'::bigint NOT NULL")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.stats_user_daily_provider"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.stats_user_daily_api_format"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.stats_daily_cost_savings_model_provider"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains(
        "CREATE TABLE IF NOT EXISTS public.stats_user_daily_cost_savings_model_provider"
    ));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("successful_response_time_sum_ms double precision")
    );
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("cache_hit_total_requests bigint DEFAULT 0 NOT NULL")
    );
    let normalized_snapshot_sql = EMPTY_DATABASE_SNAPSHOT_SQL.replace("\r\n", "\n");
    assert!(normalized_snapshot_sql.contains(
        "ALTER TABLE public.stats_daily_model\n    ADD COLUMN IF NOT EXISTS cache_creation_ephemeral_5m_tokens bigint DEFAULT '0'::bigint NOT NULL,"
    ));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.usage_counter_deltas"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("ix_usage_counter_deltas_unprocessed"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_entitlement_usage_entitlement_date"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_provider_api_keys_provider_default_sort"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_provider_api_keys_provider_name_id"));
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_provider_api_keys_provider_active_priority_id")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("pool_member_scores_scheduler_account_rank_idx"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_video_tasks_due_poll"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_usage_created_id_desc"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_request_candidates_endpoint_status_created"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_background_task_runs_status_created"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_usage_legacy_body_ref_cleanup_created_at"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_usage_settlement_dashboard_cover"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("autovacuum_analyze_scale_factor = 0.02"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("request_count bigint DEFAULT 0"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("usage_count bigint DEFAULT 0 NOT NULL"));
}

#[test]
fn usage_identity_foreign_keys_are_decoupled_for_historical_ingestion() {
    let migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260522000000)
        .expect("usage identity foreign key decoupling migration should be embedded");

    for constraint in [
        "usage_provider_id_fkey",
        "usage_provider_endpoint_id_fkey",
        "usage_provider_api_key_id_fkey",
        "usage_api_key_id_fkey",
        "usage_user_id_fkey",
        "usage_wallet_id_fkey",
    ] {
        assert!(
            migration
                .sql
                .contains(format!("DROP CONSTRAINT IF EXISTS {constraint}").as_str()),
            "migration should drop {constraint}"
        );
        assert!(
            !EMPTY_DATABASE_SNAPSHOT_SQL.contains(format!("ADD CONSTRAINT {constraint}").as_str()),
            "fresh bootstrap snapshot should not recreate {constraint}"
        );
    }
}

#[test]
fn request_candidate_api_key_identity_is_decoupled_for_historical_ingestion() {
    let migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260718000000)
        .expect("request candidate API key identity decoupling migration should be embedded");

    assert!(
        migration
            .sql
            .contains("DROP CONSTRAINT IF EXISTS request_candidates_api_key_id_fkey"),
        "migration should drop request_candidates_api_key_id_fkey"
    );
    assert!(
        !EMPTY_DATABASE_SNAPSHOT_SQL.contains("ADD CONSTRAINT request_candidates_api_key_id_fkey"),
        "fresh bootstrap snapshot should not recreate request_candidates_api_key_id_fkey"
    );
}

#[test]
fn stats_daily_api_key_identity_is_decoupled_for_historical_reaggregation() {
    let migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260718010000)
        .expect("daily API key stats identity decoupling migration should be embedded");

    assert!(
        migration
            .sql
            .contains("DROP CONSTRAINT IF EXISTS stats_daily_api_key_api_key_id_fkey"),
        "migration should drop stats_daily_api_key_api_key_id_fkey"
    );
    assert!(
        !EMPTY_DATABASE_SNAPSHOT_SQL.contains("ADD CONSTRAINT stats_daily_api_key_api_key_id_fkey"),
        "fresh bootstrap snapshot should not recreate stats_daily_api_key_api_key_id_fkey"
    );
}

#[test]
fn empty_database_snapshot_sql_includes_payment_gateway_and_plans() {
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("payment_provider character varying(64)"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.payment_gateway_configs"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("CREATE TABLE IF NOT EXISTS public.billing_plans"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("purchase_limit_scope character varying(32) DEFAULT 'active_period'"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL
        .contains("CREATE TABLE IF NOT EXISTS public.user_plan_entitlements"));
}

#[test]
fn provider_api_keys_api_formats_remains_nullable_in_baselines() {
    let baseline_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260403000000)
        .expect("baseline migration should be embedded");

    assert!(baseline_migration.sql.contains("api_formats json,"));
    assert!(!baseline_migration
        .sql
        .contains("api_formats json DEFAULT '[]'::json NOT NULL"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("api_formats json,"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("concurrent_limit integer,"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("allow_auth_channel_mismatch_formats json,"));
    assert!(!EMPTY_DATABASE_SNAPSHOT_SQL.contains("api_formats json DEFAULT '[]'::json NOT NULL"));

    let auth_mismatch_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260502000000)
        .expect("auth mismatch migration should be embedded");
    assert!(auth_mismatch_migration
        .sql
        .contains("allow_auth_channel_mismatch_formats = rebuilt.api_formats"));
    assert!(auth_mismatch_migration
        .sql
        .contains("pak.allow_auth_channel_mismatch_formats IS NULL"));
}

#[test]
fn management_tokens_json_columns_are_normalized_to_jsonb_in_postgres_schema_paths() {
    let normalization_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260510000000)
        .expect("management token jsonb normalization migration should be embedded");
    assert!(normalization_migration
        .sql
        .contains("ALTER COLUMN allowed_ips TYPE jsonb USING allowed_ips::jsonb"));
    assert!(normalization_migration
        .sql
        .contains("ALTER COLUMN permissions TYPE jsonb USING permissions::jsonb"));
    assert!(normalization_migration
        .sql
        .contains("jsonb_array_length(allowed_ips) > 0"));

    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("allowed_ips jsonb,"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("permissions jsonb,"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("jsonb_array_length(allowed_ips)"));

    let bootstrap_schema =
        include_str!("../../../schema/bootstrap/postgres/001_types_and_tables.sql");
    assert!(bootstrap_schema.contains("allowed_ips jsonb,"));
    assert!(bootstrap_schema.contains("permissions jsonb,"));
    assert!(bootstrap_schema.contains("jsonb_array_length(allowed_ips)"));

    let driver_schema =
        include_str!("../../../schema/drivers/postgres/baseline/001_types_and_tables.sql");
    assert!(driver_schema.contains("allowed_ips jsonb,"));
    assert!(driver_schema.contains("permissions jsonb,"));
    assert!(driver_schema.contains("jsonb_array_length(allowed_ips)"));

    let generated_identity =
        include_str!("../../../schema/generated/postgres/baseline/001_identity.sql");
    assert!(generated_identity.contains("allowed_ips jsonb,"));
    assert!(generated_identity.contains("permissions jsonb,"));
}

#[test]
fn api_key_ip_rules_is_jsonb_in_postgres_schema_paths() {
    let api_key_ip_rules_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260520000000)
        .expect("api key IP rules migration should be embedded");
    assert!(api_key_ip_rules_migration
        .sql
        .contains("ADD COLUMN IF NOT EXISTS ip_rules jsonb NULL"));
    assert!(api_key_ip_rules_migration
        .sql
        .contains("ALTER COLUMN ip_rules TYPE jsonb USING ip_rules::jsonb"));

    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("ip_rules jsonb,"));

    let bootstrap_schema =
        include_str!("../../../schema/bootstrap/postgres/001_types_and_tables.sql");
    assert!(bootstrap_schema.contains("ip_rules jsonb,"));

    let generated_identity =
        include_str!("../../../schema/generated/postgres/baseline/001_identity.sql");
    assert!(generated_identity.contains("ip_rules jsonb,"));
}

#[test]
fn provider_api_keys_api_key_is_nullable() {
    let baseline_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260403000000)
        .expect("baseline migration should be embedded");
    let normalization_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260428000000)
        .expect("api format normalization migration should be embedded");

    assert!(baseline_migration.sql.contains("api_key text,"));
    assert!(!baseline_migration.sql.contains("api_key text NOT NULL"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("api_key text,"));
    assert!(!EMPTY_DATABASE_SNAPSHOT_SQL.contains("api_key text NOT NULL"));
    assert!(normalization_migration
        .sql
        .contains("ALTER COLUMN api_key DROP NOT NULL"));
}

#[test]
fn normalized_endpoint_formats_do_not_require_unique_provider_format_pairs() {
    let normalization_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260428000000)
        .expect("api format normalization migration should be embedded");

    assert!(normalization_migration
        .sql
        .contains("DROP CONSTRAINT IF EXISTS uq_provider_api_format"));
    assert!(normalization_migration
        .sql
        .contains("idx_provider_endpoints_provider_api_format"));
    assert!(!EMPTY_DATABASE_SNAPSHOT_SQL.contains("uq_provider_api_format"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("idx_provider_endpoints_provider_api_format"));
}

#[test]
fn split_baseline_sources_match_executable_migrations() {
    fn schema_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema")
    }

    fn compose_manifest(relative_manifest: &str) -> String {
        let root = schema_root();
        let manifest_path = root.join(relative_manifest);
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|err| panic!("failed to read {manifest_path:?}: {err}"));
        let manifest_dir = manifest_path
            .parent()
            .expect("schema manifest should have a parent directory");

        let mut output = String::new();
        for line in manifest.lines() {
            let part = line.trim();
            if part.is_empty() || part.starts_with('#') {
                continue;
            }
            let part_path = manifest_dir.join(part);
            output.push_str(
                &fs::read_to_string(&part_path)
                    .unwrap_or_else(|err| panic!("failed to read {part_path:?}: {err}")),
            );
        }
        output
    }

    assert_eq!(
        include_str!("../../../../adapters/postgres/migrations/20260403000000_baseline.sql"),
        compose_manifest("drivers/postgres/baseline/manifest.txt")
    );
    assert_eq!(
        EMPTY_DATABASE_SNAPSHOT_SQL,
        compose_manifest("bootstrap/postgres/manifest.txt")
    );
    assert_eq!(
        include_str!("../../../../adapters/mysql/migrations/20260403000000_baseline.sql"),
        compose_manifest("drivers/mysql/baseline/manifest.txt")
    );
    assert_eq!(
        include_str!("../../../../adapters/sqlite/migrations/20260403000000_baseline.sql"),
        compose_manifest("drivers/sqlite/baseline/manifest.txt")
    );
}

#[test]
fn mysql_and_sqlite_migrations_do_not_use_postgres_jsonb() {
    let mysql_sources = super::mysql::MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.sql.as_ref());
    let sqlite_sources = super::sqlite::MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.sql.as_ref());

    for source in mysql_sources.chain(sqlite_sources) {
        assert!(
            !source.to_ascii_lowercase().contains("jsonb"),
            "Postgres jsonb must stay out of MySQL/SQLite migrations"
        );
    }
}

#[test]
fn mysql_and_sqlite_migrations_include_enabled_incrementals() {
    let mysql_versions = super::mysql::MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.version)
        .collect::<Vec<_>>();
    let sqlite_versions = super::sqlite::MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.version)
        .collect::<Vec<_>>();

    assert_eq!(
        mysql_versions,
        vec![
            20260403000000,
            20260507120000,
            20260508000000,
            20260509000000,
            20260509120000,
            20260510120000,
            20260511120000,
            20260511130000,
            20260512000000,
            20260512090000,
            20260512110000,
            20260516000000,
            20260518000000,
            20260519000000,
            20260519120000,
            20260519130000,
            20260520000000,
            20260520010000,
            20260524000000,
            20260527000000,
            20260528000000,
            20260528020000,
        ]
    );
    assert_eq!(
        sqlite_versions,
        vec![
            20260403000000,
            20260507120000,
            20260508000000,
            20260509000000,
            20260509120000,
            20260510120000,
            20260511120000,
            20260511130000,
            20260512000000,
            20260512090000,
            20260512110000,
            20260516000000,
            20260518000000,
            20260519000000,
            20260519120000,
            20260519130000,
            20260520000000,
            20260520010000,
            20260524000000,
            20260527000000,
            20260528000000,
            20260528020000,
        ]
    );
}

#[tokio::test]
async fn endpoint_api_root_migration_moves_v1_from_stored_default_paths() {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    query(
        r#"
CREATE TABLE providers (
  id TEXT PRIMARY KEY,
  provider_type TEXT NOT NULL
);
"#,
    )
    .execute(&pool)
    .await
    .expect("providers table should be created");
    query(
        r#"
CREATE TABLE provider_endpoints (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  api_format TEXT NOT NULL,
  base_url TEXT NOT NULL,
  custom_path TEXT
);
"#,
    )
    .execute(&pool)
    .await
    .expect("provider_endpoints table should be created");
    query(
        r#"
INSERT INTO providers (id, provider_type) VALUES
  ('provider-custom', 'custom'),
  ('provider-fixed-vertex', 'vertex_ai'),
  ('provider-fixed-grok', 'grok');
"#,
    )
    .execute(&pool)
    .await
    .expect("providers fixture should insert");
    query(
        r#"
INSERT INTO provider_endpoints (id, provider_id, api_format, base_url, custom_path) VALUES
  ('openai-root', 'provider-custom', 'openai:chat', 'https://api.openai.example', NULL),
  ('responses-root', 'provider-custom', 'openai:responses', 'https://responses.example.com', NULL),
  ('responses-compact-root', 'provider-custom', 'openai:responses:compact', 'https://compact.example.com', NULL),
  ('openai-path-root', 'provider-custom', 'openai:chat', 'https://proxy.example.com/api', NULL),
  ('openai-old-default-path', 'provider-custom', 'openai:chat', 'https://proxy.example.com/api?tenant=demo', '/v1/chat/completions'),
  ('openai-mismatched-custom-path', 'provider-custom', 'openai:chat', 'https://proxy.example.com/api', '/v1/responses'),
  ('openai-v1-slash-old-default', 'provider-custom', 'openai:chat', 'https://already-versioned.example.com/v1/', '/v1/chat/completions'),
  ('openai-v4-old-default-path', 'provider-custom', 'openai:chat', 'https://open.bigmodel.cn/api/coding/paas/v4', '/v1/chat/completions'),
  ('embedding-root', 'provider-custom', 'openai:embedding', 'https://embedding.example.com', NULL),
  ('embedding-v4-old-default-path', 'provider-custom', 'openai:embedding', 'https://embedding.example.com/api/v4', '/v1/embeddings'),
  ('jina-embedding-root', 'provider-custom', 'jina:embedding', 'https://api.jina.example', NULL),
  ('rerank-old-default-path', 'provider-custom', 'openai:rerank', 'https://rerank.example.com/api', '/v1/rerank'),
  ('jina-rerank-old-default-path', 'provider-custom', 'jina:rerank', 'https://api.jina.example?tenant=demo', '/v1/rerank'),
  ('image-root', 'provider-custom', 'openai:image', 'https://image.example.com', NULL),
  ('image-edit-custom-path', 'provider-custom', 'openai:image', 'https://image.example.com/api', '/v1/images/edits'),
  ('image-v4-edit-custom-path', 'provider-custom', 'openai:image', 'https://image.example.com/api/v4', '/v1/images/edits'),
  ('video-root', 'provider-custom', 'openai:video', 'https://video.example.com', NULL),
  ('video-v1beta-old-default-path', 'provider-custom', 'openai:video', 'https://video.example.com/api/v1beta', '/v1/videos'),
  ('video-versioned-root', 'provider-custom', 'openai:video', 'https://ark.example.com/api/v3', NULL),
  ('google-versioned-segment-root', 'provider-custom', 'openai:embedding', 'https://generativelanguage.googleapis.com/v1beta/openai', NULL),
  ('gemini-root', 'provider-custom', 'gemini:generate_content', 'https://generativelanguage.googleapis.com', NULL),
  ('gemini-old-default-path', 'provider-custom', 'gemini:generate_content', 'https://generativelanguage.googleapis.com?tenant=demo', '/v1beta/models/{model}:{action}'),
  ('gemini-custom-path', 'provider-custom', 'gemini:generate_content', 'https://proxy.example.com/google', '/v1beta/models/gemini-upstream:generateContent'),
  ('gemini-versioned-old-default', 'provider-custom', 'gemini:generate_content', 'https://generativelanguage.googleapis.com/v1beta', '/v1beta/models/{model}:{action}'),
  ('gemini-embedding-root', 'provider-custom', 'gemini:embedding', 'https://generativelanguage.googleapis.com', NULL),
  ('gemini-embedding-old-default', 'provider-custom', 'gemini:embedding', 'https://generativelanguage.googleapis.com', '/v1beta/models/{model}:embedContent'),
  ('gemini-video-root', 'provider-custom', 'gemini:video', 'https://generativelanguage.googleapis.com', NULL),
  ('gemini-video-versioned-old-default', 'provider-custom', 'gemini:video', 'https://generativelanguage.googleapis.com/v1beta', '/v1beta/models/{model}:predictLongRunning'),
  ('fixed-vertex-gemini-root', 'provider-fixed-vertex', 'gemini:embedding', 'https://aiplatform.googleapis.com', NULL),
  ('claude-path-root', 'provider-custom', 'claude:messages', 'https://proxy.example.com/anthropic', NULL),
  ('claude-old-default-path', 'provider-custom', 'claude:messages', 'https://proxy.example.com/anthropic', '/v1/messages'),
  ('fixed-grok-root', 'provider-fixed-grok', 'openai:chat', 'https://grok.com', NULL);
"#,
    )
    .execute(&pool)
    .await
    .expect("endpoint fixture should insert");

    let migration = super::sqlite::MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260528000000)
        .expect("endpoint API root migration should be embedded");
    sqlx::raw_sql(migration.sql.as_ref())
        .execute(&pool)
        .await
        .expect("endpoint API root migration should apply");

    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, base_url, custom_path FROM provider_endpoints ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("endpoint rows should load");
    let rows = rows
        .into_iter()
        .map(|(id, base_url, custom_path)| (id, (base_url, custom_path)))
        .collect::<std::collections::BTreeMap<_, _>>();

    assert_eq!(
        rows.get("openai-root"),
        Some(&("https://api.openai.example/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("responses-root"),
        Some(&("https://responses.example.com/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("responses-compact-root"),
        Some(&("https://compact.example.com/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("openai-path-root"),
        Some(&("https://proxy.example.com/api/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("openai-old-default-path"),
        Some(&(
            "https://proxy.example.com/api/v1?tenant=demo".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("openai-mismatched-custom-path"),
        Some(&(
            "https://proxy.example.com/api/v1".to_string(),
            Some("/responses".to_string())
        ))
    );
    assert_eq!(
        rows.get("openai-v1-slash-old-default"),
        Some(&(
            "https://already-versioned.example.com/v1/".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("openai-v4-old-default-path"),
        Some(&(
            "https://open.bigmodel.cn/api/coding/paas/v4".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("embedding-root"),
        Some(&("https://embedding.example.com/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("embedding-v4-old-default-path"),
        Some(&("https://embedding.example.com/api/v4".to_string(), None))
    );
    assert_eq!(
        rows.get("jina-embedding-root"),
        Some(&("https://api.jina.example/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("rerank-old-default-path"),
        Some(&("https://rerank.example.com/api/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("jina-rerank-old-default-path"),
        Some(&("https://api.jina.example/v1?tenant=demo".to_string(), None))
    );
    assert_eq!(
        rows.get("image-root"),
        Some(&("https://image.example.com/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("image-edit-custom-path"),
        Some(&(
            "https://image.example.com/api/v1".to_string(),
            Some("/images/edits".to_string())
        ))
    );
    assert_eq!(
        rows.get("image-v4-edit-custom-path"),
        Some(&(
            "https://image.example.com/api/v4".to_string(),
            Some("/images/edits".to_string())
        ))
    );
    assert_eq!(
        rows.get("video-root"),
        Some(&("https://video.example.com/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("video-v1beta-old-default-path"),
        Some(&("https://video.example.com/api/v1beta".to_string(), None))
    );
    assert_eq!(
        rows.get("video-versioned-root"),
        Some(&("https://ark.example.com/api/v3".to_string(), None))
    );
    assert_eq!(
        rows.get("google-versioned-segment-root"),
        Some(&(
            "https://generativelanguage.googleapis.com/v1beta/openai".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("gemini-root"),
        Some(&(
            "https://generativelanguage.googleapis.com/v1beta".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("gemini-old-default-path"),
        Some(&(
            "https://generativelanguage.googleapis.com/v1beta?tenant=demo".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("gemini-custom-path"),
        Some(&(
            "https://proxy.example.com/google/v1beta".to_string(),
            Some("/models/gemini-upstream:generateContent".to_string())
        ))
    );
    assert_eq!(
        rows.get("gemini-versioned-old-default"),
        Some(&(
            "https://generativelanguage.googleapis.com/v1beta".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("gemini-embedding-root"),
        Some(&(
            "https://generativelanguage.googleapis.com/v1beta".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("gemini-embedding-old-default"),
        Some(&(
            "https://generativelanguage.googleapis.com/v1beta".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("gemini-video-root"),
        Some(&(
            "https://generativelanguage.googleapis.com/v1beta".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("gemini-video-versioned-old-default"),
        Some(&(
            "https://generativelanguage.googleapis.com/v1beta".to_string(),
            None
        ))
    );
    assert_eq!(
        rows.get("fixed-vertex-gemini-root"),
        Some(&("https://aiplatform.googleapis.com".to_string(), None))
    );
    assert_eq!(
        rows.get("claude-path-root"),
        Some(&("https://proxy.example.com/anthropic/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("claude-old-default-path"),
        Some(&("https://proxy.example.com/anthropic/v1".to_string(), None))
    );
    assert_eq!(
        rows.get("fixed-grok-root"),
        Some(&("https://grok.com".to_string(), None))
    );
}

#[test]
fn fresh_usage_schema_projects_upstream_stream_mode_for_all_drivers() {
    let mysql_baseline =
        include_str!("../../../../adapters/mysql/migrations/20260403000000_baseline.sql");
    let sqlite_baseline =
        include_str!("../../../../adapters/sqlite/migrations/20260403000000_baseline.sql");

    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("upstream_is_stream boolean"));
    assert!(mysql_baseline.contains("upstream_is_stream TINYINT(1)"));
    assert!(sqlite_baseline.contains("upstream_is_stream INTEGER"));
}

#[tokio::test]
async fn api_format_normalization_migration_preserves_duplicate_endpoint_transports() {
    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres migration test should start or skip")
    else {
        return;
    };
    let pool = PgPool::connect(server.database_url())
        .await
        .expect("pool should connect");
    let normalization_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260428000000)
        .expect("api format normalization migration should be embedded");

    sqlx::raw_sql(
        r#"
CREATE TABLE public.providers (
  id text PRIMARY KEY,
  provider_type text NOT NULL
);

CREATE TABLE public.provider_endpoints (
  id text PRIMARY KEY,
  provider_id text NOT NULL,
  api_format text NOT NULL,
  api_family text,
  endpoint_kind text,
  base_url text NOT NULL,
  max_retries integer,
  is_active boolean DEFAULT true NOT NULL,
  custom_path text,
  config json,
  created_at timestamp with time zone DEFAULT now() NOT NULL,
  updated_at timestamp with time zone DEFAULT now() NOT NULL,
  proxy jsonb,
  header_rules json,
  format_acceptance_config json,
  body_rules json
);

ALTER TABLE ONLY public.provider_endpoints
  ADD CONSTRAINT uq_provider_api_format UNIQUE (provider_id, api_format);

CREATE TABLE public.provider_api_keys (
  id text PRIMARY KEY,
  provider_id text NOT NULL,
  api_key text,
  auth_type text DEFAULT 'api_key' NOT NULL,
  auth_type_by_format json,
  api_formats json,
  updated_at timestamp with time zone DEFAULT now() NOT NULL,
  rate_multipliers json,
  global_priority_by_format json,
  health_by_format jsonb,
  circuit_breaker_by_format jsonb
);

CREATE TABLE public.api_keys (
  id text PRIMARY KEY,
  allowed_api_formats json,
  updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.users (
  id text PRIMARY KEY,
  allowed_api_formats json,
  updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.models (
  id text PRIMARY KEY,
  provider_model_mappings jsonb,
  updated_at timestamp with time zone DEFAULT now() NOT NULL
);

INSERT INTO public.providers (id, provider_type)
VALUES
  ('provider-conflict', 'custom'),
  ('provider-claude-code', 'claude_code'),
  ('provider-gemini-cli', 'gemini_cli');

INSERT INTO public.provider_endpoints (
  id,
  provider_id,
  api_format,
  api_family,
  endpoint_kind,
  base_url,
  custom_path,
  max_retries,
  header_rules,
  body_rules,
  config,
  proxy,
  format_acceptance_config
) VALUES
  (
    'endpoint-claude-chat',
    'provider-conflict',
    'claude:chat',
    'claude',
    'chat',
    'https://claude-chat.example',
    '/v1/messages',
    2,
    '{"x-channel":"chat"}'::json,
    '{"mode":"chat"}'::json,
    '{"transport":"chat"}'::json,
    '{"url":"http://proxy-chat"}'::jsonb,
    '{"accept":"chat"}'::json
  ),
  (
    'endpoint-claude-cli',
    'provider-conflict',
    'claude:cli',
    'claude',
    'cli',
    'https://claude-cli.example',
    '/v1/messages',
    3,
    '{"x-channel":"cli"}'::json,
    '{"mode":"cli"}'::json,
    '{"transport":"cli"}'::json,
    '{"url":"http://proxy-cli"}'::jsonb,
    '{"accept":"cli"}'::json
  ),
  (
    'endpoint-claude-oauth-cli',
    'provider-claude-code',
    'claude:cli',
    'claude',
    'cli',
    'https://claude-oauth.example',
    '/v1/messages',
    3,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
  ),
  (
    'endpoint-gemini-oauth-cli',
    'provider-gemini-cli',
    'gemini:cli',
    'gemini',
    'cli',
    'https://gemini-oauth.example',
    '/v1beta/models',
    3,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
  );

INSERT INTO public.provider_api_keys (
  id,
  provider_id,
  auth_type,
  auth_type_by_format,
  api_formats,
  rate_multipliers,
  global_priority_by_format,
  health_by_format,
  circuit_breaker_by_format
) VALUES
  (
    'provider-key',
    'provider-conflict',
    'api_key',
    '{"claude:cli":"bearer","gemini:chat":"api-key"}'::json,
    '["claude:chat","claude:cli","openai:cli","openai:responses"]'::json,
    '{"claude:chat":1,"openai:compact":2}'::json,
    '{"gemini:cli":3}'::json,
    '{"openai:cli":{"health_score":0.9}}'::jsonb,
    '{"openai:compact":{"open":false}}'::jsonb
  ),
  (
    'provider-raw-cli-key',
    'provider-conflict',
    'api_key',
    NULL,
    '["claude:cli"]'::json,
    NULL,
    NULL,
    NULL,
    NULL
  ),
  (
    'provider-chat-key',
    'provider-conflict',
    'bearer',
    NULL,
    '["gemini:chat"]'::json,
    NULL,
    NULL,
    NULL,
    NULL
  ),
  (
    'provider-claude-oauth-key',
    'provider-claude-code',
    'api_key',
    NULL,
    '["claude:cli"]'::json,
    NULL,
    NULL,
    NULL,
    NULL
  ),
  (
    'provider-claude-oauth-null-key',
    'provider-claude-code',
    'api_key',
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
  ),
  (
    'provider-gemini-oauth-key',
    'provider-gemini-cli',
    'api_key',
    NULL,
    '["gemini:cli"]'::json,
    NULL,
    NULL,
    NULL,
    NULL
  ),
  (
    'provider-gemini-oauth-null-key',
    'provider-gemini-cli',
    'api_key',
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
  );

INSERT INTO public.api_keys (id, allowed_api_formats)
VALUES ('api-key', '["gemini:chat","gemini:cli"]'::json);

INSERT INTO public.users (id, allowed_api_formats)
VALUES ('user', '["openai:compact","openai:responses:compact"]'::json);

INSERT INTO public.models (id, provider_model_mappings)
VALUES (
  'model',
  '[{"api_formats":["claude:chat","claude:cli","gemini:chat"]}]'::jsonb
);
"#,
    )
    .execute(&pool)
    .await
    .expect("fixture schema should be created");

    sqlx::raw_sql(&normalization_migration.sql)
        .execute(&pool)
        .await
        .expect("api format normalization migration should preserve duplicate endpoints");

    let endpoint_rows = sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
        r#"
SELECT id, api_format, api_family, endpoint_kind
FROM public.provider_endpoints
WHERE provider_id = 'provider-conflict'
ORDER BY id
"#,
    )
    .fetch_all(&pool)
    .await
    .expect("endpoint rows should be readable");
    assert_eq!(
        endpoint_rows,
        vec![
            (
                "endpoint-claude-chat".to_string(),
                "claude:messages".to_string(),
                Some("claude".to_string()),
                Some("messages".to_string())
            ),
            (
                "endpoint-claude-cli".to_string(),
                "claude:messages".to_string(),
                Some("claude".to_string()),
                Some("messages".to_string())
            ),
        ]
    );

    let base_urls = sqlx::query_as::<_, (String,)>(
        r#"
SELECT base_url
FROM public.provider_endpoints
WHERE provider_id = 'provider-conflict'
ORDER BY id
"#,
    )
    .fetch_all(&pool)
    .await
    .expect("endpoint transport rows should be readable")
    .into_iter()
    .map(|(base_url,)| base_url)
    .collect::<Vec<_>>();
    assert_eq!(
        base_urls,
        vec![
            "https://claude-chat.example".to_string(),
            "https://claude-cli.example".to_string()
        ]
    );

    let provider_key_formats: serde_json::Value = query_scalar(
        "SELECT api_formats::jsonb FROM public.provider_api_keys WHERE id = 'provider-key'",
    )
    .fetch_one(&pool)
    .await
    .expect("provider key formats should be readable");
    assert_eq!(
        provider_key_formats,
        serde_json::json!(["claude:messages", "openai:responses"])
    );

    let provider_key_auth_rows = sqlx::query_as::<_, (String, String, Option<serde_json::Value>)>(
        r#"
SELECT id, auth_type, auth_type_by_format::jsonb
FROM public.provider_api_keys
WHERE id IN (
  'provider-chat-key',
  'provider-claude-oauth-key',
  'provider-claude-oauth-null-key',
  'provider-gemini-oauth-key',
  'provider-gemini-oauth-null-key',
  'provider-key',
  'provider-raw-cli-key'
)
ORDER BY id
"#,
    )
    .fetch_all(&pool)
    .await
    .expect("provider key auth rows should be readable");
    assert_eq!(
        provider_key_auth_rows,
        vec![
            ("provider-chat-key".to_string(), "api_key".to_string(), None),
            (
                "provider-claude-oauth-key".to_string(),
                "oauth".to_string(),
                None
            ),
            (
                "provider-claude-oauth-null-key".to_string(),
                "oauth".to_string(),
                None
            ),
            (
                "provider-gemini-oauth-key".to_string(),
                "oauth".to_string(),
                None
            ),
            (
                "provider-gemini-oauth-null-key".to_string(),
                "oauth".to_string(),
                None
            ),
            (
                "provider-key".to_string(),
                "api_key".to_string(),
                Some(serde_json::json!({
                    "claude:messages": "bearer",
                    "gemini:generate_content": "api_key"
                }))
            ),
            (
                "provider-raw-cli-key".to_string(),
                "api_key".to_string(),
                Some(serde_json::json!({"claude:messages": "bearer"}))
            ),
        ]
    );

    let provider_format_constraint_count: i64 = query_scalar(
        "SELECT COUNT(*)::BIGINT FROM pg_constraint WHERE conname = 'uq_provider_api_format'",
    )
    .fetch_one(&pool)
    .await
    .expect("constraint count should be readable");
    assert_eq!(provider_format_constraint_count, 0);
}

#[test]
fn deprecation_migration_and_baseline_mark_legacy_usage_columns() {
    let settlement_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == 20260413020000)
        .expect("deprecation migration should be embedded");
    let http_migration = POSTGRES_MIGRATOR
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
    assert!(
        EMPTY_DATABASE_SNAPSHOT_SQL.contains("COMMENT ON COLUMN public.usage.output_price_per_1m")
    );
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("COMMENT ON COLUMN public.usage.wallet_id"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("COMMENT ON COLUMN public.usage.username"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("COMMENT ON COLUMN public.usage.api_key_name"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("COMMENT ON COLUMN public.usage.request_headers"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("COMMENT ON COLUMN public.usage.request_body"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("COMMENT ON COLUMN public.usage.billing_status"));
    assert!(EMPTY_DATABASE_SNAPSHOT_SQL.contains("COMMENT ON COLUMN public.usage.finalized_at"));
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
            20260415000000,
            20260418000000,
            20260421000000,
            20260422110000,
            20260422120000,
            20260423000000,
            20260424000000,
            20260428000000,
            20260502000000,
            20260505000000,
            20260505130000,
            20260507000000,
            20260507120000,
            20260508000000,
            20260509000000,
            20260509120000,
            20260510000000,
            20260510120000,
            20260511000000,
            20260511120000,
            20260511130000,
            20260512000000,
            20260512090000,
            20260512110000,
            20260515000000,
            20260516000000,
            20260518000000,
            20260519000000,
            20260519120000,
            20260519130000,
            20260520000000,
            20260520010000,
            20260522000000,
            20260524000000,
            20260527000000,
            20260528000000,
            20260528010000,
            20260528020000,
            20260711000000,
            20260715000000,
            20260715130000,
            20260715130100,
            20260716000000,
            20260718000000,
            20260718010000,
        ]
    );
}

#[test]
fn pending_migrations_from_applied_is_empty_after_empty_database_snapshot_stamp() {
    let applied = empty_database_snapshot_migrations(&POSTGRES_MIGRATOR)
        .expect("empty database snapshot migrations should resolve")
        .into_iter()
        .map(|migration| AppliedMigration {
            version: migration.version,
            checksum: migration.checksum.clone(),
        })
        .collect::<Vec<_>>();

    let pending = pending_migrations_from_applied(&applied);

    assert!(
            pending.is_empty(),
            "empty database snapshot-stamped databases should not require a manual migration before first startup"
        );
}

#[tokio::test]
async fn sqlite_migrations_create_core_config_tables() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite in-memory pool should connect");

    let pending = super::prepare_sqlite_database_for_startup(&pool)
        .await
        .expect("sqlite startup preparation should inspect pending migrations");
    assert!(
        !pending.is_empty(),
        "fresh sqlite databases should report pending migrations before migration"
    );

    super::run_sqlite_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    let pending = super::prepare_sqlite_database_for_startup(&pool)
        .await
        .expect("sqlite startup preparation should inspect applied migrations");
    assert!(
        pending.is_empty(),
        "sqlite startup preparation should report no pending migrations after migration"
    );

    for table_name in [
        "users",
        "user_preferences",
        "user_sessions",
        "api_keys",
        "management_tokens",
        "billing_rules",
        "dimension_collectors",
        "providers",
        "provider_api_keys",
        "provider_endpoints",
        "models",
        "global_models",
        "system_configs",
        "auth_modules",
        "oauth_providers",
        "proxy_nodes",
        "wallets",
        "wallet_transactions",
        "wallet_daily_usage_ledgers",
        "payment_orders",
        "payment_callbacks",
        "refund_requests",
        "redeem_code_batches",
        "redeem_codes",
    ] {
        let exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table_name)
        .fetch_one(&pool)
        .await
        .expect("sqlite_master query should succeed");
        assert_eq!(exists, 1, "missing sqlite table {table_name}");
    }

    let total_adjusted_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('wallets') WHERE name = ?")
            .bind("total_adjusted")
            .fetch_one(&pool)
            .await
            .expect("sqlite wallet column query should succeed");
    assert_eq!(
        total_adjusted_exists, 1,
        "missing sqlite wallets.total_adjusted"
    );

    let upstream_is_stream_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('usage') WHERE name = ?")
            .bind("upstream_is_stream")
            .fetch_one(&pool)
            .await
            .expect("sqlite usage column query should succeed");
    assert_eq!(
        upstream_is_stream_exists, 1,
        "missing sqlite usage.upstream_is_stream"
    );
}

#[tokio::test]
async fn postgres_migrations_create_core_config_tables_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "skipping postgres migration smoke test because AETHER_TEST_POSTGRES_URL is unset"
        );
        return;
    };

    let pool = PgPool::connect(&database_url)
        .await
        .expect("postgres test pool should connect");

    super::run_migrations(&pool)
        .await
        .expect("postgres migrations should run");

    for table_name in [
        "users",
        "user_preferences",
        "user_sessions",
        "api_keys",
        "management_tokens",
        "billing_rules",
        "dimension_collectors",
        "providers",
        "provider_api_keys",
        "provider_endpoints",
        "models",
        "global_models",
        "system_configs",
        "auth_modules",
        "oauth_providers",
        "proxy_nodes",
        "usage",
        "usage_settlement_snapshots",
        "wallets",
        "wallet_transactions",
        "wallet_daily_usage_ledgers",
        "payment_orders",
        "payment_callbacks",
        "refund_requests",
        "redeem_code_batches",
        "redeem_codes",
    ] {
        let exists: i64 = query_scalar(
            r#"
SELECT COUNT(*)
FROM information_schema.tables
WHERE table_schema = 'public'
  AND table_name = $1
"#,
        )
        .bind(table_name)
        .fetch_one(&pool)
        .await
        .expect("postgres information_schema query should succeed");
        assert_eq!(exists, 1, "missing postgres table {table_name}");
    }

    let total_adjusted_exists: i64 = query_scalar(
        r#"
SELECT COUNT(*)
FROM information_schema.columns
WHERE table_schema = 'public'
  AND table_name = 'wallets'
  AND column_name = 'total_adjusted'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("postgres information_schema column query should succeed");
    assert_eq!(
        total_adjusted_exists, 1,
        "missing postgres wallets.total_adjusted"
    );
}

#[tokio::test]
async fn postgres_provider_upstream_metadata_migration_preserves_json_when_url_is_set() {
    const MIGRATION_VERSION: i64 = 20260711000000;

    let Some(database_url) = std::env::var("AETHER_TEST_POSTGRES_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!(
            "skipping postgres provider metadata migration test because AETHER_TEST_POSTGRES_URL is unset"
        );
        return;
    };

    let pool = PgPool::connect(&database_url)
        .await
        .expect("postgres test pool should connect");
    super::run_migrations(&pool)
        .await
        .expect("postgres migrations should run");

    POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == MIGRATION_VERSION)
        .expect("provider upstream metadata migration should be embedded");

    query("DELETE FROM public.providers WHERE id = 'metadata-migration-provider'")
        .execute(&pool)
        .await
        .expect("provider migration fixture should reset");
    query(
        r#"
INSERT INTO public.providers (id, name)
VALUES ('metadata-migration-provider', 'Metadata Migration Provider')
"#,
    )
    .execute(&pool)
    .await
    .expect("provider migration fixture should insert");
    query(
        r#"
INSERT INTO public.provider_api_keys (
  id,
  provider_id,
  name,
  total_tokens,
  total_cost_usd,
  upstream_metadata
)
VALUES (
  'metadata-migration-key',
  'metadata-migration-provider',
  'Metadata Migration Key',
  0,
  0,
  '{"catalog":{"model":"gpt-5.6","nested":[1,true,null]}}'::jsonb
)
"#,
    )
    .execute(&pool)
    .await
    .expect("provider key migration fixture should insert");

    query(
        r#"
ALTER TABLE public.provider_api_keys
  ALTER COLUMN upstream_metadata TYPE json
  USING upstream_metadata::json
"#,
    )
    .execute(&pool)
    .await
    .expect("provider metadata fixture should use json storage");
    query("DELETE FROM public._sqlx_migrations WHERE version = $1")
        .bind(MIGRATION_VERSION)
        .execute(&pool)
        .await
        .expect("provider metadata migration fixture should be pending");

    super::run_migrations(&pool)
        .await
        .expect("provider upstream metadata migration should run");

    let data_type: String = query_scalar(
        r#"
SELECT data_type
FROM information_schema.columns
WHERE table_schema = 'public'
  AND table_name = 'provider_api_keys'
  AND column_name = 'upstream_metadata'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("provider upstream metadata type should load");
    assert_eq!(data_type, "jsonb");

    let metadata: serde_json::Value = query_scalar(
        "SELECT upstream_metadata FROM public.provider_api_keys WHERE id = 'metadata-migration-key'",
    )
    .fetch_one(&pool)
    .await
    .expect("provider upstream metadata should load");
    assert_eq!(
        metadata,
        serde_json::json!({
            "catalog": {
                "model": "gpt-5.6",
                "nested": [1, true, null]
            }
        })
    );

    let repository =
        crate::repository::provider_catalog::SqlxProviderCatalogReadRepository::new(pool.clone());
    assert!(repository
        .upsert_key_upstream_metadata_namespace(
            "metadata-migration-key",
            "admin",
            &serde_json::json!({"source": "manual"}),
            Some(1_740_000_001),
        )
        .await
        .expect("provider upstream metadata namespace should update"));
    assert!(repository
        .update_key_model_fetch_success(
            "metadata-migration-key",
            Some(&serde_json::json!(["gpt-5.6-sol"])),
            1_740_000_002,
            &[crate::repository::provider_catalog::ProviderCatalogUpstreamMetadataNamespaceUpdate {
                namespace: "codex_models".to_string(),
                value: serde_json::json!({
                    "cards": {
                        "gpt-5.6-sol": {
                            "slug": "gpt-5.6-sol",
                            "use_responses_lite": true
                        }
                    }
                }),
            }],
            Some(1_740_000_002),
        )
        .await
        .expect("provider model fetch state should update atomically"));

    let (allowed_models, metadata, fetched_at, fetch_error): (
        serde_json::Value,
        serde_json::Value,
        i64,
        Option<String>,
    ) = sqlx::query_as(
        r#"
SELECT
  allowed_models,
  upstream_metadata,
  EXTRACT(EPOCH FROM last_models_fetch_at)::bigint,
  last_models_fetch_error
FROM public.provider_api_keys
WHERE id = 'metadata-migration-key'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("provider model fetch state should load");
    assert_eq!(allowed_models, serde_json::json!(["gpt-5.6-sol"]));
    assert_eq!(fetched_at, 1_740_000_002);
    assert_eq!(fetch_error, None);
    assert_eq!(
        metadata,
        serde_json::json!({
            "admin": {"source": "manual"},
            "catalog": {
                "model": "gpt-5.6",
                "nested": [1, true, null]
            },
            "codex_models": {
                "cards": {
                    "gpt-5.6-sol": {
                        "slug": "gpt-5.6-sol",
                        "use_responses_lite": true
                    }
                }
            }
        })
    );

    query("DELETE FROM public.providers WHERE id = 'metadata-migration-provider'")
        .execute(&pool)
        .await
        .expect("provider migration fixture should clean up");
}

#[tokio::test]
async fn mysql_migrations_create_core_config_tables_when_url_is_set() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping mysql migration smoke test because AETHER_TEST_MYSQL_URL is unset");
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql test pool should connect");

    super::run_mysql_migrations(&pool)
        .await
        .expect("mysql migrations should run");

    let pending = super::prepare_mysql_database_for_startup(&pool)
        .await
        .expect("mysql startup preparation should inspect applied migrations");
    assert!(
        pending.is_empty(),
        "mysql startup preparation should report no pending migrations after migration"
    );

    for table_name in [
        "users",
        "user_preferences",
        "user_sessions",
        "api_keys",
        "management_tokens",
        "billing_rules",
        "dimension_collectors",
        "providers",
        "provider_api_keys",
        "provider_endpoints",
        "models",
        "global_models",
        "system_configs",
        "auth_modules",
        "oauth_providers",
        "proxy_nodes",
        "usage",
        "usage_settlement_snapshots",
        "wallets",
        "wallet_transactions",
        "wallet_daily_usage_ledgers",
        "payment_orders",
        "payment_callbacks",
        "refund_requests",
        "redeem_code_batches",
        "redeem_codes",
    ] {
        let exists: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM information_schema.tables
WHERE table_schema = DATABASE()
  AND table_name = ?
"#,
        )
        .bind(table_name)
        .fetch_one(&pool)
        .await
        .expect("mysql information_schema query should succeed");
        assert_eq!(exists, 1, "missing mysql table {table_name}");
    }

    let total_adjusted_exists: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM information_schema.columns
WHERE table_schema = DATABASE()
  AND table_name = 'wallets'
  AND column_name = 'total_adjusted'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("mysql information_schema column query should succeed");
    assert_eq!(
        total_adjusted_exists, 1,
        "missing mysql wallets.total_adjusted"
    );

    let upstream_is_stream_exists: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM information_schema.columns
WHERE table_schema = DATABASE()
  AND table_name = 'usage'
  AND column_name = 'upstream_is_stream'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("mysql usage column query should succeed");
    assert_eq!(
        upstream_is_stream_exists, 1,
        "missing mysql usage.upstream_is_stream"
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
    assert!(column_exists(&pool, "api_keys", "total_tokens")
        .await
        .expect("api_keys.total_tokens lookup should succeed"));

    let audit_admin_exists: bool = query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_enum WHERE enumtypid = 'public.userrole'::regtype AND enumlabel = 'audit_admin')",
    )
    .fetch_one(&pool)
    .await
    .expect("public.userrole audit_admin lookup should succeed");
    assert!(
        audit_admin_exists,
        "fresh database snapshot should include public.userrole.audit_admin"
    );

    let applied_count: i64 = query_scalar("SELECT COUNT(*)::BIGINT FROM public._sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("migration count query should succeed");
    assert_eq!(
        applied_count,
        empty_database_snapshot_migrations(&POSTGRES_MIGRATOR)
            .expect("baseline migrations should resolve")
            .len() as i64
    );
}

#[tokio::test]
async fn postgres_request_candidates_preserve_deleted_api_key_identity() {
    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres request candidate lifecycle test should start or skip")
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
        "clean database bootstrap should not leave pending migrations: {pending:?}"
    );

    query(
        r#"
INSERT INTO public.users (id, username, email_verified)
VALUES ('request-candidate-user', 'request-candidate-user', TRUE)
"#,
    )
    .execute(&pool)
    .await
    .expect("request candidate user fixture should be inserted");
    query(
        r#"
INSERT INTO public.api_keys (id, user_id, key_hash)
VALUES ('deleted-api-key', 'request-candidate-user', 'request-candidate-key-hash')
"#,
    )
    .execute(&pool)
    .await
    .expect("request candidate API key fixture should be inserted");
    query(
        r#"
INSERT INTO public.request_candidates (
  id,
  request_id,
  api_key_id,
  candidate_index,
  status
) VALUES (
  'request-candidate-before-delete',
  'request-before-delete',
  'deleted-api-key',
  0,
  'pending'
)
"#,
    )
    .execute(&pool)
    .await
    .expect("request candidate fixture should be inserted");
    query(
        r#"
INSERT INTO public.usage (
  id,
  request_id,
  api_key_id,
  api_key_name,
  provider_name,
  model,
  created_at
) VALUES (
  'usage-before-api-key-delete',
  'usage-request-before-api-key-delete',
  'deleted-api-key',
  'Deleted API Key Snapshot',
  'historical-provider',
  'historical-model',
  '2026-07-17 07:03:43+00'
)
"#,
    )
    .execute(&pool)
    .await
    .expect("usage fixture should be inserted");

    let repository = aether_data_postgres::SqlxAuthApiKeySnapshotReadRepository::new(pool.clone());
    assert!(repository
        .delete_user_api_key("request-candidate-user", "deleted-api-key")
        .await
        .expect("API key deletion should succeed"));

    let api_key_id: Option<String> = query_scalar(
        "SELECT api_key_id FROM public.request_candidates WHERE id = 'request-candidate-before-delete'",
    )
    .fetch_one(&pool)
    .await
    .expect("request candidate API key identity should be readable");
    assert_eq!(api_key_id.as_deref(), Some("deleted-api-key"));

    let usage_api_key_id: Option<String> = query_scalar(
        "SELECT api_key_id FROM public.usage WHERE id = 'usage-before-api-key-delete'",
    )
    .fetch_one(&pool)
    .await
    .expect("usage API key identity should be readable");
    assert_eq!(usage_api_key_id.as_deref(), Some("deleted-api-key"));

    let stats_day = historical_stats_day();
    let stats_backend = postgres_backend(server.database_url());
    let stats_summary = stats_backend
        .aggregate_stats_daily(&crate::StatsDailyAggregationInput {
            target_day_utc: stats_day,
            aggregated_at: stats_day + chrono::Duration::days(1),
        })
        .await
        .expect("daily aggregation should accept a deleted API Key identity")
        .expect("daily aggregation should find the historical usage day");
    assert_eq!(stats_summary.day_start_utc, stats_day);
    assert_eq!(stats_summary.total_requests, 1);
    assert_eq!(stats_summary.api_key_rows, 1);

    let stats_api_key_id: Option<String> = query_scalar(
        "SELECT api_key_id FROM public.stats_daily_api_key WHERE api_key_id = 'deleted-api-key'",
    )
    .fetch_one(&pool)
    .await
    .expect("daily stats API key identity should be readable");
    assert_eq!(stats_api_key_id.as_deref(), Some("deleted-api-key"));
    let stats_api_key_name: Option<String> = query_scalar(
        "SELECT api_key_name FROM public.stats_daily_api_key WHERE api_key_id = 'deleted-api-key'",
    )
    .fetch_one(&pool)
    .await
    .expect("daily stats API key name snapshot should be readable");
    assert_eq!(
        stats_api_key_name.as_deref(),
        Some("Deleted API Key Snapshot")
    );

    query(
        r#"
INSERT INTO public.request_candidates (
  id,
  request_id,
  api_key_id,
  candidate_index,
  status
) VALUES (
  'request-candidate-after-delete',
  'request-after-delete',
  'deleted-api-key',
  0,
  'pending'
)
"#,
    )
    .execute(&pool)
    .await
    .expect("late request candidate should preserve a deleted API key identity");

    let late_api_key_id: Option<String> = query_scalar(
        "SELECT api_key_id FROM public.request_candidates WHERE id = 'request-candidate-after-delete'",
    )
    .fetch_one(&pool)
    .await
    .expect("late request candidate API key identity should be readable");
    assert_eq!(late_api_key_id.as_deref(), Some("deleted-api-key"));

    query(
        r#"
INSERT INTO public.api_keys (id, user_id, key_hash, is_standalone)
VALUES (
  'deleted-standalone-api-key',
  'request-candidate-user',
  'request-candidate-standalone-hash',
  TRUE
)
"#,
    )
    .execute(&pool)
    .await
    .expect("standalone API key fixture should be inserted");
    query(
        r#"
INSERT INTO public.request_candidates (
  id,
  request_id,
  api_key_id,
  candidate_index,
  status
) VALUES (
  'candidate-standalone-before-delete',
  'standalone-before-delete',
  'deleted-standalone-api-key',
  0,
  'pending'
)
"#,
    )
    .execute(&pool)
    .await
    .expect("standalone request candidate fixture should be inserted");

    assert!(repository
        .delete_standalone_api_key("deleted-standalone-api-key")
        .await
        .expect("standalone API key deletion should succeed"));

    let standalone_api_key_id: Option<String> = query_scalar(
        "SELECT api_key_id FROM public.request_candidates WHERE id = 'candidate-standalone-before-delete'",
    )
    .fetch_one(&pool)
    .await
    .expect("standalone request candidate API key identity should be readable");
    assert_eq!(
        standalone_api_key_id.as_deref(),
        Some("deleted-standalone-api-key")
    );
}

#[tokio::test]
async fn postgres_expired_api_key_cleanup_preserves_historical_identity() {
    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres expired API key cleanup test should start or skip")
    else {
        return;
    };
    let database_url = server.database_url();

    let pool = PgPool::connect(database_url)
        .await
        .expect("pool should connect");
    let pending = prepare_database_for_startup(&pool)
        .await
        .expect("clean database bootstrap should succeed");
    assert!(
        pending.is_empty(),
        "clean database bootstrap should not leave pending migrations: {pending:?}"
    );

    query(
        r#"
INSERT INTO public.users (id, username, email_verified)
VALUES ('expired-cleanup-user', 'expired-cleanup-user', TRUE)
"#,
    )
    .execute(&pool)
    .await
    .expect("expired cleanup user fixture should be inserted");
    query(
        r#"
INSERT INTO public.api_keys (
  id,
  user_id,
  key_hash,
  expires_at,
  auto_delete_on_expiry
) VALUES (
  'expired-cleanup-api-key',
  'expired-cleanup-user',
  'expired-cleanup-key-hash',
  NOW() - INTERVAL '1 day',
  TRUE
)
"#,
    )
    .execute(&pool)
    .await
    .expect("expired API key fixture should be inserted");
    query(
        r#"
INSERT INTO public.wallets (id, api_key_id, created_at, updated_at)
VALUES (
  'expired-cleanup-wallet',
  'expired-cleanup-api-key',
  NOW(),
  NOW()
)
"#,
    )
    .execute(&pool)
    .await
    .expect("expired API key wallet fixture should be inserted");
    query(
        r#"
INSERT INTO public.usage (
  id,
  request_id,
  api_key_id,
  provider_name,
  model
) VALUES (
  'expired-cleanup-usage',
  'expired-cleanup-usage-request',
  'expired-cleanup-api-key',
  'historical-provider',
  'historical-model'
)
"#,
    )
    .execute(&pool)
    .await
    .expect("expired API key usage fixture should be inserted");
    query(
        r#"
INSERT INTO public.request_candidates (
  id,
  request_id,
  api_key_id,
  candidate_index,
  status
) VALUES (
  'expired-cleanup-candidate',
  'expired-cleanup-candidate-request',
  'expired-cleanup-api-key',
  0,
  'pending'
)
"#,
    )
    .execute(&pool)
    .await
    .expect("expired API key request candidate fixture should be inserted");

    let now = chrono::Utc::now();
    let summary = postgres_backend(database_url)
        .usage_write_repository()
        .cleanup_usage(
            &UsageCleanupWindow {
                detail_cutoff: now,
                compressed_cutoff: now,
                header_cutoff: now,
                log_cutoff: now,
            },
            100,
            false,
            UsageCleanupTargets {
                detail_body: false,
                compressed_body: false,
                headers: false,
                records: false,
                expired_keys: true,
            },
            UsageCleanupExecutionMode::Policy,
        )
        .await
        .expect("expired API key cleanup should succeed");
    assert_eq!(summary.keys_cleaned, 1);

    let api_key_exists: bool = query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.api_keys WHERE id = 'expired-cleanup-api-key')",
    )
    .fetch_one(&pool)
    .await
    .expect("expired API key deletion should be observable");
    assert!(!api_key_exists);

    let wallet_status: String =
        query_scalar("SELECT status FROM public.wallets WHERE id = 'expired-cleanup-wallet'")
            .fetch_one(&pool)
            .await
            .expect("expired API key wallet should remain readable");
    assert_eq!(wallet_status, "disabled");

    let usage_api_key_id: Option<String> =
        query_scalar("SELECT api_key_id FROM public.usage WHERE id = 'expired-cleanup-usage'")
            .fetch_one(&pool)
            .await
            .expect("historical usage identity should remain readable");
    assert_eq!(usage_api_key_id.as_deref(), Some("expired-cleanup-api-key"));

    let candidate_api_key_id: Option<String> = query_scalar(
        "SELECT api_key_id FROM public.request_candidates WHERE id = 'expired-cleanup-candidate'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical request candidate identity should remain readable");
    assert_eq!(
        candidate_api_key_id.as_deref(),
        Some("expired-cleanup-api-key")
    );
}

#[tokio::test]
async fn postgres_api_key_leaderboard_user_filter_preserves_aggregate_history() {
    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres API key leaderboard test should start or skip")
    else {
        return;
    };
    let database_url = server.database_url();

    let pool = PgPool::connect(database_url)
        .await
        .expect("pool should connect");
    let pending = prepare_database_for_startup(&pool)
        .await
        .expect("clean database bootstrap should succeed");
    assert!(
        pending.is_empty(),
        "clean database bootstrap should not leave pending migrations: {pending:?}"
    );

    query(
        r#"
INSERT INTO public.users (id, username, email_verified)
VALUES ('leaderboard-owner', 'leaderboard-owner', TRUE)
"#,
    )
    .execute(&pool)
    .await
    .expect("leaderboard owner fixture should be inserted");
    query(
        r#"
INSERT INTO public.api_keys (id, user_id, key_hash, name)
VALUES
  ('aggregate-current-key', 'leaderboard-owner', 'aggregate-current-key-hash', 'Current Key'),
  ('aggregate-deleted-key', 'leaderboard-owner', 'aggregate-deleted-key-hash', 'Deleted Key')
"#,
    )
    .execute(&pool)
    .await
    .expect("leaderboard API key fixtures should be inserted");

    let stats_day = historical_stats_day();
    query(
        r#"
INSERT INTO public.usage (
  id,
  request_id,
  user_id,
  api_key_id,
  api_key_name,
  provider_name,
  model,
  created_at
) VALUES (
  'deleted-key-identity-usage',
  'deleted-key-identity-request',
  'leaderboard-owner',
  'aggregate-deleted-key',
  'Deleted Key',
  'historical-provider',
  'historical-model',
  $1
)
"#,
    )
    .bind(stats_day)
    .execute(&pool)
    .await
    .expect("deleted API key identity evidence should be inserted");

    let repository = aether_data_postgres::SqlxAuthApiKeySnapshotReadRepository::new(pool.clone());
    assert!(repository
        .delete_user_api_key("leaderboard-owner", "aggregate-deleted-key")
        .await
        .expect("historical API key deletion should succeed"));

    query(
        r#"
INSERT INTO public.stats_daily_api_key (
  id,
  api_key_id,
  api_key_name,
  date,
  total_requests,
  input_tokens,
  total_cost
) VALUES
  ('aggregate-current-stats', 'aggregate-current-key', 'Current Key', $1, 3, 30, 0.3),
  ('aggregate-deleted-stats', 'aggregate-deleted-key', 'Deleted Key', $1, 7, 70, 0.7)
"#,
    )
    .bind(stats_day)
    .execute(&pool)
    .await
    .expect("API key daily aggregate fixtures should be inserted");

    let leaderboard_query = UsageLeaderboardQuery {
        created_from_unix_secs: u64::try_from(stats_day.timestamp())
            .expect("historical stats day should be nonnegative"),
        created_until_unix_secs: u64::try_from((stats_day + chrono::Duration::days(1)).timestamp())
            .expect("historical stats end should be nonnegative"),
        group_by: UsageLeaderboardGroupBy::ApiKey,
        user_id: Some("leaderboard-owner".to_string()),
        provider_name: None,
        model: None,
    };
    let usage_reader = postgres_backend(database_url).usage_read_repository();
    let summaries = usage_reader
        .summarize_usage_leaderboard(&leaderboard_query)
        .await
        .expect("user-filtered API key aggregate leaderboard should succeed");
    let by_key: std::collections::BTreeMap<_, _> = summaries
        .iter()
        .map(|item| (item.group_key.as_str(), item.request_count))
        .collect();
    assert_eq!(by_key.get("aggregate-current-key"), Some(&3));
    assert_eq!(by_key.get("aggregate-deleted-key"), Some(&7));

    query("DELETE FROM public.usage WHERE id = 'deleted-key-identity-usage'")
        .execute(&pool)
        .await
        .expect("historical identity evidence should be removable");
    let summaries = usage_reader
        .summarize_usage_leaderboard(&leaderboard_query)
        .await
        .expect("aggregate-only current API key leaderboard should succeed");
    let by_key: std::collections::BTreeMap<_, _> = summaries
        .iter()
        .map(|item| (item.group_key.as_str(), item.request_count))
        .collect();
    assert_eq!(by_key.get("aggregate-current-key"), Some(&3));
    assert_eq!(by_key.get("aggregate-deleted-key"), None);
}

#[tokio::test]
async fn postgres_request_candidate_migration_decouples_legacy_api_key_foreign_key() {
    const PREVIOUS_SNAPSHOT_CUTOFF_VERSION: i64 = 20260716000000;

    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres request candidate migration test should start or skip")
    else {
        return;
    };

    let mut conn = PgConnection::connect(server.database_url())
        .await
        .expect("postgres migration connection should open");
    conn.ensure_migrations_table()
        .await
        .expect("migration table should be created");
    for migration in POSTGRES_MIGRATOR
        .iter()
        .filter(|migration| migration.version <= PREVIOUS_SNAPSHOT_CUTOFF_VERSION)
    {
        conn.apply(migration)
            .await
            .expect("legacy postgres migration should apply");
    }
    drop(conn);

    let pool = PgPool::connect(server.database_url())
        .await
        .expect("pool should connect");
    let legacy_constraint_exists = foreign_key_exists(
        &pool,
        "request_candidates",
        "request_candidates_api_key_id_fkey",
    )
    .await
    .expect("legacy request candidate constraint should be readable");
    assert!(legacy_constraint_exists);

    super::run_migrations(&pool)
        .await
        .expect("request candidate API key decoupling migration should apply");

    let upgraded_constraint_exists = foreign_key_exists(
        &pool,
        "request_candidates",
        "request_candidates_api_key_id_fkey",
    )
    .await
    .expect("upgraded request candidate constraint should be readable");
    assert!(!upgraded_constraint_exists);

    query(
        r#"
INSERT INTO public.request_candidates (
  id,
  request_id,
  api_key_id,
  candidate_index,
  status
) VALUES (
  'legacy-late-request-candidate',
  'legacy-late-request',
  'legacy-deleted-api-key',
  0,
  'pending'
)
"#,
    )
    .execute(&pool)
    .await
    .expect("upgraded schema should accept a late historical API key identity");

    let api_key_id: Option<String> = query_scalar(
        "SELECT api_key_id FROM public.request_candidates WHERE id = 'legacy-late-request-candidate'",
    )
    .fetch_one(&pool)
    .await
    .expect("upgraded request candidate identity should be readable");
    assert_eq!(api_key_id.as_deref(), Some("legacy-deleted-api-key"));
}

#[tokio::test]
async fn postgres_stats_daily_api_key_migration_decouples_legacy_foreign_key() {
    const PREVIOUS_SNAPSHOT_CUTOFF_VERSION: i64 = 20260718000000;

    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres daily API key stats migration test should start or skip")
    else {
        return;
    };

    let mut conn = PgConnection::connect(server.database_url())
        .await
        .expect("postgres migration connection should open");
    conn.ensure_migrations_table()
        .await
        .expect("migration table should be created");
    for migration in POSTGRES_MIGRATOR
        .iter()
        .filter(|migration| migration.version <= PREVIOUS_SNAPSHOT_CUTOFF_VERSION)
    {
        conn.apply(migration)
            .await
            .expect("previous postgres migration should apply");
    }
    drop(conn);

    let pool = PgPool::connect(server.database_url())
        .await
        .expect("pool should connect");
    let legacy_constraint_exists = foreign_key_exists(
        &pool,
        "stats_daily_api_key",
        "stats_daily_api_key_api_key_id_fkey",
    )
    .await
    .expect("legacy daily API key stats constraint should be readable");
    assert!(legacy_constraint_exists);

    super::run_migrations(&pool)
        .await
        .expect("daily API key stats identity decoupling migration should apply");

    let upgraded_constraint_exists = foreign_key_exists(
        &pool,
        "stats_daily_api_key",
        "stats_daily_api_key_api_key_id_fkey",
    )
    .await
    .expect("upgraded daily API key stats constraint should be readable");
    assert!(!upgraded_constraint_exists);

    query(
        r#"
INSERT INTO public.usage (
  id,
  request_id,
  api_key_id,
  api_key_name,
  provider_name,
  model,
  created_at
) VALUES (
  'legacy-deleted-api-key-usage',
  'legacy-deleted-api-key-request',
  'legacy-deleted-api-key',
  'Deleted API Key Snapshot',
  'historical-provider',
  'historical-model',
  '2026-07-17 07:03:43+00'
)
"#,
    )
    .execute(&pool)
    .await
    .expect("upgraded schema should accept a deleted API Key usage snapshot");

    let stats_day = historical_stats_day();
    let stats_summary = postgres_backend(server.database_url())
        .aggregate_stats_daily(&crate::StatsDailyAggregationInput {
            target_day_utc: stats_day,
            aggregated_at: stats_day + chrono::Duration::days(1),
        })
        .await
        .expect("upgraded schema should reaggregate a deleted API Key identity")
        .expect("upgraded schema should find the historical usage day");
    assert_eq!(stats_summary.day_start_utc, stats_day);
    assert_eq!(stats_summary.total_requests, 1);
    assert_eq!(stats_summary.api_key_rows, 1);

    let api_key_id: Option<String> = query_scalar(
        "SELECT api_key_id FROM public.stats_daily_api_key WHERE api_key_id = 'legacy-deleted-api-key'",
    )
    .fetch_one(&pool)
    .await
    .expect("upgraded daily API key stats identity should be readable");
    assert_eq!(api_key_id.as_deref(), Some("legacy-deleted-api-key"));

    let api_key_name: Option<String> = query_scalar(
        "SELECT api_key_name FROM public.stats_daily_api_key WHERE api_key_id = 'legacy-deleted-api-key'",
    )
    .fetch_one(&pool)
    .await
    .expect("upgraded daily API key stats name snapshot should be readable");
    assert_eq!(api_key_name.as_deref(), Some("Deleted API Key Snapshot"));
}

#[tokio::test]
async fn postgres_usage_billing_facts_total_tokens_counts_cached_input_once() {
    const LEGACY_VIEW_MIGRATION_VERSION: i64 = 20260505130000;
    const FIX_MIGRATION_VERSION: i64 = 20260716000000;

    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres billing-facts migration test should start or skip")
    else {
        return;
    };

    let pool = PgPool::connect(server.database_url())
        .await
        .expect("pool should connect");
    let pending = prepare_database_for_startup(&pool)
        .await
        .expect("clean database bootstrap should succeed");
    assert!(pending.is_empty());

    let legacy_view_migration = POSTGRES_MIGRATOR
        .iter()
        .find(|migration| migration.version == LEGACY_VIEW_MIGRATION_VERSION)
        .expect("legacy billing-facts view migration should be embedded");
    assert!(POSTGRES_MIGRATOR
        .iter()
        .any(|migration| migration.version == FIX_MIGRATION_VERSION));

    query(
        r#"
INSERT INTO public."usage" (
  id,
  request_id,
  api_key_id,
  provider_name,
  model,
  api_format,
  endpoint_api_format,
  input_tokens,
  output_tokens,
  cache_creation_input_tokens,
  cache_read_input_tokens,
  total_tokens,
  status,
  billing_status
) VALUES
  (
    'billing-facts-cache-read',
    'billing-facts-cache-read',
    'billing-facts-api-key',
    'openai',
    'gpt-5',
    'openai:chat',
    'openai:chat',
    166103,
    94,
    0,
    164608,
    999999,
    'completed',
    'settled'
  ),
  (
    'billing-facts-cache-create',
    'billing-facts-cache-create',
    'billing-facts-api-key',
    'openai',
    'gpt-5',
    'openai:chat',
    'openai:chat',
    1000,
    20,
    100,
    400,
    999999,
    'completed',
    'settled'
  ),
  (
    'billing-facts-legacy',
    'billing-facts-legacy',
    'billing-facts-legacy-api-key',
    'legacy',
    'legacy-model',
    NULL,
    NULL,
    50,
    27,
    0,
    0,
    77,
    'completed',
    'settled'
  )
"#,
    )
    .execute(&pool)
    .await
    .expect("usage fixtures should be inserted");

    query(
        r#"
INSERT INTO public.usage_settlement_snapshots (
  request_id,
  billing_status,
  billing_input_tokens,
  billing_effective_input_tokens,
  billing_output_tokens,
  billing_cache_creation_tokens,
  billing_cache_read_tokens,
  billing_total_input_context
) VALUES
  (
    'billing-facts-cache-read',
    'settled',
    166103,
    1495,
    94,
    0,
    164608,
    166103
  ),
  (
    'billing-facts-cache-create',
    'settled',
    1000,
    500,
    20,
    100,
    400,
    900
  )
"#,
    )
    .execute(&pool)
    .await
    .expect("settlement fixtures should be inserted");

    sqlx::raw_sql(legacy_view_migration.sql.as_ref())
        .execute(&pool)
        .await
        .expect("legacy billing-facts view should be restored for the upgrade fixture");
    let duplicated_total: i64 = query_scalar(
        "SELECT total_tokens FROM public.usage_billing_facts WHERE request_id = 'billing-facts-cache-read'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy billing-facts total should be readable");
    assert_eq!(duplicated_total, 330805);

    query("DELETE FROM public._sqlx_migrations WHERE version = $1")
        .bind(FIX_MIGRATION_VERSION)
        .execute(&pool)
        .await
        .expect("billing-facts fix migration stamp should be reset");
    super::run_migrations(&pool)
        .await
        .expect("billing-facts fix migration should rebuild the view");

    let cache_read_facts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
SELECT
  effective_input_tokens,
  cache_creation_input_tokens,
  cache_read_input_tokens,
  total_input_context,
  total_tokens
FROM public.usage_billing_facts
WHERE request_id = 'billing-facts-cache-read'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("canonical cache-read billing facts should be readable");
    assert_eq!(cache_read_facts, (1495, 0, 164608, 166103, 166197));

    let cache_creation_facts: (i64, i64) = sqlx::query_as(
        r#"
SELECT total_input_context, total_tokens
FROM public.usage_billing_facts
WHERE request_id = 'billing-facts-cache-create'
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("canonical cache-creation billing facts should be readable");
    assert_eq!(cache_creation_facts, (1000, 1020));

    let legacy_total: i64 = query_scalar(
        "SELECT total_tokens FROM public.usage_billing_facts WHERE request_id = 'billing-facts-legacy'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy billing-facts total should be readable");
    assert_eq!(legacy_total, 77);

    let repository = aether_data_postgres::SqlxUsageReadRepository::new(pool.clone());
    let totals = repository
        .summarize_total_tokens_by_api_key_ids(&["billing-facts-api-key".to_string()])
        .await
        .expect("API-key totals should use canonical billing facts");
    assert_eq!(totals.get("billing-facts-api-key"), Some(&167217));
}

#[tokio::test]
async fn postgres_migrations_repair_invalid_concurrent_cleanup_index() {
    const MIGRATION_VERSION: i64 = 20260715000000;

    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres migration retry test should start or skip")
    else {
        return;
    };

    let pool = PgPool::connect(server.database_url())
        .await
        .expect("pool should connect");
    let pending = prepare_database_for_startup(&pool)
        .await
        .expect("clean database bootstrap should succeed");
    assert!(pending.is_empty());

    query("DROP INDEX CONCURRENTLY public.idx_usage_legacy_body_ref_cleanup_created_at")
        .execute(&pool)
        .await
        .expect("snapshot cleanup index should exist");
    query("CREATE TABLE public.concurrent_index_failure_fixture (value integer NOT NULL)")
        .execute(&pool)
        .await
        .expect("failure fixture table should be created");
    query("INSERT INTO public.concurrent_index_failure_fixture (value) VALUES (1), (1)")
        .execute(&pool)
        .await
        .expect("duplicate failure fixtures should be inserted");

    query(
        "CREATE UNIQUE INDEX CONCURRENTLY idx_usage_legacy_body_ref_cleanup_created_at ON public.concurrent_index_failure_fixture (value)",
    )
    .execute(&pool)
    .await
    .expect_err("duplicate values should leave a failed concurrent index build");

    let invalid_index_exists: bool = query_scalar(
        r#"
SELECT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_class AS index_relation
    JOIN pg_catalog.pg_namespace AS index_namespace
      ON index_namespace.oid = index_relation.relnamespace
    JOIN pg_catalog.pg_index AS index_state
      ON index_state.indexrelid = index_relation.oid
    WHERE index_namespace.nspname = 'public'
      AND index_relation.relname = 'idx_usage_legacy_body_ref_cleanup_created_at'
      AND NOT index_state.indisvalid
)
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("failed concurrent index state should be readable");
    assert!(invalid_index_exists);

    query("DELETE FROM public._sqlx_migrations WHERE version = $1")
        .bind(MIGRATION_VERSION)
        .execute(&pool)
        .await
        .expect("cleanup index migration stamp should be reset");
    super::run_migrations(&pool)
        .await
        .expect("migration retry should replace the invalid index");

    let valid_usage_index_exists: bool = query_scalar(
        r#"
SELECT EXISTS (
    SELECT 1
    FROM pg_catalog.pg_class AS index_relation
    JOIN pg_catalog.pg_namespace AS index_namespace
      ON index_namespace.oid = index_relation.relnamespace
    JOIN pg_catalog.pg_index AS index_state
      ON index_state.indexrelid = index_relation.oid
    WHERE index_namespace.nspname = 'public'
      AND index_relation.relname = 'idx_usage_legacy_body_ref_cleanup_created_at'
      AND index_state.indrelid = 'public.usage'::regclass
      AND index_state.indisvalid
)
"#,
    )
    .fetch_one(&pool)
    .await
    .expect("rebuilt cleanup index state should be readable");
    assert!(valid_usage_index_exists);
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
