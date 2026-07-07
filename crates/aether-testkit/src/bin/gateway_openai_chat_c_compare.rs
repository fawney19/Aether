use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_gateway::build_router_with_state;
use aether_gateway::testkit::{build_openai_chat_pressure_state, OpenAiChatPressureStateConfig};
use aether_testkit::{
    fetch_prometheus_samples, run_http_load_probe, run_multi_url_http_load_probe,
    HttpLoadProbeConfig, HttpLoadProbeResponseMode, HttpLoadProbeResult,
    MultiUrlHttpLoadProbeResult, PrometheusSample, SpawnedServer,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use reqwest::Method;
use serde::Serialize;

const DEFAULT_REQUESTS: usize = 10_000;
const DEFAULT_C1K: usize = 1_000;
const DEFAULT_C1W: usize = 10_000;
const DEFAULT_TARGETS: usize = 4;
const DEFAULT_CLIENT_SHARDS: usize = 512;
const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_WARMUP_REQUESTS: usize = 1_000;
const DEFAULT_WARMUP_CONCURRENCY: usize = 50;
const BODY_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone)]
struct Config {
    requests: usize,
    c1k: usize,
    c1w: usize,
    targets: usize,
    client_shards: usize,
    timeout_ms: u64,
    warmup_requests: usize,
    warmup_concurrency: usize,
    first_body_hold_ms: u64,
    output_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            requests: DEFAULT_REQUESTS,
            c1k: DEFAULT_C1K,
            c1w: DEFAULT_C1W,
            targets: DEFAULT_TARGETS,
            client_shards: DEFAULT_CLIENT_SHARDS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            warmup_requests: DEFAULT_WARMUP_REQUESTS,
            warmup_concurrency: DEFAULT_WARMUP_CONCURRENCY,
            first_body_hold_ms: 0,
            output_path: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct CompareReport {
    suite: &'static str,
    gateway_base_url: String,
    gateway_metrics_url: String,
    upstream_targets: Vec<String>,
    config: ReportConfig,
    warmup: Option<HttpLoadProbeResult>,
    c1k: RunReport,
    c1w: RunReport,
    mock_only: MockOnlyCompareReport,
}

#[derive(Debug, Serialize)]
struct ReportConfig {
    requests: usize,
    c1k: usize,
    c1w: usize,
    targets: usize,
    client_shards: usize,
    timeout_ms: u64,
    warmup_requests: usize,
    warmup_concurrency: usize,
    first_body_hold_ms: u64,
}

#[derive(Debug, Serialize)]
struct RunReport {
    label: String,
    load: HttpLoadProbeResult,
    gateway_metrics: GatewayMetricDelta,
    mock_metrics: Vec<MockMetricDelta>,
}

#[derive(Debug, Serialize)]
struct MockOnlyCompareReport {
    c1k: MultiUrlHttpLoadProbeResult,
    c1w: MultiUrlHttpLoadProbeResult,
}

#[derive(Debug, Default, Serialize)]
struct GatewayMetricDelta {
    stream_pre_first_byte_spawn_total: u64,
    request_candidate_queue_dropped_total: u64,
    raw_candidates_scanned_total: u64,
    payload_build_selected_total: u64,
    payload_build_prefetch_avoided_total: u64,
    selected_rank_sum: u64,
    model_directive_cache_hit_total: u64,
    model_directive_cache_miss_total: u64,
    redaction_request_cache_hit_total: u64,
    redaction_request_cache_miss_total: u64,
    target_raw_seen_total: BTreeMap<String, u64>,
    target_preselect_total: BTreeMap<String, u64>,
    target_selected_total: BTreeMap<String, u64>,
    target_max_in_flight: BTreeMap<String, u64>,
    target_saturated_total: BTreeMap<String, u64>,
    stage_deltas: BTreeMap<String, StageMetricDelta>,
}

#[derive(Debug, Default, Serialize)]
struct StageMetricDelta {
    count: u64,
    sum_ms: u64,
    max_ms_after: u64,
    p95_bucket_ms: Option<u64>,
    p99_bucket_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct MockMetricSnapshot {
    requests_total: u64,
    completed_total: u64,
    in_flight: u64,
    max_in_flight: u64,
    first_chunk_yield_total: u64,
    request_body_read_sum_ms: u64,
    request_body_read_max_ms: u64,
    response_header_to_first_chunk_sum_ms: u64,
    response_header_to_first_chunk_max_ms: u64,
}

#[derive(Debug, Serialize)]
struct MockMetricDelta {
    target: String,
    requests_total: u64,
    completed_total: u64,
    in_flight_after: u64,
    max_in_flight_after: u64,
    first_chunk_yield_total: u64,
    request_body_read_sum_ms: u64,
    request_body_read_max_ms_after: u64,
    response_header_to_first_chunk_sum_ms: u64,
    response_header_to_first_chunk_max_ms_after: u64,
}

#[derive(Debug, Default)]
struct MockTargetMetrics {
    requests_total: AtomicU64,
    completed_total: AtomicU64,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    first_chunk_yield_total: AtomicU64,
    request_body_read_sum_ms: AtomicU64,
    request_body_read_max_ms: AtomicU64,
    response_header_to_first_chunk_sum_ms: AtomicU64,
    response_header_to_first_chunk_max_ms: AtomicU64,
}

#[derive(Clone)]
struct MockTargetState {
    label: String,
    metrics: Arc<MockTargetMetrics>,
    first_chunk_delay: Duration,
}

struct CompletionGuard {
    metrics: Option<Arc<MockTargetMetrics>>,
}

impl CompletionGuard {
    fn new(metrics: Arc<MockTargetMetrics>) -> Self {
        Self {
            metrics: Some(metrics),
        }
    }
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if let Some(metrics) = self.metrics.take() {
            record_mock_completed(&metrics);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args(std::env::args().skip(1).collect())?;
    if config.targets == 0 {
        return Err("targets must be positive".into());
    }

    let mut mock_servers = Vec::with_capacity(config.targets);
    let mut mock_metrics = Vec::with_capacity(config.targets);
    for index in 0..config.targets {
        let metrics = Arc::new(MockTargetMetrics::default());
        let state = MockTargetState {
            label: format!("target-{index}"),
            metrics: Arc::clone(&metrics),
            first_chunk_delay: Duration::ZERO,
        };
        let server = SpawnedServer::start(mock_router(state)).await?;
        mock_servers.push(server);
        mock_metrics.push(metrics);
    }

    let upstream_targets = mock_servers
        .iter()
        .map(|server| format!("{}/v1", server.base_url()))
        .collect::<Vec<_>>();
    let mock_chat_urls = mock_servers
        .iter()
        .map(|server| format!("{}/v1/chat/completions", server.base_url()))
        .collect::<Vec<_>>();
    let mock_health_url = format!("{}/health", mock_servers[0].base_url());
    let gateway_state = build_openai_chat_pressure_state(OpenAiChatPressureStateConfig::new(
        upstream_targets.clone(),
    ))
    .map_err(std::io::Error::other)?;
    let gateway = SpawnedServer::start(build_router_with_state(gateway_state)).await?;
    let gateway_url = format!("{}/v1/chat/completions", gateway.base_url());
    let gateway_health_url = format!("{}/_gateway/health", gateway.base_url());
    let gateway_metrics_url = format!("{}/_gateway/metrics", gateway.base_url());

    let warmup = if config.warmup_requests > 0 {
        Some(
            run_http_load_probe(&load_config(
                &gateway_url,
                &gateway_health_url,
                &config,
                config.warmup_requests,
                config.warmup_concurrency,
                0,
            ))
            .await
            .map_err(std::io::Error::other)?,
        )
    } else {
        None
    };

    let before_c1k = fetch_prometheus_samples(&gateway_metrics_url)
        .await
        .map_err(std::io::Error::other)?;
    let mock_before_c1k = snapshot_mock_metrics(&mock_metrics);
    let c1k_load = run_http_load_probe(&load_config(
        &gateway_url,
        &gateway_health_url,
        &config,
        config.requests,
        config.c1k,
        config.first_body_hold_ms,
    ))
    .await
    .map_err(std::io::Error::other)?;
    let after_c1k = fetch_prometheus_samples(&gateway_metrics_url)
        .await
        .map_err(std::io::Error::other)?;
    let mock_after_c1k = snapshot_mock_metrics(&mock_metrics);

    let before_c1w = after_c1k.clone();
    let mock_before_c1w = mock_after_c1k.clone();
    let c1w_load = run_http_load_probe(&load_config(
        &gateway_url,
        &gateway_health_url,
        &config,
        config.requests,
        config.c1w,
        config.first_body_hold_ms,
    ))
    .await
    .map_err(std::io::Error::other)?;
    let after_c1w = fetch_prometheus_samples(&gateway_metrics_url)
        .await
        .map_err(std::io::Error::other)?;
    let mock_after_c1w = snapshot_mock_metrics(&mock_metrics);
    let mock_only_c1k = run_multi_url_http_load_probe(
        &load_config(
            &mock_chat_urls[0],
            &mock_health_url,
            &config,
            config.requests,
            config.c1k,
            config.first_body_hold_ms,
        ),
        &mock_chat_urls,
    )
    .await
    .map_err(std::io::Error::other)?;
    let mock_only_c1w = run_multi_url_http_load_probe(
        &load_config(
            &mock_chat_urls[0],
            &mock_health_url,
            &config,
            config.requests,
            config.c1w,
            config.first_body_hold_ms,
        ),
        &mock_chat_urls,
    )
    .await
    .map_err(std::io::Error::other)?;

    let report = CompareReport {
        suite: "gateway_openai_chat_c_compare",
        gateway_base_url: gateway.base_url().to_string(),
        gateway_metrics_url,
        upstream_targets,
        config: ReportConfig {
            requests: config.requests,
            c1k: config.c1k,
            c1w: config.c1w,
            targets: config.targets,
            client_shards: config.client_shards,
            timeout_ms: config.timeout_ms,
            warmup_requests: config.warmup_requests,
            warmup_concurrency: config.warmup_concurrency,
            first_body_hold_ms: config.first_body_hold_ms,
        },
        warmup,
        c1k: RunReport {
            label: format!("c{}", config.c1k),
            load: c1k_load,
            gateway_metrics: gateway_metric_delta(&before_c1k, &after_c1k),
            mock_metrics: mock_metric_delta(&mock_before_c1k, &mock_after_c1k),
        },
        c1w: RunReport {
            label: format!("c{}", config.c1w),
            load: c1w_load,
            gateway_metrics: gateway_metric_delta(&before_c1w, &after_c1w),
            mock_metrics: mock_metric_delta(&mock_before_c1w, &mock_after_c1w),
        },
        mock_only: MockOnlyCompareReport {
            c1k: mock_only_c1k,
            c1w: mock_only_c1w,
        },
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

fn load_config(
    gateway_url: &str,
    gateway_health_url: &str,
    config: &Config,
    requests: usize,
    concurrency: usize,
    first_body_hold_ms: u64,
) -> HttpLoadProbeConfig {
    let mut headers = BTreeMap::new();
    headers.insert(
        CONTENT_TYPE.as_str().to_string(),
        "application/json".to_string(),
    );
    headers.insert(
        AUTHORIZATION.as_str().to_string(),
        "Bearer sk-aether-openai-chat-pressure".to_string(),
    );
    HttpLoadProbeConfig {
        url: gateway_url.to_string(),
        warmup_url: Some(gateway_health_url.to_string()),
        method: Method::POST,
        headers,
        header_sets: Vec::new(),
        body: Some(
            serde_json::json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            })
            .to_string()
            .into_bytes(),
        ),
        total_requests: requests,
        concurrency,
        warmup_connections: 0,
        timeout: Duration::from_millis(config.timeout_ms),
        connect_timeout: Some(Duration::from_millis(10_000)),
        response_mode: HttpLoadProbeResponseMode::FirstBodyByte,
        client_shards: config.client_shards,
        pool_max_idle_per_host: Some(config.requests.max(concurrency).max(1024)),
        start_ramp: Duration::ZERO,
        http1_only: true,
        http2_prior_knowledge: false,
        first_body_hold: Duration::from_millis(first_body_hold_ms),
    }
}

fn mock_router(state: MockTargetState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/chat/completions", post(mock_chat_completions))
        .with_state(state)
}

async fn mock_chat_completions(
    State(state): State<MockTargetState>,
    request: Request<Body>,
) -> Response {
    record_mock_started(&state.metrics);
    let body_started_at = Instant::now();
    let body = match to_bytes(request.into_body(), BODY_LIMIT).await {
        Ok(body) => body,
        Err(err) => {
            record_mock_completed(&state.metrics);
            return (
                StatusCode::BAD_REQUEST,
                format!("failed to read mock request body: {err}"),
            )
                .into_response();
        }
    };
    record_mock_request_body_read(&state.metrics, body_started_at.elapsed());

    let wants_stream = request_wants_stream(&body);
    let model = request_model(&body).unwrap_or_else(|| "gpt-5-upstream".to_string());
    if !wants_stream {
        record_mock_completed(&state.metrics);
        return axum::Json(serde_json::json!({
            "id": "chatcmpl-pressure-mock",
            "object": "chat.completion",
            "model": model,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }]
        }))
        .into_response();
    }

    let metrics = Arc::clone(&state.metrics);
    let first_chunk_delay = state.first_chunk_delay;
    let label = state.label.clone();
    let stream = async_stream::stream! {
        let _guard = CompletionGuard::new(Arc::clone(&metrics));
        let header_created_at = Instant::now();
        if !first_chunk_delay.is_zero() {
            tokio::time::sleep(first_chunk_delay).await;
        }
        record_mock_first_chunk_yield(&metrics, header_created_at.elapsed());
        let chunk = serde_json::json!({
            "id": format!("chatcmpl-pressure-mock-{label}"),
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {"role": "assistant", "content": "x"},
                "finish_reason": null
            }]
        });
        yield Ok::<Bytes, Infallible>(Bytes::from(format!("data: {chunk}\n\n")));
        yield Ok::<Bytes, Infallible>(Bytes::from_static(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        ));
        yield Ok::<Bytes, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
    };

    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        CONTENT_TYPE,
        "text/event-stream; charset=utf-8"
            .parse()
            .expect("valid content-type"),
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        "no-cache".parse().expect("valid cache-control"),
    );
    response
}

fn request_wants_stream(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn request_model(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

fn record_mock_started(metrics: &MockTargetMetrics) {
    metrics.requests_total.fetch_add(1, Ordering::AcqRel);
    let in_flight = metrics.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
    metrics.max_in_flight.fetch_max(in_flight, Ordering::AcqRel);
}

fn record_mock_completed(metrics: &MockTargetMetrics) {
    metrics.completed_total.fetch_add(1, Ordering::AcqRel);
    metrics.in_flight.fetch_sub(1, Ordering::AcqRel);
}

fn record_mock_request_body_read(metrics: &MockTargetMetrics, elapsed: Duration) {
    let elapsed_ms = elapsed.as_millis() as u64;
    metrics
        .request_body_read_sum_ms
        .fetch_add(elapsed_ms, Ordering::AcqRel);
    metrics
        .request_body_read_max_ms
        .fetch_max(elapsed_ms, Ordering::AcqRel);
}

fn record_mock_first_chunk_yield(metrics: &MockTargetMetrics, elapsed: Duration) {
    let elapsed_ms = elapsed.as_millis() as u64;
    metrics
        .first_chunk_yield_total
        .fetch_add(1, Ordering::AcqRel);
    metrics
        .response_header_to_first_chunk_sum_ms
        .fetch_add(elapsed_ms, Ordering::AcqRel);
    metrics
        .response_header_to_first_chunk_max_ms
        .fetch_max(elapsed_ms, Ordering::AcqRel);
}

fn snapshot_mock_metrics(metrics: &[Arc<MockTargetMetrics>]) -> Vec<MockMetricSnapshot> {
    metrics
        .iter()
        .map(|metrics| MockMetricSnapshot {
            requests_total: metrics.requests_total.load(Ordering::Acquire),
            completed_total: metrics.completed_total.load(Ordering::Acquire),
            in_flight: metrics.in_flight.load(Ordering::Acquire) as u64,
            max_in_flight: metrics.max_in_flight.load(Ordering::Acquire) as u64,
            first_chunk_yield_total: metrics.first_chunk_yield_total.load(Ordering::Acquire),
            request_body_read_sum_ms: metrics.request_body_read_sum_ms.load(Ordering::Acquire),
            request_body_read_max_ms: metrics.request_body_read_max_ms.load(Ordering::Acquire),
            response_header_to_first_chunk_sum_ms: metrics
                .response_header_to_first_chunk_sum_ms
                .load(Ordering::Acquire),
            response_header_to_first_chunk_max_ms: metrics
                .response_header_to_first_chunk_max_ms
                .load(Ordering::Acquire),
        })
        .collect()
}

fn mock_metric_delta(
    before: &[MockMetricSnapshot],
    after: &[MockMetricSnapshot],
) -> Vec<MockMetricDelta> {
    after
        .iter()
        .enumerate()
        .map(|(index, after)| {
            let before = before.get(index).cloned().unwrap_or_default();
            MockMetricDelta {
                target: format!("target-{index}"),
                requests_total: after.requests_total.saturating_sub(before.requests_total),
                completed_total: after.completed_total.saturating_sub(before.completed_total),
                in_flight_after: after.in_flight,
                max_in_flight_after: after.max_in_flight,
                first_chunk_yield_total: after
                    .first_chunk_yield_total
                    .saturating_sub(before.first_chunk_yield_total),
                request_body_read_sum_ms: after
                    .request_body_read_sum_ms
                    .saturating_sub(before.request_body_read_sum_ms),
                request_body_read_max_ms_after: after.request_body_read_max_ms,
                response_header_to_first_chunk_sum_ms: after
                    .response_header_to_first_chunk_sum_ms
                    .saturating_sub(before.response_header_to_first_chunk_sum_ms),
                response_header_to_first_chunk_max_ms_after: after
                    .response_header_to_first_chunk_max_ms,
            }
        })
        .collect()
}

fn gateway_metric_delta(
    before: &[PrometheusSample],
    after: &[PrometheusSample],
) -> GatewayMetricDelta {
    let stage_names = [
        "frontdoor_handler_queue",
        "frontdoor_admission",
        "frontdoor_context",
        "frontdoor_body_buffer",
        "frontdoor_owner_forward",
        "frontdoor_auth_model",
        "frontdoor_rpm",
        "frontdoor_rpm_system_default",
        "frontdoor_rpm_runtime_check",
        "frontdoor_rpm_memory_fallback",
        "frontdoor_local_ai_public",
        "frontdoor_execute_stream",
        "frontdoor_to_stream_response_ready",
        "frontdoor_to_stream_body_first_poll",
        "frontdoor_to_stream_first_client_yield",
        "openai_chat_stream_target_select",
        "openai_chat_payload_parts_prepare",
        "openai_chat_payload_model_directives",
        "openai_chat_payload_redaction",
        "openai_chat_payload_auth_prepare",
        "openai_chat_payload_body_build",
        "stream_candidate_execute",
        "upstream_execution_gate_wait",
        "stream_upstream_target_admission",
        "stream_upstream_headers",
        "direct_reqwest_request_send",
        "direct_send_headers",
        "direct_h2c_response_headers_wait",
        "stream_first_data",
        "stream_first_client_yield",
        "stream_total",
    ];

    GatewayMetricDelta {
        stream_pre_first_byte_spawn_total: counter_delta(
            before,
            after,
            "stream_pre_first_byte_spawn_total",
            &[],
        ),
        request_candidate_queue_dropped_total: counter_delta(
            before,
            after,
            "request_candidate_queue_dropped_total",
            &[],
        ),
        raw_candidates_scanned_total: counter_delta(
            before,
            after,
            "openai_chat_stream_target_select_raw_candidates_scanned_total",
            &[],
        ),
        payload_build_selected_total: counter_delta(
            before,
            after,
            "openai_chat_stream_payload_build_selected_total",
            &[],
        ),
        payload_build_prefetch_avoided_total: counter_delta(
            before,
            after,
            "openai_chat_stream_payload_build_prefetch_avoided_total",
            &[],
        ),
        selected_rank_sum: counter_delta(
            before,
            after,
            "openai_chat_stream_target_select_selected_rank_sum",
            &[],
        ),
        model_directive_cache_hit_total: counter_delta(
            before,
            after,
            "openai_chat_model_directive_cache_hit_total",
            &[],
        ),
        model_directive_cache_miss_total: counter_delta(
            before,
            after,
            "openai_chat_model_directive_cache_miss_total",
            &[],
        ),
        redaction_request_cache_hit_total: counter_delta(
            before,
            after,
            "chat_pii_redaction_request_cache_hit_total",
            &[],
        ),
        redaction_request_cache_miss_total: counter_delta(
            before,
            after,
            "chat_pii_redaction_request_cache_miss_total",
            &[],
        ),
        target_raw_seen_total: metric_delta_by_target(
            before,
            after,
            "upstream_target_raw_seen_total",
        ),
        target_preselect_total: metric_delta_by_target(
            before,
            after,
            "upstream_target_preselect_total",
        ),
        target_selected_total: metric_delta_by_target(
            before,
            after,
            "upstream_target_selected_total",
        ),
        target_max_in_flight: metric_after_by_target(after, "upstream_target_max_in_flight"),
        target_saturated_total: metric_delta_by_target(
            before,
            after,
            "upstream_target_saturated_total",
        ),
        stage_deltas: stage_names
            .iter()
            .map(|stage| {
                (
                    (*stage).to_string(),
                    StageMetricDelta {
                        count: counter_delta(
                            before,
                            after,
                            "gateway_stage_latency_count",
                            &[("stage", stage)],
                        ),
                        sum_ms: counter_delta(
                            before,
                            after,
                            "gateway_stage_latency_sum_ms",
                            &[("stage", stage)],
                        ),
                        max_ms_after: metric_value(
                            after,
                            "gateway_stage_latency_max_ms",
                            &[("stage", stage)],
                        )
                        .unwrap_or_default(),
                        p95_bucket_ms: stage_latency_bucket_quantile(before, after, stage, 0.95),
                        p99_bucket_ms: stage_latency_bucket_quantile(before, after, stage, 0.99),
                    },
                )
            })
            .collect(),
    }
}

fn stage_latency_bucket_quantile(
    before: &[PrometheusSample],
    after: &[PrometheusSample],
    stage: &str,
    quantile: f64,
) -> Option<u64> {
    let count = counter_delta(
        before,
        after,
        "gateway_stage_latency_count",
        &[("stage", stage)],
    );
    if count == 0 {
        return None;
    }
    let threshold = ((count as f64) * quantile).ceil().max(1.0) as u64;
    let mut buckets = after
        .iter()
        .filter(|sample| {
            metric_name_matches(&sample.name, "gateway_stage_latency_bucket")
                && labels_match(sample, &[("stage", stage)])
        })
        .filter_map(|sample| {
            let le_ms = sample.labels.get("le_ms")?.parse::<u64>().ok()?;
            let after_value = sample.value.parse::<u64>().ok()?;
            let before_value = metric_value(
                before,
                "gateway_stage_latency_bucket",
                &[("stage", stage), ("le_ms", sample.labels.get("le_ms")?)],
            )
            .unwrap_or_default();
            Some((le_ms, after_value.saturating_sub(before_value)))
        })
        .collect::<Vec<_>>();
    buckets.sort_unstable_by_key(|(le_ms, _)| *le_ms);
    buckets
        .into_iter()
        .find_map(|(le_ms, value)| (value >= threshold).then_some(le_ms))
}

fn counter_delta(
    before: &[PrometheusSample],
    after: &[PrometheusSample],
    metric_name: &str,
    labels: &[(&str, &str)],
) -> u64 {
    let before = metric_value(before, metric_name, labels).unwrap_or_default();
    let after = metric_value(after, metric_name, labels).unwrap_or_default();
    after.saturating_sub(before)
}

fn metric_value(
    samples: &[PrometheusSample],
    metric_name: &str,
    labels: &[(&str, &str)],
) -> Option<u64> {
    samples
        .iter()
        .find(|sample| {
            metric_name_matches(&sample.name, metric_name) && labels_match(sample, labels)
        })
        .and_then(|sample| sample.value.parse::<u64>().ok())
}

fn metric_delta_by_target(
    before: &[PrometheusSample],
    after: &[PrometheusSample],
    metric_name: &str,
) -> BTreeMap<String, u64> {
    let mut values = BTreeMap::new();
    for sample in after
        .iter()
        .filter(|sample| metric_name_matches(&sample.name, metric_name))
    {
        let Some(target) = sample.labels.get("target") else {
            continue;
        };
        let before_value =
            metric_value(before, metric_name, &[("target", target)]).unwrap_or_default();
        let after_value = sample.value.parse::<u64>().unwrap_or_default();
        values.insert(target.clone(), after_value.saturating_sub(before_value));
    }
    values
}

fn metric_after_by_target(
    samples: &[PrometheusSample],
    metric_name: &str,
) -> BTreeMap<String, u64> {
    samples
        .iter()
        .filter(|sample| metric_name_matches(&sample.name, metric_name))
        .filter_map(|sample| {
            Some((
                sample.labels.get("target")?.clone(),
                sample.value.parse::<u64>().ok()?,
            ))
        })
        .collect()
}

fn metric_name_matches(actual: &str, expected: &str) -> bool {
    actual == expected || actual.ends_with(&format!("_{expected}"))
}

fn labels_match(sample: &PrometheusSample, labels: &[(&str, &str)]) -> bool {
    labels
        .iter()
        .all(|(key, value)| sample.labels.get(*key).map(String::as_str) == Some(*value))
}

fn parse_args(args: Vec<String>) -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--requests" => config.requests = next_value(&mut iter, "--requests")?.parse()?,
            "--c1k" => config.c1k = next_value(&mut iter, "--c1k")?.parse()?,
            "--c1w" => config.c1w = next_value(&mut iter, "--c1w")?.parse()?,
            "--targets" => config.targets = next_value(&mut iter, "--targets")?.parse()?,
            "--client-shards" => {
                config.client_shards = next_value(&mut iter, "--client-shards")?.parse()?
            }
            "--timeout-ms" => config.timeout_ms = next_value(&mut iter, "--timeout-ms")?.parse()?,
            "--warmup-requests" => {
                config.warmup_requests = next_value(&mut iter, "--warmup-requests")?.parse()?
            }
            "--warmup-concurrency" => {
                config.warmup_concurrency =
                    next_value(&mut iter, "--warmup-concurrency")?.parse()?
            }
            "--first-body-hold-ms" => {
                config.first_body_hold_ms =
                    next_value(&mut iter, "--first-body-hold-ms")?.parse()?
            }
            "--output" => {
                config.output_path = Some(PathBuf::from(next_value(&mut iter, "--output")?))
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    Ok(config)
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    iter.next()
        .ok_or_else(|| format!("missing value for {flag}").into())
}

fn print_help() {
    println!(
        "gateway_openai_chat_c_compare \
         [--requests N] [--c1k N] [--c1w N] [--targets N] \
         [--client-shards N] [--timeout-ms N] \
         [--warmup-requests N] [--warmup-concurrency N] \
         [--first-body-hold-ms N] [--output PATH]"
    );
}
