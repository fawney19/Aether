use crate::handlers::admin::provider::shared::payloads::AdminProviderKeyUpdatePatch;
use crate::handlers::admin::provider::write::normalize::{
    normalize_auth_type, validate_vertex_api_formats,
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::{
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_fallbacks, json_string_list,
    normalize_json_object, normalize_string_list, parse_catalog_auth_config_json,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn build_admin_update_provider_key_record(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    existing: &StoredProviderCatalogKey,
    patch: AdminProviderKeyUpdatePatch,
) -> Result<StoredProviderCatalogKey, String> {
    let state = state.as_ref();
    let mut updated = existing.clone();
    let (fields, payload) = patch.into_parts();
    let auto_fetch_disabled =
        existing.auto_fetch_models && matches!(payload.auto_fetch_models, Some(false));
    let current_auth_type = normalize_auth_type(Some(&existing.auth_type))?;
    let target_auth_type = payload
        .auth_type
        .as_deref()
        .map(|value| normalize_auth_type(Some(value)))
        .transpose()?
        .unwrap_or_else(|| current_auth_type.clone());
    let auth_type_switch = payload
        .auth_type
        .as_deref()
        .is_some_and(|_| target_auth_type != current_auth_type);

    let api_key_present = fields.contains("api_key");
    let api_key_value = payload
        .api_key
        .as_deref()
        .map(str::trim)
        .map(ToOwned::to_owned);
    if api_key_present && api_key_value.as_deref() == Some("") {
        return Err("api_key 不能为空".to_string());
    }

    let auth_config_present = fields.contains("auth_config");
    let auth_config = normalize_json_object(payload.auth_config, "auth_config")?;
    let auth_config_object = auth_config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned();

    let existing_keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .map_err(|err| format!("{err:?}"))?;

    match target_auth_type.as_str() {
        "api_key" => {
            if auth_type_switch
                && matches!(
                    api_key_value.as_deref(),
                    None | Some("") | Some("__placeholder__")
                )
            {
                return Err("切换到 API Key 认证模式时，必须提供新的 API Key".to_string());
            }
            if api_key_present
                && matches!(
                    api_key_value.as_deref(),
                    None | Some("") | Some("__placeholder__")
                )
            {
                return Err("API Key 认证模式下 api_key 不能为空".to_string());
            }
            if let Some(api_key) = api_key_value.as_deref() {
                for existing_key in existing_keys.iter().filter(|key| {
                    key.id != existing.id && key.auth_type.trim().eq_ignore_ascii_case("api_key")
                }) {
                    let Some(decrypted) = decrypt_catalog_secret_with_fallbacks(
                        state.encryption_key(),
                        &existing_key.encrypted_api_key,
                    ) else {
                        continue;
                    };
                    if decrypted != "__placeholder__" && decrypted == api_key {
                        return Err(format!(
                            "该 API Key 已存在于当前 Provider 中（名称: {}）",
                            existing_key.name
                        ));
                    }
                }
                updated.encrypted_api_key =
                    encrypt_catalog_secret_with_fallbacks(state, api_key)
                        .ok_or_else(|| "gateway 未配置 provider key 加密密钥".to_string())?;
            }
            updated.encrypted_auth_config = None;
        }
        "service_account" => {
            if auth_type_switch && auth_config_object.is_none() {
                return Err(
                    "切换到 Service Account 认证模式时，必须提供 Service Account JSON".to_string(),
                );
            }
            if api_key_present
                && !matches!(
                    api_key_value.as_deref(),
                    None | Some("") | Some("__placeholder__")
                )
            {
                return Err("Service Account 认证模式下不允许直接填写 api_key".to_string());
            }
            if auth_type_switch || api_key_present {
                updated.encrypted_api_key =
                    encrypt_catalog_secret_with_fallbacks(state, "__placeholder__")
                        .ok_or_else(|| "gateway 未配置 provider key 加密密钥".to_string())?;
            }
            if let Some(client_email) = auth_config_object
                .as_ref()
                .and_then(|config| config.get("client_email"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                for existing_key in existing_keys.iter().filter(|key| {
                    key.id != existing.id
                        && matches!(
                            key.auth_type.trim().to_ascii_lowercase().as_str(),
                            "service_account" | "vertex_ai"
                        )
                }) {
                    let Some(existing_config) = parse_catalog_auth_config_json(state, existing_key)
                    else {
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
                    if existing_email == client_email {
                        return Err(format!(
                            "该 Service Account ({client_email}) 已存在于当前 Provider 中（名称: {}）",
                            existing_key.name
                        ));
                    }
                }
            }
            if auth_config_present {
                updated.encrypted_auth_config = auth_config
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|err| err.to_string())?
                    .map(|plaintext| {
                        encrypt_catalog_secret_with_fallbacks(state, &plaintext)
                            .ok_or_else(|| "gateway 未配置 provider key 加密密钥".to_string())
                    })
                    .transpose()?;
            }
        }
        "oauth" => {
            if api_key_present
                && !matches!(
                    api_key_value.as_deref(),
                    None | Some("") | Some("__placeholder__")
                )
            {
                return Err("OAuth 认证模式下不允许直接填写 api_key".to_string());
            }
            if auth_type_switch {
                updated.encrypted_api_key =
                    encrypt_catalog_secret_with_fallbacks(state, "__placeholder__")
                        .ok_or_else(|| "gateway 未配置 provider key 加密密钥".to_string())?;
                updated.encrypted_auth_config = None;
            }
        }
        _ => {}
    }

    if fields.contains("api_formats") {
        let api_formats = normalize_string_list(payload.api_formats)
            .ok_or_else(|| "api_formats 为必填字段".to_string())?;
        validate_vertex_api_formats(&provider.provider_type, &target_auth_type, &api_formats)?;
        updated.api_formats = Some(json!(api_formats));
    } else if payload.auth_type.is_some() {
        let api_formats = json_string_list(existing.api_formats.as_ref());
        validate_vertex_api_formats(&provider.provider_type, &target_auth_type, &api_formats)?;
    }

    updated.auth_type = target_auth_type;

    if let Some(name) = payload.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("name 为必填字段".to_string());
        }
        updated.name = trimmed.to_string();
    }
    if fields.contains("rate_multipliers") {
        updated.rate_multipliers =
            normalize_json_object(payload.rate_multipliers, "rate_multipliers")?;
    }
    if let Some(internal_priority) = payload.internal_priority {
        updated.internal_priority = internal_priority;
    }
    if fields.contains("global_priority_by_format") {
        updated.global_priority_by_format = normalize_json_object(
            payload.global_priority_by_format,
            "global_priority_by_format",
        )?;
    }
    if fields.contains("rpm_limit") {
        updated.rpm_limit = payload.rpm_limit;
        if payload.rpm_limit.is_none() {
            updated.learned_rpm_limit = None;
        }
    }
    if fields.contains("allowed_models") {
        updated.allowed_models =
            normalize_string_list(payload.allowed_models).map(|value| json!(value));
    }
    if fields.contains("capabilities") {
        updated.capabilities = normalize_json_object(payload.capabilities, "capabilities")?;
    }
    if let Some(cache_ttl_minutes) = payload.cache_ttl_minutes {
        updated.cache_ttl_minutes = cache_ttl_minutes;
    }
    if let Some(max_probe_interval_minutes) = payload.max_probe_interval_minutes {
        updated.max_probe_interval_minutes = max_probe_interval_minutes;
    }
    if let Some(is_active) = payload.is_active {
        updated.is_active = is_active;
    }
    if fields.contains("note") {
        updated.note = payload
            .note
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    if let Some(auto_fetch_models) = payload.auto_fetch_models {
        updated.auto_fetch_models = auto_fetch_models;
    }
    if auto_fetch_disabled {
        updated.allowed_models = None;
    }
    if fields.contains("locked_models") {
        updated.locked_models =
            normalize_string_list(payload.locked_models).map(|value| json!(value));
    }
    if fields.contains("model_include_patterns") {
        updated.model_include_patterns =
            normalize_string_list(payload.model_include_patterns).map(|value| json!(value));
    }
    if fields.contains("model_exclude_patterns") {
        updated.model_exclude_patterns =
            normalize_string_list(payload.model_exclude_patterns).map(|value| json!(value));
    }
    if fields.contains("proxy") {
        updated.proxy = normalize_json_object(payload.proxy, "proxy")?;
    }
    if fields.contains("fingerprint") {
        updated.fingerprint = normalize_json_object(payload.fingerprint, "fingerprint")?;
    }
    if auth_config_present && !auth_type_switch && updated.auth_type != "api_key" {
        updated.encrypted_auth_config = auth_config
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|err| err.to_string())?
            .map(|plaintext| {
                encrypt_catalog_secret_with_fallbacks(state, &plaintext)
                    .ok_or_else(|| "gateway 未配置 provider key 加密密钥".to_string())
            })
            .transpose()?;
    }

    updated.updated_at_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    Ok(updated)
}
