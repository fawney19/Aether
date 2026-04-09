mod aggregation;
mod cache_affinity_hit_analysis;
mod cache_affinity_interval_timeline;
mod cache_affinity_ttl_analysis;
mod heatmap;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{body::Body, http, response::Response};

pub(super) async fn maybe_build_local_admin_usage_analytics_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let route_kind = request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.route_kind.as_deref());

    match route_kind {
        Some("aggregation_stats")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/admin/usage/aggregation/stats" | "/api/admin/usage/aggregation/stats/"
                ) =>
        {
            return aggregation::build_admin_usage_aggregation_stats_response(
                state,
                request_context,
            )
            .await
            .map(Some);
        }
        Some("heatmap")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/admin/usage/heatmap" | "/api/admin/usage/heatmap/"
                ) =>
        {
            return heatmap::build_admin_usage_heatmap_response(state)
                .await
                .map(Some);
        }
        Some("cache_affinity_hit_analysis")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/admin/usage/cache-affinity/hit-analysis"
                        | "/api/admin/usage/cache-affinity/hit-analysis/"
                ) =>
        {
            return cache_affinity_hit_analysis::build_admin_usage_cache_affinity_hit_analysis_response(
                state,
                request_context,
            )
            .await
            .map(Some);
        }
        Some("cache_affinity_interval_timeline")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/admin/usage/cache-affinity/interval-timeline"
                        | "/api/admin/usage/cache-affinity/interval-timeline/"
                ) =>
        {
            return cache_affinity_interval_timeline::build_admin_usage_cache_affinity_interval_timeline_response(
                state,
                request_context,
            )
            .await
            .map(Some);
        }
        Some("cache_affinity_ttl_analysis")
            if request_context.request_method == http::Method::GET
                && matches!(
                    request_context.request_path.as_str(),
                    "/api/admin/usage/cache-affinity/ttl-analysis"
                        | "/api/admin/usage/cache-affinity/ttl-analysis/"
                ) =>
        {
            return cache_affinity_ttl_analysis::build_admin_usage_cache_affinity_ttl_analysis_response(
                state,
                request_context,
            )
            .await
            .map(Some);
        }
        _ => {}
    }

    Ok(None)
}
