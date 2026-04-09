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
use crate::model_fetch::ModelFetchRuntimeState;
use crate::handlers::admin::request::AdminAppState;
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_model_fetch::{
    aggregate_models_for_cache, build_models_fetch_execution_plan, extract_error_message,
    parse_models_response,
};
use axum::{body::Body, http::Response, response::IntoResponse, Json};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(crate) const ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_MESSAGE: &str =
    "Rust local provider-query model test is not configured";
pub(crate) const ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_FAILOVER_MESSAGE: &str =
    "Rust local provider-query failover simulation is not configured";
const ADMIN_PROVIDER_QUERY_NO_ACTIVE_ENDPOINT_DETAIL: &str =
    "No active endpoints found for this provider";
const ADMIN_PROVIDER_QUERY_NO_MODELS_FROM_ENDPOINT_DETAIL: &str =
    "No models returned from any endpoint";
const PROVIDER_QUERY_FETCH_FORMAT_PRIORITY: &[&[&str]] = &[
    &["openai:chat", "openai:cli", "openai:compact"],
    &["claude:chat", "claude:cli"],
    &["gemini:chat", "gemini:cli"],
];

#[derive(Debug)]
struct ProviderQueryKeyFetchResult {
    models: Vec<Value>,
    error: Option<String>,
    from_cache: bool,
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

fn provider_query_normalize_api_format(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn provider_query_selected_fetch_endpoints(
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Vec<StoredProviderCatalogEndpoint> {
    let mut by_format = BTreeMap::<String, StoredProviderCatalogEndpoint>::new();
    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        let api_format = provider_query_normalize_api_format(&endpoint.api_format);
        if api_format.is_empty() {
            continue;
        }
        by_format.insert(api_format, endpoint.clone());
    }

    // 与 Python 版本保持一致：同族优先使用 chat 端点，其次才回退到 cli/compact。
    PROVIDER_QUERY_FETCH_FORMAT_PRIORITY
        .iter()
        .filter_map(|candidates| {
            candidates
                .iter()
                .find_map(|api_format| by_format.get(*api_format))
                .cloned()
        })
        .collect()
}

async fn provider_query_read_cached_models(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
) -> Option<Vec<Value>> {
    let runner = state.redis_kv_runner()?;
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

async fn provider_query_fetch_models_from_transport(
    state: &AppState,
    transport: &crate::provider_transport::GatewayProviderTransportSnapshot,
) -> Result<Vec<Value>, String> {
    let plan = build_models_fetch_execution_plan(state, transport).await?;
    let result = execution_runtime::execute_execution_runtime_sync_plan(state, None, &plan)
        .await
        .map_err(|err| format!("{err:?}"))?;

    if result.status_code != 200 {
        let message = result
            .body
            .as_ref()
            .and_then(|body| body.json_body.as_ref())
            .and_then(extract_error_message)
            .or_else(|| {
                result.error.as_ref().and_then(|error| {
                    let message = error.message.trim();
                    (!message.is_empty()).then_some(message.to_string())
                })
            })
            .unwrap_or_else(|| format!("upstream returned status {}", result.status_code));
        return Err(message);
    }

    let body_json = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
        .ok_or_else(|| "models fetch response body is missing JSON payload".to_string())?;
    let parsed = parse_models_response(&transport.endpoint.api_format, body_json)?;
    Ok(parsed.cached_models)
}

async fn provider_query_fetch_models_for_key(
    state: &AppState,
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
            });
        }
    }

    let selected_endpoints = provider_query_selected_fetch_endpoints(endpoints);
    if selected_endpoints.is_empty() {
        return Ok(ProviderQueryKeyFetchResult {
            models: Vec::new(),
            error: Some(ADMIN_PROVIDER_QUERY_NO_ACTIVE_ENDPOINT_DETAIL.to_string()),
            from_cache: false,
        });
    }

    let mut all_models = Vec::new();
    let mut all_errors = Vec::new();
    for endpoint in selected_endpoints {
        let Some(transport) = state
            .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
            .await?
        else {
            all_errors.push(format!(
                "{} transport snapshot unavailable",
                endpoint.api_format.trim()
            ));
            continue;
        };
        match provider_query_fetch_models_from_transport(state, &transport).await {
            Ok(models) => all_models.extend(models),
            Err(err) => all_errors.push(err),
        }
    }

    let unique_models = aggregate_models_for_cache(&all_models);
    if !unique_models.is_empty() {
        <AppState as ModelFetchRuntimeState>::write_upstream_models_cache(
            state,
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
            state.app(),
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

    let active_keys = keys.iter().filter(|key| key.is_active).collect::<Vec<_>>();
    if active_keys.is_empty() {
        return Ok(build_admin_provider_query_bad_request_response(
            ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
        ));
    }
    let active_key_count = active_keys.len();

    let mut all_models = Vec::new();
    let mut all_errors = Vec::new();
    let mut cache_hit_count = 0usize;
    let mut fetch_count = 0usize;
    for key in active_keys {
        let result =
            provider_query_fetch_models_for_key(state.app(), &provider, &endpoints, key, force_refresh)
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
    }

    let models = aggregate_models_for_cache(&all_models);
    let success = !models.is_empty();
    let mut error = if all_errors.is_empty() {
        None
    } else {
        Some(all_errors.join("; "))
    };
    if !success && error.is_none() {
        error = Some("No models returned from any key".to_string());
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
