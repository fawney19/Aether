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
            skip_exhausted_accounts: false,
            cost_window_seconds: 18_000,
            cost_limit_per_key_tokens: None,
        });
    };

    Some(AdminProviderPoolConfig {
        lru_enabled: admin_provider_pool_lru_enabled(pool_advanced),
        skip_exhausted_accounts: pool_advanced
            .get("skip_exhausted_accounts")
            .and_then(Value::as_bool)
            .unwrap_or(false),
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

#[cfg(test)]
mod tests {
    use super::admin_provider_pool_config;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
    use serde_json::json;

    fn sample_provider(config: serde_json::Value) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "provider-1".to_string(),
            Some("https://example.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            Some(config),
        )
    }

    #[test]
    fn defaults_skip_exhausted_accounts_to_false() {
        let provider = sample_provider(json!({ "pool_advanced": {} }));
        let config = admin_provider_pool_config(&provider).expect("pool config should exist");

        assert!(!config.skip_exhausted_accounts);
    }

    #[test]
    fn parses_skip_exhausted_accounts_from_pool_advanced() {
        let provider = sample_provider(json!({
            "pool_advanced": {
                "skip_exhausted_accounts": true,
                "lru_enabled": true,
                "cost_window_seconds": 7200,
                "cost_limit_per_key_tokens": 12000
            }
        }));
        let config = admin_provider_pool_config(&provider).expect("pool config should exist");

        assert!(config.skip_exhausted_accounts);
        assert!(config.lru_enabled);
        assert_eq!(config.cost_window_seconds, 7200);
        assert_eq!(config.cost_limit_per_key_tokens, Some(12_000));
    }
}
