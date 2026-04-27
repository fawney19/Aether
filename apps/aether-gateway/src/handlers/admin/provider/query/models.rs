use super::payload::{
    provider_query_extract_api_key_id, provider_query_extract_force_refresh,
    provider_query_extract_model, provider_query_extract_provider_id,
    provider_query_extract_request_id,
};
use super::response::{
    build_admin_provider_query_bad_request_response, build_admin_provider_query_not_found_response,
    ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL, ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
    ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL, ADMIN_PROVIDER_QUERY_NO_LOCAL_MODELS_DETAIL,
    ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
    ADMIN_PROVIDER_QUERY_PROVIDER_NOT_FOUND_DETAIL,
};
use crate::ai_pipeline::{maybe_build_sync_finalize_outcome, GatewayControlDecision};
use crate::execution_runtime;
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::model_fetch::ModelFetchRuntimeState;
use crate::provider_key_auth::{
    provider_key_configured_api_formats, provider_key_inherits_provider_api_formats,
};
use crate::provider_transport::kiro::{
    build_kiro_generate_assistant_response_url, build_kiro_provider_headers,
    build_kiro_provider_request_body, supports_local_kiro_request_transport_with_network,
    KiroProviderHeadersInput, KIRO_ENVELOPE_NAME,
};
use crate::usage::GatewaySyncReportRequest;
use crate::{AppState, GatewayError};
use aether_admin::provider::pool as admin_provider_pool_pure;
use aether_contracts::{ExecutionPlan, RequestBody};
use aether_data_contracts::repository::global_models::AdminProviderModelListQuery;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_model_fetch::{
    aggregate_models_for_cache, fetch_models_from_transports, json_string_list,
    preset_models_for_provider, selected_models_fetch_endpoints,
};
use axum::{
    body::{to_bytes, Body},
    http::{self, HeaderMap, HeaderName, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
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
const ANTIGRAVITY_PROVIDER_CACHE_KEY_PREFIX: &str = "upstream_models_provider:";
const DEFAULT_PROVIDER_QUERY_TEST_MESSAGE: &str = "Hello! This is a test message.";

#[derive(Debug)]
struct ProviderQueryKeyFetchResult {
    models: Vec<Value>,
    error: Option<String>,
    from_cache: bool,
    has_success: bool,
}

#[derive(Debug, Clone)]
struct ProviderQueryTestCandidate {
    endpoint: StoredProviderCatalogEndpoint,
    key: StoredProviderCatalogKey,
    effective_model: String,
}

#[derive(Debug, Clone)]
struct ProviderQueryTestAttempt {
    candidate_index: usize,
    endpoint_api_format: String,
    endpoint_base_url: String,
    key_name: String,
    key_id: String,
    auth_type: String,
    effective_model: String,
    status: &'static str,
    skip_reason: Option<String>,
    error_message: Option<String>,
    status_code: Option<u16>,
    latency_ms: Option<u64>,
    request_url: Option<String>,
    request_headers: Option<BTreeMap<String, String>>,
    request_body: Option<Value>,
    response_headers: Option<BTreeMap<String, String>>,
    response_body: Option<Value>,
}

#[derive(Debug, Clone)]
struct ProviderQueryExecutionOutcome {
    status: &'static str,
    skip_reason: Option<String>,
    error_message: Option<String>,
    status_code: Option<u16>,
    latency_ms: Option<u64>,
    request_url: String,
    request_headers: BTreeMap<String, String>,
    request_body: Value,
    response_headers: BTreeMap<String, String>,
    response_body: Option<Value>,
}

fn provider_query_skipped_execution_outcome(
    request_body: Value,
    skip_reason: impl Into<String>,
) -> ProviderQueryExecutionOutcome {
    ProviderQueryExecutionOutcome {
        status: "skipped",
        skip_reason: Some(skip_reason.into()),
        error_message: None,
        status_code: None,
        latency_ms: None,
        request_url: String::new(),
        request_headers: BTreeMap::new(),
        request_body,
        response_headers: BTreeMap::new(),
        response_body: None,
    }
}

fn provider_query_default_local_test_error(route_path: &str) -> &'static str {
    if route_path.ends_with("/test-model") {
        ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_MESSAGE
    } else {
        ADMIN_PROVIDER_QUERY_LOCAL_TEST_MODEL_FAILOVER_MESSAGE
    }
}

fn provider_query_unsupported_test_api_format_message(api_format: &str) -> String {
    let api_format = api_format.trim();
    if api_format.is_empty() {
        "Rust local provider-query model test does not support an empty endpoint format".to_string()
    } else {
        format!(
            "Rust local provider-query model test does not support endpoint format {api_format}"
        )
    }
}

fn provider_query_provider_payload(provider: &StoredProviderCatalogProvider) -> Value {
    json!({
        "id": provider.id.clone(),
        "name": provider.name.clone(),
        "display_name": provider.name.clone(),
    })
}

fn provider_query_test_mode(payload: &Value) -> &str {
    payload
        .get("mode")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("global")
}

fn provider_query_extract_endpoint_id(payload: &Value) -> Option<String> {
    payload
        .get("endpoint_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn provider_query_extract_api_format(payload: &Value) -> Option<String> {
    payload
        .get("api_format")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn provider_query_extract_message(payload: &Value) -> Option<String> {
    payload
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn provider_query_extract_request_body(payload: &Value) -> Option<Value> {
    payload
        .get("request_body")
        .filter(|value| value.is_object())
        .cloned()
}

fn provider_query_extract_request_headers(payload: &Value) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let Some(values) = payload.get("request_headers").and_then(Value::as_object) else {
        return headers;
    };
    for (key, value) in values {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let Some(value) = (match value {
            Value::String(value) => Some(value.trim().to_string()),
            Value::Bool(value) => Some(value.to_string()),
            Value::Number(value) => Some(value.to_string()),
            other => serde_json::to_string(other).ok(),
        }) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(key.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            continue;
        };
        headers.insert(name, value);
    }
    headers
}

fn provider_query_build_test_request_body(payload: &Value, model: &str) -> Value {
    if let Some(mut body) = provider_query_extract_request_body(payload) {
        if let Some(object) = body.as_object_mut() {
            object.insert("model".to_string(), Value::String(model.to_string()));
        }
        return body;
    }

    json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": provider_query_extract_message(payload)
                .unwrap_or_else(|| DEFAULT_PROVIDER_QUERY_TEST_MESSAGE.to_string())
        }],
        "max_tokens": 30,
        "temperature": 0.7,
        "stream": true,
    })
}

fn provider_query_select_kiro_endpoint<'a>(
    endpoints: &'a [StoredProviderCatalogEndpoint],
    endpoint_id: Option<&str>,
    api_format: Option<&str>,
) -> Result<Option<&'a StoredProviderCatalogEndpoint>, &'static str> {
    if let Some(endpoint_id) = endpoint_id {
        let endpoint = endpoints.iter().find(|endpoint| endpoint.id == endpoint_id);
        return endpoint
            .ok_or("Endpoint not found")
            .map(|endpoint| Some(endpoint));
    }

    if let Some(api_format) = api_format {
        let endpoint = endpoints.iter().find(|endpoint| {
            endpoint.is_active && endpoint.api_format.trim().eq_ignore_ascii_case(api_format)
        });
        return Ok(endpoint);
    }

    Ok(endpoints.iter().find(|endpoint| endpoint.is_active))
}

fn provider_query_key_supports_endpoint(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    endpoint_api_format: &str,
) -> bool {
    if provider_key_inherits_provider_api_formats(key, provider_type) {
        return true;
    }
    let formats = provider_key_configured_api_formats(key);
    let endpoint_api_format = provider_query_normalize_api_format_alias(endpoint_api_format);
    formats.is_empty()
        || formats
            .iter()
            .any(|value| provider_query_normalize_api_format_alias(value) == endpoint_api_format)
}

fn provider_query_normalize_api_format_alias(value: &str) -> String {
    crate::ai_pipeline::normalize_legacy_openai_format_alias(value)
}

fn provider_query_transport_supports_standard_test_execution(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    api_format: &str,
) -> bool {
    match crate::ai_pipeline::normalize_legacy_openai_format_alias(api_format).as_str() {
        "openai:chat" => {
            crate::provider_transport::policy::supports_local_openai_chat_transport(transport)
        }
        "openai:responses" => {
            crate::provider_transport::policy::supports_local_standard_transport_with_network(
                transport, api_format,
            )
        }
        "claude:chat" | "claude:cli" => {
            crate::provider_transport::policy::supports_local_standard_transport_with_network(
                transport, api_format,
            )
        }
        "gemini:chat" | "gemini:cli" => {
            if crate::provider_transport::is_vertex_api_key_transport_context(transport) {
                aether_provider_transport::vertex::supports_local_vertex_api_key_gemini_transport_with_network(transport)
            } else {
                state.supports_local_gemini_transport_with_network(transport, api_format)
            }
        }
        _ => false,
    }
}

async fn provider_query_select_preferred_non_kiro_endpoint(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    keys: &[StoredProviderCatalogKey],
    selected_key_id: Option<&str>,
) -> Option<StoredProviderCatalogEndpoint> {
    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        if !provider_query_prefers_chat_standard_test_api_format(&endpoint.api_format) {
            continue;
        }
        for key in keys {
            if !key.is_active
                || selected_key_id.is_some_and(|value| value != key.id.as_str())
                || !provider_query_key_supports_endpoint(
                    key,
                    &provider.provider_type,
                    &endpoint.api_format,
                )
            {
                continue;
            }
            let Ok(Some(transport)) = state
                .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
                .await
            else {
                continue;
            };
            if provider_query_transport_supports_standard_test_execution(
                state,
                &transport,
                endpoint.api_format.as_str(),
            ) {
                return Some(endpoint.clone());
            }
        }
    }

    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        if !provider_query_supports_cli_standard_test_api_format(&endpoint.api_format) {
            continue;
        }
        for key in keys {
            if !key.is_active
                || selected_key_id.is_some_and(|value| value != key.id.as_str())
                || !provider_query_key_supports_endpoint(
                    key,
                    &provider.provider_type,
                    &endpoint.api_format,
                )
            {
                continue;
            }
            let Ok(Some(transport)) = state
                .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
                .await
            else {
                continue;
            };
            if provider_query_transport_supports_standard_test_execution(
                state,
                &transport,
                endpoint.api_format.as_str(),
            ) {
                return Some(endpoint.clone());
            }
        }
    }

    endpoints
        .iter()
        .find(|endpoint| {
            endpoint.is_active
                && keys.iter().any(|key| {
                    key.is_active
                        && selected_key_id.is_none_or(|value| value == key.id.as_str())
                        && provider_query_key_supports_endpoint(
                            key,
                            &provider.provider_type,
                            &endpoint.api_format,
                        )
                })
        })
        .or_else(|| endpoints.iter().find(|endpoint| endpoint.is_active))
        .cloned()
}

fn provider_query_test_key_sort_key(
    provider_type: &str,
    key: &StoredProviderCatalogKey,
    endpoint_api_format: &str,
) -> (u8, u8, i32, u64, i32) {
    let quota_exhausted =
        admin_provider_pool_pure::admin_pool_key_account_quota_exhausted(key, provider_type);
    let circuit_open = key
        .circuit_breaker_by_format
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|value| value.get(endpoint_api_format))
        .and_then(Value::as_object)
        .and_then(|value| value.get("open"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let health_score = key
        .health_by_format
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|value| value.get(endpoint_api_format))
        .and_then(Value::as_object)
        .and_then(|value| value.get("health_score"))
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let consecutive_failures = key
        .health_by_format
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|value| value.get(endpoint_api_format))
        .and_then(Value::as_object)
        .and_then(|value| value.get("consecutive_failures"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let normalized_health = (health_score.clamp(0.0, 1.0) * 1000.0).round() as i32;

    (
        if quota_exhausted { 1 } else { 0 },
        if circuit_open { 1 } else { 0 },
        -normalized_health,
        consecutive_failures,
        key.internal_priority,
    )
}

async fn provider_query_resolve_global_effective_model(
    state: &AdminAppState<'_>,
    provider_id: &str,
    requested_model: &str,
) -> Result<String, GatewayError> {
    let models = state
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: provider_id.to_string(),
            is_active: Some(true),
            offset: 0,
            limit: 1024,
        })
        .await
        .unwrap_or_default();

    Ok(models
        .into_iter()
        .find(|model| {
            model.is_available
                && model
                    .global_model_name
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(requested_model))
        })
        .map(|model| model.provider_model_name)
        .unwrap_or_else(|| requested_model.to_string()))
}

async fn provider_query_build_kiro_test_candidates(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    payload: &Value,
    requested_model_override: Option<&str>,
) -> Result<Vec<ProviderQueryTestCandidate>, Response<Body>> {
    let provider_ids = vec![provider.id.clone()];
    let endpoints = state
        .app()
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await
        .map_err(|_| {
            build_admin_provider_query_bad_request_response(
                ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
            )
        })?;
    let all_keys = state
        .app()
        .list_provider_catalog_keys_by_provider_ids(&provider_ids)
        .await
        .map_err(|_| {
            build_admin_provider_query_bad_request_response(
                ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
            )
        })?;
    let selected_key_id = provider_query_extract_api_key_id(payload);
    let requested_endpoint_id = provider_query_extract_endpoint_id(payload);
    let requested_api_format = provider_query_extract_api_format(payload);
    let endpoint = if requested_endpoint_id.is_none()
        && requested_api_format.is_none()
        && !provider.provider_type.trim().eq_ignore_ascii_case("kiro")
    {
        provider_query_select_preferred_non_kiro_endpoint(
            state,
            provider,
            &endpoints,
            &all_keys,
            selected_key_id.as_deref(),
        )
        .await
        .ok_or_else(|| {
            build_admin_provider_query_not_found_response(
                ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
            )
        })?
    } else {
        match provider_query_select_kiro_endpoint(
            &endpoints,
            requested_endpoint_id.as_deref(),
            requested_api_format.as_deref(),
        ) {
            Ok(Some(endpoint)) => endpoint.clone(),
            Ok(None) => {
                return Err(build_admin_provider_query_not_found_response(
                    ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
                ));
            }
            Err("Endpoint not found") => {
                return Err(build_admin_provider_query_not_found_response(
                    ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL,
                ));
            }
            Err(_) => {
                return Err(build_admin_provider_query_not_found_response(
                    ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL,
                ));
            }
        }
    };

    if let Some(api_key_id) = selected_key_id.as_deref() {
        let Some(key) = all_keys.iter().find(|key| key.id == api_key_id) else {
            return Err(build_admin_provider_query_not_found_response(
                ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL,
            ));
        };
        if !key.is_active
            || !provider_query_key_supports_endpoint(
                key,
                &provider.provider_type,
                &endpoint.api_format,
            )
        {
            return Err(build_admin_provider_query_not_found_response(
                ADMIN_PROVIDER_QUERY_NO_ACTIVE_TEST_CANDIDATE_DETAIL,
            ));
        }
    }

    let requested_model = requested_model_override
        .map(ToOwned::to_owned)
        .or_else(|| provider_query_extract_model(payload))
        .or_else(|| {
            super::payload::provider_query_extract_failover_models(payload)
                .first()
                .cloned()
        })
        .ok_or_else(|| {
            build_admin_provider_query_bad_request_response(
                ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
            )
        })?;
    let effective_model = if provider_query_test_mode(payload).eq_ignore_ascii_case("direct") {
        requested_model.clone()
    } else {
        provider_query_resolve_global_effective_model(state, &provider.id, &requested_model)
            .await
            .unwrap_or(requested_model.clone())
    };

    let mut keys = all_keys
        .into_iter()
        .filter(|key| key.is_active)
        .filter(|key| {
            selected_key_id
                .as_deref()
                .is_none_or(|value| value == key.id.as_str())
        })
        .filter(|key| {
            provider_query_key_supports_endpoint(key, &provider.provider_type, &endpoint.api_format)
        })
        .collect::<Vec<_>>();
    keys.sort_by_key(|key| {
        provider_query_test_key_sort_key(provider.provider_type.as_str(), key, &endpoint.api_format)
    });

    let candidates = keys
        .into_iter()
        .map(|key| ProviderQueryTestCandidate {
            endpoint: endpoint.clone(),
            key,
            effective_model: effective_model.clone(),
        })
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        return Err(build_admin_provider_query_not_found_response(
            ADMIN_PROVIDER_QUERY_NO_ACTIVE_TEST_CANDIDATE_DETAIL,
        ));
    }

    Ok(candidates)
}

fn provider_query_decode_execution_body(
    result: &aether_contracts::ExecutionResult,
) -> Option<Vec<u8>> {
    result
        .body
        .as_ref()
        .and_then(|body| body.body_bytes_b64.as_deref())
        .and_then(|value| base64::engine::general_purpose::STANDARD.decode(value).ok())
}

fn provider_query_extract_error_message(
    result: &aether_contracts::ExecutionResult,
) -> Option<String> {
    result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
        .and_then(Value::as_object)
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_object)
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            provider_query_decode_execution_body(result)
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            result
                .error
                .as_ref()
                .map(|error| error.message.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

async fn provider_query_finalize_kiro_result(
    route_path: &str,
    trace_id: &str,
    requested_model: &str,
    endpoint_api_format: &str,
    effective_model: &str,
    original_request_body: &Value,
    result: &aether_contracts::ExecutionResult,
) -> Result<Option<Value>, GatewayError> {
    let decision = GatewayControlDecision::synthetic(
        route_path,
        Some("admin_proxy".to_string()),
        Some("provider_query_manage".to_string()),
        Some("test_model_failover".to_string()),
        Some(endpoint_api_format.to_string()),
    );
    let payload = GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind: "claude_cli_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": endpoint_api_format,
            "provider_api_format": endpoint_api_format,
            "model": requested_model,
            "mapped_model": effective_model,
            "needs_conversion": false,
            "has_envelope": true,
            "envelope_name": KIRO_ENVELOPE_NAME,
            "original_request_body": original_request_body,
        })),
        status_code: result.status_code,
        headers: result.headers.clone(),
        body_json: result.body.as_ref().and_then(|body| body.json_body.clone()),
        client_body_json: None,
        body_base64: result
            .body
            .as_ref()
            .and_then(|body| body.body_bytes_b64.clone()),
        telemetry: result.telemetry.clone(),
    };

    let Some(outcome) = maybe_build_sync_finalize_outcome(trace_id, &decision, &payload)? else {
        return Ok(None);
    };
    let bytes = to_bytes(outcome.response.into_body(), usize::MAX)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    serde_json::from_slice::<Value>(&bytes)
        .map(Some)
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

async fn provider_query_execute_kiro_test_candidate(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    candidate: &ProviderQueryTestCandidate,
    payload: &Value,
    route_path: &str,
    trace_id: &str,
    requested_model: &str,
) -> Result<ProviderQueryExecutionOutcome, GatewayError> {
    let Some(transport) = state
        .read_provider_transport_snapshot(&provider.id, &candidate.endpoint.id, &candidate.key.id)
        .await?
    else {
        return Ok(provider_query_skipped_execution_outcome(
            Value::Null,
            "Provider transport snapshot is unavailable",
        ));
    };

    if !supports_local_kiro_request_transport_with_network(&transport) {
        return Ok(provider_query_skipped_execution_outcome(
            Value::Null,
            "Kiro local transport is unavailable for this endpoint",
        ));
    }

    let Some(kiro_auth) = state
        .resolve_local_oauth_kiro_request_auth(&transport)
        .await?
    else {
        return Ok(ProviderQueryExecutionOutcome {
            status: "failed",
            skip_reason: None,
            error_message: Some("oauth auth failed".to_string()),
            status_code: None,
            latency_ms: None,
            request_url: String::new(),
            request_headers: BTreeMap::new(),
            request_body: Value::Null,
            response_headers: BTreeMap::new(),
            response_body: None,
        });
    };

    let request_body = provider_query_build_test_request_body(payload, &candidate.effective_model);
    let provider_request_body = match build_kiro_provider_request_body(
        &request_body,
        &candidate.effective_model,
        &kiro_auth.auth_config,
        transport.endpoint.body_rules.as_ref(),
    ) {
        Some(body) => body,
        None => {
            return Ok(ProviderQueryExecutionOutcome {
                status: "failed",
                skip_reason: None,
                error_message: Some("provider request body build failed".to_string()),
                status_code: None,
                latency_ms: None,
                request_url: String::new(),
                request_headers: BTreeMap::new(),
                request_body,
                response_headers: BTreeMap::new(),
                response_body: None,
            });
        }
    };

    let mut synthetic_request = http::Request::builder()
        .uri(route_path)
        .body(())
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    *synthetic_request.headers_mut() = provider_query_extract_request_headers(payload);
    let (parts, _) = synthetic_request.into_parts();

    let request_url = build_kiro_generate_assistant_response_url(
        &transport.endpoint.base_url,
        parts.uri.query(),
        Some(kiro_auth.auth_config.effective_api_region()),
    )
    .ok_or_else(|| GatewayError::Internal("kiro request url is unavailable".to_string()))?;
    let request_headers = build_kiro_provider_headers(KiroProviderHeadersInput {
        headers: &parts.headers,
        provider_request_body: &provider_request_body,
        original_request_body: &request_body,
        header_rules: transport.endpoint.header_rules.as_ref(),
        auth_header: kiro_auth.name,
        auth_value: &kiro_auth.value,
        auth_config: &kiro_auth.auth_config,
        machine_id: kiro_auth.machine_id.as_str(),
    })
    .ok_or_else(|| GatewayError::Internal("kiro request headers are unavailable".to_string()))?;

    let plan = ExecutionPlan {
        request_id: trace_id.to_string(),
        candidate_id: Some(format!("provider-query-{}", candidate.key.id)),
        provider_name: Some(provider.name.clone()),
        provider_id: provider.id.clone(),
        endpoint_id: candidate.endpoint.id.clone(),
        key_id: candidate.key.id.clone(),
        method: "POST".to_string(),
        url: request_url.clone(),
        headers: request_headers.clone(),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(provider_request_body.clone()),
        stream: true,
        client_api_format: candidate.endpoint.api_format.clone(),
        provider_api_format: candidate.endpoint.api_format.clone(),
        model_name: Some(candidate.effective_model.clone()),
        proxy: state
            .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
            .await,
        tls_profile: state.resolve_transport_tls_profile(&transport),
        timeouts: state.resolve_transport_execution_timeouts(&transport),
    };

    let result = state
        .execute_execution_runtime_sync_plan(Some(trace_id), &plan)
        .await?;
    let response_body = if result.status_code < 400 {
        provider_query_finalize_kiro_result(
            route_path,
            trace_id,
            requested_model,
            candidate.endpoint.api_format.as_str(),
            candidate.effective_model.as_str(),
            &request_body,
            &result,
        )
        .await?
    } else {
        result.body.as_ref().and_then(|body| body.json_body.clone())
    };
    let did_fail = result.status_code >= 400;
    let error_message = if did_fail {
        provider_query_extract_error_message(&result)
    } else if response_body.is_none()
        && provider_query_decode_execution_body(&result)
            .is_some_and(|body| crate::ai_pipeline::stream_body_contains_error_event(&body))
    {
        Some("Kiro upstream returned embedded stream error".to_string())
    } else {
        None
    };

    Ok(ProviderQueryExecutionOutcome {
        status: if did_fail || error_message.is_some() {
            "failed"
        } else {
            "success"
        },
        skip_reason: None,
        error_message,
        status_code: Some(result.status_code),
        latency_ms: result.telemetry.as_ref().and_then(|value| value.elapsed_ms),
        request_url,
        request_headers,
        request_body: provider_request_body,
        response_headers: result.headers,
        response_body,
    })
}

async fn provider_query_execute_standard_test_candidate(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    candidate: &ProviderQueryTestCandidate,
    payload: &Value,
    route_path: &str,
    trace_id: &str,
) -> Result<ProviderQueryExecutionOutcome, GatewayError> {
    let Some(transport) = state
        .read_provider_transport_snapshot(&provider.id, &candidate.endpoint.id, &candidate.key.id)
        .await?
    else {
        return Ok(provider_query_skipped_execution_outcome(
            Value::Null,
            "Provider transport snapshot is unavailable",
        ));
    };
    if !provider_query_transport_supports_standard_test_execution(
        state,
        &transport,
        candidate.endpoint.api_format.as_str(),
    ) {
        return Ok(ProviderQueryExecutionOutcome {
            status: "skipped",
            skip_reason: None,
            error_message: None,
            status_code: None,
            latency_ms: None,
            request_url: String::new(),
            request_headers: BTreeMap::new(),
            request_body: Value::Null,
            response_headers: BTreeMap::new(),
            response_body: None,
        });
    }

    let original_request_body =
        provider_query_build_test_request_body(payload, &candidate.effective_model);
    let mut request_body = original_request_body.clone();
    if let Some(object) = request_body.as_object_mut() {
        object.insert("stream".to_string(), Value::Bool(false));
    }

    let provider_api_format = candidate.endpoint.api_format.as_str();
    let normalized_provider_api_format =
        crate::ai_pipeline::normalize_legacy_openai_format_alias(provider_api_format);
    let provider_request_body = match normalized_provider_api_format.as_str() {
        "openai:chat" => {
            let Some(mut provider_request_body) =
                crate::ai_pipeline::build_local_openai_chat_request_body(
                    &request_body,
                    &candidate.effective_model,
                    false,
                )
            else {
                return Ok(provider_query_skipped_execution_outcome(
                    request_body.clone(),
                    format!("Provider request body could not be built for {provider_api_format}"),
                ));
            };
            if !crate::provider_transport::apply_local_body_rules(
                &mut provider_request_body,
                transport.endpoint.body_rules.as_ref(),
                Some(&request_body),
            ) {
                return Ok(provider_query_skipped_execution_outcome(
                    request_body.clone(),
                    format!("Provider request body rules rejected {provider_api_format}"),
                ));
            }
            provider_request_body
        }
        "claude:chat" | "claude:cli" | "gemini:chat" | "gemini:cli" => {
            let Some(mut provider_request_body) =
                crate::ai_pipeline::build_cross_format_openai_chat_request_body(
                    &request_body,
                    &candidate.effective_model,
                    normalized_provider_api_format.as_str(),
                    false,
                )
            else {
                return Ok(provider_query_skipped_execution_outcome(
                    request_body.clone(),
                    format!("Provider request body could not be built for {provider_api_format}"),
                ));
            };
            if !crate::provider_transport::apply_local_body_rules(
                &mut provider_request_body,
                transport.endpoint.body_rules.as_ref(),
                Some(&request_body),
            ) {
                return Ok(provider_query_skipped_execution_outcome(
                    request_body.clone(),
                    format!("Provider request body rules rejected {provider_api_format}"),
                ));
            }
            provider_request_body
        }
        "openai:responses" => {
            let Some(mut provider_request_body) =
                crate::ai_pipeline::build_cross_format_openai_chat_request_body(
                    &request_body,
                    &candidate.effective_model,
                    normalized_provider_api_format.as_str(),
                    false,
                )
            else {
                return Ok(provider_query_skipped_execution_outcome(
                    request_body.clone(),
                    format!("Provider request body could not be built for {provider_api_format}"),
                ));
            };
            if !crate::provider_transport::apply_local_body_rules(
                &mut provider_request_body,
                transport.endpoint.body_rules.as_ref(),
                Some(&request_body),
            ) {
                return Ok(provider_query_skipped_execution_outcome(
                    request_body.clone(),
                    format!("Provider request body rules rejected {provider_api_format}"),
                ));
            }
            crate::ai_pipeline::apply_codex_openai_responses_special_body_edits(
                &mut provider_request_body,
                transport.provider.provider_type.as_str(),
                provider_api_format,
                transport.endpoint.body_rules.as_ref(),
                Some(candidate.key.id.as_str()),
            );
            crate::ai_pipeline::apply_openai_responses_compact_special_body_edits(
                &mut provider_request_body,
                provider_api_format,
            );
            provider_request_body
        }
        _ => {
            return Ok(provider_query_skipped_execution_outcome(
                request_body.clone(),
                provider_query_unsupported_test_api_format_message(provider_api_format),
            ));
        }
    };

    let uses_vertex_query_auth =
        crate::provider_transport::uses_vertex_api_key_query_auth(&transport, provider_api_format);
    let vertex_query_auth = if uses_vertex_query_auth {
        aether_provider_transport::vertex::resolve_local_vertex_api_key_query_auth(&transport)
    } else {
        None
    };
    let oauth_auth = match crate::ai_pipeline::normalize_legacy_openai_format_alias(
        provider_api_format,
    )
    .as_str()
    {
        "openai:chat" | "openai:responses" | "claude:chat" | "claude:cli" | "gemini:chat"
        | "gemini:cli" => state.resolve_local_oauth_header_auth(&transport).await?,
        _ => None,
    };
    let auth = match crate::ai_pipeline::normalize_legacy_openai_format_alias(provider_api_format)
        .as_str()
    {
        "openai:chat" | "openai:responses" => {
            crate::provider_transport::auth::resolve_local_openai_bearer_auth(&transport)
                .or(oauth_auth)
        }
        "claude:chat" | "claude:cli" => {
            crate::provider_transport::auth::resolve_local_standard_auth(&transport).or(oauth_auth)
        }
        "gemini:chat" | "gemini:cli" => {
            if uses_vertex_query_auth {
                oauth_auth
            } else {
                state.resolve_local_gemini_auth(&transport).or(oauth_auth)
            }
        }
        _ => None,
    };
    let (auth_header, auth_value) = match auth {
        Some((auth_header, auth_value)) => (Some(auth_header), Some(auth_value)),
        None if uses_vertex_query_auth && vertex_query_auth.is_some() => (None, None),
        None => {
            return Ok(provider_query_skipped_execution_outcome(
                provider_request_body,
                format!("Provider auth is unavailable for {provider_api_format}"),
            ));
        }
    };

    let mut synthetic_request = http::Request::builder()
        .uri(route_path)
        .body(())
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    *synthetic_request.headers_mut() = provider_query_extract_request_headers(payload);
    let (parts, _) = synthetic_request.into_parts();

    let request_url = crate::provider_transport::build_transport_request_url(
        &transport,
        crate::provider_transport::TransportRequestUrlParams {
            provider_api_format,
            mapped_model: Some(candidate.effective_model.as_str()),
            upstream_is_stream: false,
            request_query: parts.uri.query(),
            kiro_api_region: None,
        },
    );
    let Some(request_url) = request_url else {
        return Ok(provider_query_skipped_execution_outcome(
            provider_request_body,
            format!("Provider request URL is unavailable for {provider_api_format}"),
        ));
    };

    let mut request_headers = match provider_api_format {
        "claude:chat" | "claude:cli" => {
            crate::provider_transport::auth::build_claude_passthrough_headers(
                &parts.headers,
                auth_header.as_deref().unwrap_or_default(),
                auth_value.as_deref().unwrap_or_default(),
                &BTreeMap::new(),
                Some("application/json"),
            )
        }
        "openai:responses" => {
            crate::provider_transport::auth::build_complete_passthrough_headers_with_auth(
                &parts.headers,
                auth_header.as_deref().unwrap_or_default(),
                auth_value.as_deref().unwrap_or_default(),
                &BTreeMap::new(),
                Some("application/json"),
            )
        }
        _ => match (auth_header.as_deref(), auth_value.as_deref()) {
            (Some(auth_header), Some(auth_value)) => state.build_passthrough_headers_with_auth(
                &parts.headers,
                auth_header,
                auth_value,
                &BTreeMap::new(),
            ),
            _ => crate::provider_transport::auth::build_passthrough_headers(
                &parts.headers,
                &BTreeMap::new(),
                Some("application/json"),
            ),
        },
    };
    if uses_vertex_query_auth {
        request_headers.remove("x-goog-api-key");
    }
    request_headers
        .entry("content-type".to_string())
        .or_insert_with(|| "application/json".to_string());
    let protected_headers = if uses_vertex_query_auth {
        vec!["content-type"]
    } else {
        vec![auth_header.as_deref().unwrap_or_default(), "content-type"]
    };
    if !state.apply_local_header_rules(
        &mut request_headers,
        transport.endpoint.header_rules.as_ref(),
        &protected_headers,
        &provider_request_body,
        Some(&request_body),
    ) {
        return Ok(ProviderQueryExecutionOutcome {
            status: "failed",
            skip_reason: None,
            error_message: Some("provider request headers build failed".to_string()),
            status_code: None,
            latency_ms: None,
            request_url,
            request_headers,
            request_body: provider_request_body,
            response_headers: BTreeMap::new(),
            response_body: None,
        });
    }
    if crate::ai_pipeline::is_openai_responses_format(provider_api_format) {
        crate::ai_pipeline::apply_codex_openai_responses_special_headers(
            &mut request_headers,
            &provider_request_body,
            &parts.headers,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            Some(trace_id),
            transport.key.decrypted_auth_config.as_deref(),
        );
    }
    if !uses_vertex_query_auth {
        if let (Some(auth_header), Some(auth_value)) =
            (auth_header.as_deref(), auth_value.as_deref())
        {
            crate::provider_transport::ensure_upstream_auth_header(
                &mut request_headers,
                auth_header,
                auth_value,
            );
        }
    }

    let plan = ExecutionPlan {
        request_id: trace_id.to_string(),
        candidate_id: Some(format!("provider-query-{}", candidate.key.id)),
        provider_name: Some(provider.name.clone()),
        provider_id: provider.id.clone(),
        endpoint_id: candidate.endpoint.id.clone(),
        key_id: candidate.key.id.clone(),
        method: "POST".to_string(),
        url: request_url.clone(),
        headers: request_headers.clone(),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(provider_request_body.clone()),
        stream: false,
        client_api_format: "openai:chat".to_string(),
        provider_api_format: candidate.endpoint.api_format.clone(),
        model_name: Some(candidate.effective_model.clone()),
        proxy: state
            .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
            .await,
        tls_profile: state.resolve_transport_tls_profile(&transport),
        timeouts: state.resolve_transport_execution_timeouts(&transport),
    };

    let result = state
        .execute_execution_runtime_sync_plan(Some(trace_id), &plan)
        .await?;
    let response_body = result.body.as_ref().and_then(|body| body.json_body.clone());
    let did_fail = result.status_code >= 400;
    let error_message = if did_fail {
        provider_query_extract_error_message(&result)
    } else {
        None
    };

    Ok(ProviderQueryExecutionOutcome {
        status: if did_fail { "failed" } else { "success" },
        skip_reason: None,
        error_message,
        status_code: Some(result.status_code),
        latency_ms: result.telemetry.as_ref().and_then(|value| value.elapsed_ms),
        request_url,
        request_headers,
        request_body: provider_request_body,
        response_headers: result.headers,
        response_body,
    })
}

fn provider_query_test_attempt_payload(
    candidate_index: usize,
    candidate: &ProviderQueryTestCandidate,
    execution: &ProviderQueryExecutionOutcome,
) -> Value {
    json!({
        "candidate_index": candidate_index,
        "retry_index": 0,
        "endpoint_api_format": candidate.endpoint.api_format,
        "endpoint_base_url": candidate.endpoint.base_url,
        "key_name": provider_query_key_display_name(&candidate.key),
        "key_id": candidate.key.id,
        "auth_type": candidate.key.auth_type,
        "effective_model": candidate.effective_model,
        "status": execution.status,
        "skip_reason": execution.skip_reason,
        "error_message": execution.error_message,
        "status_code": execution.status_code,
        "latency_ms": execution.latency_ms,
        "request_url": execution.request_url,
        "request_headers": execution.request_headers,
        "request_body": execution.request_body,
        "response_headers": execution.response_headers,
        "response_body": execution.response_body,
    })
}

fn provider_query_prefers_chat_standard_test_api_format(api_format: &str) -> bool {
    matches!(api_format, "openai:chat" | "claude:chat" | "gemini:chat")
}

fn provider_query_supports_cli_standard_test_api_format(api_format: &str) -> bool {
    matches!(
        crate::ai_pipeline::normalize_legacy_openai_format_alias(api_format).as_str(),
        "openai:responses" | "claude:cli" | "gemini:cli"
    )
}

async fn build_admin_provider_query_kiro_failover_response(
    state: &AdminAppState<'_>,
    payload: &Value,
    route_path: &str,
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
    let failover_models = super::payload::provider_query_extract_failover_models(payload);
    let is_kiro = provider.provider_type.trim().eq_ignore_ascii_case("kiro");
    let Some(requested_model) =
        provider_query_extract_model(payload).or_else(|| failover_models.first().cloned())
    else {
        return Ok(build_admin_provider_query_bad_request_response(
            ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
        ));
    };

    let requested_models = if is_kiro {
        vec![requested_model.clone()]
    } else if failover_models.is_empty() {
        vec![requested_model.clone()]
    } else {
        failover_models.clone()
    };
    let mut candidates = Vec::new();
    for requested_failover_model in &requested_models {
        match provider_query_build_kiro_test_candidates(
            state,
            &provider,
            payload,
            Some(requested_failover_model.as_str()),
        )
        .await
        {
            Ok(mut built_candidates) => candidates.append(&mut built_candidates),
            Err(response) => return Ok(response),
        }
    }
    if !is_kiro {
        let mut supported_candidates = Vec::new();
        for candidate in candidates {
            let Some(transport) = state
                .read_provider_transport_snapshot(
                    &provider.id,
                    &candidate.endpoint.id,
                    &candidate.key.id,
                )
                .await?
            else {
                continue;
            };
            if provider_query_transport_supports_standard_test_execution(
                state,
                &transport,
                candidate.endpoint.api_format.as_str(),
            ) {
                supported_candidates.push(candidate);
            }
        }
        candidates = supported_candidates;
    }
    if !is_kiro && candidates.is_empty() {
        return Ok(build_admin_provider_query_test_model_failover_response(
            provider_id,
            requested_models,
        ));
    }
    let trace_id = provider_query_extract_request_id(payload)
        .unwrap_or_else(|| format!("provider-query-test-{}", Uuid::new_v4().simple()));
    let mut attempts = Vec::new();
    let mut total_attempts = 0usize;
    let mut success_body = None;

    for (candidate_index, candidate) in candidates.iter().enumerate() {
        let execution = if is_kiro {
            provider_query_execute_kiro_test_candidate(
                state,
                &provider,
                candidate,
                payload,
                route_path,
                &trace_id,
                &requested_model,
            )
            .await?
        } else {
            provider_query_execute_standard_test_candidate(
                state, &provider, candidate, payload, route_path, &trace_id,
            )
            .await?
        };
        if execution.status != "skipped" {
            total_attempts += 1;
        }
        let is_success = execution.status == "success";
        let response_body = execution.response_body.clone();
        attempts.push(provider_query_test_attempt_payload(
            candidate_index,
            candidate,
            &execution,
        ));
        if is_success {
            success_body = response_body;
            break;
        }
    }

    let success = success_body.is_some();
    let error = if success {
        Value::Null
    } else {
        attempts
            .iter()
            .rev()
            .find_map(|attempt| {
                attempt
                    .get("error_message")
                    .cloned()
                    .filter(|value| !value.is_null())
            })
            .or_else(|| {
                attempts.iter().rev().find_map(|attempt| {
                    attempt
                        .get("skip_reason")
                        .cloned()
                        .filter(|value| !value.is_null())
                })
            })
            .unwrap_or_else(|| json!(provider_query_default_local_test_error(route_path)))
    };

    Ok(Json(json!({
        "success": success,
        "model": requested_model,
        "provider": provider_query_provider_payload(&provider),
        "attempts": attempts,
        "total_candidates": candidates.len(),
        "total_attempts": total_attempts,
        "data": success_body.as_ref().map(|body| json!({
            "stream": is_kiro,
            "response": body,
        })),
        "error": error,
    }))
    .into_response())
}

pub(crate) async fn build_admin_provider_query_test_model_local_response(
    state: &AdminAppState<'_>,
    payload: &Value,
) -> Result<Response<Body>, GatewayError> {
    let response = build_admin_provider_query_kiro_failover_response(
        state,
        payload,
        "/api/admin/provider-query/test-model",
    )
    .await?;
    if !response.status().is_success() {
        return Ok(response);
    }
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let parsed: Value =
        serde_json::from_slice(&body).map_err(|err| GatewayError::Internal(err.to_string()))?;

    Ok(Json(json!({
        "success": parsed.get("success").cloned().unwrap_or(Value::Bool(false)),
        "error": parsed.get("error").cloned().unwrap_or(Value::Null),
        "data": parsed.get("data").cloned().unwrap_or(Value::Null),
        "provider": parsed.get("provider").cloned().unwrap_or(Value::Null),
        "model": parsed.get("model").cloned().unwrap_or(Value::Null),
    }))
    .into_response())
}

pub(crate) async fn build_admin_provider_query_test_model_failover_local_response(
    state: &AdminAppState<'_>,
    payload: &Value,
) -> Result<Response<Body>, GatewayError> {
    build_admin_provider_query_kiro_failover_response(
        state,
        payload,
        "/api/admin/provider-query/test-model-failover",
    )
    .await
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
