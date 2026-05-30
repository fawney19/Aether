use std::collections::BTreeMap;
use std::io::Error as IoError;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_ai_serving::UPSTREAM_IS_STREAM_KEY;
use aether_contracts::{
    ExecutionError, ExecutionErrorKind, ExecutionPhase, ExecutionPlan, ExecutionResult,
    ExecutionTelemetry,
};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::{
    execution_error_details, parse_request_candidate_report_context,
    SchedulerRequestCandidateStatusUpdate,
};
use aether_usage_runtime::{
    build_lifecycle_usage_seed, build_sync_terminal_usage_payload_seed,
    build_terminal_usage_context_seed, build_usage_event_data_seed, UsageEvent, UsageEventType,
};
use async_stream::stream;
use axum::body::{to_bytes, Body, Bytes};
use axum::http::header::{CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderName, HeaderValue, Response, StatusCode};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};

use crate::ai_serving::api::{
    build_core_error_body_for_client_format, extract_provider_private_stream_error_body,
    implicit_sync_finalize_report_kind, maybe_build_sync_finalize_outcome, LocalCoreSyncErrorKind,
    LocalCoreSyncFinalizeOutcome,
};
use crate::api::response::{
    attach_control_metadata_headers, build_client_response, build_client_response_from_parts,
    build_client_response_from_parts_with_mutator,
};
use crate::clock::current_unix_ms as current_request_candidate_unix_ms;
use crate::control::GatewayControlDecision;
use crate::execution_runtime::chatgpt_web_image::maybe_execute_chatgpt_web_image_sync;
use crate::execution_runtime::grok::maybe_execute_grok_sync;
use crate::execution_runtime::oauth_retry::refresh_oauth_plan_auth_for_retry;
#[cfg(test)]
use crate::execution_runtime::remote_compat::post_sync_plan_to_remote_execution_runtime;
use crate::execution_runtime::submission::{
    resolve_local_sync_error_status_code, submit_local_core_error_or_sync_finalize,
};
use crate::execution_runtime::transport::{
    build_execution_response_body, build_request_body, collect_response_headers,
    decode_response_body_bytes, format_upstream_request_error, format_wreq_upstream_request_error,
    response_body_is_json, send_request, DirectHttpResponse, DirectSyncExecutionRuntime,
    ExecutionRuntimeTransportError,
};
use crate::execution_runtime::windsurf::maybe_execute_windsurf_sync;
use crate::execution_runtime::{
    analyze_local_candidate_failover_sync, apply_endpoint_response_header_rules,
    attach_provider_response_headers_to_report_context, local_failover_response_text,
    resolve_core_sync_error_finalize_report_kind, should_fallback_to_control_sync,
    should_finalize_sync_response, LocalFailoverDecision,
};
use crate::log_ids::short_request_id;
use crate::orchestration::{
    apply_local_execution_effect, build_local_error_flow_metadata, trace_upstream_response_body,
    with_error_flow_report_context, with_upstream_response_report_context,
    LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect, LocalAttemptFailureEffect,
    LocalExecutionEffect, LocalExecutionEffectContext, LocalHealthFailureEffect,
    LocalHealthSuccessEffect, LocalOAuthInvalidationEffect, LocalPoolErrorEffect,
};
use crate::provider_pool_demand::acquire_provider_pool_in_flight_guard;
use crate::request_candidate_runtime::{
    ensure_execution_request_candidate_slot, record_local_request_candidate_extra_data,
    record_local_request_candidate_status,
};
use crate::usage::{spawn_sync_report, submit_sync_report};
use crate::video_tasks::VideoTaskSyncReportMode;
use crate::{usage::GatewaySyncReportRequest, AppState, GatewayError};

#[path = "execution/policy.rs"]
mod policy;
#[path = "execution/response.rs"]
mod response;

use policy::decode_execution_result_body;
pub(crate) use response::{
    maybe_build_local_sync_finalize_response, maybe_build_local_video_error_response,
    maybe_build_local_video_success_outcome, resolve_local_sync_error_background_report_kind,
    resolve_local_sync_success_background_report_kind, LocalVideoSyncSuccessBuild,
    LocalVideoSyncSuccessOutcome,
};

const OPENAI_IMAGE_SYNC_PLAN_KIND: &str = "openai_image_sync";
const OPENAI_IMAGE_SYNC_DEFAULT_TOTAL_TIMEOUT_MS: u64 = 900_000;
const SYNC_EXECUTION_IDLE_LOG_INTERVAL: Duration = Duration::from_secs(60);
const OPENAI_IMAGE_SYNC_JSON_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const OPENAI_IMAGE_SYNC_JSON_HEARTBEAT_BYTES: &[u8] = b"\n";
const OPENAI_IMAGE_SYNC_PROGRESS_WRITE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct SyncExecutionFailure {
    error_type: &'static str,
    message: String,
    status_code: Option<u16>,
    latency_ms: Option<u64>,
}

struct SyncAttemptTerminalGuard {
    state: AppState,
    plan: ExecutionPlan,
    report_context: Option<Value>,
    candidate_started_unix_ms: u64,
    armed: bool,
}

impl SyncAttemptTerminalGuard {
    fn new(
        state: &AppState,
        plan: &ExecutionPlan,
        report_context: Option<Value>,
        candidate_started_unix_ms: u64,
    ) -> Self {
        Self {
            state: state.clone(),
            plan: plan.clone(),
            report_context,
            candidate_started_unix_ms,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    async fn fail_and_disarm(&mut self, error: &GatewayError) {
        if !self.armed {
            return;
        }
        self.armed = false;
        record_sync_attempt_forced_terminal_state(
            self.state.clone(),
            self.plan.clone(),
            self.report_context.clone(),
            self.candidate_started_unix_ms,
            UsageEventType::Failed,
            RequestCandidateStatus::Failed,
            StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            "local_sync_attempt_aborted",
            format!("Local sync attempt failed before terminal finalization: {error:?}"),
        )
        .await;
    }
}

impl Drop for SyncAttemptTerminalGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let state = self.state.clone();
        let plan = self.plan.clone();
        let report_context = self.report_context.clone();
        let candidate_started_unix_ms = self.candidate_started_unix_ms;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                record_sync_attempt_forced_terminal_state(
                    state,
                    plan,
                    report_context,
                    candidate_started_unix_ms,
                    UsageEventType::Cancelled,
                    RequestCandidateStatus::Cancelled,
                    499,
                    "local_sync_attempt_cancelled",
                    "Local sync attempt was dropped before terminal finalization, usually because the client disconnected or the request task was cancelled.",
                )
                .await;
            });
        } else {
            warn!(
                event_name = "local_sync_attempt_terminal_guard_no_runtime",
                log_type = "ops",
                request_id = %short_request_id(self.plan.request_id.as_str()),
                candidate_id = ?self.plan.candidate_id,
                "gateway could not finalize dropped local sync attempt because no Tokio runtime is available"
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_sync_attempt_forced_terminal_state(
    state: AppState,
    plan: ExecutionPlan,
    report_context: Option<Value>,
    candidate_started_unix_ms: u64,
    usage_event_type: UsageEventType,
    candidate_status: RequestCandidateStatus,
    status_code: u16,
    error_type: &'static str,
    error_message: impl Into<String>,
) {
    let error_message = error_message.into();
    let terminal_unix_ms = current_request_candidate_unix_ms();
    let latency_ms = terminal_unix_ms.saturating_sub(candidate_started_unix_ms);
    record_local_request_candidate_status(
        &state,
        &plan,
        report_context.as_ref(),
        SchedulerRequestCandidateStatusUpdate {
            status: candidate_status,
            status_code: Some(status_code),
            error_type: Some(error_type.to_string()),
            error_message: Some(error_message.clone()),
            latency_ms: Some(latency_ms),
            started_at_unix_ms: Some(candidate_started_unix_ms),
            finished_at_unix_ms: Some(terminal_unix_ms),
        },
    )
    .await;

    if !state.usage_runtime.is_enabled() {
        return;
    }

    let mut usage_data = build_usage_event_data_seed(&plan, report_context.as_ref());
    usage_data.status_code = Some(status_code);
    usage_data.error_message = Some(error_message.clone());
    usage_data.error_category = Some(
        match usage_event_type {
            UsageEventType::Cancelled => "cancelled",
            _ => "server_error",
        }
        .to_string(),
    );
    usage_data.response_time_ms = Some(latency_ms);
    let error_body = json!({
        "error": {
            "type": error_type,
            "message": error_message,
            "code": status_code
        }
    });
    usage_data.response_headers = Some(json!({"content-type": "application/json"}));
    usage_data.response_body = Some(error_body.clone());
    usage_data.client_response_headers = Some(json!({"content-type": "application/json"}));
    usage_data.client_response_body = Some(error_body);

    state
        .usage_runtime
        .record_terminal_event_direct(
            state.data.as_ref(),
            UsageEvent::new(usage_event_type, plan.request_id.clone(), usage_data),
        )
        .await;
}

impl SyncExecutionFailure {
    fn from_transport(err: ExecutionRuntimeTransportError) -> Self {
        Self {
            error_type: "execution_runtime_unavailable",
            message: err.to_string(),
            status_code: None,
            latency_ms: None,
        }
    }

    fn image_sync_total_timeout(timeout_ms: u64, elapsed_ms: u64) -> Self {
        Self {
            error_type: "image_sync_total_timeout",
            message: format!(
                "OpenAI image sync execution exceeded total timeout of {timeout_ms}ms"
            ),
            status_code: Some(StatusCode::GATEWAY_TIMEOUT.as_u16()),
            latency_ms: Some(elapsed_ms),
        }
    }
}

struct ImplicitSyncFinalizeOutcome {
    payload: GatewaySyncReportRequest,
    outcome: LocalCoreSyncFinalizeOutcome,
}

fn record_sync_terminal_usage(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewaySyncReportRequest,
) {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_sync_terminal_usage_payload_seed(payload);
    state
        .usage_runtime
        .record_sync_terminal(state.data.as_ref(), context_seed, payload_seed);
}

fn record_sync_terminal_usage_and_disarm_guard(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewaySyncReportRequest,
    terminal_guard: &mut SyncAttemptTerminalGuard,
) {
    record_sync_terminal_usage(state, plan, report_context, payload);
    terminal_guard.disarm();
}

fn with_sync_error_trace_context(
    report_context: Option<&serde_json::Value>,
    status_code: u16,
    headers: &BTreeMap<String, String>,
    body_json: Option<&serde_json::Value>,
    body_bytes: &[u8],
    response_text: Option<&str>,
    local_failover_analysis: crate::orchestration::LocalFailoverAnalysis,
) -> Option<serde_json::Value> {
    let body = trace_upstream_response_body(body_json, body_bytes);
    let upstream_context = with_upstream_response_report_context(
        report_context,
        status_code,
        Some(headers),
        body.as_ref(),
        None,
        None,
    );
    with_error_flow_report_context(
        upstream_context.as_ref().or(report_context),
        build_local_error_flow_metadata(status_code, response_text, local_failover_analysis),
    )
}

fn build_sync_report_payload(
    trace_id: &str,
    report_kind: String,
    report_context: Option<serde_json::Value>,
    status_code: u16,
    headers: BTreeMap<String, String>,
    body_json: Option<serde_json::Value>,
    body_base64: Option<String>,
    telemetry: Option<ExecutionTelemetry>,
) -> GatewaySyncReportRequest {
    GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind,
        report_context,
        status_code,
        headers,
        body_json,
        client_body_json: None,
        body_base64,
        telemetry,
    }
}

fn invalid_gemini_provider_success_message(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    status_code: u16,
    body_json: Option<&Value>,
) -> Option<&'static str> {
    if status_code >= 400 {
        return None;
    }
    let provider_api_format = report_context
        .and_then(|value| value.get("provider_api_format"))
        .and_then(Value::as_str)
        .unwrap_or(plan.provider_api_format.as_str());
    if crate::ai_serving::normalize_api_format_alias(provider_api_format)
        != "gemini:generate_content"
    {
        return None;
    }
    let body_json = body_json?;
    if body_json
        .as_object()
        .is_some_and(|object| object.get("error").is_some_and(|error| !error.is_null()))
    {
        return None;
    }
    let normalized_body_json = report_context
        .filter(|context| {
            context
                .get("has_envelope")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .and_then(|context| {
            crate::ai_serving::normalize_provider_private_response_value(body_json.clone(), context)
        });
    let body_json = normalized_body_json.as_ref().unwrap_or(body_json);
    if crate::ai_serving::gemini_generate_content_response_has_visible_output(body_json) {
        return None;
    }
    Some("Provider returned HTTP 200 but the Gemini response did not contain visible model output; refusing to finalize it as a successful response.")
}

fn build_invalid_provider_success_body(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    message: &str,
) -> Option<Value> {
    let client_api_format = report_context
        .and_then(|value| value.get("client_api_format"))
        .and_then(Value::as_str)
        .unwrap_or(plan.client_api_format.as_str());
    build_core_error_body_for_client_format(
        client_api_format,
        message,
        Some("invalid_provider_success_response"),
        LocalCoreSyncErrorKind::ServerError,
    )
}

fn provider_private_error_details(body_json: &Value) -> (Option<String>, Option<String>) {
    let body_object = body_json.as_object();
    let error_object = body_object
        .and_then(|object| object.get("error"))
        .and_then(Value::as_object);
    let error_type =
        first_non_empty_error_text(error_object, body_object, &["type", "code", "status"]);
    let error_message = first_non_empty_error_text(
        error_object,
        body_object,
        &["message", "detail", "reason", "status", "type", "code"],
    );
    (error_type, error_message)
}

fn first_non_empty_error_text(
    error_object: Option<&serde_json::Map<String, Value>>,
    body_object: Option<&serde_json::Map<String, Value>>,
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

#[derive(Debug, Clone)]
struct OpenAiImageSyncProgressSnapshot {
    phase: &'static str,
    upstream_ttfb_ms: Option<u64>,
    upstream_sse_frame_count: u64,
    last_upstream_event: Option<String>,
    last_upstream_frame_at_unix_ms: Option<u64>,
    partial_image_count: u64,
    last_client_visible_event: Option<String>,
    downstream_heartbeat_count: u64,
    last_downstream_heartbeat_at_unix_ms: Option<u64>,
    downstream_heartbeat_interval_ms: Option<u64>,
}

struct OpenAiImageSyncProgressRecorder<'a> {
    state: &'a AppState,
    plan: &'a ExecutionPlan,
    report_context: Option<&'a Value>,
    snapshot: Arc<Mutex<OpenAiImageSyncProgressSnapshot>>,
    buffer: Vec<u8>,
    last_persist_at: Option<Instant>,
}

#[derive(Clone)]
struct OpenAiImageSyncJsonHeartbeatContext {
    state: AppState,
    plan: ExecutionPlan,
    report_context: Option<Value>,
    snapshot: Arc<Mutex<OpenAiImageSyncProgressSnapshot>>,
    started_at: Instant,
    trace_id: String,
    request_id_for_log: String,
    candidate_id: Option<String>,
}

#[derive(Debug)]
struct OpenAiImageSyncSseFrame {
    event_name: String,
    is_partial_image: bool,
    is_completed: bool,
    is_failed: bool,
    client_visible_event: Option<&'static str>,
}

impl OpenAiImageSyncProgressSnapshot {
    fn new() -> Self {
        Self {
            phase: "upstream_connecting",
            upstream_ttfb_ms: None,
            upstream_sse_frame_count: 0,
            last_upstream_event: None,
            last_upstream_frame_at_unix_ms: None,
            partial_image_count: 0,
            last_client_visible_event: None,
            downstream_heartbeat_count: 0,
            last_downstream_heartbeat_at_unix_ms: None,
            downstream_heartbeat_interval_ms: None,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "phase": self.phase,
            "upstream_ttfb_ms": self.upstream_ttfb_ms,
            "upstream_sse_frame_count": self.upstream_sse_frame_count,
            "last_upstream_event": self.last_upstream_event,
            "last_upstream_frame_at_unix_ms": self.last_upstream_frame_at_unix_ms,
            "partial_image_count": self.partial_image_count,
            "last_client_visible_event": self.last_client_visible_event,
            "downstream_heartbeat_count": self.downstream_heartbeat_count,
            "last_downstream_heartbeat_at_unix_ms": self.last_downstream_heartbeat_at_unix_ms,
            "downstream_heartbeat_interval_ms": self.downstream_heartbeat_interval_ms,
        })
    }
}

impl<'a> OpenAiImageSyncProgressRecorder<'a> {
    fn new(
        state: &'a AppState,
        plan: &'a ExecutionPlan,
        report_context: Option<&'a Value>,
        snapshot: Option<Arc<Mutex<OpenAiImageSyncProgressSnapshot>>>,
    ) -> Self {
        Self {
            state,
            plan,
            report_context,
            snapshot: snapshot
                .unwrap_or_else(|| Arc::new(Mutex::new(OpenAiImageSyncProgressSnapshot::new()))),
            buffer: Vec::new(),
            last_persist_at: None,
        }
    }

    async fn persist(
        &mut self,
        status: RequestCandidateStatus,
        status_code: Option<u16>,
        latency_ms: Option<u64>,
        force: bool,
    ) {
        let now = Instant::now();
        if !force
            && self.last_persist_at.is_some_and(|last| {
                now.duration_since(last) < OPENAI_IMAGE_SYNC_PROGRESS_WRITE_INTERVAL
            })
        {
            return;
        }
        let snapshot = self.snapshot.lock().await.clone();
        let extra_data = json!({
            "image_progress": snapshot.to_json(),
        });
        record_local_request_candidate_extra_data(
            self.state,
            self.plan,
            self.report_context,
            status,
            status_code,
            latency_ms,
            extra_data,
        )
        .await;
        self.last_persist_at = Some(now);
    }

    async fn record_connecting(&mut self) {
        self.snapshot.lock().await.phase = "upstream_connecting";
        self.persist(RequestCandidateStatus::Pending, None, None, true)
            .await;
    }

    async fn record_response_started(&mut self, status_code: u16, ttfb_ms: u64) {
        {
            let mut snapshot = self.snapshot.lock().await;
            snapshot.phase = if status_code >= 400 {
                "failed"
            } else {
                "upstream_streaming"
            };
            snapshot.upstream_ttfb_ms = Some(ttfb_ms);
        }
        self.persist(
            if status_code >= 400 {
                RequestCandidateStatus::Failed
            } else {
                RequestCandidateStatus::Streaming
            },
            Some(status_code),
            Some(ttfb_ms),
            true,
        )
        .await;
    }

    async fn observe_chunk(&mut self, chunk: &[u8], status_code: u16, elapsed_ms: u64) {
        if chunk.is_empty() {
            return;
        }
        self.buffer.extend_from_slice(chunk);
        let mut force_persist = false;
        while let Some(block_end) = find_sse_block_end(&self.buffer) {
            let block = self.buffer.drain(..block_end).collect::<Vec<_>>();
            let Some(frame) = parse_openai_image_sync_sse_frame(&block) else {
                continue;
            };
            {
                let mut snapshot = self.snapshot.lock().await;
                snapshot.upstream_sse_frame_count =
                    snapshot.upstream_sse_frame_count.saturating_add(1);
                snapshot.last_upstream_event = Some(frame.event_name);
                snapshot.last_upstream_frame_at_unix_ms = Some(current_request_candidate_unix_ms());
                if frame.is_partial_image {
                    snapshot.partial_image_count = snapshot.partial_image_count.saturating_add(1);
                }
                if let Some(client_visible_event) = frame.client_visible_event {
                    snapshot.last_client_visible_event = Some(client_visible_event.to_string());
                    force_persist = true;
                }
                if frame.is_failed || status_code >= 400 {
                    snapshot.phase = "failed";
                    force_persist = true;
                } else if frame.is_completed {
                    snapshot.phase = "upstream_completed";
                    force_persist = true;
                } else {
                    snapshot.phase = "upstream_streaming";
                }
            }
        }
        let phase = self.snapshot.lock().await.phase;
        self.persist(
            if phase == "failed" {
                RequestCandidateStatus::Failed
            } else {
                RequestCandidateStatus::Streaming
            },
            Some(status_code),
            Some(elapsed_ms),
            force_persist,
        )
        .await;
    }

    async fn finish(&mut self, status_code: u16, elapsed_ms: u64) {
        {
            let mut snapshot = self.snapshot.lock().await;
            if status_code >= 400 || snapshot.phase == "failed" {
                snapshot.phase = "failed";
            } else {
                snapshot.phase = "upstream_completed";
            }
        }
        self.persist(
            if status_code >= 400 {
                RequestCandidateStatus::Failed
            } else {
                RequestCandidateStatus::Streaming
            },
            Some(status_code),
            Some(elapsed_ms),
            true,
        )
        .await;
    }

    async fn fail(&mut self, status_code: Option<u16>, elapsed_ms: u64) {
        self.snapshot.lock().await.phase = "failed";
        self.persist(
            RequestCandidateStatus::Failed,
            status_code,
            Some(elapsed_ms),
            true,
        )
        .await;
    }
}

impl OpenAiImageSyncJsonHeartbeatContext {
    async fn record_heartbeat(&self, heartbeat_kind: &'static str, heartbeat_interval: Duration) {
        let now_unix_ms = current_request_candidate_unix_ms();
        let elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        let interval_ms = heartbeat_interval
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        let (count, phase, progress_json) = {
            let mut snapshot = self.snapshot.lock().await;
            snapshot.downstream_heartbeat_count =
                snapshot.downstream_heartbeat_count.saturating_add(1);
            snapshot.last_downstream_heartbeat_at_unix_ms = Some(now_unix_ms);
            snapshot.downstream_heartbeat_interval_ms = Some(interval_ms);
            (
                snapshot.downstream_heartbeat_count,
                snapshot.phase,
                snapshot.to_json(),
            )
        };
        let status = match phase {
            "failed" => RequestCandidateStatus::Failed,
            "upstream_connecting" => RequestCandidateStatus::Pending,
            _ => RequestCandidateStatus::Streaming,
        };
        record_local_request_candidate_extra_data(
            &self.state,
            &self.plan,
            self.report_context.as_ref(),
            status,
            None,
            Some(elapsed_ms),
            json!({ "image_progress": progress_json }),
        )
        .await;
        debug!(
            event_name = "openai_image_sync_json_heartbeat_sent",
            log_type = "event",
            trace_id = %self.trace_id,
            request_id = %self.request_id_for_log,
            candidate_id = self.candidate_id.as_deref().unwrap_or("-"),
            heartbeat_kind,
            heartbeat_count = count,
            heartbeat_interval_ms = interval_ms,
            elapsed_ms,
            phase,
            "gateway emitted OpenAI image sync JSON whitespace heartbeat"
        );
    }
}

fn find_sse_block_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| index + 2)
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
        })
}

fn parse_openai_image_sync_sse_frame(block: &[u8]) -> Option<OpenAiImageSyncSseFrame> {
    let text = std::str::from_utf8(block).ok()?.trim();
    if text.is_empty() {
        return None;
    }

    let mut event_name = None;
    let mut data_lines = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            event_name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim().to_string());
        }
    }

    let data_text = data_lines.join("\n");
    if data_text.trim().eq("[DONE]") {
        let event_name = event_name.unwrap_or_else(|| "done".to_string());
        return Some(OpenAiImageSyncSseFrame {
            event_name,
            is_partial_image: false,
            is_completed: true,
            is_failed: false,
            client_visible_event: None,
        });
    }

    let data_event_name = serde_json::from_str::<Value>(&data_text)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    value
                        .get("error")
                        .and_then(Value::as_object)
                        .map(|_| "error".to_string())
                })
        });
    let event_name = event_name.or(data_event_name)?;
    let is_partial_image = event_name == "response.image_generation_call.partial_image";
    let is_completed = event_name == "response.completed";
    let is_failed = event_name == "response.failed"
        || event_name == "response.error"
        || event_name == "error"
        || event_name.ends_with(".failed");
    let client_visible_event = if is_partial_image {
        Some("image_generation.partial_image")
    } else if is_completed {
        Some("image_generation.completed")
    } else if is_failed {
        Some("image_generation.failed")
    } else {
        None
    };

    Some(OpenAiImageSyncSseFrame {
        event_name,
        is_partial_image,
        is_completed,
        is_failed,
        client_visible_event,
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_direct_sync_runtime_candidate(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    trace_id: &str,
    plan_kind: &str,
    request_id_for_log: &str,
    candidate_id: Option<&str>,
    provider_name: &str,
    endpoint_id: &str,
    key_id: &str,
    model_name: &str,
    candidate_index: &str,
    progress_snapshot: Option<Arc<Mutex<OpenAiImageSyncProgressSnapshot>>>,
) -> Result<ExecutionResult, SyncExecutionFailure> {
    if let Some(result) = maybe_execute_windsurf_sync(state, plan, report_context)
        .await
        .map_err(SyncExecutionFailure::from_transport)?
    {
        return Ok(result);
    }
    if !should_track_openai_image_sync_upstream_sse(plan_kind, plan, report_context) {
        return DirectSyncExecutionRuntime::new()
            .execute_sync(plan)
            .await
            .map_err(SyncExecutionFailure::from_transport);
    }

    let started_at = Instant::now();
    let timeout_ms = resolve_openai_image_sync_total_timeout_ms(plan);
    let mut execution = Box::pin(execute_openai_image_sync_upstream_sse_candidate(
        state,
        plan,
        report_context,
        progress_snapshot.clone(),
    ));
    let mut idle_interval = tokio::time::interval(SYNC_EXECUTION_IDLE_LOG_INTERVAL);
    idle_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    idle_interval.tick().await;
    let mut total_timeout = Box::pin(tokio::time::sleep(Duration::from_millis(timeout_ms)));

    loop {
        tokio::select! {
            result = execution.as_mut() => {
                match result {
                    Ok(result) => return Ok(result),
                    Err(err) => {
                        let elapsed_ms = started_at.elapsed().as_millis() as u64;
                        let status_code = err.status_code;
                        record_openai_image_sync_failed_progress(
                            state,
                            plan,
                            report_context,
                            status_code,
                            elapsed_ms,
                            progress_snapshot.clone(),
                        )
                        .await;
                        return Err(err);
                    }
                }
            }
            _ = idle_interval.tick() => {
                warn!(
                    event_name = "openai_image_sync_execution_idle",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %request_id_for_log,
                    candidate_id = candidate_id.unwrap_or("-"),
                    provider_name,
                    endpoint_id,
                    key_id,
                    model_name,
                    candidate_index,
                    elapsed_ms = started_at.elapsed().as_millis() as u64,
                    timeout_ms,
                    "gateway OpenAI image sync execution still waiting for upstream response"
                );
            }
            _ = total_timeout.as_mut() => {
                let elapsed_ms = started_at.elapsed().as_millis() as u64;
                record_openai_image_sync_failed_progress(
                    state,
                    plan,
                    report_context,
                    Some(StatusCode::GATEWAY_TIMEOUT.as_u16()),
                    elapsed_ms,
                    progress_snapshot.clone(),
                )
                .await;
                warn!(
                    event_name = "openai_image_sync_total_timeout",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %request_id_for_log,
                    candidate_id = candidate_id.unwrap_or("-"),
                    provider_name,
                    endpoint_id,
                    key_id,
                    model_name,
                    candidate_index,
                    elapsed_ms,
                    timeout_ms,
                    "gateway OpenAI image sync execution exceeded total timeout"
                );
                return Err(SyncExecutionFailure::image_sync_total_timeout(
                    timeout_ms,
                    elapsed_ms,
                ));
            }
        }
    }
}

async fn execute_openai_image_sync_upstream_sse_candidate(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    progress_snapshot: Option<Arc<Mutex<OpenAiImageSyncProgressSnapshot>>>,
) -> Result<ExecutionResult, SyncExecutionFailure> {
    let request_body = build_request_body(plan).map_err(SyncExecutionFailure::from_transport)?;
    let started_at = Instant::now();
    let mut progress =
        OpenAiImageSyncProgressRecorder::new(state, plan, report_context, progress_snapshot);
    progress.record_connecting().await;

    let response = send_request(plan, request_body)
        .await
        .map_err(SyncExecutionFailure::from_transport)?;
    let ttfb_ms = started_at.elapsed().as_millis() as u64;
    let status_code = response.status_code();
    let headers = response.headers();
    progress.record_response_started(status_code, ttfb_ms).await;

    let mut body_bytes = Vec::new();
    match response {
        DirectHttpResponse::Reqwest(response) => {
            let mut upstream_stream = response.bytes_stream();
            while let Some(chunk) = upstream_stream.next().await {
                let chunk = chunk.map_err(|err| {
                    SyncExecutionFailure::from_transport(
                        ExecutionRuntimeTransportError::UpstreamRequest(
                            format_upstream_request_error(&err),
                        ),
                    )
                })?;
                let elapsed_ms = started_at.elapsed().as_millis() as u64;
                progress
                    .observe_chunk(&chunk, status_code, elapsed_ms)
                    .await;
                body_bytes.extend_from_slice(&chunk);
            }
        }
        DirectHttpResponse::BrowserWreq(response) => {
            let mut upstream_stream = response.bytes_stream();
            while let Some(chunk) = upstream_stream.next().await {
                let chunk = chunk.map_err(|err| {
                    SyncExecutionFailure::from_transport(
                        ExecutionRuntimeTransportError::UpstreamRequest(
                            format_wreq_upstream_request_error(&err),
                        ),
                    )
                })?;
                let elapsed_ms = started_at.elapsed().as_millis() as u64;
                progress
                    .observe_chunk(&chunk, status_code, elapsed_ms)
                    .await;
                body_bytes.extend_from_slice(&chunk);
            }
        }
    }

    let decoded_body_bytes =
        decode_response_body_bytes(&headers, &body_bytes).unwrap_or_else(|| body_bytes.clone());
    let elapsed_ms = started_at.elapsed().as_millis() as u64;
    let upstream_bytes = body_bytes.len() as u64;
    progress.finish(status_code, elapsed_ms).await;

    let body =
        build_execution_response_body(&headers, &body_bytes, &decoded_body_bytes, plan.stream)
            .map_err(SyncExecutionFailure::from_transport)?;

    Ok(ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers,
        body,
        telemetry: Some(ExecutionTelemetry {
            ttfb_ms: Some(ttfb_ms),
            elapsed_ms: Some(elapsed_ms),
            upstream_bytes: Some(upstream_bytes),
        }),
        error: None,
    })
}

async fn record_openai_image_sync_failed_progress(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    status_code: Option<u16>,
    elapsed_ms: u64,
    progress_snapshot: Option<Arc<Mutex<OpenAiImageSyncProgressSnapshot>>>,
) {
    let mut progress =
        OpenAiImageSyncProgressRecorder::new(state, plan, report_context, progress_snapshot);
    progress.fail(status_code, elapsed_ms).await;
}

fn resolve_openai_image_sync_total_timeout_ms(plan: &ExecutionPlan) -> u64 {
    plan.timeouts
        .as_ref()
        .and_then(|timeouts| timeouts.total_ms)
        .unwrap_or(OPENAI_IMAGE_SYNC_DEFAULT_TOTAL_TIMEOUT_MS)
        .max(1)
}

fn report_context_upstream_is_stream(report_context: Option<&Value>) -> bool {
    report_context
        .and_then(|value| value.get(UPSTREAM_IS_STREAM_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn should_track_openai_image_sync_upstream_sse(
    plan_kind: &str,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> bool {
    plan_kind == OPENAI_IMAGE_SYNC_PLAN_KIND
        && (plan.stream || report_context_upstream_is_stream(report_context))
}

fn should_enable_openai_image_sync_json_heartbeat(
    _plan_kind: &str,
    _plan: &ExecutionPlan,
    _report_context: Option<&Value>,
) -> bool {
    false
}

#[allow(clippy::too_many_arguments)]
fn build_openai_image_sync_json_heartbeat_response(
    state: AppState,
    request_path: String,
    plan: ExecutionPlan,
    trace_id: String,
    decision: GatewayControlDecision,
    plan_kind: String,
    report_kind: Option<String>,
    report_context: Option<Value>,
) -> Result<Response<Body>, GatewayError> {
    let request_id = plan.request_id.clone();
    let candidate_id = plan.candidate_id.clone();
    let trace_id_for_response = trace_id.clone();
    let decision_for_response = decision.clone();
    let progress_snapshot = Arc::new(Mutex::new(OpenAiImageSyncProgressSnapshot::new()));
    let heartbeat_context = OpenAiImageSyncJsonHeartbeatContext {
        state: state.clone(),
        plan: plan.clone(),
        report_context: report_context.clone(),
        snapshot: progress_snapshot.clone(),
        started_at: Instant::now(),
        trace_id: trace_id.clone(),
        request_id_for_log: short_request_id(request_id.as_str()),
        candidate_id: candidate_id.clone(),
    };
    let (tx, rx) = mpsc::channel::<Result<Bytes, IoError>>(1);

    tokio::spawn(async move {
        let bytes = openai_image_sync_json_heartbeat_final_bytes(
            execute_execution_runtime_sync_impl(
                &state,
                request_path.as_str(),
                plan,
                trace_id.as_str(),
                &decision,
                plan_kind.as_str(),
                report_kind,
                report_context,
                false,
                Some(progress_snapshot),
            )
            .await,
        )
        .await;
        let _ = tx.send(Ok(Bytes::from(bytes))).await;
    });

    let headers = BTreeMap::from([(
        CONTENT_TYPE.as_str().to_string(),
        "application/json".to_string(),
    )]);
    let response = build_client_response_from_parts_with_mutator(
        StatusCode::OK.as_u16(),
        &headers,
        Body::from_stream(build_json_whitespace_heartbeat_stream(
            rx,
            OPENAI_IMAGE_SYNC_JSON_HEARTBEAT_INTERVAL,
            Some(heartbeat_context),
        )),
        trace_id_for_response.as_str(),
        Some(&decision_for_response),
        |headers| {
            headers.remove(CONTENT_LENGTH);
            headers.remove(CONTENT_ENCODING);
            headers.insert(
                CACHE_CONTROL,
                HeaderValue::from_static("no-cache, no-transform"),
            );
            headers.insert(
                HeaderName::from_static("x-accel-buffering"),
                HeaderValue::from_static("no"),
            );
            Ok(())
        },
    )?;
    attach_control_metadata_headers(response, Some(request_id.as_str()), candidate_id.as_deref())
}

fn build_json_whitespace_heartbeat_stream(
    mut rx: mpsc::Receiver<Result<Bytes, IoError>>,
    heartbeat_interval: Duration,
    heartbeat_context: Option<OpenAiImageSyncJsonHeartbeatContext>,
) -> impl futures_util::Stream<Item = Result<Bytes, IoError>> + Send + 'static {
    stream! {
        if let Some(context) = heartbeat_context.as_ref() {
            context.record_heartbeat("initial", heartbeat_interval).await;
        }
        yield Ok(Bytes::from_static(OPENAI_IMAGE_SYNC_JSON_HEARTBEAT_BYTES));

        let mut heartbeat = tokio::time::interval(heartbeat_interval);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        heartbeat.tick().await;
        loop {
            tokio::select! {
                biased;
                item = rx.recv() => {
                    let Some(item) = item else {
                        break;
                    };
                    yield item;
                    break;
                }
                _ = heartbeat.tick() => {
                    if let Some(context) = heartbeat_context.as_ref() {
                        context.record_heartbeat("interval", heartbeat_interval).await;
                    }
                    yield Ok(Bytes::from_static(OPENAI_IMAGE_SYNC_JSON_HEARTBEAT_BYTES));
                }
            }
        }
    }
}

pub(crate) fn build_openai_image_sync_json_whitespace_heartbeat_stream(
    rx: mpsc::Receiver<Result<Bytes, IoError>>,
) -> impl futures_util::Stream<Item = Result<Bytes, IoError>> + Send + 'static {
    build_json_whitespace_heartbeat_stream(rx, OPENAI_IMAGE_SYNC_JSON_HEARTBEAT_INTERVAL, None)
}

async fn openai_image_sync_json_heartbeat_final_bytes(
    result: Result<Option<Response<Body>>, GatewayError>,
) -> Vec<u8> {
    match result {
        Ok(Some(response)) => match to_bytes(response.into_body(), usize::MAX).await {
            Ok(bytes) if !bytes.is_empty() => bytes.to_vec(),
            Ok(_) => openai_image_sync_json_heartbeat_error_body("empty sync image response"),
            Err(err) => openai_image_sync_json_heartbeat_error_body(&err.to_string()),
        },
        Ok(None) => openai_image_sync_json_heartbeat_error_body(
            "sync image execution ended without a local response",
        ),
        Err(err) => openai_image_sync_json_heartbeat_error_body(&format!("{err:?}")),
    }
}

fn openai_image_sync_json_heartbeat_error_body(message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "error": {
            "type": "aether_gateway_error",
            "message": message,
        }
    }))
    .unwrap_or_else(|_| b"{\"error\":{\"type\":\"aether_gateway_error\"}}".to_vec())
}

async fn apply_sync_success_effects(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewaySyncReportRequest,
) {
    apply_local_execution_effect(
        state,
        LocalExecutionEffectContext {
            plan,
            report_context,
        },
        LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
    )
    .await;
    apply_local_execution_effect(
        state,
        LocalExecutionEffectContext {
            plan,
            report_context,
        },
        LocalExecutionEffect::AdaptiveSuccess(LocalAdaptiveSuccessEffect),
    )
    .await;
    apply_local_execution_effect(
        state,
        LocalExecutionEffectContext {
            plan,
            report_context,
        },
        LocalExecutionEffect::PoolSuccessSync { payload },
    )
    .await;
}

#[cfg(test)]
enum RemoteSyncFallbackOutcome {
    Executed(ExecutionResult),
    ClientResponse(Response<Body>),
    Unavailable,
}

#[allow(clippy::too_many_arguments)] // internal function, grouping would add unnecessary indirection
pub(crate) async fn execute_execution_runtime_sync(
    state: &AppState,
    request_path: &str,
    mut plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    mut report_context: Option<serde_json::Value>,
) -> Result<Option<Response<Body>>, GatewayError> {
    execute_execution_runtime_sync_impl(
        state,
        request_path,
        plan,
        trace_id,
        decision,
        plan_kind,
        report_kind,
        report_context,
        true,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)] // internal function, grouping would add unnecessary indirection
async fn execute_execution_runtime_sync_impl(
    state: &AppState,
    request_path: &str,
    mut plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    mut report_context: Option<serde_json::Value>,
    allow_json_heartbeat: bool,
    progress_snapshot: Option<Arc<Mutex<OpenAiImageSyncProgressSnapshot>>>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if allow_json_heartbeat
        && should_enable_openai_image_sync_json_heartbeat(plan_kind, &plan, report_context.as_ref())
    {
        return build_openai_image_sync_json_heartbeat_response(
            state.clone(),
            request_path.to_string(),
            plan,
            trace_id.to_string(),
            decision.clone(),
            plan_kind.to_string(),
            report_kind,
            report_context,
        )
        .map(Some);
    }

    ensure_execution_request_candidate_slot(state, &mut plan, &mut report_context).await;
    let plan_request_id = plan.request_id.clone();
    let plan_request_id_for_log = short_request_id(plan_request_id.as_str());
    let plan_candidate_id = plan.candidate_id.clone();
    let provider_name = plan
        .provider_name
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let endpoint_id = plan.endpoint_id.clone();
    let key_id = plan.key_id.clone();
    let model_name = plan.model_name.clone().unwrap_or_else(|| "-".to_string());
    let candidate_index = parse_request_candidate_report_context(report_context.as_ref())
        .and_then(|context| context.candidate_index)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let candidate_started_unix_secs = current_request_candidate_unix_ms();
    let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
    state
        .usage_runtime
        .record_pending(state.data.as_ref(), lifecycle_seed);
    record_local_request_candidate_status(
        state,
        &plan,
        report_context.as_ref(),
        SchedulerRequestCandidateStatusUpdate {
            status: RequestCandidateStatus::Pending,
            status_code: None,
            error_type: None,
            error_message: None,
            latency_ms: None,
            started_at_unix_ms: Some(candidate_started_unix_secs),
            finished_at_unix_ms: None,
        },
    )
    .await;
    let mut terminal_guard = SyncAttemptTerminalGuard::new(
        state,
        &plan,
        report_context.clone(),
        candidate_started_unix_secs,
    );
    let result = (async {
    let _provider_pool_in_flight_guard = acquire_provider_pool_in_flight_guard(
        state.runtime_state.clone(),
        &plan.provider_id,
        plan_request_id.as_str(),
        plan_candidate_id.as_deref(),
        key_id.as_str(),
    )
    .await;
    #[cfg(not(test))]
    let mut result = {
        match maybe_execute_grok_sync(&plan, report_context.as_ref()).await {
            Ok(Some(result)) => result,
            Ok(None) => {
                match maybe_execute_chatgpt_web_image_sync(state, &plan, report_context.as_ref())
                    .await
                {
                    Ok(Some(result)) => result,
                    Ok(None) => match execute_direct_sync_runtime_candidate(
                        state,
                        &plan,
                        report_context.as_ref(),
                        trace_id,
                        plan_kind,
                        plan_request_id_for_log.as_str(),
                        plan_candidate_id.as_deref(),
                        provider_name.as_str(),
                        endpoint_id.as_str(),
                        key_id.as_str(),
                        model_name.as_str(),
                        candidate_index.as_str(),
                        progress_snapshot.clone(),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(err) => {
                            warn!(
                                event_name = "sync_execution_runtime_unavailable",
                                log_type = "ops",
                                trace_id = %trace_id,
                                request_id = %plan_request_id_for_log,
                                candidate_id = ?plan_candidate_id,
                                provider_name,
                                endpoint_id,
                                key_id,
                                model_name,
                                candidate_index = candidate_index.as_str(),
                                error_type = err.error_type,
                                error = %err.message,
                                "gateway in-process sync execution unavailable"
                            );
                            let terminal_unix_secs = current_request_candidate_unix_ms();
                            record_local_request_candidate_status(
                                state,
                                &plan,
                                report_context.as_ref(),
                                SchedulerRequestCandidateStatusUpdate {
                                    status: RequestCandidateStatus::Failed,
                                    status_code: err.status_code,
                                    error_type: Some(err.error_type.to_string()),
                                    error_message: Some(err.message),
                                    latency_ms: err.latency_ms,
                                    started_at_unix_ms: Some(candidate_started_unix_secs),
                                    finished_at_unix_ms: Some(terminal_unix_secs),
                                },
                            )
                            .await;
                            return Ok(None);
                        }
                    },
                    Err(err) => {
                        warn!(
                            event_name = "chatgpt_web_image_execution_unavailable",
                            log_type = "ops",
                            trace_id = %trace_id,
                            request_id = %plan_request_id_for_log,
                            candidate_id = ?plan_candidate_id,
                            provider_name,
                            endpoint_id,
                            key_id,
                            model_name,
                            candidate_index = candidate_index.as_str(),
                            error = %err,
                            "gateway ChatGPT-Web image execution unavailable"
                        );
                        let terminal_unix_secs = current_request_candidate_unix_ms();
                        record_local_request_candidate_status(
                            state,
                            &plan,
                            report_context.as_ref(),
                            SchedulerRequestCandidateStatusUpdate {
                                status: RequestCandidateStatus::Failed,
                                status_code: None,
                                error_type: Some(
                                    "chatgpt_web_image_execution_unavailable".to_string(),
                                ),
                                error_message: Some(err.to_string()),
                                latency_ms: None,
                                started_at_unix_ms: Some(candidate_started_unix_secs),
                                finished_at_unix_ms: Some(terminal_unix_secs),
                            },
                        )
                        .await;
                        return Ok(None);
                    }
                }
            }
            Err(err) => {
                warn!(
                    event_name = "grok_execution_unavailable",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %plan_request_id_for_log,
                    candidate_id = ?plan_candidate_id,
                    provider_name,
                    endpoint_id,
                    key_id,
                    model_name,
                    candidate_index = candidate_index.as_str(),
                    error = %err,
                    "gateway Grok execution unavailable"
                );
                let terminal_unix_secs = current_request_candidate_unix_ms();
                record_local_request_candidate_status(
                    state,
                    &plan,
                    report_context.as_ref(),
                    SchedulerRequestCandidateStatusUpdate {
                        status: RequestCandidateStatus::Failed,
                        status_code: None,
                        error_type: Some("grok_execution_unavailable".to_string()),
                        error_message: Some(err.to_string()),
                        latency_ms: None,
                        started_at_unix_ms: Some(candidate_started_unix_secs),
                        finished_at_unix_ms: Some(terminal_unix_secs),
                    },
                )
                .await;
                return Ok(None);
            }
        }
    };
    #[cfg(test)]
    let mut result = {
        if let Some(override_fn) = state.execution_runtime_sync_override.as_ref() {
            match (override_fn.0)(&plan) {
                Ok(result) => result,
                Err(err) => {
                    warn!(
                        event_name = "sync_execution_runtime_test_override_failed",
                        log_type = "ops",
                        trace_id = %trace_id,
                        request_id = %plan_request_id_for_log,
                        candidate_id = ?plan_candidate_id,
                        provider_name,
                        endpoint_id,
                        key_id,
                        model_name,
                        candidate_index = candidate_index.as_str(),
                        error = ?err,
                        "gateway test sync execution override failed"
                    );
                    let terminal_unix_secs = current_request_candidate_unix_ms();
                    record_local_request_candidate_status(
                        state,
                        &plan,
                        report_context.as_ref(),
                        SchedulerRequestCandidateStatusUpdate {
                            status: RequestCandidateStatus::Failed,
                            status_code: None,
                            error_type: Some("execution_runtime_unavailable".to_string()),
                            error_message: Some(format!("{err:?}")),
                            latency_ms: None,
                            started_at_unix_ms: Some(candidate_started_unix_secs),
                            finished_at_unix_ms: Some(terminal_unix_secs),
                        },
                    )
                    .await;
                    return Ok(None);
                }
            }
        } else if state
            .execution_runtime_override_base_url()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            match maybe_execute_grok_sync(&plan, report_context.as_ref()).await {
                Ok(Some(result)) => result,
                Ok(None) => match maybe_execute_chatgpt_web_image_sync(
                    state,
                    &plan,
                    report_context.as_ref(),
                )
                .await
                {
                    Ok(Some(result)) => result,
                    Ok(None) => match execute_direct_sync_runtime_candidate(
                        state,
                        &plan,
                        report_context.as_ref(),
                        trace_id,
                        plan_kind,
                        plan_request_id_for_log.as_str(),
                        plan_candidate_id.as_deref(),
                        provider_name.as_str(),
                        endpoint_id.as_str(),
                        key_id.as_str(),
                        model_name.as_str(),
                        candidate_index.as_str(),
                        progress_snapshot.clone(),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(err) => {
                            warn!(
                                event_name = "sync_execution_runtime_unavailable",
                                log_type = "ops",
                                trace_id = %trace_id,
                                request_id = %plan_request_id_for_log,
                                candidate_id = ?plan_candidate_id,
                                provider_name,
                                endpoint_id,
                                key_id,
                                model_name,
                                candidate_index = candidate_index.as_str(),
                                error_type = err.error_type,
                                error = %err.message,
                                "gateway in-process sync execution unavailable"
                            );
                            let terminal_unix_secs = current_request_candidate_unix_ms();
                            record_local_request_candidate_status(
                                state,
                                &plan,
                                report_context.as_ref(),
                                SchedulerRequestCandidateStatusUpdate {
                                    status: RequestCandidateStatus::Failed,
                                    status_code: err.status_code,
                                    error_type: Some(err.error_type.to_string()),
                                    error_message: Some(err.message),
                                    latency_ms: err.latency_ms,
                                    started_at_unix_ms: Some(candidate_started_unix_secs),
                                    finished_at_unix_ms: Some(terminal_unix_secs),
                                },
                            )
                            .await;
                            return Ok(None);
                        }
                    },
                    Err(err) => {
                        warn!(
                            event_name = "chatgpt_web_image_execution_unavailable",
                            log_type = "ops",
                            trace_id = %trace_id,
                            request_id = %plan_request_id_for_log,
                            candidate_id = ?plan_candidate_id,
                            provider_name,
                            endpoint_id,
                            key_id,
                            model_name,
                            candidate_index = candidate_index.as_str(),
                            error = %err,
                            "gateway ChatGPT-Web image execution unavailable"
                        );
                        let terminal_unix_secs = current_request_candidate_unix_ms();
                        record_local_request_candidate_status(
                            state,
                            &plan,
                            report_context.as_ref(),
                            SchedulerRequestCandidateStatusUpdate {
                                status: RequestCandidateStatus::Failed,
                                status_code: None,
                                error_type: Some(
                                    "chatgpt_web_image_execution_unavailable".to_string(),
                                ),
                                error_message: Some(err.to_string()),
                                latency_ms: None,
                                started_at_unix_ms: Some(candidate_started_unix_secs),
                                finished_at_unix_ms: Some(terminal_unix_secs),
                            },
                        )
                        .await;
                        return Ok(None);
                    }
                },
                Err(err) => {
                    warn!(
                        event_name = "grok_execution_unavailable",
                        log_type = "ops",
                        trace_id = %trace_id,
                        request_id = %plan_request_id_for_log,
                        candidate_id = ?plan_candidate_id,
                        provider_name,
                        endpoint_id,
                        key_id,
                        model_name,
                        candidate_index = candidate_index.as_str(),
                        error = %err,
                        "gateway Grok execution unavailable"
                    );
                    let terminal_unix_secs = current_request_candidate_unix_ms();
                    record_local_request_candidate_status(
                        state,
                        &plan,
                        report_context.as_ref(),
                        SchedulerRequestCandidateStatusUpdate {
                            status: RequestCandidateStatus::Failed,
                            status_code: None,
                            error_type: Some("grok_execution_unavailable".to_string()),
                            error_message: Some(err.to_string()),
                            latency_ms: None,
                            started_at_unix_ms: Some(candidate_started_unix_secs),
                            finished_at_unix_ms: Some(terminal_unix_secs),
                        },
                    )
                    .await;
                    return Ok(None);
                }
            }
        } else {
            let remote_execution_runtime_base_url = state
                .execution_runtime_override_base_url()
                .unwrap_or_default();
            let remote_outcome = execute_sync_via_remote_execution_runtime(
                state,
                remote_execution_runtime_base_url,
                trace_id,
                decision,
                &plan,
                plan_request_id.as_str(),
                plan_candidate_id.as_deref(),
                report_context.as_ref(),
                candidate_started_unix_secs,
            )
            .await?;
            match remote_outcome {
                RemoteSyncFallbackOutcome::Executed(result) => result,
                RemoteSyncFallbackOutcome::ClientResponse(response) => return Ok(Some(response)),
                RemoteSyncFallbackOutcome::Unavailable => return Ok(None),
            }
        }
    };
    let mut oauth_retry_attempted = false;
    let (
        result_error_type,
        result_error_message,
        result_latency_ms,
        headers,
        body_bytes,
        body_json,
        body_base64,
        local_failover_response_text,
        local_failover_analysis,
    ) = loop {
        let result_latency_ms = result
            .telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.elapsed_ms);
        let mut headers = std::mem::take(&mut result.headers);
        let (body_bytes, mut body_json, body_base64) =
            decode_execution_result_body(result.body.take(), &mut headers)?;
        if let Some(message) = invalid_gemini_provider_success_message(
            &plan,
            report_context.as_ref(),
            result.status_code,
            body_json.as_ref(),
        ) {
            result.status_code = StatusCode::BAD_GATEWAY.as_u16();
            result.error = Some(ExecutionError {
                kind: ExecutionErrorKind::Upstream5xx,
                phase: ExecutionPhase::Finalize,
                message: message.to_string(),
                upstream_status: Some(StatusCode::OK.as_u16()),
                retryable: false,
                failover_recommended: false,
            });
            if let Some(error_body) =
                build_invalid_provider_success_body(&plan, report_context.as_ref(), message)
            {
                body_json = Some(error_body);
                headers.insert("content-type".to_string(), "application/json".to_string());
            }
        }
        let (mut result_error_type, mut result_error_message) =
            execution_error_details(result.error.as_ref(), body_json.as_ref());
        if result.status_code < 400 && body_json.is_none() {
            if let Some(error_body_json) =
                extract_provider_private_stream_error_body(report_context.as_ref(), &body_bytes)
            {
                result.status_code =
                    resolve_local_sync_error_status_code(result.status_code, &error_body_json);
                let (private_error_type, private_error_message) =
                    provider_private_error_details(&error_body_json);
                result_error_type = private_error_type.or(result_error_type);
                result_error_message = private_error_message.or(result_error_message);
                body_json = Some(error_body_json);
            }
        }
        let local_failover_response_text = local_failover_response_text(
            body_json.as_ref(),
            &body_bytes,
            result.error.as_ref().map(|error| error.message.as_str()),
        );

        if result.status_code >= 400
            && !oauth_retry_attempted
            && refresh_oauth_plan_auth_for_retry(
                state,
                &mut plan,
                result.status_code,
                local_failover_response_text.as_deref(),
                trace_id,
            )
            .await
        {
            oauth_retry_attempted = true;
            match crate::execution_runtime::execute_execution_runtime_sync_plan(
                state,
                Some(trace_id),
                &plan,
            )
            .await
            {
                Ok(retry_result) => {
                    result = retry_result;
                    continue;
                }
                Err(err) => {
                    warn!(
                        event_name = "local_sync_oauth_retry_execution_failed",
                        log_type = "ops",
                        trace_id = %trace_id,
                        request_id = %plan_request_id_for_log,
                        candidate_id = ?plan_candidate_id,
                        provider_name,
                        endpoint_id,
                        key_id,
                        model_name,
                        candidate_index = candidate_index.as_str(),
                        error = ?err,
                        "gateway oauth retry sync execution failed"
                    );
                }
            }
        }

        let local_failover_analysis = analyze_local_candidate_failover_sync(
            state,
            &plan,
            plan_kind,
            report_context.as_ref(),
            &result,
            local_failover_response_text.as_deref(),
        )
        .await;
        break (
            result_error_type,
            result_error_message,
            result_latency_ms,
            headers,
            body_bytes,
            body_json,
            body_base64,
            local_failover_response_text,
            local_failover_analysis,
        );
    };
    if result.status_code >= 400 {
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code: result.status_code,
                classification: local_failover_analysis.classification,
            }),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::AdaptiveRateLimit(LocalAdaptiveRateLimitEffect {
                status_code: result.status_code,
                classification: local_failover_analysis.classification,
                headers: Some(&headers),
            }),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
                status_code: result.status_code,
                classification: local_failover_analysis.classification,
            }),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code: result.status_code,
                response_text: local_failover_response_text.as_deref(),
            }),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::PoolError(LocalPoolErrorEffect {
                status_code: result.status_code,
                classification: local_failover_analysis.classification,
                headers: &headers,
                error_body: local_failover_response_text.as_deref(),
            }),
        )
        .await;
    }
    if matches!(
        local_failover_analysis.decision,
        LocalFailoverDecision::RetryNextCandidate
    ) {
        let terminal_unix_secs = current_request_candidate_unix_ms();
        let error_trace_report_context = with_sync_error_trace_context(
            report_context.as_ref(),
            result.status_code,
            &headers,
            body_json.as_ref(),
            &body_bytes,
            local_failover_response_text.as_deref(),
            local_failover_analysis,
        );
        record_local_request_candidate_status(
            state,
            &plan,
            error_trace_report_context
                .as_ref()
                .or(report_context.as_ref()),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(result.status_code),
                error_type: result_error_type.clone(),
                error_message: result_error_message.clone(),
                latency_ms: result_latency_ms,
                started_at_unix_ms: Some(candidate_started_unix_secs),
                finished_at_unix_ms: Some(terminal_unix_secs),
            },
        )
        .await;
        warn!(
            event_name = "local_sync_candidate_retry_scheduled",
            log_type = "event",
            trace_id = %trace_id,
            request_id = %plan_request_id_for_log,
            status_code = result.status_code,
            provider_name,
            endpoint_id,
            key_id,
            model_name,
            candidate_index = candidate_index.as_str(),
            "gateway local sync decision retrying next candidate after retryable execution runtime result"
        );
        return Ok(None);
    }
    let status_code = result.status_code;
    let has_body_bytes = body_base64.is_some();
    let report_context =
        attach_provider_response_headers_to_report_context(report_context, &headers);
    let mut client_headers = headers.clone();
    apply_endpoint_response_header_rules(state, &plan, &mut client_headers, body_json.as_ref())
        .await?;
    let explicit_finalize = should_finalize_sync_response(report_kind.as_deref());
    let mapped_error_finalize_kind =
        resolve_core_sync_error_finalize_report_kind(plan_kind, &result, body_json.as_ref());
    let implicit_finalize = if !explicit_finalize && mapped_error_finalize_kind.is_none() {
        maybe_build_implicit_sync_finalize_outcome(
            trace_id,
            decision,
            plan_kind,
            &report_context,
            status_code,
            &client_headers,
            &body_json,
            &body_base64,
            &result.telemetry,
        )?
    } else {
        None
    };
    if !matches!(
        local_failover_analysis.decision,
        LocalFailoverDecision::StopLocalFailover
    ) && should_fallback_to_control_sync(
        plan_kind,
        &result,
        body_json.as_ref(),
        has_body_bytes,
        explicit_finalize || implicit_finalize.is_some(),
        mapped_error_finalize_kind.is_some(),
    ) {
        let terminal_unix_secs = current_request_candidate_unix_ms();
        let error_trace_report_context = with_sync_error_trace_context(
            report_context.as_ref(),
            result.status_code,
            &headers,
            body_json.as_ref(),
            &body_bytes,
            local_failover_response_text.as_deref(),
            local_failover_analysis,
        );
        record_local_request_candidate_status(
            state,
            &plan,
            error_trace_report_context
                .as_ref()
                .or(report_context.as_ref()),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(result.status_code),
                error_type: result_error_type.clone(),
                error_message: result_error_message.clone(),
                latency_ms: result_latency_ms,
                started_at_unix_ms: Some(candidate_started_unix_secs),
                finished_at_unix_ms: Some(terminal_unix_secs),
            },
        )
        .await;
        return Ok(None);
    }

    let terminal_unix_secs = current_request_candidate_unix_ms();
    let error_flow_report_context = (result.status_code >= 400)
        .then(|| {
            with_sync_error_trace_context(
                report_context.as_ref(),
                result.status_code,
                &headers,
                body_json.as_ref(),
                &body_bytes,
                local_failover_response_text.as_deref(),
                local_failover_analysis,
            )
        })
        .flatten();
    record_local_request_candidate_status(
        state,
        &plan,
        error_flow_report_context
            .as_ref()
            .or(report_context.as_ref()),
        SchedulerRequestCandidateStatusUpdate {
            status: if result.status_code >= 400 {
                RequestCandidateStatus::Failed
            } else {
                RequestCandidateStatus::Success
            },
            status_code: Some(result.status_code),
            error_type: result_error_type.clone(),
            error_message: result_error_message.clone(),
            latency_ms: result_latency_ms,
            started_at_unix_ms: Some(candidate_started_unix_secs),
            finished_at_unix_ms: Some(terminal_unix_secs),
        },
    )
    .await;

    let request_id_owned = result.request_id;
    let candidate_id_owned = result.candidate_id;
    let request_id = (!request_id_owned.trim().is_empty())
        .then_some(request_id_owned.as_str())
        .or(Some(plan_request_id.as_str()));
    let request_id_for_log = short_request_id(request_id.unwrap_or("-"));
    let candidate_id = candidate_id_owned
        .as_deref()
        .or(plan_candidate_id.as_deref());
    let report_context = report_context;
    let body_json = body_json;
    let telemetry = result.telemetry;

    if let Some(implicit_finalize) = implicit_finalize {
        let usage_payload = implicit_finalize
            .outcome
            .background_report
            .as_ref()
            .unwrap_or(&implicit_finalize.payload);
        apply_sync_success_effects(
            state,
            &plan,
            implicit_finalize.payload.report_context.as_ref(),
            usage_payload,
        )
        .await;
        record_sync_terminal_usage_and_disarm_guard(
            state,
            &plan,
            implicit_finalize.payload.report_context.as_ref(),
            usage_payload,
            &mut terminal_guard,
        );
        if let Some(report_payload) = implicit_finalize.outcome.background_report {
            spawn_sync_report(state.clone(), report_payload);
        } else {
            warn!(
                event_name = "local_core_finalize_missing_success_report_mapping",
                log_type = "event",
                trace_id = %trace_id,
                report_kind = %implicit_finalize.payload.report_kind,
                "gateway implicit local core finalize produced response without background success report mapping"
            );
        }
        return Ok(Some(attach_control_metadata_headers(
            implicit_finalize.outcome.response,
            request_id,
            candidate_id,
        )?));
    }

    let finalize_report_kind = if explicit_finalize {
        report_kind.clone()
    } else {
        mapped_error_finalize_kind
    };

    if let Some(finalize_report_kind) = finalize_report_kind {
        let mut payload = build_sync_report_payload(
            trace_id,
            finalize_report_kind,
            report_context,
            status_code,
            client_headers,
            body_json,
            body_base64,
            telemetry,
        );
        if let Some(outcome) = maybe_build_sync_finalize_outcome(trace_id, decision, &payload)? {
            let usage_payload = outcome.background_report.as_ref().unwrap_or(&payload);
            if status_code < 400 {
                apply_sync_success_effects(
                    state,
                    &plan,
                    payload.report_context.as_ref(),
                    usage_payload,
                )
                .await;
            }
            record_sync_terminal_usage_and_disarm_guard(
                state,
                &plan,
                payload.report_context.as_ref(),
                usage_payload,
                &mut terminal_guard,
            );
            if let Some(report_payload) = outcome.background_report {
                spawn_sync_report(state.clone(), report_payload);
            } else {
                warn!(
                    event_name = "local_core_finalize_missing_success_report_mapping",
                    log_type = "event",
                    trace_id = %trace_id,
                    report_kind = %payload.report_kind,
                    "gateway local core finalize produced response without background success report mapping"
                );
            }
            return Ok(Some(attach_control_metadata_headers(
                outcome.response,
                request_id,
                candidate_id,
            )?));
        }
        let mut payload = match maybe_build_local_video_success_outcome(
            trace_id,
            decision,
            payload,
            &state.video_tasks,
            &plan,
        )? {
            LocalVideoSyncSuccessBuild::Handled(outcome) => {
                let LocalVideoSyncSuccessOutcome {
                    response,
                    report_payload,
                    original_report_context,
                    report_mode,
                    local_task_snapshot,
                } = outcome;
                apply_sync_success_effects(
                    state,
                    &plan,
                    original_report_context.as_ref(),
                    &report_payload,
                )
                .await;
                record_sync_terminal_usage_and_disarm_guard(
                    state,
                    &plan,
                    original_report_context.as_ref(),
                    &report_payload,
                    &mut terminal_guard,
                );
                if let Some(snapshot) = local_task_snapshot {
                    let _ = state.upsert_video_task_snapshot(&snapshot).await?;
                    state.video_tasks.record_snapshot(snapshot);
                }
                match report_mode {
                    VideoTaskSyncReportMode::InlineSync => {
                        submit_sync_report(state, report_payload).await?;
                    }
                    VideoTaskSyncReportMode::Background => {
                        spawn_sync_report(state.clone(), report_payload);
                    }
                }
                return Ok(Some(attach_control_metadata_headers(
                    response,
                    request_id,
                    candidate_id,
                )?));
            }
            LocalVideoSyncSuccessBuild::NotHandled(payload) => payload,
        };
        if let Some(response) =
            maybe_build_local_sync_finalize_response(trace_id, decision, &payload)?
        {
            let background_success_report_kind =
                resolve_local_sync_success_background_report_kind(payload.report_kind.as_str());
            apply_sync_success_effects(state, &plan, payload.report_context.as_ref(), &payload)
                .await;
            record_sync_terminal_usage_and_disarm_guard(
                state,
                &plan,
                payload.report_context.as_ref(),
                &payload,
                &mut terminal_guard,
            );
            state
                .video_tasks
                .apply_finalize_mutation(request_path, payload.report_kind.as_str());
            if let Some(snapshot) = state
                .video_tasks
                .snapshot_for_route(decision.route_family.as_deref(), request_path)
            {
                let _ = state.upsert_video_task_snapshot(&snapshot).await?;
            }
            if let Some(success_report_kind) = background_success_report_kind {
                payload.report_kind = success_report_kind.to_string();
            }
            if background_success_report_kind.is_some() {
                spawn_sync_report(state.clone(), payload);
            } else {
                warn!(
                    event_name = "local_video_finalize_missing_success_report_mapping",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %request_id_for_log,
                    candidate_id = ?candidate_id,
                    report_kind = %payload.report_kind,
                    "gateway local video finalize produced response without background success report mapping"
                );
            }
            return Ok(Some(attach_control_metadata_headers(
                response,
                request_id,
                candidate_id,
            )?));
        }
        if let Some(response) =
            maybe_build_local_video_error_response(trace_id, decision, &payload)?
        {
            let background_error_report_kind =
                resolve_local_sync_error_background_report_kind(payload.report_kind.as_str());
            if let Some(error_report_kind) = background_error_report_kind {
                payload.report_kind = error_report_kind.to_string();
            }
            record_sync_terminal_usage_and_disarm_guard(
                state,
                &plan,
                payload.report_context.as_ref(),
                &payload,
                &mut terminal_guard,
            );
            if background_error_report_kind.is_some() {
                spawn_sync_report(state.clone(), payload);
            } else {
                warn!(
                    event_name = "local_video_finalize_missing_error_report_mapping",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %request_id_for_log,
                    candidate_id = ?candidate_id,
                    report_kind = %payload.report_kind,
                    "gateway local video finalize produced response without background error report mapping"
                );
            }
            return Ok(Some(attach_control_metadata_headers(
                response,
                request_id,
                candidate_id,
            )?));
        }
        record_sync_terminal_usage_and_disarm_guard(
            state,
            &plan,
            payload.report_context.as_ref(),
            &payload,
            &mut terminal_guard,
        );
        let response =
            submit_local_core_error_or_sync_finalize(state, trace_id, decision, payload).await?;
        return Ok(Some(attach_control_metadata_headers(
            response,
            request_id,
            candidate_id,
        )?));
    }

    let usage_payload = build_sync_report_payload(
        trace_id,
        report_kind.unwrap_or_default(),
        report_context,
        status_code,
        client_headers,
        body_json,
        body_base64,
        telemetry,
    );
    if status_code < 400 {
        apply_sync_success_effects(
            state,
            &plan,
            usage_payload.report_context.as_ref(),
            &usage_payload,
        )
        .await;
    }
    record_sync_terminal_usage_and_disarm_guard(
        state,
        &plan,
        usage_payload.report_context.as_ref(),
        &usage_payload,
        &mut terminal_guard,
    );
    let response = attach_control_metadata_headers(
        build_client_response_from_parts(
            status_code,
            &usage_payload.headers,
            Body::from(body_bytes),
            trace_id,
            Some(decision),
        )?,
        request_id,
        candidate_id,
    )?;
    if !usage_payload.report_kind.trim().is_empty() {
        spawn_sync_report(state.clone(), usage_payload);
    }

    Ok(Some(response))
    })
    .await;
    if let Err(error) = result.as_ref() {
        terminal_guard.fail_and_disarm(error).await;
    } else {
        terminal_guard.disarm();
    }
    result
}

#[allow(clippy::too_many_arguments)] // mirrors sync execution context
fn maybe_build_implicit_sync_finalize_outcome(
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_context: &Option<serde_json::Value>,
    status_code: u16,
    headers: &BTreeMap<String, String>,
    body_json: &Option<serde_json::Value>,
    body_base64: &Option<String>,
    telemetry: &Option<ExecutionTelemetry>,
) -> Result<Option<ImplicitSyncFinalizeOutcome>, GatewayError> {
    if status_code >= 400 || body_json.is_some() || body_base64.is_none() {
        return Ok(None);
    }

    let Some(report_kind) = implicit_sync_finalize_report_kind(plan_kind) else {
        return Ok(None);
    };

    let payload = GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind: report_kind.to_string(),
        report_context: report_context.clone(),
        status_code,
        headers: headers.clone(),
        body_json: body_json.clone(),
        client_body_json: None,
        body_base64: body_base64.clone(),
        telemetry: telemetry.clone(),
    };
    let Some(outcome) = maybe_build_sync_finalize_outcome(trace_id, decision, &payload)? else {
        return Ok(None);
    };

    Ok(Some(ImplicitSyncFinalizeOutcome { payload, outcome }))
}

#[allow(clippy::too_many_arguments)] // internal helper mirroring execute path context
#[cfg(test)]
async fn execute_sync_via_remote_execution_runtime(
    state: &AppState,
    remote_execution_runtime_base_url: &str,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan: &ExecutionPlan,
    plan_request_id: &str,
    plan_candidate_id: Option<&str>,
    report_context: Option<&serde_json::Value>,
    candidate_started_unix_secs: u64,
) -> Result<RemoteSyncFallbackOutcome, GatewayError> {
    let response = match post_sync_plan_to_remote_execution_runtime(
        state,
        remote_execution_runtime_base_url,
        Some(trace_id),
        plan,
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            warn!(
                event_name = "sync_execution_runtime_remote_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %short_request_id(plan_request_id),
                candidate_id = ?plan_candidate_id,
                error = ?err,
                "gateway remote execution runtime sync unavailable"
            );
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                plan,
                report_context,
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("execution_runtime_unavailable".to_string()),
                    error_message: Some(format!("{err:?}")),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            return Ok(RemoteSyncFallbackOutcome::Unavailable);
        }
    };

    if response.status() != http::StatusCode::OK {
        let terminal_unix_secs = current_request_candidate_unix_ms();
        record_local_request_candidate_status(
            state,
            plan,
            report_context,
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(response.status().as_u16()),
                error_type: Some("execution_runtime_http_error".to_string()),
                error_message: Some(format!(
                    "execution runtime returned HTTP {}",
                    response.status()
                )),
                latency_ms: None,
                started_at_unix_ms: Some(candidate_started_unix_secs),
                finished_at_unix_ms: Some(terminal_unix_secs),
            },
        )
        .await;
        return Ok(RemoteSyncFallbackOutcome::ClientResponse(
            attach_control_metadata_headers(
                build_client_response(response, trace_id, Some(decision))?,
                Some(plan_request_id),
                plan_candidate_id,
            )?,
        ));
    }

    response
        .json()
        .await
        .map(RemoteSyncFallbackOutcome::Executed)
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::candidates::RequestCandidateReadRepository;
    use aether_data_contracts::repository::usage::UsageReadRepository;
    use aether_usage_runtime::UsageRuntimeConfig;
    use futures_util::{pin_mut, StreamExt as _};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn test_openai_image_plan(stream: bool) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".to_string(),
            candidate_id: Some("candidate-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: aether_contracts::RequestBody::from_json(json!({"stream": true})),
            stream,
            client_api_format: "openai:image".to_string(),
            provider_api_format: "openai:image".to_string(),
            model_name: Some("gpt-image-2".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn test_gemini_chat_plan() -> ExecutionPlan {
        let mut plan = test_openai_image_plan(false);
        plan.client_api_format = "openai:chat".to_string();
        plan.provider_api_format = "gemini:generate_content".to_string();
        plan.model_name = Some("gemini-3-flash-preview".to_string());
        plan
    }

    #[test]
    fn invalid_gemini_provider_success_uses_plan_format_when_context_is_missing() {
        let plan = test_gemini_chat_plan();
        let body = json!({
            "candidates": [{
                "content": {"role": "model"},
                "finishReason": "MAX_TOKENS"
            }],
            "usageMetadata": {
                "promptTokenCount": 8,
                "candidatesTokenCount": 1,
                "thoughtsTokenCount": 25,
                "totalTokenCount": 34
            }
        });

        let message = invalid_gemini_provider_success_message(
            &plan,
            None,
            StatusCode::OK.as_u16(),
            Some(&body),
        )
        .expect("empty Gemini 200 response should be rejected from plan format");

        assert!(message.contains("visible model output"));
    }

    #[test]
    fn invalid_gemini_provider_success_unwraps_gemini_cli_v1internal_envelope() {
        let plan = test_gemini_chat_plan();
        let report_context = json!({
            "has_envelope": true,
            "envelope_name": "gemini_cli:v1internal",
            "provider_api_format": "gemini:generate_content",
        });
        let body = json!({
            "response": {
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{"text": "Hello from Gemini CLI"}]
                    },
                    "finishReason": "STOP"
                }]
            },
            "remainingCredits": 41,
            "consumedCredits": 1,
            "traceId": "trace-upstream-sync-1"
        });

        let message = invalid_gemini_provider_success_message(
            &plan,
            Some(&report_context),
            StatusCode::OK.as_u16(),
            Some(&body),
        );

        assert!(message.is_none());
    }

    #[tokio::test]
    async fn sync_attempt_terminal_guard_marks_dropped_pending_attempt_cancelled() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
        let mut plan = test_openai_image_plan(false);
        plan.request_id = "sync-cancel-guard-request".to_string();
        plan.candidate_id = None;
        let mut report_context = Some(json!({
            "candidate_index": 0,
            "retry_index": 0,
            "user_id": "user-cancel",
            "api_key_id": "api-key-cancel",
            "client_api_format": "openai:image",
            "provider_api_format": "openai:image",
            "request_path": "/v1/images/generations",
            "request_path_and_query": "/v1/images/generations",
            "upstream_url": "https://example.test/v1/images/generations",
            "mapped_model": "gpt-image-2",
        }));

        ensure_execution_request_candidate_slot(&state, &mut plan, &mut report_context).await;
        let started_at = current_request_candidate_unix_ms();
        state.usage_runtime.record_pending(
            state.data.as_ref(),
            build_lifecycle_usage_seed(&plan, report_context.as_ref()),
        );
        record_local_request_candidate_status(
            &state,
            &plan,
            report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Pending,
                status_code: None,
                error_type: None,
                error_message: None,
                latency_ms: None,
                started_at_unix_ms: Some(started_at),
                finished_at_unix_ms: None,
            },
        )
        .await;

        {
            let _guard =
                SyncAttemptTerminalGuard::new(&state, &plan, report_context.clone(), started_at);
        }

        let mut stored_usage = None;
        for _ in 0..50 {
            if let Some(usage) = usage_repository
                .find_by_request_id("sync-cancel-guard-request")
                .await
                .expect("usage should read")
            {
                if usage.status == "cancelled" {
                    stored_usage = Some(usage);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let stored_usage = stored_usage.expect("cancelled usage should be recorded");
        assert_eq!(stored_usage.status, "cancelled");
        assert_eq!(stored_usage.billing_status, "pending");
        assert_eq!(stored_usage.status_code, Some(499));
        assert_eq!(stored_usage.error_category.as_deref(), Some("cancelled"));

        let stored_candidates = request_candidate_repository
            .list_by_request_id("sync-cancel-guard-request")
            .await
            .expect("request candidates should read");
        assert_eq!(stored_candidates.len(), 1);
        assert_eq!(
            stored_candidates[0].status,
            RequestCandidateStatus::Cancelled
        );
        assert_eq!(stored_candidates[0].status_code, Some(499));
        assert_eq!(
            stored_candidates[0].error_type.as_deref(),
            Some("local_sync_attempt_cancelled")
        );
    }

    #[tokio::test]
    async fn json_whitespace_heartbeat_stream_prefixes_final_json() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, IoError>>(1);
        tx.send(Ok(Bytes::from_static(br#"{"data":[]}"#)))
            .await
            .expect("final body should send");
        drop(tx);

        let body = to_bytes(
            Body::from_stream(build_json_whitespace_heartbeat_stream(
                rx,
                Duration::from_secs(60),
                None,
            )),
            usize::MAX,
        )
        .await
        .expect("body should collect");

        assert!(body.starts_with(b"\n"));
        let parsed: Value =
            serde_json::from_slice(&body).expect("leading whitespace is valid JSON");
        assert_eq!(parsed, json!({"data": []}));
    }

    #[tokio::test]
    async fn json_whitespace_heartbeat_stream_emits_interval_whitespace() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, IoError>>(1);
        let stream = build_json_whitespace_heartbeat_stream(rx, Duration::from_millis(5), None);
        pin_mut!(stream);

        let first = stream
            .next()
            .await
            .expect("initial whitespace")
            .expect("initial whitespace ok");
        assert_eq!(first, Bytes::from_static(b"\n"));

        let second = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("interval heartbeat")
            .expect("interval heartbeat item")
            .expect("interval heartbeat ok");
        assert_eq!(second, Bytes::from_static(b"\n"));

        tx.send(Ok(Bytes::from_static(br#"{"data":[{"b64_json":"x"}]}"#)))
            .await
            .expect("final body should send");
        let final_body = tokio::time::timeout(Duration::from_millis(100), stream.next())
            .await
            .expect("final body")
            .expect("final body item")
            .expect("final body ok");
        assert_eq!(
            serde_json::from_slice::<Value>(&final_body).expect("final body json"),
            json!({"data": [{"b64_json": "x"}]})
        );
    }

    #[test]
    fn openai_image_sync_sse_parser_tracks_partial_and_completed_frames() {
        let partial = parse_openai_image_sync_sse_frame(
            concat!(
                "event: response.image_generation_call.partial_image\n",
                "data: {\"type\":\"response.image_generation_call.partial_image\",\"partial_image_index\":0}\n\n"
            )
            .as_bytes(),
        )
        .expect("partial frame");
        assert_eq!(
            partial.event_name,
            "response.image_generation_call.partial_image"
        );
        assert!(partial.is_partial_image);
        assert_eq!(
            partial.client_visible_event,
            Some("image_generation.partial_image")
        );

        let completed = parse_openai_image_sync_sse_frame(
            b"data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
        )
        .expect("completed frame");
        assert_eq!(completed.event_name, "response.completed");
        assert!(completed.is_completed);
        assert_eq!(
            completed.client_visible_event,
            Some("image_generation.completed")
        );
    }

    #[test]
    fn openai_image_sync_progress_tracks_upstream_stream_without_json_heartbeat_wrapper() {
        let plan = test_openai_image_plan(false);
        let report_context = json!({"upstream_is_stream": true});

        assert!(should_track_openai_image_sync_upstream_sse(
            OPENAI_IMAGE_SYNC_PLAN_KIND,
            &plan,
            Some(&report_context),
        ));
        assert!(!should_enable_openai_image_sync_json_heartbeat(
            OPENAI_IMAGE_SYNC_PLAN_KIND,
            &plan,
            Some(&report_context),
        ));
    }

    #[test]
    fn openai_image_sync_progress_ignores_non_stream_upstream() {
        let plan = test_openai_image_plan(false);
        let report_context = json!({"upstream_is_stream": false});

        assert!(!should_track_openai_image_sync_upstream_sse(
            OPENAI_IMAGE_SYNC_PLAN_KIND,
            &plan,
            Some(&report_context),
        ));
        assert!(!should_enable_openai_image_sync_json_heartbeat(
            OPENAI_IMAGE_SYNC_PLAN_KIND,
            &plan,
            Some(&report_context),
        ));
    }
}
