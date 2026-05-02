//! Per-stream request handler.
//!
//! Receives request frames, executes the upstream HTTP request,
//! and sends response frames back through the writer channel.

use std::io;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use aether_runtime::hold_admission_permit_until;
use bytes::{Bytes, BytesMut};
use futures_util::stream;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use hyper::body::Frame as BodyFrame;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn};

use crate::state::{AppState, ServerContext};
use crate::target_filter;
use crate::upstream_client;

use super::protocol::{
    compress_payload, decompress_if_gzip, flags, Frame as TunnelFrame, MsgType, RequestMeta,
    ResponseMeta,
};
use super::writer::FrameSender;

/// Maximum response body chunk size per frame (32 KB).
const MAX_CHUNK_SIZE: usize = 32 * 1024;

/// Timeout for sending a single frame to the writer channel.
/// If the writer is congested (TCP backpressure), we abandon the stream
/// rather than blocking indefinitely and exhausting the stream pool.
const FRAME_SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Minimum allowed upstream request timeout (seconds).
const MIN_TIMEOUT_SECS: u64 = 5;
/// Maximum allowed upstream request timeout (seconds).
const MAX_TIMEOUT_SECS: u64 = 300;
/// Match reqwest's default redirect budget so direct execution and tunnel relay
/// fail at the same point instead of diverging after a different number of hops.
const MAX_REDIRECTS: usize = 10;

/// Headers that must not be forwarded to upstream (hop-by-hop or security-sensitive).
///
/// `host` and `content-length` are managed by the HTTP client (reqwest/hyper):
/// - `host` → translated to `:authority` pseudo-header in HTTP/2; forwarding
///   the original `host` alongside `:authority` triggers PROTOCOL_ERROR on
///   strict H2 implementations (e.g. Google APIs).
/// - `content-length` → recalculated by hyper from the actual body; a stale
///   value from the tunnel (body may have been re-compressed) causes H2
///   PROTOCOL_ERROR when it mismatches the real frame length.
const BLOCKED_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];
const REDIRECT_DROP_BODY_HEADERS: &[&str] = &[
    "content-encoding",
    "content-length",
    "content-type",
    "transfer-encoding",
];
const REDIRECT_SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "cookie2",
    "proxy-authorization",
    "www-authenticate",
];

#[derive(Debug, Clone)]
enum ReplayableRequestBody {
    None,
    Pending(Arc<RequestBodyReplayState>),
    NonReplayable,
}

struct PreparedRequestBody {
    first_request_body: Option<upstream_client::UpstreamRequestBody>,
    replay_body: ReplayableRequestBody,
}

#[derive(Debug)]
struct RequestBodyReplayState {
    budget_bytes: usize,
    state: Mutex<RequestBodyReplayStatus>,
    ready: Notify,
}

#[derive(Debug)]
enum RequestBodyReplayStatus {
    Collecting {
        chunks: Vec<Bytes>,
        buffered_len: usize,
    },
    Ready(Bytes),
    Empty,
    NonReplayable,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplayBodyResolution {
    Empty,
    Replayable(Bytes),
    NonReplayable,
}

#[derive(Debug)]
enum SpoolBodyEvent {
    Data(Bytes),
    Error(String),
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedirectBodyMode {
    Empty,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RedirectDecision {
    Stop,
    Follow {
        method: hyper::Method,
        url: url::Url,
        headers: Vec<(String, String)>,
        body_mode: RedirectBodyMode,
    },
    Error(&'static str),
}

struct UpstreamResponseContext {
    response: hyper::Response<hyper::body::Incoming>,
    dns_ms: u64,
    request_timing: upstream_client::RequestTiming,
}

#[derive(Clone, Copy)]
struct StreamLogContext<'a> {
    server: &'a ServerContext,
    stream_id: u32,
    method: &'a hyper::Method,
    url: Option<&'a url::Url>,
    redirect_count: usize,
    request_body_size: usize,
}

fn parse_request_method(method: &str) -> hyper::Method {
    method.parse().unwrap_or(hyper::Method::GET)
}

fn request_log_host(url: &url::Url) -> &str {
    url.host_str().unwrap_or("")
}

fn request_log_port(url: &url::Url) -> u16 {
    url.port_or_known_default().unwrap_or(0)
}

fn request_log_path(url: &url::Url) -> &str {
    let path = url.path();
    if path.is_empty() {
        "/"
    } else {
        path
    }
}

fn stream_log_context<'a>(
    server: &'a ServerContext,
    stream_id: u32,
    method: &'a hyper::Method,
    url: Option<&'a url::Url>,
    redirect_count: usize,
    request_body_size: usize,
) -> StreamLogContext<'a> {
    StreamLogContext {
        server,
        stream_id,
        method,
        url,
        redirect_count,
        request_body_size,
    }
}

fn log_stream_success(ctx: StreamLogContext<'_>, status: u16, duration: Duration) {
    let url = ctx
        .url
        .expect("successful requests should always have a URL");
    info!(
        server = %ctx.server.server_label,
        stream_id = ctx.stream_id,
        method = %ctx.method,
        scheme = url.scheme(),
        host = request_log_host(url),
        port = request_log_port(url),
        path = request_log_path(url),
        query_present = url.query().is_some(),
        status,
        duration_ms = duration.as_millis() as u64,
        redirect_count = ctx.redirect_count,
        request_body_bytes = ctx.request_body_size,
        "proxy request completed"
    );
}

fn log_stream_failure(ctx: StreamLogContext<'_>, error: &str, duration: Duration) {
    match ctx.url {
        Some(url) => {
            warn!(
                server = %ctx.server.server_label,
                stream_id = ctx.stream_id,
                method = %ctx.method,
                scheme = url.scheme(),
                host = request_log_host(url),
                port = request_log_port(url),
                path = request_log_path(url),
                query_present = url.query().is_some(),
                error = %error,
                duration_ms = duration.as_millis() as u64,
                redirect_count = ctx.redirect_count,
                request_body_bytes = ctx.request_body_size,
                "proxy request failed"
            );
        }
        None => {
            warn!(
                server = %ctx.server.server_label,
                stream_id = ctx.stream_id,
                method = %ctx.method,
                error = %error,
                duration_ms = duration.as_millis() as u64,
                redirect_count = ctx.redirect_count,
                request_body_bytes = ctx.request_body_size,
                "proxy request failed"
            );
        }
    }
}

impl PreparedRequestBody {
    fn take_first_request_body(&mut self) -> upstream_client::UpstreamRequestBody {
        self.first_request_body
            .take()
            .unwrap_or_else(empty_request_body)
    }
}

async fn prepare_redirect_request_body(
    replay_body: ReplayableRequestBody,
    body_mode: RedirectBodyMode,
    deadline: Instant,
) -> Result<Option<upstream_client::UpstreamRequestBody>, String> {
    match body_mode {
        RedirectBodyMode::Empty => Ok(Some(empty_request_body())),
        RedirectBodyMode::Replay => match replay_body {
            ReplayableRequestBody::None => Ok(Some(empty_request_body())),
            ReplayableRequestBody::Pending(state) => {
                match state.wait_for_resolution(deadline).await? {
                    ReplayBodyResolution::Empty => Ok(Some(empty_request_body())),
                    ReplayBodyResolution::Replayable(body) => Ok(Some(buffered_request_body(body))),
                    ReplayBodyResolution::NonReplayable => Ok(None),
                }
            }
            ReplayableRequestBody::NonReplayable => Ok(None),
        },
    }
}

impl RequestBodyReplayState {
    fn new(budget_bytes: usize) -> Self {
        Self {
            budget_bytes,
            state: Mutex::new(RequestBodyReplayStatus::Collecting {
                chunks: Vec::new(),
                buffered_len: 0,
            }),
            ready: Notify::new(),
        }
    }

    fn push_chunk(&self, payload: Bytes) {
        let mut notify = false;
        {
            let mut state = self.state.lock().expect("request body replay state lock");
            if let RequestBodyReplayStatus::Collecting {
                chunks,
                buffered_len,
            } = &mut *state
            {
                let next_len = buffered_len.saturating_add(payload.len());
                if next_len > self.budget_bytes {
                    chunks.clear();
                    *state = RequestBodyReplayStatus::NonReplayable;
                    notify = true;
                } else {
                    *buffered_len = next_len;
                    chunks.push(payload);
                }
            }
        }
        if notify {
            self.ready.notify_waiters();
        }
    }

    fn finish(&self) {
        let notify;
        {
            let mut state = self.state.lock().expect("request body replay state lock");
            let next_state = match std::mem::replace(&mut *state, RequestBodyReplayStatus::Empty) {
                RequestBodyReplayStatus::Collecting {
                    chunks,
                    buffered_len,
                } => {
                    if buffered_len == 0 {
                        RequestBodyReplayStatus::Empty
                    } else {
                        let mut buffered = BytesMut::with_capacity(buffered_len);
                        for chunk in chunks {
                            buffered.extend_from_slice(&chunk);
                        }
                        RequestBodyReplayStatus::Ready(buffered.freeze())
                    }
                }
                terminal => terminal,
            };
            notify = !matches!(next_state, RequestBodyReplayStatus::Collecting { .. });
            *state = next_state;
        }
        if notify {
            self.ready.notify_waiters();
        }
    }

    fn fail(&self, message: String) {
        {
            let mut state = self.state.lock().expect("request body replay state lock");
            *state = RequestBodyReplayStatus::Error(message);
        }
        self.ready.notify_waiters();
    }

    async fn wait_for_resolution(&self, deadline: Instant) -> Result<ReplayBodyResolution, String> {
        loop {
            let resolution = {
                let state = self.state.lock().expect("request body replay state lock");
                match &*state {
                    RequestBodyReplayStatus::Collecting { .. } => None,
                    RequestBodyReplayStatus::Ready(body) => {
                        Some(Ok(ReplayBodyResolution::Replayable(body.clone())))
                    }
                    RequestBodyReplayStatus::Empty => Some(Ok(ReplayBodyResolution::Empty)),
                    RequestBodyReplayStatus::NonReplayable => {
                        Some(Ok(ReplayBodyResolution::NonReplayable))
                    }
                    RequestBodyReplayStatus::Error(message) => Some(Err(message.clone())),
                }
            };
            if let Some(resolution) = resolution {
                return resolution;
            }

            let Some(remaining) = remaining_timeout(deadline) else {
                return Err("upstream timeout".to_string());
            };
            tokio::time::timeout(remaining, self.ready.notified())
                .await
                .map_err(|_| "upstream timeout".to_string())?;
        }
    }
}

fn follow_redirects_enabled(meta: &RequestMeta) -> bool {
    meta.follow_redirects == Some(true)
}

fn request_likely_has_body(
    method: &hyper::Method,
    headers: &std::collections::HashMap<String, String>,
) -> bool {
    if matches!(
        *method,
        hyper::Method::GET | hyper::Method::HEAD | hyper::Method::OPTIONS | hyper::Method::TRACE
    ) {
        return headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("content-length")
                && value
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .is_some_and(|value| value > 0)
        }) || headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("transfer-encoding"));
    }

    true
}

fn sanitize_upstream_headers(
    headers: &std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            if BLOCKED_HEADERS.contains(&normalized.as_str()) {
                None
            } else {
                Some((key.clone(), value.clone()))
            }
        })
        .collect()
}

fn apply_upstream_headers(headers: &mut hyper::HeaderMap, values: &[(String, String)]) {
    for (key, value) in values {
        if let (Ok(name), Ok(value)) = (
            hyper::header::HeaderName::from_bytes(key.as_bytes()),
            hyper::header::HeaderValue::from_str(value),
        ) {
            headers.insert(name, value);
        }
    }
}

fn empty_request_body() -> upstream_client::UpstreamRequestBody {
    upstream_client::stream_request_body(stream::empty::<Result<BodyFrame<Bytes>, io::Error>>())
}

fn buffered_request_body(body: Bytes) -> upstream_client::UpstreamRequestBody {
    upstream_client::full_request_body(body)
}

// Drain tunnel body frames on a detached task so the shared dispatcher is no
// longer coupled to upstream body polling. Redirect replay still reuses a full
// in-memory copy when the request body completes within budget.
fn prepare_request_body(
    body_rx: mpsc::Receiver<TunnelFrame>,
    body_size: Arc<AtomicUsize>,
    deadline: Instant,
    replay_budget_bytes: usize,
) -> PreparedRequestBody {
    let (spool_tx, spool_rx) = mpsc::unbounded_channel();
    let replay_state = if replay_budget_bytes == 0 {
        None
    } else {
        Some(Arc::new(RequestBodyReplayState::new(replay_budget_bytes)))
    };
    let replay_body = match replay_state.as_ref() {
        Some(state) => ReplayableRequestBody::Pending(Arc::clone(state)),
        None => ReplayableRequestBody::NonReplayable,
    };

    tokio::spawn(spool_request_body(
        body_rx,
        spool_tx,
        replay_state,
        body_size,
        deadline,
    ));

    PreparedRequestBody {
        first_request_body: Some(build_spooled_request_body(spool_rx)),
        replay_body,
    }
}

async fn collect_request_body_for_replay(
    mut body_rx: mpsc::Receiver<TunnelFrame>,
    body_size: Arc<AtomicUsize>,
    deadline: Instant,
    replay_budget_bytes: usize,
) -> Result<Bytes, String> {
    let mut body = BytesMut::new();

    loop {
        let frame = recv_body_frame_with_deadline(&mut body_rx, deadline).await?;
        let Some(frame) = frame else {
            return Ok(body.freeze());
        };

        match frame.msg_type {
            MsgType::RequestBody => {
                let end_stream = frame.is_end_stream();
                let payload = decompress_if_gzip(&frame)
                    .map_err(|error| format!("gzip decompress failed: {error}"))?;

                if !payload.is_empty() {
                    if body.len().saturating_add(payload.len()) > replay_budget_bytes {
                        return Err(format!(
                            "request body exceeds redirect replay budget: {} > {}",
                            body.len().saturating_add(payload.len()),
                            replay_budget_bytes
                        ));
                    }
                    body_size.fetch_add(payload.len(), Ordering::Relaxed);
                    body.extend_from_slice(&payload);
                }

                if end_stream {
                    return Ok(body.freeze());
                }
            }
            MsgType::StreamError => {
                return Err(String::from_utf8(frame.payload.to_vec())
                    .unwrap_or_else(|_| "client cancelled request body".to_string()));
            }
            MsgType::StreamEnd => return Ok(body.freeze()),
            _ => continue,
        }
    }
}

fn replay_body_from_buffered(body: Bytes, replay_budget_bytes: usize) -> ReplayableRequestBody {
    let state = Arc::new(RequestBodyReplayState::new(
        replay_budget_bytes.max(body.len()).max(1),
    ));
    if !body.is_empty() {
        state.push_chunk(body);
    }
    state.finish();
    ReplayableRequestBody::Pending(state)
}

async fn recv_body_frame_with_deadline(
    body_rx: &mut mpsc::Receiver<TunnelFrame>,
    deadline: Instant,
) -> Result<Option<TunnelFrame>, String> {
    let Some(remaining) = remaining_timeout(deadline) else {
        return Err("upstream timeout".to_string());
    };
    tokio::time::timeout(remaining, body_rx.recv())
        .await
        .map_err(|_| "upstream timeout".to_string())
}

fn remaining_timeout(deadline: Instant) -> Option<Duration> {
    deadline.checked_duration_since(Instant::now())
}

async fn spool_request_body(
    mut body_rx: mpsc::Receiver<TunnelFrame>,
    spool_tx: mpsc::UnboundedSender<SpoolBodyEvent>,
    replay_state: Option<Arc<RequestBodyReplayState>>,
    body_size: Arc<AtomicUsize>,
    deadline: Instant,
) {
    let mut spool_tx = Some(spool_tx);
    loop {
        let frame = match recv_body_frame_with_deadline(&mut body_rx, deadline).await {
            Ok(frame) => frame,
            Err(message) => {
                if let Some(state) = &replay_state {
                    state.fail(message.clone());
                }
                send_spool_event(&mut spool_tx, SpoolBodyEvent::Error(message));
                return;
            }
        };

        let Some(frame) = frame else {
            if let Some(state) = &replay_state {
                state.finish();
            }
            send_spool_event(&mut spool_tx, SpoolBodyEvent::End);
            return;
        };

        match frame.msg_type {
            MsgType::RequestBody => {
                let end_stream = frame.is_end_stream();
                let payload = match decompress_if_gzip(&frame) {
                    Ok(payload) => payload,
                    Err(error) => {
                        let message = format!("gzip decompress failed: {error}");
                        if let Some(state) = &replay_state {
                            state.fail(message.clone());
                        }
                        send_spool_event(&mut spool_tx, SpoolBodyEvent::Error(message));
                        return;
                    }
                };

                if !payload.is_empty() {
                    body_size.fetch_add(payload.len(), Ordering::Relaxed);
                    if let Some(state) = &replay_state {
                        state.push_chunk(payload.clone());
                    }
                    send_spool_event(&mut spool_tx, SpoolBodyEvent::Data(payload));
                }

                if end_stream {
                    if let Some(state) = &replay_state {
                        state.finish();
                    }
                    send_spool_event(&mut spool_tx, SpoolBodyEvent::End);
                    return;
                }
            }
            MsgType::StreamError => {
                let message = String::from_utf8(frame.payload.to_vec())
                    .unwrap_or_else(|_| "client cancelled request body".to_string());
                if let Some(state) = &replay_state {
                    state.fail(message.clone());
                }
                send_spool_event(&mut spool_tx, SpoolBodyEvent::Error(message));
                return;
            }
            MsgType::StreamEnd => {
                if let Some(state) = &replay_state {
                    state.finish();
                }
                send_spool_event(&mut spool_tx, SpoolBodyEvent::End);
                return;
            }
            _ => continue,
        }
    }
}

fn send_spool_event(
    spool_tx: &mut Option<mpsc::UnboundedSender<SpoolBodyEvent>>,
    event: SpoolBodyEvent,
) {
    if let Some(sender) = spool_tx.as_ref() {
        if sender.send(event).is_err() {
            *spool_tx = None;
        }
    }
}

fn remove_headers_case_insensitive(headers: &mut Vec<(String, String)>, blocked: &[&str]) {
    headers.retain(|(name, _)| {
        let normalized = name.to_ascii_lowercase();
        !blocked.contains(&normalized.as_str())
    });
}

fn strip_sensitive_headers_for_redirect(
    headers: &mut Vec<(String, String)>,
    next: &url::Url,
    previous: &url::Url,
) {
    let cross_host = next.host_str() != previous.host_str()
        || next.port_or_known_default() != previous.port_or_known_default();
    if cross_host {
        remove_headers_case_insensitive(headers, REDIRECT_SENSITIVE_HEADERS);
    }
}

fn resolve_redirect<B>(
    response: &hyper::Response<B>,
    current_url: &url::Url,
    current_method: &hyper::Method,
    current_headers: &[(String, String)],
    replay_body: &ReplayableRequestBody,
    redirects_followed: usize,
) -> RedirectDecision {
    use hyper::StatusCode;

    let mut next_method = current_method.clone();
    let mut next_headers = current_headers.to_vec();
    let body_mode = match response.status() {
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER => {
            remove_headers_case_insensitive(&mut next_headers, REDIRECT_DROP_BODY_HEADERS);
            if next_method != hyper::Method::GET && next_method != hyper::Method::HEAD {
                next_method = hyper::Method::GET;
            }
            RedirectBodyMode::Empty
        }
        StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT => match replay_body {
            ReplayableRequestBody::NonReplayable => return RedirectDecision::Stop,
            ReplayableRequestBody::None | ReplayableRequestBody::Pending(_) => {
                RedirectBodyMode::Replay
            }
        },
        _ => return RedirectDecision::Stop,
    };

    let Some(location) = response.headers().get(hyper::header::LOCATION) else {
        return RedirectDecision::Stop;
    };
    let Ok(location) = location.to_str() else {
        return RedirectDecision::Stop;
    };
    let Ok(next_url) = current_url.join(location) else {
        return RedirectDecision::Stop;
    };
    match next_url.scheme() {
        "http" | "https" => {}
        _ => return RedirectDecision::Stop,
    }

    if redirects_followed >= MAX_REDIRECTS {
        return RedirectDecision::Error("too many redirects");
    }

    strip_sensitive_headers_for_redirect(&mut next_headers, &next_url, current_url);
    RedirectDecision::Follow {
        method: next_method,
        url: next_url,
        headers: next_headers,
        body_mode,
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_upstream_request(
    state: &AppState,
    server: &ServerContext,
    current_url: &url::Url,
    method: hyper::Method,
    headers: &[(String, String)],
    request_body: upstream_client::UpstreamRequestBody,
    timeout: Duration,
    http1_only: bool,
) -> Result<UpstreamResponseContext, String> {
    let host = current_url
        .host_str()
        .ok_or_else(|| "missing host in URL".to_string())?;
    let port = current_url.port_or_known_default().unwrap_or(443);

    let dns_start = Instant::now();
    {
        let allowed_ports = Arc::clone(&server.dynamic.load().allowed_ports);
        if let Err(error) = target_filter::validate_target(
            host,
            port,
            &allowed_ports,
            state.config.allow_private_targets,
            &state.dns_cache,
        )
        .await
        {
            server.metrics.dns_failures.fetch_add(1, Ordering::Release);
            return Err(format!("target blocked: {error}"));
        }
    }
    let dns_ms = dns_start.elapsed().as_millis() as u64;

    let client = if http1_only {
        &state.upstream_http1_client
    } else {
        &state.upstream_client
    };

    let mut request = hyper::Request::builder()
        .method(method)
        .uri(current_url.as_str())
        .body(request_body)
        .map_err(|error| format!("invalid upstream request: {error}"))?;
    apply_upstream_headers(request.headers_mut(), headers);

    let connection_start = Instant::now();
    let mut captured_connection = upstream_client::capture_connection(&mut request);
    let connection_capture = tokio::spawn(async move {
        let connected = captured_connection.wait_for_connection_metadata().await;
        connected
            .as_ref()
            .map(|_| connection_start.elapsed().as_millis() as u64)
    });

    let response = match tokio::time::timeout(timeout, client.request(request)).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            connection_capture.abort();
            server
                .metrics
                .failed_requests
                .fetch_add(1, Ordering::Release);
            let message = if error.is_connect() {
                format!("upstream connect error: {error}")
            } else {
                format!("upstream error: {error}")
            };
            return Err(message);
        }
        Err(_) => {
            connection_capture.abort();
            server
                .metrics
                .failed_requests
                .fetch_add(1, Ordering::Release);
            return Err("upstream timeout".to_string());
        }
    };

    let connection_acquire_ms =
        match tokio::time::timeout(Duration::from_millis(100), connection_capture).await {
            Ok(Ok(ms)) => ms,
            Ok(Err(_)) => None,
            Err(_) => None,
        };
    let request_timing = upstream_client::resolve_request_timing(
        &response,
        connection_acquire_ms,
        connection_start.elapsed().as_millis() as u64,
    );

    Ok(UpstreamResponseContext {
        response,
        dns_ms,
        request_timing,
    })
}

#[allow(clippy::too_many_arguments)]
async fn relay_upstream_response(
    server: &ServerContext,
    stream_id: u32,
    method: &hyper::Method,
    request_url: &url::Url,
    frame_tx: &FrameSender,
    response: hyper::Response<hyper::body::Incoming>,
    total_dns_ms: u64,
    total_elapsed: Duration,
    request_timing: upstream_client::RequestTiming,
    request_body_size: &AtomicUsize,
    redirect_count: usize,
    request_body_mode: &'static str,
) -> Option<Duration> {
    let status = response.status().as_u16();
    let ttfb_ms = total_elapsed.as_millis() as u64;
    let mut resp_headers: Vec<(String, String)> = Vec::with_capacity(response.headers().len() + 1);
    for (key, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            resp_headers.push((key.as_str().to_string(), value.to_string()));
        }
    }
    let timing = serde_json::json!({
        "dns_ms": total_dns_ms,
        "connection_acquire_ms": request_timing.connection_acquire_ms,
        "connection_reused": request_timing.connection_reused,
        "connect_ms": request_timing.connect_ms,
        "tls_ms": request_timing.tls_ms,
        "ttfb_ms": ttfb_ms,
        "upstream_ms": ttfb_ms,
        "response_wait_ms": request_timing.response_wait_ms,
        "upstream_processing_ms": request_timing.response_wait_ms,
        "timing_source": "instrumented_connector",
        "total_ms": total_elapsed.as_millis() as u64,
        "body_size": request_body_size.load(Ordering::Relaxed),
        "request_body_mode": request_body_mode,
        "mode": "tunnel",
        "redirect_count": redirect_count,
    });
    resp_headers.push(("x-proxy-timing".to_string(), timing.to_string()));
    let resp_meta = ResponseMeta {
        status,
        headers: resp_headers,
    };
    let meta_json: Bytes = serde_json::to_vec(&resp_meta).unwrap_or_default().into();
    let (meta_payload, meta_flags) = compress_payload(meta_json);
    if !send_frame(
        frame_tx,
        TunnelFrame::new(
            stream_id,
            MsgType::ResponseHeaders,
            meta_flags,
            meta_payload,
        ),
    )
    .await
    {
        log_stream_failure(
            stream_log_context(
                server,
                stream_id,
                method,
                Some(request_url),
                redirect_count,
                request_body_size.load(Ordering::Relaxed),
            ),
            "tunnel response headers relay failed",
            total_elapsed,
        );
        return Some(total_elapsed);
    }

    let mut stream = response.into_body().into_data_stream();
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                if chunk.len() <= MAX_CHUNK_SIZE {
                    let (payload, extra_flags) = compress_payload(chunk);
                    if !send_frame(
                        frame_tx,
                        TunnelFrame::new(stream_id, MsgType::ResponseBody, extra_flags, payload),
                    )
                    .await
                    {
                        log_stream_failure(
                            stream_log_context(
                                server,
                                stream_id,
                                method,
                                Some(request_url),
                                redirect_count,
                                request_body_size.load(Ordering::Relaxed),
                            ),
                            "tunnel response body relay failed",
                            total_elapsed,
                        );
                        return Some(total_elapsed);
                    }
                } else {
                    let mut offset = 0;
                    while offset < chunk.len() {
                        let end = (offset + MAX_CHUNK_SIZE).min(chunk.len());
                        let slice = chunk.slice(offset..end);
                        let (payload, extra_flags) = compress_payload(slice);
                        if !send_frame(
                            frame_tx,
                            TunnelFrame::new(
                                stream_id,
                                MsgType::ResponseBody,
                                extra_flags,
                                payload,
                            ),
                        )
                        .await
                        {
                            log_stream_failure(
                                stream_log_context(
                                    server,
                                    stream_id,
                                    method,
                                    Some(request_url),
                                    redirect_count,
                                    request_body_size.load(Ordering::Relaxed),
                                ),
                                "tunnel response body relay failed",
                                total_elapsed,
                            );
                            return Some(total_elapsed);
                        }
                        offset = end;
                    }
                }
            }
            Err(error) => {
                server.metrics.stream_errors.fetch_add(1, Ordering::Release);
                warn!(stream_id, error = %error, "upstream body read error");
                let error_message = format!("upstream body read error: {error}");
                log_stream_failure(
                    stream_log_context(
                        server,
                        stream_id,
                        method,
                        Some(request_url),
                        redirect_count,
                        request_body_size.load(Ordering::Relaxed),
                    ),
                    &error_message,
                    total_elapsed,
                );
                send_error(frame_tx, stream_id, &format!("body read error: {error}")).await;
                return Some(total_elapsed);
            }
        }
    }

    if !send_frame(
        frame_tx,
        TunnelFrame::new(
            stream_id,
            MsgType::StreamEnd,
            flags::END_STREAM,
            Bytes::new(),
        ),
    )
    .await
    {
        log_stream_failure(
            stream_log_context(
                server,
                stream_id,
                method,
                Some(request_url),
                redirect_count,
                request_body_size.load(Ordering::Relaxed),
            ),
            "tunnel stream end relay failed",
            total_elapsed,
        );
        return Some(total_elapsed);
    }

    debug!(
        stream_id,
        status,
        redirects = redirect_count,
        "stream completed"
    );
    log_stream_success(
        stream_log_context(
            server,
            stream_id,
            method,
            Some(request_url),
            redirect_count,
            request_body_size.load(Ordering::Relaxed),
        ),
        status,
        total_elapsed,
    );
    Some(total_elapsed)
}

#[cfg(test)]
fn upstream_client_for_request<'a>(
    state: &'a AppState,
    meta: &RequestMeta,
) -> &'a upstream_client::UpstreamClient {
    if meta.http1_only {
        &state.upstream_http1_client
    } else {
        &state.upstream_client
    }
}

/// Handle a single stream: receive body, execute upstream, send response.
pub async fn handle_stream(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    stream_id: u32,
    meta: RequestMeta,
    body_rx: mpsc::Receiver<TunnelFrame>,
    frame_tx: FrameSender,
) {
    let request_method = parse_request_method(&meta.method);
    let request_url = url::Url::parse(&meta.url).ok();
    let permit = match state.try_acquire_stream_permit().await {
        Ok(permit) => permit,
        Err(err) => {
            let message = match err {
                crate::state::ProxyAdmissionError::Saturated { .. } => "proxy overloaded",
                crate::state::ProxyAdmissionError::Unavailable { .. } => {
                    "proxy admission unavailable"
                }
            };
            log_stream_failure(
                stream_log_context(
                    &server,
                    stream_id,
                    &request_method,
                    request_url.as_ref(),
                    0,
                    0,
                ),
                message,
                Duration::ZERO,
            );
            send_error(&frame_tx, stream_id, message).await;
            return;
        }
    };

    server.active_connections.fetch_add(1, Ordering::Release);

    let connect_elapsed = hold_admission_permit_until(permit, async {
        handle_stream_inner(&state, &server, stream_id, meta, body_rx, &frame_tx).await
    })
    .await;

    server.active_connections.fetch_sub(1, Ordering::Release);
    if let Some(d) = connect_elapsed {
        server.metrics.record_request(d);
    }
}

/// Send a frame to the writer with a timeout. Returns false if send failed.
async fn send_frame(tx: &FrameSender, frame: TunnelFrame) -> bool {
    match tokio::time::timeout(FRAME_SEND_TIMEOUT, tx.send(frame)).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => {
            // Channel closed (writer exited)
            false
        }
        Err(_) => {
            // Timeout — writer is congested
            warn!("frame send timeout (writer congested), abandoning stream");
            false
        }
    }
}

/// Returns the connection-establishment duration (DNS + TCP/TLS + TTFB) if the
/// upstream request succeeded, or `None` if the request never reached the
/// response-headers stage.
async fn handle_stream_inner(
    state: &AppState,
    server: &ServerContext,
    stream_id: u32,
    meta: RequestMeta,
    body_rx: mpsc::Receiver<TunnelFrame>,
    frame_tx: &FrameSender,
) -> Option<Duration> {
    let mut current_method: hyper::Method = parse_request_method(&meta.method);
    let mut current_url = match url::Url::parse(&meta.url) {
        Ok(u) => u,
        Err(e) => {
            log_stream_failure(
                stream_log_context(server, stream_id, &current_method, None, 0, 0),
                &format!("invalid URL: {e}"),
                Duration::ZERO,
            );
            send_error(frame_tx, stream_id, &format!("invalid URL: {e}")).await;
            return None;
        }
    };

    // Only allow http/https schemes (block file://, data://, etc.)
    match current_url.scheme() {
        "http" | "https" => {}
        other => {
            let error_message = format!("unsupported URL scheme: {other}");
            log_stream_failure(
                stream_log_context(server, stream_id, &current_method, Some(&current_url), 0, 0),
                &error_message,
                Duration::ZERO,
            );
            send_error(frame_tx, stream_id, &error_message).await;
            return None;
        }
    }

    let deadline = Instant::now()
        + Duration::from_secs(meta.timeout.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS));
    let follow_redirects = follow_redirects_enabled(&meta);
    let mut current_headers = sanitize_upstream_headers(&meta.headers);
    let timeout = Duration::from_secs(meta.timeout.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS));
    let request_body_size = Arc::new(AtomicUsize::new(0));
    let request_has_body = request_likely_has_body(&current_method, &meta.headers);
    let replay_budget_bytes = state.config.redirect_replay_budget_bytes;
    let can_buffer_redirect_body = request_has_body && follow_redirects && replay_budget_bytes > 0;
    let overall_start = Instant::now();
    let request_body_mode = if can_buffer_redirect_body {
        "buffered_fixed"
    } else if request_has_body {
        "streaming"
    } else {
        "empty"
    };
    let mut prepared_body = if can_buffer_redirect_body {
        let buffered_body = match collect_request_body_for_replay(
            body_rx,
            Arc::clone(&request_body_size),
            deadline,
            replay_budget_bytes,
        )
        .await
        {
            Ok(body) => body,
            Err(message) => {
                log_stream_failure(
                    stream_log_context(
                        server,
                        stream_id,
                        &current_method,
                        Some(&current_url),
                        0,
                        request_body_size.load(Ordering::Relaxed),
                    ),
                    &message,
                    overall_start.elapsed(),
                );
                send_error(frame_tx, stream_id, &message).await;
                return None;
            }
        };
        PreparedRequestBody {
            first_request_body: Some(buffered_request_body(buffered_body.clone())),
            replay_body: replay_body_from_buffered(buffered_body, replay_budget_bytes),
        }
    } else if request_has_body {
        prepare_request_body(body_rx, Arc::clone(&request_body_size), deadline, 0)
    } else {
        PreparedRequestBody {
            first_request_body: Some(build_streaming_request_body(
                body_rx,
                Arc::clone(&request_body_size),
            )),
            replay_body: if follow_redirects {
                ReplayableRequestBody::None
            } else {
                ReplayableRequestBody::NonReplayable
            },
        }
    };

    let mut total_dns_ms = 0u64;
    let mut redirects_followed = 0usize;
    let mut next_request_body = None::<upstream_client::UpstreamRequestBody>;

    loop {
        let Some(remaining) = remaining_timeout(deadline) else {
            log_stream_failure(
                stream_log_context(
                    server,
                    stream_id,
                    &current_method,
                    Some(&current_url),
                    redirects_followed,
                    request_body_size.load(Ordering::Relaxed),
                ),
                "upstream timeout",
                overall_start.elapsed(),
            );
            send_error(frame_tx, stream_id, "upstream timeout").await;
            return None;
        };
        let request_body = next_request_body
            .take()
            .unwrap_or_else(|| prepared_body.take_first_request_body());

        let response_ctx = match execute_upstream_request(
            state,
            server,
            &current_url,
            current_method.clone(),
            &current_headers,
            request_body,
            remaining.min(timeout),
            meta.http1_only,
        )
        .await
        {
            Ok(context) => context,
            Err(message) => {
                log_stream_failure(
                    stream_log_context(
                        server,
                        stream_id,
                        &current_method,
                        Some(&current_url),
                        redirects_followed,
                        request_body_size.load(Ordering::Relaxed),
                    ),
                    &message,
                    overall_start.elapsed(),
                );
                send_error(frame_tx, stream_id, &message).await;
                return None;
            }
        };
        total_dns_ms = total_dns_ms.saturating_add(response_ctx.dns_ms);

        if follow_redirects {
            match resolve_redirect(
                &response_ctx.response,
                &current_url,
                &current_method,
                &current_headers,
                &prepared_body.replay_body,
                redirects_followed,
            ) {
                RedirectDecision::Stop => {
                    return relay_upstream_response(
                        server,
                        stream_id,
                        &current_method,
                        &current_url,
                        frame_tx,
                        response_ctx.response,
                        total_dns_ms,
                        overall_start.elapsed(),
                        response_ctx.request_timing,
                        request_body_size.as_ref(),
                        redirects_followed,
                        request_body_mode,
                    )
                    .await;
                }
                RedirectDecision::Follow {
                    method,
                    url,
                    headers,
                    body_mode,
                } => match prepare_redirect_request_body(
                    prepared_body.replay_body.clone(),
                    body_mode,
                    deadline,
                )
                .await
                {
                    Ok(Some(body)) => {
                        redirects_followed += 1;
                        current_method = method;
                        current_url = url;
                        current_headers = headers;
                        next_request_body = Some(body);
                        continue;
                    }
                    Ok(None) => {
                        return relay_upstream_response(
                            server,
                            stream_id,
                            &current_method,
                            &current_url,
                            frame_tx,
                            response_ctx.response,
                            total_dns_ms,
                            overall_start.elapsed(),
                            response_ctx.request_timing,
                            request_body_size.as_ref(),
                            redirects_followed,
                            request_body_mode,
                        )
                        .await;
                    }
                    Err(message) => {
                        log_stream_failure(
                            stream_log_context(
                                server,
                                stream_id,
                                &current_method,
                                Some(&current_url),
                                redirects_followed,
                                request_body_size.load(Ordering::Relaxed),
                            ),
                            &message,
                            overall_start.elapsed(),
                        );
                        send_error(frame_tx, stream_id, &message).await;
                        return None;
                    }
                },
                RedirectDecision::Error(message) => {
                    let error_message = format!("upstream redirect error: {message}");
                    log_stream_failure(
                        stream_log_context(
                            server,
                            stream_id,
                            &current_method,
                            Some(&current_url),
                            redirects_followed,
                            request_body_size.load(Ordering::Relaxed),
                        ),
                        &error_message,
                        overall_start.elapsed(),
                    );
                    send_error(frame_tx, stream_id, &error_message).await;
                    return None;
                }
            }
        }

        return relay_upstream_response(
            server,
            stream_id,
            &current_method,
            &current_url,
            frame_tx,
            response_ctx.response,
            total_dns_ms,
            overall_start.elapsed(),
            response_ctx.request_timing,
            request_body_size.as_ref(),
            redirects_followed,
            request_body_mode,
        )
        .await;
    }
}

async fn send_error(tx: &FrameSender, stream_id: u32, msg: &str) {
    // Error frames use best-effort delivery — don't block if writer is congested
    let _ = send_frame(
        tx,
        TunnelFrame::new(
            stream_id,
            MsgType::StreamError,
            0,
            Bytes::from(msg.to_string()),
        ),
    )
    .await;
}

fn build_streaming_request_body(
    body_rx: mpsc::Receiver<TunnelFrame>,
    body_size: Arc<AtomicUsize>,
) -> upstream_client::UpstreamRequestBody {
    build_prefixed_request_body(Vec::new(), body_rx, body_size)
}

fn build_spooled_request_body(
    spool_rx: mpsc::UnboundedReceiver<SpoolBodyEvent>,
) -> upstream_client::UpstreamRequestBody {
    let body_stream = stream::unfold((spool_rx, false), |(mut spool_rx, finished)| async move {
        if finished {
            return None;
        }

        match spool_rx.recv().await {
            Some(SpoolBodyEvent::Data(payload)) => {
                Some((Ok(BodyFrame::data(payload)), (spool_rx, false)))
            }
            Some(SpoolBodyEvent::Error(message)) => {
                Some((Err(io::Error::other(message)), (spool_rx, true)))
            }
            Some(SpoolBodyEvent::End) | None => None,
        }
    });

    upstream_client::stream_request_body(body_stream)
}

fn build_prefixed_request_body(
    prefix_chunks: Vec<Bytes>,
    body_rx: mpsc::Receiver<TunnelFrame>,
    body_size: Arc<AtomicUsize>,
) -> upstream_client::UpstreamRequestBody {
    let prefix_stream = stream::iter(
        prefix_chunks
            .into_iter()
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| Ok(BodyFrame::data(chunk))),
    );
    let body_stream = stream::unfold(
        (body_rx, body_size, false),
        |(mut body_rx, body_size, finished)| async move {
            if finished {
                return None;
            }

            loop {
                let frame = match body_rx.recv().await {
                    Some(frame) => frame,
                    None => return None,
                };

                match frame.msg_type {
                    MsgType::RequestBody => {
                        let end_stream = frame.is_end_stream();
                        let payload = match decompress_if_gzip(&frame) {
                            Ok(payload) => payload,
                            Err(error) => {
                                let err =
                                    io::Error::other(format!("gzip decompress failed: {error}"));
                                return Some((Err(err), (body_rx, body_size, true)));
                            }
                        };

                        if payload.is_empty() {
                            if end_stream {
                                return None;
                            }
                            continue;
                        }

                        body_size.fetch_add(payload.len(), Ordering::Relaxed);
                        return Some((
                            Ok(BodyFrame::data(payload)),
                            (body_rx, body_size, end_stream),
                        ));
                    }
                    MsgType::StreamError => {
                        let message = String::from_utf8(frame.payload.to_vec())
                            .unwrap_or_else(|_| "client cancelled request body".to_string());
                        return Some((Err(io::Error::other(message)), (body_rx, body_size, true)));
                    }
                    MsgType::StreamEnd => return None,
                    _ => continue,
                }
            }
        },
    );

    upstream_client::stream_request_body(prefix_stream.chain(body_stream))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Mutex, Once};
    use std::task::{Context, Poll};

    use aether_runtime::{ConcurrencyGate, DistributedConcurrencyGate};
    use arc_swap::ArcSwap;
    use axum::body::Body;
    use axum::http::{header, HeaderMap, Response, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use futures_util::Sink;
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};

    use super::*;
    use crate::config::Config;
    use crate::registration::client::AetherClient;
    use crate::runtime::DynamicConfig;
    use crate::state::ProxyMetrics;
    use crate::target_filter::DnsCache;
    use crate::tunnel::client::build_tls_config;

    fn completed_replay_body(body: Bytes) -> ReplayableRequestBody {
        let state = Arc::new(RequestBodyReplayState::new(body.len().max(1)));
        if !body.is_empty() {
            state.push_chunk(body);
        }
        state.finish();
        ReplayableRequestBody::Pending(state)
    }

    #[tokio::test]
    async fn streaming_request_body_yields_chunks_and_tracks_size() {
        let (tx, rx) = mpsc::channel(4);
        let body_size = Arc::new(AtomicUsize::new(0));
        let mut body = build_streaming_request_body(rx, Arc::clone(&body_size));

        tx.send(TunnelFrame::new(
            1,
            MsgType::RequestBody,
            0,
            Bytes::from_static(b"abc"),
        ))
        .await
        .expect("send first chunk");
        tx.send(TunnelFrame::new(
            1,
            MsgType::RequestBody,
            flags::END_STREAM,
            Bytes::from_static(b"def"),
        ))
        .await
        .expect("send final chunk");
        drop(tx);

        let first = body
            .frame()
            .await
            .expect("first frame")
            .expect("first frame ok")
            .into_data()
            .expect("first data frame");
        let second = body
            .frame()
            .await
            .expect("second frame")
            .expect("second frame ok")
            .into_data()
            .expect("second data frame");

        assert_eq!(first, Bytes::from_static(b"abc"));
        assert_eq!(second, Bytes::from_static(b"def"));
        assert!(body.frame().await.is_none());
        assert_eq!(body_size.load(Ordering::Relaxed), 6);
    }

    #[tokio::test]
    async fn streaming_request_body_surfaces_client_cancel_as_error() {
        let (tx, rx) = mpsc::channel(4);
        let body_size = Arc::new(AtomicUsize::new(0));
        let mut body = build_streaming_request_body(rx, Arc::clone(&body_size));

        tx.send(TunnelFrame::new(
            1,
            MsgType::StreamError,
            0,
            Bytes::from_static(b"client cancelled"),
        ))
        .await
        .expect("send cancel frame");
        drop(tx);

        let err = body
            .frame()
            .await
            .expect("error frame present")
            .expect_err("body should surface cancellation error");
        assert!(err.to_string().contains("client cancelled"));
        assert!(body.frame().await.is_none());
        assert_eq!(body_size.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn prepare_request_body_streams_immediately_and_replays_after_completion() {
        let (tx, rx) = mpsc::channel(4);
        let body_size = Arc::new(AtomicUsize::new(0));
        let prepared = prepare_request_body(
            rx,
            Arc::clone(&body_size),
            Instant::now() + Duration::from_secs(1),
            1024,
        );
        let mut body = prepared
            .first_request_body
            .expect("first request body should be present");

        tx.send(TunnelFrame::new(
            1,
            MsgType::RequestBody,
            0,
            Bytes::from_static(b"hello "),
        ))
        .await
        .expect("send first chunk");

        let first = body
            .frame()
            .await
            .expect("first frame should exist")
            .expect("first frame should be ok")
            .into_data()
            .expect("first data frame");
        assert_eq!(first, Bytes::from_static(b"hello "));

        tx.send(TunnelFrame::new(
            1,
            MsgType::RequestBody,
            flags::END_STREAM,
            Bytes::from_static(b"world"),
        ))
        .await
        .expect("send final chunk");
        drop(tx);

        let second = body
            .frame()
            .await
            .expect("second frame should exist")
            .expect("second frame should be ok")
            .into_data()
            .expect("second data frame");
        assert_eq!(second, Bytes::from_static(b"world"));
        assert!(body.frame().await.is_none());

        let mut replay = prepare_redirect_request_body(
            prepared.replay_body.clone(),
            RedirectBodyMode::Replay,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .expect("redirect replay should resolve")
        .expect("body should be replayable");
        let frame = replay
            .frame()
            .await
            .expect("replayed frame should exist")
            .expect("replayed frame should be ok");
        assert_eq!(
            frame.into_data().expect("data frame"),
            Bytes::from_static(b"hello world")
        );
        assert!(replay.frame().await.is_none());
        assert_eq!(body_size.load(Ordering::Relaxed), 11);
    }

    #[test]
    fn selects_http1_only_client_when_request_metadata_requires_it() {
        let state = sample_state(None, None);
        let default_meta = sample_request_meta();
        assert!(std::ptr::eq(
            upstream_client_for_request(state.as_ref(), &default_meta),
            &state.upstream_client
        ));

        let mut http1_meta = sample_request_meta();
        http1_meta.http1_only = true;
        assert!(std::ptr::eq(
            upstream_client_for_request(state.as_ref(), &http1_meta),
            &state.upstream_http1_client
        ));
    }

    #[test]
    fn resolve_redirect_changes_post_to_get_for_302() {
        let current_url = url::Url::parse("https://redirect.test/start").expect("url");
        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, "/final")
            .body(())
            .expect("response");

        let decision = resolve_redirect(
            &response,
            &current_url,
            &hyper::Method::POST,
            &[("content-type".into(), "application/json".into())],
            &completed_replay_body(Bytes::from_static(br#"{"ok":true}"#)),
            0,
        );

        match decision {
            RedirectDecision::Follow {
                method,
                url,
                headers,
                body_mode,
            } => {
                assert_eq!(method, hyper::Method::GET);
                assert_eq!(url.as_str(), "https://redirect.test/final");
                assert_eq!(body_mode, RedirectBodyMode::Empty);
                assert!(!headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("content-type")));
            }
            other => panic!("unexpected redirect decision: {other:?}"),
        }
    }

    #[test]
    fn resolve_redirect_strips_sensitive_headers_for_cross_host_redirect() {
        let current_url = url::Url::parse("https://redirect-a.test/start").expect("url");
        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, "https://redirect-b.test/final")
            .body(())
            .expect("response");

        let decision = resolve_redirect(
            &response,
            &current_url,
            &hyper::Method::GET,
            &[
                ("authorization".into(), "Bearer secret".into()),
                ("cookie".into(), "sid=123".into()),
                ("x-custom".into(), "keep".into()),
            ],
            &ReplayableRequestBody::None,
            0,
        );

        match decision {
            RedirectDecision::Follow { headers, .. } => {
                assert!(!headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("authorization")));
                assert!(!headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("cookie")));
                assert!(headers
                    .iter()
                    .any(|(name, value)| name.eq_ignore_ascii_case("x-custom") && value == "keep"));
            }
            other => panic!("unexpected redirect decision: {other:?}"),
        }
    }

    #[tokio::test]
    async fn preserves_redirect_response_by_default_when_follow_redirects_unspecified() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new().route(
            "/start",
            get(|| async {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, "/final")
                    .body(Body::empty())
                    .expect("redirect response")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let host = "redirect-default-disabled.test";
        let state = sample_state_for_port(addr.port());
        cache_test_host(&state, host, addr).await;
        let server_ctx = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (_body_tx, body_rx) = mpsc::channel(1);

        let mut meta = sample_request_meta();
        meta.url = format!("http://{host}:{}/start", addr.port());

        handle_stream(
            Arc::clone(&state),
            server_ctx,
            5,
            meta,
            body_rx,
            frame_tx.clone(),
        )
        .await;
        let result = collect_stream_result(frame_tx, sent, writer_handle).await;
        server.abort();

        assert!(
            result.error.is_none(),
            "unexpected stream error: {:?}",
            result.error
        );
        let response = result.response.expect("response metadata");
        assert_eq!(response.status, 302);
        assert_eq!(
            response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                .map(|(_, value)| value.as_str()),
            Some("/final")
        );
    }

    #[tokio::test]
    async fn relays_basic_get_request_successfully_through_tunnel() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new().route(
            "/ok",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/plain")
                    .body(Body::from("proxy-ok"))
                    .expect("ok response")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let host = "basic-relay.test";
        let state = sample_state_for_port(addr.port());
        cache_test_host(&state, host, addr).await;
        let server_ctx = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (_body_tx, body_rx) = mpsc::channel(1);

        let mut meta = sample_request_meta();
        meta.url = format!("http://{host}:{}/ok", addr.port());

        handle_stream(
            Arc::clone(&state),
            server_ctx,
            3,
            meta,
            body_rx,
            frame_tx.clone(),
        )
        .await;
        let result = collect_stream_result(frame_tx, sent, writer_handle).await;
        server.abort();

        assert!(
            result.error.is_none(),
            "unexpected stream error: {:?}",
            result.error
        );
        let response = result.response.expect("response metadata");
        assert_eq!(response.status, 200);
        assert_eq!(result.body, Bytes::from_static(b"proxy-ok"));
        assert!(response
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("content-type")
                && value.starts_with("text/plain")));
    }

    #[tokio::test]
    async fn follows_redirects_when_explicitly_enabled_for_replayable_post_requests() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new()
            .route(
                "/start",
                post(|headers: HeaderMap, body: Bytes| async move {
                    assert_eq!(
                        headers
                            .get(header::CONTENT_LENGTH)
                            .and_then(|value| value.to_str().ok()),
                        Some("5")
                    );
                    assert!(headers.get(header::TRANSFER_ENCODING).is_none());
                    assert_eq!(body, Bytes::from_static(b"hello"));
                    Response::builder()
                        .status(StatusCode::TEMPORARY_REDIRECT)
                        .header(header::LOCATION, "/final")
                        .body(Body::empty())
                        .expect("redirect response")
                }),
            )
            .route(
                "/final",
                post(|headers: HeaderMap, body: Bytes| async move {
                    assert_eq!(
                        headers
                            .get(header::CONTENT_LENGTH)
                            .and_then(|value| value.to_str().ok()),
                        Some("5")
                    );
                    assert!(headers.get(header::TRANSFER_ENCODING).is_none());
                    assert_eq!(body, Bytes::from_static(b"hello"));
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("redirected"))
                        .expect("final response")
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let host = "redirect-default.test";
        let state = sample_state_for_port(addr.port());
        cache_test_host(&state, host, addr).await;
        let server_ctx = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (body_tx, body_rx) = mpsc::channel(4);
        body_tx
            .send(TunnelFrame::new(
                1,
                MsgType::RequestBody,
                flags::END_STREAM,
                Bytes::from_static(b"hello"),
            ))
            .await
            .expect("send body");
        drop(body_tx);

        let mut meta = sample_request_meta();
        meta.method = "POST".to_string();
        meta.url = format!("http://{host}:{}/start", addr.port());
        meta.follow_redirects = Some(true);

        handle_stream(
            Arc::clone(&state),
            server_ctx,
            1,
            meta,
            body_rx,
            frame_tx.clone(),
        )
        .await;
        let result = collect_stream_result(frame_tx, sent, writer_handle).await;
        server.abort();

        assert!(
            result.error.is_none(),
            "unexpected stream error: {:?}",
            result.error
        );
        let response = result.response.expect("response metadata");
        assert_eq!(response.status, 200);
        assert_eq!(result.body, Bytes::from_static(b"redirected"));
        let timing_header = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-proxy-timing"))
            .map(|(_, value)| value.clone())
            .expect("timing header");
        let timing: serde_json::Value =
            serde_json::from_str(&timing_header).expect("timing header json");
        assert_eq!(timing["redirect_count"], serde_json::json!(1));
    }

    #[tokio::test]
    async fn preserves_redirect_response_when_follow_redirects_disabled() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new().route(
            "/start",
            get(|| async {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, "/final")
                    .body(Body::empty())
                    .expect("redirect response")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let host = "redirect-disabled.test";
        let state = sample_state_for_port(addr.port());
        cache_test_host(&state, host, addr).await;
        let server_ctx = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (_body_tx, body_rx) = mpsc::channel(1);

        let mut meta = sample_request_meta();
        meta.url = format!("http://{host}:{}/start", addr.port());
        meta.follow_redirects = Some(false);

        handle_stream(
            Arc::clone(&state),
            server_ctx,
            7,
            meta,
            body_rx,
            frame_tx.clone(),
        )
        .await;
        let result = collect_stream_result(frame_tx, sent, writer_handle).await;
        server.abort();

        assert!(
            result.error.is_none(),
            "unexpected stream error: {:?}",
            result.error
        );
        let response = result.response.expect("response metadata");
        assert_eq!(response.status, 302);
        assert_eq!(
            response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                .map(|(_, value)| value.as_str()),
            Some("/final")
        );
    }

    #[tokio::test]
    async fn preserves_redirect_response_when_replay_budget_is_zero() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new()
            .route(
                "/start",
                post(|body: Bytes| async move {
                    assert_eq!(body, Bytes::from_static(b"hello"));
                    Response::builder()
                        .status(StatusCode::TEMPORARY_REDIRECT)
                        .header(header::LOCATION, "/final")
                        .body(Body::empty())
                        .expect("redirect response")
                }),
            )
            .route(
                "/final",
                post(|| async {
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("unexpected"))
                        .expect("final response")
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let host = "redirect-budget-zero.test";
        let state = sample_state_for_budget(addr.port(), 0);
        cache_test_host(&state, host, addr).await;
        let server_ctx = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (body_tx, body_rx) = mpsc::channel(4);
        body_tx
            .send(TunnelFrame::new(
                1,
                MsgType::RequestBody,
                flags::END_STREAM,
                Bytes::from_static(b"hello"),
            ))
            .await
            .expect("send body");
        drop(body_tx);

        let mut meta = sample_request_meta();
        meta.method = "POST".to_string();
        meta.url = format!("http://{host}:{}/start", addr.port());
        meta.follow_redirects = Some(true);

        handle_stream(
            Arc::clone(&state),
            server_ctx,
            11,
            meta,
            body_rx,
            frame_tx.clone(),
        )
        .await;
        let result = collect_stream_result(frame_tx, sent, writer_handle).await;
        server.abort();

        assert!(
            result.error.is_none(),
            "unexpected stream error: {:?}",
            result.error
        );
        let response = result.response.expect("response metadata");
        assert_eq!(response.status, 307);
        assert_eq!(
            response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                .map(|(_, value)| value.as_str()),
            Some("/final")
        );
    }

    #[tokio::test]
    async fn rejects_stream_when_local_admission_gate_is_saturated() {
        let gate = Arc::new(ConcurrencyGate::new("proxy_streams", 1));
        let _permit = gate.try_acquire().expect("first permit");
        let state = sample_state(Some(gate), None);
        let server = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (_body_tx, body_rx) = mpsc::channel(1);

        handle_stream(
            Arc::clone(&state),
            server,
            7,
            sample_request_meta(),
            body_rx,
            frame_tx.clone(),
        )
        .await;

        let frame = collect_emitted_frames(frame_tx, sent, writer_handle)
            .await
            .into_iter()
            .find(|frame| frame.msg_type == MsgType::StreamError)
            .expect("overload frame");
        assert_eq!(frame.stream_id, 7);
        assert_eq!(frame.msg_type, MsgType::StreamError);
        assert_eq!(frame.payload, Bytes::from_static(b"proxy overloaded"));
        assert_eq!(
            state
                .stream_gate
                .as_ref()
                .expect("stream gate")
                .snapshot()
                .rejected,
            1
        );
    }

    #[tokio::test]
    async fn rejects_stream_when_distributed_admission_gate_is_saturated() {
        let gate = Arc::new(DistributedConcurrencyGate::new_in_memory(
            "proxy_streams_distributed",
            1,
        ));
        let _permit = gate.try_acquire().await.expect("first permit");
        let state = sample_state(None, Some(gate));
        let server = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (_body_tx, body_rx) = mpsc::channel(1);

        handle_stream(
            Arc::clone(&state),
            server,
            9,
            sample_request_meta(),
            body_rx,
            frame_tx.clone(),
        )
        .await;

        let frame = collect_emitted_frames(frame_tx, sent, writer_handle)
            .await
            .into_iter()
            .find(|frame| frame.msg_type == MsgType::StreamError)
            .expect("overload frame");
        assert_eq!(frame.stream_id, 9);
        assert_eq!(frame.msg_type, MsgType::StreamError);
        assert_eq!(frame.payload, Bytes::from_static(b"proxy overloaded"));
        assert_eq!(
            state
                .distributed_stream_gate
                .as_ref()
                .expect("distributed gate")
                .snapshot()
                .await
                .expect("distributed snapshot")
                .rejected,
            1
        );
    }

    fn sample_request_meta() -> RequestMeta {
        RequestMeta {
            method: "GET".to_string(),
            url: "https://example.com/ok".to_string(),
            headers: HashMap::new(),
            timeout: 30,
            follow_redirects: None,
            http1_only: false,
        }
    }

    fn sample_state(
        stream_gate: Option<Arc<ConcurrencyGate>>,
        distributed_stream_gate: Option<Arc<DistributedConcurrencyGate>>,
    ) -> Arc<AppState> {
        ensure_rustls_provider();
        let config = Arc::new(sample_config());
        let dns_cache = Arc::new(DnsCache::new(Duration::from_secs(60), 128));
        let upstream_client =
            upstream_client::build_upstream_client(&config, Arc::clone(&dns_cache));
        let upstream_http1_client =
            upstream_client::build_http1_only_upstream_client(&config, Arc::clone(&dns_cache));
        Arc::new(AppState {
            config,
            dns_cache,
            upstream_client,
            upstream_http1_client,
            tunnel_tls_config: Arc::new(build_tls_config()),
            stream_gate,
            distributed_stream_gate,
        })
    }

    fn sample_state_for_port(port: u16) -> Arc<AppState> {
        ensure_rustls_provider();
        let mut config = sample_config();
        config.allowed_ports.push(port);
        sample_state_with_config(config)
    }

    fn sample_state_for_budget(port: u16, redirect_replay_budget_bytes: usize) -> Arc<AppState> {
        ensure_rustls_provider();
        let mut config = sample_config();
        config.allowed_ports.push(port);
        config.redirect_replay_budget_bytes = redirect_replay_budget_bytes;
        sample_state_with_config(config)
    }

    fn sample_state_with_config(config: Config) -> Arc<AppState> {
        let config = Arc::new(config);
        let dns_cache = Arc::new(DnsCache::new(Duration::from_secs(60), 128));
        let upstream_client =
            upstream_client::build_upstream_client(&config, Arc::clone(&dns_cache));
        let upstream_http1_client =
            upstream_client::build_http1_only_upstream_client(&config, Arc::clone(&dns_cache));
        Arc::new(AppState {
            config,
            dns_cache,
            upstream_client,
            upstream_http1_client,
            tunnel_tls_config: Arc::new(build_tls_config()),
            stream_gate: None,
            distributed_stream_gate: None,
        })
    }

    fn sample_server(state: &Arc<AppState>) -> Arc<ServerContext> {
        let config = Arc::clone(&state.config);
        Arc::new(ServerContext {
            server_label: "server".to_string(),
            aether_url: config.aether_url.clone(),
            management_token: config.management_token.clone(),
            node_name: config.node_name.clone(),
            node_id: Arc::new(std::sync::RwLock::new("node-1".to_string())),
            aether_client: Arc::new(AetherClient::new(
                &config,
                &config.aether_url,
                &config.management_token,
            )),
            dynamic: Arc::new(ArcSwap::from_pointee(DynamicConfig::from_config(&config))),
            active_connections: Arc::new(AtomicU64::new(0)),
            metrics: Arc::new(ProxyMetrics::new()),
        })
    }

    fn sample_config() -> Config {
        Config {
            aether_url: "https://aether.example.com".to_string(),
            management_token: "token".to_string(),
            public_ip: None,
            node_name: "proxy-test".to_string(),
            node_region: None,
            heartbeat_interval: 30,
            allowed_ports: vec![80, 443],
            allow_private_targets: false,
            aether_request_timeout_secs: 10,
            aether_connect_timeout_secs: 10,
            aether_pool_max_idle_per_host: 8,
            aether_pool_idle_timeout_secs: 90,
            aether_tcp_keepalive_secs: 60,
            aether_tcp_nodelay: true,
            aether_http2: true,
            aether_retry_max_attempts: 3,
            aether_retry_base_delay_ms: 200,
            aether_retry_max_delay_ms: 2_000,
            max_concurrent_connections: None,
            max_in_flight_streams: None,
            distributed_stream_limit: None,
            distributed_stream_redis_url: None,
            distributed_stream_redis_key_prefix: None,
            distributed_stream_lease_ttl_ms: 30_000,
            distributed_stream_renew_interval_ms: 10_000,
            distributed_stream_command_timeout_ms: 1_000,
            dns_cache_ttl_secs: 60,
            dns_cache_capacity: 128,
            upstream_connect_timeout_secs: 30,
            upstream_pool_max_idle_per_host: 4,
            upstream_pool_idle_timeout_secs: 60,
            upstream_tcp_keepalive_secs: 60,
            upstream_tcp_nodelay: true,
            redirect_replay_budget_bytes: crate::config::DEFAULT_REDIRECT_REPLAY_BUDGET_BYTES,
            log_level: "info".to_string(),
            log_destination: crate::config::ProxyLogDestinationArg::Stdout,
            log_dir: None,
            log_rotation: crate::config::ProxyLogRotationArg::Daily,
            log_retention_days: 7,
            log_max_files: 30,
            tunnel_reconnect_base_ms: 500,
            tunnel_reconnect_max_ms: 30_000,
            tunnel_ping_interval_ms: 15_000,
            tunnel_max_streams: Some(8),
            tunnel_connect_timeout_ms: 15_000,
            tunnel_tcp_keepalive_secs: 30,
            tunnel_tcp_nodelay: true,
            tunnel_stale_timeout_ms: 45_000,
            tunnel_connections: Some(1),
            tunnel_connections_max: Some(1),
            tunnel_scale_check_interval_ms: 1_000,
            tunnel_scale_up_threshold_percent: 70,
            tunnel_scale_down_threshold_percent: 35,
            tunnel_scale_down_grace_secs: 15,
        }
    }

    async fn cache_test_host(state: &Arc<AppState>, host: &str, addr: SocketAddr) {
        state
            .dns_cache
            .insert(host, addr.port(), Arc::new(vec![addr]))
            .await;
    }

    #[derive(Clone, Default)]
    struct VecSink {
        sent: Arc<Mutex<Vec<Message>>>,
    }

    impl Sink<Message> for VecSink {
        type Error = WebSocketError;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.sent.lock().expect("sink lock").push(item);
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    fn spawn_test_writer() -> (FrameSender, Arc<Mutex<Vec<Message>>>, JoinHandle<()>) {
        let sink = VecSink::default();
        let sent = Arc::clone(&sink.sent);
        let (frame_tx, handle) = crate::tunnel::writer::spawn_writer(sink, Duration::from_secs(60));
        (frame_tx, sent, handle)
    }

    struct StreamResult {
        response: Option<ResponseMeta>,
        body: Bytes,
        error: Option<String>,
    }

    async fn collect_emitted_frames(
        frame_tx: FrameSender,
        sent: Arc<Mutex<Vec<Message>>>,
        writer_handle: JoinHandle<()>,
    ) -> Vec<TunnelFrame> {
        drop(frame_tx);
        writer_handle.await.expect("writer should exit cleanly");

        sent.lock()
            .expect("sink lock")
            .iter()
            .filter_map(|message| match message {
                Message::Binary(data) => {
                    Some(TunnelFrame::decode(data.clone().into()).expect("frame should decode"))
                }
                Message::Ping(_) | Message::Pong(_) | Message::Close(_) => None,
                other => panic!("unexpected writer message: {other:?}"),
            })
            .collect()
    }

    async fn collect_stream_result(
        frame_tx: FrameSender,
        sent: Arc<Mutex<Vec<Message>>>,
        writer_handle: JoinHandle<()>,
    ) -> StreamResult {
        let mut response = None;
        let mut body = BytesMut::new();
        let mut error = None;

        for frame in collect_emitted_frames(frame_tx, sent, writer_handle).await {
            match frame.msg_type {
                MsgType::ResponseHeaders => {
                    let payload = decompress_if_gzip(&frame).expect("headers payload");
                    response = Some(
                        serde_json::from_slice(&payload).expect("response metadata should decode"),
                    );
                }
                MsgType::ResponseBody => {
                    let payload = decompress_if_gzip(&frame).expect("body payload");
                    body.extend_from_slice(&payload);
                }
                MsgType::StreamError => {
                    error = Some(
                        String::from_utf8(frame.payload.to_vec())
                            .unwrap_or_else(|_| "stream error".to_string()),
                    );
                    break;
                }
                MsgType::StreamEnd => break,
                _ => continue,
            }
        }

        StreamResult {
            response,
            body: body.freeze(),
            error,
        }
    }

    fn ensure_rustls_provider() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }
}
