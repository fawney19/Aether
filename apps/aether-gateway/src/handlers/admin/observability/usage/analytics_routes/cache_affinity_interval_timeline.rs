use super::super::analytics::list_usage_cache_affinity_intervals;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_bool, query_param_value, unix_secs_to_rfc3339};
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_parse_recent_hours, admin_usage_parse_timeline_limit, admin_usage_point_sort_key,
    admin_usage_proportional_limits, ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::UsageCacheAffinityIntervalGroupBy;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

async fn load_usage_cache_affinity_usernames(
    state: &AdminAppState<'_>,
    user_ids: &[String],
) -> Result<BTreeMap<String, String>, GatewayError> {
    Ok(state
        .resolve_auth_user_summaries_by_ids(user_ids)
        .await?
        .into_iter()
        .filter_map(|(user_id, user)| {
            (!user.username.trim().is_empty()).then_some((user_id, user.username))
        })
        .collect())
}

pub(super) async fn build_admin_usage_cache_affinity_interval_timeline_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_usage_data_reader() {
        return Ok(admin_usage_data_unavailable_response(
            ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
        ));
    }

    let query = request_context.request_query_string.as_deref();
    let hours = match admin_usage_parse_recent_hours(query, 24) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
    let limit = match admin_usage_parse_timeline_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_usage_bad_request_response(detail)),
    };
    let user_id = query_param_value(query, "user_id");
    let include_user_info = query_param_bool(query, "include_user_info", false);
    let intervals = list_usage_cache_affinity_intervals(
        state,
        hours,
        UsageCacheAffinityIntervalGroupBy::User,
        user_id.as_deref(),
        None,
    )
    .await?;
    let mut grouped: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    let mut models = BTreeSet::new();
    let mut legacy_usernames_by_user_id = BTreeMap::new();
    let mut usernames_by_user_id = BTreeMap::new();

    for row in intervals {
        if row.interval_minutes > 120.0 {
            continue;
        }
        let mut point = json!({
            "x": unix_secs_to_rfc3339(row.created_at_unix_secs),
            "y": ((row.interval_minutes * 100.0).round()) / 100.0,
        });
        if !row.model.trim().is_empty() {
            point["model"] = json!(row.model.clone());
            models.insert(row.model);
        }
        if include_user_info && user_id.is_none() {
            point["user_id"] = json!(row.group_id.clone());
            if let Some(username) = row.username {
                legacy_usernames_by_user_id
                    .entry(row.group_id.clone())
                    .or_insert(username);
            }
        }
        grouped.entry(row.group_id).or_default().push(point);
    }

    if include_user_info && user_id.is_none() {
        let user_ids: Vec<_> = grouped.keys().cloned().collect();
        let user_map = load_usage_cache_affinity_usernames(state, &user_ids).await?;
        for (user_id, username) in user_map {
            usernames_by_user_id.insert(user_id, username);
        }
        if !state.has_auth_user_data_reader() {
            for (user_id, username) in legacy_usernames_by_user_id {
                usernames_by_user_id.entry(user_id).or_insert(username);
            }
        }
    }

    let total_points_before_limit: usize = grouped.values().map(Vec::len).sum();
    let points: Vec<serde_json::Value> = if include_user_info && user_id.is_none() {
        let user_limits =
            admin_usage_proportional_limits(&grouped, limit, total_points_before_limit);
        let mut selected = Vec::new();
        for (group_user_id, mut items) in grouped {
            let take = user_limits
                .get(&group_user_id)
                .copied()
                .unwrap_or(items.len());
            selected.extend(items.drain(..std::cmp::min(take, items.len())));
        }
        selected.sort_by(admin_usage_point_sort_key);
        selected
    } else {
        let mut selected = grouped.into_values().flatten().collect::<Vec<_>>();
        selected.sort_by(admin_usage_point_sort_key);
        if selected.len() > limit {
            selected.truncate(limit);
        }
        selected
    };

    let mut response = json!({
        "analysis_period_hours": hours,
        "total_points": points.len(),
        "points": points,
    });
    if include_user_info && user_id.is_none() {
        response["users"] = json!(usernames_by_user_id);
    }
    if !models.is_empty() {
        response["models"] = json!(models.into_iter().collect::<Vec<_>>());
    }

    Ok(Json(response).into_response())
}
