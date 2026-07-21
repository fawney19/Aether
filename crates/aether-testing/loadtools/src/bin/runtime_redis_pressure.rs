use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_loadtools::{init_load_runtime_for, ManagedRedisServer};
use aether_runtime_state::{
    DataLayerError, RedisClientConfig, RedisRuntimeDiagnostics, RuntimeQueueStore,
    RuntimeSemaphoreConfig, RuntimeState,
};
use futures_util::stream::{self, StreamExt};
use serde::Serialize;

#[derive(Debug, Clone)]
struct RuntimeRedisPressureConfig {
    kv_total: usize,
    kv_concurrency: usize,
    lock_total: usize,
    lock_concurrency: usize,
    semaphore_total: usize,
    semaphore_concurrency: usize,
    stream_total: usize,
    stream_concurrency: usize,
    blocking_probe_total: usize,
    blocking_probe_concurrency: usize,
    command_timeout_ms: u64,
    output_path: Option<PathBuf>,
    redis_url: Option<String>,
}

impl Default for RuntimeRedisPressureConfig {
    fn default() -> Self {
        Self {
            kv_total: 20_000,
            kv_concurrency: 200,
            lock_total: 10_000,
            lock_concurrency: 100,
            semaphore_total: 5_000,
            semaphore_concurrency: 100,
            stream_total: 10_000,
            stream_concurrency: 100,
            blocking_probe_total: 1_000,
            blocking_probe_concurrency: 100,
            command_timeout_ms: 2_000,
            output_path: None,
            redis_url: None,
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
    p99_ms: u64,
    max_ms: u64,
    mean_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeRedisPressureReport {
    suite: &'static str,
    redis_url: String,
    total_connections_before: Option<u64>,
    total_connections_after: Option<u64>,
    total_connections_delta: Option<i64>,
    connected_clients_after: Option<u64>,
    diagnostics_after: RedisRuntimeDiagnostics,
    kv: OperationSummary,
    lock: OperationSummary,
    semaphore: OperationSummary,
    stream_append: OperationSummary,
    stream_read_group: OperationSummary,
    stream_ack: OperationSummary,
    blocking_fast_lane_probe: OperationSummary,
}

#[derive(Default)]
struct SummaryCollector {
    latencies_ms: tokio::sync::Mutex<Vec<u64>>,
    total_calls: AtomicUsize,
    total_items: AtomicUsize,
    failed_calls: AtomicUsize,
}

impl SummaryCollector {
    async fn record(&self, elapsed: Duration, items: usize, failed: bool) {
        self.latencies_ms
            .lock()
            .await
            .push(elapsed.as_millis() as u64);
        self.total_calls.fetch_add(1, Ordering::AcqRel);
        self.total_items.fetch_add(items, Ordering::AcqRel);
        if failed {
            self.failed_calls.fetch_add(1, Ordering::AcqRel);
        }
    }

    async fn summarize(&self) -> OperationSummary {
        let mut latencies = self.latencies_ms.lock().await.clone();
        latencies.sort_unstable();
        let total_calls = self.total_calls.load(Ordering::Acquire);
        OperationSummary {
            total_calls,
            total_items: self.total_items.load(Ordering::Acquire),
            failed_calls: self.failed_calls.load(Ordering::Acquire),
            p50_ms: percentile(&latencies, 50),
            p95_ms: percentile(&latencies, 95),
            p99_ms: percentile(&latencies, 99),
            max_ms: latencies.last().copied().unwrap_or_default(),
            mean_ms: if total_calls == 0 {
                0
            } else {
                latencies.iter().sum::<u64>() / total_calls as u64
            },
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_load_runtime_for("runtime-redis-pressure");
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
    config: &RuntimeRedisPressureConfig,
) -> Result<RuntimeRedisPressureReport, Box<dyn std::error::Error>> {
    let managed_redis = if config.redis_url.is_none() {
        Some(ManagedRedisServer::start().await?)
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
    let runtime = Arc::new(
        RuntimeState::redis(
            RedisClientConfig {
                url: redis_url.clone(),
                key_prefix: Some(format!("aether-runtime-pressure-{}", std::process::id())),
            },
            Some(config.command_timeout_ms),
        )
        .await?,
    );
    let before = runtime
        .redis_diagnostics()
        .await?
        .expect("redis diagnostics should be available");

    let kv = benchmark_kv(runtime.clone(), config).await;
    let lock = benchmark_lock(runtime.clone(), config).await;
    let semaphore = benchmark_semaphore(runtime.clone(), config).await?;
    let (stream_append, stream_read_group, stream_ack) =
        benchmark_stream(runtime.clone(), config).await;
    let blocking_fast_lane_probe = benchmark_blocking_fast_lane(runtime.clone(), config).await?;

    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = runtime
        .redis_diagnostics()
        .await?
        .expect("redis diagnostics should be available");
    let total_connections_delta = match (
        before.total_connections_received,
        after.total_connections_received,
    ) {
        (Some(before), Some(after)) => Some(after as i64 - before as i64),
        _ => None,
    };

    Ok(RuntimeRedisPressureReport {
        suite: "runtime_redis_pressure",
        redis_url,
        total_connections_before: before.total_connections_received,
        total_connections_after: after.total_connections_received,
        total_connections_delta,
        connected_clients_after: after.connected_clients,
        diagnostics_after: after,
        kv,
        lock,
        semaphore,
        stream_append,
        stream_read_group,
        stream_ack,
        blocking_fast_lane_probe,
    })
}

async fn benchmark_kv(
    runtime: Arc<RuntimeState>,
    config: &RuntimeRedisPressureConfig,
) -> OperationSummary {
    let collector = Arc::new(SummaryCollector::default());
    stream::iter(0..config.kv_total)
        .for_each_concurrent(config.kv_concurrency, |index| {
            let runtime = runtime.clone();
            let collector = collector.clone();
            async move {
                let started = Instant::now();
                let value = format!("value-{index}");
                let key = format!("pressure:kv:{index}");
                let result = async {
                    runtime
                        .kv_set(&key, value.clone(), Some(Duration::from_secs(60)))
                        .await?;
                    let actual = runtime.kv_get(&key).await?;
                    if actual.as_deref() != Some(value.as_str()) {
                        return Err(DataLayerError::UnexpectedValue(format!(
                            "kv pressure mismatch for {key}"
                        )));
                    }
                    runtime.kv_delete(&key).await?;
                    Ok::<usize, DataLayerError>(3)
                }
                .await;
                let failed = result.is_err();
                collector
                    .record(started.elapsed(), result.unwrap_or(0), failed)
                    .await;
            }
        })
        .await;
    collector.summarize().await
}

async fn benchmark_lock(
    runtime: Arc<RuntimeState>,
    config: &RuntimeRedisPressureConfig,
) -> OperationSummary {
    let collector = Arc::new(SummaryCollector::default());
    stream::iter(0..config.lock_total)
        .for_each_concurrent(config.lock_concurrency, |index| {
            let runtime = runtime.clone();
            let collector = collector.clone();
            async move {
                let started = Instant::now();
                let key = format!("pressure:lock:{index}");
                let owner = format!("owner-{index}");
                let result = async {
                    let Some(lease) = runtime
                        .lock_try_acquire(&key, &owner, Duration::from_secs(30))
                        .await?
                    else {
                        return Err(DataLayerError::UnexpectedValue(format!(
                            "lock pressure acquire returned none for {key}"
                        )));
                    };
                    if !runtime.lock_renew(&lease, Duration::from_secs(30)).await? {
                        return Err(DataLayerError::UnexpectedValue(format!(
                            "lock pressure renew returned false for {key}"
                        )));
                    }
                    if !runtime.lock_release(&lease).await? {
                        return Err(DataLayerError::UnexpectedValue(format!(
                            "lock pressure release returned false for {key}"
                        )));
                    }
                    Ok::<usize, DataLayerError>(3)
                }
                .await;
                let failed = result.is_err();
                collector
                    .record(started.elapsed(), result.unwrap_or(0), failed)
                    .await;
            }
        })
        .await;
    collector.summarize().await
}

async fn benchmark_semaphore(
    runtime: Arc<RuntimeState>,
    config: &RuntimeRedisPressureConfig,
) -> Result<OperationSummary, Box<dyn std::error::Error>> {
    let collector = Arc::new(SummaryCollector::default());
    let semaphore = Arc::new(runtime.semaphore(
        "redis_pressure",
        config.semaphore_concurrency.saturating_mul(4).max(1),
        RuntimeSemaphoreConfig {
            lease_ttl_ms: 10_000,
            renew_interval_ms: 5_000,
            command_timeout_ms: Some(config.command_timeout_ms),
        },
    )?);
    stream::iter(0..config.semaphore_total)
        .for_each_concurrent(config.semaphore_concurrency, |_| {
            let semaphore = semaphore.clone();
            let collector = collector.clone();
            async move {
                let started = Instant::now();
                let result = semaphore.try_acquire().await;
                let failed = result.is_err();
                drop(result);
                collector
                    .record(started.elapsed(), usize::from(!failed), failed)
                    .await;
            }
        })
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(collector.summarize().await)
}

async fn benchmark_stream(
    runtime: Arc<RuntimeState>,
    config: &RuntimeRedisPressureConfig,
) -> (OperationSummary, OperationSummary, OperationSummary) {
    let stream_name = "pressure-stream";
    let group = "pressure-workers";
    let consumer = "consumer-a";
    RuntimeQueueStore::ensure_consumer_group(runtime.as_ref(), stream_name, group, "0-0")
        .await
        .expect("stream consumer group should initialize");

    let append = Arc::new(SummaryCollector::default());
    stream::iter(0..config.stream_total)
        .for_each_concurrent(config.stream_concurrency, |index| {
            let runtime = runtime.clone();
            let append = append.clone();
            async move {
                let started = Instant::now();
                let mut fields = BTreeMap::new();
                fields.insert("payload".to_string(), format!("stream-value-{index}"));
                let result = RuntimeQueueStore::append_fields_with_maxlen(
                    runtime.as_ref(),
                    stream_name,
                    &fields,
                    Some(config.stream_total.saturating_mul(2)),
                )
                .await;
                append
                    .record(
                        started.elapsed(),
                        usize::from(result.is_ok()),
                        result.is_err(),
                    )
                    .await;
            }
        })
        .await;

    let read = SummaryCollector::default();
    let mut ids = Vec::with_capacity(config.stream_total);
    while ids.len() < config.stream_total {
        let started = Instant::now();
        match RuntimeQueueStore::read_group(
            runtime.as_ref(),
            stream_name,
            group,
            consumer,
            128,
            Some(10),
        )
        .await
        {
            Ok(entries) => {
                let item_count = entries.len();
                ids.extend(entries.into_iter().map(|entry| entry.id));
                read.record(started.elapsed(), item_count, false).await;
            }
            Err(_) => {
                read.record(started.elapsed(), 0, true).await;
            }
        }
    }

    let ack = SummaryCollector::default();
    for chunk in ids.chunks(128) {
        let started = Instant::now();
        match RuntimeQueueStore::ack(runtime.as_ref(), stream_name, group, chunk).await {
            Ok(count) => ack.record(started.elapsed(), count, false).await,
            Err(_) => ack.record(started.elapsed(), 0, true).await,
        }
    }

    (
        append.summarize().await,
        read.summarize().await,
        ack.summarize().await,
    )
}

async fn benchmark_blocking_fast_lane(
    runtime: Arc<RuntimeState>,
    config: &RuntimeRedisPressureConfig,
) -> Result<OperationSummary, Box<dyn std::error::Error>> {
    let stream_name = "pressure-blocking-empty";
    let group = "pressure-blocking-workers";
    RuntimeQueueStore::ensure_consumer_group(runtime.as_ref(), stream_name, group, "0-0").await?;
    let blocking_runtime = runtime.clone();
    let blocking = tokio::spawn(async move {
        RuntimeQueueStore::read_group(
            blocking_runtime.as_ref(),
            stream_name,
            group,
            "blocked-consumer",
            1,
            Some(1_000),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let summary = benchmark_fast_lane_probe(runtime, config).await;
    let _ = blocking.await?;
    Ok(summary)
}

async fn benchmark_fast_lane_probe(
    runtime: Arc<RuntimeState>,
    config: &RuntimeRedisPressureConfig,
) -> OperationSummary {
    let collector = Arc::new(SummaryCollector::default());
    stream::iter(0..config.blocking_probe_total)
        .for_each_concurrent(config.blocking_probe_concurrency, |index| {
            let runtime = runtime.clone();
            let collector = collector.clone();
            async move {
                let started = Instant::now();
                let key = format!("pressure:blocking-probe:{index}");
                let result = async {
                    runtime
                        .kv_set(&key, "ok", Some(Duration::from_secs(30)))
                        .await?;
                    let ok = runtime.kv_get(&key).await?.as_deref() == Some("ok");
                    if !ok {
                        return Err(DataLayerError::UnexpectedValue(format!(
                            "blocking fast lane probe mismatch for {key}"
                        )));
                    }
                    Ok::<usize, DataLayerError>(2)
                }
                .await;
                let failed = result.is_err();
                collector
                    .record(started.elapsed(), result.unwrap_or(0), failed)
                    .await;
            }
        })
        .await;
    collector.summarize().await
}

fn percentile(latencies: &[u64], percentile: u8) -> u64 {
    if latencies.is_empty() {
        return 0;
    }
    let last_index = latencies.len() - 1;
    let rank = ((last_index as f64) * (percentile as f64 / 100.0)).round() as usize;
    latencies[rank.min(last_index)]
}

fn parse_args(args: Vec<String>) -> Result<RuntimeRedisPressureConfig, Box<dyn std::error::Error>> {
    let mut config = RuntimeRedisPressureConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--kv-total" => config.kv_total = next_value(&mut iter, "--kv-total")?.parse()?,
            "--kv-concurrency" => {
                config.kv_concurrency = next_value(&mut iter, "--kv-concurrency")?.parse()?
            }
            "--lock-total" => config.lock_total = next_value(&mut iter, "--lock-total")?.parse()?,
            "--lock-concurrency" => {
                config.lock_concurrency = next_value(&mut iter, "--lock-concurrency")?.parse()?
            }
            "--semaphore-total" => {
                config.semaphore_total = next_value(&mut iter, "--semaphore-total")?.parse()?
            }
            "--semaphore-concurrency" => {
                config.semaphore_concurrency =
                    next_value(&mut iter, "--semaphore-concurrency")?.parse()?
            }
            "--stream-total" => {
                config.stream_total = next_value(&mut iter, "--stream-total")?.parse()?
            }
            "--stream-concurrency" => {
                config.stream_concurrency =
                    next_value(&mut iter, "--stream-concurrency")?.parse()?
            }
            "--blocking-probe-total" => {
                config.blocking_probe_total =
                    next_value(&mut iter, "--blocking-probe-total")?.parse()?
            }
            "--blocking-probe-concurrency" => {
                config.blocking_probe_concurrency =
                    next_value(&mut iter, "--blocking-probe-concurrency")?.parse()?
            }
            "--command-timeout-ms" => {
                config.command_timeout_ms =
                    next_value(&mut iter, "--command-timeout-ms")?.parse()?
            }
            "--redis-url" => config.redis_url = Some(next_value(&mut iter, "--redis-url")?),
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
        "usage: cargo run -p aether-loadtools --bin runtime_redis_pressure -- [--kv-total 20000] [--kv-concurrency 200] [--lock-total 10000] [--lock-concurrency 100] [--semaphore-total 5000] [--semaphore-concurrency 100] [--stream-total 10000] [--stream-concurrency 100] [--blocking-probe-total 1000] [--blocking-probe-concurrency 100] [--command-timeout-ms 2000] [--redis-url redis://127.0.0.1:6379/0] [--output /tmp/runtime_redis_pressure.json]"
    );
}
