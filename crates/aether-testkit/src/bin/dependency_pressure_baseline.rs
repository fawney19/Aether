use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_data::driver::postgres::{
    DatabaseRecordId, PostgresLeaseClaimOptions, PostgresLeaseClaimSpec, PostgresLeaseRunnerConfig,
    PostgresPoolConfig,
};
use aether_data::PostgresBackend;
use aether_runtime_state::{
    RedisClientConfig, RedisConsumerGroup, RedisConsumerName, RedisKeyspace, RedisLockLease,
    RedisLockRunner, RedisLockRunnerConfig, RedisStreamName, RedisStreamReclaimConfig,
    RedisStreamRunner, RedisStreamRunnerConfig,
};
use aether_testkit::{init_test_runtime_for, ManagedPostgresServer, ManagedRedisServer};
use futures_util::stream::{self, StreamExt};
use serde::Serialize;

#[derive(Debug, Clone)]
struct DependencyPressureBaselineConfig {
    redis_lock_total: usize,
    redis_lock_concurrency: usize,
    redis_stream_total: usize,
    redis_stream_concurrency: usize,
    redis_reclaim_total: usize,
    redis_reclaim_min_idle: Duration,
    postgres_rows: usize,
    postgres_lease_cycles: usize,
    postgres_lease_concurrency: usize,
    postgres_lease_batch_size: usize,
    postgres_lease_ms: u64,
    timeout: Duration,
    output_path: Option<PathBuf>,
    redis_url: Option<String>,
    postgres_url: Option<String>,
}

impl Default for DependencyPressureBaselineConfig {
    fn default() -> Self {
        Self {
            redis_lock_total: 1_000,
            redis_lock_concurrency: 20,
            redis_stream_total: 2_000,
            redis_stream_concurrency: 20,
            redis_reclaim_total: 256,
            redis_reclaim_min_idle: Duration::from_millis(100),
            postgres_rows: 512,
            postgres_lease_cycles: 128,
            postgres_lease_concurrency: 16,
            postgres_lease_batch_size: 16,
            postgres_lease_ms: 250,
            timeout: Duration::from_secs(10),
            output_path: None,
            redis_url: None,
            postgres_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct OperationSummary {
    total_calls: usize,
    total_items: usize,
    failed_calls: usize,
    p50_ms: u64,
    p95_ms: u64,
    max_ms: u64,
    mean_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct RedisLockPressureReport {
    acquire: OperationSummary,
    renew: OperationSummary,
    release: OperationSummary,
}

#[derive(Debug, Clone, Serialize)]
struct RedisStreamPressureReport {
    append: OperationSummary,
    read_group: OperationSummary,
    reclaim: OperationSummary,
    ack: OperationSummary,
}

#[derive(Debug, Clone, Serialize)]
struct PostgresLeasePressureReport {
    claim: OperationSummary,
    renew: OperationSummary,
    release: OperationSummary,
}

#[derive(Debug, Clone, Serialize)]
struct DependencyPressureBaselineReport {
    suite: &'static str,
    redis_url: String,
    postgres_url: String,
    redis_lock: RedisLockPressureReport,
    redis_stream: RedisStreamPressureReport,
    postgres_lease: PostgresLeasePressureReport,
}

#[derive(Default)]
struct SummaryCollector {
    latencies_ms: tokio::sync::Mutex<Vec<u64>>,
    total_items: std::sync::atomic::AtomicUsize,
    failed_calls: std::sync::atomic::AtomicUsize,
    total_calls: std::sync::atomic::AtomicUsize,
}

impl SummaryCollector {
    async fn record(&self, elapsed: Duration, items: usize, failed: bool) {
        self.latencies_ms
            .lock()
            .await
            .push(elapsed.as_millis() as u64);
        self.total_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.total_items
            .fetch_add(items, std::sync::atomic::Ordering::AcqRel);
        if failed {
            self.failed_calls
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    async fn summarize(&self) -> OperationSummary {
        let mut latencies = self.latencies_ms.lock().await.clone();
        latencies.sort_unstable();
        let (p50_ms, p95_ms, max_ms, mean_ms) = summarize_latencies(&latencies);
        OperationSummary {
            total_calls: self.total_calls.load(std::sync::atomic::Ordering::Acquire),
            total_items: self.total_items.load(std::sync::atomic::Ordering::Acquire),
            failed_calls: self.failed_calls.load(std::sync::atomic::Ordering::Acquire),
            p50_ms,
            p95_ms,
            max_ms,
            mean_ms,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("dependency-pressure-baseline");
    let config = parse_args(std::env::args().skip(1).collect())?;
    let report = run_suite(&config).await?;
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

async fn run_suite(
    config: &DependencyPressureBaselineConfig,
) -> Result<DependencyPressureBaselineReport, Box<dyn std::error::Error>> {
    let managed_redis = if config.redis_url.is_none() {
        Some(ManagedRedisServer::start().await?)
    } else {
        None
    };
    let managed_postgres = if config.postgres_url.is_none() {
        Some(ManagedPostgresServer::start().await?)
    } else {
        None
    };

    let redis_url = config
        .redis_url
        .clone()
        .or_else(|| {
            managed_redis
                .as_ref()
                .map(|server| server.redis_url().to_string())
        })
        .expect("redis url should resolve");
    let postgres_url = config
        .postgres_url
        .clone()
        .or_else(|| {
            managed_postgres
                .as_ref()
                .map(|server| server.database_url().to_string())
        })
        .expect("postgres url should resolve");

    let redis_config = RedisClientConfig {
        url: redis_url.clone(),
        key_prefix: Some(format!("aether-dependency-pressure-{}", std::process::id())),
    };
    let redis_keyspace = redis_config.keyspace();
    let postgres_backend = PostgresBackend::from_config(PostgresPoolConfig {
        database_url: postgres_url.clone(),
        min_connections: 1,
        max_connections: (config.postgres_lease_concurrency as u32).saturating_add(8),
        acquire_timeout_ms: config.timeout.as_millis() as u64,
        idle_timeout_ms: 60_000,
        max_lifetime_ms: 10 * 60_000,
        statement_cache_capacity: 128,
        require_ssl: false,
    })?;

    bootstrap_postgres_lease_table(postgres_backend.pool_clone(), config).await?;

    let lock_runner = RedisLockRunner::from_config(
        redis_config.clone(),
        RedisLockRunnerConfig {
            command_timeout_ms: Some(config.timeout.as_millis() as u64),
            default_ttl_ms: 5_000,
        },
    )
    .await?;
    let stream_runner = RedisStreamRunner::from_config(
        redis_config,
        RedisStreamRunnerConfig {
            command_timeout_ms: Some(config.timeout.as_millis() as u64),
            read_block_ms: Some(10),
            read_count: 64,
        },
    )
    .await?;
    let lease_runner = postgres_backend.lease_runner(PostgresLeaseRunnerConfig {
        statement_timeout_ms: Some(config.timeout.as_millis() as u64),
        lock_timeout_ms: Some(1_000),
    })?;

    let redis_lock = benchmark_redis_lock(&redis_keyspace, &lock_runner, config).await?;
    let redis_stream = benchmark_redis_stream(&redis_keyspace, &stream_runner, config).await?;
    let postgres_lease = benchmark_postgres_lease(&lease_runner, config).await?;

    Ok(DependencyPressureBaselineReport {
        suite: "dependency_pressure_baseline",
        redis_url,
        postgres_url,
        redis_lock,
        redis_stream,
        postgres_lease,
    })
}

async fn bootstrap_postgres_lease_table(
    pool: sqlx::PgPool,
    config: &DependencyPressureBaselineConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query("DROP TABLE IF EXISTS baseline_lease_jobs")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE baseline_lease_jobs (
             id TEXT PRIMARY KEY,
             status TEXT NOT NULL,
             updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
             lease_owner TEXT,
             lease_expires_at TIMESTAMPTZ
         )",
    )
    .execute(&pool)
    .await?;

    let mut builder = sqlx::QueryBuilder::new("INSERT INTO baseline_lease_jobs (id, status) ");
    builder.push_values(0..config.postgres_rows, |mut row, index| {
        row.push_bind(format!("job-{index:05}")).push_bind("ready");
    });
    builder.build().execute(&pool).await?;

    Ok(())
}

async fn benchmark_redis_lock(
    keyspace: &RedisKeyspace,
    runner: &RedisLockRunner,
    config: &DependencyPressureBaselineConfig,
) -> Result<RedisLockPressureReport, Box<dyn std::error::Error>> {
    let acquire = Arc::new(SummaryCollector::default());
    let renew = Arc::new(SummaryCollector::default());
    let release = Arc::new(SummaryCollector::default());
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    stream::iter(0..config.redis_lock_concurrency)
        .for_each_concurrent(config.redis_lock_concurrency, |_| {
            let runner = runner.clone();
            let acquire = acquire.clone();
            let renew = renew.clone();
            let release = release.clone();
            let next = next.clone();
            let keyspace = keyspace.clone();
            async move {
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    if index >= config.redis_lock_total {
                        break;
                    }
                    let owner = format!("lock-owner-{index}");
                    let key = keyspace.lock_key(&format!("dependency-pressure-{index}"));
                    let acquire_started = Instant::now();
                    match runner.try_acquire(&key, &owner, None).await {
                        Ok(Some(lease)) => {
                            acquire.record(acquire_started.elapsed(), 1, false).await;
                            record_redis_lock_follow_up(&runner, &lease, &renew, &release).await;
                        }
                        Ok(None) => {
                            acquire.record(acquire_started.elapsed(), 0, true).await;
                        }
                        Err(_) => {
                            acquire.record(acquire_started.elapsed(), 0, true).await;
                        }
                    }
                }
            }
        })
        .await;

    Ok(RedisLockPressureReport {
        acquire: acquire.summarize().await,
        renew: renew.summarize().await,
        release: release.summarize().await,
    })
}

async fn record_redis_lock_follow_up(
    runner: &RedisLockRunner,
    lease: &RedisLockLease,
    renew: &SummaryCollector,
    release: &SummaryCollector,
) {
    let renew_started = Instant::now();
    let renew_ok = runner.renew(lease, None).await.unwrap_or(false);
    renew
        .record(renew_started.elapsed(), usize::from(renew_ok), !renew_ok)
        .await;

    let release_started = Instant::now();
    let release_ok = runner.release(lease).await.unwrap_or(false);
    release
        .record(
            release_started.elapsed(),
            usize::from(release_ok),
            !release_ok,
        )
        .await;
}

async fn benchmark_redis_stream(
    keyspace: &RedisKeyspace,
    runner: &RedisStreamRunner,
    config: &DependencyPressureBaselineConfig,
) -> Result<RedisStreamPressureReport, Box<dyn std::error::Error>> {
    let stream = keyspace.stream_name("dependency-pressure");
    let group = RedisConsumerGroup("dependency-group".to_string());
    let consumer_a = RedisConsumerName("consumer-a".to_string());
    let consumer_b = RedisConsumerName("consumer-b".to_string());
    runner
        .ensure_consumer_group(&stream, &group, "0-0")
        .await
        .map_err(std::io::Error::other)?;

    let append = benchmark_redis_stream_append(runner, &stream, config).await?;
    let (read_group, drained_ids) =
        benchmark_redis_stream_read_group(runner, &stream, &group, &consumer_a, config).await?;
    let ack = SummaryCollector::default();
    benchmark_redis_stream_ack_into(&ack, runner, &stream, &group, &drained_ids).await?;
    let (reclaim, reclaimed_ids) =
        benchmark_redis_stream_reclaim(runner, &stream, &group, &consumer_a, &consumer_b, config)
            .await?;
    benchmark_redis_stream_ack_into(&ack, runner, &stream, &group, &reclaimed_ids).await?;

    Ok(RedisStreamPressureReport {
        append,
        read_group,
        reclaim,
        ack: ack.summarize().await,
    })
}

async fn benchmark_redis_stream_append(
    runner: &RedisStreamRunner,
    stream: &RedisStreamName,
    config: &DependencyPressureBaselineConfig,
) -> Result<OperationSummary, Box<dyn std::error::Error>> {
    let collector = Arc::new(SummaryCollector::default());
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    stream::iter(0..config.redis_stream_concurrency)
        .for_each_concurrent(config.redis_stream_concurrency, |_| {
            let runner = runner.clone();
            let stream = stream.clone();
            let collector = collector.clone();
            let next = next.clone();
            async move {
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    if index >= config.redis_stream_total {
                        break;
                    }
                    let started = Instant::now();
                    let result = runner
                        .append_json(
                            &stream,
                            "payload",
                            &serde_json::json!({
                                "job_id": index,
                                "kind": "dependency-pressure",
                            }),
                        )
                        .await;
                    collector
                        .record(
                            started.elapsed(),
                            usize::from(result.is_ok()),
                            result.is_err(),
                        )
                        .await;
                }
            }
        })
        .await;

    Ok(collector.summarize().await)
}

async fn benchmark_redis_stream_read_group(
    runner: &RedisStreamRunner,
    stream: &RedisStreamName,
    group: &RedisConsumerGroup,
    consumer: &RedisConsumerName,
    config: &DependencyPressureBaselineConfig,
) -> Result<(OperationSummary, Vec<String>), Box<dyn std::error::Error>> {
    let collector = SummaryCollector::default();
    let mut ids = Vec::with_capacity(config.redis_stream_total);

    while ids.len() < config.redis_stream_total {
        let started = Instant::now();
        match runner.read_group(stream, group, consumer).await {
            Ok(entries) => {
                let item_count = entries.len();
                ids.extend(entries.into_iter().map(|entry| entry.id));
                collector.record(started.elapsed(), item_count, false).await;
            }
            Err(_) => {
                collector.record(started.elapsed(), 0, true).await;
            }
        }
    }

    Ok((collector.summarize().await, ids))
}

async fn benchmark_redis_stream_reclaim(
    runner: &RedisStreamRunner,
    stream: &RedisStreamName,
    group: &RedisConsumerGroup,
    consumer_a: &RedisConsumerName,
    consumer_b: &RedisConsumerName,
    config: &DependencyPressureBaselineConfig,
) -> Result<(OperationSummary, Vec<String>), Box<dyn std::error::Error>> {
    for index in 0..config.redis_reclaim_total {
        runner
            .append_json(
                stream,
                "payload",
                &serde_json::json!({
                    "job_id": format!("reclaim-{index}"),
                    "kind": "dependency-pressure",
                }),
            )
            .await
            .map_err(std::io::Error::other)?;
    }

    let mut pending_ids = Vec::with_capacity(config.redis_reclaim_total);
    while pending_ids.len() < config.redis_reclaim_total {
        let entries = runner
            .read_group(stream, group, consumer_a)
            .await
            .map_err(std::io::Error::other)?;
        pending_ids.extend(entries.into_iter().map(|entry| entry.id));
    }

    tokio::time::sleep(config.redis_reclaim_min_idle + Duration::from_millis(20)).await;

    let collector = SummaryCollector::default();
    let mut reclaimed_ids = Vec::with_capacity(config.redis_reclaim_total);
    let mut next_start_id = "0-0".to_string();

    while reclaimed_ids.len() < config.redis_reclaim_total {
        let started = Instant::now();
        match runner
            .claim_stale(
                stream,
                group,
                consumer_b,
                &next_start_id,
                RedisStreamReclaimConfig {
                    min_idle_ms: config.redis_reclaim_min_idle.as_millis() as u64,
                    count: 64,
                },
            )
            .await
        {
            Ok(result) => {
                next_start_id = result.next_start_id.clone();
                let item_count = result.entries.len();
                reclaimed_ids.extend(result.entries.into_iter().map(|entry| entry.id));
                collector.record(started.elapsed(), item_count, false).await;
            }
            Err(_) => {
                collector.record(started.elapsed(), 0, true).await;
            }
        }
    }

    Ok((collector.summarize().await, reclaimed_ids))
}

async fn benchmark_redis_stream_ack_into(
    collector: &SummaryCollector,
    runner: &RedisStreamRunner,
    stream: &RedisStreamName,
    group: &RedisConsumerGroup,
    ids: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    for chunk in ids.chunks(64) {
        let started = Instant::now();
        match runner.ack(stream, group, chunk).await {
            Ok(acked) => {
                collector.record(started.elapsed(), acked, false).await;
            }
            Err(_) => {
                collector.record(started.elapsed(), 0, true).await;
            }
        }
    }

    Ok(())
}

async fn benchmark_postgres_lease(
    runner: &aether_data::driver::postgres::PostgresLeaseRunner,
    config: &DependencyPressureBaselineConfig,
) -> Result<PostgresLeasePressureReport, Box<dyn std::error::Error>> {
    let spec = PostgresLeaseClaimSpec {
        table: "baseline_lease_jobs",
        id_column: "id",
        lease_owner_column: "lease_owner",
        lease_expires_at_column: "lease_expires_at",
        eligibility_predicate_sql: "status = 'ready'",
        order_by_sql: "id ASC",
    };
    let claim_options = PostgresLeaseClaimOptions {
        batch_size: config.postgres_lease_batch_size,
        lease_ms: config.postgres_lease_ms,
    };

    let claim = Arc::new(SummaryCollector::default());
    let renew = Arc::new(SummaryCollector::default());
    let release = Arc::new(SummaryCollector::default());
    let next_cycle = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    stream::iter(0..config.postgres_lease_concurrency)
        .for_each_concurrent(config.postgres_lease_concurrency, |worker_index| {
            let runner = runner.clone();
            let spec = spec.clone();
            let claim = claim.clone();
            let renew = renew.clone();
            let release = release.clone();
            let next_cycle = next_cycle.clone();
            async move {
                let owner = format!("lease-owner-{worker_index}");
                loop {
                    let cycle = next_cycle.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    if cycle >= config.postgres_lease_cycles {
                        break;
                    }

                    let claim_started = Instant::now();
                    match runner.claim_ids(&spec, claim_options, &owner).await {
                        Ok(ids) => {
                            let item_count = ids.len();
                            claim
                                .record(claim_started.elapsed(), item_count, false)
                                .await;
                            if ids.is_empty() {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                                continue;
                            }
                            record_postgres_lease_follow_up(
                                &runner,
                                &spec,
                                &owner,
                                &ids,
                                config.postgres_lease_ms,
                                &renew,
                                &release,
                            )
                            .await;
                        }
                        Err(_) => {
                            claim.record(claim_started.elapsed(), 0, true).await;
                        }
                    }
                }
            }
        })
        .await;

    Ok(PostgresLeasePressureReport {
        claim: claim.summarize().await,
        renew: renew.summarize().await,
        release: release.summarize().await,
    })
}

async fn record_postgres_lease_follow_up(
    runner: &aether_data::driver::postgres::PostgresLeaseRunner,
    spec: &PostgresLeaseClaimSpec,
    owner: &str,
    ids: &[DatabaseRecordId],
    lease_ms: u64,
    renew: &SummaryCollector,
    release: &SummaryCollector,
) {
    let renew_started = Instant::now();
    match runner.renew_ids(spec, ids, owner, lease_ms).await {
        Ok(renewed) => {
            let item_count = renewed.len();
            renew
                .record(renew_started.elapsed(), item_count, false)
                .await;
        }
        Err(_) => {
            renew.record(renew_started.elapsed(), 0, true).await;
        }
    }

    let release_started = Instant::now();
    match runner.release_ids(spec, ids, owner).await {
        Ok(released) => {
            let item_count = released.len();
            release
                .record(release_started.elapsed(), item_count, false)
                .await;
        }
        Err(_) => {
            release.record(release_started.elapsed(), 0, true).await;
        }
    }
}

fn summarize_latencies(latencies: &[u64]) -> (u64, u64, u64, u64) {
    if latencies.is_empty() {
        return (0, 0, 0, 0);
    }
    let max_ms = *latencies.last().unwrap_or(&0);
    let mean_ms = latencies.iter().sum::<u64>() / latencies.len() as u64;
    let p50_ms = percentile(latencies, 50);
    let p95_ms = percentile(latencies, 95);
    (p50_ms, p95_ms, max_ms, mean_ms)
}

fn percentile(latencies: &[u64], percentile: u8) -> u64 {
    if latencies.is_empty() {
        return 0;
    }
    let last_index = latencies.len() - 1;
    let rank = ((last_index as f64) * (percentile as f64 / 100.0)).round() as usize;
    latencies[rank.min(last_index)]
}

fn parse_args(
    args: Vec<String>,
) -> Result<DependencyPressureBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = DependencyPressureBaselineConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--redis-lock-total" => {
                config.redis_lock_total = iter
                    .next()
                    .ok_or("missing value for --redis-lock-total")?
                    .parse()?;
            }
            "--redis-lock-concurrency" => {
                config.redis_lock_concurrency = iter
                    .next()
                    .ok_or("missing value for --redis-lock-concurrency")?
                    .parse()?;
            }
            "--redis-stream-total" => {
                config.redis_stream_total = iter
                    .next()
                    .ok_or("missing value for --redis-stream-total")?
                    .parse()?;
            }
            "--redis-stream-concurrency" => {
                config.redis_stream_concurrency = iter
                    .next()
                    .ok_or("missing value for --redis-stream-concurrency")?
                    .parse()?;
            }
            "--redis-reclaim-total" => {
                config.redis_reclaim_total = iter
                    .next()
                    .ok_or("missing value for --redis-reclaim-total")?
                    .parse()?;
            }
            "--redis-reclaim-min-idle-ms" => {
                config.redis_reclaim_min_idle = Duration::from_millis(
                    iter.next()
                        .ok_or("missing value for --redis-reclaim-min-idle-ms")?
                        .parse()?,
                );
            }
            "--postgres-rows" => {
                config.postgres_rows = iter
                    .next()
                    .ok_or("missing value for --postgres-rows")?
                    .parse()?;
            }
            "--postgres-lease-cycles" => {
                config.postgres_lease_cycles = iter
                    .next()
                    .ok_or("missing value for --postgres-lease-cycles")?
                    .parse()?;
            }
            "--postgres-lease-concurrency" => {
                config.postgres_lease_concurrency = iter
                    .next()
                    .ok_or("missing value for --postgres-lease-concurrency")?
                    .parse()?;
            }
            "--postgres-lease-batch-size" => {
                config.postgres_lease_batch_size = iter
                    .next()
                    .ok_or("missing value for --postgres-lease-batch-size")?
                    .parse()?;
            }
            "--postgres-lease-ms" => {
                config.postgres_lease_ms = iter
                    .next()
                    .ok_or("missing value for --postgres-lease-ms")?
                    .parse()?;
            }
            "--timeout-ms" => {
                config.timeout = Duration::from_millis(
                    iter.next()
                        .ok_or("missing value for --timeout-ms")?
                        .parse()?,
                );
            }
            "--output" => {
                config.output_path = Some(PathBuf::from(
                    iter.next().ok_or("missing value for --output")?,
                ));
            }
            "--redis-url" => {
                config.redis_url = Some(iter.next().ok_or("missing value for --redis-url")?);
            }
            "--postgres-url" => {
                config.postgres_url = Some(iter.next().ok_or("missing value for --postgres-url")?);
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(
    config: &DependencyPressureBaselineConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    if config.redis_lock_total == 0
        || config.redis_lock_concurrency == 0
        || config.redis_stream_total == 0
        || config.redis_stream_concurrency == 0
        || config.redis_reclaim_total == 0
        || config.postgres_rows == 0
        || config.postgres_lease_cycles == 0
        || config.postgres_lease_concurrency == 0
        || config.postgres_lease_batch_size == 0
        || config.postgres_lease_ms == 0
        || config.timeout.is_zero()
    {
        return Err("all dependency pressure baseline numeric settings must be positive".into());
    }
    Ok(())
}
