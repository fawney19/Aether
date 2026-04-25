use aether_data_contracts::repository::global_models::{
    CreateAdminGlobalModelRecord, StoredAdminProviderModel, UpsertAdminProviderModelRecord,
};
use serde_json::{json, Value};

pub fn normalize_required_trimmed_string(value: &str, field_name: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field_name} 不能为空"));
    }
    Ok(trimmed.to_string())
}

pub fn normalize_optional_price(
    value: Option<f64>,
    field_name: &str,
) -> Result<Option<f64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{field_name} 必须是非负数"));
    }
    Ok(Some(value))
}

#[allow(clippy::too_many_arguments)]
pub fn build_admin_provider_model_create_record(
    id: String,
    provider_id: String,
    global_model_id: String,
    provider_model_name: String,
    provider_model_mappings: Option<serde_json::Value>,
    price_per_request: Option<f64>,
    tiered_pricing: Option<serde_json::Value>,
    supports_vision: Option<bool>,
    supports_function_calling: Option<bool>,
    supports_streaming: Option<bool>,
    supports_extended_thinking: Option<bool>,
    supports_image_generation: Option<bool>,
    is_active: Option<bool>,
    config: Option<serde_json::Value>,
) -> Result<UpsertAdminProviderModelRecord, String> {
    UpsertAdminProviderModelRecord::new(
        id,
        provider_id,
        global_model_id,
        provider_model_name,
        provider_model_mappings,
        price_per_request,
        tiered_pricing,
        supports_vision,
        supports_function_calling,
        supports_streaming,
        supports_extended_thinking,
        supports_image_generation,
        is_active.unwrap_or(true),
        true,
        config,
    )
    .map_err(|err| err.to_string())
}

#[allow(clippy::too_many_arguments)]
pub fn build_admin_provider_model_update_record(
    existing: &StoredAdminProviderModel,
    global_model_id: String,
    provider_model_name: String,
    provider_model_mappings: Option<serde_json::Value>,
    price_per_request: Option<f64>,
    tiered_pricing: Option<serde_json::Value>,
    supports_vision: Option<bool>,
    supports_function_calling: Option<bool>,
    supports_streaming: Option<bool>,
    supports_extended_thinking: Option<bool>,
    supports_image_generation: Option<bool>,
    is_active: bool,
    is_available: bool,
    config: Option<serde_json::Value>,
) -> Result<UpsertAdminProviderModelRecord, String> {
    UpsertAdminProviderModelRecord::new(
        existing.id.clone(),
        existing.provider_id.clone(),
        global_model_id,
        provider_model_name,
        provider_model_mappings,
        price_per_request,
        tiered_pricing,
        supports_vision,
        supports_function_calling,
        supports_streaming,
        supports_extended_thinking,
        supports_image_generation,
        is_active,
        is_available,
        config,
    )
    .map_err(|err| err.to_string())
}

pub fn normalize_admin_import_model_id(model_id: &str) -> Result<String, String> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() || trimmed.len() > 100 {
        return Err("Invalid model_id: must be 1-100 characters".to_string());
    }
    Ok(trimmed.to_string())
}

pub fn default_admin_import_tiered_pricing() -> Value {
    json!({
        "tiers": [{
            "up_to": null,
            "input_price_per_1m": 0.0,
            "output_price_per_1m": 0.0,
        }]
    })
}

pub fn build_admin_import_global_model_record(
    id: String,
    model_name: String,
    price_per_request: Option<f64>,
    tiered_pricing: Option<Value>,
) -> Result<CreateAdminGlobalModelRecord, String> {
    CreateAdminGlobalModelRecord::new(
        id,
        model_name.clone(),
        model_name,
        true,
        price_per_request,
        tiered_pricing.or_else(|| Some(default_admin_import_tiered_pricing())),
        None,
        None,
    )
    .map_err(|err| err.to_string())
}

pub fn build_admin_import_provider_model_record(
    id: String,
    provider_id: String,
    global_model_id: String,
    provider_model_name: String,
    price_per_request: Option<f64>,
    tiered_pricing: Option<Value>,
) -> Result<UpsertAdminProviderModelRecord, String> {
    UpsertAdminProviderModelRecord::new(
        id,
        provider_id,
        global_model_id,
        provider_model_name,
        None,
        price_per_request,
        tiered_pricing,
        None,
        None,
        None,
        None,
        None,
        true,
        true,
        None,
    )
    .map_err(|err| err.to_string())
}

pub fn build_admin_batch_assign_provider_model_record(
    id: String,
    provider_id: String,
    global_model_id: String,
    provider_model_name: String,
) -> Result<UpsertAdminProviderModelRecord, String> {
    UpsertAdminProviderModelRecord::new(
        id,
        provider_id,
        global_model_id,
        provider_model_name,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        true,
        true,
        None,
    )
    .map_err(|err| err.to_string())
}
