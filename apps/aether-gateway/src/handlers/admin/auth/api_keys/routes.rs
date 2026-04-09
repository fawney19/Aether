use super::mutation_routes::{
    build_admin_create_api_key_response, build_admin_delete_api_key_response,
    build_admin_toggle_api_key_response, build_admin_update_api_key_response,
};
use super::read_routes::{build_admin_api_key_detail_response, build_admin_list_api_keys_response};
use super::shared::build_admin_api_keys_data_unavailable_response;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{body::Body, http, response::Response};

pub(super) async fn maybe_build_local_admin_api_keys_routes_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("api_keys_manage") {
        return Ok(None);
    }

    let path = request_context.path();
    let is_api_keys_route = matches!(path, "/api/admin/api-keys" | "/api/admin/api-keys/")
        || (path.starts_with("/api/admin/api-keys/") && path.matches('/').count() == 4);

    if !is_api_keys_route {
        return Ok(None);
    }

    match decision.route_kind.as_deref() {
        Some("list_api_keys")
            if request_context.method() == http::Method::GET
                && matches!(path, "/api/admin/api-keys" | "/api/admin/api-keys/") =>
        {
            Ok(Some(
                build_admin_list_api_keys_response(state, request_context).await?,
            ))
        }
        Some("api_key_detail")
            if request_context.method() == http::Method::GET
                && path.starts_with("/api/admin/api-keys/") =>
        {
            Ok(Some(
                build_admin_api_key_detail_response(state, request_context).await?,
            ))
        }
        Some("create_api_key")
            if request_context.method() == http::Method::POST
                && matches!(path, "/api/admin/api-keys" | "/api/admin/api-keys/") =>
        {
            Ok(Some(
                build_admin_create_api_key_response(state, request_context, request_body).await?,
            ))
        }
        Some("update_api_key")
            if request_context.method() == http::Method::PUT
                && path.starts_with("/api/admin/api-keys/") =>
        {
            Ok(Some(
                build_admin_update_api_key_response(state, request_context, request_body).await?,
            ))
        }
        Some("toggle_api_key")
            if request_context.method() == http::Method::PATCH
                && path.starts_with("/api/admin/api-keys/") =>
        {
            Ok(Some(
                build_admin_toggle_api_key_response(state, request_context, request_body).await?,
            ))
        }
        Some("delete_api_key")
            if request_context.method() == http::Method::DELETE
                && path.starts_with("/api/admin/api-keys/") =>
        {
            Ok(Some(
                build_admin_delete_api_key_response(state, request_context).await?,
            ))
        }
        _ => Ok(Some(build_admin_api_keys_data_unavailable_response())),
    }
}
