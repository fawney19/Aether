use super::range::{build_time_range_from_days, parse_bounded_u32, parse_tz_offset_minutes};
use super::resolve_admin_usage_time_range;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::stats::{
    admin_stats_bad_request_response, admin_stats_cost_forecast_empty_response,
    admin_stats_cost_savings_empty_response, build_admin_stats_cost_forecast_response,
    build_admin_stats_cost_forecast_response_from_summaries,
    build_admin_stats_cost_savings_response, build_admin_stats_cost_savings_response_from_summary,
    AdminStatsGranularity, AdminStatsUsageFilter,
};
use aether_data_contracts::repository::usage::{
    UsageCostSavingsSummaryQuery, UsageTimeSeriesGranularity, UsageTimeSeriesQuery,
};
use axum::{body::Body, http, response::Response};

fn resolve_cost_forecast_time_range(
    query: Option<&str>,
) -> Result<super::AdminStatsTimeRange, String> {
    match super::AdminStatsTimeRange::resolve_optional(query)? {
        Some(value) => Ok(value),
        None => {
            let tz_offset_minutes = parse_tz_offset_minutes(query)?;
            let days = query_param_value(query, "days")
                .map(|value| parse_bounded_u32("days", &value, 7, 365))
                .transpose()?
                .unwrap_or(30);
            build_time_range_from_days(days, tz_offset_minutes)
        }
    }
}

pub(super) async fn maybe_build_local_admin_stats_cost_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let query = request_context.query_string();

    if request_context
        .decision()
        .and_then(|decision| decision.route_kind.as_deref())
        == Some("cost_forecast")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/cost/forecast" | "/api/admin/stats/cost/forecast/"
        )
    {
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_cost_forecast_empty_response()));
        }

        let forecast_days = match query_param_value(query, "forecast_days")
            .map(|value| parse_bounded_u32("forecast_days", &value, 1, 90))
            .transpose()
        {
            Ok(value) => value.unwrap_or(7),
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let time_range = match resolve_cost_forecast_time_range(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if let Err(detail) = time_range.validate_for_time_series(AdminStatsGranularity::Day) {
            return Ok(Some(admin_stats_bad_request_response(detail)));
        }

        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_cost_forecast_empty_response()));
        };
        let buckets = state
            .summarize_usage_time_series(&UsageTimeSeriesQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                granularity: UsageTimeSeriesGranularity::Day,
                tz_offset_minutes: time_range.tz_offset_minutes,
                user_id: None,
                provider_name: None,
                model: None,
            })
            .await?;
        return Ok(Some(
            build_admin_stats_cost_forecast_response_from_summaries(
                &time_range,
                forecast_days,
                &buckets,
            ),
        ));
    }

    if request_context
        .decision()
        .and_then(|decision| decision.route_kind.as_deref())
        == Some("cost_savings")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/stats/cost/savings" | "/api/admin/stats/cost/savings/"
        )
    {
        let time_range = match resolve_admin_usage_time_range(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_cost_savings_empty_response()));
        }

        let filters = AdminStatsUsageFilter {
            user_id: None,
            provider_name: query_param_value(query, "provider_name"),
            model: query_param_value(query, "model"),
        };
        let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
        else {
            return Ok(Some(admin_stats_cost_savings_empty_response()));
        };
        let summary = state
            .summarize_usage_cost_savings(&UsageCostSavingsSummaryQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                user_id: None,
                provider_name: filters.provider_name,
                model: filters.model,
            })
            .await?;

        return Ok(Some(build_admin_stats_cost_savings_response_from_summary(
            &summary,
        )));
    }

    Ok(None)
}
