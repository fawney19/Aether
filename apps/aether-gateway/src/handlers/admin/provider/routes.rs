use super::{crud, models, oauth, ops, query, strategy};
use crate::handlers::admin::request::{AdminRouteRequest, AdminRouteResult};

pub(crate) async fn maybe_build_local_admin_provider_response(
    request: AdminRouteRequest<'_>,
) -> AdminRouteResult {
    if let Some(response) = oauth::maybe_build_local_admin_provider_oauth_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = models::maybe_build_local_admin_provider_models_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = crud::maybe_build_local_admin_providers_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = ops::maybe_build_local_admin_provider_ops_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = query::maybe_build_local_admin_provider_query_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = strategy::maybe_build_local_admin_provider_strategy_response(
        &request.state(),
        &request.request_context(),
        request.request_body(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    Ok(None)
}
