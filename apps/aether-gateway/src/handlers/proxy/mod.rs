mod local;

use self::local::{
    maybe_build_local_admin_proxy_response, maybe_build_local_internal_proxy_response,
};
use super::internal::resolve_local_proxy_execution_path;
pub(crate) use super::public::matches_model_mapping_for_models;
use crate::ai_serving::api::{
    aggregate_claude_stream_sync_response, aggregate_gemini_stream_sync_response,
    aggregate_openai_chat_stream_sync_response, aggregate_openai_responses_stream_sync_response,
    maybe_bridge_standard_sync_json_to_stream,
};
use crate::api::response::{
    build_client_response, build_client_response_from_parts, build_local_auth_rejection_response,
    build_local_http_error_response, build_local_overloaded_response,
    build_local_user_rpm_limited_response,
};
use crate::constants::{
    DEPENDENCY_REASON_HEADER, EXECUTION_PATH_CONTROL_EXECUTE_STREAM,
    EXECUTION_PATH_CONTROL_EXECUTE_SYNC, EXECUTION_PATH_DISTRIBUTED_OVERLOADED,
    EXECUTION_PATH_EXECUTION_RUNTIME_STREAM, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
    EXECUTION_PATH_LOCAL_AI_PUBLIC, EXECUTION_PATH_LOCAL_API_KEY_CONCURRENCY_LIMITED,
    EXECUTION_PATH_LOCAL_AUTH_DENIED, EXECUTION_PATH_LOCAL_EXECUTION_LOOP_DETECTED,
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS, EXECUTION_PATH_LOCAL_OVERLOADED,
    EXECUTION_PATH_LOCAL_PROXY_PASSTHROUGH_REMOVED, EXECUTION_PATH_LOCAL_RATE_LIMITED,
    EXECUTION_PATH_LOCAL_ROUTE_NOT_FOUND, EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH,
    EXECUTION_RUNTIME_LOOP_GUARD_HEADER, FORWARDED_FOR_HEADER, FORWARDED_HOST_HEADER,
    FORWARDED_PROTO_HEADER, GATEWAY_HEADER, LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER,
    TRACE_ID_HEADER, TRUSTED_AUTH_ACCESS_ALLOWED_HEADER, TRUSTED_AUTH_API_KEY_ID_HEADER,
    TRUSTED_AUTH_BALANCE_HEADER, TRUSTED_AUTH_USER_ID_HEADER, TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
    TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
};
use crate::control::{
    allows_control_execute_emergency, maybe_execute_via_control, request_model_local_rejection,
    should_buffer_request_for_local_auth, trusted_auth_local_rejection, GatewayControlDecision,
    GatewayPublicRequestContext,
};
use crate::executor::{
    beautify_local_execution_client_error_message, build_local_execution_runtime_miss_context,
    maybe_execute_stream_request, maybe_execute_sync_request,
    record_failed_usage_for_exhausted_request, record_failed_usage_for_runtime_miss_request,
    LocalExecutionRequestOutcome,
};
use crate::frontdoor_loop_guard::{
    frontdoor_self_loop_public_ai_path, request_has_execution_runtime_loop_guard,
};
use crate::handlers::shared::{
    build_admin_proxy_auth_required_response, build_unhandled_admin_proxy_response,
    local_proxy_route_requires_buffered_body, request_enables_control_execute,
    should_strip_forwarded_provider_credential_header, should_strip_forwarded_trusted_admin_header,
};
use crate::headers::{
    extract_or_generate_trace_id, request_origin_from_headers_and_remote_addr,
    should_skip_request_header,
};
use crate::router::RequestAdmissionError;
use crate::{
    AppState, FrontdoorUserRpmOutcome, GatewayError, GatewayFallbackMetricKind,
    GatewayFallbackReason, LocalExecutionRuntimeMissDiagnostic,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{self, header::HeaderName, header::HeaderValue, Response};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Instant};
use tracing::{debug, info, warn};

const OPENAI_CHAT_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "当前 OpenAI Chat Completions 请求无法在本地执行：没有匹配到可用的执行路径";
const OPENAI_RESPONSES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "当前 OpenAI Responses 请求无法在本地执行：没有匹配到可用的执行路径";
const OPENAI_RESPONSES_COMPACT_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "当前 OpenAI Responses Compact 请求无法在本地执行：没有匹配到可用的执行路径";
const OPENAI_VIDEO_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "当前 OpenAI Video 请求无法在本地执行：没有匹配到可用的执行路径";
const CLAUDE_MESSAGES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "当前 Claude Messages 请求无法在本地执行：没有匹配到可用的执行路径";
const GEMINI_PUBLIC_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "当前 Gemini Public 请求无法在本地执行：没有匹配到可用的执行路径";
const GEMINI_FILES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "当前 Gemini Files 请求无法在本地执行：没有匹配到可用的执行路径";
const LOCAL_ROUTE_NOT_FOUND_DETAIL: &str = "Route not found";
const LOCAL_PROXY_PASSTHROUGH_REMOVED_DETAIL: &str =
    "Route matched a removed compatibility passthrough; implement it in Rust or retire the route";
const LOCAL_EXECUTION_LOOP_DETECTED_DETAIL: &str =
    "Gateway detected an execution runtime request loop back into the local frontdoor";
const AUTH_API_KEY_CONCURRENCY_LIMIT_REACHED_DETAIL: &str =
    "当前 API Key 并发请求数已达上限，请稍后重试";
const EXECUTION_PATH_TUNNEL_AFFINITY_FORWARD: &str = "tunnel_affinity_forward";
const MANAGEMENT_TOKEN_PREFIX: &str = "ae-";
const LEGACY_MANAGEMENT_TOKEN_PREFIX: &str = "ae_";

fn local_execution_outcome_label(outcome: &LocalExecutionRequestOutcome) -> &'static str {
    match outcome {
        LocalExecutionRequestOutcome::Responded(_) => "responded",
        LocalExecutionRequestOutcome::Exhausted(_) => "exhausted",
        LocalExecutionRequestOutcome::NoPath => "no_path",
    }
}

fn request_hits_execution_loop_guard(parts: &http::request::Parts) -> bool {
    request_has_execution_runtime_loop_guard(&parts.headers)
        && frontdoor_self_loop_public_ai_path(parts.uri.path())
}

fn execution_runtime_candidate_header_value(decision: &GatewayControlDecision) -> &'static str {
    if decision.is_execution_runtime_candidate() {
        "true"
    } else {
        "false"
    }
}

fn extract_management_token_bearer(headers: &http::HeaderMap) -> Option<String> {
    let header = crate::headers::header_value_str(headers, http::header::AUTHORIZATION.as_str())?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))?
        .trim()
        .to_string();
    (!token.is_empty()
        && (token.starts_with(MANAGEMENT_TOKEN_PREFIX)
            || token.starts_with(LEGACY_MANAGEMENT_TOKEN_PREFIX)))
    .then_some(token)
}

fn hash_management_token(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn remote_ip_allowed(allowed_ips: Option<&serde_json::Value>, remote_ip: std::net::IpAddr) -> bool {
    let Some(allowed_ips) = allowed_ips else {
        return true;
    };
    if allowed_ips.is_null() {
        return true;
    }
    let Some(items) = allowed_ips.as_array() else {
        return false;
    };
    if items.is_empty() {
        return false;
    }
    items
        .iter()
        .filter_map(serde_json::Value::as_str)
        .any(|value| ip_or_cidr_matches(value, remote_ip))
}

fn ip_or_cidr_matches(pattern: &str, remote_ip: std::net::IpAddr) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if let Ok(ip) = pattern.parse::<std::net::IpAddr>() {
        return ip == remote_ip;
    }
    let Some((network, prefix)) = pattern.split_once('/') else {
        return false;
    };
    let Ok(prefix) = prefix.trim().parse::<u8>() else {
        return false;
    };
    match (network.trim().parse::<std::net::IpAddr>(), remote_ip) {
        (Ok(std::net::IpAddr::V4(network)), std::net::IpAddr::V4(remote)) if prefix <= 32 => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            (u32::from(network) & mask) == (u32::from(remote) & mask)
        }
        (Ok(std::net::IpAddr::V6(network)), std::net::IpAddr::V6(remote)) if prefix <= 128 => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            (u128::from(network) & mask) == (u128::from(remote) & mask)
        }
        _ => false,
    }
}

async fn maybe_promote_management_token_admin_principal(
    state: &AppState,
    remote_addr: &std::net::SocketAddr,
    headers: &http::HeaderMap,
    trace_id: &str,
    request_context: &mut GatewayPublicRequestContext,
) -> Result<(), GatewayError> {
    let Some(decision) = request_context.control_decision.as_mut() else {
        return Ok(());
    };
    if decision.route_class.as_deref() != Some("admin_proxy") || decision.admin_principal.is_some()
    {
        return Ok(());
    }

    let Some(token) = extract_management_token_bearer(headers) else {
        return Ok(());
    };
    let token_hash = hash_management_token(&token);
    let Some(token_with_user) = state
        .get_management_token_with_user_by_hash(&token_hash)
        .await?
    else {
        return Ok(());
    };

    if !token_with_user.token.is_active {
        return Ok(());
    }
    if token_with_user
        .token
        .expires_at_unix_secs
        .is_some_and(|value| value <= chrono::Utc::now().timestamp().max(0) as u64)
    {
        return Ok(());
    }
    if !remote_ip_allowed(token_with_user.token.allowed_ips.as_ref(), remote_addr.ip()) {
        return Ok(());
    }
    let Some(user) = state.find_user_auth_by_id(&token_with_user.user.id).await? else {
        return Ok(());
    };
    if !user.is_active || user.is_deleted || !user.role.eq_ignore_ascii_case("admin") {
        return Ok(());
    }

    decision.admin_principal = Some(crate::control::GatewayAdminPrincipalContext {
        user_id: user.id.clone(),
        user_role: user.role.clone(),
        session_id: None,
        management_token_id: Some(token_with_user.token.id.clone()),
    });

    let remote_ip = remote_addr.ip().to_string();
    if let Err(err) = state
        .record_management_token_usage(&token_with_user.token.id, Some(remote_ip.as_str()))
        .await
    {
        warn!(
            trace_id = %trace_id,
            token_id = %token_with_user.token.id,
            error = ?err,
            "gateway failed to record management token usage"
        );
    }
    Ok(())
}

async fn maybe_forward_public_request_to_tunnel_owner(
    state: &AppState,
    remote_addr: &std::net::SocketAddr,
    request_context: &GatewayPublicRequestContext,
    parts: &http::request::Parts,
    buffered_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };
    if decision.route_class.as_deref() != Some("ai_public")
        || !decision.is_execution_runtime_candidate()
    {
        return Ok(None);
    }
    if parts
        .headers
        .get(TUNNEL_AFFINITY_FORWARDED_BY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return Ok(None);
    }

    let Some(auth_context) = decision.auth_context.as_ref().filter(|auth_context| {
        auth_context.access_allowed && !auth_context.api_key_id.trim().is_empty()
    }) else {
        return Ok(None);
    };
    let Some(api_format) = decision
        .auth_endpoint_signature
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let empty_body = Bytes::new();
    let Some(requested_model) = crate::control::extract_requested_model(
        decision,
        &parts.uri,
        &parts.headers,
        buffered_body.unwrap_or(&empty_body),
    ) else {
        return Ok(None);
    };
    let Some(target) = crate::scheduler::affinity::read_cached_scheduler_affinity_target(
        state,
        &auth_context.api_key_id,
        api_format,
        &requested_model,
    ) else {
        return Ok(None);
    };

    let transport = match state
        .read_provider_transport_snapshot(&target.provider_id, &target.endpoint_id, &target.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return Ok(None),
        Err(err) => {
            warn!(
                trace_id = %request_context.trace_id,
                provider_id = %target.provider_id,
                endpoint_id = %target.endpoint_id,
                key_id = %target.key_id,
                error = ?err,
                "gateway failed to read provider transport for tunnel affinity forward"
            );
            return Ok(None);
        }
    };

    let Some(proxy) = state
        .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
        .await
    else {
        return Ok(None);
    };
    if proxy.enabled == Some(false) {
        return Ok(None);
    }
    let Some(node_id) = proxy
        .node_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if state.tunnel.has_local_proxy(node_id) {
        return Ok(None);
    }

    let Some(owner) = state
        .tunnel
        .lookup_attachment_owner(state.data.as_ref(), node_id)
        .await
        .map_err(GatewayError::Internal)?
    else {
        return Ok(None);
    };
    if owner.gateway_instance_id == state.tunnel.local_instance_id() {
        return Ok(None);
    }

    let owner_url = format!(
        "{}{}",
        owner.relay_base_url.trim_end_matches('/'),
        request_context.request_path_and_query()
    );
    let mut upstream_request = state.client.request(parts.method.clone(), owner_url);
    for (name, value) in &parts.headers {
        if should_skip_request_header(name.as_str()) || name == http::header::HOST {
            continue;
        }
        if should_strip_forwarded_provider_credential_header(Some(decision), name) {
            continue;
        }
        if should_strip_forwarded_trusted_admin_header(Some(decision), name) {
            continue;
        }
        upstream_request = upstream_request.header(name, value);
    }
    if let Some(host) = request_context.host_header.as_deref() {
        if !parts.headers.contains_key(FORWARDED_HOST_HEADER) {
            upstream_request = upstream_request.header(FORWARDED_HOST_HEADER, host);
        }
    }
    if !parts.headers.contains_key(FORWARDED_FOR_HEADER) {
        upstream_request =
            upstream_request.header(FORWARDED_FOR_HEADER, remote_addr.ip().to_string());
    }
    if !parts.headers.contains_key(FORWARDED_PROTO_HEADER) {
        upstream_request = upstream_request.header(FORWARDED_PROTO_HEADER, "http");
    }
    if !parts.headers.contains_key(TRACE_ID_HEADER) {
        upstream_request = upstream_request.header(TRACE_ID_HEADER, &request_context.trace_id);
    }
    upstream_request = upstream_request
        .header(GATEWAY_HEADER, "rust-phase3b-affinity")
        .header(
            TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
            state.tunnel.local_instance_id(),
        )
        .header(
            TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
            owner.gateway_instance_id.as_str(),
        )
        .header(TRUSTED_AUTH_USER_ID_HEADER, &auth_context.user_id)
        .header(TRUSTED_AUTH_API_KEY_ID_HEADER, &auth_context.api_key_id)
        .header(TRUSTED_AUTH_ACCESS_ALLOWED_HEADER, "true");
    if let Some(balance_remaining) = auth_context.balance_remaining {
        upstream_request =
            upstream_request.header(TRUSTED_AUTH_BALANCE_HEADER, balance_remaining.to_string());
    }

    let upstream_response = upstream_request
        .body(buffered_body.cloned().unwrap_or_default())
        .send()
        .await
        .map_err(|err| GatewayError::UpstreamUnavailable {
            trace_id: request_context.trace_id.clone(),
            message: format!("owner gateway affinity forward failed: {err}"),
        })?;

    let mut response = build_sync_aware_affinity_forward_response(
        request_context,
        buffered_body,
        decision,
        upstream_response,
    )
    .await?;
    response.headers_mut().insert(
        HeaderName::from_static(TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER),
        HeaderValue::from_str(owner.gateway_instance_id.as_str())
            .map_err(|err| GatewayError::Internal(err.to_string()))?,
    );
    Ok(Some(response))
}

fn upstream_response_is_sse(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
}

fn collect_upstream_response_headers(
    headers: &reqwest::header::HeaderMap,
) -> BTreeMap<String, String> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn aggregate_sync_sse_response_for_client(
    decision: &GatewayControlDecision,
    public_path: &str,
    body: &[u8],
) -> Option<serde_json::Value> {
    let api_format = decision
        .auth_endpoint_signature
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match api_format.map(crate::ai_serving::normalize_api_format_alias) {
        Some(value) if value.eq_ignore_ascii_case("openai:chat") => {
            aggregate_openai_chat_stream_sync_response(body)
        }
        Some(value)
            if value.eq_ignore_ascii_case("openai:responses")
                || value.eq_ignore_ascii_case("openai:responses:compact") =>
        {
            aggregate_openai_responses_stream_sync_response(body)
        }
        Some(value) if value.eq_ignore_ascii_case("claude:messages") => {
            aggregate_claude_stream_sync_response(body)
        }
        Some(value) if value.eq_ignore_ascii_case("gemini:generate_content") => {
            aggregate_gemini_stream_sync_response(body)
        }
        _ if public_path == "/v1/chat/completions" => {
            aggregate_openai_chat_stream_sync_response(body)
        }
        _ if public_path == "/v1/responses" || public_path == "/v1/responses/compact" => {
            aggregate_openai_responses_stream_sync_response(body)
        }
        _ if public_path == "/v1/messages" => aggregate_claude_stream_sync_response(body),
        _ if decision.route_family.as_deref() == Some("gemini")
            && (public_path.contains(":generateContent")
                || public_path.contains(":streamGenerateContent")) =>
        {
            aggregate_gemini_stream_sync_response(body)
        }
        _ => None,
    }
}

fn build_sync_json_proxy_response(
    status_code: u16,
    upstream_headers: &BTreeMap<String, String>,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<Response<Body>, GatewayError> {
    let mut headers = upstream_headers.clone();
    headers.remove("content-encoding");
    headers.remove("content-length");
    headers.insert("content-type".to_string(), "application/json".to_string());
    let body_bytes =
        serde_json::to_vec(body_json).map_err(|err| GatewayError::Internal(err.to_string()))?;
    headers.insert("content-length".to_string(), body_bytes.len().to_string());
    build_client_response_from_parts(
        status_code,
        &headers,
        Body::from(body_bytes),
        trace_id,
        Some(decision),
    )
}

fn resolve_affinity_forward_client_api_format(
    decision: &GatewayControlDecision,
    public_path: &str,
) -> Option<&'static str> {
    let api_format = decision
        .auth_endpoint_signature
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match api_format.map(crate::ai_serving::normalize_api_format_alias) {
        Some(value) if value.eq_ignore_ascii_case("openai:chat") => Some("openai:chat"),
        Some(value) if value.eq_ignore_ascii_case("openai:responses") => Some("openai:responses"),
        Some(value) if value.eq_ignore_ascii_case("openai:responses:compact") => {
            Some("openai:responses:compact")
        }
        Some(value) if value.eq_ignore_ascii_case("claude:messages") => Some("claude:messages"),
        Some(value) if value.eq_ignore_ascii_case("gemini:generate_content") => {
            Some("gemini:generate_content")
        }
        _ if public_path == "/v1/chat/completions" => Some("openai:chat"),
        _ if public_path == "/v1/responses" => Some("openai:responses"),
        _ if public_path == "/v1/responses/compact" => Some("openai:responses:compact"),
        _ if public_path == "/v1/messages" => Some("claude:messages"),
        _ if decision.route_family.as_deref() == Some("gemini")
            && (public_path.contains(":generateContent")
                || public_path.contains(":streamGenerateContent")) =>
        {
            Some("gemini:generate_content")
        }
        _ => None,
    }
}

fn build_stream_sse_proxy_response(
    status_code: u16,
    upstream_headers: &BTreeMap<String, String>,
    sse_body: &[u8],
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<Response<Body>, GatewayError> {
    let mut headers = upstream_headers.clone();
    headers.remove("content-encoding");
    headers.remove("content-length");
    headers.insert("content-type".to_string(), "text/event-stream".to_string());
    headers.insert("content-length".to_string(), sse_body.len().to_string());
    build_client_response_from_parts(
        status_code,
        &headers,
        Body::from(sse_body.to_vec()),
        trace_id,
        Some(decision),
    )
}

async fn build_sync_aware_affinity_forward_response(
    request_context: &GatewayPublicRequestContext,
    buffered_body: Option<&Bytes>,
    decision: &GatewayControlDecision,
    upstream_response: reqwest::Response,
) -> Result<Response<Body>, GatewayError> {
    let Some(buffered_body) = buffered_body else {
        return build_client_response(upstream_response, &request_context.trace_id, Some(decision));
    };
    let stream_request = request_wants_stream(request_context, buffered_body);
    let upstream_is_sse = upstream_response_is_sse(upstream_response.headers());
    if (!stream_request && !upstream_is_sse) || (stream_request && upstream_is_sse) {
        return build_client_response(upstream_response, &request_context.trace_id, Some(decision));
    }

    let status_code = upstream_response.status().as_u16();
    let headers = collect_upstream_response_headers(upstream_response.headers());
    let body_bytes = upstream_response
        .bytes()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    if stream_request {
        if (200..300).contains(&status_code) {
            if let Some(client_api_format) = resolve_affinity_forward_client_api_format(
                decision,
                request_context.request_path.as_str(),
            ) {
                if let Ok(body_json) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                    if let Some(outcome) = maybe_bridge_standard_sync_json_to_stream(
                        &body_json,
                        client_api_format,
                        client_api_format,
                        None,
                    )? {
                        return build_stream_sse_proxy_response(
                            status_code,
                            &headers,
                            &outcome.sse_body,
                            &request_context.trace_id,
                            decision,
                        );
                    }
                }
            }
        }
    } else if let Some(body_json) = aggregate_sync_sse_response_for_client(
        decision,
        request_context.request_path.as_str(),
        &body_bytes,
    ) {
        return build_sync_json_proxy_response(
            status_code,
            &headers,
            &body_json,
            &request_context.trace_id,
            decision,
        );
    }

    build_client_response_from_parts(
        status_code,
        &headers,
        Body::from(body_bytes),
        &request_context.trace_id,
        Some(decision),
    )
}

pub(crate) async fn proxy_request(
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> Result<Response<Body>, GatewayError> {
    let started_at = Instant::now();
    let mut request_permit = match state.try_acquire_request_permit().await {
        Ok(permit) => permit,
        Err(RequestAdmissionError::Local(aether_runtime::ConcurrencyError::Saturated {
            gate,
            limit,
        })) => {
            let trace_id = extract_or_generate_trace_id(request.headers());
            let response = build_local_overloaded_response(&trace_id, None, gate, limit)?;
            return Ok(finalize_gateway_response(
                &state,
                response,
                &trace_id,
                &remote_addr,
                request.method(),
                request
                    .uri()
                    .path_and_query()
                    .map(|value| value.as_str())
                    .unwrap_or("/"),
                None,
                EXECUTION_PATH_LOCAL_OVERLOADED,
                &started_at,
                None,
            ));
        }
        Err(RequestAdmissionError::Local(aether_runtime::ConcurrencyError::Closed { gate })) => {
            return Err(GatewayError::Internal(format!(
                "gateway request concurrency gate {gate} is closed"
            )));
        }
        Err(RequestAdmissionError::Distributed(
            aether_runtime::DistributedConcurrencyError::Saturated { gate, limit },
        ))
        | Err(RequestAdmissionError::Distributed(
            aether_runtime::DistributedConcurrencyError::Unavailable { gate, limit, .. },
        )) => {
            let trace_id = extract_or_generate_trace_id(request.headers());
            let response = build_local_overloaded_response(&trace_id, None, gate, limit)?;
            return Ok(finalize_gateway_response(
                &state,
                response,
                &trace_id,
                &remote_addr,
                request.method(),
                request
                    .uri()
                    .path_and_query()
                    .map(|value| value.as_str())
                    .unwrap_or("/"),
                None,
                EXECUTION_PATH_DISTRIBUTED_OVERLOADED,
                &started_at,
                None,
            ));
        }
        Err(RequestAdmissionError::Distributed(
            aether_runtime::DistributedConcurrencyError::InvalidConfiguration(message),
        )) => return Err(GatewayError::Internal(message)),
    };
    let request_admission_ms = started_at.elapsed().as_millis() as u64;
    let (mut parts, body) = request.into_parts();
    parts
        .extensions
        .insert(request_origin_from_headers_and_remote_addr(
            &parts.headers,
            &remote_addr,
        ));
    let trace_id = extract_or_generate_trace_id(&parts.headers);
    state.clear_local_execution_runtime_miss_diagnostic(&trace_id);
    if request_hits_execution_loop_guard(&parts) {
        warn!(
            event_name = "frontdoor_execution_loop_detected",
            log_type = "ops",
            trace_id = %trace_id,
            method = %parts.method,
            path = %parts
                .uri
                .path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
            loop_guard_header = EXECUTION_RUNTIME_LOOP_GUARD_HEADER,
            "gateway rejected execution runtime request loop into frontdoor"
        );
        let response = build_local_http_error_response(
            &trace_id,
            None,
            http::StatusCode::LOOP_DETECTED,
            LOCAL_EXECUTION_LOOP_DETECTED_DETAIL,
        )?;
        return Ok(finalize_gateway_response(
            &state,
            response,
            &trace_id,
            &remote_addr,
            &parts.method,
            parts
                .uri
                .path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
            None,
            EXECUTION_PATH_LOCAL_EXECUTION_LOOP_DETECTED,
            &started_at,
            request_permit.take(),
        ));
    }
    let request_context_started_at = Instant::now();
    let mut request_context = crate::control::resolve_public_request_context(
        &state,
        &parts.method,
        &parts.uri,
        &parts.headers,
        &trace_id,
    )
    .await?;
    maybe_promote_management_token_admin_principal(
        &state,
        &remote_addr,
        &parts.headers,
        &trace_id,
        &mut request_context,
    )
    .await?;
    let request_context_ms = request_context_started_at.elapsed().as_millis() as u64;
    if request_context
        .control_decision
        .as_ref()
        .is_some_and(|decision| {
            decision.route_family.as_deref() == Some("api_keys_manage")
                && decision.route_kind.as_deref() == Some("list_api_keys")
        })
    {
        info!(
            event_name = "admin_api_keys_route_breakdown",
            log_type = "event",
            trace_id = %trace_id,
            method = %parts.method,
            path = %parts
                .uri
                .path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
            request_admission_ms,
            request_context_ms,
            "measured admin api keys route pre-handler timing"
        );
    }
    let mut request_body = Some(body);
    let local_proxy_body = if local_proxy_route_requires_buffered_body(&request_context) {
        Some(
            to_bytes(
                request_body
                    .take()
                    .expect("local proxy body buffering should own request body"),
                usize::MAX,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?,
        )
    } else {
        None
    };
    let method = request_context.request_method.clone();
    let request_path_and_query = request_context.request_path_and_query();
    let path_and_query = request_path_and_query.as_str();
    let control_decision = request_context.control_decision.as_ref();
    if let Some(response) = maybe_build_local_internal_proxy_response(
        &state,
        &request_context,
        &remote_addr,
        local_proxy_body.as_ref(),
    )
    .await?
    {
        let execution_path =
            resolve_local_proxy_execution_path(&response, EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH);
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            execution_path,
            &started_at,
            request_permit.take(),
        ));
    }
    if let Some(response) =
        maybe_build_local_admin_proxy_response(&state, &request_context, local_proxy_body.as_ref())
            .await?
    {
        let execution_path =
            resolve_local_proxy_execution_path(&response, EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH);
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            execution_path,
            &started_at,
            request_permit.take(),
        ));
    }
    if request_context
        .control_decision
        .as_ref()
        .is_some_and(|decision| {
            decision.route_class.as_deref() == Some("admin_proxy")
                && decision.admin_principal.is_none()
        })
    {
        let response = build_admin_proxy_auth_required_response(&request_context);
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH,
            &started_at,
            request_permit.take(),
        ));
    }
    if request_context
        .control_decision
        .as_ref()
        .is_some_and(|decision| {
            decision.route_class.as_deref() == Some("admin_proxy")
                && decision.admin_principal.is_some()
        })
    {
        let response = build_unhandled_admin_proxy_response(&request_context);
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH,
            &started_at,
            request_permit.take(),
        ));
    }
    if request_context.request_path.starts_with("/api/admin/") {
        let response = build_unhandled_admin_proxy_response(&request_context);
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH,
            &started_at,
            request_permit.take(),
        ));
    }
    if let Some(response) = super::public::maybe_build_local_public_support_response(
        &state,
        &request_context,
        &parts.headers,
        local_proxy_body.as_ref(),
    )
    .await
    {
        let execution_path =
            resolve_local_proxy_execution_path(&response, EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH);
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            execution_path,
            &started_at,
            request_permit.take(),
        ));
    }
    if request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.route_class.as_deref())
        == Some("public_support")
    {
        let response = super::public::build_unhandled_public_support_response(&request_context);
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH,
            &started_at,
            request_permit.take(),
        ));
    }
    if let Some(buffered_body) = local_proxy_body {
        request_body = Some(Body::from(buffered_body));
    }
    let should_try_control_execute = control_decision
        .map(|decision| {
            decision.is_execution_runtime_candidate()
                && decision.route_class.as_deref() == Some("ai_public")
        })
        .unwrap_or(false);
    let should_buffer_for_local_ai_public =
        super::public::ai_public_local_requires_buffered_body(&request_context);
    let should_buffer_for_local_auth =
        should_buffer_request_for_local_auth(control_decision, &parts.headers);
    let should_buffer_body = should_try_control_execute
        || should_buffer_for_local_auth
        || should_buffer_for_local_ai_public;

    let allow_control_execute_fallback = should_try_control_execute
        && control_decision.is_some_and(allows_control_execute_emergency)
        && request_enables_control_execute(&parts.headers);

    let buffered_body = if should_buffer_body {
        Some(
            to_bytes(
                request_body
                    .take()
                    .expect("buffered auth/execution runtime path should own request body"),
                usize::MAX,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?,
        )
    } else {
        None
    };

    if let Some(response) = maybe_forward_public_request_to_tunnel_owner(
        &state,
        &remote_addr,
        &request_context,
        &parts,
        buffered_body.as_ref(),
    )
    .await?
    {
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_TUNNEL_AFFINITY_FORWARD,
            &started_at,
            request_permit.take(),
        ));
    }

    if let Some(rejection) = trusted_auth_local_rejection(control_decision, &parts.headers) {
        let response =
            build_local_auth_rejection_response(&trace_id, control_decision, &rejection)?;
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_LOCAL_AUTH_DENIED,
            &started_at,
            request_permit.take(),
        ));
    }

    if let Some(buffered_body) = buffered_body.as_ref() {
        if let Some(rejection) = request_model_local_rejection(
            &state,
            control_decision,
            &parts.uri,
            &parts.headers,
            buffered_body,
        )
        .await?
        {
            let response =
                build_local_auth_rejection_response(&trace_id, control_decision, &rejection)?;
            return Ok(finalize_gateway_response_with_context(
                &state,
                response,
                &remote_addr,
                &request_context,
                EXECUTION_PATH_LOCAL_AUTH_DENIED,
                &started_at,
                request_permit.take(),
            ));
        }
    }

    let rate_limit_outcome = state
        .frontdoor_user_rpm()
        .check_and_consume(&state, control_decision)
        .await?;
    if let FrontdoorUserRpmOutcome::Rejected(rejection) = &rate_limit_outcome {
        let auth_context = control_decision.and_then(|decision| decision.auth_context.as_ref());
        let user_id = auth_context
            .map(|auth_context| auth_context.user_id.as_str())
            .unwrap_or("-");
        let api_key_id = auth_context
            .map(|auth_context| auth_context.api_key_id.as_str())
            .unwrap_or("-");
        let path_and_query = request_context.request_path_and_query();
        info!(
            event_name = "frontdoor_user_rpm_rejected",
            log_type = "event",
            trace_id = %trace_id,
            method = %parts.method,
            path = %path_and_query,
            user_id,
            api_key_id,
            scope = rejection.scope,
            limit = rejection.limit,
            retry_after = rejection.retry_after,
            "gateway rejected request at frontdoor user rpm limit"
        );
        let response =
            build_local_user_rpm_limited_response(&trace_id, control_decision, rejection)?;
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_LOCAL_RATE_LIMITED,
            &started_at,
            request_permit.take(),
        ));
    }

    if let Some(response) = super::public::maybe_build_local_ai_public_response(
        &state,
        &request_context,
        buffered_body.as_ref(),
    )
    .await
    {
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_LOCAL_AI_PUBLIC,
            &started_at,
            request_permit.take(),
        ));
    }

    if control_decision.is_none() {
        let response = build_local_http_error_response(
            &trace_id,
            None,
            http::StatusCode::NOT_FOUND,
            LOCAL_ROUTE_NOT_FOUND_DETAIL,
        )?;
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_LOCAL_ROUTE_NOT_FOUND,
            &started_at,
            request_permit.take(),
        ));
    }

    if should_try_control_execute {
        let buffered_body = buffered_body
            .as_ref()
            .expect("execution runtime/control auth gate should have buffered request body");
        let stream_request = request_wants_stream(&request_context, buffered_body);
        let mut local_execution_exhaustion = None;
        if stream_request {
            let stream_outcome = maybe_execute_stream_request(
                &state,
                &parts,
                buffered_body,
                &trace_id,
                control_decision,
            )
            .await?;
            debug!(
                event_name = "proxy_stream_local_execute_outcome",
                log_type = "debug",
                trace_id = %trace_id,
                outcome = local_execution_outcome_label(&stream_outcome),
                route_family = control_decision
                    .and_then(|decision| decision.route_family.as_deref())
                    .unwrap_or("-"),
                route_kind = control_decision
                    .and_then(|decision| decision.route_kind.as_deref())
                    .unwrap_or("-"),
                request_path = %request_context.request_path_and_query(),
                "gateway local stream execution returned to proxy"
            );
            match stream_outcome {
                LocalExecutionRequestOutcome::Responded(execution_runtime_response) => {
                    state.clear_local_execution_runtime_miss_diagnostic(&trace_id);
                    return Ok(finalize_gateway_response_with_context(
                        &state,
                        execution_runtime_response,
                        &remote_addr,
                        &request_context,
                        EXECUTION_PATH_EXECUTION_RUNTIME_STREAM,
                        &started_at,
                        request_permit.take(),
                    ));
                }
                LocalExecutionRequestOutcome::Exhausted(outcome) => {
                    local_execution_exhaustion = Some(outcome);
                }
                LocalExecutionRequestOutcome::NoPath => {}
            }
        }
        match maybe_execute_sync_request(&state, &parts, buffered_body, &trace_id, control_decision)
            .await?
        {
            LocalExecutionRequestOutcome::Responded(execution_runtime_response) => {
                state.clear_local_execution_runtime_miss_diagnostic(&trace_id);
                return Ok(finalize_gateway_response_with_context(
                    &state,
                    execution_runtime_response,
                    &remote_addr,
                    &request_context,
                    EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
                    &started_at,
                    request_permit.take(),
                ));
            }
            LocalExecutionRequestOutcome::Exhausted(outcome) => {
                local_execution_exhaustion = Some(outcome);
            }
            LocalExecutionRequestOutcome::NoPath => {}
        }
        if parts.method != http::Method::POST {
            match maybe_execute_stream_request(
                &state,
                &parts,
                buffered_body,
                &trace_id,
                control_decision,
            )
            .await?
            {
                LocalExecutionRequestOutcome::Responded(execution_runtime_response) => {
                    state.clear_local_execution_runtime_miss_diagnostic(&trace_id);
                    return Ok(finalize_gateway_response_with_context(
                        &state,
                        execution_runtime_response,
                        &remote_addr,
                        &request_context,
                        EXECUTION_PATH_EXECUTION_RUNTIME_STREAM,
                        &started_at,
                        request_permit.take(),
                    ));
                }
                LocalExecutionRequestOutcome::Exhausted(outcome) => {
                    local_execution_exhaustion = Some(outcome);
                }
                LocalExecutionRequestOutcome::NoPath => {}
            }
        }
        if allow_control_execute_fallback {
            match maybe_execute_via_control(
                &state,
                &parts,
                buffered_body.clone(),
                &trace_id,
                control_decision,
                stream_request,
            )
            .await?
            {
                LocalExecutionRequestOutcome::Responded(control_response) => {
                    let reason = GatewayFallbackReason::ControlExecuteEmergency;
                    let control_execution_path = if stream_request {
                        EXECUTION_PATH_CONTROL_EXECUTE_STREAM
                    } else {
                        EXECUTION_PATH_CONTROL_EXECUTE_SYNC
                    };
                    state.record_fallback_metric(
                        GatewayFallbackMetricKind::ControlExecuteFallback,
                        control_decision,
                        None,
                        Some(control_execution_path),
                        reason,
                    );
                    state.record_fallback_metric(
                        GatewayFallbackMetricKind::RemoteExecuteEmergency,
                        control_decision,
                        None,
                        Some(control_execution_path),
                        reason,
                    );
                    let mut control_response = control_response;
                    state.clear_local_execution_runtime_miss_diagnostic(&trace_id);
                    control_response.headers_mut().insert(
                        HeaderName::from_static(DEPENDENCY_REASON_HEADER),
                        HeaderValue::from_static(reason.as_label_value()),
                    );
                    return Ok(finalize_gateway_response_with_context(
                        &state,
                        control_response,
                        &remote_addr,
                        &request_context,
                        control_execution_path,
                        &started_at,
                        request_permit.take(),
                    ));
                }
                LocalExecutionRequestOutcome::Exhausted(outcome) => {
                    local_execution_exhaustion = Some(outcome);
                }
                LocalExecutionRequestOutcome::NoPath => {}
            }
        }
        let local_execution_runtime_miss_diagnostic =
            state.take_local_execution_runtime_miss_diagnostic(&trace_id);
        let local_execution_runtime_miss_context =
            build_local_execution_runtime_miss_context(&state, &trace_id, control_decision).await;
        let auth_api_key_concurrency_limited = diagnostic_is_auth_api_key_concurrency_limited(
            local_execution_runtime_miss_diagnostic.as_ref(),
        ) || local_execution_runtime_miss_context
            .all_candidates_skipped_for_reason("api_key_concurrency_limit_reached");
        let local_execution_runtime_miss_detail = local_execution_runtime_miss_detail(
            control_decision,
            local_execution_runtime_miss_diagnostic.as_ref(),
            auth_api_key_concurrency_limited,
            stream_request,
        )
        .unwrap_or_else(|| "当前 AI 请求无法在本地执行：没有匹配到可用的执行路径".to_string());
        let local_execution_failure_path = if auth_api_key_concurrency_limited {
            EXECUTION_PATH_LOCAL_API_KEY_CONCURRENCY_LIMITED
        } else {
            EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS
        };
        let local_execution_failure_log = if auth_api_key_concurrency_limited {
            "gateway local execution blocked by api key concurrency limit"
        } else {
            "gateway local execution runtime miss"
        };
        state.record_fallback_metric(
            GatewayFallbackMetricKind::LocalExecutionRuntimeMiss,
            control_decision,
            None,
            Some(local_execution_failure_path),
            GatewayFallbackReason::LocalExecutionPathRequired,
        );
        warn!(
            trace_id = %trace_id,
            local_execution_runtime_miss_reason = local_execution_runtime_miss_diagnostic
                .as_ref()
                .map(|value| value.reason.as_str())
                .unwrap_or("unknown"),
            route_family = local_execution_runtime_miss_diagnostic
                .as_ref()
                .and_then(|value| value.route_family.as_deref())
                .or_else(|| control_decision.and_then(|value| value.route_family.as_deref()))
                .unwrap_or_default(),
            route_kind = local_execution_runtime_miss_diagnostic
                .as_ref()
                .and_then(|value| value.route_kind.as_deref())
                .or_else(|| control_decision.and_then(|value| value.route_kind.as_deref()))
                .unwrap_or_default(),
            public_path = local_execution_runtime_miss_diagnostic
                .as_ref()
                .and_then(|value| value.public_path.as_deref())
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .or_else(|| control_decision.map(GatewayControlDecision::proxy_path_and_query))
                .unwrap_or_default(),
            plan_kind = local_execution_runtime_miss_diagnostic
                .as_ref()
                .and_then(|value| value.plan_kind.as_deref())
                .unwrap_or_default(),
            requested_model = local_execution_runtime_miss_diagnostic
                .as_ref()
                .and_then(|value| value.requested_model.as_deref())
                .unwrap_or_default(),
            candidate_count = local_execution_runtime_miss_diagnostic
                .as_ref()
                .and_then(|value| value.candidate_count)
                .unwrap_or(0),
            persisted_candidate_count = local_execution_runtime_miss_context.persisted_candidate_count(),
            skipped_candidate_count = local_execution_runtime_miss_diagnostic
                .as_ref()
                .and_then(|value| value.skipped_candidate_count)
                .unwrap_or(0),
            skip_reasons = local_execution_runtime_miss_diagnostic
                .as_ref()
                .and_then(|value| value.skip_reasons_summary())
                .unwrap_or_default(),
            auth_user_id = local_execution_runtime_miss_context
                .auth_user_id
                .as_deref()
                .unwrap_or_default(),
            auth_api_key_id = local_execution_runtime_miss_context
                .auth_api_key_id
                .as_deref()
                .unwrap_or_default(),
            auth_api_key_name = local_execution_runtime_miss_context
                .auth_api_key_name
                .as_deref()
                .unwrap_or_default(),
            request_candidates = local_execution_runtime_miss_context
                .candidate_summary()
                .unwrap_or_default(),
            local_execution_failure_log
        );
        if let Some(exhaustion) = local_execution_exhaustion {
            record_failed_usage_for_exhausted_request(
                &state,
                exhaustion,
                &started_at,
                local_execution_runtime_miss_detail.as_str(),
                local_execution_failure_path,
                local_execution_runtime_miss_diagnostic.as_ref(),
            )
            .await;
        } else {
            record_failed_usage_for_runtime_miss_request(
                &state,
                &trace_id,
                &started_at,
                local_execution_runtime_miss_detail.as_str(),
                local_execution_failure_path,
                control_decision,
                local_execution_runtime_miss_diagnostic.as_ref(),
                &local_execution_runtime_miss_context,
                &parts.headers,
                Some(buffered_body),
            )
            .await;
        }
        let mut response = build_local_http_error_response(
            &trace_id,
            control_decision,
            http::StatusCode::SERVICE_UNAVAILABLE,
            local_execution_runtime_miss_client_message(
                local_execution_runtime_miss_detail.as_str(),
            )
            .as_str(),
        )?;
        let local_execution_runtime_miss_reason = local_execution_runtime_miss_diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.reason.trim())
            .filter(|reason| !reason.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                auth_api_key_concurrency_limited
                    .then_some("api_key_concurrency_limit_reached".to_string())
            });
        if let Some(reason) = local_execution_runtime_miss_reason {
            response.headers_mut().insert(
                HeaderName::from_static(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER),
                HeaderValue::from_str(reason.as_str())
                    .map_err(|err| GatewayError::Internal(err.to_string()))?,
            );
        }
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            local_execution_failure_path,
            &started_at,
            request_permit.take(),
        ));
    }

    let response = build_local_http_error_response(
        &trace_id,
        control_decision,
        http::StatusCode::NOT_IMPLEMENTED,
        LOCAL_PROXY_PASSTHROUGH_REMOVED_DETAIL,
    )?;
    Ok(finalize_gateway_response_with_context(
        &state,
        response,
        &remote_addr,
        &request_context,
        EXECUTION_PATH_LOCAL_PROXY_PASSTHROUGH_REMOVED,
        &started_at,
        request_permit.take(),
    ))
}

fn local_execution_runtime_miss_detail(
    decision: Option<&GatewayControlDecision>,
    diagnostic: Option<&LocalExecutionRuntimeMissDiagnostic>,
    auth_api_key_concurrency_limited: bool,
    stream_request: bool,
) -> Option<String> {
    if auth_api_key_concurrency_limited
        || diagnostic_is_auth_api_key_concurrency_limited(diagnostic)
    {
        return Some(AUTH_API_KEY_CONCURRENCY_LIMIT_REACHED_DETAIL.to_string());
    }

    if let Some(detail) =
        local_execution_runtime_miss_diagnostic_detail(decision, diagnostic, stream_request)
    {
        return Some(detail);
    }

    local_execution_runtime_miss_route_detail(decision).map(ToOwned::to_owned)
}

fn local_execution_runtime_miss_client_message(detail: &str) -> String {
    beautify_local_execution_client_error_message(detail)
}

fn local_execution_runtime_miss_diagnostic_detail(
    decision: Option<&GatewayControlDecision>,
    diagnostic: Option<&LocalExecutionRuntimeMissDiagnostic>,
    stream_request: bool,
) -> Option<String> {
    let diagnostic = diagnostic?;
    let route_label = local_execution_runtime_miss_route_label(decision);
    let request_mode = local_execution_runtime_miss_request_mode(stream_request);

    match diagnostic.reason.as_str() {
        "candidate_list_empty" => {
            return Some(local_execution_runtime_miss_candidate_list_empty_detail(
                diagnostic,
                request_mode,
            ));
        }
        "all_candidates_skipped" => {
            return Some(local_execution_runtime_miss_all_candidates_skipped_detail(
                diagnostic,
                request_mode,
            ));
        }
        "missing_auth_context" => {
            return Some(format!(
                "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商（{route_label}，原因代码: missing_auth_context）"
            ));
        }
        "missing_requested_model" => {
            return Some(format!(
                "请求缺少 model 字段，无法选择上游提供商（{route_label}，原因代码: missing_requested_model）"
            ));
        }
        "auth_snapshot_missing" => {
            return Some(format!(
                "当前 API Key 的本地执行配置不存在或已过期，无法选择上游提供商（{route_label}，原因代码: auth_snapshot_missing）"
            ));
        }
        "auth_snapshot_read_failed" => {
            return Some(format!(
                "读取 API Key 的本地执行配置失败，无法选择上游提供商（{route_label}，原因代码: auth_snapshot_read_failed）"
            ));
        }
        "decision_input_unavailable" => {
            return Some(format!(
                "请求缺少本地执行所需的认证、模型或配置上下文，无法选择上游提供商（{route_label}，原因代码: decision_input_unavailable）"
            ));
        }
        "execution_runtime_candidates_exhausted" => {
            return Some(format!(
                "已尝试所有本地执行候选提供商，但没有任何候选成功完成请求（{route_label}，原因代码: execution_runtime_candidates_exhausted）"
            ));
        }
        "candidate_evaluation_incomplete" => {
            return Some(format!(
                "本地执行候选评估未完成，暂时无法为本次{request_mode}请求选择上游提供商（{route_label}，原因代码: candidate_evaluation_incomplete）"
            ));
        }
        "no_local_sync_plans" | "no_local_stream_plans" => {
            return Some(format!(
                "找到了候选提供商，但无法为本次{request_mode}请求构建本地执行计划。请检查端点路径、认证方式、Header/Body 规则和格式转换配置（{route_label}，原因代码: {}）",
                diagnostic.reason
            ));
        }
        _ => {}
    }

    let reason = diagnostic.reason.trim();
    if reason.is_empty() {
        None
    } else {
        Some(format!(
            "当前请求无法在本地执行：{route_label} 的执行路径未就绪（原因代码: {reason}）"
        ))
    }
}

fn local_execution_runtime_miss_candidate_list_empty_detail(
    diagnostic: &LocalExecutionRuntimeMissDiagnostic,
    request_mode: &str,
) -> String {
    if let Some(requested_model) = diagnostic_requested_model(diagnostic) {
        return format!(
            "没有可用提供商支持模型 {requested_model} 的{request_mode}请求。请检查模型映射、端点启用状态和 API Key 权限（原因代码: candidate_list_empty）"
        );
    }

    format!(
        "没有可用提供商支持本次{request_mode}请求。请检查模型字段、模型映射、端点启用状态和 API Key 权限（原因代码: candidate_list_empty）"
    )
}

fn local_execution_runtime_miss_all_candidates_skipped_detail(
    diagnostic: &LocalExecutionRuntimeMissDiagnostic,
    request_mode: &str,
) -> String {
    let candidate_count = diagnostic.candidate_count.unwrap_or(0);
    let skipped_count = diagnostic
        .skipped_candidate_count
        .unwrap_or(candidate_count);
    let skipped_summary =
        local_execution_runtime_miss_skip_reasons_summary(&diagnostic.skip_reasons);
    let requested_model = diagnostic_requested_model(diagnostic);

    match (candidate_count, skipped_summary, requested_model) {
        (count, Some(summary), Some(model)) if count > 0 => format!(
            "找到 {count} 个支持模型 {model} 的候选提供商，但本次{request_mode}请求全部不可用：{summary}（原因代码: all_candidates_skipped）"
        ),
        (count, Some(summary), None) if count > 0 => format!(
            "找到 {count} 个候选提供商，但本次{request_mode}请求全部不可用：{summary}（原因代码: all_candidates_skipped）"
        ),
        (_, Some(summary), Some(model)) => format!(
            "支持模型 {model} 的候选提供商全部不可用：{summary}（原因代码: all_candidates_skipped）"
        ),
        (_, Some(summary), None) => format!(
            "候选提供商全部不可用：{summary}（原因代码: all_candidates_skipped）"
        ),
        (count, None, Some(model)) if count > 0 => format!(
            "找到 {count} 个支持模型 {model} 的候选提供商，但都不满足本次{request_mode}请求要求（原因代码: all_candidates_skipped）"
        ),
        (count, None, None) if count > 0 => format!(
            "找到 {count} 个候选提供商，但都不满足本次{request_mode}请求要求（原因代码: all_candidates_skipped）"
        ),
        (_, None, Some(model)) if skipped_count > 0 => format!(
            "支持模型 {model} 的 {skipped_count} 个候选提供商都不满足本次{request_mode}请求要求（原因代码: all_candidates_skipped）"
        ),
        _ => format!(
            "候选提供商都不满足本次{request_mode}请求要求（原因代码: all_candidates_skipped）"
        ),
    }
}

fn diagnostic_requested_model(diagnostic: &LocalExecutionRuntimeMissDiagnostic) -> Option<&str> {
    diagnostic
        .requested_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn local_execution_runtime_miss_skip_reasons_summary(
    skip_reasons: &BTreeMap<String, usize>,
) -> Option<String> {
    if skip_reasons.is_empty() {
        return None;
    }

    Some(
        skip_reasons
            .iter()
            .map(|(reason, count)| {
                format!(
                    "{} {} 次",
                    local_execution_runtime_miss_skip_reason_label(reason),
                    count
                )
            })
            .collect::<Vec<_>>()
            .join("，"),
    )
}

fn local_execution_runtime_miss_skip_reason_label(reason: &str) -> &str {
    match reason {
        "api_key_concurrency_limit_reached" => "API Key 并发已达上限",
        "auth_snapshot_missing" => "API Key 本地执行配置缺失",
        "endpoint_api_format_changed" => "端点 API 格式已变更",
        "endpoint_inactive" => "端点未启用",
        "format_conversion_disabled" => "格式转换未启用",
        "key_api_format_disabled" => "API Key 未启用该 API 格式",
        "key_inactive" => "API Key 未启用",
        "key_model_disabled" => "API Key 未允许该模型",
        "mapped_model_missing" => "模型映射缺失",
        "provider_inactive" => "提供商未启用",
        "provider_request_body_missing" => "无法构建上游请求体",
        "provider_request_body_build_failed" => "上游请求体转换失败",
        "transport_api_format_mismatch" => "传输层 API 格式不匹配",
        "transport_api_format_unsupported" => "传输层不支持该 API 格式",
        "transport_auth_unavailable" => "上游认证信息不可用",
        "transport_body_rules_unsupported" => "Body 规则不支持本地执行",
        "transport_custom_path_unsupported" => "自定义路径不支持本地执行",
        "transport_header_rules_unsupported" => "Header 规则不支持本地执行",
        "transport_header_rules_apply_failed" => "Header 规则应用失败",
        "transport_oauth_resolution_unsupported" => "OAuth 认证解析不支持本地执行",
        "transport_provider_type_unsupported" => "提供商类型不支持本地执行",
        "transport_proxy_or_tls_unsupported" => "代理或 TLS 配置不支持本地执行",
        "transport_proxy_unsupported" => "代理配置不支持本地执行",
        "transport_snapshot_missing" => "提供商传输配置缺失",
        "transport_tls_profile_unsupported" => "TLS 指纹配置不支持本地执行",
        "transport_unsupported" => "传输配置不支持本地执行",
        "upstream_url_missing" => "无法构建上游请求地址",
        other => other,
    }
}

fn local_execution_runtime_miss_request_mode(stream_request: bool) -> &'static str {
    if stream_request {
        "流式"
    } else {
        "同步"
    }
}

fn local_execution_runtime_miss_route_label(
    decision: Option<&GatewayControlDecision>,
) -> &'static str {
    let Some(decision) = decision else {
        return "AI 请求";
    };
    match decision.public_path.as_str() {
        "/v1/chat/completions" => "OpenAI Chat Completions",
        "/v1/responses" => "OpenAI Responses",
        "/v1/responses/compact" => "OpenAI Responses Compact",
        "/v1/messages" => "Claude Messages",
        path if path.starts_with("/v1/videos") => "OpenAI Video",
        path if path.starts_with("/upload/v1beta/files") || path.starts_with("/v1beta/files") => {
            "Gemini Files"
        }
        path if decision.route_family.as_deref() == Some("gemini")
            && (path.starts_with("/v1beta/models/") || path.starts_with("/v1/models/")) =>
        {
            "Gemini Public"
        }
        _ => "AI 请求",
    }
}

fn diagnostic_is_auth_api_key_concurrency_limited(
    diagnostic: Option<&LocalExecutionRuntimeMissDiagnostic>,
) -> bool {
    let Some(diagnostic) = diagnostic else {
        return false;
    };
    diagnostic.reason == "api_key_concurrency_limit_reached"
        || (diagnostic.reason == "all_candidates_skipped"
            && diagnostic.skip_reasons.len() == 1
            && diagnostic
                .skip_reasons
                .get("api_key_concurrency_limit_reached")
                .copied()
                .unwrap_or(0)
                > 0)
}

fn local_execution_runtime_miss_route_detail(
    decision: Option<&GatewayControlDecision>,
) -> Option<&'static str> {
    let decision = decision?;
    if decision.route_class.as_deref() != Some("ai_public") {
        return None;
    }
    let public_path = decision.public_path.as_str();
    match public_path {
        "/v1/chat/completions" => Some(OPENAI_CHAT_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL),
        "/v1/responses" => Some(OPENAI_RESPONSES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL),
        "/v1/responses/compact" => {
            Some(OPENAI_RESPONSES_COMPACT_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL)
        }
        "/v1/messages" => Some(CLAUDE_MESSAGES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL),
        path if path.starts_with("/v1/videos") => {
            Some(OPENAI_VIDEO_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL)
        }
        path if path.starts_with("/upload/v1beta/files") || path.starts_with("/v1beta/files") => {
            Some(GEMINI_FILES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL)
        }
        path if decision.route_family.as_deref() == Some("gemini")
            && (path.starts_with("/v1beta/models/") || path.starts_with("/v1/models/")) =>
        {
            Some(GEMINI_PUBLIC_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        diagnostic_is_auth_api_key_concurrency_limited, local_execution_runtime_miss_detail,
        GatewayControlDecision, LocalExecutionRuntimeMissDiagnostic,
    };

    #[test]
    fn runtime_miss_detail_returns_model_specific_stream_message_when_candidates_are_unavailable() {
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );
        let diagnostic = LocalExecutionRuntimeMissDiagnostic {
            reason: "candidate_list_empty".to_string(),
            requested_model: Some("gpt-5.4".to_string()),
            ..LocalExecutionRuntimeMissDiagnostic::default()
        };

        let detail =
            local_execution_runtime_miss_detail(Some(&decision), Some(&diagnostic), false, true);

        assert_eq!(
            detail.as_deref(),
            Some(
                "没有可用提供商支持模型 gpt-5.4 的流式请求。请检查模型映射、端点启用状态和 API Key 权限（原因代码: candidate_list_empty）"
            )
        );
    }

    #[test]
    fn runtime_miss_detail_returns_auth_context_message_when_auth_context_is_missing() {
        let decision = GatewayControlDecision::synthetic(
            "/v1/messages",
            Some("ai_public".to_string()),
            Some("claude".to_string()),
            Some("messages".to_string()),
            Some("claude:messages".to_string()),
        );
        let diagnostic = LocalExecutionRuntimeMissDiagnostic {
            reason: "missing_auth_context".to_string(),
            requested_model: Some("claude-sonnet-4-5".to_string()),
            ..LocalExecutionRuntimeMissDiagnostic::default()
        };

        let detail =
            local_execution_runtime_miss_detail(Some(&decision), Some(&diagnostic), false, false);

        assert_eq!(
            detail.as_deref(),
            Some(
                "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商（Claude Messages，原因代码: missing_auth_context）"
            )
        );
    }

    #[test]
    fn runtime_miss_detail_returns_api_key_concurrency_message_for_exact_all_skipped_limit_case() {
        let decision = GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("responses".to_string()),
            Some("openai:responses".to_string()),
        );
        let diagnostic = LocalExecutionRuntimeMissDiagnostic {
            reason: "all_candidates_skipped".to_string(),
            skip_reasons: std::collections::BTreeMap::from([(
                "api_key_concurrency_limit_reached".to_string(),
                1,
            )]),
            requested_model: Some("gpt-5.4".to_string()),
            ..LocalExecutionRuntimeMissDiagnostic::default()
        };

        let detail =
            local_execution_runtime_miss_detail(Some(&decision), Some(&diagnostic), false, false);

        assert_eq!(
            detail.as_deref(),
            Some("当前 API Key 并发请求数已达上限，请稍后重试")
        );
        assert!(diagnostic_is_auth_api_key_concurrency_limited(Some(
            &diagnostic
        )));
    }

    #[test]
    fn runtime_miss_detail_prefers_api_key_concurrency_message_when_classified_from_context() {
        let decision = GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("responses".to_string()),
            Some("openai:responses".to_string()),
        );
        let diagnostic = LocalExecutionRuntimeMissDiagnostic {
            reason: "all_candidates_skipped".to_string(),
            skip_reasons: std::collections::BTreeMap::from([(
                "format_conversion_disabled".to_string(),
                1,
            )]),
            requested_model: Some("gpt-5.4".to_string()),
            ..LocalExecutionRuntimeMissDiagnostic::default()
        };

        let detail =
            local_execution_runtime_miss_detail(Some(&decision), Some(&diagnostic), true, false);

        assert_eq!(
            detail.as_deref(),
            Some("当前 API Key 并发请求数已达上限，请稍后重试")
        );
    }
}

#[path = "finalize.rs"]
mod finalize;

use self::finalize::{
    finalize_gateway_response, finalize_gateway_response_with_context, request_wants_stream,
};
