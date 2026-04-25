use super::super::super::errors::build_internal_control_error_response;
use super::helpers::RefreshSuccessContext;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

pub(super) fn control_error_response(
    status: http::StatusCode,
    message: impl Into<String>,
) -> Response<Body> {
    build_internal_control_error_response(status, message)
}

pub(super) fn oauth_refresh_failed_bad_request_response(
    error_reason: impl AsRef<str>,
) -> Response<Body> {
    control_error_response(
        http::StatusCode::BAD_REQUEST,
        format!("Token 刷新失败：{}", error_reason.as_ref()),
    )
}

pub(super) fn oauth_refresh_failed_service_unavailable_response(
    error_reason: impl Into<String>,
) -> Response<Body> {
    control_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        format!("Token 刷新失败：{}", error_reason.into()),
    )
}

pub(super) fn admin_provider_oauth_refresh_success_response(
    success: RefreshSuccessContext,
) -> Response<Body> {
    let expires_at = success
        .refreshed_expires_at_unix_secs
        .map(serde_json::Value::from)
        .or_else(|| success.refreshed_auth_config.get("expires_at").cloned())
        .unwrap_or(Value::Null);
    Json(json!({
        "provider_type": success.provider_type,
        "expires_at": expires_at,
        "has_refresh_token": success
            .refreshed_auth_config
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()),
        "email": success
            .refreshed_auth_config
            .get("email")
            .cloned()
            .unwrap_or(Value::Null),
        "account_state_recheck_attempted": success.account_state_recheck_attempted,
        "account_state_recheck_error": success.account_state_recheck_error,
    }))
    .into_response()
}
