pub(super) use super::{
    build_unhandled_public_support_response, decrypt_catalog_secret_with_fallbacks,
    decrypt_or_migrate_ldap_bind_password, decrypt_or_migrate_system_config_secret,
    escape_admin_email_template_html, ldap_module_config_is_valid, module_available_from_env,
    read_admin_email_template_payload, render_admin_email_template_html, system_config_bool,
    system_config_string, AppState, GatewayError, GatewayPublicRequestContext,
};
pub(super) use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
pub(super) use regex::Regex;
use serde::Deserialize;
pub(super) use serde_json::json;

const AUTH_DUMMY_PASSWORD_HASH: &str =
    "$2y$10$.OBQfixAECpsb8V/VS3csOMf00x2E/jD/gnud20t6RG0yiQosyOZ2";

#[path = "auth_helpers.rs"]
mod auth_helpers;
pub(crate) use auth_helpers::*;

#[path = "auth_rate_limit.rs"]
mod auth_rate_limit;
use auth_rate_limit::*;

#[path = "auth_turnstile.rs"]
mod auth_turnstile;
use auth_turnstile::*;

#[path = "auth_email.rs"]
mod auth_email;
use auth_email::*;
pub(in crate::handlers::public::support) use auth_email::{
    auth_email_is_verified, consume_auth_email_registration_proof,
};

#[path = "auth_ldap.rs"]
mod auth_ldap;
use auth_ldap::*;

pub(super) async fn local_password_login_allowed_for_user(
    state: &AppState,
    user: Option<&aether_data::repository::users::StoredUserAuthRecord>,
) -> Result<bool, GatewayError> {
    auth_ldap::auth_local_login_allowed_for_user(state, user).await
}

#[path = "auth_session.rs"]
pub(super) mod auth_session;
use auth_session::*;

#[path = "auth_registration.rs"]
pub(super) mod auth_registration;
use auth_registration::*;

#[derive(Deserialize)]
struct AuthLoginRequest {
    email: String,
    password: String,
    #[serde(default = "default_auth_login_type")]
    auth_type: String,
}

fn default_auth_login_type() -> String {
    "local".to_string()
}

fn system_config_f64(value: Option<&serde_json::Value>, default: f64) -> f64 {
    match value {
        Some(serde_json::Value::Number(value)) => value.as_f64().unwrap_or(default),
        Some(serde_json::Value::String(value)) => value.trim().parse::<f64>().unwrap_or(default),
        _ => default,
    }
}

fn system_config_u16(value: Option<&serde_json::Value>, default: u16) -> u16 {
    match value {
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(default),
        Some(serde_json::Value::String(value)) => value.trim().parse::<u16>().unwrap_or(default),
        _ => default,
    }
}

fn system_config_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    match value {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
            .collect(),
        Some(serde_json::Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn build_auth_internal_error_response(
    event_name: &'static str,
    _error: impl std::fmt::Debug,
    clear_cookie: bool,
) -> Response<Body> {
    tracing::error!(event_name = event_name, "authentication operation failed");
    build_auth_error_response(
        http::StatusCode::INTERNAL_SERVER_ERROR,
        "认证服务暂不可用，请稍后重试",
        clear_cookie,
    )
}

async fn verify_auth_password(
    password: &str,
    password_hash: &str,
) -> Result<bool, tokio::task::JoinError> {
    let password = password.to_string();
    let password_hash = password_hash.to_string();
    tokio::task::spawn_blocking(move || bcrypt::verify(password, &password_hash).unwrap_or(false))
        .await
}

async fn handle_auth_login(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    if let Err(response) = enforce_auth_ip_rate_limit(state, AUTH_LOGIN_RATE_LIMIT, client_ip).await
    {
        return response;
    }
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少登录请求体", false);
    };
    let payload = match serde_json::from_slice::<AuthLoginRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "无效的登录请求",
                false,
            )
        }
    };
    let identifier = normalize_auth_login_identifier(&payload.email);
    if identifier.is_empty() {
        return build_auth_error_response(
            http::StatusCode::BAD_REQUEST,
            "邮箱或用户名不能为空",
            false,
        );
    }
    if let Err(response) =
        enforce_auth_identity_rate_limit(state, AUTH_LOGIN_RATE_LIMIT, &identifier).await
    {
        return response;
    }
    if let Err(detail) = validate_auth_login_password(&payload.password) {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false);
    }
    // Login creates a cookie-backed session. Requiring a custom header prevents
    // cross-site forms from logging a victim's browser into an attacker's account.
    let client_device_id = match extract_client_device_id_header(headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let auth_type = payload.auth_type.trim().to_ascii_lowercase();
    let (user, expected_password_hash) = match auth_type.as_str() {
        "local" => {
            let user = match state.find_user_auth_by_identifier(&identifier).await {
                Ok(user) => user,
                Err(err) => {
                    return build_auth_internal_error_response(
                        "auth_login_user_lookup_failed",
                        err,
                        false,
                    )
                }
            };
            let basic_user_is_eligible = user.as_ref().is_some_and(|user| {
                !user.is_deleted
                    && user.is_active
                    && user.auth_source.eq_ignore_ascii_case("local")
                    && user
                        .password_hash
                        .as_deref()
                        .is_some_and(|hash| !hash.is_empty())
            });
            let local_login_is_allowed =
                match auth_local_login_allowed_for_user(state, user.as_ref()).await {
                    Ok(allowed) => allowed,
                    Err(err) => {
                        return build_auth_internal_error_response(
                            "auth_login_settings_lookup_failed",
                            err,
                            false,
                        )
                    }
                };
            let user_is_eligible = basic_user_is_eligible && local_login_is_allowed;
            let password_hash = user
                .as_ref()
                .filter(|_| user_is_eligible)
                .and_then(|user| user.password_hash.as_deref())
                .unwrap_or(AUTH_DUMMY_PASSWORD_HASH)
                .to_string();
            let password_matches =
                match verify_auth_password(&payload.password, &password_hash).await {
                    Ok(matches) => matches,
                    Err(err) => {
                        return build_auth_internal_error_response(
                            "auth_login_password_worker_failed",
                            err,
                            false,
                        )
                    }
                };
            if !user_is_eligible || !password_matches {
                return build_auth_error_response(
                    http::StatusCode::UNAUTHORIZED,
                    "邮箱或密码错误",
                    false,
                );
            }
            (
                user.expect("eligible authenticated local user should exist"),
                Some(password_hash),
            )
        }
        "ldap" => {
            let ldap_user = match authenticate_auth_ldap_user(state, &identifier, &payload.password)
                .await
            {
                Ok(Some(user)) => user,
                Ok(None) => {
                    return build_auth_error_response(
                        http::StatusCode::UNAUTHORIZED,
                        "邮箱或密码错误",
                        false,
                    )
                }
                Err(err) => {
                    return build_auth_internal_error_response("auth_ldap_login_failed", err, false)
                }
            };
            let _ = &ldap_user.display_name;
            let initial_gift = match state
                .read_system_config_json_value("default_user_initial_gift_usd")
                .await
            {
                Ok(value) => system_config_f64(value.as_ref(), 10.0),
                Err(err) => {
                    return build_auth_internal_error_response(
                        "auth_ldap_settings_lookup_failed",
                        err,
                        false,
                    )
                }
            };
            let user = match state
                .get_or_create_ldap_auth_user(
                    ldap_user.email,
                    ldap_user.username,
                    Some(ldap_user.ldap_dn),
                    Some(ldap_user.ldap_username),
                    auth_now(),
                    initial_gift,
                    false,
                )
                .await
            {
                Ok(Some(user)) => user,
                Ok(None) => {
                    return build_auth_error_response(
                        http::StatusCode::UNAUTHORIZED,
                        "邮箱或密码错误",
                        false,
                    )
                }
                Err(err) => {
                    return build_auth_internal_error_response(
                        "auth_ldap_user_sync_failed",
                        err,
                        false,
                    )
                }
            };
            (user, None)
        }
        _ => {
            return build_auth_error_response(
                http::StatusCode::BAD_REQUEST,
                "不支持的认证类型",
                false,
            )
        }
    };

    build_auth_login_success_response(
        state,
        headers,
        client_ip,
        client_device_id,
        user,
        expected_password_hash.as_deref(),
    )
    .await
}

pub(super) async fn maybe_build_local_auth_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    client_ip: std::net::IpAddr,
    request_body: Option<&axum::body::Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("auth") {
        return None;
    }

    match decision.route_kind.as_deref() {
        Some("send_verification_code")
            if request_context.request_path == "/api/auth/send-verification-code" =>
        {
            Some(handle_auth_send_verification_code(state, headers, client_ip, request_body).await)
        }
        Some("login") if request_context.request_path == "/api/auth/login" => {
            Some(handle_auth_login(state, request_context, headers, client_ip, request_body).await)
        }
        Some("register") if request_context.request_path == "/api/auth/register" => {
            Some(handle_auth_register(state, headers, client_ip, request_body).await)
        }
        Some("verify_email") if request_context.request_path == "/api/auth/verify-email" => {
            Some(handle_auth_verify_email(state, client_ip, request_body).await)
        }
        Some("verification_status")
            if request_context.request_path == "/api/auth/verification-status" =>
        {
            Some(handle_auth_verification_status(state, client_ip, request_body).await)
        }
        Some("me") if request_context.request_path == "/api/auth/me" => {
            Some(handle_auth_me(state, request_context, headers).await)
        }
        Some("refresh") if request_context.request_path == "/api/auth/refresh" => {
            Some(handle_auth_refresh(state, request_context, headers).await)
        }
        Some("logout") if request_context.request_path == "/api/auth/logout" => {
            Some(handle_auth_logout(state, request_context, headers).await)
        }
        _ => Some(build_unhandled_public_support_response(request_context)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_auth_internal_error_response, maybe_build_local_auth_response, verify_auth_password,
        AppState, GatewayPublicRequestContext, AUTH_DUMMY_PASSWORD_HASH,
    };
    use crate::control::GatewayControlDecision;
    use axum::body::to_bytes;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use std::net::{IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn auth_dummy_password_hash_has_fixed_bcrypt_work() {
        assert!(verify_auth_password("secret123", AUTH_DUMMY_PASSWORD_HASH)
            .await
            .expect("password worker should complete"));
        assert!(
            !verify_auth_password("incorrect-password", AUTH_DUMMY_PASSWORD_HASH)
                .await
                .expect("password worker should complete")
        );
    }

    #[tokio::test]
    async fn auth_internal_error_response_does_not_expose_error_detail() {
        let response = build_auth_internal_error_response(
            "auth_internal_error_response_test",
            "database secret diagnostic",
            false,
        );
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("json body should parse");
        assert_eq!(payload["detail"], "服务暂不可用，请稍后重试");
        assert!(!String::from_utf8_lossy(&body).contains("database secret diagnostic"));
    }

    fn request_context(method: Method, uri: &str, route_kind: &str) -> GatewayPublicRequestContext {
        GatewayPublicRequestContext::from_request_parts(
            "trace-auth-unhandled",
            &method,
            &uri.parse::<Uri>().expect("uri should parse"),
            &HeaderMap::new(),
            Some(GatewayControlDecision::synthetic(
                uri,
                Some("public_support".to_string()),
                Some("auth".to_string()),
                Some(route_kind.to_string()),
                Some("user:auth".to_string()),
            )),
        )
    }

    #[tokio::test]
    async fn auth_unhandled_route_returns_local_not_implemented_response() {
        let state = AppState::new().expect("gateway should build");
        let request_context = request_context(Method::POST, "/api/auth/login/history", "login");
        let response = maybe_build_local_auth_response(
            &state,
            &request_context,
            &HeaderMap::new(),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            None,
        )
        .await
        .expect("auth handler should return response");

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("json body should parse");
        assert_eq!(
            payload["detail"],
            "public support route not implemented in rust frontdoor"
        );
        assert_eq!(payload["route_family"], "auth");
        assert_eq!(payload["route_kind"], "login");
        assert_eq!(payload["request_path"], "/api/auth/login/history");
    }
}
