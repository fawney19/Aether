mod local;

use self::local::{
    maybe_build_local_admin_proxy_response, maybe_build_local_internal_proxy_response,
};
use super::internal::resolve_local_proxy_execution_path;
pub(crate) use super::public::matches_model_mapping_for_models;
use crate::ai_pipeline_api;
use crate::api::response::{
    build_client_response, build_local_auth_rejection_response, build_local_http_error_response,
    build_local_overloaded_response, build_local_user_rpm_limited_response,
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
    build_local_execution_runtime_miss_context, maybe_execute_stream_request,
    maybe_execute_sync_request, record_failed_usage_for_exhausted_request,
    record_failed_usage_for_runtime_miss_request, LocalExecutionRequestOutcome,
};
use crate::frontdoor_loop_guard::{
    frontdoor_self_loop_public_ai_path, request_has_execution_runtime_loop_guard,
};
use crate::handlers::shared::{
    build_admin_proxy_auth_required_response, build_unhandled_admin_proxy_response,
    local_proxy_route_requires_buffered_body, request_enables_control_execute,
    should_strip_forwarded_provider_credential_header, should_strip_forwarded_trusted_admin_header,
};
use crate::headers::{extract_or_generate_trace_id, should_skip_request_header};
use crate::router::RequestAdmissionError;
use crate::{
    AppState, FrontdoorUserRpmOutcome, GatewayError, GatewayFallbackMetricKind,
    GatewayFallbackReason, LocalExecutionRuntimeMissDiagnostic,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{self, header::HeaderName, header::HeaderValue, Response};
use sha2::{Digest, Sha256};
use std::time::Instant;
use tracing::{debug, info, warn};

const OPENAI_CHAT_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "OpenAI chat execution runtime miss did not match a Rust execution path";
const OPENAI_RESPONSES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "OpenAI responses execution runtime miss did not match a Rust execution path";
const OPENAI_COMPACT_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "OpenAI compact execution runtime miss did not match a Rust execution path";
const OPENAI_VIDEO_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "OpenAI video execution runtime miss did not match a Rust execution path";
const CLAUDE_MESSAGES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "Claude messages execution runtime miss did not match a Rust execution path";
const GEMINI_PUBLIC_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "Gemini public execution runtime miss did not match a Rust execution path";
const GEMINI_FILES_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL: &str =
    "Gemini files execution runtime miss did not match a Rust execution path";
const LOCAL_ROUTE_NOT_FOUND_DETAIL: &str = "Route not found";
const LOCAL_PROXY_PASSTHROUGH_REMOVED_DETAIL: &str =
    "Route matched a removed compatibility passthrough; implement it in Rust or retire the route";
const LOCAL_EXECUTION_LOOP_DETECTED_DETAIL: &str =
    "Gateway detected an execution runtime request loop back into the local frontdoor";
const AUTH_API_KEY_CONCURRENCY_LIMIT_REACHED_DETAIL: &str =
    "当前 API Key 并发请求数已达上限，请稍后重试";
const EXECUTION_PATH_TUNNEL_AFFINITY_FORWARD: &str = "tunnel_affinity_forward";

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
    (!token.is_empty() && token.starts_with("ae_")).then_some(token)
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

    let mut response =
        build_client_response(upstream_response, &request_context.trace_id, Some(decision))?;
    response.headers_mut().insert(
        HeaderName::from_static(TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER),
        HeaderValue::from_str(owner.gateway_instance_id.as_str())
            .map_err(|err| GatewayError::Internal(err.to_string()))?,
    );
    Ok(Some(response))
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
    let (parts, body) = request.into_parts();
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
            control_decision,
            &parts.uri,
            &parts.headers,
            buffered_body,
        ) {
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
        .unwrap_or_else(|| {
            "AI public execution runtime miss did not match a Rust execution path".to_string()
        });
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
            )
            .await;
        }
        let mut response = build_local_http_error_response(
            &trace_id,
            control_decision,
            http::StatusCode::SERVICE_UNAVAILABLE,
            local_execution_runtime_miss_detail.as_str(),
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

    if let Some(detail) = local_execution_runtime_miss_model_detail(diagnostic, stream_request) {
        return Some(detail);
    }

    local_execution_runtime_miss_route_detail(decision).map(ToOwned::to_owned)
}

fn local_execution_runtime_miss_model_detail(
    diagnostic: Option<&LocalExecutionRuntimeMissDiagnostic>,
    stream_request: bool,
) -> Option<String> {
    let diagnostic = diagnostic?;
    if !matches!(
        diagnostic.reason.as_str(),
        "candidate_list_empty" | "all_candidates_skipped"
    ) {
        return None;
    }

    let requested_model = diagnostic
        .requested_model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let request_mode = if stream_request { "流式" } else { "同步" };
    Some(format!(
        "没有可用的提供商支持模型 {requested_model} 的{request_mode}请求"
    ))
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
        "/v1/responses/compact" => Some(OPENAI_COMPACT_LOCAL_EXECUTION_RUNTIME_MISS_DETAIL),
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
            Some("没有可用的提供商支持模型 gpt-5.4 的流式请求")
        );
    }

    #[test]
    fn runtime_miss_detail_falls_back_to_route_default_when_reason_is_not_model_unavailable() {
        let decision = GatewayControlDecision::synthetic(
            "/v1/messages",
            Some("ai_public".to_string()),
            Some("claude".to_string()),
            Some("chat".to_string()),
            Some("claude:chat".to_string()),
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
            Some("Claude messages execution runtime miss did not match a Rust execution path")
        );
    }

    #[test]
    fn runtime_miss_detail_returns_api_key_concurrency_message_for_exact_all_skipped_limit_case() {
        let decision = GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("cli".to_string()),
            Some("openai:cli".to_string()),
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
            Some("cli".to_string()),
            Some("openai:cli".to_string()),
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
