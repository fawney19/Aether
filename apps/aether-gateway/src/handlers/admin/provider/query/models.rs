use super::payload::{
    provider_query_extract_api_key_id, provider_query_extract_force_refresh,
    provider_query_extract_provider_id,
};
use super::response::{
    build_admin_provider_query_bad_request_response, build_admin_provider_query_not_found_response,
    ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL, ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
    ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
    ADMIN_PROVIDER_QUERY_PROVIDER_NOT_FOUND_DETAIL,
};
use crate::execution_runtime;
use crate::handlers::admin::request::AdminAppState;
use crate::model_fetch::ModelFetchRuntimeState;
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_model_fetch::{
    aggregate_models_for_cache, fetch_models_from_transports, json_string_list,
    preset_models_for_provider, selected_models_fetch_endpoints,
};
use axum::{body::Body, http::Response, response::IntoResponse, Json};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) const ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_MESSAGE: &str =
    "Rust local provider-query model test is not configured";
pub(crate) const ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_FAILOVER_MESSAGE: &str =
    "Rust local provider-query failover simulation is not configured";
const ADMIN_PROVIDER_QUERY_NO_ACTIVE_ENDPOINT_DETAIL: &str =
    "No active endpoints found for this provider";
const ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_ENDPOINT_DETAIL: &str =
    "No models returned from any endpoint";
const ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_KEY_DETAIL: &str = "No models returned from any key";
const ANTIGRAVITY_PROVIDER_CACHE_KEY_PREFIX: &str = "upstream_models_provider:";

#[derive(Debug)]
struct ProviderQueryKeyFetchResult {
    models: Vec<Value>,
    error: Option<String>,
    from_cache: bool,
    has_success: bool,
}

fn provider_query_provider_payload(provider: &StoredProviderCatalogProvider) -> Value {
    json!({
        "id": provider.id.clone(),
        "name": provider.name.clone(),
        "display_name": provider.name.clone(),
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
    let runner = state.app().redis_kv_runner()?;
    let cache_key = runner
        .keyspace()
        .key(&format!("upstream_models:{provider_id}:{key_id}"));
    let mut connection = runner
        .client()
        .get_multiplexed_async_connection()
        .await
        .ok()?;
    let raw = redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<Option<String>>(&mut connection)
        .await
        .ok()??;
    let parsed = serde_json::from_str::<Vec<Value>>(&raw).ok()?;
    Some(aggregate_models_for_cache(&parsed))
}

async fn provider_query_read_provider_cached_models(
    state: &AdminAppState<'_>,
    provider_id: &str,
) -> Option<Vec<Value>> {
    let runner = state.app().redis_kv_runner()?;
    let cache_key = runner.keyspace().key(&format!(
        "{ANTIGRAVITY_PROVIDER_CACHE_KEY_PREFIX}{provider_id}"
    ));
    let mut connection = runner
        .client()
        .get_multiplexed_async_connection()
        .await
        .ok()?;
    let raw = redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<Option<String>>(&mut connection)
        .await
        .ok()??;
    let parsed = serde_json::from_str::<Vec<Value>>(&raw).ok()?;
    Some(aggregate_models_for_cache(&parsed))
}

async fn provider_query_write_provider_cached_models(
    state: &AdminAppState<'_>,
    provider_id: &str,
    models: &[Value],
) {
    let Some(runner) = state.app().redis_kv_runner() else {
        return;
    };
    let Ok(serialized) = serde_json::to_string(&aggregate_models_for_cache(models)) else {
        return;
    };
    let cache_key = format!("{ANTIGRAVITY_PROVIDER_CACHE_KEY_PREFIX}{provider_id}");
    let _ = runner
        .setex(
            &cache_key,
            &serialized,
            Some(aether_model_fetch::model_fetch_interval_minutes().saturating_mul(60)),
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
    ranked.sort_by(|left, right| right.0.cmp(&left.0));
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
            return Ok(ProviderQueryKeyFetchResult {
                models: cached_models,
                error: None,
                from_cache: true,
                has_success: true,
            });
        }
    }

    let selected_endpoints = selected_models_fetch_endpoints(endpoints, key);
    if selected_endpoints.is_empty() {
        if let Some(models) = preset_models_for_provider(&provider.provider_type) {
            return Ok(ProviderQueryKeyFetchResult {
                models: aggregate_models_for_cache(&models),
                error: None,
                from_cache: false,
                has_success: true,
            });
        }
        return Ok(ProviderQueryKeyFetchResult {
            models: Vec::new(),
            error: Some(ADMIN_PROVIDER_QUERY_NO_ACTIVE_ENDPOINT_DETAIL.to_string()),
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
            from_cache: false,
            has_success: false,
        });
    }

    let outcome = match fetch_models_from_transports(state.app(), &transports).await {
        Ok(outcome) => outcome,
        Err(err) => {
            all_errors.push(err);
            return Ok(ProviderQueryKeyFetchResult {
                models: Vec::new(),
                error: Some(all_errors.join("; ")),
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

    let mut error = if all_errors.is_empty() {
        None
    } else {
        Some(all_errors.join("; "))
    };
    if unique_models.is_empty() && error.is_none() {
        error = Some(ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_ENDPOINT_DETAIL.to_string());
    }

    Ok(ProviderQueryKeyFetchResult {
        models: unique_models,
        error,
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
        let success = !result.models.is_empty();
        return Ok(Json(json!({
            "success": success,
            "data": {
                "models": result.models,
                "error": result.error,
                "from_cache": result.from_cache,
            },
            "provider": provider_query_provider_payload(&provider),
        }))
        .into_response());
    }

    let active_keys = keys
        .into_iter()
        .filter(|key| key.is_active)
        .collect::<Vec<_>>();
    if active_keys.is_empty() {
        return Ok(build_admin_provider_query_bad_request_response(
            ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
        ));
    }
    let active_key_count = active_keys.len();

    if provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("antigravity")
        && !force_refresh
    {
        if let Some(models) = provider_query_read_provider_cached_models(state, &provider.id).await
        {
            return Ok(Json(json!({
                "success": !models.is_empty(),
                "data": {
                    "models": models,
                    "error": serde_json::Value::Null,
                    "from_cache": true,
                    "keys_total": active_key_count,
                    "keys_cached": active_key_count,
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
        provider_query_sort_antigravity_keys(state, &provider, &endpoints, active_keys).await?
    } else {
        active_keys
    };

    let mut all_models = Vec::new();
    let mut all_errors = Vec::new();
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
    if provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("antigravity")
        && !models.is_empty()
    {
        provider_query_write_provider_cached_models(state, &provider.id, &models).await;
    }
    let success = !models.is_empty();
    let mut error = if all_errors.is_empty() {
        None
    } else {
        Some(all_errors.join("; "))
    };
    if !success && error.is_none() {
        error = Some(ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_KEY_DETAIL.to_string());
    }

    Ok(Json(json!({
        "success": success,
        "data": {
            "models": models,
            "error": error,
            "from_cache": fetch_count == 0 && cache_hit_count > 0,
            "keys_total": active_key_count,
            "keys_cached": cache_hit_count,
            "keys_fetched": fetch_count,
        },
        "provider": provider_query_provider_payload(&provider),
    }))
    .into_response())
}

pub(crate) fn build_admin_provider_query_test_model_response(
    provider_id: String,
    model: String,
) -> Response<Body> {
    Json(json!({
        "success": false,
        "tested": false,
        "provider_id": provider_id,
        "model": model,
        "attempts": [],
        "total_candidates": 0,
        "total_attempts": 0,
        "error": ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_MESSAGE,
        "source": "local",
        "message": ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_MESSAGE,
    }))
    .into_response()
}

pub(crate) fn build_admin_provider_query_test_model_failover_response(
    provider_id: String,
    failover_models: Vec<String>,
) -> Response<Body> {
    Json(json!({
        "success": false,
        "tested": false,
        "provider_id": provider_id,
        "model": failover_models.first().cloned(),
        "failover_models": failover_models,
        "attempts": [],
        "total_candidates": 0,
        "total_attempts": 0,
        "error": ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_FAILOVER_MESSAGE,
        "source": "local",
        "message": ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_FAILOVER_MESSAGE,
    }))
    .into_response()
}
