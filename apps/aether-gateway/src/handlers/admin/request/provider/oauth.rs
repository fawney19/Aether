use super::*;
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use aether_data::repository::provider_oauth::{
    build_provider_oauth_batch_task_status_payload, provider_oauth_batch_task_storage_key,
    provider_oauth_device_session_storage_key, provider_oauth_state_storage_key,
    StoredAdminProviderOAuthDeviceSession, StoredAdminProviderOAuthState,
    PROVIDER_OAUTH_BATCH_TASK_TTL_SECS, PROVIDER_OAUTH_STATE_TTL_SECS,
};
use axum::http;
use serde_json::json;
use url::Url;

const KIRO_IDC_AMZ_USER_AGENT: &str =
    "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";

impl<'a> AdminAppState<'a> {
    pub(crate) async fn update_provider_catalog_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, GatewayError> {
        self.app
            .update_provider_catalog_key_oauth_credentials(
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
        self.app
            .clear_provider_catalog_key_oauth_invalid_marker(key_id)
            .await
    }

    pub(crate) async fn force_local_oauth_refresh_entry(
        &self,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<crate::provider_transport::CachedOAuthEntry>, AdminLocalOAuthRefreshError>
    {
        self.app.force_local_oauth_refresh_entry(transport).await
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
        if let Some(runner) = self.redis_kv_runner() {
            runner
                .setex(&key, &value, Some(PROVIDER_OAUTH_STATE_TTL_SECS))
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            return Ok(nonce);
        }
        if self
            .as_ref()
            .save_provider_oauth_state_for_tests(&key, &value)
        {
            return Ok(nonce);
        }
        Err(GatewayError::Internal(
            "provider oauth redis unavailable".to_string(),
        ))
    }

    pub(crate) async fn consume_provider_oauth_state(
        &self,
        nonce: &str,
    ) -> Result<Option<StoredAdminProviderOAuthState>, GatewayError> {
        let key = provider_oauth_state_storage_key(nonce);
        let raw = if let Some(runner) = self.redis_kv_runner() {
            let mut connection = runner
                .client()
                .get_multiplexed_async_connection()
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            let namespaced_key = runner.keyspace().key(&key);
            redis::cmd("GETDEL")
                .arg(&namespaced_key)
                .query_async::<Option<String>>(&mut connection)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
        } else {
            self.as_ref().take_provider_oauth_state_for_tests(&key)
        };
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
    ) -> Result<serde_json::Value, Response<Body>> {
        crate::handlers::admin::provider::oauth::state::exchange_admin_provider_oauth_code(
            self,
            template,
            code,
            state_nonce,
            pkce_verifier,
        )
        .await
    }

    pub(crate) async fn exchange_admin_provider_oauth_refresh_token(
        &self,
        template: AdminProviderOAuthTemplate,
        refresh_token: &str,
    ) -> Result<serde_json::Value, Response<Body>> {
        crate::handlers::admin::provider::oauth::state::exchange_admin_provider_oauth_refresh_token(
            self,
            template,
            refresh_token,
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

        if let Some(runner) = self.redis_kv_runner() {
            let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await
            else {
                return Err(GatewayError::Internal(
                    "provider oauth batch task redis unavailable".to_string(),
                ));
            };
            let redis_key = runner.keyspace().key(&key);
            redis::cmd("SET")
                .arg(redis_key)
                .arg(&serialized)
                .arg("EX")
                .arg(PROVIDER_OAUTH_BATCH_TASK_TTL_SECS)
                .query_async::<()>(&mut connection)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            return Ok(());
        }

        if self
            .as_ref()
            .save_provider_oauth_batch_task_for_tests(&key, &serialized)
        {
            return Ok(());
        }

        Err(GatewayError::Internal(
            "provider oauth batch task redis unavailable".to_string(),
        ))
    }

    pub(crate) async fn read_provider_oauth_batch_task_payload(
        &self,
        provider_id: &str,
        task_id: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let key = provider_oauth_batch_task_storage_key(task_id);
        let raw = if let Some(runner) = self.redis_kv_runner() {
            let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await
            else {
                return Err(GatewayError::Internal(
                    "provider oauth batch task redis unavailable".to_string(),
                ));
            };
            let redis_key = runner.keyspace().key(&key);
            redis::cmd("GET")
                .arg(redis_key)
                .query_async(&mut connection)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
        } else {
            self.as_ref().load_provider_oauth_batch_task_for_tests(&key)
        };
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
        if let Some(runner) = self.redis_kv_runner() {
            runner
                .setex(&key, &value, Some(ttl_seconds))
                .await
                .map_err(|_| {
                    build_internal_control_error_response(
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        "provider oauth redis unavailable",
                    )
                })?;
            return Ok(());
        }
        if self
            .as_ref()
            .save_provider_oauth_device_session_for_tests(&key, &value)
        {
            return Ok(());
        }
        Err(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth redis unavailable",
        ))
    }

    pub(crate) async fn read_provider_oauth_device_session(
        &self,
        session_id: &str,
    ) -> Result<Option<StoredAdminProviderOAuthDeviceSession>, GatewayError> {
        let key = provider_oauth_device_session_storage_key(session_id);
        let raw = if let Some(runner) = self.redis_kv_runner() {
            let mut connection = runner
                .client()
                .get_multiplexed_async_connection()
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            let namespaced_key = runner.keyspace().key(&key);
            redis::cmd("GET")
                .arg(&namespaced_key)
                .query_async::<Option<String>>(&mut connection)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
        } else {
            self.as_ref()
                .load_provider_oauth_device_session_for_tests(&key)
        };
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
        crate::handlers::admin::provider::oauth::duplicates::find_duplicate_provider_oauth_key(
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
        crate::handlers::admin::provider::oauth::provisioning::create_provider_oauth_catalog_key(
            self,
            provider_id,
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
        access_token: &str,
        auth_config: &serde_json::Map<String, serde_json::Value>,
        proxy: Option<serde_json::Value>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        crate::handlers::admin::provider::oauth::provisioning::update_existing_provider_oauth_catalog_key(
            self,
            existing_key,
            access_token,
            auth_config,
            proxy,
            expires_at_unix_secs,
        )
        .await
    }

    pub(crate) async fn refresh_provider_oauth_account_state_after_update(
        &self,
        provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        key_id: &str,
    ) -> Result<(bool, Option<String>), GatewayError> {
        crate::handlers::admin::provider::oauth::runtime::refresh_provider_oauth_account_state_after_update(
            self,
            provider,
            key_id,
        )
        .await
    }
}

async fn post_kiro_device_oidc_json(
    state: &AdminAppState<'_>,
    endpoint_key: &str,
    default_url: String,
    body: serde_json::Value,
) -> Result<serde_json::Value, Response<Body>> {
    let url = state.provider_oauth_token_url(endpoint_key, &default_url);
    let host = Url::parse(&url)
        .ok()
        .and_then(|value| value.host_str().map(ToOwned::to_owned))
        .unwrap_or_default();
    let response = state
        .http_client()
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "*/*")
        .header("User-Agent", "node")
        .header("x-amz-user-agent", KIRO_IDC_AMZ_USER_AGENT)
        .header("Host", host)
        .json(&body)
        .send()
        .await
        .map_err(|_| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "发起设备授权失败: unknown",
            )
        })?;
    let status = response.status();
    let body_text = response.text().await.map_err(|_| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "发起设备授权失败: unknown",
        )
    })?;
    Ok(
        serde_json::from_str::<serde_json::Value>(&body_text).unwrap_or_else(|_| {
            json!({
                "_error": !status.is_success(),
                "error": body_text.trim(),
            })
        }),
    )
}
