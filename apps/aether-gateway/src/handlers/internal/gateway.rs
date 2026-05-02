use super::{
    attach_execution_path_header, build_internal_control_error_response,
    build_internal_finalize_decision, build_internal_gateway_fallback_plan_payload,
    build_internal_gateway_header_map, build_internal_gateway_passthrough_payload,
    build_internal_gateway_proxy_public_response, build_internal_gateway_request_parts,
    build_internal_gateway_resolve_payload, build_internal_gateway_uri,
    build_internal_tunnel_heartbeat_ack, build_management_token_payload, gateway_error_message,
    maybe_build_internal_finalize_video_response, parse_internal_tunnel_heartbeat_request,
    parse_internal_tunnel_node_status_request,
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
use crate::tunnel::{is_tunnel_heartbeat_path, is_tunnel_node_status_path, TUNNEL_ROUTE_FAMILY};
use crate::{AppState, GatewayError};
use aether_data::repository::proxy_nodes::{
    ProxyNodeHeartbeatMutation, ProxyNodeTunnelStatusMutation,
};
use axum::body::{Body, Bytes};
use axum::http::{self, HeaderName, HeaderValue, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

pub(crate) async fn maybe_build_local_internal_proxy_response_impl(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    remote_addr: &std::net::SocketAddr,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };
    if decision.route_class.as_deref() != Some("internal_proxy") {
        return Ok(None);
    }
    if decision.route_family.as_deref() == Some("internal_gateway") {
        if !remote_addr.ip().is_loopback() {
            return Ok(Some(build_internal_control_error_response(
                http::StatusCode::FORBIDDEN,
                "loopback access only",
            )));
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
                let provided_auth_context = payload.auth_context.is_some();
                if let Some(auth_context) = payload.auth_context {
                    resolved.auth_context = Some(auth_context);
                    resolved.local_auth_rejection = None;
                }
                let auth_context = resolved.auth_context.as_ref();
                if auth_context
                    .map(|value| !value.access_allowed)
                    .unwrap_or(true)
                {
                    let fallback_auth_context = if !provided_auth_context {
                        auth_context
                    } else {
                        None
                    };
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(
                            fallback_auth_context,
                        ))
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
                    let fallback_auth_context = if !provided_auth_context {
                        auth_context
                    } else {
                        None
                    };
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(
                            fallback_auth_context,
                        ))
                        .into_response(),
                    ));
                };
                if provided_auth_context {
                    local_payload.auth_context = None;
                }
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
                let provided_auth_context = payload.auth_context.is_some();
                if let Some(auth_context) = payload.auth_context {
                    resolved.auth_context = Some(auth_context);
                    resolved.local_auth_rejection = None;
                }
                let auth_context = resolved.auth_context.as_ref();
                if auth_context
                    .map(|value| !value.access_allowed)
                    .unwrap_or(true)
                {
                    let fallback_auth_context = if !provided_auth_context {
                        auth_context
                    } else {
                        None
                    };
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(
                            fallback_auth_context,
                        ))
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
                    let fallback_auth_context = if !provided_auth_context {
                        auth_context
                    } else {
                        None
                    };
                    return Ok(Some(
                        Json(build_internal_gateway_fallback_plan_payload(
                            fallback_auth_context,
                        ))
                        .into_response(),
                    ));
                };
                if provided_auth_context {
                    local_payload.auth_context = None;
                }
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
                let provided_auth_context = payload.auth_context.is_some();
                if let Some(auth_context) = payload.auth_context {
                    resolved.auth_context = Some(auth_context);
                    resolved.local_auth_rejection = None;
                }
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
                    if provided_auth_context {
                        planned.auth_context = None;
                    }
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
                let provided_auth_context = payload.auth_context.is_some();
                if let Some(auth_context) = payload.auth_context {
                    resolved.auth_context = Some(auth_context);
                    resolved.local_auth_rejection = None;
                }
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
                    if provided_auth_context {
                        planned.auth_context = None;
                    }
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
                if let Some(auth_context) = payload.auth_context {
                    resolved.auth_context = Some(auth_context);
                    resolved.local_auth_rejection = None;
                }
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
                if let Some(auth_context) = payload.auth_context {
                    resolved.auth_context = Some(auth_context);
                    resolved.local_auth_rejection = None;
                }
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
                let payload = match serde_json::from_slice::<crate::usage::GatewaySyncReportRequest>(
                    request_body,
                ) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return Ok(Some(build_internal_control_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid internal gateway report-sync payload",
                        )));
                    }
                };
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
                let payload = match serde_json::from_slice::<crate::usage::GatewayStreamReportRequest>(
                    request_body,
                ) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return Ok(Some(build_internal_control_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid internal gateway report-stream payload",
                        )));
                    }
                };
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
                let payload = match serde_json::from_slice::<crate::usage::GatewaySyncReportRequest>(
                    request_body,
                ) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return Ok(Some(build_internal_control_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid internal gateway finalize-sync payload",
                        )));
                    }
                };
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
            let mutation = ProxyNodeHeartbeatMutation {
                node_id: node_id.clone(),
                heartbeat_interval: payload.heartbeat_interval,
                active_connections: payload.active_connections,
                total_requests_delta: payload.total_requests,
                avg_latency_ms: payload.avg_latency_ms,
                failed_requests_delta: payload.failed_requests,
                dns_failures_delta: payload.dns_failures,
                stream_errors_delta: payload.stream_errors,
                proxy_metadata: payload.proxy_metadata,
                proxy_version: payload.proxy_version,
            };

            let response = match state.apply_proxy_node_heartbeat(&mutation).await {
                Ok(Some(node)) => Json(build_internal_tunnel_heartbeat_ack(
                    &node,
                    payload.heartbeat_id,
                ))
                .into_response(),
                Ok(None) => build_internal_control_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("heartbeat sync failed: ProxyNode {node_id} 不存在"),
                ),
                Err(err) => build_internal_control_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("heartbeat sync failed: {}", gateway_error_message(err)),
                ),
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
            let mutation = ProxyNodeTunnelStatusMutation {
                node_id: payload.node_id.trim().to_string(),
                connected: payload.connected,
                conn_count: payload.conn_count,
                detail: None,
                observed_at_unix_secs: payload.observed_at_unix_secs,
            };

            let response = match state.update_proxy_node_tunnel_status(&mutation).await {
                Ok(node) => Json(json!({ "updated": node.is_some() })).into_response(),
                Err(err) => build_internal_control_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("node status sync failed: {}", gateway_error_message(err)),
                ),
            };
            return Ok(Some(response));
        }
        _ => {}
    }

    Ok(None)
}
