use crate::api::ai::{admin_default_body_rules_for_signature, admin_endpoint_signature_parts};
use crate::handlers::public::normalize_admin_base_url;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use aether_provider_transport::provider_types::{
    FixedProviderEndpointTemplate, FixedProviderTemplate,
};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdminFixedProviderEndpointDefaults {
    pub(crate) api_format: String,
    pub(crate) api_family: String,
    pub(crate) endpoint_kind: String,
    pub(crate) is_active: bool,
    pub(crate) base_url: String,
    pub(crate) header_rules: Option<serde_json::Value>,
    pub(crate) body_rules: Option<serde_json::Value>,
    pub(crate) max_retries: Option<i32>,
    pub(crate) custom_path: Option<String>,
    pub(crate) config: Option<serde_json::Value>,
    pub(crate) format_acceptance_config: Option<serde_json::Value>,
    pub(crate) proxy: Option<serde_json::Value>,
}

pub(crate) fn build_admin_fixed_provider_endpoint_defaults(
    provider: &StoredProviderCatalogProvider,
    template: &FixedProviderTemplate,
    endpoint_template: &FixedProviderEndpointTemplate,
) -> Result<AdminFixedProviderEndpointDefaults, String> {
    let (normalized_api_format, api_family, endpoint_kind) =
        admin_endpoint_signature_parts(endpoint_template.api_format)
            .ok_or_else(|| format!("无效的 api_format: {}", endpoint_template.api_format))?;
    let body_rules = admin_default_body_rules_for_signature(
        normalized_api_format,
        Some(provider.provider_type.as_str()),
    )
    .and_then(|(_, rules)| (!rules.is_empty()).then_some(serde_json::Value::Array(rules)));

    Ok(AdminFixedProviderEndpointDefaults {
        api_format: normalized_api_format.to_string(),
        api_family: api_family.to_string(),
        endpoint_kind: endpoint_kind.to_string(),
        is_active: true,
        base_url: normalize_admin_base_url(template.base_url)?,
        header_rules: None,
        body_rules,
        max_retries: Some(provider.max_retries.unwrap_or(2)),
        custom_path: endpoint_template.custom_path.map(ToOwned::to_owned),
        config: fixed_provider_endpoint_default_config(endpoint_template),
        format_acceptance_config: None,
        proxy: None,
    })
}

pub(crate) fn build_admin_fixed_provider_endpoint_record(
    provider: &StoredProviderCatalogProvider,
    template: &FixedProviderTemplate,
    endpoint_template: &FixedProviderEndpointTemplate,
) -> Result<StoredProviderCatalogEndpoint, String> {
    let defaults =
        build_admin_fixed_provider_endpoint_defaults(provider, template, endpoint_template)?;
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    StoredProviderCatalogEndpoint::new(
        Uuid::new_v4().to_string(),
        provider.id.clone(),
        defaults.api_format,
        Some(defaults.api_family),
        Some(defaults.endpoint_kind),
        defaults.is_active,
    )
    .map_err(|err| err.to_string())?
    .with_timestamps(Some(now_unix_secs), Some(now_unix_secs))
    .with_transport_fields(
        defaults.base_url,
        defaults.header_rules,
        defaults.body_rules,
        defaults.max_retries,
        defaults.custom_path,
        defaults.config,
        defaults.format_acceptance_config,
        defaults.proxy,
    )
    .map_err(|err| err.to_string())
}

fn fixed_provider_endpoint_default_config(
    endpoint_template: &FixedProviderEndpointTemplate,
) -> Option<serde_json::Value> {
    let mut config = serde_json::Map::new();
    for default in endpoint_template.config_defaults {
        config.insert(default.key.to_string(), default.value.to_json_value());
    }
    (!config.is_empty()).then_some(serde_json::Value::Object(config))
}
