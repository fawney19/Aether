use std::path::PathBuf;
use std::time::{Duration, Instant};

use aether_loadtools::{init_load_runtime_for, ManagedRedisServer};
use aether_runtime_state::{
    RedisClientConfig, RedisConsumerGroup, RedisConsumerName, RedisStreamName,
    RedisStreamReclaimConfig, RedisStreamRunner, RedisStreamRunnerConfig,
};
use serde::Serialize;

#[derive(Debug, Clone)]
struct RedisWorkerBaselineConfig {
    append_total: usize,
    append_concurrency: usize,
    reclaim_total: usize,
    reclaim_min_idle: Duration,
    output_path: Option<PathBuf>,
    redis_url: Option<String>,
}

impl Default for RedisWorkerBaselineConfig {
    fn default() -> Self {
        Self {
            append_total: 1_000,
            append_concurrency: 20,
            reclaim_total: 128,
            reclaim_min_idle: Duration::from_millis(100),
            output_path: None,
            redis_url: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct OperationSummary {
    total_calls: usize,
    total_items: usize,
    failed_calls: usize,
    p50_ms: u64,
    p95_ms: u64,
    max_ms: u64,
    mean_ms: u64,
}

#[derive(Debug, Serialize)]
struct RedisWorkerBaselineReport {
    suite: &'static str,
    redis_url: String,
    append: OperationSummary,
    read_group: OperationSummary,
    reclaim: OperationSummary,
    ack: OperationSummary,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_load_runtime_for("redis-worker-baseline");
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
    config: &RedisWorkerBaselineConfig,
) -> Result<RedisWorkerBaselineReport, Box<dyn std::error::Error>> {
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
        .expect("redis url should be resolved");

    let redis_config = RedisClientConfig {
        url: redis_url.clone(),
        key_prefix: Some(format!("aether-baseline-{}", std::process::id())),
    };
    let keyspace = redis_config.keyspace();
    let stream = keyspace.stream_name("worker-baseline");
    let group = RedisConsumerGroup("worker-group".to_string());
    let consumer_a = RedisConsumerName("consumer-a".to_string());
    let consumer_b = RedisConsumerName("consumer-b".to_string());
    let runner = RedisStreamRunner::from_config(
        redis_config,
        RedisStreamRunnerConfig {
            command_timeout_ms: Some(2_000),
            read_block_ms: Some(10),
            read_count: 64,
        },
    )
    .await?;
    runner
        .ensure_consumer_group(&stream, &group, "0-0")
        .await
        .map_err(std::io::Error::other)?;

    let append = benchmark_append(&runner, &stream, config).await?;
    let (read_group, drained_ids) =
        benchmark_read_group(&runner, &stream, &group, &consumer_a, config).await?;
    let ack_read = benchmark_ack(&runner, &stream, &group, &drained_ids).await?;
    let (reclaim, reclaimed_ids) =
        benchmark_reclaim(&runner, &stream, &group, &consumer_a, &consumer_b, config).await?;
    let ack_reclaim = benchmark_ack(&runner, &stream, &group, &reclaimed_ids).await?;

    Ok(RedisWorkerBaselineReport {
        suite: "redis_worker_baseline",
        redis_url,
        append,
        read_group,
        reclaim,
        ack: combine_summaries(&ack_read, &ack_reclaim),
    })
}

async fn benchmark_append(
    runner: &RedisStreamRunner,
    stream: &RedisStreamName,
    config: &RedisWorkerBaselineConfig,
) -> Result<OperationSummary, Box<dyn std::error::Error>> {
    let next = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let latencies = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(
        config.append_total,
    )));
    let failed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut tasks = tokio::task::JoinSet::new();

    for _ in 0..config.append_concurrency {
        let runner = runner.clone();
        let stream = stream.clone();
        let next = next.clone();
        let latencies = latencies.clone();
        let failed = failed.clone();
        let total = config.append_total;
        tasks.spawn(async move {
            loop {
                let current = next.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                if current >= total {
                    break;
                }
                let started = Instant::now();
                let result = runner
                    .append_json(
                        &stream,
                        "payload",
                        &serde_json::json!({
                            "job_id": current,
                            "kind": "baseline",
                        }),
                    )
                    .await;
                latencies
                    .lock()
                    .await
                    .push(started.elapsed().as_millis() as u64);
                if result.is_err() {
                    failed.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                }
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.map_err(std::io::Error::other)?;
    }

    let samples = latencies.lock().await.clone();
    Ok(summarize_operation(
        samples,
        config.append_total,
        failed.load(std::sync::atomic::Ordering::Acquire),
    ))
}

async fn benchmark_read_group(
    runner: &RedisStreamRunner,
    stream: &RedisStreamName,
    group: &RedisConsumerGroup,
    consumer: &RedisConsumerName,
    config: &RedisWorkerBaselineConfig,
) -> Result<(OperationSummary, Vec<String>), Box<dyn std::error::Error>> {
    let mut latencies = Vec::new();
    let mut failed = 0usize;
    let mut ids = Vec::with_capacity(config.append_total);

    while ids.len() < config.append_total {
        let started = Instant::now();
        match runner.read_group(stream, group, consumer).await {
            Ok(entries) => {
                latencies.push(started.elapsed().as_millis() as u64);
                ids.extend(entries.into_iter().map(|entry| entry.id));
            }
            Err(_) => {
                latencies.push(started.elapsed().as_millis() as u64);
                failed += 1;
            }
        }
    }

    let summary = summarize_operation(latencies, ids.len(), failed);
    Ok((summary, ids))
}

async fn benchmark_reclaim(
    runner: &RedisStreamRunner,
    stream: &RedisStreamName,
    group: &RedisConsumerGroup,
    consumer_a: &RedisConsumerName,
    consumer_b: &RedisConsumerName,
    config: &RedisWorkerBaselineConfig,
) -> Result<(OperationSummary, Vec<String>), Box<dyn std::error::Error>> {
    for index in 0..config.reclaim_total {
        runner
            .append_json(
                stream,
                "payload",
                &serde_json::json!({
                    "job_id": format!("reclaim-{index}"),
                    "kind": "baseline",
                }),
            )
            .await
            .map_err(std::io::Error::other)?;
    }

    let mut pending_ids = Vec::with_capacity(config.reclaim_total);
    while pending_ids.len() < config.reclaim_total {
        let entries = runner
            .read_group(stream, group, consumer_a)
            .await
            .map_err(std::io::Error::other)?;
        pending_ids.extend(entries.into_iter().map(|entry| entry.id));
    }

    tokio::time::sleep(config.reclaim_min_idle + Duration::from_millis(20)).await;

    let mut latencies = Vec::new();
    let mut failed = 0usize;
    let mut reclaimed_ids = Vec::with_capacity(config.reclaim_total);
    let mut next_start_id = "0-0".to_string();
    while reclaimed_ids.len() < config.reclaim_total {
        let started = Instant::now();
        match runner
            .claim_stale(
                stream,
                group,
                consumer_b,
                &next_start_id,
                RedisStreamReclaimConfig {
                    min_idle_ms: config.reclaim_min_idle.as_millis() as u64,
                    count: 64,
                },
            )
            .await
        {
            Ok(result) => {
                latencies.push(started.elapsed().as_millis() as u64);
                next_start_id = result.next_start_id;
                reclaimed_ids.extend(result.entries.into_iter().map(|entry| entry.id));
                if next_start_id == "0-0" && reclaimed_ids.len() < config.reclaim_total {
                    failed += 1;
                    break;
                }
            }
            Err(_) => {
                latencies.push(started.elapsed().as_millis() as u64);
                failed += 1;
            }
        }
    }

    let summary = summarize_operation(latencies, reclaimed_ids.len(), failed);
    Ok((summary, reclaimed_ids))
}

async fn benchmark_ack(
    runner: &RedisStreamRunner,
    stream: &RedisStreamName,
    group: &RedisConsumerGroup,
    ids: &[String],
) -> Result<OperationSummary, Box<dyn std::error::Error>> {
    let mut latencies = Vec::new();
    let mut failed = 0usize;
    let mut acked = 0usize;
    for chunk in ids.chunks(64) {
        let started = Instant::now();
        match runner.ack(stream, group, chunk).await {
            Ok(count) => {
                latencies.push(started.elapsed().as_millis() as u64);
                acked += count;
            }
            Err(_) => {
                latencies.push(started.elapsed().as_millis() as u64);
                failed += 1;
            }
        }
    }
    Ok(summarize_operation(latencies, acked, failed))
}

fn summarize_operation(
    latencies: Vec<u64>,
    total_items: usize,
    failed_calls: usize,
) -> OperationSummary {
    if latencies.is_empty() {
        return OperationSummary {
            total_calls: 0,
            total_items,
            failed_calls,
            p50_ms: 0,
            p95_ms: 0,
            max_ms: 0,
            mean_ms: 0,
        };
    }
    let mut sorted = latencies;
    sorted.sort_unstable();
    let total_calls = sorted.len();
    let max_ms = *sorted.last().unwrap_or(&0);
    let mean_ms = sorted.iter().sum::<u64>() / total_calls as u64;
    let p50_ms = percentile(&sorted, 50);
    let p95_ms = percentile(&sorted, 95);
    OperationSummary {
        total_calls,
        total_items,
        failed_calls,
        p50_ms,
        p95_ms,
        max_ms,
        mean_ms,
    }
}

fn combine_summaries(first: &OperationSummary, second: &OperationSummary) -> OperationSummary {
    let total_calls = first.total_calls + second.total_calls;
    let total_items = first.total_items + second.total_items;
    let failed_calls = first.failed_calls + second.failed_calls;
    let max_ms = first.max_ms.max(second.max_ms);
    let mean_ms = if total_calls == 0 {
        0
    } else {
        ((first.mean_ms * first.total_calls as u64) + (second.mean_ms * second.total_calls as u64))
            / total_calls as u64
    };
    OperationSummary {
        total_calls,
        total_items,
        failed_calls,
        p50_ms: first.p50_ms.min(second.p50_ms),
        p95_ms: first.p95_ms.max(second.p95_ms),
        max_ms,
        mean_ms,
    }
}

fn percentile(latencies: &[u64], percentile: u8) -> u64 {
    if latencies.is_empty() {
        return 0;
    }
    let last_index = latencies.len() - 1;
    let rank = ((last_index as f64) * (percentile as f64 / 100.0)).round() as usize;
    latencies[rank.min(last_index)]
}

fn parse_args(args: Vec<String>) -> Result<RedisWorkerBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = RedisWorkerBaselineConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--append-total" => {
                config.append_total = next_value(&mut iter, "--append-total")?.parse()?
            }
            "--append-concurrency" => {
                config.append_concurrency =
                    next_value(&mut iter, "--append-concurrency")?.parse()?
            }
            "--reclaim-total" => {
                config.reclaim_total = next_value(&mut iter, "--reclaim-total")?.parse()?
            }
            "--reclaim-min-idle-ms" => {
                config.reclaim_min_idle =
                    Duration::from_millis(next_value(&mut iter, "--reclaim-min-idle-ms")?.parse()?)
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
        "usage: cargo run -p aether-loadtools --bin redis_worker_baseline -- [--append-total 1000] [--append-concurrency 20] [--reclaim-total 128] [--reclaim-min-idle-ms 100] [--redis-url redis://127.0.0.1:6379/0] [--output /tmp/redis_worker_baseline.json]"
    );
}
