// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aether_data::repository::usage::SqlxUsageReadRepository;
use aether_data_contracts::repository::usage::UpsertUsageRecord;
use aether_testkit::{
    init_test_runtime_for, prepare_aether_postgres_schema, ManagedPostgresServer,
};
use serde::Serialize;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
struct Config {
    requests: usize,
    concurrency: usize,
    max_connections: u32,
    flush_batch_size: usize,
    flush_interval: Duration,
    monitor_interval: Duration,
    output_path: Option<PathBuf>,
    postgres_url: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            requests: 2_000,
            concurrency: 100,
            max_connections: 64,
            flush_batch_size: 1_000,
            flush_interval: Duration::from_millis(100),
            monitor_interval: Duration::from_millis(100),
            output_path: None,
            postgres_url: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct Report {
    suite: &'static str,
    config: ReportConfig,
    duration_ms: u64,
    throughput_rps: u64,
    completed_requests: usize,
    failed_requests: usize,
    p50_ms: u64,
    p95_ms: u64,
    max_ms: u64,
    mean_ms: u64,
    flush: FlushReport,
    counters: CounterReport,
    lock_monitor: LockMonitorReport,
}

#[derive(Debug, Serialize)]
struct ReportConfig {
    requests: usize,
    concurrency: usize,
    max_connections: u32,
    flush_batch_size: usize,
    flush_interval_ms: u64,
    monitor_interval_ms: u64,
    managed_postgres: bool,
}

#[derive(Debug, Serialize, Default)]
struct FlushReport {
    calls: usize,
    rows_claimed: usize,
    api_key_targets: usize,
    provider_api_key_targets: usize,
    model_targets: usize,
    provider_monthly_targets: usize,
    proxy_node_targets: usize,
    management_token_targets: usize,
    api_key_last_used_targets: usize,
}

#[derive(Debug, Serialize)]
struct CounterReport {
    usage_rows: i64,
    outbox_pending_rows: i64,
    outbox_processed_rows: i64,
    api_key_total_requests: i64,
    api_key_total_tokens: i64,
    provider_key_request_count: i64,
    provider_key_success_count: i64,
    global_model_usage_count: i64,
}

#[derive(Debug, Serialize, Clone, Copy, Default)]
struct LockMonitorReport {
    samples: usize,
    max_lock_waiters: i64,
    max_api_key_update_waiters: i64,
    max_provider_key_update_waiters: i64,
    max_global_model_update_waiters: i64,
    max_provider_update_waiters: i64,
    max_oldest_lock_wait_ms: i64,
}

#[derive(Debug, Clone, Copy, Default)]
struct LockSample {
    lock_waiters: i64,
    api_key_update_waiters: i64,
    provider_key_update_waiters: i64,
    global_model_update_waiters: i64,
    provider_update_waiters: i64,
    oldest_lock_wait_ms: i64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("usage-counter-hotspot-baseline");
    let config = parse_args(std::env::args().skip(1).collect())?;

    let managed_postgres;
    let database_url;
    let _server;
    if let Some(url) = config.postgres_url.as_ref() {
        managed_postgres = false;
        database_url = url.clone();
        _server = None;
    } else {
        managed_postgres = true;
        let server = ManagedPostgresServer::start().await?;
        database_url = server.database_url().to_string();
        _server = Some(server);
    }

    prepare_aether_postgres_schema(&database_url).await?;
    let pool = PgPoolOptions::new()
        .min_connections(1)
        .max_connections(config.max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&database_url)
        .await?;
    seed_hotspot_rows(&pool).await?;

    let repository = SqlxUsageReadRepository::new(pool.clone());
    let stop = Arc::new(AtomicBool::new(false));
    let flush_report = Arc::new(Mutex::new(FlushReport::default()));
    let lock_report = Arc::new(Mutex::new(LockMonitorReport::default()));

    let flush_handle = spawn_flush_loop(
        repository.clone(),
        Arc::clone(&stop),
        Arc::clone(&flush_report),
        config.flush_batch_size,
        config.flush_interval,
    );
    let monitor_handle = spawn_lock_monitor(
        pool.clone(),
        Arc::clone(&stop),
        Arc::clone(&lock_report),
        config.monitor_interval,
    );

    let started_at = Instant::now();
    let load_result = run_usage_load(repository, config.requests, config.concurrency).await;
    wait_for_outbox_drain(
        &pool,
        &SqlxUsageReadRepository::new(pool.clone()),
        config.flush_batch_size,
    )
    .await?;
    let duration_ms = started_at.elapsed().as_millis() as u64;

    stop.store(true, Ordering::Release);
    flush_handle.await??;
    monitor_handle.await??;

    let mut flush = flush_report.lock().await;
    let final_flush = SqlxUsageReadRepository::new(pool.clone())
        .flush_usage_counter_deltas(config.flush_batch_size)
        .await?;
    flush.calls += 1;
    flush.rows_claimed += final_flush.rows_claimed;
    flush.api_key_targets += final_flush.api_key_targets;
    flush.provider_api_key_targets += final_flush.provider_api_key_targets;
    flush.model_targets += final_flush.model_targets;
    flush.provider_monthly_targets += final_flush.provider_monthly_targets;
    flush.proxy_node_targets += final_flush.proxy_node_targets;
    flush.management_token_targets += final_flush.management_token_targets;
    flush.api_key_last_used_targets += final_flush.api_key_last_used_targets;
    drop(flush);

    let counters = read_counters(&pool).await?;
    let latencies = load_result.latencies.lock().await.clone();
    let (p50_ms, p95_ms, max_ms, mean_ms) = summarize_latencies(latencies);
    let completed_requests = load_result.completed.load(Ordering::Acquire);
    let throughput_rps = if duration_ms == 0 {
        completed_requests as u64
    } else {
        ((completed_requests as u64) * 1_000) / duration_ms.max(1)
    };

    let report = Report {
        suite: "usage_counter_hotspot_baseline",
        config: ReportConfig {
            requests: config.requests,
            concurrency: config.concurrency,
            max_connections: config.max_connections,
            flush_batch_size: config.flush_batch_size,
            flush_interval_ms: config.flush_interval.as_millis() as u64,
            monitor_interval_ms: config.monitor_interval.as_millis() as u64,
            managed_postgres,
        },
        duration_ms,
        throughput_rps,
        completed_requests,
        failed_requests: load_result.failed.load(Ordering::Acquire),
        p50_ms,
        p95_ms,
        max_ms,
        mean_ms,
        flush: Arc::try_unwrap(flush_report)
            .unwrap_or_else(|_| panic!("flush report still referenced"))
            .into_inner(),
        counters,
        lock_monitor: Arc::try_unwrap(lock_report)
            .unwrap_or_else(|_| panic!("lock report still referenced"))
            .into_inner(),
    };

    let raw = serde_json::to_string_pretty(&report)?;
    println!("{raw}");
    if let Some(path) = config.output_path.as_ref() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{raw}\n"))?;
    }
    Ok(())
}

#[derive(Clone)]
struct LoadResult {
    completed: Arc<AtomicUsize>,
    failed: Arc<AtomicUsize>,
    latencies: Arc<Mutex<Vec<u64>>>,
}

async fn run_usage_load(
    repository: SqlxUsageReadRepository,
    requests: usize,
    concurrency: usize,
) -> LoadResult {
    let next = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let latencies = Arc::new(Mutex::new(Vec::with_capacity(requests)));
    let mut tasks = tokio::task::JoinSet::new();

    for _ in 0..concurrency {
        let repository = repository.clone();
        let next = Arc::clone(&next);
        let completed = Arc::clone(&completed);
        let failed = Arc::clone(&failed);
        let latencies = Arc::clone(&latencies);
        tasks.spawn(async move {
            loop {
                let index = next.fetch_add(1, Ordering::AcqRel);
                if index >= requests {
                    break;
                }
                let started_at = Instant::now();
                let result = repository.upsert(usage_record(index)).await;
                latencies
                    .lock()
                    .await
                    .push(started_at.elapsed().as_millis() as u64);
                completed.fetch_add(1, Ordering::AcqRel);
                if result.is_err() {
                    failed.fetch_add(1, Ordering::AcqRel);
                }
            }
        });
    }

    while let Some(result) = tasks.join_next().await {
        if result.is_err() {
            failed.fetch_add(1, Ordering::AcqRel);
        }
    }

    LoadResult {
        completed,
        failed,
        latencies,
    }
}

fn spawn_flush_loop(
    repository: SqlxUsageReadRepository,
    stop: Arc<AtomicBool>,
    report: Arc<Mutex<FlushReport>>,
    batch_size: usize,
    interval: Duration,
) -> tokio::task::JoinHandle<Result<(), aether_data::DataLayerError>> {
    tokio::spawn(async move {
        while !stop.load(Ordering::Acquire) {
            let summary = repository.flush_usage_counter_deltas(batch_size).await?;
            let mut report = report.lock().await;
            report.calls += 1;
            report.rows_claimed += summary.rows_claimed;
            report.api_key_targets += summary.api_key_targets;
            report.provider_api_key_targets += summary.provider_api_key_targets;
            report.model_targets += summary.model_targets;
            report.provider_monthly_targets += summary.provider_monthly_targets;
            report.proxy_node_targets += summary.proxy_node_targets;
            report.management_token_targets += summary.management_token_targets;
            report.api_key_last_used_targets += summary.api_key_last_used_targets;
            drop(report);
            tokio::time::sleep(interval).await;
        }
        Ok(())
    })
}

fn spawn_lock_monitor(
    pool: PgPool,
    stop: Arc<AtomicBool>,
    report: Arc<Mutex<LockMonitorReport>>,
    interval: Duration,
) -> tokio::task::JoinHandle<Result<(), sqlx::Error>> {
    tokio::spawn(async move {
        while !stop.load(Ordering::Acquire) {
            let sample = read_lock_sample(&pool).await?;
            let mut report = report.lock().await;
            report.samples += 1;
            report.max_lock_waiters = report.max_lock_waiters.max(sample.lock_waiters);
            report.max_api_key_update_waiters = report
                .max_api_key_update_waiters
                .max(sample.api_key_update_waiters);
            report.max_provider_key_update_waiters = report
                .max_provider_key_update_waiters
                .max(sample.provider_key_update_waiters);
            report.max_global_model_update_waiters = report
                .max_global_model_update_waiters
                .max(sample.global_model_update_waiters);
            report.max_provider_update_waiters = report
                .max_provider_update_waiters
                .max(sample.provider_update_waiters);
            report.max_oldest_lock_wait_ms = report
                .max_oldest_lock_wait_ms
                .max(sample.oldest_lock_wait_ms);
            drop(report);
            tokio::time::sleep(interval).await;
        }
        Ok(())
    })
}

async fn wait_for_outbox_drain(
    pool: &PgPool,
    repository: &SqlxUsageReadRepository,
    batch_size: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let summary = repository.flush_usage_counter_deltas(batch_size).await?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM usage_counter_deltas WHERE processed_at IS NULL",
        )
        .fetch_one(pool)
        .await?;
        if pending == 0 && summary.rows_claimed == 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("usage counter outbox did not drain; pending={pending}"),
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn seed_hotspot_rows(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
INSERT INTO users (id, username, email_verified)
VALUES ('user-hotspot', 'usage-hotspot', true)
ON CONFLICT (id) DO NOTHING
"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
INSERT INTO api_keys (id, user_id, key_hash, name, is_active, total_requests, total_tokens, total_cost_usd)
VALUES ('api-key-hotspot', 'user-hotspot', 'hash-hotspot', 'hotspot key', true, 0, 0, 0)
ON CONFLICT (id) DO UPDATE SET
  total_requests = 0,
  total_tokens = 0,
  total_cost_usd = 0,
  last_used_at = NULL
"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
INSERT INTO providers (id, name, provider_type, monthly_used_usd)
VALUES ('provider-hotspot', 'Hotspot Provider', 'openai', 0)
ON CONFLICT (id) DO UPDATE SET monthly_used_usd = 0
"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
INSERT INTO provider_api_keys (
  id, provider_id, name, total_tokens, total_cost_usd, request_count,
  success_count, error_count, total_response_time_ms
)
VALUES ('provider-key-hotspot', 'provider-hotspot', 'Hotspot Provider Key', 0, 0, 0, 0, 0, 0)
ON CONFLICT (id) DO UPDATE SET
  total_tokens = 0,
  total_cost_usd = 0,
  request_count = 0,
  success_count = 0,
  error_count = 0,
  total_response_time_ms = 0,
  last_used_at = NULL
"#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
INSERT INTO global_models (id, name, display_name, enabled, is_active, usage_count)
VALUES ('model-hotspot', 'gpt-5', 'gpt-5', true, true, 0)
ON CONFLICT (id) DO UPDATE SET usage_count = 0
"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

fn usage_record(index: usize) -> UpsertUsageRecord {
    let now_ms = now_unix_ms().saturating_add(index as u64);
    let now_secs = now_ms / 1_000;
    UpsertUsageRecord {
        request_id: format!("usage-hotspot-{index:08}"),
        user_id: Some("user-hotspot".to_string()),
        api_key_id: Some("api-key-hotspot".to_string()),
        username: None,
        api_key_name: None,
        provider_name: "openai".to_string(),
        model: "gpt-5".to_string(),
        target_model: None,
        provider_id: Some("provider-hotspot".to_string()),
        provider_endpoint_id: None,
        provider_api_key_id: Some("provider-key-hotspot".to_string()),
        request_type: Some("chat".to_string()),
        api_format: Some("openai:chat".to_string()),
        api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_api_format: Some("openai:chat".to_string()),
        provider_api_family: Some("openai".to_string()),
        provider_endpoint_kind: Some("chat".to_string()),
        has_format_conversion: Some(false),
        is_stream: Some(false),
        input_tokens: Some(10),
        output_tokens: Some(20),
        total_tokens: Some(30),
        cache_creation_input_tokens: None,
        cache_creation_ephemeral_5m_input_tokens: None,
        cache_creation_ephemeral_1h_input_tokens: None,
        cache_read_input_tokens: None,
        cache_creation_cost_usd: None,
        cache_read_cost_usd: None,
        output_price_per_1m: None,
        total_cost_usd: Some(0.001),
        actual_total_cost_usd: Some(0.001),
        status_code: Some(200),
        error_message: None,
        error_category: None,
        response_time_ms: Some(100),
        first_byte_time_ms: Some(20),
        status: "completed".to_string(),
        billing_status: "pending".to_string(),
        request_headers: None,
        request_body: Some(json!({"model": "gpt-5"})),
        request_body_ref: None,
        request_body_state: None,
        provider_request_headers: None,
        provider_request_body: None,
        provider_request_body_ref: None,
        provider_request_body_state: None,
        response_headers: None,
        response_body: Some(json!({"id": format!("chatcmpl-{index}")})),
        response_body_ref: None,
        response_body_state: None,
        client_response_headers: None,
        client_response_body: None,
        client_response_body_ref: None,
        client_response_body_state: None,
        candidate_id: Some(format!("candidate-{index:08}")),
        candidate_index: Some(0),
        key_name: None,
        planner_kind: Some("hotspot_baseline".to_string()),
        route_family: Some("openai".to_string()),
        route_kind: Some("chat".to_string()),
        execution_path: Some("testkit".to_string()),
        local_execution_runtime_miss_reason: None,
        request_metadata: None,
        finalized_at_unix_secs: None,
        created_at_unix_ms: Some(now_ms),
        updated_at_unix_secs: now_secs,
    }
}

async fn read_lock_sample(pool: &PgPool) -> Result<LockSample, sqlx::Error> {
    let row = sqlx::query(
        r#"
SELECT
  COUNT(*) FILTER (WHERE wait_event_type = 'Lock')::BIGINT AS lock_waiters,
  COUNT(*) FILTER (
    WHERE wait_event_type = 'Lock' AND query LIKE 'UPDATE api_keys%'
  )::BIGINT AS api_key_update_waiters,
  COUNT(*) FILTER (
    WHERE wait_event_type = 'Lock' AND query LIKE 'UPDATE provider_api_keys%'
  )::BIGINT AS provider_key_update_waiters,
  COUNT(*) FILTER (
    WHERE wait_event_type = 'Lock' AND query LIKE 'UPDATE global_models%'
  )::BIGINT AS global_model_update_waiters,
  COUNT(*) FILTER (
    WHERE wait_event_type = 'Lock' AND query LIKE 'UPDATE providers%'
  )::BIGINT AS provider_update_waiters,
  COALESCE(
    MAX(EXTRACT(EPOCH FROM (NOW() - query_start)) * 1000)
      FILTER (WHERE wait_event_type = 'Lock'),
    0
  )::BIGINT AS oldest_lock_wait_ms
FROM pg_stat_activity
WHERE datname = current_database()
"#,
    )
    .fetch_one(pool)
    .await?;
    Ok(LockSample {
        lock_waiters: row.try_get("lock_waiters")?,
        api_key_update_waiters: row.try_get("api_key_update_waiters")?,
        provider_key_update_waiters: row.try_get("provider_key_update_waiters")?,
        global_model_update_waiters: row.try_get("global_model_update_waiters")?,
        provider_update_waiters: row.try_get("provider_update_waiters")?,
        oldest_lock_wait_ms: row.try_get("oldest_lock_wait_ms")?,
    })
}

async fn read_counters(pool: &PgPool) -> Result<CounterReport, sqlx::Error> {
    let row = sqlx::query(
        r#"
SELECT
  (SELECT COUNT(*)::BIGINT FROM usage) AS usage_rows,
  (SELECT COUNT(*)::BIGINT FROM usage_counter_deltas WHERE processed_at IS NULL) AS outbox_pending_rows,
  (SELECT COUNT(*)::BIGINT FROM usage_counter_deltas WHERE processed_at IS NOT NULL) AS outbox_processed_rows,
  (SELECT COALESCE(total_requests, 0)::BIGINT FROM api_keys WHERE id = 'api-key-hotspot') AS api_key_total_requests,
  (SELECT COALESCE(total_tokens, 0)::BIGINT FROM api_keys WHERE id = 'api-key-hotspot') AS api_key_total_tokens,
  (SELECT COALESCE(request_count, 0)::BIGINT FROM provider_api_keys WHERE id = 'provider-key-hotspot') AS provider_key_request_count,
  (SELECT COALESCE(success_count, 0)::BIGINT FROM provider_api_keys WHERE id = 'provider-key-hotspot') AS provider_key_success_count,
  (SELECT COALESCE(usage_count, 0)::BIGINT FROM global_models WHERE name = 'gpt-5' LIMIT 1) AS global_model_usage_count
"#,
    )
    .fetch_one(pool)
    .await?;
    Ok(CounterReport {
        usage_rows: row.try_get("usage_rows")?,
        outbox_pending_rows: row.try_get("outbox_pending_rows")?,
        outbox_processed_rows: row.try_get("outbox_processed_rows")?,
        api_key_total_requests: row.try_get("api_key_total_requests")?,
        api_key_total_tokens: row.try_get("api_key_total_tokens")?,
        provider_key_request_count: row.try_get("provider_key_request_count")?,
        provider_key_success_count: row.try_get("provider_key_success_count")?,
        global_model_usage_count: row.try_get("global_model_usage_count")?,
    })
}

fn summarize_latencies(mut latencies: Vec<u64>) -> (u64, u64, u64, u64) {
    if latencies.is_empty() {
        return (0, 0, 0, 0);
    }
    latencies.sort_unstable();
    let p50 = percentile(&latencies, 50);
    let p95 = percentile(&latencies, 95);
    let max = *latencies.last().unwrap_or(&0);
    let mean = latencies.iter().sum::<u64>() / latencies.len() as u64;
    (p50, p95, max, mean)
}

fn percentile(latencies: &[u64], percentile: usize) -> u64 {
    let index = ((latencies.len() - 1) * percentile) / 100;
    latencies[index]
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn parse_args(args: Vec<String>) -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--requests" => config.requests = next_value(&mut iter, "--requests")?.parse()?,
            "--concurrency" => {
                config.concurrency = next_value(&mut iter, "--concurrency")?.parse()?
            }
            "--max-connections" => {
                config.max_connections = next_value(&mut iter, "--max-connections")?.parse()?
            }
            "--flush-batch-size" => {
                config.flush_batch_size = next_value(&mut iter, "--flush-batch-size")?.parse()?
            }
            "--flush-interval-ms" => {
                config.flush_interval =
                    Duration::from_millis(next_value(&mut iter, "--flush-interval-ms")?.parse()?)
            }
            "--monitor-interval-ms" => {
                config.monitor_interval =
                    Duration::from_millis(next_value(&mut iter, "--monitor-interval-ms")?.parse()?)
            }
            "--postgres-url" => {
                config.postgres_url = Some(next_value(&mut iter, "--postgres-url")?)
            }
            "--output" => {
                config.output_path = Some(PathBuf::from(next_value(&mut iter, "--output")?))
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown argument: {other}"),
                )
                .into());
            }
        }
    }
    if config.requests == 0 || config.concurrency == 0 || config.max_connections == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "requests, concurrency, and max-connections must be positive",
        )
        .into());
    }
    Ok(config)
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    iter.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("missing value for {flag}"),
        )
        .into()
    })
}

fn print_usage() {
    eprintln!(
        "usage: cargo run -p aether-integration-tests --bin usage_counter_hotspot_baseline -- [--requests 2000] [--concurrency 100] [--max-connections 64] [--flush-batch-size 1000] [--flush-interval-ms 100] [--monitor-interval-ms 100] [--postgres-url postgres://...] [--output /tmp/usage_counter_hotspot.json]"
    );
}
