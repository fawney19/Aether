use std::collections::BTreeSet;

use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::{
    build_auth_error_response, resolve_authenticated_local_user, AppState,
    GatewayPublicRequestContext, PUBLIC_CAPABILITY_DEFINITIONS,
};
use crate::GatewayUserPreferenceView;

const USERS_ME_PREFERENCES_STORAGE_UNAVAILABLE_DETAIL: &str = "用户偏好设置存储暂不可用";
const USERS_ME_MODEL_CAPABILITIES_STORAGE_UNAVAILABLE_DETAIL: &str = "用户模型能力配置存储暂不可用";

pub(super) fn user_configurable_capability_names() -> BTreeSet<&'static str> {
    PUBLIC_CAPABILITY_DEFINITIONS
        .iter()
        .filter(|capability| capability.config_mode == "user_configurable")
        .map(|capability| capability.name)
        .collect()
}

pub(super) fn known_capability_names() -> BTreeSet<&'static str> {
    PUBLIC_CAPABILITY_DEFINITIONS
        .iter()
        .map(|capability| capability.name)
        .collect()
}

pub(super) fn normalize_user_model_capability_settings_input(
    value: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    match value {
        Some(serde_json::Value::Null) | None => None,
        Some(value) => Some(value),
    }
}

fn validate_user_model_capability_settings(
    value: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = normalize_user_model_capability_settings_input(value) else {
        return Ok(None);
    };
    let Some(settings) = value.as_object() else {
        return Err("model_capability_settings 必须是对象类型".to_string());
    };

    let user_configurable = user_configurable_capability_names();
    let known_capabilities = known_capability_names();
    for (model_name, capabilities) in settings {
        let Some(capabilities) = capabilities.as_object() else {
            return Err(format!("模型 {model_name} 的能力配置必须是对象类型"));
        };
        for (capability_name, capability_value) in capabilities {
            if !known_capabilities.contains(capability_name.as_str()) {
                return Err(format!("未知的能力类型: {capability_name}"));
            }
            if !user_configurable.contains(capability_name.as_str()) {
                return Err(format!("能力 {capability_name} 不支持用户配置"));
            }
            if !capability_value.is_boolean() {
                return Err(format!("能力 {capability_name} 的值必须是布尔类型"));
            }
        }
    }

    Ok(Some(value))
}

fn build_users_me_preferences_payload(
    preferences: &GatewayUserPreferenceView,
    expose_provider_details: bool,
) -> serde_json::Value {
    let mut payload = json!({
        "avatar_url": preferences.avatar_url,
        "bio": preferences.bio,
        "default_provider_id": preferences.default_provider_id,
        "theme": preferences.theme,
        "language": preferences.language,
        "timezone": preferences.timezone,
        "allow_wallet_overage": preferences.allow_wallet_overage,
        "notifications": {
            "email": preferences.email_notifications,
            "usage_alerts": preferences.usage_alerts,
            "announcements": preferences.announcement_notifications,
        },
    });
    if expose_provider_details {
        payload["default_provider"] = json!(preferences.default_provider_name);
    }
    payload
}

fn parse_users_me_optional_string_field(
    payload: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match payload.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        _ => Err("输入验证失败".to_string()),
    }
}

fn parse_users_me_optional_bool_field(
    payload: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<bool>, String> {
    match payload.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(value)) => Ok(Some(*value)),
        _ => Err("输入验证失败".to_string()),
    }
}

fn parse_users_me_optional_provider_id_field(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<String>, String> {
    match payload.get("default_provider_id") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err("输入验证失败".to_string())
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(serde_json::Value::Number(value)) => Ok(Some(value.to_string())),
        _ => Err("输入验证失败".to_string()),
    }
}

fn validate_users_me_preference_theme(theme: &str) -> Result<(), String> {
    if matches!(theme, "light" | "dark" | "auto" | "system") {
        Ok(())
    } else {
        Err("Invalid theme. Must be 'light', 'dark', 'auto', or 'system'".to_string())
    }
}

pub(super) async fn handle_users_me_model_capabilities_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let settings = match state
        .read_user_model_capability_settings(&auth.user.id)
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => serde_json::Value::Object(serde_json::Map::new()),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user model capability lookup failed: {err:?}"),
                false,
            )
        }
    };
    Json(json!({ "model_capability_settings": settings })).into_response()
}

pub(super) async fn handle_users_me_preferences_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };

    let preferences = match state.read_user_preferences(&auth.user.id).await {
        Ok(Some(value)) => value,
        Ok(None) => GatewayUserPreferenceView::default_for_user(&auth.user.id),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user preference lookup failed: {err:?}"),
                false,
            )
        }
    };

    Json(build_users_me_preferences_payload(
        &preferences,
        auth.user.role.eq_ignore_ascii_case("admin"),
    ))
    .into_response()
}

pub(super) async fn handle_users_me_preferences_put(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, "输入验证失败", false)
        }
    };
    let Some(payload) = payload.as_object() else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "输入验证失败", false);
    };

    let mut preferences = match state.read_user_preferences(&auth.user.id).await {
        Ok(Some(value)) => value,
        Ok(None) => GatewayUserPreferenceView::default_for_user(&auth.user.id),
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user preference lookup failed: {err:?}"),
                false,
            )
        }
    };

    let avatar_url = match parse_users_me_optional_string_field(payload, "avatar_url") {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if let Some(avatar_url) = avatar_url {
        preferences.avatar_url = Some(avatar_url);
    }

    let bio = match parse_users_me_optional_string_field(payload, "bio") {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if let Some(bio) = bio {
        preferences.bio = Some(bio);
    }

    let default_provider_id = match parse_users_me_optional_provider_id_field(payload) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if let Some(default_provider_id) = default_provider_id {
        let provider_name = match state.find_active_provider_name(&default_provider_id).await {
            Ok(Some(value)) => value,
            Ok(None) => {
                return build_auth_error_response(
                    http::StatusCode::NOT_FOUND,
                    "Provider not found or inactive",
                    false,
                )
            }
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("provider preference lookup failed: {err:?}"),
                    false,
                )
            }
        };
        preferences.default_provider_id = Some(default_provider_id);
        preferences.default_provider_name = Some(provider_name);
    }

    let theme = match parse_users_me_optional_string_field(payload, "theme") {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if let Some(theme) = theme {
        if let Err(detail) = validate_users_me_preference_theme(&theme) {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
        }
        preferences.theme = theme;
    }

    let language = match parse_users_me_optional_string_field(payload, "language") {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if let Some(language) = language {
        preferences.language = language;
    }

    let timezone = match parse_users_me_optional_string_field(payload, "timezone") {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if let Some(timezone) = timezone {
        preferences.timezone = timezone;
    }

    let email_notifications =
        match parse_users_me_optional_bool_field(payload, "email_notifications") {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
    if let Some(email_notifications) = email_notifications {
        preferences.email_notifications = email_notifications;
    }

    let usage_alerts = match parse_users_me_optional_bool_field(payload, "usage_alerts") {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    if let Some(usage_alerts) = usage_alerts {
        preferences.usage_alerts = usage_alerts;
    }

    let announcement_notifications =
        match parse_users_me_optional_bool_field(payload, "announcement_notifications") {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
    if let Some(announcement_notifications) = announcement_notifications {
        preferences.announcement_notifications = announcement_notifications;
    }

    let allow_wallet_overage =
        match parse_users_me_optional_bool_field(payload, "allow_wallet_overage") {
            Ok(value) => value,
            Err(detail) => {
                return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
        };
    if let Some(allow_wallet_overage) = allow_wallet_overage {
        preferences.allow_wallet_overage = allow_wallet_overage;
    }

    match state.write_user_preferences(&preferences).await {
        Ok(Some(_)) => Json(json!({ "message": "偏好设置更新成功" })).into_response(),
        Ok(None) => build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            USERS_ME_PREFERENCES_STORAGE_UNAVAILABLE_DETAIL,
            false,
        ),
        Err(err) => build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("user preference update failed: {err:?}"),
            false,
        ),
    }
}

pub(super) async fn handle_users_me_model_capabilities_put(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, "输入验证失败", false)
        }
    };
    let Some(payload) = payload.as_object() else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "输入验证失败", false);
    };
    let settings = match validate_user_model_capability_settings(
        payload.get("model_capability_settings").cloned(),
    ) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let persisted = match state
        .update_user_model_capability_settings(&auth.user.id, settings)
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                USERS_ME_MODEL_CAPABILITIES_STORAGE_UNAVAILABLE_DETAIL,
                false,
            )
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user model capability update failed: {err:?}"),
                false,
            )
        }
    };

    Json(json!({
        "message": "模型能力配置已更新",
        "model_capability_settings": persisted,
    }))
    .into_response()
}
