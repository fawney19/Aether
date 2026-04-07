use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};
mod support;

mod batch_routes;
mod payloads;
mod read_routes;
mod selection;
use self::support::{build_admin_pool_error_response, is_admin_pool_route};

pub(crate) async fn maybe_build_local_admin_pool_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("pool_manage") {
        return Ok(None);
    }

    if !is_admin_pool_route(request_context) {
        return Ok(None);
    }

    if let Some(response) = batch_routes::maybe_build_local_admin_pool_batch_response(
        state,
        request_context,
        request_body,
    )
    .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) = read_routes::maybe_build_local_admin_pool_read_response(
        state,
        request_context,
        request_body,
    )
    .await?
    {
        return Ok(Some(response));
    }

    Ok(Some(build_admin_pool_error_response(
        http::StatusCode::NOT_FOUND,
        format!(
            "Unsupported admin pool route {} {}",
            request_context.request_method, request_context.request_path
        ),
    )))
}
