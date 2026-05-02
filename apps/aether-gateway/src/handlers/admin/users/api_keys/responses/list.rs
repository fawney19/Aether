use super::super::super::build_admin_users_bad_request_response;
use super::super::helpers::{format_optional_unix_secs_iso8601, masked_user_api_key_display};
use super::super::paths::admin_user_id_from_api_keys_path;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_optional_bool;
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn build_admin_list_user_api_keys_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_api_keys_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };
    let Some(user) = state.find_user_auth_by_id(&user_id).await? else {
        return Ok((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    };

    let active_filter = query_param_optional_bool(request_context.query_string(), "is_active");
    let mut export_records = state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user_id))
        .await?;
    if let Some(is_active) = active_filter {
        export_records.retain(|record| record.is_active == is_active);
    }

    let snapshot_ids = export_records
        .iter()
        .map(|record| record.api_key_id.clone())
        .collect::<Vec<_>>();
    let snapshot_by_id = state
        .list_auth_api_key_snapshots_by_ids(&snapshot_ids)
        .await?
        .into_iter()
        .map(|snapshot| (snapshot.api_key_id.clone(), snapshot))
        .collect::<std::collections::BTreeMap<_, _>>();

    let api_keys = export_records
        .into_iter()
        .map(|record| {
            let is_locked = snapshot_by_id
                .get(&record.api_key_id)
                .map(|snapshot| snapshot.api_key_is_locked)
                .unwrap_or(false);
            json!({
                "id": record.api_key_id,
                "name": record.name,
                "key_display": masked_user_api_key_display(state, record.key_encrypted.as_deref()),
                "is_active": record.is_active,
                "is_locked": is_locked,
                "total_requests": record.total_requests,
                "total_cost_usd": record.total_cost_usd,
                "rate_limit": record.rate_limit,
                "concurrent_limit": record.concurrent_limit,
                "expires_at": format_optional_unix_secs_iso8601(record.expires_at_unix_secs),
                "last_used_at": format_optional_unix_secs_iso8601(record.last_used_at_unix_secs),
                "created_at": format_optional_unix_secs_iso8601(record.created_at_unix_secs),
            })
        })
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "api_keys": api_keys,
        "total": api_keys.len(),
        "user_email": user.email,
        "username": user.username,
    }))
    .into_response())
}
