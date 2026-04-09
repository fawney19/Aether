use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::pool as admin_provider_pool_pure;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

pub(super) fn admin_pool_normalize_text(value: impl AsRef<str>) -> String {
    admin_provider_pool_pure::admin_pool_normalize_text(value)
}

fn admin_pool_parse_auth_config_json(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let ciphertext = key.encrypted_auth_config.as_deref()?.trim();
    if ciphertext.is_empty() {
        return None;
    }
    let plaintext = state.decrypt_catalog_secret_with_fallbacks(ciphertext)?;
    serde_json::from_str::<serde_json::Value>(&plaintext)
        .ok()?
        .as_object()
        .cloned()
}

fn admin_pool_derive_oauth_plan_type(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<String> {
    let normalize = |value: &str| {
        let mut text = value.trim().to_string();
        if text.is_empty() {
            return None;
        }
        let provider_type = provider_type.trim().to_ascii_lowercase();
        if !provider_type.is_empty() && text.to_ascii_lowercase().starts_with(&provider_type) {
            text = text[provider_type.len()..]
                .trim_matches(|ch: char| [' ', ':', '-', '_'].contains(&ch))
                .to_string();
        }
        if text.is_empty() {
            None
        } else {
            Some(text.to_ascii_lowercase())
        }
    };

    if key.auth_type.trim() != "oauth" {
        return None;
    }

    if let Some(upstream_metadata) = key
        .upstream_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
    {
        let provider_bucket = upstream_metadata
            .get(&provider_type.trim().to_ascii_lowercase())
            .and_then(serde_json::Value::as_object);
        for source in provider_bucket
            .into_iter()
            .chain(std::iter::once(upstream_metadata))
        {
            for plan_key in [
                "plan_type",
                "tier",
                "subscription_title",
                "subscription_plan",
            ] {
                if let Some(value) = source.get(plan_key).and_then(serde_json::Value::as_str) {
                    if let Some(normalized) = normalize(value) {
                        return Some(normalized);
                    }
                }
            }
        }
    }

    if let Some(auth_config) = admin_pool_parse_auth_config_json(state, key) {
        for plan_key in ["plan_type", "tier", "plan", "subscription_plan"] {
            if let Some(value) = auth_config
                .get(plan_key)
                .and_then(serde_json::Value::as_str)
            {
                if let Some(normalized) = normalize(value) {
                    return Some(normalized);
                }
            }
        }
    }

    None
}

pub(super) fn admin_pool_matches_quick_selector(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    selector: &str,
) -> bool {
    let oauth_plan_type = admin_pool_derive_oauth_plan_type(state, key, provider_type);
    admin_provider_pool_pure::admin_pool_matches_quick_selector(
        key,
        selector,
        oauth_plan_type.as_deref(),
        admin_provider_pool_pure::admin_pool_now_unix_secs(),
    )
}

pub(super) fn admin_pool_matches_search(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    search: Option<&str>,
) -> bool {
    let oauth_plan_type = admin_pool_derive_oauth_plan_type(state, key, provider_type);
    admin_provider_pool_pure::admin_pool_matches_search(key, search, oauth_plan_type.as_deref())
}

pub(super) fn admin_pool_key_is_known_banned(key: &StoredProviderCatalogKey) -> bool {
    admin_provider_pool_pure::admin_pool_key_is_known_banned(key)
}

pub(super) fn admin_pool_sort_keys(keys: &mut [StoredProviderCatalogKey]) {
    admin_provider_pool_pure::admin_pool_sort_keys(keys);
}
