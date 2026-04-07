use super::payloads::{
    admin_provider_model_effective_capability, admin_provider_model_effective_input_price,
    admin_provider_model_effective_output_price, model_tiered_pricing_first_tier_value,
    timestamp_or_now,
};
use crate::handlers::admin::shared::json_string_list;
use crate::AppState;
use aether_data_contracts::repository::global_models::{
    AdminGlobalModelListQuery, StoredAdminGlobalModel, StoredAdminProviderModel,
};
use futures_util::stream::{self, StreamExt};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn resolve_admin_global_model_by_id_or_err(
    state: &AppState,
    global_model_id: &str,
) -> Result<StoredAdminGlobalModel, String> {
    state
        .get_admin_global_model_by_id(global_model_id)
        .await
        .map_err(|err| format!("{err:?}"))?
        .ok_or_else(|| format!("GlobalModel {global_model_id} 不存在"))
}

fn admin_global_model_provider_counts(
    provider_models: &[StoredAdminProviderModel],
) -> (usize, usize, usize) {
    let total_models = provider_models.len();
    let total_providers = provider_models
        .iter()
        .map(|model| model.provider_id.clone())
        .collect::<BTreeSet<_>>()
        .len();
    let active_provider_count = provider_models
        .iter()
        .filter(|model| model.is_active && model.is_available)
        .map(|model| model.provider_id.clone())
        .collect::<BTreeSet<_>>()
        .len();
    (total_models, total_providers, active_provider_count)
}

fn build_admin_global_model_price_range(
    global_model: &StoredAdminGlobalModel,
    provider_models: &[StoredAdminProviderModel],
) -> serde_json::Value {
    let mut input_values = provider_models
        .iter()
        .filter_map(admin_provider_model_effective_input_price)
        .collect::<Vec<_>>();
    let mut output_values = provider_models
        .iter()
        .filter_map(admin_provider_model_effective_output_price)
        .collect::<Vec<_>>();

    if input_values.is_empty() {
        if let Some(value) = model_tiered_pricing_first_tier_value(
            global_model.default_tiered_pricing.as_ref(),
            "input_price_per_1m",
        ) {
            input_values.push(value);
        }
    }
    if output_values.is_empty() {
        if let Some(value) = model_tiered_pricing_first_tier_value(
            global_model.default_tiered_pricing.as_ref(),
            "output_price_per_1m",
        ) {
            output_values.push(value);
        }
    }

    json!({
        "min_input": input_values.iter().copied().reduce(f64::min),
        "max_input": input_values.iter().copied().reduce(f64::max),
        "min_output": output_values.iter().copied().reduce(f64::min),
        "max_output": output_values.iter().copied().reduce(f64::max),
    })
}

async fn admin_global_model_provider_models_by_global_model_id(
    state: &AppState,
    global_model_ids: &[String],
) -> BTreeMap<String, Vec<StoredAdminProviderModel>> {
    let state = state.clone();
    stream::iter(
        global_model_ids
            .iter()
            .map(String::clone)
            .map(|global_model_id| {
                let state = state.clone();
                async move {
                    let provider_models = state
                        .list_admin_provider_models_by_global_model_id(&global_model_id)
                        .await
                        .ok()
                        .unwrap_or_default();
                    (global_model_id, provider_models)
                }
            }),
    )
    .buffer_unordered(32)
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .collect()
}

pub(crate) fn build_admin_global_model_response(
    global_model: &StoredAdminGlobalModel,
    provider_models: &[StoredAdminProviderModel],
    now_unix_secs: u64,
) -> serde_json::Value {
    let (_, provider_count, active_provider_count) =
        admin_global_model_provider_counts(provider_models);
    json!({
        "id": &global_model.id,
        "name": &global_model.name,
        "display_name": &global_model.display_name,
        "is_active": global_model.is_active,
        "default_price_per_request": global_model.default_price_per_request,
        "default_tiered_pricing": global_model.default_tiered_pricing.clone(),
        "supported_capabilities": json_string_list(global_model.supported_capabilities.as_ref()),
        "config": global_model.config.clone(),
        "provider_count": provider_count,
        "active_provider_count": active_provider_count,
        "created_at": timestamp_or_now(global_model.created_at_unix_secs, now_unix_secs),
        "updated_at": timestamp_or_now(global_model.updated_at_unix_secs, now_unix_secs),
    })
}

pub(crate) async fn build_admin_global_models_payload(
    state: &AppState,
    skip: usize,
    limit: usize,
    is_active: Option<bool>,
    search: Option<String>,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() {
        return None;
    }
    let page = state
        .list_admin_global_models(&AdminGlobalModelListQuery {
            offset: skip,
            limit,
            is_active,
            search,
        })
        .await
        .ok()?;
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut models = page.items;
    models.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    let global_model_ids = models
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    let mut provider_models_by_global_model =
        admin_global_model_provider_models_by_global_model_id(state, &global_model_ids).await;
    let mut payload_models = Vec::with_capacity(models.len());
    for model in models {
        let provider_models = provider_models_by_global_model
            .remove(&model.id)
            .unwrap_or_default();
        payload_models.push(build_admin_global_model_response(
            &model,
            &provider_models,
            now_unix_secs,
        ));
    }
    Some(json!({
        "models": payload_models,
        "total": page.total,
    }))
}

pub(crate) async fn build_admin_global_model_payload(
    state: &AppState,
    global_model_id: &str,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() {
        return None;
    }
    let model = state
        .get_admin_global_model_by_id(global_model_id)
        .await
        .ok()??;
    let provider_models = state
        .list_admin_provider_models_by_global_model_id(&model.id)
        .await
        .ok()
        .unwrap_or_default();
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let (total_models, total_providers, _) = admin_global_model_provider_counts(&provider_models);
    let mut payload = build_admin_global_model_response(&model, &provider_models, now_unix_secs);
    if let Some(object) = payload.as_object_mut() {
        object.insert("total_models".to_string(), json!(total_models));
        object.insert("total_providers".to_string(), json!(total_providers));
        object.insert(
            "price_range".to_string(),
            build_admin_global_model_price_range(&model, &provider_models),
        );
    }
    Some(payload)
}

pub(crate) async fn build_admin_global_model_providers_payload(
    state: &AppState,
    global_model_id: &str,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() || !state.has_provider_catalog_data_reader() {
        return None;
    }
    let global_model = state
        .get_admin_global_model_by_id(global_model_id)
        .await
        .ok()??;
    let provider_models = state
        .list_admin_provider_models_by_global_model_id(&global_model.id)
        .await
        .ok()?;
    let provider_ids = provider_models
        .iter()
        .map(|model| model.provider_id.clone())
        .collect::<Vec<_>>();
    let provider_by_id = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await
        .ok()?
        .into_iter()
        .map(|provider| (provider.id.clone(), provider))
        .collect::<BTreeMap<_, _>>();
    let mut providers = provider_models
        .into_iter()
        .filter_map(|model| {
            let provider = provider_by_id.get(&model.provider_id)?;
            Some(json!({
                "provider_id": provider.id,
                "provider_name": provider.name,
                "model_id": model.id,
                "target_model": model.provider_model_name,
                "input_price_per_1m": admin_provider_model_effective_input_price(&model),
                "output_price_per_1m": admin_provider_model_effective_output_price(&model),
                "price_per_request": model.price_per_request.or(model.global_model_default_price_per_request),
                "effective_tiered_pricing": model
                    .tiered_pricing
                    .clone()
                    .or(model.global_model_default_tiered_pricing.clone()),
                "supports_vision": admin_provider_model_effective_capability(&model, "vision"),
                "supports_function_calling": admin_provider_model_effective_capability(&model, "function_calling"),
                "supports_streaming": admin_provider_model_effective_capability(&model, "streaming"),
                "is_active": model.is_active,
            }))
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| {
        left.get("provider_name")
            .and_then(serde_json::Value::as_str)
            .cmp(
                &right
                    .get("provider_name")
                    .and_then(serde_json::Value::as_str),
            )
    });
    let total = providers.len();
    Some(json!({
        "providers": providers,
        "total": total,
    }))
}

pub(crate) async fn build_admin_model_catalog_payload(
    state: &AppState,
) -> Option<serde_json::Value> {
    if !state.has_global_model_data_reader() || !state.has_provider_catalog_data_reader() {
        return None;
    }
    let global_models = state
        .list_admin_global_models(&AdminGlobalModelListQuery {
            offset: 0,
            limit: 10_000,
            is_active: Some(true),
            search: None,
        })
        .await
        .ok()?
        .items;
    let provider_ids = state
        .list_provider_catalog_providers(false)
        .await
        .ok()?
        .into_iter()
        .map(|provider| (provider.id.clone(), provider))
        .collect::<BTreeMap<_, _>>();
    let mut models = Vec::new();
    for global_model in global_models {
        let provider_models = state
            .list_admin_provider_models_by_global_model_id(&global_model.id)
            .await
            .ok()
            .unwrap_or_default();
        let price_range = build_admin_global_model_price_range(&global_model, &provider_models);
        let mut providers = Vec::new();
        let mut supports_vision = global_model
            .config
            .as_ref()
            .and_then(|value| value.get("vision"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mut supports_function_calling = global_model
            .config
            .as_ref()
            .and_then(|value| value.get("function_calling"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mut supports_streaming = global_model
            .config
            .as_ref()
            .and_then(|value| value.get("streaming"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        for model in provider_models {
            let Some(provider) = provider_ids.get(&model.provider_id) else {
                continue;
            };
            let effective_tiered_pricing = model
                .tiered_pricing
                .clone()
                .or_else(|| model.global_model_default_tiered_pricing.clone());
            let tier_count = effective_tiered_pricing
                .as_ref()
                .and_then(|value| value.get("tiers"))
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
                .unwrap_or(1);
            let model_supports_vision = admin_provider_model_effective_capability(&model, "vision");
            let model_supports_function_calling =
                admin_provider_model_effective_capability(&model, "function_calling");
            let model_supports_streaming =
                admin_provider_model_effective_capability(&model, "streaming");
            supports_vision |= model_supports_vision;
            supports_function_calling |= model_supports_function_calling;
            supports_streaming |= model_supports_streaming;
            providers.push(json!({
                "provider_id": provider.id,
                "provider_name": provider.name,
                "model_id": model.id,
                "target_model": model.provider_model_name,
                "input_price_per_1m": admin_provider_model_effective_input_price(&model),
                "output_price_per_1m": admin_provider_model_effective_output_price(&model),
                "cache_creation_price_per_1m": serde_json::Value::Null,
                "cache_read_price_per_1m": serde_json::Value::Null,
                "cache_1h_creation_price_per_1m": serde_json::Value::Null,
                "price_per_request": model.price_per_request.or(model.global_model_default_price_per_request),
                "effective_tiered_pricing": effective_tiered_pricing,
                "tier_count": tier_count,
                "supports_vision": model_supports_vision,
                "supports_function_calling": model_supports_function_calling,
                "supports_streaming": model_supports_streaming,
                "is_active": model.is_active,
            }));
        }
        providers.sort_by(|left, right| {
            left.get("provider_name")
                .and_then(serde_json::Value::as_str)
                .cmp(
                    &right
                        .get("provider_name")
                        .and_then(serde_json::Value::as_str),
                )
        });
        models.push(json!({
            "global_model_name": global_model.name,
            "display_name": global_model.display_name,
            "description": global_model
                .config
                .as_ref()
                .and_then(|value| value.get("description"))
                .and_then(serde_json::Value::as_str),
            "providers": providers,
            "price_range": price_range,
            "total_providers": providers.len(),
            "capabilities": json!({
                "supports_vision": supports_vision,
                "supports_function_calling": supports_function_calling,
                "supports_streaming": supports_streaming,
            }),
        }));
    }
    let total = models.len();
    models.sort_by(|left, right| {
        left.get("global_model_name")
            .and_then(serde_json::Value::as_str)
            .cmp(
                &right
                    .get("global_model_name")
                    .and_then(serde_json::Value::as_str),
            )
    });
    Some(json!({
        "models": models,
        "total": total,
    }))
}
