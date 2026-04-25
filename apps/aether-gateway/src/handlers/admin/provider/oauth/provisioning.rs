use super::state::{
    enrich_admin_provider_oauth_auth_config, json_non_empty_string, json_u64_value,
};
use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_active_api_formats;
use crate::GatewayError;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_provider_transport::provider_types::provider_type_is_fixed;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) fn provider_oauth_key_proxy_value(
    proxy_node_id: Option<&str>,
) -> Option<serde_json::Value> {
    proxy_node_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| json!({ "node_id": value, "enabled": true }))
}

pub(crate) fn provider_oauth_active_api_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Vec<String> {
    provider_active_api_formats(endpoints)
}

pub(crate) fn build_provider_oauth_auth_config_from_token_payload(
    provider_type: &str,
    token_payload: &serde_json::Value,
) -> (
    serde_json::Map<String, serde_json::Value>,
    Option<String>,
    Option<String>,
    Option<u64>,
) {
    let access_token = json_non_empty_string(token_payload.get("access_token"));
    let refresh_token = json_non_empty_string(token_payload.get("refresh_token"));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let expires_at = json_u64_value(token_payload.get("expires_in"))
        .map(|expires_in| now_unix_secs.saturating_add(expires_in));

    let mut auth_config = serde_json::Map::new();
    auth_config.insert("provider_type".to_string(), json!(provider_type));
    auth_config.insert("updated_at".to_string(), json!(now_unix_secs));
    if let Some(token_type) = token_payload.get("token_type").cloned() {
        auth_config.insert("token_type".to_string(), token_type);
    }
    if let Some(refresh_token) = refresh_token.as_ref() {
        auth_config.insert("refresh_token".to_string(), json!(refresh_token));
    }
    if let Some(expires_at) = expires_at {
        auth_config.insert("expires_at".to_string(), json!(expires_at));
    }
    if let Some(scope) = token_payload.get("scope").cloned() {
        auth_config.insert("scope".to_string(), scope);
    }
    enrich_admin_provider_oauth_auth_config(provider_type, &mut auth_config, token_payload);
    (auth_config, access_token, refresh_token, expires_at)
}

pub(crate) async fn create_provider_oauth_catalog_key(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider_type: &str,
    name: &str,
    access_token: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    api_formats: &[String],
    proxy: Option<serde_json::Value>,
    expires_at_unix_secs: Option<u64>,
) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
    let Some(encrypted_api_key) = state.encrypt_catalog_secret_with_fallbacks(access_token) else {
        return Ok(None);
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(encrypted_auth_config) =
        state.encrypt_catalog_secret_with_fallbacks(&auth_config_json)
    else {
        return Ok(None);
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut record = StoredProviderCatalogKey::new(
        Uuid::new_v4().to_string(),
        provider_id.to_string(),
        name.to_string(),
        "oauth".to_string(),
        None,
        true,
    )
    .map_err(|err| GatewayError::Internal(err.to_string()))?
    .with_transport_fields(
        provider_oauth_catalog_key_api_formats(provider_type, api_formats),
        encrypted_api_key,
        Some(encrypted_auth_config),
        None,
        None,
        None,
        expires_at_unix_secs,
        proxy,
        None,
    )
    .map_err(|err| GatewayError::Internal(err.to_string()))?;
    record.internal_priority = 50;
    record.cache_ttl_minutes = 5;
    record.max_probe_interval_minutes = 32;
    record.request_count = Some(0);
    record.success_count = Some(0);
    record.error_count = Some(0);
    record.total_response_time_ms = Some(0);
    record.health_by_format = Some(json!({}));
    record.circuit_breaker_by_format = Some(json!({}));
    record.created_at_unix_ms = Some(now_unix_secs);
    record.updated_at_unix_secs = Some(now_unix_secs);
    state.create_provider_catalog_key(&record).await
}

pub(crate) async fn update_existing_provider_oauth_catalog_key(
    state: &AdminAppState<'_>,
    existing_key: &StoredProviderCatalogKey,
    provider_type: &str,
    access_token: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    api_formats: &[String],
    proxy: Option<serde_json::Value>,
    expires_at_unix_secs: Option<u64>,
) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
    let Some(encrypted_api_key) = state.encrypt_catalog_secret_with_fallbacks(access_token) else {
        return Ok(None);
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(encrypted_auth_config) =
        state.encrypt_catalog_secret_with_fallbacks(&auth_config_json)
    else {
        return Ok(None);
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut updated = existing_key.clone();
    updated.encrypted_api_key = encrypted_api_key;
    updated.encrypted_auth_config = Some(encrypted_auth_config);
    updated.api_formats = provider_oauth_catalog_key_api_formats(provider_type, api_formats);
    updated.is_active = true;
    updated.expires_at_unix_secs = expires_at_unix_secs;
    updated.oauth_invalid_at_unix_secs = None;
    updated.oauth_invalid_reason = None;
    updated.health_by_format = Some(json!({}));
    updated.circuit_breaker_by_format = Some(json!({}));
    updated.error_count = Some(0);
    if let Some(proxy) = proxy {
        updated.proxy = Some(proxy);
    }
    updated.updated_at_unix_secs = Some(now_unix_secs);
    state.update_provider_catalog_key(&updated).await
}

fn provider_oauth_catalog_key_api_formats(
    provider_type: &str,
    api_formats: &[String],
) -> Option<serde_json::Value> {
    if provider_type_is_fixed(provider_type) {
        None
    } else {
        Some(json!(api_formats))
    }
}
