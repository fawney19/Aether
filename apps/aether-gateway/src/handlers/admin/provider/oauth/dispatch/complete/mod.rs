mod key;
mod provider;
mod shared;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::Response,
};

pub(super) async fn handle_admin_provider_oauth_complete_key(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    key::handle_admin_provider_oauth_complete_key(state, request_context, request_body).await
}

pub(super) async fn handle_admin_provider_oauth_complete_provider(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    provider::handle_admin_provider_oauth_complete_provider(state, request_context, request_body)
        .await
}
