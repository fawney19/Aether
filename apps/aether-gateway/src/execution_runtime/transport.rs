use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error as _;
use std::future::Future;
use std::io::Read;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::{Duration, Instant};

use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTelemetry, ProxySnapshot, ResolvedTransportProfile,
    ResponseBody, EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
    TRANSPORT_BACKEND_BROWSER_WREQ, TRANSPORT_BACKEND_REQWEST_RUSTLS,
    TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE, TRANSPORT_HTTP_MODE_HTTP1_ONLY,
};
use aether_data::repository::proxy_nodes::ProxyNodeTrafficMutation;
use aether_http::{apply_http_client_config, HttpClientConfig};
use aether_runtime::{MetricKind, MetricSample};
use axum::body::Bytes;
use base64::Engine as _;
use flate2::read::{DeflateDecoder, GzDecoder};
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming as HyperIncomingBody;
use hyper::client::conn::http2::SendRequest as HyperH2cSendRequest;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client as HyperLegacyClient;
use hyper_util::rt::{TokioExecutor, TokioIo};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::OnceCell as TokioOnceCell;

use crate::ai_serving::api::extract_provider_private_stream_error_body;
#[cfg(test)]
use crate::execution_runtime::remote_compat::execute_sync_plan_via_remote_execution_runtime;
use crate::execution_runtime::windsurf::maybe_execute_windsurf_sync;
use crate::frontdoor_loop_guard::{
    configured_gateway_frontdoor_base_url, gateway_frontdoor_self_loop_guard_error,
};
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::tunnel::{self, tunnel_protocol};
use crate::upstream_admission::UpstreamTargetAdmissionPermit;
use crate::{AppState, GatewayError};

const HUB_RELAY_CONTENT_TYPE: &str = "application/vnd.aether.tunnel-envelope";
const HUB_RELAY_ERROR_HEADER: &str = "x-aether-tunnel-error";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";
const DEFAULT_TUNNEL_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_STREAM_FIRST_BYTE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_NON_STREAM_TOTAL_TIMEOUT_MS: u64 = 300_000;
const MIN_TUNNEL_TIMEOUT_SECS: u64 = 1;
const MAX_TUNNEL_TIMEOUT_SECS: u64 = 300;
const DIRECT_REQWEST_H2_CLIENT_SHARDS_ENV: &str = "AETHER_GATEWAY_DIRECT_REQWEST_H2_CLIENT_SHARDS";
const DIRECT_REQWEST_CLIENT_SHARDS_ENV: &str = "AETHER_GATEWAY_DIRECT_REQWEST_CLIENT_SHARDS";
const DIRECT_REQWEST_H2_TARGET_STREAMS_PER_CLIENT_ENV: &str =
    "AETHER_GATEWAY_DIRECT_REQWEST_H2_TARGET_STREAMS_PER_CLIENT";
const DIRECT_REQWEST_HTTP1_TARGET_STREAMS_PER_CLIENT_ENV: &str =
    "AETHER_GATEWAY_DIRECT_REQWEST_HTTP1_TARGET_STREAMS_PER_CLIENT";
const DIRECT_REQWEST_STREAM_HTTP_MODE_ENV: &str = "AETHER_GATEWAY_DIRECT_REQWEST_STREAM_HTTP_MODE";
const DIRECT_REQWEST_CACHE_PER_ORIGIN_ENV: &str = "AETHER_GATEWAY_DIRECT_REQWEST_CACHE_PER_ORIGIN";
const DIRECT_H2C_FAST_PATH_ENV: &str = "AETHER_GATEWAY_DIRECT_H2C_FAST_PATH";
const DIRECT_H2C_CLIENT_SHARDS_ENV: &str = "AETHER_GATEWAY_DIRECT_H2C_CLIENT_SHARDS";
const DIRECT_H2C_POOL_MAX_IDLE_PER_HOST_ENV: &str =
    "AETHER_GATEWAY_DIRECT_H2C_POOL_MAX_IDLE_PER_HOST";
const DIRECT_H2C_TARGET_STREAMS_PER_CLIENT_ENV: &str =
    "AETHER_GATEWAY_DIRECT_H2C_TARGET_STREAMS_PER_CLIENT";
const DIRECT_H2C_SENDER_SELECT_WINDOW_ENV: &str = "AETHER_GATEWAY_DIRECT_H2C_SENDER_SELECT_WINDOW";
const DIRECT_H2C_PREWARM_URLS_ENV: &str = "AETHER_GATEWAY_DIRECT_H2C_PREWARM_URLS";
const DIRECT_H2C_PREWARM_READY_ENV: &str = "AETHER_GATEWAY_DIRECT_H2C_PREWARM_READY";
const DIRECT_H2C_PREWARM_CONNECT_TIMEOUT_MS_ENV: &str =
    "AETHER_GATEWAY_DIRECT_H2C_PREWARM_CONNECT_TIMEOUT_MS";
const DIRECT_REQWEST_SYNC_WARM_CLIENTS_ENV: &str =
    "AETHER_GATEWAY_DIRECT_REQWEST_SYNC_WARM_CLIENTS";
const DIRECT_REQWEST_PREWARM_SYNC_CLIENTS_ENV: &str =
    "AETHER_GATEWAY_DIRECT_REQWEST_PREWARM_SYNC_CLIENTS";
const DEFAULT_H2_TARGET_STREAMS_PER_CLIENT: usize = 8;
const DEFAULT_HTTP1_TARGET_STREAMS_PER_CLIENT: usize = 512;
const DEFAULT_DIRECT_H2C_POOL_MAX_IDLE_PER_HOST: usize = 512;
const DEFAULT_DIRECT_H2C_TARGET_STREAMS_PER_CLIENT: usize = 128;
const DEFAULT_DIRECT_H2C_SENDER_SELECT_WINDOW: usize = 4;
const DEFAULT_DIRECT_REQWEST_SYNC_WARM_CLIENTS: usize = 4;
const MAX_DIRECT_REQWEST_SYNC_WARM_CLIENTS: usize = 16;
const MAX_DIRECT_H2C_CLIENT_SHARDS: usize = 512;
const MAX_DIRECT_REQWEST_H2_CLIENT_SHARDS: usize = 2048;

type DirectHyperH2cRequestBody = Full<Bytes>;
type DirectHyperH2cClient = HyperLegacyClient<HttpConnector, DirectHyperH2cRequestBody>;
type DirectHyperH2cSender = HyperH2cSendRequest<DirectHyperH2cRequestBody>;
type DirectHyperH2cSenderCacheCell = TokioOnceCell<Arc<DirectHyperH2cSenderCacheEntry>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DirectReqwestClientCacheKey {
    upstream_origin: Option<String>,
    connect_timeout_ms: Option<u64>,
    proxy_url: Option<String>,
    follow_redirects: bool,
    http1_only: bool,
    accept_invalid_certs: bool,
    transport_profile: Option<DirectReqwestTransportProfileCacheKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DirectReqwestTransportProfileCacheKey {
    profile_id: String,
    backend: String,
    http_mode: String,
    pool_scope: String,
    header_fingerprint: Option<String>,
    extra: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DirectHyperH2cClientCacheKey {
    upstream_origin: String,
    connect_timeout_ms: Option<u64>,
    pool_max_idle_per_host: usize,
}

struct DirectReqwestClientCacheEntry {
    clients: Vec<reqwest::Client>,
    next: AtomicU64,
    target_len: usize,
    warming: bool,
}

impl DirectReqwestClientCacheEntry {
    fn new(clients: Vec<reqwest::Client>, target_len: usize, warming: bool) -> Self {
        Self {
            clients,
            next: AtomicU64::new(0),
            target_len: target_len.max(1),
            warming,
        }
    }

    fn select(&self) -> reqwest::Client {
        if self.clients.len() <= 1 {
            return self
                .clients
                .first()
                .expect("direct reqwest client cache entry should contain a client")
                .clone();
        }
        let index = self.next.fetch_add(1, Ordering::Relaxed) as usize % self.clients.len();
        self.clients[index].clone()
    }

    fn len(&self) -> usize {
        self.clients.len()
    }

    fn should_warm(&self) -> bool {
        self.clients.len() < self.target_len && !self.warming
    }
}

struct DirectHyperH2cClientCacheEntry {
    clients: Vec<DirectHyperH2cClient>,
    next: AtomicU64,
    target_len: usize,
}

struct DirectHyperH2cSenderCacheEntry {
    senders: Vec<Arc<DirectHyperH2cSenderSlot>>,
    next: AtomicU64,
    target_len: usize,
}

impl DirectHyperH2cSenderCacheEntry {
    fn new(senders: Vec<DirectHyperH2cSender>, target_len: usize) -> Self {
        Self {
            senders: senders
                .into_iter()
                .map(DirectHyperH2cSenderSlot::new)
                .collect(),
            next: AtomicU64::new(0),
            target_len: target_len.max(1),
        }
    }

    fn select(&self) -> DirectHyperH2cSenderLease {
        if self.senders.len() <= 1 {
            let slot = self
                .senders
                .first()
                .expect("direct h2c sender cache entry should contain a sender")
                .clone();
            return DirectHyperH2cSenderLease::new(slot);
        }
        let start = self.next.fetch_add(1, Ordering::Relaxed) as usize;
        let window = direct_h2c_sender_select_window()
            .min(self.senders.len())
            .max(1);
        let mut selected_index = start % self.senders.len();
        let mut selected_load = self.senders[selected_index].in_flight();
        for offset in 1..window {
            let index = start.wrapping_add(offset) % self.senders.len();
            let load = self.senders[index].in_flight();
            if load < selected_load {
                selected_index = index;
                selected_load = load;
                if load == 0 {
                    break;
                }
            }
        }
        DirectHyperH2cSenderLease::new(Arc::clone(&self.senders[selected_index]))
    }

    fn len(&self) -> usize {
        self.senders.len()
    }

    fn in_flight(&self) -> u64 {
        self.senders.iter().map(|sender| sender.in_flight()).sum()
    }

    fn max_in_flight(&self) -> u64 {
        self.senders
            .iter()
            .map(|sender| sender.max_in_flight())
            .max()
            .unwrap_or(0)
    }
}

struct DirectHyperH2cSenderSlot {
    sender: DirectHyperH2cSender,
    in_flight: AtomicU64,
    max_in_flight: AtomicU64,
}

impl DirectHyperH2cSenderSlot {
    fn new(sender: DirectHyperH2cSender) -> Arc<Self> {
        Arc::new(Self {
            sender,
            in_flight: AtomicU64::new(0),
            max_in_flight: AtomicU64::new(0),
        })
    }

    fn acquire(self: &Arc<Self>) -> DirectHyperH2cSenderLease {
        let in_flight = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_in_flight.fetch_max(in_flight, Ordering::AcqRel);
        DirectHyperH2cSenderLease {
            sender: self.sender.clone(),
            slot: Some(Arc::clone(self)),
        }
    }

    fn in_flight(&self) -> u64 {
        self.in_flight.load(Ordering::Acquire)
    }

    fn max_in_flight(&self) -> u64 {
        self.max_in_flight.load(Ordering::Acquire)
    }
}

struct DirectHyperH2cSenderLease {
    sender: DirectHyperH2cSender,
    slot: Option<Arc<DirectHyperH2cSenderSlot>>,
}

impl DirectHyperH2cSenderLease {
    fn new(slot: Arc<DirectHyperH2cSenderSlot>) -> Self {
        slot.acquire()
    }

    fn sender(&mut self) -> &mut DirectHyperH2cSender {
        &mut self.sender
    }

    fn release(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.in_flight.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Drop for DirectHyperH2cSenderLease {
    fn drop(&mut self) {
        self.release();
    }
}

impl DirectHyperH2cClientCacheEntry {
    fn new(clients: Vec<DirectHyperH2cClient>, target_len: usize) -> Self {
        Self {
            clients,
            next: AtomicU64::new(0),
            target_len: target_len.max(1),
        }
    }

    fn select(&self) -> DirectHyperH2cClient {
        if self.clients.len() <= 1 {
            return self
                .clients
                .first()
                .expect("direct h2c client cache entry should contain a client")
                .clone();
        }
        let index = self.next.fetch_add(1, Ordering::Relaxed) as usize % self.clients.len();
        self.clients[index].clone()
    }

    fn len(&self) -> usize {
        self.clients.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectReqwestStreamHttpMode {
    Http1,
    Auto,
}

static DIRECT_REQWEST_CLIENT_CACHE: LazyLock<
    StdMutex<HashMap<DirectReqwestClientCacheKey, DirectReqwestClientCacheEntry>>,
> = LazyLock::new(|| StdMutex::new(HashMap::new()));

static DIRECT_H2C_CLIENT_CACHE: LazyLock<
    StdMutex<HashMap<DirectHyperH2cClientCacheKey, DirectHyperH2cClientCacheEntry>>,
> = LazyLock::new(|| StdMutex::new(HashMap::new()));

static DIRECT_H2C_SENDER_CACHE: LazyLock<
    StdMutex<HashMap<DirectHyperH2cClientCacheKey, Arc<DirectHyperH2cSenderCacheCell>>>,
> = LazyLock::new(|| StdMutex::new(HashMap::new()));

static DIRECT_H2C_POOL_MAX_IDLE_PER_HOST: LazyLock<usize> = LazyLock::new(|| {
    env_positive_usize(DIRECT_H2C_POOL_MAX_IDLE_PER_HOST_ENV)
        .unwrap_or(DEFAULT_DIRECT_H2C_POOL_MAX_IDLE_PER_HOST)
});

static DIRECT_H2C_SENDER_SELECT_WINDOW: LazyLock<usize> = LazyLock::new(|| {
    env_positive_usize(DIRECT_H2C_SENDER_SELECT_WINDOW_ENV)
        .unwrap_or(DEFAULT_DIRECT_H2C_SENDER_SELECT_WINDOW)
        .clamp(1, MAX_DIRECT_H2C_CLIENT_SHARDS)
});

static DIRECT_REQWEST_STREAM_HTTP_MODE: LazyLock<DirectReqwestStreamHttpMode> =
    LazyLock::new(|| {
        std::env::var(DIRECT_REQWEST_STREAM_HTTP_MODE_ENV)
            .ok()
            .map(|value| parse_direct_reqwest_stream_http_mode(&value))
            .unwrap_or(DirectReqwestStreamHttpMode::Http1)
    });

#[derive(Debug, Default)]
struct DirectReqwestClientCacheMetrics {
    hits: AtomicU64,
    misses: AtomicU64,
    builds: AtomicU64,
    warm_enqueues: AtomicU64,
    warm_skipped_total: AtomicU64,
    http1_selections: AtomicU64,
    h2c_selections: AtomicU64,
    auto_selections: AtomicU64,
}

static DIRECT_REQWEST_CLIENT_CACHE_METRICS: LazyLock<DirectReqwestClientCacheMetrics> =
    LazyLock::new(DirectReqwestClientCacheMetrics::default);

#[derive(Debug, Default)]
struct DirectHyperH2cClientCacheMetrics {
    hits: AtomicU64,
    misses: AtomicU64,
    builds: AtomicU64,
}

static DIRECT_H2C_CLIENT_CACHE_METRICS: LazyLock<DirectHyperH2cClientCacheMetrics> =
    LazyLock::new(DirectHyperH2cClientCacheMetrics::default);

#[derive(Debug, Default)]
struct DirectHyperH2cSenderCacheMetrics {
    hits: AtomicU64,
    misses: AtomicU64,
    builds: AtomicU64,
    prewarm_requested: AtomicU64,
    prewarm_success: AtomicU64,
    prewarm_failed: AtomicU64,
}

static DIRECT_H2C_SENDER_CACHE_METRICS: LazyLock<DirectHyperH2cSenderCacheMetrics> =
    LazyLock::new(DirectHyperH2cSenderCacheMetrics::default);

#[derive(Debug, Clone, Default)]
pub struct DirectH2cSenderPrewarmReport {
    pub requested_urls: u64,
    pub unique_targets: u64,
    pub warmed_targets: u64,
    pub failed_targets: u64,
    pub ready_required: bool,
    pub first_error: Option<String>,
}

pub(crate) fn format_upstream_request_error(err: &reqwest::Error) -> String {
    let mut kinds = Vec::new();
    if err.is_connect() {
        kinds.push("connect");
    }
    if err.is_timeout() {
        kinds.push("timeout");
    }
    if err.is_redirect() {
        kinds.push("redirect");
    }
    if err.is_body() {
        kinds.push("body");
    }
    if err.is_decode() {
        kinds.push("decode");
    }
    if err.is_request() {
        kinds.push("request");
    }

    let mut detail = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !cause_text.is_empty() && !detail.contains(&cause_text) {
            detail.push_str(": ");
            detail.push_str(&cause_text);
        }
        source = cause.source();
    }

    if let Some(url) = err.url() {
        detail.push_str(" [url=");
        detail.push_str(url.as_str());
        detail.push(']');
    }
    if !kinds.is_empty() {
        detail.push_str(" [kind=");
        detail.push_str(&kinds.join(","));
        detail.push(']');
    }

    detail
}

pub(crate) fn format_wreq_upstream_request_error(err: &wreq::Error) -> String {
    let mut kinds = Vec::new();
    if err.is_connect() {
        kinds.push("connect");
    }
    if err.is_timeout() {
        kinds.push("timeout");
    }
    if err.is_redirect() {
        kinds.push("redirect");
    }
    if err.is_body() {
        kinds.push("body");
    }
    if err.is_decode() {
        kinds.push("decode");
    }
    if err.is_request() {
        kinds.push("request");
    }

    let mut detail = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !cause_text.is_empty() && !detail.contains(&cause_text) {
            detail.push_str(": ");
            detail.push_str(&cause_text);
        }
        source = cause.source();
    }

    if let Some(uri) = err.uri() {
        detail.push_str(" [uri=");
        detail.push_str(&uri.to_string());
        detail.push(']');
    }
    if !kinds.is_empty() {
        detail.push_str(" [kind=");
        detail.push_str(&kinds.join(","));
        detail.push(']');
    }

    detail
}

pub(crate) fn format_hyper_error_chain(err: &dyn std::error::Error) -> String {
    let mut detail = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !cause_text.is_empty() && !detail.contains(&cause_text) {
            detail.push_str(": ");
            detail.push_str(&cause_text);
        }
        source = cause.source();
    }
    detail
}

#[derive(Debug, Error)]
pub(crate) enum ExecutionRuntimeTransportError {
    #[error("stream execution is not supported for this plan")]
    StreamUnsupported,
    #[error("request body must contain json_body or body_bytes_b64")]
    RequestBodyRequired,
    #[error("request body base64 is invalid: {0}")]
    BodyDecode(base64::DecodeError),
    #[error("request content-encoding is not supported: {0}")]
    UnsupportedContentEncoding(String),
    #[error("proxy execution is not supported")]
    ProxyUnsupported,
    #[error("invalid method: {0}")]
    InvalidMethod(#[from] http::method::InvalidMethod),
    #[error("invalid upstream header name: {0}")]
    InvalidHeaderName(String),
    #[error("invalid upstream header value for {0}")]
    InvalidHeaderValue(String),
    #[error("invalid proxy configuration: {0}")]
    InvalidProxy(reqwest::Error),
    #[error("unsupported transport profile backend: {0}")]
    UnsupportedTransportProfile(String),
    #[error("failed to encode request body: {0}")]
    BodyEncode(serde_json::Error),
    #[error("failed to build HTTP client: {0}")]
    ClientBuild(reqwest::Error),
    #[error("failed to build browser impersonation HTTP client: {0}")]
    BrowserClientBuild(wreq::Error),
    #[error("browser impersonation response body failed: {0}")]
    BrowserBody(String),
    #[error("failed to execute upstream request: {0}")]
    UpstreamRequest(String),
    #[error("hub relay request failed: {0}")]
    RelayError(String),
    #[error("upstream response is not valid JSON: {0}")]
    InvalidJson(serde_json::Error),
}

#[derive(Debug, Serialize)]
struct RelayRequestMeta {
    provider_id: String,
    endpoint_id: String,
    key_id: String,
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "is_false")]
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_first_byte_timeout_ms: Option<u64>,
    timeout: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    http1_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport_profile: Option<ResolvedTransportProfile>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DirectSyncExecutionRuntime;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ExecutionTransportControls {
    follow_redirects: Option<bool>,
    http1_only: bool,
    accept_invalid_certs: bool,
}

#[derive(Debug, Clone, Copy)]
struct TunnelTimeoutMetadata {
    request_timeout_ms: Option<u64>,
    stream_first_byte_timeout_ms: Option<u64>,
    legacy_timeout_secs: u64,
}

pub(crate) enum DirectUpstreamResponse {
    Reqwest(reqwest::Response),
    HyperH2c(hyper::Response<HyperIncomingBody>),
    BrowserWreq(wreq::Response),
    LocalTunnel(tunnel::DirectRelayResponse),
}

pub(crate) struct DirectUpstreamStreamExecution {
    pub(crate) request_id: String,
    pub(crate) candidate_id: Option<String>,
    pub(crate) status_code: u16,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) provider_api_format: String,
    pub(crate) stream_summary_report_context: Value,
    pub(crate) response: DirectUpstreamResponse,
    pub(crate) started_at: Instant,
    pub(crate) stream_first_byte_timeout: Option<Duration>,
    pub(crate) upstream_target_permit: Option<UpstreamTargetAdmissionPermit>,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectSyncResponseStarted {
    pub(crate) status_code: u16,
    pub(crate) ttfb_ms: u64,
}

impl DirectSyncExecutionRuntime {
    pub(crate) const fn new() -> Self {
        Self
    }

    pub(crate) async fn execute_sync(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<ExecutionResult, ExecutionRuntimeTransportError> {
        self.execute_sync_with_response_started(plan, |_| {}).await
    }

    pub(crate) async fn execute_sync_with_response_started<F>(
        &self,
        plan: &ExecutionPlan,
        on_response_started: F,
    ) -> Result<ExecutionResult, ExecutionRuntimeTransportError>
    where
        F: FnOnce(DirectSyncResponseStarted),
    {
        let body_bytes = build_request_body(plan)?;

        let started_at = Instant::now();
        with_non_stream_total_timeout(plan, async move {
            let response = send_request_inner(plan, body_bytes, false).await?;
            let ttfb_ms = started_at.elapsed().as_millis() as u64;
            let status_code = response.status_code();
            let headers = response.headers();
            on_response_started(DirectSyncResponseStarted {
                status_code,
                ttfb_ms,
            });
            let (body_bytes, stream_ttfb_ms) =
                response.bytes_with_stream_timeout(plan, started_at).await?;
            let decoded_body_bytes = decode_response_body_bytes(&headers, &body_bytes)
                .unwrap_or_else(|| body_bytes.to_vec());
            let elapsed_ms = started_at.elapsed().as_millis() as u64;
            let upstream_bytes = body_bytes.len() as u64;

            let body = build_execution_response_body(
                &headers,
                &body_bytes,
                &decoded_body_bytes,
                plan.stream,
            )?;

            Ok(ExecutionResult {
                request_id: plan.request_id.clone(),
                candidate_id: plan.candidate_id.clone(),
                status_code,
                headers,
                body,
                telemetry: Some(ExecutionTelemetry {
                    ttfb_ms: stream_ttfb_ms.or(Some(ttfb_ms)),
                    elapsed_ms: Some(elapsed_ms),
                    upstream_bytes: Some(upstream_bytes),
                }),
                error: None,
            })
        })
        .await
    }

    pub(crate) async fn execute_stream(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<DirectUpstreamStreamExecution, ExecutionRuntimeTransportError> {
        if !plan.stream {
            return Err(ExecutionRuntimeTransportError::StreamUnsupported);
        }

        let build_body_started_at = Instant::now();
        let body_bytes = build_request_body(plan)?;
        observe_gateway_stage_ms(
            "direct_build_body",
            build_body_started_at.elapsed().as_millis() as u64,
        );

        let started_at = Instant::now();
        let response = send_request(plan, body_bytes).await?;
        observe_gateway_stage_ms(
            "direct_send_headers",
            started_at.elapsed().as_millis() as u64,
        );
        let status_code = response.status_code();
        let headers = response.headers();

        let stream_summary_report_context = build_stream_summary_report_context(plan);

        Ok(DirectUpstreamStreamExecution {
            request_id: plan.request_id.clone(),
            candidate_id: plan.candidate_id.clone(),
            status_code,
            headers,
            provider_api_format: plan.provider_api_format.clone(),
            stream_summary_report_context,
            response: response.into_direct_upstream_response(),
            started_at,
            stream_first_byte_timeout: resolve_stream_first_byte_timeout(plan),
            upstream_target_permit: None,
        })
    }
}

pub(crate) async fn execute_sync_plan(
    state: &AppState,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
) -> Result<ExecutionResult, GatewayError> {
    execute_sync_plan_with_report_context(state, trace_id, plan, None).await
}

pub(crate) async fn execute_sync_plan_with_report_context(
    state: &AppState,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) -> Result<ExecutionResult, GatewayError> {
    #[cfg(test)]
    {
        let remote_execution_runtime_base_url = state
            .execution_runtime_override_base_url()
            .unwrap_or_default();
        if !remote_execution_runtime_base_url.trim().is_empty() {
            return execute_sync_plan_via_remote_execution_runtime(
                state,
                remote_execution_runtime_base_url,
                trace_id,
                plan,
            )
            .await;
        }
    }

    if resolve_local_tunnel_node_id(state, plan.proxy.as_ref()).is_some() {
        return execute_sync_plan_via_local_tunnel(state, plan)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()));
    }

    match super::grok::maybe_execute_grok_sync(plan, report_context).await {
        Ok(Some(result)) => {
            record_manual_proxy_request_outcome(state, plan, result.status_code).await;
            return Ok(result);
        }
        Ok(None) => {}
        Err(err) => {
            record_manual_proxy_request_failure(state, plan).await;
            return Err(GatewayError::Internal(err.to_string()));
        }
    }

    let _ = trace_id;
    match maybe_execute_windsurf_sync(state, plan, None).await {
        Ok(Some(result)) => return Ok(result),
        Ok(None) => {}
        Err(err) => return Err(GatewayError::Internal(err.to_string())),
    }
    match DirectSyncExecutionRuntime::new().execute_sync(plan).await {
        Ok(result) => {
            record_manual_proxy_request_outcome(state, plan, result.status_code).await;
            Ok(result)
        }
        Err(err) => {
            record_manual_proxy_request_failure(state, plan).await;
            Err(GatewayError::Internal(err.to_string()))
        }
    }
}

pub(crate) async fn execute_stream_plan_via_local_tunnel(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<Option<DirectUpstreamStreamExecution>, ExecutionRuntimeTransportError> {
    let Some(node_id) = resolve_local_tunnel_node_id(state, plan.proxy.as_ref()) else {
        return Ok(None);
    };

    if let Some(detail) = gateway_frontdoor_self_loop_guard_error(plan.url.as_str()) {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(detail));
    }

    let body_bytes = build_request_body(plan)?;
    let transport_controls = resolve_execution_transport_controls(&plan.headers);
    let headers = build_request_headers(
        &plan.headers,
        plan.content_encoding.as_deref(),
        plan.body.body_bytes_b64.is_some(),
    )?;
    let started_at = Instant::now();
    let response = state
        .tunnel
        .open_direct_relay_stream(
            &node_id,
            build_direct_tunnel_request_meta(plan, &headers, transport_controls),
            Bytes::from(body_bytes),
        )
        .await
        .map_err(ExecutionRuntimeTransportError::RelayError)?;
    let status_code = response.status();
    let headers = collect_tunnel_response_headers(response.headers());

    Ok(Some(DirectUpstreamStreamExecution {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers,
        provider_api_format: plan.provider_api_format.clone(),
        stream_summary_report_context: build_stream_summary_report_context(plan),
        response: DirectUpstreamResponse::LocalTunnel(response),
        started_at,
        stream_first_byte_timeout: resolve_stream_first_byte_timeout(plan),
        upstream_target_permit: None,
    }))
}

fn build_stream_summary_report_context(plan: &ExecutionPlan) -> Value {
    json!({
        "provider_api_format": plan.provider_api_format,
        "client_api_format": plan.client_api_format,
        "model": plan.model_name,
    })
}

pub(crate) async fn record_manual_proxy_request_success(state: &AppState, plan: &ExecutionPlan) {
    record_manual_proxy_traffic(state, plan, 1, 0, 0, 0).await;
}

pub(crate) async fn record_manual_proxy_request_outcome(
    state: &AppState,
    plan: &ExecutionPlan,
    status_code: u16,
) {
    let failed_requests_delta = i64::from(status_code >= 400);
    record_manual_proxy_traffic(state, plan, 1, failed_requests_delta, 0, 0).await;
}

pub(crate) async fn record_manual_proxy_request_failure(state: &AppState, plan: &ExecutionPlan) {
    record_manual_proxy_traffic(state, plan, 1, 1, 0, 0).await;
}

pub(crate) async fn record_manual_proxy_stream_error(state: &AppState, plan: &ExecutionPlan) {
    record_manual_proxy_traffic(state, plan, 0, 0, 0, 1).await;
}

async fn record_manual_proxy_traffic(
    state: &AppState,
    plan: &ExecutionPlan,
    total_requests_delta: i64,
    failed_requests_delta: i64,
    dns_failures_delta: i64,
    stream_errors_delta: i64,
) {
    let Some(node_id) = manual_proxy_node_id(plan.proxy.as_ref()) else {
        return;
    };
    let mutation = ProxyNodeTrafficMutation {
        node_id: node_id.clone(),
        total_requests_delta,
        failed_requests_delta,
        dns_failures_delta,
        stream_errors_delta,
    };

    if let Err(error) = state.record_proxy_node_traffic(&mutation).await {
        tracing::warn!(
            node_id = %node_id,
            error = ?error,
            "failed to record manual proxy node traffic"
        );
    }
}

fn manual_proxy_node_id(proxy: Option<&ProxySnapshot>) -> Option<String> {
    let proxy = proxy?;
    if proxy.enabled == Some(false) || resolve_tunnel_node_id(Some(proxy)).is_some() {
        return None;
    }
    proxy
        .node_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

async fn execute_sync_plan_via_local_tunnel(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<ExecutionResult, ExecutionRuntimeTransportError> {
    with_non_stream_total_timeout(plan, execute_sync_plan_via_local_tunnel_inner(state, plan)).await
}

async fn execute_sync_plan_via_local_tunnel_inner(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<ExecutionResult, ExecutionRuntimeTransportError> {
    let node_id = resolve_local_tunnel_node_id(state, plan.proxy.as_ref()).ok_or_else(|| {
        ExecutionRuntimeTransportError::RelayError("local tunnel node unavailable".to_string())
    })?;
    if let Some(detail) = gateway_frontdoor_self_loop_guard_error(plan.url.as_str()) {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(detail));
    }

    let body_bytes = build_request_body(plan)?;
    let transport_controls = resolve_execution_transport_controls(&plan.headers);
    let headers = build_request_headers(
        &plan.headers,
        plan.content_encoding.as_deref(),
        plan.body.body_bytes_b64.is_some(),
    )?;
    let timeout_secs = resolve_relay_timeout_seconds(plan);
    tracing::info!(
        request_id = %plan.request_id,
        provider_id = %plan.provider_id,
        endpoint_id = %plan.endpoint_id,
        key_id = %plan.key_id,
        method = %plan.method,
        upstream_host = %execution_log_url_host(plan.url.as_str()),
        node_id = %node_id,
        path = "local_tunnel",
        body_bytes_len = body_bytes.len(),
        timeout_secs,
        follow_redirects = ?transport_controls.follow_redirects,
        http1_only = transport_controls.http1_only,
        "gateway execution runtime local tunnel request prepared"
    );
    let started_at = Instant::now();
    let mut response = state
        .tunnel
        .open_direct_relay_stream(
            &node_id,
            build_direct_tunnel_request_meta(plan, &headers, transport_controls),
            Bytes::from(body_bytes),
        )
        .await
        .map_err(ExecutionRuntimeTransportError::RelayError)?;
    let ttfb_ms = started_at.elapsed().as_millis() as u64;
    let status_code = response.status();
    let headers = collect_tunnel_response_headers(response.headers());
    let proxy_timing = execution_header_for_log(&headers, "x-proxy-timing").unwrap_or("-");
    let (body_bytes, stream_ttfb_ms) =
        collect_local_tunnel_response_body(response, plan, started_at).await?;
    let decoded_body_bytes =
        decode_response_body_bytes(&headers, &body_bytes).unwrap_or_else(|| body_bytes.clone());
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    let upstream_bytes = body_bytes.len() as u64;
    if status_code >= 400 {
        tracing::warn!(
            request_id = %plan.request_id,
            provider_id = %plan.provider_id,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            method = %plan.method,
            upstream_host = %execution_log_url_host(plan.url.as_str()),
            node_id = %node_id,
            path = "local_tunnel",
            status_code,
            elapsed_ms,
            upstream_bytes,
            proxy_timing,
            "gateway execution runtime local tunnel response returned error"
        );
    } else {
        tracing::info!(
            request_id = %plan.request_id,
            provider_id = %plan.provider_id,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            method = %plan.method,
            upstream_host = %execution_log_url_host(plan.url.as_str()),
            node_id = %node_id,
            path = "local_tunnel",
            status_code,
            elapsed_ms,
            upstream_bytes,
            proxy_timing,
            "gateway execution runtime local tunnel response received"
        );
    }

    let body =
        build_execution_response_body(&headers, &body_bytes, &decoded_body_bytes, plan.stream)?;

    Ok(ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers,
        body,
        telemetry: Some(ExecutionTelemetry {
            ttfb_ms: stream_ttfb_ms.or(Some(ttfb_ms)),
            elapsed_ms: Some(elapsed_ms),
            upstream_bytes: Some(upstream_bytes),
        }),
        error: None,
    })
}

async fn collect_local_tunnel_response_body(
    mut response: tunnel::DirectRelayResponse,
    plan: &ExecutionPlan,
    started_at: Instant,
) -> Result<(Vec<u8>, Option<u64>), ExecutionRuntimeTransportError> {
    let mut body_bytes = Vec::new();
    let mut first_byte_ms = None;
    let first_byte_timeout = plan
        .stream
        .then(|| resolve_stream_first_byte_timeout(plan))
        .flatten();

    loop {
        let item = if first_byte_ms.is_none() && plan.stream {
            await_stream_body_first_item(response.next_chunk(), started_at, first_byte_timeout)
                .await?
        } else {
            response.next_chunk().await
        }
        .map_err(ExecutionRuntimeTransportError::UpstreamRequest)?;
        let Some(chunk) = item else {
            break;
        };
        if plan.stream && first_byte_ms.is_none() && !chunk.is_empty() {
            first_byte_ms = Some(started_at.elapsed().as_millis() as u64);
        }
        body_bytes.extend_from_slice(&chunk);
    }

    Ok((body_bytes, first_byte_ms))
}

fn build_direct_tunnel_request_meta(
    plan: &ExecutionPlan,
    headers: &HeaderMap,
    transport_controls: ExecutionTransportControls,
) -> tunnel_protocol::RequestMeta {
    let timeout_metadata = resolve_tunnel_timeout_metadata(plan);
    tunnel_protocol::RequestMeta {
        provider_id: Some(plan.provider_id.clone()),
        endpoint_id: Some(plan.endpoint_id.clone()),
        key_id: Some(plan.key_id.clone()),
        method: plan.method.clone(),
        url: plan.url.clone(),
        headers: header_map_to_string_map(headers).into_iter().collect(),
        stream: plan.stream,
        request_timeout_ms: timeout_metadata.request_timeout_ms,
        stream_first_byte_timeout_ms: timeout_metadata.stream_first_byte_timeout_ms,
        timeout: timeout_metadata.legacy_timeout_secs,
        follow_redirects: transport_controls.follow_redirects,
        http1_only: transport_controls.http1_only,
        transport_profile: plan.transport_profile.clone(),
    }
}

pub(crate) async fn send_request(
    plan: &ExecutionPlan,
    body_bytes: Vec<u8>,
) -> Result<DirectHttpResponse, ExecutionRuntimeTransportError> {
    send_request_inner(plan, body_bytes, true).await
}

async fn send_request_inner(
    plan: &ExecutionPlan,
    body_bytes: Vec<u8>,
    apply_request_total_timeout: bool,
) -> Result<DirectHttpResponse, ExecutionRuntimeTransportError> {
    if let Some(detail) = gateway_frontdoor_self_loop_guard_error(plan.url.as_str()) {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(detail));
    }

    let prepare_started_at = Instant::now();
    let method = plan.method.parse::<reqwest::Method>()?;
    let transport_controls = resolve_execution_transport_controls(&plan.headers);
    let headers = build_request_headers(
        &plan.headers,
        plan.content_encoding.as_deref(),
        plan.body.body_bytes_b64.is_some(),
    )?;
    let total_timeout = if apply_request_total_timeout {
        resolve_non_stream_total_timeout(plan)
    } else {
        None
    };
    let stream_first_byte_timeout = resolve_stream_first_byte_timeout(plan);
    observe_gateway_stage_ms(
        "direct_request_prepare",
        prepare_started_at.elapsed().as_millis() as u64,
    );

    if transport_profile_uses_browser_wreq(plan.transport_profile.as_ref()) {
        return send_via_browser_wreq_transport(
            plan,
            method,
            headers,
            body_bytes,
            total_timeout,
            stream_first_byte_timeout,
            transport_controls,
            apply_request_total_timeout,
        )
        .await;
    }

    if let Some(node_id) = resolve_tunnel_node_id(plan.proxy.as_ref()) {
        return send_via_tunnel_relay(
            plan,
            method,
            headers,
            body_bytes,
            &node_id,
            total_timeout,
            stream_first_byte_timeout,
            transport_controls,
        )
        .await
        .map(DirectHttpResponse::Reqwest);
    }

    let direct_transport_controls =
        direct_reqwest_effective_transport_controls(plan, transport_controls);
    if direct_h2c_fast_path_applies(plan, direct_transport_controls) {
        return send_via_direct_h2c_fast_path(
            plan,
            method,
            headers,
            body_bytes,
            stream_first_byte_timeout,
        )
        .await
        .map(DirectHttpResponse::HyperH2c);
    }

    let client_select_started_at = Instant::now();
    let client = build_client(
        &plan.url,
        plan.timeouts.as_ref(),
        plan.proxy.as_ref(),
        plan.transport_profile.as_ref(),
        direct_transport_controls,
    )?;
    observe_gateway_stage_ms(
        "direct_reqwest_client_select",
        client_select_started_at.elapsed().as_millis() as u64,
    );
    let request_build_started_at = Instant::now();
    let mut request = client.request(method, &plan.url);
    request = request.headers(headers).body(body_bytes);
    if let Some(timeout) = total_timeout {
        request = request.timeout(timeout);
    }
    observe_gateway_stage_ms(
        "direct_reqwest_request_build",
        request_build_started_at.elapsed().as_millis() as u64,
    );
    send_reqwest_request(request, stream_first_byte_timeout)
        .await
        .map(DirectHttpResponse::Reqwest)
}

pub(crate) enum DirectHttpResponse {
    Reqwest(reqwest::Response),
    HyperH2c(hyper::Response<HyperIncomingBody>),
    BrowserWreq(wreq::Response),
}

impl DirectHttpResponse {
    pub(crate) fn status_code(&self) -> u16 {
        match self {
            DirectHttpResponse::Reqwest(response) => response.status().as_u16(),
            DirectHttpResponse::HyperH2c(response) => response.status().as_u16(),
            DirectHttpResponse::BrowserWreq(response) => response.status().as_u16(),
        }
    }

    pub(crate) fn headers(&self) -> BTreeMap<String, String> {
        match self {
            DirectHttpResponse::Reqwest(response) => collect_response_headers(response.headers()),
            DirectHttpResponse::HyperH2c(response) => collect_response_headers(response.headers()),
            DirectHttpResponse::BrowserWreq(response) => {
                collect_response_headers(response.headers())
            }
        }
    }

    pub(crate) async fn bytes(self) -> Result<Bytes, ExecutionRuntimeTransportError> {
        match self {
            DirectHttpResponse::Reqwest(response) => response.bytes().await.map_err(|err| {
                ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(&err))
            }),
            DirectHttpResponse::HyperH2c(response) => response
                .into_body()
                .collect()
                .await
                .map(|collected| collected.to_bytes())
                .map_err(|err| {
                    ExecutionRuntimeTransportError::UpstreamRequest(format_hyper_error_chain(&err))
                }),
            DirectHttpResponse::BrowserWreq(response) => response.bytes().await.map_err(|err| {
                ExecutionRuntimeTransportError::BrowserBody(format_wreq_upstream_request_error(
                    &err,
                ))
            }),
        }
    }

    async fn bytes_with_stream_timeout(
        self,
        plan: &ExecutionPlan,
        started_at: Instant,
    ) -> Result<(Bytes, Option<u64>), ExecutionRuntimeTransportError> {
        if !plan.stream {
            return self.bytes().await.map(|bytes| (bytes, None));
        }

        let first_byte_timeout = resolve_stream_first_byte_timeout(plan);
        match self {
            DirectHttpResponse::Reqwest(response) => {
                collect_reqwest_stream_body(response, started_at, first_byte_timeout).await
            }
            DirectHttpResponse::HyperH2c(response) => {
                collect_hyper_stream_body(response, started_at, first_byte_timeout).await
            }
            DirectHttpResponse::BrowserWreq(response) => {
                collect_wreq_stream_body(response, started_at, first_byte_timeout).await
            }
        }
    }

    fn into_direct_upstream_response(self) -> DirectUpstreamResponse {
        match self {
            DirectHttpResponse::Reqwest(response) => DirectUpstreamResponse::Reqwest(response),
            DirectHttpResponse::HyperH2c(response) => DirectUpstreamResponse::HyperH2c(response),
            DirectHttpResponse::BrowserWreq(response) => {
                DirectUpstreamResponse::BrowserWreq(response)
            }
        }
    }
}

async fn await_stream_body_first_item<T, F>(
    future: F,
    started_at: Instant,
    timeout: Option<Duration>,
) -> Result<T, ExecutionRuntimeTransportError>
where
    F: Future<Output = T>,
{
    let Some(timeout) = timeout else {
        return Ok(future.await);
    };
    let Some(remaining) = timeout.checked_sub(started_at.elapsed()) else {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            stream_first_byte_timeout_message(timeout),
        ));
    };
    if remaining.is_zero() {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(
            stream_first_byte_timeout_message(timeout),
        ));
    }
    tokio::time::timeout(remaining, future).await.map_err(|_| {
        ExecutionRuntimeTransportError::UpstreamRequest(stream_first_byte_timeout_message(timeout))
    })
}

async fn collect_reqwest_stream_body(
    response: reqwest::Response,
    started_at: Instant,
    first_byte_timeout: Option<Duration>,
) -> Result<(Bytes, Option<u64>), ExecutionRuntimeTransportError> {
    let mut stream = response.bytes_stream();
    let mut body_bytes = Vec::new();
    let mut first_byte_ms = None;

    loop {
        let item = if first_byte_ms.is_none() {
            await_stream_body_first_item(stream.next(), started_at, first_byte_timeout).await?
        } else {
            stream.next().await
        };
        let Some(item) = item else {
            break;
        };
        let chunk = item.map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(&err))
        })?;
        if first_byte_ms.is_none() && !chunk.is_empty() {
            first_byte_ms = Some(started_at.elapsed().as_millis() as u64);
        }
        body_bytes.extend_from_slice(&chunk);
    }

    Ok((Bytes::from(body_bytes), first_byte_ms))
}

async fn collect_hyper_stream_body(
    response: hyper::Response<HyperIncomingBody>,
    started_at: Instant,
    first_byte_timeout: Option<Duration>,
) -> Result<(Bytes, Option<u64>), ExecutionRuntimeTransportError> {
    let mut stream = response.into_body().into_data_stream();
    let mut body_bytes = Vec::new();
    let mut first_byte_ms = None;

    loop {
        let item = if first_byte_ms.is_none() {
            await_stream_body_first_item(stream.next(), started_at, first_byte_timeout).await?
        } else {
            stream.next().await
        };
        let Some(item) = item else {
            break;
        };
        let chunk = item.map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_hyper_error_chain(&err))
        })?;
        if first_byte_ms.is_none() && !chunk.is_empty() {
            first_byte_ms = Some(started_at.elapsed().as_millis() as u64);
        }
        body_bytes.extend_from_slice(&chunk);
    }

    Ok((Bytes::from(body_bytes), first_byte_ms))
}

async fn collect_wreq_stream_body(
    response: wreq::Response,
    started_at: Instant,
    first_byte_timeout: Option<Duration>,
) -> Result<(Bytes, Option<u64>), ExecutionRuntimeTransportError> {
    let mut stream = response.bytes_stream();
    let mut body_bytes = Vec::new();
    let mut first_byte_ms = None;

    loop {
        let item = if first_byte_ms.is_none() {
            await_stream_body_first_item(stream.next(), started_at, first_byte_timeout).await?
        } else {
            stream.next().await
        };
        let Some(item) = item else {
            break;
        };
        let chunk = item.map_err(|err| {
            ExecutionRuntimeTransportError::BrowserBody(format_wreq_upstream_request_error(&err))
        })?;
        if first_byte_ms.is_none() && !chunk.is_empty() {
            first_byte_ms = Some(started_at.elapsed().as_millis() as u64);
        }
        body_bytes.extend_from_slice(&chunk);
    }

    Ok((Bytes::from(body_bytes), first_byte_ms))
}

fn direct_h2c_fast_path_applies(
    plan: &ExecutionPlan,
    transport_controls: ExecutionTransportControls,
) -> bool {
    if !direct_h2c_fast_path_enabled()
        || !plan.stream
        || transport_controls.http1_only
        || transport_controls.accept_invalid_certs
        || plan.proxy.is_some()
        || !transport_profile_h2c_prior_knowledge(plan.transport_profile.as_ref())
    {
        return false;
    }

    reqwest::Url::parse(plan.url.as_str())
        .ok()
        .is_some_and(|url| url.scheme() == "http")
}

fn direct_h2c_fast_path_enabled() -> bool {
    std::env::var(DIRECT_H2C_FAST_PATH_ENV)
        .ok()
        .is_some_and(|value| matches_truthy_env_value(value.trim()))
}

pub(crate) async fn prewarm_direct_h2c_sender_cache_from_env(
) -> Result<Option<DirectH2cSenderPrewarmReport>, ExecutionRuntimeTransportError> {
    let urls = direct_h2c_prewarm_urls_from_env();
    if urls.is_empty() {
        return Ok(None);
    }

    let ready_required = direct_h2c_prewarm_ready_required();
    let report = prewarm_direct_h2c_sender_cache_urls(urls, ready_required).await;
    if ready_required && report.failed_targets > 0 {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "direct h2c sender prewarm failed for {}/{} targets{}",
            report.failed_targets,
            report.unique_targets,
            report
                .first_error
                .as_deref()
                .map(|err| format!(": {err}"))
                .unwrap_or_default()
        )));
    }
    Ok(Some(report))
}

async fn prewarm_direct_h2c_sender_cache_urls(
    urls: Vec<String>,
    ready_required: bool,
) -> DirectH2cSenderPrewarmReport {
    let started_at = Instant::now();
    let requested_urls = urls.len() as u64;
    DIRECT_H2C_SENDER_CACHE_METRICS
        .prewarm_requested
        .fetch_add(requested_urls, Ordering::Relaxed);

    let connect_timeout_ms =
        env_positive_usize(DIRECT_H2C_PREWARM_CONNECT_TIMEOUT_MS_ENV).map(|value| value as u64);
    let timeouts = connect_timeout_ms.map(|connect_ms| aether_contracts::ExecutionTimeouts {
        connect_ms: Some(connect_ms),
        ..Default::default()
    });
    let (keys, parse_failures, mut first_error) =
        direct_h2c_sender_prewarm_cache_keys(&urls, timeouts.as_ref());
    let unique_targets = keys.len() as u64;
    if parse_failures > 0 {
        DIRECT_H2C_SENDER_CACHE_METRICS
            .prewarm_failed
            .fetch_add(parse_failures, Ordering::Relaxed);
    }

    let mut warmed_targets = 0;
    let mut failed_targets = parse_failures;
    let mut pending = FuturesUnordered::new();
    for key in keys {
        pending.push(prewarm_direct_h2c_sender_cache_key(key));
    }

    while let Some(result) = pending.next().await {
        match result {
            Ok(()) => {
                warmed_targets += 1;
                DIRECT_H2C_SENDER_CACHE_METRICS
                    .prewarm_success
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(err) => {
                failed_targets += 1;
                DIRECT_H2C_SENDER_CACHE_METRICS
                    .prewarm_failed
                    .fetch_add(1, Ordering::Relaxed);
                if first_error.is_none() {
                    first_error = Some(err.to_string());
                }
            }
        }
    }

    observe_gateway_stage_ms(
        "direct_h2c_sender_cache_prewarm",
        started_at.elapsed().as_millis() as u64,
    );
    DirectH2cSenderPrewarmReport {
        requested_urls,
        unique_targets,
        warmed_targets,
        failed_targets,
        ready_required,
        first_error,
    }
}

async fn prewarm_direct_h2c_sender_cache_key(
    cache_key: DirectHyperH2cClientCacheKey,
) -> Result<(), ExecutionRuntimeTransportError> {
    let cell = direct_h2c_sender_cache_cell(&cache_key);
    cell.get_or_try_init(|| async {
        let target_len = direct_h2c_client_shard_count();
        build_direct_h2c_sender_cache_entry_from_cache_key(&cache_key, target_len)
            .await
            .map(Arc::new)
    })
    .await?;
    Ok(())
}

fn direct_h2c_sender_prewarm_cache_keys(
    urls: &[String],
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
) -> (Vec<DirectHyperH2cClientCacheKey>, u64, Option<String>) {
    let mut seen = HashSet::new();
    let mut keys = Vec::new();
    let mut failed = 0;
    let mut first_error = None;
    for url in urls {
        match direct_h2c_client_cache_key(url, timeouts) {
            Ok(key) => {
                if seen.insert(key.clone()) {
                    keys.push(key);
                }
            }
            Err(err) => {
                failed += 1;
                if first_error.is_none() {
                    first_error = Some(err.to_string());
                }
            }
        }
    }
    (keys, failed, first_error)
}

fn direct_h2c_prewarm_urls_from_env() -> Vec<String> {
    std::env::var(DIRECT_H2C_PREWARM_URLS_ENV)
        .ok()
        .map(|value| {
            value
                .split([',', ';', '\n', '\t', ' '])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn direct_h2c_prewarm_ready_required() -> bool {
    std::env::var(DIRECT_H2C_PREWARM_READY_ENV)
        .ok()
        .is_some_and(|value| matches_truthy_env_value(value.trim()))
}

async fn cached_direct_h2c_sender(
    request_url: &str,
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
) -> Result<DirectHyperH2cSenderLease, ExecutionRuntimeTransportError> {
    let cache_key = direct_h2c_client_cache_key(request_url, timeouts)?;
    let cell = direct_h2c_sender_cache_cell(&cache_key);
    let entry = cell
        .get_or_try_init(|| async {
            let target_len = direct_h2c_client_shard_count();
            build_direct_h2c_sender_cache_entry_from_cache_key(&cache_key, target_len)
                .await
                .map(Arc::new)
        })
        .await?;
    Ok(entry.select())
}

fn direct_h2c_sender_cache_cell(
    cache_key: &DirectHyperH2cClientCacheKey,
) -> Arc<DirectHyperH2cSenderCacheCell> {
    let cache_lock_started_at = Instant::now();
    if let Ok(mut cache) = DIRECT_H2C_SENDER_CACHE.lock() {
        observe_gateway_stage_ms(
            "direct_reqwest_client_cache_lock",
            cache_lock_started_at.elapsed().as_millis() as u64,
        );
        if let Some(cell) = cache.get(cache_key) {
            DIRECT_H2C_SENDER_CACHE_METRICS
                .hits
                .fetch_add(1, Ordering::Relaxed);
            Arc::clone(cell)
        } else {
            DIRECT_H2C_SENDER_CACHE_METRICS
                .misses
                .fetch_add(1, Ordering::Relaxed);
            let cell = Arc::new(TokioOnceCell::new());
            cache.insert(cache_key.clone(), Arc::clone(&cell));
            cell
        }
    } else {
        observe_gateway_stage_ms(
            "direct_reqwest_client_cache_lock",
            cache_lock_started_at.elapsed().as_millis() as u64,
        );
        DIRECT_H2C_SENDER_CACHE_METRICS
            .misses
            .fetch_add(1, Ordering::Relaxed);
        Arc::new(TokioOnceCell::new())
    }
}

fn direct_h2c_client_cache_key(
    request_url: &str,
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
) -> Result<DirectHyperH2cClientCacheKey, ExecutionRuntimeTransportError> {
    let upstream_origin = direct_reqwest_upstream_origin(request_url).ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "invalid h2c upstream origin: {request_url}"
        ))
    })?;
    Ok(DirectHyperH2cClientCacheKey {
        upstream_origin,
        connect_timeout_ms: timeouts.and_then(|timeouts| timeouts.connect_ms),
        pool_max_idle_per_host: direct_h2c_pool_max_idle_per_host(),
    })
}

async fn build_direct_h2c_sender_cache_entry_from_cache_key(
    cache_key: &DirectHyperH2cClientCacheKey,
    target_len: usize,
) -> Result<DirectHyperH2cSenderCacheEntry, ExecutionRuntimeTransportError> {
    let mut pending = FuturesUnordered::new();
    for _ in 0..target_len {
        pending.push(connect_direct_h2c_sender(cache_key));
    }

    let mut senders = Vec::with_capacity(target_len);
    while let Some(sender) = pending.next().await {
        senders.push(sender?);
        DIRECT_H2C_SENDER_CACHE_METRICS
            .builds
            .fetch_add(1, Ordering::Relaxed);
    }
    Ok(DirectHyperH2cSenderCacheEntry::new(senders, target_len))
}

async fn connect_direct_h2c_sender(
    cache_key: &DirectHyperH2cClientCacheKey,
) -> Result<DirectHyperH2cSender, ExecutionRuntimeTransportError> {
    let upstream = reqwest::Url::parse(&cache_key.upstream_origin).map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "invalid h2c upstream origin {}: {err}",
            cache_key.upstream_origin
        ))
    })?;
    let host = upstream.host_str().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "missing h2c upstream host: {}",
            cache_key.upstream_origin
        ))
    })?;
    let port = upstream.port_or_known_default().ok_or_else(|| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "missing h2c upstream port: {}",
            cache_key.upstream_origin
        ))
    })?;
    let connect = TcpStream::connect((host, port));
    let stream = if let Some(timeout_ms) = cache_key.connect_timeout_ms {
        let timeout = Duration::from_millis(timeout_ms);
        tokio::time::timeout(timeout, connect)
            .await
            .map_err(|_| {
                ExecutionRuntimeTransportError::UpstreamRequest(stream_first_byte_timeout_message(
                    timeout,
                ))
            })?
            .map_err(|err| {
                ExecutionRuntimeTransportError::UpstreamRequest(format!(
                    "failed to connect h2c upstream {}: {err}",
                    cache_key.upstream_origin
                ))
            })?
    } else {
        connect.await.map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format!(
                "failed to connect h2c upstream {}: {err}",
                cache_key.upstream_origin
            ))
        })?
    };
    stream.set_nodelay(true).map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!(
            "failed to configure h2c upstream socket {}: {err}",
            cache_key.upstream_origin
        ))
    })?;
    let io = TokioIo::new(stream);
    let mut builder = hyper::client::conn::http2::Builder::new(TokioExecutor::new());
    builder.adaptive_window(true);
    let (sender, connection) = builder.handshake(io).await.map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format_hyper_error_chain(&err))
    })?;
    tokio::spawn(async move {
        if let Err(err) = connection.await {
            tracing::debug!(
                error = %format_hyper_error_chain(&err),
                "direct h2c sender connection closed"
            );
        }
    });
    Ok(sender)
}

fn cached_direct_h2c_client(
    request_url: &str,
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
) -> Result<DirectHyperH2cClient, ExecutionRuntimeTransportError> {
    let cache_key = direct_h2c_client_cache_key(request_url, timeouts)?;

    let cache_lock_started_at = Instant::now();
    if let Ok(mut cache) = DIRECT_H2C_CLIENT_CACHE.lock() {
        observe_gateway_stage_ms(
            "direct_reqwest_client_cache_lock",
            cache_lock_started_at.elapsed().as_millis() as u64,
        );
        if let Some(entry) = cache.get(&cache_key) {
            DIRECT_H2C_CLIENT_CACHE_METRICS
                .hits
                .fetch_add(1, Ordering::Relaxed);
            return Ok(entry.select());
        }

        DIRECT_H2C_CLIENT_CACHE_METRICS
            .misses
            .fetch_add(1, Ordering::Relaxed);
        let target_len = direct_h2c_client_shard_count();
        let mut clients = Vec::with_capacity(target_len);
        for _ in 0..target_len {
            clients.push(build_direct_h2c_client_from_cache_key(&cache_key));
            DIRECT_H2C_CLIENT_CACHE_METRICS
                .builds
                .fetch_add(1, Ordering::Relaxed);
        }
        let entry = DirectHyperH2cClientCacheEntry::new(clients, target_len);
        let client = entry.select();
        cache.insert(cache_key, entry);
        return Ok(client);
    }

    observe_gateway_stage_ms(
        "direct_reqwest_client_cache_lock",
        cache_lock_started_at.elapsed().as_millis() as u64,
    );
    DIRECT_H2C_CLIENT_CACHE_METRICS
        .misses
        .fetch_add(1, Ordering::Relaxed);
    DIRECT_H2C_CLIENT_CACHE_METRICS
        .builds
        .fetch_add(1, Ordering::Relaxed);
    Ok(build_direct_h2c_client_from_cache_key(&cache_key))
}

fn build_direct_h2c_client_from_cache_key(
    cache_key: &DirectHyperH2cClientCacheKey,
) -> DirectHyperH2cClient {
    let mut connector = HttpConnector::new();
    connector.enforce_http(true);
    connector.set_nodelay(true);
    connector.set_connect_timeout(cache_key.connect_timeout_ms.map(Duration::from_millis));

    let mut builder = HyperLegacyClient::builder(TokioExecutor::new());
    builder.http2_only(true);
    builder.http2_adaptive_window(true);
    builder.pool_max_idle_per_host(cache_key.pool_max_idle_per_host);
    builder.build(connector)
}

fn direct_h2c_pool_max_idle_per_host() -> usize {
    *DIRECT_H2C_POOL_MAX_IDLE_PER_HOST
}

fn direct_h2c_client_shard_count() -> usize {
    if let Some(shards) = env_positive_usize(DIRECT_H2C_CLIENT_SHARDS_ENV) {
        return shards.clamp(1, MAX_DIRECT_H2C_CLIENT_SHARDS);
    }
    let target_gate_limit = crate::state::upstream_target_gate_limit_from_env()
        .unwrap_or_else(crate::state::upstream_target_gate_auto_limit);
    let streams_per_client = env_positive_usize(DIRECT_H2C_TARGET_STREAMS_PER_CLIENT_ENV)
        .unwrap_or(DEFAULT_DIRECT_H2C_TARGET_STREAMS_PER_CLIENT)
        .max(1);
    target_gate_limit
        .max(1)
        .div_ceil(streams_per_client)
        .clamp(1, MAX_DIRECT_H2C_CLIENT_SHARDS)
}

fn direct_h2c_sender_select_window() -> usize {
    *DIRECT_H2C_SENDER_SELECT_WINDOW
}

async fn send_via_direct_h2c_fast_path(
    plan: &ExecutionPlan,
    method: reqwest::Method,
    headers: HeaderMap,
    body_bytes: Vec<u8>,
    stream_first_byte_timeout: Option<Duration>,
) -> Result<hyper::Response<HyperIncomingBody>, ExecutionRuntimeTransportError> {
    let client_select_started_at = Instant::now();
    let sender = cached_direct_h2c_sender(&plan.url, plan.timeouts.as_ref()).await?;
    observe_gateway_stage_ms(
        "direct_h2c_client_select",
        client_select_started_at.elapsed().as_millis() as u64,
    );

    let request_build_started_at = Instant::now();
    let uri = plan.url.parse::<hyper::Uri>().map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format!("invalid h2c upstream uri: {err}"))
    })?;
    let authority = uri
        .authority()
        .map(|authority| authority.as_str().to_string());
    let mut builder = hyper::Request::builder().method(method.as_str()).uri(uri);
    {
        let target_headers = builder.headers_mut().ok_or_else(|| {
            ExecutionRuntimeTransportError::UpstreamRequest(
                "failed to prepare h2c request headers".to_string(),
            )
        })?;
        *target_headers = headers;
        if !target_headers.contains_key(reqwest::header::HOST) {
            if let Some(authority) = authority.as_deref() {
                let value = HeaderValue::from_str(authority).map_err(|_| {
                    ExecutionRuntimeTransportError::InvalidHeaderValue("host".to_string())
                })?;
                target_headers.insert(reqwest::header::HOST, value);
            }
        }
    }
    let request = builder
        .body(Full::new(Bytes::from(body_bytes)))
        .map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format!(
                "failed to build h2c request: {err}"
            ))
        })?;
    observe_gateway_stage_ms(
        "direct_h2c_request_build",
        request_build_started_at.elapsed().as_millis() as u64,
    );

    send_hyper_h2c_request(sender, request, stream_first_byte_timeout).await
}

async fn send_hyper_h2c_request(
    mut sender: DirectHyperH2cSenderLease,
    request: hyper::Request<DirectHyperH2cRequestBody>,
    stream_first_byte_timeout: Option<Duration>,
) -> Result<hyper::Response<HyperIncomingBody>, ExecutionRuntimeTransportError> {
    let started_at = Instant::now();
    let deadline = stream_first_byte_timeout.map(|timeout| (timeout, Instant::now() + timeout));

    let ready_started_at = Instant::now();
    let ready_result = if let Some((timeout, deadline)) = deadline {
        match direct_h2c_remaining_timeout(deadline) {
            Some(remaining) => match tokio::time::timeout(remaining, sender.sender().ready()).await
            {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                    format_hyper_error_chain(&err),
                )),
                Err(_) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                    stream_first_byte_timeout_message(timeout),
                )),
            },
            None => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                stream_first_byte_timeout_message(timeout),
            )),
        }
    } else {
        sender.sender().ready().await.map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_hyper_error_chain(&err))
        })
    };
    observe_gateway_stage_ms(
        "direct_h2c_sender_ready_wait",
        ready_started_at.elapsed().as_millis() as u64,
    );
    ready_result?;

    let headers_started_at = Instant::now();
    let dispatch_started_at = Instant::now();
    let response_future = sender.sender().send_request(request);
    observe_gateway_stage_ms(
        "direct_h2c_request_dispatch",
        dispatch_started_at.elapsed().as_millis() as u64,
    );

    let response_headers_started_at = Instant::now();
    let response_result = if let Some((timeout, deadline)) = deadline {
        match direct_h2c_remaining_timeout(deadline) {
            Some(remaining) => match tokio::time::timeout(remaining, response_future).await {
                Ok(Ok(response)) => Ok(response),
                Ok(Err(err)) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                    format_hyper_error_chain(&err),
                )),
                Err(_) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                    stream_first_byte_timeout_message(timeout),
                )),
            },
            None => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                stream_first_byte_timeout_message(timeout),
            )),
        }
    } else {
        response_future.await.map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_hyper_error_chain(&err))
        })
    };
    observe_gateway_stage_ms(
        "direct_h2c_response_headers_wait",
        response_headers_started_at.elapsed().as_millis() as u64,
    );
    observe_gateway_stage_ms(
        "direct_h2c_request_headers_wait",
        headers_started_at.elapsed().as_millis() as u64,
    );
    let response = response_result?;
    sender.release();
    observe_gateway_stage_ms(
        "direct_h2c_request_send",
        started_at.elapsed().as_millis() as u64,
    );
    Ok(response)
}

fn direct_h2c_remaining_timeout(deadline: Instant) -> Option<Duration> {
    deadline.checked_duration_since(Instant::now())
}

async fn send_via_browser_wreq_transport(
    plan: &ExecutionPlan,
    method: reqwest::Method,
    headers: HeaderMap,
    body_bytes: Vec<u8>,
    total_timeout: Option<Duration>,
    stream_first_byte_timeout: Option<Duration>,
    transport_controls: ExecutionTransportControls,
    apply_request_total_timeout: bool,
) -> Result<DirectHttpResponse, ExecutionRuntimeTransportError> {
    let profile = plan.transport_profile.as_ref().ok_or_else(|| {
        ExecutionRuntimeTransportError::UnsupportedTransportProfile(String::new())
    })?;
    let client = build_browser_wreq_client(
        plan.timeouts.as_ref(),
        plan.proxy.as_ref(),
        profile,
        transport_controls,
        apply_request_total_timeout && !plan.stream,
    )?;
    let method = wreq::Method::from_bytes(method.as_str().as_bytes())
        .map_err(ExecutionRuntimeTransportError::InvalidMethod)?;
    let mut request = client
        .request(method, plan.url.as_str())
        .headers(headers)
        .body(body_bytes);
    if let Some(timeout) = total_timeout {
        request = request.timeout(timeout);
    }
    send_wreq_request(request, stream_first_byte_timeout)
        .await
        .map(DirectHttpResponse::BrowserWreq)
}

async fn send_via_tunnel_relay(
    plan: &ExecutionPlan,
    method: reqwest::Method,
    headers: HeaderMap,
    body_bytes: Vec<u8>,
    node_id: &str,
    total_timeout: Option<Duration>,
    stream_first_byte_timeout: Option<Duration>,
    transport_controls: ExecutionTransportControls,
) -> Result<reqwest::Response, ExecutionRuntimeTransportError> {
    let client = build_relay_client(plan.timeouts.as_ref())?;
    let relay_url = build_relay_url(plan.proxy.as_ref(), node_id);
    let timeout_metadata = resolve_tunnel_timeout_metadata(plan);
    let timeout_secs = timeout_metadata.legacy_timeout_secs;
    let envelope = build_relay_envelope(
        RelayRequestMeta {
            provider_id: plan.provider_id.clone(),
            endpoint_id: plan.endpoint_id.clone(),
            key_id: plan.key_id.clone(),
            method: method.as_str().to_string(),
            url: plan.url.clone(),
            headers: header_map_to_string_map(&headers),
            stream: plan.stream,
            request_timeout_ms: timeout_metadata.request_timeout_ms,
            stream_first_byte_timeout_ms: timeout_metadata.stream_first_byte_timeout_ms,
            timeout: timeout_secs,
            follow_redirects: transport_controls.follow_redirects,
            http1_only: transport_controls.http1_only,
            transport_profile: plan.transport_profile.clone(),
        },
        &body_bytes,
    )?;
    tracing::info!(
        request_id = %plan.request_id,
        provider_id = %plan.provider_id,
        endpoint_id = %plan.endpoint_id,
        key_id = %plan.key_id,
        method = %method,
        upstream_host = %execution_log_url_host(plan.url.as_str()),
        relay_host = %execution_log_url_host(relay_url.as_str()),
        node_id,
        path = "tunnel_relay",
        body_bytes_len = body_bytes.len(),
        envelope_bytes_len = envelope.len(),
        timeout_secs,
        follow_redirects = ?transport_controls.follow_redirects,
        http1_only = transport_controls.http1_only,
        "gateway execution runtime tunnel relay request prepared"
    );

    let mut request = client
        .request(reqwest::Method::POST, relay_url)
        .header(reqwest::header::CONTENT_TYPE, HUB_RELAY_CONTENT_TYPE)
        .body(envelope);
    if !plan.stream {
        if let Some(timeout) = total_timeout {
            request = request.timeout(timeout);
        }
    }

    let first_byte_timeout = if plan.stream {
        stream_first_byte_timeout.or_else(|| resolve_tunnel_first_byte_timeout(plan))
    } else {
        None
    };

    let started_at = Instant::now();
    let response = send_relay_request(request, first_byte_timeout)
        .await
        .map_err(ExecutionRuntimeTransportError::RelayError)?;
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    let status_code = response.status().as_u16();
    let proxy_timing = response
        .headers()
        .get("x-proxy-timing")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("-");
    if status_code >= 400 {
        tracing::warn!(
            request_id = %plan.request_id,
            provider_id = %plan.provider_id,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            method = %method,
            upstream_host = %execution_log_url_host(plan.url.as_str()),
            node_id,
            path = "tunnel_relay",
            status_code,
            elapsed_ms,
            proxy_timing,
            "gateway execution runtime tunnel relay response returned error"
        );
    } else {
        tracing::info!(
            request_id = %plan.request_id,
            provider_id = %plan.provider_id,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            method = %method,
            upstream_host = %execution_log_url_host(plan.url.as_str()),
            node_id,
            path = "tunnel_relay",
            status_code,
            elapsed_ms,
            proxy_timing,
            "gateway execution runtime tunnel relay response received"
        );
    }

    if let Some(kind) = response
        .headers()
        .get(HUB_RELAY_ERROR_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    {
        tracing::warn!(
            request_id = %plan.request_id,
            provider_id = %plan.provider_id,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            method = %method,
            upstream_host = %execution_log_url_host(plan.url.as_str()),
            node_id,
            path = "tunnel_relay",
            status_code,
            elapsed_ms,
            error_kind = %kind,
            "gateway execution runtime tunnel relay returned relay error"
        );
        let message = response
            .text()
            .await
            .unwrap_or_else(|_| format!("hub relay error: {kind}"));
        return Err(ExecutionRuntimeTransportError::RelayError(message));
    }

    Ok(response)
}

async fn send_relay_request(
    request: reqwest::RequestBuilder,
    first_byte_timeout: Option<Duration>,
) -> Result<reqwest::Response, String> {
    if let Some(timeout) = first_byte_timeout {
        return match tokio::time::timeout(timeout, request.send()).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err("tunnel relay first byte timeout".to_string()),
        };
    }

    request.send().await.map_err(|err| err.to_string())
}

pub(crate) fn build_request_body(
    plan: &ExecutionPlan,
) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
    let mut body_bytes = if let Some(json_body) = plan.body.json_body.clone() {
        serde_json::to_vec(&json_body).map_err(ExecutionRuntimeTransportError::BodyEncode)?
    } else if let Some(body_b64) = plan.body.body_bytes_b64.as_deref() {
        base64::engine::general_purpose::STANDARD
            .decode(body_b64)
            .map_err(ExecutionRuntimeTransportError::BodyDecode)?
    } else {
        Vec::new()
    };

    if should_gzip_request_body(plan) && plan.body.json_body.is_some() {
        body_bytes = gzip_bytes(&body_bytes)?;
    }

    Ok(body_bytes)
}

fn should_gzip_request_body(plan: &ExecutionPlan) -> bool {
    matches!(
        normalize_content_encoding(plan.content_encoding.as_deref()).as_deref(),
        Some("gzip")
    )
}

fn normalize_content_encoding(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn gzip_bytes(body_bytes: &[u8]) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(body_bytes)
        .map_err(|err| ExecutionRuntimeTransportError::RelayError(err.to_string()))?;
    encoder
        .finish()
        .map_err(|err| ExecutionRuntimeTransportError::RelayError(err.to_string()))
}

fn build_relay_client(
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
) -> Result<reqwest::Client, ExecutionRuntimeTransportError> {
    let builder = apply_http_client_config(
        reqwest::Client::builder(),
        &HttpClientConfig {
            connect_timeout_ms: timeouts.and_then(|timeouts| timeouts.connect_ms),
            use_rustls_tls: false,
            ..HttpClientConfig::default()
        },
    );
    builder
        .build()
        .map_err(ExecutionRuntimeTransportError::ClientBuild)
}

fn build_relay_envelope(
    meta: RelayRequestMeta,
    body_bytes: &[u8],
) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
    let meta_bytes =
        serde_json::to_vec(&meta).map_err(ExecutionRuntimeTransportError::BodyEncode)?;
    let mut envelope = Vec::with_capacity(4 + meta_bytes.len() + body_bytes.len());
    envelope.extend_from_slice(&(meta_bytes.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_bytes);
    envelope.extend_from_slice(body_bytes);
    Ok(envelope)
}

fn build_relay_url(proxy: Option<&ProxySnapshot>, node_id: &str) -> String {
    let base_url = proxy
        .and_then(resolve_tunnel_base_url_from_proxy)
        .or_else(|| std::env::var("AETHER_TUNNEL_BASE_URL").ok())
        .unwrap_or_else(configured_gateway_frontdoor_base_url);
    format!(
        "{}{}/{}",
        base_url.trim_end_matches('/'),
        TUNNEL_RELAY_PATH_PREFIX,
        node_id
    )
}

fn resolve_tunnel_base_url_from_proxy(proxy: &ProxySnapshot) -> Option<String> {
    let extra = proxy.extra.as_ref()?;
    let value = extra.get("tunnel_base_url")?.as_str()?.trim();
    if !value.is_empty() {
        return Some(value.to_string());
    }
    None
}

fn resolve_relay_timeout_seconds(plan: &ExecutionPlan) -> u64 {
    resolve_tunnel_timeout_metadata(plan).legacy_timeout_secs
}

fn resolve_tunnel_first_byte_timeout(plan: &ExecutionPlan) -> Option<Duration> {
    plan.stream.then(|| {
        resolve_stream_first_byte_timeout(plan)
            .unwrap_or_else(|| Duration::from_millis(DEFAULT_TUNNEL_TIMEOUT_MS))
    })
}

fn resolve_non_stream_total_timeout(plan: &ExecutionPlan) -> Option<Duration> {
    if plan.stream {
        return None;
    }
    let timeout_ms = plan
        .timeouts
        .as_ref()
        .and_then(|timeouts| timeouts.total_ms)
        .unwrap_or(DEFAULT_NON_STREAM_TOTAL_TIMEOUT_MS);
    Some(Duration::from_millis(timeout_ms.max(1)))
}

pub(crate) fn resolve_stream_first_byte_timeout(plan: &ExecutionPlan) -> Option<Duration> {
    if !plan.stream {
        return None;
    }
    let timeout_ms = plan
        .timeouts
        .as_ref()
        .and_then(|timeouts| timeouts.first_byte_ms)
        .unwrap_or(DEFAULT_STREAM_FIRST_BYTE_TIMEOUT_MS);
    Some(Duration::from_millis(timeout_ms.max(1)))
}

pub(crate) async fn with_non_stream_total_timeout<T, F>(
    plan: &ExecutionPlan,
    future: F,
) -> Result<T, ExecutionRuntimeTransportError>
where
    F: Future<Output = Result<T, ExecutionRuntimeTransportError>>,
{
    let Some(timeout) = resolve_non_stream_total_timeout(plan) else {
        return future.await;
    };

    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result,
        Err(_) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
            non_stream_total_timeout_message(timeout),
        )),
    }
}

async fn send_reqwest_request(
    request: reqwest::RequestBuilder,
    stream_first_byte_timeout: Option<Duration>,
) -> Result<reqwest::Response, ExecutionRuntimeTransportError> {
    let started_at = Instant::now();
    if let Some(timeout) = stream_first_byte_timeout {
        return match tokio::time::timeout(timeout, request.send()).await {
            Ok(Ok(response)) => {
                observe_gateway_stage_ms(
                    "direct_reqwest_request_send",
                    started_at.elapsed().as_millis() as u64,
                );
                Ok(response)
            }
            Ok(Err(error)) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                format_upstream_request_error(&error),
            )),
            Err(_) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                stream_first_byte_timeout_message(timeout),
            )),
        };
    }

    let response = request.send().await.map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(&err))
    })?;
    observe_gateway_stage_ms(
        "direct_reqwest_request_send",
        started_at.elapsed().as_millis() as u64,
    );
    Ok(response)
}

async fn send_wreq_request(
    request: wreq::RequestBuilder,
    stream_first_byte_timeout: Option<Duration>,
) -> Result<wreq::Response, ExecutionRuntimeTransportError> {
    if let Some(timeout) = stream_first_byte_timeout {
        return match tokio::time::timeout(timeout, request.send()).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                format_wreq_upstream_request_error(&error),
            )),
            Err(_) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                stream_first_byte_timeout_message(timeout),
            )),
        };
    }

    request.send().await.map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format_wreq_upstream_request_error(&err))
    })
}

fn non_stream_total_timeout_message(timeout: Duration) -> String {
    format!(
        "provider non-stream request total timeout after {} ms",
        timeout.as_millis()
    )
}

pub(crate) fn stream_first_byte_timeout_message(timeout: Duration) -> String {
    format!(
        "provider stream first byte timeout after {} ms",
        timeout.as_millis()
    )
}

fn resolve_tunnel_timeout_metadata(plan: &ExecutionPlan) -> TunnelTimeoutMetadata {
    let request_timeout_ms = if plan.stream {
        None
    } else {
        resolve_non_stream_total_timeout(plan)
            .map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX))
    };
    let stream_first_byte_timeout_ms = if plan.stream {
        resolve_stream_first_byte_timeout(plan)
            .map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX))
    } else {
        plan.timeouts
            .as_ref()
            .and_then(|timeouts| timeouts.first_byte_ms)
    };
    let legacy_timeout_ms = if plan.stream {
        stream_first_byte_timeout_ms.unwrap_or(DEFAULT_TUNNEL_TIMEOUT_MS)
    } else {
        request_timeout_ms.unwrap_or(DEFAULT_NON_STREAM_TOTAL_TIMEOUT_MS)
    };

    TunnelTimeoutMetadata {
        request_timeout_ms,
        stream_first_byte_timeout_ms,
        legacy_timeout_secs: timeout_ms_to_secs(legacy_timeout_ms),
    }
}

fn timeout_ms_to_secs(ms: u64) -> u64 {
    let secs = ms.div_ceil(1_000);
    secs.clamp(MIN_TUNNEL_TIMEOUT_SECS, MAX_TUNNEL_TIMEOUT_SECS)
}

fn resolve_tunnel_node_id(proxy: Option<&ProxySnapshot>) -> Option<String> {
    let proxy = proxy?;
    if proxy.enabled == Some(false) {
        return None;
    }

    let proxy_mode = proxy
        .mode
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let node_id = proxy.node_id.as_deref().map(str::trim).unwrap_or_default();
    let has_node_id = !node_id.is_empty();
    let has_proxy_url = proxy
        .url
        .as_deref()
        .map(str::trim)
        .is_some_and(|url| !url.is_empty());

    if has_node_id && (proxy_mode == "tunnel" || !has_proxy_url) {
        return Some(node_id.to_string());
    }

    None
}

fn resolve_local_tunnel_node_id(state: &AppState, proxy: Option<&ProxySnapshot>) -> Option<String> {
    let node_id = resolve_tunnel_node_id(proxy)?;
    state.tunnel.has_local_proxy(&node_id).then_some(node_id)
}

fn build_client(
    request_url: &str,
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
    proxy: Option<&ProxySnapshot>,
    transport_profile: Option<&ResolvedTransportProfile>,
    transport_controls: ExecutionTransportControls,
) -> Result<reqwest::Client, ExecutionRuntimeTransportError> {
    validate_reqwest_transport_profile(transport_profile)?;
    let resolved_proxy_url = resolve_proxy_url(proxy)?;
    let cache_key = direct_reqwest_client_cache_key(
        request_url,
        timeouts,
        resolved_proxy_url,
        transport_profile,
        transport_controls,
    );
    cached_direct_reqwest_client(cache_key)
}

fn direct_reqwest_effective_transport_controls(
    plan: &ExecutionPlan,
    mut transport_controls: ExecutionTransportControls,
) -> ExecutionTransportControls {
    if transport_controls.http1_only || !plan.stream {
        return transport_controls;
    }
    if transport_profile_h2c_prior_knowledge(plan.transport_profile.as_ref()) {
        return transport_controls;
    }
    if direct_reqwest_stream_http_mode() == DirectReqwestStreamHttpMode::Http1 {
        transport_controls.http1_only = true;
    }
    transport_controls
}

fn direct_reqwest_stream_http_mode() -> DirectReqwestStreamHttpMode {
    *DIRECT_REQWEST_STREAM_HTTP_MODE
}

fn parse_direct_reqwest_stream_http_mode(value: &str) -> DirectReqwestStreamHttpMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" | "profile" | "provider" => DirectReqwestStreamHttpMode::Auto,
        _ => DirectReqwestStreamHttpMode::Http1,
    }
}

pub(crate) fn prewarm_direct_reqwest_client_cache_for_plan(plan: &ExecutionPlan) {
    match try_prewarm_direct_reqwest_client_cache_for_plan(plan) {
        Ok(true) => {}
        Ok(false) => {}
        Err(err) => {
            tracing::debug!(
                error = ?err,
                request_id = %plan.request_id,
                candidate_id = ?plan.candidate_id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                "gateway direct reqwest client prewarm skipped"
            );
        }
    }
}

fn try_prewarm_direct_reqwest_client_cache_for_plan(
    plan: &ExecutionPlan,
) -> Result<bool, ExecutionRuntimeTransportError> {
    if transport_profile_uses_browser_wreq(plan.transport_profile.as_ref()) {
        return Ok(false);
    }
    if resolve_tunnel_node_id(plan.proxy.as_ref()).is_some() {
        return Ok(false);
    }

    let transport_controls = direct_reqwest_effective_transport_controls(
        plan,
        resolve_execution_transport_controls(&plan.headers),
    );
    if direct_h2c_fast_path_applies(plan, transport_controls) {
        return Ok(false);
    }
    validate_reqwest_transport_profile(plan.transport_profile.as_ref())?;
    let resolved_proxy_url = resolve_proxy_url(plan.proxy.as_ref())?;
    let cache_key = direct_reqwest_client_cache_key(
        &plan.url,
        plan.timeouts.as_ref(),
        resolved_proxy_url,
        plan.transport_profile.as_ref(),
        transport_controls,
    );
    prewarm_direct_reqwest_client_cache(cache_key)?;
    Ok(true)
}

fn prewarm_direct_reqwest_client_cache(
    cache_key: DirectReqwestClientCacheKey,
) -> Result<(), ExecutionRuntimeTransportError> {
    let mut warm_after_unlock = None;
    let cache_lock_started_at = Instant::now();
    if let Ok(mut cache) = DIRECT_REQWEST_CLIENT_CACHE.lock() {
        observe_gateway_stage_ms(
            "direct_reqwest_client_cache_lock",
            cache_lock_started_at.elapsed().as_millis() as u64,
        );
        if let Some(entry) = cache.get_mut(&cache_key) {
            if entry.should_warm() {
                entry.warming = true;
                warm_after_unlock = Some((cache_key.clone(), entry.len(), entry.target_len));
            }
            drop(cache);
            if let Some((cache_key, existing_len, target_len)) = warm_after_unlock {
                let spawned = spawn_direct_reqwest_client_cache_warm(
                    cache_key.clone(),
                    existing_len,
                    target_len,
                );
                if !spawned {
                    mark_direct_reqwest_client_cache_not_warming(&cache_key);
                }
            }
            return Ok(());
        }

        let target_len = direct_reqwest_client_shard_count(&cache_key);
        let initial_len = direct_reqwest_prewarm_client_shard_count(target_len);
        let mut clients = Vec::with_capacity(initial_len);
        for _ in 0..initial_len {
            clients.push(build_direct_reqwest_client_from_cache_key(&cache_key)?);
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .builds
                .fetch_add(1, Ordering::Relaxed);
        }
        let entry =
            DirectReqwestClientCacheEntry::new(clients, target_len, target_len > initial_len);
        let warm_key = (target_len > initial_len).then(|| cache_key.clone());
        cache.insert(cache_key, entry);
        if let Some(warm_key) = warm_key {
            warm_after_unlock = Some((warm_key, initial_len, target_len));
        }
        drop(cache);
        if let Some((cache_key, existing_len, target_len)) = warm_after_unlock {
            let spawned =
                spawn_direct_reqwest_client_cache_warm(cache_key.clone(), existing_len, target_len);
            if !spawned {
                mark_direct_reqwest_client_cache_not_warming(&cache_key);
            }
        }
    } else {
        observe_gateway_stage_ms(
            "direct_reqwest_client_cache_lock",
            cache_lock_started_at.elapsed().as_millis() as u64,
        );
    }
    Ok(())
}

fn cached_direct_reqwest_client(
    cache_key: DirectReqwestClientCacheKey,
) -> Result<reqwest::Client, ExecutionRuntimeTransportError> {
    let mut warm_after_unlock = None;
    let cache_lock_started_at = Instant::now();
    if let Ok(mut cache) = DIRECT_REQWEST_CLIENT_CACHE.lock() {
        observe_gateway_stage_ms(
            "direct_reqwest_client_cache_lock",
            cache_lock_started_at.elapsed().as_millis() as u64,
        );
        if let Some(entry) = cache.get_mut(&cache_key) {
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .hits
                .fetch_add(1, Ordering::Relaxed);
            record_direct_reqwest_client_protocol_selection(&cache_key);
            let client = entry.select();
            if entry.should_warm() {
                entry.warming = true;
                warm_after_unlock = Some((cache_key.clone(), entry.len(), entry.target_len));
            }
            drop(cache);
            if let Some((cache_key, existing_len, target_len)) = warm_after_unlock {
                let spawned = spawn_direct_reqwest_client_cache_warm(
                    cache_key.clone(),
                    existing_len,
                    target_len,
                );
                if !spawned {
                    mark_direct_reqwest_client_cache_not_warming(&cache_key);
                }
            }
            return Ok(client);
        }
        DIRECT_REQWEST_CLIENT_CACHE_METRICS
            .misses
            .fetch_add(1, Ordering::Relaxed);
        let target_len = direct_reqwest_client_shard_count(&cache_key);
        let initial_len = direct_reqwest_initial_client_shard_count(target_len);
        let mut clients = Vec::with_capacity(initial_len);
        for _ in 0..initial_len {
            clients.push(build_direct_reqwest_client_from_cache_key(&cache_key)?);
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .builds
                .fetch_add(1, Ordering::Relaxed);
        }
        let entry =
            DirectReqwestClientCacheEntry::new(clients, target_len, target_len > initial_len);
        record_direct_reqwest_client_protocol_selection(&cache_key);
        let client = entry.select();
        let warm_key = (target_len > initial_len).then(|| cache_key.clone());
        cache.insert(cache_key, entry);
        if let Some(warm_key) = warm_key {
            warm_after_unlock = Some((warm_key, initial_len, target_len));
        }
        drop(cache);
        if let Some((cache_key, existing_len, target_len)) = warm_after_unlock {
            let spawned =
                spawn_direct_reqwest_client_cache_warm(cache_key.clone(), existing_len, target_len);
            if !spawned {
                mark_direct_reqwest_client_cache_not_warming(&cache_key);
            }
        }
        return Ok(client);
    }

    observe_gateway_stage_ms(
        "direct_reqwest_client_cache_lock",
        cache_lock_started_at.elapsed().as_millis() as u64,
    );
    DIRECT_REQWEST_CLIENT_CACHE_METRICS
        .misses
        .fetch_add(1, Ordering::Relaxed);
    record_direct_reqwest_client_protocol_selection(&cache_key);
    let client = build_direct_reqwest_client_from_cache_key(&cache_key)?;
    DIRECT_REQWEST_CLIENT_CACHE_METRICS
        .builds
        .fetch_add(1, Ordering::Relaxed);
    Ok(client)
}

fn spawn_direct_reqwest_client_cache_warm(
    cache_key: DirectReqwestClientCacheKey,
    existing_len: usize,
    target_len: usize,
) -> bool {
    if target_len <= existing_len {
        DIRECT_REQWEST_CLIENT_CACHE_METRICS
            .warm_skipped_total
            .fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        DIRECT_REQWEST_CLIENT_CACHE_METRICS
            .warm_skipped_total
            .fetch_add(1, Ordering::Relaxed);
        return false;
    };
    DIRECT_REQWEST_CLIENT_CACHE_METRICS
        .warm_enqueues
        .fetch_add(1, Ordering::Relaxed);
    let enqueue_started_at = Instant::now();
    handle.spawn_blocking(move || {
        for _ in existing_len..target_len {
            match build_direct_reqwest_client_from_cache_key(&cache_key) {
                Ok(client) => {
                    DIRECT_REQWEST_CLIENT_CACHE_METRICS
                        .builds
                        .fetch_add(1, Ordering::Relaxed);
                    let Ok(mut cache) = DIRECT_REQWEST_CLIENT_CACHE.lock() else {
                        return;
                    };
                    let Some(entry) = cache.get_mut(&cache_key) else {
                        return;
                    };
                    if entry.clients.len() >= entry.target_len {
                        entry.warming = false;
                        return;
                    }
                    entry.clients.push(client);
                    if entry.clients.len() >= entry.target_len {
                        entry.warming = false;
                        return;
                    }
                }
                Err(err) => {
                    tracing::debug!(
                        error = ?err,
                        "gateway direct reqwest client cache warm failed"
                    );
                    mark_direct_reqwest_client_cache_not_warming(&cache_key);
                    break;
                }
            }
        }

        let Ok(mut cache) = DIRECT_REQWEST_CLIENT_CACHE.lock() else {
            return;
        };
        let Some(entry) = cache.get_mut(&cache_key) else {
            return;
        };
        entry.warming = false;
    });
    observe_gateway_stage_ms(
        "direct_reqwest_client_cache_warm_enqueue",
        enqueue_started_at.elapsed().as_millis() as u64,
    );
    true
}

fn mark_direct_reqwest_client_cache_warming(cache_key: &DirectReqwestClientCacheKey) {
    if let Ok(mut cache) = DIRECT_REQWEST_CLIENT_CACHE.lock() {
        if let Some(entry) = cache.get_mut(cache_key) {
            entry.warming = true;
        }
    }
}

fn mark_direct_reqwest_client_cache_not_warming(cache_key: &DirectReqwestClientCacheKey) {
    if let Ok(mut cache) = DIRECT_REQWEST_CLIENT_CACHE.lock() {
        if let Some(entry) = cache.get_mut(cache_key) {
            entry.warming = false;
        }
    }
}

fn direct_reqwest_client_cache_key(
    request_url: &str,
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
    proxy_url: Option<String>,
    transport_profile: Option<&ResolvedTransportProfile>,
    transport_controls: ExecutionTransportControls,
) -> DirectReqwestClientCacheKey {
    DirectReqwestClientCacheKey {
        upstream_origin: direct_reqwest_cache_per_origin()
            .then(|| direct_reqwest_upstream_origin(request_url))
            .flatten(),
        connect_timeout_ms: timeouts.and_then(|timeouts| timeouts.connect_ms),
        proxy_url,
        follow_redirects: transport_controls.follow_redirects == Some(true),
        http1_only: transport_controls.http1_only,
        accept_invalid_certs: transport_controls.accept_invalid_certs,
        transport_profile: transport_profile.map(direct_reqwest_transport_profile_cache_key),
    }
}

fn direct_reqwest_cache_per_origin() -> bool {
    std::env::var(DIRECT_REQWEST_CACHE_PER_ORIGIN_ENV)
        .ok()
        .is_some_and(|value| matches_truthy_env_value(value.trim()))
}

fn matches_truthy_env_value(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn direct_reqwest_upstream_origin(request_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(request_url).ok()?;
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = url.host_str()?;
    let port = url.port_or_known_default()?;
    Some(format!("{scheme}://{host}:{port}"))
}

fn direct_reqwest_transport_profile_cache_key(
    profile: &ResolvedTransportProfile,
) -> DirectReqwestTransportProfileCacheKey {
    DirectReqwestTransportProfileCacheKey {
        profile_id: profile.profile_id.trim().to_string(),
        backend: profile.backend.trim().to_ascii_lowercase(),
        http_mode: profile.http_mode.trim().to_ascii_lowercase(),
        pool_scope: profile.pool_scope.trim().to_ascii_lowercase(),
        header_fingerprint: stable_json_cache_key(profile.header_fingerprint.as_ref()),
        extra: stable_json_cache_key(profile.extra.as_ref()),
    }
}

fn stable_json_cache_key(value: Option<&Value>) -> Option<String> {
    value.and_then(|value| serde_json::to_string(value).ok())
}

fn build_direct_reqwest_client_cache_entry_from_cache_key(
    cache_key: &DirectReqwestClientCacheKey,
) -> Result<DirectReqwestClientCacheEntry, ExecutionRuntimeTransportError> {
    let shard_count = direct_reqwest_client_shard_count(cache_key);
    let mut clients = Vec::with_capacity(shard_count);
    for _ in 0..shard_count {
        clients.push(build_direct_reqwest_client_from_cache_key(cache_key)?);
    }
    Ok(DirectReqwestClientCacheEntry::new(
        clients,
        shard_count,
        false,
    ))
}

fn direct_reqwest_client_shard_count(cache_key: &DirectReqwestClientCacheKey) -> usize {
    if let Some(shards) = env_positive_usize(DIRECT_REQWEST_CLIENT_SHARDS_ENV) {
        return shards.clamp(1, MAX_DIRECT_REQWEST_H2_CLIENT_SHARDS);
    }
    let target_gate_limit = crate::state::upstream_target_gate_limit_from_env()
        .unwrap_or_else(crate::state::upstream_target_gate_auto_limit);
    if !direct_reqwest_client_cache_key_uses_http2(cache_key) {
        return direct_reqwest_client_shards_from_config(
            None,
            target_gate_limit,
            env_positive_usize(DIRECT_REQWEST_HTTP1_TARGET_STREAMS_PER_CLIENT_ENV)
                .unwrap_or(DEFAULT_HTTP1_TARGET_STREAMS_PER_CLIENT),
        );
    }
    direct_reqwest_h2_client_shards_from_config(
        env_positive_usize(DIRECT_REQWEST_H2_CLIENT_SHARDS_ENV),
        target_gate_limit,
        env_positive_usize(DIRECT_REQWEST_H2_TARGET_STREAMS_PER_CLIENT_ENV)
            .unwrap_or(DEFAULT_H2_TARGET_STREAMS_PER_CLIENT),
    )
}

fn direct_reqwest_client_cache_key_uses_http2(cache_key: &DirectReqwestClientCacheKey) -> bool {
    if cache_key.http1_only {
        return false;
    }
    direct_reqwest_client_cache_key_uses_h2c_prior_knowledge(cache_key)
}

fn direct_reqwest_client_cache_key_uses_h2c_prior_knowledge(
    cache_key: &DirectReqwestClientCacheKey,
) -> bool {
    cache_key
        .transport_profile
        .as_ref()
        .is_some_and(|profile| profile.http_mode == TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE)
}

fn record_direct_reqwest_client_protocol_selection(cache_key: &DirectReqwestClientCacheKey) {
    if cache_key.http1_only {
        DIRECT_REQWEST_CLIENT_CACHE_METRICS
            .http1_selections
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    if direct_reqwest_client_cache_key_uses_h2c_prior_knowledge(cache_key) {
        DIRECT_REQWEST_CLIENT_CACHE_METRICS
            .h2c_selections
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    DIRECT_REQWEST_CLIENT_CACHE_METRICS
        .auto_selections
        .fetch_add(1, Ordering::Relaxed);
}

fn direct_reqwest_h2_client_shards_from_config(
    explicit_shards: Option<usize>,
    target_gate_limit: usize,
    target_streams_per_client: usize,
) -> usize {
    direct_reqwest_client_shards_from_config(
        explicit_shards,
        target_gate_limit,
        target_streams_per_client,
    )
}

fn direct_reqwest_client_shards_from_config(
    explicit_shards: Option<usize>,
    target_gate_limit: usize,
    target_streams_per_client: usize,
) -> usize {
    if let Some(shards) = explicit_shards {
        return shards.clamp(1, MAX_DIRECT_REQWEST_H2_CLIENT_SHARDS);
    }
    let streams_per_client = target_streams_per_client.max(1);
    target_gate_limit
        .max(1)
        .div_ceil(streams_per_client)
        .clamp(1, MAX_DIRECT_REQWEST_H2_CLIENT_SHARDS)
}

fn direct_reqwest_initial_client_shard_count(target_len: usize) -> usize {
    env_positive_usize(DIRECT_REQWEST_SYNC_WARM_CLIENTS_ENV)
        .unwrap_or(DEFAULT_DIRECT_REQWEST_SYNC_WARM_CLIENTS)
        .clamp(1, target_len.clamp(1, MAX_DIRECT_REQWEST_SYNC_WARM_CLIENTS))
}

fn direct_reqwest_prewarm_client_shard_count(target_len: usize) -> usize {
    let request_path_cap = direct_reqwest_initial_client_shard_count(target_len);
    env_positive_usize(DIRECT_REQWEST_PREWARM_SYNC_CLIENTS_ENV)
        .unwrap_or(request_path_cap)
        .clamp(1, target_len.max(1).min(request_path_cap))
}

fn env_positive_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn build_direct_reqwest_client_from_cache_key(
    cache_key: &DirectReqwestClientCacheKey,
) -> Result<reqwest::Client, ExecutionRuntimeTransportError> {
    let mut builder = reqwest::Client::builder();
    if !cache_key.follow_redirects {
        builder = builder.redirect(Policy::none());
    }
    if cache_key.http1_only
        || cache_key
            .transport_profile
            .as_ref()
            .is_some_and(|profile| profile.http_mode == TRANSPORT_HTTP_MODE_HTTP1_ONLY)
    {
        builder = builder.http1_only();
    } else if cache_key
        .transport_profile
        .as_ref()
        .is_some_and(|profile| profile.http_mode == TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE)
    {
        builder = builder.http2_prior_knowledge();
    }
    let mut builder = apply_http_client_config(
        builder,
        &HttpClientConfig {
            connect_timeout_ms: cache_key.connect_timeout_ms,
            pool_max_idle_per_host: Some(direct_reqwest_pool_max_idle_per_host()),
            ..HttpClientConfig::default()
        },
    );
    builder = apply_transport_profile_cache_key(
        builder,
        cache_key.transport_profile.as_ref(),
        cache_key.http1_only,
    );
    if cache_key.accept_invalid_certs {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(proxy_url) = cache_key.proxy_url.as_deref() {
        let proxy =
            reqwest::Proxy::all(proxy_url).map_err(ExecutionRuntimeTransportError::InvalidProxy)?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(ExecutionRuntimeTransportError::ClientBuild)
}

fn direct_reqwest_pool_max_idle_per_host() -> usize {
    const DEFAULT_MAX_IDLE_PER_HOST: usize = 1024;
    std::env::var("AETHER_GATEWAY_UPSTREAM_POOL_MAX_IDLE_PER_HOST")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_IDLE_PER_HOST)
}

pub(crate) fn direct_reqwest_client_cache_metric_samples() -> Vec<MetricSample> {
    let (entries, clients, target_clients, ready_entries, warming_entries, pending_clients) =
        DIRECT_REQWEST_CLIENT_CACHE
            .lock()
            .map(|cache| {
                let entries = cache.len() as u64;
                let clients = cache.values().map(|entry| entry.len() as u64).sum();
                let target_clients = cache.values().map(|entry| entry.target_len as u64).sum();
                let ready_entries = cache
                    .values()
                    .filter(|entry| entry.len() >= entry.target_len)
                    .count() as u64;
                let warming_entries = cache.values().filter(|entry| entry.warming).count() as u64;
                let pending_clients = cache
                    .values()
                    .map(|entry| entry.target_len.saturating_sub(entry.len()) as u64)
                    .sum();
                (
                    entries,
                    clients,
                    target_clients,
                    ready_entries,
                    warming_entries,
                    pending_clients,
                )
            })
            .unwrap_or((0, 0, 0, 0, 0, 0));
    let (h2c_entries, h2c_clients, h2c_target_clients) = DIRECT_H2C_CLIENT_CACHE
        .lock()
        .map(|cache| {
            let entries = cache.len() as u64;
            let clients = cache.values().map(|entry| entry.len() as u64).sum();
            let target_clients = cache.values().map(|entry| entry.target_len as u64).sum();
            (entries, clients, target_clients)
        })
        .unwrap_or((0, 0, 0));
    let (
        h2c_sender_entries,
        h2c_sender_ready_entries,
        h2c_senders,
        h2c_target_senders,
        h2c_pending_senders,
        h2c_sender_in_flight,
        h2c_sender_max_in_flight,
    ) = DIRECT_H2C_SENDER_CACHE
        .lock()
        .map_or((0, 0, 0, 0, 0, 0, 0), |cache| {
            let entries = cache.len() as u64;
            let ready_entries = cache
                .values()
                .filter_map(|cell| cell.get())
                .filter(|entry| entry.len() >= entry.target_len)
                .count() as u64;
            let senders = cache
                .values()
                .filter_map(|cell| cell.get())
                .map(|entry| entry.len() as u64)
                .sum();
            let target_senders = cache
                .values()
                .filter_map(|cell| cell.get())
                .map(|entry| entry.target_len as u64)
                .sum();
            let pending_senders = cache
                .values()
                .map(|cell| {
                    cell.get()
                        .map(|entry| entry.target_len.saturating_sub(entry.len()) as u64)
                        .unwrap_or_else(|| direct_h2c_client_shard_count() as u64)
                })
                .sum();
            let in_flight = cache
                .values()
                .filter_map(|cell| cell.get())
                .map(|entry| entry.in_flight())
                .sum();
            let max_in_flight = cache
                .values()
                .filter_map(|cell| cell.get())
                .map(|entry| entry.max_in_flight())
                .max()
                .unwrap_or(0);
            (
                entries,
                ready_entries,
                senders,
                target_senders,
                pending_senders,
                in_flight,
                max_in_flight,
            )
        });
    let mut samples = vec![
        MetricSample::new(
            "direct_reqwest_client_cache_entries",
            "Number of cached direct reqwest clients.",
            MetricKind::Gauge,
            entries,
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_clients",
            "Number of direct reqwest clients across all cache entries.",
            MetricKind::Gauge,
            clients,
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_target_clients",
            "Target number of direct reqwest clients across all cache entries.",
            MetricKind::Gauge,
            target_clients,
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_ready_entries",
            "Number of direct reqwest client cache entries at target shard count.",
            MetricKind::Gauge,
            ready_entries,
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_warming_entries",
            "Number of direct reqwest client cache entries currently warming in the background.",
            MetricKind::Gauge,
            warming_entries,
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_pending_clients",
            "Number of direct reqwest client shards still missing from target cache size.",
            MetricKind::Gauge,
            pending_clients,
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_hits_total",
            "Number of direct reqwest client cache hits.",
            MetricKind::Counter,
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .hits
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_misses_total",
            "Number of direct reqwest client cache misses.",
            MetricKind::Counter,
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .misses
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_builds_total",
            "Number of direct reqwest clients built after cache misses.",
            MetricKind::Counter,
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .builds
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_warm_enqueue_total",
            "Number of background direct reqwest client cache warm jobs enqueued.",
            MetricKind::Counter,
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .warm_enqueues
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_reqwest_client_cache_warm_skipped_total",
            "Number of direct reqwest client cache warm attempts skipped before enqueue.",
            MetricKind::Counter,
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .warm_skipped_total
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_reqwest_client_http1_select_total",
            "Number of direct reqwest client selections using forced HTTP/1.",
            MetricKind::Counter,
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .http1_selections
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_reqwest_client_h2c_select_total",
            "Number of direct reqwest client selections using h2c prior knowledge.",
            MetricKind::Counter,
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .h2c_selections
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_reqwest_client_auto_select_total",
            "Number of direct reqwest client selections using reqwest automatic protocol negotiation.",
            MetricKind::Counter,
            DIRECT_REQWEST_CLIENT_CACHE_METRICS
                .auto_selections
                .load(Ordering::Relaxed),
        ),
    ];
    samples.extend([
        MetricSample::new(
            "direct_h2c_client_cache_entries",
            "Number of cached direct H2C client entries.",
            MetricKind::Gauge,
            h2c_entries,
        ),
        MetricSample::new(
            "direct_h2c_client_cache_clients",
            "Number of direct H2C clients across all cache entries.",
            MetricKind::Gauge,
            h2c_clients,
        ),
        MetricSample::new(
            "direct_h2c_client_cache_target_clients",
            "Target number of direct H2C clients across all cache entries.",
            MetricKind::Gauge,
            h2c_target_clients,
        ),
        MetricSample::new(
            "direct_h2c_client_cache_hits_total",
            "Number of direct H2C client cache hits.",
            MetricKind::Counter,
            DIRECT_H2C_CLIENT_CACHE_METRICS.hits.load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_h2c_client_cache_misses_total",
            "Number of direct H2C client cache misses.",
            MetricKind::Counter,
            DIRECT_H2C_CLIENT_CACHE_METRICS
                .misses
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_h2c_client_cache_builds_total",
            "Number of direct H2C clients built after cache misses.",
            MetricKind::Counter,
            DIRECT_H2C_CLIENT_CACHE_METRICS
                .builds
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_entries",
            "Number of cached direct H2C sender entries.",
            MetricKind::Gauge,
            h2c_sender_entries,
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_senders",
            "Number of direct H2C senders across all cache entries.",
            MetricKind::Gauge,
            h2c_senders,
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_ready_entries",
            "Number of direct H2C sender cache entries at target sender count.",
            MetricKind::Gauge,
            h2c_sender_ready_entries,
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_target_senders",
            "Target number of direct H2C senders across all cache entries.",
            MetricKind::Gauge,
            h2c_target_senders,
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_pending_senders",
            "Number of direct H2C sender connections still missing from target cache size.",
            MetricKind::Gauge,
            h2c_pending_senders,
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_in_flight",
            "Current number of direct H2C requests waiting for upstream headers across sender slots.",
            MetricKind::Gauge,
            h2c_sender_in_flight,
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_max_slot_in_flight",
            "Highest observed in-flight request count on a single direct H2C sender slot.",
            MetricKind::Gauge,
            h2c_sender_max_in_flight,
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_hits_total",
            "Number of direct H2C sender cache hits.",
            MetricKind::Counter,
            DIRECT_H2C_SENDER_CACHE_METRICS.hits.load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_misses_total",
            "Number of direct H2C sender cache misses.",
            MetricKind::Counter,
            DIRECT_H2C_SENDER_CACHE_METRICS
                .misses
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_builds_total",
            "Number of direct H2C senders built after cache misses.",
            MetricKind::Counter,
            DIRECT_H2C_SENDER_CACHE_METRICS
                .builds
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_prewarm_requested_total",
            "Number of direct H2C sender prewarm URLs requested.",
            MetricKind::Counter,
            DIRECT_H2C_SENDER_CACHE_METRICS
                .prewarm_requested
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_prewarm_success_total",
            "Number of direct H2C sender cache targets successfully prewarmed.",
            MetricKind::Counter,
            DIRECT_H2C_SENDER_CACHE_METRICS
                .prewarm_success
                .load(Ordering::Relaxed),
        ),
        MetricSample::new(
            "direct_h2c_sender_cache_prewarm_failed_total",
            "Number of direct H2C sender cache prewarm targets or URLs that failed.",
            MetricKind::Counter,
            DIRECT_H2C_SENDER_CACHE_METRICS
                .prewarm_failed
                .load(Ordering::Relaxed),
        ),
    ]);
    samples
}

pub(crate) fn build_browser_wreq_client(
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
    proxy: Option<&ProxySnapshot>,
    transport_profile: &ResolvedTransportProfile,
    transport_controls: ExecutionTransportControls,
    apply_total_timeout: bool,
) -> Result<wreq::Client, ExecutionRuntimeTransportError> {
    let emulation = browser_wreq_emulation_from_profile(transport_profile)?;
    let mut builder = wreq::Client::builder().emulation(emulation);
    if transport_controls.follow_redirects == Some(true) {
        builder = builder.redirect(wreq::redirect::Policy::limited(10));
    }
    if transport_controls.http1_only || transport_profile_http1_only(Some(transport_profile)) {
        builder = builder.http1_only();
    }
    if transport_controls.accept_invalid_certs {
        builder = builder.cert_verification(false).verify_hostname(false);
    }
    if let Some(connect_ms) = timeouts.and_then(|timeouts| timeouts.connect_ms) {
        builder = builder.connect_timeout(Duration::from_millis(connect_ms));
    }
    if apply_total_timeout {
        if let Some(total_ms) = timeouts.and_then(|timeouts| timeouts.total_ms) {
            builder = builder.timeout(Duration::from_millis(total_ms));
        }
    }
    if let Some(read_ms) = timeouts.and_then(|timeouts| timeouts.read_ms) {
        builder = builder.read_timeout(Duration::from_millis(read_ms));
    }
    if let Some(proxy_url) = resolve_proxy_url(proxy)? {
        let proxy = wreq::Proxy::all(proxy_url.as_str())
            .map_err(ExecutionRuntimeTransportError::BrowserClientBuild)?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(ExecutionRuntimeTransportError::BrowserClientBuild)
}

fn browser_wreq_emulation_from_profile(
    profile: &ResolvedTransportProfile,
) -> Result<wreq_util::Emulation, ExecutionRuntimeTransportError> {
    match normalize_browser_profile_name(browser_transport_profile_name(profile)).as_str() {
        "chrome100" => Ok(wreq_util::Emulation::Chrome100),
        "chrome101" => Ok(wreq_util::Emulation::Chrome101),
        "chrome104" => Ok(wreq_util::Emulation::Chrome104),
        "chrome105" => Ok(wreq_util::Emulation::Chrome105),
        "chrome106" => Ok(wreq_util::Emulation::Chrome106),
        "chrome107" => Ok(wreq_util::Emulation::Chrome107),
        "chrome108" => Ok(wreq_util::Emulation::Chrome108),
        "chrome109" => Ok(wreq_util::Emulation::Chrome109),
        "chrome110" => Ok(wreq_util::Emulation::Chrome110),
        "chrome114" => Ok(wreq_util::Emulation::Chrome114),
        "chrome116" => Ok(wreq_util::Emulation::Chrome116),
        "chrome117" => Ok(wreq_util::Emulation::Chrome117),
        "chrome118" => Ok(wreq_util::Emulation::Chrome118),
        "chrome119" => Ok(wreq_util::Emulation::Chrome119),
        "chrome120" => Ok(wreq_util::Emulation::Chrome120),
        "chrome123" => Ok(wreq_util::Emulation::Chrome123),
        "chrome124" => Ok(wreq_util::Emulation::Chrome124),
        "chrome126" => Ok(wreq_util::Emulation::Chrome126),
        "chrome127" => Ok(wreq_util::Emulation::Chrome127),
        "chrome128" => Ok(wreq_util::Emulation::Chrome128),
        "chrome129" => Ok(wreq_util::Emulation::Chrome129),
        "chrome130" => Ok(wreq_util::Emulation::Chrome130),
        "chrome131" => Ok(wreq_util::Emulation::Chrome131),
        "chrome132" => Ok(wreq_util::Emulation::Chrome132),
        "chrome133" => Ok(wreq_util::Emulation::Chrome133),
        "chrome134" => Ok(wreq_util::Emulation::Chrome134),
        "chrome135" => Ok(wreq_util::Emulation::Chrome135),
        "chrome136" => Ok(wreq_util::Emulation::Chrome136),
        "chrome137" => Ok(wreq_util::Emulation::Chrome137),
        "chrome138" => Ok(wreq_util::Emulation::Chrome138),
        "chrome139" => Ok(wreq_util::Emulation::Chrome139),
        "chrome140" => Ok(wreq_util::Emulation::Chrome140),
        "chrome141" => Ok(wreq_util::Emulation::Chrome141),
        "chrome142" => Ok(wreq_util::Emulation::Chrome142),
        "chrome143" => Ok(wreq_util::Emulation::Chrome143),
        "chrome144" => Ok(wreq_util::Emulation::Chrome144),
        "chrome145" => Ok(wreq_util::Emulation::Chrome145),
        other => Err(ExecutionRuntimeTransportError::UnsupportedTransportProfile(
            format!("browser_wreq:{other}"),
        )),
    }
}

fn normalize_browser_profile_name(value: String) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-', ' '], "")
}

fn validate_reqwest_transport_profile(
    transport_profile: Option<&ResolvedTransportProfile>,
) -> Result<(), ExecutionRuntimeTransportError> {
    let Some(profile) = transport_profile else {
        return Ok(());
    };
    if profile
        .backend
        .trim()
        .eq_ignore_ascii_case(TRANSPORT_BACKEND_REQWEST_RUSTLS)
    {
        return Ok(());
    }
    Err(ExecutionRuntimeTransportError::UnsupportedTransportProfile(
        profile.backend.clone(),
    ))
}

fn transport_profile_uses_browser_wreq(
    transport_profile: Option<&ResolvedTransportProfile>,
) -> bool {
    transport_profile
        .map(|profile| {
            profile
                .backend
                .trim()
                .eq_ignore_ascii_case(TRANSPORT_BACKEND_BROWSER_WREQ)
        })
        .unwrap_or(false)
}

fn browser_transport_profile_name(profile: &ResolvedTransportProfile) -> String {
    profile
        .extra
        .as_ref()
        .and_then(|value| {
            value
                .get("browser_profile")
                .or_else(|| value.get("impersonate"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            profile
                .profile_id
                .trim()
                .is_empty()
                .then_some("chrome136".to_string())
                .or_else(|| Some(profile.profile_id.trim().to_string()))
        })
        .unwrap_or_else(|| "chrome136".to_string())
}

fn insert_browser_control_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), ExecutionRuntimeTransportError> {
    headers.insert(
        HeaderName::from_static(name),
        HeaderValue::from_str(value)
            .map_err(|_| ExecutionRuntimeTransportError::InvalidHeaderValue(name.to_string()))?,
    );
    Ok(())
}

fn transport_profile_http1_only(transport_profile: Option<&ResolvedTransportProfile>) -> bool {
    transport_profile
        .map(|profile| {
            profile
                .http_mode
                .trim()
                .eq_ignore_ascii_case(TRANSPORT_HTTP_MODE_HTTP1_ONLY)
        })
        .unwrap_or(false)
}

fn transport_profile_h2c_prior_knowledge(
    transport_profile: Option<&ResolvedTransportProfile>,
) -> bool {
    transport_profile
        .map(|profile| {
            profile
                .http_mode
                .trim()
                .eq_ignore_ascii_case(TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE)
        })
        .unwrap_or(false)
}

fn apply_transport_profile(
    builder: reqwest::ClientBuilder,
    transport_profile: Option<&ResolvedTransportProfile>,
) -> reqwest::ClientBuilder {
    let Some(profile) = transport_profile else {
        return builder;
    };
    let profile_id = profile.profile_id.trim();
    if profile_id.is_empty() || transport_profile_h2c_prior_knowledge(Some(profile)) {
        return builder;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    builder.use_preconfigured_tls(build_best_effort_transport_tls_config(
        transport_profile_http1_only(transport_profile),
    ))
}

fn apply_transport_profile_cache_key(
    builder: reqwest::ClientBuilder,
    transport_profile: Option<&DirectReqwestTransportProfileCacheKey>,
    http1_only: bool,
) -> reqwest::ClientBuilder {
    let Some(profile) = transport_profile else {
        return builder;
    };
    if profile.profile_id.is_empty() || profile.http_mode == TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE
    {
        return builder;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    builder.use_preconfigured_tls(build_best_effort_transport_tls_config(http1_only))
}

fn build_best_effort_transport_tls_config(http1_only: bool) -> rustls::ClientConfig {
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = rustls::ClientConfig::builder_with_protocol_versions(&[
        &rustls::version::TLS13,
        &rustls::version::TLS12,
    ])
    .with_root_certificates(root_store)
    .with_no_client_auth();
    config.alpn_protocols = if http1_only {
        vec![b"http/1.1".to_vec()]
    } else {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    };
    config
}

fn resolve_proxy_url(
    proxy: Option<&ProxySnapshot>,
) -> Result<Option<String>, ExecutionRuntimeTransportError> {
    let Some(proxy) = proxy else {
        return Ok(None);
    };

    if proxy.enabled == Some(false) {
        return Ok(None);
    }

    if let Some(proxy_url) = proxy
        .url
        .as_ref()
        .map(|url| url.trim())
        .filter(|url| !url.is_empty())
    {
        return Ok(Some(proxy_url.to_string()));
    }

    if proxy.node_id.is_some() || proxy.mode.as_deref() == Some("tunnel") {
        return Err(ExecutionRuntimeTransportError::ProxyUnsupported);
    }

    Ok(None)
}

pub(crate) fn build_request_headers(
    headers: &BTreeMap<String, String>,
    content_encoding: Option<&str>,
    allow_passthrough_content_encoding: bool,
) -> Result<HeaderMap, ExecutionRuntimeTransportError> {
    let mut out = HeaderMap::new();
    let normalized_content_encoding = normalize_content_encoding(content_encoding);
    if let Some(encoding) = normalized_content_encoding.as_deref() {
        if encoding != "gzip" && !allow_passthrough_content_encoding {
            return Err(ExecutionRuntimeTransportError::UnsupportedContentEncoding(
                encoding.to_string(),
            ));
        }
    }
    for (key, value) in headers {
        let normalized_key = key.trim().to_ascii_lowercase();
        if is_hop_by_hop_header(&normalized_key)
            || normalized_key == "content-encoding"
            || normalized_key == EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER
            || normalized_key == EXECUTION_REQUEST_HTTP1_ONLY_HEADER
            || normalized_key == EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER
        {
            continue;
        }

        let header_name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|_| ExecutionRuntimeTransportError::InvalidHeaderName(key.clone()))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|_| ExecutionRuntimeTransportError::InvalidHeaderValue(key.clone()))?;
        out.insert(header_name, header_value);
    }
    if let Some(encoding) = normalized_content_encoding {
        out.insert(
            reqwest::header::CONTENT_ENCODING,
            HeaderValue::from_str(&encoding).map_err(|_| {
                ExecutionRuntimeTransportError::InvalidHeaderValue("content-encoding".into())
            })?,
        );
    }
    Ok(out)
}

fn resolve_execution_transport_controls(
    headers: &BTreeMap<String, String>,
) -> ExecutionTransportControls {
    ExecutionTransportControls {
        follow_redirects: execution_transport_header_value(
            headers,
            EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
        )
        .and_then(|value| parse_execution_transport_bool(value)),
        http1_only: execution_transport_header_value(headers, EXECUTION_REQUEST_HTTP1_ONLY_HEADER)
            .and_then(|value| parse_execution_transport_bool(value))
            .unwrap_or(false),
        accept_invalid_certs: execution_transport_header_value(
            headers,
            EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER,
        )
        .and_then(|value| parse_execution_transport_bool(value))
        .unwrap_or(false),
    }
}

fn execution_transport_header_value<'a>(
    headers: &'a BTreeMap<String, String>,
    target: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(target))
        .map(|(_, value)| value.as_str())
}

fn parse_execution_transport_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn header_map_to_string_map(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "content-length"
            | "connection"
            | "upgrade"
            | "keep-alive"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
    )
}

pub(crate) fn collect_response_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    header_map_to_string_map(headers)
}

fn collect_tunnel_response_headers(headers: &[(String, String)]) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

fn execution_header_for_log<'a>(
    headers: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn execution_log_url_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "-".to_string())
}

pub(crate) fn decode_response_body_bytes(
    headers: &BTreeMap<String, String>,
    body_bytes: &[u8],
) -> Option<Vec<u8>> {
    let encoding = headers
        .get("content-encoding")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    match encoding.as_deref() {
        Some("gzip") => {
            let mut decoder = GzDecoder::new(body_bytes);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        }
        Some("deflate") => {
            let mut decoder = DeflateDecoder::new(body_bytes);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        }
        _ => None,
    }
}

pub(crate) fn response_body_is_json(headers: &BTreeMap<String, String>, body_bytes: &[u8]) -> bool {
    let content_type = headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if content_type.contains("application/connect+json")
        || content_type.contains("application/connect+proto")
    {
        return false;
    }
    if content_type.contains("json") {
        return true;
    }

    serde_json::from_slice::<Value>(body_bytes).is_ok()
}

pub(crate) fn build_execution_response_body(
    headers: &BTreeMap<String, String>,
    body_bytes: &[u8],
    decoded_body_bytes: &[u8],
    stream: bool,
) -> Result<Option<ResponseBody>, ExecutionRuntimeTransportError> {
    if body_bytes.is_empty() {
        return Ok(None);
    }

    if let Some(body_json) = extract_provider_private_stream_error_body(None, decoded_body_bytes)
        .or_else(|| extract_provider_private_stream_error_body(None, body_bytes))
    {
        return Ok(Some(ResponseBody {
            json_body: Some(body_json),
            body_bytes_b64: None,
        }));
    }

    if stream {
        return Ok(Some(ResponseBody {
            json_body: None,
            body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(body_bytes)),
        }));
    }

    if response_body_is_json(headers, decoded_body_bytes) {
        let body_json: Value = serde_json::from_slice(decoded_body_bytes)
            .map_err(ExecutionRuntimeTransportError::InvalidJson)?;
        return Ok(Some(ResponseBody {
            json_body: Some(body_json),
            body_bytes_b64: None,
        }));
    }

    Ok(Some(ResponseBody {
        json_body: None,
        body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(body_bytes)),
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

    use aether_contracts::{
        ExecutionPlan, ExecutionTimeouts, ProxySnapshot, RequestBody, ResolvedTransportProfile,
        EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
        TRANSPORT_BACKEND_BROWSER_WREQ, TRANSPORT_BACKEND_REQWEST_RUSTLS, TRANSPORT_HTTP_MODE_AUTO,
        TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE, TRANSPORT_HTTP_MODE_HTTP1_ONLY,
    };
    use aether_data::repository::proxy_nodes::{
        InMemoryProxyNodeRepository, ProxyNodeReadRepository, StoredProxyNode,
    };
    use axum::body::{Body, Bytes};
    use axum::extract::ws::Message;
    use axum::extract::Path;
    use axum::http::HeaderMap as AxumHeaderMap;
    use axum::routing::{any, post};
    use axum::{Json, Router};
    use base64::Engine as _;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;

    use super::{
        build_browser_wreq_client, build_client, build_direct_tunnel_request_meta,
        build_execution_response_body, build_request_headers, execute_sync_plan,
        record_manual_proxy_request_failure, record_manual_proxy_request_outcome,
        record_manual_proxy_request_success, record_manual_proxy_stream_error,
        resolve_execution_transport_controls, resolve_non_stream_total_timeout,
        resolve_stream_first_byte_timeout, response_body_is_json, DirectSyncExecutionRuntime,
        ExecutionRuntimeTransportError, ExecutionTransportControls,
    };
    use crate::constants::{
        EXECUTION_RUNTIME_LOOP_GUARD_HEADER, EXECUTION_RUNTIME_LOOP_GUARD_VIA_TOKEN,
    };
    use crate::frontdoor_loop_guard::{
        frontdoor_self_loop_public_ai_path, gateway_frontdoor_self_loop_guard_error_with_port,
        gateway_frontdoor_self_loop_guard_matches_with_port,
    };
    use crate::tunnel::{tunnel_protocol, TunnelProxyConn};
    use crate::AppState;

    const LOCAL_HTTP_SUCCESS_TIMEOUT_MS: u64 = 15_000;

    #[test]
    fn gateway_frontdoor_self_loop_guard_matches_loopback_public_ai_route() {
        assert!(gateway_frontdoor_self_loop_guard_matches_with_port(
            8084,
            "http://127.0.0.1:8084/v1/messages"
        ));
        assert!(gateway_frontdoor_self_loop_guard_matches_with_port(
            8084,
            "http://localhost:8084/v1/responses"
        ));
        assert!(gateway_frontdoor_self_loop_guard_matches_with_port(
            8084,
            "http://localhost:8084/v1internal:streamGenerateContent?alt=sse"
        ));
    }

    #[test]
    fn gateway_frontdoor_self_loop_guard_ignores_non_ai_routes() {
        assert!(!gateway_frontdoor_self_loop_guard_matches_with_port(
            8084,
            "http://127.0.0.1:8084/_gateway/health"
        ));
        assert!(!frontdoor_self_loop_public_ai_path("/_gateway/health"));
    }

    #[test]
    fn gateway_frontdoor_self_loop_guard_ignores_different_ports() {
        assert!(!gateway_frontdoor_self_loop_guard_matches_with_port(
            8084,
            "http://127.0.0.1:9999/v1/messages"
        ));
    }

    #[test]
    fn gateway_frontdoor_self_loop_guard_reports_clear_error() {
        assert_eq!(
            gateway_frontdoor_self_loop_guard_error_with_port(
                8084,
                "http://localhost:8084/v1/responses"
            ),
            Some(
                "upstream execution target resolves back to the local aether-gateway frontdoor: http://localhost:8084/v1/responses"
                    .to_string()
            )
        );
    }

    #[test]
    fn direct_sync_execution_runtime_builds_clients_for_socks_proxy_urls() {
        let timeouts = ExecutionTimeouts {
            connect_ms: Some(5_000),
            total_ms: Some(5_000),
            ..ExecutionTimeouts::default()
        };

        for proxy_url in ["socks5://127.0.0.1:1080", "socks5h://127.0.0.1:1080"] {
            build_client(
                "https://api.example.test/v1/chat/completions",
                Some(&timeouts),
                Some(&aether_contracts::ProxySnapshot {
                    enabled: Some(true),
                    mode: Some("socks".into()),
                    node_id: None,
                    label: Some("manual-proxy".into()),
                    url: Some(proxy_url.to_string()),
                    extra: None,
                }),
                None,
                ExecutionTransportControls::default(),
            )
            .unwrap_or_else(|err| panic!("client should build for {proxy_url}: {err}"));
        }
    }

    struct TestEnvVarGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl Drop for TestEnvVarGuard {
        fn drop(&mut self) {
            match self.previous.as_ref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn set_test_env_var(key: &'static str, value: &str) -> TestEnvVarGuard {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        TestEnvVarGuard { key, previous }
    }

    fn direct_reqwest_env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("direct reqwest env lock")
    }

    #[test]
    fn direct_reqwest_client_cache_key_includes_transport_profile() {
        let _guard = direct_reqwest_env_lock();
        let timeouts = ExecutionTimeouts {
            connect_ms: Some(5_000),
            ..ExecutionTimeouts::default()
        };
        let h2c_profile = ResolvedTransportProfile {
            profile_id: "mock-h2c".into(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
            http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: Some(json!({"pool": "a"})),
        };
        let same_h2c_profile = ResolvedTransportProfile {
            extra: Some(json!({"pool": "a"})),
            ..h2c_profile.clone()
        };
        let http1_profile = ResolvedTransportProfile {
            http_mode: TRANSPORT_HTTP_MODE_HTTP1_ONLY.into(),
            ..h2c_profile.clone()
        };

        let left = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18184/v1/chat/completions",
            Some(&timeouts),
            None,
            Some(&h2c_profile),
            ExecutionTransportControls::default(),
        );
        let right = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18184/v1/responses",
            Some(&timeouts),
            None,
            Some(&same_h2c_profile),
            ExecutionTransportControls::default(),
        );
        let different_mode = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18184/v1/chat/completions",
            Some(&timeouts),
            None,
            Some(&http1_profile),
            ExecutionTransportControls::default(),
        );
        let different_proxy = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18184/v1/chat/completions",
            Some(&timeouts),
            Some("http://127.0.0.1:8080".into()),
            Some(&h2c_profile),
            ExecutionTransportControls::default(),
        );
        assert_eq!(left, right);
        assert_ne!(left, different_mode);
        assert_ne!(left, different_proxy);
        assert!(super::direct_reqwest_client_cache_key_uses_http2(&left));
        assert!(!super::direct_reqwest_client_cache_key_uses_http2(
            &different_mode
        ));
    }

    #[test]
    fn direct_reqwest_client_cache_key_splits_origin_only_when_enabled() {
        let _guard = direct_reqwest_env_lock();
        let profile = ResolvedTransportProfile {
            profile_id: "mock-h2c-origin".into(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
            http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };

        let shared_left = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18184/v1/chat/completions",
            None,
            None,
            Some(&profile),
            ExecutionTransportControls::default(),
        );
        let shared_right = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18185/v1/chat/completions",
            None,
            None,
            Some(&profile),
            ExecutionTransportControls::default(),
        );
        assert_eq!(shared_left, shared_right);

        let _per_origin = set_test_env_var(super::DIRECT_REQWEST_CACHE_PER_ORIGIN_ENV, "true");
        let split_left = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18184/v1/chat/completions",
            None,
            None,
            Some(&profile),
            ExecutionTransportControls::default(),
        );
        let split_right = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18185/v1/chat/completions",
            None,
            None,
            Some(&profile),
            ExecutionTransportControls::default(),
        );
        assert_ne!(split_left, split_right);
    }

    #[test]
    fn direct_reqwest_auto_profile_is_not_classified_as_h2() {
        let auto_profile = ResolvedTransportProfile {
            profile_id: "auto-profile".into(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
            http_mode: TRANSPORT_HTTP_MODE_AUTO.into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };
        let h2c_profile = ResolvedTransportProfile {
            http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
            ..auto_profile.clone()
        };

        let auto_key = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18184/v1/chat/completions",
            None,
            None,
            Some(&auto_profile),
            ExecutionTransportControls::default(),
        );
        let h2c_key = super::direct_reqwest_client_cache_key(
            "http://127.0.0.1:18184/v1/chat/completions",
            None,
            None,
            Some(&h2c_profile),
            ExecutionTransportControls::default(),
        );

        assert!(!super::direct_reqwest_client_cache_key_uses_http2(
            &auto_key
        ));
        assert!(super::direct_reqwest_client_cache_key_uses_http2(&h2c_key));
    }

    #[test]
    fn direct_reqwest_stream_http_mode_parser_defaults_to_http1() {
        assert_eq!(
            super::parse_direct_reqwest_stream_http_mode(""),
            super::DirectReqwestStreamHttpMode::Http1
        );
        assert_eq!(
            super::parse_direct_reqwest_stream_http_mode("http1_only"),
            super::DirectReqwestStreamHttpMode::Http1
        );
        assert_eq!(
            super::parse_direct_reqwest_stream_http_mode("auto"),
            super::DirectReqwestStreamHttpMode::Auto
        );
    }

    #[test]
    fn direct_reqwest_stream_http1_default_preserves_explicit_h2c_profile() {
        let mut plan = ExecutionPlan {
            request_id: "req-h2c-controls".into(),
            candidate_id: None,
            provider_name: Some("mock".into()),
            provider_id: "provider-1".into(),
            endpoint_id: "endpoint-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "http://127.0.0.1:18184/v1/chat/completions".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("mock-model".into()),
            proxy: None,
            transport_profile: Some(ResolvedTransportProfile {
                profile_id: "mock-h2c".into(),
                backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
                http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
                pool_scope: "key".into(),
                header_fingerprint: None,
                extra: None,
            }),
            timeouts: None,
        };

        let controls = super::direct_reqwest_effective_transport_controls(
            &plan,
            ExecutionTransportControls::default(),
        );
        assert!(!controls.http1_only);

        plan.transport_profile = None;
        if super::direct_reqwest_stream_http_mode() == super::DirectReqwestStreamHttpMode::Http1 {
            let controls = super::direct_reqwest_effective_transport_controls(
                &plan,
                ExecutionTransportControls::default(),
            );
            assert!(controls.http1_only);
        }
    }

    #[test]
    fn direct_reqwest_h2_client_shards_scale_from_target_gate() {
        assert_eq!(
            super::direct_reqwest_h2_client_shards_from_config(None, 12_000, 64),
            188
        );
        assert_eq!(
            super::direct_reqwest_h2_client_shards_from_config(None, 2_000, 64),
            32
        );
        assert_eq!(
            super::direct_reqwest_h2_client_shards_from_config(Some(4), 12_000, 64),
            4
        );
        assert_eq!(
            super::direct_reqwest_h2_client_shards_from_config(None, 200_000, 100),
            2_000
        );
    }

    #[test]
    fn direct_reqwest_http1_client_shards_scale_from_target_gate() {
        assert_eq!(
            super::direct_reqwest_client_shards_from_config(None, 10_000, 512),
            20
        );
        assert_eq!(
            super::direct_reqwest_client_shards_from_config(None, 2_000, 512),
            4
        );
        assert_eq!(
            super::direct_reqwest_client_shards_from_config(Some(8), 10_000, 512),
            8
        );
    }

    #[test]
    fn direct_h2c_client_shards_respect_explicit_env() {
        let _guard = direct_reqwest_env_lock();
        let _shards = set_test_env_var(super::DIRECT_H2C_CLIENT_SHARDS_ENV, "7");
        assert_eq!(super::direct_h2c_client_shard_count(), 7);
    }

    #[test]
    fn direct_h2c_prewarm_urls_parse_env_list() {
        let _guard = direct_reqwest_env_lock();
        let _urls = set_test_env_var(
            super::DIRECT_H2C_PREWARM_URLS_ENV,
            " http://127.0.0.1:18184/v1/chat/completions,;http://127.0.0.1:18185/v1/chat/completions\nhttp://127.0.0.1:18186/v1/chat/completions ",
        );

        assert_eq!(
            super::direct_h2c_prewarm_urls_from_env(),
            vec![
                "http://127.0.0.1:18184/v1/chat/completions".to_string(),
                "http://127.0.0.1:18185/v1/chat/completions".to_string(),
                "http://127.0.0.1:18186/v1/chat/completions".to_string(),
            ]
        );
    }

    #[test]
    fn direct_h2c_prewarm_cache_keys_dedup_by_origin() {
        let _guard = direct_reqwest_env_lock();
        let urls = vec![
            "http://127.0.0.1:18184/v1/chat/completions".to_string(),
            "http://127.0.0.1:18184/v1/responses".to_string(),
            "http://127.0.0.1:18185/v1/chat/completions".to_string(),
            "not-a-url".to_string(),
        ];

        let (keys, failures, first_error) =
            super::direct_h2c_sender_prewarm_cache_keys(&urls, None);

        assert_eq!(failures, 1);
        assert!(first_error
            .as_deref()
            .is_some_and(|err| err.contains("invalid h2c upstream origin")));
        assert_eq!(keys.len(), 2);
        assert!(keys
            .iter()
            .any(|key| key.upstream_origin == "http://127.0.0.1:18184"));
        assert!(keys
            .iter()
            .any(|key| key.upstream_origin == "http://127.0.0.1:18185"));
    }

    #[test]
    fn direct_h2c_client_cache_splits_by_origin_and_shards() {
        let _guard = direct_reqwest_env_lock();
        let _shards = set_test_env_var(super::DIRECT_H2C_CLIENT_SHARDS_ENV, "3");
        super::DIRECT_H2C_CLIENT_CACHE
            .lock()
            .expect("h2c cache lock")
            .clear();

        let left =
            super::cached_direct_h2c_client("http://127.0.0.1:18184/v1/chat/completions", None)
                .expect("left client");
        let right =
            super::cached_direct_h2c_client("http://127.0.0.1:18185/v1/chat/completions", None)
                .expect("right client");
        drop((left, right));

        let cache = super::DIRECT_H2C_CLIENT_CACHE
            .lock()
            .expect("h2c cache lock");
        assert_eq!(cache.len(), 2);
        assert!(cache.values().all(|entry| entry.len() == 3));
        assert!(cache.values().all(|entry| entry.target_len == 3));
    }

    #[test]
    fn direct_reqwest_initial_client_shards_are_bounded_by_target() {
        let _guard = direct_reqwest_env_lock();
        assert_eq!(super::direct_reqwest_initial_client_shard_count(1), 1);
        assert_eq!(super::direct_reqwest_initial_client_shard_count(2), 2);
        assert_eq!(
            super::direct_reqwest_initial_client_shard_count(21),
            super::DEFAULT_DIRECT_REQWEST_SYNC_WARM_CLIENTS
        );
    }

    #[test]
    fn direct_reqwest_initial_client_shards_cap_large_sync_env() {
        let _guard = direct_reqwest_env_lock();
        let _sync = set_test_env_var(super::DIRECT_REQWEST_SYNC_WARM_CLIENTS_ENV, "128");
        assert_eq!(
            super::direct_reqwest_initial_client_shard_count(128),
            super::MAX_DIRECT_REQWEST_SYNC_WARM_CLIENTS
        );
    }

    #[test]
    fn direct_reqwest_prewarm_client_shards_default_to_initial() {
        let _guard = direct_reqwest_env_lock();
        assert_eq!(super::direct_reqwest_prewarm_client_shard_count(1), 1);
        assert_eq!(
            super::direct_reqwest_prewarm_client_shard_count(96),
            super::direct_reqwest_initial_client_shard_count(96)
        );
    }

    #[test]
    fn direct_reqwest_prewarm_client_shards_do_not_exceed_request_path_cap() {
        let _guard = direct_reqwest_env_lock();
        let _sync = set_test_env_var(super::DIRECT_REQWEST_SYNC_WARM_CLIENTS_ENV, "4");
        let _prewarm = set_test_env_var(super::DIRECT_REQWEST_PREWARM_SYNC_CLIENTS_ENV, "128");

        assert_eq!(super::direct_reqwest_prewarm_client_shard_count(128), 4);
    }

    #[test]
    fn direct_reqwest_prewarm_populates_cache_for_plan() {
        let _guard = direct_reqwest_env_lock();
        let _shards = set_test_env_var(super::DIRECT_REQWEST_H2_CLIENT_SHARDS_ENV, "4");
        let profile = ResolvedTransportProfile {
            profile_id: "mock-h2c-prewarm".into(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
            http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };
        let plan = ExecutionPlan {
            request_id: "req-prewarm".into(),
            candidate_id: Some("candidate-prewarm".into()),
            provider_name: Some("mock".into()),
            provider_id: "provider-1".into(),
            endpoint_id: "endpoint-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "http://127.0.0.1:18184/v1/chat/completions".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("mock-model".into()),
            proxy: None,
            transport_profile: Some(profile.clone()),
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };

        assert!(
            super::try_prewarm_direct_reqwest_client_cache_for_plan(&plan)
                .expect("prewarm should succeed")
        );

        let cache_key = super::direct_reqwest_client_cache_key(
            &plan.url,
            plan.timeouts.as_ref(),
            None,
            Some(&profile),
            super::ExecutionTransportControls::default(),
        );
        let target_len = super::direct_reqwest_client_shard_count(&cache_key);
        let cache = super::DIRECT_REQWEST_CLIENT_CACHE
            .lock()
            .expect("cache lock");
        let entry = cache.get(&cache_key).expect("cache entry");
        assert_eq!(
            entry.len(),
            super::direct_reqwest_prewarm_client_shard_count(target_len)
        );
        assert_eq!(entry.target_len, target_len);
    }

    #[test]
    fn direct_reqwest_prewarm_plan_keeps_large_sync_env_off_request_path() {
        let _guard = direct_reqwest_env_lock();
        let _shards = set_test_env_var(super::DIRECT_REQWEST_H2_CLIENT_SHARDS_ENV, "128");
        let _sync = set_test_env_var(super::DIRECT_REQWEST_SYNC_WARM_CLIENTS_ENV, "4");
        let _prewarm = set_test_env_var(super::DIRECT_REQWEST_PREWARM_SYNC_CLIENTS_ENV, "128");
        let profile = ResolvedTransportProfile {
            profile_id: "mock-h2c-large-prewarm".into(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
            http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };
        let plan = ExecutionPlan {
            request_id: "req-large-prewarm".into(),
            candidate_id: Some("candidate-large-prewarm".into()),
            provider_name: Some("mock".into()),
            provider_id: "provider-1".into(),
            endpoint_id: "endpoint-large-prewarm".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "http://127.0.0.1:18184/v1/chat/completions".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("mock-model".into()),
            proxy: None,
            transport_profile: Some(profile.clone()),
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };

        assert!(
            super::try_prewarm_direct_reqwest_client_cache_for_plan(&plan)
                .expect("prewarm should succeed")
        );

        let cache_key = super::direct_reqwest_client_cache_key(
            &plan.url,
            plan.timeouts.as_ref(),
            None,
            Some(&profile),
            super::ExecutionTransportControls::default(),
        );
        let cache = super::DIRECT_REQWEST_CLIENT_CACHE
            .lock()
            .expect("cache lock");
        let entry = cache.get(&cache_key).expect("cache entry");
        assert_eq!(entry.len(), 4);
        assert_eq!(entry.target_len, 128);
    }

    #[test]
    fn direct_reqwest_prewarm_skips_h2c_fast_path() {
        let _guard = direct_reqwest_env_lock();
        let _fast_path = set_test_env_var(super::DIRECT_H2C_FAST_PATH_ENV, "1");
        let profile = ResolvedTransportProfile {
            profile_id: "mock-h2c-fast-path-prewarm-skip".into(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
            http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };
        let plan = ExecutionPlan {
            request_id: "req-h2c-fast-path-prewarm-skip".into(),
            candidate_id: Some("candidate-h2c-fast-path-prewarm-skip".into()),
            provider_name: Some("mock".into()),
            provider_id: "provider-1".into(),
            endpoint_id: "endpoint-h2c-fast-path-prewarm-skip".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "http://127.0.0.1:18184/v1/chat/completions".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("mock-model".into()),
            proxy: None,
            transport_profile: Some(profile.clone()),
            timeouts: None,
        };

        assert!(
            !super::try_prewarm_direct_reqwest_client_cache_for_plan(&plan)
                .expect("prewarm skip should succeed")
        );

        let cache_key = super::direct_reqwest_client_cache_key(
            &plan.url,
            plan.timeouts.as_ref(),
            None,
            Some(&profile),
            super::ExecutionTransportControls::default(),
        );
        let cache = super::DIRECT_REQWEST_CLIENT_CACHE
            .lock()
            .expect("cache lock");
        assert!(!cache.contains_key(&cache_key));
    }

    #[test]
    fn direct_reqwest_cache_metrics_expose_ready_state() {
        let _guard = direct_reqwest_env_lock();
        let _shards = set_test_env_var(super::DIRECT_REQWEST_H2_CLIENT_SHARDS_ENV, "1");
        let profile = ResolvedTransportProfile {
            profile_id: "mock-h2c-ready-metrics".into(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
            http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };
        let plan = ExecutionPlan {
            request_id: "req-ready-metrics".into(),
            candidate_id: Some("candidate-ready-metrics".into()),
            provider_name: Some("mock".into()),
            provider_id: "provider-1".into(),
            endpoint_id: "endpoint-ready-metrics".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "http://127.0.0.1:18184/v1/chat/completions".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("mock-model".into()),
            proxy: None,
            transport_profile: Some(profile),
            timeouts: None,
        };

        super::try_prewarm_direct_reqwest_client_cache_for_plan(&plan)
            .expect("prewarm should succeed");

        let samples = super::direct_reqwest_client_cache_metric_samples();
        assert!(samples
            .iter()
            .any(|sample| sample.name == "direct_reqwest_client_cache_ready_entries"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "direct_reqwest_client_cache_pending_clients"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "direct_reqwest_client_cache_warming_entries"));
    }

    #[test]
    fn direct_reqwest_prewarm_skips_browser_transport() {
        let plan = ExecutionPlan {
            request_id: "req-browser".into(),
            candidate_id: None,
            provider_name: Some("browser".into()),
            provider_id: "provider-1".into(),
            endpoint_id: "endpoint-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/v1/chat/completions".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("mock-model".into()),
            proxy: None,
            transport_profile: Some(ResolvedTransportProfile {
                profile_id: "chrome_136".into(),
                backend: TRANSPORT_BACKEND_BROWSER_WREQ.into(),
                http_mode: "auto".into(),
                pool_scope: "key".into(),
                header_fingerprint: None,
                extra: None,
            }),
            timeouts: None,
        };

        assert!(
            !super::try_prewarm_direct_reqwest_client_cache_for_plan(&plan)
                .expect("browser transport should skip prewarm")
        );
    }

    #[test]
    fn direct_sync_execution_runtime_strips_accept_invalid_certs_control_header() {
        let headers = BTreeMap::from([
            ("content-type".into(), "application/json".into()),
            (
                "x-aether-execution-accept-invalid-certs".into(),
                "true".into(),
            ),
        ]);

        let controls = resolve_execution_transport_controls(&headers);
        assert!(controls.accept_invalid_certs);

        let forwarded = build_request_headers(&headers, None, false)
            .expect("headers should build after stripping internal controls");
        assert!(forwarded.get("content-type").is_some());
        assert!(forwarded
            .get("x-aether-execution-accept-invalid-certs")
            .is_none());
    }

    #[test]
    fn tunnel_request_meta_uses_total_timeout_for_non_stream_requests() {
        let plan = tunnel_timeout_plan(false);
        let meta = build_direct_tunnel_request_meta(
            &plan,
            &reqwest::header::HeaderMap::new(),
            ExecutionTransportControls::default(),
        );

        assert!(!meta.stream);
        assert_eq!(meta.request_timeout_ms, Some(90_000));
        assert_eq!(meta.stream_first_byte_timeout_ms, Some(12_345));
        assert_eq!(meta.timeout, 90);
    }

    #[test]
    fn tunnel_request_meta_uses_first_byte_timeout_for_stream_requests() {
        let plan = tunnel_timeout_plan(true);
        let meta = build_direct_tunnel_request_meta(
            &plan,
            &reqwest::header::HeaderMap::new(),
            ExecutionTransportControls::default(),
        );

        assert!(meta.stream);
        assert_eq!(meta.request_timeout_ms, None);
        assert_eq!(meta.stream_first_byte_timeout_ms, Some(12_345));
        assert_eq!(meta.timeout, 13);
    }

    #[test]
    fn stream_first_byte_timeout_uses_default_when_unconfigured() {
        let mut plan = tunnel_timeout_plan(true);
        plan.timeouts = None;

        let timeout = resolve_stream_first_byte_timeout(&plan)
            .expect("stream plans should have a first-byte default");
        let meta = build_direct_tunnel_request_meta(
            &plan,
            &reqwest::header::HeaderMap::new(),
            ExecutionTransportControls::default(),
        );

        assert_eq!(timeout, std::time::Duration::from_millis(30_000));
        assert_eq!(meta.request_timeout_ms, None);
        assert_eq!(meta.stream_first_byte_timeout_ms, Some(30_000));
        assert_eq!(meta.timeout, 30);
    }

    #[test]
    fn stream_first_byte_timeout_ignores_total_timeout() {
        let mut plan = tunnel_timeout_plan(true);
        plan.timeouts = Some(ExecutionTimeouts {
            total_ms: Some(90_000),
            ..ExecutionTimeouts::default()
        });

        let timeout = resolve_stream_first_byte_timeout(&plan)
            .expect("stream plans should have a first-byte default");
        let meta = build_direct_tunnel_request_meta(
            &plan,
            &reqwest::header::HeaderMap::new(),
            ExecutionTransportControls::default(),
        );

        assert_eq!(timeout, std::time::Duration::from_millis(30_000));
        assert_eq!(meta.request_timeout_ms, None);
        assert_eq!(meta.stream_first_byte_timeout_ms, Some(30_000));
        assert_eq!(meta.timeout, 30);
    }

    #[test]
    fn non_stream_total_timeout_defaults_to_provider_request_timeout() {
        let mut plan = tunnel_timeout_plan(false);
        plan.timeouts = None;

        let timeout = resolve_non_stream_total_timeout(&plan)
            .expect("non-stream plans should have a default total timeout");

        assert_eq!(timeout, std::time::Duration::from_secs(300));
    }

    #[test]
    fn tunnel_request_meta_uses_non_stream_default_instead_of_first_byte_default() {
        let mut plan = tunnel_timeout_plan(false);
        plan.timeouts = Some(ExecutionTimeouts {
            first_byte_ms: Some(30_000),
            ..ExecutionTimeouts::default()
        });
        let meta = build_direct_tunnel_request_meta(
            &plan,
            &reqwest::header::HeaderMap::new(),
            ExecutionTransportControls::default(),
        );

        assert!(!meta.stream);
        assert_eq!(meta.request_timeout_ms, Some(300_000));
        assert_eq!(meta.stream_first_byte_timeout_ms, Some(30_000));
        assert_eq!(meta.timeout, 300);
    }

    fn tunnel_timeout_plan(stream: bool) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-timeout".into(),
            candidate_id: None,
            provider_name: Some("provider".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
            stream,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-4.1".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                total_ms: Some(90_000),
                first_byte_ms: Some(12_345),
                ..ExecutionTimeouts::default()
            }),
        }
    }

    fn direct_timeout_plan(
        url: String,
        stream: bool,
        timeouts: ExecutionTimeouts,
    ) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-direct-timeout".into(),
            candidate_id: None,
            provider_name: Some("provider".into()),
            provider_id: "prov-direct-timeout".into(),
            endpoint_id: "ep-direct-timeout".into(),
            key_id: "key-direct-timeout".into(),
            method: "POST".into(),
            url,
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
            stream,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-4.1".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(timeouts),
        }
    }

    fn tunnel_proxy_snapshot(base_url: String) -> ProxySnapshot {
        ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".into()),
            node_id: Some("node-1".into()),
            label: Some("relay-node".into()),
            url: None,
            extra: Some(json!({"tunnel_base_url": base_url})),
        }
    }

    fn manual_proxy_snapshot(node_id: &str) -> ProxySnapshot {
        ProxySnapshot {
            enabled: Some(true),
            mode: Some("http".into()),
            node_id: Some(node_id.to_string()),
            label: Some("manual-proxy".into()),
            url: Some("http://127.0.0.1:1".into()),
            extra: None,
        }
    }

    fn sample_manual_proxy_node(node_id: &str) -> StoredProxyNode {
        StoredProxyNode::new(
            node_id.to_string(),
            "manual-proxy".to_string(),
            "127.0.0.1".to_string(),
            1,
            true,
            "online".to_string(),
            0,
            0,
            0,
            0,
            0,
            0,
            false,
            false,
            0,
        )
        .expect("manual proxy node should build")
        .with_manual_proxy_fields(Some("http://127.0.0.1:1".into()), None, None)
    }

    fn decode_relay_envelope(body: &[u8]) -> (serde_json::Value, Vec<u8>) {
        assert!(
            body.len() >= 4,
            "relay body must contain meta length prefix"
        );
        let meta_len = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
        let meta_end = 4 + meta_len;
        let meta = serde_json::from_slice::<serde_json::Value>(&body[4..meta_end])
            .expect("relay meta should decode");
        (meta, body[meta_end..].to_vec())
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_preserves_upstream_status_and_json_body() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|headers: AxumHeaderMap| async move {
                assert!(
                    !headers.contains_key(EXECUTION_RUNTIME_LOOP_GUARD_HEADER),
                    "plain upstream requests must not leak internal execution loop guard headers"
                );
                assert!(
                    !headers
                        .get_all("via")
                        .iter()
                        .filter_map(|value| value.to_str().ok())
                        .any(|value| value
                            .to_ascii_lowercase()
                            .contains(EXECUTION_RUNTIME_LOOP_GUARD_VIA_TOKEN)),
                    "plain upstream requests must not leak internal execution runtime Via markers"
                );
                (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({"error": {"message": "slow down"}})),
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution_runtime = DirectSyncExecutionRuntime::new();
        let result = execution_runtime
            .execute_sync(&ExecutionPlan {
                request_id: "req-1".into(),
                candidate_id: Some("cand-1".into()),
                provider_name: Some("openai".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: format!("http://{addr}/chat"),
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
                stream: false,
                client_api_format: "openai:chat".into(),
                provider_api_format: "openai:chat".into(),
                model_name: Some("gpt-4.1".into()),
                proxy: None,
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("sync execution should succeed");

        server.abort();

        assert_eq!(result.status_code, 429);
        assert_eq!(
            result.body.and_then(|body| body.json_body),
            Some(json!({"error": {"message": "slow down"}}))
        );
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_applies_non_stream_total_timeout_to_body() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                let body = Body::from_stream(async_stream::stream! {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    yield Ok::<Bytes, std::io::Error>(Bytes::from_static(br#"{"ok":true}"#));
                });
                axum::response::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(body)
                    .expect("response should build")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let result = DirectSyncExecutionRuntime::new()
            .execute_sync(&direct_timeout_plan(
                format!("http://{addr}/chat"),
                false,
                ExecutionTimeouts {
                    total_ms: Some(50),
                    ..ExecutionTimeouts::default()
                },
            ))
            .await;

        server.abort();

        let error = match result {
            Ok(_) => panic!("non-stream body should hit total timeout"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("provider non-stream request total timeout after 50 ms"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_applies_stream_first_byte_timeout_to_body_after_headers()
    {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("client should connect");
            let mut request = [0_u8; 1024];
            let _ = socket
                .read(&mut request)
                .await
                .expect("request should read");
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                )
                .await
                .expect("headers should write");
            socket.flush().await.expect("headers should flush");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let _ = socket.write_all(b"d\r\ndata: hello\n\n\r\n0\r\n\r\n").await;
        });

        let result = DirectSyncExecutionRuntime::new()
            .execute_sync(&direct_timeout_plan(
                format!("http://{addr}/chat"),
                true,
                ExecutionTimeouts {
                    first_byte_ms: Some(50),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                },
            ))
            .await;

        server.abort();

        let error = match result {
            Ok(_) => panic!("stream sync body should hit first-byte timeout"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("provider stream first byte timeout after 50 ms"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_does_not_apply_total_timeout_after_stream_body_starts() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("client should connect");
            let mut request = [0_u8; 1024];
            let _ = socket
                .read(&mut request)
                .await
                .expect("request should read");
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
                )
                .await
                .expect("headers should write");
            socket
                .write_all(b"b\r\ndata: one\n\n\r\n")
                .await
                .expect("first chunk should write");
            socket.flush().await.expect("first chunk should flush");
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            socket
                .write_all(b"b\r\ndata: two\n\n\r\n0\r\n\r\n")
                .await
                .expect("second chunk should write");
        });

        let result = DirectSyncExecutionRuntime::new()
            .execute_sync(&direct_timeout_plan(
                format!("http://{addr}/chat"),
                true,
                ExecutionTimeouts {
                    first_byte_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    total_ms: Some(25),
                    ..ExecutionTimeouts::default()
                },
            ))
            .await
            .expect("stream body should not use total timeout after first chunk");

        server.abort();

        let body = result
            .body
            .and_then(|body| body.body_bytes_b64)
            .and_then(|body| base64::engine::general_purpose::STANDARD.decode(body).ok())
            .expect("stream body should be captured as bytes");
        let body = String::from_utf8(body).expect("stream body should be utf8");
        assert!(body.contains("data: one"));
        assert!(body.contains("data: two"));
    }

    #[tokio::test]
    async fn direct_stream_execution_runtime_applies_first_byte_timeout() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                axum::response::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(Bytes::from_static(b"data: {}\n\n")))
                    .expect("response should build")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let result = DirectSyncExecutionRuntime::new()
            .execute_stream(&direct_timeout_plan(
                format!("http://{addr}/chat"),
                true,
                ExecutionTimeouts {
                    first_byte_ms: Some(50),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                },
            ))
            .await;

        server.abort();

        let error = match result {
            Ok(_) => panic!("stream should hit first-byte timeout"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("provider stream first byte timeout after 50 ms"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test]
    async fn direct_stream_execution_runtime_prefers_first_byte_timeout_over_total_timeout() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                axum::response::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(Bytes::from_static(b"data: {}\n\n")))
                    .expect("response should build")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution = DirectSyncExecutionRuntime::new()
            .execute_stream(&direct_timeout_plan(
                format!("http://{addr}/chat"),
                true,
                ExecutionTimeouts {
                    first_byte_ms: Some(250),
                    total_ms: Some(25),
                    ..ExecutionTimeouts::default()
                },
            ))
            .await
            .expect("stream should use first-byte timeout instead of total timeout");

        server.abort();

        assert_eq!(execution.status_code, http::StatusCode::OK.as_u16());
    }

    #[tokio::test]
    async fn direct_stream_execution_runtime_ignores_total_timeout_when_first_byte_unset() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                axum::response::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(Bytes::from_static(b"data: {}\n\n")))
                    .expect("response should build")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution = DirectSyncExecutionRuntime::new()
            .execute_stream(&direct_timeout_plan(
                format!("http://{addr}/chat"),
                true,
                ExecutionTimeouts {
                    total_ms: Some(5),
                    ..ExecutionTimeouts::default()
                },
            ))
            .await
            .expect("stream should ignore total_ms and use the first-byte default");

        server.abort();

        assert_eq!(execution.status_code, http::StatusCode::OK.as_u16());
    }

    #[tokio::test]
    async fn browser_wreq_stream_execution_ignores_total_timeout_when_first_byte_unset() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                axum::response::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(Bytes::from_static(b"data: {}\n\n")))
                    .expect("response should build")
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });
        let mut plan = direct_timeout_plan(
            format!("http://{addr}/chat"),
            true,
            ExecutionTimeouts {
                total_ms: Some(5),
                ..ExecutionTimeouts::default()
            },
        );
        plan.transport_profile = Some(ResolvedTransportProfile {
            profile_id: "chrome136".into(),
            backend: TRANSPORT_BACKEND_BROWSER_WREQ.into(),
            http_mode: "auto".into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: Some(json!({
                "browser_profile": "chrome136"
            })),
        });

        let execution = DirectSyncExecutionRuntime::new()
            .execute_stream(&plan)
            .await
            .expect("browser-wreq stream should ignore total_ms and use the first-byte default");

        server.abort();

        assert_eq!(execution.status_code, http::StatusCode::OK.as_u16());
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_routes_browser_wreq_transport_in_process() {
        async fn browser_upstream(headers: AxumHeaderMap, body: Bytes) -> axum::response::Response {
            assert_eq!(
                headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok()),
                Some("application/json")
            );
            assert!(
                headers
                    .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                    .is_none(),
                "internal execution control headers must not leak upstream"
            );
            assert_eq!(body.as_ref(), br#"{"modelName":"auto"}"#);
            axum::response::Response::builder()
                .status(http::StatusCode::ACCEPTED)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "ok": true,
                        "via": "browser_wreq"
                    })
                    .to_string(),
                ))
                .expect("response should build")
        }

        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route("/request", any(browser_upstream));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let plan = ExecutionPlan {
            request_id: "req-browser-wreq".into(),
            candidate_id: None,
            provider_name: Some("grok".into()),
            provider_id: "provider-1".into(),
            endpoint_id: "endpoint-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: format!("http://{addr}/request"),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                (
                    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.into(),
                    "true".into(),
                ),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"modelName":"auto"})),
            stream: false,
            client_api_format: "openai:responses".into(),
            provider_api_format: "grok:rate_limits".into(),
            model_name: Some("grok-quota".into()),
            proxy: None,
            transport_profile: Some(ResolvedTransportProfile {
                profile_id: "chrome136".into(),
                backend: TRANSPORT_BACKEND_BROWSER_WREQ.into(),
                http_mode: "auto".into(),
                pool_scope: "key".into(),
                header_fingerprint: None,
                extra: Some(json!({
                    "browser_profile": "chrome136"
                })),
            }),
            timeouts: Some(ExecutionTimeouts {
                total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                ..ExecutionTimeouts::default()
            }),
        };

        let result = DirectSyncExecutionRuntime::new()
            .execute_sync(&plan)
            .await
            .expect("browser wreq transport plan should execute in-process");

        server.abort();

        assert_eq!(result.status_code, http::StatusCode::ACCEPTED.as_u16());
        assert_eq!(
            result
                .body
                .and_then(|body| body.json_body)
                .and_then(|body| body.get("via").cloned()),
            Some(json!("browser_wreq"))
        );
    }

    #[test]
    fn browser_wreq_transport_rejects_unknown_profile() {
        let profile = ResolvedTransportProfile {
            profile_id: "firefox999".into(),
            backend: TRANSPORT_BACKEND_BROWSER_WREQ.into(),
            http_mode: "auto".into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };

        let error = match build_browser_wreq_client(
            None,
            None,
            &profile,
            ExecutionTransportControls::default(),
            true,
        ) {
            Ok(_) => panic!("unknown browser profile should fail loudly"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            ExecutionRuntimeTransportError::UnsupportedTransportProfile(backend)
                if backend == "browser_wreq:firefox999"
        ));
    }

    #[tokio::test]
    async fn execute_sync_plan_routes_grok_marker_through_grok_runtime() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/rest/app-chat/conversations/new",
            post(|body: Bytes| async move {
                let body_json: serde_json::Value =
                    serde_json::from_slice(&body).expect("request body should be json");
                if body_json.get("message").and_then(serde_json::Value::as_str)
                    != Some("[user]: hello")
                {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": {
                                "message": "expected grok app-chat message",
                                "body": body_json,
                            }
                        })),
                    );
                }
                (
                    axum::http::StatusCode::OK,
                    Json(json!({
                        "result": {
                            "response": {
                                "token": "pong",
                                "messageTag": "final"
                            }
                        }
                    })),
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });
        let plan = ExecutionPlan {
            request_id: "req-grok-runtime".into(),
            candidate_id: Some("cand-grok".into()),
            provider_name: Some("grok".into()),
            provider_id: "provider-grok".into(),
            endpoint_id: "endpoint-grok".into(),
            key_id: "key-grok".into(),
            method: "POST".into(),
            url: format!("http://{addr}/rest/app-chat/conversations/new"),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                (
                    aether_provider_transport::GROK_INTERNAL_HEADER.into(),
                    "1".into(),
                ),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "grok-4.20-0309-non-reasoning",
                "messages": [{"role": "user", "content": "hello"}],
            })),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("grok-4.20-0309-non-reasoning".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                ..ExecutionTimeouts::default()
            }),
        };
        let report_context = json!({"mapped_model": "grok-4.20-fast"});

        let result = super::super::grok::maybe_execute_grok_sync(&plan, Some(&report_context))
            .await
            .expect("grok runtime plan should execute")
            .expect("grok runtime should handle marked plan");

        server.abort();

        assert_eq!(result.status_code, http::StatusCode::OK.as_u16());
        assert_eq!(
            result
                .body
                .and_then(|body| body.json_body)
                .and_then(|body| body["choices"][0]["message"]["content"]
                    .as_str()
                    .map(str::to_string)),
            Some("pong".to_string())
        );
    }

    #[tokio::test]
    async fn execute_sync_plan_records_manual_proxy_success() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
            sample_manual_proxy_node("manual-node-1"),
        ]));
        let data = crate::data::GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(
            &repository,
        ));
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data);
        let plan = ExecutionPlan {
            request_id: "req-manual-proxy-success".into(),
            candidate_id: None,
            provider_name: None,
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody::from_json(json!({})),
            stream: false,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: None,
            proxy: Some(manual_proxy_snapshot("manual-node-1")),
            transport_profile: None,
            timeouts: None,
        };

        record_manual_proxy_request_success(&state, &plan).await;

        let node = repository
            .find_proxy_node("manual-node-1")
            .await
            .expect("proxy node lookup should succeed")
            .expect("manual proxy node should exist");
        assert_eq!(node.total_requests, 1);
        assert_eq!(node.failed_requests, 0);
    }

    #[tokio::test]
    async fn execute_sync_plan_records_manual_proxy_failure() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
            sample_manual_proxy_node("manual-node-1"),
        ]));
        let data = crate::data::GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(
            &repository,
        ));
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data);
        let plan = ExecutionPlan {
            request_id: "req-manual-proxy-failure".into(),
            candidate_id: None,
            provider_name: None,
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody::from_json(json!({})),
            stream: false,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: None,
            proxy: Some(manual_proxy_snapshot("manual-node-1")),
            transport_profile: None,
            timeouts: None,
        };

        record_manual_proxy_request_failure(&state, &plan).await;

        let node = repository
            .find_proxy_node("manual-node-1")
            .await
            .expect("proxy node lookup should succeed")
            .expect("manual proxy node should exist");
        assert_eq!(node.total_requests, 1);
        assert_eq!(node.failed_requests, 1);
    }

    #[tokio::test]
    async fn execute_sync_plan_records_manual_proxy_http_error_as_failure() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
            sample_manual_proxy_node("manual-node-1"),
        ]));
        let data = crate::data::GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(
            &repository,
        ));
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data);
        let plan = ExecutionPlan {
            request_id: "req-manual-proxy-http-error".into(),
            candidate_id: None,
            provider_name: None,
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody::from_json(json!({})),
            stream: false,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: None,
            proxy: Some(manual_proxy_snapshot("manual-node-1")),
            transport_profile: None,
            timeouts: None,
        };

        record_manual_proxy_request_outcome(&state, &plan, 429).await;

        let node = repository
            .find_proxy_node("manual-node-1")
            .await
            .expect("proxy node lookup should succeed")
            .expect("manual proxy node should exist");
        assert_eq!(node.total_requests, 1);
        assert_eq!(node.failed_requests, 1);
    }

    #[tokio::test]
    async fn execute_sync_plan_records_manual_proxy_http_success_without_failure() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
            sample_manual_proxy_node("manual-node-1"),
        ]));
        let data = crate::data::GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(
            &repository,
        ));
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data);
        let plan = ExecutionPlan {
            request_id: "req-manual-proxy-http-success".into(),
            candidate_id: None,
            provider_name: None,
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody::from_json(json!({})),
            stream: false,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: None,
            proxy: Some(manual_proxy_snapshot("manual-node-1")),
            transport_profile: None,
            timeouts: None,
        };

        record_manual_proxy_request_outcome(&state, &plan, 200).await;

        let node = repository
            .find_proxy_node("manual-node-1")
            .await
            .expect("proxy node lookup should succeed")
            .expect("manual proxy node should exist");
        assert_eq!(node.total_requests, 1);
        assert_eq!(node.failed_requests, 0);
    }

    #[tokio::test]
    async fn execute_sync_plan_records_manual_proxy_stream_error_without_extra_request_count() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
            sample_manual_proxy_node("manual-node-1"),
        ]));
        let data = crate::data::GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(
            &repository,
        ));
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data);
        let plan = ExecutionPlan {
            request_id: "req-manual-proxy-stream-error".into(),
            candidate_id: None,
            provider_name: None,
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody::from_json(json!({})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: None,
            proxy: Some(manual_proxy_snapshot("manual-node-1")),
            transport_profile: None,
            timeouts: None,
        };

        record_manual_proxy_request_success(&state, &plan).await;
        record_manual_proxy_stream_error(&state, &plan).await;

        let node = repository
            .find_proxy_node("manual-node-1")
            .await
            .expect("proxy node lookup should succeed")
            .expect("manual proxy node should exist");
        assert_eq!(node.total_requests, 1);
        assert_eq!(node.failed_requests, 0);
        assert_eq!(node.stream_errors, 1);
    }

    #[tokio::test]
    async fn execute_sync_plan_ignores_stream_error_for_tunnel_proxy() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
            sample_manual_proxy_node("manual-node-1"),
        ]));
        let data = crate::data::GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(
            &repository,
        ));
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data);
        let plan = ExecutionPlan {
            request_id: "req-tunnel-proxy-stream-error".into(),
            candidate_id: None,
            provider_name: None,
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody::from_json(json!({})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: None,
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            transport_profile: None,
            timeouts: None,
        };

        record_manual_proxy_stream_error(&state, &plan).await;

        let node = repository
            .find_proxy_node("manual-node-1")
            .await
            .expect("proxy node lookup should succeed")
            .expect("manual proxy node should exist");
        assert_eq!(node.total_requests, 0);
        assert_eq!(node.failed_requests, 0);
        assert_eq!(node.stream_errors, 0);
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_supports_tunnel_relay() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/api/internal/tunnel/relay/{node_id}",
            post(|Path(node_id): Path<String>, body: Bytes| async move {
                let (meta, request_body) = decode_relay_envelope(&body);
                assert_eq!(node_id, "node-1");
                assert_eq!(meta["method"], "POST");
                assert_eq!(meta["url"], "https://example.com/chat");
                let headers = meta["headers"]
                    .as_object()
                    .expect("relay meta headers should be an object");
                assert!(
                    !headers.contains_key(EXECUTION_RUNTIME_LOOP_GUARD_HEADER),
                    "tunnel relay metadata must not leak internal execution loop guard headers"
                );
                let via = headers
                    .get("via")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                assert!(
                    !via.to_ascii_lowercase()
                        .contains(EXECUTION_RUNTIME_LOOP_GUARD_VIA_TOKEN),
                    "tunnel relay metadata must not leak internal execution runtime Via markers"
                );
                let request_json: serde_json::Value =
                    serde_json::from_slice(&request_body).expect("request body should be json");
                assert_eq!(request_json["model"], "gpt-4.1");
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"tunnel": true, "node_id": node_id})),
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("relay test server should run");
        });

        let execution_runtime = DirectSyncExecutionRuntime::new();
        let result = execution_runtime
            .execute_sync(&ExecutionPlan {
                request_id: "req-1".into(),
                candidate_id: None,
                provider_name: None,
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: "https://example.com/chat".into(),
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
                stream: false,
                client_api_format: "openai:chat".into(),
                provider_api_format: "openai:chat".into(),
                model_name: Some("gpt-4.1".into()),
                proxy: Some(tunnel_proxy_snapshot(format!("http://{addr}"))),
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("tunnel relay execution should succeed");

        server.abort();

        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.body.and_then(|body| body.json_body),
            Some(json!({"tunnel": true, "node_id": "node-1"}))
        );
    }

    #[tokio::test]
    async fn execute_sync_plan_prefers_local_tunnel_stream_over_http_relay_loopback() {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            701,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        )));

        let plan = ExecutionPlan {
            request_id: "req-local-tunnel-1".into(),
            candidate_id: Some("cand-local-tunnel-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
            stream: false,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-4.1".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                ..ExecutionTimeouts::default()
            }),
        };

        let state_for_task = state.clone();
        let plan_for_task = plan.clone();
        let execution_task = tokio::spawn(async move {
            execute_sync_plan(&state_for_task, Some("trace-local-tunnel"), &plan_for_task).await
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, tunnel_protocol::REQUEST_HEADERS);
        let request_meta_payload =
            tunnel_protocol::decode_payload(&request_headers, &request_header)
                .expect("request meta payload should decode");
        let request_meta =
            serde_json::from_slice::<tunnel_protocol::RequestMeta>(&request_meta_payload)
                .expect("request meta should decode");
        assert_eq!(request_meta.method, "POST");
        assert_eq!(request_meta.url, "https://example.com/chat");

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header = tunnel_protocol::FrameHeader::parse(&request_body)
            .expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, tunnel_protocol::REQUEST_BODY);
        let request_body_payload =
            tunnel_protocol::decode_payload(&request_body, &request_body_header)
                .expect("request body payload should decode");
        let request_json = serde_json::from_slice::<serde_json::Value>(&request_body_payload)
            .expect("request body should decode");
        assert_eq!(request_json["model"], "gpt-4.1");

        let response_meta = tunnel_protocol::ResponseMeta {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(701, &mut response_headers_frame)
            .await;

        let mut response_body_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_BODY,
            0,
            br#"{"local_tunnel":true}"#,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(701, &mut response_body_frame)
            .await;

        let mut response_end_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::STREAM_END,
            0,
            &[],
        );
        tunnel_app
            .hub
            .handle_proxy_frame(701, &mut response_end_frame)
            .await;

        let result = execution_task
            .await
            .expect("execution task should complete")
            .expect("local tunnel execution should succeed");

        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.body.and_then(|body| body.json_body),
            Some(json!({"local_tunnel": true}))
        );
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_disables_redirects_by_default() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new()
            .route(
                "/redirect",
                post(|| async {
                    (
                        axum::http::StatusCode::TEMPORARY_REDIRECT,
                        [(
                            axum::http::header::LOCATION,
                            axum::http::HeaderValue::from_static("/final"),
                        )],
                    )
                }),
            )
            .route(
                "/final",
                post(|| async {
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({"redirected": true})),
                    )
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution_runtime = DirectSyncExecutionRuntime::new();
        let result = execution_runtime
            .execute_sync(&ExecutionPlan {
                request_id: "req-redirect-1".into(),
                candidate_id: None,
                provider_name: Some("provider_ops".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: format!("http://{addr}/redirect"),
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
                stream: false,
                client_api_format: "provider_ops:verify".into(),
                provider_api_format: "provider_ops:verify".into(),
                model_name: Some("verify-auth".into()),
                proxy: None,
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("sync execution should succeed");

        server.abort();

        assert_eq!(result.status_code, 307);
        assert_eq!(
            result.headers.get("location").map(String::as_str),
            Some("/final")
        );
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_follows_redirects_when_explicitly_enabled() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new()
            .route(
                "/redirect",
                post(|| async {
                    (
                        axum::http::StatusCode::TEMPORARY_REDIRECT,
                        [(
                            axum::http::header::LOCATION,
                            axum::http::HeaderValue::from_static("/final"),
                        )],
                    )
                }),
            )
            .route(
                "/final",
                post(|| async {
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({"redirected": true})),
                    )
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution_runtime = DirectSyncExecutionRuntime::new();
        let result = execution_runtime
            .execute_sync(&ExecutionPlan {
                request_id: "req-redirect-2".into(),
                candidate_id: None,
                provider_name: Some("provider_oauth".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: format!("http://{addr}/redirect"),
                headers: BTreeMap::from([
                    ("content-type".into(), "application/json".into()),
                    (
                        EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.into(),
                        "true".into(),
                    ),
                ]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
                stream: false,
                client_api_format: "provider_oauth:exchange".into(),
                provider_api_format: "provider_oauth:exchange".into(),
                model_name: Some("oauth-exchange".into()),
                proxy: None,
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("sync execution should succeed");

        server.abort();

        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.body.and_then(|body| body.json_body),
            Some(json!({"redirected": true}))
        );
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_forwards_http1_only_control_to_tunnel_relay() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/api/internal/tunnel/relay/{node_id}",
            post(|Path(node_id): Path<String>, body: Bytes| async move {
                let (meta, request_body) = decode_relay_envelope(&body);
                assert_eq!(node_id, "node-1");
                assert_eq!(meta["provider_id"], "prov-1");
                assert_eq!(meta["endpoint_id"], "ep-1");
                assert_eq!(meta["key_id"], "key-1");
                assert_eq!(meta["http1_only"], true);
                assert_eq!(meta["follow_redirects"], json!(false));
                assert_eq!(meta["transport_profile"]["profile_id"], "relay-profile");
                let request_json: serde_json::Value =
                    serde_json::from_slice(&request_body).expect("request body should be json");
                assert_eq!(request_json["model"], "gpt-4.1");
                (axum::http::StatusCode::OK, Json(json!({"ok": true})))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("relay test server should run");
        });

        let execution_runtime = DirectSyncExecutionRuntime::new();
        let result = execution_runtime
            .execute_sync(&ExecutionPlan {
                request_id: "req-relay-http1-1".into(),
                candidate_id: None,
                provider_name: Some("provider_ops".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: "https://example.com/chat".into(),
                headers: BTreeMap::from([
                    ("content-type".into(), "application/json".into()),
                    (EXECUTION_REQUEST_HTTP1_ONLY_HEADER.into(), "true".into()),
                    (
                        EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.into(),
                        "false".into(),
                    ),
                ]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
                stream: false,
                client_api_format: "provider_ops:verify".into(),
                provider_api_format: "provider_ops:verify".into(),
                model_name: Some("verify-auth".into()),
                proxy: Some(tunnel_proxy_snapshot(format!("http://{addr}"))),
                transport_profile: Some(ResolvedTransportProfile {
                    profile_id: "relay-profile".into(),
                    backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
                    http_mode: "auto".into(),
                    pool_scope: "key".into(),
                    header_fingerprint: None,
                    extra: None,
                }),
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("tunnel relay execution should succeed");

        server.abort();

        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.body.and_then(|body| body.json_body),
            Some(json!({"ok": true}))
        );
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_allows_transport_profile_best_effort() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"transport_profile": true})),
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution_runtime = DirectSyncExecutionRuntime::new();
        let result = execution_runtime
            .execute_sync(&ExecutionPlan {
                request_id: "req-tls-1".into(),
                candidate_id: Some("cand-1".into()),
                provider_name: Some("claude".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: format!("http://{addr}/chat"),
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(json!({"model": "claude-3.7-sonnet"})),
                stream: false,
                client_api_format: "claude:messages".into(),
                provider_api_format: "claude:messages".into(),
                model_name: Some("claude-3.7-sonnet".into()),
                proxy: None,
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("sync execution with transport profile should succeed");

        server.abort();

        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.body.and_then(|body| body.json_body),
            Some(json!({"transport_profile": true}))
        );
    }

    #[test]
    fn direct_sync_execution_runtime_prepares_h2c_prior_knowledge_profile() {
        let profile = ResolvedTransportProfile {
            profile_id: "mock-h2c".into(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.into(),
            http_mode: TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE.into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };
        let plan = ExecutionPlan {
            request_id: "req-h2c-1".into(),
            candidate_id: Some("cand-h2c-1".into()),
            provider_name: Some("mock".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "http://127.0.0.1:18184/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "mock-model"})),
            stream: false,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("mock-model".into()),
            proxy: None,
            transport_profile: Some(profile.clone()),
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                ..ExecutionTimeouts::default()
            }),
        };

        let transport_controls = super::direct_reqwest_effective_transport_controls(
            &plan,
            super::ExecutionTransportControls::default(),
        );
        let cache_key = super::direct_reqwest_client_cache_key(
            &plan.url,
            plan.timeouts.as_ref(),
            None,
            Some(&profile),
            transport_controls,
        );

        assert!(!transport_controls.http1_only);
        assert!(!super::direct_h2c_fast_path_applies(
            &plan,
            transport_controls
        ));
        assert!(super::direct_reqwest_client_cache_key_uses_http2(
            &cache_key
        ));
        assert!(super::direct_reqwest_client_cache_key_uses_h2c_prior_knowledge(&cache_key));
        assert_eq!(
            cache_key
                .transport_profile
                .as_ref()
                .map(|profile| profile.http_mode.as_str()),
            Some(TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE)
        );
        super::build_direct_reqwest_client_from_cache_key(&cache_key)
            .expect("h2c prior-knowledge client should build");
    }

    #[test]
    fn direct_sync_execution_runtime_rejects_unsupported_transport_backend() {
        let profile = ResolvedTransportProfile {
            profile_id: "chrome-120".into(),
            backend: "utls".into(),
            http_mode: "auto".into(),
            pool_scope: "key".into(),
            header_fingerprint: None,
            extra: None,
        };

        let error = match build_client(
            "https://api.example.test/v1/chat/completions",
            None,
            None,
            Some(&profile),
            ExecutionTransportControls::default(),
        ) {
            Ok(_) => panic!("unsupported backend should fail"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            ExecutionRuntimeTransportError::UnsupportedTransportProfile(backend)
                if backend == "utls"
        ));
    }

    #[test]
    fn connect_json_response_is_not_treated_as_plain_json() {
        let headers = BTreeMap::from([(
            "content-type".to_string(),
            "application/connect+json".to_string(),
        )]);
        let body = [2, 0, 0, 0, 2, b'{', b'}'];

        assert!(!response_body_is_json(&headers, &body));
    }

    #[test]
    fn connect_json_error_response_is_decoded_for_stream_sync_body() {
        let headers = BTreeMap::from([(
            "content-type".to_string(),
            "application/connect+json".to_string(),
        )]);
        let payload = br#"{"error":{"code":"resource_exhausted","message":"quota exhausted"}}"#;
        let mut body_bytes = vec![2];
        body_bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        body_bytes.extend_from_slice(payload);

        let body = build_execution_response_body(&headers, &body_bytes, &body_bytes, true)
            .expect("body should build")
            .expect("body should be present");

        assert_eq!(
            body.json_body
                .as_ref()
                .and_then(|value| value.pointer("/error/code")),
            Some(&json!("resource_exhausted"))
        );
        assert!(body.body_bytes_b64.is_none());
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_compresses_json_body_when_requested() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|headers: axum::http::HeaderMap, body: Bytes| async move {
                let header_encoding = headers
                    .get(axum::http::header::CONTENT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let mut decoder = flate2::read::GzDecoder::new(body.as_ref());
                let mut decoded = String::new();
                decoder
                    .read_to_string(&mut decoded)
                    .expect("gzip body should decode");
                let decoded_json: serde_json::Value =
                    serde_json::from_str(&decoded).expect("decoded json should parse");
                (
                    axum::http::StatusCode::OK,
                    Json(json!({
                        "content_encoding": header_encoding,
                        "body": decoded_json,
                    })),
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution_runtime = DirectSyncExecutionRuntime::new();
        let result = execution_runtime
            .execute_sync(&ExecutionPlan {
                request_id: "req-gzip-1".into(),
                candidate_id: Some("cand-1".into()),
                provider_name: Some("openai".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: format!("http://{addr}/chat"),
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                content_type: Some("application/json".into()),
                content_encoding: Some("gzip".into()),
                body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
                stream: false,
                client_api_format: "openai:chat".into(),
                provider_api_format: "openai:chat".into(),
                model_name: Some("gpt-4.1".into()),
                proxy: None,
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("gzip sync execution should succeed");

        server.abort();

        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.body.and_then(|body| body.json_body),
            Some(json!({
                "content_encoding": "gzip",
                "body": {"model": "gpt-4.1"},
            }))
        );
    }

    #[tokio::test]
    async fn direct_sync_execution_runtime_reports_ttfb_once_upstream_response_starts() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                (axum::http::StatusCode::OK, Json(json!({"ok": true})))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let execution_runtime = DirectSyncExecutionRuntime::new();
        let result = execution_runtime
            .execute_sync(&ExecutionPlan {
                request_id: "req-ttfb-1".into(),
                candidate_id: Some("cand-1".into()),
                provider_name: Some("openai".into()),
                provider_id: "prov-1".into(),
                endpoint_id: "ep-1".into(),
                key_id: "key-1".into(),
                method: "POST".into(),
                url: format!("http://{addr}/chat"),
                headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
                content_type: Some("application/json".into()),
                content_encoding: None,
                body: RequestBody::from_json(json!({"model": "gpt-4.1"})),
                stream: false,
                client_api_format: "openai:chat".into(),
                provider_api_format: "openai:chat".into(),
                model_name: Some("gpt-4.1".into()),
                proxy: None,
                transport_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(LOCAL_HTTP_SUCCESS_TIMEOUT_MS),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("sync execution should succeed");

        server.abort();

        let telemetry = result
            .telemetry
            .expect("sync execution should include telemetry");
        let ttfb_ms = telemetry
            .ttfb_ms
            .expect("sync execution should include ttfb");
        let elapsed_ms = telemetry
            .elapsed_ms
            .expect("sync execution should include elapsed time");
        assert!(ttfb_ms > 0);
        assert!(elapsed_ms >= ttfb_ms);
    }
}
