use super::support::{
    collect_admin_system_export_provider_endpoint_formats,
    decrypt_admin_system_export_provider_config, decrypt_admin_system_export_secret,
    resolve_admin_system_export_key_api_formats, ADMIN_SYSTEM_EXPORT_PAGE_LIMIT,
};
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::system::{
    AdminSystemConfigEndpoint, AdminSystemConfigProvider, AdminSystemConfigProviderKey,
    AdminSystemConfigProviderModel,
};
use aether_data_contracts::repository::global_models::AdminProviderModelListQuery;
use std::collections::BTreeMap;

pub(crate) async fn build_admin_system_export_providers_payload(
    state: &AdminAppState<'_>,
    global_model_name_by_id: &BTreeMap<String, String>,
) -> Result<Vec<AdminSystemConfigProvider>, GatewayError> {
    let providers = state.list_provider_catalog_providers(false).await?;
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await?;
    let keys = state
        .list_provider_catalog_keys_by_provider_ids(&provider_ids)
        .await?;

    let mut endpoints_by_provider = BTreeMap::<String, Vec<_>>::new();
    for endpoint in endpoints {
        endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }
    let mut keys_by_provider = BTreeMap::<String, Vec<_>>::new();
    for key in keys {
        keys_by_provider
            .entry(key.provider_id.clone())
            .or_default()
            .push(key);
    }

    let mut provider_models_by_provider = BTreeMap::<String, Vec<_>>::new();
    for provider in &providers {
        let models = state
            .list_admin_provider_models(&AdminProviderModelListQuery {
                provider_id: provider.id.clone(),
                is_active: None,
                offset: 0,
                limit: ADMIN_SYSTEM_EXPORT_PAGE_LIMIT,
            })
            .await?;
        provider_models_by_provider.insert(provider.id.clone(), models);
    }

    Ok(providers
        .iter()
        .map(|provider| {
            let endpoints = endpoints_by_provider
                .remove(&provider.id)
                .unwrap_or_default();
            let provider_endpoint_formats =
                collect_admin_system_export_provider_endpoint_formats(&endpoints);
            let endpoints_data = endpoints
                .iter()
                .map(|endpoint| AdminSystemConfigEndpoint {
                    api_format: endpoint.api_format.clone(),
                    base_url: endpoint.base_url.clone(),
                    header_rules: endpoint.header_rules.clone(),
                    body_rules: endpoint.body_rules.clone(),
                    max_retries: endpoint.max_retries,
                    is_active: endpoint.is_active,
                    custom_path: endpoint.custom_path.clone(),
                    config: endpoint.config.clone(),
                    format_acceptance_config: endpoint.format_acceptance_config.clone(),
                    proxy: endpoint.proxy.clone(),
                })
                .collect::<Vec<_>>();

            let mut keys = keys_by_provider.remove(&provider.id).unwrap_or_default();
            keys.sort_by(|left, right| {
                left.internal_priority
                    .cmp(&right.internal_priority)
                    .then(
                        left.created_at_unix_ms
                            .unwrap_or(0)
                            .cmp(&right.created_at_unix_ms.unwrap_or(0)),
                    )
                    .then(left.id.cmp(&right.id))
            });
            let keys_data = keys
                .iter()
                .map(|key| {
                    let api_formats = resolve_admin_system_export_key_api_formats(
                        key.api_formats.as_ref(),
                        &provider_endpoint_formats,
                    );
                    let auth_config = key
                        .encrypted_auth_config
                        .as_deref()
                        .and_then(|ciphertext| {
                            decrypt_admin_system_export_secret(state, ciphertext)
                        })
                        .map(serde_json::Value::String);
                    AdminSystemConfigProviderKey {
                        api_key: key.encrypted_api_key.as_deref().map(|ciphertext| {
                            decrypt_admin_system_export_secret(state, ciphertext)
                                .unwrap_or_default()
                        }),
                        auth_type: Some(key.auth_type.clone()),
                        auth_config,
                        name: Some(key.name.clone()),
                        note: key.note.clone(),
                        api_formats: Some(api_formats.clone()),
                        supported_endpoints: Some(api_formats),
                        rate_multipliers: key.rate_multipliers.clone(),
                        internal_priority: Some(key.internal_priority),
                        global_priority_by_format: key.global_priority_by_format.clone(),
                        auth_type_by_format: key.auth_type_by_format.clone(),
                        allow_auth_channel_mismatch_formats: key
                            .allow_auth_channel_mismatch_formats
                            .as_ref()
                            .and_then(serde_json::Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(ToOwned::to_owned)
                                    .collect::<Vec<_>>()
                            }),
                        rpm_limit: key.rpm_limit,
                        allowed_models: key.allowed_models.as_ref().and_then(|value| {
                            value.as_array().map(|items| {
                                items
                                    .iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(ToOwned::to_owned)
                                    .collect::<Vec<_>>()
                            })
                        }),
                        capabilities: key.capabilities.clone(),
                        cache_ttl_minutes: Some(key.cache_ttl_minutes),
                        max_probe_interval_minutes: Some(key.max_probe_interval_minutes),
                        auto_fetch_models: Some(key.auto_fetch_models),
                        locked_models: key.locked_models.as_ref().and_then(|value| {
                            value.as_array().map(|items| {
                                items
                                    .iter()
                                    .filter_map(serde_json::Value::as_str)
                                    .map(ToOwned::to_owned)
                                    .collect::<Vec<_>>()
                            })
                        }),
                        model_include_patterns: key.model_include_patterns.as_ref().and_then(
                            |value| {
                                value.as_array().map(|items| {
                                    items
                                        .iter()
                                        .filter_map(serde_json::Value::as_str)
                                        .map(ToOwned::to_owned)
                                        .collect::<Vec<_>>()
                                })
                            },
                        ),
                        model_exclude_patterns: key.model_exclude_patterns.as_ref().and_then(
                            |value| {
                                value.as_array().map(|items| {
                                    items
                                        .iter()
                                        .filter_map(serde_json::Value::as_str)
                                        .map(ToOwned::to_owned)
                                        .collect::<Vec<_>>()
                                })
                            },
                        ),
                        is_active: key.is_active,
                        proxy: key.proxy.clone(),
                        fingerprint: key.fingerprint.clone(),
                    }
                })
                .collect::<Vec<_>>();

            let models_data = provider_models_by_provider
                .remove(&provider.id)
                .unwrap_or_default()
                .into_iter()
                .map(|model| AdminSystemConfigProviderModel {
                    global_model_name: global_model_name_by_id.get(&model.global_model_id).cloned(),
                    provider_model_name: model.provider_model_name,
                    provider_model_mappings: model.provider_model_mappings,
                    price_per_request: model.price_per_request,
                    tiered_pricing: model.tiered_pricing,
                    supports_vision: model.supports_vision,
                    supports_function_calling: model.supports_function_calling,
                    supports_streaming: model.supports_streaming,
                    supports_extended_thinking: model.supports_extended_thinking,
                    supports_image_generation: model.supports_image_generation,
                    is_active: model.is_active,
                    config: model.config,
                })
                .collect::<Vec<_>>();

            AdminSystemConfigProvider {
                name: provider.name.clone(),
                description: provider.description.clone(),
                website: provider.website.clone(),
                provider_type: Some(provider.provider_type.clone()),
                billing_type: provider.billing_type.clone(),
                monthly_quota_usd: provider.monthly_quota_usd,
                quota_reset_day: provider.quota_reset_day,
                provider_priority: Some(provider.provider_priority),
                keep_priority_on_conversion: Some(provider.keep_priority_on_conversion),
                enable_format_conversion: Some(provider.enable_format_conversion),
                is_active: provider.is_active,
                concurrent_limit: provider.concurrent_limit,
                max_retries: provider.max_retries,
                stream_first_byte_timeout: provider.stream_first_byte_timeout_secs,
                request_timeout: provider.request_timeout_secs,
                proxy: provider.proxy.clone(),
                config: decrypt_admin_system_export_provider_config(
                    state,
                    provider.config.as_ref(),
                ),
                endpoints: endpoints_data,
                api_keys: keys_data,
                models: models_data,
            }
        })
        .collect::<Vec<_>>())
}
