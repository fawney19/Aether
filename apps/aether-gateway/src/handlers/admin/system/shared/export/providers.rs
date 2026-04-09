use super::support::{
    collect_admin_system_export_provider_endpoint_formats,
    decrypt_admin_system_export_provider_config, decrypt_admin_system_export_secret,
    resolve_admin_system_export_key_api_formats, ADMIN_SYSTEM_EXPORT_PAGE_LIMIT,
};
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::global_models::AdminProviderModelListQuery;
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) async fn build_admin_system_export_providers_payload(
    state: &AdminAppState<'_>,
    global_model_name_by_id: &BTreeMap<String, String>,
) -> Result<Vec<serde_json::Value>, GatewayError> {
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
            let endpoints = endpoints_by_provider.remove(&provider.id).unwrap_or_default();
            let provider_endpoint_formats =
                collect_admin_system_export_provider_endpoint_formats(&endpoints);
            let endpoints_data = endpoints
                .iter()
                .map(|endpoint| {
                    json!({
                        "api_format": endpoint.api_format,
                        "base_url": endpoint.base_url,
                        "header_rules": endpoint.header_rules,
                        "body_rules": endpoint.body_rules,
                        "max_retries": endpoint.max_retries,
                        "is_active": endpoint.is_active,
                        "custom_path": endpoint.custom_path,
                        "config": endpoint.config,
                        "format_acceptance_config": endpoint.format_acceptance_config,
                        "proxy": endpoint.proxy,
                    })
                })
                .collect::<Vec<_>>();

            let mut keys = keys_by_provider.remove(&provider.id).unwrap_or_default();
            keys.sort_by(|left, right| {
                left.internal_priority
                    .cmp(&right.internal_priority)
                    .then(
                        left.created_at_unix_secs
                            .unwrap_or(0)
                            .cmp(&right.created_at_unix_secs.unwrap_or(0)),
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
                    let mut payload = json!({
                        "api_formats": api_formats,
                        "supported_endpoints": api_formats,
                        "auth_type": key.auth_type,
                        "name": key.name,
                        "note": key.note,
                        "rate_multipliers": key.rate_multipliers,
                        "internal_priority": key.internal_priority,
                        "global_priority_by_format": key.global_priority_by_format,
                        "rpm_limit": key.rpm_limit,
                        "allowed_models": key.allowed_models,
                        "capabilities": key.capabilities,
                        "cache_ttl_minutes": key.cache_ttl_minutes,
                        "max_probe_interval_minutes": key.max_probe_interval_minutes,
                        "is_active": key.is_active,
                        "proxy": key.proxy,
                        "fingerprint": key.fingerprint,
                        "auto_fetch_models": key.auto_fetch_models,
                        "locked_models": key.locked_models,
                        "model_include_patterns": key.model_include_patterns,
                        "model_exclude_patterns": key.model_exclude_patterns,
                        "api_key": decrypt_admin_system_export_secret(state, &key.encrypted_api_key)
                            .unwrap_or_default(),
                    });
                    if let Some(ciphertext) = key.encrypted_auth_config.as_deref() {
                        if let Some(plaintext) =
                            decrypt_admin_system_export_secret(state, ciphertext)
                        {
                            payload["auth_config"] = json!(plaintext);
                        }
                    }
                    payload
                })
                .collect::<Vec<_>>();

            let models_data = provider_models_by_provider
                .remove(&provider.id)
                .unwrap_or_default()
                .into_iter()
                .map(|model| {
                    json!({
                        "provider_model_name": model.provider_model_name,
                        "provider_model_mappings": model.provider_model_mappings,
                        "price_per_request": model.price_per_request,
                        "tiered_pricing": model.tiered_pricing,
                        "supports_vision": model.supports_vision,
                        "supports_function_calling": model.supports_function_calling,
                        "supports_streaming": model.supports_streaming,
                        "supports_extended_thinking": model.supports_extended_thinking,
                        "supports_image_generation": model.supports_image_generation,
                        "is_active": model.is_active,
                        "config": model.config,
                        "global_model_name": global_model_name_by_id.get(&model.global_model_id),
                    })
                })
                .collect::<Vec<_>>();

            json!({
                "name": provider.name,
                "description": provider.description,
                "website": provider.website,
                "provider_type": provider.provider_type,
                "billing_type": provider.billing_type,
                "monthly_quota_usd": provider.monthly_quota_usd,
                "quota_reset_day": provider.quota_reset_day,
                "provider_priority": provider.provider_priority,
                "keep_priority_on_conversion": provider.keep_priority_on_conversion,
                "enable_format_conversion": provider.enable_format_conversion,
                "is_active": provider.is_active,
                "concurrent_limit": provider.concurrent_limit,
                "max_retries": provider.max_retries,
                "proxy": provider.proxy,
                "request_timeout": provider.request_timeout_secs,
                "stream_first_byte_timeout": provider.stream_first_byte_timeout_secs,
                "config": decrypt_admin_system_export_provider_config(state, provider.config.as_ref()),
                "endpoints": endpoints_data,
                "api_keys": keys_data,
                "models": models_data,
            })
        })
        .collect::<Vec<_>>())
}
