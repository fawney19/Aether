use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::{endpoint_keys, endpoints_admin};
use crate::{AppState, GatewayError};
use axum::body::{Body, Bytes};
use axum::http::Response;

mod extractors;
mod health;
mod health_builders;
mod rpm;

pub(crate) use self::health_builders::build_admin_endpoint_health_status_payload;

pub(crate) async fn maybe_build_local_admin_endpoints_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if let Some(response) =
        health::maybe_build_local_admin_endpoints_health_response(state, request_context).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        rpm::maybe_build_local_admin_endpoints_rpm_response(state, request_context).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = endpoint_keys::maybe_build_local_admin_endpoints_keys_response(
        state,
        request_context,
        request_body,
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = endpoints_admin::maybe_build_local_admin_endpoints_routes_response(
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
