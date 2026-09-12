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
    let turnstile_enabled_config = state
        .read_system_config_json_value("turnstile_enabled")
        .await?;
    let turnstile_site_key_config = state
        .read_system_config_json_value("turnstile_site_key")
        .await?;
    let privacy_enabled_config = state
        .read_system_config_json_value("registration_privacy_policy_enabled")
        .await?;
    let privacy_format_config = state
        .read_system_config_json_value("registration_privacy_policy_format")
        .await?;
    let privacy_content_config = state
        .read_system_config_json_value("registration_privacy_policy_content")
        .await?;
    let privacy_version_config = state
        .read_system_config_json_value("registration_privacy_policy_version")
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
    let require_email_verification = system_config_bool(require_email_verification.as_ref(), false);
    let password_policy_level = match system_config_string(password_policy_level_config.as_ref()) {
        Some(value) if matches!(value.as_str(), "weak" | "medium" | "strong") => value,
        _ => "weak".to_string(),
    };
    let turnstile_enabled = system_config_bool(turnstile_enabled_config.as_ref(), false);
    let turnstile_site_key = system_config_string(turnstile_site_key_config.as_ref());
    let privacy_policy_enabled = system_config_bool(privacy_enabled_config.as_ref(), false);
    let privacy_policy_format = match system_config_string(privacy_format_config.as_ref()) {
        Some(value) if matches!(value.as_str(), "markdown" | "html") => value,
        _ => "markdown".to_string(),
    };
    let privacy_policy_content =
        system_config_string(privacy_content_config.as_ref()).unwrap_or_default();
    let privacy_policy_version =
        system_config_string(privacy_version_config.as_ref()).unwrap_or_else(|| "1".to_string());

    Ok(json!({
        "enable_registration": enable_registration,
        "require_email_verification": require_email_verification,
        "email_configured": email_configured,
        "password_policy_level": password_policy_level,
        "turnstile_enabled": turnstile_enabled,
        "turnstile_site_key": turnstile_site_key,
        "turnstile_required_actions": ["send_verification_code", "register"],
        "privacy_policy": {
            "enabled": privacy_policy_enabled,
            "format": privacy_policy_format,
            "content": privacy_policy_content,
            "version": privacy_policy_version,
        },
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

pub(crate) fn build_auth_json_response(
    status: http::StatusCode,
    payload: serde_json::Value,
    set_cookie: Option<String>,
) -> Response<Body> {
    let has_set_cookie = set_cookie.is_some();
    let mut response = (status, Json(payload)).into_response();
    if let Some(set_cookie) = set_cookie {
        if let Ok(value) = axum::http::HeaderValue::from_str(&set_cookie) {
            response
                .headers_mut()
                .append(axum::http::header::SET_COOKIE, value);
        }
    }
    if has_set_cookie {
        mark_sensitive_response_no_store(response)
    } else {
        response
    }
}

pub(crate) fn mark_sensitive_response_no_store(mut response: Response<Body>) -> Response<Body> {
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        axum::http::header::PRAGMA,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    response
}

pub(crate) fn build_auth_error_response(
    status: http::StatusCode,
    detail: impl Into<String>,
    clear_cookie: bool,
) -> Response<Body> {
    let detail = detail.into();
    let public_detail = if status.is_server_error() {
        tracing::error!(
            event_name = "public_api_internal_error",
            %status,
            "internal public API error hidden from client"
        );
        "服务暂不可用，请稍后重试".to_string()
    } else {
        detail
    };
    let cookie = clear_cookie.then(build_auth_refresh_cookie_clear_header);
    build_auth_json_response(status, json!({ "detail": public_detail }), cookie)
}

#[cfg(test)]
mod public_error_projection_tests {
    use super::{build_auth_error_response, build_auth_json_response};
    use axum::body::to_bytes;

    #[tokio::test]
    async fn public_server_errors_never_return_internal_details() {
        let response = build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "database failed for https://user:password@db.example?token=secret",
            false,
        );
        let body = to_bytes(response.into_body(), 4096).await.expect("body");
        let encoded = String::from_utf8_lossy(&body);
        assert!(encoded.contains("服务暂不可用"));
        for secret in ["user", "password", "token", "db.example"] {
            assert!(!encoded.contains(secret), "leaked {secret}");
        }
    }

    #[test]
    fn cookie_authenticated_responses_are_never_cacheable() {
        let response = build_auth_json_response(
            http::StatusCode::OK,
            serde_json::json!({ "access_token": "secret" }),
            Some("session=secret; HttpOnly".to_string()),
        );

        assert_eq!(
            response.headers().get(http::header::CACHE_CONTROL),
            Some(&http::HeaderValue::from_static("no-store"))
        );
        assert_eq!(
            response.headers().get(http::header::PRAGMA),
            Some(&http::HeaderValue::from_static("no-cache"))
        );
    }
}

fn auth_environment() -> Option<String> {
    std::env::var("ENVIRONMENT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn auth_jwt_secret() -> Result<String, String> {
    crate::local_auth_token::local_auth_jwt_secret()
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

pub(crate) fn auth_refresh_cookie_name() -> String {
    std::env::var("AUTH_REFRESH_COOKIE_NAME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "aether_refresh_token".to_string())
}

pub(crate) fn auth_refresh_cookie_secure() -> bool {
    auth_refresh_cookie_secure_from_values(
        std::env::var("AUTH_REFRESH_COOKIE_SECURE").ok().as_deref(),
        auth_environment().as_deref(),
    )
}

fn auth_refresh_cookie_secure_from_values(
    explicit_secure: Option<&str>,
    environment: Option<&str>,
) -> bool {
    match explicit_secure.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("true") => true,
        Some(value) if value.eq_ignore_ascii_case("false") => false,
        Some(_) => true,
        None => !environment.is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "development" | "test" | "local"
            )
        }),
    }
}

fn auth_refresh_cookie_samesite() -> &'static str {
    match std::env::var("AUTH_REFRESH_COOKIE_SAMESITE") {
        Ok(value) if value.trim().eq_ignore_ascii_case("strict") => "Strict",
        Ok(value) if value.trim().eq_ignore_ascii_case("none") => "None",
        Ok(value) if value.trim().eq_ignore_ascii_case("lax") => "Lax",
        _ if auth_environment()
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("production")) =>
        {
            "None"
        }
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

pub(crate) fn build_auth_refresh_cookie_clear_header() -> String {
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
    let mut values = headers.get_all(http::header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?.trim();
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    Some(token.to_string())
}

pub(crate) fn extract_cookie_value(headers: &http::HeaderMap, cookie_name: &str) -> Option<String> {
    let mut found = None;
    for header in headers.get_all(http::header::COOKIE).iter() {
        let header = header.to_str().ok()?;
        for pair in header.split(';') {
            let (name, value) = pair.trim().split_once('=')?;
            if name.trim() != cookie_name {
                continue;
            }
            let value = auth_non_empty_string(Some(value.to_string()))?;
            if found.replace(value).is_some() {
                return None;
            }
        }
    }
    found
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

pub(crate) fn extract_client_device_id_header(
    headers: &http::HeaderMap,
) -> Result<String, Response<Body>> {
    let candidate =
        crate::headers::header_value_str(headers, "x-client-device-id").unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::{
        auth_refresh_cookie_secure_from_values, extract_bearer_token, extract_cookie_value,
    };
    use axum::http::{header, HeaderMap, HeaderValue};

    #[test]
    fn bearer_token_requires_one_unambiguous_authorization_value() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bEaReR abc.def_ghi-jkl"),
        );
        assert_eq!(
            extract_bearer_token(&headers).as_deref(),
            Some("abc.def_ghi-jkl")
        );

        headers.append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer second-token"),
        );
        assert_eq!(extract_bearer_token(&headers), None);

        headers.clear();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer first-token, Bearer second-token"),
        );
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn cookie_value_rejects_duplicate_cookie_names() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("theme=dark; refresh=first-token"),
        );
        assert_eq!(
            extract_cookie_value(&headers, "refresh").as_deref(),
            Some("first-token")
        );

        headers.append(
            header::COOKIE,
            HeaderValue::from_static("refresh=second-token"),
        );
        assert_eq!(extract_cookie_value(&headers, "refresh"), None);

        headers.clear();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("refresh=first-token; refresh=second-token"),
        );
        assert_eq!(extract_cookie_value(&headers, "refresh"), None);
    }

    #[test]
    fn refresh_cookie_secure_fails_closed_when_environment_is_missing_or_unknown() {
        assert!(auth_refresh_cookie_secure_from_values(None, None));
        assert!(auth_refresh_cookie_secure_from_values(
            None,
            Some("staging")
        ));
        assert!(auth_refresh_cookie_secure_from_values(
            Some("invalid"),
            Some("development")
        ));
    }

    #[test]
    fn refresh_cookie_secure_allows_only_explicit_local_or_override_opt_out() {
        assert!(!auth_refresh_cookie_secure_from_values(
            None,
            Some("development")
        ));
        assert!(!auth_refresh_cookie_secure_from_values(None, Some("test")));
        assert!(!auth_refresh_cookie_secure_from_values(Some("false"), None));
        assert!(auth_refresh_cookie_secure_from_values(
            Some("true"),
            Some("development")
        ));
    }
}
