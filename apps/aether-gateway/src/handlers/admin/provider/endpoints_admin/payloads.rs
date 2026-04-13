use crate::handlers::admin::shared::AdminTypedObjectPatch;
use aether_admin::provider::endpoints as admin_provider_endpoints_pure;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use serde::Deserialize;

pub(super) fn key_api_formats_without_entry(
    key: &StoredProviderCatalogKey,
    api_format: &str,
) -> Option<Vec<String>> {
    admin_provider_endpoints_pure::key_api_formats_without_entry(key, api_format)
}

pub(super) fn endpoint_key_counts_by_format(
    keys: &[StoredProviderCatalogKey],
) -> (
    std::collections::BTreeMap<String, usize>,
    std::collections::BTreeMap<String, usize>,
) {
    admin_provider_endpoints_pure::endpoint_key_counts_by_format(keys)
}

pub(super) fn build_admin_provider_endpoint_response(
    endpoint: &StoredProviderCatalogEndpoint,
    provider_name: &str,
    total_keys: usize,
    active_keys: usize,
    now_unix_secs: u64,
) -> serde_json::Value {
    admin_provider_endpoints_pure::build_admin_provider_endpoint_response(
        endpoint,
        provider_name,
        total_keys,
        active_keys,
        now_unix_secs,
    )
}

fn default_admin_endpoint_max_retries() -> i32 {
    2
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderEndpointCreateRequest {
    pub(crate) provider_id: String,
    pub(crate) api_format: String,
    pub(crate) base_url: String,
    #[serde(default)]
    pub(crate) custom_path: Option<String>,
    #[serde(default)]
    pub(crate) header_rules: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) body_rules: Option<serde_json::Value>,
    #[serde(default = "default_admin_endpoint_max_retries")]
    pub(crate) max_retries: i32,
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) proxy: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) format_acceptance_config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderEndpointUpdateRequest {
    #[serde(default)]
    pub(crate) base_url: Option<String>,
    #[serde(default)]
    pub(crate) custom_path: Option<String>,
    #[serde(default)]
    pub(crate) header_rules: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) body_rules: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) max_retries: Option<i32>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) proxy: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) format_acceptance_config: Option<serde_json::Value>,
}

pub(crate) type AdminProviderEndpointUpdatePatch =
    AdminTypedObjectPatch<AdminProviderEndpointUpdateRequest>;
