mod reads;
mod shared;
mod writes;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::Response,
};

pub(crate) async fn maybe_build_local_admin_global_models_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if let Some(response) =
        reads::maybe_build_local_admin_global_models_read_response(state, request_context).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = writes::maybe_build_local_admin_global_models_write_response(
        state,
        request_context,
        request_body,
    )
    .await?
    {
        return Ok(Some(response));
    }

    Ok(None)
}
