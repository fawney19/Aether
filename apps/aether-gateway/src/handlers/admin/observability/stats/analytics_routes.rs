use super::range::{build_comparison_range, parse_bounded_u32};
use super::resolve_admin_usage_time_range;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{
    build_admin_usage_counter_health_payload, query_param_optional_bool, query_param_value,
};
use crate::GatewayError;
use aether_admin::observability::stats::{
    admin_stats_bad_request_response, admin_stats_comparison_empty_response,
    admin_stats_error_distribution_empty_response,
    admin_stats_performance_percentiles_empty_response,
    admin_stats_provider_performance_empty_response, admin_stats_time_series_empty_response,
    build_admin_stats_comparison_response_from_aggregates,
    build_admin_stats_error_distribution_response_from_summaries,
    build_admin_stats_performance_percentiles_response_from_summaries,
    build_admin_stats_provider_performance_response,
    build_admin_stats_time_series_response_from_summaries, AdminStatsAggregate,
    AdminStatsComparisonType, AdminStatsGranularity, AdminStatsTimeRange, AdminStatsUsageFilter,
};
use aether_data_contracts::repository::usage::{
    UsageAuditSummaryQuery, UsageErrorDistributionQuery, UsagePerformancePercentilesQuery,
    UsageProviderPerformanceQuery, UsageTimeSeriesGranularity, UsageTimeSeriesQuery,
};
use axum::{body::Body, http, response::Response};

async fn build_usage_counter_health_payload(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    let snapshot = state
        .as_ref()
        .read_cached_usage_counter_health()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(build_admin_usage_counter_health_payload(
        &snapshot,
        now_unix_secs,
    ))
}

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

        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_error_distribution_empty_response()));
        };
        let rows = state
            .summarize_usage_error_distribution(&UsageErrorDistributionQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                tz_offset_minutes: time_range.tz_offset_minutes,
            })
            .await?;
        return Ok(Some(
            build_admin_stats_error_distribution_response_from_summaries(&rows),
        ));
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

        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_performance_percentiles_empty_response()));
        };
        let rows = state
            .summarize_usage_performance_percentiles(&UsagePerformancePercentilesQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                tz_offset_minutes: time_range.tz_offset_minutes,
            })
            .await?;
        return Ok(Some(
            build_admin_stats_performance_percentiles_response_from_summaries(&time_range, &rows),
        ));
    }

    if request_context.route_kind() == Some("provider_performance")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/performance/providers" | "/api/admin/stats/performance/providers/"
        )
    {
        let time_range = match resolve_admin_usage_time_range(request_context.query_string()) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let granularity =
            match query_param_value(request_context.query_string(), "granularity").as_deref() {
                None | Some("day") => UsageTimeSeriesGranularity::Day,
                Some("hour") => UsageTimeSeriesGranularity::Hour,
                Some(_) => {
                    return Ok(Some(admin_stats_bad_request_response(
                        "granularity must be one of: day, hour".to_string(),
                    )));
                }
            };
        let limit = match query_param_value(request_context.query_string(), "limit")
            .map(|value| parse_bounded_u32("limit", &value, 1, 20))
            .transpose()
        {
            Ok(value) => value.unwrap_or(8) as usize,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let slow_threshold_ms =
            match query_param_value(request_context.query_string(), "slow_threshold_ms")
                .map(|value| parse_bounded_u32("slow_threshold_ms", &value, 1, 600_000))
                .transpose()
            {
                Ok(value) => u64::from(value.unwrap_or(10_000)),
                Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
            };
        let usage_counter = build_usage_counter_health_payload(state).await?;
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_provider_performance_empty_response(
                usage_counter,
            )));
        }

        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_provider_performance_empty_response(
                usage_counter,
            )));
        };
        let performance = state
            .summarize_usage_provider_performance(&UsageProviderPerformanceQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                granularity,
                tz_offset_minutes: time_range.tz_offset_minutes,
                limit,
                provider_id: query_param_value(request_context.query_string(), "provider_id"),
                model: query_param_value(request_context.query_string(), "model"),
                api_format: query_param_value(request_context.query_string(), "api_format"),
                endpoint_kind: query_param_value(request_context.query_string(), "endpoint_kind"),
                is_stream: query_param_optional_bool(request_context.query_string(), "is_stream"),
                has_format_conversion: query_param_optional_bool(
                    request_context.query_string(),
                    "has_format_conversion",
                ),
                slow_threshold_ms,
                include_timeline: query_param_optional_bool(
                    request_context.query_string(),
                    "include_timeline",
                )
                .unwrap_or(true),
            })
            .await?;
        return Ok(Some(build_admin_stats_provider_performance_response(
            &performance,
            usage_counter,
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
