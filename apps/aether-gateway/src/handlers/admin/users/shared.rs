use super::ADMIN_USERS_DATA_UNAVAILABLE_DETAIL;
use crate::handlers::admin::shared::AdminTypedObjectPatch;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use regex::Regex;
use serde_json::json;

#[derive(Debug, serde::Deserialize)]
pub(super) struct AdminCreateUserApiKeyRequest {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) allowed_providers: Option<Vec<String>>,
    #[serde(default)]
    pub(super) allowed_api_formats: Option<Vec<String>>,
    #[serde(default)]
    pub(super) allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub(super) rate_limit: Option<i32>,
    #[serde(default)]
    pub(super) concurrent_limit: Option<i32>,
    #[serde(default)]
    pub(super) expire_days: Option<i32>,
    #[serde(default)]
    pub(super) expires_at: Option<String>,
    #[serde(default)]
    pub(super) initial_balance_usd: Option<f64>,
    #[serde(default)]
    pub(super) unlimited_balance: Option<bool>,
    #[serde(default)]
    pub(super) is_standalone: Option<bool>,
    #[serde(default)]
    pub(super) auto_delete_on_expiry: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct AdminUpdateUserApiKeyRequest {
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) rate_limit: Option<i32>,
    #[serde(default)]
    pub(super) concurrent_limit: Option<i32>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct AdminToggleUserApiKeyLockRequest {
    #[serde(default)]
    pub(super) locked: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct AdminCreateUserRequest {
    pub(super) username: String,
    pub(super) password: String,
    #[serde(default)]
    pub(super) email: Option<String>,
    #[serde(default)]
    pub(super) role: Option<String>,
    #[serde(default)]
    pub(super) initial_gift_usd: Option<f64>,
    #[serde(default)]
    pub(super) unlimited: bool,
    #[serde(default)]
    pub(super) allowed_providers: Option<Vec<String>>,
    #[serde(default)]
    pub(super) allowed_api_formats: Option<Vec<String>>,
    #[serde(default)]
    pub(super) allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub(super) rate_limit: Option<i32>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct AdminUpdateUserRequest {
    #[serde(default)]
    pub(super) email: Option<String>,
    #[serde(default)]
    pub(super) username: Option<String>,
    #[serde(default)]
    pub(super) password: Option<String>,
    #[serde(default)]
    pub(super) role: Option<String>,
    #[serde(default)]
    pub(super) unlimited: Option<bool>,
    #[serde(default)]
    pub(super) allowed_providers: Option<Vec<String>>,
    #[serde(default)]
    pub(super) allowed_api_formats: Option<Vec<String>>,
    #[serde(default)]
    pub(super) allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub(super) rate_limit: Option<i32>,
    #[serde(default)]
    pub(super) is_active: Option<bool>,
}

pub(super) type AdminUpdateUserPatch = AdminTypedObjectPatch<AdminUpdateUserRequest>;

pub(super) fn build_admin_users_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_USERS_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub(super) fn build_admin_users_read_only_response(detail: &'static str) -> Response<Body> {
    (
        http::StatusCode::CONFLICT,
        Json(json!({
            "detail": detail,
            "error_code": "read_only_mode",
        })),
    )
        .into_response()
}

pub(super) fn build_admin_users_bad_request_response(detail: &'static str) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

pub(super) fn normalize_admin_optional_user_email(
    value: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let normalized = value.to_ascii_lowercase();
    let pattern = Regex::new(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$")
        .expect("email regex should compile");
    if !pattern.is_match(&normalized) {
        return Err("邮箱格式无效".to_string());
    }
    Ok(Some(normalized))
}

pub(super) fn normalize_admin_username(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("用户名不能为空".to_string());
    }
    if value.len() < 3 {
        return Err("用户名长度至少为3个字符".to_string());
    }
    if value.len() > 30 {
        return Err("用户名长度不能超过30个字符".to_string());
    }
    let pattern = Regex::new(r"^[a-zA-Z0-9_.-]+$").expect("username regex should compile");
    if !pattern.is_match(value) {
        return Err("用户名只能包含字母、数字、下划线、连字符和点号".to_string());
    }
    Ok(value.to_string())
}

pub(super) fn validate_admin_user_password(password: &str, policy: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("密码不能为空".to_string());
    }
    if password.as_bytes().len() > 72 {
        return Err("密码长度不能超过72字节".to_string());
    }
    let min_len = if matches!(policy, "medium" | "strong") {
        8
    } else {
        6
    };
    if password.chars().count() < min_len {
        return Err(format!("密码长度至少为{min_len}个字符"));
    }
    if policy == "medium" {
        if !password.chars().any(|ch| ch.is_ascii_alphabetic()) {
            return Err("密码必须包含至少一个字母".to_string());
        }
        if !password.chars().any(|ch| ch.is_ascii_digit()) {
            return Err("密码必须包含至少一个数字".to_string());
        }
    } else if policy == "strong" {
        if !password.chars().any(|ch| ch.is_ascii_uppercase()) {
            return Err("密码必须包含至少一个大写字母".to_string());
        }
        if !password.chars().any(|ch| ch.is_ascii_lowercase()) {
            return Err("密码必须包含至少一个小写字母".to_string());
        }
        if !password.chars().any(|ch| ch.is_ascii_digit()) {
            return Err("密码必须包含至少一个数字".to_string());
        }
        if !password.chars().any(|ch| !ch.is_ascii_alphanumeric()) {
            return Err("密码必须包含至少一个特殊字符".to_string());
        }
    }
    Ok(())
}

pub(super) fn normalize_admin_user_role(value: Option<&str>) -> Result<String, String> {
    match value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("user")
        .to_ascii_lowercase()
        .as_str()
    {
        "user" => Ok("user".to_string()),
        "admin" => Ok("admin".to_string()),
        _ => Err("角色参数不合法".to_string()),
    }
}

pub(crate) fn normalize_admin_user_string_list(
    value: Option<Vec<String>>,
    field_name: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(values) = value else {
        return Ok(None);
    };
    let mut normalized = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for item in values {
        let item = item.trim();
        if item.is_empty() {
            return Err(format!("{field_name} 不能为空"));
        }
        if seen.insert(item.to_string()) {
            normalized.push(item.to_string());
        }
    }
    Ok(Some(normalized))
}

pub(crate) fn normalize_admin_user_api_formats(
    value: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, String> {
    let Some(values) = value else {
        return Ok(None);
    };
    let mut normalized = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let pattern =
        Regex::new(r"^[A-Za-z0-9_.-]+:[A-Za-z0-9_.-]+$").expect("api format regex should compile");
    for item in values {
        let item = item.trim();
        if item.is_empty() {
            return Err("allowed_api_formats 不能为空".to_string());
        }
        if !pattern.is_match(item) {
            return Err(format!("allowed_api_formats 格式无效: {item}"));
        }
        let normalized_item = item.to_ascii_lowercase();
        if seen.insert(normalized_item.clone()) {
            normalized.push(normalized_item);
        }
    }
    Ok(Some(normalized))
}

pub(super) fn admin_default_user_initial_gift(value: Option<&serde_json::Value>) -> f64 {
    match value {
        Some(serde_json::Value::Number(number)) => number.as_f64().unwrap_or(10.0),
        Some(serde_json::Value::String(value)) => value.parse::<f64>().unwrap_or(10.0),
        _ => 10.0,
    }
}

pub(super) fn format_optional_datetime_iso8601(
    value: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<String> {
    value.map(|value| value.to_rfc3339())
}
