use super::super::super::stats::{AdminStatsTimeRange, AdminStatsUsageFilter};
use super::super::analytics::admin_usage_aggregation_by_user_json;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_aggregation_by_api_format_json, admin_usage_aggregation_by_model_json,
    admin_usage_aggregation_by_provider_json, admin_usage_bad_request_response,
    admin_usage_data_unavailable_response, admin_usage_parse_aggregation_limit,
    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn build_admin_usage_aggregation_stats_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_usage_data_reader() {
        return Ok(admin_usage_data_unavailable_response(
            ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
        ));
    }

    let query = request_context.request_query_string.as_deref();
    let group_by = query_param_value(query, "group_by")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        group_by.as_str(),
        "model" | "user" | "provider" | "api_format"
    ) {
        return Ok(admin_usage_bad_request_response(
            "Invalid group_by value: must be one of model, user, provider, api_format",
        ));
    }
    let limit = match admin_usage_parse_aggregation_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
    let time_range = match AdminStatsTimeRange::resolve_optional(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };

    let mut usage = state
        .list_admin_usage_for_optional_range(time_range.as_ref(), &AdminStatsUsageFilter::default())
        .await?;
    usage.retain(|item| item.status != "pending" && item.status != "streaming");

    let response = match group_by.as_str() {
        "model" => admin_usage_aggregation_by_model_json(&usage, limit),
        "user" => admin_usage_aggregation_by_user_json(state, &usage, limit).await?,
        "provider" => admin_usage_aggregation_by_provider_json(&usage, limit),
        "api_format" => admin_usage_aggregation_by_api_format_json(&usage, limit),
        _ => unreachable!(),
    };
    Ok(Json(response).into_response())
}
