use super::super::super::stats::round_to;
use super::super::analytics::list_recent_completed_usage_for_cache_affinity;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_cache_creation_tokens,
    admin_usage_data_unavailable_response, admin_usage_matches_optional_id,
    admin_usage_parse_recent_hours, admin_usage_total_input_context,
    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn build_admin_usage_cache_affinity_hit_analysis_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_usage_data_reader() {
        return Ok(admin_usage_data_unavailable_response(
            ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
        ));
    }

    let query = request_context.request_query_string.as_deref();
    let hours = match admin_usage_parse_recent_hours(query, 168) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
    let user_id = query_param_value(query, "user_id");
    let api_key_id = query_param_value(query, "api_key_id");
    let usage =
        list_recent_completed_usage_for_cache_affinity(state, hours, user_id.as_deref()).await?;
    let filtered: Vec<_> = usage
        .into_iter()
        .filter(|item| {
            admin_usage_matches_optional_id(item.user_id.as_deref(), user_id.as_deref())
                && admin_usage_matches_optional_id(
                    item.api_key_id.as_deref(),
                    api_key_id.as_deref(),
                )
        })
        .collect();
    let total_requests = filtered.len();
    let total_input_tokens: u64 = filtered.iter().map(|item| item.input_tokens).sum();
    let total_cache_read_tokens: u64 = filtered
        .iter()
        .map(|item| item.cache_read_input_tokens)
        .sum();
    let total_cache_creation_tokens: u64 =
        filtered.iter().map(admin_usage_cache_creation_tokens).sum();
    let total_input_context: u64 = filtered.iter().map(admin_usage_total_input_context).sum();
    let total_cache_read_cost: f64 = filtered.iter().map(|item| item.cache_read_cost_usd).sum();
    let total_cache_creation_cost: f64 = filtered
        .iter()
        .map(|item| item.cache_creation_cost_usd)
        .sum();
    let requests_with_cache_hit = filtered
        .iter()
        .filter(|item| item.cache_read_input_tokens > 0)
        .count();
    let token_cache_hit_rate = if total_input_context == 0 {
        0.0
    } else {
        round_to(
            total_cache_read_tokens as f64 / total_input_context as f64 * 100.0,
            2,
        )
    };
    let request_cache_hit_rate = if total_requests == 0 {
        0.0
    } else {
        round_to(
            requests_with_cache_hit as f64 / total_requests as f64 * 100.0,
            2,
        )
    };

    Ok(Json(json!({
        "analysis_period_hours": hours,
        "total_requests": total_requests,
        "requests_with_cache_hit": requests_with_cache_hit,
        "request_cache_hit_rate": request_cache_hit_rate,
        "total_input_tokens": total_input_tokens,
        "total_cache_read_tokens": total_cache_read_tokens,
        "total_cache_creation_tokens": total_cache_creation_tokens,
        "token_cache_hit_rate": token_cache_hit_rate,
        "total_cache_read_cost_usd": round_to(total_cache_read_cost, 4),
        "total_cache_creation_cost_usd": round_to(total_cache_creation_cost, 4),
        "estimated_savings_usd": round_to(total_cache_read_cost * 9.0, 4),
    }))
    .into_response())
}
