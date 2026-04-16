use super::range::build_comparison_range;
use super::resolve_admin_usage_time_range;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::stats::{
    admin_stats_bad_request_response, admin_stats_comparison_empty_response,
    admin_stats_error_distribution_empty_response,
    admin_stats_performance_percentiles_empty_response, admin_stats_time_series_empty_response,
    build_admin_stats_comparison_response_from_aggregates,
    build_admin_stats_error_distribution_response,
    build_admin_stats_performance_percentiles_response,
    build_admin_stats_time_series_response_from_summaries, AdminStatsAggregate,
    AdminStatsComparisonType, AdminStatsGranularity, AdminStatsTimeRange, AdminStatsUsageFilter,
};
use aether_data_contracts::repository::usage::{
    UsageAuditSummaryQuery, UsageTimeSeriesGranularity, UsageTimeSeriesQuery,
};
use axum::{body::Body, http, response::Response};

fn usage_summary_to_admin_stats_aggregate(
    summary: &aether_data_contracts::repository::usage::StoredUsageAuditSummary,
) -> AdminStatsAggregate {
    AdminStatsAggregate {
        total_requests: summary.total_requests,
        total_tokens: summary.recorded_total_tokens,
        total_cost: summary.total_cost_usd,
        actual_total_cost: summary.actual_total_cost_usd,
        total_response_time_ms: summary.total_response_time_ms,
        error_requests: summary.error_requests,
    }
}

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
        let Some((current_from_unix_secs, current_until_unix_secs)) =
            current_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_comparison_empty_response(
                &current_range,
                &comparison_range,
            )));
        };
        let Some((comparison_from_unix_secs, comparison_until_unix_secs)) =
            comparison_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_comparison_empty_response(
                &current_range,
                &comparison_range,
            )));
        };
        let current_summary = state
            .summarize_usage_audits(&UsageAuditSummaryQuery {
                created_from_unix_secs: current_from_unix_secs,
                created_until_unix_secs: current_until_unix_secs,
                ..Default::default()
            })
            .await?;
        let comparison_summary = state
            .summarize_usage_audits(&UsageAuditSummaryQuery {
                created_from_unix_secs: comparison_from_unix_secs,
                created_until_unix_secs: comparison_until_unix_secs,
                ..Default::default()
            })
            .await?;
        return Ok(Some(build_admin_stats_comparison_response_from_aggregates(
            &usage_summary_to_admin_stats_aggregate(&current_summary),
            &usage_summary_to_admin_stats_aggregate(&comparison_summary),
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
        let time_range = match resolve_admin_usage_time_range(request_context.query_string()) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
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
        let time_range = match resolve_admin_usage_time_range(request_context.query_string()) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
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
        let time_range = match resolve_admin_usage_time_range(request_context.query_string()) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if let Err(detail) = time_range.validate_for_time_series(granularity) {
            return Ok(Some(admin_stats_bad_request_response(detail)));
        }
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_time_series_empty_response()));
        }

        let filters = AdminStatsUsageFilter::from_query(request_context.query_string());
        let query_granularity = match granularity {
            AdminStatsGranularity::Hour => UsageTimeSeriesGranularity::Hour,
            AdminStatsGranularity::Day
            | AdminStatsGranularity::Week
            | AdminStatsGranularity::Month => UsageTimeSeriesGranularity::Day,
        };
        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_time_series_empty_response()));
        };
        let buckets = state
            .summarize_usage_time_series(&UsageTimeSeriesQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                granularity: query_granularity,
                tz_offset_minutes: time_range.tz_offset_minutes,
                user_id: filters.user_id,
                provider_name: filters.provider_name,
                model: filters.model,
            })
            .await?;
        return Ok(Some(build_admin_stats_time_series_response_from_summaries(
            &time_range,
            granularity,
            &buckets,
        )));
    }

    Ok(None)
}
