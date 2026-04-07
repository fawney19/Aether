use super::normalize::{normalize_auth_type, validate_vertex_api_formats};
use crate::handlers::admin::provider::shared::payloads::{
    AdminProviderKeyCreateRequest, AdminProviderKeyUpdateRequest,
};
use crate::handlers::admin::shared::{
    build_admin_provider_key_response, decrypt_catalog_secret_with_fallbacks,
    encrypt_catalog_secret_with_fallbacks, json_string_list, normalize_json_object,
    normalize_string_list, parse_catalog_auth_config_json,
};
use crate::AppState;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) async fn build_admin_create_provider_key_record(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    payload: AdminProviderKeyCreateRequest,
) -> Result<StoredProviderCatalogKey, String> {
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
        "api_key" => {
            if api_key.is_empty() {
                return Err("API Key 认证模式下 api_key 为必填字段".to_string());
            }
        }
        "service_account" => {
            if auth_config_object.is_none() {
                return Err("Service Account 认证模式下 auth_config 为必填字段".to_string());
            }
        }
        "oauth" => {
            if !api_key.is_empty() {
                return Err("OAuth 认证模式下不允许直接填写 api_key".to_string());
            }
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
        Some(json!(api_formats)),
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
    key.created_at_unix_secs = Some(now_unix_secs);
    key.updated_at_unix_secs = Some(now_unix_secs);
    Ok(key)
}

pub(crate) async fn build_admin_update_provider_key_record(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    existing: &StoredProviderCatalogKey,
    raw_payload: &serde_json::Map<String, serde_json::Value>,
    payload: AdminProviderKeyUpdateRequest,
) -> Result<StoredProviderCatalogKey, String> {
    let mut updated = existing.clone();
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

    let api_key_present = raw_payload.contains_key("api_key");
    let api_key_value = payload
        .api_key
        .as_deref()
        .map(str::trim)
        .map(ToOwned::to_owned);
    if api_key_present && api_key_value.as_deref() == Some("") {
        return Err("api_key 不能为空".to_string());
    }

    let auth_config_present = raw_payload.contains_key("auth_config");
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

    if raw_payload.contains_key("api_formats") {
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
    if raw_payload.contains_key("rate_multipliers") {
        updated.rate_multipliers =
            normalize_json_object(payload.rate_multipliers, "rate_multipliers")?;
    }
    if let Some(internal_priority) = payload.internal_priority {
        updated.internal_priority = internal_priority;
    }
    if raw_payload.contains_key("global_priority_by_format") {
        updated.global_priority_by_format = normalize_json_object(
            payload.global_priority_by_format,
            "global_priority_by_format",
        )?;
    }
    if raw_payload.contains_key("rpm_limit") {
        updated.rpm_limit = payload.rpm_limit;
        if payload.rpm_limit.is_none() {
            updated.learned_rpm_limit = None;
        }
    }
    if raw_payload.contains_key("allowed_models") {
        updated.allowed_models =
            normalize_string_list(payload.allowed_models).map(|value| json!(value));
    }
    if raw_payload.contains_key("capabilities") {
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
    if raw_payload.contains_key("note") {
        updated.note = payload
            .note
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    if let Some(auto_fetch_models) = payload.auto_fetch_models {
        updated.auto_fetch_models = auto_fetch_models;
    }
    if raw_payload.contains_key("locked_models") {
        updated.locked_models =
            normalize_string_list(payload.locked_models).map(|value| json!(value));
    }
    if raw_payload.contains_key("model_include_patterns") {
        updated.model_include_patterns =
            normalize_string_list(payload.model_include_patterns).map(|value| json!(value));
    }
    if raw_payload.contains_key("model_exclude_patterns") {
        updated.model_exclude_patterns =
            normalize_string_list(payload.model_exclude_patterns).map(|value| json!(value));
    }
    if raw_payload.contains_key("proxy") {
        updated.proxy = normalize_json_object(payload.proxy, "proxy")?;
    }
    if raw_payload.contains_key("fingerprint") {
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

pub(crate) async fn build_admin_provider_keys_payload(
    state: &AppState,
    provider_id: &str,
    skip: usize,
    limit: usize,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }
    let provider = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await
        .ok()
        .and_then(|mut providers| providers.drain(..).next())?;
    let mut keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .ok()
        .unwrap_or_default();
    keys.sort_by(|left, right| {
        left.internal_priority
            .cmp(&right.internal_priority)
            .then_with(|| {
                left.created_at_unix_secs
                    .unwrap_or_default()
                    .cmp(&right.created_at_unix_secs.unwrap_or_default())
            })
            .then_with(|| left.id.cmp(&right.id))
    });
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Some(serde_json::Value::Array(
        keys.into_iter()
            .skip(skip)
            .take(limit)
            .map(|key| build_admin_provider_key_response(state, &key, now_unix_secs))
            .collect(),
    ))
}
