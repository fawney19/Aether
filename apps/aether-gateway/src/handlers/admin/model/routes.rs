use super::{catalog_routes, global_models};
use crate::handlers::admin::request::{AdminRouteRequest, AdminRouteResult};

pub(crate) async fn maybe_build_local_admin_model_response(
    request: AdminRouteRequest<'_>,
) -> AdminRouteResult {
    if let Some(response) = catalog_routes::maybe_build_local_admin_model_catalog_response(
        &request.state(),
        &request.request_context(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = global_models::maybe_build_local_admin_global_models_response(
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
