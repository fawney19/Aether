use super::super::kiro::{
    admin_provider_oauth_kiro_refresh_base_url_override, fetch_admin_provider_oauth_kiro_email,
    refresh_admin_provider_oauth_kiro_auth_config,
};
use super::session::{
    attach_admin_provider_oauth_device_poll_terminal_response, AdminProviderOAuthDevicePollPayload,
};
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::provisioning::{
    provider_oauth_active_api_formats, provider_oauth_key_proxy_value,
};
use crate::handlers::admin::provider::oauth::runtime::{
    resolve_provider_oauth_runtime_endpoints,
    spawn_provider_oauth_account_state_refresh_after_update,
};
use crate::handlers::admin::provider::oauth::state::{
    build_admin_provider_oauth_backend_unavailable_response, build_kiro_device_key_name,
    current_unix_secs, decode_jwt_claims, json_non_empty_string, json_u64_value,
    parse_provider_oauth_callback_params,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_device_poll_provider_id;
use crate::handlers::admin::request::{AdminAppState, AdminKiroAuthConfig, AdminRequestContext};
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data::repository::provider_oauth::StoredAdminProviderOAuthDeviceSession;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use aether_oauth::provider::{
    ProviderOAuthImportInput, ProviderOAuthService, ProviderOAuthTransportContext,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use url::{form_urlencoded, Url};

const KIRO_SOCIAL_TOKEN_URL: &str = "https://prod.us-east-1.auth.desktop.kiro.dev/oauth/token";
const KIRO_SOCIAL_AUTH_KIRO_VERSION: &str = "0.6.18";

fn kiro_device_session_is_social(session: &StoredAdminProviderOAuthDeviceSession) -> bool {
    session
        .auth_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("social"))
        || session
            .social_provider
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

fn kiro_social_key_name(
    email: Option<&str>,
    social_provider: Option<&str>,
    refresh_token: Option<&str>,
) -> String {
    let provider = social_provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("social");
    if let Some(email) = email.map(str::trim).filter(|value| !value.is_empty()) {
        return format!("{email} ({provider})");
    }
    let fallback = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(value.as_bytes());
            digest[..3]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        })
        .unwrap_or_else(|| "unknown".to_string());
    format!("kiro_{fallback} ({provider})")
}

fn kiro_social_poll_error_response(error: impl Into<String>) -> Response<Body> {
    Json(json!({
        "status": "error",
        "error": error.into(),
        "replaced": false,
    }))
    .into_response()
}

fn windsurf_browser_poll_error_response(error: impl Into<String>) -> Response<Body> {
    Json(json!({
        "status": "error",
        "error": error.into(),
        "replaced": false,
    }))
    .into_response()
}

fn sanitize_windsurf_browser_poll_detail(detail: impl AsRef<str>) -> String {
    let detail = detail.as_ref().trim();
    if detail.is_empty() {
        return "-".to_string();
    }
    if contains_windsurf_sensitive_marker(detail) {
        "[REDACTED upstream error body]".to_string()
    } else {
        detail.chars().take(500).collect()
    }
}

fn sanitize_windsurf_browser_poll_callback_error(error: &str, description: &str) -> String {
    let error = sanitize_windsurf_browser_poll_error_code(error);
    let description = sanitize_windsurf_browser_poll_detail(description);
    format!("{error}: {description}")
}

fn sanitize_windsurf_browser_poll_error_code(error: &str) -> String {
    let error = error.trim();
    if !error.is_empty()
        && error.len() <= 80
        && error
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return error.to_string();
    }
    sanitize_windsurf_browser_poll_detail(error)
}

fn sanitize_windsurf_browser_poll_oauth_error(error: &aether_oauth::core::OAuthError) -> String {
    match error {
        aether_oauth::core::OAuthError::InvalidRequest(_) => {
            "Windsurf token 验证失败: 请求参数无效".to_string()
        }
        aether_oauth::core::OAuthError::HttpStatus { status_code, .. } => {
            format!("Windsurf token 验证失败: HTTP {status_code}")
        }
        _ => "Windsurf token 验证失败".to_string(),
    }
}

fn contains_windsurf_sensitive_marker(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    [
        "token",
        "api_key",
        "apikey",
        "sessiontoken",
        "firebase_id_token",
        "idtoken",
        "authorization",
        "password",
        "secret",
        "devin-session-token$",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
        || value.contains("sk-")
}

fn secret_fingerprint(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    Some(
        digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

fn insert_secret_fingerprint(target: &mut serde_json::Map<String, Value>, key: &str, secret: &str) {
    if let Some(fingerprint) = secret_fingerprint(secret) {
        target.insert(key.to_string(), json!(fingerprint));
    }
}

fn kiro_social_provider_from_login_option(login_option: Option<&str>) -> Option<&'static str> {
    match login_option
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_ascii_lowercase()
        .as_str()
    {
        "google" => Some("Google"),
        "github" | "git_hub" | "git-hub" => Some("Github"),
        _ => None,
    }
}

fn kiro_social_token_redirect_uri(
    session_redirect_uri: &str,
    callback_url: &str,
    login_option: Option<&str>,
) -> String {
    let base = session_redirect_uri
        .trim()
        .trim_end_matches('/')
        .to_string();
    let Some(login_option) = login_option
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return base;
    };

    let Ok(base_url) = Url::parse(&base) else {
        return base;
    };
    let Ok(callback_url) = Url::parse(callback_url.trim()) else {
        return base;
    };

    let base_path = base_url.path().trim_end_matches('/');
    let callback_path = callback_url.path();
    let suffix = if base_path.is_empty() || base_path == "/" {
        callback_path.to_string()
    } else if callback_path == base_path {
        String::new()
    } else {
        callback_path
            .strip_prefix(&format!("{base_path}/"))
            .map(|value| {
                if value.is_empty() {
                    String::new()
                } else {
                    format!("/{value}")
                }
            })
            .unwrap_or_default()
    };

    let mut redirect_uri = if suffix.is_empty() {
        base
    } else {
        format!("{base}{suffix}")
    };
    redirect_uri.push('?');
    redirect_uri.push_str(
        &form_urlencoded::Serializer::new(String::new())
            .append_pair("login_option", login_option)
            .finish(),
    );
    redirect_uri
}

async fn exchange_admin_provider_oauth_kiro_social_code(
    state: &AdminAppState<'_>,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    machine_id: &str,
    proxy: Option<ProxySnapshot>,
) -> Result<Value, String> {
    let url = state.provider_oauth_token_url("kiro_social_token", KIRO_SOCIAL_TOKEN_URL);
    let host = reqwest::Url::parse(&url)
        .ok()
        .and_then(|value| value.host_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "prod.us-east-1.auth.desktop.kiro.dev".to_string());
    let user_agent = format!("KiroIDE-{KIRO_SOCIAL_AUTH_KIRO_VERSION}-{machine_id}");
    let headers = reqwest::header::HeaderMap::from_iter([
        (
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        ),
        (
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        ),
        (
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_str(&user_agent)
                .map_err(|_| "Kiro social User-Agent 无效".to_string())?,
        ),
        (
            reqwest::header::HOST,
            reqwest::header::HeaderValue::from_str(&host)
                .map_err(|_| "Kiro social host 无效".to_string())?,
        ),
    ]);
    let response = state
        .execute_admin_provider_oauth_http_request(
            "kiro_social_token",
            reqwest::Method::POST,
            &url,
            &headers,
            Some("application/json"),
            Some(json!({
                "code": code,
                "code_verifier": code_verifier,
                "redirect_uri": redirect_uri,
            })),
            None,
            proxy,
        )
        .await
        .map_err(|err| format!("Kiro social token 请求失败: {err}"))?;
    if !response.status.is_success() {
        let detail = response.body_text.trim();
        return Err(if detail.is_empty() {
            format!("HTTP {}", response.status.as_u16())
        } else {
            detail.to_string()
        });
    }
    response
        .json_body
        .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())
        .ok_or_else(|| "Kiro social token 返回了非 JSON 响应".to_string())
}

pub(super) async fn handle_admin_provider_oauth_device_poll(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) = admin_provider_oauth_device_poll_provider_id(request_context.path())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let Some(request_body) = request_body else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "请求体必须是合法的 JSON 对象",
        ));
    };
    let payload = match serde_json::from_slice::<AdminProviderOAuthDevicePollPayload>(request_body)
    {
        Ok(payload) => payload,
        Err(_) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ));
        }
    };
    let session_id = payload.session_id.trim();
    if session_id.is_empty() {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "session_id 不能为空",
        ));
    }

    let Some(mut session) = state.read_provider_oauth_device_session(session_id).await? else {
        return Ok(Json(json!({
            "status": "expired",
            "error": "会话不存在或已过期",
            "replaced": false,
        }))
        .into_response());
    };
    if session.provider_id != provider_id {
        return Ok(Json(json!({
            "status": "error",
            "error": "会话与 Provider 不匹配",
            "replaced": false,
        }))
        .into_response());
    }
    if session.status == "authorized" {
        return Ok(Json(json!({
            "status": "authorized",
            "key_id": session.key_id,
            "email": session.email,
            "replaced": session.replaced,
        }))
        .into_response());
    }
    if matches!(session.status.as_str(), "expired" | "error") {
        return Ok(Json(json!({
            "status": session.status,
            "error": session.error_msg,
            "replaced": session.replaced,
        }))
        .into_response());
    }

    if current_unix_secs() > session.expires_at_unix_secs {
        session.status = "expired".to_string();
        session.error_msg = Some("设备码已过期".to_string());
        let _ = state
            .save_provider_oauth_device_session(session_id, &session, 30)
            .await;
        return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
            session_id,
            "expired",
            Json(json!({
                "status": "expired",
                "error": "设备码已过期",
                "replaced": false,
            }))
            .into_response(),
        ));
    }

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    let endpoint_resolution =
        resolve_provider_oauth_runtime_endpoints(state, &provider, &provider_type).await?;
    let endpoints = endpoint_resolution.endpoints;
    let runtime_endpoint = endpoint_resolution.runtime_endpoint;
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            session.proxy_node_id.as_deref(),
            &[
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;

    if provider_type == "windsurf" {
        return handle_admin_provider_oauth_windsurf_browser_device_poll(
            state,
            &provider,
            &endpoints,
            request_proxy,
            session_id,
            session,
            payload.callback_url.as_deref(),
            payload.token.as_deref(),
        )
        .await;
    }

    if kiro_device_session_is_social(&session) {
        return handle_admin_provider_oauth_kiro_social_device_poll(
            state,
            &provider,
            &endpoints,
            request_proxy,
            session_id,
            session,
            payload.callback_url.as_deref(),
        )
        .await;
    }

    let token_result = match state
        .poll_admin_kiro_device_token(
            &session.region,
            &session.client_id,
            &session.client_secret,
            &session.device_code,
            request_proxy.clone(),
        )
        .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    if token_result
        .get("_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let error_code = json_non_empty_string(token_result.get("error")).unwrap_or_default();
        if error_code == "authorization_pending" {
            return Ok(Json(json!({"status": "pending", "replaced": false})).into_response());
        }
        if error_code == "slow_down" {
            return Ok(Json(json!({"status": "slow_down", "replaced": false})).into_response());
        }
        if error_code == "expired_token" {
            session.status = "expired".to_string();
            session.error_msg = Some("设备码已过期".to_string());
            let _ = state
                .save_provider_oauth_device_session(session_id, &session, 30)
                .await;
            return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
                session_id,
                "expired",
                Json(json!({
                    "status": "expired",
                    "error": "设备码已过期",
                    "replaced": false,
                }))
                .into_response(),
            ));
        }
        if error_code == "access_denied" {
            session.status = "error".to_string();
            session.error_msg = Some("用户拒绝授权".to_string());
            let _ = state
                .save_provider_oauth_device_session(session_id, &session, 30)
                .await;
            return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
                session_id,
                "error",
                Json(json!({
                    "status": "error",
                    "error": "用户拒绝授权",
                    "replaced": false,
                }))
                .into_response(),
            ));
        }
        let error_message = json_non_empty_string(token_result.get("error_description"))
            .or_else(|| (!error_code.is_empty()).then_some(error_code.clone()))
            .unwrap_or_else(|| "未知错误".to_string());
        return Ok(Json(json!({
            "status": "error",
            "error": error_message,
            "replaced": false,
        }))
        .into_response());
    }

    let Some(access_token) = json_non_empty_string(
        token_result
            .get("accessToken")
            .or_else(|| token_result.get("access_token")),
    ) else {
        return Ok(Json(json!({
            "status": "error",
            "error": "token 响应缺少 accessToken 或 refreshToken",
            "replaced": false,
        }))
        .into_response());
    };
    let Some(refresh_token) = json_non_empty_string(
        token_result
            .get("refreshToken")
            .or_else(|| token_result.get("refresh_token")),
    ) else {
        return Ok(Json(json!({
            "status": "error",
            "error": "token 响应缺少 accessToken 或 refreshToken",
            "replaced": false,
        }))
        .into_response());
    };
    let initial_expires_at = json_u64_value(
        token_result
            .get("expiresIn")
            .or_else(|| token_result.get("expires_in")),
    )
    .map(|expires_in| current_unix_secs().saturating_add(expires_in))
    .unwrap_or_else(|| current_unix_secs().saturating_add(3600));
    let social_refresh_base_url =
        admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_social_refresh");
    let idc_refresh_base_url =
        admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_idc_refresh");
    let mut refreshed_auth_config = match refresh_admin_provider_oauth_kiro_auth_config(
        state,
        &AdminKiroAuthConfig {
            auth_method: Some("idc".to_string()),
            refresh_token: Some(refresh_token.clone()),
            expires_at: Some(initial_expires_at),
            profile_arn: None,
            region: Some(session.region.clone()),
            auth_region: Some(session.region.clone()),
            api_region: None,
            client_id: Some(session.client_id.clone()),
            client_secret: Some(session.client_secret.clone()),
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: Some(access_token.clone()),
        },
        request_proxy.clone(),
        social_refresh_base_url.as_deref(),
        idc_refresh_base_url.as_deref(),
    )
    .await
    {
        Ok(config) => config,
        Err(detail) => {
            return Ok(Json(json!({
                "status": "error",
                "error": format!("token 验证失败: {detail}"),
                "replaced": false,
            }))
            .into_response());
        }
    };
    if refreshed_auth_config.auth_method.is_none() {
        refreshed_auth_config.auth_method = Some("idc".to_string());
    }
    let Some(access_token) = refreshed_auth_config
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(Json(json!({
            "status": "error",
            "error": "token 验证失败: accessToken 为空",
            "replaced": false,
        }))
        .into_response());
    };
    let expires_at = refreshed_auth_config
        .expires_at
        .unwrap_or_else(|| current_unix_secs().saturating_add(3600));
    let mut email = decode_jwt_claims(&access_token)
        .and_then(|claims| claims.get("email").cloned())
        .and_then(|value| value.as_str().map(ToOwned::to_owned));
    if email.is_none() {
        email = fetch_admin_provider_oauth_kiro_email(
            state,
            &refreshed_auth_config,
            request_proxy.clone(),
        )
        .await;
    }

    let mut auth_config = refreshed_auth_config
        .to_json_value()
        .as_object()
        .cloned()
        .unwrap_or_default();
    auth_config.insert("provider_type".to_string(), json!("kiro"));
    if let Some(email) = email.as_ref() {
        auth_config.insert("email".to_string(), json!(email));
    }

    let duplicate = match state
        .find_duplicate_provider_oauth_key(&provider_id, &auth_config, None)
        .await
    {
        Ok(duplicate) => duplicate,
        Err(detail) => {
            return Ok(Json(json!({
                "status": "error",
                "error": detail,
                "replaced": false,
            }))
            .into_response());
        }
    };

    let api_formats = provider_oauth_active_api_formats(&endpoints);
    let key_proxy = provider_oauth_key_proxy_value(session.proxy_node_id.as_deref());
    let mut replaced = false;
    let persisted_key = if let Some(existing_key) = duplicate {
        replaced = true;
        match state
            .update_existing_provider_oauth_catalog_key(
                &existing_key,
                &provider.provider_type,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy.clone(),
                Some(expires_at),
            )
            .await?
        {
            Some(key) => key,
            None => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    } else {
        let key_name = build_kiro_device_key_name(
            email.as_deref(),
            refreshed_auth_config.refresh_token.as_deref(),
        );
        match state
            .create_provider_oauth_catalog_key(
                &provider_id,
                &provider.provider_type,
                &key_name,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy,
                Some(expires_at),
            )
            .await?
        {
            Some(key) => key,
            None => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    };

    spawn_provider_oauth_account_state_refresh_after_update(
        state.cloned_app(),
        provider.clone(),
        persisted_key.id.clone(),
        request_proxy.clone(),
    );

    session.status = "authorized".to_string();
    session.key_id = Some(persisted_key.id.clone());
    session.email = email.clone();
    session.replaced = replaced;
    session.error_msg = None;
    let _ = state
        .save_provider_oauth_device_session(session_id, &session, 60)
        .await;

    Ok(attach_admin_provider_oauth_device_poll_terminal_response(
        session_id,
        "authorized",
        Json(json!({
            "status": "authorized",
            "key_id": persisted_key.id,
            "email": email,
            "replaced": replaced,
        }))
        .into_response(),
    ))
}

fn windsurf_raw_api_key(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.starts_with("devin-session-token$") || value.starts_with("sk-") {
        Some(value)
    } else {
        None
    }
}

async fn handle_admin_provider_oauth_windsurf_browser_device_poll(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    request_proxy: Option<ProxySnapshot>,
    session_id: &str,
    mut session: StoredAdminProviderOAuthDeviceSession,
    callback_url: Option<&str>,
    token: Option<&str>,
) -> Result<Response<Body>, GatewayError> {
    let callback_url = callback_url
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let token = token.map(str::trim).filter(|value| !value.is_empty());
    if callback_url.is_none() && token.is_none() {
        return Ok(Json(json!({"status": "pending", "replaced": false})).into_response());
    }

    let mut social_provider = session
        .social_provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let imported_token = if let Some(callback_url) = callback_url {
        let callback_params = parse_provider_oauth_callback_params(callback_url);
        if let Some(error) = callback_params.get("error").map(String::as_str) {
            let error_description = callback_params
                .get("error_description")
                .map(String::as_str)
                .unwrap_or("用户拒绝授权");
            let sanitized_error =
                sanitize_windsurf_browser_poll_callback_error(error, error_description);
            session.status = "error".to_string();
            session.error_msg = Some(sanitized_error.clone());
            let _ = state
                .save_provider_oauth_device_session(session_id, &session, 30)
                .await;
            return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
                session_id,
                "error",
                windsurf_browser_poll_error_response(sanitized_error),
            ));
        }
        let Some(callback_state) = callback_params
            .get("state")
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(windsurf_browser_poll_error_response("回调 URL 缺少 state"));
        };
        if callback_state != session_id {
            return Ok(windsurf_browser_poll_error_response(
                "回调 state 与会话不匹配",
            ));
        }
        if let Some(provider) = callback_params
            .get("provider")
            .or_else(|| callback_params.get("login_option"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            social_provider = Some(provider.to_string());
        }
        let Some(callback_token) = callback_params
            .get("token")
            .or_else(|| callback_params.get("auth_token"))
            .or_else(|| callback_params.get("access_token"))
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(windsurf_browser_poll_error_response("回调 URL 缺少 token"));
        };
        callback_token.to_string()
    } else {
        let token = token.unwrap_or_default();
        if windsurf_raw_api_key(token).is_none() {
            return Ok(windsurf_browser_poll_error_response(
                "浏览器授权请提交包含 state 的回调 URL；纯 token 请使用导入授权",
            ));
        }
        token.to_string()
    };

    let mut raw_credentials = serde_json::Map::new();
    if windsurf_raw_api_key(&imported_token).is_some() {
        raw_credentials.insert("api_key".to_string(), json!(imported_token));
    } else {
        raw_credentials.insert("token".to_string(), json!(imported_token));
    }
    if let Some(social_provider) = social_provider.as_ref() {
        raw_credentials.insert("social_provider".to_string(), json!(social_provider));
    }

    let ctx = ProviderOAuthTransportContext {
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type.clone(),
        endpoint_id: None,
        key_id: None,
        auth_type: Some("oauth".to_string()),
        decrypted_api_key: None,
        decrypted_auth_config: None,
        provider_config: provider.config.clone(),
        endpoint_config: None,
        key_config: None,
        network: aether_oauth::network::OAuthNetworkContext::provider_operation(
            request_proxy.clone(),
        ),
    };
    let executor = crate::oauth::GatewayOAuthHttpExecutor::new(*state);
    let result = match ProviderOAuthService::with_builtin_adapters()
        .import_credentials(
            &executor,
            &ctx,
            ProviderOAuthImportInput {
                provider_type: provider.provider_type.clone(),
                name: None,
                refresh_token: None,
                raw_credentials: Some(Value::Object(raw_credentials)),
                network: ctx.network.clone(),
            },
        )
        .await
    {
        Ok(result) => result,
        Err(error) => {
            let sanitized_error = sanitize_windsurf_browser_poll_oauth_error(&error);
            session.status = "error".to_string();
            session.error_msg = Some(sanitized_error.clone());
            let _ = state
                .save_provider_oauth_device_session(session_id, &session, 30)
                .await;
            return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
                session_id,
                "error",
                windsurf_browser_poll_error_response(sanitized_error),
            ));
        }
    };
    let access_token = result.token_set.access_token.trim().to_string();
    if access_token.is_empty() {
        return Ok(windsurf_browser_poll_error_response(
            "Windsurf token 验证返回缺少 apiKey/sessionToken",
        ));
    }
    let mut auth_config = result.auth_config.as_object().cloned().unwrap_or_default();
    auth_config.insert("provider_type".to_string(), json!("windsurf"));
    auth_config.insert("auth_method".to_string(), json!("browser"));
    if let Some(social_provider) = social_provider.as_ref() {
        auth_config
            .entry("social_provider".to_string())
            .or_insert_with(|| json!(social_provider));
    }

    let duplicate = match state
        .find_duplicate_provider_oauth_key(&provider.id, &auth_config, None)
        .await
    {
        Ok(duplicate) => duplicate,
        Err(detail) => {
            return Ok(Json(json!({
                "status": "error",
                "error": detail,
                "replaced": false,
            }))
            .into_response());
        }
    };

    let api_formats = provider_oauth_active_api_formats(endpoints);
    let key_proxy = provider_oauth_key_proxy_value(session.proxy_node_id.as_deref());
    let expires_at = result.token_set.expires_at_unix_secs;
    let email = auth_config
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let mut replaced = false;
    let persisted_key = if let Some(existing_key) = duplicate {
        replaced = true;
        match state
            .update_existing_provider_oauth_catalog_key(
                &existing_key,
                &provider.provider_type,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy.clone(),
                expires_at,
            )
            .await?
        {
            Some(key) => key,
            None => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    } else {
        let key_name = email
            .as_deref()
            .map(|email| format!("windsurf_{email}"))
            .unwrap_or_else(|| format!("windsurf_{}", current_unix_secs()));
        match state
            .create_provider_oauth_catalog_key(
                &provider.id,
                &provider.provider_type,
                &key_name,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy,
                expires_at,
            )
            .await?
        {
            Some(key) => key,
            None => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    };

    spawn_provider_oauth_account_state_refresh_after_update(
        state.cloned_app(),
        provider.clone(),
        persisted_key.id.clone(),
        request_proxy.clone(),
    );

    session.status = "authorized".to_string();
    session.key_id = Some(persisted_key.id.clone());
    session.email = email.clone();
    session.replaced = replaced;
    session.error_msg = None;
    let _ = state
        .save_provider_oauth_device_session(session_id, &session, 60)
        .await;

    Ok(attach_admin_provider_oauth_device_poll_terminal_response(
        session_id,
        "authorized",
        Json(json!({
            "status": "authorized",
            "key_id": persisted_key.id,
            "email": email,
            "replaced": replaced,
        }))
        .into_response(),
    ))
}

async fn handle_admin_provider_oauth_kiro_social_device_poll(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    request_proxy: Option<ProxySnapshot>,
    session_id: &str,
    mut session: StoredAdminProviderOAuthDeviceSession,
    callback_url: Option<&str>,
) -> Result<Response<Body>, GatewayError> {
    let callback_url = callback_url
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(callback_url) = callback_url else {
        return Ok(Json(json!({"status": "pending", "replaced": false})).into_response());
    };

    let callback_params = parse_provider_oauth_callback_params(callback_url);
    if let Some(error) = callback_params.get("error").map(String::as_str) {
        let error_description = callback_params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("用户拒绝授权");
        session.status = "error".to_string();
        session.error_msg = Some(format!("{error}: {error_description}"));
        let _ = state
            .save_provider_oauth_device_session(session_id, &session, 30)
            .await;
        return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
            session_id,
            "error",
            kiro_social_poll_error_response(format!("{error}: {error_description}")),
        ));
    }

    let Some(code) = callback_params
        .get("code")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(kiro_social_poll_error_response("回调 URL 缺少 code"));
    };
    let Some(callback_state) = callback_params
        .get("state")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(kiro_social_poll_error_response("回调 URL 缺少 state"));
    };
    if callback_state != session_id {
        return Ok(kiro_social_poll_error_response("回调 state 与会话不匹配"));
    }

    let Some(code_verifier) = session
        .code_verifier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(kiro_social_poll_error_response("会话缺少 code_verifier"));
    };
    let redirect_uri = session
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("kiro://kiro.kiroAgent/authenticate-success");
    let login_option = callback_params.get("login_option").map(String::as_str);
    let token_redirect_uri =
        kiro_social_token_redirect_uri(redirect_uri, callback_url, login_option);
    let machine_id = session
        .machine_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");

    let token_result = match exchange_admin_provider_oauth_kiro_social_code(
        state,
        code,
        code_verifier,
        &token_redirect_uri,
        machine_id,
        request_proxy.clone(),
    )
    .await
    {
        Ok(payload) => payload,
        Err(detail) => {
            session.status = "error".to_string();
            session.error_msg = Some(format!("token exchange 失败: {detail}"));
            let _ = state
                .save_provider_oauth_device_session(session_id, &session, 30)
                .await;
            return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
                session_id,
                "error",
                kiro_social_poll_error_response(format!("token exchange 失败: {detail}")),
            ));
        }
    };

    let Some(access_token) = json_non_empty_string(
        token_result
            .get("accessToken")
            .or_else(|| token_result.get("access_token")),
    ) else {
        return Ok(kiro_social_poll_error_response(
            "token 响应缺少 accessToken 或 refreshToken",
        ));
    };
    let Some(refresh_token) = json_non_empty_string(
        token_result
            .get("refreshToken")
            .or_else(|| token_result.get("refresh_token")),
    ) else {
        return Ok(kiro_social_poll_error_response(
            "token 响应缺少 accessToken 或 refreshToken",
        ));
    };
    let expires_at = json_u64_value(
        token_result
            .get("expiresIn")
            .or_else(|| token_result.get("expires_in")),
    )
    .map(|expires_in| current_unix_secs().saturating_add(expires_in))
    .unwrap_or_else(|| current_unix_secs().saturating_add(3600));
    let social_provider = kiro_social_provider_from_login_option(login_option)
        .or_else(|| {
            session
                .social_provider
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or("Google")
        .to_string();
    session.social_provider = Some(social_provider.clone());
    let profile_arn = json_non_empty_string(
        token_result
            .get("profileArn")
            .or_else(|| token_result.get("profile_arn")),
    );
    let auth_config = AdminKiroAuthConfig {
        auth_method: Some("social".to_string()),
        refresh_token: Some(refresh_token.clone()),
        expires_at: Some(expires_at),
        profile_arn,
        region: Some("us-east-1".to_string()),
        auth_region: Some("us-east-1".to_string()),
        api_region: None,
        client_id: None,
        client_secret: None,
        machine_id: session.machine_id.clone(),
        kiro_version: Some(KIRO_SOCIAL_AUTH_KIRO_VERSION.to_string()),
        system_version: None,
        node_version: None,
        access_token: Some(access_token.clone()),
    };

    let mut email = decode_jwt_claims(&access_token)
        .and_then(|claims| claims.get("email").cloned())
        .and_then(|value| value.as_str().map(ToOwned::to_owned));
    if email.is_none() {
        email =
            fetch_admin_provider_oauth_kiro_email(state, &auth_config, request_proxy.clone()).await;
    }

    let mut auth_config_object = auth_config
        .to_json_value()
        .as_object()
        .cloned()
        .unwrap_or_default();
    auth_config_object.insert("provider_type".to_string(), json!("kiro"));
    auth_config_object.insert("provider".to_string(), json!(social_provider));
    if let Some(email) = email.as_ref() {
        auth_config_object.insert("email".to_string(), json!(email));
    }
    if let Some(id_token) = json_non_empty_string(
        token_result
            .get("idToken")
            .or_else(|| token_result.get("id_token")),
    ) {
        insert_secret_fingerprint(&mut auth_config_object, "id_token_fingerprint", &id_token);
    }
    if let Some(token_type) = json_non_empty_string(
        token_result
            .get("tokenType")
            .or_else(|| token_result.get("token_type")),
    ) {
        auth_config_object.insert("token_type".to_string(), json!(token_type));
    }

    let duplicate = match state
        .find_duplicate_provider_oauth_key(&provider.id, &auth_config_object, None)
        .await
    {
        Ok(duplicate) => duplicate,
        Err(detail) => {
            return Ok(Json(json!({
                "status": "error",
                "error": detail,
                "replaced": false,
            }))
            .into_response());
        }
    };

    let api_formats = provider_oauth_active_api_formats(endpoints);
    let key_proxy = provider_oauth_key_proxy_value(session.proxy_node_id.as_deref());
    let mut replaced = false;
    let persisted_key = if let Some(existing_key) = duplicate {
        replaced = true;
        match state
            .update_existing_provider_oauth_catalog_key(
                &existing_key,
                &provider.provider_type,
                &access_token,
                &auth_config_object,
                &api_formats,
                key_proxy.clone(),
                Some(expires_at),
            )
            .await?
        {
            Some(key) => key,
            None => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    } else {
        let key_name = kiro_social_key_name(
            email.as_deref(),
            session.social_provider.as_deref(),
            Some(&refresh_token),
        );
        match state
            .create_provider_oauth_catalog_key(
                &provider.id,
                &provider.provider_type,
                &key_name,
                &access_token,
                &auth_config_object,
                &api_formats,
                key_proxy,
                Some(expires_at),
            )
            .await?
        {
            Some(key) => key,
            None => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    };

    spawn_provider_oauth_account_state_refresh_after_update(
        state.cloned_app(),
        provider.clone(),
        persisted_key.id.clone(),
        request_proxy.clone(),
    );

    session.status = "authorized".to_string();
    session.key_id = Some(persisted_key.id.clone());
    session.email = email.clone();
    session.replaced = replaced;
    session.error_msg = None;
    let _ = state
        .save_provider_oauth_device_session(session_id, &session, 60)
        .await;

    Ok(attach_admin_provider_oauth_device_poll_terminal_response(
        session_id,
        "authorized",
        Json(json!({
            "status": "authorized",
            "key_id": persisted_key.id,
            "email": email,
            "replaced": replaced,
        }))
        .into_response(),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn windsurf_browser_poll_callback_error_redacts_sensitive_values() {
        let detail = super::sanitize_windsurf_browser_poll_callback_error(
            "access_denied",
            "bad token devin-session-token$secret and apiKey sk-secret",
        );

        assert_eq!(detail, "access_denied: [REDACTED upstream error body]");
        assert!(!detail.contains("devin-session-token$secret"));
        assert!(!detail.contains("sk-secret"));
    }
}
