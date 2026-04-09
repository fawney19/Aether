use super::super::payloads::{
    admin_provider_model_effective_input_price, admin_provider_model_effective_output_price,
    model_tiered_pricing_first_tier_value,
};
use crate::handlers::admin::request::AdminAppState;
use aether_data_contracts::repository::global_models::{
    StoredAdminGlobalModel, StoredAdminProviderModel,
};
use futures_util::stream::{self, StreamExt};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn resolve_admin_global_model_by_id_or_err(
    state: &AdminAppState<'_>,
    global_model_id: &str,
) -> Result<StoredAdminGlobalModel, String> {
    state
        .get_admin_global_model_by_id(global_model_id)
        .await
        .map_err(|err| format!("{err:?}"))?
        .ok_or_else(|| format!("GlobalModel {global_model_id} 不存在"))
}

pub(super) fn admin_global_models_now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(super) fn admin_global_model_provider_counts(
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

pub(super) fn build_admin_global_model_price_range(
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

pub(super) async fn admin_global_model_provider_models_by_global_model_id(
    state: &AdminAppState<'_>,
    global_model_ids: &[String],
) -> BTreeMap<String, Vec<StoredAdminProviderModel>> {
    let state = *state;
    stream::iter(global_model_ids.iter().cloned().map(|global_model_id| {
        let state = state;
        async move {
            let provider_models = state
                .list_admin_provider_models_by_global_model_id(&global_model_id)
                .await
                .ok()
                .unwrap_or_default();
            (global_model_id, provider_models)
        }
    }))
    .buffer_unordered(32)
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .collect()
}
