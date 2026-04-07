use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::body::{Body, Bytes};
use axum::http::Response;

mod models;
mod payload;
mod response;
mod routes;

pub(crate) async fn maybe_build_local_admin_provider_query_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    routes::maybe_build_local_admin_provider_query_response(state, request_context, request_body)
        .await
}
