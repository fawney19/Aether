use super::{build_admin_users_bad_request_response, format_optional_datetime_iso8601};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::{GatewayError, GatewayUserSessionView};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn admin_user_id_from_sessions_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/users/")?
        .strip_suffix("/sessions")
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn admin_user_session_parts(request_path: &str) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/users/")?;
    let (user_id, session_id) = raw.split_once("/sessions/")?;
    let user_id = user_id.trim().trim_matches('/');
    let session_id = session_id.trim().trim_matches('/');
    if user_id.is_empty()
        || session_id.is_empty()
        || user_id.contains('/')
        || session_id.contains('/')
    {
        None
    } else {
        Some((user_id.to_string(), session_id.to_string()))
    }
}

fn format_required_session_datetime_iso8601(session: &GatewayUserSessionView) -> String {
    session
        .created_at
        .or(session.updated_at)
        .or(session.last_seen_at)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

pub(super) async fn build_admin_list_user_sessions_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_sessions_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };

    if state.find_user_auth_by_id(&user_id).await?.is_none() {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }

    let sessions = state.list_user_sessions(&user_id).await?;
    let payload = sessions
        .into_iter()
        .map(|session| {
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
                "last_seen_at": format_optional_datetime_iso8601(session.last_seen_at),
                "created_at": format_required_session_datetime_iso8601(&session),
                "is_current": false,
                "revoked_at": format_optional_datetime_iso8601(session.revoked_at),
                "revoke_reason": session.revoke_reason,
            })
        })
        .collect::<Vec<_>>();

    Ok(Json(payload).into_response())
}

pub(super) async fn build_admin_delete_user_session_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((user_id, session_id)) = admin_user_session_parts(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response(
            "缺少 user_id 或 session_id",
        ));
    };

    if state.find_user_auth_by_id(&user_id).await?.is_none() {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }

    if state
        .find_user_session(&user_id, &session_id)
        .await?
        .is_none()
    {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "会话不存在" })),
        )
            .into_response());
    }

    state
        .revoke_user_session(
            &user_id,
            &session_id,
            chrono::Utc::now(),
            "admin_session_revoked",
        )
        .await?;

    Ok(attach_admin_audit_response(
        Json(json!({ "message": "用户设备已强制下线" })).into_response(),
        "admin_user_session_deleted",
        "delete_user_session",
        "user_session",
        &session_id,
    ))
}

pub(super) async fn build_admin_delete_user_sessions_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_id) = admin_user_id_from_sessions_path(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response("缺少 user_id"));
    };

    if state.find_user_auth_by_id(&user_id).await?.is_none() {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "用户不存在" })),
        )
            .into_response());
    }

    let revoked_count = state
        .revoke_all_user_sessions(&user_id, chrono::Utc::now(), "admin_revoke_all_sessions")
        .await?;

    Ok(attach_admin_audit_response(
        Json(json!({
            "message": "已强制下线该用户所有设备",
            "revoked_count": revoked_count,
        }))
        .into_response(),
        "admin_user_sessions_deleted",
        "delete_user_sessions",
        "user",
        &user_id,
    ))
}
