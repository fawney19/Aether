use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use serde_json::json;

pub fn admin_provider_ops_config_object(
    provider: &StoredProviderCatalogProvider,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    provider
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("provider_ops"))
        .and_then(serde_json::Value::as_object)
}

pub fn admin_provider_ops_connector_object(
    provider_ops_config: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    provider_ops_config
        .get("connector")
        .and_then(serde_json::Value::as_object)
}

pub fn admin_provider_ops_is_supported_auth_type(auth_type: &str) -> bool {
    matches!(
        auth_type,
        "api_key" | "session_login" | "oauth" | "cookie" | "none"
    )
}

pub fn admin_provider_ops_uses_python_verify_fallback(
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

pub fn admin_provider_ops_sensitive_placeholder_or_empty(
    value: Option<&serde_json::Value>,
) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::String(raw)) => raw.is_empty() || raw.chars().all(|ch| ch == '*'),
        Some(serde_json::Value::Array(items)) => items.is_empty(),
        Some(serde_json::Value::Object(map)) => map.is_empty(),
        _ => false,
    }
}

pub fn resolve_admin_provider_ops_base_url(
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

pub fn build_admin_provider_ops_status_payload(
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
