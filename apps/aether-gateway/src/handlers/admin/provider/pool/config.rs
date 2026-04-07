use crate::handlers::admin::provider::shared::support::AdminProviderPoolConfig;
use serde_json::{Map, Value};

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|raw| u64::try_from(raw).ok()))
}

fn admin_provider_pool_lru_enabled(raw_pool_advanced: &Map<String, Value>) -> bool {
    if let Some(explicit) = raw_pool_advanced
        .get("lru_enabled")
        .and_then(Value::as_bool)
    {
        return explicit;
    }

    let Some(presets) = raw_pool_advanced
        .get("scheduling_presets")
        .and_then(Value::as_array)
    else {
        return false;
    };

    let Some(first) = presets.first() else {
        return false;
    };

    if first.is_string() {
        return raw_pool_advanced
            .get("lru_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
    }

    presets.iter().filter_map(Value::as_object).any(|item| {
        item.get("preset")
            .and_then(Value::as_str)
            .is_some_and(|preset| preset.eq_ignore_ascii_case("lru"))
            && item.get("enabled").and_then(Value::as_bool).unwrap_or(true)
    })
}

pub(crate) fn admin_provider_pool_config(
    provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
) -> Option<AdminProviderPoolConfig> {
    let raw_pool_advanced = provider
        .config
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|config| config.get("pool_advanced"))?;

    let Some(pool_advanced) = raw_pool_advanced.as_object() else {
        return Some(AdminProviderPoolConfig {
            lru_enabled: false,
            cost_window_seconds: 18_000,
            cost_limit_per_key_tokens: None,
        });
    };

    Some(AdminProviderPoolConfig {
        lru_enabled: admin_provider_pool_lru_enabled(pool_advanced),
        cost_window_seconds: pool_advanced
            .get("cost_window_seconds")
            .and_then(json_u64)
            .filter(|value| *value > 0)
            .unwrap_or(18_000),
        cost_limit_per_key_tokens: pool_advanced
            .get("cost_limit_per_key_tokens")
            .and_then(json_u64),
    })
}
