use super::{
    attach_execution_path_header, build_internal_control_error_response,
    build_internal_finalize_decision, build_internal_gateway_fallback_plan_payload,
    build_internal_gateway_header_map, build_internal_gateway_passthrough_payload,
    build_internal_gateway_proxy_public_response, build_internal_gateway_request_parts,
    build_internal_gateway_resolve_payload, build_internal_gateway_uri,
    build_internal_tunnel_heartbeat_ack, build_management_token_payload,
    internal_finalize_report_kind_is_supported, maybe_build_internal_finalize_video_response,
    parse_internal_tunnel_heartbeat_request, parse_internal_tunnel_node_status_request,
};
use crate::ai_serving::api;
use crate::constants::{
    CONTROL_EXECUTED_HEADER, EXECUTION_PATH_EXECUTION_RUNTIME_STREAM,
    EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
};
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::execution_runtime::{execute_execution_runtime_stream, execute_execution_runtime_sync};
use crate::handlers::shared::{
    InternalGatewayAuthContextRequest, InternalGatewayExecuteRequest, InternalGatewayResolveRequest,
};
use crate::tunnel::{claim_tunnel_heartbeat, finish_tunnel_heartbeat_claim};
use crate::tunnel::{is_tunnel_heartbeat_path, is_tunnel_node_status_path, TUNNEL_ROUTE_FAMILY};
use crate::{AppState, GatewayError};
use aether_data::repository::proxy_nodes::{
    ProxyNodeHeartbeatMutation, ProxyNodeTunnelStatusMutation,
};
use axum::body::{Body, Bytes};
use axum::http::{self, HeaderName, HeaderValue, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};

fn reject_supplied_auth_context(
    auth_context: Option<&crate::control::GatewayControlAuthContext>,
) -> Result<(), Response<Body>> {
    if auth_context.is_some() {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "supplied auth_context is not accepted; authenticate through request headers",
        ));
    }
    Ok(())
}

fn internal_gateway_data_error_response(operation: &'static str) -> Response<Body> {
    tracing::error!(
        event_name = "internal_gateway_data_error",
        operation,
        error_category = "repository_unavailable",
        "internal gateway data operation failed"
    );
    build_internal_control_error_response(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        "internal gateway data unavailable",
    )
}

async fn resolve_bound_internal_report_context(
    state: &AppState,
    trace_id: &str,
    report_kind: &str,
    report_context: Option<&Value>,
    operation: &'static str,
) -> Result<Value, Response<Body>> {
    match crate::usage::resolve_bound_internal_gateway_report_context(
        state,
        trace_id,
        report_kind,
        report_context,
    )
    .await
    {
        Ok(Some(report_context)) => Ok(report_context),
        Ok(None) => {
            tracing::warn!(
                event_name = "internal_gateway_report_context_rejected",
                operation,
                error_category = "unbound_report_context",
                "internal gateway report context did not carry a valid planner capability"
            );
            Err(build_internal_control_error_response(
                http::StatusCode::CONFLICT,
                "internal gateway report context does not carry a valid planner capability",
            ))
        }
        Err(_) => Err(internal_gateway_data_error_response(operation)),
    }
}

pub(crate) async fn maybe_build_local_internal_proxy_response_impl(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    remote_addr: &std::net::SocketAddr,
    request_headers: &http::HeaderMap,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };
    if decision.route_class.as_deref() != Some("internal_proxy") {
        return Ok(None);
    }
    if decision.route_family.as_deref() == Some("internal_gateway") {
        if request_context.request_method != http::Method::POST
            || decision.route_kind.as_deref() == Some("unhandled")
        {
            return Ok(Some(build_internal_control_error_response(
                http::StatusCode::NOT_FOUND,
                "route not found",
            )));
        }
        let authenticated_body = request_body.map_or(&[][..], Bytes::as_ref);
        if let Err(error) = crate::internal_gateway_auth::authenticate_internal_gateway_request(
            state,
            remote_addr,
            &request_context.request_method,
            &request_context.request_path_and_query(),
            request_headers,
            authenticated_body,
        )
        .await
        {
            let (status, message) = match error {
                crate::internal_gateway_auth::InternalGatewayAuthError::Disabled => {
                    (http::StatusCode::NOT_FOUND, "route not found")
                }
                crate::internal_gateway_auth::InternalGatewayAuthError::Invalid => (
                    http::StatusCode::FORBIDDEN,
                    "invalid internal gateway authentication",
                ),
                crate::internal_gateway_auth::InternalGatewayAuthError::Unavailable => (
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "internal gateway authentication unavailable",
                ),
            };
            return Ok(Some(build_internal_control_error_response(status, message)));
        }
        match decision.route_kind.as_deref() {
            Some("resolve") if request_context.request_path == "/api/internal/gateway/resolve" => {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway resolve payload",
                    )));
                };
                let payload =
                    match serde_json::from_slice::<InternalGatewayResolveRequest>(request_body) {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(build_internal_control_error_response(
                                http::StatusCode::BAD_REQUEST,
                                "invalid internal gateway resolve payload",
                            )));
                        }
                    };
                let headers = match build_internal_gateway_header_map(&payload.headers) {
                    Ok(headers) => headers,
                    Err(response) => return Ok(Some(response)),
                };
                let method = match http::Method::from_bytes(payload.method.as_bytes()) {
                    Ok(method) => method,
                    Err(_) => {
                        return Ok(Some(build_internal_control_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid internal gateway method",
                        )));
                    }
                };
                let uri = match build_internal_gateway_uri(
                    &payload.path,
                    payload.query_string.as_deref(),
                ) {
                    Ok(uri) => uri,
                    Err(response) => return Ok(Some(response)),
                };
                let resolved = crate::control::resolve_control_route(
                    state,
                    &method,
                    &uri,
                    &headers,
                    payload
                        .trace_id
                        .as_deref()
                        .unwrap_or(request_context.trace_id.as_str()),
                )
                .await?;
                let response_payload = resolved
                    .map(build_internal_gateway_resolve_payload)
                    .unwrap_or_else(|| build_internal_gateway_passthrough_payload(&uri));
                return Ok(Some(Json(response_payload).into_response()));
            }
            Some("auth_context")
                if request_context.request_path == "/api/internal/gateway/auth-context" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway auth-context payload",
                    )));
                };
                let payload =
                    match serde_json::from_slice::<InternalGatewayAuthContextRequest>(request_body)
                    {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(build_internal_control_error_response(
                                http::StatusCode::BAD_REQUEST,
                                "invalid internal gateway auth-context payload",
                            )));
                        }
                    };
                let headers = match build_internal_gateway_header_map(&payload.headers) {
                    Ok(headers) => headers,
                    Err(response) => return Ok(Some(response)),
                };
                let uri = match build_internal_gateway_uri("/", payload.query_string.as_deref()) {
                    Ok(uri) => uri,
                    Err(response) => return Ok(Some(response)),
                };
                let mut synthetic_decision = GatewayControlDecision::synthetic(
                    "/",
                    Some("internal_proxy".to_string()),
                    Some("internal_gateway".to_string()),
                    Some("auth_context".to_string()),
                    Some(payload.auth_endpoint_signature),
                );
                synthetic_decision.public_query_string = uri.query().map(ToOwned::to_owned);
                let auth_context = crate::control::resolve_execution_runtime_auth_context(
                    state,
                    &synthetic_decision,
                    &headers,
                    &uri,
                    payload
                        .trace_id
                        .as_deref()
                        .unwrap_or(request_context.trace_id.as_str()),
                )
                .await?;
                return Ok(Some(
                    Json(json!({ "auth_context": auth_context })).into_response(),
                ));
            }
            Some("decision_sync")
                if request_context.request_path == "/api/internal/gateway/decision-sync" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway decision-sync payload",
                    )));
                };
                let payload =
                    match serde_json::from_slice::<InternalGatewayExecuteRequest>(request_body) {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(build_internal_control_error_response(
                                http::StatusCode::BAD_REQUEST,
                                "invalid internal gateway decision-sync payload",
                            )));
                        }
                    };
                if let Err(response) = reject_supplied_auth_context(payload.auth_context.as_ref()) {
                    return Ok(Some(response));
                }
                let parts = match build_internal_gateway_request_parts(
                    &payload.method,
                    &payload.path,
                    payload.query_string.as_deref(),
                    &payload.headers,
                ) {
                    Ok(parts) => parts,
                    Err(response) => return Ok(Some(response)),
                };
                let trace_id = payload
                    .trace_id
                    .as_deref()
                    .unwrap_or(request_context.trace_id.as_str())
                    .to_string();
                let body_is_empty = payload.body_base64.is_none()
                    && payload
                        .body_json
                        .as_object()
                        .map(|value| value.is_empty())
                        .unwrap_or(false);
                let Some(mut resolved) = crate::control::resolve_control_route(
                    state,
                    &parts.method,
                    &parts.uri,
                    &parts.headers,
                    trace_id.as_str(),
                )
                .await?
                else {
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(None)).into_response(),
                    ));
                };
                let auth_context = resolved.auth_context.as_ref();
                if auth_context
                    .map(|value| !value.access_allowed)
                    .unwrap_or(true)
                {
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(auth_context))
                            .into_response(),
                    ));
                }
                let Some(mut local_payload) = api::maybe_build_sync_decision_payload(
                    state,
                    &parts,
                    trace_id.as_str(),
                    &resolved,
                    &payload.body_json,
                    payload.body_base64.as_deref(),
                    body_is_empty,
                )
                .await?
                else {
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(auth_context))
                            .into_response(),
                    ));
                };
                let report_kind = local_payload.report_kind.clone();
                crate::usage::attach_internal_gateway_report_capability(
                    state,
                    trace_id.as_str(),
                    report_kind.as_deref(),
                    &local_payload.provider_request_headers,
                    &mut local_payload.report_context,
                )
                .await?;
                return Ok(Some(Json(local_payload).into_response()));
            }
            Some("decision_stream")
                if request_context.request_path == "/api/internal/gateway/decision-stream" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway decision-stream payload",
                    )));
                };
                let payload =
                    match serde_json::from_slice::<InternalGatewayExecuteRequest>(request_body) {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(build_internal_control_error_response(
                                http::StatusCode::BAD_REQUEST,
                                "invalid internal gateway decision-stream payload",
                            )));
                        }
                    };
                if let Err(response) = reject_supplied_auth_context(payload.auth_context.as_ref()) {
                    return Ok(Some(response));
                }
                let parts = match build_internal_gateway_request_parts(
                    &payload.method,
                    &payload.path,
                    payload.query_string.as_deref(),
                    &payload.headers,
                ) {
                    Ok(parts) => parts,
                    Err(response) => return Ok(Some(response)),
                };
                let trace_id = payload
                    .trace_id
                    .as_deref()
                    .unwrap_or(request_context.trace_id.as_str())
                    .to_string();
                let body_is_empty = payload.body_base64.is_none()
                    && payload
                        .body_json
                        .as_object()
                        .map(|value| value.is_empty())
                        .unwrap_or(false);
                let Some(mut resolved) = crate::control::resolve_control_route(
                    state,
                    &parts.method,
                    &parts.uri,
                    &parts.headers,
                    trace_id.as_str(),
                )
                .await?
                else {
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(None)).into_response(),
                    ));
                };
                let auth_context = resolved.auth_context.as_ref();
                if auth_context
                    .map(|value| !value.access_allowed)
                    .unwrap_or(true)
                {
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(auth_context))
                            .into_response(),
                    ));
                }
                let Some(mut local_payload) = api::maybe_build_stream_decision_payload(
                    state,
                    &parts,
                    trace_id.as_str(),
                    &resolved,
                    &payload.body_json,
                    payload.body_base64.as_deref(),
                )
                .await?
                else {
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(auth_context))
                            .into_response(),
                    ));
                };
                let report_kind = local_payload.report_kind.clone();
                crate::usage::attach_internal_gateway_report_capability(
                    state,
                    trace_id.as_str(),
                    report_kind.as_deref(),
                    &local_payload.provider_request_headers,
                    &mut local_payload.report_context,
                )
                .await?;
                return Ok(Some(Json(local_payload).into_response()));
            }
            Some("plan_sync")
                if request_context.request_path == "/api/internal/gateway/plan-sync" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway plan-sync payload",
                    )));
                };
                let payload =
                    match serde_json::from_slice::<InternalGatewayExecuteRequest>(request_body) {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(build_internal_control_error_response(
                                http::StatusCode::BAD_REQUEST,
                                "invalid internal gateway plan-sync payload",
                            )));
                        }
                    };
                if let Err(response) = reject_supplied_auth_context(payload.auth_context.as_ref()) {
                    return Ok(Some(response));
                }
                let parts = match build_internal_gateway_request_parts(
                    &payload.method,
                    &payload.path,
                    payload.query_string.as_deref(),
                    &payload.headers,
                ) {
                    Ok(parts) => parts,
                    Err(response) => return Ok(Some(response)),
                };
                let trace_id = payload
                    .trace_id
                    .as_deref()
                    .unwrap_or(request_context.trace_id.as_str())
                    .to_string();
                let body_is_empty = payload.body_base64.is_none()
                    && payload
                        .body_json
                        .as_object()
                        .map(|value| value.is_empty())
                        .unwrap_or(false);
                let Some(mut resolved) = crate::control::resolve_control_route(
                    state,
                    &parts.method,
                    &parts.uri,
                    &parts.headers,
                    trace_id.as_str(),
                )
                .await?
                else {
                    return Ok(Some(build_internal_gateway_proxy_public_response()));
                };
                if let Some(mut planned) = api::maybe_build_sync_plan_payload(
                    state,
                    &parts,
                    trace_id.as_str(),
                    &resolved,
                    &payload.body_json,
                    payload.body_base64.as_deref(),
                    body_is_empty,
                )
                .await?
                {
                    let report_kind = planned.report_kind.clone();
                    let provider_request_headers = planned
                        .plan
                        .as_ref()
                        .map(|plan| &plan.headers)
                        .ok_or_else(|| {
                            crate::GatewayError::Internal(
                                "internal gateway sync plan omitted its execution plan".to_string(),
                            )
                        })?;
                    crate::usage::attach_internal_gateway_report_capability(
                        state,
                        trace_id.as_str(),
                        report_kind.as_deref(),
                        provider_request_headers,
                        &mut planned.report_context,
                    )
                    .await?;
                    return Ok(Some(Json(planned).into_response()));
                }
                return Ok(Some(build_internal_gateway_proxy_public_response()));
            }
            Some("plan_stream")
                if request_context.request_path == "/api/internal/gateway/plan-stream" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway plan-stream payload",
                    )));
                };
                let payload =
                    match serde_json::from_slice::<InternalGatewayExecuteRequest>(request_body) {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(build_internal_control_error_response(
                                http::StatusCode::BAD_REQUEST,
                                "invalid internal gateway plan-stream payload",
                            )));
                        }
                    };
                if let Err(response) = reject_supplied_auth_context(payload.auth_context.as_ref()) {
                    return Ok(Some(response));
                }
                let parts = match build_internal_gateway_request_parts(
                    &payload.method,
                    &payload.path,
                    payload.query_string.as_deref(),
                    &payload.headers,
                ) {
                    Ok(parts) => parts,
                    Err(response) => return Ok(Some(response)),
                };
                let trace_id = payload
                    .trace_id
                    .as_deref()
                    .unwrap_or(request_context.trace_id.as_str())
                    .to_string();
                let Some(mut resolved) = crate::control::resolve_control_route(
                    state,
                    &parts.method,
                    &parts.uri,
                    &parts.headers,
                    trace_id.as_str(),
                )
                .await?
                else {
                    return Ok(Some(build_internal_gateway_proxy_public_response()));
                };
                if let Some(mut planned) = api::maybe_build_stream_plan_payload(
                    state,
                    &parts,
                    trace_id.as_str(),
                    &resolved,
                    &payload.body_json,
                    payload.body_base64.as_deref(),
                )
                .await?
                {
                    let report_kind = planned.report_kind.clone();
                    let provider_request_headers = planned
                        .plan
                        .as_ref()
                        .map(|plan| &plan.headers)
                        .ok_or_else(|| {
                            crate::GatewayError::Internal(
                                "internal gateway stream plan omitted its execution plan"
                                    .to_string(),
                            )
                        })?;
                    crate::usage::attach_internal_gateway_report_capability(
                        state,
                        trace_id.as_str(),
                        report_kind.as_deref(),
                        provider_request_headers,
                        &mut planned.report_context,
                    )
                    .await?;
                    return Ok(Some(Json(planned).into_response()));
                }
                return Ok(Some(build_internal_gateway_proxy_public_response()));
            }
            Some("execute_sync")
                if request_context.request_path == "/api/internal/gateway/execute-sync" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway execute-sync payload",
                    )));
                };
                let payload =
                    match serde_json::from_slice::<InternalGatewayExecuteRequest>(request_body) {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(build_internal_control_error_response(
                                http::StatusCode::BAD_REQUEST,
                                "invalid internal gateway execute-sync payload",
                            )));
                        }
                    };
                if let Err(response) = reject_supplied_auth_context(payload.auth_context.as_ref()) {
                    return Ok(Some(response));
                }
                let parts = match build_internal_gateway_request_parts(
                    &payload.method,
                    &payload.path,
                    payload.query_string.as_deref(),
                    &payload.headers,
                ) {
                    Ok(parts) => parts,
                    Err(response) => return Ok(Some(response)),
                };
                let trace_id = payload
                    .trace_id
                    .as_deref()
                    .unwrap_or(request_context.trace_id.as_str())
                    .to_string();
                let body_is_empty = payload.body_base64.is_none()
                    && payload
                        .body_json
                        .as_object()
                        .map(|value| value.is_empty())
                        .unwrap_or(false);
                let Some(mut resolved) = crate::control::resolve_control_route(
                    state,
                    &parts.method,
                    &parts.uri,
                    &parts.headers,
                    trace_id.as_str(),
                )
                .await?
                else {
                    return Ok(None);
                };
                if let Some(plan_payload) = api::maybe_build_sync_plan_payload(
                    state,
                    &parts,
                    trace_id.as_str(),
                    &resolved,
                    &payload.body_json,
                    payload.body_base64.as_deref(),
                    body_is_empty,
                )
                .await?
                {
                    let plan_kind = plan_payload.plan_kind.unwrap_or_default();
                    if let Some(plan) = plan_payload.plan {
                        if !plan_kind.trim().is_empty() {
                            let executed_response = execute_execution_runtime_sync(
                                state,
                                parts.uri.path(),
                                plan,
                                trace_id.as_str(),
                                &resolved,
                                plan_kind.as_str(),
                                plan_payload.report_kind,
                                plan_payload.report_context,
                            )
                            .await?;
                            if let Some(executed_response) = executed_response {
                                return Ok(Some(attach_execution_path_header(
                                    executed_response,
                                    EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
                                )));
                            }
                        }
                    }
                }
                return Ok(Some(build_internal_gateway_proxy_public_response()));
            }
            Some("execute_stream")
                if request_context.request_path == "/api/internal/gateway/execute-stream" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway execute-stream payload",
                    )));
                };
                let payload =
                    match serde_json::from_slice::<InternalGatewayExecuteRequest>(request_body) {
                        Ok(payload) => payload,
                        Err(_) => {
                            return Ok(Some(build_internal_control_error_response(
                                http::StatusCode::BAD_REQUEST,
                                "invalid internal gateway execute-stream payload",
                            )));
                        }
                    };
                if let Err(response) = reject_supplied_auth_context(payload.auth_context.as_ref()) {
                    return Ok(Some(response));
                }
                let parts = match build_internal_gateway_request_parts(
                    &payload.method,
                    &payload.path,
                    payload.query_string.as_deref(),
                    &payload.headers,
                ) {
                    Ok(parts) => parts,
                    Err(response) => return Ok(Some(response)),
                };
                let trace_id = payload
                    .trace_id
                    .as_deref()
                    .unwrap_or(request_context.trace_id.as_str())
                    .to_string();
                let Some(mut resolved) = crate::control::resolve_control_route(
                    state,
                    &parts.method,
                    &parts.uri,
                    &parts.headers,
                    trace_id.as_str(),
                )
                .await?
                else {
                    return Ok(None);
                };
                if let Some(plan_payload) = api::maybe_build_stream_plan_payload(
                    state,
                    &parts,
                    trace_id.as_str(),
                    &resolved,
                    &payload.body_json,
                    payload.body_base64.as_deref(),
                )
                .await?
                {
                    let plan_kind = plan_payload.plan_kind.unwrap_or_default();
                    if let Some(plan) = plan_payload.plan {
                        if !plan_kind.trim().is_empty() {
                            let executed_response = execute_execution_runtime_stream(
                                state,
                                plan,
                                trace_id.as_str(),
                                &resolved,
                                plan_kind.as_str(),
                                plan_payload.report_kind,
                                plan_payload.report_context,
                            )
                            .await?;
                            if let Some(executed_response) = executed_response {
                                return Ok(Some(attach_execution_path_header(
                                    executed_response,
                                    EXECUTION_PATH_EXECUTION_RUNTIME_STREAM,
                                )));
                            }
                        }
                    }
                }
                return Ok(Some(build_internal_gateway_proxy_public_response()));
            }
            Some("report_sync")
                if request_context.request_path == "/api/internal/gateway/report-sync" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway report-sync payload",
                    )));
                };
                let mut payload = match serde_json::from_slice::<
                    crate::usage::GatewaySyncReportRequest,
                >(request_body)
                {
                    Ok(payload) => payload,
                    Err(_) => {
                        return Ok(Some(build_internal_control_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid internal gateway report-sync payload",
                        )));
                    }
                };
                payload.report_context = Some(
                    match resolve_bound_internal_report_context(
                        state,
                        payload.trace_id.as_str(),
                        payload.report_kind.as_str(),
                        payload.report_context.as_ref(),
                        "report_sync",
                    )
                    .await
                    {
                        Ok(report_context) => report_context,
                        Err(response) => return Ok(Some(response)),
                    },
                );
                crate::usage::submit_sync_report(state, payload).await?;
                return Ok(Some(Json(json!({ "ok": true })).into_response()));
            }
            Some("report_stream")
                if request_context.request_path == "/api/internal/gateway/report-stream" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway report-stream payload",
                    )));
                };
                let mut payload = match serde_json::from_slice::<
                    crate::usage::GatewayStreamReportRequest,
                >(request_body)
                {
                    Ok(payload) => payload,
                    Err(_) => {
                        return Ok(Some(build_internal_control_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid internal gateway report-stream payload",
                        )));
                    }
                };
                payload.report_context = Some(
                    match resolve_bound_internal_report_context(
                        state,
                        payload.trace_id.as_str(),
                        payload.report_kind.as_str(),
                        payload.report_context.as_ref(),
                        "report_stream",
                    )
                    .await
                    {
                        Ok(report_context) => report_context,
                        Err(response) => return Ok(Some(response)),
                    },
                );
                crate::usage::submit_stream_report(state, payload).await?;
                return Ok(Some(Json(json!({ "ok": true })).into_response()));
            }
            Some("finalize_sync")
                if request_context.request_path == "/api/internal/gateway/finalize-sync" =>
            {
                let Some(request_body) = request_body else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "invalid internal gateway finalize-sync payload",
                    )));
                };
                let mut payload = match serde_json::from_slice::<
                    crate::usage::GatewaySyncReportRequest,
                >(request_body)
                {
                    Ok(payload) => payload,
                    Err(_) => {
                        return Ok(Some(build_internal_control_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid internal gateway finalize-sync payload",
                        )));
                    }
                };
                if !internal_finalize_report_kind_is_supported(payload.report_kind.as_str()) {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "Unsupported gateway sync finalize kind",
                    )));
                }
                payload.report_context = Some(
                    match resolve_bound_internal_report_context(
                        state,
                        payload.trace_id.as_str(),
                        payload.report_kind.as_str(),
                        payload.report_context.as_ref(),
                        "finalize_sync",
                    )
                    .await
                    {
                        Ok(report_context) => report_context,
                        Err(response) => return Ok(Some(response)),
                    },
                );
                let Some(synthetic_decision) = build_internal_finalize_decision(&payload) else {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "Unsupported gateway sync finalize kind",
                    )));
                };
                let trace_id = payload.trace_id.clone();
                if let Some(outcome) = api::maybe_build_sync_finalize_outcome(
                    trace_id.as_str(),
                    &synthetic_decision,
                    &payload,
                )? {
                    if let Some(background_report) = outcome.background_report {
                        crate::usage::spawn_sync_report(state.clone(), background_report);
                    }
                    let mut response = outcome.response;
                    response.headers_mut().insert(
                        HeaderName::from_static(CONTROL_EXECUTED_HEADER),
                        HeaderValue::from_static("true"),
                    );
                    return Ok(Some(response));
                }
                if let Some(response) = maybe_build_internal_finalize_video_response(
                    state,
                    trace_id.as_str(),
                    &synthetic_decision,
                    payload,
                )
                .await?
                {
                    return Ok(Some(response));
                }
                return Ok(Some(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "Unsupported gateway sync finalize kind",
                )));
            }
            _ => {
                return Ok(Some(build_internal_control_error_response(
                    http::StatusCode::NOT_FOUND,
                    "unsupported internal gateway route",
                )));
            }
        }
    }
    if !remote_addr.ip().is_loopback() {
        return Ok(Some(build_internal_control_error_response(
            http::StatusCode::FORBIDDEN,
            "loopback access only",
        )));
    }

    if decision.route_family.as_deref() != Some(TUNNEL_ROUTE_FAMILY)
        || request_context.request_method != http::Method::POST
    {
        return Ok(None);
    }

    match decision.route_kind.as_deref() {
        Some("heartbeat") if is_tunnel_heartbeat_path(request_context.request_path.as_str()) => {
            let Some(request_body) = request_body else {
                return Ok(Some(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "invalid heartbeat payload",
                )));
            };
            let payload = match parse_internal_tunnel_heartbeat_request(request_body) {
                Ok(payload) => payload,
                Err(response) => return Ok(Some(response)),
            };
            let node_id = payload.node_id.trim().to_string();
            let authenticated_generation = match state
                .tunnel
                .authenticate_control_plane_request(
                    request_headers,
                    request_context.request_method.as_str(),
                    request_context.request_path.as_str(),
                    &node_id,
                    request_body,
                )
                .await
            {
                Ok(generation) => generation,
                Err(error) => {
                    let (status, message) = match error {
                        crate::tunnel::ControlPlaneAuthError::Unavailable => (
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            "tunnel control-plane authentication unavailable",
                        ),
                        crate::tunnel::ControlPlaneAuthError::Invalid => (
                            http::StatusCode::FORBIDDEN,
                            "invalid tunnel control-plane authentication",
                        ),
                    };
                    return Ok(Some(build_internal_control_error_response(status, message)));
                }
            };
            let claim = match claim_tunnel_heartbeat(
                state.runtime_state.as_ref(),
                &node_id,
                &payload.heartbeat_session_id,
                payload.heartbeat_id,
            )
            .await
            {
                Ok(claim) => claim,
                Err(error) => {
                    return Ok(Some(build_internal_control_error_response(
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        error,
                    )));
                }
            };
            if claim.is_none() {
                let response = match state.find_proxy_node(&node_id).await {
                    Ok(Some(node)) if node.tunnel_generation == authenticated_generation => Json(
                        build_internal_tunnel_heartbeat_ack(&node, payload.heartbeat_id),
                    )
                    .into_response(),
                    Ok(Some(_)) | Ok(None) => build_internal_control_error_response(
                        http::StatusCode::FORBIDDEN,
                        "proxy tunnel credential was revoked",
                    ),
                    Err(_) => internal_gateway_data_error_response("heartbeat_duplicate_lookup"),
                };
                return Ok(Some(response));
            }
            let claim = claim.expect("fresh heartbeat claim should be present");
            let mutation = ProxyNodeHeartbeatMutation {
                node_id: node_id.clone(),
                expected_tunnel_generation: Some(authenticated_generation.clone()),
                heartbeat_interval: payload.heartbeat_interval,
                active_connections: payload.active_connections,
                total_requests_delta: payload.window_total_requests.or(payload.total_requests),
                avg_latency_ms: payload.avg_latency_ms,
                failed_requests_delta: payload.window_failed_requests.or(payload.failed_requests),
                dns_failures_delta: payload.window_dns_failures.or(payload.dns_failures),
                stream_errors_delta: payload.window_stream_errors.or(payload.stream_errors),
                proxy_metadata: payload.proxy_metadata,
                proxy_version: payload.proxy_version,
            };

            let response = match state.apply_proxy_node_heartbeat(&mutation).await {
                Ok(Some(node)) => {
                    finish_tunnel_heartbeat_claim(state.runtime_state.as_ref(), claim).await;
                    Json(build_internal_tunnel_heartbeat_ack(
                        &node,
                        payload.heartbeat_id,
                    ))
                    .into_response()
                }
                Ok(None) => {
                    finish_tunnel_heartbeat_claim(state.runtime_state.as_ref(), claim).await;
                    build_internal_control_error_response(
                        http::StatusCode::FORBIDDEN,
                        "proxy tunnel credential was revoked",
                    )
                }
                Err(_) => {
                    finish_tunnel_heartbeat_claim(state.runtime_state.as_ref(), claim).await;
                    internal_gateway_data_error_response("heartbeat_sync")
                }
            };
            return Ok(Some(response));
        }
        Some("node_status")
            if is_tunnel_node_status_path(request_context.request_path.as_str()) =>
        {
            let Some(request_body) = request_body else {
                return Ok(Some(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "invalid node-status payload",
                )));
            };
            let payload = match parse_internal_tunnel_node_status_request(request_body) {
                Ok(payload) => payload,
                Err(response) => return Ok(Some(response)),
            };
            let node_id = payload.node_id.trim().to_string();
            let authenticated_generation = match state
                .tunnel
                .authenticate_control_plane_request(
                    request_headers,
                    request_context.request_method.as_str(),
                    request_context.request_path.as_str(),
                    &node_id,
                    request_body,
                )
                .await
            {
                Ok(generation) => generation,
                Err(error) => {
                    let (status, message) = match error {
                        crate::tunnel::ControlPlaneAuthError::Unavailable => (
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            "tunnel control-plane authentication unavailable",
                        ),
                        crate::tunnel::ControlPlaneAuthError::Invalid => (
                            http::StatusCode::FORBIDDEN,
                            "invalid tunnel control-plane authentication",
                        ),
                    };
                    return Ok(Some(build_internal_control_error_response(status, message)));
                }
            };
            let mutation = ProxyNodeTunnelStatusMutation {
                node_id,
                expected_tunnel_generation: Some(authenticated_generation),
                connected: payload.connected,
                conn_count: payload.conn_count,
                detail: None,
                observed_at_unix_secs: payload.observed_at_unix_secs,
            };

            let response = match state.update_proxy_node_tunnel_status(&mutation).await {
                Ok(Some(_)) => Json(json!({ "updated": true })).into_response(),
                Ok(None) => build_internal_control_error_response(
                    http::StatusCode::FORBIDDEN,
                    "proxy tunnel credential was revoked",
                ),
                Err(_) => internal_gateway_data_error_response("node_status_sync"),
            };
            return Ok(Some(response));
        }
        _ => {}
    }

    Ok(None)
}
