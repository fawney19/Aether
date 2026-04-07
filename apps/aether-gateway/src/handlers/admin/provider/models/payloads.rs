use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::global_models::{
    AdminProviderModelListQuery, StoredAdminProviderModel,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn model_tiered_pricing_first_tier_value(
    tiered_pricing: Option<&serde_json::Value>,
    field_name: &str,
) -> Option<f64> {
    tiered_pricing
        .and_then(|value| value.get("tiers"))
        .and_then(serde_json::Value::as_array)
        .and_then(|tiers| tiers.first())
        .and_then(|tier| tier.get(field_name))
        .and_then(serde_json::Value::as_f64)
}

fn model_effective_capability(
    explicit: Option<bool>,
    global_model_config: Option<&serde_json::Value>,
    config_key: &str,
) -> bool {
    explicit.unwrap_or_else(|| {
        global_model_config
            .and_then(|value| value.get(config_key))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    })
}

fn merge_json_values(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) => {
            for (key, value) in overlay_map {
                match base_map.get_mut(&key) {
                    Some(existing) => merge_json_values(existing, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn merge_admin_provider_model_effective_config(
    model: &StoredAdminProviderModel,
) -> Option<serde_json::Value> {
    let mut merged = match model.global_model_config.clone() {
        Some(serde_json::Value::Object(map)) => serde_json::Value::Object(map),
        Some(other) => other,
        None => serde_json::Value::Object(serde_json::Map::new()),
    };

    if let Some(config) = model.config.clone() {
        merge_json_values(&mut merged, config);
    }

    match merged {
        serde_json::Value::Null => None,
        serde_json::Value::Object(ref map) if map.is_empty() => None,
        value => Some(value),
    }
}

fn timestamp_or_now(value: Option<u64>, now_unix_secs: u64) -> serde_json::Value {
    unix_secs_to_rfc3339(value.unwrap_or(now_unix_secs))
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

pub(super) fn admin_provider_model_effective_input_price(
    model: &StoredAdminProviderModel,
) -> Option<f64> {
    model_tiered_pricing_first_tier_value(model.tiered_pricing.as_ref(), "input_price_per_1m")
        .or_else(|| {
            model_tiered_pricing_first_tier_value(
                model.global_model_default_tiered_pricing.as_ref(),
                "input_price_per_1m",
            )
        })
}

pub(super) fn admin_provider_model_effective_output_price(
    model: &StoredAdminProviderModel,
) -> Option<f64> {
    model_tiered_pricing_first_tier_value(model.tiered_pricing.as_ref(), "output_price_per_1m")
        .or_else(|| {
            model_tiered_pricing_first_tier_value(
                model.global_model_default_tiered_pricing.as_ref(),
                "output_price_per_1m",
            )
        })
}

pub(super) fn admin_provider_model_effective_capability(
    model: &StoredAdminProviderModel,
    capability: &str,
) -> bool {
    match capability {
        "vision" => model_effective_capability(
            model.supports_vision,
            model.global_model_config.as_ref(),
            "vision",
        ),
        "function_calling" => model_effective_capability(
            model.supports_function_calling,
            model.global_model_config.as_ref(),
            "function_calling",
        ),
        "streaming" => model_effective_capability(
            model.supports_streaming,
            model.global_model_config.as_ref(),
            "streaming",
        ),
        "extended_thinking" => model_effective_capability(
            model.supports_extended_thinking,
            model.global_model_config.as_ref(),
            "extended_thinking",
        ),
        "image_generation" => model_effective_capability(
            model.supports_image_generation,
            model.global_model_config.as_ref(),
            "image_generation",
        ),
        _ => false,
    }
}

pub(super) fn build_admin_provider_model_response(
    model: &StoredAdminProviderModel,
    now_unix_secs: u64,
) -> serde_json::Value {
    let effective_tiered_pricing = model
        .tiered_pricing
        .clone()
        .or_else(|| model.global_model_default_tiered_pricing.clone());
    let effective_config = merge_admin_provider_model_effective_config(model);

    json!({
        "id": &model.id,
        "provider_id": &model.provider_id,
        "global_model_id": &model.global_model_id,
        "provider_model_name": &model.provider_model_name,
        "provider_model_mappings": model.provider_model_mappings.clone(),
        "price_per_request": model.price_per_request,
        "tiered_pricing": model.tiered_pricing.clone(),
        "effective_tiered_pricing": effective_tiered_pricing,
        "effective_input_price": admin_provider_model_effective_input_price(model),
        "effective_output_price": admin_provider_model_effective_output_price(model),
        "effective_price_per_request": model
            .price_per_request
            .or(model.global_model_default_price_per_request),
        "supports_vision": model.supports_vision,
        "supports_function_calling": model.supports_function_calling,
        "supports_streaming": model.supports_streaming,
        "supports_extended_thinking": model.supports_extended_thinking,
        "supports_image_generation": model.supports_image_generation,
        "effective_supports_vision": admin_provider_model_effective_capability(model, "vision"),
        "effective_supports_function_calling": admin_provider_model_effective_capability(
            model,
            "function_calling",
        ),
        "effective_supports_streaming": admin_provider_model_effective_capability(model, "streaming"),
        "effective_supports_extended_thinking": admin_provider_model_effective_capability(
            model,
            "extended_thinking",
        ),
        "effective_supports_image_generation": admin_provider_model_effective_capability(
            model,
            "image_generation",
        ),
        "is_active": model.is_active,
        "is_available": model.is_available,
        "config": model.config.clone(),
        "effective_config": effective_config,
        "global_model_name": model.global_model_name.clone(),
        "global_model_display_name": model.global_model_display_name.clone(),
        "created_at": timestamp_or_now(model.created_at_unix_secs, now_unix_secs),
        "updated_at": timestamp_or_now(model.updated_at_unix_secs, now_unix_secs),
    })
}

pub(super) async fn build_admin_provider_models_payload(
    state: &AppState,
    provider_id: &str,
    skip: usize,
    limit: usize,
    is_active: Option<bool>,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() || !state.has_global_model_data_reader() {
        return None;
    }
    let provider = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await
        .ok()?
        .into_iter()
        .next()?;
    let mut models = state
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: provider.id,
            is_active,
            offset: skip,
            limit,
        })
        .await
        .ok()?;
    models.sort_by(|left, right| {
        left.provider_model_name
            .cmp(&right.provider_model_name)
            .then_with(|| left.id.cmp(&right.id))
    });
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Some(serde_json::Value::Array(
        models
            .iter()
            .map(|model| build_admin_provider_model_response(model, now_unix_secs))
            .collect(),
    ))
}

pub(super) async fn build_admin_provider_model_payload(
    state: &AppState,
    provider_id: &str,
    model_id: &str,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() {
        return None;
    }
    let model = state
        .get_admin_provider_model(provider_id, model_id)
        .await
        .ok()??;
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Some(build_admin_provider_model_response(&model, now_unix_secs))
}

pub(super) async fn admin_provider_model_name_exists(
    state: &AppState,
    provider_id: &str,
    provider_model_name: &str,
    exclude_model_id: Option<&str>,
) -> Result<bool, GatewayError> {
    let target = provider_model_name.trim();
    if target.is_empty() {
        return Ok(false);
    }
    let models = state
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: provider_id.to_string(),
            is_active: None,
            offset: 0,
            limit: 10_000,
        })
        .await?;
    Ok(models.into_iter().any(|model| {
        model.provider_model_name == target
            && exclude_model_id.is_none_or(|exclude| model.id != exclude)
    }))
}
