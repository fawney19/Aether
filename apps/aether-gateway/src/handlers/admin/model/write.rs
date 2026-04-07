use super::payloads::{normalize_optional_price, normalize_required_trimmed_string};
use crate::handlers::admin::model::shared::{
    AdminGlobalModelCreateRequest, AdminGlobalModelUpdateRequest,
};
use crate::handlers::admin::shared::{normalize_json_object, normalize_string_list};
use crate::AppState;
use aether_data_contracts::repository::global_models::{
    CreateAdminGlobalModelRecord, StoredAdminGlobalModel, UpdateAdminGlobalModelRecord,
};
use serde_json::json;
use uuid::Uuid;

pub(crate) async fn build_admin_global_model_create_record(
    state: &AppState,
    payload: AdminGlobalModelCreateRequest,
) -> Result<CreateAdminGlobalModelRecord, String> {
    let name = normalize_required_trimmed_string(&payload.name, "name")?;
    let display_name = normalize_required_trimmed_string(&payload.display_name, "display_name")?;
    if state
        .get_admin_global_model_by_name(&name)
        .await
        .map_err(|err| format!("{err:?}"))?
        .is_some()
    {
        return Err(format!("GlobalModel '{name}' 已存在"));
    }
    let default_price_per_request = normalize_optional_price(
        payload.default_price_per_request,
        "default_price_per_request",
    )?;
    let default_tiered_pricing =
        normalize_json_object(payload.default_tiered_pricing, "default_tiered_pricing")?;
    let supported_capabilities =
        normalize_string_list(payload.supported_capabilities).map(|value| json!(value));
    let config = normalize_json_object(payload.config, "config")?;
    CreateAdminGlobalModelRecord::new(
        Uuid::new_v4().to_string(),
        name,
        display_name,
        payload.is_active.unwrap_or(true),
        default_price_per_request,
        default_tiered_pricing,
        supported_capabilities,
        config,
    )
    .map_err(|err| err.to_string())
}

pub(crate) async fn build_admin_global_model_update_record(
    _state: &AppState,
    existing: &StoredAdminGlobalModel,
    raw_payload: &serde_json::Map<String, serde_json::Value>,
    payload: AdminGlobalModelUpdateRequest,
) -> Result<UpdateAdminGlobalModelRecord, String> {
    let display_name = if let Some(value) = raw_payload.get("display_name") {
        let Some(display_name) = payload.display_name.as_deref() else {
            return Err(if value.is_null() {
                "display_name 不能为空".to_string()
            } else {
                "display_name 必须是字符串".to_string()
            });
        };
        normalize_required_trimmed_string(display_name, "display_name")?
    } else {
        existing.display_name.clone()
    };

    let default_price_per_request = if raw_payload.contains_key("default_price_per_request") {
        normalize_optional_price(
            payload.default_price_per_request,
            "default_price_per_request",
        )?
    } else {
        existing.default_price_per_request
    };

    let default_tiered_pricing = if raw_payload.contains_key("default_tiered_pricing") {
        normalize_json_object(payload.default_tiered_pricing, "default_tiered_pricing")?
    } else {
        existing.default_tiered_pricing.clone()
    };

    let supported_capabilities = if raw_payload.contains_key("supported_capabilities") {
        normalize_string_list(payload.supported_capabilities).map(|value| json!(value))
    } else {
        existing.supported_capabilities.clone()
    };

    let config = if raw_payload.contains_key("config") {
        normalize_json_object(payload.config, "config")?
    } else {
        existing.config.clone()
    };

    UpdateAdminGlobalModelRecord::new(
        existing.id.clone(),
        display_name,
        payload.is_active.unwrap_or(existing.is_active),
        default_price_per_request,
        default_tiered_pricing,
        supported_capabilities,
        config,
    )
    .map_err(|err| err.to_string())
}
