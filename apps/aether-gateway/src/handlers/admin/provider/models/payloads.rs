use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::provider::models as admin_provider_models_pure;
use aether_data_contracts::repository::global_models::{
    AdminProviderModelListQuery, StoredAdminProviderModel,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn admin_provider_model_effective_input_price(
    model: &StoredAdminProviderModel,
) -> Option<f64> {
    admin_provider_models_pure::admin_provider_model_effective_input_price(model)
}

pub(super) fn admin_provider_model_effective_output_price(
    model: &StoredAdminProviderModel,
) -> Option<f64> {
    admin_provider_models_pure::admin_provider_model_effective_output_price(model)
}

pub(super) fn admin_provider_model_effective_capability(
    model: &StoredAdminProviderModel,
    capability: &str,
) -> bool {
    admin_provider_models_pure::admin_provider_model_effective_capability(model, capability)
}

pub(super) fn build_admin_provider_model_response(
    model: &StoredAdminProviderModel,
    now_unix_secs: u64,
) -> serde_json::Value {
    admin_provider_models_pure::build_admin_provider_model_response(model, now_unix_secs)
}

pub(super) async fn build_admin_provider_models_payload(
    state: &AdminAppState<'_>,
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
    state: &AdminAppState<'_>,
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
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider_model_name: &str,
    exclude_model_id: Option<&str>,
) -> Result<bool, GatewayError> {
    state
        .admin_provider_model_name_exists(provider_id, provider_model_name, exclude_model_id)
        .await
}
