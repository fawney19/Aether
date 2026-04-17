use super::super::super::stats::round_to;
use super::super::analytics::list_usage_cache_affinity_intervals;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_calculate_recommended_ttl,
    admin_usage_data_unavailable_response, admin_usage_parse_recent_hours,
    admin_usage_percentile_cont, admin_usage_ttl_recommendation_reason,
    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::UsageCacheAffinityIntervalGroupBy;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) async fn build_admin_usage_cache_affinity_ttl_analysis_response(
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
    let group_by_api_key = api_key_id.is_some();
    let intervals = list_usage_cache_affinity_intervals(
        state,
        hours,
        if group_by_api_key {
            UsageCacheAffinityIntervalGroupBy::ApiKey
        } else {
            UsageCacheAffinityIntervalGroupBy::User
        },
        user_id.as_deref(),
        api_key_id.as_deref(),
    )
    .await?;
    let grouped =
        intervals
            .into_iter()
            .fold(BTreeMap::<String, Vec<f64>>::new(), |mut grouped, row| {
                grouped
                    .entry(row.group_id)
                    .or_default()
                    .push(row.interval_minutes);
                grouped
            });

    let user_map: BTreeMap<String, aether_data::repository::users::StoredUserSummary> =
        if !group_by_api_key && state.has_user_data_reader() {
            let user_ids = grouped.keys().cloned().collect::<Vec<_>>();
            state
                .list_users_by_ids(&user_ids)
                .await?
                .into_iter()
                .map(|user| (user.id.clone(), user))
                .collect()
        } else {
            BTreeMap::new()
        };

    let mut ttl_distribution = json!({
        "5min": 0_u64,
        "15min": 0_u64,
        "30min": 0_u64,
        "60min": 0_u64,
    });
    let mut users = Vec::new();

    for (group_id, intervals) in grouped {
        if intervals.len() < 2 {
            continue;
        }

        let within_5min = intervals.iter().filter(|value| **value <= 5.0).count() as u64;
        let within_15min = intervals
            .iter()
            .filter(|value| **value > 5.0 && **value <= 15.0)
            .count() as u64;
        let within_30min = intervals
            .iter()
            .filter(|value| **value > 15.0 && **value <= 30.0)
            .count() as u64;
        let within_60min = intervals
            .iter()
            .filter(|value| **value > 30.0 && **value <= 60.0)
            .count() as u64;
        let over_60min = intervals.iter().filter(|value| **value > 60.0).count() as u64;
        let request_count = intervals.len() as u64;
        let p50 = admin_usage_percentile_cont(&intervals, 0.5);
        let p75 = admin_usage_percentile_cont(&intervals, 0.75);
        let p90 = admin_usage_percentile_cont(&intervals, 0.90);
        let avg_interval = intervals.iter().copied().sum::<f64>() / intervals.len() as f64;
        let min_interval = intervals.iter().copied().reduce(f64::min);
        let max_interval = intervals.iter().copied().reduce(f64::max);
        let recommended_ttl = admin_usage_calculate_recommended_ttl(p75, p90);
        match recommended_ttl {
            0..=5 => {
                ttl_distribution["5min"] = json!(ttl_distribution["5min"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_add(1))
            }
            6..=15 => {
                ttl_distribution["15min"] = json!(ttl_distribution["15min"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_add(1))
            }
            16..=30 => {
                ttl_distribution["30min"] = json!(ttl_distribution["30min"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_add(1))
            }
            _ => {
                ttl_distribution["60min"] = json!(ttl_distribution["60min"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_add(1))
            }
        }

        let (username, email) = if group_by_api_key {
            (Value::Null, Value::Null)
        } else if let Some(user) = user_map.get(&group_id) {
            (
                json!(user.username.clone()),
                json!(user.email.clone().unwrap_or_default()),
            )
        } else {
            (Value::Null, Value::Null)
        };

        users.push(json!({
            "group_id": group_id,
            "username": username,
            "email": email,
            "request_count": request_count,
            "interval_distribution": {
                "within_5min": within_5min,
                "within_15min": within_15min,
                "within_30min": within_30min,
                "within_60min": within_60min,
                "over_60min": over_60min,
            },
            "interval_percentages": {
                "within_5min": round_to(within_5min as f64 / request_count as f64 * 100.0, 1),
                "within_15min": round_to(within_15min as f64 / request_count as f64 * 100.0, 1),
                "within_30min": round_to(within_30min as f64 / request_count as f64 * 100.0, 1),
                "within_60min": round_to(within_60min as f64 / request_count as f64 * 100.0, 1),
                "over_60min": round_to(over_60min as f64 / request_count as f64 * 100.0, 1),
            },
            "percentiles": {
                "p50": p50.map(|value| round_to(value, 2)),
                "p75": p75.map(|value| round_to(value, 2)),
                "p90": p90.map(|value| round_to(value, 2)),
            },
            "avg_interval_minutes": round_to(avg_interval, 2),
            "min_interval_minutes": min_interval.map(|value| round_to(value, 2)),
            "max_interval_minutes": max_interval.map(|value| round_to(value, 2)),
            "recommended_ttl_minutes": recommended_ttl,
            "recommendation_reason": admin_usage_ttl_recommendation_reason(recommended_ttl, p75, p90),
        }));
    }

    users.sort_by(|left, right| {
        right["request_count"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&left["request_count"].as_u64().unwrap_or(0))
            .then_with(|| {
                left["group_id"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["group_id"].as_str().unwrap_or_default())
            })
    });

    Ok(Json(json!({
        "analysis_period_hours": hours,
        "total_users_analyzed": users.len(),
        "ttl_distribution": ttl_distribution,
        "users": users,
    }))
    .into_response())
}
