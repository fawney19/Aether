use std::time::Instant;

use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_usage_runtime::{
    build_usage_event_data_seed, UsageEvent, UsageEventData, UsageEventType,
};
use axum::body::Body;
use axum::http::{self, Response};
use serde_json::{json, Map, Value};
use tracing::warn;

use crate::constants::LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER;
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

impl LocalExecutionRequestOutcome {
    pub(crate) fn responded(response: Response<Body>) -> Self {
        Self::Responded(response)
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

pub(crate) async fn record_failed_usage_for_exhausted_request(
    state: &AppState,
    exhaustion: LocalExecutionExhaustion,
    started_at: &Instant,
    local_execution_runtime_miss_detail: &str,
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
            "message": local_execution_runtime_miss_detail,
        }
    }));

    let mut request_metadata = match data.request_metadata.take() {
        Some(Value::Object(object)) => object,
        Some(other) => Map::from_iter([("seed".to_string(), other)]),
        None => Map::new(),
    };
    request_metadata.insert("trace_id".to_string(), Value::String(request_id.clone()));
    if let Some(candidate_id) = candidate_id {
        request_metadata.insert("candidate_id".to_string(), Value::String(candidate_id));
    }
    if let Some(candidate_index) = candidate_index {
        request_metadata.insert(
            "candidate_index".to_string(),
            Value::Number(candidate_index.into()),
        );
    }
    data.request_metadata = Some(Value::Object(request_metadata));

    state
        .usage_runtime
        .record_terminal_event(
            state.data.as_ref(),
            UsageEvent::new(UsageEventType::Failed, request_id, data),
        )
        .await;
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
