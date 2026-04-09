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
    EXECUTION_PATH_LOCAL_AI_PUBLIC, EXECUTION_PATH_LOCAL_AUTH_DENIED,
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS, EXECUTION_PATH_LOCAL_OVERLOADED,
    EXECUTION_PATH_LOCAL_PROXY_PASSTHROUGH_REMOVED, EXECUTION_PATH_LOCAL_RATE_LIMITED,
    EXECUTION_PATH_LOCAL_ROUTE_NOT_FOUND, EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH,
    FORWARDED_FOR_HEADER, FORWARDED_HOST_HEADER, FORWARDED_PROTO_HEADER, GATEWAY_HEADER,
    LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER, TRACE_ID_HEADER,
    TRUSTED_AUTH_ACCESS_ALLOWED_HEADER, TRUSTED_AUTH_API_KEY_ID_HEADER,
    TRUSTED_AUTH_BALANCE_HEADER, TRUSTED_AUTH_USER_ID_HEADER, TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
    TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
};
use crate::control::{
    allows_control_execute_emergency, maybe_execute_via_control, request_model_local_rejection,
    should_buffer_request_for_local_auth, trusted_auth_local_rejection, GatewayControlDecision,
    GatewayPublicRequestContext,
};
use crate::executor::{maybe_execute_stream_request, maybe_execute_sync_request};
use crate::handlers::shared::{
    build_admin_proxy_auth_required_response, build_unhandled_admin_proxy_response,
    local_proxy_route_requires_buffered_body, request_enables_control_execute,
    should_strip_forwarded_provider_credential_header, should_strip_forwarded_trusted_admin_header,
};
use crate::headers::{extract_or_generate_trace_id, should_skip_request_header};
use crate::router::RequestAdmissionError;
use crate::{
    AppState, FrontdoorUserRpmOutcome, GatewayError, GatewayFallbackMetricKind,
    GatewayFallbackReason,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{self, header::HeaderName, header::HeaderValue, Response};
use chrono::Utc;
use std::time::Instant;
use tracing::{info, warn};

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
const EXECUTION_PATH_TUNNEL_AFFINITY_FORWARD: &str = "tunnel_affinity_forward";

fn execution_runtime_candidate_header_value(decision: &GatewayControlDecision) -> &'static str {
    if decision.is_execution_runtime_candidate() {
        "true"
    } else {
        "false"
    }
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

    let Some(proxy) = crate::provider_transport::resolve_transport_proxy_snapshot(&transport)
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
    let request_context_started_at = Instant::now();
    let request_context = crate::control::resolve_public_request_context(
        &state,
        &parts.method,
        &parts.uri,
        &parts.headers,
        &trace_id,
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
        if stream_request {
            if let Some(execution_runtime_response) = maybe_execute_stream_request(
                &state,
                &parts,
                buffered_body,
                &trace_id,
                control_decision,
            )
            .await?
            {
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
        }
        if let Some(execution_runtime_response) =
            maybe_execute_sync_request(&state, &parts, buffered_body, &trace_id, control_decision)
                .await?
        {
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
        if parts.method != http::Method::POST {
            if let Some(execution_runtime_response) = maybe_execute_stream_request(
                &state,
                &parts,
                buffered_body,
                &trace_id,
                control_decision,
            )
            .await?
            {
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
        }
        if allow_control_execute_fallback {
            if let Some(control_response) = maybe_execute_via_control(
                &state,
                &parts,
                buffered_body.clone(),
                &trace_id,
                control_decision,
                stream_request,
            )
            .await?
            {
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
        }
        let local_execution_runtime_miss_detail =
            local_execution_runtime_miss_detail(control_decision)
                .unwrap_or("AI public execution runtime miss did not match a Rust execution path");
        state.record_fallback_metric(
            GatewayFallbackMetricKind::LocalExecutionRuntimeMiss,
            control_decision,
            None,
            Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS),
            GatewayFallbackReason::LocalExecutionPathRequired,
        );
        let local_execution_runtime_miss_diagnostic =
            state.take_local_execution_runtime_miss_diagnostic(&trace_id);
        if let Some(diagnostic) = local_execution_runtime_miss_diagnostic.as_ref() {
            warn!(
                trace_id = %trace_id,
                local_execution_runtime_miss_reason = %diagnostic.reason,
                route_family = diagnostic.route_family.as_deref().unwrap_or_default(),
                route_kind = diagnostic.route_kind.as_deref().unwrap_or_default(),
                public_path = diagnostic.public_path.as_deref().unwrap_or_default(),
                plan_kind = diagnostic.plan_kind.as_deref().unwrap_or_default(),
                requested_model = diagnostic.requested_model.as_deref().unwrap_or_default(),
                candidate_count = diagnostic.candidate_count.unwrap_or(0),
                skipped_candidate_count = diagnostic.skipped_candidate_count.unwrap_or(0),
                skip_reasons = diagnostic.skip_reasons_summary().unwrap_or_default(),
                "gateway local execution runtime miss"
            );
        }
        let mut response = build_local_http_error_response(
            &trace_id,
            control_decision,
            http::StatusCode::SERVICE_UNAVAILABLE,
            local_execution_runtime_miss_detail,
        )?;
        if let Some(diagnostic) = local_execution_runtime_miss_diagnostic {
            if !diagnostic.reason.trim().is_empty() {
                response.headers_mut().insert(
                    HeaderName::from_static(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER),
                    HeaderValue::from_str(diagnostic.reason.as_str())
                        .map_err(|err| GatewayError::Internal(err.to_string()))?,
                );
            }
        }
        return Ok(finalize_gateway_response_with_context(
            &state,
            response,
            &remote_addr,
            &request_context,
            EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS,
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

#[path = "finalize.rs"]
mod finalize;

use self::finalize::{
    finalize_gateway_response, finalize_gateway_response_with_context, request_wants_stream,
};
