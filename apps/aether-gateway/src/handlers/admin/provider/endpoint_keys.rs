mod mutations;
mod quota;
mod reads;

use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    response::Response,
};

pub(crate) async fn maybe_build_local_admin_endpoints_keys_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if let Some(response) = reads::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = mutations::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = quota::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    Ok(None)
}
