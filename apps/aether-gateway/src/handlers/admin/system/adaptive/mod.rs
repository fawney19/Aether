use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::body::{Body, Bytes};
use axum::http::Response;

mod routes;
mod shared;

pub(crate) async fn maybe_build_local_admin_adaptive_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    routes::maybe_build_local_admin_adaptive_response(state, request_context, request_body).await
}
