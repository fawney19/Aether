use std::collections::BTreeMap;
use std::error::Error as _;
use std::io::Read;
use std::io::Write;
use std::time::{Duration, Instant};

use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTelemetry, ProxySnapshot, ResponseBody,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
};
use aether_http::{apply_http_client_config, HttpClientConfig};
use axum::body::Bytes;
use base64::Engine as _;
use flate2::read::{DeflateDecoder, GzDecoder};
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use reqwest::tls::Version;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use thiserror::Error;

#[cfg(test)]
use crate::execution_runtime::remote_compat::execute_sync_plan_via_remote_execution_runtime;
use crate::frontdoor_loop_guard::{
    configured_gateway_frontdoor_base_url, gateway_frontdoor_self_loop_guard_error,
};
use crate::tunnel::{self, tunnel_protocol};
use crate::{AppState, GatewayError};

const HUB_RELAY_CONTENT_TYPE: &str = "application/vnd.aether.tunnel-envelope";
const HUB_RELAY_ERROR_HEADER: &str = "x-aether-tunnel-error";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";
const CLAUDE_CODE_TLS_PROFILE: &str = "claude_code_nodejs";

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
    #[error("failed to encode request body: {0}")]
    BodyEncode(serde_json::Error),
    #[error("failed to build HTTP client: {0}")]
    ClientBuild(reqwest::Error),
    #[error("failed to execute upstream request: {0}")]
    UpstreamRequest(String),
    #[error("hub relay request failed: {0}")]
    RelayError(String),
    #[error("upstream response is not valid JSON: {0}")]
    InvalidJson(serde_json::Error),
}

#[derive(Debug, Serialize)]
struct RelayRequestMeta {
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    timeout: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    http1_only: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DirectSyncExecutionRuntime;

#[derive(Debug, Clone, Copy, Default)]
struct ExecutionTransportControls {
    follow_redirects: Option<bool>,
    http1_only: bool,
}

pub(crate) enum DirectUpstreamResponse {
    Reqwest(reqwest::Response),
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
}

impl DirectSyncExecutionRuntime {
    pub(crate) const fn new() -> Self {
        Self
    }

    pub(crate) async fn execute_sync(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<ExecutionResult, ExecutionRuntimeTransportError> {
        let body_bytes = build_request_body(plan)?;

        let started_at = Instant::now();
        let response = send_request(plan, body_bytes).await?;
        let ttfb_ms = started_at.elapsed().as_millis() as u64;
        let status_code = response.status().as_u16();
        let headers = collect_response_headers(response.headers());
        let body_bytes = response.bytes().await.map_err(|err| {
            ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(&err))
        })?;
        let decoded_body_bytes = decode_response_body_bytes(&headers, &body_bytes)
            .unwrap_or_else(|| body_bytes.to_vec());
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        let upstream_bytes = body_bytes.len() as u64;

        let body = if body_bytes.is_empty() {
            None
        } else if plan.stream {
            Some(ResponseBody {
                json_body: None,
                body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(&body_bytes)),
            })
        } else if response_body_is_json(&headers, &decoded_body_bytes) {
            let body_json: Value = serde_json::from_slice(&decoded_body_bytes)
                .map_err(ExecutionRuntimeTransportError::InvalidJson)?;
            Some(ResponseBody {
                json_body: Some(body_json),
                body_bytes_b64: None,
            })
        } else {
            Some(ResponseBody {
                json_body: None,
                body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(&body_bytes)),
            })
        };

        Ok(ExecutionResult {
            request_id: plan.request_id.clone(),
            candidate_id: plan.candidate_id.clone(),
            status_code,
            headers,
            body,
            telemetry: Some(ExecutionTelemetry {
                ttfb_ms: Some(ttfb_ms),
                elapsed_ms: Some(elapsed_ms),
                upstream_bytes: Some(upstream_bytes),
            }),
            error: None,
        })
    }

    pub(crate) async fn execute_stream(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<DirectUpstreamStreamExecution, ExecutionRuntimeTransportError> {
        if !plan.stream {
            return Err(ExecutionRuntimeTransportError::StreamUnsupported);
        }

        let body_bytes = build_request_body(plan)?;

        let started_at = Instant::now();
        let response = send_request(plan, body_bytes).await?;
        let status_code = response.status().as_u16();
        let headers = collect_response_headers(response.headers());

        let stream_summary_report_context = build_stream_summary_report_context(plan);

        Ok(DirectUpstreamStreamExecution {
            request_id: plan.request_id.clone(),
            candidate_id: plan.candidate_id.clone(),
            status_code,
            headers,
            provider_api_format: plan.provider_api_format.clone(),
            stream_summary_report_context,
            response: DirectUpstreamResponse::Reqwest(response),
            started_at,
        })
    }
}

pub(crate) async fn execute_sync_plan(
    state: &AppState,
    trace_id: Option<&str>,
    plan: &ExecutionPlan,
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

    let _ = state;
    let _ = trace_id;
    DirectSyncExecutionRuntime::new()
        .execute_sync(plan)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))
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
    }))
}

fn build_stream_summary_report_context(plan: &ExecutionPlan) -> Value {
    json!({
        "provider_api_format": plan.provider_api_format,
        "client_api_format": plan.client_api_format,
        "model": plan.model_name,
    })
}

async fn execute_sync_plan_via_local_tunnel(
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
    let mut body_bytes = Vec::new();
    while let Some(chunk) = response
        .next_chunk()
        .await
        .map_err(ExecutionRuntimeTransportError::UpstreamRequest)?
    {
        body_bytes.extend_from_slice(&chunk);
    }
    let decoded_body_bytes =
        decode_response_body_bytes(&headers, &body_bytes).unwrap_or_else(|| body_bytes.clone());
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    let upstream_bytes = body_bytes.len() as u64;

    let body = if body_bytes.is_empty() {
        None
    } else if plan.stream {
        Some(ResponseBody {
            json_body: None,
            body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(&body_bytes)),
        })
    } else if response_body_is_json(&headers, &decoded_body_bytes) {
        let body_json: Value = serde_json::from_slice(&decoded_body_bytes)
            .map_err(ExecutionRuntimeTransportError::InvalidJson)?;
        Some(ResponseBody {
            json_body: Some(body_json),
            body_bytes_b64: None,
        })
    } else {
        Some(ResponseBody {
            json_body: None,
            body_bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(&body_bytes)),
        })
    };

    Ok(ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers,
        body,
        telemetry: Some(ExecutionTelemetry {
            ttfb_ms: Some(ttfb_ms),
            elapsed_ms: Some(elapsed_ms),
            upstream_bytes: Some(upstream_bytes),
        }),
        error: None,
    })
}

fn build_direct_tunnel_request_meta(
    plan: &ExecutionPlan,
    headers: &HeaderMap,
    transport_controls: ExecutionTransportControls,
) -> tunnel_protocol::RequestMeta {
    tunnel_protocol::RequestMeta {
        method: plan.method.clone(),
        url: plan.url.clone(),
        headers: header_map_to_string_map(headers).into_iter().collect(),
        timeout: resolve_relay_timeout_seconds(plan),
        follow_redirects: transport_controls.follow_redirects,
        http1_only: transport_controls.http1_only,
    }
}

async fn send_request(
    plan: &ExecutionPlan,
    body_bytes: Vec<u8>,
) -> Result<reqwest::Response, ExecutionRuntimeTransportError> {
    if let Some(detail) = gateway_frontdoor_self_loop_guard_error(plan.url.as_str()) {
        return Err(ExecutionRuntimeTransportError::UpstreamRequest(detail));
    }

    let method = plan.method.parse::<reqwest::Method>()?;
    let transport_controls = resolve_execution_transport_controls(&plan.headers);
    let headers = build_request_headers(
        &plan.headers,
        plan.content_encoding.as_deref(),
        plan.body.body_bytes_b64.is_some(),
    )?;
    let total_timeout = plan
        .timeouts
        .as_ref()
        .and_then(|timeouts| timeouts.total_ms)
        .map(Duration::from_millis);

    if let Some(node_id) = resolve_tunnel_node_id(plan.proxy.as_ref()) {
        return send_via_tunnel_relay(
            plan,
            method,
            headers,
            body_bytes,
            &node_id,
            total_timeout,
            transport_controls,
        )
        .await;
    }

    let client = build_client(
        plan.timeouts.as_ref(),
        plan.proxy.as_ref(),
        plan.tls_profile.as_deref(),
        transport_controls,
    )?;
    let mut request = client.request(method, &plan.url);
    request = request.headers(headers).body(body_bytes);
    if let Some(timeout) = total_timeout {
        request = request.timeout(timeout);
    }
    request.send().await.map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(&err))
    })
}

async fn send_via_tunnel_relay(
    plan: &ExecutionPlan,
    method: reqwest::Method,
    headers: HeaderMap,
    body_bytes: Vec<u8>,
    node_id: &str,
    total_timeout: Option<Duration>,
    transport_controls: ExecutionTransportControls,
) -> Result<reqwest::Response, ExecutionRuntimeTransportError> {
    let client = build_relay_client(plan.timeouts.as_ref())?;
    let relay_url = build_relay_url(plan.proxy.as_ref(), node_id);
    let envelope = build_relay_envelope(
        RelayRequestMeta {
            method: method.as_str().to_string(),
            url: plan.url.clone(),
            headers: header_map_to_string_map(&headers),
            timeout: resolve_relay_timeout_seconds(plan),
            follow_redirects: transport_controls.follow_redirects,
            http1_only: transport_controls.http1_only,
        },
        &body_bytes,
    )?;

    let mut request = client
        .request(reqwest::Method::POST, relay_url)
        .header(reqwest::header::CONTENT_TYPE, HUB_RELAY_CONTENT_TYPE)
        .body(envelope);
    if let Some(timeout) = total_timeout {
        request = request.timeout(timeout);
    }

    let response = request
        .send()
        .await
        .map_err(|err| ExecutionRuntimeTransportError::RelayError(err.to_string()))?;

    if let Some(kind) = response
        .headers()
        .get(HUB_RELAY_ERROR_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    {
        let message = response
            .text()
            .await
            .unwrap_or_else(|_| format!("hub relay error: {kind}"));
        return Err(ExecutionRuntimeTransportError::RelayError(message));
    }

    Ok(response)
}

fn build_request_body(plan: &ExecutionPlan) -> Result<Vec<u8>, ExecutionRuntimeTransportError> {
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
    let ms = plan
        .timeouts
        .as_ref()
        .and_then(|timeouts| {
            timeouts
                .read_ms
                .or(timeouts.total_ms)
                .or(timeouts.connect_ms)
        })
        .unwrap_or(60_000);
    let secs = ms.div_ceil(1_000);
    secs.clamp(1, 300)
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
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
    proxy: Option<&ProxySnapshot>,
    tls_profile: Option<&str>,
    transport_controls: ExecutionTransportControls,
) -> Result<reqwest::Client, ExecutionRuntimeTransportError> {
    let mut builder = reqwest::Client::builder();
    if transport_controls.follow_redirects != Some(true) {
        builder = builder.redirect(Policy::none());
    }
    if transport_controls.http1_only {
        builder = builder.http1_only();
    }
    let mut builder = apply_http_client_config(
        builder,
        &HttpClientConfig {
            connect_timeout_ms: timeouts.and_then(|timeouts| timeouts.connect_ms),
            ..HttpClientConfig::default()
        },
    );
    builder = apply_tls_profile(builder, tls_profile);
    if let Some(proxy_url) = resolve_proxy_url(proxy)? {
        let proxy = reqwest::Proxy::all(&proxy_url)
            .map_err(ExecutionRuntimeTransportError::InvalidProxy)?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(ExecutionRuntimeTransportError::ClientBuild)
}

fn apply_tls_profile(
    builder: reqwest::ClientBuilder,
    tls_profile: Option<&str>,
) -> reqwest::ClientBuilder {
    let profile = normalize_tls_profile(tls_profile);
    if profile.is_none() {
        return builder;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    let tls_config = build_best_effort_tls_config();
    let builder = builder
        .use_preconfigured_tls(tls_config)
        .min_tls_version(Version::TLS_1_2)
        .max_tls_version(Version::TLS_1_3);

    if profile.as_deref() == Some(CLAUDE_CODE_TLS_PROFILE) {
        return builder;
    }

    builder
}

fn normalize_tls_profile(tls_profile: Option<&str>) -> Option<String> {
    let profile = tls_profile
        .map(str::trim)
        .filter(|profile| !profile.is_empty())?
        .to_ascii_lowercase();
    Some(profile)
}

fn build_best_effort_tls_config() -> rustls::ClientConfig {
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = rustls::ClientConfig::builder_with_protocol_versions(&[
        &rustls::version::TLS13,
        &rustls::version::TLS12,
    ])
    .with_root_certificates(root_store)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
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

fn build_request_headers(
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

fn collect_response_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    header_map_to_string_map(headers)
}

fn collect_tunnel_response_headers(headers: &[(String, String)]) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

fn decode_response_body_bytes(
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

fn response_body_is_json(headers: &BTreeMap<String, String>, body_bytes: &[u8]) -> bool {
    if headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .is_some_and(|value| value.contains("json"))
    {
        return true;
    }

    serde_json::from_slice::<Value>(body_bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::sync::Arc;

    use aether_contracts::{
        ExecutionPlan, ExecutionTimeouts, RequestBody, EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
        EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
    };
    use axum::body::Bytes;
    use axum::extract::ws::Message;
    use axum::extract::Path;
    use axum::http::HeaderMap as AxumHeaderMap;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::sync::watch;

    use super::{
        build_client, execute_sync_plan, DirectSyncExecutionRuntime, ExecutionTransportControls,
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

    fn tunnel_proxy_snapshot(base_url: String) -> aether_contracts::ProxySnapshot {
        aether_contracts::ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".into()),
            node_id: Some("node-1".into()),
            label: Some("relay-node".into()),
            url: None,
            extra: Some(json!({"tunnel_base_url": base_url})),
        }
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
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
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
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
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
            tls_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
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
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
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
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
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
                assert_eq!(meta["http1_only"], true);
                assert_eq!(meta["follow_redirects"], json!(false));
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
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
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
    async fn direct_sync_execution_runtime_allows_tls_profile_best_effort() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let app = Router::new().route(
            "/chat",
            post(|| async {
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"tls_profile": true})),
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
                client_api_format: "claude:chat".into(),
                provider_api_format: "claude:chat".into(),
                model_name: Some("claude-3.7-sonnet".into()),
                proxy: None,
                tls_profile: Some("claude_code_nodejs".into()),
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
                    ..ExecutionTimeouts::default()
                }),
            })
            .await
            .expect("sync execution with tls profile should succeed");

        server.abort();

        assert_eq!(result.status_code, 200);
        assert_eq!(
            result.body.and_then(|body| body.json_body),
            Some(json!({"tls_profile": true}))
        );
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
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
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
                tls_profile: None,
                timeouts: Some(ExecutionTimeouts {
                    connect_ms: Some(5_000),
                    total_ms: Some(5_000),
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
