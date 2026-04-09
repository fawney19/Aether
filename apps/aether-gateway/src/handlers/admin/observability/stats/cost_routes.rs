use super::range::{build_time_range_from_days, parse_bounded_u32, parse_tz_offset_minutes};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::stats::{
    admin_stats_bad_request_response, admin_stats_cost_forecast_empty_response,
    admin_stats_cost_savings_empty_response, build_admin_stats_cost_forecast_response,
    build_admin_stats_cost_savings_response, AdminStatsGranularity, AdminStatsTimeRange,
    AdminStatsUsageFilter,
};
use axum::{body::Body, http, response::Response};

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
            Ok(Some(value)) => value,
            Ok(None) => 7,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let tz_offset_minutes = match parse_tz_offset_minutes(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        let time_range = match AdminStatsTimeRange::resolve_optional(query) {
            Ok(Some(value)) => value,
            Ok(None) => {
                let days = match query_param_value(query, "days")
                    .map(|value| parse_bounded_u32("days", &value, 7, 365))
                    .transpose()
                {
                    Ok(Some(value)) => value,
                    Ok(None) => 30,
                    Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
                };
                match build_time_range_from_days(days, tz_offset_minutes) {
                    Ok(value) => value,
                    Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
                }
            }
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if let Err(detail) = time_range.validate_for_time_series(AdminStatsGranularity::Day) {
            return Ok(Some(admin_stats_bad_request_response(detail)));
        }

        let usage = state
            .list_admin_usage_for_range(&time_range, &AdminStatsUsageFilter::default())
            .await?;
        return Ok(Some(build_admin_stats_cost_forecast_response(
            &time_range,
            forecast_days,
            &usage,
        )));
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
        let time_range = match AdminStatsTimeRange::resolve_optional(query) {
            Ok(value) => value,
            Err(detail) => return Ok(Some(admin_stats_bad_request_response(detail))),
        };
        if time_range.is_none() {
            return Ok(Some(admin_stats_cost_savings_empty_response()));
        }
        if !state.has_usage_data_reader() {
            return Ok(Some(admin_stats_cost_savings_empty_response()));
        }

        let filters = AdminStatsUsageFilter {
            user_id: None,
            provider_name: query_param_value(query, "provider_name"),
            model: query_param_value(query, "model"),
        };
        let usage = state
            .list_admin_usage_for_range(time_range.as_ref().expect("time range exists"), &filters)
            .await?;

        return Ok(Some(build_admin_stats_cost_savings_response(&usage)));
    }

    Ok(None)
}
