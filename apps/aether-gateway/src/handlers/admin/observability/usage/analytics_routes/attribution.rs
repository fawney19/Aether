use super::super::super::stats::resolve_admin_usage_time_range;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::stats::round_to;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_parse_aggregation_limit, ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::{
    StoredUsageAuditAggregation, UsageAuditAggregationGroupBy, UsageAuditAggregationQuery,
};
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttributionMetric {
    ActualCost,
    TotalCost,
    Tokens,
    Requests,
}

impl AttributionMetric {
    fn parse(value: Option<String>) -> Result<Self, String> {
        match value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("actual_cost")
        {
            "actual_cost" => Ok(Self::ActualCost),
            "total_cost" => Ok(Self::TotalCost),
            "tokens" => Ok(Self::Tokens),
            "requests" => Ok(Self::Requests),
            _ => Err(
                "Invalid metric value: must be one of actual_cost, total_cost, tokens, requests"
                    .to_string(),
            ),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ActualCost => "actual_cost",
            Self::TotalCost => "total_cost",
            Self::Tokens => "tokens",
            Self::Requests => "requests",
        }
    }

    fn value(self, row: &StoredUsageAuditAggregation) -> f64 {
        match self {
            Self::ActualCost => row.actual_total_cost_usd,
            Self::TotalCost => row.total_cost_usd,
            Self::Tokens => row.total_tokens as f64,
            Self::Requests => row.request_count as f64,
        }
    }
}

fn parse_group_by(value: Option<String>) -> Result<&'static str, String> {
    match value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("user")
    {
        "user" => Ok("user"),
        "api_key" => Err("group_by=api_key is not supported yet".to_string()),
        _ => Err("Invalid group_by value: must be user".to_string()),
    }
}

fn attribution_metric_total(
    metric: AttributionMetric,
    rows: &[StoredUsageAuditAggregation],
) -> f64 {
    rows.iter().map(|row| metric.value(row).max(0.0)).sum()
}

fn attribution_share(value: f64, total: f64) -> f64 {
    if total <= 0.0 {
        0.0
    } else {
        round_to((value / total).clamp(0.0, 1.0), 6)
    }
}

fn empty_others(total: f64) -> Value {
    json!({
        "id": "others",
        "name": "Others",
        "requests": 0,
        "total_tokens": 0,
        "total_cost": 0.0,
        "actual_cost": 0.0,
        "share": attribution_share(0.0, total),
    })
}

fn aggregate_others(
    rows: &[StoredUsageAuditAggregation],
    metric: AttributionMetric,
    total: f64,
) -> Value {
    if rows.is_empty() {
        return empty_others(total);
    }
    let requests = rows
        .iter()
        .fold(0_u64, |sum, row| sum.saturating_add(row.request_count));
    let total_tokens = rows
        .iter()
        .fold(0_u64, |sum, row| sum.saturating_add(row.total_tokens));
    let total_cost = rows.iter().map(|row| row.total_cost_usd).sum::<f64>();
    let actual_cost = rows
        .iter()
        .map(|row| row.actual_total_cost_usd)
        .sum::<f64>();
    let metric_value = rows
        .iter()
        .map(|row| metric.value(row).max(0.0))
        .sum::<f64>();
    json!({
        "id": "others",
        "name": "Others",
        "requests": requests,
        "total_tokens": total_tokens,
        "total_cost": round_to(total_cost, 6),
        "actual_cost": round_to(actual_cost, 6),
        "share": attribution_share(metric_value, total),
    })
}

async fn attribution_items_json(
    state: &AdminAppState<'_>,
    rows: &[StoredUsageAuditAggregation],
    metric: AttributionMetric,
    total: f64,
) -> Result<Vec<Value>, GatewayError> {
    let user_ids = rows
        .iter()
        .map(|row| row.group_key.clone())
        .collect::<Vec<_>>();
    let users = if state.has_user_data_reader() && !user_ids.is_empty() {
        state
            .list_users_by_ids(&user_ids)
            .await?
            .into_iter()
            .map(|user| (user.id, (user.email, user.username)))
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };

    Ok(rows
        .iter()
        .map(|row| {
            let (email, username) = users
                .get(&row.group_key)
                .cloned()
                .unwrap_or((None, String::new()));
            let display_name = if username.trim().is_empty() {
                email.clone().unwrap_or_else(|| row.group_key.clone())
            } else {
                username.clone()
            };
            let metric_value = metric.value(row).max(0.0);
            json!({
                "id": row.group_key,
                "user_id": row.group_key,
                "name": display_name,
                "email": email,
                "username": if username.is_empty() { Value::Null } else { json!(username) },
                "requests": row.request_count,
                "total_tokens": row.total_tokens,
                "total_cost": round_to(row.total_cost_usd, 6),
                "actual_cost": round_to(row.actual_total_cost_usd, 6),
                "share": attribution_share(metric_value, total),
            })
        })
        .collect())
}

pub(super) async fn build_admin_usage_attribution_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_usage_data_reader() {
        return Ok(admin_usage_data_unavailable_response(
            ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
        ));
    }

    let query = request_context.request_query_string.as_deref();
    let provider_id = query_param_value(query, "provider_id");
    let provider_name = query_param_value(query, "provider_name");
    if provider_id.is_none() && provider_name.is_none() {
        return Ok(admin_usage_bad_request_response(
            "provider_id or provider_name is required",
        ));
    }
    let group_by = match parse_group_by(query_param_value(query, "group_by")) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
    let metric = match AttributionMetric::parse(query_param_value(query, "metric")) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
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
        return Ok(Json(json!({
            "provider": { "id": provider_id, "name": provider_name },
            "group_by": group_by,
            "metric": metric.as_str(),
            "total": 0.0,
            "items": [],
            "others": empty_others(0.0),
        }))
        .into_response());
    };

    let mut rows = state
        .aggregate_usage_audits(&UsageAuditAggregationQuery {
            created_from_unix_secs,
            created_until_unix_secs,
            group_by: UsageAuditAggregationGroupBy::User,
            provider_id: provider_id.clone(),
            provider_name: provider_name.clone(),
            limit: 10_000,
            exclude_reserved_provider_labels: false,
        })
        .await?;
    rows.sort_by(|left, right| {
        metric
            .value(right)
            .partial_cmp(&metric.value(left))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.request_count.cmp(&left.request_count))
            .then_with(|| left.group_key.cmp(&right.group_key))
    });
    let total = attribution_metric_total(metric, &rows);
    let (visible_rows, overflow_rows) = if rows.len() > limit {
        rows.split_at(limit)
    } else {
        (rows.as_slice(), &[][..])
    };
    let items = attribution_items_json(state, visible_rows, metric, total).await?;
    let others = aggregate_others(overflow_rows, metric, total);

    Ok(Json(json!({
        "provider": {
            "id": provider_id,
            "name": provider_name,
        },
        "group_by": group_by,
        "metric": metric.as_str(),
        "total": round_to(total, 6),
        "items": items,
        "others": others,
    }))
    .into_response())
}
