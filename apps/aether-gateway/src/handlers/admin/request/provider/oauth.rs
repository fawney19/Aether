use super::*;
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTimeouts, ProxySnapshot, RequestBody,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
};
use aether_data::repository::provider_oauth::{
    build_provider_oauth_batch_task_status_payload, provider_oauth_batch_task_storage_key,
    provider_oauth_device_session_storage_key, provider_oauth_state_storage_key,
    StoredAdminProviderOAuthDeviceSession, StoredAdminProviderOAuthState,
    PROVIDER_OAUTH_BATCH_TASK_TTL_SECS, PROVIDER_OAUTH_STATE_TTL_SECS,
};
use axum::http;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use flate2::read::{DeflateDecoder, GzDecoder};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Read;
use url::Url;

const KIRO_IDC_AMZ_USER_AGENT: &str =
    "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";
const ADMIN_PROVIDER_OAUTH_TIMEOUT_MS: u64 = 30_000;
const ADMIN_PROVIDER_OAUTH_PROXY_TIMEOUT_MS: u64 = 60_000;

pub(crate) struct AdminProviderOAuthHttpResponse {
    pub(crate) status: http::StatusCode,
    pub(crate) body_text: String,
    pub(crate) json_body: Option<serde_json::Value>,
}

impl<'a> AdminAppState<'a> {
    pub(crate) async fn update_provider_catalog_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, GatewayError> {
        crate::oauth::ProviderOAuthRepository::update_provider_catalog_key_oauth_credentials(
            self,
            key_id,
            encrypted_api_key,
            encrypted_auth_config,
            expires_at_unix_secs,
        )
        .await
    }

    pub(crate) async fn clear_provider_catalog_key_oauth_invalid_marker(
        &self,
        key_id: &str,
    ) -> Result<bool, GatewayError> {
        crate::oauth::ProviderOAuthRepository::clear_provider_catalog_key_oauth_invalid_marker(
            self, key_id,
        )
        .await
    }

    pub(crate) async fn force_local_oauth_refresh_entry(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<crate::provider_transport::CachedOAuthEntry>, AdminLocalOAuthRefreshError>
    {
        crate::oauth::ProviderOAuthRepository::force_local_oauth_refresh_entry(self, transport)
            .await
    }

    pub(crate) async fn save_provider_oauth_state(
        &self,
        key_id: &str,
        provider_id: &str,
        provider_type: &str,
        pkce_verifier: Option<&str>,
    ) -> Result<String, GatewayError> {
        let nonce = aether_admin::provider::state::generate_provider_oauth_nonce();
        let payload = json!({
            "nonce": nonce,
            "key_id": key_id,
            "provider_id": provider_id,
            "provider_type": provider_type,
            "pkce_verifier": pkce_verifier,
            "created_at": aether_admin::provider::state::current_unix_secs(),
        });
        let key = provider_oauth_state_storage_key(&nonce);
        let value = payload.to_string();
        self.as_ref()
            .runtime_kv_setex(&key, &value, PROVIDER_OAUTH_STATE_TTL_SECS)
            .await?;
        self.as_ref()
            .save_provider_oauth_state_for_tests(&key, &value);
        Ok(nonce)
    }

    pub(crate) async fn consume_provider_oauth_state(
        &self,
        nonce: &str,
    ) -> Result<Option<StoredAdminProviderOAuthState>, GatewayError> {
        let key = provider_oauth_state_storage_key(nonce);
        let raw = self.as_ref().runtime_kv_getdel(&key).await?;
        raw.map(|value| {
            serde_json::from_str::<StoredAdminProviderOAuthState>(&value)
                .map_err(|err| GatewayError::Internal(err.to_string()))
        })
        .transpose()
    }

    pub(crate) async fn exchange_admin_provider_oauth_code(
        &self,
        template: AdminProviderOAuthTemplate,
        code: &str,
        state_nonce: &str,
        pkce_verifier: Option<&str>,
        proxy: Option<ProxySnapshot>,
    ) -> Result<serde_json::Value, Response<Body>> {
        crate::handlers::admin::provider::oauth::state::exchange_admin_provider_oauth_code(
            self,
            template,
            code,
            state_nonce,
            pkce_verifier,
            proxy,
        )
        .await
    }

    pub(crate) async fn exchange_admin_provider_oauth_refresh_token(
        &self,
        template: AdminProviderOAuthTemplate,
        refresh_token: &str,
        proxy: Option<ProxySnapshot>,
    ) -> Result<serde_json::Value, Response<Body>> {
        crate::handlers::admin::provider::oauth::state::exchange_admin_provider_oauth_refresh_token(
            self,
            template,
            refresh_token,
            proxy,
        )
        .await
    }

    pub(crate) async fn save_provider_oauth_batch_task_payload(
        &self,
        task_id: &str,
        task_state: &serde_json::Value,
    ) -> Result<(), GatewayError> {
        let key = provider_oauth_batch_task_storage_key(task_id);
        let serialized = serde_json::to_string(task_state)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;

        self.as_ref()
            .runtime_kv_setex(&key, &serialized, PROVIDER_OAUTH_BATCH_TASK_TTL_SECS)
            .await?;
        self.as_ref()
            .save_provider_oauth_batch_task_for_tests(&key, &serialized);
        Ok(())
    }

    pub(crate) async fn read_provider_oauth_batch_task_payload(
        &self,
        provider_id: &str,
        task_id: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let key = provider_oauth_batch_task_storage_key(task_id);
        let raw = self.as_ref().runtime_kv_get(&key).await?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let parsed = match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let Some(state) = parsed.as_object() else {
            return Ok(None);
        };
        if state
            .get("provider_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            != provider_id
        {
            return Ok(None);
        }
        Ok(Some(build_provider_oauth_batch_task_status_payload(
            provider_id,
            state,
        )))
    }

    pub(crate) async fn save_provider_oauth_device_session(
        &self,
        session_id: &str,
        session: &StoredAdminProviderOAuthDeviceSession,
        ttl_seconds: u64,
    ) -> Result<(), Response<Body>> {
        let key = provider_oauth_device_session_storage_key(session_id);
        let value = serde_json::to_string(session).map_err(|_| {
            build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth redis unavailable",
            )
        })?;
        self.as_ref()
            .runtime_kv_setex(&key, &value, ttl_seconds)
            .await
            .map_err(|_| {
                build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth redis unavailable",
                )
            })?;
        self.as_ref()
            .save_provider_oauth_device_session_for_tests(&key, &value);
        Ok(())
    }

    pub(crate) async fn read_provider_oauth_device_session(
        &self,
        session_id: &str,
    ) -> Result<Option<StoredAdminProviderOAuthDeviceSession>, GatewayError> {
        let key = provider_oauth_device_session_storage_key(session_id);
        let raw = self.as_ref().runtime_kv_get(&key).await?;
        raw.map(|value| {
            serde_json::from_str::<StoredAdminProviderOAuthDeviceSession>(&value)
                .map_err(|err| GatewayError::Internal(err.to_string()))
        })
        .transpose()
    }

    pub(crate) async fn register_admin_kiro_device_oidc_client(
        &self,
        region: &str,
        start_url: &str,
        proxy: Option<ProxySnapshot>,
    ) -> Result<serde_json::Value, Response<Body>> {
        let payload = post_kiro_device_oidc_json(
            self,
            "kiro_device_register",
            format!("https://oidc.{region}.amazonaws.com/client/register"),
            json!({
                "clientName": "Aether Gateway",
                "clientType": "public",
                "scopes": [
                    "codewhisperer:completions",
                    "codewhisperer:analysis",
                    "codewhisperer:conversations",
                    "codewhisperer:transformations",
                    "codewhisperer:taskassist"
                ],
                "grantTypes": [
                    "urn:ietf:params:oauth:grant-type:device_code",
                    "refresh_token"
                ],
                "issuerUrl": start_url,
            }),
            proxy,
        )
        .await?;
        if payload
            .get("_error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            let error_desc = aether_admin::provider::state::json_non_empty_string(
                payload.get("error_description"),
            )
            .or_else(|| aether_admin::provider::state::json_non_empty_string(payload.get("error")))
            .unwrap_or_else(|| "unknown".to_string());
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                format!("注册 OIDC 客户端失败: {error_desc}"),
            ));
        }
        Ok(payload)
    }

    pub(crate) async fn start_admin_kiro_device_authorization(
        &self,
        region: &str,
        client_id: &str,
        client_secret: &str,
        start_url: &str,
        proxy: Option<ProxySnapshot>,
    ) -> Result<serde_json::Value, Response<Body>> {
        let payload = post_kiro_device_oidc_json(
            self,
            "kiro_device_authorize",
            format!("https://oidc.{region}.amazonaws.com/device_authorization"),
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "startUrl": start_url,
            }),
            proxy,
        )
        .await?;
        if payload
            .get("_error")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            let error_desc = aether_admin::provider::state::json_non_empty_string(
                payload.get("error_description"),
            )
            .or_else(|| aether_admin::provider::state::json_non_empty_string(payload.get("error")))
            .unwrap_or_else(|| "unknown".to_string());
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                format!("发起设备授权失败: {error_desc}"),
            ));
        }
        Ok(payload)
    }

    pub(crate) async fn poll_admin_kiro_device_token(
        &self,
        region: &str,
        client_id: &str,
        client_secret: &str,
        device_code: &str,
        proxy: Option<ProxySnapshot>,
    ) -> Result<serde_json::Value, Response<Body>> {
        post_kiro_device_oidc_json(
            self,
            "kiro_device_poll",
            format!("https://oidc.{region}.amazonaws.com/token"),
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": "urn:ietf:params:oauth:grant-type:device_code",
                "deviceCode": device_code,
            }),
            proxy,
        )
        .await
    }

    pub(crate) async fn resolve_admin_provider_oauth_operation_proxy_snapshot(
        &self,
        temporary_proxy_node_id: Option<&str>,
        configured_proxies: &[Option<&serde_json::Value>],
    ) -> Option<ProxySnapshot> {
        crate::oauth::resolve_provider_oauth_operation_proxy_snapshot(
            self,
            temporary_proxy_node_id,
            configured_proxies,
        )
        .await
    }

    pub(crate) async fn find_duplicate_provider_oauth_key(
        &self,
        provider_id: &str,
        auth_config: &serde_json::Map<String, serde_json::Value>,
        exclude_key_id: Option<&str>,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        String,
    > {
        crate::oauth::ProviderOAuthRepository::find_duplicate_provider_oauth_key(
            self,
            provider_id,
            auth_config,
            exclude_key_id,
        )
        .await
    }

    pub(crate) async fn create_provider_oauth_catalog_key(
        &self,
        provider_id: &str,
        provider_type: &str,
        name: &str,
        access_token: &str,
        auth_config: &serde_json::Map<String, serde_json::Value>,
        api_formats: &[String],
        proxy: Option<serde_json::Value>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        crate::oauth::ProviderOAuthRepository::create_provider_oauth_catalog_key(
            self,
            provider_id,
            provider_type,
            name,
            access_token,
            auth_config,
            api_formats,
            proxy,
            expires_at_unix_secs,
        )
        .await
    }

    pub(crate) async fn update_existing_provider_oauth_catalog_key(
        &self,
        existing_key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
        provider_type: &str,
        access_token: &str,
        auth_config: &serde_json::Map<String, serde_json::Value>,
        api_formats: &[String],
        proxy: Option<serde_json::Value>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        crate::oauth::ProviderOAuthRepository::update_existing_provider_oauth_catalog_key(
            self,
            existing_key,
            provider_type,
            access_token,
            auth_config,
            api_formats,
            proxy,
            expires_at_unix_secs,
        )
        .await
    }

    pub(crate) async fn refresh_provider_oauth_account_state_after_update(
        &self,
        provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        key_id: &str,
        proxy_override: Option<&ProxySnapshot>,
    ) -> Result<(bool, Option<String>), GatewayError> {
        crate::oauth::ProviderOAuthRepository::refresh_provider_oauth_account_state_after_update(
            self,
            provider,
            key_id,
            proxy_override,
        )
        .await
    }
}

async fn post_kiro_device_oidc_json(
    state: &AdminAppState<'_>,
    endpoint_key: &str,
    default_url: String,
    body: serde_json::Value,
    proxy: Option<ProxySnapshot>,
) -> Result<serde_json::Value, Response<Body>> {
    let url = state.provider_oauth_token_url(endpoint_key, &default_url);
    let host = Url::parse(&url)
        .ok()
        .and_then(|value| value.host_str().map(ToOwned::to_owned))
        .unwrap_or_default();
    let headers = reqwest::header::HeaderMap::from_iter([
        (
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        ),
        (
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("*/*"),
        ),
        (
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("node"),
        ),
        (
            reqwest::header::HeaderName::from_static("x-amz-user-agent"),
            reqwest::header::HeaderValue::from_static(KIRO_IDC_AMZ_USER_AGENT),
        ),
    ]);
    let headers = maybe_insert_host_header(headers, host.as_str());
    let response = state
        .execute_admin_provider_oauth_http_request(
            endpoint_key,
            reqwest::Method::POST,
            &url,
            &headers,
            Some("application/json"),
            Some(body),
            None,
            proxy,
        )
        .await
        .map_err(|_| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "发起设备授权失败: unknown",
            )
        })?;
    let status = response.status;
    let body_text = response.body_text;
    Ok(
        match serde_json::from_str::<serde_json::Value>(&body_text) {
            Ok(mut payload) => {
                if !status.is_success() {
                    if let Some(object) = payload.as_object_mut() {
                        object.insert("_error".to_string(), json!(true));
                    } else {
                        payload = json!({
                            "_error": true,
                            "data": payload,
                        });
                    }
                }
                payload
            }
            Err(_) => json!({
                "_error": !status.is_success(),
                "error": body_text.trim(),
            }),
        },
    )
}

impl<'a> AdminAppState<'a> {
    pub(crate) async fn execute_admin_provider_oauth_http_request(
        &self,
        request_id: &str,
        method: reqwest::Method,
        url: &str,
        headers: &reqwest::header::HeaderMap,
        content_type: Option<&str>,
        json_body: Option<serde_json::Value>,
        body_bytes: Option<Vec<u8>>,
        proxy: Option<ProxySnapshot>,
    ) -> Result<AdminProviderOAuthHttpResponse, String> {
        let network = aether_oauth::network::OAuthNetworkContext::provider_operation(proxy);
        let request = aether_oauth::network::OAuthHttpRequest {
            request_id: request_id.to_string(),
            method,
            url: url.to_string(),
            headers: admin_provider_oauth_execution_headers(headers),
            content_type: content_type
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            json_body,
            body_bytes,
            network,
        };
        let response = aether_oauth::network::OAuthHttpExecutor::execute(
            &crate::oauth::GatewayOAuthHttpExecutor::new(*self),
            request,
        )
        .await
        .map_err(|err| err.to_string())?;
        Ok(AdminProviderOAuthHttpResponse {
            status: http::StatusCode::from_u16(response.status_code)
                .unwrap_or(http::StatusCode::BAD_GATEWAY),
            body_text: response.body_text,
            json_body: response.json_body,
        })
    }
}

fn admin_provider_oauth_timeout_ms(proxy: Option<&ProxySnapshot>) -> u64 {
    if proxy.is_some() {
        ADMIN_PROVIDER_OAUTH_PROXY_TIMEOUT_MS
    } else {
        ADMIN_PROVIDER_OAUTH_TIMEOUT_MS
    }
}

fn maybe_insert_host_header(
    mut headers: reqwest::header::HeaderMap,
    host: &str,
) -> reqwest::header::HeaderMap {
    let host = host.trim();
    if host.is_empty() {
        return headers;
    }
    if let Ok(value) = reqwest::header::HeaderValue::from_str(host) {
        headers.insert(reqwest::header::HOST, value);
    }
    headers
}

fn admin_provider_oauth_execution_headers(
    headers: &reqwest::header::HeaderMap,
) -> BTreeMap<String, String> {
    let mut headers: BTreeMap<String, String> = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|text| (name.as_str().to_string(), text.to_string()))
        })
        .collect();
    headers.insert(
        EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER.to_string(),
        "true".to_string(),
    );
    headers
}

fn admin_provider_oauth_execution_json_body(result: &ExecutionResult) -> Option<serde_json::Value> {
    result
        .body
        .as_ref()
        .and_then(|body| body.json_body.clone())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| admin_provider_oauth_execution_body_bytes(&result.headers, body))
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        })
}

fn admin_provider_oauth_execution_body_text(result: &ExecutionResult) -> String {
    result
        .body
        .as_ref()
        .and_then(|body| admin_provider_oauth_execution_body_bytes(&result.headers, body))
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
                .and_then(|value| serde_json::to_string(value).ok())
        })
        .unwrap_or_default()
}

fn admin_provider_oauth_execution_body_bytes(
    headers: &BTreeMap<String, String>,
    body: &aether_contracts::ResponseBody,
) -> Option<Vec<u8>> {
    let bytes = body
        .body_bytes_b64
        .as_deref()
        .and_then(|value| STANDARD.decode(value).ok())?;
    admin_provider_oauth_decode_response_bytes(
        &bytes,
        headers.get("content-encoding").map(String::as_str),
    )
    .or(Some(bytes))
}

fn admin_provider_oauth_decode_response_bytes(
    bytes: &[u8],
    content_encoding: Option<&str>,
) -> Option<Vec<u8>> {
    let encoding = content_encoding
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    match encoding.as_deref() {
        Some("gzip") => {
            let mut decoder = GzDecoder::new(bytes);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        }
        Some("deflate") => {
            let mut decoder = DeflateDecoder::new(bytes);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).ok()?;
            Some(out)
        }
        _ => None,
    }
}

fn admin_provider_oauth_gateway_error_message(error: GatewayError) -> String {
    match error {
        GatewayError::UpstreamUnavailable { message, .. }
        | GatewayError::ControlUnavailable { message, .. }
        | GatewayError::Client { message, .. }
        | GatewayError::Internal(message) => message,
    }
}
