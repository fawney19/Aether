//! Per-stream request handler.
//!
//! Receives request frames, executes the upstream HTTP request,
//! and sends response frames back through the writer channel.

use std::io;
use std::pin::Pin;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use aether_runtime::{AdmissionPermit, QueueSendError};
use bytes::Bytes;
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
    compress_payload, decompress_if_gzip_with_limit, flags, raw_payload, Frame as TunnelFrame,
    MsgType, RequestMeta, ResetStreamPayload, ResponseMeta,
};
use super::writer::FrameSender;

/// Maximum response body chunk size per frame (32 KB).
const MAX_CHUNK_SIZE: usize = 32 * 1024;

/// Timeout for sending a single frame to the writer channel.
/// Control frames are allowed a short wait; body frames fail fast.
const CONTROL_FRAME_SEND_TIMEOUT: Duration = Duration::from_millis(250);
const FLOW_CONTROL_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const SLOW_STREAM_LOG_THRESHOLD: Duration = Duration::from_secs(2);
const SUCCESS_LOG_SAMPLE_MODULO: u32 = 256;
const REQUEST_BODY_SPOOL_QUEUE_CAPACITY: usize = 64;
/// Request bytes retained only to support same-origin 307/308 replay. This does
/// not limit or buffer the first upstream request, which remains streaming.
const REDIRECT_REPLAY_PER_REQUEST_BUDGET_BYTES: usize = 5 * 1024 * 1024;
const REDIRECT_REPLAY_MAX_CHUNKS: usize = 1024;
/// Bound replay retention across all active streams without reducing stream
/// admission or rejecting the original request when the cache is exhausted.
const REDIRECT_REPLAY_GLOBAL_BUDGET_BYTES: usize = 256 * 1024 * 1024;
static REDIRECT_REPLAY_BUFFERED_BYTES: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
pub(crate) struct StreamSendWindow {
    initial_window_bytes: u32,
    available: Mutex<u64>,
    notify: Notify,
}

impl StreamSendWindow {
    pub(crate) fn new(initial_window_bytes: u32) -> Self {
        Self {
            initial_window_bytes: initial_window_bytes.max(1),
            available: Mutex::new(u64::from(initial_window_bytes.max(1))),
            notify: Notify::new(),
        }
    }

    pub(crate) fn add_credit(&self, delta_bytes: u32) {
        if delta_bytes == 0 {
            return;
        }
        let mut available = self.available.lock().expect("stream window lock poisoned");
        *available = available.saturating_add(u64::from(delta_bytes));
        drop(available);
        self.notify.notify_waiters();
    }

    async fn acquire(&self, bytes: usize, timeout: Duration) -> Result<Duration, ()> {
        if bytes == 0 {
            return Ok(Duration::ZERO);
        }

        let requested = bytes as u64;
        let started_at = Instant::now();
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut available = self.available.lock().expect("stream window lock poisoned");
                if *available >= requested {
                    *available -= requested;
                    return Ok(started_at.elapsed());
                }
            }

            let Some(remaining) = timeout.checked_sub(started_at.elapsed()) else {
                return Err(());
            };
            if tokio::time::timeout(remaining, notified).await.is_err() {
                return Err(());
            }
        }
    }
}

fn stream_reset_message(frame: &TunnelFrame) -> String {
    // Reset/error payloads are supplied by the peer and can contain arbitrary
    // user data (or a provider error copied by the gateway).  They are used as
    // an `io::Error` below and may otherwise be echoed back in a later
    // StreamError frame, so keep only a stable protocol-level category.
    match frame.msg_type {
        MsgType::ResetStream => "request reset by peer".to_string(),
        MsgType::StreamError => "client cancelled request body".to_string(),
        _ => "request body terminated".to_string(),
    }
}

/// Project an internal stream failure to a bounded, protocol-safe message.
///
/// Hyper, URL, DNS, TLS, and proxy errors can include complete request URLs,
/// query credentials, private addresses, or implementation details.  Tunnel
/// errors cross the authenticated tunnel and are eventually exposed by the
/// gateway, so never put those error strings on the wire (or in logs).
fn safe_stream_error_message(message: &str) -> &'static str {
    let lower = message.trim().to_ascii_lowercase();
    if lower == "tunnel overloaded" {
        return "tunnel overloaded";
    }
    if lower == "tunnel admission unavailable" {
        return "tunnel admission unavailable";
    }
    if lower.contains("client cancelled") {
        return "client cancelled request body";
    }
    if lower.contains("response body timeout") {
        return "upstream response body timeout";
    }
    if lower.contains("flow_control_timeout") {
        return "response flow-control timeout";
    }
    if lower == "upstream timeout" || lower.contains("timed out") {
        return "upstream timeout";
    }
    if lower.contains("invalid") && lower.contains("url") {
        return "invalid upstream URL";
    }
    if lower.contains("unsupported") && lower.contains("scheme") {
        return "unsupported upstream URL scheme";
    }
    if lower.contains("target blocked")
        || lower.contains("private/reserved")
        || lower.contains("port not allowed")
        || lower.contains("dns resolution")
        || lower.contains("no public")
    {
        return "upstream target blocked";
    }
    if lower.contains("gzip") || lower.contains("decompress") || lower.contains("request body") {
        return "invalid request body";
    }
    if lower.contains("redirect") {
        return "upstream redirect failed";
    }
    if lower.contains("connect") || lower.contains("tls") || lower.contains("proxy") {
        return "upstream connect failed";
    }
    if lower.contains("body") && (lower.contains("read") || lower.contains("response")) {
        return "upstream response body failed";
    }
    if lower.contains("request") {
        return "upstream request failed";
    }
    "upstream request failed"
}

async fn send_window_update(frame_tx: &FrameSender, stream_id: u32, bytes: usize) -> bool {
    if bytes == 0 {
        return true;
    }
    let delta = bytes.min(u32::MAX as usize) as u32;
    if matches!(
        tokio::time::timeout(
            FLOW_CONTROL_WAIT_TIMEOUT,
            frame_tx.send(TunnelFrame::new(
                stream_id,
                MsgType::WindowUpdate,
                0,
                Bytes::from(
                    serde_json::to_vec(&aether_contracts::tunnel::WindowUpdatePayload {
                        delta_bytes: delta,
                    })
                    .expect("window update payload should serialize"),
                ),
            ))
        )
        .await,
        Ok(Ok(()))
    ) {
        return true;
    }
    frame_tx.close();
    false
}

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
#[derive(Debug, Clone)]
enum ReplayableRequestBody {
    None,
    Pending(Arc<RequestBodyReplayState>),
    NonReplayable,
}

struct PreparedRequestBody {
    first_request_body: Option<upstream_client::UpstreamRequestBody>,
    replay_body: ReplayableRequestBody,
    spool_task: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for PreparedRequestBody {
    fn drop(&mut self) {
        if let Some(task) = self.spool_task.take() {
            task.abort();
        }
    }
}

struct ActiveStreamGuard(Arc<ServerContext>);

impl Drop for ActiveStreamGuard {
    fn drop(&mut self) {
        self.0.active_connections.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy)]
struct RequestTimeouts {
    first_byte_timeout: Duration,
    response_body_timeout: Option<Duration>,
}

#[derive(Debug)]
struct RequestBodyReplayState {
    budget_bytes: usize,
    reserved_bytes: AtomicUsize,
    state: Mutex<RequestBodyReplayStatus>,
    ready: Notify,
}

#[derive(Debug)]
enum RequestBodyReplayStatus {
    Collecting {
        chunks: Vec<Bytes>,
        buffered_len: usize,
    },
    Ready {
        chunks: Vec<Bytes>,
        buffered_len: usize,
    },
    Empty,
    NonReplayable,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplayBodyResolution {
    Empty,
    Replayable {
        chunks: Vec<Bytes>,
        buffered_len: usize,
    },
    NonReplayable,
}

struct ReplayRequestBody {
    chunks: std::vec::IntoIter<Bytes>,
    remaining: u64,
}

struct DecodedRequestBodyPayload {
    decoded: Bytes,
    _compressed_and_budget: Bytes,
}

impl AsRef<[u8]> for DecodedRequestBodyPayload {
    fn as_ref(&self) -> &[u8] {
        self.decoded.as_ref()
    }
}

impl hyper::body::Body for ReplayRequestBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<BodyFrame<Self::Data>, Self::Error>>> {
        let body = self.get_mut();
        loop {
            let Some(chunk) = body.chunks.next() else {
                return Poll::Ready(None);
            };
            if chunk.is_empty() {
                continue;
            }
            body.remaining = body.remaining.saturating_sub(chunk.len() as u64);
            return Poll::Ready(Some(Ok(BodyFrame::data(chunk))));
        }
    }

    fn is_end_stream(&self) -> bool {
        self.remaining == 0
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        hyper::body::SizeHint::with_exact(self.remaining)
    }
}

#[derive(Debug)]
enum SpoolBodyEvent {
    Data {
        payload: Bytes,
        credit_returned: bool,
    },
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
    let slow = duration >= SLOW_STREAM_LOG_THRESHOLD;
    if slow {
        ctx.server.metrics.record_slow_request();
    }
    let sampled = slow
        || ctx.redirect_count > 0
        || ctx.request_body_size >= 1_048_576
        || ctx.stream_id.is_multiple_of(SUCCESS_LOG_SAMPLE_MODULO);
    if sampled {
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
            slow,
            sampled,
            "tunnel request completed"
        );
    } else {
        debug!(
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
            slow,
            sampled,
            "tunnel request completed"
        );
    }
}

fn log_stream_failure(ctx: StreamLogContext<'_>, error: &str, duration: Duration) {
    let error = safe_stream_error_message(error);
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
                "tunnel request failed"
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
                "tunnel request failed"
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

    /// Prefer the bounded replay snapshot for the first request when redirect
    /// replay is enabled. A replay body advertises an exact size to Hyper, so
    /// HTTP/1 requests get a correct `Content-Length` instead of implicit
    /// chunked framing. Requests that exceed the replay budget remain streamed.
    async fn resolve_initial_replay_body(&mut self, deadline: Instant) -> Result<(), String> {
        let ReplayableRequestBody::Pending(state) = &self.replay_body else {
            return Ok(());
        };
        match state.wait_for_resolution(deadline).await? {
            ReplayBodyResolution::Empty => {
                self.first_request_body = Some(empty_request_body());
            }
            ReplayBodyResolution::Replayable {
                chunks,
                buffered_len,
            } => {
                self.first_request_body = Some(replay_request_body(chunks, buffered_len));
            }
            ReplayBodyResolution::NonReplayable => {}
        }
        Ok(())
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
                    ReplayBodyResolution::Replayable {
                        chunks,
                        buffered_len,
                    } => Ok(Some(replay_request_body(chunks, buffered_len))),
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
            reserved_bytes: AtomicUsize::new(0),
            state: Mutex::new(RequestBodyReplayStatus::Collecting {
                chunks: Vec::new(),
                buffered_len: 0,
            }),
            ready: Notify::new(),
        }
    }

    fn push_chunk(&self, payload: Bytes) -> bool {
        let mut disable_replay = false;
        let mut retained = false;
        let mut state = self.state.lock().expect("request body replay state lock");
        if let RequestBodyReplayStatus::Collecting {
            chunks,
            buffered_len,
        } = &mut *state
        {
            let Some(next_len) = buffered_len.checked_add(payload.len()) else {
                chunks.clear();
                *state = RequestBodyReplayStatus::NonReplayable;
                drop(state);
                self.release_reserved_bytes();
                self.ready.notify_waiters();
                return false;
            };
            let accounted_bytes = payload.len().checked_add(std::mem::size_of::<Bytes>());
            if next_len > self.budget_bytes
                || chunks.len() >= REDIRECT_REPLAY_MAX_CHUNKS
                || accounted_bytes.is_none_or(|bytes| !self.try_reserve_bytes(bytes))
            {
                disable_replay = true;
                chunks.clear();
                *state = RequestBodyReplayStatus::NonReplayable;
            } else {
                *buffered_len = next_len;
                chunks.push(payload);
                retained = true;
            }
        }
        drop(state);
        if disable_replay {
            self.release_reserved_bytes();
            self.ready.notify_waiters();
        }
        retained
    }

    fn try_reserve_bytes(&self, bytes: usize) -> bool {
        if bytes == 0 {
            return true;
        }
        let mut current = REDIRECT_REPLAY_BUFFERED_BYTES.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(bytes) else {
                return false;
            };
            if next > REDIRECT_REPLAY_GLOBAL_BUDGET_BYTES {
                return false;
            }
            match REDIRECT_REPLAY_BUFFERED_BYTES.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.reserved_bytes.fetch_add(bytes, Ordering::Release);
                    return true;
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn release_reserved_bytes(&self) {
        let reserved = self.reserved_bytes.swap(0, Ordering::AcqRel);
        if reserved > 0 {
            REDIRECT_REPLAY_BUFFERED_BYTES.fetch_sub(reserved, Ordering::AcqRel);
        }
    }

    fn discard(&self) {
        {
            let mut state = self.state.lock().expect("request body replay state lock");
            match &*state {
                RequestBodyReplayStatus::Collecting { .. }
                | RequestBodyReplayStatus::Ready { .. }
                | RequestBodyReplayStatus::Empty => {
                    *state = RequestBodyReplayStatus::NonReplayable;
                }
                RequestBodyReplayStatus::NonReplayable | RequestBodyReplayStatus::Error(_) => {
                    return;
                }
            }
        }
        self.release_reserved_bytes();
        self.ready.notify_waiters();
    }

    /// Disable replay while retaining the queued body for the first streaming
    /// request. This prevents the preflight waiter from deadlocking when the
    /// bounded spool queue fills before the request starts.
    fn disable_replay(&self) {
        let changed = {
            let mut state = self.state.lock().expect("request body replay state lock");
            match &*state {
                RequestBodyReplayStatus::Collecting { .. }
                | RequestBodyReplayStatus::Ready { .. } => {
                    *state = RequestBodyReplayStatus::NonReplayable;
                    true
                }
                RequestBodyReplayStatus::Empty
                | RequestBodyReplayStatus::NonReplayable
                | RequestBodyReplayStatus::Error(_) => false,
            }
        };
        if changed {
            self.release_reserved_bytes();
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
                        RequestBodyReplayStatus::Ready {
                            chunks,
                            buffered_len,
                        }
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
        self.release_reserved_bytes();
        self.ready.notify_waiters();
    }

    async fn wait_for_resolution(&self, deadline: Instant) -> Result<ReplayBodyResolution, String> {
        loop {
            let notified = self.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let resolution = {
                let state = self.state.lock().expect("request body replay state lock");
                match &*state {
                    RequestBodyReplayStatus::Collecting { .. } => None,
                    RequestBodyReplayStatus::Ready {
                        chunks,
                        buffered_len,
                    } => Some(Ok(ReplayBodyResolution::Replayable {
                        chunks: chunks.clone(),
                        buffered_len: *buffered_len,
                    })),
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
            tokio::time::timeout(remaining, &mut notified)
                .await
                .map_err(|_| "upstream timeout".to_string())?;
        }
    }
}

impl Drop for RequestBodyReplayState {
    fn drop(&mut self) {
        self.release_reserved_bytes();
    }
}

fn follow_redirects_enabled(meta: &RequestMeta) -> bool {
    meta.follow_redirects == Some(true)
}

/// Validate URL syntax at the tunnel trust boundary before any request body
/// is consumed or a connection is attempted.
///
/// The gateway performs the same checks when it builds `RequestMeta`, but the
/// tunnel must not rely on a remote peer having constructed metadata through a
/// particular code path.  In particular, URL userinfo and fragments are not
/// valid upstream request components: userinfo can alter authority parsing and
/// fragments must never cross an HTTP request boundary.
fn validate_tunnel_upstream_url(
    url: &url::Url,
    allow_private_targets: bool,
) -> Result<(), &'static str> {
    if url.host_str().is_none() {
        return Err("invalid upstream URL");
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err("unsupported upstream URL scheme");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("invalid upstream URL");
    }
    if url.fragment().is_some() {
        return Err("invalid upstream URL");
    }

    // Domain names are checked again against the resolved DNS answers by
    // `target_filter::validate_target`.  Reject literal private/reserved
    // addresses here as well when the tunnel's private-target policy is
    // disabled, so a metadata URL cannot bypass that policy through a
    // different parser or connector. Explicitly enabled private-target
    // deployments keep their existing behavior (including loopback HTTP
    // endpoints).
    if !allow_private_targets {
        let literal_ip = match url.host() {
            Some(url::Host::Ipv4(address)) => Some(std::net::IpAddr::V4(address)),
            Some(url::Host::Ipv6(address)) => Some(std::net::IpAddr::V6(address)),
            _ => None,
        };
        if literal_ip.is_some_and(aether_http::is_private_or_reserved_ip) {
            return Err("upstream target blocked");
        }
    }

    Ok(())
}

fn validate_tunnel_redirect_url(url: &url::Url) -> Result<(), &'static str> {
    if url.host_str().is_none() {
        return Err("invalid upstream URL");
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err("unsupported upstream URL scheme");
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("invalid upstream URL");
    }
    Ok(())
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
    let connection_declared = aether_http::connection_declared_header_names(
        headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(hyper::header::CONNECTION.as_str()))
            .map(|(_, value)| value.as_str()),
    );
    headers
        .iter()
        .filter_map(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            if BLOCKED_HEADERS.contains(&normalized.as_str())
                || connection_declared.contains(&normalized)
            {
                None
            } else {
                Some((key.clone(), value.clone()))
            }
        })
        .collect()
}

/// Return a single, syntactically valid request length that can safely be
/// applied to the streamed body.  The original header is otherwise removed
/// because tunnel compression/decoding may change the bytes seen upstream.
/// Conflicting case variants and `Transfer-Encoding` are deliberately treated
/// as unknown framing rather than forwarded as an ambiguous pair.
fn validated_request_content_length(
    headers: &std::collections::HashMap<String, String>,
) -> Option<u64> {
    if headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("transfer-encoding"))
    {
        return None;
    }

    let values = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.as_str())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }

    let parsed = values
        .iter()
        .map(|value| {
            let value = value.trim_matches(|character| matches!(character, ' ' | '\t'));
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            value.parse::<u64>().ok()
        })
        .collect::<Option<Vec<_>>>()?;
    let first = *parsed.first()?;
    parsed.iter().all(|value| *value == first).then_some(first)
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

fn replay_request_body(
    chunks: Vec<Bytes>,
    buffered_len: usize,
) -> upstream_client::UpstreamRequestBody {
    ReplayRequestBody {
        chunks: chunks.into_iter(),
        remaining: buffered_len as u64,
    }
    .boxed_unsync()
}

pub(super) fn decode_request_body_frame(frame: TunnelFrame) -> Result<Bytes, std::io::Error> {
    if frame.is_gzip() {
        let decoded = decompress_if_gzip_with_limit(
            &frame,
            aether_contracts::tunnel::MAX_TUNNEL_DECOMPRESSED_PAYLOAD_BYTES,
        )?;
        return Ok(Bytes::from_owner(DecodedRequestBodyPayload {
            decoded,
            _compressed_and_budget: frame.payload,
        }));
    }
    if frame.payload.len() > aether_contracts::tunnel::MAX_TUNNEL_DECOMPRESSED_PAYLOAD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "decoded tunnel payload exceeds {} bytes",
                aether_contracts::tunnel::MAX_TUNNEL_DECOMPRESSED_PAYLOAD_BYTES
            ),
        ));
    }
    Ok(frame.payload)
}

fn prepare_request_body(
    stream_id: u32,
    body_rx: mpsc::Receiver<TunnelFrame>,
    body_size: Arc<AtomicUsize>,
    deadline: Instant,
    capture_for_redirects: bool,
    frame_tx: FrameSender,
) -> PreparedRequestBody {
    let (spool_tx, spool_rx) = mpsc::channel(REQUEST_BODY_SPOOL_QUEUE_CAPACITY);
    let replay_state = capture_for_redirects.then(|| {
        Arc::new(RequestBodyReplayState::new(
            REDIRECT_REPLAY_PER_REQUEST_BUDGET_BYTES,
        ))
    });
    let replay_body = match replay_state.as_ref() {
        Some(state) => ReplayableRequestBody::Pending(Arc::clone(state)),
        None => ReplayableRequestBody::NonReplayable,
    };

    let spool_task = tokio::spawn(spool_request_body(
        stream_id,
        body_rx,
        spool_tx,
        replay_state,
        body_size,
        deadline,
        frame_tx.clone(),
    ));

    PreparedRequestBody {
        first_request_body: Some(build_spooled_request_body(spool_rx, stream_id, frame_tx)),
        replay_body,
        spool_task: Some(spool_task),
    }
}

fn prepare_bodyless_request_body(
    body_rx: mpsc::Receiver<TunnelFrame>,
    follow_redirects: bool,
) -> PreparedRequestBody {
    drop(body_rx);
    PreparedRequestBody {
        first_request_body: Some(empty_request_body()),
        replay_body: if follow_redirects {
            ReplayableRequestBody::None
        } else {
            ReplayableRequestBody::NonReplayable
        },
        spool_task: None,
    }
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

fn resolve_request_timeouts(meta: &RequestMeta) -> RequestTimeouts {
    let resolved = aether_contracts::tunnel::resolve_tunnel_request_timeouts(meta);

    RequestTimeouts {
        first_byte_timeout: Duration::from_millis(resolved.first_byte_ms),
        response_body_timeout: resolved.response_body_ms.map(Duration::from_millis),
    }
}

async fn spool_request_body(
    stream_id: u32,
    mut body_rx: mpsc::Receiver<TunnelFrame>,
    mut spool_tx: mpsc::Sender<SpoolBodyEvent>,
    replay_state: Option<Arc<RequestBodyReplayState>>,
    body_size: Arc<AtomicUsize>,
    deadline: Instant,
    frame_tx: FrameSender,
) {
    loop {
        let frame = match recv_body_frame_with_deadline(&mut body_rx, deadline).await {
            Ok(frame) => frame,
            Err(message) => {
                if let Some(state) = &replay_state {
                    state.fail(message.clone());
                }
                let _ = send_spool_event(
                    &mut spool_tx,
                    SpoolBodyEvent::Error(message),
                    replay_state.as_ref(),
                )
                .await;
                return;
            }
        };

        let Some(frame) = frame else {
            let message = "tunnel request body closed before stream end".to_string();
            if let Some(state) = &replay_state {
                state.fail(message.clone());
            }
            let _ = send_spool_event(
                &mut spool_tx,
                SpoolBodyEvent::Error(message),
                replay_state.as_ref(),
            )
            .await;
            return;
        };

        match frame.msg_type {
            MsgType::RequestBody => {
                let end_stream = frame.is_end_stream();
                let payload = match decode_request_body_frame(frame) {
                    Ok(payload) => payload,
                    Err(error) => {
                        let message = format!("gzip decompress failed: {error}");
                        if let Some(state) = &replay_state {
                            state.fail(message.clone());
                        }
                        let _ = send_spool_event(
                            &mut spool_tx,
                            SpoolBodyEvent::Error(message),
                            replay_state.as_ref(),
                        )
                        .await;
                        return;
                    }
                };

                if !payload.is_empty() {
                    body_size.fetch_add(payload.len(), Ordering::Relaxed);
                    let credit_returned = replay_state
                        .as_ref()
                        .is_some_and(|state| state.push_chunk(payload.clone()));
                    if credit_returned
                        && !send_window_update(&frame_tx, stream_id, payload.len()).await
                    {
                        if let Some(state) = &replay_state {
                            state.fail("tunnel flow-control update failed".to_string());
                        }
                        return;
                    }
                    if send_spool_event(
                        &mut spool_tx,
                        SpoolBodyEvent::Data {
                            payload,
                            credit_returned,
                        },
                        replay_state.as_ref(),
                    )
                    .await
                    .is_err()
                    {
                        if let Some(state) = &replay_state {
                            state.fail("request body replay channel closed".to_string());
                        }
                        return;
                    }
                }

                if end_stream {
                    if let Some(state) = &replay_state {
                        state.finish();
                    }
                    let _ =
                        send_spool_event(&mut spool_tx, SpoolBodyEvent::End, replay_state.as_ref())
                            .await;
                    return;
                }
            }
            MsgType::StreamError | MsgType::ResetStream => {
                let message = stream_reset_message(&frame);
                if let Some(state) = &replay_state {
                    state.fail(message.clone());
                }
                let _ = send_spool_event(
                    &mut spool_tx,
                    SpoolBodyEvent::Error(message),
                    replay_state.as_ref(),
                )
                .await;
                return;
            }
            MsgType::StreamEnd => {
                if let Some(state) = &replay_state {
                    state.finish();
                }
                let _ = send_spool_event(&mut spool_tx, SpoolBodyEvent::End, replay_state.as_ref())
                    .await;
                return;
            }
            _ => continue,
        }
    }
}

async fn send_spool_event(
    spool_tx: &mut mpsc::Sender<SpoolBodyEvent>,
    event: SpoolBodyEvent,
    replay_state: Option<&Arc<RequestBodyReplayState>>,
) -> Result<(), ()> {
    match spool_tx.try_send(event) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Closed(_)) => Err(()),
        Err(mpsc::error::TrySendError::Full(event)) => {
            if let Some(state) = replay_state {
                state.disable_replay();
            }
            spool_tx.send(event).await.map_err(|_| ())
        }
    }
}

fn remove_headers_case_insensitive(headers: &mut Vec<(String, String)>, blocked: &[&str]) {
    headers.retain(|(name, _)| {
        let normalized = name.to_ascii_lowercase();
        !blocked.contains(&normalized.as_str())
    });
}

fn redirect_urls_have_same_origin(left: &url::Url, right: &url::Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && left
            .host_str()
            .zip(right.host_str())
            .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
        && left.port_or_known_default() == right.port_or_known_default()
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
    // A same-origin redirect can still smuggle credentials or a fragment into
    // the next request. Validate the resolved URL before considering it for
    // replay; the target filter performs the address policy check when it is
    // actually connected.
    if validate_tunnel_redirect_url(&next_url).is_err() {
        return RedirectDecision::Stop;
    }

    if redirects_followed >= MAX_REDIRECTS {
        return RedirectDecision::Error("too many redirects");
    }
    if !redirect_urls_have_same_origin(current_url, &next_url) {
        return RedirectDecision::Stop;
    }

    RedirectDecision::Follow {
        method: next_method,
        url: next_url,
        headers: next_headers,
        body_mode,
    }
}

async fn resolve_upstream_target(
    current_url: &url::Url,
    allowed_ports: &std::collections::HashSet<u16>,
    allow_private_targets: bool,
    proxy_remote_dns: bool,
    dns_cache: &target_filter::DnsCache,
) -> Result<upstream_client::ValidatedUpstreamTarget, String> {
    validate_tunnel_upstream_url(current_url, allow_private_targets).map_err(str::to_string)?;
    let host = current_url
        .host_str()
        .ok_or_else(|| "missing host in URL".to_string())?;
    let port = current_url
        .port_or_known_default()
        .ok_or_else(|| "missing port in URL".to_string())?;
    let addresses = if proxy_remote_dns {
        match target_filter::validate_target_literal(
            host,
            port,
            allowed_ports,
            allow_private_targets,
        )
        .map_err(|_| "upstream target blocked".to_string())?
        {
            Some(address) => vec![address],
            None => return upstream_client::ValidatedUpstreamTarget::proxy_resolved(current_url),
        }
    } else {
        target_filter::validate_target(host, port, allowed_ports, allow_private_targets, dns_cache)
            .await
            .map_err(|_| "upstream target blocked".to_string())?
    };
    upstream_client::ValidatedUpstreamTarget::new(current_url, addresses)
}

#[allow(clippy::too_many_arguments)]
async fn execute_upstream_request(
    state: &AppState,
    server: &ServerContext,
    meta: &RequestMeta,
    current_url: &url::Url,
    method: hyper::Method,
    headers: &[(String, String)],
    request_body: upstream_client::UpstreamRequestBody,
    timeout: Duration,
    http1_only: bool,
) -> Result<UpstreamResponseContext, String> {
    let dns_start = Instant::now();
    let validated_target = {
        let allowed_ports = Arc::clone(&server.dynamic.load().allowed_ports);
        match resolve_upstream_target(
            current_url,
            &allowed_ports,
            state.config.allow_private_targets,
            state.config.upstream_proxy_remote_dns,
            &state.dns_cache,
        )
        .await
        {
            Ok(target) => target,
            Err(_error) => {
                server.metrics.dns_failures.fetch_add(1, Ordering::Release);
                // Keep the detailed filter error out of the tunnel response;
                // the request URL origin is already present in the structured
                // failure log context.
                return Err("upstream target blocked".to_string());
            }
        }
    };
    let dns_ms = dns_start.elapsed().as_millis() as u64;

    let client_key = upstream_client::upstream_client_pool_key(
        meta.provider_id.as_deref(),
        meta.endpoint_id.as_deref(),
        meta.key_id.as_deref(),
        meta.transport_profile.as_ref(),
        http1_only,
        validated_target,
    );
    let client = state.upstream_client_pool.get_or_build(client_key)?;

    let mut request = hyper::Request::builder()
        .method(method)
        .uri(current_url.as_str())
        .body(request_body)
        .map_err(|_| "invalid upstream request".to_string())?;
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
                "upstream connect failed".to_string()
            } else {
                "upstream request failed".to_string()
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

async fn acquire_response_credit(
    response_window: &StreamSendWindow,
    frame_tx: &FrameSender,
    stream_id: u32,
    bytes: usize,
) -> bool {
    match response_window
        .acquire(bytes, FLOW_CONTROL_WAIT_TIMEOUT)
        .await
    {
        Ok(waited) => {
            if waited > Duration::from_millis(1) {
                debug!(
                    stream_id,
                    bytes,
                    waited_ms = waited.as_millis() as u64,
                    "waited for tunnel response flow-control credit"
                );
            }
            true
        }
        Err(()) => {
            warn!(
                stream_id,
                bytes,
                timeout_ms = FLOW_CONTROL_WAIT_TIMEOUT.as_millis() as u64,
                "response flow-control window timeout"
            );
            send_reset_stream(frame_tx, stream_id, "response_flow_control_timeout").await;
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn relay_upstream_response<B>(
    server: &ServerContext,
    stream_id: u32,
    method: &hyper::Method,
    request_url: &url::Url,
    frame_tx: &FrameSender,
    response_window: &StreamSendWindow,
    response: hyper::Response<B>,
    total_dns_ms: u64,
    total_elapsed: Duration,
    request_timing: upstream_client::RequestTiming,
    request_body_size: &AtomicUsize,
    redirect_count: usize,
    request_body_mode: &'static str,
    emit_proxy_timing_header: bool,
    response_body_deadline: Option<Instant>,
) -> Option<Duration>
where
    B: hyper::body::Body<Data = Bytes> + Send + Unpin + 'static,
    B::Error: std::fmt::Display,
{
    let status = response.status().as_u16();
    let ttfb_ms = total_elapsed.as_millis() as u64;
    let connection_declared = aether_http::connection_declared_header_names(
        response
            .headers()
            .get_all(hyper::header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok()),
    );
    let mut resp_headers: Vec<(String, String)> = Vec::with_capacity(response.headers().len() + 1);
    for (key, value) in response.headers() {
        let normalized = key.as_str().to_ascii_lowercase();
        if BLOCKED_HEADERS.contains(&normalized.as_str())
            || connection_declared.contains(&normalized)
        {
            continue;
        }
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
    if emit_proxy_timing_header {
        resp_headers.push(("x-proxy-timing".to_string(), timing.to_string()));
    }
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
    let chunk_size = MAX_CHUNK_SIZE.min(response_window.initial_window_bytes as usize);
    loop {
        let chunk_result = if let Some(deadline) = response_body_deadline {
            let Some(remaining) = remaining_timeout(deadline) else {
                server.metrics.stream_errors.fetch_add(1, Ordering::Release);
                let error_message = "upstream response body timeout".to_string();
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
                send_error(frame_tx, stream_id, &error_message).await;
                return Some(total_elapsed);
            };

            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(chunk_result) => chunk_result,
                Err(_) => {
                    server.metrics.stream_errors.fetch_add(1, Ordering::Release);
                    let error_message = "upstream response body timeout".to_string();
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
                    send_error(frame_tx, stream_id, &error_message).await;
                    return Some(total_elapsed);
                }
            }
        } else {
            stream.next().await
        };

        let Some(chunk_result) = chunk_result else {
            break;
        };

        match chunk_result {
            Ok(chunk) => {
                if chunk.len() <= chunk_size {
                    let (payload, extra_flags) = raw_payload(chunk);
                    if !acquire_response_credit(response_window, frame_tx, stream_id, payload.len())
                        .await
                    {
                        return Some(total_elapsed);
                    }
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
                        let end = (offset + chunk_size).min(chunk.len());
                        let slice = chunk.slice(offset..end);
                        let (payload, extra_flags) = raw_payload(slice);
                        if !acquire_response_credit(
                            response_window,
                            frame_tx,
                            stream_id,
                            payload.len(),
                        )
                        .await
                        {
                            return Some(total_elapsed);
                        }
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
                let error_kind = safe_stream_error_message(&error.to_string());
                warn!(stream_id, error_kind, "upstream body read error");
                let error_message = error_kind;
                log_stream_failure(
                    stream_log_context(
                        server,
                        stream_id,
                        method,
                        Some(request_url),
                        redirect_count,
                        request_body_size.load(Ordering::Relaxed),
                    ),
                    error_message,
                    total_elapsed,
                );
                send_error(frame_tx, stream_id, error_message).await;
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
fn upstream_client_pool_key_for_request(
    meta: &RequestMeta,
) -> upstream_client::UpstreamClientPoolKey {
    let target_url = url::Url::parse(&meta.url).expect("test request URL should parse");
    let port = target_url
        .port_or_known_default()
        .expect("test request URL should have a port");
    let validated_target = upstream_client::ValidatedUpstreamTarget::new(
        &target_url,
        vec![std::net::SocketAddr::from(([203, 0, 113, 1], port))],
    )
    .expect("test target should validate");
    upstream_client::upstream_client_pool_key(
        meta.provider_id.as_deref(),
        meta.endpoint_id.as_deref(),
        meta.key_id.as_deref(),
        meta.transport_profile.as_ref(),
        meta.http1_only,
        validated_target,
    )
}

/// Handle a single stream: receive body, execute upstream, send response.
pub async fn handle_stream(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    stream_id: u32,
    meta: RequestMeta,
    body_rx: mpsc::Receiver<TunnelFrame>,
    frame_tx: FrameSender,
    response_window: Arc<StreamSendWindow>,
) {
    let request_method = parse_request_method(&meta.method);
    let request_url = url::Url::parse(&meta.url).ok();
    let permit = match state.try_acquire_stream_permit().await {
        Ok(permit) => permit,
        Err(err) => {
            let message = match err {
                crate::state::TunnelAdmissionError::Saturated { .. } => "tunnel overloaded",
                crate::state::TunnelAdmissionError::Unavailable { .. } => {
                    "tunnel admission unavailable"
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
    let _active_stream = ActiveStreamGuard(Arc::clone(&server));

    let stream_io = StreamIo {
        body_rx,
        frame_tx: &frame_tx,
        response_window: response_window.as_ref(),
        admission_permit: permit,
    };

    let connect_elapsed = handle_stream_inner(&state, &server, stream_id, meta, stream_io).await;

    if let Some(d) = connect_elapsed {
        server.metrics.record_request(d);
    }
}

/// Send a frame to the writer with a timeout. Returns false if send failed.
async fn send_frame(tx: &FrameSender, frame: TunnelFrame) -> bool {
    let stream_id = frame.stream_id;
    let msg_type = frame.msg_type;
    let flags = frame.flags;
    let is_body_frame = matches!(
        msg_type,
        MsgType::RequestBody | MsgType::ResponseBody | MsgType::StreamEnd
    );

    if is_body_frame {
        match tokio::time::timeout(FLOW_CONTROL_WAIT_TIMEOUT, tx.send(frame)).await {
            Ok(Ok(())) => true,
            Ok(Err(QueueSendError::Closed(_))) | Err(_) => {
                warn!(
                    stream_id,
                    msg_type = ?msg_type,
                    flags = flags,
                    timeout_ms = FLOW_CONTROL_WAIT_TIMEOUT.as_millis() as u64,
                    "writer channel stalled for body frame, abandoning stream"
                );
                let reset = TunnelFrame::new(
                    stream_id,
                    MsgType::ResetStream,
                    0,
                    Bytes::from_static(b"{\"reason\":\"tunnel writer stalled\"}"),
                );
                if !matches!(
                    tokio::time::timeout(CONTROL_FRAME_SEND_TIMEOUT, tx.send(reset)).await,
                    Ok(Ok(()))
                ) {
                    tx.close();
                }
                false
            }
            Ok(Err(QueueSendError::Full(_))) => {
                unreachable!("bounded queue send should not report full")
            }
        }
    } else {
        match tokio::time::timeout(CONTROL_FRAME_SEND_TIMEOUT, tx.send(frame)).await {
            Ok(Ok(())) => true,
            Ok(Err(_)) => {
                tx.close();
                false
            }
            Err(_) => {
                warn!(
                    stream_id,
                    msg_type = ?msg_type,
                    flags = flags,
                    "control frame send timeout (writer congested), abandoning stream"
                );
                tx.close();
                false
            }
        }
    }
}

/// Returns the connection-establishment duration (DNS + TCP/TLS + TTFB) if the
/// upstream request succeeded, or `None` if the request never reached the
/// response-headers stage.
struct StreamIo<'a> {
    body_rx: mpsc::Receiver<TunnelFrame>,
    frame_tx: &'a FrameSender,
    response_window: &'a StreamSendWindow,
    admission_permit: Option<AdmissionPermit>,
}

async fn handle_stream_inner(
    state: &AppState,
    server: &ServerContext,
    stream_id: u32,
    meta: RequestMeta,
    stream_io: StreamIo<'_>,
) -> Option<Duration> {
    let StreamIo {
        body_rx,
        frame_tx,
        response_window,
        mut admission_permit,
    } = stream_io;

    let mut current_method: hyper::Method = parse_request_method(&meta.method);
    let mut current_url = match url::Url::parse(&meta.url) {
        Ok(u) => u,
        Err(_) => {
            log_stream_failure(
                stream_log_context(server, stream_id, &current_method, None, 0, 0),
                "invalid upstream URL",
                Duration::ZERO,
            );
            send_error(frame_tx, stream_id, "invalid upstream URL").await;
            return None;
        }
    };

    if let Err(error_message) =
        validate_tunnel_upstream_url(&current_url, state.config.allow_private_targets)
    {
        log_stream_failure(
            stream_log_context(server, stream_id, &current_method, Some(&current_url), 0, 0),
            error_message,
            Duration::ZERO,
        );
        send_error(frame_tx, stream_id, error_message).await;
        return None;
    }

    let overall_start = Instant::now();
    let request_timeouts = resolve_request_timeouts(&meta);
    let first_byte_deadline = overall_start + request_timeouts.first_byte_timeout;
    let response_body_deadline = request_timeouts
        .response_body_timeout
        .map(|timeout| overall_start + timeout);
    let follow_redirects = follow_redirects_enabled(&meta);
    let request_has_body = request_likely_has_body(&current_method, &meta.headers);
    let mut current_headers = sanitize_upstream_headers(&meta.headers);
    if request_has_body {
        if let Some(content_length) = validated_request_content_length(&meta.headers) {
            current_headers.push((
                hyper::header::CONTENT_LENGTH.as_str().to_string(),
                content_length.to_string(),
            ));
        }
    }
    let first_byte_timeout = request_timeouts.first_byte_timeout;
    let request_body_size = Arc::new(AtomicUsize::new(0));
    let request_body_mode = if request_has_body {
        "streaming"
    } else {
        "empty"
    };
    let mut prepared_body = if request_has_body {
        prepare_request_body(
            stream_id,
            body_rx,
            Arc::clone(&request_body_size),
            first_byte_deadline,
            follow_redirects,
            frame_tx.clone(),
        )
    } else {
        prepare_bodyless_request_body(body_rx, follow_redirects)
    };

    let mut total_dns_ms = 0u64;
    let mut redirects_followed = 0usize;
    let mut next_request_body = None::<upstream_client::UpstreamRequestBody>;

    if follow_redirects {
        if let Err(message) = prepared_body
            .resolve_initial_replay_body(first_byte_deadline)
            .await
        {
            if let ReplayableRequestBody::Pending(state) = &prepared_body.replay_body {
                state.discard();
            }
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
    }

    loop {
        let Some(remaining) = remaining_timeout(first_byte_deadline) else {
            if let ReplayableRequestBody::Pending(state) = &prepared_body.replay_body {
                state.discard();
            }
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
            &meta,
            &current_url,
            current_method.clone(),
            &current_headers,
            request_body,
            remaining.min(first_byte_timeout),
            meta.http1_only,
        )
        .await
        {
            Ok(context) => context,
            Err(message) => {
                if let ReplayableRequestBody::Pending(state) = &prepared_body.replay_body {
                    state.discard();
                }
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
                    if let ReplayableRequestBody::Pending(state) = &prepared_body.replay_body {
                        state.discard();
                    }
                    drop(admission_permit.take());
                    return relay_upstream_response(
                        server,
                        stream_id,
                        &current_method,
                        &current_url,
                        frame_tx,
                        response_window,
                        response_ctx.response,
                        total_dns_ms,
                        overall_start.elapsed(),
                        response_ctx.request_timing,
                        request_body_size.as_ref(),
                        redirects_followed,
                        request_body_mode,
                        state.config.emit_proxy_timing_header,
                        response_body_deadline,
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
                    first_byte_deadline,
                )
                .await
                {
                    Ok(Some(body)) => {
                        if body_mode == RedirectBodyMode::Empty {
                            if let ReplayableRequestBody::Pending(state) =
                                &prepared_body.replay_body
                            {
                                state.discard();
                            }
                            prepared_body.replay_body = ReplayableRequestBody::None;
                        }
                        redirects_followed += 1;
                        current_method = method;
                        current_url = url;
                        current_headers = headers;
                        next_request_body = Some(body);
                        continue;
                    }
                    Ok(None) => {
                        if let ReplayableRequestBody::Pending(state) = &prepared_body.replay_body {
                            state.discard();
                        }
                        drop(admission_permit.take());
                        return relay_upstream_response(
                            server,
                            stream_id,
                            &current_method,
                            &current_url,
                            frame_tx,
                            response_window,
                            response_ctx.response,
                            total_dns_ms,
                            overall_start.elapsed(),
                            response_ctx.request_timing,
                            request_body_size.as_ref(),
                            redirects_followed,
                            request_body_mode,
                            state.config.emit_proxy_timing_header,
                            response_body_deadline,
                        )
                        .await;
                    }
                    Err(message) => {
                        if let ReplayableRequestBody::Pending(state) = &prepared_body.replay_body {
                            state.discard();
                        }
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
                    if let ReplayableRequestBody::Pending(state) = &prepared_body.replay_body {
                        state.discard();
                    }
                    let error_message =
                        safe_stream_error_message(&format!("upstream redirect error: {message}"));
                    log_stream_failure(
                        stream_log_context(
                            server,
                            stream_id,
                            &current_method,
                            Some(&current_url),
                            redirects_followed,
                            request_body_size.load(Ordering::Relaxed),
                        ),
                        error_message,
                        overall_start.elapsed(),
                    );
                    send_error(frame_tx, stream_id, error_message).await;
                    return None;
                }
            }
        }

        if let ReplayableRequestBody::Pending(state) = &prepared_body.replay_body {
            state.discard();
        }
        drop(admission_permit.take());
        return relay_upstream_response(
            server,
            stream_id,
            &current_method,
            &current_url,
            frame_tx,
            response_window,
            response_ctx.response,
            total_dns_ms,
            overall_start.elapsed(),
            response_ctx.request_timing,
            request_body_size.as_ref(),
            redirects_followed,
            request_body_mode,
            state.config.emit_proxy_timing_header,
            response_body_deadline,
        )
        .await;
    }
}

async fn send_error(tx: &FrameSender, stream_id: u32, msg: &str) {
    let safe_message = safe_stream_error_message(msg);
    let _ = send_frame(
        tx,
        TunnelFrame::new(
            stream_id,
            MsgType::StreamError,
            0,
            Bytes::from_static(safe_message.as_bytes()),
        ),
    )
    .await;
}

async fn send_reset_stream(tx: &FrameSender, stream_id: u32, reason: &str) {
    let safe_reason = safe_stream_error_message(reason);
    let payload = serde_json::to_vec(&ResetStreamPayload {
        reason: safe_reason.to_string(),
    })
    .expect("reset stream payload should serialize");
    let _ = send_frame(
        tx,
        TunnelFrame::new(stream_id, MsgType::ResetStream, 0, Bytes::from(payload)),
    )
    .await;
}

#[cfg(test)]
fn build_streaming_request_body(
    body_rx: mpsc::Receiver<TunnelFrame>,
    body_size: Arc<AtomicUsize>,
) -> upstream_client::UpstreamRequestBody {
    build_prefixed_request_body(Vec::new(), body_rx, body_size)
}

fn build_spooled_request_body(
    spool_rx: mpsc::Receiver<SpoolBodyEvent>,
    stream_id: u32,
    frame_tx: FrameSender,
) -> upstream_client::UpstreamRequestBody {
    let body_stream = stream::unfold(
        (spool_rx, frame_tx, false),
        move |(mut spool_rx, frame_tx, finished)| async move {
            if finished {
                return None;
            }

            match spool_rx.recv().await {
                Some(SpoolBodyEvent::Data {
                    payload,
                    credit_returned,
                }) => {
                    if !credit_returned
                        && !send_window_update(&frame_tx, stream_id, payload.len()).await
                    {
                        return Some((
                            Err(io::Error::other("tunnel flow-control update failed")),
                            (spool_rx, frame_tx, true),
                        ));
                    }
                    Some((Ok(BodyFrame::data(payload)), (spool_rx, frame_tx, false)))
                }
                Some(SpoolBodyEvent::Error(message)) => {
                    Some((Err(io::Error::other(message)), (spool_rx, frame_tx, true)))
                }
                Some(SpoolBodyEvent::End) => None,
                None => Some((
                    Err(io::Error::other("tunnel request body ended unexpectedly")),
                    (spool_rx, frame_tx, true),
                )),
            }
        },
    );

    upstream_client::stream_request_body(body_stream)
}

#[cfg(test)]
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
                        let payload = match decode_request_body_frame(frame) {
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
                    MsgType::StreamError | MsgType::ResetStream => {
                        let message = stream_reset_message(&frame);
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
    #[tokio::test]
    async fn remote_dns_target_resolution_skips_local_dns_and_keeps_literal_acl() {
        let cache = target_filter::DnsCache::new(Duration::from_secs(60), 16);
        let ports = [80, 443].into_iter().collect();
        let url = url::Url::parse("https://remote-dns-test.invalid/path").unwrap();
        let target = resolve_upstream_target(&url, &ports, false, true, &cache)
            .await
            .expect("trusted proxy should receive an unresolved hostname");
        assert!(target.uses_proxy_dns());
        assert!(cache.get("remote-dns-test.invalid", 443).await.is_none());

        for address in ["https://8.8.8.8/", "https://[2606:4700:4700::1111]/"] {
            let target = resolve_upstream_target(
                &url::Url::parse(address).unwrap(),
                &ports,
                false,
                true,
                &cache,
            )
            .await
            .unwrap();
            assert!(!target.uses_proxy_dns(), "IP literals must remain pinned");
        }

        for address in [
            "https://127.0.0.1/",
            "https://10.0.0.1/",
            "https://198.18.0.1/",
            "https://[::1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://localhost/",
            "https://LOCALHOST./",
            "https://remote-dns-test.invalid:25/",
            "https://user:secret@remote-dns-test.invalid/",
            "https://remote-dns-test.invalid/#fragment",
            "ftp://remote-dns-test.invalid/",
        ] {
            assert!(
                resolve_upstream_target(
                    &url::Url::parse(address).unwrap(),
                    &ports,
                    false,
                    true,
                    &cache,
                )
                .await
                .is_err(),
                "target should remain blocked: {address}"
            );
        }
    }

    #[tokio::test]
    async fn strict_dns_targets_stay_pinned_and_separate_from_remote_dns_targets() {
        let cache = target_filter::DnsCache::new(Duration::from_secs(60), 16);
        let ports = [443].into_iter().collect();
        let url = url::Url::parse("https://remote-dns-test.invalid/").unwrap();
        cache
            .insert(
                "remote-dns-test.invalid",
                443,
                Arc::new(vec!["8.8.8.8:443".parse().unwrap()]),
            )
            .await;
        let strict = resolve_upstream_target(&url, &ports, false, false, &cache)
            .await
            .unwrap();
        let remote = resolve_upstream_target(&url, &ports, false, true, &cache)
            .await
            .unwrap();
        assert!(!strict.uses_proxy_dns());
        assert!(remote.uses_proxy_dns());
        assert_ne!(strict, remote);

        let private_url = url::Url::parse("http://[::1]/").unwrap();
        let private_ports = [80].into_iter().collect();
        let explicitly_allowed =
            resolve_upstream_target(&private_url, &private_ports, true, true, &cache)
                .await
                .unwrap();
        assert!(!explicitly_allowed.uses_proxy_dns());
    }

    #[tokio::test(start_paused = true)]
    async fn window_updates_wait_for_capacity_instead_of_disappearing() {
        let (high_tx, mut high_rx) = aether_runtime::bounded_queue(1);
        let (normal_tx, _normal_rx) = aether_runtime::bounded_queue(1);
        let sender = FrameSender::from_test_queues(high_tx, normal_tx);
        sender
            .try_send(TunnelFrame::control(MsgType::Ping, Bytes::new()))
            .unwrap();
        let task = tokio::spawn(async move { send_window_update(&sender, 7, 1024).await });
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!task.is_finished());
        high_rx.recv().await.unwrap();
        assert!(task.await.unwrap());
        let update = high_rx.recv().await.unwrap();
        assert_eq!(update.msg_type, MsgType::WindowUpdate);
        let payload: aether_contracts::tunnel::WindowUpdatePayload =
            serde_json::from_slice(&update.payload).unwrap();
        assert_eq!(payload.delta_bytes, 1024);
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_body_delivery_emits_a_reset() {
        let (high_tx, mut high_rx) = aether_runtime::bounded_queue(4);
        let (normal_tx, _normal_rx) = aether_runtime::bounded_queue(1);
        let sender = FrameSender::from_test_queues(high_tx, normal_tx);
        sender
            .try_send(TunnelFrame::new(
                7,
                MsgType::ResponseBody,
                0,
                Bytes::from_static(b"first"),
            ))
            .unwrap();
        assert!(
            !send_frame(
                &sender,
                TunnelFrame::new(7, MsgType::ResponseBody, 0, Bytes::from_static(b"second"))
            )
            .await
        );
        let reset = high_rx.recv().await.unwrap();
        assert_eq!(reset.msg_type, MsgType::ResetStream);
        assert_eq!(reset.stream_id, 7);
    }

    #[tokio::test]
    async fn request_credit_follows_consumption_without_redirect_replay() {
        let (body_tx, body_rx) = mpsc::channel(4);
        let (high_tx, mut high_rx) = aether_runtime::bounded_queue(4);
        let (normal_tx, _normal_rx) = aether_runtime::bounded_queue(4);
        let sender = FrameSender::from_test_queues(high_tx, normal_tx);
        let mut prepared = prepare_request_body(
            7,
            body_rx,
            Arc::new(AtomicUsize::new(0)),
            Instant::now() + Duration::from_secs(10),
            false,
            sender,
        );
        body_tx
            .send(TunnelFrame::new(
                7,
                MsgType::RequestBody,
                flags::END_STREAM,
                Bytes::from_static(b"body"),
            ))
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert!(high_rx.try_recv().is_err());
        let mut body = prepared.take_first_request_body();
        assert!(body.frame().await.unwrap().is_ok());
        assert_eq!(
            high_rx.recv().await.unwrap().msg_type,
            MsgType::WindowUpdate
        );
        assert!(body.frame().await.is_none());
    }

    #[tokio::test]
    async fn dropping_prepared_body_cancels_its_spooler() {
        let (body_tx, body_rx) = mpsc::channel(4);
        let (high_tx, _high_rx) = aether_runtime::bounded_queue(4);
        let (normal_tx, _normal_rx) = aether_runtime::bounded_queue(4);
        let sender = FrameSender::from_test_queues(high_tx, normal_tx);
        let prepared = prepare_request_body(
            7,
            body_rx,
            Arc::new(AtomicUsize::new(0)),
            Instant::now() + Duration::from_secs(3600),
            false,
            sender,
        );
        drop(prepared);
        tokio::time::timeout(Duration::from_secs(1), body_tx.closed())
            .await
            .unwrap();
    }

    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Mutex, Once};
    use std::task::{Context, Poll};

    use aether_runtime::ConcurrencyGate;
    use aether_runtime_state::{
        MemoryRuntimeStateConfig, RuntimeSemaphore, RuntimeSemaphoreConfig, RuntimeState,
    };
    use arc_swap::ArcSwap;
    use axum::body::Body;
    use axum::http::{header, HeaderMap, Response, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use bytes::BytesMut;
    use futures_util::Sink;
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};

    use super::*;
    use crate::config::Config;
    use crate::registration::client::AetherClient;
    use crate::runtime::DynamicConfig;
    use crate::state::{TunnelMetrics, TunnelRequestMetrics};
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
    async fn bodyless_request_body_completes_without_waiting_for_tunnel_sender() {
        let (_tx, rx) = mpsc::channel(4);
        let mut prepared = prepare_bodyless_request_body(rx, true);
        let mut body = prepared
            .first_request_body
            .take()
            .expect("bodyless request should have an initial body");

        let frame = tokio::time::timeout(Duration::from_millis(25), body.frame())
            .await
            .expect("bodyless request body should not wait for tunnel body frames");
        assert!(frame.is_none());
        assert!(matches!(prepared.replay_body, ReplayableRequestBody::None));
    }

    #[tokio::test]
    async fn prepare_request_body_streams_immediately_and_replays_after_completion() {
        let (tx, rx) = mpsc::channel(4);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let body_size = Arc::new(AtomicUsize::new(0));
        let mut prepared = prepare_request_body(
            1,
            rx,
            Arc::clone(&body_size),
            Instant::now() + Duration::from_secs(1),
            true,
            frame_tx.clone(),
        );
        let mut body = prepared
            .first_request_body
            .take()
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

        let replay = prepare_redirect_request_body(
            prepared.replay_body.clone(),
            RedirectBodyMode::Replay,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .expect("redirect replay should resolve")
        .expect("body should be replayable");
        let replay = replay
            .collect()
            .await
            .expect("replayed body should be readable")
            .to_bytes();
        assert_eq!(replay, Bytes::from_static(b"hello world"));
        assert_eq!(body_size.load(Ordering::Relaxed), 11);
        let window_update_bytes = collect_emitted_frames(frame_tx, sent, writer_handle)
            .await
            .into_iter()
            .filter(|frame| frame.msg_type == MsgType::WindowUpdate)
            .filter_map(|frame| {
                serde_json::from_slice::<aether_contracts::tunnel::WindowUpdatePayload>(
                    &frame.payload,
                )
                .ok()
            })
            .map(|payload| payload.delta_bytes as usize)
            .sum::<usize>();
        assert_eq!(window_update_bytes, 11);
    }

    #[tokio::test]
    async fn replay_state_disables_and_releases_cache_after_per_request_budget() {
        let state = RequestBodyReplayState::new(5);
        state.push_chunk(Bytes::from_static(b"123"));
        assert!(state.reserved_bytes.load(Ordering::Acquire) > 0);

        state.push_chunk(Bytes::from_static(b"456"));

        assert_eq!(state.reserved_bytes.load(Ordering::Acquire), 0);
        assert_eq!(
            state
                .wait_for_resolution(Instant::now() + Duration::from_secs(1))
                .await
                .expect("over-budget replay should resolve without failing the request"),
            ReplayBodyResolution::NonReplayable
        );
    }

    #[test]
    fn selects_http1_only_client_when_request_metadata_requires_it() {
        let default_meta = sample_request_meta();
        assert_eq!(
            upstream_client_pool_key_for_request(&default_meta).http_mode,
            "auto"
        );

        let mut http1_meta = sample_request_meta();
        http1_meta.http1_only = true;
        assert_eq!(
            upstream_client_pool_key_for_request(&http1_meta).http_mode,
            "http1_only"
        );
    }

    #[test]
    fn stream_error_projection_never_returns_upstream_details() {
        let secret_error = concat!(
            "upstream connect error: error sending request for url (",
            "https://user:password@example.test/v1/models?api_key=query-secret",
            ")"
        );
        assert_eq!(
            safe_stream_error_message(secret_error),
            "upstream connect failed"
        );
        assert_eq!(
            safe_stream_error_message(
                "upstream body read error: authorization Bearer secret-token at 10.0.0.4"
            ),
            "upstream response body failed"
        );
        assert_eq!(
            safe_stream_error_message("invalid URL: https://user:pass@example.test/?token=secret"),
            "invalid upstream URL"
        );
    }

    #[test]
    fn tunnel_upstream_url_validation_rejects_ambiguous_url_components() {
        for raw in [
            "https://user:password@example.test/v1",
            "https://user@example.test/v1",
            "https://example.test/v1#fragment",
            "file:///etc/passwd",
        ] {
            let url = url::Url::parse(raw).expect("fixture URL should parse");
            assert!(
                validate_tunnel_upstream_url(&url, true).is_err(),
                "URL should be rejected at the tunnel boundary: {raw}"
            );
        }

        assert!(validate_tunnel_upstream_url(
            &url::Url::parse("https://example.test/v1?api_key=query-secret")
                .expect("query URL should parse"),
            true,
        )
        .is_ok());
    }

    #[test]
    fn tunnel_upstream_url_validation_applies_literal_target_policy() {
        let private = url::Url::parse("https://10.0.0.8/private").expect("private URL");
        assert!(validate_tunnel_upstream_url(&private, false).is_err());
        assert!(validate_tunnel_upstream_url(&private, true).is_ok());

        // Loopback remains available to explicitly enabled local deployments;
        // disabling private targets rejects it before connection setup.
        let loopback = url::Url::parse("http://127.0.0.1:8080/local").expect("loopback URL");
        assert!(validate_tunnel_upstream_url(&loopback, false).is_err());
        assert!(validate_tunnel_upstream_url(&loopback, true).is_ok());
    }

    #[test]
    fn peer_reset_payload_is_not_echoed_into_request_errors() {
        let frame = TunnelFrame::new(
            1,
            MsgType::StreamError,
            0,
            Bytes::from_static(b"Authorization: Bearer secret-token"),
        );
        assert_eq!(
            stream_reset_message(&frame),
            "client cancelled request body"
        );

        let reset = TunnelFrame::new(
            1,
            MsgType::ResetStream,
            0,
            Bytes::from_static(b"https://user:pass@example.test/?token=secret"),
        );
        assert_eq!(stream_reset_message(&reset), "request reset by peer");
    }

    #[test]
    fn upstream_client_pool_key_isolates_accounts() {
        let mut first = sample_request_meta();
        first.provider_id = Some("provider-1".to_string());
        first.endpoint_id = Some("endpoint-1".to_string());
        first.key_id = Some("key-1".to_string());
        first.transport_profile = Some(aether_contracts::ResolvedTransportProfile {
            profile_id: "profile-a".to_string(),
            backend: "reqwest_rustls".to_string(),
            http_mode: "auto".to_string(),
            pool_scope: "key".to_string(),
            header_fingerprint: None,
            extra: None,
        });
        let mut second = first.clone();
        second.key_id = Some("key-2".to_string());

        assert_ne!(
            upstream_client_pool_key_for_request(&first),
            upstream_client_pool_key_for_request(&second)
        );
    }

    #[test]
    fn stream_request_timeouts_use_first_byte_without_response_body_deadline() {
        let mut meta = sample_request_meta();
        meta.stream = true;
        meta.request_timeout_ms = Some(90_000);
        meta.stream_first_byte_timeout_ms = Some(12_345);

        let timeouts = resolve_request_timeouts(&meta);

        assert_eq!(timeouts.first_byte_timeout, Duration::from_millis(12_345));
        assert!(timeouts.response_body_timeout.is_none());
    }

    #[test]
    fn stream_request_timeouts_ignore_request_timeout_when_first_byte_missing() {
        let mut meta = sample_request_meta();
        meta.stream = true;
        meta.request_timeout_ms = Some(90_000);
        meta.stream_first_byte_timeout_ms = None;
        meta.timeout = 7;

        let timeouts = resolve_request_timeouts(&meta);

        assert_eq!(timeouts.first_byte_timeout, Duration::from_secs(7));
        assert!(timeouts.response_body_timeout.is_none());
    }

    #[test]
    fn non_stream_request_timeouts_use_total_for_response_body_deadline() {
        let mut meta = sample_request_meta();
        meta.request_timeout_ms = Some(90_000);
        meta.stream_first_byte_timeout_ms = Some(12_345);

        let timeouts = resolve_request_timeouts(&meta);

        assert_eq!(timeouts.first_byte_timeout, Duration::from_millis(90_000));
        assert_eq!(
            timeouts.response_body_timeout,
            Some(Duration::from_millis(90_000))
        );
    }

    #[test]
    fn non_stream_request_timeouts_keep_the_protocol_maximum() {
        let mut meta = sample_request_meta();
        meta.request_timeout_ms = Some(aether_contracts::MAX_EXECUTION_REQUEST_TIMEOUT_MS);

        let timeouts = resolve_request_timeouts(&meta);

        let expected = Duration::from_millis(aether_contracts::MAX_EXECUTION_REQUEST_TIMEOUT_MS);
        assert_eq!(timeouts.first_byte_timeout, expected);
        assert_eq!(timeouts.response_body_timeout, Some(expected));
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
    fn resolve_redirect_stops_cross_origin_redirects() {
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
                ("api-key".into(), "api-key-secret".into()),
                ("cookie".into(), "sid=123".into()),
                ("x-api-key".into(), "x-api-key-secret".into()),
                ("x-goog-api-key".into(), "google-secret".into()),
                ("x-custom".into(), "keep".into()),
            ],
            &ReplayableRequestBody::None,
            0,
        );

        assert_eq!(decision, RedirectDecision::Stop);
    }

    #[test]
    fn resolve_redirect_stops_https_to_http_downgrade() {
        let current_url = url::Url::parse("https://redirect.test/start").expect("url");
        let response = Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, "http://redirect.test/final")
            .body(())
            .expect("response");

        let decision = resolve_redirect(
            &response,
            &current_url,
            &hyper::Method::GET,
            &[("authorization".into(), "Bearer secret".into())],
            &ReplayableRequestBody::None,
            0,
        );

        assert_eq!(decision, RedirectDecision::Stop);
    }

    #[test]
    fn resolve_redirect_does_not_follow_userinfo_or_fragment_urls() {
        let current_url = url::Url::parse("https://redirect.test/start").expect("url");
        for location in ["https://user:password@redirect.test/final", "/final#secret"] {
            let response = Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, location)
                .body(())
                .expect("response");

            let decision = resolve_redirect(
                &response,
                &current_url,
                &hyper::Method::GET,
                &[],
                &ReplayableRequestBody::None,
                0,
            );
            assert_eq!(decision, RedirectDecision::Stop, "location: {location}");
        }
    }

    #[test]
    fn resolve_redirect_never_replays_post_body_cross_origin() {
        let current_url = url::Url::parse("https://oauth.example/token").expect("url");
        let response = Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, "https://attacker.example/capture")
            .body(())
            .expect("response");

        let decision = resolve_redirect(
            &response,
            &current_url,
            &hyper::Method::POST,
            &[(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            &completed_replay_body(Bytes::from_static(
                b"refresh_token=secret&client_secret=secret",
            )),
            0,
        );

        assert_eq!(decision, RedirectDecision::Stop);
    }

    #[test]
    fn connection_declared_response_headers_are_not_relayed() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONNECTION, "x-hop-private, x-accel-redirect")
            .header("x-hop-private", "secret")
            .header("x-accel-redirect", "/internal")
            .header("x-visible", "ok")
            .body(())
            .expect("response");
        let declared = aether_http::connection_declared_header_names(
            response
                .headers()
                .get_all(header::CONNECTION)
                .iter()
                .filter_map(|value| value.to_str().ok()),
        );

        assert!(declared.contains("x-hop-private"));
        assert!(declared.contains("x-accel-redirect"));
        assert!(!declared.contains("x-visible"));
    }

    #[test]
    fn connection_declared_request_headers_are_not_sent_upstream() {
        let headers = std::collections::HashMap::from([
            ("Connection".to_string(), "x-hop-private".to_string()),
            ("X-Hop-Private".to_string(), "secret".to_string()),
            ("X-Visible".to_string(), "ok".to_string()),
        ]);

        let sanitized = sanitize_upstream_headers(&headers);

        assert!(!sanitized
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("connection")));
        assert!(!sanitized
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("x-hop-private")));
        assert!(sanitized
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("x-visible") && value == "ok"));
    }

    #[test]
    fn validates_single_content_length_value() {
        let headers = HashMap::from([("content-length".to_string(), "42".to_string())]);

        assert_eq!(validated_request_content_length(&headers), Some(42));
    }

    #[test]
    fn accepts_identical_case_variant_content_lengths() {
        let headers = HashMap::from([
            ("Content-Length".to_string(), " 42 ".to_string()),
            ("content-length".to_string(), "42".to_string()),
        ]);

        assert_eq!(validated_request_content_length(&headers), Some(42));
    }

    #[test]
    fn rejects_conflicting_case_variant_content_lengths() {
        let headers = HashMap::from([
            ("Content-Length".to_string(), "42".to_string()),
            ("CONTENT-LENGTH".to_string(), "43".to_string()),
        ]);

        assert_eq!(validated_request_content_length(&headers), None);
    }

    #[test]
    fn rejects_content_length_when_transfer_encoding_is_present() {
        let headers = HashMap::from([
            ("Content-Length".to_string(), "42".to_string()),
            ("Transfer-Encoding".to_string(), "chunked".to_string()),
        ]);

        assert_eq!(validated_request_content_length(&headers), None);
    }

    #[test]
    fn rejects_empty_or_invalid_content_length_values() {
        for value in [
            "",
            " ",
            "+42",
            "-1",
            "42, 42",
            "not-a-length",
            "18446744073709551616",
        ] {
            let headers = HashMap::from([("content-length".to_string(), value.to_string())]);
            assert_eq!(
                validated_request_content_length(&headers),
                None,
                "unexpectedly accepted Content-Length value {value:?}"
            );
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
            test_response_window(),
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
            test_response_window(),
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
    async fn response_body_timeout_emits_stream_error() {
        let state = sample_state(None, None);
        let server = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let request_url = url::Url::parse("https://example.com/slow").expect("url");
        let request_body_size = AtomicUsize::new(0);
        let body = Body::from_stream(futures_util::stream::pending::<
            Result<Bytes, std::convert::Infallible>,
        >());
        let response = Response::builder()
            .status(StatusCode::OK)
            .body(body)
            .expect("response");
        let response_window = test_response_window();

        relay_upstream_response(
            &server,
            13,
            &hyper::Method::GET,
            &request_url,
            &frame_tx,
            response_window.as_ref(),
            response,
            0,
            Duration::ZERO,
            upstream_client::RequestTiming::default(),
            &request_body_size,
            0,
            "empty",
            true,
            Some(Instant::now()),
        )
        .await;

        let result = collect_stream_result(frame_tx, sent, writer_handle).await;
        assert_eq!(result.response.expect("response metadata").status, 200);
        assert_eq!(
            result.error.as_deref(),
            Some("upstream response body timeout")
        );
        assert_eq!(server.metrics.stream_errors.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn stream_response_body_without_total_deadline_allows_late_chunk() {
        let state = sample_state(None, None);
        let server = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let request_url = url::Url::parse("https://example.com/stream").expect("url");
        let request_body_size = AtomicUsize::new(0);
        let body = Body::from_stream(stream::once(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<Bytes, std::convert::Infallible>(Bytes::from_static(b"late"))
        }));
        let response = Response::builder()
            .status(StatusCode::OK)
            .body(body)
            .expect("response");
        let response_window = test_response_window();

        relay_upstream_response(
            &server,
            14,
            &hyper::Method::GET,
            &request_url,
            &frame_tx,
            response_window.as_ref(),
            response,
            0,
            Duration::ZERO,
            upstream_client::RequestTiming::default(),
            &request_body_size,
            0,
            "empty",
            true,
            None,
        )
        .await;

        let result = collect_stream_result(frame_tx, sent, writer_handle).await;
        assert!(
            result.error.is_none(),
            "unexpected error: {:?}",
            result.error
        );
        assert_eq!(result.response.expect("response metadata").status, 200);
        assert_eq!(result.body, Bytes::from_static(b"late"));
        assert_eq!(server.metrics.stream_errors.load(Ordering::Acquire), 0);
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
            test_response_window(),
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
    async fn cross_origin_redirect_is_preserved_without_a_second_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new().route(
            "/start",
            get(move || async move {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(
                        header::LOCATION,
                        format!("http://127.0.0.1:{}/private", addr.port()),
                    )
                    .body(Body::empty())
                    .expect("redirect response")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let host = "redirect-to-private.test";
        let state = sample_state_for_port(addr.port());
        cache_test_host(&state, host, addr).await;
        let server_ctx = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (_body_tx, body_rx) = mpsc::channel(1);
        let mut meta = sample_request_meta();
        meta.url = format!("http://{host}:{}/start", addr.port());
        meta.follow_redirects = Some(true);

        handle_stream(
            Arc::clone(&state),
            server_ctx,
            19,
            meta,
            body_rx,
            frame_tx.clone(),
            test_response_window(),
        )
        .await;
        let result = collect_stream_result(frame_tx, sent, writer_handle).await;
        server.abort();

        assert!(result.error.is_none());
        let response = result.response.expect("redirect response metadata");
        assert_eq!(response.status, 302);
        assert_eq!(
            response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                .map(|(_, value)| value.as_str()),
            Some(format!("http://127.0.0.1:{}/private", addr.port()).as_str())
        );
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
            test_response_window(),
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
    async fn preserves_307_after_replay_budget_without_truncating_first_request() {
        const BODY_LEN: usize = 5 * 1024 * 1024 + 1;
        const REQUEST_FRAME_BYTES: usize = 32 * 1024;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let addr = listener.local_addr().expect("addr");
        let app = Router::new()
            .route(
                "/start",
                post(|body: Bytes| async move {
                    assert_eq!(body.len(), BODY_LEN);
                    assert!(body.iter().all(|byte| *byte == b'x'));
                    Response::builder()
                        .status(StatusCode::TEMPORARY_REDIRECT)
                        .header(header::LOCATION, "/final")
                        .body(Body::empty())
                        .expect("redirect response")
                }),
            )
            .route(
                "/final",
                post(|body: Bytes| async move {
                    assert_eq!(body.len(), BODY_LEN);
                    assert!(body.iter().all(|byte| *byte == b'x'));
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("redirected"))
                        .expect("final response")
                }),
            )
            .layer(axum::extract::DefaultBodyLimit::disable());
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let host = "redirect-over-budget.test";
        let mut config = sample_config();
        config.allowed_ports.push(addr.port());
        let state = sample_state_with_config(config);
        cache_test_host(&state, host, addr).await;
        let server_ctx = sample_server(&state);
        let (frame_tx, sent, writer_handle) = spawn_test_writer();
        let (body_tx, body_rx) = mpsc::channel(4);
        let body_sender = tokio::spawn(async move {
            let body = vec![b'x'; BODY_LEN];
            let chunk_count = body.len().div_ceil(REQUEST_FRAME_BYTES);
            for (index, chunk) in body.chunks(REQUEST_FRAME_BYTES).enumerate() {
                let frame_flags = if index + 1 == chunk_count {
                    flags::END_STREAM
                } else {
                    0
                };
                body_tx
                    .send(TunnelFrame::new(
                        1,
                        MsgType::RequestBody,
                        frame_flags,
                        Bytes::copy_from_slice(chunk),
                    ))
                    .await
                    .expect("send request body chunk");
            }
        });

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
            test_response_window(),
        )
        .await;
        body_sender.await.expect("request body sender task");
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
        assert!(result.body.is_empty());
    }

    #[tokio::test]
    async fn rejects_stream_when_local_admission_gate_is_saturated() {
        let gate = Arc::new(ConcurrencyGate::new("tunnel_streams", 1));
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
            test_response_window(),
        )
        .await;

        let frame = collect_emitted_frames(frame_tx, sent, writer_handle)
            .await
            .into_iter()
            .find(|frame| frame.msg_type == MsgType::StreamError)
            .expect("overload frame");
        assert_eq!(frame.stream_id, 7);
        assert_eq!(frame.msg_type, MsgType::StreamError);
        assert_eq!(frame.payload, Bytes::from_static(b"tunnel overloaded"));
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
        let gate = Arc::new(
            RuntimeState::memory(MemoryRuntimeStateConfig::default())
                .semaphore(
                    "tunnel_streams_distributed",
                    1,
                    RuntimeSemaphoreConfig::default(),
                )
                .expect("distributed semaphore"),
        );
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
            test_response_window(),
        )
        .await;

        let frame = collect_emitted_frames(frame_tx, sent, writer_handle)
            .await
            .into_iter()
            .find(|frame| frame.msg_type == MsgType::StreamError)
            .expect("overload frame");
        assert_eq!(frame.stream_id, 9);
        assert_eq!(frame.msg_type, MsgType::StreamError);
        assert_eq!(frame.payload, Bytes::from_static(b"tunnel overloaded"));
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
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            method: "GET".to_string(),
            url: "https://example.com/ok".to_string(),
            headers: HashMap::new(),
            stream: false,
            request_timeout_ms: None,
            stream_first_byte_timeout_ms: None,
            timeout: 30,
            follow_redirects: None,
            http1_only: false,
            transport_profile: None,
        }
    }

    fn sample_state(
        stream_gate: Option<Arc<ConcurrencyGate>>,
        distributed_stream_gate: Option<Arc<RuntimeSemaphore>>,
    ) -> Arc<AppState> {
        ensure_rustls_provider();
        let config = Arc::new(sample_config());
        let dns_cache = Arc::new(DnsCache::new(Duration::from_secs(60), 128));
        let upstream_client_pool =
            upstream_client::UpstreamClientPool::new(Arc::clone(&config), Arc::clone(&dns_cache));
        Arc::new(AppState {
            config,
            dns_cache,
            upstream_client_pool,
            tunnel_tls_config: Arc::new(build_tls_config()),
            resource_monitor: Arc::new(crate::hardware::RuntimeResourceMonitor::new()),
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

    fn sample_state_with_config(config: Config) -> Arc<AppState> {
        let config = Arc::new(config);
        let dns_cache = Arc::new(DnsCache::new(Duration::from_secs(60), 128));
        let upstream_client_pool =
            upstream_client::UpstreamClientPool::new(Arc::clone(&config), Arc::clone(&dns_cache));
        Arc::new(AppState {
            config,
            dns_cache,
            upstream_client_pool,
            tunnel_tls_config: Arc::new(build_tls_config()),
            resource_monitor: Arc::new(crate::hardware::RuntimeResourceMonitor::new()),
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
            tunnel_security: config.tunnel_security,
            tunnel_encryption_key: config.tunnel_encryption_key.clone(),
            node_name: config.node_name.clone(),
            node_id: Arc::new(std::sync::RwLock::new("node-1".to_string())),
            tunnel_generation: "test-generation-1".to_string(),
            aether_client: Arc::new(AetherClient::new(
                &config,
                &config.aether_url,
                &config.management_token,
            )),
            dynamic: Arc::new(ArcSwap::from_pointee(DynamicConfig::from_config(&config))),
            active_connections: Arc::new(AtomicU64::new(0)),
            metrics: Arc::new(TunnelRequestMetrics::new()),
            tunnel_metrics: Arc::new(TunnelMetrics::new()),
        })
    }

    fn sample_config() -> Config {
        Config {
            aether_url: "https://aether.example.com".to_string(),
            management_token: "token".to_string(),
            public_ip: None,
            node_name: "tunnel-test".to_string(),
            tunnel_security: crate::config::TunnelSecurity::Off,
            tunnel_encryption_key: None,
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
            aether_outbound_proxy_url: None,
            aether_retry_max_attempts: 3,
            aether_retry_base_delay_ms: 200,
            aether_retry_max_delay_ms: 2_000,
            diagnostics_bind: None,
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
            upstream_client_pool_capacity: crate::config::DEFAULT_UPSTREAM_CLIENT_POOL_CAPACITY,
            upstream_tcp_keepalive_secs: 60,
            upstream_tcp_nodelay: true,
            upstream_proxy_url: None,
            upstream_proxy_remote_dns: false,
            legacy_redirect_replay_budget_bytes_ignored: None,
            emit_proxy_timing_header: true,
            log_level: "info".to_string(),
            log_destination: crate::config::TunnelLogDestinationArg::Stdout,
            log_dir: None,
            log_rotation: crate::config::TunnelLogRotationArg::Daily,
            log_retention_days: 7,
            log_max_files: 30,
            tunnel_reconnect_base_ms: 500,
            tunnel_reconnect_max_ms: 30_000,
            tunnel_ping_interval_ms: 15_000,
            tunnel_max_streams: Some(8),
            tunnel_profile: crate::config::TunnelProfileArg::Lite,
            tunnel_stream_initial_window_bytes:
                crate::config::DEFAULT_TUNNEL_STREAM_INITIAL_WINDOW_BYTES,
            tunnel_drain_deadline_ms: crate::config::DEFAULT_TUNNEL_DRAIN_DEADLINE_MS,
            tunnel_connect_timeout_ms: 15_000,
            tunnel_ipv4_only: false,
            tunnel_ipv6_only: false,
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

    fn test_response_window() -> Arc<StreamSendWindow> {
        Arc::new(StreamSendWindow::new(u32::MAX))
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
                    let payload = decompress_if_gzip_with_limit(
                        &frame,
                        aether_contracts::tunnel::MAX_TUNNEL_RELAY_META_LEN,
                    )
                    .expect("headers payload");
                    response = Some(
                        serde_json::from_slice(&payload).expect("response metadata should decode"),
                    );
                }
                MsgType::ResponseBody => {
                    let payload = decompress_if_gzip_with_limit(
                        &frame,
                        aether_contracts::tunnel::MAX_TUNNEL_DECOMPRESSED_PAYLOAD_BYTES,
                    )
                    .expect("body payload");
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
