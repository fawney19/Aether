use crate::admin_api::build_internal_control_error_response;
use crate::constants::{
    CONTROL_ACTION_HEADER, CONTROL_ACTION_PROXY_PUBLIC, CONTROL_EXECUTED_HEADER,
    CONTROL_EXECUTION_RUNTIME_CANDIDATE_KEY, EXECUTION_PATH_CONTROL_EXECUTE_STREAM,
    EXECUTION_PATH_CONTROL_EXECUTE_SYNC, EXECUTION_PATH_DISTRIBUTED_OVERLOADED,
    EXECUTION_PATH_EXECUTION_RUNTIME_STREAM, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
    EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_AUTH_DENIED, EXECUTION_PATH_LOCAL_OVERLOADED,
    EXECUTION_PATH_LOCAL_RATE_LIMITED, EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH,
};
use crate::control::{management_token_permission_mode_and_summary, GatewayControlDecision};
use crate::execution_runtime::{
    maybe_build_local_sync_finalize_response, maybe_build_local_video_error_response,
    maybe_build_local_video_success_outcome, resolve_local_sync_error_background_report_kind,
    resolve_local_sync_success_background_report_kind, LocalVideoSyncSuccessBuild,
};
use crate::handlers::shared::{
    unix_secs_to_rfc3339, InternalTunnelHeartbeatRequest, InternalTunnelNodeStatusRequest,
};
use crate::video_tasks::{
    build_internal_finalize_video_plan, build_local_sync_finalize_request_path,
};
use crate::{AppState, GatewayError};
use aether_data::repository::management_tokens::{
    StoredManagementToken, StoredManagementTokenUserSummary,
};
use aether_data::repository::proxy_nodes::StoredProxyNode;
use aether_usage_runtime::{infer_internal_finalize_signature, resolve_internal_finalize_route};
use axum::body::Body;
use axum::http::{self, header::HeaderName, header::HeaderValue, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn insert_execution_runtime_candidate_fields(
    payload: &mut serde_json::Map<String, serde_json::Value>,
    value: bool,
) {
    payload.insert(
        CONTROL_EXECUTION_RUNTIME_CANDIDATE_KEY.to_string(),
        json!(value),
    );
}

pub(crate) fn build_internal_gateway_passthrough_payload(uri: &http::Uri) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert("action".to_string(), json!("proxy_public"));
    payload.insert("route_class".to_string(), json!("passthrough"));
    payload.insert("public_path".to_string(), json!(uri.path()));
    insert_execution_runtime_candidate_fields(&mut payload, false);
    if let Some(query) = uri.query().filter(|value| !value.is_empty()) {
        payload.insert("public_query_string".to_string(), json!(query));
    }
    serde_json::Value::Object(payload)
}

pub(crate) fn build_internal_gateway_resolve_payload(
    decision: GatewayControlDecision,
) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert("action".to_string(), json!("proxy_public"));
    payload.insert("route_class".to_string(), json!(decision.route_class));
    payload.insert("public_path".to_string(), json!(decision.public_path));
    insert_execution_runtime_candidate_fields(
        &mut payload,
        decision.is_execution_runtime_candidate(),
    );
    if let Some(query) = decision.public_query_string {
        payload.insert("public_query_string".to_string(), json!(query));
    }
    if let Some(route_family) = decision.route_family {
        payload.insert("route_family".to_string(), json!(route_family));
    }
    if let Some(route_kind) = decision.route_kind {
        payload.insert("route_kind".to_string(), json!(route_kind));
    }
    if let Some(request_auth_channel) = decision.request_auth_channel {
        payload.insert(
            "request_auth_channel".to_string(),
            json!(request_auth_channel),
        );
    }
    if let Some(signature) = decision.auth_endpoint_signature {
        payload.insert("auth_endpoint_signature".to_string(), json!(signature));
    }
    if let Some(auth_context) = decision.auth_context {
        payload.insert(
            "auth_context".to_string(),
            serde_json::to_value(auth_context).unwrap_or(serde_json::Value::Null),
        );
    }
    serde_json::Value::Object(payload)
}

pub(crate) fn build_internal_gateway_fallback_plan_payload(
    auth_context: Option<&crate::control::GatewayControlAuthContext>,
) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert("action".to_string(), json!("fallback_plan"));
    if let Some(auth_context) = auth_context {
        payload.insert(
            "auth_context".to_string(),
            serde_json::to_value(auth_context).unwrap_or(serde_json::Value::Null),
        );
    }
    serde_json::Value::Object(payload)
}

pub(crate) fn build_internal_gateway_proxy_public_response() -> Response<Body> {
    (
        http::StatusCode::CONFLICT,
        [(CONTROL_ACTION_HEADER, CONTROL_ACTION_PROXY_PUBLIC)],
        Json(json!({ "action": CONTROL_ACTION_PROXY_PUBLIC })),
    )
        .into_response()
}

pub(crate) fn attach_execution_path_header(
    mut response: Response<Body>,
    execution_path: &'static str,
) -> Response<Body> {
    response.headers_mut().insert(
        HeaderName::from_static(EXECUTION_PATH_HEADER),
        HeaderValue::from_static(execution_path),
    );
    response
}

pub(crate) fn resolve_local_proxy_execution_path(
    response: &Response<Body>,
    default_execution_path: &'static str,
) -> &'static str {
    match response
        .headers()
        .get(EXECUTION_PATH_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_SYNC) => EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_STREAM) => EXECUTION_PATH_EXECUTION_RUNTIME_STREAM,
        Some(EXECUTION_PATH_CONTROL_EXECUTE_SYNC) => EXECUTION_PATH_CONTROL_EXECUTE_SYNC,
        Some(EXECUTION_PATH_CONTROL_EXECUTE_STREAM) => EXECUTION_PATH_CONTROL_EXECUTE_STREAM,
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED) => EXECUTION_PATH_LOCAL_AUTH_DENIED,
        Some(EXECUTION_PATH_LOCAL_RATE_LIMITED) => EXECUTION_PATH_LOCAL_RATE_LIMITED,
        Some(EXECUTION_PATH_LOCAL_OVERLOADED) => EXECUTION_PATH_LOCAL_OVERLOADED,
        Some(EXECUTION_PATH_DISTRIBUTED_OVERLOADED) => EXECUTION_PATH_DISTRIBUTED_OVERLOADED,
        Some(EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH) => EXECUTION_PATH_PUBLIC_PROXY_PASSTHROUGH,
        _ => default_execution_path,
    }
}

pub(crate) fn build_internal_gateway_header_map(
    headers: &BTreeMap<String, String>,
) -> Result<http::HeaderMap, Response<Body>> {
    let mut mapped = http::HeaderMap::new();
    for (name, value) in headers {
        let header_name = match HeaderName::from_bytes(name.as_bytes()) {
            Ok(name) => name,
            Err(_) => {
                return Err(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "invalid internal gateway header",
                ));
            }
        };
        let header_value = match HeaderValue::from_str(value) {
            Ok(value) => value,
            Err(_) => {
                return Err(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "invalid internal gateway header",
                ));
            }
        };
        mapped.append(header_name, header_value);
    }
    Ok(mapped)
}

pub(crate) fn build_internal_gateway_request_parts(
    method: &str,
    path: &str,
    query_string: Option<&str>,
    headers: &BTreeMap<String, String>,
) -> Result<http::request::Parts, Response<Body>> {
    let mapped_headers = build_internal_gateway_header_map(headers)?;
    let method = match http::Method::from_bytes(method.as_bytes()) {
        Ok(method) => method,
        Err(_) => {
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid internal gateway method",
            ));
        }
    };
    let uri = build_internal_gateway_uri(path, query_string)?;
    let request = match http::Request::builder().method(method).uri(uri).body(()) {
        Ok(request) => request,
        Err(_) => {
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid internal gateway request",
            ));
        }
    };
    let (mut parts, _) = request.into_parts();
    parts.headers = mapped_headers;
    Ok(parts)
}

pub(crate) fn build_internal_gateway_uri(
    path: &str,
    query_string: Option<&str>,
) -> Result<http::Uri, Response<Body>> {
    let normalized_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let uri_text = if let Some(query) = query_string.filter(|value| !value.is_empty()) {
        format!("{normalized_path}?{query}")
    } else {
        normalized_path
    };
    uri_text.parse::<http::Uri>().map_err(|_| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "invalid internal gateway uri",
        )
    })
}

pub(crate) fn build_internal_finalize_decision(
    payload: &crate::usage::GatewaySyncReportRequest,
) -> Option<GatewayControlDecision> {
    let signature = infer_internal_finalize_signature(payload)?;
    let route = resolve_internal_finalize_route(signature.as_str())?;
    Some(
        GatewayControlDecision::synthetic(
            route.public_path,
            Some("ai_public".to_string()),
            Some(route.route_family.to_string()),
            Some(route.route_kind.to_string()),
            Some(signature),
        )
        .with_execution_runtime_candidate(true),
    )
}

pub(crate) async fn maybe_build_internal_finalize_video_response(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    payload: crate::usage::GatewaySyncReportRequest,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some((signature, plan)) =
        infer_internal_finalize_signature(&payload).and_then(|signature| {
            build_internal_finalize_video_plan(
                payload.trace_id.as_str(),
                signature.as_str(),
                payload.report_context.as_ref(),
            )
            .map(|plan| (signature, plan))
        })
    else {
        return Ok(None);
    };

    let mut payload = match maybe_build_local_video_success_outcome(
        trace_id,
        decision,
        payload,
        &state.video_tasks,
        &plan,
    )? {
        LocalVideoSyncSuccessBuild::Handled(outcome) => {
            let crate::execution_runtime::LocalVideoSyncSuccessOutcome {
                response,
                report_payload,
                original_report_context: _,
                report_mode,
                local_task_snapshot,
            } = outcome;
            if let Some(snapshot) = local_task_snapshot {
                let _ = state.upsert_video_task_snapshot(&snapshot).await?;
                state.video_tasks.record_snapshot(snapshot);
            }
            match report_mode {
                crate::video_tasks::VideoTaskSyncReportMode::InlineSync => {
                    crate::usage::submit_sync_report(state, report_payload).await?;
                }
                crate::video_tasks::VideoTaskSyncReportMode::Background => {
                    crate::usage::spawn_sync_report(state.clone(), report_payload);
                }
            }
            let mut response = response;
            response.headers_mut().insert(
                HeaderName::from_static(CONTROL_EXECUTED_HEADER),
                HeaderValue::from_static("true"),
            );
            return Ok(Some(response));
        }
        LocalVideoSyncSuccessBuild::NotHandled(payload) => payload,
    };

    if let Some(mut response) =
        maybe_build_local_sync_finalize_response(trace_id, decision, &payload)?
    {
        let request_path = build_local_sync_finalize_request_path(
            payload.report_kind.as_str(),
            signature.as_str(),
            payload.report_context.as_ref(),
        );
        if let Some(request_path) = request_path {
            state
                .video_tasks
                .apply_finalize_mutation(request_path.as_str(), payload.report_kind.as_str());
            if let Some(snapshot) = state
                .video_tasks
                .snapshot_for_route(decision.route_family.as_deref(), request_path.as_str())
            {
                let _ = state.upsert_video_task_snapshot(&snapshot).await?;
            }
        }
        if let Some(success_report_kind) =
            resolve_local_sync_success_background_report_kind(payload.report_kind.as_str())
        {
            payload.report_kind = success_report_kind.to_string();
            crate::usage::spawn_sync_report(state.clone(), payload);
        }
        response.headers_mut().insert(
            HeaderName::from_static(CONTROL_EXECUTED_HEADER),
            HeaderValue::from_static("true"),
        );
        return Ok(Some(response));
    }

    if let Some(mut response) =
        maybe_build_local_video_error_response(trace_id, decision, &payload)?
    {
        if let Some(error_report_kind) =
            resolve_local_sync_error_background_report_kind(payload.report_kind.as_str())
        {
            payload.report_kind = error_report_kind.to_string();
            crate::usage::spawn_sync_report(state.clone(), payload);
        }
        response.headers_mut().insert(
            HeaderName::from_static(CONTROL_EXECUTED_HEADER),
            HeaderValue::from_static("true"),
        );
        return Ok(Some(response));
    }

    Ok(None)
}

pub(crate) fn gateway_error_message(error: GatewayError) -> String {
    match error {
        GatewayError::UpstreamUnavailable { message, .. }
        | GatewayError::ControlUnavailable { message, .. }
        | GatewayError::Client { message, .. }
        | GatewayError::Internal(message) => message,
    }
}

pub(crate) fn build_internal_tunnel_heartbeat_ack(
    node: &StoredProxyNode,
    heartbeat_id: u64,
) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    payload.insert("heartbeat_id".to_string(), json!(heartbeat_id));
    if let Some(remote_config) = node.remote_config.as_ref() {
        payload.insert("remote_config".to_string(), remote_config.clone());
        payload.insert("config_version".to_string(), json!(node.config_version));
        if let Some(upgrade_to) = remote_config
            .as_object()
            .and_then(|value| value.get("upgrade_to"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            payload.insert("upgrade_to".to_string(), json!(upgrade_to));
        }
    }
    serde_json::Value::Object(payload)
}

pub(crate) fn parse_internal_tunnel_heartbeat_request(
    request_body: &[u8],
) -> Result<InternalTunnelHeartbeatRequest, Response<Body>> {
    let payload =
        serde_json::from_slice::<InternalTunnelHeartbeatRequest>(request_body).map_err(|_| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid heartbeat payload",
            )
        })?;

    let node_id = payload.node_id.trim();
    if node_id.is_empty() || node_id.len() > 36 || payload.heartbeat_id == 0 {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "invalid heartbeat payload",
        ));
    }
    if payload
        .heartbeat_interval
        .is_some_and(|value| !(5..=600).contains(&value))
        || payload.active_connections.is_some_and(|value| value < 0)
        || payload.total_requests.is_some_and(|value| value < 0)
        || payload.avg_latency_ms.is_some_and(|value| value < 0.0)
        || payload.failed_requests.is_some_and(|value| value < 0)
        || payload.dns_failures.is_some_and(|value| value < 0)
        || payload.stream_errors.is_some_and(|value| value < 0)
        || payload
            .proxy_version
            .as_deref()
            .is_some_and(|value: &str| value.chars().count() > 20)
        || payload
            .proxy_metadata
            .as_ref()
            .is_some_and(|value: &serde_json::Value| !value.is_object())
    {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "invalid heartbeat payload",
        ));
    }

    Ok(payload)
}

pub(crate) fn parse_internal_tunnel_node_status_request(
    request_body: &[u8],
) -> Result<InternalTunnelNodeStatusRequest, Response<Body>> {
    let payload =
        serde_json::from_slice::<InternalTunnelNodeStatusRequest>(request_body).map_err(|_| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid node-status payload",
            )
        })?;

    let node_id = payload.node_id.trim();
    if node_id.is_empty() || node_id.len() > 36 || payload.conn_count < 0 {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "invalid node-status payload",
        ));
    }

    Ok(payload)
}

fn build_management_token_user_payload(
    user: &StoredManagementTokenUserSummary,
) -> serde_json::Value {
    json!({
        "id": user.id,
        "email": user.email,
        "username": user.username,
        "role": user.role,
    })
}

pub(crate) fn build_management_token_payload(
    token: &StoredManagementToken,
    user: Option<&StoredManagementTokenUserSummary>,
) -> serde_json::Value {
    let (permission_mode, permission_summary) =
        management_token_permission_mode_and_summary(token.permissions.as_ref());
    let mut payload = json!({
        "id": token.id,
        "user_id": token.user_id,
        "name": token.name,
        "description": token.description,
        "token_display": token.token_display(),
        "allowed_ips": token.allowed_ips,
        "permissions": token.permissions,
        "permission_mode": permission_mode,
        "permission_summary": permission_summary,
        "expires_at": token.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "last_used_at": token.last_used_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "last_used_ip": token.last_used_ip,
        "usage_count": token.usage_count,
        "is_active": token.is_active,
        "created_at": token.created_at_unix_ms.and_then(unix_secs_to_rfc3339),
        "updated_at": token.updated_at_unix_secs.and_then(unix_secs_to_rfc3339),
    });
    if let Some(user) = user {
        payload["user"] = build_management_token_user_payload(user);
    }
    payload
}
