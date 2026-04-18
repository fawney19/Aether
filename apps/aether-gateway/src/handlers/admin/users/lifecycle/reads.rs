use super::super::{build_admin_users_bad_request_response, format_optional_datetime_iso8601};
use super::support::{
    admin_user_id_from_detail_path, build_admin_user_payload, find_admin_export_user,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_optional_bool, query_param_value};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeMap;

pub(in super::super) async fn build_admin_list_users_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let skip = query_param_value(request_context.query_string(), "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(request_context.query_string(), "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100)
        .clamp(1, 1000);
    let role = query_param_value(request_context.query_string(), "role")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let is_active = query_param_optional_bool(request_context.query_string(), "is_active");

    let paged_rows = state
        .list_export_users_page(&aether_data::repository::users::UserExportListQuery {
            skip,
            limit,
            role: role.clone(),
            is_active,
        })
        .await?;
    let user_ids = paged_rows
        .iter()
        .map(|row| row.id.clone())
        .collect::<Vec<_>>();
    let auth_by_user_id = state
        .list_user_auth_by_ids(&user_ids)
        .await?
        .into_iter()
        .map(|user| (user.id.clone(), user))
        .collect::<BTreeMap<_, _>>();
    let wallet_by_user_id = state
        .list_wallet_snapshots_by_user_ids(&user_ids)
        .await?
        .into_iter()
        .filter_map(|wallet| wallet.user_id.clone().map(|user_id| (user_id, wallet)))
        .collect::<BTreeMap<_, _>>();
    let usage_totals_by_user_id = state
        .summarize_usage_totals_by_user_ids(&user_ids)
        .await?
        .into_iter()
        .map(|item| (item.user_id.clone(), item))
        .collect::<BTreeMap<_, _>>();

    let mut payload = Vec::with_capacity(paged_rows.len());
    for row in paged_rows {
        let auth = auth_by_user_id.get(&row.id);
        let unlimited = wallet_by_user_id
            .get(&row.id)
            .is_some_and(|wallet| wallet.limit_mode.eq_ignore_ascii_case("unlimited"));
        let usage_totals = usage_totals_by_user_id.get(&row.id);
        payload.push(json!({
            "id": row.id,
            "email": row.email,
            "username": row.username,
            "role": row.role,
            "allowed_providers": row.allowed_providers,
            "allowed_api_formats": row.allowed_api_formats,
            "allowed_models": row.allowed_models,
            "rate_limit": row.rate_limit,
            "unlimited": unlimited,
            "is_active": row.is_active,
            "created_at": format_optional_datetime_iso8601(auth.as_ref().and_then(|user| user.created_at)),
            "updated_at": serde_json::Value::Null,
            "last_login_at": format_optional_datetime_iso8601(
                auth.as_ref().and_then(|user| user.last_login_at),
            ),
            "request_count": usage_totals.map(|item| item.request_count).unwrap_or_default(),
            "total_tokens": usage_totals.map(|item| item.total_tokens).unwrap_or_default(),
        }));
    }

    Ok(Json(payload).into_response())
}

pub(in super::super) async fn build_admin_get_user_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_detail_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };
    let Some(user) = state.find_user_auth_by_id(&user_id).await? else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    };

    let wallet = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            &user_id,
        ))
        .await?;
    let export_row = find_admin_export_user(state, &user_id).await?;
    let unlimited = wallet
        .as_ref()
        .is_some_and(|wallet| wallet.limit_mode.eq_ignore_ascii_case("unlimited"));
    Ok(Json(build_admin_user_payload(
        &user,
        export_row.as_ref().and_then(|row| row.rate_limit),
        unlimited,
    ))
    .into_response())
}
