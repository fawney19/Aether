use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_key_is_oauth_managed;
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

fn admin_pool_derive_plan_tier(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<String> {
    if !provider_key_is_oauth_managed(key, provider_type) {
        return None;
    }

    let auth_config = admin_pool_parse_auth_config_json(state, key);
    aether_provider_pool::derive_plan_tier(provider_type, key, auth_config.as_ref())
}

pub(super) fn admin_pool_matches_quick_selector(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    selector: &str,
) -> bool {
    let oauth_plan_type = admin_pool_derive_plan_tier(state, key, provider_type);
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
    let oauth_plan_type = admin_pool_derive_plan_tier(state, key, provider_type);
    admin_provider_pool_pure::admin_pool_matches_search(key, search, oauth_plan_type.as_deref())
}

pub(super) fn admin_pool_matches_catalog_search(
    key: &StoredProviderCatalogKey,
    search: Option<&str>,
) -> bool {
    let Some(search) = search else {
        return true;
    };
    let search = admin_pool_normalize_text(search);
    search.is_empty()
        || admin_pool_normalize_text(&key.name).contains(&search)
        || admin_pool_normalize_text(&key.id).contains(&search)
}

pub(super) fn admin_pool_key_is_known_banned(key: &StoredProviderCatalogKey) -> bool {
    admin_provider_pool_pure::admin_pool_key_is_known_banned(key)
}

pub(super) fn admin_pool_sort_keys(keys: &mut [StoredProviderCatalogKey]) {
    admin_provider_pool_pure::admin_pool_sort_keys(keys);
}
