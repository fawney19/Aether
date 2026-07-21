// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    binds: Vec<SocketAddr>,
    chunks: u64,
    first_byte_delay: Duration,
    first_byte_jitter: Duration,
    chunk_delay: Duration,
    chunk_delay_jitter: Duration,
    payload_bytes: usize,
    payload_bytes_jitter: usize,
    status: StatusCode,
    assume_stream: bool,
    seed: u64,
    fault_429_bps: u16,
    fault_500_bps: u16,
    fault_timeout_bps: u16,
    fault_truncate_stream_bps: u16,
    timeout_hold: Duration,
}

const MAX_MOCK_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const BASIS_POINTS: u16 = 10_000;
const DEFAULT_TIMEOUT_HOLD_MS: u64 = 60_000;
const REQUEST_SEQUENCE_HEADER: &str = "x-mock-request-sequence";

const RANDOM_DOMAIN_FAULT: u64 = 0x5c32_22f7_27d4_7a6f;
const RANDOM_DOMAIN_FIRST_BYTE: u64 = 0x087d_89d9_3bc3_15db;
const RANDOM_DOMAIN_PAYLOAD: u64 = 0xdf91_7f4b_a229_09ed;
const RANDOM_DOMAIN_CHUNK_DELAY: u64 = 0x6d07_68d3_891f_e0bb;
const RANDOM_DOMAIN_TRUNCATE_AFTER: u64 = 0xa71d_a531_6f92_c405;

impl Default for Config {
    fn default() -> Self {
        Self {
            binds: vec!["127.0.0.1:18181"
                .parse()
                .expect("default bind address should parse")],
            chunks: 8,
            first_byte_delay: Duration::from_millis(0),
            first_byte_jitter: Duration::ZERO,
            chunk_delay: Duration::from_millis(20),
            chunk_delay_jitter: Duration::ZERO,
            payload_bytes: 32,
            payload_bytes_jitter: 0,
            status: StatusCode::OK,
            assume_stream: false,
            seed: 0,
            fault_429_bps: 0,
            fault_500_bps: 0,
            fault_timeout_bps: 0,
            fault_truncate_stream_bps: 0,
            timeout_hold: Duration::from_millis(DEFAULT_TIMEOUT_HOLD_MS),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Status429,
    Status500,
    Timeout,
    TruncateStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RequestProfile {
    sequence: u64,
    expose_sequence: bool,
    first_byte_delay: Duration,
    payload_bytes: usize,
    fault: Fault,
    truncate_after_chunks: Option<u64>,
}

#[derive(Debug, Default)]
struct Metrics {
    requests_total: AtomicU64,
    completed_total: AtomicU64,
    in_flight: AtomicU64,
    max_in_flight: AtomicU64,
    request_body_read_sum_ms: AtomicU64,
    request_body_read_max_ms: AtomicU64,
    accepted_to_response_header_sum_ms: AtomicU64,
    accepted_to_response_header_max_ms: AtomicU64,
    first_chunk_yield_total: AtomicU64,
    response_header_to_first_chunk_sum_ms: AtomicU64,
    response_header_to_first_chunk_max_ms: AtomicU64,
    fault_429_total: AtomicU64,
    fault_500_total: AtomicU64,
    fault_timeout_total: AtomicU64,
    fault_truncated_stream_total: AtomicU64,
    binds: BTreeMap<String, Arc<BindMetrics>>,
}

#[derive(Debug, Default)]
struct BindMetrics {
    requests_total: AtomicU64,
    completed_total: AtomicU64,
    in_flight: AtomicU64,
    max_in_flight: AtomicU64,
    request_body_read_sum_ms: AtomicU64,
    request_body_read_max_ms: AtomicU64,
    accepted_to_response_header_sum_ms: AtomicU64,
    accepted_to_response_header_max_ms: AtomicU64,
    first_chunk_yield_total: AtomicU64,
    response_header_to_first_chunk_sum_ms: AtomicU64,
    response_header_to_first_chunk_max_ms: AtomicU64,
    fault_429_total: AtomicU64,
    fault_500_total: AtomicU64,
    fault_timeout_total: AtomicU64,
    fault_truncated_stream_total: AtomicU64,
}

#[derive(Debug, Clone)]
struct App {
    config: Config,
    metrics: Arc<Metrics>,
    bind_label: Arc<str>,
}

struct RequestCompletionGuard {
    app: App,
    completed: bool,
}

impl RequestCompletionGuard {
    fn new(app: App) -> Self {
        Self {
            app,
            completed: false,
        }
    }

    fn complete(mut self) {
        self.complete_once();
    }

    fn complete_once(&mut self) {
        if self.completed {
            return;
        }
        record_request_completed(&self.app);
        self.completed = true;
    }
}

impl Drop for RequestCompletionGuard {
    fn drop(&mut self) {
        self.complete_once();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args(std::env::args().skip(1).collect())?;
    let metrics = Arc::new(Metrics::for_binds(&config.binds));

    serve_listeners(&config, metrics).await?;
    Ok(())
}

async fn serve_listeners(
    config: &Config,
    metrics: Arc<Metrics>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut listeners = Vec::with_capacity(config.binds.len());
    for bind in &config.binds {
        listeners.push((*bind, tokio::net::TcpListener::bind(bind).await?));
    }
    if listeners.len() == 1 {
        let (bind, listener) = listeners
            .into_iter()
            .next()
            .ok_or_else(|| std::io::Error::other("mock upstream listener set is empty"))?;
        let app = build_router(config.clone(), Arc::clone(&metrics), bind);
        eprintln!("mock OpenAI upstream listening on http://{bind}");
        axum::serve(listener, app).await?;
        return Ok(());
    }

    let mut servers = tokio::task::JoinSet::new();
    for (bind, listener) in listeners {
        let app = build_router(config.clone(), Arc::clone(&metrics), bind);
        eprintln!("mock OpenAI upstream listening on http://{bind}");
        servers.spawn(async move { axum::serve(listener, app).await });
    }
    if let Some(result) = servers.join_next().await {
        servers.abort_all();
        let serve_result = result
            .map_err(|err| std::io::Error::other(format!("mock listener task failed: {err}")))?;
        serve_result?;
    }
    Ok(())
}

fn build_router(config: Config, shared_metrics: Arc<Metrics>, bind: SocketAddr) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/chat/completions", post(chat_completions))
        .route("/v1/responses", post(responses))
        .route("/responses", post(responses))
        .with_state(App {
            config,
            metrics: shared_metrics,
            bind_label: Arc::from(bind.to_string()),
        })
}

impl Metrics {
    fn for_binds(binds: &[SocketAddr]) -> Self {
        let mut metrics = Self::default();
        for bind in binds {
            metrics
                .binds
                .insert(bind.to_string(), Arc::new(BindMetrics::default()));
        }
        metrics
    }

    fn bind_metrics(&self, bind: &str) -> Option<Arc<BindMetrics>> {
        self.binds.get(bind).cloned()
    }
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

async fn metrics(State(app): State<App>) -> Response {
    let mut body = format!(
        concat!(
            "# HELP mock_upstream_requests_total Total requests accepted by the mock upstream.\n",
            "# TYPE mock_upstream_requests_total counter\n",
            "mock_upstream_requests_total {}\n",
            "# HELP mock_upstream_completed_total Total requests completed by the mock upstream.\n",
            "# TYPE mock_upstream_completed_total counter\n",
            "mock_upstream_completed_total {}\n",
            "# HELP mock_upstream_in_flight Current in-flight requests/streams.\n",
            "# TYPE mock_upstream_in_flight gauge\n",
            "mock_upstream_in_flight {}\n",
            "# HELP mock_upstream_max_in_flight Maximum in-flight requests/streams observed.\n",
            "# TYPE mock_upstream_max_in_flight gauge\n",
            "mock_upstream_max_in_flight {}\n",
            "# HELP mock_upstream_first_chunk_yield_total Total first stream chunks yielded by the mock upstream.\n",
            "# TYPE mock_upstream_first_chunk_yield_total counter\n",
            "mock_upstream_first_chunk_yield_total {}\n",
            "# HELP mock_upstream_request_body_read_sum_ms Total milliseconds spent reading request bodies after handler entry.\n",
            "# TYPE mock_upstream_request_body_read_sum_ms counter\n",
            "mock_upstream_request_body_read_sum_ms {}\n",
            "# HELP mock_upstream_request_body_read_max_ms Maximum milliseconds spent reading a request body after handler entry.\n",
            "# TYPE mock_upstream_request_body_read_max_ms gauge\n",
            "mock_upstream_request_body_read_max_ms {}\n",
            "# HELP mock_upstream_accepted_to_response_header_sum_ms Total milliseconds from request acceptance to response construction.\n",
            "# TYPE mock_upstream_accepted_to_response_header_sum_ms counter\n",
            "mock_upstream_accepted_to_response_header_sum_ms {}\n",
            "# HELP mock_upstream_accepted_to_response_header_max_ms Maximum milliseconds from request acceptance to response construction.\n",
            "# TYPE mock_upstream_accepted_to_response_header_max_ms gauge\n",
            "mock_upstream_accepted_to_response_header_max_ms {}\n",
            "# HELP mock_upstream_response_header_to_first_chunk_sum_ms Total milliseconds from response construction to first stream chunk yield.\n",
            "# TYPE mock_upstream_response_header_to_first_chunk_sum_ms counter\n",
            "mock_upstream_response_header_to_first_chunk_sum_ms {}\n",
            "# HELP mock_upstream_response_header_to_first_chunk_max_ms Maximum milliseconds from response construction to first stream chunk yield.\n",
            "# TYPE mock_upstream_response_header_to_first_chunk_max_ms gauge\n",
            "mock_upstream_response_header_to_first_chunk_max_ms {}\n",
            "# HELP mock_upstream_fault_429_total Requests assigned the configured HTTP 429 fault.\n",
            "# TYPE mock_upstream_fault_429_total counter\n",
            "mock_upstream_fault_429_total {}\n",
            "# HELP mock_upstream_fault_500_total Requests assigned the configured HTTP 500 fault.\n",
            "# TYPE mock_upstream_fault_500_total counter\n",
            "mock_upstream_fault_500_total {}\n",
            "# HELP mock_upstream_fault_timeout_total Requests assigned the configured timeout fault.\n",
            "# TYPE mock_upstream_fault_timeout_total counter\n",
            "mock_upstream_fault_timeout_total {}\n",
            "# HELP mock_upstream_fault_truncated_stream_total Streams intentionally terminated before their protocol completion event.\n",
            "# TYPE mock_upstream_fault_truncated_stream_total counter\n",
            "mock_upstream_fault_truncated_stream_total {}\n"
        ),
        app.metrics.requests_total.load(Ordering::Acquire),
        app.metrics.completed_total.load(Ordering::Acquire),
        app.metrics.in_flight.load(Ordering::Acquire),
        app.metrics.max_in_flight.load(Ordering::Acquire),
        app.metrics.first_chunk_yield_total.load(Ordering::Acquire),
        app.metrics
            .request_body_read_sum_ms
            .load(Ordering::Acquire),
        app.metrics
            .request_body_read_max_ms
            .load(Ordering::Acquire),
        app.metrics
            .accepted_to_response_header_sum_ms
            .load(Ordering::Acquire),
        app.metrics
            .accepted_to_response_header_max_ms
            .load(Ordering::Acquire),
        app.metrics
            .response_header_to_first_chunk_sum_ms
            .load(Ordering::Acquire),
        app.metrics
            .response_header_to_first_chunk_max_ms
            .load(Ordering::Acquire),
        app.metrics.fault_429_total.load(Ordering::Acquire),
        app.metrics.fault_500_total.load(Ordering::Acquire),
        app.metrics.fault_timeout_total.load(Ordering::Acquire),
        app.metrics
            .fault_truncated_stream_total
            .load(Ordering::Acquire),
    );
    for (bind, metrics) in &app.metrics.binds {
        let bind = prometheus_label_value(bind);
        body.push_str(&format!(
            concat!(
                "mock_upstream_requests_total{{bind=\"{}\"}} {}\n",
                "mock_upstream_completed_total{{bind=\"{}\"}} {}\n",
                "mock_upstream_in_flight{{bind=\"{}\"}} {}\n",
                "mock_upstream_max_in_flight{{bind=\"{}\"}} {}\n",
                "mock_upstream_first_chunk_yield_total{{bind=\"{}\"}} {}\n",
                "mock_upstream_request_body_read_sum_ms{{bind=\"{}\"}} {}\n",
                "mock_upstream_request_body_read_max_ms{{bind=\"{}\"}} {}\n",
                "mock_upstream_accepted_to_response_header_sum_ms{{bind=\"{}\"}} {}\n",
                "mock_upstream_accepted_to_response_header_max_ms{{bind=\"{}\"}} {}\n",
                "mock_upstream_response_header_to_first_chunk_sum_ms{{bind=\"{}\"}} {}\n",
                "mock_upstream_response_header_to_first_chunk_max_ms{{bind=\"{}\"}} {}\n",
                "mock_upstream_fault_429_total{{bind=\"{}\"}} {}\n",
                "mock_upstream_fault_500_total{{bind=\"{}\"}} {}\n",
                "mock_upstream_fault_timeout_total{{bind=\"{}\"}} {}\n",
                "mock_upstream_fault_truncated_stream_total{{bind=\"{}\"}} {}\n"
            ),
            bind,
            metrics.requests_total.load(Ordering::Acquire),
            bind,
            metrics.completed_total.load(Ordering::Acquire),
            bind,
            metrics.in_flight.load(Ordering::Acquire),
            bind,
            metrics.max_in_flight.load(Ordering::Acquire),
            bind,
            metrics.first_chunk_yield_total.load(Ordering::Acquire),
            bind,
            metrics.request_body_read_sum_ms.load(Ordering::Acquire),
            bind,
            metrics.request_body_read_max_ms.load(Ordering::Acquire),
            bind,
            metrics
                .accepted_to_response_header_sum_ms
                .load(Ordering::Acquire),
            bind,
            metrics
                .accepted_to_response_header_max_ms
                .load(Ordering::Acquire),
            bind,
            metrics
                .response_header_to_first_chunk_sum_ms
                .load(Ordering::Acquire),
            bind,
            metrics
                .response_header_to_first_chunk_max_ms
                .load(Ordering::Acquire),
            bind,
            metrics.fault_429_total.load(Ordering::Acquire),
            bind,
            metrics.fault_500_total.load(Ordering::Acquire),
            bind,
            metrics.fault_timeout_total.load(Ordering::Acquire),
            bind,
            metrics.fault_truncated_stream_total.load(Ordering::Acquire),
        ));
    }
    (StatusCode::OK, body).into_response()
}

async fn chat_completions(State(app): State<App>, request: axum::extract::Request) -> Response {
    let request_started = record_request_started(&app);
    let mut completion = Some(RequestCompletionGuard::new(app.clone()));
    let mut profile = request_profile(&app.config, request_started.sequence);
    if app.config.status != StatusCode::OK {
        let fault = fault_for_status(app.config.status);
        if fault != Fault::None {
            record_fault(&app, fault);
        }
        record_response_header_created(&app, request_started.started_at.elapsed());
        completion
            .take()
            .expect("request completion guard should be present")
            .complete();
        return with_sequence(
            (app.config.status, "mock upstream error\n").into_response(),
            profile,
        );
    }
    if profile.fault == Fault::Timeout {
        record_fault(&app, Fault::Timeout);
        tokio::time::sleep(app.config.timeout_hold).await;
        record_response_header_created(&app, request_started.started_at.elapsed());
        completion
            .take()
            .expect("request completion guard should be present")
            .complete();
        return with_sequence(
            (StatusCode::GATEWAY_TIMEOUT, "mock upstream timeout\n").into_response(),
            profile,
        );
    }
    if matches!(profile.fault, Fault::Status429 | Fault::Status500) {
        let status = match profile.fault {
            Fault::Status429 => StatusCode::TOO_MANY_REQUESTS,
            Fault::Status500 => StatusCode::INTERNAL_SERVER_ERROR,
            _ => unreachable!("status fault was checked above"),
        };
        record_fault(&app, profile.fault);
        record_response_header_created(&app, request_started.started_at.elapsed());
        completion
            .take()
            .expect("request completion guard should be present")
            .complete();
        return with_sequence((status, "mock upstream error\n").into_response(), profile);
    }
    if app.config.assume_stream {
        record_response_header_created(&app, request_started.started_at.elapsed());
        return build_chat_sse_response(
            app,
            profile,
            completion
                .take()
                .expect("request completion guard should be present"),
        );
    }

    let body = match read_request_body(&app, request).await {
        Ok(body) => body,
        Err(response) => {
            record_response_header_created(&app, request_started.started_at.elapsed());
            completion
                .take()
                .expect("request completion guard should be present")
                .complete();
            return with_sequence(response, profile);
        }
    };
    let stream = request_wants_stream(&body);
    if stream {
        record_response_header_created(&app, request_started.started_at.elapsed());
        return build_chat_sse_response(
            app,
            profile,
            completion
                .take()
                .expect("request completion guard should be present"),
        );
    }
    // A stream truncation profile only applies after the request is known to be streaming.
    profile.fault = Fault::None;
    profile.truncate_after_chunks = None;
    let payload = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "created": current_unix_secs(),
        "model": request_model(&body).unwrap_or_else(|| "mock-model".to_string()),
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": mock_payload(profile.payload_bytes)
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": app.config.chunks.max(1),
            "total_tokens": app.config.chunks.max(1) + 1
        }
    });
    record_response_header_created(&app, request_started.started_at.elapsed());
    completion
        .take()
        .expect("request completion guard should be present")
        .complete();
    with_sequence(axum::Json(payload).into_response(), profile)
}

async fn responses(State(app): State<App>, request: axum::extract::Request) -> Response {
    let request_started = record_request_started(&app);
    let mut completion = Some(RequestCompletionGuard::new(app.clone()));
    let mut profile = request_profile(&app.config, request_started.sequence);
    if app.config.status != StatusCode::OK {
        let fault = fault_for_status(app.config.status);
        if fault != Fault::None {
            record_fault(&app, fault);
        }
        record_response_header_created(&app, request_started.started_at.elapsed());
        completion
            .take()
            .expect("request completion guard should be present")
            .complete();
        return with_sequence(
            (app.config.status, "mock upstream error\n").into_response(),
            profile,
        );
    }
    if profile.fault == Fault::Timeout {
        record_fault(&app, Fault::Timeout);
        tokio::time::sleep(app.config.timeout_hold).await;
        record_response_header_created(&app, request_started.started_at.elapsed());
        completion
            .take()
            .expect("request completion guard should be present")
            .complete();
        return with_sequence(
            (StatusCode::GATEWAY_TIMEOUT, "mock upstream timeout\n").into_response(),
            profile,
        );
    }
    if matches!(profile.fault, Fault::Status429 | Fault::Status500) {
        let status = match profile.fault {
            Fault::Status429 => StatusCode::TOO_MANY_REQUESTS,
            Fault::Status500 => StatusCode::INTERNAL_SERVER_ERROR,
            _ => unreachable!("status fault was checked above"),
        };
        record_fault(&app, profile.fault);
        record_response_header_created(&app, request_started.started_at.elapsed());
        completion
            .take()
            .expect("request completion guard should be present")
            .complete();
        return with_sequence((status, "mock upstream error\n").into_response(), profile);
    }
    if app.config.assume_stream {
        record_response_header_created(&app, request_started.started_at.elapsed());
        return build_responses_sse_response(
            app,
            profile,
            completion
                .take()
                .expect("request completion guard should be present"),
        );
    }

    let body = match read_request_body(&app, request).await {
        Ok(body) => body,
        Err(response) => {
            record_response_header_created(&app, request_started.started_at.elapsed());
            completion
                .take()
                .expect("request completion guard should be present")
                .complete();
            return with_sequence(response, profile);
        }
    };
    let stream = request_wants_stream(&body);
    if stream {
        record_response_header_created(&app, request_started.started_at.elapsed());
        return build_responses_sse_response(
            app,
            profile,
            completion
                .take()
                .expect("request completion guard should be present"),
        );
    }
    profile.fault = Fault::None;
    profile.truncate_after_chunks = None;
    let payload = json!({
        "id": "resp_mock",
        "object": "response",
        "created_at": current_unix_secs(),
        "model": request_model(&body).unwrap_or_else(|| "mock-model".to_string()),
        "output": [{
            "type": "message",
            "id": "msg_mock",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": mock_payload(profile.payload_bytes)
            }]
        }],
        "usage": {
            "input_tokens": 1,
            "output_tokens": app.config.chunks.max(1),
            "total_tokens": app.config.chunks.max(1) + 1
        }
    });
    record_response_header_created(&app, request_started.started_at.elapsed());
    completion
        .take()
        .expect("request completion guard should be present")
        .complete();
    with_sequence(axum::Json(payload).into_response(), profile)
}

fn build_chat_sse_response(
    app: App,
    profile: RequestProfile,
    completion: RequestCompletionGuard,
) -> Response {
    let response_created_at = Instant::now();
    let config = app.config.clone();
    let mut completion = Some(completion);
    let stream = async_stream::stream! {
        if !profile.first_byte_delay.is_zero() {
            tokio::time::sleep(profile.first_byte_delay).await;
        }
        let chunk_count = if profile.truncate_after_chunks == Some(0) {
            1
        } else {
            config.chunks
        };
        for index in 0..chunk_count {
            let chunk_delay = chunk_delay_for(&config, profile.sequence, index);
            if index > 0 && !chunk_delay.is_zero() {
                tokio::time::sleep(chunk_delay).await;
            }
            let payload = json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion.chunk",
                "created": current_unix_secs(),
                "model": "mock-model",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "content": mock_payload(profile.payload_bytes)
                    },
                    "finish_reason": serde_json::Value::Null
                }]
            });
            if index == 0 {
                record_first_chunk_yield(&app, response_created_at.elapsed());
            }
            yield Ok::<Bytes, std::io::Error>(Bytes::from(format!("data: {payload}\n\n")));
            if profile.truncate_after_chunks == Some(0)
                || profile.truncate_after_chunks == Some(index + 1)
            {
                // Force Hyper to flush the successful frame before observing the body error.
                tokio::task::yield_now().await;
                record_fault(&app, Fault::TruncateStream);
                yield Err::<Bytes, std::io::Error>(truncated_stream_error());
                return;
            }
        }
        yield Ok::<Bytes, std::io::Error>(Bytes::from(
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        ));
        yield Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n"));
        if let Some(completion) = completion.take() {
            completion.complete();
        }
    };
    with_sequence(sse_response(Body::from_stream(stream)), profile)
}

fn build_responses_sse_response(
    app: App,
    profile: RequestProfile,
    completion: RequestCompletionGuard,
) -> Response {
    let response_created_at = Instant::now();
    let config = app.config.clone();
    let mut completion = Some(completion);
    let stream = async_stream::stream! {
        if !profile.first_byte_delay.is_zero() {
            tokio::time::sleep(profile.first_byte_delay).await;
        }
        record_first_chunk_yield(&app, response_created_at.elapsed());
        yield Ok::<Bytes, std::io::Error>(Bytes::from(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_mock\",\"status\":\"in_progress\"}}\n\n",
        ));
        let chunk_count = if profile.truncate_after_chunks == Some(0) {
            1
        } else {
            config.chunks
        };
        for index in 0..chunk_count {
            let chunk_delay = chunk_delay_for(&config, profile.sequence, index);
            if index > 0 && !chunk_delay.is_zero() {
                tokio::time::sleep(chunk_delay).await;
            }
            let payload = json!({
                "type": "response.output_text.delta",
                "item_id": "msg_mock",
                "output_index": 0,
                "content_index": 0,
                "delta": mock_payload(profile.payload_bytes)
            });
            yield Ok::<Bytes, std::io::Error>(Bytes::from(format!("event: response.output_text.delta\ndata: {payload}\n\n")));
            if profile.truncate_after_chunks == Some(0)
                || profile.truncate_after_chunks == Some(index + 1)
            {
                // Force Hyper to flush the successful frame before observing the body error.
                tokio::task::yield_now().await;
                record_fault(&app, Fault::TruncateStream);
                yield Err::<Bytes, std::io::Error>(truncated_stream_error());
                return;
            }
        }
        yield Ok::<Bytes, std::io::Error>(Bytes::from(
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_mock\",\"status\":\"completed\"}}\n\n",
        ));
        yield Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n"));
        if let Some(completion) = completion.take() {
            completion.complete();
        }
    };
    with_sequence(sse_response(Body::from_stream(stream)), profile)
}

fn truncated_stream_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "mock upstream intentionally truncated the response stream",
    )
}

fn sse_response(body: Body) -> Response {
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

fn with_sequence(mut response: Response, profile: RequestProfile) -> Response {
    if profile.expose_sequence {
        response.headers_mut().insert(
            REQUEST_SEQUENCE_HEADER,
            HeaderValue::from_str(&profile.sequence.to_string())
                .expect("a decimal u64 should always be a valid HTTP header value"),
        );
    }
    response
}

async fn read_request_body(app: &App, request: axum::extract::Request) -> Result<Bytes, Response> {
    let body_started_at = Instant::now();
    let body = to_bytes(request.into_body(), MAX_MOCK_REQUEST_BODY_BYTES)
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("mock upstream failed to read request body: {err}\n"),
            )
                .into_response()
        });
    record_request_body_read(app, body_started_at.elapsed());
    body
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

fn mock_payload(bytes: usize) -> String {
    if bytes == 0 {
        return String::new();
    }
    "x".repeat(bytes)
}

#[derive(Debug, Clone, Copy)]
struct RequestStarted {
    started_at: Instant,
    sequence: u64,
}

fn request_profile(config: &Config, sequence: u64) -> RequestProfile {
    let fault = select_fault(
        config,
        deterministic_u64(config.seed, sequence, RANDOM_DOMAIN_FAULT),
    );
    let first_byte_delay_ms = jittered_value(
        config.first_byte_delay.as_millis() as u64,
        config.first_byte_jitter.as_millis() as u64,
        deterministic_u64(config.seed, sequence, RANDOM_DOMAIN_FIRST_BYTE),
    );
    let payload_bytes = jittered_value(
        config.payload_bytes as u64,
        config.payload_bytes_jitter as u64,
        deterministic_u64(config.seed, sequence, RANDOM_DOMAIN_PAYLOAD),
    ) as usize;
    let truncate_after_chunks = (fault == Fault::TruncateStream).then(|| {
        if config.chunks == 0 {
            0
        } else {
            deterministic_u64(config.seed, sequence, RANDOM_DOMAIN_TRUNCATE_AFTER) % config.chunks
                + 1
        }
    });
    RequestProfile {
        sequence,
        expose_sequence: random_profile_enabled(config),
        first_byte_delay: Duration::from_millis(first_byte_delay_ms),
        payload_bytes,
        fault,
        truncate_after_chunks,
    }
}

fn random_profile_enabled(config: &Config) -> bool {
    !config.first_byte_jitter.is_zero()
        || !config.chunk_delay_jitter.is_zero()
        || config.payload_bytes_jitter > 0
        || config.fault_429_bps > 0
        || config.fault_500_bps > 0
        || config.fault_timeout_bps > 0
        || config.fault_truncate_stream_bps > 0
}

fn chunk_delay_for(config: &Config, sequence: u64, chunk_index: u64) -> Duration {
    let random = deterministic_u64(
        config.seed,
        sequence,
        RANDOM_DOMAIN_CHUNK_DELAY ^ chunk_index.wrapping_mul(0x9e37_79b9_7f4a_7c15),
    );
    Duration::from_millis(jittered_value(
        config.chunk_delay.as_millis() as u64,
        config.chunk_delay_jitter.as_millis() as u64,
        random,
    ))
}

fn select_fault(config: &Config, random: u64) -> Fault {
    let draw = (random % u64::from(BASIS_POINTS)) as u32;
    let end_429 = u32::from(config.fault_429_bps);
    let end_500 = end_429 + u32::from(config.fault_500_bps);
    let end_timeout = end_500 + u32::from(config.fault_timeout_bps);
    let end_truncate = end_timeout + u32::from(config.fault_truncate_stream_bps);
    if draw < end_429 {
        Fault::Status429
    } else if draw < end_500 {
        Fault::Status500
    } else if draw < end_timeout {
        Fault::Timeout
    } else if draw < end_truncate {
        Fault::TruncateStream
    } else {
        Fault::None
    }
}

fn jittered_value(base: u64, jitter: u64, random: u64) -> u64 {
    if jitter == 0 {
        return base;
    }
    let width = jitter * 2 + 1;
    base - jitter + random % width
}

fn deterministic_u64(seed: u64, sequence: u64, domain: u64) -> u64 {
    let mut value = seed
        ^ sequence.wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ domain.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn fault_for_status(status: StatusCode) -> Fault {
    match status {
        StatusCode::TOO_MANY_REQUESTS => Fault::Status429,
        StatusCode::INTERNAL_SERVER_ERROR => Fault::Status500,
        _ => Fault::None,
    }
}

fn record_request_started(app: &App) -> RequestStarted {
    let sequence = app
        .metrics
        .requests_total
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    let in_flight = app.metrics.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
    app.metrics
        .max_in_flight
        .fetch_max(in_flight, Ordering::AcqRel);
    if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
        bind.requests_total.fetch_add(1, Ordering::AcqRel);
        let in_flight = bind.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        bind.max_in_flight.fetch_max(in_flight, Ordering::AcqRel);
    }
    RequestStarted {
        started_at: Instant::now(),
        sequence,
    }
}

fn record_request_completed(app: &App) {
    app.metrics.completed_total.fetch_add(1, Ordering::AcqRel);
    app.metrics.in_flight.fetch_sub(1, Ordering::AcqRel);
    if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
        bind.completed_total.fetch_add(1, Ordering::AcqRel);
        bind.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

fn record_first_chunk_yield(app: &App, elapsed: Duration) {
    let elapsed_ms = elapsed.as_millis() as u64;
    app.metrics
        .first_chunk_yield_total
        .fetch_add(1, Ordering::AcqRel);
    app.metrics
        .response_header_to_first_chunk_sum_ms
        .fetch_add(elapsed_ms, Ordering::AcqRel);
    app.metrics
        .response_header_to_first_chunk_max_ms
        .fetch_max(elapsed_ms, Ordering::AcqRel);
    if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
        bind.first_chunk_yield_total.fetch_add(1, Ordering::AcqRel);
        bind.response_header_to_first_chunk_sum_ms
            .fetch_add(elapsed_ms, Ordering::AcqRel);
        bind.response_header_to_first_chunk_max_ms
            .fetch_max(elapsed_ms, Ordering::AcqRel);
    }
}

fn record_fault(app: &App, fault: Fault) {
    match fault {
        Fault::Status429 => {
            app.metrics.fault_429_total.fetch_add(1, Ordering::AcqRel);
            if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
                bind.fault_429_total.fetch_add(1, Ordering::AcqRel);
            }
        }
        Fault::Status500 => {
            app.metrics.fault_500_total.fetch_add(1, Ordering::AcqRel);
            if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
                bind.fault_500_total.fetch_add(1, Ordering::AcqRel);
            }
        }
        Fault::Timeout => {
            app.metrics
                .fault_timeout_total
                .fetch_add(1, Ordering::AcqRel);
            if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
                bind.fault_timeout_total.fetch_add(1, Ordering::AcqRel);
            }
        }
        Fault::TruncateStream => {
            app.metrics
                .fault_truncated_stream_total
                .fetch_add(1, Ordering::AcqRel);
            if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
                bind.fault_truncated_stream_total
                    .fetch_add(1, Ordering::AcqRel);
            }
        }
        Fault::None => {}
    }
}

fn record_request_body_read(app: &App, elapsed: Duration) {
    let elapsed_ms = elapsed.as_millis() as u64;
    app.metrics
        .request_body_read_sum_ms
        .fetch_add(elapsed_ms, Ordering::AcqRel);
    app.metrics
        .request_body_read_max_ms
        .fetch_max(elapsed_ms, Ordering::AcqRel);
    if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
        bind.request_body_read_sum_ms
            .fetch_add(elapsed_ms, Ordering::AcqRel);
        bind.request_body_read_max_ms
            .fetch_max(elapsed_ms, Ordering::AcqRel);
    }
}

fn record_response_header_created(app: &App, elapsed: Duration) {
    let elapsed_ms = elapsed.as_millis() as u64;
    app.metrics
        .accepted_to_response_header_sum_ms
        .fetch_add(elapsed_ms, Ordering::AcqRel);
    app.metrics
        .accepted_to_response_header_max_ms
        .fetch_max(elapsed_ms, Ordering::AcqRel);
    if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
        bind.accepted_to_response_header_sum_ms
            .fetch_add(elapsed_ms, Ordering::AcqRel);
        bind.accepted_to_response_header_max_ms
            .fetch_max(elapsed_ms, Ordering::AcqRel);
    }
}

fn prometheus_label_value(value: &str) -> String {
    value
        .replace('\\', r"\\")
        .replace('\n', r"\n")
        .replace('"', r#"\""#)
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn parse_args(args: Vec<String>) -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::default();
    let mut binds_overridden = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--bind" => {
                if !binds_overridden {
                    config.binds.clear();
                    binds_overridden = true;
                }
                config.binds.push(next_value(&mut iter, "--bind")?.parse()?);
            }
            "--chunks" => config.chunks = next_value(&mut iter, "--chunks")?.parse()?,
            "--first-byte-delay-ms" => {
                config.first_byte_delay =
                    Duration::from_millis(next_value(&mut iter, "--first-byte-delay-ms")?.parse()?)
            }
            "--first-byte-jitter-ms" => {
                config.first_byte_jitter =
                    Duration::from_millis(next_value(&mut iter, "--first-byte-jitter-ms")?.parse()?)
            }
            "--chunk-delay-ms" => {
                config.chunk_delay =
                    Duration::from_millis(next_value(&mut iter, "--chunk-delay-ms")?.parse()?)
            }
            "--chunk-delay-jitter-ms" => {
                config.chunk_delay_jitter = Duration::from_millis(
                    next_value(&mut iter, "--chunk-delay-jitter-ms")?.parse()?,
                )
            }
            "--payload-bytes" => {
                config.payload_bytes = next_value(&mut iter, "--payload-bytes")?.parse()?
            }
            "--payload-bytes-jitter" => {
                config.payload_bytes_jitter =
                    next_value(&mut iter, "--payload-bytes-jitter")?.parse()?
            }
            "--status" => {
                let status = next_value(&mut iter, "--status")?.parse::<u16>()?;
                config.status = StatusCode::from_u16(status)?;
            }
            "--seed" => config.seed = next_value(&mut iter, "--seed")?.parse()?,
            "--fault-429-bps" => {
                config.fault_429_bps = next_value(&mut iter, "--fault-429-bps")?.parse()?
            }
            "--fault-500-bps" => {
                config.fault_500_bps = next_value(&mut iter, "--fault-500-bps")?.parse()?
            }
            "--fault-timeout-bps" => {
                config.fault_timeout_bps = next_value(&mut iter, "--fault-timeout-bps")?.parse()?
            }
            "--fault-truncate-stream-bps" => {
                config.fault_truncate_stream_bps =
                    next_value(&mut iter, "--fault-truncate-stream-bps")?.parse()?
            }
            "--timeout-hold-ms" => {
                config.timeout_hold =
                    Duration::from_millis(next_value(&mut iter, "--timeout-hold-ms")?.parse()?)
            }
            "--assume-stream" => {
                config.assume_stream = true;
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
    if config.binds.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "at least one --bind is required",
        )
        .into());
    }
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let fault_total = u32::from(config.fault_429_bps)
        + u32::from(config.fault_500_bps)
        + u32::from(config.fault_timeout_bps)
        + u32::from(config.fault_truncate_stream_bps);
    if fault_total > u32::from(BASIS_POINTS) {
        return Err(invalid_config(
            "fault percentages must sum to at most 10000 basis points",
        ));
    }
    for (name, value) in [
        ("--fault-429-bps", config.fault_429_bps),
        ("--fault-500-bps", config.fault_500_bps),
        ("--fault-timeout-bps", config.fault_timeout_bps),
        (
            "--fault-truncate-stream-bps",
            config.fault_truncate_stream_bps,
        ),
    ] {
        if value > BASIS_POINTS {
            return Err(invalid_config(&format!(
                "{name} must be between 0 and {BASIS_POINTS}"
            )));
        }
    }
    if config.first_byte_jitter > config.first_byte_delay {
        return Err(invalid_config(
            "--first-byte-jitter-ms cannot exceed --first-byte-delay-ms",
        ));
    }
    if config.chunk_delay_jitter > config.chunk_delay {
        return Err(invalid_config(
            "--chunk-delay-jitter-ms cannot exceed --chunk-delay-ms",
        ));
    }
    if config.payload_bytes_jitter > config.payload_bytes {
        return Err(invalid_config(
            "--payload-bytes-jitter cannot exceed --payload-bytes",
        ));
    }
    let first_byte_ms = u64::try_from(config.first_byte_delay.as_millis())
        .ok()
        .and_then(|base| {
            u64::try_from(config.first_byte_jitter.as_millis())
                .ok()
                .and_then(|jitter| base.checked_add(jitter))
                .and_then(|_| {
                    u64::try_from(config.first_byte_jitter.as_millis())
                        .ok()
                        .and_then(|jitter| jitter.checked_mul(2)?.checked_add(1))
                })
        });
    let chunk_delay_ms = u64::try_from(config.chunk_delay.as_millis())
        .ok()
        .and_then(|base| {
            u64::try_from(config.chunk_delay_jitter.as_millis())
                .ok()
                .and_then(|jitter| base.checked_add(jitter))
                .and_then(|_| {
                    u64::try_from(config.chunk_delay_jitter.as_millis())
                        .ok()
                        .and_then(|jitter| jitter.checked_mul(2)?.checked_add(1))
                })
        });
    let payload_bytes = config
        .payload_bytes
        .checked_add(config.payload_bytes_jitter)
        .and_then(|_| {
            (config.payload_bytes_jitter as u64)
                .checked_mul(2)?
                .checked_add(1)
        });
    if first_byte_ms.is_none() || chunk_delay_ms.is_none() || payload_bytes.is_none() {
        return Err(invalid_config("base value plus jitter is too large"));
    }
    if config.fault_timeout_bps > 0 && config.timeout_hold.is_zero() {
        return Err(invalid_config(
            "--timeout-hold-ms must be greater than zero when timeout faults are enabled",
        ));
    }
    if config.status != StatusCode::OK && fault_total > 0 {
        return Err(invalid_config(
            "--status cannot be combined with randomized fault rates; choose one fault mode",
        ));
    }
    Ok(())
}

fn invalid_config(message: &str) -> Box<dyn std::error::Error> {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message).into()
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
        "usage: cargo run -p aether-integration-tests --bin mock_openai_upstream -- [--bind 127.0.0.1:18181]... [--chunks 8] [--first-byte-delay-ms 0] [--first-byte-jitter-ms 0] [--chunk-delay-ms 20] [--chunk-delay-jitter-ms 0] [--payload-bytes 32] [--payload-bytes-jitter 0] [--seed 0] [--fault-429-bps 0] [--fault-500-bps 0] [--fault-timeout-bps 0] [--timeout-hold-ms 60000] [--fault-truncate-stream-bps 0] [--status 200] [--assume-stream]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn parse_error(values: &[&str]) -> String {
        parse_args(args(values))
            .expect_err("arguments should be rejected")
            .to_string()
    }

    #[test]
    fn default_arguments_preserve_the_fixed_profile() {
        let config = parse_args(Vec::new()).expect("default arguments should parse");
        assert_eq!(config, Config::default());

        let profile = request_profile(&config, 17);
        assert_eq!(profile.sequence, 17);
        assert!(!profile.expose_sequence);
        assert_eq!(profile.first_byte_delay, config.first_byte_delay);
        assert_eq!(profile.payload_bytes, config.payload_bytes);
        assert_eq!(profile.fault, Fault::None);
        assert_eq!(profile.truncate_after_chunks, None);
        assert_eq!(chunk_delay_for(&config, 17, 3), config.chunk_delay);
    }

    #[test]
    fn parses_a_complete_random_profile() {
        let config = parse_args(args(&[
            "--chunks",
            "7",
            "--first-byte-delay-ms",
            "100",
            "--first-byte-jitter-ms",
            "30",
            "--chunk-delay-ms",
            "20",
            "--chunk-delay-jitter-ms",
            "5",
            "--payload-bytes",
            "64",
            "--payload-bytes-jitter",
            "12",
            "--seed",
            "42",
            "--fault-429-bps",
            "100",
            "--fault-500-bps",
            "200",
            "--fault-timeout-bps",
            "300",
            "--timeout-hold-ms",
            "90000",
            "--fault-truncate-stream-bps",
            "400",
            "--assume-stream",
        ]))
        .expect("random profile arguments should parse");

        assert_eq!(config.chunks, 7);
        assert_eq!(config.first_byte_delay, Duration::from_millis(100));
        assert_eq!(config.first_byte_jitter, Duration::from_millis(30));
        assert_eq!(config.chunk_delay, Duration::from_millis(20));
        assert_eq!(config.chunk_delay_jitter, Duration::from_millis(5));
        assert_eq!(config.payload_bytes, 64);
        assert_eq!(config.payload_bytes_jitter, 12);
        assert_eq!(config.seed, 42);
        assert_eq!(config.fault_429_bps, 100);
        assert_eq!(config.fault_500_bps, 200);
        assert_eq!(config.fault_timeout_bps, 300);
        assert_eq!(config.fault_truncate_stream_bps, 400);
        assert_eq!(config.timeout_hold, Duration::from_millis(90_000));
        assert!(config.assume_stream);
    }

    #[test]
    fn profile_values_depend_only_on_seed_sequence_and_domain() {
        let mut config = Config::default();
        config.seed = 0xfeed_beef;
        config.first_byte_delay = Duration::from_millis(100);
        config.first_byte_jitter = Duration::from_millis(50);
        config.chunk_delay = Duration::from_millis(30);
        config.chunk_delay_jitter = Duration::from_millis(10);
        config.payload_bytes = 128;
        config.payload_bytes_jitter = 64;
        config.fault_truncate_stream_bps = BASIS_POINTS;
        config.chunks = 9;

        let first = request_profile(&config, 1234);
        let unrelated = request_profile(&config, 9999);
        let second = request_profile(&config, 1234);
        assert_eq!(first, second);
        assert_ne!(first, unrelated);
        assert!(first.expose_sequence);
        assert!((50..=150).contains(&first.first_byte_delay.as_millis()));
        assert!((64..=192).contains(&first.payload_bytes));
        assert_eq!(first.fault, Fault::TruncateStream);
        assert!((1..=config.chunks).contains(
            &first
                .truncate_after_chunks
                .expect("truncation point should be selected")
        ));

        let delay_before = chunk_delay_for(&config, 1234, 7);
        let _ = chunk_delay_for(&config, 9999, 2);
        let delay_after = chunk_delay_for(&config, 1234, 7);
        assert_eq!(delay_before, delay_after);
        assert!((20..=40).contains(&delay_before.as_millis()));
    }

    #[test]
    fn fault_buckets_are_disjoint_and_ordered() {
        let mut config = Config::default();
        config.fault_429_bps = 100;
        config.fault_500_bps = 200;
        config.fault_timeout_bps = 300;
        config.fault_truncate_stream_bps = 400;

        assert_eq!(select_fault(&config, 0), Fault::Status429);
        assert_eq!(select_fault(&config, 99), Fault::Status429);
        assert_eq!(select_fault(&config, 100), Fault::Status500);
        assert_eq!(select_fault(&config, 299), Fault::Status500);
        assert_eq!(select_fault(&config, 300), Fault::Timeout);
        assert_eq!(select_fault(&config, 599), Fault::Timeout);
        assert_eq!(select_fault(&config, 600), Fault::TruncateStream);
        assert_eq!(select_fault(&config, 999), Fault::TruncateStream);
        assert_eq!(select_fault(&config, 1000), Fault::None);
    }

    #[test]
    fn rejects_invalid_random_profile_arguments() {
        assert!(parse_error(&[
            "--first-byte-delay-ms",
            "10",
            "--first-byte-jitter-ms",
            "11"
        ])
        .contains("cannot exceed"));
        assert!(
            parse_error(&["--chunk-delay-ms", "10", "--chunk-delay-jitter-ms", "11"])
                .contains("cannot exceed")
        );
        assert!(
            parse_error(&["--payload-bytes", "10", "--payload-bytes-jitter", "11"])
                .contains("cannot exceed")
        );
        assert!(
            parse_error(&["--fault-429-bps", "5001", "--fault-500-bps", "5000"])
                .contains("at most 10000")
        );
        assert!(
            parse_error(&["--fault-timeout-bps", "1", "--timeout-hold-ms", "0"])
                .contains("greater than zero")
        );
        assert!(parse_error(&["--status", "500", "--fault-429-bps", "1"])
            .contains("cannot be combined"));
    }

    #[test]
    fn fault_metrics_are_kept_separate_globally_and_per_bind() {
        let config = Config::default();
        let bind = config.binds[0];
        let metrics = Arc::new(Metrics::for_binds(&config.binds));
        let app = App {
            config,
            metrics: Arc::clone(&metrics),
            bind_label: Arc::from(bind.to_string()),
        };

        record_fault(&app, Fault::Status429);
        record_fault(&app, Fault::Status500);
        record_fault(&app, Fault::Timeout);
        record_fault(&app, Fault::TruncateStream);
        assert_eq!(metrics.fault_429_total.load(Ordering::Acquire), 1);
        assert_eq!(metrics.fault_500_total.load(Ordering::Acquire), 1);
        assert_eq!(metrics.fault_timeout_total.load(Ordering::Acquire), 1);
        assert_eq!(
            metrics.fault_truncated_stream_total.load(Ordering::Acquire),
            1
        );

        let bind_metrics = metrics
            .bind_metrics(&bind.to_string())
            .expect("bind metrics should exist");
        assert_eq!(bind_metrics.fault_429_total.load(Ordering::Acquire), 1);
        assert_eq!(bind_metrics.fault_500_total.load(Ordering::Acquire), 1);
        assert_eq!(bind_metrics.fault_timeout_total.load(Ordering::Acquire), 1);
        assert_eq!(
            bind_metrics
                .fault_truncated_stream_total
                .load(Ordering::Acquire),
            1
        );
    }

    async fn start_truncating_server() -> (String, Arc<Metrics>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let bind = listener
            .local_addr()
            .expect("test listener should have a local address");
        let mut config = Config::default();
        config.binds = vec![bind];
        config.chunks = 0;
        config.chunk_delay = Duration::ZERO;
        config.fault_truncate_stream_bps = BASIS_POINTS;
        config.seed = 0x1234_5678;
        let metrics = Arc::new(Metrics::for_binds(&config.binds));
        let router = build_router(config, Arc::clone(&metrics), bind);
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test mock server should run");
        });
        (format!("http://{bind}"), metrics, server)
    }

    async fn assert_truncated_response(
        client: &reqwest::Client,
        url: &str,
        expected_version: axum::http::Version,
        completion_marker: &str,
    ) {
        let response = client
            .post(url)
            .json(&json!({"stream": true, "model": "mock-test"}))
            .send()
            .await
            .expect("headers should arrive before the body error");
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.version(), expected_version);
        assert!(response.headers().contains_key(REQUEST_SEQUENCE_HEADER));

        let mut body_stream = response.bytes_stream();
        let mut body = Vec::new();
        let mut saw_body_error = false;
        while let Some(item) = tokio::time::timeout(Duration::from_secs(2), body_stream.next())
            .await
            .expect("truncated response should not stall")
        {
            match item {
                Ok(chunk) => body.extend_from_slice(&chunk),
                Err(_) => {
                    saw_body_error = true;
                    break;
                }
            }
        }
        assert!(
            saw_body_error,
            "truncated stream should surface a body error"
        );
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.contains("data:"),
            "at least one SSE data frame is required"
        );
        assert!(
            !body.contains(completion_marker),
            "truncated stream must not emit its protocol completion event"
        );
        assert!(
            !body.contains("[DONE]"),
            "truncated stream must not emit [DONE]"
        );
    }

    async fn wait_for_completed(metrics: &Metrics, expected: u64) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while metrics.completed_total.load(Ordering::Acquire) < expected
            && tokio::time::Instant::now() < deadline
        {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn truncated_sse_is_a_partial_body_error_over_http1() {
        let (base_url, metrics, server) = start_truncating_server().await;
        let client = reqwest::Client::builder()
            .http1_only()
            .build()
            .expect("HTTP/1 test client should build");

        assert_truncated_response(
            &client,
            &format!("{base_url}/v1/chat/completions"),
            axum::http::Version::HTTP_11,
            "\"finish_reason\":\"stop\"",
        )
        .await;
        assert_truncated_response(
            &client,
            &format!("{base_url}/v1/responses"),
            axum::http::Version::HTTP_11,
            "response.completed",
        )
        .await;

        wait_for_completed(&metrics, 2).await;
        assert_eq!(metrics.requests_total.load(Ordering::Acquire), 2);
        assert_eq!(metrics.completed_total.load(Ordering::Acquire), 2);
        assert_eq!(metrics.in_flight.load(Ordering::Acquire), 0);
        assert_eq!(
            metrics.fault_truncated_stream_total.load(Ordering::Acquire),
            2
        );
        assert_eq!(metrics.first_chunk_yield_total.load(Ordering::Acquire), 2);

        server.abort();
        let _ = server.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn truncated_sse_is_a_partial_body_error_over_h2c() {
        let (base_url, metrics, server) = start_truncating_server().await;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .build()
            .expect("H2C test client should build");

        assert_truncated_response(
            &client,
            &format!("{base_url}/v1/chat/completions"),
            axum::http::Version::HTTP_2,
            "\"finish_reason\":\"stop\"",
        )
        .await;
        assert_truncated_response(
            &client,
            &format!("{base_url}/v1/responses"),
            axum::http::Version::HTTP_2,
            "response.completed",
        )
        .await;

        wait_for_completed(&metrics, 2).await;
        assert_eq!(metrics.requests_total.load(Ordering::Acquire), 2);
        assert_eq!(metrics.completed_total.load(Ordering::Acquire), 2);
        assert_eq!(metrics.in_flight.load(Ordering::Acquire), 0);
        assert_eq!(
            metrics.fault_truncated_stream_total.load(Ordering::Acquire),
            2
        );
        assert_eq!(metrics.first_chunk_yield_total.load(Ordering::Acquire), 2);

        server.abort();
        let _ = server.await;
    }
}
