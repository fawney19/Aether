use crate::handlers::admin::provider::oauth::state::{current_unix_secs, json_non_empty_string};
use crate::handlers::admin::provider::shared::payloads::{
    KIRO_USAGE_LIMITS_PATH, KIRO_USAGE_SDK_VERSION,
};
use crate::handlers::admin::request::{AdminAppState, AdminKiroAuthConfig};
use crate::provider_transport::kiro::{build_kiro_request_auth_from_config, KiroRequestAuth};
use aether_contracts::ProxySnapshot;
use aether_oauth::core::OAuthError;
use aether_oauth::provider::providers::KiroProviderOAuthAdapter;
use aether_oauth::provider::ProviderOAuthTransportContext;
use serde_json::Value;
use url::form_urlencoded;

pub(super) fn admin_provider_oauth_kiro_refresh_base_url_override(
    state: &AdminAppState<'_>,
    override_key: &str,
) -> Option<String> {
    let override_url = state.app().provider_oauth_token_url(override_key, "");
    let normalized = override_url.trim();
    (!normalized.is_empty()).then(|| normalized.to_string())
}

fn admin_provider_oauth_kiro_ide_tag(kiro_version: &str, machine_id: &str) -> String {
    if machine_id.trim().is_empty() {
        format!("KiroIDE-{kiro_version}")
    } else {
        format!("KiroIDE-{kiro_version}-{machine_id}")
    }
}

fn admin_provider_oauth_kiro_refresh_context(
    proxy: Option<ProxySnapshot>,
) -> ProviderOAuthTransportContext {
    ProviderOAuthTransportContext {
        provider_id: String::new(),
        provider_type: "kiro".to_string(),
        endpoint_id: None,
        key_id: None,
        auth_type: Some("oauth".to_string()),
        decrypted_api_key: None,
        decrypted_auth_config: None,
        provider_config: None,
        endpoint_config: None,
        key_config: None,
        network: aether_oauth::network::OAuthNetworkContext::provider_operation(proxy),
    }
}

fn admin_provider_oauth_kiro_refresh_error(
    auth_config: &AdminKiroAuthConfig,
    error: OAuthError,
) -> String {
    let prefix = if auth_config.is_idc_auth() {
        "IDC refresh"
    } else {
        "social refresh"
    };
    match error {
        OAuthError::HttpStatus {
            status_code,
            body_excerpt,
        } => {
            let detail = body_excerpt.trim();
            if detail.is_empty() {
                format!("{prefix} 失败: HTTP {status_code}")
            } else {
                format!("{prefix} 失败: {detail}")
            }
        }
        OAuthError::Transport(message) => format!("{prefix} 请求失败: {message}"),
        OAuthError::InvalidRequest(message) => format!("{prefix} 参数无效: {message}"),
        OAuthError::InvalidResponse(message) => format!("{prefix} 返回无效响应: {message}"),
        error => format!("{prefix} 失败: {error}"),
    }
}

pub(super) async fn refresh_admin_provider_oauth_kiro_auth_config(
    state: &AdminAppState<'_>,
    auth_config: &AdminKiroAuthConfig,
    proxy: Option<ProxySnapshot>,
    social_refresh_base_url: Option<&str>,
    idc_refresh_base_url: Option<&str>,
) -> Result<AdminKiroAuthConfig, String> {
    let adapter = KiroProviderOAuthAdapter::default().with_refresh_base_urls(
        social_refresh_base_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        idc_refresh_base_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    );
    let ctx = admin_provider_oauth_kiro_refresh_context(proxy);
    adapter
        .refresh_auth_config(
            &crate::oauth::GatewayOAuthHttpExecutor::new(*state),
            &ctx,
            auth_config,
        )
        .await
        .map_err(|error| admin_provider_oauth_kiro_refresh_error(auth_config, error))
}

fn build_kiro_usage_url(auth: &KiroRequestAuth) -> String {
    let host = format!(
        "management.{}.kiro.dev",
        auth.auth_config.effective_api_region()
    );
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("origin", "AI_EDITOR");
    serializer.append_pair("resourceType", "AGENTIC_REQUEST");
    serializer.append_pair("isEmailRequired", "true");
    if let Some(profile_arn) = auth.auth_config.profile_arn_for_payload() {
        serializer.append_pair("profileArn", profile_arn);
    }
    format!(
        "https://{host}{KIRO_USAGE_LIMITS_PATH}?{}",
        serializer.finish()
    )
}

fn build_kiro_usage_headers(
    auth: &KiroRequestAuth,
    host: &str,
) -> Result<reqwest::header::HeaderMap, String> {
    let kiro_version = auth.auth_config.effective_kiro_version();
    let machine_id = auth.machine_id.trim();
    let ide_tag = admin_provider_oauth_kiro_ide_tag(kiro_version, machine_id);
    Ok(reqwest::header::HeaderMap::from_iter([
        (
            reqwest::header::HeaderName::from_static("x-amz-user-agent"),
            reqwest::header::HeaderValue::from_str(&format!(
                "aws-sdk-js/{KIRO_USAGE_SDK_VERSION} {ide_tag}"
            ))
            .map_err(|_| "Kiro usage x-amz-user-agent 无效".to_string())?,
        ),
        (
            reqwest::header::USER_AGENT,
            // getUsageLimits 仍使用 HAR 中的 SDK 指纹，主机和路径已切换为新版 Management。
            reqwest::header::HeaderValue::from_str(&format!(
                "aws-sdk-js/{KIRO_USAGE_SDK_VERSION} ua/2.1 os/other#unknown lang/js md/nodejs#22.21.1 api/codewhispererruntime#1.0.0 m/N,E {ide_tag}"
            ))
            .map_err(|_| "Kiro usage User-Agent 无效".to_string())?,
        ),
        (
            reqwest::header::HOST,
            reqwest::header::HeaderValue::from_str(host)
                .map_err(|_| "Kiro usage host 无效".to_string())?,
        ),
        (
            reqwest::header::HeaderName::from_static("amz-sdk-invocation-id"),
            reqwest::header::HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
                .map_err(|_| "Kiro usage invocation id 无效".to_string())?,
        ),
        (
            reqwest::header::HeaderName::from_static("amz-sdk-request"),
            reqwest::header::HeaderValue::from_static("attempt=1; max=1"),
        ),
        (
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(auth.value.as_str())
                .map_err(|_| "Kiro usage authorization 无效".to_string())?,
        ),
        (
            reqwest::header::CONNECTION,
            reqwest::header::HeaderValue::from_static("close"),
        ),
    ]))
}

pub(super) async fn fetch_admin_provider_oauth_kiro_email(
    state: &AdminAppState<'_>,
    auth_config: &AdminKiroAuthConfig,
    proxy: Option<ProxySnapshot>,
) -> Option<String> {
    let request_auth = build_kiro_request_auth_from_config(auth_config.clone(), None)?;
    let default_url = build_kiro_usage_url(&request_auth);
    let url = state
        .app()
        .provider_oauth_token_url("kiro_device_email", &default_url);
    let host = reqwest::Url::parse(&url)
        .ok()
        .and_then(|value| value.host_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| {
            format!(
                "management.{}.kiro.dev",
                request_auth.auth_config.effective_api_region()
            )
        });
    let headers = build_kiro_usage_headers(&request_auth, &host).ok()?;
    let response = state
        .execute_admin_provider_oauth_http_request(
            "kiro_device_email",
            reqwest::Method::GET,
            &url,
            &headers,
            None,
            None,
            None,
            proxy,
        )
        .await
        .ok()?;
    if !response.status.is_success() {
        return None;
    }
    let payload = response
        .json_body
        .or_else(|| serde_json::from_str::<Value>(&response.body_text).ok())?;
    let metadata =
        aether_admin::provider::quota::parse_kiro_usage_response(&payload, current_unix_secs())?;
    json_non_empty_string(metadata.get("email"))
}
