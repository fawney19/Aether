use super::support::{AdminProviderOpsSaveConfigRequest, ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS};
use crate::handlers::admin::shared::{
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_fallbacks,
};
use crate::AppState;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use serde_json::json;

pub(super) fn admin_provider_ops_config_object(
    provider: &StoredProviderCatalogProvider,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(serde_json::Value::as_object)
}

pub(super) fn admin_provider_ops_connector_object(
    provider_ops_config: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    provider_ops_config
        .get("connector")
        .and_then(serde_json::Value::as_object)
}

fn admin_provider_ops_masked_secret(
    state: &AppState,
    field: &str,
    ciphertext: &str,
) -> serde_json::Value {
    let plaintext = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
        .unwrap_or_else(|| ciphertext.to_string());
    if plaintext.is_empty() {
        return serde_json::Value::String(String::new());
    }

    let masked = if field == "password" {
        "********".to_string()
    } else if plaintext.len() > 12 {
        format!(
            "{}****{}",
            &plaintext[..4],
            &plaintext[plaintext.len().saturating_sub(4)..]
        )
    } else if plaintext.len() > 8 {
        format!(
            "{}****{}",
            &plaintext[..2],
            &plaintext[plaintext.len().saturating_sub(2)..]
        )
    } else {
        "*".repeat(plaintext.len())
    };

    serde_json::Value::String(masked)
}

fn admin_provider_ops_masked_credentials(
    state: &AppState,
    raw_credentials: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(credentials) = raw_credentials.and_then(serde_json::Value::as_object) else {
        return json!({});
    };

    let mut masked = serde_json::Map::new();
    for (key, value) in credentials {
        if ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS.contains(&key.as_str()) {
            if let Some(ciphertext) = value.as_str().filter(|value| !value.is_empty()) {
                masked.insert(
                    key.clone(),
                    admin_provider_ops_masked_secret(state, key, ciphertext),
                );
                continue;
            }
        }
        masked.insert(key.clone(), value.clone());
    }
    serde_json::Value::Object(masked)
}

fn admin_provider_ops_is_supported_auth_type(auth_type: &str) -> bool {
    matches!(
        auth_type,
        "api_key" | "session_login" | "oauth" | "cookie" | "none"
    )
}

pub(super) fn admin_provider_ops_uses_python_verify_fallback(
    architecture_id: &str,
    config: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let _ = architecture_id;
    config
        .get("proxy_enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || config
            .get("proxy_node_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

pub(super) fn admin_provider_ops_decrypted_credentials(
    state: &AppState,
    raw_credentials: Option<&serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let Some(credentials) = raw_credentials.and_then(serde_json::Value::as_object) else {
        return serde_json::Map::new();
    };

    let mut decrypted = serde_json::Map::new();
    for (key, value) in credentials {
        if ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS.contains(&key.as_str()) {
            if let Some(ciphertext) = value.as_str() {
                let plaintext =
                    decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
                        .unwrap_or_else(|| ciphertext.to_string());
                decrypted.insert(key.clone(), serde_json::Value::String(plaintext));
                continue;
            }
        }
        decrypted.insert(key.clone(), value.clone());
    }
    decrypted
}

fn admin_provider_ops_sensitive_placeholder_or_empty(value: Option<&serde_json::Value>) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(raw)) => raw.is_empty() || raw.chars().all(|ch| ch == '*'),
        Some(serde_json::Value::Array(items)) => items.is_empty(),
        Some(serde_json::Value::Object(map)) => map.is_empty(),
        _ => false,
    }
}

pub(super) fn admin_provider_ops_merge_credentials(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    mut request_credentials: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let saved_credentials = admin_provider_ops_decrypted_credentials(
        state,
        admin_provider_ops_config_object(provider)
            .and_then(admin_provider_ops_connector_object)
            .and_then(|connector| connector.get("credentials")),
    );

    for field in ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS {
        if admin_provider_ops_sensitive_placeholder_or_empty(request_credentials.get(*field))
            && saved_credentials.contains_key(*field)
        {
            if let Some(saved_value) = saved_credentials.get(*field) {
                request_credentials.insert((*field).to_string(), saved_value.clone());
            }
        }
    }

    for (key, value) in saved_credentials {
        if key.starts_with('_') && !request_credentials.contains_key(&key) {
            request_credentials.insert(key, value);
        }
    }

    request_credentials
}

fn admin_provider_ops_encrypt_credentials(
    state: &AppState,
    credentials: serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut encrypted = serde_json::Map::new();
    for (key, value) in credentials {
        if ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS.contains(&key.as_str()) {
            if let Some(plaintext) = value.as_str() {
                if plaintext.is_empty() {
                    encrypted.insert(key, value);
                } else {
                    let ciphertext = encrypt_catalog_secret_with_fallbacks(state, plaintext)
                        .ok_or_else(|| "gateway 未配置 Provider Ops 加密密钥".to_string())?;
                    encrypted.insert(key, serde_json::Value::String(ciphertext));
                }
                continue;
            }
        }
        encrypted.insert(key, value);
    }
    Ok(encrypted)
}

pub(super) fn build_admin_provider_ops_saved_config_value(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    payload: AdminProviderOpsSaveConfigRequest,
) -> Result<serde_json::Value, String> {
    let auth_type = payload.connector.auth_type.trim().to_string();
    if auth_type.is_empty() || !admin_provider_ops_is_supported_auth_type(auth_type.as_str()) {
        return Err("connector.auth_type 必须是合法的认证类型".to_string());
    }

    let merged_credentials =
        admin_provider_ops_merge_credentials(state, provider, payload.connector.credentials);
    let encrypted_credentials = admin_provider_ops_encrypt_credentials(state, merged_credentials)?;

    let actions = payload
        .actions
        .into_iter()
        .map(|(action_type, config)| {
            (
                action_type,
                json!({
                    "enabled": config.enabled,
                    "config": config.config,
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();

    Ok(json!({
        "architecture_id": payload.architecture_id,
        "base_url": payload.base_url,
        "connector": {
            "auth_type": auth_type,
            "config": payload.connector.config,
            "credentials": encrypted_credentials,
        },
        "actions": actions,
        "schedule": payload.schedule,
    }))
}

pub(super) fn resolve_admin_provider_ops_base_url(
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    provider_ops_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    let from_saved_config = provider_ops_config
        .and_then(|config| config.get("base_url"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if from_saved_config.is_some() {
        return from_saved_config;
    }

    if let Some(base_url) = endpoints.iter().find_map(|endpoint| {
        let value = endpoint.base_url.trim();
        (!value.is_empty()).then(|| value.to_string())
    }) {
        return Some(base_url);
    }

    let from_provider_config = provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("base_url"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if from_provider_config.is_some() {
        return from_provider_config;
    }

    provider
        .website
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn build_admin_provider_ops_status_payload(
    provider_id: &str,
    provider: Option<&StoredProviderCatalogProvider>,
) -> serde_json::Value {
    let provider_ops_config = provider.and_then(admin_provider_ops_config_object);
    let auth_type = provider_ops_config
        .and_then(admin_provider_ops_connector_object)
        .and_then(|connector| connector.get("auth_type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| {
            if provider_ops_config.is_some() {
                "api_key"
            } else {
                "none"
            }
        });
    let mut enabled_actions = provider_ops_config
        .and_then(|config| config.get("actions"))
        .and_then(serde_json::Value::as_object)
        .map(|actions| {
            actions
                .iter()
                .filter_map(|(action_type, config)| {
                    let enabled = config
                        .as_object()
                        .and_then(|config| config.get("enabled"))
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true);
                    enabled.then(|| serde_json::Value::String(action_type.clone()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    enabled_actions.sort_by(|left, right| left.as_str().cmp(&right.as_str()));

    json!({
        "provider_id": provider_id,
        "is_configured": provider_ops_config.is_some(),
        "architecture_id": provider_ops_config.map(|config| {
            config
                .get("architecture_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("generic_api")
        }),
        "connection_status": {
            "status": "disconnected",
            "auth_type": auth_type,
            "connected_at": serde_json::Value::Null,
            "expires_at": serde_json::Value::Null,
            "last_error": serde_json::Value::Null,
        },
        "enabled_actions": enabled_actions,
    })
}

pub(super) fn build_admin_provider_ops_config_payload(
    state: &AppState,
    provider_id: &str,
    provider: Option<&StoredProviderCatalogProvider>,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> serde_json::Value {
    let Some(provider) = provider else {
        return json!({
            "provider_id": provider_id,
            "is_configured": false,
        });
    };
    let Some(provider_ops_config) = admin_provider_ops_config_object(provider) else {
        return json!({
            "provider_id": provider_id,
            "is_configured": false,
        });
    };
    let connector = admin_provider_ops_connector_object(provider_ops_config);

    json!({
        "provider_id": provider_id,
        "is_configured": true,
        "architecture_id": provider_ops_config
            .get("architecture_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("generic_api"),
        "base_url": resolve_admin_provider_ops_base_url(
            provider,
            endpoints,
            Some(provider_ops_config),
        ),
        "connector": {
            "auth_type": connector
                .and_then(|connector| connector.get("auth_type"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("api_key"),
            "config": connector
                .and_then(|connector| connector.get("config"))
                .filter(|value| value.is_object())
                .cloned()
                .unwrap_or_else(|| json!({})),
            "credentials": admin_provider_ops_masked_credentials(
                state,
                connector.and_then(|connector| connector.get("credentials")),
            ),
        },
    })
}
