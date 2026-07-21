use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::{Map, Value};

pub fn derive_plan_tier(
    provider_type: &str,
    key: &StoredProviderCatalogKey,
    auth_config: Option<&Map<String, Value>>,
) -> Option<String> {
    let has_auth_config = auth_config.is_some()
        || key
            .encrypted_auth_config
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    if !provider_pool_auth_managed(key, provider_type, has_auth_config) {
        return None;
    }

    if let Some(quota_snapshot) = key
        .status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)
    {
        if let Some(normalized) = derive_plan_tier_from_map(quota_snapshot, provider_type) {
            return Some(normalized);
        }
    }

    if let Some(upstream_metadata) = key.upstream_metadata.as_ref().and_then(Value::as_object) {
        let provider_bucket = upstream_metadata
            .get(&provider_type.trim().to_ascii_lowercase())
            .and_then(Value::as_object);
        for source in provider_bucket
            .into_iter()
            .chain(std::iter::once(upstream_metadata))
        {
            if let Some(normalized) = derive_plan_tier_from_map(source, provider_type) {
                return Some(normalized);
            }
        }
    }

    if let Some(config) = auth_config {
        if let Some(normalized) = derive_plan_tier_from_map(config, provider_type) {
            return Some(normalized);
        }
    }

    None
}

fn derive_plan_tier_from_map(source: &Map<String, Value>, provider_type: &str) -> Option<String> {
    for field in [
        "plan_type",
        "tier",
        "plan",
        "subscription_title",
        "subscription_plan",
    ] {
        if let Some(value) = source.get(field).and_then(Value::as_str) {
            if let Some(normalized) = normalize_provider_plan_tier(value, provider_type) {
                return Some(normalized);
            }
        }
    }
    None
}

pub fn derive_oauth_plan_type(
    provider_type: &str,
    key: &StoredProviderCatalogKey,
    auth_config: Option<&Map<String, Value>>,
) -> Option<String> {
    derive_plan_tier(provider_type, key, auth_config)
}

pub fn normalize_provider_plan_tier(value: &str, provider_type: &str) -> Option<String> {
    let mut normalized = value.trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    let provider_type = provider_type.trim().to_ascii_lowercase();
    if !provider_type.is_empty() && normalized.to_ascii_lowercase().starts_with(&provider_type) {
        normalized = normalized[provider_type.len()..]
            .trim_matches(|ch: char| [' ', ':', '-', '_'].contains(&ch))
            .to_string();
    }

    let normalized = normalized.trim().to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn provider_pool_auth_managed(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    has_auth_config: bool,
) -> bool {
    key.auth_type.trim().eq_ignore_ascii_case("oauth")
        || (provider_type.trim().eq_ignore_ascii_case("kiro")
            && key.auth_type.trim().eq_ignore_ascii_case("bearer")
            && has_auth_config)
}
