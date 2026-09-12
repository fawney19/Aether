use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    build_auth_error_response, format_users_me_optional_datetime_iso8601,
    format_users_me_required_session_datetime_iso8601, resolve_authenticated_local_user, AppState,
    GatewayPublicRequestContext,
};
use crate::handlers::public::support::mark_sensitive_response_no_store;
use crate::GatewayUserSessionView;

#[derive(Debug, Deserialize)]
struct UsersMeUpdateSessionLabelRequest {
    device_label: String,
}

fn users_me_session_id_from_path(request_path: &str) -> Option<String> {
    let raw = request_path
        .strip_prefix("/api/users/me/sessions/")?
        .trim()
        .trim_matches('/');
    if raw.is_empty() || raw.contains('/') {
        return None;
    }
    Some(raw.to_string())
}

pub(super) fn users_me_session_detail_path_matches(request_path: &str) -> bool {
    users_me_session_id_from_path(request_path).is_some()
}

fn build_users_me_session_payload(
    session: GatewayUserSessionView,
    current_session_id: &str,
) -> serde_json::Value {
    json!({
        "id": session.id,
        "device_label": session
            .device_label
            .clone()
            .unwrap_or_else(|| "未知设备".to_string()),
        "device_type": "unknown",
        "browser_name": serde_json::Value::Null,
        "browser_version": serde_json::Value::Null,
        "os_name": serde_json::Value::Null,
        "os_version": serde_json::Value::Null,
        "device_model": serde_json::Value::Null,
        "ip_address": session.ip_address,
        "last_seen_at": format_users_me_optional_datetime_iso8601(session.last_seen_at),
        "created_at": format_users_me_required_session_datetime_iso8601(&session),
        "is_current": session.id == current_session_id,
        "revoked_at": format_users_me_optional_datetime_iso8601(session.revoked_at),
        "revoke_reason": session.revoke_reason,
    })
}

pub(super) async fn handle_users_me_sessions_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let sessions = match state.list_user_sessions(&auth.user.id).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user session lookup failed: {err:?}"),
                false,
            )
        }
    };

    mark_sensitive_response_no_store(
        Json(
            sessions
                .into_iter()
                .map(|session| build_users_me_session_payload(session, &auth.session_id))
                .collect::<Vec<_>>(),
        )
        .into_response(),
    )
}

pub(super) async fn handle_users_me_delete_other_sessions(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let sessions = match state.list_user_sessions(&auth.user.id).await {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user session lookup failed: {err:?}"),
                false,
            )
        }
    };

    let now = chrono::Utc::now();
    let mut revoked_count = 0_u64;
    for session in sessions {
        if session.id == auth.session_id {
            continue;
        }
        match state
            .revoke_user_session(&auth.user.id, &session.id, now, "logout_other_sessions")
            .await
        {
            Ok(true) => revoked_count += 1,
            Ok(false) => {}
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user session revoke failed: {err:?}"),
                    false,
                )
            }
        }
    }

    Json(json!({
        "message": "其他设备已退出登录",
        "revoked_count": revoked_count,
    }))
    .into_response()
}

pub(super) async fn handle_users_me_delete_session(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(session_id) = users_me_session_id_from_path(&request_context.request_path) else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "会话不存在", false);
    };

    let session = match state.find_user_session(&auth.user.id, &session_id).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_error_response(http::StatusCode::NOT_FOUND, "会话不存在", false)
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user session lookup failed: {err:?}"),
                false,
            )
        }
    };
    if session.is_revoked() || session.is_expired(chrono::Utc::now()) {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "会话不存在", false);
    }

    match state
        .revoke_user_session(
            &auth.user.id,
            &session_id,
            chrono::Utc::now(),
            "user_session_revoked",
        )
        .await
    {
        Ok(true) => Json(json!({ "message": "设备已退出登录" })).into_response(),
        Ok(false) => build_auth_error_response(http::StatusCode::NOT_FOUND, "会话不存在", false),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("user session revoke failed: {err:?}"),
            false,
        ),
    }
}

pub(super) async fn handle_users_me_update_session(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(session_id) = users_me_session_id_from_path(&request_context.request_path) else {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "会话不存在", false);
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "请求数据验证失败", false);
    };
    let payload = match serde_json::from_slice::<UsersMeUpdateSessionLabelRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求数据验证失败",
                false,
            )
        }
    };
    let device_label = payload.device_label.trim();
    if device_label.is_empty() {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "设备名称不能为空", false);
    }
    let device_label = device_label.chars().take(120).collect::<String>();

    let session = match state.find_user_session(&auth.user.id, &session_id).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_error_response(http::StatusCode::NOT_FOUND, "会话不存在", false)
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user session lookup failed: {err:?}"),
                false,
            )
        }
    };
    if session.is_revoked() || session.is_expired(chrono::Utc::now()) {
        return build_auth_error_response(http::StatusCode::NOT_FOUND, "会话不存在", false);
    }

    let now = chrono::Utc::now();
    match state
        .update_user_session_device_label(&auth.user.id, &session_id, &device_label, now)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return build_auth_error_response(http::StatusCode::NOT_FOUND, "会话不存在", false)
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user session update failed: {err:?}"),
                false,
            )
        }
    }

    let mut updated = session;
    updated.device_label = Some(device_label);
    updated.updated_at = Some(now);
    Json(build_users_me_session_payload(updated, &auth.session_id)).into_response()
}
