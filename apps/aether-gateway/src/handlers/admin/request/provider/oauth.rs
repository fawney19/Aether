use super::*;
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTimeouts, ProxySnapshot, RequestBody,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
};
use aether_data::repository::provider_oauth::{
    build_provider_oauth_batch_task_status_payload, provider_oauth_batch_task_secret_purpose,
    provider_oauth_batch_task_storage_key, provider_oauth_device_session_secret_purpose,
    provider_oauth_device_session_storage_key, provider_oauth_state_storage_key,
    StoredAdminProviderOAuthDeviceSession, StoredAdminProviderOAuthState,
    PROVIDER_OAUTH_BATCH_TASK_TTL_SECS, PROVIDER_OAUTH_STATE_TTL_SECS,
};
use axum::http;
use flate2::read::{DeflateDecoder, GzDecoder};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;
use url::Url;

const KIRO_IDC_AMZ_USER_AGENT: &str = "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";
const ADMIN_PROVIDER_OAUTH_TIMEOUT_MS: u64 = 30_000;
const ADMIN_PROVIDER_OAUTH_PROXY_TIMEOUT_MS: u64 = 60_000;
const ADMIN_PROVIDER_OAUTH_RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const ADMIN_PROVIDER_OAUTH_STATE_SECRET_PURPOSE: &str = "provider-oauth-state";
const ADMIN_PROVIDER_OAUTH_STATE_MAX_CLOCK_SKEW_SECS: u64 = 60;

pub(crate) struct AdminProviderOAuthHttpResponse {
    pub(crate) status: http::StatusCode,
    pub(crate) body_text: String,
    pub(crate) json_body: Option<serde_json::Value>,
}

fn admin_provider_oauth_state_secret_purpose(nonce: &str) -> String {
    format!(
        "{ADMIN_PROVIDER_OAUTH_STATE_SECRET_PURPOSE}:{}",
        provider_oauth_state_storage_key(nonce.trim())
    )
}

fn is_generated_admin_provider_oauth_nonce(nonce: &str) -> bool {
    let nonce = nonce.trim();
    nonce.len() == 64
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_admin_provider_oauth_state(
    state: &AdminAppState<'_>,
    expected_nonce: &str,
    stored: &str,
) -> Result<StoredAdminProviderOAuthState, GatewayError> {
    let expected_nonce = expected_nonce.trim();
    let purpose = admin_provider_oauth_state_secret_purpose(expected_nonce);
    let plaintext =
        crate::handlers::shared::open_runtime_secret_payload(state.as_ref(), &purpose, stored)
            .ok_or_else(invalid_admin_provider_oauth_state_error)?;
    let record = serde_json::from_str::<StoredAdminProviderOAuthState>(&plaintext)
        .map_err(|_| invalid_admin_provider_oauth_state_error())?;
    validate_admin_provider_oauth_state(expected_nonce, &record)?;
    Ok(record)
}

fn invalid_admin_provider_oauth_state_error() -> GatewayError {
    GatewayError::Client {
        status: http::StatusCode::BAD_REQUEST,
        message: "provider OAuth state is invalid".to_string(),
    }
}

fn invalid_admin_provider_oauth_device_session_error() -> GatewayError {
    GatewayError::Client {
        status: http::StatusCode::BAD_REQUEST,
        message: "provider OAuth device session is invalid".to_string(),
    }
}

fn is_valid_admin_provider_oauth_ephemeral_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn validate_admin_provider_oauth_device_session(
    expected_session_id: &str,
    record: &StoredAdminProviderOAuthDeviceSession,
) -> Result<(), GatewayError> {
    let raw_expected_session_id = expected_session_id;
    let expected_session_id = expected_session_id.trim();
    let optional_identity_is_invalid = |value: Option<&str>| {
        value.is_some_and(|value| value.trim().is_empty() || value != value.trim())
    };
    if raw_expected_session_id != expected_session_id
        || !is_valid_admin_provider_oauth_ephemeral_id(expected_session_id)
        || record.session_id != expected_session_id
        || !is_valid_admin_provider_oauth_ephemeral_id(&record.session_id)
        || record.provider_id.trim().is_empty()
        || record.provider_id != record.provider_id.trim()
        || record.initiated_by_user_id.trim().is_empty()
        || record.initiated_by_user_id != record.initiated_by_user_id.trim()
        || optional_identity_is_invalid(record.initiated_by_session_id.as_deref())
        || optional_identity_is_invalid(record.initiated_by_management_token_id.as_deref())
        || (record.initiated_by_session_id.is_none()
            && record.initiated_by_management_token_id.is_none())
        || !matches!(
            record.status.as_str(),
            "pending" | "authorized" | "expired" | "error"
        )
        || record.created_at_unix_ms == 0
        || record.expires_at_unix_secs < record.created_at_unix_ms
        || (record.status == "authorized"
            && record
                .key_id
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty))
    {
        return Err(invalid_admin_provider_oauth_device_session_error());
    }
    Ok(())
}

fn decode_admin_provider_oauth_device_session(
    state: &AdminAppState<'_>,
    expected_session_id: &str,
    stored: &str,
) -> Result<StoredAdminProviderOAuthDeviceSession, GatewayError> {
    let purpose = provider_oauth_device_session_secret_purpose(expected_session_id);
    let plaintext =
        crate::handlers::shared::open_runtime_secret_payload(state.as_ref(), &purpose, stored)
            .ok_or_else(invalid_admin_provider_oauth_device_session_error)?;
    let record = serde_json::from_str::<StoredAdminProviderOAuthDeviceSession>(&plaintext)
        .map_err(|_| invalid_admin_provider_oauth_device_session_error())?;
    validate_admin_provider_oauth_device_session(expected_session_id, &record)?;
    Ok(record)
}

fn decode_admin_provider_oauth_batch_task(
    state: &AdminAppState<'_>,
    expected_task_id: &str,
    stored: &str,
) -> Result<serde_json::Value, GatewayError> {
    let invalid = || GatewayError::Internal("provider OAuth batch task is invalid".to_string());
    let purpose = provider_oauth_batch_task_secret_purpose(expected_task_id);
    let plaintext =
        crate::handlers::shared::open_runtime_secret_payload(state.as_ref(), &purpose, stored)
            .ok_or_else(invalid)?;
    let parsed = serde_json::from_str::<serde_json::Value>(&plaintext).map_err(|_| invalid())?;
    let state = parsed.as_object().ok_or_else(invalid)?;
    if state.get("task_id").and_then(serde_json::Value::as_str) != Some(expected_task_id) {
        return Err(invalid());
    }
    Ok(parsed)
}

fn validate_admin_provider_oauth_state(
    expected_nonce: &str,
    record: &StoredAdminProviderOAuthState,
) -> Result<(), GatewayError> {
    let invalid = invalid_admin_provider_oauth_state_error;
    if !is_generated_admin_provider_oauth_nonce(expected_nonce)
        || record.nonce != expected_nonce
        || !is_generated_admin_provider_oauth_nonce(&record.nonce)
        || record.provider_id.trim().is_empty()
        || record.provider_id != record.provider_id.trim()
        || record.provider_type.trim().is_empty()
        || record.provider_type != record.provider_type.trim().to_ascii_lowercase()
        || record.initiated_by_user_id.trim().is_empty()
        || record.initiated_by_user_id != record.initiated_by_user_id.trim()
        || record
            .pkce_verifier
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || record
            .initiated_by_session_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || record
            .initiated_by_management_token_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || (record.initiated_by_session_id.is_none()
            && record.initiated_by_management_token_id.is_none())
    {
        return Err(invalid());
    }
    let now = aether_admin::provider::state::current_unix_secs();
    if record.created_at > now.saturating_add(ADMIN_PROVIDER_OAUTH_STATE_MAX_CLOCK_SKEW_SECS)
        || now.saturating_sub(record.created_at) > PROVIDER_OAUTH_STATE_TTL_SECS
    {
        return Err(invalid());
    }
    Ok(())
}

impl<'a> AdminAppState<'a> {
    pub(crate) async fn update_provider_catalog_key_oauth_runtime_state(
        &self,
        key_id: &str,
        oauth_invalid_at_unix_secs: Option<u64>,
        oauth_invalid_reason: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, GatewayError> {
        self.app
            .update_provider_catalog_key_oauth_runtime_state(
                key_id,
                oauth_invalid_at_unix_secs,
                oauth_invalid_reason,
                updated_at_unix_secs,
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
        expected_encrypted_auth_config: Option<&str>,
        initiated_by_user_id: &str,
        initiated_by_session_id: Option<&str>,
        initiated_by_management_token_id: Option<&str>,
    ) -> Result<String, GatewayError> {
        let nonce = aether_admin::provider::state::generate_provider_oauth_nonce();
        let record = StoredAdminProviderOAuthState {
            nonce: nonce.clone(),
            key_id: key_id.to_string(),
            provider_id: provider_id.to_string(),
            provider_type: provider_type.to_string(),
            pkce_verifier: pkce_verifier.map(ToOwned::to_owned),
            expected_encrypted_auth_config: expected_encrypted_auth_config.map(ToOwned::to_owned),
            initiated_by_user_id: initiated_by_user_id.to_string(),
            initiated_by_session_id: initiated_by_session_id.map(ToOwned::to_owned),
            initiated_by_management_token_id: initiated_by_management_token_id
                .map(ToOwned::to_owned),
            created_at: aether_admin::provider::state::current_unix_secs(),
        };
        validate_admin_provider_oauth_state(&nonce, &record)?;
        let key = provider_oauth_state_storage_key(&nonce);
        let value = serde_json::to_string(&record)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let purpose = admin_provider_oauth_state_secret_purpose(&nonce);
        let sealed =
            crate::handlers::shared::seal_runtime_secret_payload(self.as_ref(), &purpose, &value)
                .ok_or_else(|| {
                GatewayError::Internal("provider OAuth state encryption unavailable".to_string())
            })?;
        self.as_ref()
            .runtime_kv_setex(&key, &sealed, PROVIDER_OAUTH_STATE_TTL_SECS)
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
        raw.map(|value| decode_admin_provider_oauth_state(self, nonce, &value))
            .transpose()
    }

    pub(crate) async fn load_provider_oauth_state(
        &self,
        nonce: &str,
    ) -> Result<Option<StoredAdminProviderOAuthState>, GatewayError> {
        let key = provider_oauth_state_storage_key(nonce);
        let raw = self.as_ref().runtime_kv_get(&key).await?;
        raw.map(|value| decode_admin_provider_oauth_state(self, nonce, &value))
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
        let Some(state) = task_state.as_object() else {
            return Err(GatewayError::Internal(
                "provider OAuth batch task is invalid".to_string(),
            ));
        };
        if !is_valid_admin_provider_oauth_ephemeral_id(task_id)
            || state.get("task_id").and_then(serde_json::Value::as_str) != Some(task_id)
            || state
                .get("provider_id")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|value| value.trim().is_empty() || value != value.trim())
        {
            return Err(GatewayError::Internal(
                "provider OAuth batch task is invalid".to_string(),
            ));
        }
        let key = provider_oauth_batch_task_storage_key(task_id);
        let serialized = serde_json::to_string(task_state)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let purpose = provider_oauth_batch_task_secret_purpose(task_id);
        let sealed = crate::handlers::shared::seal_runtime_secret_payload(
            self.as_ref(),
            &purpose,
            &serialized,
        )
        .ok_or_else(|| {
            GatewayError::Internal("provider OAuth batch task encryption unavailable".to_string())
        })?;

        self.as_ref()
            .runtime_kv_setex(&key, &sealed, PROVIDER_OAUTH_BATCH_TASK_TTL_SECS)
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
        if provider_id.trim().is_empty()
            || provider_id != provider_id.trim()
            || !is_valid_admin_provider_oauth_ephemeral_id(task_id)
        {
            return Ok(None);
        }
        let key = provider_oauth_batch_task_storage_key(task_id);
        let raw = self.as_ref().runtime_kv_get(&key).await?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let parsed = decode_admin_provider_oauth_batch_task(self, task_id, &raw)?;
        let Some(state) = parsed.as_object() else {
            unreachable!("validated provider OAuth batch task must be an object");
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
        if validate_admin_provider_oauth_device_session(session_id, session).is_err() {
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "provider oauth device session is invalid",
            ));
        }
        let key = provider_oauth_device_session_storage_key(session_id);
        let value = serde_json::to_string(session).map_err(|_| {
            build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth redis unavailable",
            )
        })?;
        let purpose = provider_oauth_device_session_secret_purpose(session_id);
        let sealed =
            crate::handlers::shared::seal_runtime_secret_payload(self.as_ref(), &purpose, &value)
                .ok_or_else(|| {
                build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth session encryption unavailable",
                )
            })?;
        self.as_ref()
            .runtime_kv_setex(&key, &sealed, ttl_seconds)
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
        if session_id != session_id.trim()
            || !is_valid_admin_provider_oauth_ephemeral_id(session_id)
        {
            return Err(invalid_admin_provider_oauth_device_session_error());
        }
        let key = provider_oauth_device_session_storage_key(session_id);
        let raw = self.as_ref().runtime_kv_get(&key).await?;
        raw.map(|value| decode_admin_provider_oauth_device_session(self, session_id, &value))
            .transpose()
    }

    pub(crate) async fn register_admin_kiro_device_oidc_client(
        &self,
        region: &str,
        start_url: &str,
        proxy: Option<ProxySnapshot>,
    ) -> Result<serde_json::Value, Response<Body>> {
        let region = aether_provider_transport::kiro::normalize_kiro_region(region);
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
            let error_code = kiro_device_oidc_error_code(&payload);
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                format!("注册 OIDC 客户端失败: {error_code}"),
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
        let region = aether_provider_transport::kiro::normalize_kiro_region(region);
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
            let error_code = kiro_device_oidc_error_code(&payload);
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                format!("发起设备授权失败: {error_code}"),
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
        let region = aether_provider_transport::kiro::normalize_kiro_region(region);
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
    Ok(project_kiro_device_oidc_response(
        response.status,
        &response.body_text,
    ))
}

fn project_kiro_device_oidc_response(status: http::StatusCode, body_text: &str) -> Value {
    match serde_json::from_str::<Value>(body_text) {
        Ok(payload) if status.is_success() => payload,
        Ok(payload) => json!({
            "_error": true,
            "error": kiro_device_oidc_error_code(&payload),
        }),
        Err(_) => json!({
            "_error": true,
            "error": if status.is_success() {
                "invalid_response"
            } else {
                "upstream_error"
            },
        }),
    }
}

fn kiro_device_oidc_error_code(payload: &Value) -> &'static str {
    let Some(error_code) = payload.get("error").and_then(Value::as_str).map(str::trim) else {
        return "upstream_error";
    };
    match error_code {
        "access_denied" => "access_denied",
        "authorization_pending" => "authorization_pending",
        "expired_token" => "expired_token",
        "invalid_client" => "invalid_client",
        "invalid_client_metadata" => "invalid_client_metadata",
        "invalid_grant" => "invalid_grant",
        "invalid_redirect_uri" => "invalid_redirect_uri",
        "invalid_request" => "invalid_request",
        "invalid_scope" => "invalid_scope",
        "slow_down" => "slow_down",
        "unauthorized_client" => "unauthorized_client",
        _ => "upstream_error",
    }
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
            transport_profile: None,
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
    let bytes = body.body_bytes_b64.as_deref().and_then(|value| {
        crate::execution_runtime::transport::decode_base64_body_with_limit(
            value,
            ADMIN_PROVIDER_OAUTH_RESPONSE_BODY_LIMIT_BYTES,
        )
        .ok()
    })?;
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
            read_admin_provider_oauth_decoder_with_limit(
                &mut decoder,
                ADMIN_PROVIDER_OAUTH_RESPONSE_BODY_LIMIT_BYTES,
            )
        }
        Some("deflate") => {
            let mut decoder = DeflateDecoder::new(bytes);
            read_admin_provider_oauth_decoder_with_limit(
                &mut decoder,
                ADMIN_PROVIDER_OAUTH_RESPONSE_BODY_LIMIT_BYTES,
            )
        }
        _ => None,
    }
}

fn read_admin_provider_oauth_decoder_with_limit(
    decoder: &mut impl Read,
    limit_bytes: usize,
) -> Option<Vec<u8>> {
    let read_limit = u64::try_from(limit_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut limited = decoder.take(read_limit);
    let mut out = Vec::new();
    limited.read_to_end(&mut out).ok()?;
    (out.len() <= limit_bytes).then_some(out)
}

fn admin_provider_oauth_gateway_error_message(error: GatewayError) -> String {
    error.into_message()
}

#[cfg(test)]
mod response_decode_tests {
    use std::io::Cursor;

    use super::{
        admin_provider_oauth_state_secret_purpose, decode_admin_provider_oauth_batch_task,
        decode_admin_provider_oauth_device_session, decode_admin_provider_oauth_state,
        project_kiro_device_oidc_response, read_admin_provider_oauth_decoder_with_limit,
    };
    use crate::{data::GatewayDataState, AppState};
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_data::repository::provider_oauth::{
        provider_oauth_batch_task_secret_purpose, provider_oauth_device_session_secret_purpose,
        StoredAdminProviderOAuthDeviceSession, StoredAdminProviderOAuthState,
    };
    use axum::http::StatusCode;
    use serde_json::json;

    #[test]
    fn admin_oauth_decoder_accepts_exact_limit_and_rejects_limit_plus_one() {
        let mut exact = Cursor::new(vec![b'x'; 8]);
        assert_eq!(
            read_admin_provider_oauth_decoder_with_limit(&mut exact, 8),
            Some(vec![b'x'; 8])
        );

        let mut oversized = Cursor::new(vec![b'x'; 9]);
        assert!(read_admin_provider_oauth_decoder_with_limit(&mut oversized, 8).is_none());
    }

    #[test]
    fn admin_provider_oauth_state_ciphertext_is_bound_to_its_nonce() {
        let app = AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let admin = super::AdminAppState::new(&app);
        let record = StoredAdminProviderOAuthState {
            nonce: "a".repeat(64),
            key_id: "key-1".to_string(),
            provider_id: "provider-1".to_string(),
            provider_type: "codex".to_string(),
            pkce_verifier: Some("verifier".to_string()),
            expected_encrypted_auth_config: None,
            initiated_by_user_id: "admin-1".to_string(),
            initiated_by_session_id: Some("session-1".to_string()),
            initiated_by_management_token_id: None,
            created_at: aether_admin::provider::state::current_unix_secs(),
        };
        let plaintext = serde_json::to_string(&record).expect("state should serialize");
        let sealed = crate::handlers::shared::seal_runtime_secret_payload(
            &app,
            &admin_provider_oauth_state_secret_purpose(&record.nonce),
            &plaintext,
        )
        .expect("state should seal");

        assert_eq!(
            decode_admin_provider_oauth_state(&admin, &record.nonce, &sealed)
                .expect("matching state should open"),
            record
        );
        assert!(matches!(
            decode_admin_provider_oauth_state(&admin, &"b".repeat(64), &sealed),
            Err(crate::GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ..
            })
        ));
        assert!(matches!(
            decode_admin_provider_oauth_state(&admin, &record.nonce, "damaged-ciphertext"),
            Err(crate::GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ..
            })
        ));
    }

    #[test]
    fn admin_provider_oauth_device_session_ciphertext_is_bound_to_session_and_principal() {
        let app = AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let admin = super::AdminAppState::new(&app);
        let now = aether_admin::provider::state::current_unix_secs();
        let record = StoredAdminProviderOAuthDeviceSession {
            session_id: "session-123".to_string(),
            provider_id: "provider-1".to_string(),
            initiated_by_user_id: "admin-1".to_string(),
            initiated_by_session_id: Some("admin-session-1".to_string()),
            initiated_by_management_token_id: None,
            region: "us-east-1".to_string(),
            client_id: "client-1".to_string(),
            client_secret: "secret-1".to_string(),
            device_code: "device-code-1".to_string(),
            auth_type: Some("idc".to_string()),
            social_provider: None,
            code_verifier: None,
            redirect_uri: None,
            machine_id: None,
            interval: 5,
            expires_at_unix_secs: now.saturating_add(600),
            status: "pending".to_string(),
            proxy_node_id: None,
            created_at_unix_ms: now,
            key_id: None,
            email: None,
            replaced: false,
            error_msg: None,
        };
        let plaintext = serde_json::to_string(&record).expect("device session should serialize");
        let sealed = crate::handlers::shared::seal_runtime_secret_payload(
            &app,
            &provider_oauth_device_session_secret_purpose(&record.session_id),
            &plaintext,
        )
        .expect("device session should seal");

        let decoded =
            decode_admin_provider_oauth_device_session(&admin, &record.session_id, &sealed)
                .expect("matching device session should open");
        assert_eq!(decoded.session_id, record.session_id);
        assert_eq!(decoded.initiated_by_user_id, record.initiated_by_user_id);
        assert_eq!(
            decoded.initiated_by_session_id,
            record.initiated_by_session_id
        );
        assert!(matches!(
            decode_admin_provider_oauth_device_session(&admin, "session-456", &sealed),
            Err(crate::GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ..
            })
        ));
        assert!(matches!(
            decode_admin_provider_oauth_device_session(
                &admin,
                &record.session_id,
                "damaged-ciphertext"
            ),
            Err(crate::GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ..
            })
        ));

        let mut mismatched = record.clone();
        mismatched.session_id = "session-456".to_string();
        let mismatched_plaintext =
            serde_json::to_string(&mismatched).expect("mismatched session should serialize");
        let mismatched_sealed = crate::handlers::shared::seal_runtime_secret_payload(
            &app,
            &provider_oauth_device_session_secret_purpose("session-123"),
            &mismatched_plaintext,
        )
        .expect("mismatched session should seal");
        assert!(matches!(
            decode_admin_provider_oauth_device_session(&admin, "session-123", &mismatched_sealed),
            Err(crate::GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ..
            })
        ));
    }

    #[test]
    fn admin_provider_oauth_batch_task_ciphertext_is_bound_to_task_id() {
        let app = AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let admin = super::AdminAppState::new(&app);
        let task = json!({
            "task_id": "task-123",
            "provider_id": "provider-1",
            "provider_type": "codex",
            "status": "processing",
        });
        let plaintext = task.to_string();
        let sealed = crate::handlers::shared::seal_runtime_secret_payload(
            &app,
            &provider_oauth_batch_task_secret_purpose("task-123"),
            &plaintext,
        )
        .expect("batch task should seal");

        assert_eq!(
            decode_admin_provider_oauth_batch_task(&admin, "task-123", &sealed)
                .expect("matching batch task should open"),
            task
        );
        assert!(decode_admin_provider_oauth_batch_task(&admin, "task-456", &sealed).is_err());

        let mismatched = json!({
            "task_id": "task-456",
            "provider_id": "provider-1",
            "status": "processing",
        });
        let mismatched_sealed = crate::handlers::shared::seal_runtime_secret_payload(
            &app,
            &provider_oauth_batch_task_secret_purpose("task-123"),
            &mismatched.to_string(),
        )
        .expect("mismatched batch task should seal");
        assert!(
            decode_admin_provider_oauth_batch_task(&admin, "task-123", &mismatched_sealed).is_err()
        );
    }

    #[test]
    fn kiro_oidc_error_projection_discards_upstream_free_text() {
        let known = project_kiro_device_oidc_response(
            StatusCode::BAD_REQUEST,
            r#"{
                "error": "authorization_pending",
                "error_description": "Bearer upstream-secret at https://internal.test"
            }"#,
        );
        assert_eq!(
            known,
            json!({"_error": true, "error": "authorization_pending"})
        );

        let unknown = project_kiro_device_oidc_response(
            StatusCode::BAD_REQUEST,
            r#"{
                "error": "Bearer-upstream-secret",
                "error_description": "https://user:password@internal.test"
            }"#,
        );
        assert_eq!(unknown, json!({"_error": true, "error": "upstream_error"}));

        let non_json = project_kiro_device_oidc_response(
            StatusCode::BAD_GATEWAY,
            "Bearer upstream-secret at https://internal.test",
        );
        assert_eq!(non_json, json!({"_error": true, "error": "upstream_error"}));

        let exposed = format!("{known}{unknown}{non_json}");
        for secret in ["upstream-secret", "password", "internal.test", "Bearer"] {
            assert!(!exposed.contains(secret));
        }
    }
}
