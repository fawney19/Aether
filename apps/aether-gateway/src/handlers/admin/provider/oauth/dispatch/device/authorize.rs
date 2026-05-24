use super::session::AdminProviderOAuthDeviceAuthorizePayload;
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::runtime::resolve_provider_oauth_runtime_endpoints;
use crate::handlers::admin::provider::oauth::state::{
    build_admin_provider_oauth_backend_unavailable_response, current_unix_secs,
    default_kiro_device_start_url, generate_provider_oauth_nonce, json_non_empty_string,
    json_u64_value, normalize_kiro_device_region, provider_oauth_pkce_s256,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_device_authorize_provider_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_data::repository::provider_oauth::{
    StoredAdminProviderOAuthDeviceSession, KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS,
};
use aether_oauth::provider::{ProviderOAuthService, ProviderOAuthTransportContext};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use url::{form_urlencoded, Url};

const KIRO_SOCIAL_PORTAL_SIGNIN_URL: &str = "https://app.kiro.dev/signin";
const KIRO_SOCIAL_AUTH_EXPIRES_IN_SECS: u64 = 600;
const KIRO_SOCIAL_AUTH_POLL_INTERVAL_SECS: u64 = 5;
const KIRO_SOCIAL_MANUAL_CALLBACK_PORT: u16 = 49153;
const KIRO_SOCIAL_ALLOWED_CALLBACK_PORTS: &[u16] = &[
    3128, 4649, 6588, 8008, 9091, 49153, 50153, 51153, 52153, 53153,
];
const WINDSURF_BROWSER_AUTH_EXPIRES_IN_SECS: u64 = 600;
const WINDSURF_BROWSER_AUTH_POLL_INTERVAL_SECS: u64 = 5;

fn normalize_kiro_device_auth_type(raw: Option<&str>) -> String {
    match raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("identity_center")
        .to_ascii_lowercase()
        .as_str()
    {
        "google" => "google".to_string(),
        "github" | "git_hub" | "git-hub" => "github".to_string(),
        "builderid" | "builder_id" | "builder-id" | "builder" => "builder_id".to_string(),
        "identitycenter" | "identity_center" | "identity-center" | "idc" | "enterprise" => {
            "identity_center".to_string()
        }
        _ => "identity_center".to_string(),
    }
}

fn kiro_social_provider_id(auth_type: &str) -> Option<&'static str> {
    match auth_type {
        "google" => Some("Google"),
        "github" => Some("Github"),
        _ => None,
    }
}

fn default_kiro_social_redirect_uri() -> String {
    format!("http://localhost:{KIRO_SOCIAL_MANUAL_CALLBACK_PORT}")
}

fn normalize_kiro_social_redirect_uri(raw: Option<&str>) -> Result<String, &'static str> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(default_kiro_social_redirect_uri());
    };

    let url = Url::parse(raw).map_err(|_| "Kiro social redirect_uri 必须是合法 URL")?;
    if url.scheme() != "http" {
        return Err("Kiro social redirect_uri 必须使用 http://localhost");
    }
    if !url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("localhost"))
    {
        return Err("Kiro social redirect_uri 必须使用 localhost");
    }
    let Some(port) = url.port() else {
        return Err("Kiro social redirect_uri 必须包含端口");
    };
    if !KIRO_SOCIAL_ALLOWED_CALLBACK_PORTS.contains(&port) {
        return Err("Kiro social redirect_uri 端口不是 Kiro 允许的回调端口");
    }
    if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
        return Err("Kiro social redirect_uri 只能是 http://localhost:{port}");
    }

    Ok(format!("http://localhost:{port}"))
}

fn build_kiro_social_authorization_url(
    portal_url: &str,
    auth_type: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
) -> String {
    if let Ok(mut url) = Url::parse(portal_url) {
        url.query_pairs_mut()
            .append_pair("state", state)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("redirect_from", "KiroIDE")
            .append_pair("login_option", auth_type);
        return url.to_string();
    }

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("state", state);
    serializer.append_pair("code_challenge", code_challenge);
    serializer.append_pair("code_challenge_method", "S256");
    serializer.append_pair("redirect_uri", redirect_uri);
    serializer.append_pair("redirect_from", "KiroIDE");
    serializer.append_pair("login_option", auth_type);
    format!(
        "{}?{}",
        portal_url.trim_end_matches('?'),
        serializer.finish()
    )
}

fn build_windsurf_authorization_url(authorize_url: &str, login_option: &str) -> String {
    let login_option = login_option.trim();
    if login_option.is_empty() {
        return authorize_url.to_string();
    }
    if let Ok(mut url) = Url::parse(authorize_url) {
        url.query_pairs_mut()
            .append_pair("login_option", login_option);
        return url.to_string();
    }
    let separator = if authorize_url.contains('?') {
        '&'
    } else {
        '?'
    };
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("login_option", login_option);
    format!("{authorize_url}{separator}{}", serializer.finish())
}

pub(super) async fn handle_admin_provider_oauth_device_authorize(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) =
        admin_provider_oauth_device_authorize_provider_id(request_context.path())
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
    let payload =
        match serde_json::from_slice::<AdminProviderOAuthDeviceAuthorizePayload>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "请求体必须是合法的 JSON 对象",
                ));
            }
        };
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
    if provider_type != "kiro" && provider_type != "windsurf" {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "设备授权仅支持 Kiro / Windsurf provider",
        ));
    }
    let endpoint_resolution =
        resolve_provider_oauth_runtime_endpoints(state, &provider, &provider_type).await?;
    let runtime_endpoint = endpoint_resolution.runtime_endpoint;
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            payload.proxy_node_id.as_deref(),
            &[
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;

    if provider_type == "windsurf" {
        let session_id = generate_provider_oauth_nonce();
        let login_option = payload
            .login_option
            .as_deref()
            .or(payload.auth_type.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default")
            .to_ascii_lowercase();
        let ctx = ProviderOAuthTransportContext {
            provider_id: provider_id.clone(),
            provider_type: provider_type.clone(),
            endpoint_id: runtime_endpoint
                .as_ref()
                .map(|endpoint| endpoint.id.clone()),
            key_id: None,
            auth_type: Some("oauth".to_string()),
            decrypted_api_key: None,
            decrypted_auth_config: None,
            provider_config: provider.config.clone(),
            endpoint_config: runtime_endpoint
                .as_ref()
                .and_then(|endpoint| endpoint.config.clone()),
            key_config: None,
            network: aether_oauth::network::OAuthNetworkContext::provider_operation(
                request_proxy.clone(),
            ),
        };
        let mut authorization = match ProviderOAuthService::with_builtin_adapters()
            .build_authorize_url(&ctx, &session_id, None)
        {
            Ok(authorization) => authorization,
            Err(error) => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    format!("Windsurf 授权 URL 构建失败: {error}"),
                ));
            }
        };
        authorization.authorize_url =
            build_windsurf_authorization_url(&authorization.authorize_url, &login_option);
        let now_unix_secs = current_unix_secs();
        let session = StoredAdminProviderOAuthDeviceSession {
            provider_id: provider_id.clone(),
            region: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            device_code: String::new(),
            auth_type: Some("browser".to_string()),
            social_provider: Some(login_option.clone()),
            code_verifier: None,
            redirect_uri: Some("show-auth-token".to_string()),
            machine_id: Some(uuid::Uuid::new_v4().to_string().to_ascii_lowercase()),
            interval: WINDSURF_BROWSER_AUTH_POLL_INTERVAL_SECS,
            expires_at_unix_secs: now_unix_secs
                .saturating_add(WINDSURF_BROWSER_AUTH_EXPIRES_IN_SECS),
            status: "pending".to_string(),
            proxy_node_id: payload
                .proxy_node_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            created_at_unix_ms: now_unix_secs,
            key_id: None,
            email: None,
            replaced: false,
            error_msg: None,
        };
        if let Err(response) = state
            .save_provider_oauth_device_session(
                &session_id,
                &session,
                WINDSURF_BROWSER_AUTH_EXPIRES_IN_SECS
                    .saturating_add(KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS),
            )
            .await
        {
            return Ok(response);
        }

        return Ok(Json(json!({
            "session_id": session_id,
            "user_code": "",
            "verification_uri": "https://windsurf.com/windsurf/signin",
            "verification_uri_complete": authorization.authorize_url,
            "expires_in": WINDSURF_BROWSER_AUTH_EXPIRES_IN_SECS,
            "interval": WINDSURF_BROWSER_AUTH_POLL_INTERVAL_SECS,
            "auth_type": "browser",
            "login_option": login_option,
            "redirect_uri": "show-auth-token",
            "callback_required": true,
        }))
        .into_response());
    }

    let auth_type = normalize_kiro_device_auth_type(payload.auth_type.as_deref());
    if let Some(social_provider) = kiro_social_provider_id(&auth_type) {
        let redirect_uri = match normalize_kiro_social_redirect_uri(payload.redirect_uri.as_deref())
        {
            Ok(redirect_uri) => redirect_uri,
            Err(message) => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    message,
                ));
            }
        };
        let code_verifier = generate_provider_oauth_nonce();
        let code_challenge = provider_oauth_pkce_s256(&code_verifier);
        let session_id = generate_provider_oauth_nonce();
        let portal_url =
            state.provider_oauth_token_url("kiro_social_portal", KIRO_SOCIAL_PORTAL_SIGNIN_URL);
        let authorization_url = build_kiro_social_authorization_url(
            &portal_url,
            &auth_type,
            &redirect_uri,
            &code_challenge,
            &session_id,
        );
        let now_unix_secs = current_unix_secs();
        let session = StoredAdminProviderOAuthDeviceSession {
            provider_id: provider_id.clone(),
            region: "us-east-1".to_string(),
            client_id: String::new(),
            client_secret: String::new(),
            device_code: String::new(),
            auth_type: Some("social".to_string()),
            social_provider: Some(social_provider.to_string()),
            code_verifier: Some(code_verifier),
            redirect_uri: Some(redirect_uri.clone()),
            machine_id: Some(uuid::Uuid::new_v4().to_string().to_ascii_lowercase()),
            interval: KIRO_SOCIAL_AUTH_POLL_INTERVAL_SECS,
            expires_at_unix_secs: now_unix_secs.saturating_add(KIRO_SOCIAL_AUTH_EXPIRES_IN_SECS),
            status: "pending".to_string(),
            proxy_node_id: payload
                .proxy_node_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            created_at_unix_ms: now_unix_secs,
            key_id: None,
            email: None,
            replaced: false,
            error_msg: None,
        };
        if let Err(response) = state
            .save_provider_oauth_device_session(
                &session_id,
                &session,
                KIRO_SOCIAL_AUTH_EXPIRES_IN_SECS
                    .saturating_add(KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS),
            )
            .await
        {
            return Ok(response);
        }

        return Ok(Json(json!({
            "session_id": session_id,
            "user_code": "",
            "verification_uri": portal_url,
            "verification_uri_complete": authorization_url,
            "expires_in": KIRO_SOCIAL_AUTH_EXPIRES_IN_SECS,
            "interval": KIRO_SOCIAL_AUTH_POLL_INTERVAL_SECS,
            "auth_type": auth_type,
            "redirect_uri": redirect_uri,
            "callback_required": true,
        }))
        .into_response());
    }

    let region = normalize_kiro_device_region(Some(payload.region.as_str())).ok_or_else(|| {
        build_internal_control_error_response(http::StatusCode::BAD_REQUEST, "region 格式无效")
    });
    let region = match region {
        Ok(region) => region,
        Err(response) => return Ok(response),
    };
    let start_url = payload.start_url.trim();
    let start_url = if start_url.is_empty() {
        default_kiro_device_start_url()
    } else {
        start_url.to_string()
    };

    let client_registration = match state
        .register_admin_kiro_device_oidc_client(&region, &start_url, request_proxy.clone())
        .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let Some(client_id) = json_non_empty_string(client_registration.get("clientId")) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "注册 OIDC 客户端失败: unknown",
        ));
    };
    let Some(client_secret) = json_non_empty_string(client_registration.get("clientSecret")) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "注册 OIDC 客户端失败: unknown",
        ));
    };

    let device_authorization = match state
        .start_admin_kiro_device_authorization(
            &region,
            &client_id,
            &client_secret,
            &start_url,
            request_proxy,
        )
        .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let Some(device_code) = json_non_empty_string(
        device_authorization
            .get("deviceCode")
            .or_else(|| device_authorization.get("device_code")),
    ) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "发起设备授权失败: unknown",
        ));
    };
    let user_code = json_non_empty_string(
        device_authorization
            .get("userCode")
            .or_else(|| device_authorization.get("user_code")),
    )
    .unwrap_or_default();
    let verification_uri = json_non_empty_string(
        device_authorization
            .get("verificationUri")
            .or_else(|| device_authorization.get("verification_uri"))
            .or_else(|| device_authorization.get("verificationUrl")),
    )
    .unwrap_or_default();
    let verification_uri_complete = json_non_empty_string(
        device_authorization
            .get("verificationUriComplete")
            .or_else(|| device_authorization.get("verification_uri_complete"))
            .or_else(|| device_authorization.get("verificationUrlComplete")),
    )
    .unwrap_or_else(|| verification_uri.clone());
    let expires_in = json_u64_value(
        device_authorization
            .get("expiresIn")
            .or_else(|| device_authorization.get("expires_in")),
    )
    .unwrap_or(600);
    let interval = json_u64_value(device_authorization.get("interval")).unwrap_or(5);
    let now_unix_secs = current_unix_secs();
    let session_id = generate_provider_oauth_nonce();
    let session = StoredAdminProviderOAuthDeviceSession {
        provider_id: provider_id.clone(),
        region,
        client_id,
        client_secret,
        device_code,
        auth_type: Some("idc".to_string()),
        social_provider: None,
        code_verifier: None,
        redirect_uri: None,
        machine_id: None,
        interval,
        expires_at_unix_secs: now_unix_secs.saturating_add(expires_in),
        status: "pending".to_string(),
        proxy_node_id: payload
            .proxy_node_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        created_at_unix_ms: now_unix_secs,
        key_id: None,
        email: None,
        replaced: false,
        error_msg: None,
    };
    if let Err(response) = state
        .save_provider_oauth_device_session(
            &session_id,
            &session,
            expires_in.saturating_add(KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS),
        )
        .await
    {
        return Ok(response);
    }

    Ok(Json(json!({
        "session_id": session_id,
        "user_code": user_code,
        "verification_uri": verification_uri,
        "verification_uri_complete": verification_uri_complete,
        "expires_in": expires_in,
        "interval": interval,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::build_windsurf_authorization_url;

    #[test]
    fn windsurf_authorization_url_includes_login_option() {
        let url = build_windsurf_authorization_url(
            "https://windsurf.com/windsurf/signin?state=session-1",
            "github",
        );

        let parsed = url::Url::parse(&url).expect("url should parse");
        let params = parsed
            .query_pairs()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(params.get("state").map(String::as_str), Some("session-1"));
        assert_eq!(
            params.get("login_option").map(String::as_str),
            Some("github")
        );
    }
}
