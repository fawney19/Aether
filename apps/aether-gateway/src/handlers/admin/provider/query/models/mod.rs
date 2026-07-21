use super::payload::{
    provider_query_extract_api_key_id, provider_query_extract_api_key_ids,
    provider_query_extract_force_refresh, provider_query_extract_model,
    provider_query_extract_provider_id, provider_query_extract_request_id,
};
use super::response::{
    build_admin_provider_query_bad_request_response, build_admin_provider_query_not_found_response,
    ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL, ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
    ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL, ADMIN_PROVIDER_QUERY_NO_LOCAL_MODELS_DETAIL,
    ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
    ADMIN_PROVIDER_QUERY_PROVIDER_NOT_FOUND_DETAIL,
};
use crate::ai_serving::{
    maybe_build_sync_finalize_outcome, GatewayControlDecision,
    ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME, GEMINI_CHAT_SYNC_FINALIZE_REPORT_KIND,
    OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND,
};
use crate::clock::{current_unix_ms, current_unix_secs};
use crate::execution_runtime;
use crate::handlers::admin::provider::shared::model_test_capabilities::{
    admin_provider_model_supports_image_generation, admin_provider_model_test_capabilities_payload,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::handlers::shared::provider_pool::{
    admin_provider_pool_config_from_config_value, read_admin_provider_pool_runtime_state,
    AdminProviderPoolConfig, AdminProviderPoolRuntimeState,
};
use crate::handlers::shared::{
    parse_catalog_auth_config_json, provider_key_health_summary,
    provider_key_status_snapshot_payload,
};
use crate::model_fetch::ModelFetchRuntimeState;
use crate::provider_key_auth::{
    provider_key_auth_semantics, provider_key_configured_api_formats,
    provider_key_inherits_provider_api_formats,
};
use crate::provider_transport::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestEnvelopeSupport, AntigravityRequestSideSupport,
    AntigravityRequestSideUnsupportedReason,
};
use crate::provider_transport::kiro::{
    build_kiro_generate_assistant_response_url, build_kiro_provider_headers,
    build_kiro_provider_request_body, supports_local_kiro_request_transport_with_network,
    KiroProviderHeadersInput, KIRO_ENVELOPE_NAME,
};
use crate::usage::GatewaySyncReportRequest;
use crate::{AppState, GatewayError};
use aether_admin::provider::pool as admin_provider_pool_pure;
use aether_ai_serving::{
    run_ai_pool_scheduler, AiPoolCandidateFacts, AiPoolCandidateInput, AiPoolCatalogKeyContext,
    AiPoolRuntimeState, AiPoolSchedulingConfig, AiPoolSchedulingPreset,
};
use aether_contracts::{ExecutionPlan, RequestBody};
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::global_models::{
    AdminProviderModelListQuery, StoredAdminProviderModel,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_model_fetch::{
    aggregate_models_for_cache, fetch_models_from_transports, json_string_list,
    model_catalog_upstream_metadata, preset_models_for_provider, selected_models_fetch_endpoints,
    upstream_metadata_namespace_updates,
};
use axum::{
    body::{to_bytes, Body},
    http::{self, HeaderMap, HeaderName, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine as _;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use uuid::Uuid;

pub(crate) const ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_MESSAGE: &str =
    "Rust local provider-query model test is not configured";
pub(crate) const ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_FAILOVER_MESSAGE: &str =
    "Rust local provider-query failover simulation is not configured";
const ADMIN_PROVIDER_QUERY_NO_ACTIVE_ENDPOINT_DETAIL: &str =
    "No active endpoints found for this provider";
const ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_ENDPOINT_DETAIL: &str =
    "No models returned from any endpoint";
const ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_KEY_DETAIL: &str = "No models returned from any key";
const ADMIN_PROVIDER_QUERY_NO_ACTIVE_TEST_CANDIDATE_DETAIL: &str =
    "No active endpoint or API key found";
const ADMIN_PROVIDER_QUERY_INVALID_MAPPED_MODEL_DETAIL: &str =
    "mapped_model_name is not valid for the selected model and endpoint";
const ANTIGRAVITY_PROVIDER_CACHE_KEY_PREFIX: &str = "upstream_models_provider:";
const DEFAULT_PROVIDER_QUERY_TEST_MESSAGE: &str = "Hello! This is a test message.";
static PROVIDER_QUERY_POOL_LOAD_BALANCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct ProviderQueryKeyFetchResult {
    models: Vec<Value>,
    error: Option<String>,
    warning: Option<String>,
    from_cache: bool,
    has_success: bool,
}

fn provider_query_select_model_keys(
    keys: Vec<StoredProviderCatalogKey>,
    selected_key_ids: Option<&BTreeSet<String>>,
) -> Result<Vec<StoredProviderCatalogKey>, ()> {
    if selected_key_ids.is_some_and(|selected| {
        selected
            .iter()
            .any(|key_id| !keys.iter().any(|key| key.id == *key_id))
    }) {
        return Err(());
    }
    Ok(match selected_key_ids {
        Some(selected) => keys
            .into_iter()
            .filter(|key| selected.contains(&key.id))
            .collect(),
        None => keys.into_iter().filter(|key| key.is_active).collect(),
    })
}

fn provider_query_model_id(model: &Value) -> Option<&str> {
    model
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn provider_query_grok_required_tier_rank(model_id: &str) -> Option<u8> {
    match model_id.trim() {
        "grok-4.20-0309-non-reasoning" | "grok-4.20-fast" | "grok-imagine-image-lite" => Some(0),
        "grok-4.20-0309"
        | "grok-4.20-0309-reasoning"
        | "grok-4.20-0309-non-reasoning-super"
        | "grok-4.20-0309-super"
        | "grok-4.20-0309-reasoning-super"
        | "grok-4.20-auto"
        | "grok-4.20-expert"
        | "grok-4.3-beta"
        | "grok-imagine-image"
        | "grok-imagine-image-pro"
        | "grok-imagine-image-edit" => Some(1),
        "grok-4.20-0309-non-reasoning-heavy"
        | "grok-4.20-0309-heavy"
        | "grok-4.20-0309-reasoning-heavy"
        | "grok-4.20-multi-agent-0309"
        | "grok-4.20-heavy" => Some(2),
        _ => None,
    }
}

fn provider_query_normalize_grok_pool_tier(value: Option<&str>) -> Option<&'static str> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "basic" => Some("basic"),
        "super" => Some("super"),
        "heavy" => Some("heavy"),
        _ => None,
    }
}

fn provider_query_grok_pool_tier_rank(value: Option<&str>) -> u8 {
    match provider_query_normalize_grok_pool_tier(value).unwrap_or("basic") {
        "heavy" => 2,
        "super" => 1,
        _ => 0,
    }
}

fn provider_query_grok_quota_string(quota: &Map<String, Value>, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        quota
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn provider_query_grok_window_limit(quota: &Map<String, Value>, model_name: &str) -> Option<f64> {
    quota
        .get("windows")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_object)
        .find(|window| {
            window
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim() == model_name)
        })
        .and_then(|window| window.get("limit_value"))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn provider_query_grok_pool_tier_from_quota(quota: &Map<String, Value>) -> Option<&'static str> {
    if let Some(tier) =
        provider_query_grok_quota_string(quota, &["pool_tier", "tier", "plan_type", "plan"])
            .and_then(|value| provider_query_normalize_grok_pool_tier(Some(&value)))
    {
        return Some(tier);
    }

    if let Some(auto_total) = provider_query_grok_window_limit(quota, "quota_auto") {
        if (auto_total - 150.0).abs() < f64::EPSILON {
            return Some("heavy");
        }
        if (auto_total - 50.0).abs() < f64::EPSILON {
            return Some("super");
        }
    }

    if let Some(fast_total) = provider_query_grok_window_limit(quota, "quota_fast") {
        if (fast_total - 400.0).abs() < f64::EPSILON {
            return Some("heavy");
        }
        if (fast_total - 140.0).abs() < f64::EPSILON {
            return Some("super");
        }
        if (fast_total - 30.0).abs() < f64::EPSILON {
            return Some("basic");
        }
    }

    None
}

fn provider_query_grok_key_pool_tier(key: &StoredProviderCatalogKey) -> Option<&'static str> {
    key.status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)
        .and_then(provider_query_grok_pool_tier_from_quota)
}

fn provider_query_filter_models_for_key(
    provider: &StoredProviderCatalogProvider,
    key: &StoredProviderCatalogKey,
    models: Vec<Value>,
) -> Vec<Value> {
    if !provider.provider_type.trim().eq_ignore_ascii_case("grok") {
        return models;
    }

    let allowed_rank = provider_query_grok_pool_tier_rank(provider_query_grok_key_pool_tier(key));
    models
        .into_iter()
        .filter(|model| {
            provider_query_model_id(model)
                .and_then(provider_query_grok_required_tier_rank)
                .is_some_and(|required_rank| required_rank <= allowed_rank)
        })
        .collect()
}

fn provider_query_attach_model_test_capabilities(
    provider: &StoredProviderCatalogProvider,
    models: Vec<Value>,
) -> Vec<Value> {
    models
        .into_iter()
        .map(|mut model| {
            let Some(object) = model.as_object_mut() else {
                return model;
            };
            let model_id = object
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            let supports_image_generation = admin_provider_model_supports_image_generation(
                &provider.provider_type,
                &model_id,
                object
                    .get("supports_image_generation")
                    .or_else(|| object.get("effective_supports_image_generation"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            );
            object.insert(
                "model_test_capabilities".to_string(),
                admin_provider_model_test_capabilities_payload(
                    &provider.provider_type,
                    &model_id,
                    supports_image_generation,
                ),
            );
            model
        })
        .collect()
}

fn provider_query_codex_preset_fallback(
    provider: &StoredProviderCatalogProvider,
    fetch_error: &str,
) -> Option<ProviderQueryKeyFetchResult> {
    if !provider.provider_type.trim().eq_ignore_ascii_case("codex") {
        return None;
    }
    let models = preset_models_for_provider(&provider.provider_type)?;
    Some(ProviderQueryKeyFetchResult {
        models: aggregate_models_for_cache(&models),
        error: None,
        warning: Some(format!(
            "Codex 动态模型目录不可用，已使用内置模型卡：{fetch_error}"
        )),
        from_cache: false,
        has_success: true,
    })
}

async fn provider_query_persist_preset_models(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    key: &StoredProviderCatalogKey,
    models: &[Value],
) -> Result<(), GatewayError> {
    if models.is_empty() {
        return Ok(());
    }
    <AppState as ModelFetchRuntimeState>::write_upstream_models_cache(
        state.app(),
        &provider.id,
        &key.id,
        models,
    )
    .await;
    if let Some(catalog_metadata) = model_catalog_upstream_metadata(&provider.provider_type, models)
    {
        provider_query_persist_upstream_metadata(state, key, &catalog_metadata).await?;
    }
    Ok(())
}

async fn provider_query_persist_upstream_metadata(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
    upstream_metadata: &Value,
) -> Result<(), GatewayError> {
    let updated_at = current_unix_secs();
    for (namespace, value) in
        upstream_metadata_namespace_updates(key.upstream_metadata.as_ref(), upstream_metadata)
    {
        state
            .app()
            .upsert_provider_catalog_key_upstream_metadata_namespace(
                &key.id,
                &namespace,
                &value,
                Some(updated_at),
            )
            .await?;
    }
    Ok(())
}

mod model_test;

pub(crate) use self::model_test::{
    build_admin_provider_query_test_model_failover_local_response,
    build_admin_provider_query_test_model_failover_response,
    build_admin_provider_query_test_model_local_response,
    build_admin_provider_query_test_model_response,
};

fn provider_query_provider_payload(provider: &StoredProviderCatalogProvider) -> Value {
    json!({
        "id": provider.id.clone(),
        "name": provider.name.clone(),
        "display_name": provider.name.clone(),
        "provider_type": provider.provider_type.clone(),
    })
}

fn provider_query_key_display_name(key: &StoredProviderCatalogKey) -> String {
    let trimmed = key.name.trim();
    if trimmed.is_empty() {
        key.id.clone()
    } else {
        trimmed.to_string()
    }
}

async fn provider_query_read_cached_models(
    state: &AdminAppState<'_>,
    provider_id: &str,
    key_id: &str,
) -> Option<Vec<Value>> {
    let cache_key = format!("upstream_models:{provider_id}:{key_id}");
    let raw = state.runtime_state().kv_get(&cache_key).await.ok()??;
    let parsed = serde_json::from_str::<Vec<Value>>(&raw).ok()?;
    Some(aggregate_models_for_cache(&parsed))
}

async fn provider_query_read_provider_cached_models(
    state: &AdminAppState<'_>,
    provider_id: &str,
) -> Option<Vec<Value>> {
    let cache_key = format!("{ANTIGRAVITY_PROVIDER_CACHE_KEY_PREFIX}{provider_id}");
    let raw = state.runtime_state().kv_get(&cache_key).await.ok()??;
    let parsed = serde_json::from_str::<Vec<Value>>(&raw).ok()?;
    Some(aggregate_models_for_cache(&parsed))
}

async fn provider_query_write_provider_cached_models(
    state: &AdminAppState<'_>,
    provider_id: &str,
    models: &[Value],
) {
    let Ok(serialized) = serde_json::to_string(&aggregate_models_for_cache(models)) else {
        return;
    };
    let cache_key = format!("{ANTIGRAVITY_PROVIDER_CACHE_KEY_PREFIX}{provider_id}");
    let _ = state
        .runtime_state()
        .kv_set(
            &cache_key,
            serialized,
            Some(std::time::Duration::from_secs(
                aether_model_fetch::model_fetch_interval_minutes().saturating_mul(60),
            )),
        )
        .await;
}

fn provider_query_antigravity_tier_weight(raw_auth_config: Option<&str>) -> i32 {
    raw_auth_config
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .and_then(|value| value.get("tier").cloned())
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .map(|tier| match tier.trim().to_ascii_lowercase().as_str() {
            "ultra" => 3,
            "pro" => 2,
            "free" => 1,
            _ => 0,
        })
        .unwrap_or(0)
}

async fn provider_query_sort_antigravity_keys(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    keys: Vec<StoredProviderCatalogKey>,
) -> Result<Vec<StoredProviderCatalogKey>, GatewayError> {
    let mut ranked = Vec::new();
    for key in keys {
        let availability = if key.oauth_invalid_at_unix_secs.is_some() {
            0
        } else {
            1
        };
        let tier_weight = if let Some(endpoint) = selected_models_fetch_endpoints(endpoints, &key)
            .into_iter()
            .next()
        {
            state
                .app()
                .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
                .await?
                .map(|transport| {
                    provider_query_antigravity_tier_weight(
                        transport.key.decrypted_auth_config.as_deref(),
                    )
                })
                .unwrap_or(0)
        } else {
            0
        };
        ranked.push(((availability, tier_weight), key));
    }
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    Ok(ranked.into_iter().map(|(_, key)| key).collect())
}

async fn provider_query_fetch_models_for_key(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    key: &StoredProviderCatalogKey,
    force_refresh: bool,
) -> Result<ProviderQueryKeyFetchResult, GatewayError> {
    if !force_refresh {
        if let Some(cached_models) =
            provider_query_read_cached_models(state, &provider.id, &key.id).await
        {
            let models = provider_query_filter_models_for_key(provider, key, cached_models);
            return Ok(ProviderQueryKeyFetchResult {
                models,
                error: None,
                warning: None,
                from_cache: true,
                has_success: true,
            });
        }
    }

    let selected_endpoints = selected_models_fetch_endpoints(endpoints, key);
    if selected_endpoints.is_empty() {
        if let Some(models) = preset_models_for_provider(&provider.provider_type) {
            let models = aggregate_models_for_cache(&models);
            provider_query_persist_preset_models(state, provider, key, &models).await?;
            let models = provider_query_filter_models_for_key(provider, key, models);
            return Ok(ProviderQueryKeyFetchResult {
                models,
                error: None,
                warning: None,
                from_cache: false,
                has_success: true,
            });
        }
        return Ok(ProviderQueryKeyFetchResult {
            models: Vec::new(),
            error: Some(ADMIN_PROVIDER_QUERY_NO_ACTIVE_ENDPOINT_DETAIL.to_string()),
            warning: None,
            from_cache: false,
            has_success: false,
        });
    }

    let mut transports = Vec::new();
    let mut all_errors = Vec::new();
    for endpoint in selected_endpoints {
        let Some(transport) = state
            .app()
            .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
            .await?
        else {
            all_errors.push(format!(
                "{} transport snapshot unavailable",
                endpoint.api_format.trim()
            ));
            continue;
        };
        transports.push(transport);
    }

    if transports.is_empty() {
        return Ok(ProviderQueryKeyFetchResult {
            models: Vec::new(),
            error: Some(all_errors.join("; ")),
            warning: None,
            from_cache: false,
            has_success: false,
        });
    }

    let outcome = match fetch_models_from_transports(state.app(), &transports).await {
        Ok(outcome) => outcome,
        Err(err) => {
            all_errors.push(err);
            if let Some(fallback) =
                provider_query_codex_preset_fallback(provider, &all_errors.join("; "))
            {
                provider_query_persist_preset_models(state, provider, key, &fallback.models)
                    .await?;
                return Ok(fallback);
            }
            return Ok(ProviderQueryKeyFetchResult {
                models: Vec::new(),
                error: Some(all_errors.join("; ")),
                warning: None,
                from_cache: false,
                has_success: false,
            });
        }
    };

    all_errors.extend(outcome.errors);
    let unique_models = aggregate_models_for_cache(&outcome.cached_models);
    if outcome.has_success && !unique_models.is_empty() {
        <AppState as ModelFetchRuntimeState>::write_upstream_models_cache(
            state.app(),
            &provider.id,
            &key.id,
            &unique_models,
        )
        .await;
    }
    if let Some(upstream_metadata) = outcome.upstream_metadata.as_ref() {
        provider_query_persist_upstream_metadata(state, key, upstream_metadata).await?;
    }

    if unique_models.is_empty() && !all_errors.is_empty() {
        if let Some(fallback) =
            provider_query_codex_preset_fallback(provider, &all_errors.join("; "))
        {
            provider_query_persist_preset_models(state, provider, key, &fallback.models).await?;
            return Ok(fallback);
        }
    }

    let has_models = !unique_models.is_empty();
    let mut error = if !has_models && !all_errors.is_empty() {
        Some(all_errors.join("; "))
    } else {
        None
    };
    let warning = if has_models && !all_errors.is_empty() {
        Some(all_errors.join("; "))
    } else {
        None
    };
    if !has_models && error.is_none() {
        error = Some(ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_ENDPOINT_DETAIL.to_string());
    }

    Ok(ProviderQueryKeyFetchResult {
        models: provider_query_filter_models_for_key(provider, key, unique_models),
        error,
        warning,
        from_cache: false,
        has_success: outcome.has_success,
    })
}

pub(crate) async fn build_admin_provider_query_models_response(
    state: &AdminAppState<'_>,
    payload: &serde_json::Value,
) -> Result<Response<Body>, GatewayError> {
    let Some(provider_id) = provider_query_extract_provider_id(payload) else {
        return Ok(build_admin_provider_query_bad_request_response(
            ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
        ));
    };

    let Some(provider) = state
        .app()
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .find(|item| item.id == provider_id)
    else {
        return Ok(build_admin_provider_query_not_found_response(
            ADMIN_PROVIDER_QUERY_PROVIDER_NOT_FOUND_DETAIL,
        ));
    };

    let provider_ids = vec![provider.id.clone()];
    let endpoints = state
        .app()
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await?;
    let keys = state
        .app()
        .list_provider_catalog_keys_by_provider_ids(&provider_ids)
        .await?;
    let force_refresh = provider_query_extract_force_refresh(payload);

    if let Some(api_key_id) = provider_query_extract_api_key_id(payload) {
        let Some(selected_key) = keys.iter().find(|key| key.id == api_key_id) else {
            return Ok(build_admin_provider_query_not_found_response(
                ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL,
            ));
        };

        let result = provider_query_fetch_models_for_key(
            state,
            &provider,
            &endpoints,
            selected_key,
            force_refresh,
        )
        .await?;
        let models = provider_query_attach_model_test_capabilities(&provider, result.models);
        let success = !models.is_empty();
        return Ok(Json(json!({
            "success": success,
            "data": {
                "models": models,
                "error": result.error,
                "warning": result.warning,
                "from_cache": result.from_cache,
            },
            "provider": provider_query_provider_payload(&provider),
        }))
        .into_response());
    }

    let selected_key_ids = provider_query_extract_api_key_ids(payload);
    let query_keys = match provider_query_select_model_keys(keys, selected_key_ids.as_ref()) {
        Ok(keys) => keys,
        Err(()) => {
            return Ok(build_admin_provider_query_not_found_response(
                ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL,
            ));
        }
    };
    if query_keys.is_empty() {
        return Ok(build_admin_provider_query_bad_request_response(
            ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
        ));
    }
    let query_key_count = query_keys.len();

    if selected_key_ids.is_none()
        && provider
            .provider_type
            .trim()
            .eq_ignore_ascii_case("antigravity")
        && !force_refresh
    {
        if let Some(models) = provider_query_read_provider_cached_models(state, &provider.id).await
        {
            let models = provider_query_attach_model_test_capabilities(&provider, models);
            return Ok(Json(json!({
                "success": !models.is_empty(),
                "data": {
                    "models": models,
                    "error": serde_json::Value::Null,
                    "warning": serde_json::Value::Null,
                    "from_cache": true,
                    "keys_total": query_key_count,
                    "keys_cached": query_key_count,
                    "keys_fetched": 0,
                },
                "provider": provider_query_provider_payload(&provider),
            }))
            .into_response());
        }
    }

    let ordered_keys = if provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("antigravity")
    {
        provider_query_sort_antigravity_keys(state, &provider, &endpoints, query_keys).await?
    } else {
        query_keys
    };

    let mut all_models = Vec::new();
    let mut all_errors = Vec::new();
    let mut all_warnings = Vec::new();
    let mut cache_hit_count = 0usize;
    let mut fetch_count = 0usize;
    for key in &ordered_keys {
        let result =
            provider_query_fetch_models_for_key(state, &provider, &endpoints, key, force_refresh)
                .await?;
        all_models.extend(result.models);
        if let Some(error) = result.error {
            all_errors.push(format!(
                "Key {}: {}",
                provider_query_key_display_name(key),
                error
            ));
        }
        if let Some(warning) = result.warning {
            all_warnings.push(format!(
                "Key {}: {}",
                provider_query_key_display_name(key),
                warning
            ));
        }
        if result.from_cache {
            cache_hit_count += 1;
        } else {
            fetch_count += 1;
        }
        if provider
            .provider_type
            .trim()
            .eq_ignore_ascii_case("antigravity")
            && result.has_success
        {
            break;
        }
    }

    let models = aggregate_models_for_cache(&all_models);
    if selected_key_ids.is_none()
        && provider
            .provider_type
            .trim()
            .eq_ignore_ascii_case("antigravity")
        && !models.is_empty()
    {
        provider_query_write_provider_cached_models(state, &provider.id, &models).await;
    }
    let success = !models.is_empty();
    let mut all_issues = all_errors;
    all_issues.extend(all_warnings);
    let mut error = if !success && !all_issues.is_empty() {
        Some(all_issues.join("; "))
    } else {
        None
    };
    let warning = if success && !all_issues.is_empty() {
        Some(all_issues.join("; "))
    } else {
        None
    };
    if !success && error.is_none() {
        error = Some(ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_KEY_DETAIL.to_string());
    }
    let models = provider_query_attach_model_test_capabilities(&provider, models);

    Ok(Json(json!({
        "success": success,
        "data": {
            "models": models,
            "error": error,
            "warning": warning,
            "from_cache": fetch_count == 0 && cache_hit_count > 0,
            "keys_total": query_key_count,
            "keys_cached": cache_hit_count,
            "keys_fetched": fetch_count,
        },
        "provider": provider_query_provider_payload(&provider),
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };

    fn grok_provider() -> StoredProviderCatalogProvider {
        let mut provider = StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "Grok".to_string(),
            None,
            "grok".to_string(),
        )
        .expect("provider should build");
        provider.provider_type = "grok".to_string();
        provider
    }

    #[test]
    fn selected_model_keys_use_the_explicit_batch_scope() {
        let mut first = StoredProviderCatalogKey::new(
            "key-a".to_string(),
            "provider-1".to_string(),
            "A".to_string(),
            "api_key".to_string(),
            None,
            false,
        )
        .expect("key should build");
        first.is_active = false;
        let second = StoredProviderCatalogKey::new(
            "key-b".to_string(),
            "provider-1".to_string(),
            "B".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");

        let selected = BTreeSet::from(["key-a".to_string()]);
        let keys =
            provider_query_select_model_keys(vec![first.clone(), second.clone()], Some(&selected))
                .expect("explicit selection should resolve");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, "key-a");

        let active = provider_query_select_model_keys(vec![first, second], None)
            .expect("default selection should resolve");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "key-b");
    }

    #[test]
    fn selected_model_keys_reject_unknown_ids() {
        let key = StoredProviderCatalogKey::new(
            "key-a".to_string(),
            "provider-1".to_string(),
            "A".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");
        let selected = BTreeSet::from(["key-missing".to_string()]);
        assert!(provider_query_select_model_keys(vec![key], Some(&selected)).is_err());
    }

    fn grok_key_with_quota(quota: Value) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.status_snapshot = Some(json!({ "quota": quota }));
        key
    }

    fn model(id: &str) -> Value {
        json!({ "id": id })
    }

    fn filtered_ids(key: &StoredProviderCatalogKey) -> Vec<String> {
        provider_query_filter_models_for_key(
            &grok_provider(),
            key,
            vec![
                model("grok-4.20-0309-non-reasoning"),
                model("grok-4.20-auto"),
                model("grok-4.20-heavy"),
                model("grok-imagine-image-lite"),
                model("grok-imagine-image"),
                model("grok-imagine-image-edit"),
            ],
        )
        .into_iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
        .collect()
    }

    #[test]
    fn provider_query_grok_basic_tier_hides_super_and_heavy_models() {
        let key = grok_key_with_quota(json!({ "pool_tier": "basic" }));

        assert_eq!(
            filtered_ids(&key),
            ["grok-4.20-0309-non-reasoning", "grok-imagine-image-lite"]
        );
    }

    #[test]
    fn provider_query_grok_super_tier_hides_heavy_models() {
        let key = grok_key_with_quota(json!({ "plan_type": "super" }));

        assert_eq!(
            filtered_ids(&key),
            [
                "grok-4.20-0309-non-reasoning",
                "grok-4.20-auto",
                "grok-imagine-image-lite",
                "grok-imagine-image",
                "grok-imagine-image-edit"
            ]
        );
    }

    #[test]
    fn provider_query_grok_heavy_tier_keeps_full_non_video_catalog() {
        let key = grok_key_with_quota(json!({ "pool_tier": "heavy" }));

        assert_eq!(
            filtered_ids(&key),
            [
                "grok-4.20-0309-non-reasoning",
                "grok-4.20-auto",
                "grok-4.20-heavy",
                "grok-imagine-image-lite",
                "grok-imagine-image",
                "grok-imagine-image-edit"
            ]
        );
    }

    #[test]
    fn provider_query_grok_tier_falls_back_to_live_quota_windows() {
        let key = grok_key_with_quota(json!({
            "windows": [
                { "model": "quota_fast", "limit_value": 140.0 }
            ]
        }));

        assert_eq!(
            filtered_ids(&key),
            [
                "grok-4.20-0309-non-reasoning",
                "grok-4.20-auto",
                "grok-imagine-image-lite",
                "grok-imagine-image",
                "grok-imagine-image-edit"
            ]
        );
    }

    #[test]
    fn provider_query_attaches_model_test_capabilities_to_models() {
        let models = provider_query_attach_model_test_capabilities(
            &grok_provider(),
            vec![
                model("grok-4.20-fast"),
                model("grok-imagine-image"),
                model("grok-imagine-image-edit"),
            ],
        );

        assert!(models[0]["model_test_capabilities"]["openai:image"].is_null());
        assert_eq!(
            models[1]["model_test_capabilities"]["openai:image"]["max_generation_count"],
            json!(4)
        );
        assert_eq!(
            models[1]["model_test_capabilities"]["openai:image"]["supports_generation"],
            json!(true)
        );
        assert_eq!(
            models[2]["model_test_capabilities"]["openai:image"]["supports_generation"],
            json!(false)
        );
        assert_eq!(
            models[2]["model_test_capabilities"]["openai:image"]["supports_edit"],
            json!(true)
        );
    }
}
