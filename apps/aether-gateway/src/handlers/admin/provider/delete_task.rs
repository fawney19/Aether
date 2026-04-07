use crate::handlers::admin::provider::shared::support::{
    put_admin_provider_delete_task, ADMIN_PROVIDER_MAPPING_PREVIEW_FETCH_LIMIT,
    ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_KEYS, ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_MODELS,
};
use crate::handlers::admin::shared::{decrypt_catalog_secret_with_fallbacks, json_string_list};
use crate::handlers::public::matches_model_mapping_for_models;
use crate::{AppState, GatewayError, LocalProviderDeleteTaskState};
use aether_data_contracts::repository::global_models::{
    AdminProviderModelListQuery, PublicGlobalModelQuery, StoredPublicGlobalModel,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn run_admin_provider_delete_task(
    state: &AppState,
    provider_id: &str,
    task_id: &str,
) -> Result<LocalProviderDeleteTaskState, GatewayError> {
    let Some(mut provider) = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Err(GatewayError::Internal(format!(
            "provider {provider_id} not found for delete task"
        )));
    };

    let keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let models = state
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: provider.id.clone(),
            offset: 0,
            limit: 10_000,
            is_active: None,
        })
        .await
        .unwrap_or_default();

    let mut task = LocalProviderDeleteTaskState {
        task_id: task_id.to_string(),
        provider_id: provider.id.clone(),
        status: "running".to_string(),
        stage: "preparing".to_string(),
        total_keys: keys.len(),
        deleted_keys: 0,
        total_endpoints: endpoints.len(),
        deleted_endpoints: 0,
        message: format!(
            "preparing delete for {} keys and {} endpoints",
            keys.len(),
            endpoints.len()
        ),
    };
    put_admin_provider_delete_task(state, &task);

    if provider.is_active {
        provider.is_active = false;
        provider.updated_at_unix_secs = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
        );
        let _ = state.update_provider_catalog_provider(&provider).await?;
    }
    task.stage = "disabling".to_string();
    task.message = "provider disabled; starting cleanup".to_string();
    put_admin_provider_delete_task(state, &task);

    let endpoint_ids = endpoints
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let key_ids = keys.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
    state
        .cleanup_deleted_provider_catalog_refs(&provider.id, &endpoint_ids, &key_ids)
        .await?;

    task.stage = "deleting_models".to_string();
    task.message = format!("deleting {} provider models", models.len());
    put_admin_provider_delete_task(state, &task);
    for model in &models {
        let _ = state
            .delete_admin_provider_model(&provider.id, &model.id)
            .await?;
    }

    task.stage = "deleting_keys".to_string();
    task.message = "deleting provider keys".to_string();
    put_admin_provider_delete_task(state, &task);
    for key in &keys {
        if state.delete_provider_catalog_key(&key.id).await? {
            task.deleted_keys += 1;
            task.message = format!("deleted {} / {} keys", task.deleted_keys, task.total_keys);
            put_admin_provider_delete_task(state, &task);
        }
    }

    task.stage = "deleting_endpoints".to_string();
    task.message = "deleting provider endpoints".to_string();
    put_admin_provider_delete_task(state, &task);
    for endpoint in &endpoints {
        if state.delete_provider_catalog_endpoint(&endpoint.id).await? {
            task.deleted_endpoints += 1;
            task.message = format!(
                "deleted {} / {} endpoints",
                task.deleted_endpoints, task.total_endpoints
            );
            put_admin_provider_delete_task(state, &task);
        }
    }

    task.stage = "deleting_provider".to_string();
    task.message = "deleting provider record".to_string();
    put_admin_provider_delete_task(state, &task);
    if !state.delete_provider_catalog_provider(&provider.id).await? {
        task.status = "failed".to_string();
        task.stage = "failed".to_string();
        task.message = "provider delete failed".to_string();
        put_admin_provider_delete_task(state, &task);
        return Ok(task);
    }

    task.status = "completed".to_string();
    task.stage = "completed".to_string();
    task.message = format!(
        "provider deleted: keys={}, endpoints={}",
        task.deleted_keys, task.deleted_endpoints
    );
    put_admin_provider_delete_task(state, &task);
    Ok(task)
}

pub(crate) fn public_global_model_mapping_patterns(model: &StoredPublicGlobalModel) -> Vec<String> {
    model
        .config
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|config| config.get("model_mappings"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn mapping_preview_masked_catalog_api_key(
    state: &AppState,
    key: &StoredProviderCatalogKey,
) -> String {
    let ciphertext = key.encrypted_api_key.trim();
    if ciphertext.is_empty() {
        return "***".to_string();
    }

    decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
        .map(|value| {
            if value.len() > 8 {
                format!(
                    "{}***{}",
                    &value[..4],
                    &value[value.len().saturating_sub(4)..]
                )
            } else if value.len() >= 2 {
                format!("{}***", &value[..2])
            } else {
                "***".to_string()
            }
        })
        .unwrap_or_else(|| "***".to_string())
}

pub(crate) async fn build_admin_provider_mapping_preview_payload(
    state: &AppState,
    provider_id: &str,
) -> Option<serde_json::Value> {
    if !state.has_provider_catalog_data_reader() || !state.has_global_model_data_reader() {
        return None;
    }

    let provider = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await
        .ok()
        .and_then(|mut providers| providers.drain(..).next())?;

    let mut keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .ok()
        .unwrap_or_default()
        .into_iter()
        .filter(|key| key.allowed_models.is_some())
        .collect::<Vec<_>>();
    let total_keys_with_allowed_models = keys.len();
    let truncated_keys =
        total_keys_with_allowed_models.saturating_sub(ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_KEYS);
    if keys.len() > ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_KEYS {
        keys.truncate(ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_KEYS);
    }

    let public_models = state
        .list_public_global_models(&PublicGlobalModelQuery {
            offset: 0,
            limit: ADMIN_PROVIDER_MAPPING_PREVIEW_FETCH_LIMIT,
            is_active: None,
            search: None,
        })
        .await
        .ok()
        .unwrap_or_else(|| {
            aether_data_contracts::repository::global_models::StoredPublicGlobalModelPage {
                items: Vec::new(),
                total: 0,
            }
        })
        .items;
    let mut models_with_mappings = public_models
        .into_iter()
        .filter_map(|model| {
            let mappings = public_global_model_mapping_patterns(&model);
            (!mappings.is_empty()).then_some((model, mappings))
        })
        .collect::<Vec<_>>();
    let total_models_with_mappings = models_with_mappings.len();
    let truncated_models =
        total_models_with_mappings.saturating_sub(ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_MODELS);
    if models_with_mappings.len() > ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_MODELS {
        models_with_mappings.truncate(ADMIN_PROVIDER_MAPPING_PREVIEW_MAX_MODELS);
    }

    if models_with_mappings.is_empty() {
        return Some(json!({
            "provider_id": provider.id,
            "provider_name": provider.name,
            "keys": [],
            "total_keys": 0,
            "total_matches": 0,
            "truncated": truncated_keys > 0 || truncated_models > 0,
            "truncated_keys": truncated_keys,
            "truncated_models": truncated_models,
        }));
    }

    let mut key_payloads = Vec::new();
    let mut total_matches = 0_u64;
    for key in keys {
        let allowed_models = json_string_list(key.allowed_models.as_ref());
        if allowed_models.is_empty() {
            continue;
        }

        let mut matching_global_models = Vec::new();
        for (global_model, mappings) in &models_with_mappings {
            let mut matched_models = Vec::new();
            for allowed_model in &allowed_models {
                for mapping_pattern in mappings {
                    if matches_model_mapping_for_models(mapping_pattern, allowed_model) {
                        matched_models.push(json!({
                            "allowed_model": allowed_model,
                            "mapping_pattern": mapping_pattern,
                        }));
                        break;
                    }
                }
            }

            if !matched_models.is_empty() {
                matching_global_models.push(json!({
                    "global_model_id": global_model.id,
                    "global_model_name": global_model.name,
                    "display_name": global_model
                        .display_name
                        .clone()
                        .unwrap_or_else(|| global_model.name.clone()),
                    "is_active": global_model.is_active,
                    "matched_models": matched_models,
                }));
                total_matches = total_matches.saturating_add(1);
            }
        }

        if matching_global_models.is_empty() {
            continue;
        }

        key_payloads.push(json!({
            "key_id": key.id,
            "key_name": key.name,
            "masked_key": mapping_preview_masked_catalog_api_key(state, &key),
            "is_active": key.is_active,
            "allowed_models": allowed_models,
            "matching_global_models": matching_global_models,
        }));
    }

    Some(json!({
        "provider_id": provider.id,
        "provider_name": provider.name,
        "keys": key_payloads,
        "total_keys": key_payloads.len(),
        "total_matches": total_matches,
        "truncated": truncated_keys > 0 || truncated_models > 0,
        "truncated_keys": truncated_keys,
        "truncated_models": truncated_models,
    }))
}
