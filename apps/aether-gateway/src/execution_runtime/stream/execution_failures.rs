use aether_contracts::{ExecutionError, ExecutionPlan, ExecutionTelemetry};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::SchedulerRequestCandidateStatusUpdate;
use aether_usage_runtime::{
    build_sync_terminal_usage_payload_seed, build_terminal_usage_context_seed,
};
use axum::body::Body;
use axum::http::Response;
use base64::Engine as _;
use serde::Serialize;
use serde_json::{Map, Value};
use tracing::warn;

use crate::api::response::attach_control_metadata_headers;
use crate::clock::current_unix_ms as current_request_candidate_unix_ms;
use crate::control::GatewayControlDecision;
use crate::execution_runtime::submission::{
    resolve_core_error_background_report_kind, submit_local_core_error_or_sync_finalize,
};
use crate::log_ids::short_request_id;
use crate::orchestration::{
    apply_local_execution_effect, resolve_local_failover_analysis_for_attempt,
    trace_upstream_response_body, with_upstream_response_report_context,
    LocalAdaptiveRateLimitEffect, LocalAttemptFailureEffect, LocalExecutionEffect,
    LocalExecutionEffectContext, LocalHealthFailureEffect, LocalOAuthInvalidationEffect,
    LocalPoolErrorEffect,
};
use crate::request_candidate_runtime::record_report_request_candidate_status;
use crate::usage::submit_sync_report;
use crate::{usage::GatewaySyncReportRequest, AppState, GatewayError};

#[derive(Debug, Clone)]
pub(super) struct StreamFailureReport {
    pub(super) status_code: u16,
    pub(super) error_type: String,
    pub(super) error_message: String,
    extra_error_fields: Map<String, Value>,
}

#[derive(Serialize)]
struct StreamFailureBody<'a> {
    error: StreamFailureBodyFields<'a>,
}

#[derive(Serialize)]
struct StreamFailureBodyFields<'a> {
    #[serde(rename = "type")]
    error_type: &'a str,
    message: &'a str,
    code: u16,
    #[serde(flatten)]
    extra_error_fields: &'a Map<String, Value>,
}

impl StreamFailureReport {
    fn into_body_json(self) -> Value {
        let Self {
            status_code,
            error_type,
            error_message,
            mut extra_error_fields,
        } = self;
        extra_error_fields.insert("type".to_string(), Value::String(error_type));
        extra_error_fields.insert("message".to_string(), Value::String(error_message));
        extra_error_fields.insert("code".to_string(), Value::from(status_code));
        Value::Object(Map::from_iter([(
            "error".to_string(),
            Value::Object(extra_error_fields),
        )]))
    }

    pub(super) fn to_json_string(&self) -> serde_json::Result<String> {
        serde_json::to_string(&StreamFailureBody {
            error: StreamFailureBodyFields {
                error_type: self.error_type.as_str(),
                message: self.error_message.as_str(),
                code: self.status_code,
                extra_error_fields: &self.extra_error_fields,
            },
        })
    }
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
        error_type,
        error_message,
        extra_error_fields: Map::new(),
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
    let error_message = error.message.trim().to_string();
    let phase = serde_json::to_value(&error.phase).unwrap_or(Value::Null);
    let mut error_object = Map::from_iter([
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
        error_message,
        extra_error_fields: error_object,
    }
}

pub(super) fn build_stream_failure_from_provider_error_body(
    status_code: u16,
    body_json: &Value,
) -> StreamFailureReport {
    let body_object = body_json.as_object();
    let error_object = body_object
        .and_then(|object| object.get("error"))
        .and_then(Value::as_object);
    let error_type =
        first_non_empty_error_text(error_object, body_object, &["type", "code", "status"])
            .unwrap_or_else(|| "upstream_error".to_string());
    let error_message = first_non_empty_error_text(
        error_object,
        body_object,
        &["message", "detail", "reason", "status", "type", "code"],
    )
    .unwrap_or_else(|| format!("upstream stream returned error status {status_code}"));

    StreamFailureReport {
        status_code,
        error_type,
        error_message,
        extra_error_fields: Map::new(),
    }
}

fn first_non_empty_error_text(
    error_object: Option<&Map<String, Value>>,
    body_object: Option<&Map<String, Value>>,
    keys: &[&str],
) -> Option<String> {
    for object in [error_object, body_object].into_iter().flatten() {
        for key in keys {
            let Some(value) = object.get(*key) else {
                continue;
            };
            match value {
                Value::String(text) if !text.trim().is_empty() => {
                    return Some(text.trim().to_string());
                }
                Value::Number(number) => return Some(number.to_string()),
                _ => {}
            }
        }
    }
    None
}

fn build_stream_failure_sync_payload(
    trace_id: &str,
    report_kind: String,
    report_context: Option<Value>,
    mut headers: std::collections::BTreeMap<String, String>,
    telemetry: Option<ExecutionTelemetry>,
    provider_buffered_body: &[u8],
    failure: StreamFailureReport,
) -> GatewaySyncReportRequest {
    let status_code = failure.status_code;
    let body = trace_upstream_response_body(None, provider_buffered_body);
    let report_context = with_upstream_response_report_context(
        report_context.as_ref(),
        status_code,
        Some(&headers),
        body.as_ref(),
        None,
        None,
    )
    .or(report_context);
    headers.remove("content-encoding");
    headers.remove("content-length");
    headers.insert("content-type".to_string(), "application/json".to_string());

    GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind,
        report_context,
        status_code,
        headers,
        body_json: Some(failure.into_body_json()),
        client_body_json: None,
        body_base64: (!provider_buffered_body.is_empty())
            .then(|| base64::engine::general_purpose::STANDARD.encode(provider_buffered_body)),
        telemetry,
    }
}

fn stream_failure_body_field<'a>(
    payload: &'a GatewaySyncReportRequest,
    field: &str,
) -> Option<&'a str> {
    payload
        .body_json
        .as_ref()
        .and_then(|body_json| body_json.get("error"))
        .and_then(|value| value.get(field))
        .and_then(Value::as_str)
}

async fn record_stream_sync_failure(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    payload: &GatewaySyncReportRequest,
    started_at_unix_ms: Option<u64>,
) {
    let error_type = stream_failure_body_field(payload, "type").unwrap_or("internal");
    let error_message = stream_failure_body_field(payload, "message").unwrap_or_default();
    let error_body = payload
        .body_json
        .as_ref()
        .and_then(|body_json| serde_json::to_string(body_json).ok());
    let failure_analysis = resolve_local_failover_analysis_for_attempt(
        state,
        plan,
        report_context,
        payload.status_code,
        error_body.as_deref(),
    )
    .await;
    if matches!(error_type, "first_byte_timeout" | "read_timeout") {
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan,
                report_context,
            },
            LocalExecutionEffect::PoolStreamTimeout,
        )
        .await;
    }
    apply_local_execution_effect(
        state,
        LocalExecutionEffectContext {
            plan,
            report_context,
        },
        LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
            status_code: payload.status_code,
            classification: failure_analysis.classification,
        }),
    )
    .await;
    apply_local_execution_effect(
        state,
        LocalExecutionEffectContext {
            plan,
            report_context,
        },
        LocalExecutionEffect::AdaptiveRateLimit(LocalAdaptiveRateLimitEffect {
            status_code: payload.status_code,
            classification: failure_analysis.classification,
            headers: Some(&payload.headers),
        }),
    )
    .await;
    apply_local_execution_effect(
        state,
        LocalExecutionEffectContext {
            plan,
            report_context,
        },
        LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
            status_code: payload.status_code,
            classification: failure_analysis.classification,
        }),
    )
    .await;
    apply_local_execution_effect(
        state,
        LocalExecutionEffectContext {
            plan,
            report_context,
        },
        LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
            status_code: payload.status_code,
            response_text: error_body.as_deref(),
        }),
    )
    .await;
    apply_local_execution_effect(
        state,
        LocalExecutionEffectContext {
            plan,
            report_context,
        },
        LocalExecutionEffect::PoolError(LocalPoolErrorEffect {
            status_code: payload.status_code,
            classification: failure_analysis.classification,
            headers: &payload.headers,
            error_body: error_body.as_deref(),
        }),
    )
    .await;
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_sync_terminal_usage_payload_seed(payload);
    state
        .usage_runtime
        .record_sync_terminal(state.data.as_ref(), context_seed, payload_seed);
    let terminal_unix_secs = current_request_candidate_unix_ms();
    record_report_request_candidate_status(
        state,
        report_context,
        SchedulerRequestCandidateStatusUpdate {
            status: RequestCandidateStatus::Failed,
            status_code: Some(payload.status_code),
            error_type: Some(error_type.to_string()),
            error_message: Some(error_message.to_string()),
            latency_ms: payload
                .telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.elapsed_ms),
            started_at_unix_ms: started_at_unix_ms.or(Some(terminal_unix_secs)),
            finished_at_unix_ms: Some(terminal_unix_secs),
        },
    )
    .await;
}

#[allow(clippy::too_many_arguments)] // internal helper for prefetch error handling
pub(super) async fn handle_prefetch_provider_private_stream_error(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan: &ExecutionPlan,
    report_context: Option<Value>,
    request_id: &str,
    candidate_id: Option<&str>,
    report_kind: &str,
    mut headers: std::collections::BTreeMap<String, String>,
    telemetry: Option<ExecutionTelemetry>,
    buffered_body: &[u8],
    status_code: u16,
    body_json: Value,
) -> Result<Option<Response<Body>>, GatewayError> {
    headers.remove("content-encoding");
    headers.remove("content-length");
    headers.insert("content-type".to_string(), "application/json".to_string());

    let payload = GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind: report_kind.to_string(),
        report_context,
        status_code,
        headers,
        body_json: Some(body_json),
        client_body_json: None,
        body_base64: (!buffered_body.is_empty())
            .then(|| base64::engine::general_purpose::STANDARD.encode(buffered_body)),
        telemetry,
    };
    record_stream_sync_failure(state, plan, payload.report_context.as_ref(), &payload, None).await;

    let response =
        submit_local_core_error_or_sync_finalize(state, trace_id, decision, payload).await?;
    Ok(Some(attach_control_metadata_headers(
        response,
        Some(request_id),
        candidate_id,
    )?))
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
    headers: std::collections::BTreeMap<String, String>,
    telemetry: Option<ExecutionTelemetry>,
    buffered_body: &[u8],
    failure: StreamFailureReport,
) -> Result<Option<Response<Body>>, GatewayError> {
    let payload = build_stream_failure_sync_payload(
        trace_id,
        report_kind.to_string(),
        report_context,
        headers,
        telemetry,
        buffered_body,
        failure,
    );
    record_stream_sync_failure(state, plan, payload.report_context.as_ref(), &payload, None).await;

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
    report_context: Option<Value>,
    headers: std::collections::BTreeMap<String, String>,
    telemetry: Option<ExecutionTelemetry>,
    buffered_body: &[u8],
    started_at_unix_ms: u64,
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
        report_context,
        headers,
        telemetry,
        buffered_body,
        failure,
    );
    record_stream_sync_failure(
        state,
        plan,
        payload.report_context.as_ref(),
        &payload,
        Some(started_at_unix_ms),
    )
    .await;
    if let Err(err) = submit_sync_report(state, payload).await {
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
