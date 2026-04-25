use super::endpoint::{
    build_admin_fixed_provider_endpoint_defaults, build_admin_fixed_provider_endpoint_record,
};
use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::GatewayError;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_transport::provider_types::{
    fixed_provider_template, FixedProviderEndpointTemplate, FixedProviderTemplate,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const FIXED_PROVIDER_TEMPLATE_METADATA_KEY: &str = "_aether_fixed_provider_template";
const OVERRIDE_BODY_RULES: &str = "body_rules";
const OVERRIDE_FORMAT_ACCEPTANCE_CONFIG: &str = "format_acceptance_config";
const OVERRIDE_HEADER_RULES: &str = "header_rules";
const OVERRIDE_IS_ACTIVE: &str = "is_active";
const OVERRIDE_MAX_RETRIES: &str = "max_retries";
const OVERRIDE_PROXY: &str = "proxy";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FixedProviderEndpointMetadata {
    provider_type: String,
    item_key: String,
    version: u32,
    retired: bool,
    overrides: BTreeSet<String>,
    config_keys: BTreeSet<String>,
}

pub(crate) async fn reconcile_admin_fixed_provider_template_endpoints(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
) -> Result<(), GatewayError> {
    let Some(template) = state.fixed_provider_template(&provider.provider_type) else {
        return Ok(());
    };

    let existing_endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let mut matched_endpoint_ids = BTreeSet::new();

    for endpoint_template in template.endpoints {
        let existing_endpoint = existing_endpoints
            .iter()
            .find(|endpoint| endpoint_matches_fixed_provider_template(endpoint, endpoint_template));
        match existing_endpoint {
            Some(existing_endpoint) => {
                matched_endpoint_ids.insert(existing_endpoint.id.clone());
                let updated = reconcile_fixed_provider_endpoint(
                    provider,
                    existing_endpoint,
                    template,
                    endpoint_template,
                )
                .map_err(GatewayError::Internal)?;
                if updated != *existing_endpoint {
                    let Some(_) = state.update_provider_catalog_endpoint(&updated).await? else {
                        return Err(GatewayError::Internal(
                            "provider catalog endpoint writer unavailable".to_string(),
                        ));
                    };
                }
            }
            None => {
                let mut created = build_admin_fixed_provider_endpoint_record(
                    provider,
                    template,
                    endpoint_template,
                )
                .map_err(GatewayError::Internal)?;
                let metadata =
                    managed_fixed_provider_endpoint_metadata(template, endpoint_template);
                upsert_fixed_provider_endpoint_metadata(&mut created, &metadata);
                let Some(_) = state.create_provider_catalog_endpoint(&created).await? else {
                    return Err(GatewayError::Internal(
                        "provider catalog endpoint writer unavailable".to_string(),
                    ));
                };
            }
        }
    }

    for existing_endpoint in &existing_endpoints {
        if matched_endpoint_ids.contains(&existing_endpoint.id) {
            continue;
        }
        let Some(metadata) = fixed_provider_endpoint_metadata(existing_endpoint) else {
            continue;
        };
        if metadata.retired && !existing_endpoint.is_active {
            continue;
        }
        let mut retired = existing_endpoint.clone();
        let mut retired_metadata = metadata;
        retired.is_active = false;
        retired_metadata.retired = true;
        upsert_fixed_provider_endpoint_metadata(&mut retired, &retired_metadata);
        if retired != *existing_endpoint {
            retired.updated_at_unix_secs = Some(current_unix_secs());
            let Some(_) = state.update_provider_catalog_endpoint(&retired).await? else {
                return Err(GatewayError::Internal(
                    "provider catalog endpoint writer unavailable".to_string(),
                ));
            };
        }
    }

    Ok(())
}

pub(crate) async fn reconcile_admin_fixed_provider_template_keys(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
) -> Result<(), GatewayError> {
    let Some(_) = state.fixed_provider_template(&provider.provider_type) else {
        return Ok(());
    };

    let existing_keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    for existing_key in existing_keys {
        let Some(updated_key) = reconcile_fixed_provider_key(provider, &existing_key) else {
            continue;
        };
        let Some(_) = state.update_provider_catalog_key(&updated_key).await? else {
            return Err(GatewayError::Internal(
                "provider catalog key writer unavailable".to_string(),
            ));
        };
    }

    Ok(())
}

pub(crate) fn apply_admin_fixed_provider_endpoint_template_overrides(
    provider: &StoredProviderCatalogProvider,
    existing_endpoint: &StoredProviderCatalogEndpoint,
    updated_endpoint: &mut StoredProviderCatalogEndpoint,
) -> Result<(), String> {
    let Some(template) = fixed_provider_template(&provider.provider_type) else {
        return Ok(());
    };
    let Some(endpoint_template) =
        resolve_fixed_provider_endpoint_template(template, existing_endpoint, updated_endpoint)
    else {
        return Ok(());
    };

    let defaults =
        build_admin_fixed_provider_endpoint_defaults(provider, template, endpoint_template)?;
    let mut metadata = fixed_provider_endpoint_metadata(existing_endpoint)
        .unwrap_or_else(|| managed_fixed_provider_endpoint_metadata(template, endpoint_template));
    let mut overrides = metadata.overrides.clone();

    sync_override_if_changed(
        &mut overrides,
        OVERRIDE_HEADER_RULES,
        &existing_endpoint.header_rules,
        &updated_endpoint.header_rules,
        &defaults.header_rules,
    );
    sync_override_if_changed(
        &mut overrides,
        OVERRIDE_BODY_RULES,
        &existing_endpoint.body_rules,
        &updated_endpoint.body_rules,
        &defaults.body_rules,
    );
    sync_override_if_changed(
        &mut overrides,
        OVERRIDE_MAX_RETRIES,
        &existing_endpoint.max_retries,
        &updated_endpoint.max_retries,
        &defaults.max_retries,
    );
    sync_override_if_changed(
        &mut overrides,
        OVERRIDE_IS_ACTIVE,
        &existing_endpoint.is_active,
        &updated_endpoint.is_active,
        &defaults.is_active,
    );
    sync_override_if_changed(
        &mut overrides,
        OVERRIDE_PROXY,
        &existing_endpoint.proxy,
        &updated_endpoint.proxy,
        &defaults.proxy,
    );
    sync_override_if_changed(
        &mut overrides,
        OVERRIDE_FORMAT_ACCEPTANCE_CONFIG,
        &existing_endpoint.format_acceptance_config,
        &updated_endpoint.format_acceptance_config,
        &defaults.format_acceptance_config,
    );

    let current_config_defaults = fixed_provider_endpoint_config_defaults(endpoint_template);
    let config = endpoint_config_without_metadata(updated_endpoint.config.as_ref());
    let existing_config = endpoint_config_without_metadata(existing_endpoint.config.as_ref());
    let current_config_keys = current_config_defaults
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut tracked_config_keys = metadata.config_keys.clone();
    tracked_config_keys.extend(current_config_keys.iter().cloned());

    for key in tracked_config_keys {
        let before = existing_config.get(&key);
        let actual = config.get(&key);
        let desired = current_config_defaults.get(&key);
        sync_override_if_changed(
            &mut overrides,
            &config_override_key(&key),
            &before.cloned(),
            &actual.cloned(),
            &desired.cloned(),
        );
    }

    metadata.provider_type = template.provider_type.to_string();
    metadata.item_key = endpoint_template.item_key.to_string();
    metadata.version = template.version;
    metadata.retired = false;
    metadata.overrides = overrides;
    metadata.config_keys = current_config_keys;
    updated_endpoint.config = materialize_endpoint_config(config, &metadata);
    Ok(())
}

fn reconcile_fixed_provider_endpoint(
    provider: &StoredProviderCatalogProvider,
    existing_endpoint: &StoredProviderCatalogEndpoint,
    template: &FixedProviderTemplate,
    endpoint_template: &FixedProviderEndpointTemplate,
) -> Result<StoredProviderCatalogEndpoint, String> {
    let defaults =
        build_admin_fixed_provider_endpoint_defaults(provider, template, endpoint_template)?;
    let mut updated = existing_endpoint.clone();
    let metadata = fixed_provider_endpoint_metadata(existing_endpoint)
        .unwrap_or_else(|| managed_fixed_provider_endpoint_metadata(template, endpoint_template));

    updated.api_format = defaults.api_format.clone();
    updated.api_family = Some(defaults.api_family.clone());
    updated.endpoint_kind = Some(defaults.endpoint_kind.clone());
    updated.base_url = defaults.base_url;
    updated.custom_path = defaults.custom_path;

    if !metadata.overrides.contains(OVERRIDE_HEADER_RULES) {
        updated.header_rules = defaults.header_rules;
    }
    if !metadata.overrides.contains(OVERRIDE_BODY_RULES) {
        updated.body_rules = defaults.body_rules;
    }
    if !metadata.overrides.contains(OVERRIDE_MAX_RETRIES) {
        updated.max_retries = defaults.max_retries;
    }
    if !metadata.overrides.contains(OVERRIDE_IS_ACTIVE) {
        updated.is_active = defaults.is_active;
    }
    if !metadata.overrides.contains(OVERRIDE_PROXY) {
        updated.proxy = defaults.proxy;
    }
    if !metadata
        .overrides
        .contains(OVERRIDE_FORMAT_ACCEPTANCE_CONFIG)
    {
        updated.format_acceptance_config = defaults.format_acceptance_config;
    }

    let mut config = endpoint_config_without_metadata(updated.config.as_ref());
    let current_config_defaults = fixed_provider_endpoint_config_defaults(endpoint_template);
    let current_config_keys = current_config_defaults
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();

    for old_key in metadata.config_keys.difference(&current_config_keys) {
        if !metadata
            .overrides
            .contains(config_override_key(old_key.as_str()).as_str())
        {
            config.remove(old_key);
        }
    }
    for (key, value) in &current_config_defaults {
        if !metadata
            .overrides
            .contains(config_override_key(key.as_str()).as_str())
        {
            config.insert(key.clone(), value.clone());
        }
    }

    let mut next_metadata = metadata;
    next_metadata.provider_type = template.provider_type.to_string();
    next_metadata.item_key = endpoint_template.item_key.to_string();
    next_metadata.version = template.version;
    next_metadata.retired = false;
    next_metadata.config_keys = current_config_keys;
    updated.config = materialize_endpoint_config(config, &next_metadata);

    if updated != *existing_endpoint {
        updated.updated_at_unix_secs = Some(current_unix_secs());
    }
    Ok(updated)
}

fn resolve_fixed_provider_endpoint_template<'a>(
    template: &'a FixedProviderTemplate,
    existing_endpoint: &StoredProviderCatalogEndpoint,
    updated_endpoint: &StoredProviderCatalogEndpoint,
) -> Option<&'a FixedProviderEndpointTemplate> {
    if let Some(metadata) = fixed_provider_endpoint_metadata(existing_endpoint) {
        if let Some(item) = template
            .endpoints
            .iter()
            .find(|item| item.item_key == metadata.item_key)
        {
            return Some(item);
        }
    }

    template.endpoints.iter().find(|item| {
        item.api_format
            .eq_ignore_ascii_case(updated_endpoint.api_format.trim())
            || item
                .api_format
                .eq_ignore_ascii_case(existing_endpoint.api_format.trim())
    })
}

fn endpoint_matches_fixed_provider_template(
    endpoint: &StoredProviderCatalogEndpoint,
    endpoint_template: &FixedProviderEndpointTemplate,
) -> bool {
    if let Some(metadata) = fixed_provider_endpoint_metadata(endpoint) {
        if metadata.item_key == endpoint_template.item_key {
            return true;
        }
    }
    endpoint
        .api_format
        .trim()
        .eq_ignore_ascii_case(endpoint_template.api_format)
}

fn fixed_provider_endpoint_metadata(
    endpoint: &StoredProviderCatalogEndpoint,
) -> Option<FixedProviderEndpointMetadata> {
    let config = endpoint.config.as_ref()?.as_object()?;
    let metadata = config
        .get(FIXED_PROVIDER_TEMPLATE_METADATA_KEY)?
        .as_object()?;
    let provider_type = metadata
        .get("provider_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let item_key = metadata
        .get("item_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if !metadata
        .get("managed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    Some(FixedProviderEndpointMetadata {
        provider_type: provider_type.to_string(),
        item_key: item_key.to_string(),
        version: metadata
            .get("version")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(0),
        retired: metadata
            .get("retired")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        overrides: metadata
            .get("overrides")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        config_keys: metadata
            .get("config_keys")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn managed_fixed_provider_endpoint_metadata(
    template: &FixedProviderTemplate,
    endpoint_template: &FixedProviderEndpointTemplate,
) -> FixedProviderEndpointMetadata {
    FixedProviderEndpointMetadata {
        provider_type: template.provider_type.to_string(),
        item_key: endpoint_template.item_key.to_string(),
        version: template.version,
        retired: false,
        overrides: BTreeSet::new(),
        config_keys: fixed_provider_endpoint_config_defaults(endpoint_template)
            .into_keys()
            .collect(),
    }
}

fn upsert_fixed_provider_endpoint_metadata(
    endpoint: &mut StoredProviderCatalogEndpoint,
    metadata: &FixedProviderEndpointMetadata,
) {
    let config = endpoint_config_without_metadata(endpoint.config.as_ref());
    endpoint.config = materialize_endpoint_config(config, metadata);
}

fn endpoint_config_without_metadata(config: Option<&Value>) -> Map<String, Value> {
    let mut config = config
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    config.remove(FIXED_PROVIDER_TEMPLATE_METADATA_KEY);
    config
}

fn materialize_endpoint_config(
    mut config: Map<String, Value>,
    metadata: &FixedProviderEndpointMetadata,
) -> Option<Value> {
    config.insert(
        FIXED_PROVIDER_TEMPLATE_METADATA_KEY.to_string(),
        json!({
            "managed": true,
            "provider_type": metadata.provider_type,
            "item_key": metadata.item_key,
            "version": metadata.version,
            "retired": metadata.retired,
            "overrides": metadata.overrides.iter().cloned().collect::<Vec<_>>(),
            "config_keys": metadata.config_keys.iter().cloned().collect::<Vec<_>>(),
        }),
    );
    Some(Value::Object(config))
}

fn fixed_provider_endpoint_config_defaults(
    endpoint_template: &FixedProviderEndpointTemplate,
) -> BTreeMap<String, Value> {
    endpoint_template
        .config_defaults
        .iter()
        .map(|item| (item.key.to_string(), item.value.to_json_value()))
        .collect()
}

fn config_override_key(key: &str) -> String {
    format!("config.{key}")
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn reconcile_fixed_provider_key(
    provider: &StoredProviderCatalogProvider,
    existing_key: &StoredProviderCatalogKey,
) -> Option<StoredProviderCatalogKey> {
    if !provider_key_is_oauth_managed(existing_key, &provider.provider_type)
        || existing_key.api_formats.is_none()
    {
        return None;
    }

    let mut updated = existing_key.clone();
    updated.api_formats = None;
    updated.updated_at_unix_secs = Some(current_unix_secs());
    Some(updated)
}

fn sync_override<T>(overrides: &mut BTreeSet<String>, key: &str, actual: &T, desired: &T)
where
    T: PartialEq,
{
    if actual == desired {
        overrides.remove(key);
    } else {
        overrides.insert(key.to_string());
    }
}

fn sync_override_if_changed<T>(
    overrides: &mut BTreeSet<String>,
    key: &str,
    before: &T,
    actual: &T,
    desired: &T,
) where
    T: PartialEq,
{
    if before == actual {
        return;
    }
    sync_override(overrides, key, actual, desired);
}
