use aether_data_contracts::repository::global_models::StoredAdminProviderModel;
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    let timestamp = i64::try_from(unix_secs).ok()?;
    Some(
        chrono::DateTime::<Utc>::from_timestamp(timestamp, 0)?
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

fn model_tiered_pricing_first_tier_value(
    tiered_pricing: Option<&Value>,
    field_name: &str,
) -> Option<f64> {
    tiered_pricing
        .and_then(|value| value.get("tiers"))
        .and_then(Value::as_array)
        .and_then(|tiers| tiers.first())
        .and_then(|tier| tier.get(field_name))
        .and_then(Value::as_f64)
}

fn model_effective_capability(
    explicit: Option<bool>,
    global_model_config: Option<&Value>,
    config_key: &str,
) -> bool {
    explicit.unwrap_or_else(|| {
        global_model_config
            .and_then(|value| value.get(config_key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

fn merge_json_values(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
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

fn merge_admin_provider_model_effective_config(model: &StoredAdminProviderModel) -> Option<Value> {
    let mut merged = match model.global_model_config.clone() {
        Some(Value::Object(map)) => Value::Object(map),
        Some(other) => other,
        None => Value::Object(Map::new()),
    };

    if let Some(config) = model.config.clone() {
        merge_json_values(&mut merged, config);
    }

    match merged {
        Value::Null => None,
        Value::Object(ref map) if map.is_empty() => None,
        value => Some(value),
    }
}

fn timestamp_or_now(value: Option<u64>, now_unix_secs: u64) -> Value {
    unix_secs_to_rfc3339(value.unwrap_or(now_unix_secs))
        .map(Value::String)
        .unwrap_or(Value::Null)
}

pub fn admin_provider_model_effective_input_price(model: &StoredAdminProviderModel) -> Option<f64> {
    model_tiered_pricing_first_tier_value(model.tiered_pricing.as_ref(), "input_price_per_1m")
        .or_else(|| {
            model_tiered_pricing_first_tier_value(
                model.global_model_default_tiered_pricing.as_ref(),
                "input_price_per_1m",
            )
        })
}

pub fn admin_provider_model_effective_output_price(
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

pub fn admin_provider_model_effective_capability(
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

pub fn build_admin_provider_model_response(
    model: &StoredAdminProviderModel,
    now_unix_secs: u64,
) -> Value {
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

pub fn build_admin_provider_available_source_models_payload(
    models: Vec<StoredAdminProviderModel>,
) -> Value {
    let mut by_global_model = BTreeMap::<String, StoredAdminProviderModel>::new();
    for model in models {
        by_global_model
            .entry(model.global_model_id.clone())
            .or_insert(model);
    }
    let mut payload_models = by_global_model
        .into_values()
        .map(|model| {
            json!({
                "global_model_name": model.global_model_name,
                "display_name": model.global_model_display_name,
                "provider_model_name": model.provider_model_name,
                "model_id": model.id,
                "price": {
                    "input_price_per_1m": admin_provider_model_effective_input_price(&model),
                    "output_price_per_1m": admin_provider_model_effective_output_price(&model),
                    "cache_creation_price_per_1m": Value::Null,
                    "cache_read_price_per_1m": Value::Null,
                    "price_per_request": model.price_per_request.or(model.global_model_default_price_per_request),
                },
                "capabilities": json!({
                    "supports_vision": admin_provider_model_effective_capability(&model, "vision"),
                    "supports_function_calling": admin_provider_model_effective_capability(&model, "function_calling"),
                    "supports_streaming": admin_provider_model_effective_capability(&model, "streaming"),
                }),
                "is_active": model.is_active,
            })
        })
        .collect::<Vec<_>>();
    let total = payload_models.len();
    payload_models.sort_by(|left, right| {
        left.get("global_model_name")
            .and_then(Value::as_str)
            .cmp(&right.get("global_model_name").and_then(Value::as_str))
    });
    json!({
        "models": payload_models,
        "total": total,
    })
}
