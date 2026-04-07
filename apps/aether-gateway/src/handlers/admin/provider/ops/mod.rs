use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::body::{Body, Bytes};
use axum::http::Response;

mod architectures;
pub(crate) mod providers;

pub(crate) async fn maybe_build_local_admin_provider_ops_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if let Some(response) =
        architectures::maybe_build_local_admin_provider_ops_architectures_response(request_context)
            .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = providers::maybe_build_local_admin_provider_ops_providers_response(
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
