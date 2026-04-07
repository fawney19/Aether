use aether_contracts::{ExecutionError, ExecutionPlan, ExecutionTelemetry};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::SchedulerRequestCandidateStatusUpdate;
use axum::body::Body;
use axum::http::Response;
use base64::Engine as _;
use serde_json::{Map, Value};
use tracing::warn;

use crate::api::response::attach_control_metadata_headers;
use crate::clock::current_unix_secs as current_request_candidate_unix_secs;
use crate::control::GatewayControlDecision;
use crate::execution_runtime::submission::{
    resolve_core_error_background_report_kind, submit_local_core_error_or_sync_finalize,
};
use crate::log_ids::short_request_id;
use crate::request_candidate_runtime::record_report_request_candidate_status;
use crate::usage::submit_sync_report;
use crate::{usage::GatewaySyncReportRequest, AppState, GatewayError};

#[derive(Debug, Clone)]
pub(super) struct StreamFailureReport {
    pub(super) status_code: u16,
    pub(super) error_type: String,
    pub(super) error_message: String,
    pub(super) body_json: Value,
}

pub(super) fn build_stream_failure_report(
    error_type: impl Into<String>,
    error_message: impl Into<String>,
    status_code: u16,
) -> StreamFailureReport {
    let error_type = error_type.into();
    let error_message = error_message.into();
    StreamFailureReport {
        status_code,
        body_json: Value::Object(Map::from_iter([(
            "error".to_string(),
            Value::Object(Map::from_iter([
                ("type".to_string(), Value::String(error_type.clone())),
                ("message".to_string(), Value::String(error_message.clone())),
                ("code".to_string(), Value::from(status_code)),
            ])),
        )])),
        error_type,
        error_message,
    }
}

pub(super) fn build_stream_failure_from_execution_error(
    error: &ExecutionError,
) -> StreamFailureReport {
    let status_code = error.upstream_status.unwrap_or(502);
    let error_type = serde_json::to_value(&error.kind)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "internal".to_string());
    let phase = serde_json::to_value(&error.phase).unwrap_or(Value::Null);
    let mut error_object = Map::from_iter([
        ("type".to_string(), Value::String(error_type.clone())),
        ("message".to_string(), Value::String(error.message.clone())),
        ("code".to_string(), Value::from(status_code)),
        ("phase".to_string(), phase),
        ("retryable".to_string(), Value::Bool(error.retryable)),
        (
            "failover_recommended".to_string(),
            Value::Bool(error.failover_recommended),
        ),
    ]);
    if let Some(upstream_status) = error.upstream_status {
        error_object.insert("upstream_status".to_string(), Value::from(upstream_status));
    }

    StreamFailureReport {
        status_code,
        error_type,
        error_message: error.message.trim().to_string(),
        body_json: Value::Object(Map::from_iter([(
            "error".to_string(),
            Value::Object(error_object),
        )])),
    }
}

fn build_stream_failure_sync_payload(
    trace_id: &str,
    report_kind: String,
    report_context: Option<Value>,
    headers: &std::collections::BTreeMap<String, String>,
    telemetry: Option<ExecutionTelemetry>,
    provider_buffered_body: &[u8],
    failure: &StreamFailureReport,
) -> GatewaySyncReportRequest {
    let mut response_headers = headers.clone();
    response_headers.remove("content-encoding");
    response_headers.remove("content-length");
    response_headers.insert("content-type".to_string(), "application/json".to_string());

    GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind,
        report_context,
        status_code: failure.status_code,
        headers: response_headers,
        body_json: Some(failure.body_json.clone()),
        client_body_json: None,
        body_base64: (!provider_buffered_body.is_empty())
            .then(|| base64::engine::general_purpose::STANDARD.encode(provider_buffered_body)),
        telemetry,
    }
}

async fn record_stream_sync_failure(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewaySyncReportRequest,
    failure: &StreamFailureReport,
    started_at_unix_secs: Option<u64>,
) {
    state
        .usage_runtime
        .record_sync_terminal(state.data.as_ref(), plan, report_context, payload)
        .await;
    let terminal_unix_secs = current_request_candidate_unix_secs();
    record_report_request_candidate_status(
        state,
        report_context,
        SchedulerRequestCandidateStatusUpdate {
            status: RequestCandidateStatus::Failed,
            status_code: Some(failure.status_code),
            error_type: Some(failure.error_type.clone()),
            error_message: Some(failure.error_message.clone()),
            latency_ms: payload
                .telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.elapsed_ms),
            started_at_unix_secs: started_at_unix_secs.or(Some(terminal_unix_secs)),
            finished_at_unix_secs: Some(terminal_unix_secs),
        },
    )
    .await;
}

#[allow(clippy::too_many_arguments)] // internal helper for prefetch error handling
pub(super) async fn handle_prefetch_stream_failure(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan: &ExecutionPlan,
    report_context: Option<Value>,
    request_id: &str,
    candidate_id: Option<&str>,
    report_kind: &str,
    headers: &std::collections::BTreeMap<String, String>,
    telemetry: Option<ExecutionTelemetry>,
    buffered_body: &[u8],
    failure: StreamFailureReport,
) -> Result<Option<Response<Body>>, GatewayError> {
    let payload = build_stream_failure_sync_payload(
        trace_id,
        report_kind.to_string(),
        report_context.clone(),
        headers,
        telemetry,
        buffered_body,
        &failure,
    );
    record_stream_sync_failure(
        state,
        plan,
        report_context.as_ref(),
        &payload,
        &failure,
        None,
    )
    .await;

    let response =
        submit_local_core_error_or_sync_finalize(state, trace_id, decision, payload).await?;
    Ok(Some(attach_control_metadata_headers(
        response,
        Some(request_id),
        candidate_id,
    )?))
}

pub(super) async fn submit_midstream_stream_failure(
    state: &AppState,
    trace_id: &str,
    plan: &ExecutionPlan,
    direct_stream_finalize_kind: Option<&str>,
    report_context: Option<&Value>,
    headers: &std::collections::BTreeMap<String, String>,
    telemetry: Option<ExecutionTelemetry>,
    buffered_body: &[u8],
    started_at_unix_secs: u64,
    failure: StreamFailureReport,
) {
    let Some(report_kind) =
        direct_stream_finalize_kind.and_then(resolve_core_error_background_report_kind)
    else {
        return;
    };

    let payload = build_stream_failure_sync_payload(
        trace_id,
        report_kind,
        report_context.cloned(),
        headers,
        telemetry,
        buffered_body,
        &failure,
    );
    record_stream_sync_failure(
        state,
        plan,
        report_context,
        &payload,
        &failure,
        Some(started_at_unix_secs),
    )
    .await;
    if let Err(err) = submit_sync_report(state, trace_id, payload).await {
        let request_id = short_request_id(plan.request_id.as_str());
        warn!(
            event_name = "execution_report_submit_failed",
            log_type = "ops",
            trace_id = %trace_id,
            request_id = %request_id,
            candidate_id = ?plan.candidate_id,
            report_scope = "stream_failure",
            error = ?err,
            "gateway failed to submit sync execution report for terminal stream failure"
        );
    }
}
