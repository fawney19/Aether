// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use aether_runtime_state::{
    RedisClientConfig, RuntimeSemaphore, RuntimeSemaphoreConfig, RuntimeState,
};
use aether_testkit::{
    init_test_runtime_for, run_multi_url_http_load_probe, BenchmarkRuntimeSampler,
    BenchmarkRuntimeSnapshot, ExecutionRuntimeHarness, ExecutionRuntimeHarnessConfig,
    GatewayHarness, GatewayHarnessConfig, HttpLoadProbeConfig, HttpLoadProbeResponseMode,
    ManagedRedisServer, MultiUrlHttpLoadProbeResult, SpawnedServer, TunnelHarness,
    TunnelHarnessConfig, GATEWAY_HARNESS_API_KEY,
};
use axum::body::to_bytes;
use axum::extract::Request;
use axum::response::IntoResponse;
use axum::routing::any;
use axum::{Json, Router};
use futures_util::StreamExt;
use reqwest::Method;
use serde::Serialize;
use serde_json::json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";

#[derive(Debug, Clone)]
struct MultiInstanceAdmissionBaselineConfig {
    gateway_requests: usize,
    gateway_concurrency: usize,
    execution_runtime_requests: usize,
    execution_runtime_concurrency: usize,
    tunnel_attempts: usize,
    tunnel_concurrency: usize,
    tunnel_hold: Duration,
    upstream_delay: Duration,
    request_limit: usize,
    tunnel_request_limit: usize,
    timeout: Duration,
    output_path: Option<PathBuf>,
    redis_url: Option<String>,
}

impl Default for MultiInstanceAdmissionBaselineConfig {
    fn default() -> Self {
        Self {
            gateway_requests: 200,
            gateway_concurrency: 20,
            execution_runtime_requests: 200,
            execution_runtime_concurrency: 20,
            tunnel_attempts: 40,
            tunnel_concurrency: 10,
            tunnel_hold: Duration::from_millis(100),
            upstream_delay: Duration::from_millis(100),
            request_limit: 8,
            tunnel_request_limit: 4,
            timeout: Duration::from_secs(10),
            output_path: None,
            redis_url: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct MultiInstanceAdmissionBaselineReport {
    suite: &'static str,
    redis_url: String,
    gateway_sync: MultiUrlHttpLoadProbeResult,
    execution_runtime_sync: MultiUrlHttpLoadProbeResult,
    tunnel_proxy: WebSocketAdmissionProbeResult,
}

#[derive(Debug, Clone, Serialize)]
struct WebSocketAdmissionProbeResult {
    target_urls: Vec<String>,
    target_attempt_counts: BTreeMap<String, usize>,
    total_attempts: usize,
    concurrency: usize,
    completed_attempts: usize,
    failed_attempts: usize,
    rejected_attempts: usize,
    successful_attempts: usize,
    p50_ms: u64,
    p95_ms: u64,
    p99_ms: u64,
    max_ms: u64,
    mean_ms: u64,
    status_counts: BTreeMap<u16, usize>,
    runtime: BenchmarkRuntimeSnapshot,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("multi-instance-admission-baseline");
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
    config: &MultiInstanceAdmissionBaselineConfig,
) -> Result<MultiInstanceAdmissionBaselineReport, Box<dyn std::error::Error>> {
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

    let upstream = SpawnedServer::start(build_delayed_upstream(config.upstream_delay)).await?;

    let (gateway_urls, _gateways) =
        start_gateway_pair(&redis_url, upstream.base_url(), config).await?;
    let (execution_runtime_urls, _runtimes) =
        start_execution_runtime_pair(&redis_url, upstream.base_url(), config).await?;
    let (tunnel_urls, _tunnels) = start_tunnel_pair(&redis_url, config).await?;

    let gateway_sync = run_multi_url_http_load_probe(
        &gateway_sync_probe_config(&gateway_urls, config),
        &gateway_urls,
    )
    .await
    .map_err(std::io::Error::other)?;
    let execution_runtime_sync = run_multi_url_http_load_probe(
        &execution_runtime_sync_probe_config(&execution_runtime_urls, upstream.base_url(), config),
        &execution_runtime_urls,
    )
    .await
    .map_err(std::io::Error::other)?;
    let tunnel_proxy = run_tunnel_proxy_connection_probe(&tunnel_urls, config)
        .await
        .map_err(std::io::Error::other)?;

    Ok(MultiInstanceAdmissionBaselineReport {
        suite: "multi_instance_admission_baseline",
        redis_url,
        gateway_sync,
        execution_runtime_sync,
        tunnel_proxy,
    })
}

async fn start_gateway_pair(
    redis_url: &str,
    upstream_base_url: &str,
    config: &MultiInstanceAdmissionBaselineConfig,
) -> Result<(Vec<String>, Vec<GatewayHarness>), Box<dyn std::error::Error>> {
    let gate_a = distributed_request_gate(
        "gateway_requests_distributed",
        config.request_limit,
        redis_url,
        "gateway-a",
    )
    .await?;
    let gate_b = distributed_request_gate(
        "gateway_requests_distributed",
        config.request_limit,
        redis_url,
        "gateway-b",
    )
    .await?;
    let gateway_a = GatewayHarness::start(GatewayHarnessConfig {
        upstream_base_url: upstream_base_url.to_string(),
        data_config: None,
        max_in_flight_requests: None,
        distributed_request_gate: Some(gate_a),
        tunnel_instance_id: None,
        tunnel_relay_base_url: None,
    })
    .await?;
    let gateway_b = GatewayHarness::start(GatewayHarnessConfig {
        upstream_base_url: upstream_base_url.to_string(),
        data_config: None,
        max_in_flight_requests: None,
        distributed_request_gate: Some(gate_b),
        tunnel_instance_id: None,
        tunnel_relay_base_url: None,
    })
    .await?;
    Ok((
        vec![
            format!("{}/v1/chat/completions", gateway_a.base_url()),
            format!("{}/v1/chat/completions", gateway_b.base_url()),
        ],
        vec![gateway_a, gateway_b],
    ))
}

async fn start_execution_runtime_pair(
    redis_url: &str,
    upstream_base_url: &str,
    config: &MultiInstanceAdmissionBaselineConfig,
) -> Result<(Vec<String>, Vec<ExecutionRuntimeHarness>), Box<dyn std::error::Error>> {
    let gate_a = distributed_request_gate(
        "execution_runtime_requests_distributed",
        config.request_limit,
        redis_url,
        "execution-runtime-a",
    )
    .await?;
    let gate_b = distributed_request_gate(
        "execution_runtime_requests_distributed",
        config.request_limit,
        redis_url,
        "execution-runtime-b",
    )
    .await?;
    let runtime_a = ExecutionRuntimeHarness::start(ExecutionRuntimeHarnessConfig {
        max_in_flight_requests: None,
        distributed_request_gate: Some(gate_a),
    })
    .await?;
    let runtime_b = ExecutionRuntimeHarness::start(ExecutionRuntimeHarnessConfig {
        max_in_flight_requests: None,
        distributed_request_gate: Some(gate_b),
    })
    .await?;
    let _ = upstream_base_url;
    Ok((
        vec![
            format!("{}/v1/execute/sync", runtime_a.base_url()),
            format!("{}/v1/execute/sync", runtime_b.base_url()),
        ],
        vec![runtime_a, runtime_b],
    ))
}

async fn start_tunnel_pair(
    redis_url: &str,
    config: &MultiInstanceAdmissionBaselineConfig,
) -> Result<(Vec<String>, Vec<TunnelHarness>), Box<dyn std::error::Error>> {
    let gate_a = distributed_request_gate(
        "tunnel_requests_distributed",
        config.tunnel_request_limit,
        redis_url,
        "tunnel-a",
    )
    .await?;
    let gate_b = distributed_request_gate(
        "tunnel_requests_distributed",
        config.tunnel_request_limit,
        redis_url,
        "tunnel-b",
    )
    .await?;
    let tunnel_a = TunnelHarness::start(TunnelHarnessConfig {
        distributed_request_gate: Some(gate_a),
        ..TunnelHarnessConfig::default()
    })
    .await?;
    let tunnel_b = TunnelHarness::start(TunnelHarnessConfig {
        distributed_request_gate: Some(gate_b),
        ..TunnelHarnessConfig::default()
    })
    .await?;
    Ok((
        vec![
            format!(
                "{}{}",
                tunnel_a.base_url().replace("http://", "ws://"),
                PROXY_TUNNEL_PATH
            ),
            format!(
                "{}{}",
                tunnel_b.base_url().replace("http://", "ws://"),
                PROXY_TUNNEL_PATH
            ),
        ],
        vec![tunnel_a, tunnel_b],
    ))
}

async fn distributed_request_gate(
    name: &'static str,
    limit: usize,
    redis_url: &str,
    _instance_id: &str,
) -> Result<RuntimeSemaphore, Box<dyn std::error::Error>> {
    let runtime = RuntimeState::redis(
        RedisClientConfig {
            url: redis_url.to_string(),
            key_prefix: Some(format!("aether-baseline-{}-{name}", std::process::id())),
        },
        Some(1_000),
    )
    .await?;
    Ok(runtime.semaphore(
        name,
        limit,
        RuntimeSemaphoreConfig {
            lease_ttl_ms: 30_000,
            renew_interval_ms: 10_000,
            command_timeout_ms: Some(1_000),
        },
    )?)
}

fn gateway_sync_probe_config(
    urls: &[String],
    config: &MultiInstanceAdmissionBaselineConfig,
) -> HttpLoadProbeConfig {
    let mut probe = chat_probe_config(
        urls[0].clone(),
        config.gateway_requests,
        config.gateway_concurrency,
        config.timeout,
    );
    probe.response_mode = HttpLoadProbeResponseMode::FullBody;
    probe
}

fn execution_runtime_sync_probe_config(
    urls: &[String],
    upstream_base_url: &str,
    config: &MultiInstanceAdmissionBaselineConfig,
) -> HttpLoadProbeConfig {
    let _ = urls;
    HttpLoadProbeConfig {
        url: urls[0].clone(),
        method: Method::POST,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(
            serde_json::to_vec(&execution_plan(format!(
                "{upstream_base_url}/v1/chat/completions"
            )))
            .expect("execution plan should serialize"),
        ),
        total_requests: config.execution_runtime_requests,
        concurrency: config.execution_runtime_concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    }
}

fn chat_probe_config(
    url: String,
    total_requests: usize,
    concurrency: usize,
    timeout: Duration,
) -> HttpLoadProbeConfig {
    HttpLoadProbeConfig {
        url,
        method: Method::POST,
        headers: BTreeMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            (
                "authorization".to_string(),
                format!("Bearer {GATEWAY_HARNESS_API_KEY}"),
            ),
        ]),
        body: Some(
            serde_json::to_vec(&json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": false,
            }))
            .expect("chat body should serialize"),
        ),
        total_requests,
        concurrency,
        timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    }
}

fn execution_plan(url: String) -> ExecutionPlan {
    ExecutionPlan {
        request_id: "multi-instance-sync-request".to_string(),
        candidate_id: Some("multi-instance-sync-candidate".to_string()),
        provider_name: Some("openai".to_string()),
        provider_id: "provider-baseline".to_string(),
        endpoint_id: "endpoint-baseline".to_string(),
        key_id: "key-baseline".to_string(),
        method: "POST".to_string(),
        url,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": false,
        })),
        stream: false,
        client_api_format: "openai:chat".to_string(),
        provider_api_format: "openai:chat".to_string(),
        model_name: Some("gpt-5".to_string()),
        proxy: None,
        transport_profile: None,
        timeouts: Some(ExecutionTimeouts {
            connect_ms: Some(2_000),
            read_ms: Some(10_000),
            first_byte_ms: Some(5_000),
            total_ms: Some(10_000),
            ..ExecutionTimeouts::default()
        }),
    }
}

fn build_delayed_upstream(delay: Duration) -> Router {
    Router::new().route(
        "/v1/chat/completions",
        any(move |request: Request| {
            let delay = delay;
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX)
                    .await
                    .expect("fake upstream body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).unwrap_or_else(|_| json!({}));
                tokio::time::sleep(delay).await;
                Json(json!({
                    "id": "chatcmpl-distributed",
                    "object": "chat.completion",
                    "model": payload.get("model").and_then(|value| value.as_str()).unwrap_or("gpt-5"),
                    "choices": [{"message": {"role": "assistant", "content": "hello"}}]
                }))
                .into_response()
            }
        }),
    )
}

async fn run_tunnel_proxy_connection_probe(
    urls: &[String],
    config: &MultiInstanceAdmissionBaselineConfig,
) -> Result<WebSocketAdmissionProbeResult, String> {
    if urls.is_empty() {
        return Err("tunnel proxy connection probe requires at least one target url".to_string());
    }
    let mut runtime_sampler = BenchmarkRuntimeSampler::new();
    let next_attempt = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let latencies_ms = Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(
        config.tunnel_attempts,
    )));
    let target_attempt_counts = Arc::new(tokio::sync::Mutex::new(BTreeMap::<String, usize>::new()));
    let status_counts = Arc::new(tokio::sync::Mutex::new(BTreeMap::<u16, usize>::new()));
    let failed_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rejected_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let successful_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let completed_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut workers = tokio::task::JoinSet::new();
    for worker_index in 0..config.tunnel_concurrency {
        let urls = urls.to_vec();
        let next_attempt = Arc::clone(&next_attempt);
        let latencies_ms = Arc::clone(&latencies_ms);
        let target_attempt_counts = Arc::clone(&target_attempt_counts);
        let status_counts = Arc::clone(&status_counts);
        let failed_attempts = Arc::clone(&failed_attempts);
        let rejected_attempts = Arc::clone(&rejected_attempts);
        let successful_attempts = Arc::clone(&successful_attempts);
        let completed_attempts = Arc::clone(&completed_attempts);
        let timeout = config.timeout;
        let hold = config.tunnel_hold;
        let total_attempts = config.tunnel_attempts;

        workers.spawn(async move {
            loop {
                let current = next_attempt.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                if current >= total_attempts {
                    break;
                }
                let url = urls[current % urls.len()].clone();
                {
                    let mut counts = target_attempt_counts.lock().await;
                    *counts.entry(url.clone()).or_insert(0) += 1;
                }
                let mut request = url
                    .into_client_request()
                    .map_err(|err| format!("failed to build websocket request: {err}"))?;
                request.headers_mut().insert(
                    "x-node-id",
                    format!("baseline-node-{worker_index}-{current}")
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
                    format!("baseline-node-{worker_index}-{current}")
                        .parse()
                        .map_err(|err| format!("failed to build x-node-name header: {err}"))?,
                );

                let started_at = Instant::now();
                match tokio::time::timeout(timeout, tokio_tungstenite::connect_async(request)).await
                {
                    Ok(Ok((mut ws, _response))) => {
                        {
                            let mut counts = status_counts.lock().await;
                            *counts.entry(101).or_insert(0) += 1;
                        }
                        latencies_ms
                            .lock()
                            .await
                            .push(started_at.elapsed().as_millis() as u64);
                        successful_attempts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        completed_attempts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        tokio::time::sleep(hold).await;
                        let _ = ws.close(None).await;
                        while let Some(message) = ws.next().await {
                            if matches!(message, Ok(Message::Close(_))) || message.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response))) => {
                        let status = response.status().as_u16();
                        {
                            let mut counts = status_counts.lock().await;
                            *counts.entry(status).or_insert(0) += 1;
                        }
                        latencies_ms
                            .lock()
                            .await
                            .push(started_at.elapsed().as_millis() as u64);
                        if status == 503 {
                            rejected_attempts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        } else {
                            failed_attempts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        }
                        completed_attempts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    }
                    Ok(Err(_)) | Err(_) => {
                        latencies_ms
                            .lock()
                            .await
                            .push(started_at.elapsed().as_millis() as u64);
                        failed_attempts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                        completed_attempts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    }
                }
            }
            Ok::<(), String>(())
        });
    }

    while let Some(result) = workers.join_next().await {
        result
            .map_err(|err| format!("tunnel admission worker task failed: {err}"))?
            .map_err(|err| format!("tunnel admission worker failed: {err}"))?;
    }

    let mut latencies = latencies_ms.lock().await.clone();
    latencies.sort_unstable();
    let (p50_ms, p95_ms, p99_ms, max_ms, mean_ms) = summarize_latencies(&latencies);

    let target_attempt_counts = target_attempt_counts.lock().await.clone();
    let status_counts = status_counts.lock().await.clone();
    Ok(WebSocketAdmissionProbeResult {
        target_urls: urls.to_vec(),
        target_attempt_counts,
        total_attempts: config.tunnel_attempts,
        concurrency: config.tunnel_concurrency,
        completed_attempts: completed_attempts.load(std::sync::atomic::Ordering::Acquire),
        failed_attempts: failed_attempts.load(std::sync::atomic::Ordering::Acquire),
        rejected_attempts: rejected_attempts.load(std::sync::atomic::Ordering::Acquire),
        successful_attempts: successful_attempts.load(std::sync::atomic::Ordering::Acquire),
        p50_ms,
        p95_ms,
        p99_ms,
        max_ms,
        mean_ms,
        status_counts,
        runtime: runtime_sampler.snapshot(),
    })
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
) -> Result<MultiInstanceAdmissionBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = MultiInstanceAdmissionBaselineConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--gateway-requests" => {
                config.gateway_requests = next_value(&mut iter, "--gateway-requests")?.parse()?
            }
            "--gateway-concurrency" => {
                config.gateway_concurrency =
                    next_value(&mut iter, "--gateway-concurrency")?.parse()?
            }
            "--execution-runtime-requests" | "--executor-requests" => {
                config.execution_runtime_requests = next_value(&mut iter, arg.as_str())?.parse()?
            }
            "--execution-runtime-concurrency" | "--executor-concurrency" => {
                config.execution_runtime_concurrency =
                    next_value(&mut iter, arg.as_str())?.parse()?
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
            "--upstream-delay-ms" => {
                config.upstream_delay =
                    Duration::from_millis(next_value(&mut iter, "--upstream-delay-ms")?.parse()?)
            }
            "--request-limit" => {
                config.request_limit = next_value(&mut iter, "--request-limit")?.parse()?
            }
            "--tunnel-request-limit" => {
                config.tunnel_request_limit =
                    next_value(&mut iter, "--tunnel-request-limit")?.parse()?
            }
            "--timeout-ms" => {
                config.timeout =
                    Duration::from_millis(next_value(&mut iter, "--timeout-ms")?.parse()?)
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
        "usage: cargo run -p aether-integration-tests --bin multi_instance_admission_baseline -- [--gateway-requests 200] [--gateway-concurrency 20] [--execution-runtime-requests 200] [--execution-runtime-concurrency 20] [--tunnel-attempts 40] [--tunnel-concurrency 10] [--tunnel-hold-ms 100] [--upstream-delay-ms 100] [--request-limit 8] [--tunnel-request-limit 4] [--redis-url redis://127.0.0.1:6379/0] [--output /tmp/multi_instance_admission_baseline.json]"
    );
}
