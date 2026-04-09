use super::super::super::{
    build_admin_users_bad_request_response, build_admin_users_read_only_response,
    AdminToggleUserApiKeyLockRequest,
};
use super::super::helpers::attach_audit_response;
use super::super::paths::admin_user_api_key_lock_parts;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn build_admin_toggle_user_api_key_lock_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_auth_api_key_writer() {
        return Ok(build_admin_users_read_only_response(
            "当前为只读模式，无法锁定或解锁用户 API Key",
        ));
    }

    let Some((user_id, api_key_id)) = admin_user_api_key_lock_parts(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response(
            "缺少 user_id 或 key_id",
        ));
    };

    let Some(snapshot) = state
        .list_auth_api_key_snapshots_by_ids(std::slice::from_ref(&api_key_id))
        .await?
        .into_iter()
        .find(|snapshot| snapshot.user_id == user_id && snapshot.api_key_id == api_key_id)
    else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "API Key不存在或不属于该用户" })),
        )
            .into_response());
    };

    if snapshot.api_key_is_standalone {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "API Key不存在或不属于该用户" })),
        )
            .into_response());
    }

    let desired_is_locked = match request_body {
        None => !snapshot.api_key_is_locked,
        Some(body) if body.is_empty() => !snapshot.api_key_is_locked,
        Some(body) => match serde_json::from_slice::<AdminToggleUserApiKeyLockRequest>(body) {
            Ok(payload) => payload.locked.unwrap_or(!snapshot.api_key_is_locked),
            Err(_) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求数据验证失败" })),
                )
                    .into_response());
            }
        },
    };

    if !state
        .set_user_api_key_locked(&user_id, &api_key_id, desired_is_locked)
        .await?
    {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "API Key不存在或不属于该用户" })),
        )
            .into_response());
    }

    Ok(attach_audit_response(
        Json(json!({
            "id": api_key_id,
            "is_locked": desired_is_locked,
            "message": if desired_is_locked {
                "API密钥已锁定"
            } else {
                "API密钥已解锁"
            },
        }))
        .into_response(),
        "admin_user_api_key_lock_toggled",
        "toggle_user_api_key_lock",
        "user_api_key",
        &api_key_id,
    ))
}
