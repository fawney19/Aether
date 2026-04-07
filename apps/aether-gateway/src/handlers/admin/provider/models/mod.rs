use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
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
mod write;

pub(crate) async fn maybe_build_local_admin_provider_models_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if let Some(response) =
        list::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        detail::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        create::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        update::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        delete::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        batch::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        available_source::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        assign_global::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        import::maybe_handle(state, request_context, request_body, decision).await?
    {
        return Ok(Some(response));
    }

    Ok(None)
}
