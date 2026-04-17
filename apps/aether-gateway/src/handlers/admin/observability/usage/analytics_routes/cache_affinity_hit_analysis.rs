use super::super::super::stats::round_to;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_parse_recent_hours, ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::UsageCacheAffinityHitSummaryQuery;
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
    let now_unix_secs = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default();
    let summary = state
        .summarize_usage_cache_affinity_hit_summary(&UsageCacheAffinityHitSummaryQuery {
            created_from_unix_secs: now_unix_secs.saturating_sub(u64::from(hours) * 3600),
            created_until_unix_secs: now_unix_secs.saturating_add(1),
            user_id,
            api_key_id,
        })
        .await?;
    let token_cache_hit_rate = if summary.total_input_context == 0 {
        0.0
    } else {
        round_to(
            summary.cache_read_tokens as f64 / summary.total_input_context as f64 * 100.0,
            2,
        )
    };
    let request_cache_hit_rate = if summary.total_requests == 0 {
        0.0
    } else {
        round_to(
            summary.requests_with_cache_hit as f64 / summary.total_requests as f64 * 100.0,
            2,
        )
    };

    Ok(Json(json!({
        "analysis_period_hours": hours,
        "total_requests": summary.total_requests,
        "requests_with_cache_hit": summary.requests_with_cache_hit,
        "request_cache_hit_rate": request_cache_hit_rate,
        "total_input_tokens": summary.input_tokens,
        "total_cache_read_tokens": summary.cache_read_tokens,
        "total_cache_creation_tokens": summary.cache_creation_tokens,
        "token_cache_hit_rate": token_cache_hit_rate,
        "total_cache_read_cost_usd": round_to(summary.cache_read_cost_usd, 4),
        "total_cache_creation_cost_usd": round_to(summary.cache_creation_cost_usd, 4),
        "estimated_savings_usd": round_to(summary.cache_read_cost_usd * 9.0, 4),
    }))
    .into_response())
}
