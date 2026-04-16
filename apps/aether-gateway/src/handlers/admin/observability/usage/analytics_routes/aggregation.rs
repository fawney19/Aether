use super::super::super::stats::resolve_admin_usage_time_range;
use super::super::analytics::admin_usage_aggregation_by_user_json;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::stats::round_to;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_parse_aggregation_limit, admin_usage_token_cache_hit_rate,
    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::{
    StoredUsageAuditAggregation, UsageAuditAggregationGroupBy, UsageAuditAggregationQuery,
};
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn admin_usage_aggregation_by_model_json(
    rows: &[StoredUsageAuditAggregation],
) -> serde_json::Value {
    json!(rows
        .iter()
        .map(|row| {
            json!({
                "model": row.group_key,
                "request_count": row.request_count,
                "total_tokens": row.total_tokens,
                "effective_input_tokens": row.effective_input_tokens,
                "total_input_context": row.total_input_context,
                "output_tokens": row.output_tokens,
                "total_cost": round_to(row.total_cost_usd, 6),
                "actual_cost": round_to(row.actual_total_cost_usd, 6),
                "cache_creation_tokens": row.cache_creation_tokens,
                "cache_creation_ephemeral_5m_tokens": row.cache_creation_ephemeral_5m_tokens,
                "cache_creation_ephemeral_1h_tokens": row.cache_creation_ephemeral_1h_tokens,
                "cache_read_tokens": row.cache_read_tokens,
                "cache_hit_rate": admin_usage_token_cache_hit_rate(
                    row.total_input_context,
                    row.cache_read_tokens,
                ),
            })
        })
        .collect::<Vec<_>>())
}

fn admin_usage_aggregation_by_provider_json(
    rows: &[StoredUsageAuditAggregation],
) -> serde_json::Value {
    json!(rows
        .iter()
        .map(|row| {
            let success_count = row.success_count.unwrap_or_default();
            let error_count = row.request_count.saturating_sub(success_count);
            let success_rate = if row.request_count == 0 {
                0.0
            } else {
                round_to(success_count as f64 / row.request_count as f64 * 100.0, 2)
            };
            json!({
                "provider_id": row.group_key,
                "provider": row.display_name.clone().unwrap_or_else(|| "Unknown".to_string()),
                "request_count": row.request_count,
                "total_tokens": row.total_tokens,
                "effective_input_tokens": row.effective_input_tokens,
                "total_input_context": row.total_input_context,
                "output_tokens": row.output_tokens,
                "total_cost": round_to(row.total_cost_usd, 6),
                "actual_cost": round_to(row.actual_total_cost_usd, 6),
                "avg_response_time_ms": round_to(row.avg_response_time_ms.unwrap_or(0.0), 2),
                "success_rate": success_rate,
                "error_count": error_count,
                "cache_creation_tokens": row.cache_creation_tokens,
                "cache_creation_ephemeral_5m_tokens": row.cache_creation_ephemeral_5m_tokens,
                "cache_creation_ephemeral_1h_tokens": row.cache_creation_ephemeral_1h_tokens,
                "cache_read_tokens": row.cache_read_tokens,
                "cache_hit_rate": admin_usage_token_cache_hit_rate(
                    row.total_input_context,
                    row.cache_read_tokens,
                ),
            })
        })
        .collect::<Vec<_>>())
}

fn admin_usage_aggregation_by_api_format_json(
    rows: &[StoredUsageAuditAggregation],
) -> serde_json::Value {
    json!(rows
        .iter()
        .map(|row| {
            json!({
                "api_format": row.group_key,
                "request_count": row.request_count,
                "total_tokens": row.total_tokens,
                "effective_input_tokens": row.effective_input_tokens,
                "total_input_context": row.total_input_context,
                "output_tokens": row.output_tokens,
                "total_cost": round_to(row.total_cost_usd, 6),
                "actual_cost": round_to(row.actual_total_cost_usd, 6),
                "avg_response_time_ms": round_to(row.avg_response_time_ms.unwrap_or(0.0), 2),
                "cache_creation_tokens": row.cache_creation_tokens,
                "cache_creation_ephemeral_5m_tokens": row.cache_creation_ephemeral_5m_tokens,
                "cache_creation_ephemeral_1h_tokens": row.cache_creation_ephemeral_1h_tokens,
                "cache_read_tokens": row.cache_read_tokens,
                "cache_hit_rate": admin_usage_token_cache_hit_rate(
                    row.total_input_context,
                    row.cache_read_tokens,
                ),
            })
        })
        .collect::<Vec<_>>())
}

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
    let time_range = match resolve_admin_usage_time_range(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
    let Some((created_from_unix_secs, created_until_unix_secs)) = time_range.to_unix_bounds()
    else {
        return Ok(Json(json!([])).into_response());
    };
    let group_by_query = match group_by.as_str() {
        "model" => UsageAuditAggregationGroupBy::Model,
        "user" => UsageAuditAggregationGroupBy::User,
        "provider" => UsageAuditAggregationGroupBy::Provider,
        "api_format" => UsageAuditAggregationGroupBy::ApiFormat,
        _ => unreachable!(),
    };
    let usage = state
        .aggregate_usage_audits(&UsageAuditAggregationQuery {
            created_from_unix_secs,
            created_until_unix_secs,
            group_by: group_by_query,
            limit,
        })
        .await?;

    let response = match group_by.as_str() {
        "model" => admin_usage_aggregation_by_model_json(&usage),
        "user" => admin_usage_aggregation_by_user_json(state, &usage).await?,
        "provider" => admin_usage_aggregation_by_provider_json(&usage),
        "api_format" => admin_usage_aggregation_by_api_format_json(&usage),
        _ => unreachable!(),
    };
    Ok(Json(response).into_response())
}
