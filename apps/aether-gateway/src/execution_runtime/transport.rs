use std::collections::BTreeMap;
use std::error::Error as _;
use std::future::Future;
use std::io::Read;
use std::io::Write;
use std::time::{Duration, Instant};

use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTelemetry, ProxySnapshot, ResolvedTransportProfile,
    ResponseBody, EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
    TRANSPORT_BACKEND_BROWSER_WREQ, TRANSPORT_BACKEND_REQWEST_RUSTLS,
    TRANSPORT_HTTP_MODE_HTTP1_ONLY,
};
use aether_data::repository::proxy_nodes::ProxyNodeTrafficMutation;
use aether_http::{apply_http_client_config, HttpClientConfig};
use axum::body::Bytes;
use base64::Engine as _;
use flate2::read::{DeflateDecoder, GzDecoder};
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use thiserror::Error;

use crate::ai_serving::api::extract_provider_private_stream_error_body;
#[cfg(test)]
use crate::execution_runtime::remote_compat::execute_sync_plan_via_remote_execution_runtime;
use crate::execution_runtime::windsurf::maybe_execute_windsurf_sync;
use crate::frontdoor_loop_guard::{
    configured_gateway_frontdoor_base_url, gateway_frontdoor_self_loop_guard_error,
};
use crate::tunnel::{self, tunnel_protocol};
use crate::{AppState, GatewayError};

const HUB_RELAY_CONTENT_TYPE: &str = "application/vnd.aether.tunnel-envelope";
const HUB_RELAY_ERROR_HEADER: &str = "x-aether-tunnel-error";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";
const DEFAULT_TUNNEL_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_STREAM_FIRST_BYTE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_NON_STREAM_TOTAL_TIMEOUT_MS: u64 = 300_000;
const MIN_TUNNEL_TIMEOUT_SECS: u64 = 1;
const MAX_TUNNEL_TIMEOUT_SECS: u64 = 300;
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
        with_non_stream_total_timeout(plan, async move {
            let response = send_request_inner(plan, body_bytes, false).await?;
            let ttfb_ms = started_at.elapsed().as_millis() as u64;
            let status_code = response.status_code();
            let headers = response.headers();
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

        let body_bytes = build_request_body(plan)?;

        let started_at = Instant::now();
        let response = send_request(plan, body_bytes).await?;
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

    let client = build_client(
        plan.timeouts.as_ref(),
        plan.proxy.as_ref(),
        plan.transport_profile.as_ref(),
        transport_controls,
    )?;
    let mut request = client.request(method, &plan.url);
    request = request.headers(headers).body(body_bytes);
    if let Some(timeout) = total_timeout {
        request = request.timeout(timeout);
    }
    send_reqwest_request(request, stream_first_byte_timeout)
        .await
        .map(DirectHttpResponse::Reqwest)
}

pub(crate) enum DirectHttpResponse {
    Reqwest(reqwest::Response),
    BrowserWreq(wreq::Response),
}

impl DirectHttpResponse {
    pub(crate) fn status_code(&self) -> u16 {
        match self {
            DirectHttpResponse::Reqwest(response) => response.status().as_u16(),
            DirectHttpResponse::BrowserWreq(response) => response.status().as_u16(),
        }
    }

    pub(crate) fn headers(&self) -> BTreeMap<String, String> {
        match self {
            DirectHttpResponse::Reqwest(response) => collect_response_headers(response.headers()),
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
            DirectHttpResponse::BrowserWreq(response) => {
                collect_wreq_stream_body(response, started_at, first_byte_timeout).await
            }
        }
    }

    fn into_direct_upstream_response(self) -> DirectUpstreamResponse {
        match self {
            DirectHttpResponse::Reqwest(response) => DirectUpstreamResponse::Reqwest(response),
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
    if let Some(timeout) = stream_first_byte_timeout {
        return match tokio::time::timeout(timeout, request.send()).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                format_upstream_request_error(&error),
            )),
            Err(_) => Err(ExecutionRuntimeTransportError::UpstreamRequest(
                stream_first_byte_timeout_message(timeout),
            )),
        };
    }

    request.send().await.map_err(|err| {
        ExecutionRuntimeTransportError::UpstreamRequest(format_upstream_request_error(&err))
    })
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
    timeouts: Option<&aether_contracts::ExecutionTimeouts>,
    proxy: Option<&ProxySnapshot>,
    transport_profile: Option<&ResolvedTransportProfile>,
    transport_controls: ExecutionTransportControls,
) -> Result<reqwest::Client, ExecutionRuntimeTransportError> {
    validate_reqwest_transport_profile(transport_profile)?;
    let mut builder = reqwest::Client::builder();
    if transport_controls.follow_redirects != Some(true) {
        builder = builder.redirect(Policy::none());
    }
    if transport_controls.http1_only || transport_profile_http1_only(transport_profile) {
        builder = builder.http1_only();
    }
    let mut builder = apply_http_client_config(
        builder,
        &HttpClientConfig {
            connect_timeout_ms: timeouts.and_then(|timeouts| timeouts.connect_ms),
            ..HttpClientConfig::default()
        },
    );
    builder = apply_transport_profile(builder, transport_profile);
    if transport_controls.accept_invalid_certs {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(proxy_url) = resolve_proxy_url(proxy)? {
        let proxy = reqwest::Proxy::all(&proxy_url)
            .map_err(ExecutionRuntimeTransportError::InvalidProxy)?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(ExecutionRuntimeTransportError::ClientBuild)
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

fn apply_transport_profile(
    builder: reqwest::ClientBuilder,
    transport_profile: Option<&ResolvedTransportProfile>,
) -> reqwest::ClientBuilder {
    let Some(profile) = transport_profile else {
        return builder;
    };
    let profile_id = profile.profile_id.trim();
    if profile_id.is_empty() {
        return builder;
    }

    let _ = rustls::crypto::ring::default_provider().install_default();

    builder.use_preconfigured_tls(build_best_effort_transport_tls_config())
}

fn build_best_effort_transport_tls_config() -> rustls::ClientConfig {
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
    use std::sync::Arc;

    use aether_contracts::{
        ExecutionPlan, ExecutionTimeouts, ProxySnapshot, RequestBody, ResolvedTransportProfile,
        EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
        TRANSPORT_BACKEND_BROWSER_WREQ, TRANSPORT_BACKEND_REQWEST_RUSTLS,
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
