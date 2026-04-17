use super::super::super::{
    build_admin_users_bad_request_response, build_admin_users_read_only_response,
    AdminUpdateUserApiKeyRequest,
};
use super::super::helpers::{
    attach_audit_response, build_admin_user_api_key_detail_payload,
    normalize_admin_optional_api_key_name,
};
use super::super::paths::admin_user_api_key_parts;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::normalize_optional_api_key_concurrent_limit;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn build_admin_update_user_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法更新用户 API Key",
        ));
    }

    let Some((user_id, api_key_id)) = admin_user_api_key_parts(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response(
            "缺少 user_id 或 key_id",
        ));
    };
    let Some(request_body) = request_body else {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "请求数据验证失败" })),
        )
            .into_response());
    };
    let payload = match serde_json::from_slice::<AdminUpdateUserApiKeyRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "请求数据验证失败" })),
            )
                .into_response());
        }
    };
    let name = match normalize_admin_optional_api_key_name(payload.name) {
        Ok(value) => value,
        Err(detail) => {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response());
        }
    };
    if payload.rate_limit.is_some_and(|value| value < 0) {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "rate_limit 必须大于等于 0" })),
        )
            .into_response());
    }
    let concurrent_limit =
        match normalize_optional_api_key_concurrent_limit(payload.concurrent_limit) {
            Ok(value) => value,
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response());
            }
        };

    let Some(updated) = state
        .update_user_api_key_basic(aether_data::repository::auth::UpdateUserApiKeyBasicRecord {
            user_id,
            api_key_id: api_key_id.clone(),
            name,
            rate_limit: payload.rate_limit,
            concurrent_limit,
        })
        .await?
    else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "API Key不存在或不属于该用户" })),
        )
            .into_response());
    };

    let is_locked = state
        .list_auth_api_key_snapshots_by_ids(std::slice::from_ref(&api_key_id))
        .await?
        .into_iter()
        .find(|snapshot| snapshot.api_key_id == api_key_id)
        .map(|snapshot| snapshot.api_key_is_locked)
        .unwrap_or(false);
    let mut payload = build_admin_user_api_key_detail_payload(state, &updated, is_locked);
    payload["message"] = json!("API Key更新成功");
    Ok(attach_audit_response(
        Json(payload).into_response(),
        "admin_user_api_key_updated",
        "update_user_api_key",
        "user_api_key",
        &api_key_id,
    ))
}
