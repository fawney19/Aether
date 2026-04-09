mod batch;
mod create;
mod delete;
mod oauth_invalid;
mod update;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::Response,
};

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if let Some(response) = update::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }
    if let Some(response) = delete::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }
    if let Some(response) = batch::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }
    if let Some(response) =
        oauth_invalid::maybe_handle(state, request_context, request_body).await?
    {
        return Ok(Some(response));
    }
    if let Some(response) = create::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    Ok(None)
}
