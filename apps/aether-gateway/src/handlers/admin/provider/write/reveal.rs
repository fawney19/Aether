fn normalize_reveal_auth_type(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "service_account" | "vertex_ai" => "service_account",
        "oauth" => "oauth",
        _ => "api_key",
    }
}

use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::parse_catalog_auth_config_json;
use crate::provider_key_auth::provider_key_auth_semantics;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use chrono::{SecondsFormat, Utc};
use serde_json::json;

fn reveal_provider_type_from_auth_config(
    auth_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> String {
    auth_config
        .and_then(|value| value.get("provider_type"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn build_admin_reveal_key_payload(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
) -> Result<serde_json::Value, String> {
    let parsed_auth_config = state.parse_catalog_auth_config_json(key);
    let provider_type = reveal_provider_type_from_auth_config(parsed_auth_config.as_ref());
    let auth_semantics = provider_key_auth_semantics(key, provider_type.as_str());
    let auth_type = if auth_semantics.oauth_managed() {
        "oauth"
    } else {
        normalize_reveal_auth_type(&key.auth_type)
    };
    if matches!(auth_type, "service_account") {
        if let Some(auth_config) = parsed_auth_config {
            return Ok(json!({
                "auth_type": auth_type,
                "auth_config": auth_config,
            }));
        }
        let decrypted = state
            .decrypt_catalog_secret_with_fallbacks(&key.encrypted_api_key)
            .ok_or_else(|| {
                "无法解密认证配置，可能是加密密钥已更改。请重新添加该密钥。".to_string()
            })?;
        if decrypted == "__placeholder__" {
            return Err("认证配置丢失，请重新添加该密钥。".to_string());
        }
        return Ok(json!({
            "auth_type": auth_type,
            "auth_config": decrypted,
        }));
    }

    let decrypted = state
        .decrypt_catalog_secret_with_fallbacks(&key.encrypted_api_key)
        .ok_or_else(|| "无法解密 API Key，可能是加密密钥已更改。请重新添加该密钥。".to_string())?;
    Ok(json!({
        "auth_type": auth_type,
        "api_key": decrypted,
    }))
}

fn provider_oauth_export_payload(
    provider_type: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    upstream_metadata: Option<&serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    let skip_keys: &[&str] = match normalized_provider_type.as_str() {
        "kiro" => &["access_token", "expires_at", "updated_at"],
        _ => &[
            "access_token",
            "expires_at",
            "updated_at",
            "token_type",
            "scope",
        ],
    };
    let mut payload = serde_json::Map::new();
    for (key, value) in auth_config {
        if skip_keys.contains(&key.as_str()) {
            continue;
        }
        if value.is_null() || value.as_str().is_some_and(str::is_empty) {
            continue;
        }
        payload.insert(key.clone(), value.clone());
    }
    if normalized_provider_type == "kiro" && !payload.contains_key("email") {
        if let Some(email) = upstream_metadata
            .and_then(serde_json::Value::as_object)
            .and_then(|meta| meta.get("kiro"))
            .and_then(serde_json::Value::as_object)
            .and_then(|meta| meta.get("email"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            payload.insert("email".to_string(), json!(email));
        }
    }
    payload
}

pub(crate) async fn build_admin_export_key_payload(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
) -> Result<serde_json::Value, String> {
    let ciphertext = key
        .encrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "缺少认证配置，无法导出".to_string())?;
    let plaintext = state
        .decrypt_catalog_secret_with_fallbacks(ciphertext)
        .ok_or_else(|| "无法解密认证配置".to_string())?;
    let auth_config = serde_json::from_str::<serde_json::Value>(&plaintext)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "无法解密认证配置".to_string())?;
    if !auth_config
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err("缺少 refresh_token，无法导出".to_string());
    }

    let provider_type_from_config = auth_config
        .get("provider_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let provider_type = if let Some(provider_type) = provider_type_from_config {
        provider_type
    } else {
        state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
            .await
            .map_err(|err| format!("{err:?}"))?
            .into_iter()
            .next()
            .map(|provider| provider.provider_type)
            .unwrap_or_default()
    };
    if !provider_key_auth_semantics(key, provider_type.as_str()).can_export_oauth() {
        return Err("仅 OAuth 管理账号支持导出".to_string());
    }

    let mut payload =
        provider_oauth_export_payload(&provider_type, &auth_config, key.upstream_metadata.as_ref());
    payload.insert("name".to_string(), json!(key.name));
    payload.insert(
        "exported_at".to_string(),
        json!(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    Ok(serde_json::Value::Object(payload))
}
