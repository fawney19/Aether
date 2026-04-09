use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::body::{Body, Bytes};
use axum::http::Response;

mod assign_global;
mod available_source;
mod batch;
mod create;
mod delete;
mod detail;
mod import;
mod list;
mod payloads;
mod update;

pub(crate) async fn maybe_build_local_admin_provider_models_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_family() != Some("provider_models_manage") {
        return Ok(None);
    }

    if let Some(response) = list::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = detail::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    if let Some(response) = create::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

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
        available_source::maybe_handle(state, request_context, request_body).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        assign_global::maybe_handle(state, request_context, request_body).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = import::maybe_handle(state, request_context, request_body).await? {
        return Ok(Some(response));
    }

    Ok(None)
}
