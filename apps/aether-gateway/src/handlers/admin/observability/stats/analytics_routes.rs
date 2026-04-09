use super::range::build_comparison_range;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::stats::{
    admin_stats_bad_request_response, admin_stats_comparison_empty_response,
    admin_stats_error_distribution_empty_response,
    admin_stats_performance_percentiles_empty_response, admin_stats_time_series_empty_response,
    build_admin_stats_comparison_response, build_admin_stats_error_distribution_response,
    build_admin_stats_performance_percentiles_response, build_admin_stats_time_series_response,
    AdminStatsComparisonType, AdminStatsGranularity, AdminStatsTimeRange, AdminStatsUsageFilter,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_build_local_admin_stats_analytics_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_kind() == Some("comparison")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/comparison" | "/api/admin/stats/comparison/"
        )
    {
        let current_range = match AdminStatsTimeRange::resolve_required(
            request_context.query_string(),
            "current_start",
            "current_end",
        ) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let comparison_type =
            match query_param_value(request_context.query_string(), "comparison_type").as_deref() {
                None | Some("period") => AdminStatsComparisonType::Period,
                Some("year") => AdminStatsComparisonType::Year,
                Some(_) => {
                    return Ok(Some(admin_stats_bad_request_response(
                        "comparison_type must be 'period' or 'year'".to_string(),
                    )));
                }
            };

        let comparison_range = match build_comparison_range(&current_range, comparison_type) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_comparison_empty_response(
                &current_range,
                &comparison_range,
            )));
        }
        let current_usage = state
            .list_admin_usage_for_range(&current_range, &AdminStatsUsageFilter::default())
            .await?;
        let comparison_usage = state
            .list_admin_usage_for_range(&comparison_range, &AdminStatsUsageFilter::default())
            .await?;
        return Ok(Some(build_admin_stats_comparison_response(
            &current_usage,
            &comparison_usage,
            &current_range,
            &comparison_range,
        )));
    }

    if request_context.route_kind() == Some("error_distribution")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/errors/distribution" | "/api/admin/stats/errors/distribution/"
        )
    {
        let Some(time_range) =
            (match AdminStatsTimeRange::resolve_optional(request_context.query_string()) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
            })
        else {
            return Ok(Some(
                Json(json!({
                    "distribution": [],
                    "trend": [],
                }))
                .into_response(),
            ));
        };
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_error_distribution_empty_response()));
        }

        let usage = state
            .list_admin_usage_for_range(&time_range, &AdminStatsUsageFilter::default())
            .await?;
        return Ok(Some(build_admin_stats_error_distribution_response(
            &time_range,
            &usage,
        )));
    }

    if request_context.route_kind() == Some("performance_percentiles")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/performance/percentiles"
                | "/api/admin/stats/performance/percentiles/"
        )
    {
        let Some(time_range) =
            (match AdminStatsTimeRange::resolve_optional(request_context.query_string()) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
            })
        else {
            return Ok(Some(Json(json!([])).into_response()));
        };
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_performance_percentiles_empty_response()));
        }

        let usage = state
            .list_admin_usage_for_range(&time_range, &AdminStatsUsageFilter::default())
            .await?;
        return Ok(Some(build_admin_stats_performance_percentiles_response(
            &time_range,
            &usage,
        )));
    }

    if request_context.route_kind() == Some("time_series")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/time-series" | "/api/admin/stats/time-series/"
        )
    {
        let granularity = match AdminStatsGranularity::parse(request_context.query_string()) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let Some(time_range) =
            (match AdminStatsTimeRange::resolve_optional(request_context.query_string()) {
                Ok(value) => value,
                Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
            })
        else {
            return Ok(Some(Json(json!([])).into_response()));
        };
        if let Err(detail) = time_range.validate_for_time_series(granularity) {
            return Ok(Some(admin_stats_bad_request_response(detail)));
        }
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_time_series_empty_response()));
        }

        let filters = AdminStatsUsageFilter::from_query(request_context.query_string());
        let usage = state
            .list_admin_usage_for_range(&time_range, &filters)
            .await?;
        return Ok(Some(build_admin_stats_time_series_response(
            &time_range,
            granularity,
            &usage,
        )));
    }

    Ok(None)
}
