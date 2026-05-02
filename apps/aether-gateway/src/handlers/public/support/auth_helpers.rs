use super::{
    http, json, ldap_module_config_is_valid, module_available_from_env, system_config_bool,
    system_config_string, AppState, Body, GatewayError, GatewayPublicRequestContext, IntoResponse,
    Json, Response,
};

pub(crate) async fn build_auth_registration_settings_payload(
    state: &AppState,
) -> Result<serde_json::Value, GatewayError> {
    let enable_registration = state
        .read_system_config_json_value("enable_registration")
        .await?;
    let require_email_verification = state
        .read_system_config_json_value("require_email_verification")
        .await?;
    let smtp_host = state.read_system_config_json_value("smtp_host").await?;
    let smtp_from_email = state
        .read_system_config_json_value("smtp_from_email")
        .await?;
    let password_policy_level_config = state
        .read_system_config_json_value("password_policy_level")
        .await?;

    let email_configured = smtp_host
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        && smtp_from_email
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some();
    let enable_registration = system_config_bool(enable_registration.as_ref(), false);
    let require_email_verification =
        system_config_bool(require_email_verification.as_ref(), false) && email_configured;
    let password_policy_level = match system_config_string(password_policy_level_config.as_ref()) {
        Some(value) if matches!(value.as_str(), "weak" | "medium" | "strong") => value,
        _ => "weak".to_string(),
    };

    Ok(json!({
        "enable_registration": enable_registration,
        "require_email_verification": require_email_verification,
        "email_configured": email_configured,
        "password_policy_level": password_policy_level,
    }))
}

pub(crate) async fn build_auth_settings_payload(
    state: &AppState,
) -> Result<serde_json::Value, GatewayError> {
    let ldap_enabled_config = state
        .read_system_config_json_value("module.ldap.enabled")
        .await?;
    let ldap_config = state.get_ldap_module_config().await?;
    let ldap_enabled = module_available_from_env("LDAP_AVAILABLE", true)
        && system_config_bool(ldap_enabled_config.as_ref(), false)
        && ldap_config_is_enabled(ldap_config.as_ref());
    let ldap_exclusive = ldap_enabled
        && ldap_config
            .as_ref()
            .map(|config| config.is_exclusive)
            .unwrap_or(false);

    Ok(json!({
        "local_enabled": !ldap_exclusive,
        "ldap_enabled": ldap_enabled,
        "ldap_exclusive": ldap_exclusive,
    }))
}

pub(super) fn ldap_config_is_enabled(
    config: Option<&aether_data::repository::auth_modules::StoredLdapModuleConfig>,
) -> bool {
    config.is_some_and(|config| config.is_enabled) && ldap_module_config_is_valid(config)
}

const AUTH_ACCESS_TOKEN_DEFAULT_EXPIRATION_HOURS: i64 = 24;
pub(super) const AUTH_REFRESH_TOKEN_EXPIRATION_DAYS: i64 = 7;
pub(super) const AUTH_EMAIL_VERIFICATION_PREFIX: &str = "email:verification:";
pub(super) const AUTH_EMAIL_VERIFIED_PREFIX: &str = "email:verified:";
pub(super) const AUTH_EMAIL_VERIFIED_TTL_SECS: u64 = 3600;
pub(super) const AUTH_SMTP_TIMEOUT_SECS: u64 = 30;

pub(crate) fn build_auth_json_response(
    status: http::StatusCode,
    payload: serde_json::Value,
    set_cookie: Option<String>,
) -> Response<Body> {
    let mut response = (status, Json(payload)).into_response();
    if let Some(set_cookie) = set_cookie {
        if let Ok(value) = axum::http::HeaderValue::from_str(&set_cookie) {
            response
                .headers_mut()
                .append(axum::http::header::SET_COOKIE, value);
        }
    }
    response
}

pub(crate) fn build_auth_error_response(
    status: http::StatusCode,
    detail: impl Into<String>,
    clear_cookie: bool,
) -> Response<Body> {
    let cookie = clear_cookie.then(build_auth_refresh_cookie_clear_header);
    build_auth_json_response(status, json!({ "detail": detail.into() }), cookie)
}

fn auth_environment() -> String {
    std::env::var("ENVIRONMENT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "development".to_string())
}

pub(super) fn auth_jwt_secret() -> Result<String, String> {
    if let Ok(value) = std::env::var("JWT_SECRET_KEY") {
        let value = value.trim();
        if !value.is_empty() {
            return Ok(value.to_string());
        }
    }
    if auth_environment().eq_ignore_ascii_case("production") {
        return Err("JWT_SECRET_KEY 未配置".to_string());
    }
    Ok("aether-rust-dev-jwt-secret".to_string())
}

pub(super) fn auth_access_token_expiry_hours() -> i64 {
    std::env::var("JWT_EXPIRATION_HOURS")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(AUTH_ACCESS_TOKEN_DEFAULT_EXPIRATION_HOURS)
}

pub(super) fn auth_verification_code_expire_minutes() -> i64 {
    std::env::var("VERIFICATION_CODE_EXPIRE_MINUTES")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5)
}

pub(super) fn auth_verification_send_cooldown_seconds() -> i64 {
    std::env::var("VERIFICATION_SEND_COOLDOWN")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(60)
}

pub(super) fn auth_refresh_cookie_name() -> String {
    std::env::var("AUTH_REFRESH_COOKIE_NAME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "aether_refresh_token".to_string())
}

fn auth_refresh_cookie_secure() -> bool {
    std::env::var("AUTH_REFRESH_COOKIE_SECURE")
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or_else(|| auth_environment().eq_ignore_ascii_case("production"))
}

fn auth_refresh_cookie_samesite() -> &'static str {
    match std::env::var("AUTH_REFRESH_COOKIE_SAMESITE") {
        Ok(value) if value.trim().eq_ignore_ascii_case("strict") => "Strict",
        Ok(value) if value.trim().eq_ignore_ascii_case("none") => "None",
        Ok(value) if value.trim().eq_ignore_ascii_case("lax") => "Lax",
        _ if auth_environment().eq_ignore_ascii_case("production") => "None",
        _ => "Lax",
    }
}

pub(super) fn build_auth_refresh_cookie_header(refresh_token: &str) -> String {
    let mut cookie = format!(
        "{}={}; Path=/api/auth; HttpOnly; SameSite={}; Max-Age={}",
        auth_refresh_cookie_name(),
        refresh_token,
        auth_refresh_cookie_samesite(),
        AUTH_REFRESH_TOKEN_EXPIRATION_DAYS * 24 * 60 * 60,
    );
    if auth_refresh_cookie_secure() {
        cookie.push_str("; Secure");
    }
    cookie
}

pub(super) fn build_auth_refresh_cookie_clear_header() -> String {
    let mut cookie = format!(
        "{}=; Path=/api/auth; HttpOnly; SameSite={}; Max-Age=0",
        auth_refresh_cookie_name(),
        auth_refresh_cookie_samesite(),
    );
    if auth_refresh_cookie_secure() {
        cookie.push_str("; Secure");
    }
    cookie
}

pub(super) fn auth_now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn auth_non_empty_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn extract_bearer_token(headers: &http::HeaderMap) -> Option<String> {
    let value = crate::headers::header_value_str(headers, http::header::AUTHORIZATION.as_str())?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    auth_non_empty_string(Some(token.to_string()))
}

pub(super) fn extract_cookie_value(headers: &http::HeaderMap, cookie_name: &str) -> Option<String> {
    let header = crate::headers::header_value_str(headers, http::header::COOKIE.as_str())?;
    for pair in header.split(';') {
        let (name, value) = pair.trim().split_once('=')?;
        if name.trim() == cookie_name {
            return auth_non_empty_string(Some(value.to_string()));
        }
    }
    None
}

pub(crate) fn extract_client_device_id(
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Result<String, Response<Body>> {
    let header_value = crate::headers::header_value_str(headers, "x-client-device-id");
    let query_value = request_context
        .request_query_string
        .as_deref()
        .and_then(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "client_device_id")
                .map(|(_, value)| value.into_owned())
        });
    let candidate = header_value.or(query_value).unwrap_or_default();
    let candidate = candidate.trim();
    if candidate.is_empty()
        || candidate.len() > 128
        || !candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "缺少或无效的设备标识",
            false,
        ));
    }
    Ok(candidate.to_string())
}

pub(super) fn auth_user_agent(headers: &http::HeaderMap) -> Option<String> {
    crate::headers::header_value_str(headers, http::header::USER_AGENT.as_str())
        .map(|value| value.chars().take(1000).collect())
}

pub(super) fn auth_client_ip(headers: &http::HeaderMap) -> Option<String> {
    crate::headers::header_value_str(headers, "x-forwarded-for")
        .and_then(|value| {
            value
                .split(',')
                .next()
                .map(|segment| segment.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(45).collect())
        .or_else(|| {
            crate::headers::header_value_str(headers, "x-real-ip")
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(45).collect())
        })
}

pub(super) fn normalize_auth_login_identifier(value: &str) -> String {
    let normalized = value.trim();
    if normalized.contains('@') {
        normalized.to_ascii_lowercase()
    } else {
        normalized.to_string()
    }
}

pub(super) fn validate_auth_login_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("密码不能为空".to_string());
    }
    if password.len() > 72 || password.as_bytes().len() > 72 {
        return Err("密码长度不能超过72字节".to_string());
    }
    Ok(())
}
