use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_usage_runtime::{
    build_usage_event_data_seed, UsageEvent, UsageEventData, UsageEventType,
};
use axum::body::Body;
use axum::body::Bytes;
use axum::http::{self, HeaderMap, Response};
use base64::Engine as _;
use serde_json::{json, Map, Value};
use tracing::warn;

use crate::constants::{
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS, LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER,
};
use crate::control::GatewayControlDecision;
use crate::state::LocalExecutionRuntimeMissDiagnostic;
use crate::AppState;

#[derive(Debug)]
pub(crate) enum LocalExecutionRequestOutcome {
    Responded(Response<Body>),
    Exhausted(LocalExecutionExhaustion),
    NoPath,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalExecutionExhaustion {
    request_id: String,
    data: UsageEventData,
    candidate_id: Option<String>,
    candidate_index: Option<u32>,
    upstream_status_code: Option<u16>,
    upstream_error_type: Option<String>,
    upstream_error_message: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LocalExecutionRuntimeMissContext {
    pub(crate) auth_user_id: Option<String>,
    pub(crate) auth_api_key_id: Option<String>,
    pub(crate) auth_username: Option<String>,
    pub(crate) auth_api_key_name: Option<String>,
    candidate_contexts: Vec<RuntimeMissCandidateContext>,
}

#[derive(Debug, Clone)]
struct RuntimeMissCandidateContext {
    candidate: StoredRequestCandidate,
    provider_name: Option<String>,
    key_name: Option<String>,
    client_api_format: Option<String>,
    provider_api_format: Option<String>,
    global_model_name: Option<String>,
    selected_provider_model_name: Option<String>,
    endpoint_url: Option<String>,
}

impl LocalExecutionRequestOutcome {
    pub(crate) fn responded(response: Response<Body>) -> Self {
        Self::Responded(response)
    }
}

impl LocalExecutionRuntimeMissContext {
    pub(crate) fn persisted_candidate_count(&self) -> usize {
        self.candidate_contexts.len()
    }

    pub(crate) fn all_candidates_skipped_for_reason(&self, reason: &str) -> bool {
        let reason = reason.trim();
        if reason.is_empty() || self.candidate_contexts.is_empty() {
            return false;
        }

        self.candidate_contexts.iter().all(|candidate| {
            candidate.candidate.status == RequestCandidateStatus::Skipped
                && candidate
                    .candidate
                    .skip_reason
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| value == reason)
        })
    }

    pub(crate) fn candidate_summary(&self) -> Option<String> {
        const MAX_ITEMS: usize = 5;

        if self.candidate_contexts.is_empty() {
            return None;
        }

        let mut summaries = self
            .candidate_contexts
            .iter()
            .take(MAX_ITEMS)
            .map(format_runtime_miss_candidate_summary)
            .collect::<Vec<_>>();
        let remaining = self.candidate_contexts.len().saturating_sub(MAX_ITEMS);
        if remaining > 0 {
            summaries.push(format!("+{remaining} more"));
        }
        Some(summaries.join(" | "))
    }
}

pub(crate) async fn build_local_execution_exhaustion(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> LocalExecutionExhaustion {
    let mut data = build_usage_event_data_seed(plan, report_context);
    let last_failed_candidate = match state
        .read_request_candidates_by_request_id(plan.request_id.as_str())
        .await
    {
        Ok(candidates) => select_last_failed_request_candidate(&candidates).cloned(),
        Err(err) => {
            warn!(
                request_id = %plan.request_id,
                error = ?err,
                "gateway failed to load request candidates for exhausted local execution"
            );
            None
        }
    };

    if let Some(candidate) = last_failed_candidate.as_ref() {
        data.user_id = data.user_id.or_else(|| candidate.user_id.clone());
        data.api_key_id = data.api_key_id.or_else(|| candidate.api_key_id.clone());
        data.username = data.username.or_else(|| candidate.username.clone());
        data.api_key_name = data.api_key_name.or_else(|| candidate.api_key_name.clone());
        data.provider_id = data.provider_id.or_else(|| candidate.provider_id.clone());
        data.provider_endpoint_id = data
            .provider_endpoint_id
            .or_else(|| candidate.endpoint_id.clone());
        data.provider_api_key_id = data
            .provider_api_key_id
            .or_else(|| candidate.key_id.clone());
    }

    LocalExecutionExhaustion {
        request_id: plan.request_id.clone(),
        data,
        candidate_id: last_failed_candidate
            .as_ref()
            .map(|candidate| candidate.id.clone()),
        candidate_index: last_failed_candidate
            .as_ref()
            .map(|candidate| candidate.candidate_index),
        upstream_status_code: last_failed_candidate
            .as_ref()
            .and_then(|candidate| candidate.status_code),
        upstream_error_type: last_failed_candidate
            .as_ref()
            .and_then(|candidate| candidate.error_type.clone())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        upstream_error_message: last_failed_candidate
            .as_ref()
            .and_then(|candidate| candidate.error_message.clone())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    }
}

pub(crate) async fn build_local_execution_runtime_miss_context(
    state: &AppState,
    request_id: &str,
    decision: Option<&GatewayControlDecision>,
) -> LocalExecutionRuntimeMissContext {
    let auth_context = decision.and_then(|value| value.auth_context.as_ref());

    LocalExecutionRuntimeMissContext {
        auth_user_id: auth_context.map(|value| value.user_id.clone()),
        auth_api_key_id: auth_context.map(|value| value.api_key_id.clone()),
        auth_username: auth_context.and_then(|value| value.username.clone()),
        auth_api_key_name: auth_context.and_then(|value| value.api_key_name.clone()),
        candidate_contexts: load_runtime_miss_candidate_contexts_with_retry(
            state, request_id, decision,
        )
        .await,
    }
}

pub(crate) async fn record_failed_usage_for_exhausted_request(
    state: &AppState,
    exhaustion: LocalExecutionExhaustion,
    started_at: &Instant,
    local_execution_runtime_miss_detail: &str,
    execution_path: &str,
    diagnostic: Option<&LocalExecutionRuntimeMissDiagnostic>,
) {
    if !state.usage_runtime.is_enabled() {
        return;
    }

    let LocalExecutionExhaustion {
        request_id,
        mut data,
        candidate_id,
        candidate_index,
        upstream_status_code,
        upstream_error_type,
        upstream_error_message,
    } = exhaustion;

    let status_code = http::StatusCode::SERVICE_UNAVAILABLE.as_u16();
    let candidate_status_code = upstream_status_code.unwrap_or(status_code);
    data.status_code = Some(status_code);
    data.error_message = upstream_error_message
        .clone()
        .or_else(|| Some(local_execution_runtime_miss_detail.to_string()));
    data.error_category = error_category_for_failed_status(status_code);
    data.response_time_ms = Some(started_at.elapsed().as_millis() as u64);
    data.response_headers = Some(json_header_map());
    data.response_body = Some(json!({
        "error": {
            "type": upstream_error_type
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("upstream_error"),
            "message": upstream_error_message
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(local_execution_runtime_miss_detail),
            "code": candidate_status_code,
        }
    }));

    let mut client_headers = Map::from_iter([(
        "content-type".to_string(),
        Value::String("application/json".to_string()),
    )]);
    if let Some(reason) = diagnostic
        .map(|value| value.reason.trim())
        .filter(|value| !value.is_empty())
    {
        client_headers.insert(
            LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER.to_string(),
            Value::String(reason.to_string()),
        );
    }
    data.client_response_headers = Some(Value::Object(client_headers));
    data.client_response_body = Some(json!({
        "error": {
            "type": "http_error",
            "message": beautify_local_execution_client_error_message(local_execution_runtime_miss_detail),
        }
    }));

    let mut request_metadata = match data.request_metadata.take() {
        Some(Value::Object(object)) => object,
        Some(other) => Map::from_iter([("seed".to_string(), other)]),
        None => Map::new(),
    };
    request_metadata.insert("trace_id".to_string(), Value::String(request_id.clone()));
    apply_runtime_miss_usage_routing(
        &mut data,
        &mut request_metadata,
        execution_path,
        candidate_id.as_deref(),
        candidate_index,
        None,
        diagnostic,
        None,
        None,
    );
    data.request_metadata = Some(Value::Object(request_metadata));

    state
        .usage_runtime
        .record_terminal_event(
            state.data.as_ref(),
            UsageEvent::new(UsageEventType::Failed, request_id, data),
        )
        .await;
}

pub(crate) async fn record_failed_usage_for_runtime_miss_request(
    state: &AppState,
    request_id: &str,
    started_at: &Instant,
    local_execution_runtime_miss_detail: &str,
    execution_path: &str,
    decision: Option<&GatewayControlDecision>,
    diagnostic: Option<&LocalExecutionRuntimeMissDiagnostic>,
    context: &LocalExecutionRuntimeMissContext,
    request_headers: &HeaderMap,
    request_body: Option<&Bytes>,
) {
    if !state.usage_runtime.is_enabled() {
        return;
    }

    let selected_candidate =
        select_last_runtime_miss_executed_candidate(&context.candidate_contexts);
    let api_format = selected_candidate
        .and_then(|value| value.client_api_format.clone())
        .or_else(|| {
            trimmed_non_empty(decision.and_then(|value| value.auth_endpoint_signature.as_deref()))
        });
    let provider_api_format = selected_candidate
        .and_then(|value| value.provider_api_format.clone())
        .or_else(|| api_format.clone());
    let provider_name = selected_candidate
        .and_then(|value| value.provider_name.clone())
        .or_else(|| selected_candidate.and_then(|value| value.candidate.provider_id.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    let model = trimmed_non_empty(diagnostic.and_then(|value| value.requested_model.as_deref()))
        .or_else(|| selected_candidate.and_then(|value| value.global_model_name.clone()))
        .or_else(|| selected_candidate.and_then(|value| value.selected_provider_model_name.clone()))
        .unwrap_or_else(|| "unknown".to_string());
    let target_model = selected_candidate
        .and_then(|value| value.selected_provider_model_name.clone())
        .filter(|value| !value.eq_ignore_ascii_case(model.as_str()));

    let status_code = http::StatusCode::SERVICE_UNAVAILABLE.as_u16();
    let client_message =
        beautify_local_execution_client_error_message(local_execution_runtime_miss_detail);
    let client_body = json!({
        "error": {
            "type": "http_error",
            "message": client_message,
        }
    });
    let mut client_headers = Map::from_iter([(
        "content-type".to_string(),
        Value::String("application/json".to_string()),
    )]);
    if let Some(reason) = diagnostic
        .map(|value| value.reason.trim())
        .filter(|value| !value.is_empty())
    {
        client_headers.insert(
            LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER.to_string(),
            Value::String(reason.to_string()),
        );
    }

    let mut request_metadata = Map::new();
    request_metadata.insert(
        "trace_id".to_string(),
        Value::String(request_id.to_string()),
    );
    let mut data = UsageEventData {
        user_id: context.auth_user_id.clone(),
        api_key_id: context.auth_api_key_id.clone(),
        username: context.auth_username.clone(),
        api_key_name: context.auth_api_key_name.clone(),
        provider_name,
        model,
        target_model,
        provider_id: selected_candidate.and_then(|value| value.candidate.provider_id.clone()),
        provider_endpoint_id: selected_candidate
            .and_then(|value| value.candidate.endpoint_id.clone()),
        provider_api_key_id: selected_candidate.and_then(|value| value.candidate.key_id.clone()),
        request_type: Some(infer_request_type(api_format.as_deref())),
        api_format: api_format.clone(),
        api_family: api_format
            .as_deref()
            .and_then(infer_api_family)
            .map(ToOwned::to_owned),
        endpoint_kind: api_format
            .as_deref()
            .and_then(infer_endpoint_kind)
            .map(ToOwned::to_owned),
        endpoint_api_format: provider_api_format.clone(),
        provider_api_family: provider_api_format
            .as_deref()
            .and_then(infer_api_family)
            .map(ToOwned::to_owned),
        provider_endpoint_kind: provider_api_format
            .as_deref()
            .and_then(infer_endpoint_kind)
            .map(ToOwned::to_owned),
        has_format_conversion: selected_candidate.and_then(|value| {
            value
                .client_api_format
                .as_deref()
                .zip(value.provider_api_format.as_deref())
                .map(|(left, right)| !left.eq_ignore_ascii_case(right))
        }),
        status_code: Some(status_code),
        error_message: Some(local_execution_runtime_miss_detail.to_string()),
        error_category: error_category_for_failed_status(status_code),
        response_time_ms: Some(started_at.elapsed().as_millis() as u64),
        request_headers: Some(runtime_miss_original_headers_json(request_headers)),
        request_body: runtime_miss_original_request_body_json(request_headers, request_body),
        response_headers: Some(json_header_map()),
        response_body: Some(client_body.clone()),
        client_response_headers: Some(Value::Object(client_headers)),
        client_response_body: Some(client_body),
        ..UsageEventData::default()
    };
    apply_runtime_miss_usage_routing(
        &mut data,
        &mut request_metadata,
        execution_path,
        selected_candidate.map(|value| value.candidate.id.as_str()),
        selected_candidate.map(|value| value.candidate.candidate_index),
        selected_candidate.and_then(|value| value.key_name.as_deref()),
        diagnostic,
        decision.and_then(|value| value.route_family.as_deref()),
        decision.and_then(|value| value.route_kind.as_deref()),
    );
    data.request_metadata =
        (!request_metadata.is_empty()).then_some(Value::Object(request_metadata));

    state
        .usage_runtime
        .record_terminal_event(
            state.data.as_ref(),
            UsageEvent::new(UsageEventType::Failed, request_id, data),
        )
        .await;
}

pub(crate) fn beautify_local_execution_client_error_message(message: &str) -> String {
    let without_reason_code = strip_parenthesized_reason_code(message);
    let mut simplified = collapse_whitespace(without_reason_code.as_str());
    if let Some(unavailable_message) =
        simplify_all_candidates_skipped_client_error_message(simplified.as_str())
    {
        return unavailable_message;
    }
    for marker in [
        "。请检查",
        "。请确认",
        ". 请检查",
        ". 请确认",
        "! 请检查",
        "! 请确认",
        "? 请检查",
        "? 请确认",
        "。Reason",
        ". Reason",
        "。Code",
        ". Code",
    ] {
        if let Some(index) = simplified.find(marker) {
            simplified.truncate(index);
            break;
        }
    }
    trim_trailing_message_punctuation(simplified.as_str()).to_string()
}

fn simplify_all_candidates_skipped_client_error_message(message: &str) -> Option<String> {
    if !message.contains("候选提供商")
        || !(message.contains("全部不可用") || message.contains("都不满足本次"))
    {
        return None;
    }

    let request_mode = extract_local_execution_request_mode(message)?;
    if let Some(model) = extract_candidate_supported_model(message) {
        return Some(format!(
            "没有可用提供商支持模型 {model} 的{request_mode}请求"
        ));
    }

    Some(format!("没有可用提供商支持本次{request_mode}请求"))
}

fn extract_local_execution_request_mode(message: &str) -> Option<&str> {
    let rest = message.get(message.find("本次")? + "本次".len()..)?;
    let mode = rest.get(..rest.find("请求")?)?.trim();
    (!mode.is_empty()).then_some(mode)
}

fn extract_candidate_supported_model(message: &str) -> Option<&str> {
    let rest = message.get(message.find("支持模型 ")? + "支持模型 ".len()..)?;
    let model = rest.get(..rest.find(" 的")?)?.trim();
    (!model.is_empty()).then_some(model)
}

fn strip_parenthesized_reason_code(message: &str) -> String {
    let Some(reason_index) = message.find("原因代码") else {
        return message.to_string();
    };
    let Some((start, open)) = message[..reason_index]
        .char_indices()
        .rev()
        .find(|(_, ch)| *ch == '（' || *ch == '(')
    else {
        return message.to_string();
    };
    let close = if open == '(' { ')' } else { '）' };
    let Some(close_offset) = message[start..].find(close) else {
        return message[..start].to_string();
    };
    let end = start + close_offset + close.len_utf8();
    format!("{}{}", &message[..start], &message[end..])
}

fn collapse_whitespace(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_trailing_message_punctuation(message: &str) -> &str {
    message
        .trim_end_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '。' | '.' | '!' | '?' | '；' | ';' | '，' | ',' | '：' | ':'
                )
        })
        .trim()
}

fn select_last_failed_request_candidate(
    candidates: &[StoredRequestCandidate],
) -> Option<&StoredRequestCandidate> {
    candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.status,
                RequestCandidateStatus::Failed | RequestCandidateStatus::Cancelled
            )
        })
        .max_by_key(|candidate| {
            (
                candidate.retry_index,
                candidate.candidate_index,
                candidate
                    .finished_at_unix_ms
                    .or(candidate.started_at_unix_ms)
                    .unwrap_or(candidate.created_at_unix_ms),
            )
        })
}

fn select_last_runtime_miss_executed_candidate(
    candidates: &[RuntimeMissCandidateContext],
) -> Option<&RuntimeMissCandidateContext> {
    candidates
        .iter()
        .filter(|candidate| request_candidate_represents_provider_execution(&candidate.candidate))
        .max_by_key(|candidate| {
            (
                candidate.candidate.retry_index,
                candidate.candidate.candidate_index,
                candidate
                    .candidate
                    .finished_at_unix_ms
                    .or(candidate.candidate.started_at_unix_ms)
                    .unwrap_or(candidate.candidate.created_at_unix_ms),
            )
        })
}

fn request_candidate_represents_provider_execution(candidate: &StoredRequestCandidate) -> bool {
    matches!(
        candidate.status,
        RequestCandidateStatus::Pending
            | RequestCandidateStatus::Streaming
            | RequestCandidateStatus::Success
            | RequestCandidateStatus::Failed
            | RequestCandidateStatus::Cancelled
    )
}

fn error_category_for_failed_status(status_code: u16) -> Option<String> {
    if status_code >= 500 {
        Some("server_error".to_string())
    } else if status_code >= 400 {
        Some("client_error".to_string())
    } else {
        None
    }
}

fn json_header_map() -> Value {
    Value::Object(Map::from_iter([(
        "content-type".to_string(),
        Value::String("application/json".to_string()),
    )]))
}

fn runtime_miss_original_headers_json(headers: &HeaderMap) -> Value {
    let mut headers = crate::headers::collect_control_headers(headers);
    for (name, value) in headers.iter_mut() {
        if runtime_miss_sensitive_header(name) {
            *value = runtime_miss_mask_header_value(value);
        }
    }
    serde_json::to_value(headers).unwrap_or_else(|_| json!({}))
}

fn runtime_miss_original_request_body_json(
    headers: &HeaderMap,
    body: Option<&Bytes>,
) -> Option<Value> {
    let body = body?;
    if crate::headers::is_json_request(headers) {
        if body.is_empty() {
            return Some(json!({}));
        }
        return serde_json::from_slice::<Value>(body.as_ref()).ok();
    }

    (!body.is_empty()).then(|| {
        json!({
            "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(body.as_ref())
        })
    })
}

fn runtime_miss_sensitive_header(name: &str) -> bool {
    const SENSITIVE_HEADERS: &[&str] = &[
        "authorization",
        "x-api-key",
        "api-key",
        "x-goog-api-key",
        "cookie",
        "proxy-authorization",
    ];

    SENSITIVE_HEADERS
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn runtime_miss_mask_header_value(value: &str) -> String {
    let value = value.trim();
    let char_count = value.chars().count();
    if char_count <= 8 {
        return "****".to_string();
    }

    let prefix: String = value.chars().take(4).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}****{suffix}")
}

async fn load_runtime_miss_candidate_contexts(
    state: &AppState,
    request_id: &str,
    decision: Option<&GatewayControlDecision>,
) -> Vec<RuntimeMissCandidateContext> {
    let mut candidates = match state
        .read_request_candidates_by_request_id(request_id)
        .await
    {
        Ok(candidates) => candidates,
        Err(err) => {
            warn!(
                request_id = %request_id,
                error = ?err,
                "gateway failed to load request candidates for local execution runtime miss"
            );
            return Vec::new();
        }
    };
    if candidates.is_empty() {
        return Vec::new();
    }

    candidates.sort_by_key(|candidate| {
        (
            candidate.candidate_index,
            candidate.retry_index,
            candidate.created_at_unix_ms,
        )
    });

    let (providers_by_id, endpoints_by_id, keys_by_id) = if state.has_provider_catalog_data_reader()
    {
        let provider_ids = collect_present_ids(
            candidates
                .iter()
                .filter_map(|value| value.provider_id.as_deref()),
        );
        let endpoint_ids = collect_present_ids(
            candidates
                .iter()
                .filter_map(|value| value.endpoint_id.as_deref()),
        );
        let key_ids = collect_present_ids(
            candidates
                .iter()
                .filter_map(|value| value.key_id.as_deref()),
        );
        let (providers_result, endpoints_result, keys_result) = tokio::join!(
            state.read_provider_catalog_providers_by_ids(&provider_ids),
            state.read_provider_catalog_endpoints_by_ids(&endpoint_ids),
            state.read_provider_catalog_keys_by_ids(&key_ids),
        );
        (
            match providers_result {
                Ok(values) => values
                    .into_iter()
                    .map(|value| (value.id.clone(), value))
                    .collect::<BTreeMap<_, _>>(),
                Err(err) => {
                    warn!(
                        request_id = %request_id,
                        error = ?err,
                        "gateway failed to load provider catalog providers for local execution runtime miss"
                    );
                    BTreeMap::new()
                }
            },
            match endpoints_result {
                Ok(values) => values
                    .into_iter()
                    .map(|value| (value.id.clone(), value))
                    .collect::<BTreeMap<_, _>>(),
                Err(err) => {
                    warn!(
                        request_id = %request_id,
                        error = ?err,
                        "gateway failed to load provider catalog endpoints for local execution runtime miss"
                    );
                    BTreeMap::new()
                }
            },
            match keys_result {
                Ok(values) => values
                    .into_iter()
                    .map(|value| (value.id.clone(), value))
                    .collect::<BTreeMap<_, _>>(),
                Err(err) => {
                    warn!(
                        request_id = %request_id,
                        error = ?err,
                        "gateway failed to load provider catalog keys for local execution runtime miss"
                    );
                    BTreeMap::new()
                }
            },
        )
    } else {
        (BTreeMap::new(), BTreeMap::new(), BTreeMap::new())
    };

    candidates
        .into_iter()
        .map(|candidate| {
            let provider = candidate
                .provider_id
                .as_deref()
                .and_then(|value| providers_by_id.get(value));
            let endpoint = candidate
                .endpoint_id
                .as_deref()
                .and_then(|value| endpoints_by_id.get(value));
            let key = candidate
                .key_id
                .as_deref()
                .and_then(|value| keys_by_id.get(value));
            RuntimeMissCandidateContext {
                provider_name: candidate_extra_data_string(&candidate, "provider_name")
                    .or_else(|| provider.map(|value| value.name.clone())),
                key_name: candidate_extra_data_string(&candidate, "key_name")
                    .or_else(|| key.map(|value| value.name.clone())),
                client_api_format: candidate_extra_data_string(&candidate, "client_api_format")
                    .or_else(|| candidate_extra_data_string(&candidate, "client_contract")),
                provider_api_format: candidate_extra_data_string(&candidate, "provider_api_format")
                    .or_else(|| candidate_extra_data_string(&candidate, "provider_contract"))
                    .or_else(|| endpoint.map(|value| value.api_format.clone())),
                global_model_name: candidate_extra_data_string(&candidate, "global_model_name"),
                selected_provider_model_name: candidate_extra_data_string(
                    &candidate,
                    "selected_provider_model_name",
                ),
                endpoint_url: endpoint.and_then(|value| {
                    build_runtime_miss_candidate_endpoint_url(&candidate, value, decision)
                }),
                candidate,
            }
        })
        .collect()
}

async fn load_runtime_miss_candidate_contexts_with_retry(
    state: &AppState,
    request_id: &str,
    decision: Option<&GatewayControlDecision>,
) -> Vec<RuntimeMissCandidateContext> {
    let mut contexts = load_runtime_miss_candidate_contexts(state, request_id, decision).await;
    if !contexts.is_empty() {
        return contexts;
    }

    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        contexts = load_runtime_miss_candidate_contexts(state, request_id, decision).await;
        if !contexts.is_empty() {
            break;
        }
    }

    contexts
}

fn collect_present_ids<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<String> {
    ids.filter_map(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_string())
    })
    .collect::<BTreeSet<_>>()
    .into_iter()
    .collect()
}

fn candidate_extra_data_string(candidate: &StoredRequestCandidate, key: &str) -> Option<String> {
    candidate
        .extra_data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn build_runtime_miss_candidate_endpoint_url(
    candidate: &StoredRequestCandidate,
    endpoint: &StoredProviderCatalogEndpoint,
    decision: Option<&GatewayControlDecision>,
) -> Option<String> {
    if let Some(upstream_url) = candidate_extra_data_string(candidate, "upstream_url") {
        return Some(upstream_url);
    }

    let path = endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            decision
                .map(|value| value.public_path.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    let query = decision
        .and_then(|value| value.public_query_string.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());

    path.and_then(|value| {
        crate::provider_transport::url::build_passthrough_path_url(
            &endpoint.base_url,
            value,
            query,
            &[],
        )
    })
    .or_else(|| trimmed_non_empty(Some(endpoint.base_url.as_str())))
}

fn format_runtime_miss_candidate_summary(candidate: &RuntimeMissCandidateContext) -> String {
    let mut parts = Vec::new();
    parts.push(format!("idx={}", candidate.candidate.candidate_index));
    parts.push(format!("retry={}", candidate.candidate.retry_index));
    parts.push(format!(
        "status={}",
        request_candidate_status_label(candidate.candidate.status)
    ));
    if let Some(provider_label) = format_name_with_id(
        candidate.provider_name.as_deref(),
        candidate.candidate.provider_id.as_deref(),
    ) {
        parts.push(format!("provider={provider_label}"));
    }
    if let Some(endpoint_id) = candidate
        .candidate
        .endpoint_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("endpoint={endpoint_id}"));
    }
    if let Some(endpoint_url) = candidate
        .endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("url={endpoint_url}"));
    }
    if let Some(key_label) = format_name_with_id(
        candidate.key_name.as_deref(),
        candidate.candidate.key_id.as_deref(),
    ) {
        parts.push(format!("key={key_label}"));
    }
    if let Some(skip_reason) = candidate
        .candidate
        .skip_reason
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("skip={skip_reason}"));
    }
    if let Some(status_code) = candidate.candidate.status_code {
        parts.push(format!("code={status_code}"));
    }
    if let Some(error_type) = candidate
        .candidate
        .error_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("error_type={error_type}"));
    }
    parts.join(" ")
}

fn format_name_with_id(name: Option<&str>, id: Option<&str>) -> Option<String> {
    let name = name.map(str::trim).filter(|value| !value.is_empty());
    let id = id.map(str::trim).filter(|value| !value.is_empty());

    match (name, id) {
        (Some(name), Some(id)) => Some(format!("{name}({id})")),
        (Some(name), None) => Some(name.to_string()),
        (None, Some(id)) => Some(id.to_string()),
        (None, None) => None,
    }
}

fn request_candidate_status_label(status: RequestCandidateStatus) -> &'static str {
    match status {
        RequestCandidateStatus::Available => "available",
        RequestCandidateStatus::Unused => "unused",
        RequestCandidateStatus::Pending => "pending",
        RequestCandidateStatus::Streaming => "streaming",
        RequestCandidateStatus::Success => "success",
        RequestCandidateStatus::Failed => "failed",
        RequestCandidateStatus::Cancelled => "cancelled",
        RequestCandidateStatus::Skipped => "skipped",
    }
}

fn infer_request_type(api_format: Option<&str>) -> String {
    match infer_endpoint_kind(api_format.unwrap_or_default()) {
        Some("video") => "video".to_string(),
        Some("image") => "image".to_string(),
        _ => "chat".to_string(),
    }
}

fn infer_api_family(api_format: &str) -> Option<&str> {
    api_format.split_once(':').map(|(family, _)| family)
}

fn infer_endpoint_kind(api_format: &str) -> Option<&str> {
    api_format.split_once(':').map(|(_, kind)| kind)
}

fn apply_runtime_miss_usage_routing(
    data: &mut UsageEventData,
    request_metadata: &mut Map<String, Value>,
    execution_path: &str,
    candidate_id: Option<&str>,
    candidate_index: Option<u32>,
    key_name: Option<&str>,
    diagnostic: Option<&LocalExecutionRuntimeMissDiagnostic>,
    route_family_fallback: Option<&str>,
    route_kind_fallback: Option<&str>,
) {
    data.candidate_id = data
        .candidate_id
        .clone()
        .or_else(|| trimmed_non_empty(candidate_id));
    data.candidate_index = data
        .candidate_index
        .or_else(|| candidate_index.map(u64::from));
    data.key_name = data
        .key_name
        .clone()
        .or_else(|| trimmed_non_empty(key_name));
    data.execution_path = data
        .execution_path
        .clone()
        .or_else(|| trimmed_non_empty(Some(execution_path)));
    data.local_execution_runtime_miss_reason = data
        .local_execution_runtime_miss_reason
        .clone()
        .or_else(|| trimmed_non_empty(diagnostic.map(|value| value.reason.as_str())));
    data.route_family = data.route_family.clone().or_else(|| {
        trimmed_non_empty(
            diagnostic
                .and_then(|value| value.route_family.as_deref())
                .or(route_family_fallback),
        )
    });
    data.route_kind = data.route_kind.clone().or_else(|| {
        trimmed_non_empty(
            diagnostic
                .and_then(|value| value.route_kind.as_deref())
                .or(route_kind_fallback),
        )
    });
    data.planner_kind = data
        .planner_kind
        .clone()
        .or_else(|| trimmed_non_empty(diagnostic.and_then(|value| value.plan_kind.as_deref())));
    let _ = request_metadata;
}

fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_runtime_miss_usage_routing, beautify_local_execution_client_error_message,
        request_candidate_represents_provider_execution,
        select_last_runtime_miss_executed_candidate, RuntimeMissCandidateContext,
    };
    use crate::constants::EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS;
    use crate::state::LocalExecutionRuntimeMissDiagnostic;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_usage_runtime::UsageEventData;
    use serde_json::{json, Map, Value};

    #[test]
    fn local_execution_client_error_message_is_client_friendly() {
        assert_eq!(
            beautify_local_execution_client_error_message(
                "没有可用提供商支持模型 gpt-5.4 的同步请求。请检查模型映射、端点启用状态和 API Key 权限（原因代码: candidate_list_empty）",
            ),
            "没有可用提供商支持模型 gpt-5.4 的同步请求"
        );
        assert_eq!(
            beautify_local_execution_client_error_message(
                "请求缺少 model 字段，无法选择上游提供商（openai/chat，原因代码: missing_requested_model）",
            ),
            "请求缺少 model 字段，无法选择上游提供商"
        );
        assert_eq!(
            beautify_local_execution_client_error_message(
                "找到 1 个支持模型 gpt-5.4 的候选提供商，但本次流式请求全部不可用：provider_quota_blocked 2 次（原因代码: all_candidates_skipped）",
            ),
            "没有可用提供商支持模型 gpt-5.4 的流式请求"
        );
    }

    #[test]
    fn runtime_miss_routing_moves_to_typed_usage_fields_and_keeps_metadata_lightweight() {
        let mut data = UsageEventData::default();
        let mut request_metadata =
            Map::from_iter([("trace_id".to_string(), Value::String("trace-1".to_string()))]);

        apply_runtime_miss_usage_routing(
            &mut data,
            &mut request_metadata,
            EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS,
            Some("cand-1"),
            Some(2),
            Some("primary"),
            Some(&LocalExecutionRuntimeMissDiagnostic {
                reason: "all_candidates_skipped".to_string(),
                route_family: Some("claude".to_string()),
                route_kind: Some("cli".to_string()),
                plan_kind: Some("claude_cli_sync".to_string()),
                ..LocalExecutionRuntimeMissDiagnostic::default()
            }),
            None,
            None,
        );

        assert_eq!(data.candidate_id.as_deref(), Some("cand-1"));
        assert_eq!(data.candidate_index, Some(2));
        assert_eq!(data.key_name.as_deref(), Some("primary"));
        assert_eq!(data.planner_kind.as_deref(), Some("claude_cli_sync"));
        assert_eq!(data.route_family.as_deref(), Some("claude"));
        assert_eq!(data.route_kind.as_deref(), Some("cli"));
        assert_eq!(
            data.execution_path.as_deref(),
            Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS)
        );
        assert_eq!(
            data.local_execution_runtime_miss_reason.as_deref(),
            Some("all_candidates_skipped")
        );
        assert_eq!(
            Value::Object(request_metadata),
            json!({
                "trace_id": "trace-1"
            })
        );
    }

    #[test]
    fn runtime_miss_executed_candidate_selection_ignores_skipped_only_histories() {
        let skipped_candidate = StoredRequestCandidate::new(
            "cand-skipped".to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            RequestCandidateStatus::Skipped,
            Some("api_key_concurrency_limit_reached".to_string()),
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            100,
            None,
            None,
        )
        .expect("candidate should build");

        assert!(!request_candidate_represents_provider_execution(
            &skipped_candidate
        ));

        let contexts = vec![RuntimeMissCandidateContext {
            candidate: skipped_candidate,
            provider_name: Some("openai".to_string()),
            key_name: Some("prod".to_string()),
            client_api_format: Some("openai:responses".to_string()),
            provider_api_format: Some("openai:responses".to_string()),
            global_model_name: Some("gpt-5".to_string()),
            selected_provider_model_name: Some("gpt-5-upstream".to_string()),
            endpoint_url: Some("https://api.openai.example/v1/responses".to_string()),
        }];

        assert!(select_last_runtime_miss_executed_candidate(&contexts).is_none());
    }
}
