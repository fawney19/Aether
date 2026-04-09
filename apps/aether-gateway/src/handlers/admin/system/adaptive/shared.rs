use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use axum::{body::Body, response::Response};

pub(super) fn admin_adaptive_effective_limit(key: &StoredProviderCatalogKey) -> Option<u32> {
    aether_admin::system::admin_adaptive_effective_limit(key)
}

pub(super) fn admin_adaptive_adjustment_items(
    value: Option<&serde_json::Value>,
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    aether_admin::system::admin_adaptive_adjustment_items(value)
}

pub(super) fn admin_adaptive_key_payload(key: &StoredProviderCatalogKey) -> serde_json::Value {
    aether_admin::system::admin_adaptive_key_payload(key)
}

pub(super) fn admin_adaptive_key_not_found_response(key_id: &str) -> Response<Body> {
    aether_admin::system::admin_adaptive_key_not_found_response(key_id)
}

pub(super) fn admin_adaptive_dispatcher_not_found_response() -> Response<Body> {
    aether_admin::system::admin_adaptive_dispatcher_not_found_response()
}

pub(super) fn admin_adaptive_key_id_from_path(path: &str) -> Option<String> {
    aether_admin::system::admin_adaptive_key_id_from_path(path)
}

pub(super) async fn admin_adaptive_find_key(
    state: &AdminAppState<'_>,
    key_id: &str,
) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
    Ok(state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id.to_string()))
        .await?
        .into_iter()
        .next())
}

pub(super) async fn admin_adaptive_load_candidate_keys(
    state: &AdminAppState<'_>,
    provider_id: Option<&str>,
) -> Result<Vec<StoredProviderCatalogKey>, GatewayError> {
    if let Some(provider_id) = provider_id.filter(|value| !value.trim().is_empty()) {
        return state
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(
                &provider_id.to_string(),
            ))
            .await;
    }

    let provider_ids = state
        .list_provider_catalog_providers(false)
        .await?
        .into_iter()
        .map(|provider| provider.id)
        .collect::<Vec<_>>();
    if provider_ids.is_empty() {
        return Ok(vec![]);
    }
    state
        .list_provider_catalog_keys_by_provider_ids(&provider_ids)
        .await
}
