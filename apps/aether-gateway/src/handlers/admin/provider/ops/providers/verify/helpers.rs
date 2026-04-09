use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::verify as admin_provider_verify_pure;
use regex::Regex;
use serde_json::{json, Map, Value};

pub(super) fn admin_provider_ops_normalized_verify_architecture_id(architecture_id: &str) -> &str {
    admin_provider_verify_pure::admin_provider_ops_normalized_verify_architecture_id(
        architecture_id,
    )
}

pub(super) fn admin_provider_ops_extract_cookie_value(cookie_input: &str, key: &str) -> String {
    admin_provider_verify_pure::admin_provider_ops_extract_cookie_value(cookie_input, key)
}

pub(super) fn admin_provider_ops_yescode_cookie_header(cookie_input: &str) -> String {
    admin_provider_verify_pure::admin_provider_ops_yescode_cookie_header(cookie_input)
}

pub(super) fn admin_provider_ops_anyrouter_compute_acw_sc_v2(arg1: &str) -> Option<String> {
    admin_provider_verify_pure::admin_provider_ops_anyrouter_compute_acw_sc_v2(arg1)
}

pub(super) fn admin_provider_ops_anyrouter_parse_session_user_id(
    cookie_input: &str,
) -> Option<String> {
    admin_provider_verify_pure::admin_provider_ops_anyrouter_parse_session_user_id(cookie_input)
}

pub(super) async fn admin_provider_ops_anyrouter_acw_cookie(
    state: &AdminAppState<'_>,
    base_url: &str,
) -> Option<String> {
    let response = state
        .http_client()
        .get(base_url.trim_end_matches('/'))
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .send()
        .await
        .ok()?;
    let body = response.text().await.ok()?;
    let compiled = Regex::new(r"var\s+arg1\s*=\s*'([0-9a-fA-F]{40})'").ok()?;
    let captures = compiled.captures(&body)?;
    let arg1 = captures.get(1)?.as_str();
    admin_provider_ops_anyrouter_compute_acw_sc_v2(arg1).map(|value| format!("acw_sc__v2={value}"))
}

pub(super) fn admin_provider_ops_verify_failure(message: impl Into<String>) -> Value {
    admin_provider_verify_pure::admin_provider_ops_verify_failure(message)
}

pub(super) fn admin_provider_ops_verify_success(
    data: Value,
    updated_credentials: Option<Map<String, Value>>,
) -> Value {
    admin_provider_verify_pure::admin_provider_ops_verify_success(data, updated_credentials)
}

pub(super) fn admin_provider_ops_verify_user_payload(
    username: Option<String>,
    display_name: Option<String>,
    email: Option<String>,
    quota: Option<f64>,
    extra: Option<Map<String, Value>>,
) -> Value {
    admin_provider_verify_pure::admin_provider_ops_verify_user_payload(
        username,
        display_name,
        email,
        quota,
        extra,
    )
}

pub(super) fn admin_provider_ops_value_as_f64(value: Option<&Value>) -> Option<f64> {
    admin_provider_verify_pure::admin_provider_ops_value_as_f64(value)
}

pub(super) fn admin_provider_ops_json_object(
    value: &Value,
) -> Option<&serde_json::Map<String, Value>> {
    admin_provider_verify_pure::admin_provider_ops_json_object(value)
}

pub(super) fn admin_provider_ops_frontend_updated_credentials(
    credentials: Map<String, Value>,
) -> Option<Map<String, Value>> {
    admin_provider_verify_pure::admin_provider_ops_frontend_updated_credentials(credentials)
}

pub(super) async fn admin_provider_ops_sub2api_exchange_token(
    state: &AdminAppState<'_>,
    base_url: &str,
    credentials: &Map<String, Value>,
) -> Result<(String, Option<Map<String, Value>>), String> {
    let email = credentials
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let password = credentials
        .get("password")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let refresh_token = credentials
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();

    let (path, body, default_error, previous_refresh_token) =
        if !email.is_empty() && !password.is_empty() {
            (
                "/api/v1/auth/login",
                json!({ "email": email, "password": password }),
                "登录失败",
                None,
            )
        } else if !refresh_token.is_empty() {
            (
                "/api/v1/auth/refresh",
                json!({ "refresh_token": refresh_token }),
                "Refresh Token 无效或已过期",
                Some(refresh_token),
            )
        } else {
            return Err("请填写账号密码或 Refresh Token".to_string());
        };

    let response = match state
        .http_client()
        .post(format!("{}{path}", base_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) if err.is_timeout() => return Err("连接超时".to_string()),
        Err(err) if err.is_connect() => return Err(format!("连接失败: {err}")),
        Err(err) => return Err(format!("验证失败: {err}")),
    };

    let status = response.status();
    let response_json = match response.bytes().await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    };
    let payload = response_json.as_object().cloned().unwrap_or_default();
    if status != http::StatusCode::OK
        || payload.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0
    {
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(default_error);
        return Err(message.to_string());
    }

    let Some(token_data) = payload.get("data").and_then(Value::as_object) else {
        return Err("响应格式无效".to_string());
    };
    let access_token = token_data
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "响应格式无效".to_string())?;

    let mut updated_credentials = Map::new();
    if let Some(new_refresh_token) = token_data
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if previous_refresh_token != Some(new_refresh_token) {
            updated_credentials.insert(
                "refresh_token".to_string(),
                Value::String(new_refresh_token.to_string()),
            );
        }
    }

    Ok((
        access_token.to_string(),
        admin_provider_ops_frontend_updated_credentials(updated_credentials),
    ))
}
