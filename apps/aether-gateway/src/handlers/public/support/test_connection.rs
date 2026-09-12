use axum::{body::Body, http, response::Response};

pub(super) use super::{
    build_auth_error_response, query_param_value, resolve_authenticated_local_user, AppState,
    GatewayPublicRequestContext,
};
use crate::handlers::shared::provider_catalog_key_supports_format;

#[path = "test_connection/route.rs"]
mod test_connection_route;
#[path = "test_connection/shared.rs"]
mod test_connection_shared;

pub(super) async fn maybe_build_local_test_connection_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Option<Response<Body>> {
    if request_context.request_path != "/v1/test-connection" {
        return None;
    }
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return Some(response),
    };
    if !crate::roles::can_write_admin_console(&auth.user.role) {
        return Some(build_auth_error_response(
            http::StatusCode::FORBIDDEN,
            "仅管理员可以测试供应商连接",
            false,
        ));
    }
    test_connection_route::maybe_build_local_test_connection_route_response(state, request_context)
        .await
}
