mod builders;
mod create;
mod defaults;
mod delete;
mod detail;
mod extractors;
mod list;
mod payloads;
mod support;
mod update;

use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    response::Response,
};

pub(crate) async fn maybe_build_local_admin_endpoints_routes_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if let Some(response) = create::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = update::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = delete::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = list::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = detail::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = defaults::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    Ok(None)
}
