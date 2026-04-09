use crate::handlers::admin::request::AdminAppState;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use std::time::{SystemTime, UNIX_EPOCH};

use super::payloads::{build_admin_provider_endpoint_response, endpoint_key_counts_by_format};

pub(crate) async fn build_admin_provider_endpoints_payload(
    state: &AdminAppState<'_>,
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
    let mut endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .ok()
        .unwrap_or_default();
    endpoints.sort_by(|left, right| {
        right
            .created_at_unix_secs
            .unwrap_or_default()
            .cmp(&left.created_at_unix_secs.unwrap_or_default())
            .then_with(|| left.id.cmp(&right.id))
    });
    let keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .ok()
        .unwrap_or_default();
    let (total_keys_by_format, active_keys_by_format) = endpoint_key_counts_by_format(&keys);
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    Some(serde_json::Value::Array(
        endpoints
            .into_iter()
            .skip(skip)
            .take(limit)
            .map(|endpoint| {
                build_admin_provider_endpoint_response(
                    &endpoint,
                    &provider.name,
                    total_keys_by_format
                        .get(endpoint.api_format.as_str())
                        .copied()
                        .unwrap_or(0),
                    active_keys_by_format
                        .get(endpoint.api_format.as_str())
                        .copied()
                        .unwrap_or(0),
                    now_unix_secs,
                )
            })
            .collect(),
    ))
}

pub(crate) async fn build_admin_endpoint_payload(
    state: &AdminAppState<'_>,
    endpoint_id: &str,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() {
        return None;
    }

    let endpoint = state
        .read_provider_catalog_endpoints_by_ids(&[endpoint_id.to_string()])
        .await
        .ok()
        .and_then(|mut endpoints| endpoints.drain(..).next())?;
    let provider = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&endpoint.provider_id))
        .await
        .ok()
        .and_then(|mut providers| providers.drain(..).next())?;
    let keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&endpoint.provider_id))
        .await
        .ok()
        .unwrap_or_default();
    let (total_keys_by_format, active_keys_by_format) = endpoint_key_counts_by_format(&keys);
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    Some(build_admin_provider_endpoint_response(
        &endpoint,
        &provider.name,
        total_keys_by_format
            .get(endpoint.api_format.as_str())
            .copied()
            .unwrap_or(0),
        active_keys_by_format
            .get(endpoint.api_format.as_str())
            .copied()
            .unwrap_or(0),
        now_unix_secs,
    ))
}
