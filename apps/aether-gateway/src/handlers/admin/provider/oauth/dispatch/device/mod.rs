mod authorize;
mod poll;
mod session;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::Response,
};

#[cfg(any())]
pub(super) use self::authorize::handle_admin_provider_oauth_device_authorize;
#[cfg(any())]
pub(super) use self::poll::handle_admin_provider_oauth_device_poll;

pub(super) async fn handle_admin_provider_oauth_device_authorize(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    authorize::handle_admin_provider_oauth_device_authorize(state, request_context, request_body)
        .await
}

pub(super) async fn handle_admin_provider_oauth_device_poll(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    poll::handle_admin_provider_oauth_device_poll(state, request_context, request_body).await
}
