use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};
const ADMIN_AWS_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ca-central-1",
    "ca-west-1",
    "eu-central-1",
    "eu-central-2",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
];
mod system_routes;

pub(crate) async fn maybe_build_local_admin_core_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if let Some(response) =
        crate::handlers::admin::system::maybe_build_local_admin_management_tokens_response(
            state,
            request_context,
        )
        .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) = crate::handlers::admin::auth::maybe_build_local_admin_oauth_response(
        state,
        request_context,
        request_body,
    )
    .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) =
        crate::handlers::admin::system::maybe_build_local_admin_modules_response(
            state,
            request_context,
            request_body,
        )
        .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) = system_routes::maybe_build_local_admin_core_system_response(
        state,
        request_context,
        request_body,
    )
    .await?
    {
        return Ok(Some(response));
    }
    if let Some(response) =
        crate::handlers::admin::model::maybe_build_local_admin_model_catalog_response(
            state,
            request_context,
        )
        .await?
    {
        return Ok(Some(response));
    }

    Ok(None)
}
