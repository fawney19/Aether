use crate::handlers::admin::provider::shared::payloads::{
    OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_EXPIRED_PREFIX,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn oauth_invalid_reason_is_account_level_block(reason: Option<&str>) -> bool {
    let Some(reason) = reason.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    if reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX) {
        return true;
    }
    aether_admin::provider::status::resolve_account_status_snapshot(None, None, Some(reason))
        .blocked
}

pub(crate) fn build_internal_control_error_response(
    status: http::StatusCode,
    message: impl Into<String>,
) -> Response<Body> {
    (status, Json(json!({ "detail": message.into() }))).into_response()
}

pub(crate) fn normalize_provider_oauth_refresh_error_message(
    status_code: Option<u16>,
    body_excerpt: Option<&str>,
) -> String {
    let mut message = None::<String>;
    let mut error_code = None::<String>;
    let mut error_type = None::<String>;

    if let Some(body_excerpt) = body_excerpt {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body_excerpt) {
            if let Some(object) = value.as_object() {
                if let Some(error_object) =
                    object.get("error").and_then(serde_json::Value::as_object)
                {
                    message = error_object
                        .get("message")
                        .or_else(|| error_object.get("error_description"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned);
                    error_code = error_object
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                    error_type = error_object
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
                if message.is_none() {
                    message = object
                        .get("message")
                        .or_else(|| object.get("error_description"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned);
                }
                if error_code.is_none() {
                    error_code = object
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
                if error_type.is_none() {
                    error_type = object
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
            }
        }
    }

    let message = message
        .or_else(|| {
            body_excerpt
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(300).collect::<String>())
        })
        .unwrap_or_default();
    let lowered = message.to_ascii_lowercase();
    let error_code = error_code.unwrap_or_default();
    let error_type = error_type.unwrap_or_default();

    if error_code == "refresh_token_reused"
        || lowered.contains("already been used to generate a new access token")
    {
        return "refresh_token 已被使用并轮换，请重新登录授权".to_string();
    }
    if error_code == "invalid_grant"
        || error_code == "invalid_refresh_token"
        || error_code == "refresh_token_expired"
        || lowered.contains("could not validate your refresh token")
        || (lowered.contains("refresh token")
            && ["expired", "revoked", "invalid"]
                .iter()
                .any(|keyword| lowered.contains(keyword)))
    {
        return "refresh_token 无效、已过期或已撤销，请重新登录授权".to_string();
    }
    if error_type == "invalid_request_error" && !message.is_empty() {
        return message;
    }
    if !message.is_empty() {
        return message;
    }
    status_code
        .map(|status_code| format!("HTTP {status_code}"))
        .unwrap_or_else(|| "未知错误".to_string())
}

pub(crate) fn merge_provider_oauth_refresh_failure_reason(
    current_reason: Option<&str>,
    refresh_reason: &str,
) -> Option<String> {
    let current_reason = current_reason.map(str::trim).unwrap_or_default();
    let refresh_reason = refresh_reason.trim();
    if refresh_reason.is_empty() {
        return (!current_reason.is_empty()).then(|| current_reason.to_string());
    }
    if current_reason.is_empty() {
        return Some(refresh_reason.to_string());
    }
    if current_reason.starts_with(OAUTH_EXPIRED_PREFIX) {
        return None;
    }
    if oauth_invalid_reason_is_account_level_block(Some(current_reason)) {
        return None;
    }
    Some(refresh_reason.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        merge_provider_oauth_refresh_failure_reason, normalize_provider_oauth_refresh_error_message,
    };

    #[test]
    fn normalizes_openai_refresh_token_expired_response() {
        let body = r#"{"error":{"message":"Could not validate your refresh token. Please try signing in again.","type":"invalid_request_error","param":null,"code":"refresh_token_expired"}}"#;

        assert_eq!(
            normalize_provider_oauth_refresh_error_message(Some(401), Some(body)),
            "refresh_token 无效、已过期或已撤销，请重新登录授权"
        );
    }

    #[test]
    fn refresh_failure_does_not_replace_account_level_block() {
        assert_eq!(
            merge_provider_oauth_refresh_failure_reason(
                Some("[ACCOUNT_BLOCK] account has been deactivated"),
                "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 无效",
            ),
            None,
        );
        assert_eq!(
            merge_provider_oauth_refresh_failure_reason(
                Some("account_banned"),
                "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 无效",
            ),
            None,
        );
    }
}
