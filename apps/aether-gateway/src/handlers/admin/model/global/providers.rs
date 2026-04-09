use super::super::payloads::{
    admin_provider_model_effective_capability, admin_provider_model_effective_input_price,
    admin_provider_model_effective_output_price,
};
use super::helpers::build_admin_global_model_price_range;
use crate::handlers::admin::request::AdminAppState;
use aether_data_contracts::repository::global_models::AdminGlobalModelListQuery;
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) async fn build_admin_global_model_providers_payload(
    state: &AdminAppState<'_>,
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
    state: &AdminAppState<'_>,
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
