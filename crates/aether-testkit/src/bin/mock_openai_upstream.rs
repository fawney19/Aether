use std::collections::BTreeMap;
use std::convert::Infallible;
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

#[derive(Debug, Clone)]
struct Config {
    binds: Vec<SocketAddr>,
    chunks: u64,
    first_byte_delay: Duration,
    chunk_delay: Duration,
    payload_bytes: usize,
    status: StatusCode,
    assume_stream: bool,
}

const MAX_MOCK_REQUEST_BODY_BYTES: usize = 1024 * 1024;

impl Default for Config {
    fn default() -> Self {
        Self {
            binds: vec!["127.0.0.1:18181"
                .parse()
                .expect("default bind address should parse")],
            chunks: 8,
            first_byte_delay: Duration::from_millis(0),
            chunk_delay: Duration::from_millis(20),
            payload_bytes: 32,
            status: StatusCode::OK,
            assume_stream: false,
        }
    }
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
            "mock_upstream_response_header_to_first_chunk_max_ms {}\n"
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
                "mock_upstream_response_header_to_first_chunk_max_ms{{bind=\"{}\"}} {}\n"
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
        ));
    }
    (StatusCode::OK, body).into_response()
}

async fn chat_completions(State(app): State<App>, request: axum::extract::Request) -> Response {
    let request_started_at = record_request_started(&app);
    if app.config.status != StatusCode::OK {
        record_response_header_created(&app, request_started_at.elapsed());
        record_request_completed(&app);
        return (app.config.status, "mock upstream error\n").into_response();
    }
    if app.config.assume_stream {
        record_response_header_created(&app, request_started_at.elapsed());
        return build_chat_sse_response(app);
    }

    let body = match read_request_body(&app, request).await {
        Ok(body) => body,
        Err(response) => {
            record_response_header_created(&app, request_started_at.elapsed());
            record_request_completed(&app);
            return response;
        }
    };
    let stream = request_wants_stream(&body);
    if stream {
        record_response_header_created(&app, request_started_at.elapsed());
        return build_chat_sse_response(app);
    }
    let payload = json!({
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "created": current_unix_secs(),
        "model": request_model(&body).unwrap_or_else(|| "mock-model".to_string()),
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": mock_payload(app.config.payload_bytes)
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 1,
            "completion_tokens": app.config.chunks.max(1),
            "total_tokens": app.config.chunks.max(1) + 1
        }
    });
    record_response_header_created(&app, request_started_at.elapsed());
    record_request_completed(&app);
    axum::Json(payload).into_response()
}

async fn responses(State(app): State<App>, request: axum::extract::Request) -> Response {
    let request_started_at = record_request_started(&app);
    if app.config.status != StatusCode::OK {
        record_response_header_created(&app, request_started_at.elapsed());
        record_request_completed(&app);
        return (app.config.status, "mock upstream error\n").into_response();
    }
    if app.config.assume_stream {
        record_response_header_created(&app, request_started_at.elapsed());
        return build_responses_sse_response(app);
    }

    let body = match read_request_body(&app, request).await {
        Ok(body) => body,
        Err(response) => {
            record_response_header_created(&app, request_started_at.elapsed());
            record_request_completed(&app);
            return response;
        }
    };
    let stream = request_wants_stream(&body);
    if stream {
        record_response_header_created(&app, request_started_at.elapsed());
        return build_responses_sse_response(app);
    }
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
                "text": mock_payload(app.config.payload_bytes)
            }]
        }],
        "usage": {
            "input_tokens": 1,
            "output_tokens": app.config.chunks.max(1),
            "total_tokens": app.config.chunks.max(1) + 1
        }
    });
    record_response_header_created(&app, request_started_at.elapsed());
    record_request_completed(&app);
    axum::Json(payload).into_response()
}

fn build_chat_sse_response(app: App) -> Response {
    let response_created_at = Instant::now();
    let config = app.config.clone();
    let mut completion = Some(RequestCompletionGuard::new(app.clone()));
    let stream = async_stream::stream! {
        if !config.first_byte_delay.is_zero() {
            tokio::time::sleep(config.first_byte_delay).await;
        }
        for index in 0..config.chunks {
            if index > 0 && !config.chunk_delay.is_zero() {
                tokio::time::sleep(config.chunk_delay).await;
            }
            let payload = json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion.chunk",
                "created": current_unix_secs(),
                "model": "mock-model",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "content": mock_payload(config.payload_bytes)
                    },
                    "finish_reason": serde_json::Value::Null
                }]
            });
            if index == 0 {
                record_first_chunk_yield(&app, response_created_at.elapsed());
            }
            yield Ok::<Bytes, Infallible>(Bytes::from(format!("data: {payload}\n\n")));
        }
        yield Ok::<Bytes, Infallible>(Bytes::from(
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        ));
        yield Ok::<Bytes, Infallible>(Bytes::from("data: [DONE]\n\n"));
        if let Some(completion) = completion.take() {
            completion.complete();
        }
    };
    sse_response(Body::from_stream(stream))
}

fn build_responses_sse_response(app: App) -> Response {
    let response_created_at = Instant::now();
    let config = app.config.clone();
    let mut completion = Some(RequestCompletionGuard::new(app.clone()));
    let stream = async_stream::stream! {
        if !config.first_byte_delay.is_zero() {
            tokio::time::sleep(config.first_byte_delay).await;
        }
        record_first_chunk_yield(&app, response_created_at.elapsed());
        yield Ok::<Bytes, Infallible>(Bytes::from(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_mock\",\"status\":\"in_progress\"}}\n\n",
        ));
        for index in 0..config.chunks {
            if index > 0 && !config.chunk_delay.is_zero() {
                tokio::time::sleep(config.chunk_delay).await;
            }
            let payload = json!({
                "type": "response.output_text.delta",
                "item_id": "msg_mock",
                "output_index": 0,
                "content_index": 0,
                "delta": mock_payload(config.payload_bytes)
            });
            yield Ok::<Bytes, Infallible>(Bytes::from(format!("event: response.output_text.delta\ndata: {payload}\n\n")));
        }
        yield Ok::<Bytes, Infallible>(Bytes::from(
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_mock\",\"status\":\"completed\"}}\n\n",
        ));
        yield Ok::<Bytes, Infallible>(Bytes::from("data: [DONE]\n\n"));
        if let Some(completion) = completion.take() {
            completion.complete();
        }
    };
    sse_response(Body::from_stream(stream))
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

fn record_request_started(app: &App) -> Instant {
    app.metrics.requests_total.fetch_add(1, Ordering::AcqRel);
    let in_flight = app.metrics.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
    app.metrics
        .max_in_flight
        .fetch_max(in_flight, Ordering::AcqRel);
    if let Some(bind) = app.metrics.bind_metrics(app.bind_label.as_ref()) {
        bind.requests_total.fetch_add(1, Ordering::AcqRel);
        let in_flight = bind.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        bind.max_in_flight.fetch_max(in_flight, Ordering::AcqRel);
    }
    Instant::now()
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
            "--chunk-delay-ms" => {
                config.chunk_delay =
                    Duration::from_millis(next_value(&mut iter, "--chunk-delay-ms")?.parse()?)
            }
            "--payload-bytes" => {
                config.payload_bytes = next_value(&mut iter, "--payload-bytes")?.parse()?
            }
            "--status" => {
                let status = next_value(&mut iter, "--status")?.parse::<u16>()?;
                config.status = StatusCode::from_u16(status)?;
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
        "usage: cargo run -p aether-testkit --bin mock_openai_upstream -- [--bind 127.0.0.1:18181]... [--chunks 8] [--first-byte-delay-ms 0] [--chunk-delay-ms 20] [--payload-bytes 32] [--status 200] [--assume-stream]"
    );
}
