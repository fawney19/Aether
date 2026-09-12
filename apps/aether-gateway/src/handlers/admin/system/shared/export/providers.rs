use super::support::{
    collect_admin_system_export_provider_endpoint_formats, project_admin_system_export_body_rules,
    project_admin_system_export_header_rules, project_admin_system_export_json,
    project_admin_system_export_optional_url, project_admin_system_export_provider_config,
    project_admin_system_export_proxy, project_admin_system_export_url,
    resolve_admin_system_export_key_api_formats,
};
use crate::handlers::admin::admin_provider_ops_credential_snapshot;
use crate::handlers::admin::request::{
    AdminAppState, SystemExportMode, ADMIN_SYSTEM_EXPORT_CREDENTIALS_NOT_EXPORTED,
};
use crate::GatewayError;
use aether_admin::system::{
    AdminSystemConfigEndpoint, AdminSystemConfigProvider, AdminSystemConfigProviderKey,
    AdminSystemConfigProviderModel,
};
use std::collections::BTreeMap;

pub(crate) async fn build_admin_system_export_providers_payload(
    state: &AdminAppState<'_>,
    global_model_name_by_id: &BTreeMap<String, String>,
    mode: SystemExportMode,
) -> Result<Vec<AdminSystemConfigProvider>, GatewayError> {
    let mut providers = state.list_provider_catalog_providers(false).await?;
    let mut provider_ops_credentials = BTreeMap::new();
    if mode.credentials_are_exported() {
        for provider in &mut providers {
            let has_provider_ops = provider
                .config
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .and_then(|config| config.get("provider_ops"))
                .is_some();
            if !has_provider_ops {
                continue;
            }
            let snapshot = admin_provider_ops_credential_snapshot(state, provider).await?;
            provider_ops_credentials.insert(provider.id.clone(), snapshot.credentials);
            *provider = snapshot.provider;
        }
    }
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
            .list_all_admin_provider_models_for_system_transfer(&provider.id)
            .await?;
        provider_models_by_provider.insert(provider.id.clone(), models);
    }

    providers
        .iter()
        .map(
            |provider| -> Result<AdminSystemConfigProvider, GatewayError> {
                let endpoints = endpoints_by_provider
                    .remove(&provider.id)
                    .unwrap_or_default();
                let provider_endpoint_formats =
                    collect_admin_system_export_provider_endpoint_formats(&endpoints);
                let endpoints_data = endpoints
                    .iter()
                    .map(|endpoint| AdminSystemConfigEndpoint {
                        api_format: endpoint.api_format.clone(),
                        base_url: project_admin_system_export_url(mode, &endpoint.base_url),
                        header_rules: project_admin_system_export_header_rules(
                            mode,
                            endpoint.header_rules.as_ref(),
                        ),
                        body_rules: project_admin_system_export_body_rules(
                            mode,
                            endpoint.body_rules.as_ref(),
                        ),
                        max_retries: endpoint.max_retries,
                        is_active: endpoint.is_active,
                        custom_path: endpoint.custom_path.clone(),
                        config: project_admin_system_export_json(mode, endpoint.config.as_ref()),
                        format_acceptance_config: project_admin_system_export_json(
                            mode,
                            endpoint.format_acceptance_config.as_ref(),
                        ),
                        proxy: project_admin_system_export_proxy(mode, endpoint.proxy.as_ref()),
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
                    .map(
                        |key| -> Result<AdminSystemConfigProviderKey, GatewayError> {
                            let api_formats = resolve_admin_system_export_key_api_formats(
                                key.api_formats.as_ref(),
                                &provider_endpoint_formats,
                            );
                            let auth_config = if mode.credentials_are_exported() {
                                state
                                    .app()
                                    .decrypt_provider_catalog_key_auth_config(key)?
                                    .map(serde_json::Value::String)
                            } else {
                                None
                            };
                            let api_key = if mode.credentials_are_exported() {
                                state.app().decrypt_provider_catalog_key_api_key(key)?
                            } else {
                                None
                            };
                            Ok(AdminSystemConfigProviderKey {
                                api_key,
                                auth_type: Some(key.auth_type.clone()),
                                auth_config,
                                name: Some(key.name.clone()),
                                note: key.note.clone(),
                                api_formats: Some(api_formats.clone()),
                                supported_endpoints: Some(api_formats),
                                rate_multipliers: project_admin_system_export_json(
                                    mode,
                                    key.rate_multipliers.as_ref(),
                                ),
                                internal_priority: Some(key.internal_priority),
                                global_priority_by_format: project_admin_system_export_json(
                                    mode,
                                    key.global_priority_by_format.as_ref(),
                                ),
                                auth_type_by_format: project_admin_system_export_json(
                                    mode,
                                    key.auth_type_by_format.as_ref(),
                                ),
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
                                capabilities: project_admin_system_export_json(
                                    mode,
                                    key.capabilities.as_ref(),
                                ),
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
                                model_include_patterns: key
                                    .model_include_patterns
                                    .as_ref()
                                    .and_then(|value| {
                                        value.as_array().map(|items| {
                                            items
                                                .iter()
                                                .filter_map(serde_json::Value::as_str)
                                                .map(ToOwned::to_owned)
                                                .collect::<Vec<_>>()
                                        })
                                    }),
                                model_exclude_patterns: key
                                    .model_exclude_patterns
                                    .as_ref()
                                    .and_then(|value| {
                                        value.as_array().map(|items| {
                                            items
                                                .iter()
                                                .filter_map(serde_json::Value::as_str)
                                                .map(ToOwned::to_owned)
                                                .collect::<Vec<_>>()
                                        })
                                    }),
                                is_active: mode.preserves_active_state() && key.is_active,
                                proxy: project_admin_system_export_proxy(mode, key.proxy.as_ref()),
                                fingerprint: project_admin_system_export_json(
                                    mode,
                                    key.fingerprint.as_ref(),
                                ),
                                credential_state: (!mode.credentials_are_exported()).then(|| {
                                    ADMIN_SYSTEM_EXPORT_CREDENTIALS_NOT_EXPORTED.to_string()
                                }),
                            })
                        },
                    )
                    .collect::<Result<Vec<_>, GatewayError>>()?;

                let models_data = provider_models_by_provider
                    .remove(&provider.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|model| AdminSystemConfigProviderModel {
                        global_model_name: global_model_name_by_id
                            .get(&model.global_model_id)
                            .cloned(),
                        provider_model_name: model.provider_model_name,
                        provider_model_mappings: project_admin_system_export_json(
                            mode,
                            model.provider_model_mappings.as_ref(),
                        ),
                        price_per_request: model.price_per_request,
                        tiered_pricing: project_admin_system_export_json(
                            mode,
                            model.tiered_pricing.as_ref(),
                        ),
                        supports_vision: model.supports_vision,
                        supports_function_calling: model.supports_function_calling,
                        supports_streaming: model.supports_streaming,
                        supports_extended_thinking: model.supports_extended_thinking,
                        supports_image_generation: model.supports_image_generation,
                        is_active: model.is_active,
                        config: project_admin_system_export_json(mode, model.config.as_ref()),
                    })
                    .collect::<Vec<_>>();

                Ok(AdminSystemConfigProvider {
                    name: provider.name.clone(),
                    description: provider.description.clone(),
                    website: project_admin_system_export_optional_url(
                        mode,
                        provider.website.as_deref(),
                    ),
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
                    proxy: project_admin_system_export_proxy(mode, provider.proxy.as_ref()),
                    config: project_admin_system_export_provider_config(
                        state,
                        mode,
                        provider.config.as_ref(),
                        provider_ops_credentials.get(&provider.id),
                    )?,
                    endpoints: endpoints_data,
                    api_keys: keys_data,
                    models: models_data,
                })
            },
        )
        .collect::<Result<Vec<_>, GatewayError>>()
}
