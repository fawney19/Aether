use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http::Response,
};

pub(super) async fn maybe_build_local_admin_provider_query_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    state
        .maybe_build_admin_provider_query_route_response(request_context, request_body)
        .await
}
