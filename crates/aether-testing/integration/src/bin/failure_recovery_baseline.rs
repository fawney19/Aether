// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_data::driver::postgres::{
    PostgresLeaseClaimOptions, PostgresLeaseClaimSpec, PostgresLeaseRunnerConfig,
    PostgresPoolConfig, PostgresTransactionOptions,
};
use aether_data::{DataLayerError, PostgresBackend};
use aether_runtime_state::{RedisClientConfig, RedisLockRunner, RedisLockRunnerConfig};
use aether_testkit::{
    init_test_runtime_for, reserve_local_port, BenchmarkRuntimeSampler, BenchmarkRuntimeSnapshot,
    ManagedPostgresServer, ManagedRedisServer, TunnelHarness, TunnelHarnessConfig,
};
use futures_util::{FutureExt, StreamExt};
use serde::Serialize;
use tokio::sync::{oneshot, Mutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";

#[derive(Debug, Clone)]
struct FailureRecoveryBaselineConfig {
    redis_attempts: usize,
    redis_concurrency: usize,
    redis_restart_delay: Duration,
    redis_downtime: Duration,
    postgres_statement_timeout: Duration,
    postgres_sleep: Duration,
    tunnel_attempts: usize,
    tunnel_concurrency: usize,
    tunnel_hold: Duration,
    tunnel_restart_delay: Duration,
    tunnel_downtime: Duration,
    timeout: Duration,
    output_path: Option<PathBuf>,
    redis_url: Option<String>,
    postgres_url: Option<String>,
}

impl Default for FailureRecoveryBaselineConfig {
    fn default() -> Self {
        Self {
            redis_attempts: 400,
            redis_concurrency: 10,
            redis_restart_delay: Duration::from_millis(200),
            redis_downtime: Duration::from_millis(150),
            postgres_statement_timeout: Duration::from_millis(50),
            postgres_sleep: Duration::from_millis(200),
            tunnel_attempts: 60,
            tunnel_concurrency: 4,
            tunnel_hold: Duration::from_millis(50),
            tunnel_restart_delay: Duration::from_millis(200),
            tunnel_downtime: Duration::from_millis(150),
            timeout: Duration::from_secs(10),
            output_path: None,
            redis_url: None,
            postgres_url: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FaultPhase {
    Pre,
    During,
    Post,
}

#[derive(Debug, Default, Clone, Serialize)]
struct PhaseCounts {
    pre_successes: usize,
    pre_failures: usize,
    during_successes: usize,
    during_failures: usize,
    post_successes: usize,
    post_failures: usize,
}

#[derive(Debug, Clone, Serialize)]
struct RecoverySummary {
    total_attempts: usize,
    successful_attempts: usize,
    failed_attempts: usize,
    recovered_after_restart_ms: Option<u64>,
    p50_ms: u64,
    p95_ms: u64,
    p99_ms: u64,
    max_ms: u64,
    mean_ms: u64,
    phase_counts: PhaseCounts,
    runtime: BenchmarkRuntimeSnapshot,
}

#[derive(Debug, Clone, Serialize)]
struct PostgresSlowQueryRecoveryReport {
    slow_query_timed_out: bool,
    slow_query_latency_ms: u64,
    recovery_claim_succeeded: bool,
    recovery_claim_latency_ms: u64,
    recovery_claimed_items: usize,
    runtime: BenchmarkRuntimeSnapshot,
}

#[derive(Debug, Clone, Serialize)]
struct FailureRecoveryBaselineReport {
    suite: &'static str,
    redis_url: String,
    postgres_url: String,
    redis_restart: RecoverySummary,
    postgres_slow_query: PostgresSlowQueryRecoveryReport,
    tunnel_restart: RecoverySummary,
}

#[derive(Default)]
struct RecoveryCollector {
    latencies_ms: Mutex<Vec<u64>>,
    phase_counts: Mutex<PhaseCounts>,
    successful_attempts: AtomicUsize,
    failed_attempts: AtomicUsize,
}

impl RecoveryCollector {
    async fn record(&self, phase: FaultPhase, success: bool, latency: Duration) {
        self.latencies_ms
            .lock()
            .await
            .push(latency.as_millis() as u64);
        let mut counts = self.phase_counts.lock().await;
        match (phase, success) {
            (FaultPhase::Pre, true) => counts.pre_successes += 1,
            (FaultPhase::Pre, false) => counts.pre_failures += 1,
            (FaultPhase::During, true) => counts.during_successes += 1,
            (FaultPhase::During, false) => counts.during_failures += 1,
            (FaultPhase::Post, true) => counts.post_successes += 1,
            (FaultPhase::Post, false) => counts.post_failures += 1,
        }
        if success {
            self.successful_attempts.fetch_add(1, Ordering::AcqRel);
        } else {
            self.failed_attempts.fetch_add(1, Ordering::AcqRel);
        }
    }

    async fn summarize(
        &self,
        recovered_after_restart_ms: Option<u64>,
        runtime: BenchmarkRuntimeSnapshot,
    ) -> RecoverySummary {
        let mut latencies = self.latencies_ms.lock().await.clone();
        latencies.sort_unstable();
        let (p50_ms, p95_ms, p99_ms, max_ms, mean_ms) = summarize_latencies(&latencies);
        let phase_counts = self.phase_counts.lock().await.clone();
        RecoverySummary {
            total_attempts: self.successful_attempts.load(Ordering::Acquire)
                + self.failed_attempts.load(Ordering::Acquire),
            successful_attempts: self.successful_attempts.load(Ordering::Acquire),
            failed_attempts: self.failed_attempts.load(Ordering::Acquire),
            recovered_after_restart_ms,
            p50_ms,
            p95_ms,
            p99_ms,
            max_ms,
            mean_ms,
            phase_counts,
            runtime,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("failure-recovery-baseline");
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
    config: &FailureRecoveryBaselineConfig,
) -> Result<FailureRecoveryBaselineReport, Box<dyn std::error::Error>> {
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

    let redis_server = Arc::new(Mutex::new(
        managed_redis.ok_or("failure recovery baseline requires managed redis")?,
    ));
    let postgres_server =
        managed_postgres.ok_or("failure recovery baseline requires managed postgres")?;

    let redis_restart = benchmark_redis_restart_recovery(redis_server.clone(), config).await?;
    let postgres_slow_query =
        benchmark_postgres_slow_query_recovery(postgres_server.database_url(), config).await?;
    let tunnel_restart = benchmark_tunnel_restart_recovery(config).await?;

    Ok(FailureRecoveryBaselineReport {
        suite: "failure_recovery_baseline",
        redis_url,
        postgres_url,
        redis_restart,
        postgres_slow_query,
        tunnel_restart,
    })
}

async fn benchmark_redis_restart_recovery(
    redis_server: Arc<Mutex<ManagedRedisServer>>,
    config: &FailureRecoveryBaselineConfig,
) -> Result<RecoverySummary, Box<dyn std::error::Error>> {
    let mut runtime_sampler = BenchmarkRuntimeSampler::new();
    let redis_url = redis_server.lock().await.redis_url().to_string();
    let redis_config = RedisClientConfig {
        url: redis_url,
        key_prefix: Some(format!("aether-failure-recovery-{}", std::process::id())),
    };
    let keyspace = redis_config.keyspace();
    let runner = RedisLockRunner::from_config(
        redis_config,
        RedisLockRunnerConfig {
            command_timeout_ms: Some(250),
            default_ttl_ms: 1_000,
        },
    )
    .await?;
    let collector = Arc::new(RecoveryCollector::default());
    let next_attempt = Arc::new(AtomicUsize::new(0));
    let phase = Arc::new(AtomicUsize::new(0));
    let recovered_after_restart_ms = Arc::new(AtomicU64::new(0));

    let absolute_restart_started = Arc::new(Mutex::new(None::<Instant>));
    let restart_phase = phase.clone();
    let restart_started = absolute_restart_started.clone();
    let server_for_restart = redis_server.clone();
    let redis_restart_delay = config.redis_restart_delay;
    let redis_downtime = config.redis_downtime;
    let restart_task = tokio::spawn(async move {
        tokio::time::sleep(redis_restart_delay).await;
        restart_phase.store(1, Ordering::Release);
        *restart_started.lock().await = Some(Instant::now());
        {
            let mut server = server_for_restart.lock().await;
            server.stop().map_err(std::io::Error::other)?;
        }
        tokio::time::sleep(redis_downtime).await;
        {
            let mut server = server_for_restart.lock().await;
            server
                .restart()
                .await
                .map_err(|err| std::io::Error::other(err.to_string()))?;
        }
        restart_phase.store(2, Ordering::Release);
        Ok::<(), std::io::Error>(())
    });

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..config.redis_concurrency {
        let runner = runner.clone();
        let keyspace = keyspace.clone();
        let collector = collector.clone();
        let next_attempt = next_attempt.clone();
        let phase = phase.clone();
        let restart_started = absolute_restart_started.clone();
        let recovered_after_restart_ms = recovered_after_restart_ms.clone();
        let total_attempts = config.redis_attempts;
        tasks.spawn(async move {
            loop {
                let current = next_attempt.fetch_add(1, Ordering::AcqRel);
                if current >= total_attempts {
                    break;
                }
                let current_phase = classify_phase(phase.load(Ordering::Acquire));
                let key = keyspace.lock_key(&format!("recovery-lock-{current}"));
                let owner = format!("redis-owner-{current}");
                let started = Instant::now();
                let success = match runner.try_acquire(&key, &owner, Some(1_000)).await {
                    Ok(Some(lease)) => runner.release(&lease).await.unwrap_or(false),
                    Ok(None) => false,
                    Err(_) => false,
                };
                if success && matches!(current_phase, FaultPhase::Post) {
                    if let Some(restart_started_at) = *restart_started.lock().await {
                        let elapsed = restart_started_at.elapsed().as_millis() as u64;
                        let _ = recovered_after_restart_ms.compare_exchange(
                            0,
                            elapsed,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                    }
                }
                collector
                    .record(current_phase, success, started.elapsed())
                    .await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.map_err(std::io::Error::other)?;
    }
    restart_task
        .await
        .map_err(|err| format!("redis restart task failed: {err}"))?
        .map_err(std::io::Error::other)?;

    Ok(collector
        .summarize(
            load_optional_atomic_u64(&recovered_after_restart_ms),
            runtime_sampler.snapshot(),
        )
        .await)
}

async fn benchmark_postgres_slow_query_recovery(
    postgres_url: &str,
    config: &FailureRecoveryBaselineConfig,
) -> Result<PostgresSlowQueryRecoveryReport, Box<dyn std::error::Error>> {
    let mut runtime_sampler = BenchmarkRuntimeSampler::new();
    let backend = PostgresBackend::from_config(PostgresPoolConfig {
        database_url: postgres_url.to_string(),
        min_connections: 1,
        max_connections: 8,
        acquire_timeout_ms: config.timeout.as_millis() as u64,
        idle_timeout_ms: 60_000,
        max_lifetime_ms: 10 * 60_000,
        statement_cache_capacity: 64,
        require_ssl: false,
    })?;
    bootstrap_failure_recovery_lease_table(backend.pool_clone()).await?;
    let transaction_runner = backend.transaction_runner();
    let lease_runner = backend.lease_runner(PostgresLeaseRunnerConfig {
        statement_timeout_ms: Some(config.timeout.as_millis() as u64),
        lock_timeout_ms: Some(1_000),
    })?;
    let postgres_sleep_secs = config.postgres_sleep.as_secs_f64();

    let slow_query_started = Instant::now();
    let slow_query_timed_out = transaction_runner
        .run(
            PostgresTransactionOptions {
                statement_timeout_ms: Some(config.postgres_statement_timeout.as_millis() as u64),
                ..PostgresTransactionOptions::read_write()
            },
            |tx| {
                async move {
                    sqlx::query("SELECT pg_sleep($1::double precision)")
                        .bind(postgres_sleep_secs)
                        .execute(&mut **tx)
                        .await
                        .map_err(DataLayerError::postgres)?;
                    Ok(())
                }
                .boxed()
            },
        )
        .await
        .is_err();
    let slow_query_latency_ms = slow_query_started.elapsed().as_millis() as u64;

    let recovery_claim_started = Instant::now();
    let claimed_ids = lease_runner
        .claim_ids(
            &PostgresLeaseClaimSpec {
                table: "baseline_failure_lease_jobs",
                id_column: "id",
                lease_owner_column: "lease_owner",
                lease_expires_at_column: "lease_expires_at",
                eligibility_predicate_sql: "status = 'ready'",
                order_by_sql: "id ASC",
            },
            PostgresLeaseClaimOptions {
                batch_size: 8,
                lease_ms: 250,
            },
            "recovery-owner",
        )
        .await
        .unwrap_or_default();
    let recovery_claim_latency_ms = recovery_claim_started.elapsed().as_millis() as u64;

    Ok(PostgresSlowQueryRecoveryReport {
        slow_query_timed_out,
        slow_query_latency_ms,
        recovery_claim_succeeded: !claimed_ids.is_empty(),
        recovery_claim_latency_ms,
        recovery_claimed_items: claimed_ids.len(),
        runtime: runtime_sampler.snapshot(),
    })
}

async fn bootstrap_failure_recovery_lease_table(
    pool: sqlx::PgPool,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query("DROP TABLE IF EXISTS baseline_failure_lease_jobs")
        .execute(&pool)
        .await?;
    sqlx::query(
        "CREATE TABLE baseline_failure_lease_jobs (
             id TEXT PRIMARY KEY,
             status TEXT NOT NULL,
             updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
             lease_owner TEXT,
             lease_expires_at TIMESTAMPTZ
         )",
    )
    .execute(&pool)
    .await?;

    let mut builder =
        sqlx::QueryBuilder::new("INSERT INTO baseline_failure_lease_jobs (id, status) ");
    builder.push_values(0..32, |mut row, index| {
        row.push_bind(format!("recovery-job-{index:03}"))
            .push_bind("ready");
    });
    builder.build().execute(&pool).await?;
    Ok(())
}

async fn benchmark_tunnel_restart_recovery(
    config: &FailureRecoveryBaselineConfig,
) -> Result<RecoverySummary, Box<dyn std::error::Error>> {
    let mut runtime_sampler = BenchmarkRuntimeSampler::new();
    let port = reserve_local_port()?;
    let tunnel_config = TunnelHarnessConfig::default();
    let initial_tunnel = TunnelHarness::start_on_port(tunnel_config.clone(), port).await?;
    let ws_url = format!("ws://127.0.0.1:{port}{PROXY_TUNNEL_PATH}");
    let collector = Arc::new(RecoveryCollector::default());
    let next_attempt = Arc::new(AtomicUsize::new(0));
    let phase = Arc::new(AtomicUsize::new(0));
    let recovered_after_restart_ms = Arc::new(AtomicU64::new(0));
    let restart_started = Arc::new(Mutex::new(None::<Instant>));
    let (done_tx, done_rx) = oneshot::channel::<()>();

    let tunnel_restart_delay = config.tunnel_restart_delay;
    let tunnel_downtime = config.tunnel_downtime;
    let phase_for_restart = phase.clone();
    let restart_started_for_task = restart_started.clone();
    let restart_task = tokio::spawn(async move {
        tokio::time::sleep(tunnel_restart_delay).await;
        phase_for_restart.store(1, Ordering::Release);
        *restart_started_for_task.lock().await = Some(Instant::now());
        drop(initial_tunnel);
        tokio::time::sleep(tunnel_downtime).await;
        let restarted_tunnel = start_tunnel_on_port_retry(tunnel_config, port).await?;
        phase_for_restart.store(2, Ordering::Release);
        let _ = done_rx.await;
        drop(restarted_tunnel);
        Ok::<(), String>(())
    });

    let mut workers = tokio::task::JoinSet::new();
    for worker_index in 0..config.tunnel_concurrency {
        let ws_url = ws_url.clone();
        let next_attempt = next_attempt.clone();
        let collector = collector.clone();
        let phase = phase.clone();
        let recovered_after_restart_ms = recovered_after_restart_ms.clone();
        let restart_started = restart_started.clone();
        let timeout = config.timeout;
        let hold = config.tunnel_hold;
        let total_attempts = config.tunnel_attempts;
        workers.spawn(async move {
            loop {
                let current = next_attempt.fetch_add(1, Ordering::AcqRel);
                if current >= total_attempts {
                    break;
                }
                let current_phase = classify_phase(phase.load(Ordering::Acquire));
                let mut request = ws_url
                    .clone()
                    .into_client_request()
                    .map_err(|err| format!("failed to build websocket request: {err}"))?;
                request.headers_mut().insert(
                    "x-node-id",
                    format!("recovery-node-{worker_index}-{current}")
                        .parse()
                        .map_err(|err| format!("failed to build x-node-id header: {err}"))?,
                );
                request.headers_mut().insert(
                    aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
                    aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION_STR
                        .parse()
                        .expect("protocol version header value should be valid"),
                );
                request.headers_mut().insert(
                    "x-node-name",
                    format!("recovery-node-{worker_index}-{current}")
                        .parse()
                        .map_err(|err| format!("failed to build x-node-name header: {err}"))?,
                );

                let started = Instant::now();
                let success =
                    match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(request))
                        .await
                    {
                        Ok(Ok((mut ws, _))) => {
                            tokio::time::sleep(hold).await;
                            let _ = ws.close(None).await;
                            while let Some(message) = ws.next().await {
                                if matches!(message, Ok(Message::Close(_))) || message.is_err() {
                                    break;
                                }
                            }
                            true
                        }
                        _ => false,
                    };
                if success && matches!(current_phase, FaultPhase::Post) {
                    if let Some(restart_started_at) = *restart_started.lock().await {
                        let elapsed = restart_started_at.elapsed().as_millis() as u64;
                        let _ = recovered_after_restart_ms.compare_exchange(
                            0,
                            elapsed,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                    }
                }
                collector
                    .record(current_phase, success, started.elapsed())
                    .await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<(), String>(())
        });
    }

    while let Some(result) = workers.join_next().await {
        result
            .map_err(|err| format!("tunnel recovery worker task failed: {err}"))?
            .map_err(|err| format!("tunnel recovery worker failed: {err}"))?;
    }
    let _ = done_tx.send(());
    restart_task
        .await
        .map_err(|err| format!("tunnel restart task failed: {err}"))?
        .map_err(std::io::Error::other)?;

    Ok(collector
        .summarize(
            load_optional_atomic_u64(&recovered_after_restart_ms),
            runtime_sampler.snapshot(),
        )
        .await)
}

async fn start_tunnel_on_port_retry(
    config: TunnelHarnessConfig,
    port: u16,
) -> Result<TunnelHarness, String> {
    let mut attempts = 0usize;
    loop {
        match TunnelHarness::start_on_port(config.clone(), port).await {
            Ok(tunnel) => return Ok(tunnel),
            Err(err) => {
                attempts += 1;
                if attempts >= 20 {
                    return Err(format!(
                        "failed to restart tunnel on fixed port {port}: {err}"
                    ));
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

fn classify_phase(value: usize) -> FaultPhase {
    match value {
        0 => FaultPhase::Pre,
        1 => FaultPhase::During,
        _ => FaultPhase::Post,
    }
}

fn load_optional_atomic_u64(value: &AtomicU64) -> Option<u64> {
    match value.load(Ordering::Acquire) {
        0 => None,
        millis => Some(millis),
    }
}

fn summarize_latencies(latencies: &[u64]) -> (u64, u64, u64, u64, u64) {
    if latencies.is_empty() {
        return (0, 0, 0, 0, 0);
    }
    let max_ms = *latencies.last().unwrap_or(&0);
    let mean_ms = latencies.iter().sum::<u64>() / latencies.len() as u64;
    let p50_ms = percentile(latencies, 50);
    let p95_ms = percentile(latencies, 95);
    let p99_ms = percentile(latencies, 99);
    (p50_ms, p95_ms, p99_ms, max_ms, mean_ms)
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
) -> Result<FailureRecoveryBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = FailureRecoveryBaselineConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--redis-attempts" => {
                config.redis_attempts = next_value(&mut iter, "--redis-attempts")?.parse()?
            }
            "--redis-concurrency" => {
                config.redis_concurrency = next_value(&mut iter, "--redis-concurrency")?.parse()?
            }
            "--redis-restart-delay-ms" => {
                config.redis_restart_delay = Duration::from_millis(
                    next_value(&mut iter, "--redis-restart-delay-ms")?.parse()?,
                )
            }
            "--redis-downtime-ms" => {
                config.redis_downtime =
                    Duration::from_millis(next_value(&mut iter, "--redis-downtime-ms")?.parse()?)
            }
            "--postgres-statement-timeout-ms" => {
                config.postgres_statement_timeout = Duration::from_millis(
                    next_value(&mut iter, "--postgres-statement-timeout-ms")?.parse()?,
                )
            }
            "--postgres-sleep-ms" => {
                config.postgres_sleep =
                    Duration::from_millis(next_value(&mut iter, "--postgres-sleep-ms")?.parse()?)
            }
            "--tunnel-attempts" => {
                config.tunnel_attempts = next_value(&mut iter, "--tunnel-attempts")?.parse()?
            }
            "--tunnel-concurrency" => {
                config.tunnel_concurrency =
                    next_value(&mut iter, "--tunnel-concurrency")?.parse()?
            }
            "--tunnel-hold-ms" => {
                config.tunnel_hold =
                    Duration::from_millis(next_value(&mut iter, "--tunnel-hold-ms")?.parse()?)
            }
            "--tunnel-restart-delay-ms" => {
                config.tunnel_restart_delay = Duration::from_millis(
                    next_value(&mut iter, "--tunnel-restart-delay-ms")?.parse()?,
                )
            }
            "--tunnel-downtime-ms" => {
                config.tunnel_downtime =
                    Duration::from_millis(next_value(&mut iter, "--tunnel-downtime-ms")?.parse()?)
            }
            "--timeout-ms" => {
                config.timeout =
                    Duration::from_millis(next_value(&mut iter, "--timeout-ms")?.parse()?)
            }
            "--redis-url" => config.redis_url = Some(next_value(&mut iter, "--redis-url")?),
            "--postgres-url" => {
                config.postgres_url = Some(next_value(&mut iter, "--postgres-url")?)
            }
            "--output" => {
                config.output_path = Some(PathBuf::from(next_value(&mut iter, "--output")?))
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    validate_config(&config)?;
    Ok(config)
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    iter.next()
        .ok_or_else(|| format!("missing value for {flag}").into())
}

fn validate_config(
    config: &FailureRecoveryBaselineConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    if config.redis_attempts == 0
        || config.redis_concurrency == 0
        || config.postgres_statement_timeout.is_zero()
        || config.postgres_sleep.is_zero()
        || config.tunnel_attempts == 0
        || config.tunnel_concurrency == 0
        || config.timeout.is_zero()
    {
        return Err("all failure recovery baseline numeric settings must be positive".into());
    }
    Ok(())
}
