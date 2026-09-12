use super::super::internal;
use crate::admin_api;
use crate::audit::attach_admin_audit_event;
use crate::control::{
    audit_admin_read_only_management_token_permissions,
    validate_management_token_admin_route_permission, GatewayPublicRequestContext,
};
use crate::{AppState, GatewayError};
use axum::body::{Body, Bytes};
use axum::http::{self, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use tracing::warn;

pub(super) async fn maybe_build_local_internal_proxy_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    remote_addr: &std::net::SocketAddr,
    request_headers: &http::HeaderMap,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    internal::maybe_build_local_internal_proxy_response_impl(
        state,
        request_context,
        remote_addr,
        request_headers,
        request_body,
    )
    .await
}

pub(super) async fn maybe_build_local_admin_proxy_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    remote_addr: &std::net::SocketAddr,
    request_headers: &http::HeaderMap,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };
    if decision.route_class.as_deref() != Some("admin_proxy") {
        return Ok(None);
    }
    if decision.admin_principal.is_none() {
        return Ok(None);
    }
    if let Some(response) = maybe_build_management_token_permission_denied_response(request_context)
    {
        return Ok(Some(response));
    }

    admin_api::maybe_build_local_admin_response(admin_api::AdminRouteRequest::new(
        state,
        request_context,
        remote_addr,
        request_headers,
        request_body,
    ))
    .await
}

fn maybe_build_management_token_permission_denied_response(
    request_context: &GatewayPublicRequestContext,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    let admin_principal = decision.admin_principal.as_ref()?;
    let audit_admin_read_only_permissions;
    let token_permissions = if crate::roles::can_write_admin_console(&admin_principal.user_role) {
        admin_principal.management_token_permissions.as_deref()
    } else {
        audit_admin_read_only_permissions = audit_admin_read_only_management_token_permissions();
        Some(audit_admin_read_only_permissions.as_slice())
    };
    let denied = validate_management_token_admin_route_permission(
        &request_context.request_method,
        decision,
        token_permissions,
    )
    .err()?;
    let actor_id = admin_principal
        .management_token_id
        .as_deref()
        .unwrap_or(admin_principal.user_id.as_str());

    warn!(
        trace_id = %request_context.trace_id,
        admin_actor_id = %actor_id,
        admin_user_role = %admin_principal.user_role,
        route_family = decision.route_family.as_deref().unwrap_or("unknown"),
        route_kind = decision.route_kind.as_deref().unwrap_or("unknown"),
        required_permission = %denied.required_permission,
        "admin route permission denied"
    );

    let mut response = (
        http::StatusCode::FORBIDDEN,
        Json(json!({
            "detail": "management token permission denied",
            "required_permission": denied.required_permission,
            "route_family": decision.route_family.as_deref(),
            "route_kind": decision.route_kind.as_deref(),
            "request_path": request_context.request_path,
        })),
    )
        .into_response();
    attach_admin_audit_event(
        &mut response,
        "admin_route_permission_denied",
        "permission_denied",
        "admin_route_permission",
        actor_id,
    );
    Some(response)
}
