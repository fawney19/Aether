use super::payloads::{
    admin_provider_model_effective_capability, admin_provider_model_effective_input_price,
    admin_provider_model_effective_output_price, admin_provider_model_name_exists,
};
use crate::handlers::admin::provider::shared::payloads::{
    AdminImportProviderModelsRequest, AdminProviderModelCreateRequest,
    AdminProviderModelUpdateRequest,
};
use crate::handlers::admin::shared::{
    normalize_json_array, normalize_json_object, normalize_string_list,
};
use crate::AppState;
use aether_data_contracts::repository::global_models::{
    AdminProviderModelListQuery, CreateAdminGlobalModelRecord, UpdateAdminGlobalModelRecord,
    UpsertAdminProviderModelRecord,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

fn normalize_required_trimmed_string(value: &str, field_name: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field_name} 不能为空"));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_price(value: Option<f64>, field_name: &str) -> Result<Option<f64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{field_name} 必须是非负数"));
    }
    Ok(Some(value))
}

async fn resolve_admin_global_model_by_id_or_err(
    state: &AppState,
    global_model_id: &str,
) -> Result<aether_data_contracts::repository::global_models::StoredAdminGlobalModel, String> {
    state
        .get_admin_global_model_by_id(global_model_id)
        .await
        .map_err(|err| format!("{err:?}"))?
        .ok_or_else(|| format!("GlobalModel {global_model_id} 不存在"))
}

pub(super) async fn build_admin_provider_model_create_record(
    state: &AppState,
    provider_id: &str,
    payload: AdminProviderModelCreateRequest,
) -> Result<UpsertAdminProviderModelRecord, String> {
    let provider_model_name =
        normalize_required_trimmed_string(&payload.provider_model_name, "provider_model_name")?;
    if admin_provider_model_name_exists(state, provider_id, &provider_model_name, None)
        .await
        .map_err(|err| format!("{err:?}"))?
    {
        return Err(format!("模型 '{provider_model_name}' 已存在"));
    }
    let global_model_id =
        normalize_required_trimmed_string(&payload.global_model_id, "global_model_id")?;
    resolve_admin_global_model_by_id_or_err(state, &global_model_id).await?;
    let price_per_request =
        normalize_optional_price(payload.price_per_request, "price_per_request")?;
    let tiered_pricing = normalize_json_object(payload.tiered_pricing, "tiered_pricing")?;
    let provider_model_mappings =
        normalize_json_array(payload.provider_model_mappings, "provider_model_mappings")?;
    let config = normalize_json_object(payload.config, "config")?;
    UpsertAdminProviderModelRecord::new(
        Uuid::new_v4().to_string(),
        provider_id.to_string(),
        global_model_id,
        provider_model_name,
        provider_model_mappings,
        price_per_request,
        tiered_pricing,
        payload.supports_vision,
        payload.supports_function_calling,
        payload.supports_streaming,
        payload.supports_extended_thinking,
        None,
        payload.is_active.unwrap_or(true),
        true,
        config,
    )
    .map_err(|err| err.to_string())
}

pub(super) async fn build_admin_provider_model_update_record(
    state: &AppState,
    existing: &aether_data_contracts::repository::global_models::StoredAdminProviderModel,
    raw_payload: &serde_json::Map<String, serde_json::Value>,
    payload: AdminProviderModelUpdateRequest,
) -> Result<UpsertAdminProviderModelRecord, String> {
    let provider_model_name = if let Some(value) = raw_payload.get("provider_model_name") {
        let Some(name) = payload.provider_model_name.as_deref() else {
            return Err(if value.is_null() {
                "provider_model_name 不能为空".to_string()
            } else {
                "provider_model_name 必须是字符串".to_string()
            });
        };
        let name = normalize_required_trimmed_string(name, "provider_model_name")?;
        if admin_provider_model_name_exists(state, &existing.provider_id, &name, Some(&existing.id))
            .await
            .map_err(|err| format!("{err:?}"))?
        {
            return Err(format!("模型 '{name}' 已存在"));
        }
        name
    } else {
        existing.provider_model_name.clone()
    };

    let global_model_id = if let Some(value) = raw_payload.get("global_model_id") {
        let Some(global_model_id) = payload.global_model_id.as_deref() else {
            return Err(if value.is_null() {
                "global_model_id 不能为空".to_string()
            } else {
                "global_model_id 必须是字符串".to_string()
            });
        };
        let global_model_id =
            normalize_required_trimmed_string(global_model_id, "global_model_id")?;
        resolve_admin_global_model_by_id_or_err(state, &global_model_id).await?;
        global_model_id
    } else {
        existing.global_model_id.clone()
    };

    let price_per_request = if raw_payload.contains_key("price_per_request") {
        normalize_optional_price(payload.price_per_request, "price_per_request")?
    } else {
        existing.price_per_request
    };
    let tiered_pricing = if raw_payload.contains_key("tiered_pricing") {
        normalize_json_object(payload.tiered_pricing, "tiered_pricing")?
    } else {
        existing.tiered_pricing.clone()
    };
    let provider_model_mappings = if raw_payload.contains_key("provider_model_mappings") {
        normalize_json_array(payload.provider_model_mappings, "provider_model_mappings")?
    } else {
        existing.provider_model_mappings.clone()
    };
    let config = if raw_payload.contains_key("config") {
        normalize_json_object(payload.config, "config")?
    } else {
        existing.config.clone()
    };

    UpsertAdminProviderModelRecord::new(
        existing.id.clone(),
        existing.provider_id.clone(),
        global_model_id,
        provider_model_name,
        provider_model_mappings,
        price_per_request,
        tiered_pricing,
        if raw_payload.contains_key("supports_vision") {
            payload.supports_vision
        } else {
            existing.supports_vision
        },
        if raw_payload.contains_key("supports_function_calling") {
            payload.supports_function_calling
        } else {
            existing.supports_function_calling
        },
        if raw_payload.contains_key("supports_streaming") {
            payload.supports_streaming
        } else {
            existing.supports_streaming
        },
        if raw_payload.contains_key("supports_extended_thinking") {
            payload.supports_extended_thinking
        } else {
            existing.supports_extended_thinking
        },
        existing.supports_image_generation,
        payload.is_active.unwrap_or(existing.is_active),
        payload.is_available.unwrap_or(existing.is_available),
        config,
    )
    .map_err(|err| err.to_string())
}

pub(super) async fn build_admin_provider_available_source_models_payload(
    state: &AppState,
    provider_id: &str,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() || !state.has_provider_catalog_data_reader() {
        return None;
    }
    let provider = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await
        .ok()?
        .into_iter()
        .next()?;
    let models = state
        .list_admin_provider_available_source_models(&provider.id)
        .await
        .ok()?;
    let mut by_global_model = BTreeMap::<
        String,
        aether_data_contracts::repository::global_models::StoredAdminProviderModel,
    >::new();
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
                    "cache_creation_price_per_1m": serde_json::Value::Null,
                    "cache_read_price_per_1m": serde_json::Value::Null,
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
            .and_then(serde_json::Value::as_str)
            .cmp(
                &right
                    .get("global_model_name")
                    .and_then(serde_json::Value::as_str),
            )
    });
    Some(json!({
        "models": payload_models,
        "total": total,
    }))
}

pub(super) async fn build_admin_batch_assign_global_models_payload(
    state: &AppState,
    provider_id: &str,
    global_model_ids: Vec<String>,
) -> Result<serde_json::Value, String> {
    let existing_models = state
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: provider_id.to_string(),
            is_active: None,
            offset: 0,
            limit: 10_000,
        })
        .await
        .map_err(|err| format!("{err:?}"))?;
    let existing_global_model_ids = existing_models
        .into_iter()
        .map(|model| model.global_model_id)
        .collect::<BTreeSet<_>>();

    let mut success = Vec::new();
    let mut errors = Vec::new();
    for global_model_id in global_model_ids {
        let global_model_id = global_model_id.trim().to_string();
        if global_model_id.is_empty() {
            continue;
        }
        let global_model =
            match resolve_admin_global_model_by_id_or_err(state, &global_model_id).await {
                Ok(model) => model,
                Err(detail) => {
                    errors.push(json!({
                        "global_model_id": global_model_id,
                        "error": detail,
                    }));
                    continue;
                }
            };
        if existing_global_model_ids.contains(&global_model.id) {
            errors.push(json!({
                "global_model_id": global_model.id,
                "error": "Model already exists",
            }));
            continue;
        }
        let record = UpsertAdminProviderModelRecord::new(
            Uuid::new_v4().to_string(),
            provider_id.to_string(),
            global_model.id.clone(),
            global_model.name.clone(),
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
        .map_err(|err| err.to_string())?;
        match state.create_admin_provider_model(&record).await {
            Ok(Some(created)) => success.push(json!({
                "global_model_id": global_model.id,
                "global_model_name": global_model.name,
                "provider_model_id": created.id,
            })),
            Ok(None) => errors.push(json!({
                "global_model_id": global_model.id,
                "error": "Create provider model failed",
            })),
            Err(err) => errors.push(json!({
                "global_model_id": global_model.id,
                "error": format!("{err:?}"),
            })),
        }
    }
    Ok(json!({
        "success": success,
        "errors": errors,
    }))
}

pub(super) async fn build_admin_import_provider_models_payload(
    state: &AppState,
    provider_id: &str,
    payload: AdminImportProviderModelsRequest,
) -> Result<serde_json::Value, String> {
    let default_pricing = json!({
        "tiers": [{
            "up_to": null,
            "input_price_per_1m": 0.0,
            "output_price_per_1m": 0.0,
        }]
    });
    let tiered_pricing = normalize_json_object(payload.tiered_pricing, "tiered_pricing")?;

    let existing_models = state
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: provider_id.to_string(),
            is_active: None,
            offset: 0,
            limit: 10_000,
        })
        .await
        .map_err(|err| format!("{err:?}"))?;
    let mut existing_by_name = existing_models
        .iter()
        .map(|model| (model.provider_model_name.clone(), model.clone()))
        .collect::<BTreeMap<_, _>>();

    let mut success = Vec::new();
    let mut errors = Vec::new();

    for model_id in payload.model_ids {
        let trimmed = model_id.trim();
        if trimmed.is_empty() || trimmed.len() > 100 {
            errors.push(json!({
                "model_id": if trimmed.is_empty() { "<empty>" } else { trimmed },
                "error": "Invalid model_id: must be 1-100 characters",
            }));
            continue;
        }

        if let Some(existing) = existing_by_name.get(trimmed) {
            success.push(json!({
                "model_id": trimmed,
                "global_model_id": existing.global_model_id,
                "global_model_name": existing.global_model_name,
                "provider_model_id": existing.id,
                "created_global_model": false,
            }));
            continue;
        }

        let mut created_global_model = false;
        let global_model = if let Some(existing) = state
            .get_admin_global_model_by_name(trimmed)
            .await
            .map_err(|err| format!("{err:?}"))?
        {
            existing
        } else {
            let created = state
                .create_admin_global_model(
                    &CreateAdminGlobalModelRecord::new(
                        Uuid::new_v4().to_string(),
                        trimmed.to_string(),
                        trimmed.to_string(),
                        true,
                        payload.price_per_request,
                        tiered_pricing
                            .clone()
                            .or_else(|| Some(default_pricing.clone())),
                        None,
                        None,
                    )
                    .map_err(|err| err.to_string())?,
                )
                .await
                .map_err(|err| format!("{err:?}"))?;
            let Some(created) = created else {
                errors.push(json!({"model_id": trimmed, "error": "Create GlobalModel failed"}));
                continue;
            };
            created_global_model = true;
            created
        };

        let record = UpsertAdminProviderModelRecord::new(
            Uuid::new_v4().to_string(),
            provider_id.to_string(),
            global_model.id.clone(),
            trimmed.to_string(),
            None,
            payload.price_per_request,
            tiered_pricing.clone(),
            None,
            None,
            None,
            None,
            None,
            true,
            true,
            None,
        )
        .map_err(|err| err.to_string())?;

        match state.create_admin_provider_model(&record).await {
            Ok(Some(created)) => {
                existing_by_name.insert(trimmed.to_string(), created.clone());
                success.push(json!({
                    "model_id": trimmed,
                    "global_model_id": global_model.id,
                    "global_model_name": global_model.name,
                    "provider_model_id": created.id,
                    "created_global_model": created_global_model,
                }));
            }
            Ok(None) => errors.push(json!({
                "model_id": trimmed,
                "error": "Create provider model failed",
            })),
            Err(err) => errors.push(json!({
                "model_id": trimmed,
                "error": format!("{err:?}"),
            })),
        }
    }

    Ok(json!({
        "success": success,
        "errors": errors,
    }))
}
