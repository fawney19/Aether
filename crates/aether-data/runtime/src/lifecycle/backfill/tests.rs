use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use sqlx::{query, query_scalar, Connection, PgConnection, PgPool};

use super::{
    pending_backfills, pending_backfills_from_applied, pending_mysql_backfills,
    pending_sqlite_backfills, run_backfills, run_mysql_backfills, run_sqlite_backfills,
    AppliedBackfill,
};
use crate::lifecycle::migrate::prepare_database_for_startup;
use crate::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};

#[test]
fn pending_backfills_from_applied_returns_all_versions_when_none_applied() {
    let versions = pending_backfills_from_applied(&[])
        .into_iter()
        .map(|item| item.version)
        .collect::<Vec<_>>();
    assert_eq!(
        versions,
        vec![
            20260422110000,
            20260422120000,
            20260504120000,
            20260505120000,
            20260517012000,
            20260716010000
        ]
    );
}

#[test]
fn pending_backfills_from_applied_skips_versions_already_applied() {
    let versions = pending_backfills_from_applied(&[AppliedBackfill {
        version: 20260422110000,
        checksum: Vec::new(),
    }])
    .into_iter()
    .map(|item| item.version)
    .collect::<Vec<_>>();
    assert_eq!(
        versions,
        vec![
            20260422120000,
            20260504120000,
            20260505120000,
            20260517012000,
            20260716010000
        ]
    );
}

#[tokio::test]
async fn mysql_backfills_are_empty_until_driver_specific_backfills_exist() {
    let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
        "mysql://user:pass@localhost:3306/aether"
            .parse()
            .expect("mysql options should parse"),
    );
    assert_eq!(
        pending_mysql_backfills(&pool)
            .await
            .expect("mysql pending backfills should load"),
        Vec::new()
    );
    run_mysql_backfills(&pool)
        .await
        .expect("mysql backfills should no-op");
}

#[tokio::test]
async fn sqlite_backfills_are_empty_until_driver_specific_backfills_exist() {
    let config = SqlDatabaseConfig::new(
        DatabaseDriver::Sqlite,
        "sqlite::memory:",
        SqlPoolConfig::default(),
    )
    .expect("sqlite config should build");
    let pool = crate::driver::sqlite::SqlitePoolFactory::new(config)
        .expect("sqlite factory should build")
        .connect_lazy()
        .expect("sqlite pool should build");
    assert_eq!(
        pending_sqlite_backfills(&pool)
            .await
            .expect("sqlite pending backfills should load"),
        Vec::new()
    );
    run_sqlite_backfills(&pool)
        .await
        .expect("sqlite backfills should no-op");
}

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
                    "skipping postgres backfill test because required binaries are unavailable: initdb={}, postgres={}",
                    initdb_bin, postgres_bin
                );
            return Ok(None);
        }

        match Self::start(initdb_bin, postgres_bin).await {
            Ok(server) => Ok(Some(server)),
            Err(err) if postgres_local_startup_unavailable(err.to_string().as_str()) => {
                eprintln!(
                        "skipping postgres backfill test because local postgres could not start in this environment: {err}"
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
            "aether-backfill-tests-{}-{}",
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

#[tokio::test]
async fn run_backfills_rebuilds_stats_and_records_execution() {
    let Some(server) = ManagedPostgresServer::try_start()
        .await
        .expect("postgres backfill test should start or skip")
    else {
        return;
    };

    let pool = PgPool::connect(server.database_url())
        .await
        .expect("pool should connect");
    prepare_database_for_startup(&pool)
        .await
        .expect("schema should prepare");

    query(
        r#"
            INSERT INTO public.users (id, username, email_verified)
            VALUES ('user-backfill-1', 'alice', TRUE)
            "#,
    )
    .execute(&pool)
    .await
    .expect("user fixture should insert");

    query(
        r#"
            INSERT INTO public.api_keys (
                id,
                user_id,
                key_hash,
                name,
                total_requests,
                total_tokens,
                total_cost_usd
            ) VALUES (
                'api-key-backfill-1',
                'user-backfill-1',
                'hash-backfill-1',
                'Alice CLI',
                77,
                7777,
                77.77
            )
            "#,
    )
    .execute(&pool)
    .await
    .expect("api key fixture should insert");

    query(
        r#"
            INSERT INTO public.global_models (
                id,
                name,
                display_name,
                usage_count
            ) VALUES (
                'global-model-backfill-1',
                'gpt-4o-mini',
                'GPT-4o Mini',
                77
            )
            "#,
    )
    .execute(&pool)
    .await
    .expect("global model fixture should insert");

    query(
        r#"
            INSERT INTO public.usage (
                id,
                user_id,
                api_key_id,
                request_id,
                provider_name,
                model,
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens_5m,
                cache_creation_input_tokens_1h,
                cache_read_cost_usd,
                total_cost_usd,
                actual_total_cost_usd,
                input_price_per_1m,
                output_price_per_1m,
                status,
                billing_status,
                finalized_at,
                created_at,
                username,
                api_format,
                endpoint_api_format,
                response_time_ms,
                first_byte_time_ms
            ) VALUES (
                'usage-backfill-1',
                'user-backfill-1',
                'api-key-backfill-1',
                'req-backfill-1',
                'openai',
                'gpt-4o-mini',
                120,
                30,
                50,
                20,
                40,
                10,
                0.0,
                1.25,
                1.10,
                50.0,
                50.0,
                'completed',
                'settled',
                TIMESTAMPTZ '2024-05-06 07:18:09+00',
                TIMESTAMPTZ '2024-05-06 07:08:09+00',
                'alice',
                'openai',
                'openai',
                250,
                120
            )
            "#,
    )
    .execute(&pool)
    .await
    .expect("usage fixture should insert");

    let pending_before = pending_backfills(&pool)
        .await
        .expect("pending backfills should load");
    assert_eq!(pending_before.len(), 6);
    assert_eq!(pending_before[0].version, 20260422110000);
    assert_eq!(pending_before[1].version, 20260422120000);
    assert_eq!(pending_before[2].version, 20260504120000);
    assert_eq!(pending_before[3].version, 20260505120000);
    assert_eq!(pending_before[4].version, 20260517012000);
    assert_eq!(pending_before[5].version, 20260716010000);

    run_backfills(&pool)
        .await
        .expect("backfills should apply successfully");

    let pending_after = pending_backfills(&pool)
        .await
        .expect("pending backfills should reload");
    assert!(pending_after.is_empty());

    let applied_versions: Vec<i64> =
        query_scalar("SELECT version FROM public.schema_backfills ORDER BY version ASC")
            .fetch_all(&pool)
            .await
            .expect("applied backfill versions should load");
    assert_eq!(
        applied_versions,
        vec![
            20260422110000,
            20260422120000,
            20260504120000,
            20260505120000,
            20260517012000,
            20260716010000
        ]
    );

    let api_key_total_requests: i64 = query_scalar(
            "SELECT COALESCE(total_requests, 0)::BIGINT FROM public.api_keys WHERE id = 'api-key-backfill-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("api key total requests should load");
    assert_eq!(api_key_total_requests, 1);

    let global_model_usage_count: i64 = query_scalar(
            "SELECT COALESCE(usage_count, 0)::BIGINT FROM public.global_models WHERE name = 'gpt-4o-mini'",
        )
        .fetch_one(&pool)
        .await
        .expect("global model usage count should load");
    assert_eq!(global_model_usage_count, 1);

    let api_key_total_tokens: i64 = query_scalar(
            "SELECT COALESCE(total_tokens, 0)::BIGINT FROM public.api_keys WHERE id = 'api-key-backfill-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("api key total tokens should load");
    assert_eq!(api_key_total_tokens, 150);

    let api_key_total_cost: f64 = query_scalar(
            "SELECT COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) FROM public.api_keys WHERE id = 'api-key-backfill-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("api key total cost should load");
    assert_eq!(api_key_total_cost, 1.25);

    let api_key_last_used_at_unix_secs: Option<i64> = query_scalar(
            "SELECT CAST(EXTRACT(EPOCH FROM last_used_at) AS BIGINT) FROM public.api_keys WHERE id = 'api-key-backfill-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("api key last used should load");
    assert_eq!(
        api_key_last_used_at_unix_secs,
        Some(
            chrono::DateTime::parse_from_rfc3339("2024-05-06T07:08:09Z")
                .expect("expected last used timestamp should parse")
                .timestamp(),
        )
    );

    let expected_finalized_unix_secs = chrono::DateTime::parse_from_rfc3339("2024-05-06T07:18:09Z")
        .expect("expected finalized timestamp should parse")
        .timestamp();

    let hourly_requests: i64 =
        query_scalar("SELECT COALESCE(SUM(total_requests), 0)::BIGINT FROM public.stats_hourly")
            .fetch_one(&pool)
            .await
            .expect("hourly stats should load");
    assert_eq!(hourly_requests, 1);

    let daily_requests: i64 =
        query_scalar("SELECT COALESCE(SUM(total_requests), 0)::BIGINT FROM public.stats_daily")
            .fetch_one(&pool)
            .await
            .expect("daily stats should load");
    assert_eq!(daily_requests, 1);

    let daily_ephemeral_5m_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_creation_ephemeral_5m_tokens), 0)::BIGINT FROM public.stats_daily",
        )
        .fetch_one(&pool)
        .await
        .expect("daily 5m cache tokens should load");
    assert_eq!(daily_ephemeral_5m_tokens, 40);

    let daily_ephemeral_1h_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_creation_ephemeral_1h_tokens), 0)::BIGINT FROM public.stats_daily",
        )
        .fetch_one(&pool)
        .await
        .expect("daily 1h cache tokens should load");
    assert_eq!(daily_ephemeral_1h_tokens, 10);

    let daily_cache_hit_total_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(cache_hit_total_requests), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily cache-hit total requests should load");
    assert_eq!(daily_cache_hit_total_requests, 1);

    let daily_cache_hit_requests: i64 =
        query_scalar("SELECT COALESCE(SUM(cache_hit_requests), 0)::BIGINT FROM public.stats_daily")
            .fetch_one(&pool)
            .await
            .expect("daily cache-hit requests should load");
    assert_eq!(daily_cache_hit_requests, 1);

    let hourly_cache_hit_total_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(cache_hit_total_requests), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly cache-hit total requests should load");
    assert_eq!(hourly_cache_hit_total_requests, 1);

    let hourly_cache_hit_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(cache_hit_requests), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly cache-hit requests should load");
    assert_eq!(hourly_cache_hit_requests, 1);

    let daily_completed_total_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_total_requests), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily completed total requests should load");
    assert_eq!(daily_completed_total_requests, 1);

    let daily_completed_cache_hit_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_cache_hit_requests), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily completed cache-hit requests should load");
    assert_eq!(daily_completed_cache_hit_requests, 1);

    let daily_completed_input_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_input_tokens), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily completed input tokens should load");
    assert_eq!(daily_completed_input_tokens, 120);

    let daily_completed_cache_creation_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_cache_creation_tokens), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily completed cache creation tokens should load");
    assert_eq!(daily_completed_cache_creation_tokens, 50);

    let daily_completed_cache_read_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_cache_read_tokens), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily completed cache read tokens should load");
    assert_eq!(daily_completed_cache_read_tokens, 20);

    let daily_completed_total_input_context: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_total_input_context), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily completed total input context should load");
    assert_eq!(daily_completed_total_input_context, 120);

    let daily_completed_cache_creation_cost: f64 = query_scalar(
            "SELECT COALESCE(SUM(completed_cache_creation_cost), 0)::DOUBLE PRECISION FROM public.stats_daily",
        )
        .fetch_one(&pool)
        .await
        .expect("daily completed cache creation cost should load");
    assert_eq!(daily_completed_cache_creation_cost, 0.0);

    let daily_completed_cache_read_cost: f64 = query_scalar(
            "SELECT COALESCE(SUM(completed_cache_read_cost), 0)::DOUBLE PRECISION FROM public.stats_daily",
        )
        .fetch_one(&pool)
        .await
        .expect("daily completed cache read cost should load");
    assert_eq!(daily_completed_cache_read_cost, 0.0);

    let hourly_completed_total_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_total_requests), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly completed total requests should load");
    assert_eq!(hourly_completed_total_requests, 1);

    let hourly_completed_cache_hit_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_cache_hit_requests), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly completed cache-hit requests should load");
    assert_eq!(hourly_completed_cache_hit_requests, 1);

    let hourly_completed_input_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_input_tokens), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly completed input tokens should load");
    assert_eq!(hourly_completed_input_tokens, 120);

    let hourly_completed_cache_creation_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_cache_creation_tokens), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly completed cache creation tokens should load");
    assert_eq!(hourly_completed_cache_creation_tokens, 50);

    let hourly_completed_cache_read_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_cache_read_tokens), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly completed cache read tokens should load");
    assert_eq!(hourly_completed_cache_read_tokens, 20);

    let hourly_completed_total_input_context: i64 = query_scalar(
        "SELECT COALESCE(SUM(completed_total_input_context), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly completed total input context should load");
    assert_eq!(hourly_completed_total_input_context, 120);

    let hourly_completed_cache_creation_cost: f64 = query_scalar(
            "SELECT COALESCE(SUM(completed_cache_creation_cost), 0)::DOUBLE PRECISION FROM public.stats_hourly",
        )
        .fetch_one(&pool)
        .await
        .expect("hourly completed cache creation cost should load");
    assert_eq!(hourly_completed_cache_creation_cost, 0.0);

    let hourly_completed_cache_read_cost: f64 = query_scalar(
            "SELECT COALESCE(SUM(completed_cache_read_cost), 0)::DOUBLE PRECISION FROM public.stats_hourly",
        )
        .fetch_one(&pool)
        .await
        .expect("hourly completed cache read cost should load");
    assert_eq!(hourly_completed_cache_read_cost, 0.0);

    let daily_settled_total_cost: f64 = query_scalar(
        "SELECT COALESCE(SUM(settled_total_cost), 0)::DOUBLE PRECISION FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily settled total cost should load");
    assert_eq!(daily_settled_total_cost, 1.25);

    let daily_settled_total_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_total_requests), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily settled total requests should load");
    assert_eq!(daily_settled_total_requests, 1);

    let daily_settled_input_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_input_tokens), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily settled input tokens should load");
    assert_eq!(daily_settled_input_tokens, 120);

    let daily_settled_output_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_output_tokens), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily settled output tokens should load");
    assert_eq!(daily_settled_output_tokens, 30);

    let daily_settled_cache_creation_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_cache_creation_tokens), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily settled cache creation tokens should load");
    assert_eq!(daily_settled_cache_creation_tokens, 50);

    let daily_settled_cache_read_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_cache_read_tokens), 0)::BIGINT FROM public.stats_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("daily settled cache read tokens should load");
    assert_eq!(daily_settled_cache_read_tokens, 20);

    let daily_settled_first_finalized_at: Option<i64> =
        query_scalar("SELECT MIN(settled_first_finalized_at_unix_secs) FROM public.stats_daily")
            .fetch_one(&pool)
            .await
            .expect("daily settled first finalized timestamp should load");
    assert_eq!(
        daily_settled_first_finalized_at,
        Some(expected_finalized_unix_secs)
    );

    let daily_settled_last_finalized_at: Option<i64> =
        query_scalar("SELECT MAX(settled_last_finalized_at_unix_secs) FROM public.stats_daily")
            .fetch_one(&pool)
            .await
            .expect("daily settled last finalized timestamp should load");
    assert_eq!(
        daily_settled_last_finalized_at,
        Some(expected_finalized_unix_secs)
    );

    let hourly_settled_total_cost: f64 = query_scalar(
        "SELECT COALESCE(SUM(settled_total_cost), 0)::DOUBLE PRECISION FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly settled total cost should load");
    assert_eq!(hourly_settled_total_cost, 1.25);

    let hourly_settled_total_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_total_requests), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly settled total requests should load");
    assert_eq!(hourly_settled_total_requests, 1);

    let hourly_settled_input_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_input_tokens), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly settled input tokens should load");
    assert_eq!(hourly_settled_input_tokens, 120);

    let hourly_settled_output_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_output_tokens), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly settled output tokens should load");
    assert_eq!(hourly_settled_output_tokens, 30);

    let hourly_settled_cache_creation_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_cache_creation_tokens), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly settled cache creation tokens should load");
    assert_eq!(hourly_settled_cache_creation_tokens, 50);

    let hourly_settled_cache_read_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_cache_read_tokens), 0)::BIGINT FROM public.stats_hourly",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly settled cache read tokens should load");
    assert_eq!(hourly_settled_cache_read_tokens, 20);

    let hourly_settled_first_finalized_at: Option<i64> =
        query_scalar("SELECT MIN(settled_first_finalized_at_unix_secs) FROM public.stats_hourly")
            .fetch_one(&pool)
            .await
            .expect("hourly settled first finalized timestamp should load");
    assert_eq!(
        hourly_settled_first_finalized_at,
        Some(expected_finalized_unix_secs)
    );

    let hourly_settled_last_finalized_at: Option<i64> =
        query_scalar("SELECT MAX(settled_last_finalized_at_unix_secs) FROM public.stats_hourly")
            .fetch_one(&pool)
            .await
            .expect("hourly settled last finalized timestamp should load");
    assert_eq!(
        hourly_settled_last_finalized_at,
        Some(expected_finalized_unix_secs)
    );

    let daily_model_provider_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(total_requests), 0)::BIGINT FROM public.stats_daily_model_provider",
    )
    .fetch_one(&pool)
    .await
    .expect("daily model-provider stats should load");
    assert_eq!(daily_model_provider_requests, 1);

    let user_daily_model_provider_requests: i64 = query_scalar(
            "SELECT COALESCE(SUM(total_requests), 0)::BIGINT FROM public.stats_user_daily_model_provider",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily model-provider stats should load");
    assert_eq!(user_daily_model_provider_requests, 1);

    let canonical_rollup_totals: Vec<i64> = query_scalar(
        r#"
SELECT total_tokens FROM public.stats_daily_model_provider
UNION ALL
SELECT total_tokens FROM public.stats_user_daily_model
UNION ALL
SELECT total_tokens FROM public.stats_user_daily_provider
UNION ALL
SELECT total_tokens FROM public.stats_user_daily_api_format
UNION ALL
SELECT total_tokens FROM public.stats_user_daily_model_provider
ORDER BY total_tokens
"#,
    )
    .fetch_all(&pool)
    .await
    .expect("canonical persisted rollup totals should load");
    assert_eq!(canonical_rollup_totals, vec![150, 150, 150, 150, 150]);

    let user_daily_ephemeral_5m_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_creation_ephemeral_5m_tokens), 0)::BIGINT FROM public.stats_user_daily",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily 5m cache tokens should load");
    assert_eq!(user_daily_ephemeral_5m_tokens, 40);

    let user_daily_ephemeral_1h_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_creation_ephemeral_1h_tokens), 0)::BIGINT FROM public.stats_user_daily",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily 1h cache tokens should load");
    assert_eq!(user_daily_ephemeral_1h_tokens, 10);

    let user_daily_settled_total_cost: f64 = query_scalar(
            "SELECT COALESCE(SUM(settled_total_cost), 0)::DOUBLE PRECISION FROM public.stats_user_daily",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily settled total cost should load");
    assert_eq!(user_daily_settled_total_cost, 1.25);

    let user_daily_settled_total_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_total_requests), 0)::BIGINT FROM public.stats_user_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("user daily settled total requests should load");
    assert_eq!(user_daily_settled_total_requests, 1);

    let user_daily_settled_input_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_input_tokens), 0)::BIGINT FROM public.stats_user_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("user daily settled input tokens should load");
    assert_eq!(user_daily_settled_input_tokens, 120);

    let user_daily_settled_output_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_output_tokens), 0)::BIGINT FROM public.stats_user_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("user daily settled output tokens should load");
    assert_eq!(user_daily_settled_output_tokens, 30);

    let user_daily_settled_cache_creation_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(settled_cache_creation_tokens), 0)::BIGINT FROM public.stats_user_daily",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily settled cache creation tokens should load");
    assert_eq!(user_daily_settled_cache_creation_tokens, 50);

    let user_daily_settled_cache_read_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_cache_read_tokens), 0)::BIGINT FROM public.stats_user_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("user daily settled cache read tokens should load");
    assert_eq!(user_daily_settled_cache_read_tokens, 20);

    let user_daily_settled_first_finalized_at: Option<i64> = query_scalar(
        "SELECT MIN(settled_first_finalized_at_unix_secs) FROM public.stats_user_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("user daily settled first finalized timestamp should load");
    assert_eq!(
        user_daily_settled_first_finalized_at,
        Some(expected_finalized_unix_secs)
    );

    let user_daily_settled_last_finalized_at: Option<i64> = query_scalar(
        "SELECT MAX(settled_last_finalized_at_unix_secs) FROM public.stats_user_daily",
    )
    .fetch_one(&pool)
    .await
    .expect("user daily settled last finalized timestamp should load");
    assert_eq!(
        user_daily_settled_last_finalized_at,
        Some(expected_finalized_unix_secs)
    );

    let hourly_user_settled_total_cost: f64 = query_scalar(
            "SELECT COALESCE(SUM(settled_total_cost), 0)::DOUBLE PRECISION FROM public.stats_hourly_user",
        )
        .fetch_one(&pool)
        .await
        .expect("hourly user settled total cost should load");
    assert_eq!(hourly_user_settled_total_cost, 1.25);

    let hourly_user_settled_total_requests: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_total_requests), 0)::BIGINT FROM public.stats_hourly_user",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly user settled total requests should load");
    assert_eq!(hourly_user_settled_total_requests, 1);

    let hourly_user_settled_input_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_input_tokens), 0)::BIGINT FROM public.stats_hourly_user",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly user settled input tokens should load");
    assert_eq!(hourly_user_settled_input_tokens, 120);

    let hourly_user_settled_output_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_output_tokens), 0)::BIGINT FROM public.stats_hourly_user",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly user settled output tokens should load");
    assert_eq!(hourly_user_settled_output_tokens, 30);

    let hourly_user_settled_cache_creation_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(settled_cache_creation_tokens), 0)::BIGINT FROM public.stats_hourly_user",
        )
        .fetch_one(&pool)
        .await
        .expect("hourly user settled cache creation tokens should load");
    assert_eq!(hourly_user_settled_cache_creation_tokens, 50);

    let hourly_user_settled_cache_read_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(settled_cache_read_tokens), 0)::BIGINT FROM public.stats_hourly_user",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly user settled cache read tokens should load");
    assert_eq!(hourly_user_settled_cache_read_tokens, 20);

    let hourly_user_settled_first_finalized_at: Option<i64> = query_scalar(
        "SELECT MIN(settled_first_finalized_at_unix_secs) FROM public.stats_hourly_user",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly user settled first finalized timestamp should load");
    assert_eq!(
        hourly_user_settled_first_finalized_at,
        Some(expected_finalized_unix_secs)
    );

    let hourly_user_settled_last_finalized_at: Option<i64> = query_scalar(
        "SELECT MAX(settled_last_finalized_at_unix_secs) FROM public.stats_hourly_user",
    )
    .fetch_one(&pool)
    .await
    .expect("hourly user settled last finalized timestamp should load");
    assert_eq!(
        hourly_user_settled_last_finalized_at,
        Some(expected_finalized_unix_secs)
    );

    let daily_cost_savings_cache_read_tokens: i64 = query_scalar(
        "SELECT COALESCE(SUM(cache_read_tokens), 0)::BIGINT FROM public.stats_daily_cost_savings",
    )
    .fetch_one(&pool)
    .await
    .expect("daily cost-savings cache read tokens should load");
    assert_eq!(daily_cost_savings_cache_read_tokens, 20);

    let daily_cost_savings_estimated_full_cost: f64 = query_scalar(
            "SELECT COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION FROM public.stats_daily_cost_savings",
        )
        .fetch_one(&pool)
        .await
        .expect("daily cost-savings estimated full cost should load");
    assert_eq!(daily_cost_savings_estimated_full_cost, 0.001);

    let daily_cost_savings_provider_cache_read_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)::BIGINT FROM public.stats_daily_cost_savings_provider",
        )
        .fetch_one(&pool)
        .await
        .expect("daily provider cost-savings cache read tokens should load");
    assert_eq!(daily_cost_savings_provider_cache_read_tokens, 20);

    let daily_cost_savings_model_cache_read_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)::BIGINT FROM public.stats_daily_cost_savings_model",
        )
        .fetch_one(&pool)
        .await
        .expect("daily model cost-savings cache read tokens should load");
    assert_eq!(daily_cost_savings_model_cache_read_tokens, 20);

    let daily_cost_savings_model_provider_cache_read_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)::BIGINT FROM public.stats_daily_cost_savings_model_provider",
        )
        .fetch_one(&pool)
        .await
        .expect("daily model-provider cost-savings cache read tokens should load");
    assert_eq!(daily_cost_savings_model_provider_cache_read_tokens, 20);

    let user_daily_cost_savings_cache_read_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)::BIGINT FROM public.stats_user_daily_cost_savings",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily cost-savings cache read tokens should load");
    assert_eq!(user_daily_cost_savings_cache_read_tokens, 20);

    let user_daily_cost_savings_estimated_full_cost: f64 = query_scalar(
            "SELECT COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION FROM public.stats_user_daily_cost_savings",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily cost-savings estimated full cost should load");
    assert_eq!(user_daily_cost_savings_estimated_full_cost, 0.001);

    let user_daily_cost_savings_provider_cache_read_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)::BIGINT FROM public.stats_user_daily_cost_savings_provider",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily provider cost-savings cache read tokens should load");
    assert_eq!(user_daily_cost_savings_provider_cache_read_tokens, 20);

    let user_daily_cost_savings_model_cache_read_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)::BIGINT FROM public.stats_user_daily_cost_savings_model",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily model cost-savings cache read tokens should load");
    assert_eq!(user_daily_cost_savings_model_cache_read_tokens, 20);

    let user_daily_cost_savings_model_provider_cache_read_tokens: i64 = query_scalar(
            "SELECT COALESCE(SUM(cache_read_tokens), 0)::BIGINT FROM public.stats_user_daily_cost_savings_model_provider",
        )
        .fetch_one(&pool)
        .await
        .expect("user daily model-provider cost-savings cache read tokens should load");
    assert_eq!(user_daily_cost_savings_model_provider_cache_read_tokens, 20);

    let summary_requests: i64 =
        query_scalar("SELECT all_time_requests::BIGINT FROM public.stats_summary LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("summary stats should load");
    assert_eq!(summary_requests, 1);

    let active_days: i32 = query_scalar(
        "SELECT active_days FROM public.stats_user_summary WHERE user_id = 'user-backfill-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("user summary should load");
    assert_eq!(active_days, 1);

    run_backfills(&pool)
        .await
        .expect("re-running backfills should no-op");

    let applied_count: i64 = query_scalar("SELECT COUNT(*)::BIGINT FROM public.schema_backfills")
        .fetch_one(&pool)
        .await
        .expect("backfill count should load");
    assert_eq!(applied_count, 6);
}
