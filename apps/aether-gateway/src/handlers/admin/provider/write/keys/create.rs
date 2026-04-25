use crate::handlers::admin::provider::shared::payloads::AdminProviderKeyCreateRequest;
use crate::handlers::admin::provider::write::normalize::{
    normalize_auth_type, validate_vertex_api_formats,
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::{
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_fallbacks,
    normalize_json_object, normalize_string_list, parse_catalog_auth_config_json,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_transport::provider_types::provider_type_is_fixed;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) async fn build_admin_create_provider_key_record(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    payload: AdminProviderKeyCreateRequest,
) -> Result<StoredProviderCatalogKey, String> {
    let state = state.as_ref();
    let name = payload.name.trim();
    if name.is_empty() {
        return Err("name 为必填字段".to_string());
    }

    let api_formats = normalize_string_list(payload.api_formats)
        .ok_or_else(|| "api_formats 为必填字段".to_string())?;
    let auth_type = normalize_auth_type(payload.auth_type.as_deref())?;
    validate_vertex_api_formats(&provider.provider_type, &auth_type, &api_formats)?;

    let api_key = payload.api_key.unwrap_or_default().trim().to_string();
    let auth_config = normalize_json_object(payload.auth_config, "auth_config")?;
    let auth_config_object = auth_config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned();

    match auth_type.as_str() {
        "api_key" if api_key.is_empty() => {
            return Err("API Key 认证模式下 api_key 为必填字段".to_string());
        }
        "service_account" if auth_config_object.is_none() => {
            return Err("Service Account 认证模式下 auth_config 为必填字段".to_string());
        }
        "oauth" if !api_key.is_empty() => {
            return Err("OAuth 认证模式下不允许直接填写 api_key".to_string());
        }
        _ => {}
    }

    let existing_keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .map_err(|err| format!("{err:?}"))?;

    if auth_type == "api_key" {
        for existing in existing_keys
            .iter()
            .filter(|existing| existing.auth_type.trim().eq_ignore_ascii_case("api_key"))
        {
            let Some(decrypted) = decrypt_catalog_secret_with_fallbacks(
                state.encryption_key(),
                &existing.encrypted_api_key,
            ) else {
                continue;
            };
            if decrypted != "__placeholder__" && decrypted == api_key {
                return Err(format!(
                    "该 API Key 已存在于当前 Provider 中（名称: {}）",
                    existing.name
                ));
            }
        }
    }

    if auth_type == "service_account" {
        let new_client_email = auth_config_object
            .as_ref()
            .and_then(|config| config.get("client_email"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        if let Some(new_client_email) = new_client_email {
            for existing in existing_keys.iter().filter(|existing| {
                matches!(
                    existing.auth_type.trim().to_ascii_lowercase().as_str(),
                    "service_account" | "vertex_ai"
                )
            }) {
                let Some(existing_config) = parse_catalog_auth_config_json(state, existing) else {
                    continue;
                };
                let Some(existing_email) = existing_config
                    .get("client_email")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                if existing_email == new_client_email {
                    return Err(format!(
                        "该 Service Account ({new_client_email}) 已存在于当前 Provider 中（名称: {}）",
                        existing.name
                    ));
                }
            }
        }
    }

    let encrypted_api_key = match auth_type.as_str() {
        "api_key" => encrypt_catalog_secret_with_fallbacks(state, &api_key),
        _ => encrypt_catalog_secret_with_fallbacks(state, "__placeholder__"),
    }
    .ok_or_else(|| "gateway 未配置 provider key 加密密钥".to_string())?;

    let encrypted_auth_config = auth_config
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| err.to_string())?
        .and_then(|plaintext| encrypt_catalog_secret_with_fallbacks(state, &plaintext));

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let inherits_provider_api_formats =
        auth_type == "oauth" && provider_type_is_fixed(&provider.provider_type);
    let mut key = StoredProviderCatalogKey::new(
        Uuid::new_v4().to_string(),
        provider.id.clone(),
        name.to_string(),
        auth_type,
        normalize_json_object(payload.capabilities, "capabilities")?,
        true,
    )
    .map_err(|err| err.to_string())?
    .with_transport_fields(
        if inherits_provider_api_formats {
            None
        } else {
            Some(json!(api_formats))
        },
        encrypted_api_key,
        encrypted_auth_config,
        normalize_json_object(payload.rate_multipliers, "rate_multipliers")?,
        None,
        normalize_string_list(payload.allowed_models).map(|value| json!(value)),
        None,
        None,
        None,
    )
    .map_err(|err| err.to_string())?;
    key.note = payload
        .note
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    key.internal_priority = payload.internal_priority.unwrap_or(50);
    key.rpm_limit = payload.rpm_limit;
    key.cache_ttl_minutes = payload.cache_ttl_minutes.unwrap_or(5);
    key.max_probe_interval_minutes = payload.max_probe_interval_minutes.unwrap_or(32);
    key.request_count = Some(0);
    key.success_count = Some(0);
    key.error_count = Some(0);
    key.total_response_time_ms = Some(0);
    key.auto_fetch_models = payload.auto_fetch_models.unwrap_or(false);
    key.locked_models = normalize_string_list(payload.locked_models).map(|value| json!(value));
    key.model_include_patterns =
        normalize_string_list(payload.model_include_patterns).map(|value| json!(value));
    key.model_exclude_patterns =
        normalize_string_list(payload.model_exclude_patterns).map(|value| json!(value));
    key.health_by_format = Some(json!({}));
    key.circuit_breaker_by_format = Some(json!({}));
    key.created_at_unix_ms = Some(now_unix_secs);
    key.updated_at_unix_secs = Some(now_unix_secs);
    Ok(key)
}
