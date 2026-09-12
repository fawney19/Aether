use super::{
    admin_pool_provider_id_from_scores_path, build_admin_pool_error_response,
    parse_admin_pool_page, parse_admin_pool_page_size,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::query_param_value;
use crate::GatewayError;
use aether_admin::provider::redaction::admin_provider_metadata_bucket_safe_json;
use aether_data_contracts::repository::pool_scores::{
    ListPoolMemberScoresQuery, PoolMemberHardState, PoolMemberProbeStatus,
    POOL_KIND_PROVIDER_KEY_POOL, POOL_SCORE_CAPABILITY_ACCOUNT, POOL_SCORE_SCOPE_KIND_ACCOUNT,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeMap;

pub(super) async fn build_admin_pool_scores_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(provider_id) = admin_pool_provider_id_from_scores_path(request_context.path()) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "provider_id 无效",
        ));
    };

    let query = request_context.query_string();
    let page = match parse_admin_pool_page(query) {
        Ok(value) => value,
        Err(message) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                message,
            ));
        }
    };
    let page_size = match parse_admin_pool_page_size(query) {
        Ok(value) => value.min(500),
        Err(message) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                message,
            ));
        }
    };
    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let hard_states = match parse_hard_state_filter(query) {
        Ok(value) => value,
        Err(message) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                message,
            ));
        }
    };
    let probe_statuses = match parse_probe_status_filter(query) {
        Ok(value) => value,
        Err(message) => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                message,
            ));
        }
    };

    let scores = state
        .app()
        .data
        .list_pool_member_scores(&ListPoolMemberScoresQuery {
            pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
            pool_id: provider_id.clone(),
            capability: Some(POOL_SCORE_CAPABILITY_ACCOUNT.to_string()),
            scope_kind: Some(POOL_SCORE_SCOPE_KIND_ACCOUNT.to_string()),
            scope_id: None,
            hard_states,
            probe_statuses,
            offset,
            limit: page_size,
        })
        .await
        .map_err(|err| GatewayError::Internal(format!("{err:?}")))?;

    let key_ids = scores
        .iter()
        .map(|score| score.member_id.clone())
        .collect::<Vec<_>>();
    let keys = state
        .app()
        .read_provider_catalog_keys_by_ids(&key_ids)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|key| (key.id.clone(), key))
        .collect::<BTreeMap<_, _>>();

    let items = scores
        .into_iter()
        .map(|score| {
            let key = keys.get(&score.member_id);
            json!({
                "id": score.id,
                "pool_kind": score.pool_kind,
                "pool_id": score.pool_id,
                "member_kind": score.member_kind,
                "member_id": score.member_id,
                "capability": score.capability,
                "scope_kind": score.scope_kind,
                "scope_id": score.scope_id,
                "score": score.score,
                "hard_state": score.hard_state.as_database(),
                "score_version": score.score_version,
                "score_reason": admin_provider_metadata_bucket_safe_json(
                    "pool_score",
                    Some(&score.score_reason),
                ),
                "last_ranked_at": score.last_ranked_at,
                "last_scheduled_at": score.last_scheduled_at,
                "last_success_at": score.last_success_at,
                "last_failure_at": score.last_failure_at,
                "failure_count": score.failure_count,
                "last_probe_attempt_at": score.last_probe_attempt_at,
                "last_probe_success_at": score.last_probe_success_at,
                "last_probe_failure_at": score.last_probe_failure_at,
                "probe_failure_count": score.probe_failure_count,
                "probe_status": score.probe_status.as_database(),
                "updated_at": score.updated_at,
                "key": key.map(|key| json!({
                    "id": key.id,
                    "name": key.name,
                    "auth_type": key.auth_type,
                    "is_active": key.is_active,
                    "internal_priority": key.internal_priority,
                    "last_used_at": key.last_used_at_unix_secs,
                }))
            })
        })
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "provider_id": provider_id,
        "page": page,
        "page_size": page_size,
        "filters": {
            "api_format": serde_json::Value::Null,
            "model_id": serde_json::Value::Null,
            "hard_state": query_param_value(query, "hard_state"),
            "probe_status": query_param_value(query, "probe_status")
        },
        "items": items
    }))
    .into_response())
}

fn parse_hard_state_filter(query: Option<&str>) -> Result<Vec<PoolMemberHardState>, String> {
    let Some(raw) = query_param_value(query, "hard_state") else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(|value| match value.trim() {
            "available" => Ok(PoolMemberHardState::Available),
            "unknown" => Ok(PoolMemberHardState::Unknown),
            "cooldown" => Ok(PoolMemberHardState::Cooldown),
            "quota_exhausted" => Ok(PoolMemberHardState::QuotaExhausted),
            "auth_invalid" => Ok(PoolMemberHardState::AuthInvalid),
            "banned" => Ok(PoolMemberHardState::Banned),
            "inactive" => Ok(PoolMemberHardState::Inactive),
            _ => Err("hard_state must be one of: available, unknown, cooldown, quota_exhausted, auth_invalid, banned, inactive".to_string()),
        })
        .collect()
}

fn parse_probe_status_filter(
    query: Option<&str>,
) -> Result<Option<Vec<PoolMemberProbeStatus>>, String> {
    let Some(raw) = query_param_value(query, "probe_status") else {
        return Ok(None);
    };
    raw.split(',')
        .map(|value| match value.trim() {
            "never" => Ok(PoolMemberProbeStatus::Never),
            "ok" => Ok(PoolMemberProbeStatus::Ok),
            "failed" => Ok(PoolMemberProbeStatus::Failed),
            "stale" => Ok(PoolMemberProbeStatus::Stale),
            "in_progress" => Ok(PoolMemberProbeStatus::InProgress),
            _ => Err(
                "probe_status must be one of: never, ok, failed, stale, in_progress".to_string(),
            ),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}
