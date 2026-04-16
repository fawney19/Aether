use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{body::Body, response::Response};

mod analytics_routes;
mod cost_routes;
mod leaderboard;
mod leaderboard_routes;
mod provider_quota_routes;
mod range;
pub(crate) use self::range::{
    list_usage_for_optional_range, list_usage_for_range, parse_bounded_u32,
    resolve_admin_usage_time_range,
};
pub(crate) use aether_admin::observability::stats::{
    admin_stats_bad_request_response, aggregate_usage_stats, round_to, AdminStatsTimeRange,
    AdminStatsUsageFilter,
};

pub(crate) async fn maybe_build_local_admin_stats_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_family() != Some("stats_manage") {
        return Ok(None);
    }

    if let Some(response) =
        provider_quota_routes::maybe_build_local_admin_stats_provider_quota_response(
            state,
            request_context,
        )
        .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        analytics_routes::maybe_build_local_admin_stats_analytics_response(state, request_context)
            .await?
    {
        return Ok(Some(response));
    }

    if let Some(response) =
        cost_routes::maybe_build_local_admin_stats_cost_response(state, request_context).await?
    {
        return Ok(Some(response));
    }

    if let Some(response) = leaderboard_routes::maybe_build_local_admin_stats_leaderboard_response(
        state,
        request_context,
    )
    .await?
    {
        return Ok(Some(response));
    }

    Ok(None)
}
