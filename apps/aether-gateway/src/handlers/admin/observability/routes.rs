use super::{monitoring, stats, usage};
use crate::handlers::admin::request::{AdminRouteRequest, AdminRouteResult};

pub(crate) async fn maybe_build_local_admin_observability_response(
    request: AdminRouteRequest<'_>,
) -> AdminRouteResult {
    if let Some(response) =
        stats::maybe_build_local_admin_stats_response(&request.state(), &request.request_context())
            .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = monitoring::maybe_build_local_admin_monitoring_response(
        &request.state(),
        &request.request_context(),
    )
    .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = usage::maybe_build_local_admin_usage_response(
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
