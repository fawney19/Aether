use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::io::Error as IoError;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use aether_ai_serving::{AiAttemptExecutionOutcome, AiAttemptRetryScope};
use aether_contracts::{
    ExecutionPlan, ExecutionResponseObservation, ExecutionStreamTerminalSummary,
    ExecutionTelemetry, StandardizedUsage, StreamFrame, StreamFramePayload,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, UpsertRequestCandidateRecord,
};
use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use aether_scheduler_core::{
    parse_request_candidate_report_context, SchedulerRequestCandidateStatusUpdate,
};
use aether_usage_runtime::{
    build_lifecycle_usage_seed, build_stream_terminal_usage_payload_seed,
    build_sync_terminal_usage_payload_seed, build_terminal_usage_context_seed, LifecycleUsageSeed,
    SyncTerminalUsagePayloadSeed, TerminalUsageContextSeed, UsageRequestRecordLevel,
    DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
};
use async_stream::stream;
use axum::body::{Body, Bytes};
use axum::http::Response;
use base64::Engine as _;
use futures_util::stream::{self as futures_stream, BoxStream};
use futures_util::{Stream, StreamExt, TryStreamExt};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::{debug, info, warn};

use super::commit_policy::{
    anthropic_error_status_code, find_sse_record_boundary, StreamCommitGate, StreamCommitPolicy,
    StreamPrecommitObservation,
};
use super::error::{
    build_synthetic_non_success_stream_error_body, collect_error_body, decode_stream_error_body,
    inspect_prefetched_stream_body, read_next_frame,
    should_synthesize_non_success_stream_error_body,
    stream_client_error_status_code_for_upstream_status, synthetic_error_response_headers,
    StreamPrefetchInspection,
};
#[path = "execution_failures.rs"]
mod execution_failures;
use self::execution_failures::{
    build_stream_failure_from_execution_error, build_stream_failure_from_provider_error_body,
    build_stream_failure_report, build_stream_retryable_upstream_failure_report,
    build_stream_transport_failure_report, handle_prefetch_provider_private_stream_error,
    handle_prefetch_stream_failure, submit_midstream_stream_failure, StreamFailureReport,
};
use crate::ai_serving::api::{
    extract_provider_private_stream_error_body, maybe_bridge_standard_sync_json_to_stream,
    maybe_build_provider_private_stream_normalizer, maybe_build_stream_response_rewriter,
    normalize_provider_private_report_context, StreamingStandardTerminalObserver,
};
use crate::ai_serving::is_openai_responses_family_format;
use crate::api::response::{
    attach_control_metadata_headers, build_client_response, build_client_response_from_parts,
};
use crate::clock::current_unix_ms as current_request_candidate_unix_ms;
use crate::constants::{CONTROL_CANDIDATE_ID_HEADER, CONTROL_REQUEST_ID_HEADER};
use crate::control::GatewayControlDecision;
use crate::execution_runtime::build_direct_execution_frame_stream;
use crate::execution_runtime::chatgpt_web_image::maybe_execute_chatgpt_web_image_stream;
use crate::execution_runtime::grok::maybe_execute_grok_stream;
use crate::execution_runtime::kiro_cache::{
    billed_input_tokens as kiro_billed_input_tokens, build_kiro_prompt_cache_profile,
    compute_kiro_prompt_cache_usage, estimate_kiro_prompt_input_tokens,
    kiro_simulated_cache_enabled_from_provider_config,
    kiro_simulated_cache_enabled_from_report_context, KiroPromptCacheUsage,
    KIRO_SIMULATED_CACHE_ENABLED_CONTEXT_FIELD,
};
use crate::execution_runtime::kiro_web_search::maybe_execute_kiro_web_search_stream;
use crate::execution_runtime::oauth_retry::refresh_oauth_plan_auth_for_retry;
#[cfg(test)]
use crate::execution_runtime::remote_compat::post_stream_plan_to_remote_execution_runtime;
use crate::execution_runtime::submission::{
    resolve_core_error_background_report_kind, resolve_local_sync_error_status_code,
    strip_utf8_bom_and_ws, submit_local_core_error_or_sync_finalize,
};
use crate::execution_runtime::transport::{
    execute_stream_plan_via_local_tunnel, format_hyper_error_chain, format_upstream_request_error,
    format_wreq_upstream_request_error, record_manual_proxy_request_failure,
    record_manual_proxy_request_success, record_manual_proxy_stream_error,
    stream_first_byte_timeout_message, DirectSyncExecutionRuntime, DirectUpstreamResponse,
    DirectUpstreamStreamExecution, ExecutionRuntimeTransportError,
};
use crate::execution_runtime::windsurf::maybe_execute_windsurf_stream;
use crate::execution_runtime::{
    ai_attempt_retry_scope_from_failure_disposition, apply_endpoint_response_header_rules,
    attach_provider_response_headers_to_report_context, local_failover_response_text,
    resolve_core_stream_direct_finalize_report_kind,
    resolve_core_stream_error_finalize_report_kind,
    resolve_local_candidate_failover_analysis_stream, should_fallback_to_control_stream,
    should_retry_next_local_candidate_stream, LocalFailoverDecision,
};
use crate::execution_runtime::{
    MAX_ERROR_BODY_BYTES, MAX_STREAM_PREFETCH_BYTES, MAX_STREAM_PREFETCH_FRAMES,
};
use crate::log_ids::short_request_id;
use crate::orchestration::{
    apply_local_execution_effect, build_local_error_flow_metadata, classify_failure_disposition,
    cyber_continue_failover_enabled, spawn_local_oauth_success_effect,
    trace_upstream_response_body, with_error_flow_report_context,
    with_upstream_response_report_context, FailureDisposition, FailureTokenAction,
    LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect, LocalAttemptFailureEffect,
    LocalExecutionEffect, LocalExecutionEffectContext, LocalFailoverAnalysis,
    LocalHealthFailureEffect, LocalHealthSuccessEffect, LocalOAuthInvalidationEffect,
    LocalOAuthSuccessEffect, LocalPoolErrorEffect,
};
use crate::provider_pool_demand::{
    acquire_provider_pool_in_flight_guard, ProviderPoolInFlightGuard,
};
use crate::request_candidate_runtime::{
    ensure_execution_request_candidate_slot, persist_local_request_candidate_status_record,
    record_local_request_candidate_status, record_local_request_candidate_status_snapshot,
    snapshot_local_request_candidate_status, try_enqueue_local_request_candidate_status_snapshot,
    LocalRequestCandidateStatusSnapshot,
};
use crate::request_diagnostics::{
    attach_current_request_diagnostics_to_report_context,
    attach_request_diagnostics_and_candidate_start_timing_to_report_context,
    current_request_diagnostics, RequestDiagnostics,
};
use crate::stage_metrics::{
    attach_stage_trace_to_report_context, observe_gateway_stage_ms, observe_gateway_stage_trace_ms,
    record_stream_pre_first_byte_spawn, RequestStageTrace,
};
use crate::usage::submit_stream_report;
use crate::usage::{GatewayStreamReportRequest, GatewaySyncReportRequest};
use crate::{
    AppState, GatewayError, GEMINI_FILES_DOWNLOAD_PLAN_KIND, OPENAI_VIDEO_CONTENT_PLAN_KIND,
};

const OPENAI_IMAGE_STREAM_PLAN_KIND: &str = "openai_image_stream";
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const SSE_KEEPALIVE_BYTES: &[u8] = b": aether-keepalive\n\n";
const SSE_CONTROL_FILTER_MAX_BUFFER_BYTES: usize = 1024 * 1024;
const SSE_TERMINAL_DETECTOR_MAX_LINE_BYTES: usize = 1024 * 1024;
const SSE_TERMINAL_DETECTOR_MAX_RECORD_BYTES: usize = SSE_TERMINAL_DETECTOR_MAX_LINE_BYTES;
const PROVIDER_STREAM_ERROR_INSPECTION_MAX_BYTES: usize = SSE_TERMINAL_DETECTOR_MAX_LINE_BYTES;
const BASIC_STREAM_BODY_ANALYSIS_LIMIT_BYTES: usize = 5 * 1024 * 1024;
const STREAM_IDLE_LOG_INTERVAL: Duration = Duration::from_secs(60);
const STREAM_IDLE_LOG_INTERVAL_MS: u64 = 60_000;
const REWRITTEN_STREAM_PREFETCH_TIMEOUT: Duration = Duration::from_millis(750);
const OAUTH_ERROR_PREFETCH_MAX_WAIT: Duration = Duration::from_millis(750);
const ANTHROPIC_POST_STOP_DRAIN_MAX_WAIT: Duration = Duration::from_millis(250);
const ANTHROPIC_POST_STOP_DRAIN_MAX_FRAMES: usize = 8;
const ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES: usize = 64 * 1024;
const POST_STOP_FRAME_READ_BUDGET_INACTIVE: usize = usize::MAX;
const POST_STOP_MAX_EMPTY_CHUNKS_PER_POLL: usize = 32;
const DEFAULT_DIRECT_PASSTHROUGH_CHANNEL_CAPACITY: usize = 16;
const MAX_DIRECT_PASSTHROUGH_CHANNEL_CAPACITY: usize = 1024;
const DIRECT_PASSTHROUGH_CHANNEL_CAPACITY_ENV: &str =
    "AETHER_GATEWAY_DIRECT_PASSTHROUGH_CHANNEL_CAPACITY";
const DIRECT_PASSTHROUGH_MODE_ENV: &str = "AETHER_GATEWAY_DIRECT_PASSTHROUGH_MODE";

/// Retains the incomplete tail needed to recognize provider error events split across transport
/// chunks without retaining an unbounded copy of the stream.
#[derive(Default)]
struct ProviderStreamErrorInspection {
    buffered: Vec<u8>,
}

impl ProviderStreamErrorInspection {
    fn observe(&mut self, report_context: Option<&Value>, chunk: &[u8]) -> Option<Value> {
        if chunk.is_empty() {
            return None;
        }
        if let Some(error_body) = extract_provider_private_stream_error_body(report_context, chunk)
        {
            return Some(error_body);
        }

        self.append_rolling(chunk);
        let error_body = extract_provider_private_stream_error_body(report_context, &self.buffered);
        if error_body.is_none() {
            self.trim_completed_sse_events();
        }
        error_body
    }

    fn append_rolling(&mut self, chunk: &[u8]) {
        if chunk.len() >= PROVIDER_STREAM_ERROR_INSPECTION_MAX_BYTES {
            self.buffered.clear();
            self.buffered.extend_from_slice(
                &chunk[chunk.len() - PROVIDER_STREAM_ERROR_INSPECTION_MAX_BYTES..],
            );
            return;
        }

        let overflow = self
            .buffered
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(PROVIDER_STREAM_ERROR_INSPECTION_MAX_BYTES);
        if overflow > 0 {
            self.buffered.drain(..overflow);
        }
        self.buffered.extend_from_slice(chunk);
    }

    fn trim_completed_sse_events(&mut self) {
        let Ok(text) = std::str::from_utf8(&self.buffered) else {
            return;
        };
        if !text.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("event:") || line.starts_with("data:") || line.starts_with(':')
        }) {
            return;
        }

        let lf_end = self
            .buffered
            .windows(2)
            .rposition(|window| window == b"\n\n")
            .map(|index| index + 2);
        let crlf_end = self
            .buffered
            .windows(4)
            .rposition(|window| window == b"\r\n\r\n")
            .map(|index| index + 4);
        if let Some(event_end) = lf_end.into_iter().chain(crlf_end).max() {
            self.buffered.drain(..event_end);
        }
    }
}

struct StageElapsedGuard {
    stage: &'static str,
    started_at: Instant,
}

#[derive(Debug)]
enum InProcessStreamExecutionError {
    Transport(ExecutionRuntimeTransportError),
    Gateway(GatewayError),
}

impl From<ExecutionRuntimeTransportError> for InProcessStreamExecutionError {
    fn from(error: ExecutionRuntimeTransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<GatewayError> for InProcessStreamExecutionError {
    fn from(error: GatewayError) -> Self {
        Self::Gateway(error)
    }
}

impl StageElapsedGuard {
    fn from_started_at(stage: &'static str, started_at: Instant) -> Self {
        Self { stage, started_at }
    }
}

fn report_context_with_stage_trace(
    report_context: Option<Value>,
    mut stage_trace: RequestStageTrace,
    stream_started_at: Instant,
    terminal_telemetry: Option<&ExecutionTelemetry>,
) -> Option<Value> {
    stage_trace.observe("stream_total", stream_elapsed_ms_since(stream_started_at));
    let fallback_elapsed_ms = terminal_telemetry.and_then(|telemetry| telemetry.ttfb_ms);
    attach_stage_trace_to_report_context(
        report_context,
        stage_trace.into_metadata_value(fallback_elapsed_ms),
    )
}

fn report_context_with_request_diagnostics(
    report_context: Option<Value>,
    diagnostics: Option<&Arc<RequestDiagnostics>>,
    candidate_started_at: Instant,
    terminal_telemetry: Option<&ExecutionTelemetry>,
) -> Option<Value> {
    attach_request_diagnostics_and_candidate_start_timing_to_report_context(
        report_context,
        diagnostics,
        Some(candidate_started_at),
        terminal_telemetry.and_then(|telemetry| telemetry.ttfb_ms),
    )
}

fn request_accepted_elapsed_ms(diagnostics: Option<&Arc<RequestDiagnostics>>) -> Option<u64> {
    diagnostics.and_then(|diagnostics| {
        diagnostics
            .request_accepted_at()
            .map(|accepted_at| accepted_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64)
    })
}

fn observe_request_accepted_stage_trace_ms(
    trace: &mut RequestStageTrace,
    diagnostics: Option<&Arc<RequestDiagnostics>>,
    stage: &'static str,
) {
    if let Some(elapsed_ms) = request_accepted_elapsed_ms(diagnostics) {
        observe_gateway_stage_trace_ms(trace, stage, elapsed_ms);
    }
}

impl Drop for StageElapsedGuard {
    fn drop(&mut self) {
        observe_gateway_stage_ms(self.stage, self.started_at.elapsed().as_millis() as u64);
    }
}

fn direct_passthrough_channel_capacity() -> usize {
    std::env::var(DIRECT_PASSTHROUGH_CHANNEL_CAPACITY_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_DIRECT_PASSTHROUGH_CHANNEL_CAPACITY)
        .clamp(1, MAX_DIRECT_PASSTHROUGH_CHANNEL_CAPACITY)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectPassthroughMode {
    Inline,
    Legacy,
}

fn direct_passthrough_mode() -> DirectPassthroughMode {
    std::env::var(DIRECT_PASSTHROUGH_MODE_ENV)
        .ok()
        .as_deref()
        .map(parse_direct_passthrough_mode)
        .unwrap_or(DirectPassthroughMode::Inline)
}

fn stream_body_buffer_limit_for_record_level(record_level: UsageRequestRecordLevel) -> usize {
    match record_level {
        UsageRequestRecordLevel::Basic => BASIC_STREAM_BODY_ANALYSIS_LIMIT_BYTES,
        UsageRequestRecordLevel::Full => DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
    }
}

async fn resolve_stream_body_buffer_limit(state: &AppState) -> usize {
    if !state.usage_runtime.is_enabled() {
        return BASIC_STREAM_BODY_ANALYSIS_LIMIT_BYTES;
    }

    match state
        .usage_runtime
        .body_capture_policy_for(state.usage_lifecycle_data_state().as_ref())
        .await
    {
        Ok(policy) => stream_body_buffer_limit_for_record_level(policy.record_level),
        Err(error) => {
            warn!(
                event_name = "stream_body_capture_policy_read_failed",
                log_type = "ops",
                error = %error,
                fallback = "full",
                "gateway could not resolve stream body capture policy"
            );
            DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES
        }
    }
}

fn parse_direct_passthrough_mode(value: &str) -> DirectPassthroughMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "legacy" | "pump" | "mpsc" => DirectPassthroughMode::Legacy,
        _ => DirectPassthroughMode::Inline,
    }
}

fn build_sync_terminal_usage_seeds(
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewaySyncReportRequest,
) -> (TerminalUsageContextSeed, SyncTerminalUsagePayloadSeed) {
    let report_context_with_diagnostics =
        attach_current_request_diagnostics_to_report_context(report_context);
    let context_seed = build_terminal_usage_context_seed(
        plan,
        report_context_with_diagnostics.as_ref().or(report_context),
    );
    let payload_seed = build_sync_terminal_usage_payload_seed(payload);
    (context_seed, payload_seed)
}

async fn record_sync_terminal_usage_with_handoff(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewaySyncReportRequest,
) {
    record_sync_terminal_usage_with_handoff_after_spawn(
        state,
        plan,
        report_context,
        payload,
        std::future::ready(()),
    )
    .await;
}

async fn record_sync_terminal_usage_with_handoff_after_spawn<F>(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewaySyncReportRequest,
    before_dispatch: F,
) where
    F: Future<Output = ()> + Send + 'static,
{
    crate::execution_runtime::mark_stream_candidate_watchdog_terminal_started();
    // Capture request task-local diagnostics before handing the work to a spawned task. Tokio
    // task-local values do not propagate across spawn boundaries.
    let (context_seed, payload_seed) =
        build_sync_terminal_usage_seeds(plan, report_context, payload);
    let state = state.clone();
    let task = tokio::spawn(async move {
        before_dispatch.await;
        state
            .usage_runtime
            .record_sync_terminal(
                state.usage_lifecycle_data_state().as_ref(),
                context_seed,
                payload_seed,
            )
            .await;
    });
    if let Err(err) = task.await {
        warn!(
            event_name = "sync_terminal_usage_handoff_failed",
            log_type = "ops",
            error = %err,
            "gateway sync terminal usage handoff task failed"
        );
    }
}

fn build_stream_sync_payload(
    trace_id: &str,
    report_kind: String,
    report_context: Option<Value>,
    status_code: u16,
    headers: BTreeMap<String, String>,
    body_json: Option<Value>,
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

#[allow(clippy::too_many_arguments)]
fn build_stream_error_sync_payload(
    trace_id: &str,
    report_kind: String,
    report_context: Option<Value>,
    upstream_status_code: u16,
    provider_headers: BTreeMap<String, String>,
    provider_body_json: Option<Value>,
    provider_body_base64: Option<String>,
    client_headers: BTreeMap<String, String>,
    client_body_json: Option<Value>,
    telemetry: Option<ExecutionTelemetry>,
) -> GatewaySyncReportRequest {
    let client_status_code =
        stream_client_error_status_code_for_upstream_status(upstream_status_code);
    let mut report_context = report_context;
    if client_status_code != upstream_status_code || client_headers != provider_headers {
        let mut object = match report_context {
            Some(Value::Object(object)) => object,
            Some(other) => serde_json::Map::from_iter([("seed".to_string(), other)]),
            None => serde_json::Map::new(),
        };
        object.insert(
            "client_response_status_code".to_string(),
            Value::from(client_status_code),
        );
        object.insert(
            "client_response_headers".to_string(),
            serde_json::to_value(client_headers).unwrap_or(Value::Null),
        );
        report_context = Some(Value::Object(object));
    }

    GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind,
        report_context,
        status_code: upstream_status_code,
        headers: provider_headers,
        body_json: provider_body_json,
        client_body_json,
        body_base64: provider_body_base64,
        telemetry,
    }
}

async fn record_stream_terminal_usage(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewayStreamReportRequest,
    cancelled: bool,
) {
    crate::execution_runtime::mark_stream_candidate_watchdog_terminal_started();
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_stream_terminal_usage_payload_seed(payload);
    state
        .usage_runtime
        .record_stream_terminal(
            state.usage_lifecycle_data_state().as_ref(),
            context_seed,
            payload_seed,
            cancelled,
        )
        .await;
}

async fn record_stream_admission_timeout_candidate_failure(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    candidate_started_unix_ms: u64,
    error: &GatewayError,
) {
    let status_code = 429;
    let error_type = "gateway_admission_timeout";
    let error_message = match error {
        GatewayError::AdmissionTimeout {
            gate,
            queue_budget_ms,
            ..
        } => format!("gateway admission gate {gate} timed out after {queue_budget_ms}ms"),
        other => format!("{other:?}"),
    };
    let terminal_unix_ms = current_request_candidate_unix_ms();
    let latency_ms = terminal_unix_ms.saturating_sub(candidate_started_unix_ms);
    record_local_request_candidate_status(
        state,
        plan,
        report_context,
        SchedulerRequestCandidateStatusUpdate {
            status: RequestCandidateStatus::Failed,
            status_code: Some(status_code),
            error_type: Some(error_type.to_string()),
            error_message: Some(error_message.clone()),
            latency_ms: Some(latency_ms),
            started_at_unix_ms: Some(candidate_started_unix_ms),
            finished_at_unix_ms: Some(terminal_unix_ms),
        },
    )
    .await;
}

fn build_stream_body_capture(
    body: &[u8],
    truncated: bool,
) -> (Option<String>, Option<UsageBodyCaptureState>) {
    let body_base64 =
        (!body.is_empty()).then(|| base64::engine::general_purpose::STANDARD.encode(body));
    let body_state = Some(if truncated {
        UsageBodyCaptureState::Truncated
    } else if body.is_empty() {
        UsageBodyCaptureState::None
    } else {
        UsageBodyCaptureState::Inline
    });
    (body_base64, body_state)
}

fn wrap_non_json_binary_stream_error_for_client(
    plan_kind: &str,
    headers: &BTreeMap<String, String>,
    error_body: &[u8],
) -> Result<Option<Value>, GatewayError> {
    let content_type = headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if content_type.starts_with("application/json") {
        return Ok(None);
    }

    let body = match plan_kind {
        GEMINI_FILES_DOWNLOAD_PLAN_KIND => json!({
            "error": String::from_utf8_lossy(error_body).to_string(),
        }),
        OPENAI_VIDEO_CONTENT_PLAN_KIND => json!({
            "error": {
                "type": "upstream_error",
                "message": "Video not available",
            }
        }),
        _ => return Ok(None),
    };
    Ok(Some(body))
}

fn with_stream_error_trace_context(
    report_context: Option<&Value>,
    status_code: u16,
    headers: &BTreeMap<String, String>,
    body_json: Option<&Value>,
    body_bytes: &[u8],
    response_text: Option<&str>,
    local_failover_analysis: crate::orchestration::LocalFailoverAnalysis,
) -> Option<Value> {
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

#[allow(clippy::too_many_arguments)] // stream report payload assembly mirrors runtime state
fn build_stream_usage_payload(
    trace_id: String,
    report_kind: String,
    report_context: Option<Value>,
    status_code: u16,
    headers: BTreeMap<String, String>,
    provider_body: &[u8],
    provider_body_truncated: bool,
    client_body: &[u8],
    client_body_truncated: bool,
    terminal_summary: Option<ExecutionStreamTerminalSummary>,
    telemetry: Option<ExecutionTelemetry>,
) -> GatewayStreamReportRequest {
    let (provider_body_base64, provider_body_state) =
        build_stream_body_capture(provider_body, provider_body_truncated);
    let (client_body_base64, client_body_state) =
        build_stream_body_capture(client_body, client_body_truncated);
    GatewayStreamReportRequest {
        trace_id,
        report_kind,
        report_context,
        status_code,
        headers,
        provider_body_base64,
        provider_body_state,
        client_body_base64,
        client_body_state,
        terminal_summary,
        telemetry,
    }
}

fn seed_kiro_report_context_input_tokens(plan: &ExecutionPlan, report_context: &mut Option<Value>) {
    if !plan
        .provider_name
        .as_deref()
        .is_some_and(|provider_name| provider_name.eq_ignore_ascii_case("Kiro"))
    {
        return;
    }

    let Some(context) = report_context.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if context
        .get("input_tokens")
        .and_then(Value::as_u64)
        .is_some_and(|input_tokens| input_tokens > 0)
    {
        return;
    }

    let Some(original_request_body) = context.get("original_request_body").cloned() else {
        return;
    };
    let estimated_input_tokens = estimate_kiro_prompt_input_tokens(&original_request_body);
    context.insert(
        "input_tokens".to_string(),
        Value::from(estimated_input_tokens),
    );
}

async fn seed_kiro_simulated_cache_enabled(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: &mut Option<Value>,
) {
    if !plan
        .provider_name
        .as_deref()
        .is_some_and(|provider_name| provider_name.eq_ignore_ascii_case("Kiro"))
    {
        return;
    }

    let enabled = match state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&plan.provider_id))
        .await
    {
        Ok(providers) => providers
            .iter()
            .find(|provider| provider.id == plan.provider_id)
            .filter(|provider| provider.provider_type.eq_ignore_ascii_case("kiro"))
            .is_some_and(|provider| {
                kiro_simulated_cache_enabled_from_provider_config(provider.config.as_ref())
            }),
        Err(err) => {
            warn!(
                event_name = "kiro_simulated_cache_config_read_failed",
                log_type = "event",
                request_id = %plan.request_id,
                provider_id = %plan.provider_id,
                error = ?err,
                "failed to read Kiro simulated cache provider config; defaulting disabled"
            );
            false
        }
    };

    let Some(context) = report_context.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if enabled {
        context.insert(
            KIRO_SIMULATED_CACHE_ENABLED_CONTEXT_FIELD.to_string(),
            Value::Bool(true),
        );
    } else {
        context.remove(KIRO_SIMULATED_CACHE_ENABLED_CONTEXT_FIELD);
    }
}

async fn seed_kiro_report_context_prompt_cache_usage(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: &mut Option<Value>,
) {
    if !plan
        .provider_name
        .as_deref()
        .is_some_and(|provider_name| provider_name.eq_ignore_ascii_case("Kiro"))
    {
        return;
    }

    let simulated_cache_enabled =
        kiro_simulated_cache_enabled_from_report_context(report_context.as_ref());
    let Some(context) = report_context.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if context
        .get("kiro_web_search_mcp")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return;
    }
    if !simulated_cache_enabled {
        return;
    }
    if kiro_cache_usage_from_context_object(context).is_some() {
        return;
    }

    let Some(original_request_body) = context.get("original_request_body").cloned() else {
        return;
    };
    let input_tokens = context
        .get("input_tokens")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            let estimated = estimate_kiro_prompt_input_tokens(&original_request_body);
            context.insert("input_tokens".to_string(), Value::from(estimated));
            estimated
        });
    let Some(profile) = build_kiro_prompt_cache_profile(&original_request_body, input_tokens)
    else {
        return;
    };

    let cache_usage = compute_kiro_prompt_cache_usage(
        state.runtime_state(),
        kiro_stream_cache_credential_id(plan),
        &profile,
    )
    .await;
    if cache_usage.cache_creation_input_tokens == 0 && cache_usage.cache_read_input_tokens == 0 {
        return;
    }
    context.insert(
        "cache_creation_input_tokens".to_string(),
        Value::from(cache_usage.cache_creation_input_tokens),
    );
    context.insert(
        "cache_read_input_tokens".to_string(),
        Value::from(cache_usage.cache_read_input_tokens),
    );
}

fn kiro_stream_cache_credential_id(plan: &ExecutionPlan) -> String {
    format!("{}:{}:{}", plan.provider_id, plan.endpoint_id, plan.key_id)
}

fn kiro_cache_usage_from_context_object(
    context: &serde_json::Map<String, Value>,
) -> Option<KiroPromptCacheUsage> {
    let cache_creation_input_tokens = context
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_input_tokens = context
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    (cache_creation_input_tokens > 0 || cache_read_input_tokens > 0).then_some(
        KiroPromptCacheUsage {
            cache_creation_input_tokens,
            cache_read_input_tokens,
        },
    )
}

fn kiro_cache_usage_from_report_context(report_context: &Value) -> Option<KiroPromptCacheUsage> {
    report_context
        .as_object()
        .and_then(kiro_cache_usage_from_context_object)
}

async fn maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    summary: &mut Option<ExecutionStreamTerminalSummary>,
) {
    if !plan
        .provider_name
        .as_deref()
        .is_some_and(|provider_name| provider_name.eq_ignore_ascii_case("Kiro"))
    {
        return;
    }

    let Some(report_context) = report_context else {
        return;
    };
    let Some(original_request_body) = report_context.get("original_request_body") else {
        return;
    };
    let simulated_cache_enabled =
        kiro_simulated_cache_enabled_from_report_context(Some(report_context));

    let summary = summary.get_or_insert_with(ExecutionStreamTerminalSummary::default);
    let usage = summary
        .standardized_usage
        .get_or_insert_with(StandardizedUsage::new);
    let estimated_input_tokens = report_context
        .get("input_tokens")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            let estimated_input_tokens = estimate_kiro_prompt_input_tokens(original_request_body);
            if estimated_input_tokens > 0 {
                estimated_input_tokens
            } else {
                usage.input_tokens.max(0) as u64
            }
        });

    if !simulated_cache_enabled {
        usage.cache_creation_tokens = 0;
        usage.cache_read_tokens = 0;
        if usage.input_tokens <= 0 {
            usage.input_tokens = estimated_input_tokens as i64;
        }
        return;
    }

    if let Some(cache_usage) = kiro_cache_usage_from_report_context(report_context) {
        usage.input_tokens = kiro_billed_input_tokens(estimated_input_tokens, cache_usage) as i64;
        usage.cache_creation_tokens = cache_usage.cache_creation_input_tokens as i64;
        usage.cache_read_tokens = cache_usage.cache_read_input_tokens as i64;
        return;
    }

    if usage.cache_creation_tokens > 0 || usage.cache_read_tokens > 0 {
        if usage.input_tokens <= 0 {
            usage.input_tokens = kiro_billed_input_tokens(
                estimated_input_tokens,
                KiroPromptCacheUsage {
                    cache_creation_input_tokens: usage.cache_creation_tokens.max(0) as u64,
                    cache_read_input_tokens: usage.cache_read_tokens.max(0) as u64,
                },
            ) as i64;
        }
        return;
    }

    if usage.input_tokens <= 0 {
        usage.input_tokens = estimated_input_tokens as i64;
    }

    let Some(profile) =
        build_kiro_prompt_cache_profile(original_request_body, estimated_input_tokens)
    else {
        return;
    };

    let cache_usage = compute_kiro_prompt_cache_usage(
        state.runtime_state(),
        kiro_stream_cache_credential_id(plan),
        &profile,
    )
    .await;
    if cache_usage.cache_creation_input_tokens == 0 && cache_usage.cache_read_input_tokens == 0 {
        return;
    }

    let billed_input_tokens = kiro_billed_input_tokens(estimated_input_tokens, cache_usage);
    usage.input_tokens = billed_input_tokens as i64;
    usage.cache_creation_tokens = cache_usage.cache_creation_input_tokens as i64;
    usage.cache_read_tokens = cache_usage.cache_read_input_tokens as i64;
}

fn append_stream_capture_bytes(
    buffer: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
    truncated: &mut bool,
) {
    if chunk.is_empty() || max_bytes == 0 {
        return;
    }
    if buffer.len() >= max_bytes {
        *truncated = true;
        return;
    }
    let remaining = max_bytes - buffer.len();
    let keep_len = remaining.min(chunk.len());
    buffer.extend_from_slice(&chunk[..keep_len]);
    if keep_len < chunk.len() {
        *truncated = true;
    }
}

fn observe_stream_usage_bytes(
    observer: &mut StreamingStandardTerminalObserver,
    report_context: &Value,
    buffered: &mut Vec<u8>,
    chunk: &[u8],
) {
    if chunk.is_empty()
        || observer
            .latest_summary()
            .and_then(|summary| summary.parser_error.as_deref())
            .is_some()
    {
        return;
    }

    let mut remaining = chunk;
    while !remaining.is_empty() {
        let line_part_len = remaining
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(remaining.len(), |index| index + 1);
        if buffered.len().saturating_add(line_part_len) > SSE_TERMINAL_DETECTOR_MAX_LINE_BYTES {
            observer.disable_with_error(format!(
                "stream usage event exceeded {SSE_TERMINAL_DETECTOR_MAX_LINE_BYTES} bytes"
            ));
            buffered.clear();
            return;
        }
        buffered.extend_from_slice(&remaining[..line_part_len]);
        remaining = &remaining[line_part_len..];
        if buffered.last() == Some(&b'\n') {
            let line = std::mem::take(buffered);
            if let Err(err) = observer.push_line(report_context, line) {
                observer.disable_with_error(err.to_string());
                buffered.clear();
                return;
            }
        }
    }
}

fn finalize_stream_usage_observer(
    observer: &mut Option<StreamingStandardTerminalObserver>,
    report_context: Option<&Value>,
    buffered: &mut Vec<u8>,
) -> Option<ExecutionStreamTerminalSummary> {
    let (Some(observer), Some(report_context)) = (observer.as_mut(), report_context) else {
        return None;
    };

    if !buffered.is_empty() {
        let line = std::mem::take(buffered);
        if let Err(err) = observer.push_line(report_context, line) {
            observer.disable_with_error(err.to_string());
        }
    }

    match observer.finish(report_context) {
        Ok(summary) => summary,
        Err(err) => {
            observer.disable_with_error(err.to_string());
            observer.latest_summary().cloned()
        }
    }
}

fn merge_stream_terminal_summary(
    mut current: Option<ExecutionStreamTerminalSummary>,
    observed: Option<ExecutionStreamTerminalSummary>,
) -> Option<ExecutionStreamTerminalSummary> {
    let Some(observed) = observed else {
        return current;
    };

    let Some(current_summary) = current.as_mut() else {
        return Some(observed);
    };

    if should_replace_stream_usage(
        current_summary.standardized_usage.as_ref(),
        observed.standardized_usage.as_ref(),
    ) {
        current_summary.standardized_usage = observed.standardized_usage;
    }
    if current_summary.finish_reason.is_none() {
        current_summary.finish_reason = observed.finish_reason;
    }
    if current_summary.response_id.is_none() {
        current_summary.response_id = observed.response_id;
    }
    if current_summary.model.is_none() {
        current_summary.model = observed.model;
    }
    if observed.provider_actual_service_tier.is_some() {
        current_summary.provider_actual_service_tier = observed.provider_actual_service_tier;
    }
    current_summary.observed_finish |= observed.observed_finish;
    current_summary.unknown_event_count = current_summary
        .unknown_event_count
        .saturating_add(observed.unknown_event_count);
    if current_summary.parser_error.is_none() {
        current_summary.parser_error = observed.parser_error;
    }

    current
}

fn should_replace_stream_usage(
    current: Option<&aether_contracts::StandardizedUsage>,
    observed: Option<&aether_contracts::StandardizedUsage>,
) -> bool {
    let Some(observed) = observed else {
        return false;
    };
    let Some(current) = current else {
        return true;
    };

    observed.is_more_complete_than(current)
}

fn stream_terminal_summary_missing_observed_finish(
    summary: Option<&ExecutionStreamTerminalSummary>,
) -> bool {
    summary.is_some_and(|summary| {
        !summary.observed_finish
            && !summary
                .standardized_usage
                .as_ref()
                .is_some_and(StandardizedUsage::has_token_signal)
    })
}

fn stream_report_context_format_field<'a>(
    report_context: Option<&'a Value>,
    field: &str,
) -> Option<&'a str> {
    report_context
        .and_then(Value::as_object)
        .and_then(|object| object.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn stream_requires_observed_terminal_event(
    provider_api_format: &str,
    report_context: Option<&Value>,
) -> bool {
    is_openai_responses_family_format(provider_api_format)
        || [
            "provider_stream_event_api_format",
            "provider_stream_api_format",
            "provider_api_format",
        ]
        .into_iter()
        .filter_map(|field| stream_report_context_format_field(report_context, field))
        .any(is_openai_responses_family_format)
}

fn stream_terminal_summary_missing_observed_finish_with_requirement(
    summary: Option<&ExecutionStreamTerminalSummary>,
    requires_observed_terminal_event: bool,
) -> bool {
    if !requires_observed_terminal_event {
        return stream_terminal_summary_missing_observed_finish(summary);
    }

    summary.is_some_and(|summary| !summary.observed_finish)
}

fn ensure_stream_terminal_summary_for_missing_observed_finish(
    summary: &mut Option<ExecutionStreamTerminalSummary>,
    requires_observed_terminal_event: bool,
) {
    if !requires_observed_terminal_event {
        return;
    }

    let summary = summary.get_or_insert_with(ExecutionStreamTerminalSummary::default);
    if !summary.observed_finish && summary.parser_error.is_none() {
        summary.parser_error =
            Some("execution runtime stream ended before provider terminal event".to_string());
    }
}

fn stream_terminal_summary_represents_failure_with_requirement(
    summary: Option<&ExecutionStreamTerminalSummary>,
    requires_observed_terminal_event: bool,
) -> bool {
    summary.is_some_and(|summary| {
        summary.parser_error.is_some()
            || stream_terminal_summary_missing_observed_finish_with_requirement(
                Some(summary),
                requires_observed_terminal_event,
            )
    })
}

async fn execute_in_process_stream(
    state: &AppState,
    plan: &ExecutionPlan,
    trace_id: &str,
) -> Result<DirectUpstreamStreamExecution, InProcessStreamExecutionError> {
    if let Some(execution) = execute_stream_plan_via_local_tunnel(state, plan).await? {
        return Ok(execution);
    }

    let upstream_target_permit = state
        .upstream_target_admission
        .acquire(plan, trace_id)
        .await?;
    match DirectSyncExecutionRuntime::new().execute_stream(plan).await {
        Ok(mut execution) => {
            execution.upstream_target_permit = upstream_target_permit;
            record_manual_proxy_request_success(state, plan).await;
            Ok(execution)
        }
        Err(error) => {
            record_manual_proxy_request_failure(state, plan).await;
            Err(error.into())
        }
    }
}

async fn execute_in_process_stream_with_oauth_retry(
    state: &AppState,
    plan: &mut ExecutionPlan,
    trace_id: &str,
    report_context: Option<&Value>,
) -> Result<DirectUpstreamStreamExecution, InProcessStreamExecutionError> {
    let mut execution = execute_in_process_stream(state, plan, trace_id).await?;
    apply_stream_summary_report_context(&mut execution, report_context);
    let uses_oauth_credential = stream_plan_uses_oauth_credential(state, plan).await;
    let embedded_oauth_credential = execution.status_code == 200
        && plan
            .provider_api_format
            .eq_ignore_ascii_case("claude:messages")
        && uses_oauth_credential;
    let prefetched_failure = if embedded_oauth_credential {
        prefetch_direct_anthropic_stream_failure(&mut execution, plan, report_context).await
    } else {
        None
    };
    let analyzed_prefetched_failure = match prefetched_failure {
        Some(failure) => {
            Some(analyze_prefetched_stream_failure(state, plan, report_context, failure).await)
        }
        None => None,
    };
    let response_text = if let Some(failure) = analyzed_prefetched_failure.as_ref() {
        Some(failure.response_text.clone())
    } else if execution.status_code == 403 && uses_oauth_credential {
        prefetch_direct_stream_error_body(&mut execution).await
    } else if execution.status_code == 401
        && stream_plan_uses_codex_agent_identity(state, plan).await
    {
        prefetch_direct_stream_error_body(&mut execution).await
    } else {
        None
    };
    let retry_status_code = analyzed_prefetched_failure
        .as_ref()
        .map(|failure| failure.status_code)
        .unwrap_or(execution.status_code);
    let retry_requested =
        analyzed_prefetched_failure
            .as_ref()
            .map_or(execution.status_code >= 400, |failure| {
                matches!(
                    failure.disposition.token_action,
                    FailureTokenAction::ForceRefresh
                )
            });
    if retry_requested
        && uses_oauth_credential
        && refresh_oauth_plan_auth_for_retry(
            state,
            plan,
            retry_status_code,
            response_text.as_deref(),
            trace_id,
            report_context,
            Some(execution.response_observation.request_started_at_unix_ms),
            Some(&execution.response_observation.request_order_id),
        )
        .await
    {
        drop(execution);
        execution = execute_in_process_stream(state, plan, trace_id).await?;
        apply_stream_summary_report_context(&mut execution, report_context);
    }
    Ok(execution)
}

#[derive(Debug)]
struct PrefetchedStreamFailure {
    status_code: u16,
    response_text: String,
}

#[derive(Debug)]
struct AnalyzedPrefetchedStreamFailure {
    status_code: u16,
    response_text: String,
    #[allow(dead_code)]
    analysis: LocalFailoverAnalysis,
    disposition: FailureDisposition,
}

async fn analyze_prefetched_stream_failure(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    failure: PrefetchedStreamFailure,
) -> AnalyzedPrefetchedStreamFailure {
    let analysis = resolve_local_candidate_failover_analysis_stream(
        state,
        plan,
        report_context,
        failure.status_code,
        Some(failure.response_text.as_str()),
    )
    .await;
    let disposition = classify_failure_disposition(
        plan.provider_api_format.as_str(),
        analysis.classification,
        failure.status_code,
    );
    AnalyzedPrefetchedStreamFailure {
        status_code: failure.status_code,
        response_text: failure.response_text,
        analysis,
        disposition,
    }
}

async fn stream_plan_uses_oauth_credential(state: &AppState, plan: &ExecutionPlan) -> bool {
    state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
        .ok()
        .flatten()
        .as_ref()
        .is_some_and(|transport| {
            aether_provider_transport::auth::resolve_local_auth_type_for_transport_format(transport)
                .eq_ignore_ascii_case("oauth")
        })
}

async fn prefetch_direct_anthropic_stream_failure(
    execution: &mut DirectUpstreamStreamExecution,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
) -> Option<PrefetchedStreamFailure> {
    if execution.status_code != 200 {
        return None;
    }
    let normalized_stream_report_context =
        normalize_provider_private_report_context(report_context);
    let policy = StreamCommitPolicy::for_response(
        true,
        execution.headers.get("content-type").map(String::as_str),
        plan.provider_api_format.as_str(),
        plan.client_api_format.as_str(),
        maybe_build_provider_private_stream_normalizer(report_context).is_some(),
        maybe_build_stream_response_rewriter(normalized_stream_report_context.as_ref()).is_some(),
        false,
    );
    if !policy.is_native_anthropic() {
        return None;
    }

    let mut gate = StreamCommitGate::new(policy);
    let precommit_started_at = Instant::now();
    let max_wait = policy.max_precommit_wait()?;
    let mut observed_first_body = execution
        .prefetched_body
        .iter()
        .any(|item| item.as_ref().is_ok_and(|chunk| !chunk.is_empty()));
    while gate.is_uncommitted() {
        let wait = select_direct_anthropic_prefetch_wait(
            precommit_started_at,
            max_wait,
            execution.started_at,
            execution.stream_first_byte_timeout,
            observed_first_body,
            Instant::now(),
        );
        if wait.remaining.is_zero() {
            if wait.commit_on_timeout {
                gate.commit();
            }
            break;
        }
        let next_chunk = match tokio::time::timeout(
            wait.remaining,
            next_direct_upstream_response_chunk(&mut execution.response),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                if wait.commit_on_timeout {
                    gate.commit();
                }
                break;
            }
        };
        let chunk = match next_chunk {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                execution.prefetched_body.push_back(Err(error));
                break;
            }
        };
        if chunk.is_empty() {
            continue;
        }
        observed_first_body = true;
        execution.prefetched_body.push_back(Ok(chunk.clone()));
        match gate.observe_provider_bytes(&chunk) {
            StreamPrecommitObservation::Pending => {}
            StreamPrecommitObservation::Commit => break,
            StreamPrecommitObservation::UpstreamError {
                status_code,
                body_json,
            } => {
                let response_text = serde_json::to_string(&body_json)
                    .unwrap_or_else(|_| String::from_utf8_lossy(&chunk).into_owned());
                return Some(PrefetchedStreamFailure {
                    status_code,
                    response_text,
                });
            }
        }
    }
    execution.stream_precommit_committed = !gate.is_uncommitted();
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectAnthropicPrefetchWait {
    remaining: Duration,
    commit_on_timeout: bool,
}

fn select_direct_anthropic_prefetch_wait(
    precommit_started_at: Instant,
    max_precommit_wait: Duration,
    upstream_started_at: Instant,
    first_byte_timeout: Option<Duration>,
    observed_first_body: bool,
    now: Instant,
) -> DirectAnthropicPrefetchWait {
    let precommit_remaining =
        max_precommit_wait.saturating_sub(now.saturating_duration_since(precommit_started_at));
    if observed_first_body {
        return DirectAnthropicPrefetchWait {
            remaining: precommit_remaining,
            commit_on_timeout: true,
        };
    }
    let Some(first_byte_timeout) = first_byte_timeout else {
        return DirectAnthropicPrefetchWait {
            remaining: precommit_remaining,
            commit_on_timeout: true,
        };
    };
    let first_byte_remaining =
        first_byte_timeout.saturating_sub(now.saturating_duration_since(upstream_started_at));
    if first_byte_remaining <= precommit_remaining {
        DirectAnthropicPrefetchWait {
            remaining: first_byte_remaining,
            commit_on_timeout: false,
        }
    } else {
        DirectAnthropicPrefetchWait {
            remaining: precommit_remaining,
            commit_on_timeout: true,
        }
    }
}

async fn stream_plan_uses_codex_agent_identity(state: &AppState, plan: &ExecutionPlan) -> bool {
    state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
        .ok()
        .flatten()
        .as_ref()
        .is_some_and(aether_provider_transport::is_codex_agent_identity_transport)
}

async fn next_direct_upstream_response_chunk(
    response: &mut DirectUpstreamResponse,
) -> Result<Option<Bytes>, String> {
    match response {
        DirectUpstreamResponse::Reqwest(response) => response
            .chunk()
            .await
            .map_err(|err| format_upstream_request_error(&err)),
        DirectUpstreamResponse::HyperH2c(response) => loop {
            let Some(frame) = response.body_mut().frame().await else {
                return Ok(None);
            };
            let frame = frame.map_err(|err| format_hyper_error_chain(&err))?;
            if let Ok(chunk) = frame.into_data() {
                return Ok(Some(chunk));
            }
        },
        DirectUpstreamResponse::BrowserWreq(response) => response
            .chunk()
            .await
            .map_err(|err| format_wreq_upstream_request_error(&err)),
        DirectUpstreamResponse::LocalTunnel(response) => response.next_chunk().await,
    }
}

async fn prefetch_direct_stream_error_body(
    execution: &mut DirectUpstreamStreamExecution,
) -> Option<String> {
    let prefetch_started_at = Instant::now();
    let mut inspected = Vec::with_capacity(MAX_ERROR_BODY_BYTES);
    let mut fully_buffered = false;
    while inspected.len() < MAX_ERROR_BODY_BYTES {
        let remaining = OAUTH_ERROR_PREFETCH_MAX_WAIT.saturating_sub(prefetch_started_at.elapsed());
        if remaining.is_zero() {
            break;
        }
        let next_chunk = if execution.prefetched_body.is_empty() {
            match tokio::time::timeout(
                remaining,
                await_direct_passthrough_first_item(
                    next_direct_upstream_response_chunk(&mut execution.response),
                    execution.started_at,
                    execution.stream_first_byte_timeout,
                ),
            )
            .await
            {
                Ok(Ok(item)) => item,
                Ok(Err(_)) | Err(_) => break,
            }
        } else {
            match tokio::time::timeout(
                remaining,
                next_direct_upstream_response_chunk(&mut execution.response),
            )
            .await
            {
                Ok(item) => item,
                Err(_) => break,
            }
        };
        let chunk = match next_chunk {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                fully_buffered = true;
                break;
            }
            Err(error) => {
                execution.prefetched_body.push_back(Err(error));
                break;
            }
        };
        if chunk.is_empty() {
            continue;
        }

        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(inspected.len());
        inspected.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        execution.prefetched_body.push_back(Ok(chunk));

        let response_text = String::from_utf8_lossy(&inspected);
        if aether_provider_transport::is_codex_agent_identity_invalid_task_response(
            execution.status_code,
            Some(response_text.as_ref()),
        ) {
            break;
        }
    }

    if inspected.is_empty() {
        return None;
    }
    if fully_buffered {
        let (body_json, _) = decode_stream_error_body(&execution.headers, &inspected);
        if let Some(body_json) = body_json {
            if let Ok(response_text) = serde_json::to_string(&body_json) {
                return Some(response_text);
            }
        }
    }
    Some(String::from_utf8_lossy(&inspected).into_owned())
}

fn should_use_direct_sse_passthrough(
    plan: &ExecutionPlan,
    plan_kind: &str,
    report_context: Option<&Value>,
    execution: &DirectUpstreamStreamExecution,
) -> bool {
    if !(200..300).contains(&execution.status_code) {
        return false;
    }
    if plan_kind == OPENAI_IMAGE_STREAM_PLAN_KIND {
        return false;
    }
    if is_openai_responses_family_format(plan.provider_api_format.as_str())
        || is_openai_responses_family_format(plan.client_api_format.as_str())
    {
        return false;
    }
    if !response_headers_indicate_sse(&execution.headers) {
        return false;
    }
    if !plan
        .provider_api_format
        .eq_ignore_ascii_case(plan.client_api_format.as_str())
    {
        return false;
    }
    if client_format_allows_proxy_generated_sse_control_blocks(plan) {
        return false;
    }
    if maybe_build_provider_private_stream_normalizer(report_context).is_some() {
        return false;
    }
    let normalized_stream_report_context =
        normalize_provider_private_report_context(report_context);
    if maybe_build_stream_response_rewriter(normalized_stream_report_context.as_ref()).is_some() {
        return false;
    }

    let direct_stream_finalize_kind = resolve_core_stream_direct_finalize_report_kind(plan_kind);
    should_skip_direct_finalize_prefetch(
        direct_stream_finalize_kind.as_deref(),
        execution.headers.get("content-type").map(String::as_str),
        plan.provider_api_format.as_str(),
        plan.client_api_format.as_str(),
        false,
        false,
        false,
    )
}

type DirectUpstreamByteStream = BoxStream<'static, Result<Bytes, String>>;

fn direct_upstream_response_byte_stream(
    prefetched_body: VecDeque<Result<Bytes, String>>,
    response: DirectUpstreamResponse,
) -> DirectUpstreamByteStream {
    let response_stream = match response {
        DirectUpstreamResponse::Reqwest(response) => response
            .bytes_stream()
            .map(|item| item.map_err(|err| format_upstream_request_error(&err)))
            .boxed(),
        DirectUpstreamResponse::HyperH2c(response) => response
            .into_body()
            .into_data_stream()
            .map(|item| item.map_err(|err| format_hyper_error_chain(&err)))
            .boxed(),
        DirectUpstreamResponse::BrowserWreq(response) => response
            .bytes_stream()
            .map(|item| item.map_err(|err| format_wreq_upstream_request_error(&err)))
            .boxed(),
        DirectUpstreamResponse::LocalTunnel(mut response) => stream! {
            loop {
                match response.next_chunk().await {
                    Ok(Some(chunk)) => yield Ok(chunk),
                    Ok(None) => break,
                    Err(err) => {
                        yield Err(err);
                        break;
                    }
                }
            }
        }
        .boxed(),
    };
    futures_stream::iter(prefetched_body)
        .chain(response_stream)
        .boxed()
}

async fn await_direct_passthrough_first_item<T, F>(
    future: F,
    started_at: Instant,
    timeout: Option<Duration>,
) -> Result<T, Duration>
where
    F: Future<Output = T>,
{
    let Some(timeout) = timeout else {
        return Ok(future.await);
    };
    let Some(remaining) = timeout.checked_sub(started_at.elapsed()) else {
        return Err(timeout);
    };
    if remaining.is_zero() {
        return Err(timeout);
    }
    tokio::time::timeout(remaining, future)
        .await
        .map_err(|_| timeout)
}

#[allow(clippy::too_many_arguments)]
async fn forward_direct_passthrough_client_chunk(
    tx: &mpsc::Sender<Result<Bytes, IoError>>,
    chunk: Bytes,
    downstream_dropped: &mut bool,
    client_visible_stream_completed: &mut bool,
    client_stream_completion_tracker: &mut ClientVisibleStreamCompletionTracker,
    observe_stream_completion: bool,
    client_stream_bytes: &mut u64,
    buffered_body: &mut Vec<u8>,
    client_body_truncated: &mut bool,
    max_stream_body_buffer_bytes: usize,
    stream_started_at: Instant,
    last_client_chunk_elapsed_ms: &mut u64,
    first_client_chunk: bool,
    trace_id: &str,
    request_id_for_log: &str,
    candidate_id: Option<&str>,
) -> bool {
    if chunk.is_empty() {
        return false;
    }
    append_stream_capture_bytes(
        buffered_body,
        chunk.as_ref(),
        max_stream_body_buffer_bytes,
        client_body_truncated,
    );
    if *downstream_dropped {
        return false;
    }
    let chunk_len = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
    let send_started_at = Instant::now();
    if tx.send(Ok(chunk.clone())).await.is_err() {
        debug!(
            event_name = "direct_passthrough_downstream_disconnected",
            log_type = "ops",
            trace_id = %trace_id,
            request_id = %request_id_for_log,
            candidate_id = ?candidate_id,
            "gateway direct passthrough downstream dropped; cancelling upstream stream"
        );
        *downstream_dropped = true;
        return false;
    }
    let send_wait_ms = send_started_at.elapsed().as_millis() as u64;
    observe_gateway_stage_ms("direct_passthrough_body_send_wait", send_wait_ms);
    if first_client_chunk {
        observe_gateway_stage_ms("direct_passthrough_first_client_send_wait", send_wait_ms);
    }

    if observe_stream_completion {
        *client_visible_stream_completed |=
            client_stream_completion_tracker.observe_chunk(chunk.as_ref());
    }
    *client_stream_bytes = client_stream_bytes.saturating_add(chunk_len);
    *last_client_chunk_elapsed_ms = stream_started_at
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    true
}

struct DirectPassthroughFinalizer {
    core: Option<DirectPassthroughFinalizerCore>,
}

struct DirectPassthroughFinalizerCore {
    state: AppState,
    plan: ExecutionPlan,
    trace_id: String,
    report_kind: Option<String>,
    report_context: Option<Value>,
    lifecycle_seed: LifecycleUsageSeed,
    direct_stream_finalize_kind: Option<String>,
    stream_started_at: Instant,
    stage_trace: RequestStageTrace,
    request_diagnostics: Option<Arc<RequestDiagnostics>>,
    request_id_for_log: String,
    candidate_id: Option<String>,
    request_candidate_status_snapshot: Option<LocalRequestCandidateStatusSnapshot>,
    deferred_request_candidate_status_record: Option<UpsertRequestCandidateRecord>,
    candidate_started_unix_secs: u64,
    status_code: u16,
    headers: BTreeMap<String, String>,
    stream_usage_report_context: Option<Value>,
    stream_usage_observer: Option<StreamingStandardTerminalObserver>,
    stream_usage_observer_buffered: Vec<u8>,
    provider_error_inspection: ProviderStreamErrorInspection,
    max_stream_body_buffer_bytes: usize,
    provider_buffered_body: Vec<u8>,
    buffered_body: Vec<u8>,
    provider_body_truncated: bool,
    client_body_truncated: bool,
    client_stream_completion_tracker: ClientVisibleStreamCompletionTracker,
    requires_anthropic_message_stop: bool,
    client_visible_stream_completed: bool,
    usage_stream_telemetry: Option<ExecutionTelemetry>,
    telemetry: Option<ExecutionTelemetry>,
    provider_stream_bytes: u64,
    client_stream_bytes: u64,
    last_client_chunk_elapsed_ms: u64,
    pending_recorded: bool,
    stream_started_recorded: bool,
    terminal_failure: Option<StreamFailureReport>,
    _provider_pool_in_flight_guard: Option<ProviderPoolInFlightGuard>,
    _upstream_target_permit: Option<crate::upstream_admission::UpstreamTargetAdmissionPermit>,
}

impl DirectPassthroughFinalizer {
    fn new(core: DirectPassthroughFinalizerCore) -> Self {
        Self { core: Some(core) }
    }

    fn core(&self) -> &DirectPassthroughFinalizerCore {
        self.core
            .as_ref()
            .expect("direct passthrough finalizer core should exist")
    }

    fn core_mut(&mut self) -> &mut DirectPassthroughFinalizerCore {
        self.core
            .as_mut()
            .expect("direct passthrough finalizer core should exist")
    }

    fn stream_started_at(&self) -> Instant {
        self.core().stream_started_at
    }

    fn ttfb_observed(&self) -> bool {
        self.core()
            .usage_stream_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.ttfb_ms)
            .is_some()
    }

    fn terminal_failure(&self) -> Option<&StreamFailureReport> {
        self.core().terminal_failure.as_ref()
    }

    fn set_terminal_failure(&mut self, failure: StreamFailureReport) {
        self.core_mut().terminal_failure = Some(failure);
    }

    fn prepare_upstream_chunk(&mut self, mut chunk: Bytes) -> Option<Bytes> {
        let core = self.core_mut();
        if !core.requires_anthropic_message_stop {
            return Some(chunk);
        }
        if core.client_visible_stream_completed {
            return None;
        }
        if let Some(terminal_end) = core
            .client_stream_completion_tracker
            .observe_anthropic_message_stop_terminal_end(chunk.as_ref())
        {
            chunk.truncate(terminal_end);
            core.client_visible_stream_completed = true;
        }
        Some(chunk)
    }

    fn fail_if_anthropic_message_stop_missing(&mut self) {
        let core = self.core_mut();
        if core.requires_anthropic_message_stop
            && !core.client_visible_stream_completed
            && core.terminal_failure.is_none()
        {
            core.terminal_failure = Some(build_anthropic_premature_eof_failure(
                "upstream Anthropic stream ended before message_stop",
            ));
        }
    }

    fn completed_native_anthropic_stream(&self) -> bool {
        let core = self.core();
        core.requires_anthropic_message_stop
            && core.client_visible_stream_completed
            && core.terminal_failure.is_none()
    }

    fn log_terminal_error_event_encode_failed(&self, err: impl std::fmt::Debug) {
        let core = self.core();
        warn!(
            event_name = "direct_passthrough_terminal_error_event_encode_failed",
            log_type = "ops",
            trace_id = %core.trace_id,
            request_id = %core.request_id_for_log,
            candidate_id = ?core.candidate_id.as_deref(),
            error = ?err,
            "gateway direct passthrough failed to encode terminal SSE error event"
        );
    }

    fn observe_first_body_poll(&mut self) {
        let core = self.core_mut();
        observe_gateway_stage_trace_ms(
            &mut core.stage_trace,
            "stream_body_inline_first_poll",
            stream_elapsed_ms_since(core.stream_started_at),
        );
        let request_diagnostics = core.request_diagnostics.clone();
        observe_request_accepted_stage_trace_ms(
            &mut core.stage_trace,
            request_diagnostics.as_ref(),
            "frontdoor_to_stream_body_first_poll",
        );
    }

    fn observe_upstream_chunk(&mut self, chunk: &Bytes, observed_at: Instant) {
        let core = self.core_mut();
        if core.provider_stream_bytes == 0 {
            observe_gateway_stage_trace_ms(
                &mut core.stage_trace,
                "direct_passthrough_upstream_body_first",
                stream_elapsed_ms_at(core.stream_started_at, observed_at),
            );
        }
        let captured_first_stream_event = maybe_capture_first_stream_event_telemetry(
            core.stream_started_at,
            observed_at,
            core.telemetry.as_ref(),
            &mut core.usage_stream_telemetry,
        );
        if captured_first_stream_event && core.provider_stream_bytes == 0 {
            observe_gateway_stage_trace_ms(
                &mut core.stage_trace,
                "stream_first_data",
                stream_elapsed_ms_at(core.stream_started_at, observed_at),
            );
        }

        core.provider_stream_bytes = core
            .provider_stream_bytes
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        append_stream_capture_bytes(
            &mut core.provider_buffered_body,
            chunk.as_ref(),
            core.max_stream_body_buffer_bytes,
            &mut core.provider_body_truncated,
        );
        if let (Some(observer), Some(report_context)) = (
            core.stream_usage_observer.as_mut(),
            core.stream_usage_report_context.as_ref(),
        ) {
            observe_stream_usage_bytes(
                observer,
                report_context,
                &mut core.stream_usage_observer_buffered,
                chunk.as_ref(),
            );
        }
        if let Some(error_body_json) = core
            .provider_error_inspection
            .observe(core.stream_usage_report_context.as_ref(), chunk.as_ref())
        {
            let error_status_code = resolve_provider_stream_error_status_code(
                core.plan.provider_api_format.as_str(),
                core.status_code,
                &error_body_json,
            );
            core.terminal_failure = Some(build_stream_failure_from_provider_error_body(
                error_status_code,
                &error_body_json,
            ));
        }
    }

    fn observe_client_chunk(&mut self, chunk: &Bytes) {
        if chunk.is_empty() {
            return;
        }
        let core = self.core_mut();
        append_stream_capture_bytes(
            &mut core.buffered_body,
            chunk.as_ref(),
            core.max_stream_body_buffer_bytes,
            &mut core.client_body_truncated,
        );
        if !core.requires_anthropic_message_stop {
            core.client_visible_stream_completed |= core
                .client_stream_completion_tracker
                .observe_chunk(chunk.as_ref());
        }
        core.client_stream_bytes = core
            .client_stream_bytes
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        core.last_client_chunk_elapsed_ms = core
            .stream_started_at
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
    }

    fn observe_first_client_yield(&mut self) {
        let core = self.core_mut();
        let elapsed_ms = stream_elapsed_ms_since(core.stream_started_at);
        observe_gateway_stage_trace_ms(
            &mut core.stage_trace,
            "direct_passthrough_first_client_send",
            elapsed_ms,
        );
        observe_gateway_stage_trace_ms(
            &mut core.stage_trace,
            "stream_first_client_yield",
            elapsed_ms,
        );
        let request_diagnostics = core.request_diagnostics.clone();
        observe_request_accepted_stage_trace_ms(
            &mut core.stage_trace,
            request_diagnostics.as_ref(),
            "frontdoor_to_stream_first_client_yield",
        );
    }

    fn release_upstream_target_permit_after_first_yield(&mut self) {
        let core = self.core_mut();
        if core._upstream_target_permit.take().is_some() {
            observe_gateway_stage_trace_ms(
                &mut core.stage_trace,
                "stream_upstream_target_permit_release",
                stream_elapsed_ms_since(core.stream_started_at),
            );
        }
    }

    fn record_client_visible_stream_started_if_needed(&mut self) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        core.record_client_visible_stream_started_if_needed();
    }

    async fn finalize(&mut self, downstream_dropped: bool) {
        let Some(core) = self.core.take() else {
            return;
        };
        // Move the owned terminal payload into a task before awaiting it. A
        // client disconnect or an execution timeout may cancel this body
        // future while terminal admission is backpressured; the handoff must
        // continue independently so the usage row cannot remain streaming.
        let task = tokio::spawn(async move {
            core.finalize(downstream_dropped).await;
        });
        if let Err(err) = task.await {
            warn!(
                event_name = "direct_passthrough_terminal_handoff_failed",
                log_type = "ops",
                error = %err,
                "gateway direct passthrough terminal handoff task failed"
            );
        }
    }
}

impl Drop for DirectPassthroughFinalizer {
    fn drop(&mut self) {
        let Some(core) = self.core.take() else {
            return;
        };
        observe_gateway_stage_ms("stream_finalizer_enqueue", 0);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                core.finalize(true).await;
            });
        }
    }
}

fn enqueue_stream_candidate_status_update(
    state: &AppState,
    snapshot: LocalRequestCandidateStatusSnapshot,
    status_update: SchedulerRequestCandidateStatusUpdate,
) -> Option<UpsertRequestCandidateRecord> {
    let Err(record) =
        try_enqueue_local_request_candidate_status_snapshot(state, &snapshot, status_update)
    else {
        return None;
    };
    if state.request_candidate_queue.is_some() {
        return Some(record);
    }

    // Without an async queue, preserve the first-byte path's existing
    // fire-and-handoff behavior. Queue saturation uses the bounded deferred
    // record above and does not create one task per waiter.
    let state = state.clone();
    tokio::spawn(async move {
        persist_local_request_candidate_status_record(&state, record).await;
    });
    None
}

impl DirectPassthroughFinalizerCore {
    fn record_client_visible_stream_started_if_needed(&mut self) {
        if self.stream_started_recorded || self.client_stream_bytes == 0 {
            return;
        }
        self.stream_started_recorded = true;
        if !self.pending_recorded {
            self.pending_recorded = true;
            self.state.usage_runtime.record_pending(
                self.state.usage_lifecycle_data_state().as_ref(),
                self.lifecycle_seed.clone(),
            );
        }
        self.state.usage_runtime.record_stream_started(
            self.state.usage_lifecycle_data_state().as_ref(),
            &self.lifecycle_seed,
            self.status_code,
            self.usage_stream_telemetry.as_ref(),
        );
        if let Some(snapshot) = self.request_candidate_status_snapshot.take() {
            self.deferred_request_candidate_status_record = enqueue_stream_candidate_status_update(
                &self.state,
                snapshot,
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Streaming,
                    status_code: Some(self.status_code),
                    error_type: None,
                    error_message: None,
                    latency_ms: None,
                    started_at_unix_ms: Some(self.candidate_started_unix_secs),
                    finished_at_unix_ms: None,
                },
            );
        }
    }

    async fn finalize(mut self, mut downstream_dropped: bool) {
        self.record_client_visible_stream_started_if_needed();
        observe_gateway_stage_ms(
            "stream_total",
            stream_elapsed_ms_since(self.stream_started_at),
        );
        let stream_terminal_summary = finalize_stream_usage_observer(
            &mut self.stream_usage_observer,
            self.stream_usage_report_context.as_ref(),
            &mut self.stream_usage_observer_buffered,
        );

        let DirectPassthroughFinalizerCore {
            state,
            plan,
            trace_id,
            report_kind,
            report_context,
            lifecycle_seed: _,
            direct_stream_finalize_kind,
            stream_started_at,
            stage_trace,
            request_diagnostics,
            request_id_for_log,
            candidate_id,
            request_candidate_status_snapshot: _,
            deferred_request_candidate_status_record,
            candidate_started_unix_secs,
            status_code,
            headers,
            stream_usage_report_context,
            stream_usage_observer: _,
            stream_usage_observer_buffered: _,
            provider_error_inspection: _,
            max_stream_body_buffer_bytes: _,
            provider_buffered_body,
            buffered_body,
            provider_body_truncated,
            client_body_truncated,
            client_stream_completion_tracker: _,
            requires_anthropic_message_stop: _,
            client_visible_stream_completed,
            usage_stream_telemetry,
            telemetry,
            provider_stream_bytes,
            client_stream_bytes: _,
            last_client_chunk_elapsed_ms: _,
            pending_recorded: _,
            stream_started_recorded: _,
            terminal_failure,
            _provider_pool_in_flight_guard,
            _upstream_target_permit,
        } = self;

        // Queue backpressure must not keep scarce upstream/provider permits
        // occupied after the client-visible stream has already ended.
        drop(_provider_pool_in_flight_guard);
        drop(_upstream_target_permit);
        if let Some(record) = deferred_request_candidate_status_record {
            persist_local_request_candidate_status_record(&state, record).await;
        }

        if downstream_dropped && client_visible_stream_completed && terminal_failure.is_none() {
            debug!(
                event_name = "direct_passthrough_downstream_closed_after_done",
                log_type = "debug",
                trace_id = %trace_id,
                request_id = %request_id_for_log,
                candidate_id = ?candidate_id.as_deref(),
                "gateway treats direct passthrough downstream close after terminal SSE event as completed"
            );
            downstream_dropped = false;
        }

        if let Some(failure) = terminal_failure {
            record_manual_proxy_stream_error(&state, &plan).await;
            let terminal_telemetry = Some(build_terminal_stream_telemetry(
                stream_started_at,
                telemetry.as_ref(),
                usage_stream_telemetry.as_ref(),
                provider_stream_bytes,
            ));
            let report_context_for_payload = report_context_with_stage_trace(
                report_context,
                stage_trace,
                stream_started_at,
                terminal_telemetry.as_ref(),
            );
            let report_context_for_payload = report_context_with_request_diagnostics(
                report_context_for_payload,
                request_diagnostics.as_ref(),
                stream_started_at,
                terminal_telemetry.as_ref(),
            );
            submit_midstream_stream_failure(
                &state,
                &trace_id,
                &plan,
                direct_stream_finalize_kind.as_deref(),
                report_context_for_payload,
                headers,
                terminal_telemetry,
                &provider_buffered_body,
                candidate_started_unix_secs,
                failure,
            )
            .await;
            return;
        }

        if downstream_dropped {
            let terminal_telemetry = Some(build_terminal_stream_telemetry(
                stream_started_at,
                telemetry.as_ref(),
                usage_stream_telemetry.as_ref(),
                provider_stream_bytes,
            ));
            let report_context_for_payload = report_context_with_stage_trace(
                report_context,
                stage_trace,
                stream_started_at,
                terminal_telemetry.as_ref(),
            );
            let report_context_for_payload = report_context_with_request_diagnostics(
                report_context_for_payload,
                request_diagnostics.as_ref(),
                stream_started_at,
                terminal_telemetry.as_ref(),
            );
            let usage_payload = build_stream_usage_payload(
                trace_id,
                report_kind.unwrap_or_default(),
                report_context_for_payload,
                499,
                headers,
                &provider_buffered_body,
                provider_body_truncated,
                &buffered_body,
                client_body_truncated,
                stream_terminal_summary,
                terminal_telemetry,
            );
            record_stream_terminal_usage(
                &state,
                &plan,
                usage_payload.report_context.as_ref(),
                &usage_payload,
                true,
            )
            .await;
            record_local_request_candidate_status(
                &state,
                &plan,
                usage_payload.report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Cancelled,
                    status_code: Some(499),
                    error_type: Some("downstream_disconnect".to_string()),
                    error_message: Some("client disconnected before stream completion".to_string()),
                    latency_ms: usage_payload
                        .telemetry
                        .as_ref()
                        .and_then(|value| value.elapsed_ms),
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(current_request_candidate_unix_ms()),
                },
            )
            .await;
            return;
        }

        let mut stream_terminal_summary = stream_terminal_summary;
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            report_context.as_ref(),
            &mut stream_terminal_summary,
        )
        .await;
        let requires_observed_terminal_event = stream_requires_observed_terminal_event(
            plan.provider_api_format.as_str(),
            stream_usage_report_context.as_ref(),
        );
        ensure_stream_terminal_summary_for_missing_observed_finish(
            &mut stream_terminal_summary,
            requires_observed_terminal_event,
        );
        let missing_observed_finish =
            stream_terminal_summary_missing_observed_finish_with_requirement(
                stream_terminal_summary.as_ref(),
                requires_observed_terminal_event,
            );
        let stream_failed = stream_terminal_summary_represents_failure_with_requirement(
            stream_terminal_summary.as_ref(),
            requires_observed_terminal_event,
        );
        let stream_terminal_error_message = stream_terminal_summary
            .as_ref()
            .and_then(|summary| summary.parser_error.clone())
            .or_else(|| {
                missing_observed_finish.then(|| {
                    "execution runtime stream ended before provider terminal event".to_string()
                })
            });
        let should_submit_report = report_kind.is_some();
        let terminal_telemetry = Some(build_terminal_stream_telemetry(
            stream_started_at,
            telemetry.as_ref(),
            usage_stream_telemetry.as_ref(),
            provider_stream_bytes,
        ));
        let report_context_for_payload = report_context_with_stage_trace(
            report_context,
            stage_trace,
            stream_started_at,
            terminal_telemetry.as_ref(),
        );
        let report_context_for_payload = report_context_with_request_diagnostics(
            report_context_for_payload,
            request_diagnostics.as_ref(),
            stream_started_at,
            terminal_telemetry.as_ref(),
        );
        let usage_payload = build_stream_usage_payload(
            trace_id.clone(),
            report_kind.unwrap_or_default(),
            report_context_for_payload,
            status_code,
            headers,
            &provider_buffered_body,
            provider_body_truncated,
            &buffered_body,
            client_body_truncated,
            stream_terminal_summary,
            terminal_telemetry,
        );
        if stream_failed {
            warn!(
                event_name = "direct_passthrough_stream_failed",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %request_id_for_log,
                candidate_id = ?candidate_id.as_deref(),
                status_code,
                error_message = stream_terminal_error_message.as_deref().unwrap_or_default(),
                "gateway direct passthrough stream ended with a failed terminal state"
            );
        } else {
            apply_local_execution_effect(
                &state,
                LocalExecutionEffectContext {
                    plan: &plan,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
            )
            .await;
            apply_local_execution_effect(
                &state,
                LocalExecutionEffectContext {
                    plan: &plan,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::AdaptiveSuccess(LocalAdaptiveSuccessEffect),
            )
            .await;
            apply_local_execution_effect(
                &state,
                LocalExecutionEffectContext {
                    plan: &plan,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::PoolSuccessStream {
                    payload: &usage_payload,
                },
            )
            .await;
        }
        record_stream_terminal_usage(
            &state,
            &plan,
            usage_payload.report_context.as_ref(),
            &usage_payload,
            false,
        )
        .await;
        record_local_request_candidate_status(
            &state,
            &plan,
            usage_payload.report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: if stream_failed {
                    RequestCandidateStatus::Failed
                } else {
                    RequestCandidateStatus::Success
                },
                status_code: Some(status_code),
                error_type: if stream_failed {
                    if missing_observed_finish {
                        Some("stream_missing_terminal_event".to_string())
                    } else {
                        Some("stream_terminal_error".to_string())
                    }
                } else {
                    None
                },
                error_message: stream_failed
                    .then_some(stream_terminal_error_message)
                    .flatten(),
                latency_ms: usage_payload
                    .telemetry
                    .as_ref()
                    .and_then(|value| value.elapsed_ms),
                started_at_unix_ms: Some(candidate_started_unix_secs),
                finished_at_unix_ms: Some(current_request_candidate_unix_ms()),
            },
        )
        .await;

        if should_submit_report {
            if let Err(err) = submit_stream_report(&state, usage_payload).await {
                warn!(
                    event_name = "execution_report_submit_failed",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %request_id_for_log,
                    candidate_id = ?candidate_id.as_deref(),
                    report_scope = "direct_passthrough_stream",
                    error = ?err,
                    "gateway failed to submit direct passthrough stream execution report"
                );
            }
        }
    }
}

fn build_direct_passthrough_inline_body_stream(
    finalizer: DirectPassthroughFinalizer,
    prefetched_body: VecDeque<Result<Bytes, String>>,
    response: DirectUpstreamResponse,
    upstream_started_at: Instant,
    stream_first_byte_timeout: Option<Duration>,
) -> impl futures_util::Stream<Item = Result<Bytes, IoError>> + Send + 'static {
    let state = DirectPassthroughInlineBodyState::new(
        finalizer,
        prefetched_body,
        response,
        upstream_started_at,
        stream_first_byte_timeout,
    );
    futures_stream::unfold(state, |state| async move { state.next_item().await })
}

struct DirectPassthroughInlineBodyState {
    finalizer: Option<DirectPassthroughFinalizer>,
    upstream: Option<DirectUpstreamByteStream>,
    upstream_control_filter: Option<SseControlBlockFilter>,
    upstream_started_at: Instant,
    stream_first_byte_timeout: Option<Duration>,
    observed_first_body_poll: bool,
    observed_first_client_yield: bool,
    upstream_done: bool,
    control_filter_flushed: bool,
    terminal_error_sent: bool,
    finalized: bool,
}

impl DirectPassthroughInlineBodyState {
    fn new(
        finalizer: DirectPassthroughFinalizer,
        prefetched_body: VecDeque<Result<Bytes, String>>,
        response: DirectUpstreamResponse,
        upstream_started_at: Instant,
        stream_first_byte_timeout: Option<Duration>,
    ) -> Self {
        Self {
            finalizer: Some(finalizer),
            upstream: Some(direct_upstream_response_byte_stream(
                prefetched_body,
                response,
            )),
            upstream_control_filter: Some(SseControlBlockFilter::default()),
            upstream_started_at,
            stream_first_byte_timeout,
            observed_first_body_poll: false,
            observed_first_client_yield: false,
            upstream_done: false,
            control_filter_flushed: false,
            terminal_error_sent: false,
            finalized: false,
        }
    }

    async fn next_item(mut self) -> Option<(Result<Bytes, IoError>, Self)> {
        if self.finalized {
            return None;
        }
        if self
            .finalizer
            .as_ref()
            .is_some_and(DirectPassthroughFinalizer::completed_native_anthropic_stream)
        {
            self.upstream.take();
            self.finalized = true;
            drop(self.finalizer.take());
            return None;
        }
        if !self.observed_first_body_poll {
            self.observed_first_body_poll = true;
            if let Some(finalizer) = self.finalizer.as_mut() {
                finalizer.observe_first_body_poll();
            }
        }
        loop {
            if self.upstream_done
                || self
                    .finalizer
                    .as_ref()
                    .and_then(DirectPassthroughFinalizer::terminal_failure)
                    .is_some()
            {
                break;
            }

            let item = self.next_upstream_item().await;
            let Some(item) = item else {
                self.upstream_done = true;
                break;
            };
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(message) => {
                    self.log_upstream_read_error_and_fail(message);
                    self.upstream_done = true;
                    break;
                }
            };
            if chunk.is_empty() {
                continue;
            }

            let observed_at = Instant::now();
            let Some(chunk) = self
                .finalizer
                .as_mut()
                .and_then(|finalizer| finalizer.prepare_upstream_chunk(chunk))
            else {
                continue;
            };
            let provider_error_detected = if let Some(finalizer) = self.finalizer.as_mut() {
                finalizer.observe_upstream_chunk(&chunk, observed_at);
                finalizer.terminal_failure().is_some()
            } else {
                false
            };
            if let Some(client_chunk) =
                filter_upstream_sse_control_chunk(&mut self.upstream_control_filter, chunk)
            {
                self.prepare_client_chunk_yield(&client_chunk);
                self.terminal_error_sent |= provider_error_detected;
                if self
                    .finalizer
                    .as_ref()
                    .is_some_and(DirectPassthroughFinalizer::completed_native_anthropic_stream)
                {
                    self.upstream.take();
                    self.upstream_done = true;
                }
                return Some((Ok(client_chunk), self));
            }
        }

        if let Some(finalizer) = self.finalizer.as_mut() {
            finalizer.fail_if_anthropic_message_stop_missing();
        }

        if !self.control_filter_flushed
            && self
                .finalizer
                .as_ref()
                .and_then(DirectPassthroughFinalizer::terminal_failure)
                .is_none()
        {
            self.control_filter_flushed = true;
            if let Some(client_chunk) =
                flush_upstream_sse_control_filter(&mut self.upstream_control_filter)
            {
                self.prepare_client_chunk_yield(&client_chunk);
                return Some((Ok(client_chunk), self));
            }
        }

        if !self.terminal_error_sent {
            if let Some(finalizer) = self.finalizer.as_mut() {
                if let Some(failure) = finalizer.terminal_failure() {
                    self.terminal_error_sent = true;
                    match encode_terminal_sse_error_event_for_plan(&finalizer.core().plan, failure)
                    {
                        Ok(error_event) => {
                            self.prepare_client_chunk_yield(&error_event);
                            return Some((Ok(error_event), self));
                        }
                        Err(err) => finalizer.log_terminal_error_event_encode_failed(err),
                    }
                }
            }
        }

        self.finalize(false).await;
        None
    }

    async fn next_upstream_item(&mut self) -> Option<Result<Bytes, String>> {
        let needs_first_byte_timeout = self
            .finalizer
            .as_ref()
            .is_some_and(|finalizer| !finalizer.ttfb_observed());
        let upstream = self.upstream.as_mut()?;
        if needs_first_byte_timeout {
            match await_direct_passthrough_first_item(
                upstream.next(),
                self.upstream_started_at,
                self.stream_first_byte_timeout,
            )
            .await
            {
                Ok(item) => item,
                Err(timeout) => {
                    if let Some(finalizer) = self.finalizer.as_mut() {
                        finalizer.set_terminal_failure(build_stream_transport_failure_report(
                            "first_byte_timeout",
                            stream_first_byte_timeout_message(timeout),
                            504,
                        ));
                    }
                    None
                }
            }
        } else {
            upstream.next().await
        }
    }

    fn prepare_client_chunk_yield(&mut self, chunk: &Bytes) {
        let Some(finalizer) = self.finalizer.as_mut() else {
            return;
        };
        finalizer.observe_client_chunk(chunk);
        if !self.observed_first_client_yield {
            self.observed_first_client_yield = true;
            finalizer.release_upstream_target_permit_after_first_yield();
            finalizer.record_client_visible_stream_started_if_needed();
            finalizer.observe_first_client_yield();
        }
    }

    fn log_upstream_read_error_and_fail(&mut self, message: String) {
        let Some(finalizer) = self.finalizer.as_mut() else {
            return;
        };
        if finalizer.completed_native_anthropic_stream() {
            let core = finalizer.core();
            debug!(
                event_name = "direct_passthrough_read_error_ignored_after_anthropic_stop",
                log_type = "debug",
                trace_id = %core.trace_id,
                request_id = %core.request_id_for_log,
                candidate_id = ?core.candidate_id.as_deref(),
                error = %message,
                "gateway ignored direct passthrough teardown error after Anthropic message_stop"
            );
            return;
        }
        let core = finalizer.core();
        warn!(
            event_name = "direct_passthrough_body_read_error",
            log_type = "ops",
            trace_id = %core.trace_id,
            request_id = %core.request_id_for_log,
            candidate_id = ?core.candidate_id.as_deref(),
            upstream_bytes = core.provider_stream_bytes,
            error = %message,
            "gateway direct passthrough upstream body read failed"
        );
        finalizer.set_terminal_failure(build_stream_transport_failure_report(
            "execution_runtime_stream_read_error",
            message,
            502,
        ));
    }

    async fn finalize(&mut self, downstream_dropped: bool) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        self.upstream.take();
        if let Some(finalizer) = self.finalizer.as_mut() {
            finalizer.finalize(downstream_dropped).await;
        }
        self.finalizer.take();
    }
}

impl Drop for DirectPassthroughInlineBodyState {
    fn drop(&mut self) {
        // `finalized` only prevents another poll from entering finalization;
        // the finalizer may still be waiting for its handoff task to finish.
        // Keep the fallback armed while the finalizer is present.
        if self.finalizer.is_none() {
            return;
        }
        self.upstream.take();
        if let Some(finalizer) = self.finalizer.take() {
            observe_gateway_stage_ms("stream_finalizer_enqueue", 0);
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let mut finalizer = finalizer;
                    finalizer.finalize(true).await;
                });
            }
        }
    }
}

async fn record_stream_pending_lifecycle(
    state: &AppState,
    lifecycle_seed: &LifecycleUsageSeed,
    stage_trace: &mut RequestStageTrace,
) {
    let usage_pending_started_at = Instant::now();
    let usage_data = state.usage_lifecycle_data_state().as_ref().clone();
    state
        .usage_runtime
        .record_pending_direct(&usage_data, lifecycle_seed.clone())
        .await;
    observe_gateway_stage_trace_ms(
        stage_trace,
        "stream_usage_pending",
        usage_pending_started_at.elapsed().as_millis() as u64,
    );
}

fn should_defer_stream_pending_for_direct_inline(
    state: &AppState,
    plan: &ExecutionPlan,
    plan_kind: &str,
    report_context: Option<&Value>,
) -> bool {
    if direct_passthrough_mode() != DirectPassthroughMode::Inline {
        return false;
    }
    #[cfg(test)]
    if state
        .execution_runtime_override_base_url()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return false;
    }
    if plan_kind == OPENAI_IMAGE_STREAM_PLAN_KIND {
        return false;
    }
    if is_openai_responses_family_format(plan.provider_api_format.as_str())
        || is_openai_responses_family_format(plan.client_api_format.as_str())
    {
        return false;
    }
    if !plan
        .provider_api_format
        .eq_ignore_ascii_case(plan.client_api_format.as_str())
    {
        return false;
    }
    if client_format_allows_proxy_generated_sse_control_blocks(plan) {
        return false;
    }
    if maybe_build_provider_private_stream_normalizer(report_context).is_some() {
        return false;
    }
    let normalized_stream_report_context =
        normalize_provider_private_report_context(report_context);
    if maybe_build_stream_response_rewriter(normalized_stream_report_context.as_ref()).is_some() {
        return false;
    }
    true
}

#[allow(clippy::too_many_arguments)]
async fn execute_stream_from_direct_passthrough(
    state: &AppState,
    plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    report_context: Option<serde_json::Value>,
    candidate_started_unix_secs: u64,
    stream_started_at: Instant,
    mut stage_trace: RequestStageTrace,
    execution: DirectUpstreamStreamExecution,
    in_flight_guard: Option<ProviderPoolInFlightGuard>,
    pending_recorded: bool,
) -> Result<Option<Response<Body>>, GatewayError> {
    let DirectUpstreamStreamExecution {
        request_id: _,
        candidate_id: _,
        status_code,
        mut headers,
        provider_api_format: _,
        stream_summary_report_context: _,
        prefetched_body,
        stream_precommit_committed: _,
        response,
        started_at: upstream_started_at,
        response_observation,
        stream_first_byte_timeout,
        upstream_target_permit,
    } = execution;

    let requires_anthropic_message_stop = status_code == 200
        && response_headers_indicate_sse(&headers)
        && plan
            .provider_api_format
            .eq_ignore_ascii_case("claude:messages")
        && plan
            .client_api_format
            .eq_ignore_ascii_case("claude:messages");

    let request_id = plan.request_id.clone();
    let candidate_id = plan.candidate_id.clone();
    let request_id_for_log = short_request_id(request_id.as_str());
    let mut report_context = attach_provider_response_headers_to_report_context(
        report_context,
        &headers,
        response_observation.request_started_at_unix_ms,
        response_observation.response_headers_observed_at_unix_ms,
        &response_observation.request_order_id,
    );
    spawn_local_oauth_success_effect(
        state.clone(),
        &plan,
        report_context.as_ref(),
        LocalOAuthSuccessEffect {
            status_code,
            request_started_at_unix_ms: Some(response_observation.request_started_at_unix_ms),
            request_order_id: Some(&response_observation.request_order_id),
        },
    );
    if status_code == 200 {
        seed_kiro_simulated_cache_enabled(state, &plan, &mut report_context).await;
        if kiro_simulated_cache_enabled_from_report_context(report_context.as_ref()) {
            seed_kiro_report_context_input_tokens(&plan, &mut report_context);
        }
        seed_kiro_report_context_prompt_cache_usage(state, &plan, &mut report_context).await;
    }

    let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
    let max_stream_body_buffer_bytes = resolve_stream_body_buffer_limit(state).await;
    let request_candidate_status_snapshot =
        snapshot_local_request_candidate_status(&plan, report_context.as_ref());
    let passthrough_mode = direct_passthrough_mode();
    if passthrough_mode == DirectPassthroughMode::Legacy {
        state.usage_runtime.record_stream_started(
            state.usage_lifecycle_data_state().as_ref(),
            &lifecycle_seed,
            status_code,
            None,
        );
        if let Some(snapshot) = request_candidate_status_snapshot.as_ref() {
            record_local_request_candidate_status_snapshot(
                state,
                snapshot,
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Streaming,
                    status_code: Some(status_code),
                    error_type: None,
                    error_message: None,
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: None,
                },
            )
            .await;
        }
    }

    let response_header_rules_started_at = Instant::now();
    apply_endpoint_response_header_rules(state, &plan, &mut headers, None).await?;
    observe_gateway_stage_ms(
        "stream_response_header_rules",
        response_header_rules_started_at.elapsed().as_millis() as u64,
    );
    let headers_for_report = headers.clone();
    headers.insert(CONTROL_REQUEST_ID_HEADER.to_string(), request_id.clone());
    if let Some(candidate_id) = candidate_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.insert(
            CONTROL_CANDIDATE_ID_HEADER.to_string(),
            candidate_id.to_string(),
        );
    }
    headers.remove("content-length");

    if passthrough_mode == DirectPassthroughMode::Inline {
        let direct_stream_finalize_kind =
            resolve_core_stream_direct_finalize_report_kind(plan_kind);
        let normalized_stream_report_context =
            normalize_provider_private_report_context(report_context.as_ref());
        let stream_usage_report_context = normalized_stream_report_context.clone().or_else(|| {
            Some(serde_json::json!({
                "provider_api_format": plan.provider_api_format.as_str(),
                "client_api_format": plan.client_api_format.as_str(),
            }))
        });
        let stream_usage_observer = stream_usage_report_context
            .as_ref()
            .map(|_| StreamingStandardTerminalObserver::default());
        observe_gateway_stage_trace_ms(
            &mut stage_trace,
            "stream_response_ready",
            stream_elapsed_ms_since(stream_started_at),
        );
        let request_diagnostics = current_request_diagnostics();
        observe_request_accepted_stage_trace_ms(
            &mut stage_trace,
            request_diagnostics.as_ref(),
            "frontdoor_to_stream_response_ready",
        );
        let finalizer = DirectPassthroughFinalizer::new(DirectPassthroughFinalizerCore {
            state: state.clone(),
            plan,
            trace_id: trace_id.to_string(),
            report_kind,
            report_context,
            lifecycle_seed,
            direct_stream_finalize_kind,
            stream_started_at,
            stage_trace,
            request_diagnostics,
            request_id_for_log,
            candidate_id,
            request_candidate_status_snapshot,
            deferred_request_candidate_status_record: None,
            candidate_started_unix_secs,
            status_code,
            headers: headers_for_report,
            stream_usage_report_context,
            stream_usage_observer,
            stream_usage_observer_buffered: Vec::new(),
            provider_error_inspection: ProviderStreamErrorInspection::default(),
            max_stream_body_buffer_bytes,
            provider_buffered_body: Vec::new(),
            buffered_body: Vec::new(),
            provider_body_truncated: false,
            client_body_truncated: false,
            client_stream_completion_tracker: ClientVisibleStreamCompletionTracker::default(),
            requires_anthropic_message_stop,
            client_visible_stream_completed: false,
            usage_stream_telemetry: None,
            telemetry: None,
            provider_stream_bytes: 0,
            client_stream_bytes: 0,
            last_client_chunk_elapsed_ms: 0,
            pending_recorded,
            stream_started_recorded: false,
            terminal_failure: None,
            _provider_pool_in_flight_guard: in_flight_guard,
            _upstream_target_permit: upstream_target_permit,
        });
        let body_stream = build_direct_passthrough_inline_body_stream(
            finalizer,
            prefetched_body,
            response,
            upstream_started_at,
            stream_first_byte_timeout,
        );
        return Ok(Some(build_client_response_from_parts(
            status_code,
            &headers,
            Body::from_stream(body_stream),
            trace_id,
            Some(decision),
        )?));
    }

    let (tx, rx) = mpsc::channel::<Result<Bytes, IoError>>(direct_passthrough_channel_capacity());
    let state_for_report = state.clone();
    let plan_for_report = plan;
    let trace_id_owned = trace_id.to_string();
    let report_kind_owned = report_kind;
    let report_context_owned = report_context;
    let lifecycle_seed_for_report = lifecycle_seed;
    let direct_stream_finalize_kind_owned =
        resolve_core_stream_direct_finalize_report_kind(plan_kind);
    let normalized_stream_report_context_owned =
        normalize_provider_private_report_context(report_context_owned.as_ref());
    let stream_started_at_for_report = stream_started_at;
    observe_gateway_stage_trace_ms(
        &mut stage_trace,
        "stream_response_ready",
        stream_elapsed_ms_since(stream_started_at),
    );
    let request_diagnostics_for_report = current_request_diagnostics();
    observe_request_accepted_stage_trace_ms(
        &mut stage_trace,
        request_diagnostics_for_report.as_ref(),
        "frontdoor_to_stream_response_ready",
    );
    let stage_trace_for_report = stage_trace;
    let request_id_for_report = request_id.clone();
    let request_id_for_report_log = request_id_for_log.clone();
    let candidate_id_for_report = candidate_id.clone();
    let provider_pool_in_flight_guard_for_report = in_flight_guard;
    record_stream_pre_first_byte_spawn();
    tokio::spawn(async move {
        let mut stage_trace_for_report = stage_trace_for_report;
        let _stream_total_guard =
            StageElapsedGuard::from_started_at("stream_total", stream_started_at_for_report);
        let _provider_pool_in_flight_guard = provider_pool_in_flight_guard_for_report;
        let _upstream_target_permit = upstream_target_permit;
        let stream_usage_report_context =
            normalized_stream_report_context_owned.clone().or_else(|| {
                Some(serde_json::json!({
                    "provider_api_format": plan_for_report.provider_api_format.as_str(),
                    "client_api_format": plan_for_report.client_api_format.as_str(),
                }))
            });
        let mut stream_usage_observer = stream_usage_report_context
            .as_ref()
            .map(|_| StreamingStandardTerminalObserver::default());
        let mut stream_usage_observer_buffered = Vec::new();
        let mut provider_error_inspection = ProviderStreamErrorInspection::default();
        let mut provider_buffered_body = Vec::new();
        let mut buffered_body = Vec::new();
        let mut provider_body_truncated = false;
        let mut client_body_truncated = false;
        let mut upstream_control_filter = Some(SseControlBlockFilter::default());
        let mut client_stream_completion_tracker = ClientVisibleStreamCompletionTracker::default();
        let requires_anthropic_message_stop = requires_anthropic_message_stop;
        let mut client_visible_stream_completed = false;
        let mut usage_stream_telemetry: Option<ExecutionTelemetry> = None;
        let telemetry: Option<ExecutionTelemetry> = None;
        let mut provider_stream_bytes = 0u64;
        let mut client_stream_bytes = 0u64;
        let mut last_client_chunk_elapsed_ms = 0u64;
        let mut downstream_dropped = false;
        let mut terminal_failure: Option<StreamFailureReport> = None;
        let mut provider_error_forwarded_to_client = false;
        let mut upstream = direct_upstream_response_byte_stream(prefetched_body, response);
        let mut observed_first_upstream_body = false;
        let mut observed_first_client_send = false;

        loop {
            if downstream_dropped {
                break;
            }
            let item = if usage_stream_telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.ttfb_ms)
                .is_none()
            {
                tokio::select! {
                    biased;
                    _ = tx.closed(), if !downstream_dropped => {
                        downstream_dropped = true;
                        break;
                    }
                    result = await_direct_passthrough_first_item(
                        upstream.next(),
                        upstream_started_at,
                        stream_first_byte_timeout,
                    ) => {
                        match result {
                            Ok(item) => item,
                            Err(timeout) => {
                                terminal_failure = Some(build_stream_transport_failure_report(
                                    "first_byte_timeout",
                                    stream_first_byte_timeout_message(timeout),
                                    504,
                                ));
                                break;
                            }
                        }
                    }
                }
            } else {
                tokio::select! {
                    biased;
                    _ = tx.closed(), if !downstream_dropped => {
                        downstream_dropped = true;
                        break;
                    }
                    item = upstream.next() => item,
                }
            };

            let Some(item) = item else {
                break;
            };
            let chunk = match item {
                Ok(chunk) => chunk,
                Err(message) => {
                    if requires_anthropic_message_stop && client_visible_stream_completed {
                        debug!(
                            event_name = "direct_passthrough_read_error_ignored_after_anthropic_stop",
                            log_type = "debug",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = %message,
                            "gateway ignored direct passthrough teardown error after Anthropic message_stop"
                        );
                        break;
                    }
                    warn!(
                        event_name = "direct_passthrough_body_read_error",
                        log_type = "ops",
                        trace_id = %trace_id_owned,
                        request_id = %request_id_for_report_log,
                        candidate_id = ?candidate_id_for_report.as_deref(),
                        upstream_bytes = provider_stream_bytes,
                        error = %message,
                        "gateway direct passthrough upstream body read failed"
                    );
                    terminal_failure = Some(build_stream_transport_failure_report(
                        "execution_runtime_stream_read_error",
                        message,
                        502,
                    ));
                    break;
                }
            };
            if chunk.is_empty() {
                continue;
            }
            if requires_anthropic_message_stop && client_visible_stream_completed {
                continue;
            }

            let mut provider_chunk = chunk;
            if requires_anthropic_message_stop {
                if let Some(terminal_end) = client_stream_completion_tracker
                    .observe_anthropic_message_stop_terminal_end(provider_chunk.as_ref())
                {
                    provider_chunk.truncate(terminal_end);
                    client_visible_stream_completed = true;
                }
            }

            let observed_at = Instant::now();
            if !observed_first_upstream_body {
                observed_first_upstream_body = true;
                observe_gateway_stage_trace_ms(
                    &mut stage_trace_for_report,
                    "direct_passthrough_upstream_body_first",
                    stream_elapsed_ms_at(stream_started_at_for_report, observed_at),
                );
            }
            maybe_record_first_stream_event_started(
                &state_for_report,
                &lifecycle_seed_for_report,
                status_code,
                stream_started_at_for_report,
                observed_at,
                telemetry.as_ref(),
                &mut usage_stream_telemetry,
            );
            if usage_stream_telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.ttfb_ms)
                .is_some()
                && provider_stream_bytes == 0
            {
                observe_gateway_stage_trace_ms(
                    &mut stage_trace_for_report,
                    "stream_first_data",
                    stream_elapsed_ms_at(stream_started_at_for_report, observed_at),
                );
            }

            let mut sent_client_chunk = false;
            if let Some(client_chunk) = filter_upstream_sse_control_chunk(
                &mut upstream_control_filter,
                provider_chunk.clone(),
            ) {
                sent_client_chunk = forward_direct_passthrough_client_chunk(
                    &tx,
                    client_chunk,
                    &mut downstream_dropped,
                    &mut client_visible_stream_completed,
                    &mut client_stream_completion_tracker,
                    !requires_anthropic_message_stop,
                    &mut client_stream_bytes,
                    &mut buffered_body,
                    &mut client_body_truncated,
                    max_stream_body_buffer_bytes,
                    stream_started_at_for_report,
                    &mut last_client_chunk_elapsed_ms,
                    !observed_first_client_send,
                    trace_id_owned.as_str(),
                    request_id_for_report_log.as_str(),
                    candidate_id_for_report.as_deref(),
                )
                .await;
                if sent_client_chunk && !observed_first_client_send {
                    observed_first_client_send = true;
                    observe_gateway_stage_trace_ms(
                        &mut stage_trace_for_report,
                        "direct_passthrough_first_client_send",
                        stream_elapsed_ms_since(stream_started_at_for_report),
                    );
                }
            }

            provider_stream_bytes = provider_stream_bytes
                .saturating_add(u64::try_from(provider_chunk.len()).unwrap_or(u64::MAX));
            append_stream_capture_bytes(
                &mut provider_buffered_body,
                provider_chunk.as_ref(),
                max_stream_body_buffer_bytes,
                &mut provider_body_truncated,
            );
            if let (Some(observer), Some(report_context)) = (
                stream_usage_observer.as_mut(),
                stream_usage_report_context.as_ref(),
            ) {
                observe_stream_usage_bytes(
                    observer,
                    report_context,
                    &mut stream_usage_observer_buffered,
                    provider_chunk.as_ref(),
                );
            }
            let provider_private_error_body_json = provider_error_inspection.observe(
                stream_usage_report_context.as_ref(),
                provider_chunk.as_ref(),
            );
            if let Some(error_body_json) = provider_private_error_body_json {
                provider_error_forwarded_to_client = sent_client_chunk;
                let error_status_code = resolve_provider_stream_error_status_code(
                    plan_for_report.provider_api_format.as_str(),
                    status_code,
                    &error_body_json,
                );
                terminal_failure = Some(build_stream_failure_from_provider_error_body(
                    error_status_code,
                    &error_body_json,
                ));
                break;
            }
            if requires_anthropic_message_stop && client_visible_stream_completed {
                break;
            }
        }
        drop(upstream);
        drop(_provider_pool_in_flight_guard);
        drop(_upstream_target_permit);

        if terminal_failure.is_none()
            && !downstream_dropped
            && requires_anthropic_message_stop
            && !client_visible_stream_completed
        {
            terminal_failure = Some(build_anthropic_premature_eof_failure(
                "upstream Anthropic stream ended before message_stop",
            ));
        }

        if terminal_failure.is_none() {
            if let Some(client_chunk) =
                flush_upstream_sse_control_filter(&mut upstream_control_filter)
            {
                let _ = forward_direct_passthrough_client_chunk(
                    &tx,
                    client_chunk,
                    &mut downstream_dropped,
                    &mut client_visible_stream_completed,
                    &mut client_stream_completion_tracker,
                    !requires_anthropic_message_stop,
                    &mut client_stream_bytes,
                    &mut buffered_body,
                    &mut client_body_truncated,
                    max_stream_body_buffer_bytes,
                    stream_started_at_for_report,
                    &mut last_client_chunk_elapsed_ms,
                    !observed_first_client_send,
                    trace_id_owned.as_str(),
                    request_id_for_report_log.as_str(),
                    candidate_id_for_report.as_deref(),
                )
                .await;
            }
        }

        if let Some(failure) = terminal_failure
            .as_ref()
            .filter(|_| !downstream_dropped && !provider_error_forwarded_to_client)
        {
            match encode_terminal_sse_error_event_for_plan(&plan_for_report, failure) {
                Ok(error_event) => {
                    let _ = forward_direct_passthrough_client_chunk(
                        &tx,
                        error_event,
                        &mut downstream_dropped,
                        &mut client_visible_stream_completed,
                        &mut client_stream_completion_tracker,
                        true,
                        &mut client_stream_bytes,
                        &mut buffered_body,
                        &mut client_body_truncated,
                        max_stream_body_buffer_bytes,
                        stream_started_at_for_report,
                        &mut last_client_chunk_elapsed_ms,
                        !observed_first_client_send,
                        trace_id_owned.as_str(),
                        request_id_for_report_log.as_str(),
                        candidate_id_for_report.as_deref(),
                    )
                    .await;
                }
                Err(err) => {
                    warn!(
                        event_name = "direct_passthrough_terminal_error_event_encode_failed",
                        log_type = "ops",
                        trace_id = %trace_id_owned,
                        request_id = %request_id_for_report_log,
                        candidate_id = ?candidate_id_for_report.as_deref(),
                        error = ?err,
                        "gateway direct passthrough failed to encode terminal SSE error event"
                    );
                }
            }
        }
        drop(tx);

        let mut stream_terminal_summary = finalize_stream_usage_observer(
            &mut stream_usage_observer,
            stream_usage_report_context.as_ref(),
            &mut stream_usage_observer_buffered,
        );

        if downstream_dropped && client_visible_stream_completed && terminal_failure.is_none() {
            debug!(
                event_name = "direct_passthrough_downstream_closed_after_done",
                log_type = "debug",
                trace_id = %trace_id_owned,
                request_id = %request_id_for_report_log,
                candidate_id = ?candidate_id_for_report.as_deref(),
                "gateway treats direct passthrough downstream close after terminal SSE event as completed"
            );
            downstream_dropped = false;
        }

        if downstream_dropped {
            let terminal_telemetry = Some(build_terminal_stream_telemetry(
                stream_started_at_for_report,
                telemetry.as_ref(),
                usage_stream_telemetry.as_ref(),
                provider_stream_bytes,
            ));
            let report_context_for_payload = report_context_with_stage_trace(
                report_context_owned,
                stage_trace_for_report,
                stream_started_at_for_report,
                terminal_telemetry.as_ref(),
            );
            let report_context_for_payload = report_context_with_request_diagnostics(
                report_context_for_payload,
                request_diagnostics_for_report.as_ref(),
                stream_started_at_for_report,
                terminal_telemetry.as_ref(),
            );
            let usage_payload = build_stream_usage_payload(
                trace_id_owned,
                report_kind_owned.unwrap_or_default(),
                report_context_for_payload,
                499,
                headers_for_report,
                &provider_buffered_body,
                provider_body_truncated,
                &buffered_body,
                client_body_truncated,
                stream_terminal_summary,
                terminal_telemetry,
            );
            record_stream_terminal_usage(
                &state_for_report,
                &plan_for_report,
                usage_payload.report_context.as_ref(),
                &usage_payload,
                true,
            )
            .await;
            record_local_request_candidate_status(
                &state_for_report,
                &plan_for_report,
                usage_payload.report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Cancelled,
                    status_code: Some(499),
                    error_type: Some("downstream_disconnect".to_string()),
                    error_message: Some("client disconnected before stream completion".to_string()),
                    latency_ms: usage_payload
                        .telemetry
                        .as_ref()
                        .and_then(|value| value.elapsed_ms),
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(current_request_candidate_unix_ms()),
                },
            )
            .await;
            return;
        }

        if let Some(failure) = terminal_failure {
            record_manual_proxy_stream_error(&state_for_report, &plan_for_report).await;
            let terminal_telemetry = Some(build_terminal_stream_telemetry(
                stream_started_at_for_report,
                telemetry.as_ref(),
                usage_stream_telemetry.as_ref(),
                provider_stream_bytes,
            ));
            let report_context_for_payload = report_context_with_stage_trace(
                report_context_owned,
                stage_trace_for_report,
                stream_started_at_for_report,
                terminal_telemetry.as_ref(),
            );
            let report_context_for_payload = report_context_with_request_diagnostics(
                report_context_for_payload,
                request_diagnostics_for_report.as_ref(),
                stream_started_at_for_report,
                terminal_telemetry.as_ref(),
            );
            submit_midstream_stream_failure(
                &state_for_report,
                &trace_id_owned,
                &plan_for_report,
                direct_stream_finalize_kind_owned.as_deref(),
                report_context_for_payload,
                headers_for_report,
                terminal_telemetry,
                &provider_buffered_body,
                candidate_started_unix_secs,
                failure,
            )
            .await;
            return;
        }

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state_for_report,
            &plan_for_report,
            report_context_owned.as_ref(),
            &mut stream_terminal_summary,
        )
        .await;
        let requires_observed_terminal_event = stream_requires_observed_terminal_event(
            plan_for_report.provider_api_format.as_str(),
            stream_usage_report_context.as_ref(),
        );
        ensure_stream_terminal_summary_for_missing_observed_finish(
            &mut stream_terminal_summary,
            requires_observed_terminal_event,
        );
        let missing_observed_finish =
            stream_terminal_summary_missing_observed_finish_with_requirement(
                stream_terminal_summary.as_ref(),
                requires_observed_terminal_event,
            );
        let stream_failed = stream_terminal_summary_represents_failure_with_requirement(
            stream_terminal_summary.as_ref(),
            requires_observed_terminal_event,
        );
        let stream_terminal_error_message = stream_terminal_summary
            .as_ref()
            .and_then(|summary| summary.parser_error.clone())
            .or_else(|| {
                missing_observed_finish.then(|| {
                    "execution runtime stream ended before provider terminal event".to_string()
                })
            });
        let should_submit_report = report_kind_owned.is_some();
        let terminal_telemetry = Some(build_terminal_stream_telemetry(
            stream_started_at_for_report,
            telemetry.as_ref(),
            usage_stream_telemetry.as_ref(),
            provider_stream_bytes,
        ));
        let report_context_for_payload = report_context_with_stage_trace(
            report_context_owned,
            stage_trace_for_report,
            stream_started_at_for_report,
            terminal_telemetry.as_ref(),
        );
        let report_context_for_payload = report_context_with_request_diagnostics(
            report_context_for_payload,
            request_diagnostics_for_report.as_ref(),
            stream_started_at_for_report,
            terminal_telemetry.as_ref(),
        );
        let usage_payload = build_stream_usage_payload(
            trace_id_owned.clone(),
            report_kind_owned.unwrap_or_default(),
            report_context_for_payload,
            status_code,
            headers_for_report,
            &provider_buffered_body,
            provider_body_truncated,
            &buffered_body,
            client_body_truncated,
            stream_terminal_summary,
            terminal_telemetry,
        );
        if stream_failed {
            warn!(
                event_name = "direct_passthrough_stream_failed",
                log_type = "ops",
                trace_id = %trace_id_owned,
                request_id = %request_id_for_report_log,
                candidate_id = ?candidate_id_for_report.as_deref(),
                status_code,
                error_message = stream_terminal_error_message.as_deref().unwrap_or_default(),
                "gateway direct passthrough stream ended with a failed terminal state"
            );
        } else {
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
            )
            .await;
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::AdaptiveSuccess(LocalAdaptiveSuccessEffect),
            )
            .await;
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::PoolSuccessStream {
                    payload: &usage_payload,
                },
            )
            .await;
        }
        record_stream_terminal_usage(
            &state_for_report,
            &plan_for_report,
            usage_payload.report_context.as_ref(),
            &usage_payload,
            false,
        )
        .await;
        record_local_request_candidate_status(
            &state_for_report,
            &plan_for_report,
            usage_payload.report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: if stream_failed {
                    RequestCandidateStatus::Failed
                } else {
                    RequestCandidateStatus::Success
                },
                status_code: Some(status_code),
                error_type: if stream_failed {
                    if missing_observed_finish {
                        Some("stream_missing_terminal_event".to_string())
                    } else {
                        Some("stream_terminal_error".to_string())
                    }
                } else {
                    None
                },
                error_message: stream_failed
                    .then_some(stream_terminal_error_message)
                    .flatten(),
                latency_ms: usage_payload
                    .telemetry
                    .as_ref()
                    .and_then(|value| value.elapsed_ms),
                started_at_unix_ms: Some(candidate_started_unix_secs),
                finished_at_unix_ms: Some(current_request_candidate_unix_ms()),
            },
        )
        .await;

        if should_submit_report {
            if let Err(err) = submit_stream_report(&state_for_report, usage_payload).await {
                warn!(
                    event_name = "execution_report_submit_failed",
                    log_type = "ops",
                    trace_id = %trace_id_owned,
                    request_id = %request_id_for_report_log,
                    candidate_id = ?candidate_id_for_report.as_deref(),
                    report_scope = "direct_passthrough_stream",
                    error = ?err,
                    "gateway failed to submit direct passthrough stream execution report"
                );
            }
        }
    });

    let body_stream = build_sse_body_stream(
        Vec::new(),
        rx,
        false,
        false,
        requires_anthropic_message_stop,
        SSE_KEEPALIVE_INTERVAL,
    );
    Ok(Some(build_client_response_from_parts(
        status_code,
        &headers,
        Body::from_stream(body_stream),
        trace_id,
        Some(decision),
    )?))
}

#[allow(clippy::too_many_arguments)] // internal function, grouping would add unnecessary indirection
pub(crate) fn execute_execution_runtime_stream<'a>(
    state: &'a AppState,
    plan: ExecutionPlan,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &'a str,
    report_kind: Option<String>,
    report_context: Option<serde_json::Value>,
) -> Pin<Box<dyn Future<Output = Result<Option<Response<Body>>, GatewayError>> + Send + 'a>> {
    Box::pin(execute_execution_runtime_stream_inner(
        state,
        plan,
        trace_id,
        decision,
        plan_kind,
        report_kind,
        report_context,
        None,
        None,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_execution_runtime_stream_with_retry_scope<'a>(
    state: &'a AppState,
    plan: ExecutionPlan,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &'a str,
    report_kind: Option<String>,
    report_context: Option<serde_json::Value>,
) -> Pin<
    Box<
        dyn Future<Output = Result<AiAttemptExecutionOutcome<Response<Body>>, GatewayError>>
            + Send
            + 'a,
    >,
> {
    Box::pin(async move {
        let mut retry_scope = AiAttemptRetryScope::Candidate;
        let mut fallback_response = None;
        let response = execute_execution_runtime_stream_inner(
            state,
            plan,
            trace_id,
            decision,
            plan_kind,
            report_kind,
            report_context,
            Some(&mut retry_scope),
            Some(&mut fallback_response),
        )
        .await?;
        Ok(match response {
            Some(response) => AiAttemptExecutionOutcome::Responded(response),
            None => AiAttemptExecutionOutcome::Retry {
                scope: retry_scope,
                fallback_response,
            },
        })
    })
}

async fn maybe_build_stream_transport_error_stop_response(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    trace_id: &str,
    decision: &GatewayControlDecision,
    error_type: &str,
    error_message: &str,
    elapsed_ms: u64,
) -> Result<Option<Response<Body>>, GatewayError> {
    let analysis = crate::orchestration::resolve_local_transport_failover_analysis_for_attempt(
        state,
        plan,
        report_context,
    )
    .await;
    if !matches!(analysis.decision, LocalFailoverDecision::StopLocalFailover) {
        return Ok(None);
    }

    crate::execution_runtime::build_transport_error_stop_response(
        state,
        plan,
        report_context,
        trace_id,
        decision,
        http::StatusCode::BAD_GATEWAY.as_u16(),
        error_type,
        error_message,
        elapsed_ms,
    )
    .await
    .map(Some)
}

async fn execute_execution_runtime_stream_inner(
    state: &AppState,
    mut plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    mut report_context: Option<serde_json::Value>,
    mut retry_scope_out: Option<&mut AiAttemptRetryScope>,
    mut retry_fallback_out: Option<&mut Option<Response<Body>>>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let stream_started_at = Instant::now();
    let mut stage_trace = RequestStageTrace::from_env();
    let candidate_slot_started_at = Instant::now();
    ensure_execution_request_candidate_slot(state, &mut plan, &mut report_context).await;
    observe_gateway_stage_trace_ms(
        &mut stage_trace,
        "stream_candidate_slot",
        candidate_slot_started_at.elapsed().as_millis() as u64,
    );
    let request_candidate_status_snapshot =
        snapshot_local_request_candidate_status(&plan, report_context.as_ref());
    let defer_stream_pending_for_direct_inline = should_defer_stream_pending_for_direct_inline(
        state,
        &plan,
        plan_kind,
        report_context.as_ref(),
    );
    // Inline passthrough records its lifecycle seed after upstream headers are
    // available. Avoid constructing a throwaway seed on the common path.
    let mut lifecycle_seed = (!defer_stream_pending_for_direct_inline)
        .then(|| build_lifecycle_usage_seed(&plan, report_context.as_ref()));
    let mut lifecycle_pending_recorded = false;
    if let Some(seed) = lifecycle_seed.as_ref() {
        record_stream_pending_lifecycle(state, seed, &mut stage_trace).await;
        lifecycle_pending_recorded = true;
    }
    let candidate_started_unix_secs = current_request_candidate_unix_ms();
    if let Some(snapshot) = request_candidate_status_snapshot.clone() {
        record_local_request_candidate_status_snapshot(
            state,
            &snapshot,
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
    }
    let plan_request_id_for_log = short_request_id(plan.request_id.as_str());
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
    let provider_in_flight_started_at = Instant::now();
    let mut provider_pool_in_flight_guard = acquire_provider_pool_in_flight_guard(
        state.runtime_state.clone(),
        &plan.provider_id,
        plan.request_id.as_str(),
        plan.candidate_id.as_deref(),
        key_id.as_str(),
    )
    .await;
    observe_gateway_stage_trace_ms(
        &mut stage_trace,
        "stream_provider_in_flight",
        provider_in_flight_started_at.elapsed().as_millis() as u64,
    );
    match maybe_execute_grok_stream(&plan, report_context.as_ref()).await {
        Ok(Some(grok_stream)) => {
            return execute_stream_from_frame_stream_with_retry_scope(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                grok_stream.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                stage_trace,
                lifecycle_pending_recorded,
                grok_stream.frame_stream,
                false,
                provider_pool_in_flight_guard.take(),
                retry_scope_out.as_deref_mut(),
                retry_fallback_out.as_deref_mut(),
                None,
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
            let transport_error_message = err.to_string();
            info!(
                event_name = "grok_execution_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %plan_request_id_for_log,
                candidate_id = ?plan.candidate_id,
                provider_name = provider_name.as_str(),
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                model_name = model_name.as_str(),
                candidate_index = candidate_index.as_str(),
                error = %err,
                "gateway Grok stream execution unavailable"
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
                    error_message: Some(transport_error_message.clone()),
                    latency_ms: Some(stream_elapsed_ms_since(stream_started_at)),
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            if let Some(response) = maybe_build_stream_transport_error_stop_response(
                state,
                &plan,
                report_context.as_ref(),
                trace_id,
                decision,
                "grok_execution_unavailable",
                transport_error_message.as_str(),
                stream_elapsed_ms_since(stream_started_at),
            )
            .await?
            {
                return Ok(Some(response));
            }
            return Ok(None);
        }
    }
    match maybe_execute_windsurf_stream(state, &plan, report_context.as_ref()).await {
        Ok(Some(windsurf_stream)) => {
            return execute_stream_from_frame_stream_with_retry_scope(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                windsurf_stream.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                stage_trace,
                lifecycle_pending_recorded,
                windsurf_stream.frame_stream,
                false,
                provider_pool_in_flight_guard.take(),
                retry_scope_out.as_deref_mut(),
                retry_fallback_out.as_deref_mut(),
                None,
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
            let transport_error_message = err.to_string();
            info!(
                event_name = "windsurf_native_execution_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %plan_request_id_for_log,
                candidate_id = ?plan.candidate_id,
                provider_name = provider_name.as_str(),
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                model_name = model_name.as_str(),
                candidate_index = candidate_index.as_str(),
                error = %err,
                "gateway native Windsurf stream execution unavailable"
            );
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("windsurf_native_execution_unavailable".to_string()),
                    error_message: Some(transport_error_message.clone()),
                    latency_ms: Some(stream_elapsed_ms_since(stream_started_at)),
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            if let Some(response) = maybe_build_stream_transport_error_stop_response(
                state,
                &plan,
                report_context.as_ref(),
                trace_id,
                decision,
                "windsurf_native_execution_unavailable",
                transport_error_message.as_str(),
                stream_elapsed_ms_since(stream_started_at),
            )
            .await?
            {
                return Ok(Some(response));
            }
            return Ok(None);
        }
    }
    match maybe_execute_kiro_web_search_stream(state, &plan, report_context.as_ref()).await {
        Ok(Some(kiro_web_search)) => {
            return execute_stream_from_frame_stream_with_retry_scope(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                kiro_web_search.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                stage_trace,
                lifecycle_pending_recorded,
                kiro_web_search.frame_stream,
                false,
                provider_pool_in_flight_guard.take(),
                retry_scope_out.as_deref_mut(),
                retry_fallback_out.as_deref_mut(),
                None,
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
            let transport_error_message = err.to_string();
            info!(
                event_name = "kiro_web_search_mcp_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %plan_request_id_for_log,
                candidate_id = ?plan.candidate_id,
                provider_name = provider_name.as_str(),
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                model_name = model_name.as_str(),
                candidate_index = candidate_index.as_str(),
                error = %err,
                "gateway Kiro web_search MCP execution unavailable"
            );
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("kiro_web_search_mcp_unavailable".to_string()),
                    error_message: Some(transport_error_message.clone()),
                    latency_ms: Some(stream_elapsed_ms_since(stream_started_at)),
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            if let Some(response) = maybe_build_stream_transport_error_stop_response(
                state,
                &plan,
                report_context.as_ref(),
                trace_id,
                decision,
                "kiro_web_search_mcp_unavailable",
                transport_error_message.as_str(),
                stream_elapsed_ms_since(stream_started_at),
            )
            .await?
            {
                return Ok(Some(response));
            }
            return Ok(None);
        }
    }
    match maybe_execute_chatgpt_web_image_stream(state, &plan, report_context.as_ref()).await {
        Ok(Some(chatgpt_web_image)) => {
            return execute_stream_from_frame_stream_with_retry_scope(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                chatgpt_web_image.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                stage_trace,
                lifecycle_pending_recorded,
                chatgpt_web_image.frame_stream,
                false,
                provider_pool_in_flight_guard.take(),
                retry_scope_out.as_deref_mut(),
                retry_fallback_out.as_deref_mut(),
                None,
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
            let transport_error_message = err.to_string();
            info!(
                event_name = "chatgpt_web_image_execution_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %plan_request_id_for_log,
                candidate_id = ?plan.candidate_id,
                provider_name = provider_name.as_str(),
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                model_name = model_name.as_str(),
                candidate_index = candidate_index.as_str(),
                error = %err,
                "gateway ChatGPT-Web image stream execution unavailable"
            );
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("chatgpt_web_image_execution_unavailable".to_string()),
                    error_message: Some(transport_error_message.clone()),
                    latency_ms: Some(stream_elapsed_ms_since(stream_started_at)),
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            if let Some(response) = maybe_build_stream_transport_error_stop_response(
                state,
                &plan,
                report_context.as_ref(),
                trace_id,
                decision,
                "chatgpt_web_image_execution_unavailable",
                transport_error_message.as_str(),
                stream_elapsed_ms_since(stream_started_at),
            )
            .await?
            {
                return Ok(Some(response));
            }
            return Ok(None);
        }
    }
    #[cfg(not(test))]
    {
        let upstream_headers_started_at = Instant::now();
        let execution = match execute_in_process_stream_with_oauth_retry(
            state,
            &mut plan,
            trace_id,
            report_context.as_ref(),
        )
        .await
        {
            Ok(execution) => execution,
            Err(InProcessStreamExecutionError::Gateway(err)) => {
                if matches!(err, GatewayError::AdmissionTimeout { .. }) {
                    record_stream_admission_timeout_candidate_failure(
                        state,
                        &plan,
                        report_context.as_ref(),
                        candidate_started_unix_secs,
                        &err,
                    )
                    .await;
                }
                return Err(err);
            }
            Err(InProcessStreamExecutionError::Transport(err)) => {
                let transport_error_message = err.to_string();
                info!(
                    event_name = "stream_execution_runtime_unavailable",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %plan_request_id_for_log,
                    candidate_id = ?plan.candidate_id,
                    provider_name,
                    endpoint_id,
                    key_id,
                    model_name,
                    candidate_index = candidate_index.as_str(),
                    error = %err,
                    "gateway in-process stream execution unavailable"
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
                        error_message: Some(transport_error_message.clone()),
                        latency_ms: Some(stream_elapsed_ms_since(stream_started_at)),
                        started_at_unix_ms: Some(candidate_started_unix_secs),
                        finished_at_unix_ms: Some(terminal_unix_secs),
                    },
                )
                .await;
                if let Some(response) = maybe_build_stream_transport_error_stop_response(
                    state,
                    &plan,
                    report_context.as_ref(),
                    trace_id,
                    decision,
                    "execution_runtime_unavailable",
                    transport_error_message.as_str(),
                    stream_elapsed_ms_since(stream_started_at),
                )
                .await?
                {
                    return Ok(Some(response));
                }
                return Ok(None);
            }
        };
        observe_gateway_stage_trace_ms(
            &mut stage_trace,
            "stream_upstream_headers",
            upstream_headers_started_at.elapsed().as_millis() as u64,
        );
        if should_use_direct_sse_passthrough(&plan, plan_kind, report_context.as_ref(), &execution)
        {
            return Box::pin(execute_stream_from_direct_passthrough(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                report_context,
                candidate_started_unix_secs,
                stream_started_at,
                stage_trace,
                execution,
                provider_pool_in_flight_guard.take(),
                lifecycle_pending_recorded,
            ))
            .await;
        }
        if !lifecycle_pending_recorded {
            let seed = lifecycle_seed
                .get_or_insert_with(|| build_lifecycle_usage_seed(&plan, report_context.as_ref()));
            record_stream_pending_lifecycle(state, seed, &mut stage_trace).await;
            lifecycle_pending_recorded = true;
        }
        let report_context = attach_provider_response_headers_to_report_context(
            report_context,
            &execution.headers,
            execution.response_observation.request_started_at_unix_ms,
            execution
                .response_observation
                .response_headers_observed_at_unix_ms,
            &execution.response_observation.request_order_id,
        );
        let stream_precommit_committed = execution.stream_precommit_committed;
        let frame_stream = build_direct_execution_frame_stream(execution).boxed();
        return execute_stream_from_frame_stream_with_retry_scope(
            state,
            plan,
            trace_id,
            decision,
            plan_kind,
            report_kind,
            report_context,
            candidate_started_unix_secs,
            stream_started_at,
            stage_trace,
            lifecycle_pending_recorded,
            frame_stream,
            stream_precommit_committed,
            provider_pool_in_flight_guard.take(),
            retry_scope_out,
            retry_fallback_out,
            None,
        )
        .await;
    }
    #[cfg(test)]
    {
        let remote_execution_runtime_base_url = state
            .execution_runtime_override_base_url()
            .unwrap_or_default();
        if remote_execution_runtime_base_url.trim().is_empty() {
            let upstream_headers_started_at = Instant::now();
            let execution = match execute_in_process_stream_with_oauth_retry(
                state,
                &mut plan,
                trace_id,
                report_context.as_ref(),
            )
            .await
            {
                Ok(execution) => execution,
                Err(InProcessStreamExecutionError::Gateway(err)) => {
                    if matches!(err, GatewayError::AdmissionTimeout { .. }) {
                        record_stream_admission_timeout_candidate_failure(
                            state,
                            &plan,
                            report_context.as_ref(),
                            candidate_started_unix_secs,
                            &err,
                        )
                        .await;
                    }
                    return Err(err);
                }
                Err(InProcessStreamExecutionError::Transport(err)) => {
                    let transport_error_message = err.to_string();
                    info!(
                        event_name = "stream_execution_runtime_unavailable",
                        log_type = "ops",
                        trace_id = %trace_id,
                        request_id = %plan_request_id_for_log,
                        candidate_id = ?plan.candidate_id,
                        provider_name,
                        endpoint_id,
                        key_id,
                        model_name,
                        candidate_index = candidate_index.as_str(),
                        error = %err,
                        "gateway in-process stream execution unavailable"
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
                            error_message: Some(transport_error_message.clone()),
                            latency_ms: Some(stream_elapsed_ms_since(stream_started_at)),
                            started_at_unix_ms: Some(candidate_started_unix_secs),
                            finished_at_unix_ms: Some(terminal_unix_secs),
                        },
                    )
                    .await;
                    if let Some(response) = maybe_build_stream_transport_error_stop_response(
                        state,
                        &plan,
                        report_context.as_ref(),
                        trace_id,
                        decision,
                        "execution_runtime_unavailable",
                        transport_error_message.as_str(),
                        stream_elapsed_ms_since(stream_started_at),
                    )
                    .await?
                    {
                        return Ok(Some(response));
                    }
                    return Ok(None);
                }
            };
            observe_gateway_stage_trace_ms(
                &mut stage_trace,
                "stream_upstream_headers",
                upstream_headers_started_at.elapsed().as_millis() as u64,
            );
            if should_use_direct_sse_passthrough(
                &plan,
                plan_kind,
                report_context.as_ref(),
                &execution,
            ) {
                return Box::pin(execute_stream_from_direct_passthrough(
                    state,
                    plan,
                    trace_id,
                    decision,
                    plan_kind,
                    report_kind,
                    report_context,
                    candidate_started_unix_secs,
                    stream_started_at,
                    stage_trace,
                    execution,
                    provider_pool_in_flight_guard.take(),
                    lifecycle_pending_recorded,
                ))
                .await;
            }
            if !lifecycle_pending_recorded {
                let seed = lifecycle_seed.get_or_insert_with(|| {
                    build_lifecycle_usage_seed(&plan, report_context.as_ref())
                });
                record_stream_pending_lifecycle(state, seed, &mut stage_trace).await;
                lifecycle_pending_recorded = true;
            }
            let report_context = attach_provider_response_headers_to_report_context(
                report_context,
                &execution.headers,
                execution.response_observation.request_started_at_unix_ms,
                execution
                    .response_observation
                    .response_headers_observed_at_unix_ms,
                &execution.response_observation.request_order_id,
            );
            let stream_precommit_committed = execution.stream_precommit_committed;
            let frame_stream = build_direct_execution_frame_stream(execution).boxed();
            return execute_stream_from_frame_stream_with_retry_scope(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                report_context,
                candidate_started_unix_secs,
                stream_started_at,
                stage_trace,
                lifecycle_pending_recorded,
                frame_stream,
                stream_precommit_committed,
                provider_pool_in_flight_guard.take(),
                retry_scope_out.as_deref_mut(),
                retry_fallback_out.as_deref_mut(),
                None,
            )
            .await;
        }

        let remote_request_started_at_unix_ms = current_request_candidate_unix_ms();
        let remote_request_order_id = uuid::Uuid::now_v7().to_string();
        let response = match post_stream_plan_to_remote_execution_runtime(
            state,
            remote_execution_runtime_base_url,
            Some(trace_id),
            &plan,
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                let transport_error_message = format!("{err:?}");
                warn!(
                    event_name = "stream_execution_runtime_remote_unavailable",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %plan_request_id_for_log,
                    candidate_id = ?plan.candidate_id,
                    error = ?err,
                    "gateway remote execution runtime stream unavailable"
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
                        error_message: Some(transport_error_message.clone()),
                        latency_ms: Some(stream_elapsed_ms_since(stream_started_at)),
                        started_at_unix_ms: Some(candidate_started_unix_secs),
                        finished_at_unix_ms: Some(terminal_unix_secs),
                    },
                )
                .await;
                if let Some(response) = maybe_build_stream_transport_error_stop_response(
                    state,
                    &plan,
                    report_context.as_ref(),
                    trace_id,
                    decision,
                    "execution_runtime_unavailable",
                    transport_error_message.as_str(),
                    stream_elapsed_ms_since(stream_started_at),
                )
                .await?
                {
                    return Ok(Some(response));
                }
                return Ok(None);
            }
        };

        if response.status() != http::StatusCode::OK {
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
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
            return Ok(Some(attach_control_metadata_headers(
                build_client_response(response, trace_id, Some(decision))?,
                Some(plan.request_id.as_str()),
                plan.candidate_id.as_deref(),
            )?));
        }

        let remote_response_observed_at_unix_ms = current_request_candidate_unix_ms();
        let remote_fallback_observation = ExecutionResponseObservation {
            request_started_at_unix_ms: remote_request_started_at_unix_ms,
            response_headers_observed_at_unix_ms: remote_response_observed_at_unix_ms,
            request_order_id: remote_request_order_id,
        };
        let frame_stream = response
            .bytes_stream()
            .map_err(|err| IoError::other(err.to_string()))
            .boxed();
        return execute_stream_from_frame_stream_with_retry_scope(
            state,
            plan,
            trace_id,
            decision,
            plan_kind,
            report_kind,
            report_context,
            candidate_started_unix_secs,
            stream_started_at,
            stage_trace,
            lifecycle_pending_recorded,
            frame_stream,
            false,
            provider_pool_in_flight_guard.take(),
            retry_scope_out.as_deref_mut(),
            retry_fallback_out.as_deref_mut(),
            Some(remote_fallback_observation),
        )
        .await;
    }
}

fn decode_stream_data_chunk(
    chunk_b64: Option<&str>,
    text: Option<&str>,
) -> Result<Vec<u8>, GatewayError> {
    if let Some(chunk_b64) = chunk_b64 {
        return base64::engine::general_purpose::STANDARD
            .decode(chunk_b64)
            .map_err(|err| GatewayError::Internal(err.to_string()));
    }
    Ok(text.unwrap_or_default().as_bytes().to_vec())
}

fn response_headers_indicate_sse(headers: &BTreeMap<String, String>) -> bool {
    headers
        .get("content-type")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
}

fn parse_prefetched_sync_json_body(body: &[u8]) -> Option<Value> {
    let stripped = strip_utf8_bom_and_ws(body);
    serde_json::from_slice::<Value>(stripped).ok()
}

fn resolve_provider_stream_error_status_code(
    provider_api_format: &str,
    upstream_status_code: u16,
    body_json: &Value,
) -> u16 {
    if (200..300).contains(&upstream_status_code)
        && provider_api_format
            .trim()
            .eq_ignore_ascii_case("claude:messages")
    {
        anthropic_error_status_code(body_json)
    } else {
        resolve_local_sync_error_status_code(upstream_status_code, body_json)
    }
}

fn anthropic_premature_eof_error_body(message: &str) -> Value {
    serde_json::json!({
        "type": "error",
        "error": {
            "type": "api_error",
            "message": message,
        }
    })
}

fn build_anthropic_premature_eof_failure(message: &str) -> StreamFailureReport {
    let body_json = anthropic_premature_eof_error_body(message);
    build_stream_failure_from_provider_error_body(
        anthropic_error_status_code(&body_json),
        &body_json,
    )
}

fn encode_terminal_sse_error_event(failure: &StreamFailureReport) -> Result<Bytes, std::io::Error> {
    let payload = failure
        .to_json_string()
        .map_err(|err| IoError::other(err.to_string()))?;
    let mut event = String::new();
    for line in payload.lines() {
        event.push_str("data: ");
        event.push_str(line);
        event.push('\n');
    }
    event.push_str("\ndata: [DONE]\n\n");
    Ok(Bytes::from(event))
}

fn encode_anthropic_terminal_sse_error_event(
    failure: &StreamFailureReport,
) -> Result<Bytes, std::io::Error> {
    let payload = serde_json::to_string(&serde_json::json!({
        "type": "error",
        "error": {
            "type": "api_error",
            "message": failure.error_message,
        }
    }))
    .map_err(|err| IoError::other(err.to_string()))?;
    Ok(Bytes::from(format!("event: error\ndata: {payload}\n\n")))
}

fn encode_terminal_sse_error_event_for_plan(
    plan: &ExecutionPlan,
    failure: &StreamFailureReport,
) -> Result<Bytes, std::io::Error> {
    if plan
        .client_api_format
        .trim()
        .eq_ignore_ascii_case("claude:messages")
        && plan
            .provider_api_format
            .trim()
            .eq_ignore_ascii_case("claude:messages")
    {
        encode_anthropic_terminal_sse_error_event(failure)
    } else {
        encode_terminal_sse_error_event(failure)
    }
}

fn image_stream_failed_event_name(report_context: Option<&Value>) -> &'static str {
    let operation = report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("operation"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if operation == "edit" {
        "image_edit.failed"
    } else {
        "image_generation.failed"
    }
}

fn encode_openai_image_failed_event(
    report_context: Option<&Value>,
    failure: &StreamFailureReport,
) -> Result<Bytes, std::io::Error> {
    let event_name = image_stream_failed_event_name(report_context);
    let failure_body = failure
        .to_json_string()
        .map_err(|err| IoError::other(err.to_string()))?;
    let failure_json: Value =
        serde_json::from_str(&failure_body).map_err(|err| IoError::other(err.to_string()))?;
    let error = failure_json.get("error").cloned().unwrap_or_else(|| {
        serde_json::json!({
            "type": failure.error_type.as_str(),
            "message": failure.error_message.as_str(),
            "code": failure.status_code,
        })
    });
    let payload = serde_json::json!({
        "type": event_name,
        "error": error,
    });
    let payload = serde_json::to_string(&payload).map_err(|err| IoError::other(err.to_string()))?;
    let mut event = format!("event: {event_name}\n");
    for line in payload.lines() {
        event.push_str("data: ");
        event.push_str(line);
        event.push('\n');
    }
    event.push('\n');
    Ok(Bytes::from(event))
}

fn should_limit_direct_finalize_prefetch(plan_kind: &str, has_local_stream_rewriter: bool) -> bool {
    plan_kind == OPENAI_IMAGE_STREAM_PLAN_KIND || has_local_stream_rewriter
}

fn client_format_allows_proxy_generated_sse_control_blocks(plan: &ExecutionPlan) -> bool {
    // OpenAI-compatible clients commonly parse every client-visible SSE event as
    // an OpenAI JSON payload or [DONE]. Keep the downstream wire format strict:
    // do not inject proxy-generated comments, pings, or keepalives for openai:*.
    !plan
        .client_api_format
        .trim()
        .to_ascii_lowercase()
        .starts_with("openai:")
}

fn build_sse_body_stream(
    prefetched_chunks_for_body: Vec<Bytes>,
    mut rx: mpsc::Receiver<Result<Bytes, IoError>>,
    filter_control_blocks: bool,
    emit_keepalive: bool,
    anthropic_message_stop_terminates_body: bool,
    keepalive_interval: Duration,
) -> impl futures_util::Stream<Item = Result<Bytes, IoError>> + Send + 'static {
    stream! {
        let mut upstream_control_filter = filter_control_blocks.then(SseControlBlockFilter::default);
        let mut anthropic_completion_tracker =
            anthropic_message_stop_terminates_body.then(ClientVisibleStreamCompletionTracker::default);
        let mut sent_prefetched_chunk = false;
        for chunk in prefetched_chunks_for_body {
            if let Some(mut chunk) = filter_upstream_sse_control_chunk(&mut upstream_control_filter, chunk) {
                let completed = truncate_at_anthropic_message_stop(
                    anthropic_completion_tracker.as_mut(),
                    &mut chunk,
                );
                sent_prefetched_chunk = true;
                yield Ok(chunk);
                if completed {
                    return;
                }
            }
        }

        if emit_keepalive {
            if !sent_prefetched_chunk {
                yield Ok(Bytes::from_static(SSE_KEEPALIVE_BYTES));
            }
            let mut keepalive = tokio::time::interval(keepalive_interval);
            keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
            keepalive.tick().await;
            loop {
                tokio::select! {
                    biased;
                    item = rx.recv() => {
                        let Some(item) = item else {
                            break;
                        };
                        match item {
                            Ok(chunk) => {
                                if let Some(mut chunk) = filter_upstream_sse_control_chunk(&mut upstream_control_filter, chunk) {
                                    let completed = truncate_at_anthropic_message_stop(
                                        anthropic_completion_tracker.as_mut(),
                                        &mut chunk,
                                    );
                                    yield Ok(chunk);
                                    if completed {
                                        break;
                                    }
                                }
                            }
                            Err(err) => yield Err(err),
                        }
                    }
                    _ = keepalive.tick() => {
                        yield Ok(Bytes::from_static(SSE_KEEPALIVE_BYTES));
                    }
                }
            }
            if !anthropic_completion_tracker
                .as_ref()
                .is_some_and(|tracker| tracker.completed)
            {
                if let Some(chunk) =
                    flush_upstream_sse_control_filter(&mut upstream_control_filter)
                {
                    yield Ok(chunk);
                }
            }
        } else {
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(chunk) => {
                        if let Some(mut chunk) = filter_upstream_sse_control_chunk(&mut upstream_control_filter, chunk) {
                            let completed = truncate_at_anthropic_message_stop(
                                anthropic_completion_tracker.as_mut(),
                                &mut chunk,
                            );
                            yield Ok(chunk);
                            if completed {
                                break;
                            }
                        }
                    }
                    Err(err) => yield Err(err),
                }
            }
            if !anthropic_completion_tracker
                .as_ref()
                .is_some_and(|tracker| tracker.completed)
            {
                if let Some(chunk) =
                    flush_upstream_sse_control_filter(&mut upstream_control_filter)
                {
                    yield Ok(chunk);
                }
            }
        }
    }
}

fn truncate_at_anthropic_message_stop(
    tracker: Option<&mut ClientVisibleStreamCompletionTracker>,
    chunk: &mut Bytes,
) -> bool {
    let Some(tracker) = tracker else {
        return false;
    };
    if let Some(terminal_end) = tracker.observe_anthropic_message_stop_terminal_end(chunk.as_ref())
    {
        chunk.truncate(terminal_end);
        return true;
    }
    false
}

#[derive(Default)]
struct SseControlBlockFilter {
    buffered: Vec<u8>,
    emitted_len: usize,
    passthrough_current_block: bool,
}

impl SseControlBlockFilter {
    fn push_chunk(&mut self, chunk: &[u8]) -> Vec<u8> {
        if chunk.is_empty() {
            return Vec::new();
        }

        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some((block_end, separator_len)) = find_sse_block_boundary(&self.buffered) {
            let block_len = block_end + separator_len;
            let block = self.buffered.drain(..block_len).collect::<Vec<_>>();
            if self.passthrough_current_block {
                let emitted_len = self.emitted_len.min(block.len());
                output.extend_from_slice(&block[emitted_len..]);
            } else if sse_block_has_data_line(&block) {
                output.extend_from_slice(&block);
            }
            self.emitted_len = 0;
            self.passthrough_current_block = false;
        }

        if self.passthrough_current_block {
            if self.buffered.len() > self.emitted_len {
                output.extend_from_slice(&self.buffered[self.emitted_len..]);
                self.emitted_len = self.buffered.len();
            }
        } else if sse_buffer_has_data_line(&self.buffered) {
            self.passthrough_current_block = true;
            output.extend_from_slice(&self.buffered);
            self.emitted_len = self.buffered.len();
        }

        if self.buffered.len() > SSE_CONTROL_FILTER_MAX_BUFFER_BYTES {
            let buffered = std::mem::take(&mut self.buffered);
            if self.passthrough_current_block {
                let emitted_len = self.emitted_len.min(buffered.len());
                output.extend_from_slice(&buffered[emitted_len..]);
            } else {
                output.extend(buffered);
            }
            self.emitted_len = 0;
            self.passthrough_current_block = false;
        }

        output
    }

    fn finish(&mut self) -> Vec<u8> {
        if self.buffered.is_empty() {
            return Vec::new();
        }

        let block = std::mem::take(&mut self.buffered);
        let emitted_len = self.emitted_len.min(block.len());
        let passthrough_current_block = self.passthrough_current_block;
        self.emitted_len = 0;
        self.passthrough_current_block = false;
        if passthrough_current_block {
            block[emitted_len..].to_vec()
        } else if sse_block_has_data_line(&block) {
            block
        } else {
            Vec::new()
        }
    }
}

fn filter_upstream_sse_control_chunk(
    filter: &mut Option<SseControlBlockFilter>,
    chunk: Bytes,
) -> Option<Bytes> {
    let Some(filter) = filter.as_mut() else {
        return Some(chunk);
    };

    let filtered = filter.push_chunk(chunk.as_ref());
    (!filtered.is_empty()).then(|| Bytes::from(filtered))
}

fn flush_upstream_sse_control_filter(filter: &mut Option<SseControlBlockFilter>) -> Option<Bytes> {
    let filtered = filter.as_mut()?.finish();
    (!filtered.is_empty()).then(|| Bytes::from(filtered))
}

fn find_sse_block_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    find_sse_record_boundary(buffer)
}

fn sse_block_has_data_line(block: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(block) else {
        return true;
    };

    text.split(['\r', '\n'])
        .any(|line| line.trim_start().starts_with("data:"))
}

fn sse_buffer_has_data_line(buffer: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(buffer) else {
        return true;
    };

    text.split(['\r', '\n'])
        .any(|line| line.trim_start().starts_with("data:"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SseTerminalPolicy {
    AnyKnown,
    AnthropicMessageStop,
}

#[derive(Default)]
struct ClientVisibleStreamCompletionTracker {
    line_buffer: Vec<u8>,
    event_type: Option<String>,
    data_payload: String,
    has_data_payload: bool,
    record_bytes: usize,
    dropping_oversized_record: bool,
    discarded_line_nonempty: bool,
    skip_next_lf: bool,
    completed: bool,
}

impl ClientVisibleStreamCompletionTracker {
    fn observe_chunk(&mut self, chunk: &[u8]) -> bool {
        self.observe_chunk_terminal_end(chunk);
        self.completed
    }

    fn observe_chunk_terminal_end(&mut self, chunk: &[u8]) -> Option<usize> {
        self.observe_chunk_terminal_end_with_policy(chunk, SseTerminalPolicy::AnyKnown)
    }

    fn observe_anthropic_message_stop(&mut self, chunk: &[u8]) -> bool {
        self.observe_anthropic_message_stop_terminal_end(chunk);
        self.completed
    }

    fn observe_anthropic_message_stop_terminal_end(&mut self, chunk: &[u8]) -> Option<usize> {
        self.observe_chunk_terminal_end_with_policy(chunk, SseTerminalPolicy::AnthropicMessageStop)
    }

    fn observe_chunk_terminal_end_with_policy(
        &mut self,
        chunk: &[u8],
        policy: SseTerminalPolicy,
    ) -> Option<usize> {
        if self.completed {
            return None;
        }
        if chunk.is_empty() {
            return None;
        }

        for (index, byte) in chunk.iter().enumerate() {
            self.record_bytes = self.record_bytes.saturating_add(1);
            if self.record_bytes > SSE_TERMINAL_DETECTOR_MAX_RECORD_BYTES
                && !self.dropping_oversized_record
            {
                self.dropping_oversized_record = true;
                self.discarded_line_nonempty = !self.line_buffer.is_empty();
                self.line_buffer.clear();
                self.reset_current_event();
            }

            if self.skip_next_lf {
                self.skip_next_lf = false;
                if *byte == b'\n' {
                    continue;
                }
            }

            if self.dropping_oversized_record {
                match *byte {
                    b'\n' => self.finish_discarded_line(),
                    b'\r' => {
                        self.finish_discarded_line();
                        self.skip_next_lf = true;
                    }
                    _ => self.discarded_line_nonempty = true,
                }
                continue;
            }

            match *byte {
                b'\n' => self.finish_line(policy),
                b'\r' => {
                    self.finish_line(policy);
                    self.skip_next_lf = true;
                }
                _ => self.line_buffer.push(*byte),
            }

            if self.completed {
                let terminal_end = if *byte == b'\r' && chunk.get(index + 1) == Some(&b'\n') {
                    index + 2
                } else {
                    index + 1
                };
                return Some(terminal_end);
            }
        }

        None
    }

    fn finish_line(&mut self, policy: SseTerminalPolicy) {
        let line = std::mem::take(&mut self.line_buffer);
        let Ok(line) = std::str::from_utf8(&line) else {
            self.reset_current_event();
            return;
        };
        let line = line.trim();

        if line.is_empty() {
            self.completed = self.current_event_is_terminal(policy);
            self.reset_current_event();
            self.record_bytes = 0;
            return;
        }

        if let Some(event_type) = line.strip_prefix("event:").map(str::trim) {
            self.event_type = Some(event_type.to_string());
            return;
        }

        if let Some(data) = line.strip_prefix("data:").map(str::trim) {
            if data.is_empty() {
                return;
            }
            if self.has_data_payload {
                self.data_payload.push('\n');
            }
            self.data_payload.push_str(data);
            self.has_data_payload = true;
        }
    }

    fn finish_discarded_line(&mut self) {
        if !self.discarded_line_nonempty {
            self.dropping_oversized_record = false;
            self.record_bytes = 0;
            self.reset_current_event();
        }
        self.discarded_line_nonempty = false;
    }

    fn current_event_is_terminal(&self, policy: SseTerminalPolicy) -> bool {
        match policy {
            SseTerminalPolicy::AnyKnown => {
                self.event_type
                    .as_deref()
                    .is_some_and(is_terminal_sse_event_type)
                    || (self.has_data_payload && sse_data_payload_is_terminal(&self.data_payload))
            }
            SseTerminalPolicy::AnthropicMessageStop => {
                let payload_type = self
                    .has_data_payload
                    .then(|| serde_json::from_str::<serde_json::Value>(&self.data_payload).ok())
                    .flatten()
                    .and_then(|value| {
                        value
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .map(ToOwned::to_owned)
                    });
                payload_type.as_deref() == Some("message_stop")
                    && self
                        .event_type
                        .as_deref()
                        .is_none_or(|event_type| event_type == "message_stop")
            }
        }
    }

    fn reset_current_event(&mut self) {
        self.event_type = None;
        self.data_payload.clear();
        self.has_data_payload = false;
    }
}

fn is_terminal_sse_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "message_stop" | "response.completed" | "response.failed" | "response.incomplete" | "error"
    )
}

fn sse_data_payload_is_terminal(data: &str) -> bool {
    data == "[DONE]"
        || serde_json::from_str::<serde_json::Value>(data).is_ok_and(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(is_terminal_sse_event_type)
        })
}

fn stream_chunk_contains_sse_done(chunk: &[u8]) -> bool {
    let mut tracker = ClientVisibleStreamCompletionTracker::default();
    tracker.observe_chunk(chunk)
}

struct ObservedStreamFrame {
    frame: StreamFrame,
    observed_at: Instant,
}

#[derive(Clone)]
struct PostStopFrameReadBudget {
    remaining: Arc<AtomicUsize>,
}

impl PostStopFrameReadBudget {
    fn new() -> Self {
        Self {
            remaining: Arc::new(AtomicUsize::new(POST_STOP_FRAME_READ_BUDGET_INACTIVE)),
        }
    }

    fn activate(&self, already_buffered: usize) -> bool {
        let remaining = ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES.saturating_sub(already_buffered);
        let activated = self
            .remaining
            .compare_exchange(
                POST_STOP_FRAME_READ_BUDGET_INACTIVE,
                remaining,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        activated && already_buffered > ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES
    }
}

struct PostStopLimitedStreamReader<S> {
    stream: S,
    current: Option<Bytes>,
    budget: PostStopFrameReadBudget,
}

impl<S> PostStopLimitedStreamReader<S> {
    fn new(stream: S, budget: PostStopFrameReadBudget) -> Self {
        Self {
            stream,
            current: None,
            budget,
        }
    }

    fn activate_post_stop_budget(&mut self, already_buffered: usize) -> bool {
        let over_limit = self.budget.activate(already_buffered);
        let remaining = self.budget.remaining.load(Ordering::Acquire);
        self.trim_current_to_budget(remaining, true);
        over_limit
    }

    fn trim_current_to_budget(&mut self, remaining: usize, detach_backing: bool) {
        if remaining == POST_STOP_FRAME_READ_BUDGET_INACTIVE {
            return;
        }
        if remaining == 0 {
            self.current = None;
            return;
        }
        if let Some(current) = self.current.as_mut() {
            if detach_backing || current.len() > remaining {
                let retained = current.len().min(remaining);
                // Detach even a small slice because it can retain a giant
                // producer allocation across post-stop backpressure.
                *current = Bytes::copy_from_slice(&current[..retained]);
            }
        }
    }
}

impl<S> AsyncRead for PostStopLimitedStreamReader<S>
where
    S: Stream<Item = Result<Bytes, IoError>> + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), IoError>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        let mut empty_chunks = 0usize;
        loop {
            let remaining = this.budget.remaining.load(Ordering::Acquire);
            if remaining == 0 {
                this.current = None;
                return Poll::Ready(Ok(()));
            }
            this.trim_current_to_budget(remaining, false);

            if let Some(current) = this.current.as_mut() {
                let read = current.len().min(buf.remaining());
                if read > 0 {
                    buf.put_slice(&current.split_to(read));
                    if remaining != POST_STOP_FRAME_READ_BUDGET_INACTIVE {
                        let previous = this.budget.remaining.fetch_sub(read, Ordering::AcqRel);
                        debug_assert!(previous != POST_STOP_FRAME_READ_BUDGET_INACTIVE);
                        debug_assert!(previous >= read);
                    }
                }
                if current.is_empty() {
                    this.current = None;
                }
                if read > 0 {
                    return Poll::Ready(Ok(()));
                }
            }

            match Pin::new(&mut this.stream).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(chunk))) if chunk.is_empty() => {
                    empty_chunks += 1;
                    if empty_chunks >= POST_STOP_MAX_EMPTY_CHUNKS_PER_POLL {
                        cx.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    this.current = Some(chunk);
                    if remaining != POST_STOP_FRAME_READ_BUDGET_INACTIVE {
                        this.trim_current_to_budget(remaining, true);
                    }
                }
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Err(err)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
            }
        }
    }
}

fn activate_post_stop_frame_read_budget<S>(
    lines: &mut FramedRead<PostStopLimitedStreamReader<S>, LinesCodec>,
) -> bool {
    let already_buffered = lines.read_buffer().len();
    let over_limit = already_buffered > ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES;
    let reader_over_limit = lines.get_mut().activate_post_stop_budget(already_buffered);
    let retained = if over_limit { 0 } else { already_buffered };
    let mut bounded = bytes::BytesMut::with_capacity(retained);
    bounded.extend_from_slice(&lines.read_buffer()[..retained]);
    *lines.read_buffer_mut() = bounded;
    reader_over_limit || over_limit
}

async fn read_next_observed_stream_frame<R>(
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Option<ObservedStreamFrame>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    Ok(read_next_frame(lines)
        .await?
        .map(|frame| ObservedStreamFrame {
            frame,
            observed_at: Instant::now(),
        }))
}

async fn next_stream_frame<R>(
    buffered_frames: &mut VecDeque<ObservedStreamFrame>,
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Option<ObservedStreamFrame>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    if let Some(frame) = buffered_frames.pop_front() {
        return Ok(Some(frame));
    }
    read_next_observed_stream_frame(lines).await
}

fn serialized_stream_frame_len(frame: &StreamFrame) -> usize {
    serde_json::to_vec(frame).map_or(usize::MAX, |encoded| encoded.len())
}

fn should_refresh_stream_usage_telemetry(
    previous: Option<&ExecutionTelemetry>,
    next: &ExecutionTelemetry,
) -> bool {
    let previous_ttfb = previous.and_then(|telemetry| telemetry.ttfb_ms);
    let previous_elapsed = previous.and_then(|telemetry| telemetry.elapsed_ms);
    let next_ttfb = next.ttfb_ms;
    let next_elapsed = next.elapsed_ms;

    (next_ttfb.is_some() && next_ttfb != previous_ttfb)
        || (next_elapsed.is_some() && next_elapsed != previous_elapsed)
}

fn stream_elapsed_ms_since(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn stream_elapsed_ms_at(started_at: Instant, observed_at: Instant) -> u64 {
    observed_at
        .saturating_duration_since(started_at)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn first_stream_event_telemetry(
    stream_started_at: Instant,
    event_observed_at: Instant,
    upstream_telemetry: Option<&ExecutionTelemetry>,
) -> ExecutionTelemetry {
    let elapsed_ms = stream_elapsed_ms_at(stream_started_at, event_observed_at);
    ExecutionTelemetry {
        ttfb_ms: Some(elapsed_ms),
        elapsed_ms: Some(elapsed_ms),
        upstream_bytes: upstream_telemetry.and_then(|telemetry| telemetry.upstream_bytes),
    }
}

fn maybe_capture_first_stream_event_telemetry(
    stream_started_at: Instant,
    event_observed_at: Instant,
    upstream_telemetry: Option<&ExecutionTelemetry>,
    usage_stream_telemetry: &mut Option<ExecutionTelemetry>,
) -> bool {
    if usage_stream_telemetry
        .as_ref()
        .and_then(|telemetry| telemetry.ttfb_ms)
        .is_some()
    {
        return false;
    }

    *usage_stream_telemetry = Some(first_stream_event_telemetry(
        stream_started_at,
        event_observed_at,
        upstream_telemetry,
    ));
    true
}

fn usage_refresh_telemetry(
    upstream_telemetry: &ExecutionTelemetry,
    usage_stream_telemetry: Option<&ExecutionTelemetry>,
) -> ExecutionTelemetry {
    ExecutionTelemetry {
        ttfb_ms: usage_stream_telemetry.and_then(|telemetry| telemetry.ttfb_ms),
        elapsed_ms: upstream_telemetry.elapsed_ms,
        upstream_bytes: upstream_telemetry.upstream_bytes,
    }
}

fn maybe_record_first_stream_event_started(
    state: &AppState,
    lifecycle_seed: &LifecycleUsageSeed,
    status_code: u16,
    stream_started_at: Instant,
    event_observed_at: Instant,
    upstream_telemetry: Option<&ExecutionTelemetry>,
    usage_stream_telemetry: &mut Option<ExecutionTelemetry>,
) {
    if !maybe_capture_first_stream_event_telemetry(
        stream_started_at,
        event_observed_at,
        upstream_telemetry,
        usage_stream_telemetry,
    ) {
        return;
    }
    let Some(telemetry) = usage_stream_telemetry.as_ref() else {
        return;
    };
    state.usage_runtime.record_stream_started(
        state.usage_lifecycle_data_state().as_ref(),
        lifecycle_seed,
        status_code,
        Some(telemetry),
    );
}

fn build_terminal_stream_telemetry(
    stream_started_at: Instant,
    telemetry: Option<&ExecutionTelemetry>,
    usage_stream_telemetry: Option<&ExecutionTelemetry>,
    upstream_bytes: u64,
) -> ExecutionTelemetry {
    let current_elapsed_ms = stream_elapsed_ms_since(stream_started_at);
    let ttfb_ms = usage_stream_telemetry.and_then(|telemetry| telemetry.ttfb_ms);
    let prior_elapsed_ms = telemetry
        .and_then(|telemetry| telemetry.elapsed_ms)
        .or_else(|| usage_stream_telemetry.and_then(|telemetry| telemetry.elapsed_ms))
        .unwrap_or(0);
    let elapsed_ms = current_elapsed_ms
        .max(prior_elapsed_ms)
        .max(ttfb_ms.unwrap_or(0));
    ExecutionTelemetry {
        ttfb_ms,
        elapsed_ms: Some(elapsed_ms),
        upstream_bytes: Some(upstream_bytes),
    }
}

fn should_skip_direct_finalize_prefetch(
    direct_stream_finalize_kind: Option<&str>,
    content_type: Option<&str>,
    provider_api_format: &str,
    client_api_format: &str,
    has_private_stream_normalizer: bool,
    has_local_stream_rewriter: bool,
    force_prefetch: bool,
) -> bool {
    StreamCommitPolicy::for_response(
        direct_stream_finalize_kind.is_some(),
        content_type,
        provider_api_format,
        client_api_format,
        has_private_stream_normalizer,
        has_local_stream_rewriter,
        force_prefetch,
    )
    .commits_on_response_headers()
}

fn prefetched_openai_responses_body_has_output_boundary(body: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(body) else {
        return true;
    };
    for line in text.lines() {
        let Some(data) = line.trim().strip_prefix("data:").map(str::trim) else {
            continue;
        };
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            return true;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        let event_type = event.get("type").and_then(Value::as_str).map(str::trim);
        if !event_type.is_some_and(|event_type| {
            matches!(
                event_type,
                "response.created" | "response.in_progress" | "response.queued"
            )
        }) {
            return true;
        }
    }
    false
}

fn should_probe_success_failover_before_stream(headers: &BTreeMap<String, String>) -> bool {
    let content_type = headers
        .get("content-type")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();

    content_type.contains("json") || content_type.ends_with("+json")
}

async fn probe_local_stream_success_failover_text<R>(
    buffered_frames: &mut VecDeque<ObservedStreamFrame>,
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Option<String>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut aggregated = String::new();
    let mut frames_probed = 0usize;
    while frames_probed < MAX_STREAM_PREFETCH_FRAMES && aggregated.len() < MAX_STREAM_PREFETCH_BYTES
    {
        let Some(observed_frame) = read_next_observed_stream_frame(lines).await? else {
            break;
        };
        let probe_text = match &observed_frame.frame.payload {
            StreamFramePayload::Data { chunk_b64, text } => {
                match decode_stream_data_chunk(chunk_b64.as_deref(), text.as_deref()) {
                    Ok(chunk) if !chunk.is_empty() => {
                        Some(String::from_utf8_lossy(&chunk).into_owned())
                    }
                    Ok(_) | Err(_) => None,
                }
            }
            StreamFramePayload::Error { .. } | StreamFramePayload::Eof { .. } => None,
            StreamFramePayload::Headers { .. } | StreamFramePayload::Telemetry { .. } => None,
        };
        buffered_frames.push_back(observed_frame);
        if let Some(probe_text) = probe_text {
            aggregated.push_str(&probe_text);
            // Bound the probe to the same prefetch limits as the main
            // stream inspection so chunked JSON bodies spanning multiple
            // data frames are still matched before the first visible output
            // is committed.
            frames_probed += 1;
            if aggregated.len() >= MAX_STREAM_PREFETCH_BYTES {
                break;
            }
        }
    }

    if aggregated.is_empty() {
        Ok(None)
    } else {
        Ok(Some(aggregated))
    }
}

async fn execute_stream_from_frame_stream(
    state: &AppState,
    plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    report_context: Option<serde_json::Value>,
    candidate_started_unix_secs: u64,
    stream_started_at: Instant,
    stage_trace: RequestStageTrace,
    lifecycle_pending_recorded: bool,
    frame_stream: BoxStream<'static, Result<Bytes, IoError>>,
    in_flight_guard: Option<ProviderPoolInFlightGuard>,
) -> Result<Option<Response<Body>>, GatewayError> {
    execute_stream_from_frame_stream_with_retry_scope(
        state,
        plan,
        trace_id,
        decision,
        plan_kind,
        report_kind,
        report_context,
        candidate_started_unix_secs,
        stream_started_at,
        stage_trace,
        lifecycle_pending_recorded,
        frame_stream,
        false,
        in_flight_guard,
        None,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_stream_from_frame_stream_with_retry_scope(
    state: &AppState,
    plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    report_context: Option<serde_json::Value>,
    candidate_started_unix_secs: u64,
    stream_started_at: Instant,
    mut stage_trace: RequestStageTrace,
    lifecycle_pending_recorded: bool,
    frame_stream: BoxStream<'static, Result<Bytes, IoError>>,
    stream_precommit_committed: bool,
    in_flight_guard: Option<ProviderPoolInFlightGuard>,
    mut retry_scope_out: Option<&mut AiAttemptRetryScope>,
    mut retry_fallback_out: Option<&mut Option<Response<Body>>>,
    fallback_response_observation: Option<ExecutionResponseObservation>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let request_id = plan.request_id.as_str();
    let request_id_for_log = short_request_id(request_id);
    let candidate_id = plan.candidate_id.as_deref();
    let provider_name = plan.provider_name.as_deref().unwrap_or("-");
    let model_name = plan.model_name.as_deref().unwrap_or("-");
    let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
    if !lifecycle_pending_recorded {
        record_stream_pending_lifecycle(state, &lifecycle_seed, &mut stage_trace).await;
    }
    let max_stream_body_buffer_bytes = resolve_stream_body_buffer_limit(state).await;
    let request_candidate_status_snapshot =
        snapshot_local_request_candidate_status(&plan, report_context.as_ref());
    let candidate_index = parse_request_candidate_report_context(report_context.as_ref())
        .and_then(|context| context.candidate_index)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let reader = PostStopLimitedStreamReader::new(frame_stream, PostStopFrameReadBudget::new());
    let mut lines = FramedRead::new(reader, LinesCodec::new());

    let first_frame_started_at = Instant::now();
    let first_frame = read_next_frame(&mut lines).await?.ok_or_else(|| {
        GatewayError::Internal("execution runtime stream ended before headers frame".to_string())
    })?;
    observe_gateway_stage_trace_ms(
        &mut stage_trace,
        "stream_first_frame",
        first_frame_started_at.elapsed().as_millis() as u64,
    );
    let StreamFramePayload::Headers {
        status_code,
        mut headers,
        response_observation,
    } = first_frame.payload
    else {
        return Err(GatewayError::Internal(
            "execution runtime stream must start with headers frame".to_string(),
        ));
    };
    let response_observation = response_observation
        .or(fallback_response_observation)
        .unwrap_or(ExecutionResponseObservation {
            request_started_at_unix_ms: candidate_started_unix_secs,
            response_headers_observed_at_unix_ms: current_request_candidate_unix_ms(),
            request_order_id: uuid::Uuid::now_v7().to_string(),
        });
    let mut report_context = attach_provider_response_headers_to_report_context(
        report_context,
        &headers,
        response_observation.request_started_at_unix_ms,
        response_observation.response_headers_observed_at_unix_ms,
        &response_observation.request_order_id,
    );
    spawn_local_oauth_success_effect(
        state.clone(),
        &plan,
        report_context.as_ref(),
        LocalOAuthSuccessEffect {
            status_code,
            request_started_at_unix_ms: Some(response_observation.request_started_at_unix_ms),
            request_order_id: Some(&response_observation.request_order_id),
        },
    );
    if status_code == 200 {
        seed_kiro_simulated_cache_enabled(state, &plan, &mut report_context).await;
        if kiro_simulated_cache_enabled_from_report_context(report_context.as_ref()) {
            seed_kiro_report_context_input_tokens(&plan, &mut report_context);
        }
        seed_kiro_report_context_prompt_cache_usage(state, &plan, &mut report_context).await;
    }
    let mut buffered_frames = VecDeque::new();
    let mut stream_terminal_summary: Option<ExecutionStreamTerminalSummary> = None;
    if status_code == 200 && should_probe_success_failover_before_stream(&headers) {
        let success_probe_text =
            probe_local_stream_success_failover_text(&mut buffered_frames, &mut lines).await?;
        if should_retry_next_local_candidate_stream(
            state,
            &plan,
            plan_kind,
            report_context.as_ref(),
            status_code,
            success_probe_text.as_deref(),
        )
        .await
        {
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: Some(status_code),
                    error_type: Some("success_failover_pattern".to_string()),
                    error_message: Some(
                        "execution runtime stream matched provider success failover rule"
                            .to_string(),
                    ),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            warn!(
                event_name = "local_stream_candidate_retry_scheduled",
                log_type = "event",
                trace_id = %trace_id,
                request_id = %request_id_for_log,
                status_code,
                provider_name = provider_name,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                model_name,
                candidate_index = candidate_index.as_str(),
                "gateway local stream decision retrying next candidate after success failover rule match"
            );
            return Ok(None);
        }
    }

    let stream_error_finalize_kind =
        resolve_core_stream_error_finalize_report_kind(plan_kind, status_code);

    if !(200..300).contains(&status_code) {
        let provider_error_body = collect_error_body(&mut lines).await?;
        let private_error_body_json = extract_provider_private_stream_error_body(
            report_context.as_ref(),
            &provider_error_body,
        );
        let provider_private_error_decoded = private_error_body_json.is_some();
        let synthetic_body_json = (!provider_private_error_decoded
            && should_synthesize_non_success_stream_error_body(status_code, &provider_error_body))
        .then(|| build_synthetic_non_success_stream_error_body(status_code, &headers));
        let (provider_body_json, provider_body_base64) =
            if let Some(error_body_json) = private_error_body_json {
                (Some(error_body_json), None)
            } else {
                decode_stream_error_body(&headers, &provider_error_body)
            };
        let client_status_code = stream_client_error_status_code_for_upstream_status(status_code);
        let wrapped_binary_body_json = if provider_private_error_decoded {
            None
        } else {
            wrap_non_json_binary_stream_error_for_client(plan_kind, &headers, &provider_error_body)?
        };
        let (client_body_json, client_error_body, payload_client_body_json) =
            if let Some(body_json) = synthetic_body_json.or(wrapped_binary_body_json) {
                let body_bytes = serde_json::to_vec(&body_json)
                    .map_err(|err| GatewayError::Internal(err.to_string()))?;
                (Some(body_json.clone()), body_bytes, Some(body_json))
            } else if provider_private_error_decoded {
                let body_json = provider_body_json.clone().ok_or_else(|| {
                    GatewayError::Internal(
                        "decoded provider private stream error body is missing".to_string(),
                    )
                })?;
                let body_bytes = serde_json::to_vec(&body_json)
                    .map_err(|err| GatewayError::Internal(err.to_string()))?;
                (Some(body_json), body_bytes, None)
            } else {
                (
                    provider_body_json.clone(),
                    provider_error_body.clone(),
                    provider_body_json.clone(),
                )
            };
        let error_response_text =
            local_failover_response_text(client_body_json.as_ref(), &client_error_body, None);
        let failover_analysis = resolve_local_candidate_failover_analysis_stream(
            state,
            &plan,
            report_context.as_ref(),
            status_code,
            error_response_text.as_deref(),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code,
                classification: failover_analysis.classification,
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
                status_code,
                classification: failover_analysis.classification,
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
                status_code,
                classification: failover_analysis.classification,
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
                status_code,
                response_text: error_response_text.as_deref(),
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
                status_code,
                classification: failover_analysis.classification,
                headers: &headers,
                error_body: error_response_text.as_deref(),
            }),
        )
        .await;
        let failover_decision = failover_analysis.decision;
        debug!(
            event_name = "execution_runtime_stream_failover_decided",
            log_type = "debug",
            trace_id = %trace_id,
            request_id = %request_id_for_log,
            candidate_id = ?candidate_id,
            plan_kind,
            status_code,
            provider_name,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            model_name,
            candidate_index = candidate_index.as_str(),
            failover_decision = failover_decision.as_str(),
            "gateway resolved execution runtime stream failover decision"
        );
        if matches!(failover_decision, LocalFailoverDecision::RetryNextCandidate) {
            let failure_disposition = classify_failure_disposition(
                &plan.provider_api_format,
                failover_analysis.classification,
                status_code,
            );
            if let Some(retry_scope) = retry_scope_out.as_deref_mut() {
                *retry_scope = ai_attempt_retry_scope_from_failure_disposition(failure_disposition);
            }
            if failure_disposition.preserve_upstream_error {
                if let Some(retry_fallback) = retry_fallback_out.as_deref_mut() {
                    let mut fallback_headers = headers.clone();
                    apply_endpoint_response_header_rules(
                        state,
                        &plan,
                        &mut fallback_headers,
                        provider_body_json.as_ref(),
                    )
                    .await?;
                    *retry_fallback = Some(attach_control_metadata_headers(
                        build_client_response_from_parts(
                            status_code,
                            &fallback_headers,
                            Body::from(provider_error_body.clone()),
                            trace_id,
                            Some(decision),
                        )?,
                        Some(request_id),
                        candidate_id,
                    )?);
                }
            }
            let terminal_unix_secs = current_request_candidate_unix_ms();
            let error_trace_report_context = with_stream_error_trace_context(
                report_context.as_ref(),
                status_code,
                &headers,
                provider_body_json.as_ref(),
                &provider_error_body,
                error_response_text.as_deref(),
                failover_analysis,
            );
            record_local_request_candidate_status(
                state,
                &plan,
                error_trace_report_context
                    .as_ref()
                    .or(report_context.as_ref()),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: Some(status_code),
                    error_type: Some("retryable_upstream_status".to_string()),
                    error_message: Some(format!(
                        "execution runtime stream returned retryable status {status_code}"
                    )),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            warn!(
                event_name = "local_stream_candidate_retry_scheduled",
                log_type = "event",
                trace_id = %trace_id,
                request_id = %request_id_for_log,
                status_code,
                provider_name = provider_name,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                model_name,
                candidate_index = candidate_index.as_str(),
                "gateway local stream decision retrying next candidate after retryable execution runtime status"
            );
            return Ok(None);
        }

        if !matches!(failover_decision, LocalFailoverDecision::StopLocalFailover)
            && should_fallback_to_control_stream(
                plan_kind,
                status_code,
                stream_error_finalize_kind.is_some(),
            )
        {
            let terminal_unix_secs = current_request_candidate_unix_ms();
            let error_trace_report_context = with_stream_error_trace_context(
                report_context.as_ref(),
                status_code,
                &headers,
                provider_body_json.as_ref(),
                &provider_error_body,
                error_response_text.as_deref(),
                failover_analysis,
            );
            record_local_request_candidate_status(
                state,
                &plan,
                error_trace_report_context
                    .as_ref()
                    .or(report_context.as_ref()),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: Some(status_code),
                    error_type: Some("control_fallback".to_string()),
                    error_message: Some(format!(
                        "stream decision fell back to control after status {status_code}"
                    )),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            return Ok(None);
        }

        let mut client_headers = if (300..400).contains(&status_code) {
            let mut headers = synthetic_error_response_headers(headers.clone());
            headers.insert(
                "x-aether-upstream-status".to_string(),
                status_code.to_string(),
            );
            headers
        } else {
            headers.clone()
        };
        if provider_private_error_decoded {
            client_headers.remove("content-encoding");
            client_headers.remove("content-length");
            client_headers.insert("content-type".to_string(), "application/json".to_string());
        }
        apply_endpoint_response_header_rules(
            state,
            &plan,
            &mut client_headers,
            client_body_json.as_ref(),
        )
        .await?;

        let client_response_headers = client_headers.clone();
        let error_trace_report_context = with_stream_error_trace_context(
            report_context.as_ref(),
            status_code,
            &headers,
            provider_body_json.as_ref(),
            &provider_error_body,
            error_response_text.as_deref(),
            failover_analysis,
        );
        let payload = build_stream_error_sync_payload(
            trace_id,
            stream_error_finalize_kind
                .as_deref()
                .or(report_kind.as_deref())
                .unwrap_or_default()
                .to_string(),
            error_trace_report_context.or(report_context),
            status_code,
            headers.clone(),
            provider_body_json,
            provider_body_base64,
            client_headers,
            payload_client_body_json,
            None,
        );
        record_sync_terminal_usage_with_handoff(
            state,
            &plan,
            payload.report_context.as_ref(),
            &payload,
        )
        .await;
        let terminal_unix_secs = current_request_candidate_unix_ms();
        record_local_request_candidate_status(
            state,
            &plan,
            payload.report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(status_code),
                error_type: Some("execution_runtime_stream_non_success_status".to_string()),
                error_message: Some(format!(
                    "execution runtime stream returned non-success status {status_code}"
                )),
                latency_ms: None,
                started_at_unix_ms: Some(candidate_started_unix_secs),
                finished_at_unix_ms: Some(terminal_unix_secs),
            },
        )
        .await;
        if stream_error_finalize_kind.is_some() {
            let response =
                submit_local_core_error_or_sync_finalize(state, trace_id, decision, payload)
                    .await?;
            return Ok(Some(attach_control_metadata_headers(
                response,
                Some(request_id),
                candidate_id,
            )?));
        }
        return Ok(Some(attach_control_metadata_headers(
            build_client_response_from_parts(
                client_status_code,
                &client_response_headers,
                Body::from(client_error_body),
                trace_id,
                Some(decision),
            )?,
            Some(request_id),
            candidate_id,
        )?));
    }

    let direct_stream_finalize_kind = resolve_core_stream_direct_finalize_report_kind(plan_kind);
    let normalized_stream_report_context =
        normalize_provider_private_report_context(report_context.as_ref());
    let upstream_headers = headers.clone();
    let mut private_stream_normalizer =
        maybe_build_provider_private_stream_normalizer(report_context.as_ref());
    let mut local_stream_rewriter =
        maybe_build_stream_response_rewriter(normalized_stream_report_context.as_ref());
    if private_stream_normalizer.is_some() || local_stream_rewriter.is_some() {
        headers.remove("content-encoding");
        headers.remove("content-length");
        headers.insert("content-type".to_string(), "text/event-stream".to_string());
    }
    let upstream_content_type = upstream_headers.get("content-type").map(String::as_str);
    let prefetch_for_cyber_failover =
        is_openai_responses_family_format(plan.provider_api_format.as_str())
            && cyber_continue_failover_enabled(state).await;
    let stream_commit_policy = StreamCommitPolicy::for_response(
        direct_stream_finalize_kind.is_some(),
        upstream_content_type,
        plan.provider_api_format.as_str(),
        plan.client_api_format.as_str(),
        private_stream_normalizer.is_some(),
        local_stream_rewriter.is_some(),
        prefetch_for_cyber_failover,
    );
    let reuse_committed_precommit =
        stream_precommit_committed && stream_commit_policy.is_native_anthropic();
    let skip_direct_finalize_prefetch =
        stream_commit_policy.commits_on_response_headers() || reuse_committed_precommit;
    let limit_direct_finalize_prefetch =
        should_limit_direct_finalize_prefetch(plan_kind, local_stream_rewriter.is_some())
            || stream_commit_policy.requires_bounded_frame_wait();
    // For JSON-classified responses (FirstClassifiedBody), transport frames are
    // not semantic boundaries: a single JSON error envelope may span more than
    // MAX_STREAM_PREFETCH_FRAMES data frames. Keep prefetching until the JSON
    // is complete (or the byte cap is reached) so an embedded provider error is
    // detected before the candidate stream is committed.
    //
    // The classifier treats a body as JSON when the header declares JSON or the
    // body (after BOM/whitespace strip) starts with '{' or '['. Keep both
    // signals in sync: when the header is missing/mislabeled but the body looks
    // like JSON, prefetching must also continue past the frame cap.
    let mut prefetch_waits_for_complete_json = matches!(
        stream_commit_policy,
        StreamCommitPolicy::FirstClassifiedBody
    ) && !limit_direct_finalize_prefetch
        && upstream_content_type.is_some_and(|content_type| {
            let content_type = content_type.to_ascii_lowercase();
            content_type.contains("json") || content_type.ends_with("+json")
        });
    let mut stream_commit_gate = StreamCommitGate::new(stream_commit_policy);
    let mut prefetch_client_completion_tracker = ClientVisibleStreamCompletionTracker::default();
    let mut prefetched_client_visible_stream_completed = false;
    let mut prefetched_anthropic_message_stop_observed_at = None;
    let mut prefetched_anthropic_post_stop_buffer_over_limit = false;
    if reuse_committed_precommit {
        stream_commit_gate.commit();
    }
    let mut prefetched_chunks: Vec<Bytes> = Vec::new();
    // Tracks the most recent classification of the prefetched body. Used after
    // the prefetch loop to detect a JSON body that reached the byte cap while
    // still incomplete (NeedMore): committing it would hide an embedded error
    // and disable failover, so it must go through the retryable failure path
    // instead.
    let mut last_prefetch_inspection: Option<StreamPrefetchInspection> = None;
    let mut provider_prefetched_body = Vec::new();
    let mut provider_prefetched_body_truncated = false;
    let mut prefetched_body = Vec::new();
    let mut prefetched_inspection_body = Vec::new();
    let mut prefetched_inspection_body_truncated = false;
    let mut prefetched_telemetry: Option<ExecutionTelemetry> = None;
    let mut prefetched_usage_telemetry: Option<ExecutionTelemetry> = None;
    let mut reached_eof = false;
    let mut sync_json_stream_bridge_active = false;
    let precommit_started_at = Instant::now();
    if skip_direct_finalize_prefetch {
        debug!(
            event_name = "execution_runtime_stream_prefetch_skipped",
            log_type = "debug",
            trace_id = %trace_id,
            request_id = %request_id_for_log,
            candidate_id = ?candidate_id,
            plan_kind,
            provider_name,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            model_name,
            candidate_index = candidate_index.as_str(),
            content_type = upstream_content_type.unwrap_or("-"),
            provider_api_format = plan.provider_api_format.as_str(),
            client_api_format = plan.client_api_format.as_str(),
            "gateway skipped direct finalize prefetch for same-format passthrough stream"
        );
    }
    if let Some(report_kind) = direct_stream_finalize_kind
        .as_ref()
        .filter(|_| !skip_direct_finalize_prefetch)
    {
        while (stream_commit_policy.requires_bounded_frame_wait()
            || prefetch_waits_for_complete_json
            || prefetched_chunks.len() < MAX_STREAM_PREFETCH_FRAMES)
            && prefetched_inspection_body.len() < MAX_STREAM_PREFETCH_BYTES
        {
            let next_frame_result = if limit_direct_finalize_prefetch {
                let prefetch_timeout = stream_commit_policy
                    .max_precommit_wait()
                    .map(|max_wait| max_wait.saturating_sub(precommit_started_at.elapsed()))
                    .unwrap_or(REWRITTEN_STREAM_PREFETCH_TIMEOUT);
                if prefetch_timeout.is_zero() {
                    stream_commit_gate.commit();
                    debug!(
                        event_name = "execution_runtime_stream_prefetch_limited",
                        log_type = "debug",
                        trace_id = %trace_id,
                        request_id = %request_id_for_log,
                        candidate_id = ?candidate_id,
                        plan_kind,
                        report_kind,
                        provider_name,
                        endpoint_id = %plan.endpoint_id,
                        key_id = %plan.key_id,
                        model_name,
                        candidate_index = candidate_index.as_str(),
                        timeout_ms = stream_commit_policy
                            .max_precommit_wait()
                            .unwrap_or(REWRITTEN_STREAM_PREFETCH_TIMEOUT)
                            .as_millis() as u64,
                        "gateway reached bounded stream precommit deadline"
                    );
                    break;
                }
                match tokio::time::timeout(
                    prefetch_timeout,
                    next_stream_frame(&mut buffered_frames, &mut lines),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        stream_commit_gate.commit();
                        debug!(
                            event_name = "execution_runtime_stream_prefetch_limited",
                            log_type = "debug",
                            trace_id = %trace_id,
                            request_id = %request_id_for_log,
                            candidate_id = ?candidate_id,
                            plan_kind,
                            report_kind,
                            provider_name,
                            endpoint_id = %plan.endpoint_id,
                            key_id = %plan.key_id,
                            model_name,
                            candidate_index = candidate_index.as_str(),
                            timeout_ms = stream_commit_policy
                                .max_precommit_wait()
                                .unwrap_or(REWRITTEN_STREAM_PREFETCH_TIMEOUT)
                                .as_millis() as u64,
                            "gateway stopped bounded stream prefetch before client-visible body"
                        );
                        break;
                    }
                }
            } else {
                next_stream_frame(&mut buffered_frames, &mut lines).await
            };
            let Some(observed_frame) = (match next_frame_result {
                Ok(frame) => frame,
                Err(err) => {
                    let failure = build_stream_failure_report(
                        "execution_runtime_stream_frame_decode_error",
                        format!("failed to decode execution runtime stream frame: {err:?}"),
                        502,
                    );
                    return handle_prefetch_stream_failure(
                        state,
                        trace_id,
                        decision,
                        &plan,
                        report_context,
                        request_id,
                        candidate_id,
                        report_kind,
                        headers,
                        prefetched_usage_telemetry.clone(),
                        &provider_prefetched_body,
                        candidate_started_unix_secs,
                        stream_elapsed_ms_since(stream_started_at),
                        failure,
                        None,
                    )
                    .await;
                }
            }) else {
                if stream_commit_policy.is_native_anthropic() && stream_commit_gate.is_uncommitted()
                {
                    let error_body_json = anthropic_premature_eof_error_body(
                        "upstream Anthropic stream ended before the first semantic event",
                    );
                    let error_status_code = anthropic_error_status_code(&error_body_json);
                    return handle_prefetch_provider_private_stream_error(
                        state,
                        trace_id,
                        decision,
                        &plan,
                        report_context,
                        request_id,
                        candidate_id,
                        report_kind,
                        headers,
                        prefetched_usage_telemetry.clone(),
                        &provider_prefetched_body,
                        status_code,
                        error_status_code,
                        error_body_json,
                        retry_scope_out.as_deref_mut(),
                        None,
                    )
                    .await;
                }
                reached_eof = true;
                break;
            };
            let frame_observed_at = observed_frame.observed_at;
            match observed_frame.frame.payload {
                StreamFramePayload::Data { chunk_b64, text } => {
                    if maybe_capture_first_stream_event_telemetry(
                        stream_started_at,
                        frame_observed_at,
                        prefetched_telemetry.as_ref(),
                        &mut prefetched_usage_telemetry,
                    ) {
                        observe_gateway_stage_trace_ms(
                            &mut stage_trace,
                            "stream_first_data",
                            stream_elapsed_ms_at(stream_started_at, frame_observed_at),
                        );
                    }
                    let mut chunk =
                        match decode_stream_data_chunk(chunk_b64.as_deref(), text.as_deref()) {
                            Ok(chunk) => chunk,
                            Err(err) => {
                                let failure = build_stream_failure_report(
                                    "execution_runtime_stream_chunk_decode_error",
                                    format!(
                                        "failed to decode execution runtime stream chunk: {err:?}"
                                    ),
                                    502,
                                );
                                return handle_prefetch_stream_failure(
                                    state,
                                    trace_id,
                                    decision,
                                    &plan,
                                    report_context,
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    headers,
                                    prefetched_usage_telemetry.clone(),
                                    &prefetched_body,
                                    candidate_started_unix_secs,
                                    stream_elapsed_ms_since(stream_started_at),
                                    failure,
                                    None,
                                )
                                .await;
                            }
                        };

                    if chunk.is_empty() {
                        continue;
                    }
                    if stream_commit_policy.is_native_anthropic() {
                        if prefetched_client_visible_stream_completed {
                            continue;
                        }
                        if let Some(terminal_end) = prefetch_client_completion_tracker
                            .observe_anthropic_message_stop_terminal_end(&chunk)
                        {
                            chunk.truncate(terminal_end);
                            prefetched_client_visible_stream_completed = true;
                            prefetched_anthropic_message_stop_observed_at
                                .get_or_insert_with(Instant::now);
                            prefetched_anthropic_post_stop_buffer_over_limit |=
                                activate_post_stop_frame_read_budget(&mut lines);
                        }
                    }

                    append_stream_capture_bytes(
                        &mut provider_prefetched_body,
                        &chunk,
                        MAX_STREAM_PREFETCH_BYTES,
                        &mut provider_prefetched_body_truncated,
                    );
                    append_stream_capture_bytes(
                        &mut prefetched_inspection_body,
                        &chunk,
                        MAX_STREAM_PREFETCH_BYTES,
                        &mut prefetched_inspection_body_truncated,
                    );

                    // Sync the prefetch wait flag with the classifier: a body
                    // that starts with '{' or '[' is treated as JSON even when
                    // the upstream Content-Type header is missing or mislabeled.
                    // Without this, a JSON error body split over more than
                    // MAX_STREAM_PREFETCH_FRAMES frames would still be committed
                    // after the frame cap and lose failover.
                    if matches!(
                        stream_commit_policy,
                        StreamCommitPolicy::FirstClassifiedBody
                    ) && !limit_direct_finalize_prefetch
                    {
                        let body = &prefetched_inspection_body;
                        let stripped = strip_utf8_bom_and_ws(body);
                        let looks_like_json = stripped.starts_with(b"{")
                            || stripped.starts_with(b"[")
                            || upstream_content_type.is_some_and(|content_type| {
                                let content_type = content_type.to_ascii_lowercase();
                                content_type.contains("json") || content_type.ends_with("+json")
                            });
                        prefetch_waits_for_complete_json = looks_like_json;
                    }

                    let anthropic_commit_ready =
                        match stream_commit_gate.observe_provider_bytes(&chunk) {
                            StreamPrecommitObservation::Pending => false,
                            StreamPrecommitObservation::Commit => true,
                            StreamPrecommitObservation::UpstreamError {
                                status_code: error_status_code,
                                body_json: error_body_json,
                            } => {
                                return handle_prefetch_provider_private_stream_error(
                                    state,
                                    trace_id,
                                    decision,
                                    &plan,
                                    report_context,
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    headers,
                                    prefetched_usage_telemetry.clone(),
                                    &provider_prefetched_body,
                                    status_code,
                                    error_status_code,
                                    error_body_json,
                                    retry_scope_out.as_deref_mut(),
                                    retry_fallback_out.as_deref_mut(),
                                )
                                .await;
                            }
                        };

                    if !anthropic_commit_ready {
                        if let Some(error_body_json) = extract_provider_private_stream_error_body(
                            report_context.as_ref(),
                            &prefetched_inspection_body,
                        ) {
                            let error_status_code = resolve_provider_stream_error_status_code(
                                plan.provider_api_format.as_str(),
                                status_code,
                                &error_body_json,
                            );
                            return handle_prefetch_provider_private_stream_error(
                                state,
                                trace_id,
                                decision,
                                &plan,
                                report_context,
                                request_id,
                                candidate_id,
                                report_kind,
                                headers,
                                prefetched_usage_telemetry.clone(),
                                &provider_prefetched_body,
                                status_code,
                                error_status_code,
                                error_body_json,
                                retry_scope_out.as_deref_mut(),
                                retry_fallback_out.as_deref_mut(),
                            )
                            .await;
                        }
                    }

                    let inspection = if stream_commit_policy.is_native_anthropic() {
                        StreamPrefetchInspection::NeedMore
                    } else {
                        inspect_prefetched_stream_body(
                            &upstream_headers,
                            &prefetched_inspection_body,
                        )
                    };
                    last_prefetch_inspection = Some(inspection.clone());
                    match inspection {
                        StreamPrefetchInspection::EmbeddedError(body_json) => {
                            debug!(
                                event_name = "execution_runtime_stream_prefetch_embedded_error_detected",
                                log_type = "debug",
                                trace_id = %trace_id,
                                request_id = %request_id_for_log,
                                candidate_id = ?candidate_id,
                                plan_kind,
                                report_kind,
                                provider_name,
                                endpoint_id = %plan.endpoint_id,
                                key_id = %plan.key_id,
                                model_name,
                                candidate_index = candidate_index.as_str(),
                                provider_prefetched_body_bytes = provider_prefetched_body.len(),
                                "gateway detected embedded error while prefetching execution runtime stream"
                            );
                            // Map the transport status (often HTTP 200) to the
                            // logical provider error status before failover
                            // analysis, so service_unavailable_error-style
                            // nested bodies classify as retryable 5xx instead of
                            // passing the raw 200 through to the classifier.
                            let error_status_code = resolve_provider_stream_error_status_code(
                                plan.provider_api_format.as_str(),
                                status_code,
                                &body_json,
                            );
                            // Keep diagnostic parity with the plain non-2xx
                            // path: attach upstream response body/headers to
                            // the report context so the failed candidate
                            // record retains the provider error payload even
                            // though the transport status was HTTP 200.
                            let trace_report_context = with_upstream_response_report_context(
                                report_context.as_ref(),
                                error_status_code,
                                Some(&upstream_headers),
                                Some(&body_json),
                                None,
                                None,
                            );
                            return handle_prefetch_provider_private_stream_error(
                                state,
                                trace_id,
                                decision,
                                &plan,
                                trace_report_context.or(report_context),
                                request_id,
                                candidate_id,
                                report_kind,
                                headers,
                                prefetched_usage_telemetry.clone(),
                                &provider_prefetched_body,
                                status_code,
                                error_status_code,
                                body_json,
                                retry_scope_out.as_deref_mut(),
                                retry_fallback_out.as_deref_mut(),
                            )
                            .await;
                        }
                        StreamPrefetchInspection::NeedMore => {}
                        StreamPrefetchInspection::NonError => {}
                        StreamPrefetchInspection::MalformedJson => {
                            // Complete but invalid JSON in the prefetch buffer is
                            // not a confirmed non-error response. Fail over like
                            // a malformed upstream body instead of committing
                            // the candidate stream and losing failover.
                            let failure = build_stream_retryable_upstream_failure_report(
                                "execution_runtime_stream_prefetch_malformed_json",
                                "prefetched execution runtime stream body is complete but not valid JSON".to_string(),
                                502,
                            );
                            return handle_prefetch_stream_failure(
                                state,
                                trace_id,
                                decision,
                                &plan,
                                report_context,
                                request_id,
                                candidate_id,
                                report_kind.as_str(),
                                headers,
                                prefetched_usage_telemetry.clone(),
                                &provider_prefetched_body,
                                candidate_started_unix_secs,
                                stream_elapsed_ms_since(stream_started_at),
                                failure,
                                retry_scope_out.as_deref_mut(),
                            )
                            .await;
                        }
                    }

                    if !response_headers_indicate_sse(&upstream_headers)
                        && (200..300).contains(&status_code)
                    {
                        if let Some(body_json) =
                            parse_prefetched_sync_json_body(&prefetched_inspection_body)
                        {
                            match maybe_bridge_standard_sync_json_to_stream(
                                &body_json,
                                plan.provider_api_format.as_str(),
                                plan.client_api_format.as_str(),
                                report_context.as_ref(),
                            ) {
                                Ok(Some(outcome)) => {
                                    if let Some(record) = outcome.response_history_record {
                                        crate::ai_serving::persist_response_history_record(
                                            state.runtime_state(),
                                            record,
                                        )
                                        .await;
                                    }
                                    headers.remove("content-encoding");
                                    headers.remove("content-length");
                                    headers.insert(
                                        "content-type".to_string(),
                                        "text/event-stream".to_string(),
                                    );
                                    stream_terminal_summary = outcome.terminal_summary;
                                    prefetched_body.extend_from_slice(&outcome.sse_body);
                                    prefetched_chunks.push(Bytes::from(outcome.sse_body));
                                    sync_json_stream_bridge_active = true;
                                    break;
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    let failure = build_stream_failure_report(
                                        "execution_runtime_sync_json_stream_bridge_error",
                                        format!(
                                            "failed to bridge execution runtime sync json to stream: {err:?}"
                                        ),
                                        502,
                                    );
                                    return handle_prefetch_stream_failure(
                                        state,
                                        trace_id,
                                        decision,
                                        &plan,
                                        report_context,
                                        request_id,
                                        candidate_id,
                                        report_kind,
                                        headers,
                                        prefetched_usage_telemetry.clone(),
                                        &provider_prefetched_body,
                                        candidate_started_unix_secs,
                                        stream_elapsed_ms_since(stream_started_at),
                                        failure,
                                        None,
                                    )
                                    .await;
                                }
                            }
                        }
                    }

                    let normalized_chunk = if let Some(normalizer) =
                        private_stream_normalizer.as_mut()
                    {
                        match normalizer.push_chunk(&chunk) {
                            Ok(normalized_chunk) => normalized_chunk,
                            Err(err) => {
                                let failure = build_stream_failure_report(
                                    "execution_runtime_stream_rewrite_error",
                                    format!(
                                        "failed to normalize execution runtime stream chunk: {err:?}"
                                    ),
                                    502,
                                );
                                return handle_prefetch_stream_failure(
                                    state,
                                    trace_id,
                                    decision,
                                    &plan,
                                    report_context,
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    headers,
                                    prefetched_usage_telemetry.clone(),
                                    &provider_prefetched_body,
                                    candidate_started_unix_secs,
                                    stream_elapsed_ms_since(stream_started_at),
                                    failure,
                                    None,
                                )
                                .await;
                            }
                        }
                    } else {
                        chunk
                    };
                    let rewritten_chunk = if let Some(rewriter) = local_stream_rewriter.as_mut() {
                        match rewriter.push_chunk(&normalized_chunk) {
                            Ok(rewritten_chunk) => rewritten_chunk,
                            Err(err) => {
                                let failure = build_stream_failure_report(
                                    "execution_runtime_stream_rewrite_error",
                                    format!(
                                        "failed to rewrite execution runtime stream chunk: {err:?}"
                                    ),
                                    502,
                                );
                                return handle_prefetch_stream_failure(
                                    state,
                                    trace_id,
                                    decision,
                                    &plan,
                                    report_context,
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    headers,
                                    prefetched_usage_telemetry.clone(),
                                    &provider_prefetched_body,
                                    candidate_started_unix_secs,
                                    stream_elapsed_ms_since(stream_started_at),
                                    failure,
                                    None,
                                )
                                .await;
                            }
                        }
                    } else {
                        normalized_chunk
                    };
                    if !rewritten_chunk.is_empty() {
                        prefetched_body.extend_from_slice(&rewritten_chunk);
                        prefetched_chunks.push(Bytes::from(rewritten_chunk));
                    }

                    if anthropic_commit_ready
                        || (matches!(inspection, StreamPrefetchInspection::NonError)
                            && (!prefetch_for_cyber_failover
                                || prefetched_openai_responses_body_has_output_boundary(
                                    &prefetched_inspection_body,
                                )))
                    {
                        break;
                    }
                }
                StreamFramePayload::Telemetry {
                    telemetry: frame_telemetry,
                } => {
                    prefetched_telemetry = Some(frame_telemetry);
                }
                StreamFramePayload::Eof { summary } => {
                    if stream_commit_policy.is_native_anthropic()
                        && stream_commit_gate.is_uncommitted()
                    {
                        let error_body_json = anthropic_premature_eof_error_body(
                            "upstream Anthropic stream ended before the first semantic event",
                        );
                        let error_status_code = anthropic_error_status_code(&error_body_json);
                        return handle_prefetch_provider_private_stream_error(
                            state,
                            trace_id,
                            decision,
                            &plan,
                            report_context,
                            request_id,
                            candidate_id,
                            report_kind,
                            headers,
                            prefetched_usage_telemetry.clone(),
                            &provider_prefetched_body,
                            status_code,
                            error_status_code,
                            error_body_json,
                            retry_scope_out.as_deref_mut(),
                            None,
                        )
                        .await;
                    }
                    if summary.is_some() {
                        stream_terminal_summary = summary;
                    }
                    reached_eof = true;
                    break;
                }
                StreamFramePayload::Error { error } => {
                    warn!(
                        event_name = "stream_execution_prefetch_error_frame",
                        log_type = "ops",
                        trace_id = %trace_id,
                        request_id,
                        candidate_id = ?candidate_id,
                        error = %error.message,
                        "execution runtime stream emitted error frame during prefetch"
                    );
                    return handle_prefetch_stream_failure(
                        state,
                        trace_id,
                        decision,
                        &plan,
                        report_context,
                        request_id,
                        candidate_id,
                        report_kind,
                        headers,
                        prefetched_usage_telemetry.clone(),
                        &provider_prefetched_body,
                        candidate_started_unix_secs,
                        stream_elapsed_ms_since(stream_started_at),
                        build_stream_failure_from_execution_error(&error),
                        retry_scope_out.as_deref_mut(),
                    )
                    .await;
                }
                StreamFramePayload::Headers { .. } => {}
            }
        }
    }
    if prefetch_waits_for_complete_json
        && matches!(
            last_prefetch_inspection,
            Some(StreamPrefetchInspection::NeedMore)
        )
        && !prefetched_inspection_body.is_empty()
    {
        // The JSON body could not be fully classified before the prefetch cap
        // (16 KiB) or stream end. Committing a partial JSON body would hide a
        // possible embedded provider error and lose failover, so surface it as
        // a retryable upstream failure instead.
        let failure = build_stream_retryable_upstream_failure_report(
            "execution_runtime_stream_prefetch_incomplete_json",
            "prefetched execution runtime stream JSON body exceeded the prefetch cap while incomplete".to_string(),
            502,
        );
        return handle_prefetch_stream_failure(
            state,
            trace_id,
            decision,
            &plan,
            report_context,
            request_id,
            candidate_id,
            report_kind.as_deref().unwrap_or_default(),
            headers,
            prefetched_usage_telemetry.clone(),
            &provider_prefetched_body,
            candidate_started_unix_secs,
            stream_elapsed_ms_since(stream_started_at),
            failure,
            retry_scope_out.as_deref_mut(),
        )
        .await;
    }
    if stream_commit_gate.is_uncommitted() {
        stream_commit_gate.commit();
    }
    let prefetched_response_history_persisted = if let Some(record) = local_stream_rewriter
        .as_mut()
        .and_then(|rewriter| rewriter.take_response_history_record())
    {
        crate::ai_serving::persist_response_history_record(state.runtime_state(), record).await;
        true
    } else {
        false
    };
    drop(private_stream_normalizer);
    drop(local_stream_rewriter);

    let initial_usage_telemetry = prefetched_usage_telemetry.clone().or_else(|| {
        prefetched_telemetry
            .as_ref()
            .map(|telemetry| usage_refresh_telemetry(telemetry, None))
    });
    state.usage_runtime.record_stream_started(
        state.usage_lifecycle_data_state().as_ref(),
        &lifecycle_seed,
        status_code,
        initial_usage_telemetry.as_ref(),
    );
    if let Some(snapshot) = request_candidate_status_snapshot {
        let latency_ms = prefetched_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.elapsed_ms);
        record_local_request_candidate_status_snapshot(
            state,
            &snapshot,
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Streaming,
                status_code: Some(status_code),
                error_type: None,
                error_message: None,
                latency_ms,
                started_at_unix_ms: Some(candidate_started_unix_secs),
                finished_at_unix_ms: None,
            },
        )
        .await;
    }

    apply_endpoint_response_header_rules(state, &plan, &mut headers, None).await?;

    let request_id = request_id.to_string();
    let candidate_id = candidate_id.map(ToOwned::to_owned);
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, IoError>>(16);
    let state_for_report = state.clone();
    let trace_id_owned = trace_id.to_string();
    let headers_for_report = headers.clone();
    let report_kind_owned = report_kind;
    let report_context_owned = report_context;
    let normalized_stream_report_context_owned = normalized_stream_report_context;
    let lifecycle_seed_for_report = lifecycle_seed;
    let provider_prefetched_body_for_report = provider_prefetched_body;
    let prefetched_body_for_report = prefetched_body;
    let prefetched_chunks_for_body = prefetched_chunks;
    let sync_json_stream_bridge_active_for_report = sync_json_stream_bridge_active;
    let initial_telemetry = prefetched_telemetry;
    let initial_reached_eof = reached_eof;
    let direct_stream_finalize_kind_owned = direct_stream_finalize_kind;
    let candidate_started_unix_secs_for_report = candidate_started_unix_secs;
    let request_id_for_report = request_id.clone();
    let request_id_for_report_log = short_request_id(&request_id);
    let candidate_id_for_report = candidate_id.clone();
    let candidate_index_for_report = candidate_index.clone();
    let is_openai_image_stream_for_report = plan_kind == OPENAI_IMAGE_STREAM_PLAN_KIND;
    let response_headers_are_sse = response_headers_indicate_sse(&headers);
    let emit_proxy_generated_sse_control_blocks =
        response_headers_are_sse && client_format_allows_proxy_generated_sse_control_blocks(&plan);
    let native_anthropic_stream_for_report = stream_commit_policy.is_native_anthropic();
    let plan_for_report = plan;
    let emit_passthrough_sse_terminal_error = (skip_direct_finalize_prefetch
        || stream_commit_policy.is_native_anthropic())
        && response_headers_indicate_sse(&upstream_headers)
        && !is_openai_image_stream_for_report;
    let plan_kind_for_report = plan_kind.to_string();
    let stream_started_at_for_report = stream_started_at;
    observe_gateway_stage_trace_ms(
        &mut stage_trace,
        "stream_response_ready",
        stream_elapsed_ms_since(stream_started_at),
    );
    let stage_trace_for_report = stage_trace;
    let request_diagnostics_for_report = current_request_diagnostics();
    let provider_pool_in_flight_guard_for_report = in_flight_guard;
    tokio::spawn(async move {
        let mut stage_trace_for_report = stage_trace_for_report;
        let _stream_total_guard =
            StageElapsedGuard::from_started_at("stream_total", stream_started_at_for_report);
        let _provider_pool_in_flight_guard = provider_pool_in_flight_guard_for_report;
        let mut provider_buffered_body = Vec::new();
        let mut buffered_body = Vec::new();
        let mut provider_body_truncated = false;
        let mut client_body_truncated = false;
        let mut private_stream_normalizer = if sync_json_stream_bridge_active_for_report {
            None
        } else {
            maybe_build_provider_private_stream_normalizer(report_context_owned.as_ref())
        };
        let mut local_stream_rewriter = if sync_json_stream_bridge_active_for_report {
            None
        } else {
            maybe_build_stream_response_rewriter(normalized_stream_report_context_owned.as_ref())
        };
        let stream_usage_report_context =
            normalized_stream_report_context_owned.clone().or_else(|| {
                Some(serde_json::json!({
                    "provider_api_format": plan_for_report.provider_api_format.as_str(),
                    "client_api_format": plan_for_report.client_api_format.as_str(),
                }))
            });
        let mut stream_usage_observer = stream_usage_report_context
            .as_ref()
            .filter(|_| !sync_json_stream_bridge_active_for_report)
            .map(|_| StreamingStandardTerminalObserver::default());
        let mut stream_usage_observer_buffered = Vec::new();
        let mut provider_error_inspection = ProviderStreamErrorInspection::default();
        append_stream_capture_bytes(
            &mut provider_buffered_body,
            &provider_prefetched_body_for_report,
            max_stream_body_buffer_bytes,
            &mut provider_body_truncated,
        );
        append_stream_capture_bytes(
            &mut buffered_body,
            &prefetched_body_for_report,
            max_stream_body_buffer_bytes,
            &mut client_body_truncated,
        );
        let mut client_stream_completion_tracker = ClientVisibleStreamCompletionTracker::default();
        let mut client_visible_stream_completed = if native_anthropic_stream_for_report {
            client_stream_completion_tracker
                .observe_anthropic_message_stop(&prefetched_body_for_report)
        } else {
            client_stream_completion_tracker.observe_chunk(&prefetched_body_for_report)
        };
        let mut anthropic_post_stop_drain_started_at = (native_anthropic_stream_for_report
            && client_visible_stream_completed)
            .then(|| prefetched_anthropic_message_stop_observed_at.unwrap_or_else(Instant::now));
        let mut anthropic_post_stop_buffer_over_limit =
            prefetched_anthropic_post_stop_buffer_over_limit;
        if anthropic_post_stop_drain_started_at.is_some()
            && prefetched_anthropic_message_stop_observed_at.is_none()
        {
            anthropic_post_stop_buffer_over_limit |=
                activate_post_stop_frame_read_budget(&mut lines);
        }
        let mut anthropic_post_stop_drain_frames = 0usize;
        let mut anthropic_post_stop_drain_bytes = 0usize;
        let mut usage_stream_telemetry: Option<ExecutionTelemetry> = initial_usage_telemetry;
        let mut telemetry: Option<ExecutionTelemetry> = initial_telemetry;
        let reached_eof = initial_reached_eof;
        let mut downstream_dropped = false;
        let mut terminal_failure: Option<StreamFailureReport> = None;
        let mut provider_error_forwarded_to_client = false;
        let initial_elapsed_ms = stream_started_at_for_report
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let last_upstream_frame_elapsed_ms = Arc::new(AtomicU64::new(initial_elapsed_ms));
        let last_client_chunk_elapsed_ms =
            Arc::new(AtomicU64::new(if prefetched_body_for_report.is_empty() {
                0
            } else {
                initial_elapsed_ms
            }));
        let provider_stream_bytes = Arc::new(AtomicU64::new(
            u64::try_from(provider_prefetched_body_for_report.len()).unwrap_or(u64::MAX),
        ));
        let client_stream_bytes = Arc::new(AtomicU64::new(
            u64::try_from(prefetched_body_for_report.len()).unwrap_or(u64::MAX),
        ));
        let idle_monitor_done = Arc::new(AtomicBool::new(false));
        let idle_monitor_handle = {
            let done = Arc::clone(&idle_monitor_done);
            let last_upstream = Arc::clone(&last_upstream_frame_elapsed_ms);
            let last_client = Arc::clone(&last_client_chunk_elapsed_ms);
            let provider_bytes = Arc::clone(&provider_stream_bytes);
            let client_bytes = Arc::clone(&client_stream_bytes);
            let trace_id_for_idle = trace_id_owned.clone();
            let request_id_for_idle = request_id_for_report_log.clone();
            let candidate_id_for_idle = candidate_id_for_report.clone();
            let candidate_index_for_idle = candidate_index_for_report.clone();
            let plan_kind_for_idle = plan_kind_for_report.clone();
            let provider_name_for_idle = plan_for_report
                .provider_name
                .clone()
                .unwrap_or_else(|| "-".to_string());
            let endpoint_id_for_idle = plan_for_report.endpoint_id.clone();
            let key_id_for_idle = plan_for_report.key_id.clone();
            let model_name_for_idle = plan_for_report
                .model_name
                .clone()
                .unwrap_or_else(|| "-".to_string());
            let has_local_stream_rewriter_for_idle = local_stream_rewriter.is_some();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(STREAM_IDLE_LOG_INTERVAL);
                interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    let elapsed_ms = stream_started_at_for_report
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64;
                    let last_upstream_frame_elapsed_ms = last_upstream.load(Ordering::Relaxed);
                    let last_client_chunk_elapsed_ms = last_client.load(Ordering::Relaxed);
                    let upstream_idle_ms =
                        elapsed_ms.saturating_sub(last_upstream_frame_elapsed_ms);
                    let client_idle_ms = if last_client_chunk_elapsed_ms == 0 {
                        elapsed_ms
                    } else {
                        elapsed_ms.saturating_sub(last_client_chunk_elapsed_ms)
                    };
                    if upstream_idle_ms >= STREAM_IDLE_LOG_INTERVAL_MS {
                        warn!(
                            event_name = "stream_execution_upstream_idle",
                            log_type = "ops",
                            trace_id = %trace_id_for_idle,
                            request_id = %request_id_for_idle,
                            candidate_id = ?candidate_id_for_idle.as_deref(),
                            candidate_index = candidate_index_for_idle.as_str(),
                            plan_kind = plan_kind_for_idle.as_str(),
                            provider_name = provider_name_for_idle.as_str(),
                            endpoint_id = %endpoint_id_for_idle,
                            key_id = %key_id_for_idle,
                            model_name = model_name_for_idle.as_str(),
                            elapsed_ms,
                            provider_bytes = provider_bytes.load(Ordering::Relaxed),
                            client_bytes = client_bytes.load(Ordering::Relaxed),
                            last_upstream_frame_elapsed_ms,
                            last_client_chunk_elapsed_ms,
                            "gateway stream has not received an upstream frame within the idle window"
                        );
                    } else if client_idle_ms >= STREAM_IDLE_LOG_INTERVAL_MS
                        && last_upstream_frame_elapsed_ms >= last_client_chunk_elapsed_ms
                    {
                        warn!(
                            event_name = "stream_execution_client_visible_idle",
                            log_type = "ops",
                            trace_id = %trace_id_for_idle,
                            request_id = %request_id_for_idle,
                            candidate_id = ?candidate_id_for_idle.as_deref(),
                            candidate_index = candidate_index_for_idle.as_str(),
                            plan_kind = plan_kind_for_idle.as_str(),
                            provider_name = provider_name_for_idle.as_str(),
                            endpoint_id = %endpoint_id_for_idle,
                            key_id = %key_id_for_idle,
                            model_name = model_name_for_idle.as_str(),
                            elapsed_ms,
                            provider_bytes = provider_bytes.load(Ordering::Relaxed),
                            client_bytes = client_bytes.load(Ordering::Relaxed),
                            last_upstream_frame_elapsed_ms,
                            last_client_chunk_elapsed_ms,
                            local_stream_rewriter = has_local_stream_rewriter_for_idle,
                            "gateway stream received upstream frames but has no recent client-visible chunk"
                        );
                    }
                }
            })
        };
        if !provider_prefetched_body_for_report.is_empty() {
            let normalized_prefetched_chunk = if let Some(normalizer) =
                private_stream_normalizer.as_mut()
            {
                match normalizer.push_chunk(&provider_prefetched_body_for_report) {
                    Ok(normalized_chunk) => Some(normalized_chunk),
                    Err(err) => {
                        warn!(
                            event_name = "stream_execution_prefetch_normalize_restore_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                            "gateway failed to restore private stream normalization state after prefetch"
                        );
                        terminal_failure = Some(build_stream_failure_report(
                            "execution_runtime_stream_rewrite_error",
                            format!(
                                "failed to restore private stream normalization state after prefetch: {err:?}"
                            ),
                            502,
                        ));
                        None
                    }
                }
            } else {
                None
            };
            let replay_chunk = normalized_prefetched_chunk
                .as_deref()
                .unwrap_or(provider_prefetched_body_for_report.as_slice());
            if let Some(error_body_json) = provider_error_inspection
                .observe(stream_usage_report_context.as_ref(), replay_chunk)
            {
                provider_error_forwarded_to_client = !prefetched_body_for_report.is_empty();
                let error_status_code = resolve_provider_stream_error_status_code(
                    plan_for_report.provider_api_format.as_str(),
                    status_code,
                    &error_body_json,
                );
                terminal_failure = Some(build_stream_failure_from_provider_error_body(
                    error_status_code,
                    &error_body_json,
                ));
            }
            if let (Some(observer), Some(report_context)) = (
                stream_usage_observer.as_mut(),
                stream_usage_report_context.as_ref(),
            ) {
                observe_stream_usage_bytes(
                    observer,
                    report_context,
                    &mut stream_usage_observer_buffered,
                    replay_chunk,
                );
            }
            if terminal_failure.is_none() {
                if let Some(rewriter) = local_stream_rewriter.as_mut() {
                    if let Err(err) = rewriter.push_chunk(replay_chunk) {
                        warn!(
                            event_name = "stream_execution_prefetch_rewrite_restore_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                            "gateway failed to restore local stream rewrite state after prefetch"
                        );
                        terminal_failure = Some(build_stream_failure_report(
                            "execution_runtime_stream_rewrite_error",
                            format!(
                                "failed to restore local stream rewrite state after prefetch: {err:?}"
                            ),
                            502,
                        ));
                    }
                }
            }
            if prefetched_response_history_persisted {
                if let Some(rewriter) = local_stream_rewriter.as_mut() {
                    let _ = rewriter.take_response_history_record();
                }
            }
        }

        if terminal_failure.is_none() && !reached_eof {
            loop {
                let draining_after_anthropic_stop = anthropic_post_stop_drain_started_at.is_some();
                let next_frame_result = if let Some(drain_started_at) =
                    anthropic_post_stop_drain_started_at
                {
                    if anthropic_post_stop_drain_frames >= ANTHROPIC_POST_STOP_DRAIN_MAX_FRAMES
                        || anthropic_post_stop_drain_bytes >= ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES
                        || anthropic_post_stop_buffer_over_limit
                    {
                        break;
                    }
                    let remaining = ANTHROPIC_POST_STOP_DRAIN_MAX_WAIT
                        .saturating_sub(drain_started_at.elapsed());
                    if remaining.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(
                        remaining,
                        next_stream_frame(&mut buffered_frames, &mut lines),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => break,
                    }
                } else {
                    tokio::select! {
                        biased;
                        _ = tx.closed(), if !downstream_dropped => {
                            downstream_dropped = true;
                            break;
                        }
                        result = next_stream_frame(&mut buffered_frames, &mut lines) => result,
                    }
                };
                let next_frame = match next_frame_result {
                    Ok(frame) => frame,
                    Err(err) => {
                        if native_anthropic_stream_for_report && client_visible_stream_completed {
                            debug!(
                                event_name = "stream_execution_frame_decode_ignored_after_anthropic_stop",
                                log_type = "debug",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                error = ?err,
                                "gateway ignored execution runtime teardown error after Anthropic message_stop"
                            );
                            break;
                        }
                        warn!(
                            event_name = "stream_execution_frame_decode_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                            "gateway failed to decode execution runtime stream frame"
                        );
                        terminal_failure = Some(build_stream_failure_report(
                            "execution_runtime_stream_frame_decode_error",
                            format!("failed to decode execution runtime stream frame: {err:?}"),
                            502,
                        ));
                        break;
                    }
                };
                let Some(observed_frame) = next_frame else {
                    if tx.is_closed() {
                        downstream_dropped = true;
                    } else if native_anthropic_stream_for_report && !client_visible_stream_completed
                    {
                        terminal_failure = Some(build_anthropic_premature_eof_failure(
                            "upstream Anthropic stream ended before message_stop",
                        ));
                    }
                    break;
                };
                if draining_after_anthropic_stop {
                    anthropic_post_stop_drain_frames =
                        anthropic_post_stop_drain_frames.saturating_add(1);
                    let frame_bytes = serialized_stream_frame_len(&observed_frame.frame);
                    if frame_bytes
                        > ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES
                            .saturating_sub(anthropic_post_stop_drain_bytes)
                    {
                        break;
                    }
                    anthropic_post_stop_drain_bytes =
                        anthropic_post_stop_drain_bytes.saturating_add(frame_bytes);
                }
                let frame_observed_at = observed_frame.observed_at;
                let frame_elapsed_ms =
                    stream_elapsed_ms_at(stream_started_at_for_report, frame_observed_at);
                last_upstream_frame_elapsed_ms.store(frame_elapsed_ms, Ordering::Relaxed);
                match observed_frame.frame.payload {
                    StreamFramePayload::Data { chunk_b64, text } => {
                        if native_anthropic_stream_for_report && client_visible_stream_completed {
                            continue;
                        }
                        let first_data_before = usage_stream_telemetry
                            .as_ref()
                            .and_then(|telemetry| telemetry.ttfb_ms)
                            .is_some();
                        if maybe_capture_first_stream_event_telemetry(
                            stream_started_at_for_report,
                            frame_observed_at,
                            telemetry.as_ref(),
                            &mut usage_stream_telemetry,
                        ) {
                            state_for_report.usage_runtime.record_stream_started(
                                state_for_report.usage_lifecycle_data_state().as_ref(),
                                &lifecycle_seed_for_report,
                                status_code,
                                usage_stream_telemetry.as_ref(),
                            );
                        }
                        let first_data_after = usage_stream_telemetry
                            .as_ref()
                            .and_then(|telemetry| telemetry.ttfb_ms)
                            .is_some();
                        if !first_data_before && first_data_after {
                            observe_gateway_stage_trace_ms(
                                &mut stage_trace_for_report,
                                "stream_first_data",
                                stream_elapsed_ms_at(
                                    stream_started_at_for_report,
                                    frame_observed_at,
                                ),
                            );
                        }
                        if sync_json_stream_bridge_active_for_report {
                            continue;
                        }
                        let mut chunk =
                            match decode_stream_data_chunk(chunk_b64.as_deref(), text.as_deref()) {
                                Ok(chunk) => chunk,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_chunk_decode_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to decode execution runtime chunk"
                                    );
                                    terminal_failure = Some(build_stream_failure_report(
                                        "execution_runtime_stream_chunk_decode_error",
                                        format!(
                                        "failed to decode execution runtime stream chunk: {err:?}"
                                    ),
                                        502,
                                    ));
                                    break;
                                }
                            };

                        if chunk.is_empty() {
                            continue;
                        }
                        if native_anthropic_stream_for_report {
                            if let Some(terminal_end) = client_stream_completion_tracker
                                .observe_anthropic_message_stop_terminal_end(&chunk)
                            {
                                chunk.truncate(terminal_end);
                                client_visible_stream_completed = true;
                                if anthropic_post_stop_drain_started_at.is_none() {
                                    anthropic_post_stop_drain_started_at = Some(Instant::now());
                                    anthropic_post_stop_buffer_over_limit |=
                                        activate_post_stop_frame_read_budget(&mut lines);
                                }
                            }
                        }

                        provider_stream_bytes.fetch_add(
                            u64::try_from(chunk.len()).unwrap_or(u64::MAX),
                            Ordering::Relaxed,
                        );
                        append_stream_capture_bytes(
                            &mut provider_buffered_body,
                            &chunk,
                            max_stream_body_buffer_bytes,
                            &mut provider_body_truncated,
                        );
                        let normalized_chunk = if let Some(normalizer) =
                            private_stream_normalizer.as_mut()
                        {
                            match normalizer.push_chunk(&chunk) {
                                Ok(normalized_chunk) => normalized_chunk,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_chunk_normalize_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to normalize execution runtime stream chunk"
                                    );
                                    terminal_failure = Some(build_stream_failure_report(
                                            "execution_runtime_stream_rewrite_error",
                                            format!("failed to normalize execution runtime stream chunk: {err:?}"),
                                            502,
                                        ));
                                    break;
                                }
                            }
                        } else {
                            chunk
                        };
                        let provider_private_error_body_json = provider_error_inspection
                            .observe(stream_usage_report_context.as_ref(), &normalized_chunk);
                        if let (Some(observer), Some(report_context)) = (
                            stream_usage_observer.as_mut(),
                            stream_usage_report_context.as_ref(),
                        ) {
                            observe_stream_usage_bytes(
                                observer,
                                report_context,
                                &mut stream_usage_observer_buffered,
                                &normalized_chunk,
                            );
                        }
                        let rewritten_chunk = if let Some(rewriter) = local_stream_rewriter.as_mut()
                        {
                            match rewriter.push_chunk(&normalized_chunk) {
                                Ok(rewritten_chunk) => rewritten_chunk,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_chunk_rewrite_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to rewrite execution runtime stream chunk"
                                    );
                                    terminal_failure = Some(build_stream_failure_report(
                                        "execution_runtime_stream_rewrite_error",
                                        format!("failed to rewrite execution runtime stream chunk: {err:?}"),
                                        502,
                                    ));
                                    break;
                                }
                            }
                        } else {
                            normalized_chunk
                        };

                        if provider_private_error_body_json.is_none() {
                            if let Some(record) = local_stream_rewriter
                                .as_mut()
                                .and_then(|rewriter| rewriter.take_response_history_record())
                            {
                                crate::ai_serving::persist_response_history_record(
                                    state_for_report.runtime_state(),
                                    record,
                                )
                                .await;
                            }
                        }

                        if rewritten_chunk.is_empty() {
                            if let Some(error_body_json) = provider_private_error_body_json {
                                let error_status_code = resolve_provider_stream_error_status_code(
                                    plan_for_report.provider_api_format.as_str(),
                                    status_code,
                                    &error_body_json,
                                );
                                terminal_failure =
                                    Some(build_stream_failure_from_provider_error_body(
                                        error_status_code,
                                        &error_body_json,
                                    ));
                                break;
                            }
                            continue;
                        }

                        append_stream_capture_bytes(
                            &mut buffered_body,
                            &rewritten_chunk,
                            max_stream_body_buffer_bytes,
                            &mut client_body_truncated,
                        );
                        let rewritten_chunk_len =
                            u64::try_from(rewritten_chunk.len()).unwrap_or(u64::MAX);
                        if downstream_dropped {
                            continue;
                        }
                        let rewritten_chunk = Bytes::from(rewritten_chunk);
                        if tx.send(Ok(rewritten_chunk.clone())).await.is_err() {
                            debug!(
                                event_name = "stream_execution_downstream_disconnected",
                                log_type = "ops",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                "gateway stream downstream dropped; cancelling execution runtime stream"
                            );
                            downstream_dropped = true;
                            break;
                        } else {
                            if !native_anthropic_stream_for_report {
                                client_visible_stream_completed |= client_stream_completion_tracker
                                    .observe_chunk(rewritten_chunk.as_ref());
                            }
                            client_stream_bytes.fetch_add(rewritten_chunk_len, Ordering::Relaxed);
                            last_client_chunk_elapsed_ms.store(
                                stream_started_at_for_report
                                    .elapsed()
                                    .as_millis()
                                    .min(u128::from(u64::MAX))
                                    as u64,
                                Ordering::Relaxed,
                            );
                            provider_error_forwarded_to_client =
                                provider_private_error_body_json.is_some();
                        }
                        if let Some(error_body_json) = provider_private_error_body_json {
                            let error_status_code = resolve_provider_stream_error_status_code(
                                plan_for_report.provider_api_format.as_str(),
                                status_code,
                                &error_body_json,
                            );
                            terminal_failure = Some(build_stream_failure_from_provider_error_body(
                                error_status_code,
                                &error_body_json,
                            ));
                            break;
                        }
                    }
                    StreamFramePayload::Telemetry {
                        telemetry: frame_telemetry,
                    } => {
                        let usage_frame_telemetry = usage_refresh_telemetry(
                            &frame_telemetry,
                            usage_stream_telemetry.as_ref(),
                        );
                        let should_refresh_stream_usage = should_refresh_stream_usage_telemetry(
                            usage_stream_telemetry.as_ref(),
                            &usage_frame_telemetry,
                        );
                        if should_refresh_stream_usage {
                            // The first Data frame records the live streaming transition. Later
                            // telemetry frames only refine the terminal accumulator; persisting
                            // every elapsed-time update would create one usage task per frame.
                            usage_stream_telemetry = Some(usage_frame_telemetry);
                        }
                        telemetry = Some(frame_telemetry);
                    }
                    StreamFramePayload::Eof { summary } => {
                        stream_terminal_summary =
                            merge_stream_terminal_summary(stream_terminal_summary.take(), summary);
                        if native_anthropic_stream_for_report && !client_visible_stream_completed {
                            terminal_failure = Some(build_anthropic_premature_eof_failure(
                                "upstream Anthropic stream ended before message_stop",
                            ));
                        }
                        break;
                    }
                    StreamFramePayload::Error { error } => {
                        if native_anthropic_stream_for_report && client_visible_stream_completed {
                            debug!(
                                event_name = "stream_execution_error_frame_ignored_after_anthropic_stop",
                                log_type = "debug",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                error = %error.message,
                                "gateway ignored execution runtime error frame after Anthropic message_stop"
                            );
                            continue;
                        }
                        warn!(
                            event_name = "stream_execution_error_frame",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = %error.message,
                            "execution runtime stream emitted error frame"
                        );
                        terminal_failure = Some(build_stream_failure_from_execution_error(&error));
                        break;
                    }
                    StreamFramePayload::Headers { .. } => {}
                }
            }
        }
        drop(lines);
        drop(buffered_frames);
        drop(_provider_pool_in_flight_guard);

        if downstream_dropped {
            debug!(
                event_name = "execution_runtime_stream_client_flush_skipped",
                log_type = "debug",
                debug_context = "redacted",
                stream_status = "downstream_disconnected",
                trace_id = %trace_id_owned,
                "gateway skipped client stream flush after downstream disconnect"
            );
        }
        // Buffered stream state is partial after a terminal failure; normal
        // finish paths may synthesize successful terminal events.
        let should_finish_stream_rewriters = terminal_failure.is_none();
        if let Some(normalizer) = private_stream_normalizer
            .as_mut()
            .filter(|_| should_finish_stream_rewriters)
        {
            match normalizer.finish() {
                Ok(normalized_chunk) if !normalized_chunk.is_empty() => {
                    let provider_private_error_body_json = provider_error_inspection
                        .observe(stream_usage_report_context.as_ref(), &normalized_chunk);
                    if let (Some(observer), Some(report_context)) = (
                        stream_usage_observer.as_mut(),
                        stream_usage_report_context.as_ref(),
                    ) {
                        observe_stream_usage_bytes(
                            observer,
                            report_context,
                            &mut stream_usage_observer_buffered,
                            &normalized_chunk,
                        );
                    }
                    if !downstream_dropped {
                        let rewritten_chunk = if let Some(rewriter) = local_stream_rewriter.as_mut()
                        {
                            match rewriter.push_chunk(&normalized_chunk) {
                                Ok(rewritten_chunk) => rewritten_chunk,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_normalized_flush_rewrite_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to rewrite normalized private stream chunk during flush"
                                    );
                                    let failure = build_stream_failure_report(
                                        "execution_runtime_stream_rewrite_flush_error",
                                        format!("failed to rewrite normalized private stream chunk during flush: {err:?}"),
                                        502,
                                    );
                                    terminal_failure.get_or_insert(failure);
                                    Vec::new()
                                }
                            }
                        } else {
                            normalized_chunk
                        };
                        if provider_private_error_body_json.is_none() {
                            if let Some(record) = local_stream_rewriter
                                .as_mut()
                                .and_then(|rewriter| rewriter.take_response_history_record())
                            {
                                crate::ai_serving::persist_response_history_record(
                                    state_for_report.runtime_state(),
                                    record,
                                )
                                .await;
                            }
                        }
                        if !rewritten_chunk.is_empty() {
                            append_stream_capture_bytes(
                                &mut buffered_body,
                                &rewritten_chunk,
                                max_stream_body_buffer_bytes,
                                &mut client_body_truncated,
                            );
                            let rewritten_chunk_len =
                                u64::try_from(rewritten_chunk.len()).unwrap_or(u64::MAX);
                            let rewritten_chunk = Bytes::from(rewritten_chunk);
                            if tx.send(Ok(rewritten_chunk.clone())).await.is_err() {
                                warn!(
                                    event_name = "stream_execution_downstream_flush_disconnected",
                                    log_type = "ops",
                                    trace_id = %trace_id_owned,
                                    request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                "gateway stream downstream dropped while flushing private stream normalization"
                                );
                                downstream_dropped = true;
                            } else {
                                client_visible_stream_completed |= client_stream_completion_tracker
                                    .observe_chunk(rewritten_chunk.as_ref());
                                client_stream_bytes
                                    .fetch_add(rewritten_chunk_len, Ordering::Relaxed);
                                last_client_chunk_elapsed_ms.store(
                                    stream_started_at_for_report
                                        .elapsed()
                                        .as_millis()
                                        .min(u128::from(u64::MAX))
                                        as u64,
                                    Ordering::Relaxed,
                                );
                            }
                        }
                        if let Some(error_body_json) = provider_private_error_body_json {
                            let error_status_code = resolve_provider_stream_error_status_code(
                                plan_for_report.provider_api_format.as_str(),
                                status_code,
                                &error_body_json,
                            );
                            terminal_failure.get_or_insert_with(|| {
                                build_stream_failure_from_provider_error_body(
                                    error_status_code,
                                    &error_body_json,
                                )
                            });
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(
                        event_name = "stream_execution_normalization_flush_failed",
                        log_type = "ops",
                        trace_id = %trace_id_owned,
                        request_id = %request_id_for_report_log,
                        candidate_id = ?candidate_id_for_report.as_deref(),
                        error = ?err,
                        "gateway failed to flush private stream normalization"
                    );
                    terminal_failure.get_or_insert_with(|| {
                        build_stream_failure_report(
                            "execution_runtime_stream_rewrite_flush_error",
                            format!("failed to flush private stream normalization: {err:?}"),
                            502,
                        )
                    });
                }
            }
        }
        if !downstream_dropped && terminal_failure.is_none() {
            if let Some(rewriter) = local_stream_rewriter.as_mut() {
                let finish_result = rewriter.finish();
                if let Some(record) = rewriter.take_response_history_record() {
                    crate::ai_serving::persist_response_history_record(
                        state_for_report.runtime_state(),
                        record,
                    )
                    .await;
                }
                match finish_result {
                    Ok(flushed_chunk) if !flushed_chunk.is_empty() => {
                        append_stream_capture_bytes(
                            &mut buffered_body,
                            &flushed_chunk,
                            max_stream_body_buffer_bytes,
                            &mut client_body_truncated,
                        );
                        let flushed_chunk_len =
                            u64::try_from(flushed_chunk.len()).unwrap_or(u64::MAX);
                        let flushed_chunk = Bytes::from(flushed_chunk);
                        if tx.send(Ok(flushed_chunk.clone())).await.is_err() {
                            warn!(
                                event_name = "stream_execution_downstream_rewrite_flush_disconnected",
                                log_type = "ops",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            "gateway stream downstream dropped while flushing local stream rewrite"
                            );
                            downstream_dropped = true;
                        } else {
                            client_visible_stream_completed |= client_stream_completion_tracker
                                .observe_chunk(flushed_chunk.as_ref());
                            client_stream_bytes.fetch_add(flushed_chunk_len, Ordering::Relaxed);
                            last_client_chunk_elapsed_ms.store(
                                stream_started_at_for_report
                                    .elapsed()
                                    .as_millis()
                                    .min(u128::from(u64::MAX))
                                    as u64,
                                Ordering::Relaxed,
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!(
                            event_name = "stream_execution_rewrite_flush_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                            "gateway failed to flush local stream rewrite"
                        );
                        terminal_failure.get_or_insert_with(|| {
                            build_stream_failure_report(
                                "execution_runtime_stream_rewrite_flush_error",
                                format!("failed to flush local stream rewrite: {err:?}"),
                                502,
                            )
                        });
                    }
                }
            }
        }

        if terminal_failure.is_none() {
            if let Some(record) = local_stream_rewriter
                .as_mut()
                .and_then(|rewriter| rewriter.take_response_history_record())
            {
                crate::ai_serving::persist_response_history_record(
                    state_for_report.runtime_state(),
                    record,
                )
                .await;
            }
        }

        if !downstream_dropped {
            if let Some(failure) = terminal_failure.as_ref() {
                let terminal_event = if is_openai_image_stream_for_report {
                    Some(encode_openai_image_failed_event(
                        report_context_owned.as_ref(),
                        failure,
                    ))
                } else if emit_passthrough_sse_terminal_error && !provider_error_forwarded_to_client
                {
                    Some(encode_terminal_sse_error_event_for_plan(
                        &plan_for_report,
                        failure,
                    ))
                } else {
                    None
                };
                if let Some(terminal_event) = terminal_event {
                    match terminal_event {
                        Ok(error_event) => {
                            let error_event_len =
                                u64::try_from(error_event.len()).unwrap_or(u64::MAX);
                            append_stream_capture_bytes(
                                &mut buffered_body,
                                error_event.as_ref(),
                                max_stream_body_buffer_bytes,
                                &mut client_body_truncated,
                            );
                            if tx.send(Ok(error_event)).await.is_err() {
                                warn!(
                                event_name = "stream_execution_downstream_terminal_error_disconnected",
                                log_type = "ops",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                "gateway stream downstream dropped while sending terminal SSE error event"
                                );
                                downstream_dropped = true;
                            } else {
                                client_stream_bytes.fetch_add(error_event_len, Ordering::Relaxed);
                                last_client_chunk_elapsed_ms.store(
                                    stream_started_at_for_report
                                        .elapsed()
                                        .as_millis()
                                        .min(u128::from(u64::MAX))
                                        as u64,
                                    Ordering::Relaxed,
                                );
                            }
                        }
                        Err(err) => {
                            warn!(
                            event_name = "stream_execution_terminal_error_event_encode_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                                "gateway failed to encode terminal SSE error event"
                            );
                        }
                    }
                }
            }
        }

        drop(tx);
        idle_monitor_done.store(true, Ordering::Relaxed);
        idle_monitor_handle.abort();

        stream_terminal_summary = merge_stream_terminal_summary(
            stream_terminal_summary,
            finalize_stream_usage_observer(
                &mut stream_usage_observer,
                stream_usage_report_context.as_ref(),
                &mut stream_usage_observer_buffered,
            ),
        );

        if downstream_dropped && client_visible_stream_completed && terminal_failure.is_none() {
            debug!(
                event_name = "execution_runtime_stream_downstream_closed_after_done",
                log_type = "debug",
                trace_id = %trace_id_owned,
                request_id = %request_id_for_report_log,
                candidate_id = ?candidate_id_for_report.as_deref(),
                "gateway treats downstream close after client-visible SSE DONE as completed"
            );
            downstream_dropped = false;
        }

        if downstream_dropped {
            debug!(
                event_name = "execution_runtime_stream_report_skipped",
                log_type = "debug",
                debug_context = "redacted",
                stream_status = "downstream_disconnected",
                status_code = 499_u16,
                trace_id = %trace_id_owned,
                "gateway skipped stream report because downstream disconnected before completion"
            );
            let terminal_telemetry = Some(build_terminal_stream_telemetry(
                stream_started_at_for_report,
                telemetry.as_ref(),
                usage_stream_telemetry.as_ref(),
                provider_stream_bytes.load(Ordering::Relaxed),
            ));
            let report_context_for_payload = report_context_with_stage_trace(
                report_context_owned,
                stage_trace_for_report,
                stream_started_at_for_report,
                terminal_telemetry.as_ref(),
            );
            let report_context_for_payload = report_context_with_request_diagnostics(
                report_context_for_payload,
                request_diagnostics_for_report.as_ref(),
                stream_started_at_for_report,
                terminal_telemetry.as_ref(),
            );
            let usage_payload = build_stream_usage_payload(
                trace_id_owned,
                report_kind_owned.unwrap_or_default(),
                report_context_for_payload,
                499,
                headers_for_report,
                &provider_buffered_body,
                provider_body_truncated,
                &buffered_body,
                client_body_truncated,
                stream_terminal_summary,
                terminal_telemetry,
            );
            record_stream_terminal_usage(
                &state_for_report,
                &plan_for_report,
                usage_payload.report_context.as_ref(),
                &usage_payload,
                true,
            )
            .await;
            record_local_request_candidate_status(
                &state_for_report,
                &plan_for_report,
                usage_payload.report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Cancelled,
                    status_code: Some(499),
                    error_type: Some("downstream_disconnect".to_string()),
                    error_message: Some("client disconnected before stream completion".to_string()),
                    latency_ms: usage_payload
                        .telemetry
                        .as_ref()
                        .and_then(|value| value.elapsed_ms),
                    started_at_unix_ms: Some(candidate_started_unix_secs_for_report),
                    finished_at_unix_ms: Some(current_request_candidate_unix_ms()),
                },
            )
            .await;
            return;
        }

        if let Some(failure) = terminal_failure {
            record_manual_proxy_stream_error(&state_for_report, &plan_for_report).await;
            let terminal_telemetry = Some(build_terminal_stream_telemetry(
                stream_started_at_for_report,
                telemetry.as_ref(),
                usage_stream_telemetry.as_ref(),
                provider_stream_bytes.load(Ordering::Relaxed),
            ));
            let report_context_for_payload = report_context_with_stage_trace(
                report_context_owned,
                stage_trace_for_report,
                stream_started_at_for_report,
                terminal_telemetry.as_ref(),
            );
            let report_context_for_payload = report_context_with_request_diagnostics(
                report_context_for_payload,
                request_diagnostics_for_report.as_ref(),
                stream_started_at_for_report,
                terminal_telemetry.as_ref(),
            );
            submit_midstream_stream_failure(
                &state_for_report,
                &trace_id_owned,
                &plan_for_report,
                direct_stream_finalize_kind_owned.as_deref(),
                report_context_for_payload,
                headers_for_report,
                terminal_telemetry,
                &provider_buffered_body,
                candidate_started_unix_secs_for_report,
                failure,
            )
            .await;
            return;
        }

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state_for_report,
            &plan_for_report,
            report_context_owned.as_ref(),
            &mut stream_terminal_summary,
        )
        .await;
        let requires_observed_terminal_event = stream_requires_observed_terminal_event(
            plan_for_report.provider_api_format.as_str(),
            stream_usage_report_context.as_ref(),
        );
        ensure_stream_terminal_summary_for_missing_observed_finish(
            &mut stream_terminal_summary,
            requires_observed_terminal_event,
        );
        let missing_observed_finish =
            stream_terminal_summary_missing_observed_finish_with_requirement(
                stream_terminal_summary.as_ref(),
                requires_observed_terminal_event,
            );

        let should_submit_report = report_kind_owned.is_some();
        let terminal_telemetry = Some(build_terminal_stream_telemetry(
            stream_started_at_for_report,
            telemetry.as_ref(),
            usage_stream_telemetry.as_ref(),
            provider_stream_bytes.load(Ordering::Relaxed),
        ));
        let stream_failed = stream_terminal_summary_represents_failure_with_requirement(
            stream_terminal_summary.as_ref(),
            requires_observed_terminal_event,
        );
        let stream_terminal_error_message = stream_terminal_summary
            .as_ref()
            .and_then(|summary| summary.parser_error.clone())
            .or_else(|| {
                missing_observed_finish.then(|| {
                    "execution runtime stream ended before provider terminal event".to_string()
                })
            });
        let report_context_for_payload = report_context_with_stage_trace(
            report_context_owned,
            stage_trace_for_report,
            stream_started_at_for_report,
            terminal_telemetry.as_ref(),
        );
        let report_context_for_payload = report_context_with_request_diagnostics(
            report_context_for_payload,
            request_diagnostics_for_report.as_ref(),
            stream_started_at_for_report,
            terminal_telemetry.as_ref(),
        );
        let usage_payload = build_stream_usage_payload(
            trace_id_owned.clone(),
            report_kind_owned.unwrap_or_default(),
            report_context_for_payload,
            status_code,
            headers_for_report,
            &provider_buffered_body,
            provider_body_truncated,
            &buffered_body,
            client_body_truncated,
            stream_terminal_summary,
            terminal_telemetry,
        );
        if stream_failed {
            warn!(
                event_name = "execution_runtime_stream_missing_terminal_event",
                log_type = "ops",
                trace_id = %trace_id_owned,
                request_id = %request_id_for_report_log,
                candidate_id = ?candidate_id_for_report.as_deref(),
                status_code,
                error_message = stream_terminal_error_message.as_deref().unwrap_or_default(),
                "gateway stream ended with a failed terminal state"
            );
        } else {
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
            )
            .await;
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::AdaptiveSuccess(LocalAdaptiveSuccessEffect),
            )
            .await;
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::PoolSuccessStream {
                    payload: &usage_payload,
                },
            )
            .await;
        }
        record_stream_terminal_usage(
            &state_for_report,
            &plan_for_report,
            usage_payload.report_context.as_ref(),
            &usage_payload,
            false,
        )
        .await;
        record_local_request_candidate_status(
            &state_for_report,
            &plan_for_report,
            usage_payload.report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: if stream_failed {
                    RequestCandidateStatus::Failed
                } else {
                    RequestCandidateStatus::Success
                },
                status_code: Some(status_code),
                error_type: if stream_failed {
                    if missing_observed_finish {
                        Some("stream_missing_terminal_event".to_string())
                    } else {
                        Some("stream_terminal_error".to_string())
                    }
                } else {
                    None
                },
                error_message: stream_failed
                    .then_some(stream_terminal_error_message)
                    .flatten(),
                latency_ms: usage_payload
                    .telemetry
                    .as_ref()
                    .and_then(|value| value.elapsed_ms),
                started_at_unix_ms: Some(candidate_started_unix_secs_for_report),
                finished_at_unix_ms: Some(current_request_candidate_unix_ms()),
            },
        )
        .await;

        if should_submit_report {
            if let Err(err) = submit_stream_report(&state_for_report, usage_payload).await {
                warn!(
                    event_name = "execution_report_submit_failed",
                    log_type = "ops",
                    trace_id = %trace_id_owned,
                    request_id = %request_id_for_report_log,
                    candidate_id = ?candidate_id_for_report.as_deref(),
                    report_scope = "stream",
                    error = ?err,
                    "gateway failed to submit stream execution report"
                );
            }
        }
    });

    headers.insert(CONTROL_REQUEST_ID_HEADER.to_string(), request_id.clone());

    if let Some(candidate_id) = candidate_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.insert(
            CONTROL_CANDIDATE_ID_HEADER.to_string(),
            candidate_id.to_string(),
        );
    }

    if response_headers_are_sse {
        headers.remove("content-length");
    }
    let body_stream = build_sse_body_stream(
        prefetched_chunks_for_body,
        rx,
        response_headers_are_sse,
        emit_proxy_generated_sse_control_blocks,
        native_anthropic_stream_for_report,
        SSE_KEEPALIVE_INTERVAL,
    );

    Ok(Some(build_client_response_from_parts(
        status_code,
        &headers,
        Body::from_stream(body_stream),
        trace_id,
        Some(decision),
    )?))
}

fn apply_stream_summary_report_context(
    execution: &mut DirectUpstreamStreamExecution,
    report_context: Option<&Value>,
) {
    if let Some(report_context) = report_context.cloned() {
        execution.stream_summary_report_context = report_context;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::{Duration, Instant};

    use aether_ai_serving::{AiAttemptExecutionOutcome, AiAttemptRetryScope};
    use aether_contracts::{
        ExecutionError, ExecutionErrorKind, ExecutionPhase, ExecutionPlan,
        ExecutionStreamTerminalSummary, ExecutionTelemetry, ExecutionTimeouts, RequestBody,
        StandardizedUsage, StreamFrame, StreamFramePayload, StreamFrameType,
    };
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::candidates::{
        PublicHealthStatusCount, PublicHealthTimelineBucket, RequestCandidateReadRepository,
        RequestCandidateStatus, RequestCandidateWriteRepository, StoredRequestCandidate,
        UpsertRequestCandidateRecord,
    };
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use aether_data_contracts::repository::settlement::{
        StoredUsageSettlement, UsageSettlementInput,
    };
    use aether_data_contracts::repository::usage::UsageReadRepository;
    use aether_data_contracts::repository::usage::{
        StoredRequestUsageAudit, UpsertUsageRecord, UsageBodyCaptureState,
    };
    use aether_data_contracts::DataLayerError;
    use aether_usage_runtime::{
        apply_usage_body_capture_policy_to_event, UsageBillingEventEnricher,
        UsageBodyCapturePolicy, UsageEvent, UsageEventData, UsageEventType, UsageRecordWriter,
        UsageRequestRecordLevel, UsageRuntimeAccess, UsageRuntimeConfig, UsageSettlementWriter,
    };
    use async_stream::stream;
    use async_trait::async_trait;
    use axum::body::{to_bytes, Body, Bytes};
    use axum::extract::ws::Message;
    use axum::extract::Request;
    use axum::routing::any;
    use axum::{
        http::header, http::HeaderValue, http::StatusCode, response::IntoResponse, Json, Router,
    };
    use base64::Engine as _;
    use futures_util::StreamExt as _;
    use serde_json::{json, Value};
    use tokio::sync::{mpsc, watch, Notify};

    use super::{
        activate_post_stop_frame_read_budget, build_direct_execution_frame_stream,
        build_sse_body_stream, build_stream_failure_report, build_stream_sync_payload,
        client_format_allows_proxy_generated_sse_control_blocks,
        direct_upstream_response_byte_stream, encode_terminal_sse_error_event_for_plan,
        ensure_stream_terminal_summary_for_missing_observed_finish,
        execute_execution_runtime_stream, execute_in_process_stream_with_oauth_retry,
        execute_stream_from_frame_stream, execute_stream_from_frame_stream_with_retry_scope,
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary, merge_stream_terminal_summary,
        parse_direct_passthrough_mode, prefetch_direct_stream_error_body,
        prefetched_openai_responses_body_has_output_boundary,
        record_sync_terminal_usage_with_handoff,
        record_sync_terminal_usage_with_handoff_after_spawn,
        resolve_provider_stream_error_status_code, select_direct_anthropic_prefetch_wait,
        should_limit_direct_finalize_prefetch, should_probe_success_failover_before_stream,
        should_skip_direct_finalize_prefetch, stream_chunk_contains_sse_done,
        stream_requires_observed_terminal_event, stream_terminal_summary_missing_observed_finish,
        stream_terminal_summary_missing_observed_finish_with_requirement,
        stream_terminal_summary_represents_failure_with_requirement,
        ClientVisibleStreamCompletionTracker, DirectPassthroughFinalizer,
        DirectPassthroughFinalizerCore, DirectPassthroughInlineBodyState, DirectPassthroughMode,
        PostStopFrameReadBudget, PostStopLimitedStreamReader, ProviderStreamErrorInspection,
        ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES, POST_STOP_MAX_EMPTY_CHUNKS_PER_POLL,
    };
    use crate::control::GatewayControlDecision;
    use crate::stage_metrics::RequestStageTrace;
    use crate::tunnel::{tunnel_protocol, TunnelProxyConn};
    use crate::AppState;
    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};

    fn provider_catalog_stop_429_for_plan(
        plan: &ExecutionPlan,
    ) -> InMemoryProviderCatalogReadRepository {
        provider_catalog_for_plan(
            plan,
            Some(json!({
                "failover_rules": {
                    "stop_status_codes": [429]
                }
            })),
        )
    }

    fn provider_catalog_for_plan(
        plan: &ExecutionPlan,
        provider_config: Option<Value>,
    ) -> InMemoryProviderCatalogReadRepository {
        let provider_type = plan.provider_name.as_deref().unwrap_or("custom");
        let provider = StoredProviderCatalogProvider::new(
            plan.provider_id.clone(),
            plan.provider_id.clone(),
            Some("https://provider.example".to_string()),
            provider_type.to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(3),
            None,
            None,
            None,
            provider_config,
        );
        let endpoint = StoredProviderCatalogEndpoint::new(
            plan.endpoint_id.clone(),
            plan.provider_id.clone(),
            plan.provider_api_format.clone(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://provider.example".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build");
        let key = StoredProviderCatalogKey::new(
            plan.key_id.clone(),
            plan.provider_id.clone(),
            plan.key_id.clone(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!([plan.provider_api_format.clone()])),
            "plain-upstream-key".to_string(),
            None,
            None,
            Some(json!({ "openai:chat": 1 })),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build");

        InMemoryProviderCatalogReadRepository::seed(vec![provider], vec![endpoint], vec![key])
    }

    fn provider_catalog_for_stream_auth_plan(
        plan: &ExecutionPlan,
        provider_type: &str,
        auth_type: &str,
        auth_config: Option<Value>,
    ) -> InMemoryProviderCatalogReadRepository {
        let provider = StoredProviderCatalogProvider::new(
            plan.provider_id.clone(),
            plan.provider_id.clone(),
            Some("https://provider.example".to_string()),
            provider_type.to_string(),
        )
        .expect("provider should build");
        let endpoint = StoredProviderCatalogEndpoint::new(
            plan.endpoint_id.clone(),
            plan.provider_id.clone(),
            plan.provider_api_format.clone(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            plan.url.clone(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build");
        let encrypted_auth_config = auth_config.map(|config| {
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, &config.to_string())
                .expect("auth config should encrypt")
        });
        let key = StoredProviderCatalogKey::new(
            plan.key_id.clone(),
            plan.provider_id.clone(),
            plan.key_id.clone(),
            auth_type.to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!([plan.provider_api_format.clone()])),
            None,
            encrypted_auth_config,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build");

        InMemoryProviderCatalogReadRepository::seed(vec![provider], vec![endpoint], vec![key])
    }

    fn direct_stream_test_plan(request_id: &str, url: String) -> ExecutionPlan {
        ExecutionPlan {
            request_id: request_id.to_string(),
            candidate_id: Some(format!("candidate-{request_id}")),
            provider_name: Some("codex".to_string()),
            provider_id: format!("provider-{request_id}"),
            endpoint_id: format!("endpoint-{request_id}"),
            key_id: format!("key-{request_id}"),
            method: "POST".to_string(),
            url,
            headers: BTreeMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                (
                    "authorization".to_string(),
                    "AgentAssertion stale-task".to_string(),
                ),
            ]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(30_000),
                first_byte_ms: Some(30_000),
                ..ExecutionTimeouts::default()
            }),
        }
    }

    fn agent_identity_test_auth_config(task_id: &str) -> Value {
        json!({
            "provider_type": "codex",
            "auth_mode": "agentIdentity",
            "agent_runtime_id": "runtime-test",
            "agent_private_key": "MC4CAQAwBQYDK2VwBCIEIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH",
            "task_id": task_id
        })
    }

    fn generic_oauth_test_auth_config(provider_type: &str) -> Value {
        json!({
            "provider_type": provider_type,
            "access_token": "stale-access-token",
            "refresh_token": "refresh-token",
            "expires_at": 4_102_444_800_u64
        })
    }

    async fn collect_direct_execution_body(
        mut execution: crate::execution_runtime::DirectUpstreamStreamExecution,
    ) -> Result<Vec<u8>, String> {
        let prefetched_body = std::mem::take(&mut execution.prefetched_body);
        let mut stream = direct_upstream_response_byte_stream(prefetched_body, execution.response);
        let mut body = Vec::new();
        while let Some(item) = stream.next().await {
            body.extend_from_slice(&item?);
        }
        Ok(body)
    }

    fn codex_cyber_policy_plan(request_id: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: request_id.to_string(),
            candidate_id: Some(format!("candidate-{request_id}")),
            provider_name: Some("codex".to_string()),
            provider_id: format!("provider-{request_id}"),
            endpoint_id: format!("endpoint-{request_id}"),
            key_id: format!("key-{request_id}"),
            method: "POST".to_string(),
            url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
            headers: BTreeMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("accept".to_string(), "text/event-stream".to_string()),
            ]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.5",
                "input": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5.5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    async fn execute_prefetched_codex_cyber_policy_failure(
        continue_failover: bool,
    ) -> Option<axum::http::Response<Body>> {
        let request_id = if continue_failover {
            "req-cyber-policy-retry"
        } else {
            "req-cyber-policy-stop"
        };
        let plan = codex_cyber_policy_plan(request_id);
        let provider_catalog = provider_catalog_for_plan(&plan, None);
        let data_state = crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
            Arc::new(provider_catalog),
            "development-key",
        );
        let data_state = if continue_failover {
            data_state.with_system_config_values_for_tests([(
                crate::orchestration::CYBER_CONTINUE_FAILOVER_CONFIG_KEY.to_string(),
                json!(true),
            )])
        } else {
            data_state
        };
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data_state);
        let upstream_setup = "event: response.created\ndata: {\"type\":\"response.created\"}\n\n";
        let upstream_error = "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"type\":\"invalid_request\",\"message\":\"cyber policy rejected the request\",\"code\":\"cyber_policy_violation\",\"param\":\"input\"}}}\n\n";
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(upstream_setup.to_string()),
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(upstream_error.to_string()),
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame::eof()));
        }
        .boxed();

        execute_stream_from_frame_stream(
            &state,
            plan,
            &format!("trace-{request_id}"),
            &test_decision(),
            "openai_responses_stream",
            Some("openai_responses_stream_success".to_string()),
            Some(json!({
                "request_id": request_id,
                "candidate_id": format!("candidate-{request_id}"),
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:responses",
                "client_api_format": "openai:responses"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
    }

    async fn execute_prefetched_transport_failure(
        stop_on_transport_errors: bool,
    ) -> AiAttemptExecutionOutcome<axum::http::Response<Body>> {
        let request_id = if stop_on_transport_errors {
            "req-prefetch-transport-stop"
        } else {
            "req-prefetch-transport-retry"
        };
        let plan = native_anthropic_stream_plan(request_id);
        let provider_config = stop_on_transport_errors.then(|| {
            json!({
                "failover_rules": {
                    "stop_on_transport_errors": true,
                }
            })
        });
        let provider_catalog = provider_catalog_for_plan(&plan, provider_config);
        let data_state = crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
            Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data_state);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Error,
                payload: StreamFramePayload::Error {
                    error: ExecutionError {
                        kind: ExecutionErrorKind::Internal,
                        phase: ExecutionPhase::StreamRead,
                        message: "connection reset before first body byte".to_string(),
                        upstream_status: None,
                        retryable: true,
                        failover_recommended: true,
                    },
                },
            }));
        }
        .boxed();
        let mut retry_scope = AiAttemptRetryScope::Provider;
        let response = execute_stream_from_frame_stream_with_retry_scope(
            &state,
            plan,
            &format!("trace-{request_id}"),
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": request_id,
                "candidate_id": format!("candidate-{request_id}"),
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "claude:messages",
                "client_api_format": "claude:messages"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            false,
            None,
            Some(&mut retry_scope),
            None,
            None,
        )
        .await
        .expect("prefetch transport execution should resolve");

        match response {
            Some(response) => AiAttemptExecutionOutcome::Responded(response),
            None => AiAttemptExecutionOutcome::Retry {
                scope: retry_scope,
                fallback_response: None,
            },
        }
    }

    async fn execute_prefetched_http_status_failure(
        continue_failover: bool,
    ) -> AiAttemptExecutionOutcome<axum::http::Response<Body>> {
        let request_id = if continue_failover {
            "req-prefetch-http-continue"
        } else {
            "req-prefetch-http-stop"
        };
        let plan = native_anthropic_stream_plan(request_id);
        let failover_rules = if continue_failover {
            json!({"continue_status_codes": [500]})
        } else {
            json!({"stop_status_codes": [500]})
        };
        let provider_catalog = provider_catalog_for_plan(
            &plan,
            Some(json!({
                "failover_rules": failover_rules,
            })),
        );
        let data_state = crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
            Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data_state);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Error,
                payload: StreamFramePayload::Error {
                    error: ExecutionError {
                        kind: ExecutionErrorKind::Internal,
                        phase: ExecutionPhase::StreamRead,
                        message: "upstream returned 500 before the first body byte".to_string(),
                        upstream_status: Some(500),
                        retryable: true,
                        failover_recommended: true,
                    },
                },
            }));
        }
        .boxed();
        let mut retry_scope = AiAttemptRetryScope::Provider;
        let response = execute_stream_from_frame_stream_with_retry_scope(
            &state,
            plan,
            &format!("trace-{request_id}"),
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": request_id,
                "candidate_id": format!("candidate-{request_id}"),
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "claude:messages",
                "client_api_format": "claude:messages"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            false,
            None,
            Some(&mut retry_scope),
            None,
            None,
        )
        .await
        .expect("prefetch HTTP status execution should resolve");

        match response {
            Some(response) => AiAttemptExecutionOutcome::Responded(response),
            None => AiAttemptExecutionOutcome::Retry {
                scope: retry_scope,
                fallback_response: None,
            },
        }
    }

    fn native_anthropic_stream_plan(request_id: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: request_id.to_string(),
            candidate_id: Some(format!("candidate-{request_id}")),
            provider_name: Some("custom".to_string()),
            provider_id: format!("provider-{request_id}"),
            endpoint_id: format!("endpoint-{request_id}"),
            key_id: format!("key-{request_id}"),
            method: "POST".to_string(),
            url: "https://api.anthropic.com/v1/messages".to_string(),
            headers: BTreeMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("accept".to_string(), "text/event-stream".to_string()),
            ]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "claude-sonnet-4-6",
                "messages": [{"role": "user", "content": "hello"}],
                "max_tokens": 32,
                "stream": true
            })),
            stream: true,
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "claude:messages".to_string(),
            model_name: Some("claude-sonnet-4-6".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    struct StreamDropFlag(Arc<AtomicBool>);

    impl Drop for StreamDropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn direct_anthropic_test_finalizer(request_id: &str) -> DirectPassthroughFinalizer {
        let state = AppState::new().expect("app state should build");
        let plan = native_anthropic_stream_plan(request_id);
        let lifecycle_seed = aether_usage_runtime::build_lifecycle_usage_seed(&plan, None);
        DirectPassthroughFinalizer::new(DirectPassthroughFinalizerCore {
            state,
            trace_id: format!("trace-{request_id}"),
            report_kind: None,
            report_context: None,
            lifecycle_seed,
            direct_stream_finalize_kind: None,
            stream_started_at: Instant::now(),
            stage_trace: RequestStageTrace::from_env(),
            request_diagnostics: None,
            request_id_for_log: request_id.to_string(),
            candidate_id: plan.candidate_id.clone(),
            request_candidate_status_snapshot: None,
            deferred_request_candidate_status_record: None,
            candidate_started_unix_secs: crate::clock::current_unix_ms(),
            status_code: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "text/event-stream".to_string(),
            )]),
            stream_usage_report_context: None,
            stream_usage_observer: None,
            stream_usage_observer_buffered: Vec::new(),
            provider_error_inspection: ProviderStreamErrorInspection::default(),
            max_stream_body_buffer_bytes: super::DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
            provider_buffered_body: Vec::new(),
            buffered_body: Vec::new(),
            provider_body_truncated: false,
            client_body_truncated: false,
            client_stream_completion_tracker: ClientVisibleStreamCompletionTracker::default(),
            requires_anthropic_message_stop: true,
            client_visible_stream_completed: false,
            usage_stream_telemetry: None,
            telemetry: None,
            provider_stream_bytes: 0,
            client_stream_bytes: 0,
            last_client_chunk_elapsed_ms: 0,
            pending_recorded: false,
            stream_started_recorded: false,
            terminal_failure: None,
            _provider_pool_in_flight_guard: None,
            _upstream_target_permit: None,
            plan,
        })
    }

    fn discard_direct_test_finalizer(state: &mut DirectPassthroughInlineBodyState) {
        if let Some(mut finalizer) = state.finalizer.take() {
            finalizer.core.take();
        }
    }

    fn direct_anthropic_inline_state(
        request_id: &str,
        items: Vec<Result<Bytes, String>>,
    ) -> DirectPassthroughInlineBodyState {
        DirectPassthroughInlineBodyState {
            finalizer: Some(direct_anthropic_test_finalizer(request_id)),
            upstream: Some(futures_util::stream::iter(items).boxed()),
            upstream_control_filter: Some(super::SseControlBlockFilter::default()),
            upstream_started_at: Instant::now(),
            stream_first_byte_timeout: None,
            observed_first_body_poll: false,
            observed_first_client_yield: false,
            upstream_done: false,
            control_filter_flushed: false,
            terminal_error_sent: false,
            finalized: false,
        }
    }

    async fn execute_native_anthropic_prefetch_stream(
        request_id: &str,
        chunks: Vec<String>,
    ) -> AiAttemptExecutionOutcome<axum::http::Response<Body>> {
        execute_native_anthropic_prefetch_stream_with_terminal_error(request_id, chunks, None).await
    }

    async fn execute_native_anthropic_prefetch_stream_with_terminal_error(
        request_id: &str,
        chunks: Vec<String>,
        terminal_error: Option<String>,
    ) -> AiAttemptExecutionOutcome<axum::http::Response<Body>> {
        let plan = native_anthropic_stream_plan(request_id);
        let provider_catalog = provider_catalog_for_plan(&plan, None);
        let data_state = crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
            Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data_state);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
            for chunk in chunks {
                yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                    frame_type: StreamFrameType::Data,
                    payload: StreamFramePayload::Data {
                        chunk_b64: None,
                        text: Some(chunk),
                    },
                }));
            }
            if let Some(error) = terminal_error {
                yield Err::<Bytes, std::io::Error>(std::io::Error::other(error));
            } else {
                yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame::eof()));
            }
        }
        .boxed();

        let mut retry_scope = AiAttemptRetryScope::Candidate;
        let mut fallback_response = None;
        let response = execute_stream_from_frame_stream_with_retry_scope(
            &state,
            plan,
            &format!("trace-{request_id}"),
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": request_id,
                "candidate_id": format!("candidate-{request_id}"),
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "claude:messages",
                "client_api_format": "claude:messages"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            false,
            None,
            Some(&mut retry_scope),
            Some(&mut fallback_response),
            None,
        )
        .await
        .expect("native Anthropic stream execution should succeed");
        match response {
            Some(response) => AiAttemptExecutionOutcome::Responded(response),
            None => AiAttemptExecutionOutcome::Retry {
                scope: retry_scope,
                fallback_response,
            },
        }
    }

    fn test_decision() -> GatewayControlDecision {
        GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        )
        .with_execution_runtime_candidate(true)
    }

    fn test_state() -> AppState {
        AppState::new().expect("gateway state should build")
    }

    #[tokio::test]
    async fn agent_identity_stream_error_prefetch_is_bounded_and_replayed() {
        let upstream_body = format!(
            "{}{}",
            "x".repeat(crate::execution_runtime::MAX_ERROR_BODY_BYTES),
            "body-after-inspection-limit"
        );
        let expected_body = upstream_body.clone().into_bytes();
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("address should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/responses",
                any(move || {
                    let body = upstream_body.clone();
                    async move { (StatusCode::UNAUTHORIZED, body) }
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });
        let plan =
            direct_stream_test_plan("bounded-agent-error", format!("http://{addr}/responses"));
        let mut execution = crate::execution_runtime::DirectSyncExecutionRuntime::new()
            .execute_stream(&plan)
            .await
            .expect("stream headers should execute");

        let inspected = prefetch_direct_stream_error_body(&mut execution)
            .await
            .expect("error body should be inspected");

        assert_eq!(
            inspected.len(),
            crate::execution_runtime::MAX_ERROR_BODY_BYTES
        );
        let replayed = collect_direct_execution_body(execution)
            .await
            .expect("prefetched response should replay");
        assert_eq!(replayed, expected_body);
        server.abort();
    }

    #[tokio::test]
    async fn non_agent_stream_401_is_not_prefetched_and_body_passes_through() {
        let upstream_body = br#"{"error":{"code":"ordinary_unauthorized","message":"sign in"}}"#;
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("address should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/responses",
                any(|| async {
                    (
                        StatusCode::UNAUTHORIZED,
                        [(header::CONTENT_TYPE, "application/json")],
                        upstream_body.as_slice(),
                    )
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });
        let mut plan = direct_stream_test_plan("non-agent-401", format!("http://{addr}/responses"));
        plan.provider_name = Some("openai".to_string());
        let repository = provider_catalog_for_stream_auth_plan(&plan, "openai", "api_key", None);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
                    Arc::new(repository),
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            );

        let execution = execute_in_process_stream_with_oauth_retry(
            &state,
            &mut plan,
            "trace-non-agent-401",
            None,
        )
        .await
        .expect("stream request should execute");

        assert!(execution.prefetched_body.is_empty());
        let replayed = collect_direct_execution_body(execution)
            .await
            .expect("response body should pass through");
        assert_eq!(replayed, upstream_body);
        server.abort();
    }

    #[tokio::test]
    async fn agent_identity_stream_non_task_401_replays_original_body_without_refresh() {
        let upstream_body =
            br#"{"error":{"code":"account_disabled","message":"account unavailable"}}"#;
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("address should resolve");
        let task_registration_hits = Arc::new(AtomicUsize::new(0));
        let task_registration_hits_for_server = Arc::clone(&task_registration_hits);
        let server = tokio::spawn(async move {
            let app = Router::new()
                .route(
                    "/responses",
                    any(|| async {
                        (
                            StatusCode::UNAUTHORIZED,
                            [(header::CONTENT_TYPE, "application/json")],
                            upstream_body.as_slice(),
                        )
                    }),
                )
                .route(
                    "/api/accounts/v1/agent/runtime-test/task/register",
                    any(move || {
                        let hits = Arc::clone(&task_registration_hits_for_server);
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            Json(json!({"task_id": "unexpected-task"}))
                        }
                    }),
                );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });
        let mut plan =
            direct_stream_test_plan("agent-non-task-401", format!("http://{addr}/responses"));
        let repository = Arc::new(provider_catalog_for_stream_auth_plan(
            &plan,
            "codex",
            "oauth",
            Some(agent_identity_test_auth_config("task-old")),
        ));
        let oauth_refresh =
            aether_provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
                Arc::new(
                    aether_provider_transport::CodexAgentIdentityRefreshAdapter::default()
                        .with_auth_api_base_url_for_tests(format!("http://{addr}/api/accounts")),
                ),
            ]);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(
                    repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh);

        let execution = execute_in_process_stream_with_oauth_retry(
            &state,
            &mut plan,
            "trace-agent-non-task-401",
            None,
        )
        .await
        .expect("stream request should execute");

        assert!(!execution.prefetched_body.is_empty());
        assert_eq!(task_registration_hits.load(Ordering::SeqCst), 0);
        let replayed = collect_direct_execution_body(execution)
            .await
            .expect("response body should replay");
        assert_eq!(replayed, upstream_body);
        server.abort();
    }

    #[tokio::test]
    async fn agent_identity_stream_invalid_task_refreshes_and_retries_once() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let upstream_hits_for_server = Arc::clone(&upstream_hits);
        let task_registration_hits = Arc::new(AtomicUsize::new(0));
        let task_registration_hits_for_server = Arc::clone(&task_registration_hits);
        let observed_authorization = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed_authorization_for_server = Arc::clone(&observed_authorization);
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("address should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new()
                .route(
                    "/responses",
                    any(move |request: Request| {
                        let hits = Arc::clone(&upstream_hits_for_server);
                        let authorizations = Arc::clone(&observed_authorization_for_server);
                        async move {
                            let authorization = request
                                .headers()
                                .get(header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string();
                            authorizations
                                .lock()
                                .expect("authorization mutex should lock")
                                .push(authorization);
                            if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                                (
                                    StatusCode::UNAUTHORIZED,
                                    Json(json!({
                                        "error": {
                                            "code": "invalid_task_id",
                                            "message": "registered task expired"
                                        }
                                    })),
                                )
                                    .into_response()
                            } else {
                                (StatusCode::OK, Json(json!({"ok": true}))).into_response()
                            }
                        }
                    }),
                )
                .route(
                    "/api/accounts/v1/agent/runtime-test/task/register",
                    any(move || {
                        let hits = Arc::clone(&task_registration_hits_for_server);
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            Json(json!({"task_id": "task-new"}))
                        }
                    }),
                );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });
        let mut plan =
            direct_stream_test_plan("agent-invalid-task", format!("http://{addr}/responses"));
        let repository = Arc::new(provider_catalog_for_stream_auth_plan(
            &plan,
            "codex",
            "oauth",
            Some(agent_identity_test_auth_config("task-old")),
        ));
        let oauth_refresh =
            aether_provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
                Arc::new(
                    aether_provider_transport::CodexAgentIdentityRefreshAdapter::default()
                        .with_auth_api_base_url_for_tests(format!("http://{addr}/api/accounts")),
                ),
            ]);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(
                    repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh);
        let transport = state
            .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
            .await
            .expect("transport should load")
            .expect("transport should exist");
        let initial_authorization = match state
            .resolve_local_oauth_request_auth(&transport)
            .await
            .expect("initial Agent Identity auth should resolve")
            .expect("initial Agent Identity auth should exist")
        {
            aether_provider_transport::LocalResolvedOAuthRequestAuth::Header { name, value } => {
                assert_eq!(name, "authorization");
                value
            }
            aether_provider_transport::LocalResolvedOAuthRequestAuth::Kiro(_) => {
                panic!("Agent Identity should resolve to header auth")
            }
        };
        assert!(
            aether_provider_transport::codex_agent_identity_authorization_matches_transport(
                &transport,
                &initial_authorization,
            )
        );
        plan.headers
            .insert("authorization".to_string(), initial_authorization.clone());

        let execution = execute_in_process_stream_with_oauth_retry(
            &state,
            &mut plan,
            "trace-agent-invalid-task",
            None,
        )
        .await
        .expect("stream request should recover");

        assert_eq!(execution.status_code, 200);
        assert!(execution.prefetched_body.is_empty());
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 2);
        assert_eq!(task_registration_hits.load(Ordering::SeqCst), 1);
        let authorizations = observed_authorization
            .lock()
            .expect("authorization mutex should lock");
        assert_eq!(authorizations.len(), 2);
        assert_eq!(authorizations[0], initial_authorization);
        assert!(authorizations[1].starts_with("AgentAssertion "));
        assert_ne!(authorizations[1], authorizations[0]);
        drop(authorizations);
        let replayed = collect_direct_execution_body(execution)
            .await
            .expect("retried response body should read");
        assert_eq!(
            serde_json::from_slice::<Value>(&replayed).expect("response should be JSON"),
            json!({"ok": true})
        );
        server.abort();
    }

    #[tokio::test]
    async fn native_anthropic_embedded_auth_error_refreshes_oauth_and_retries_once() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let upstream_hits_for_server = Arc::clone(&upstream_hits);
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let refresh_hits_for_server = Arc::clone(&refresh_hits);
        let observed_authorization = Arc::new(Mutex::new(Vec::<String>::new()));
        let observed_authorization_for_server = Arc::clone(&observed_authorization);
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("address should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new()
                .route(
                    "/v1/messages",
                    any(move |request: Request| {
                        let hits = Arc::clone(&upstream_hits_for_server);
                        let authorizations = Arc::clone(&observed_authorization_for_server);
                        async move {
                            authorizations
                                .lock()
                                .expect("authorization mutex should lock")
                                .push(
                                    request
                                        .headers()
                                        .get(header::AUTHORIZATION)
                                        .and_then(|value| value.to_str().ok())
                                        .unwrap_or_default()
                                        .to_string(),
                                );
                            let body = if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                                concat!(
                                    "event: error\n",
                                    "data: {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"expired token\"}}\n\n",
                                )
                            } else {
                                concat!(
                                    "event: message_start\n",
                                    "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
                                    "event: message_stop\n",
                                    "data: {\"type\":\"message_stop\"}\n\n",
                                )
                            };
                            (
                                StatusCode::OK,
                                [(header::CONTENT_TYPE, "text/event-stream")],
                                body,
                            )
                                .into_response()
                        }
                    }),
                )
                .route(
                    "/oauth/token",
                    any(move || {
                        let hits = Arc::clone(&refresh_hits_for_server);
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "access_token": "fresh-access-token",
                                "refresh_token": "fresh-refresh-token",
                                "expires_in": 3600,
                                "token_type": "Bearer"
                            }))
                        }
                    }),
                );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let mut plan = native_anthropic_stream_plan("anthropic-embedded-oauth-refresh");
        plan.url = format!("http://{addr}/v1/messages");
        plan.provider_name = Some("claude_code".to_string());
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer stale-access-token".to_string(),
        );
        let repository = Arc::new(provider_catalog_for_stream_auth_plan(
            &plan,
            "claude_code",
            "oauth",
            Some(generic_oauth_test_auth_config("claude_code")),
        ));
        let oauth_refresh =
            aether_provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
                Arc::new(
                    aether_provider_transport::GenericOAuthRefreshAdapter::default()
                        .with_token_url_for_tests(
                            "claude_code",
                            format!("http://{addr}/oauth/token"),
                        ),
                ),
            ]);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(
                    repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh);

        let execution = execute_in_process_stream_with_oauth_retry(
            &state,
            &mut plan,
            "trace-anthropic-embedded-oauth-refresh",
            None,
        )
        .await
        .expect("embedded authentication error should recover");
        let replayed = collect_direct_execution_body(execution)
            .await
            .expect("retried response should read");

        assert_eq!(upstream_hits.load(Ordering::SeqCst), 2);
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);
        assert!(String::from_utf8_lossy(&replayed).contains("event: message_start"));
        assert_eq!(
            observed_authorization
                .lock()
                .expect("authorization mutex should lock")
                .as_slice(),
            [
                "Bearer stale-access-token".to_string(),
                "Bearer fresh-access-token".to_string(),
            ]
        );
        server.abort();
    }

    #[tokio::test]
    async fn native_anthropic_http_permission_error_does_not_refresh_oauth() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let upstream_hits_for_server = Arc::clone(&upstream_hits);
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let refresh_hits_for_server = Arc::clone(&refresh_hits);
        let permission_body = concat!(
            "{\"type\":\"error\",\"error\":{",
            "\"type\":\"permission_error\",",
            "\"message\":\"this token is not authorized for the workspace\"}}",
        );
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("address should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new()
                .route(
                    "/v1/messages",
                    any(move || {
                        let hits = Arc::clone(&upstream_hits_for_server);
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            (
                                StatusCode::FORBIDDEN,
                                [(header::CONTENT_TYPE, "application/json")],
                                permission_body,
                            )
                        }
                    }),
                )
                .route(
                    "/oauth/token",
                    any(move || {
                        let hits = Arc::clone(&refresh_hits_for_server);
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "access_token": "unexpected-access-token",
                                "refresh_token": "unexpected-refresh-token",
                                "expires_in": 3600,
                                "token_type": "Bearer"
                            }))
                        }
                    }),
                );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let mut plan = native_anthropic_stream_plan("anthropic-http-oauth-permission");
        plan.url = format!("http://{addr}/v1/messages");
        plan.provider_name = Some("claude_code".to_string());
        plan.headers.insert(
            "authorization".to_string(),
            "Bearer stale-access-token".to_string(),
        );
        let repository = Arc::new(provider_catalog_for_stream_auth_plan(
            &plan,
            "claude_code",
            "oauth",
            Some(generic_oauth_test_auth_config("claude_code")),
        ));
        let oauth_refresh =
            aether_provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
                Arc::new(
                    aether_provider_transport::GenericOAuthRefreshAdapter::default()
                        .with_token_url_for_tests(
                            "claude_code",
                            format!("http://{addr}/oauth/token"),
                        ),
                ),
            ]);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(
                    repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh);

        let execution = execute_in_process_stream_with_oauth_retry(
            &state,
            &mut plan,
            "trace-anthropic-http-oauth-permission",
            None,
        )
        .await
        .expect("permission response should remain available");
        assert_eq!(execution.status_code, StatusCode::FORBIDDEN.as_u16());
        let replayed = collect_direct_execution_body(execution)
            .await
            .expect("permission response should replay");

        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 0);
        assert_eq!(replayed, permission_body.as_bytes());
        server.abort();
    }

    #[test]
    fn native_anthropic_oauth_prefetch_respects_short_first_byte_timeout() {
        let now = Instant::now();
        let precommit_started_at = now
            .checked_sub(Duration::from_millis(10))
            .expect("precommit start should be representable");
        let upstream_started_at = now
            .checked_sub(Duration::from_millis(90))
            .expect("upstream start should be representable");

        let first_byte_wait = select_direct_anthropic_prefetch_wait(
            precommit_started_at,
            Duration::from_millis(750),
            upstream_started_at,
            Some(Duration::from_millis(100)),
            false,
            now,
        );
        assert_eq!(first_byte_wait.remaining, Duration::from_millis(10));
        assert!(!first_byte_wait.commit_on_timeout);

        let precommit_wait = select_direct_anthropic_prefetch_wait(
            now.checked_sub(Duration::from_millis(750))
                .expect("precommit start should be representable"),
            Duration::from_millis(750),
            upstream_started_at,
            Some(Duration::from_secs(5)),
            false,
            now,
        );
        assert!(precommit_wait.remaining.is_zero());
        assert!(precommit_wait.commit_on_timeout);
    }

    #[tokio::test]
    async fn native_anthropic_oauth_pending_events_do_not_start_a_second_precommit_wait() {
        let request_id = "anthropic-oauth-single-precommit";
        let plan = native_anthropic_stream_plan(request_id);
        let provider_catalog = provider_catalog_for_plan(&plan, None);
        let data_state = crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
            Arc::new(provider_catalog),
            DEVELOPMENT_ENCRYPTION_KEY,
        );
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data_state);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(": ping\n\n".to_string()),
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(
                        "event: future_event\ndata: {\"type\":\"future_event\",\"value\":1}\n\n"
                            .to_string(),
                    ),
                },
            }));
            tokio::time::sleep(Duration::from_secs(2)).await;
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame::eof()));
        }
        .boxed();
        let response = tokio::time::timeout(
            Duration::from_millis(300),
            execute_stream_from_frame_stream_with_retry_scope(
                &state,
                plan,
                "trace-anthropic-oauth-single-precommit",
                &test_decision(),
                "claude_chat_stream",
                Some("claude_chat_stream_success".to_string()),
                Some(json!({
                    "request_id": request_id,
                    "candidate_id": format!("candidate-{request_id}"),
                    "candidate_index": 0,
                    "retry_index": 0,
                    "provider_api_format": "claude:messages",
                    "client_api_format": "claude:messages"
                })),
                crate::clock::current_unix_ms(),
                Instant::now(),
                RequestStageTrace::from_env(),
                true,
                frame_stream,
                true,
                None,
                None,
                None,
                None,
            ),
        )
        .await
        .expect("the committed OAuth prefetch must not be followed by another 750 ms wait")
        .expect("frame stream execution should resolve");

        assert!(response.is_some());
    }

    #[tokio::test]
    async fn native_anthropic_embedded_auth_error_does_not_refresh_api_key() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let upstream_hits_for_server = Arc::clone(&upstream_hits);
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("address should resolve");
        let upstream_body = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"invalid key\"}}\n\n",
        );
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/messages",
                any(move || {
                    let hits = Arc::clone(&upstream_hits_for_server);
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, "text/event-stream")],
                            upstream_body,
                        )
                    }
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let mut plan = native_anthropic_stream_plan("anthropic-embedded-api-key");
        plan.url = format!("http://{addr}/v1/messages");
        plan.provider_name = Some("claude_code".to_string());
        plan.headers
            .insert("x-api-key".to_string(), "invalid-api-key".to_string());
        let repository = Arc::new(provider_catalog_for_stream_auth_plan(
            &plan,
            "claude_code",
            "api_key",
            None,
        ));
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(
                    repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );

        let execution = execute_in_process_stream_with_oauth_retry(
            &state,
            &mut plan,
            "trace-anthropic-embedded-api-key",
            None,
        )
        .await
        .expect("API-key error response should remain available");
        let replayed = collect_direct_execution_body(execution)
            .await
            .expect("prefetched API-key error should replay");

        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
        assert_eq!(replayed, upstream_body.as_bytes());
        server.abort();
    }

    struct BlockingStreamingRequestCandidateRepository {
        inner: InMemoryRequestCandidateRepository,
        block_streaming: AtomicBool,
        streaming_started: Notify,
        release_streaming: Notify,
    }

    impl Default for BlockingStreamingRequestCandidateRepository {
        fn default() -> Self {
            Self {
                inner: InMemoryRequestCandidateRepository::default(),
                block_streaming: AtomicBool::new(true),
                streaming_started: Notify::new(),
                release_streaming: Notify::new(),
            }
        }
    }

    #[async_trait]
    impl RequestCandidateReadRepository for BlockingStreamingRequestCandidateRepository {
        async fn list_by_request_id(
            &self,
            request_id: &str,
        ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
            self.inner.list_by_request_id(request_id).await
        }

        async fn list_recent(
            &self,
            limit: usize,
        ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
            self.inner.list_recent(limit).await
        }

        async fn list_by_provider_id(
            &self,
            provider_id: &str,
            limit: usize,
        ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
            self.inner.list_by_provider_id(provider_id, limit).await
        }

        async fn list_finalized_by_endpoint_ids_since(
            &self,
            endpoint_ids: &[String],
            since_unix_secs: u64,
            limit: usize,
        ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
            self.inner
                .list_finalized_by_endpoint_ids_since(endpoint_ids, since_unix_secs, limit)
                .await
        }

        async fn count_finalized_statuses_by_endpoint_ids_since(
            &self,
            endpoint_ids: &[String],
            since_unix_secs: u64,
        ) -> Result<Vec<PublicHealthStatusCount>, DataLayerError> {
            self.inner
                .count_finalized_statuses_by_endpoint_ids_since(endpoint_ids, since_unix_secs)
                .await
        }

        async fn aggregate_finalized_timeline_by_endpoint_ids_since(
            &self,
            endpoint_ids: &[String],
            since_unix_secs: u64,
            until_unix_secs: u64,
            segments: u32,
        ) -> Result<Vec<PublicHealthTimelineBucket>, DataLayerError> {
            self.inner
                .aggregate_finalized_timeline_by_endpoint_ids_since(
                    endpoint_ids,
                    since_unix_secs,
                    until_unix_secs,
                    segments,
                )
                .await
        }
    }

    #[async_trait]
    impl RequestCandidateWriteRepository for BlockingStreamingRequestCandidateRepository {
        async fn upsert(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<StoredRequestCandidate, DataLayerError> {
            if candidate.status == RequestCandidateStatus::Streaming
                && self.block_streaming.swap(false, Ordering::AcqRel)
            {
                self.streaming_started.notify_one();
                self.release_streaming.notified().await;
            }
            self.inner.upsert(candidate).await
        }

        async fn delete_created_before(
            &self,
            created_before_unix_secs: u64,
            limit: usize,
        ) -> Result<usize, DataLayerError> {
            self.inner
                .delete_created_before(created_before_unix_secs, limit)
                .await
        }
    }

    fn stage_metric_count(stage: &str) -> u64 {
        crate::stage_metrics::gateway_stage_metric_samples()
            .into_iter()
            .find(|sample| {
                sample.name == "gateway_stage_latency_count"
                    && sample
                        .labels
                        .iter()
                        .any(|label| label.key == "stage" && label.value == stage)
            })
            .map(|sample| sample.value)
            .unwrap_or_default()
    }

    #[derive(Clone)]
    struct BlockingUsageAccess {
        policy_started: Arc<Notify>,
        release_policy: Arc<Notify>,
    }

    #[async_trait]
    impl UsageRecordWriter for BlockingUsageAccess {
        async fn upsert_usage_record(
            &self,
            _record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for BlockingUsageAccess {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for BlockingUsageAccess {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl UsageRuntimeAccess for BlockingUsageAccess {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            false
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn aether_runtime_state::RuntimeQueueStore>> {
            None
        }

        fn supports_first_byte_usage_fast_path(&self) -> bool {
            false
        }

        async fn body_capture_policy(&self) -> Result<UsageBodyCapturePolicy, DataLayerError> {
            self.policy_started.notify_one();
            self.release_policy.notified().await;
            Ok(UsageBodyCapturePolicy::default())
        }
    }

    #[tokio::test]
    async fn inline_first_chunk_does_not_wait_for_candidate_streaming_persistence() {
        let request_id = "req-inline-first-chunk-candidate-handoff";
        let request_candidate_repository =
            Arc::new(BlockingStreamingRequestCandidateRepository::default());
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("app state should build")
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
        let plan = codex_cyber_policy_plan(request_id);
        let lifecycle_seed = aether_usage_runtime::build_lifecycle_usage_seed(&plan, None);
        let request_candidate_status_snapshot =
            crate::request_candidate_runtime::snapshot_local_request_candidate_status(&plan, None);
        let stream_started_at = Instant::now();
        let finalizer = DirectPassthroughFinalizer::new(DirectPassthroughFinalizerCore {
            state,
            trace_id: "trace-inline-first-chunk-candidate-handoff".to_string(),
            report_kind: None,
            report_context: None,
            lifecycle_seed,
            direct_stream_finalize_kind: None,
            stream_started_at,
            stage_trace: RequestStageTrace::from_env(),
            request_diagnostics: None,
            request_id_for_log: request_id.to_string(),
            candidate_id: plan.candidate_id.clone(),
            request_candidate_status_snapshot,
            deferred_request_candidate_status_record: None,
            candidate_started_unix_secs: crate::clock::current_unix_ms(),
            status_code: 200,
            headers: BTreeMap::new(),
            stream_usage_report_context: None,
            stream_usage_observer: None,
            stream_usage_observer_buffered: Vec::new(),
            provider_error_inspection: ProviderStreamErrorInspection::default(),
            max_stream_body_buffer_bytes: super::DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
            provider_buffered_body: Vec::new(),
            buffered_body: Vec::new(),
            provider_body_truncated: false,
            client_body_truncated: false,
            client_stream_completion_tracker: ClientVisibleStreamCompletionTracker::default(),
            requires_anthropic_message_stop: false,
            client_visible_stream_completed: false,
            usage_stream_telemetry: Some(ExecutionTelemetry {
                ttfb_ms: Some(7),
                elapsed_ms: Some(7),
                upstream_bytes: Some(2),
            }),
            telemetry: None,
            provider_stream_bytes: 2,
            client_stream_bytes: 0,
            last_client_chunk_elapsed_ms: 0,
            pending_recorded: false,
            stream_started_recorded: false,
            terminal_failure: None,
            _provider_pool_in_flight_guard: None,
            _upstream_target_permit: None,
            plan,
        });
        let mut body_state = DirectPassthroughInlineBodyState {
            finalizer: Some(finalizer),
            upstream: None,
            upstream_control_filter: None,
            upstream_started_at: stream_started_at,
            stream_first_byte_timeout: None,
            observed_first_body_poll: true,
            observed_first_client_yield: false,
            upstream_done: false,
            control_filter_flushed: false,
            terminal_error_sent: false,
            finalized: false,
        };
        let first_yield_count = stage_metric_count("stream_first_client_yield");

        body_state.prepare_client_chunk_yield(&Bytes::from_static(b"hi"));

        assert!(body_state.observed_first_client_yield);
        assert!(
            stage_metric_count("stream_first_client_yield") > first_yield_count,
            "the first-yield metric must be recorded before candidate persistence completes"
        );
        tokio::time::timeout(
            Duration::from_secs(1),
            request_candidate_repository.streaming_started.notified(),
        )
        .await
        .expect("candidate streaming persistence should be handed off");
        assert!(
            request_candidate_repository
                .list_by_request_id(request_id)
                .await
                .expect("candidate read should succeed")
                .is_empty(),
            "the first chunk must return while candidate persistence is still blocked"
        );

        let live_usage = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(usage) = usage_repository
                    .find_by_request_id(request_id)
                    .await
                    .expect("usage read should succeed")
                {
                    if usage.status == "streaming" {
                        break usage;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("ordered usage lifecycle should reach streaming independently");
        assert_eq!(live_usage.billing_status, "pending");
        assert_eq!(live_usage.first_byte_time_ms, Some(7));

        request_candidate_repository.release_streaming.notify_one();
        let candidate = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(candidate) = request_candidate_repository
                    .list_by_request_id(request_id)
                    .await
                    .expect("candidate read should succeed")
                    .into_iter()
                    .next()
                {
                    break candidate;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("candidate handoff should finish after release");
        assert_eq!(candidate.status, RequestCandidateStatus::Streaming);

        if let Some(mut finalizer) = body_state.finalizer.take() {
            finalizer.core.take();
        }
    }

    #[tokio::test]
    async fn frame_stream_records_deferred_pending_before_waiting_for_headers() {
        let request_id = "req-frame-stream-deferred-pending";
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_usage_repository_for_tests(Arc::clone(
                    &usage_repository,
                )),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
        let plan = codex_cyber_policy_plan(request_id);
        let release_headers = Arc::new(Notify::new());
        let release_headers_for_stream = Arc::clone(&release_headers);
        let frame_stream = stream! {
            release_headers_for_stream.notified().await;
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
        }
        .boxed();
        let state_for_execution = state.clone();
        let execution = tokio::spawn(async move {
            execute_stream_from_frame_stream(
                &state_for_execution,
                plan,
                "trace-frame-stream-deferred-pending",
                &test_decision(),
                "openai_responses_stream",
                None,
                Some(json!({
                    "provider_api_format": "openai:responses",
                    "client_api_format": "openai:responses",
                })),
                crate::clock::current_unix_ms(),
                Instant::now(),
                RequestStageTrace::from_env(),
                false,
                frame_stream,
                None,
            )
            .await
        });

        let pending = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(usage) = usage_repository
                    .find_by_request_id(request_id)
                    .await
                    .expect("usage should read")
                {
                    break usage;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("deferred pending usage should be recorded before headers");

        assert_eq!(pending.status, "pending");
        assert_eq!(pending.billing_status, "pending");
        assert!(
            !execution.is_finished(),
            "frame execution should still be waiting for upstream headers"
        );

        execution.abort();
        let _ = execution.await;
    }

    #[tokio::test]
    async fn sync_terminal_handoff_survives_cancellation_during_admission_backpressure() {
        let blocker_request_id = "req-sync-terminal-admission-blocker";
        let target_request_id = "req-sync-terminal-handoff-cancelled";
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_usage_repository_for_tests(Arc::clone(
                    &usage_repository,
                )),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                terminal_submission_max_in_flight: 1,
                ..UsageRuntimeConfig::default()
            });
        let policy_started = Arc::new(Notify::new());
        let release_policy = Arc::new(Notify::new());
        let blocker = BlockingUsageAccess {
            policy_started: Arc::clone(&policy_started),
            release_policy: Arc::clone(&release_policy),
        };

        state
            .usage_runtime
            .submit_terminal_event(
                &blocker,
                UsageEvent::new(
                    UsageEventType::Completed,
                    blocker_request_id,
                    UsageEventData {
                        provider_name: "test".to_string(),
                        model: "test-model".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        tokio::time::timeout(Duration::from_secs(1), policy_started.notified())
            .await
            .expect("first terminal submission should be blocked in body policy");

        let plan = codex_cyber_policy_plan(target_request_id);
        let payload = build_stream_sync_payload(
            "trace-sync-terminal-handoff-cancelled",
            "openai_responses_stream".to_string(),
            Some(json!({
                "request_id": target_request_id,
                "provider_api_format": "openai:responses",
                "client_api_format": "openai:responses"
            })),
            500,
            BTreeMap::new(),
            Some(json!({"error": "synthetic terminal failure"})),
            None,
            None,
        );
        let state_for_handoff = state.clone();
        let child_started = Arc::new(Notify::new());
        let release_child = Arc::new(Notify::new());
        let child_started_for_handoff = Arc::clone(&child_started);
        let release_child_for_handoff = Arc::clone(&release_child);
        let handoff = tokio::spawn(async move {
            record_sync_terminal_usage_with_handoff_after_spawn(
                &state_for_handoff,
                &plan,
                payload.report_context.as_ref(),
                &payload,
                async move {
                    child_started_for_handoff.notify_one();
                    release_child_for_handoff.notified().await;
                },
            )
            .await;
        });
        tokio::time::timeout(Duration::from_secs(1), child_started.notified())
            .await
            .expect("detached terminal child should start before cancellation");

        handoff.abort();
        if let Err(err) = handoff.await {
            assert!(err.is_cancelled(), "terminal handoff task should not panic");
        }
        release_child.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if state
                    .usage_runtime
                    .metrics_snapshot()
                    .terminal_submission_pending
                    >= 2
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second terminal submission should reach the ordered backlog");

        release_policy.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state
                    .usage_runtime
                    .metrics_snapshot()
                    .terminal_submission_pending
                    == 0
                    && state
                        .usage_runtime
                        .metrics_snapshot()
                        .lifecycle_submission_pending
                        == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached terminal handoff should release admission");

        let record = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(record) = usage_repository
                    .find_by_request_id(target_request_id)
                    .await
                    .expect("usage repository read should succeed")
                {
                    break record;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached terminal handoff should persist the target row");
        assert_eq!(record.status, "failed");
        assert_eq!(record.billing_status, "void");
    }

    #[test]
    fn detects_client_visible_sse_terminal_events() {
        assert!(stream_chunk_contains_sse_done(b"data: [DONE]\n\n"));
        assert!(stream_chunk_contains_sse_done(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        ));
        assert!(stream_chunk_contains_sse_done(
            b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        ));
        assert!(stream_chunk_contains_sse_done(
            b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n"
        ));
        assert!(!stream_chunk_contains_sse_done(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\"}\n\n"
        ));
    }

    #[test]
    fn detects_client_visible_sse_terminal_events_across_chunks() {
        let mut tracker = ClientVisibleStreamCompletionTracker::default();
        assert!(!tracker.observe_chunk(b"data: [DO"));
        assert!(!tracker.observe_chunk(b"NE]\n"));
        assert!(tracker.observe_chunk(b"\n"));

        let mut tracker = ClientVisibleStreamCompletionTracker::default();
        assert!(!tracker.observe_chunk(b"event: response.comp"));
        assert!(!tracker.observe_chunk(b"leted\r\n"));
        assert!(tracker
            .observe_chunk(b"data: {\"type\":\"response.completed\",\"response\":{}}\r\n\r\n"));
    }

    #[test]
    fn client_visible_terminal_tracker_reports_the_exact_record_boundary() {
        let message_stop = b"event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n";
        let trailing_error =
            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\"}}\n\n";
        let chunk = [message_stop.as_slice(), trailing_error.as_slice()].concat();
        let mut tracker = ClientVisibleStreamCompletionTracker::default();

        assert_eq!(
            tracker.observe_chunk_terminal_end(&chunk),
            Some(message_stop.len())
        );
        assert!(tracker.completed);
    }

    #[test]
    fn anthropic_terminal_tracker_ignores_non_message_stop_terminals() {
        let mut tracker = ClientVisibleStreamCompletionTracker::default();

        assert!(!tracker.observe_anthropic_message_stop(b"data: [DONE]\n\n"));
        assert!(!tracker.observe_anthropic_message_stop(
            b"event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n"
        ));
        assert!(tracker.observe_anthropic_message_stop(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        ));
    }

    #[test]
    fn terminal_tracker_caps_multiline_record_and_resumes_after_boundary() {
        let line = b"data: short-payload\r\n";
        let repeated = super::SSE_TERMINAL_DETECTOR_MAX_RECORD_BYTES / line.len() + 2;
        let oversized_record = line.repeat(repeated);
        let mut tracker = ClientVisibleStreamCompletionTracker::default();

        assert!(!tracker.observe_anthropic_message_stop(&oversized_record));
        assert!(tracker.dropping_oversized_record);
        assert!(tracker.line_buffer.is_empty());
        assert!(tracker.data_payload.is_empty());

        assert!(!tracker.observe_anthropic_message_stop(b"\r\n"));
        assert!(!tracker.dropping_oversized_record);
        assert!(tracker.observe_anthropic_message_stop(
            b"event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n"
        ));
    }

    #[test]
    fn stream_capture_hard_caps_a_single_oversized_chunk() {
        let mut buffer = vec![1, 2];
        let mut truncated = false;

        super::append_stream_capture_bytes(&mut buffer, &[3, 4, 5, 6], 4, &mut truncated);

        assert_eq!(buffer, vec![1, 2, 3, 4]);
        assert!(truncated);
    }

    #[test]
    fn stream_capture_policy_keeps_full_unbounded_and_caps_basic_analysis_buffer() {
        let oversized_chunk = vec![b'x'; super::BASIC_STREAM_BODY_ANALYSIS_LIMIT_BYTES + 1];

        let full_limit =
            super::stream_body_buffer_limit_for_record_level(UsageRequestRecordLevel::Full);
        assert_eq!(full_limit, usize::MAX);
        let mut full_buffer = Vec::new();
        let mut full_truncated = false;
        super::append_stream_capture_bytes(
            &mut full_buffer,
            &oversized_chunk,
            full_limit,
            &mut full_truncated,
        );
        assert_eq!(full_buffer, oversized_chunk);
        assert!(!full_truncated);
        let (full_body, full_state) =
            super::build_stream_body_capture(&full_buffer, full_truncated);
        assert!(full_body.is_some());
        assert_eq!(full_state, Some(UsageBodyCaptureState::Inline));
        drop(full_body);

        let basic_limit =
            super::stream_body_buffer_limit_for_record_level(UsageRequestRecordLevel::Basic);
        assert_eq!(basic_limit, super::BASIC_STREAM_BODY_ANALYSIS_LIMIT_BYTES);
        let mut basic_buffer = Vec::new();
        let mut basic_truncated = false;
        super::append_stream_capture_bytes(
            &mut basic_buffer,
            &oversized_chunk,
            basic_limit,
            &mut basic_truncated,
        );
        assert_eq!(basic_buffer.len(), basic_limit);
        assert!(basic_truncated);
        let (basic_body, basic_state) =
            super::build_stream_body_capture(&basic_buffer, basic_truncated);
        assert!(basic_body.is_some());
        assert_eq!(basic_state, Some(UsageBodyCaptureState::Truncated));

        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-basic-stream-capture",
            UsageEventData {
                provider_name: "provider".to_string(),
                model: "model".to_string(),
                response_body: basic_body.map(Value::String),
                response_body_state: basic_state,
                client_response_body: Some(json!("captured client body")),
                client_response_body_state: Some(UsageBodyCaptureState::Truncated),
                ..UsageEventData::default()
            },
        );
        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Basic,
            },
            &mut event,
        );

        assert_eq!(event.data.response_body, None);
        assert_eq!(
            event.data.response_body_state,
            Some(UsageBodyCaptureState::Disabled)
        );
        assert_eq!(event.data.client_response_body, None);
        assert_eq!(
            event.data.client_response_body_state,
            Some(UsageBodyCaptureState::Disabled)
        );
    }

    #[test]
    fn provider_error_inspection_detects_response_failed_at_every_chunk_boundary() {
        let body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"status\":\"in_progress\"}}\n\n",
            "event: response.failed\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"type\":\"invalid_request\",\"message\":\"cyber policy rejected the request\",\"code\":\"cyber_policy_violation\",\"param\":\"input\"}}}\n\n",
        )
        .as_bytes();

        for split in 1..body.len() {
            let mut inspection = ProviderStreamErrorInspection::default();
            let detected = inspection
                .observe(None, &body[..split])
                .or_else(|| inspection.observe(None, &body[split..]))
                .unwrap_or_else(|| panic!("response.failed was missed at byte split {split}"));

            assert_eq!(
                detected.pointer("/error/code"),
                Some(&json!("cyber_policy_violation")),
                "string provider code changed at byte split {split}"
            );
            assert_eq!(
                detected.pointer("/error/param"),
                Some(&json!("input")),
                "provider error fields changed at byte split {split}"
            );
        }

        let mut inspection = ProviderStreamErrorInspection::default();
        let mut detected = None;
        for byte in body.chunks(1) {
            if let Some(error_body) = inspection.observe(None, byte) {
                detected = Some(error_body);
                break;
            }
        }
        let detected = detected.expect("byte-wise response.failed stream should be detected");
        assert_eq!(
            detected.pointer("/error/code"),
            Some(&json!("cyber_policy_violation"))
        );
        assert_eq!(detected.pointer("/error/param"), Some(&json!("input")));
    }

    #[tokio::test]
    async fn prefetched_codex_cyber_policy_violation_stops_failover_by_default() {
        let response = execute_prefetched_codex_cyber_policy_failure(false)
            .await
            .expect("default Codex cyber policy handling should return the provider error");

        assert_eq!(response.status().as_u16(), 200);
    }

    #[tokio::test]
    async fn prefetched_codex_cyber_policy_violation_retries_when_system_setting_is_enabled() {
        assert!(
            execute_prefetched_codex_cyber_policy_failure(true)
                .await
                .is_none(),
            "enabling cyber failover should retry the next candidate"
        );
    }

    #[tokio::test]
    async fn prefetched_transport_failure_retries_by_default() {
        assert!(matches!(
            execute_prefetched_transport_failure(false).await,
            AiAttemptExecutionOutcome::Retry {
                scope: AiAttemptRetryScope::Candidate,
                fallback_response: None,
            }
        ));
    }

    #[tokio::test]
    async fn prefetched_transport_failure_can_stop_without_matching_http_status_rules() {
        let AiAttemptExecutionOutcome::Responded(response) =
            execute_prefetched_transport_failure(true).await
        else {
            panic!("transport stop policy should return a local response");
        };
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn prefetched_http_error_frame_honors_continue_status_codes() {
        assert!(matches!(
            execute_prefetched_http_status_failure(true).await,
            AiAttemptExecutionOutcome::Retry { .. }
        ));
    }

    #[tokio::test]
    async fn prefetched_http_error_frame_honors_stop_status_codes() {
        let AiAttemptExecutionOutcome::Responded(response) =
            execute_prefetched_http_status_failure(false).await
        else {
            panic!("HTTP stop policy should return the upstream error");
        };
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    fn tunnel_proxy_snapshot(base_url: String) -> aether_contracts::ProxySnapshot {
        aether_contracts::ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".into()),
            node_id: Some("node-1".into()),
            label: Some("relay-node".into()),
            url: None,
            extra: Some(json!({"tunnel_base_url": base_url})),
        }
    }

    fn connect_json_frame(flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + payload.len());
        out.push(flags);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn ndjson_frame(frame: StreamFrame) -> Bytes {
        let mut bytes = serde_json::to_vec(&frame).expect("stream frame should serialize");
        bytes.push(b'\n');
        Bytes::from(bytes)
    }

    #[test]
    fn post_stop_reader_yields_after_bounded_empty_chunks() {
        let polls = Arc::new(AtomicUsize::new(0));
        let polls_for_stream = Arc::clone(&polls);
        let stream = futures_util::stream::poll_fn(move |_| {
            let poll = polls_for_stream.fetch_add(1, Ordering::SeqCst);
            if poll < POST_STOP_MAX_EMPTY_CHUNKS_PER_POLL * 2 {
                std::task::Poll::Ready(Some(Ok::<Bytes, std::io::Error>(Bytes::new())))
            } else {
                std::task::Poll::Ready(Some(Ok::<Bytes, std::io::Error>(Bytes::from_static(b"x"))))
            }
        });
        let mut reader = Box::pin(PostStopLimitedStreamReader::new(
            stream,
            PostStopFrameReadBudget::new(),
        ));
        let waker = futures_util::task::noop_waker();
        let mut context = std::task::Context::from_waker(&waker);
        let mut storage = [0u8; 1];
        let mut read_buf = tokio::io::ReadBuf::new(&mut storage);

        let result = tokio::io::AsyncRead::poll_read(reader.as_mut(), &mut context, &mut read_buf);

        assert!(result.is_pending());
        assert_eq!(
            polls.load(Ordering::SeqCst),
            POST_STOP_MAX_EMPTY_CHUNKS_PER_POLL
        );
    }

    #[tokio::test]
    async fn post_stop_activation_trims_prefetched_current_item_immediately() {
        const GIANT_TAIL_BYTES: usize = 4 * 1024 * 1024;

        let mut combined = ndjson_frame(StreamFrame {
            frame_type: StreamFrameType::Data,
            payload: StreamFramePayload::Data {
                chunk_b64: None,
                text: Some(
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string(),
                ),
            },
        })
        .to_vec();
        combined.resize(combined.len() + GIANT_TAIL_BYTES, b'x');
        let frame_stream =
            futures_util::stream::iter([Ok::<Bytes, std::io::Error>(Bytes::from(combined))]);
        let reader = PostStopLimitedStreamReader::new(frame_stream, PostStopFrameReadBudget::new());
        let mut lines =
            tokio_util::codec::FramedRead::new(reader, tokio_util::codec::LinesCodec::new());

        super::read_next_frame(&mut lines)
            .await
            .expect("frame should decode")
            .expect("data frame should exist");
        assert!(lines
            .get_ref()
            .current
            .as_ref()
            .is_some_and(|current| current.len() > ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES));

        let already_buffered = lines.read_buffer().len();
        lines.read_buffer_mut().reserve(GIANT_TAIL_BYTES);
        assert!(lines.read_buffer().capacity() >= GIANT_TAIL_BYTES);
        assert!(!activate_post_stop_frame_read_budget(&mut lines));
        let retained = lines
            .get_ref()
            .current
            .as_ref()
            .map(Bytes::len)
            .unwrap_or_default();
        assert_eq!(
            retained,
            ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES.saturating_sub(already_buffered)
        );
        assert_eq!(lines.read_buffer().len(), already_buffered);
        assert!(lines.read_buffer().capacity() <= ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES);
    }

    #[test]
    fn post_stop_activation_releases_over_limit_framed_buffer() {
        let frame_stream = futures_util::stream::empty::<Result<Bytes, std::io::Error>>();
        let reader = PostStopLimitedStreamReader::new(frame_stream, PostStopFrameReadBudget::new());
        let mut lines =
            tokio_util::codec::FramedRead::new(reader, tokio_util::codec::LinesCodec::new());
        lines
            .read_buffer_mut()
            .resize(ANTHROPIC_POST_STOP_DRAIN_MAX_BYTES + 1, b'x');

        assert!(activate_post_stop_frame_read_budget(&mut lines));
        assert!(lines.read_buffer().is_empty());
        assert_eq!(lines.read_buffer().capacity(), 0);
    }

    #[test]
    fn merge_stream_terminal_summary_prefers_more_complete_observed_usage() {
        let mut runtime_usage = StandardizedUsage::new();
        runtime_usage.output_tokens = 137;
        let mut observed_usage = StandardizedUsage::new();
        observed_usage.input_tokens = 26;
        observed_usage.output_tokens = 137;

        let merged = merge_stream_terminal_summary(
            Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(runtime_usage),
                model: Some("gpt-5.5".to_string()),
                provider_actual_service_tier: Some("priority".to_string()),
                unknown_event_count: 1,
                ..ExecutionStreamTerminalSummary::default()
            }),
            Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(observed_usage),
                response_id: Some("resp_123".to_string()),
                provider_actual_service_tier: Some("default".to_string()),
                observed_finish: true,
                unknown_event_count: 2,
                ..ExecutionStreamTerminalSummary::default()
            }),
        )
        .expect("summary should merge");
        let usage = merged
            .standardized_usage
            .expect("merged usage should exist");

        assert_eq!(usage.input_tokens, 26);
        assert_eq!(usage.output_tokens, 137);
        assert_eq!(merged.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(merged.response_id.as_deref(), Some("resp_123"));
        assert_eq!(
            merged.provider_actual_service_tier.as_deref(),
            Some("default")
        );
        assert!(merged.observed_finish);
        assert_eq!(merged.unknown_event_count, 3);
    }

    #[test]
    fn detects_missing_observed_finish_only_without_usage_signal() {
        assert!(stream_terminal_summary_missing_observed_finish(Some(
            &ExecutionStreamTerminalSummary {
                response_id: Some("resp_missing_finish".to_string()),
                model: Some("gpt-5.5".to_string()),
                observed_finish: false,
                ..ExecutionStreamTerminalSummary::default()
            }
        )));

        let mut usage = StandardizedUsage::new();
        usage.output_tokens = 12;
        assert!(!stream_terminal_summary_missing_observed_finish(Some(
            &ExecutionStreamTerminalSummary {
                standardized_usage: Some(usage),
                observed_finish: false,
                ..ExecutionStreamTerminalSummary::default()
            }
        )));
        assert!(!stream_terminal_summary_missing_observed_finish(Some(
            &ExecutionStreamTerminalSummary {
                observed_finish: true,
                ..ExecutionStreamTerminalSummary::default()
            }
        )));
        assert!(!stream_terminal_summary_missing_observed_finish(None));
    }

    #[test]
    fn requires_terminal_event_for_openai_responses_streams() {
        assert!(stream_requires_observed_terminal_event(
            "openai:responses",
            None
        ));
        assert!(stream_requires_observed_terminal_event(
            "openai:responses:compact",
            None
        ));
        assert!(!stream_requires_observed_terminal_event(
            "openai:chat",
            None
        ));
        assert!(stream_requires_observed_terminal_event(
            "openai:chat",
            Some(&json!({
                "provider_stream_event_api_format": "openai:responses"
            }))
        ));
    }

    #[test]
    fn synthesizes_missing_terminal_summary_for_openai_responses_empty_stream() {
        let mut summary = None;
        ensure_stream_terminal_summary_for_missing_observed_finish(&mut summary, true);

        let summary = summary.expect("summary should be synthesized");
        assert!(!summary.observed_finish);
        assert_eq!(
            summary.parser_error.as_deref(),
            Some("execution runtime stream ended before provider terminal event")
        );
        assert!(
            stream_terminal_summary_missing_observed_finish_with_requirement(Some(&summary), true)
        );
        assert!(stream_terminal_summary_represents_failure_with_requirement(
            Some(&summary),
            true
        ));
    }

    #[test]
    fn terminal_required_stream_fails_even_with_usage_without_finish() {
        let mut usage = StandardizedUsage::new();
        usage.output_tokens = 12;
        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(usage),
            observed_finish: false,
            ..ExecutionStreamTerminalSummary::default()
        });

        ensure_stream_terminal_summary_for_missing_observed_finish(&mut summary, true);
        let summary = summary.as_ref().expect("summary should remain present");
        assert!(
            stream_terminal_summary_missing_observed_finish_with_requirement(Some(summary), true)
        );
        assert!(stream_terminal_summary_represents_failure_with_requirement(
            Some(summary),
            true
        ));
    }

    #[tokio::test]
    async fn kiro_stream_summary_applies_prompt_cache_usage_from_original_request() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "cacheable system ".repeat(600),
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "cacheable prompt ".repeat(1200),
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-cache-stream".into(),
            candidate_id: Some("cand-kiro-cache-stream".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-cache-stream".into(),
            endpoint_id: "endpoint-kiro-cache-stream".into(),
            key_id: "key-kiro-cache-stream".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let state = test_state();

        let mut first_summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 6_000,
                output_tokens: 17,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            Some(&report_context),
            &mut first_summary,
        )
        .await;
        let first_usage = first_summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("first usage should exist");
        assert!(first_usage.cache_creation_tokens > 0);
        assert_eq!(first_usage.cache_read_tokens, 0);

        let mut second_summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 6_000,
                output_tokens: 19,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            Some(&report_context),
            &mut second_summary,
        )
        .await;
        let second_usage = second_summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("second usage should exist");
        assert!(second_usage.cache_read_tokens > 0);
        assert_eq!(second_usage.cache_creation_tokens, 0);
        assert!(second_usage.input_tokens < 6_000);
        assert_eq!(second_usage.output_tokens, 19);
    }

    #[tokio::test]
    async fn kiro_stream_summary_reads_cached_prefix_within_prompt_cache_lookback_window() {
        let first_request_body = json!({
            "model": "claude-sonnet-4.6",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "shared first turn ".repeat(600),
                    "cache_control": {"type": "ephemeral"}
                }]
            }]
        });
        let mut second_messages = vec![json!({
            "role": "user",
            "content": [{
                "type": "text",
                "text": "shared first turn ".repeat(600)
            }]
        })];
        for index in 0..12 {
            second_messages.push(json!({
                "role": if index % 2 == 0 { "assistant" } else { "user" },
                "content": format!("intermediate stream turn {index}")
            }));
        }
        second_messages.push(json!({
            "role": "user",
            "content": [{
                "type": "text",
                "text": "new tail turn ".repeat(600),
                "cache_control": {"type": "ephemeral"}
            }]
        }));
        let second_request_body = json!({
            "model": "claude-sonnet-4.6",
            "messages": second_messages
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-cache-stream-long-tail".into(),
            candidate_id: Some("cand-kiro-cache-stream-long-tail".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-cache-stream-long-tail".into(),
            endpoint_id: "endpoint-kiro-cache-stream-long-tail".into(),
            key_id: "key-kiro-cache-stream-long-tail".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-sonnet-4.6".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let first_report_context = json!({
            "original_request_body": first_request_body,
            "kiro_simulated_cache_enabled": true,
        });
        let second_report_context = json!({
            "original_request_body": second_request_body,
            "kiro_simulated_cache_enabled": true,
        });
        let state = test_state();

        let mut first_summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 4_000,
                output_tokens: 17,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            Some(&first_report_context),
            &mut first_summary,
        )
        .await;
        let first_usage = first_summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("first usage should exist");
        assert!(first_usage.cache_creation_tokens > 0);
        assert_eq!(first_usage.cache_read_tokens, 0);

        let mut second_summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 8_000,
                output_tokens: 19,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            Some(&second_report_context),
            &mut second_summary,
        )
        .await;
        let second_usage = second_summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("second usage should exist");
        assert!(
            second_usage.cache_read_tokens > 0,
            "stream summary should reuse the far earlier cached prefix"
        );
        assert!(second_usage.cache_creation_tokens > 0);
        assert_eq!(second_usage.output_tokens, 19);
    }

    #[tokio::test]
    async fn kiro_stream_summary_seeds_input_tokens_without_cache_control() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "non cacheable system ".repeat(400)
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "non cacheable prompt ".repeat(800)
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-non-cache".into(),
            candidate_id: Some("cand-kiro-non-cache".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-non-cache".into(),
            endpoint_id: "endpoint-kiro-non-cache".into(),
            key_id: "key-kiro-non-cache".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let state = test_state();

        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 0,
                output_tokens: 13,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            Some(&report_context),
            &mut summary,
        )
        .await;

        let usage = summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("usage should exist");

        assert!(usage.input_tokens > 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.output_tokens, 13);
    }

    #[tokio::test]
    async fn kiro_stream_summary_bills_existing_cache_usage_when_input_is_zero() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "cached system ".repeat(800)
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "cached prompt ".repeat(1400)
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-existing-cache".into(),
            candidate_id: Some("cand-kiro-existing-cache".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-existing-cache".into(),
            endpoint_id: "endpoint-kiro-existing-cache".into(),
            key_id: "key-kiro-existing-cache".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let state = test_state();

        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 0,
                output_tokens: 23,
                cache_read_tokens: 200,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            Some(&report_context),
            &mut summary,
        )
        .await;

        let usage = summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("usage should exist");

        assert!(usage.input_tokens > 0);
        assert_eq!(usage.cache_read_tokens, 200);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.output_tokens, 23);
    }

    #[tokio::test]
    async fn kiro_stream_summary_clears_cache_usage_when_simulated_cache_disabled() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "disabled cache summary system ".repeat(800),
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "disabled cache summary prompt ".repeat(1400),
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-summary-cache-disabled".into(),
            candidate_id: Some("cand-kiro-summary-cache-disabled".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-summary-cache-disabled".into(),
            endpoint_id: "endpoint-kiro-summary-cache-disabled".into(),
            key_id: "key-kiro-summary-cache-disabled".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let state = test_state();

        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 0,
                output_tokens: 23,
                cache_creation_tokens: 500,
                cache_read_tokens: 700,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            Some(&report_context),
            &mut summary,
        )
        .await;

        let usage = summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("usage should exist");

        assert!(usage.input_tokens > 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.output_tokens, 23);
    }

    #[tokio::test]
    async fn kiro_stream_summary_does_not_subtract_cache_from_already_billed_input() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "cached history ".repeat(400),
                            "cache_control": {"type": "ephemeral"}
                        },
                        {
                            "type": "text",
                            "text": "new user turn"
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
            "input_tokens": 24_770,
            "cache_creation_input_tokens": 175,
            "cache_read_input_tokens": 24_463,
            "kiro_simulated_cache_enabled": true
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-billed-input".into(),
            candidate_id: Some("cand-kiro-billed-input".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-billed-input".into(),
            endpoint_id: "endpoint-kiro-billed-input".into(),
            key_id: "key-kiro-billed-input".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let state = test_state();

        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 132,
                output_tokens: 167,
                cache_creation_tokens: 175,
                cache_read_tokens: 24_463,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &state,
            &plan,
            Some(&report_context),
            &mut summary,
        )
        .await;

        let usage = summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("usage should exist");

        assert_eq!(usage.input_tokens, 132);
        assert_eq!(usage.cache_creation_tokens, 175);
        assert_eq!(usage.cache_read_tokens, 24_463);
        assert_eq!(usage.output_tokens, 167);
    }

    #[tokio::test]
    async fn kiro_report_context_seeds_input_tokens_from_original_request_body() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "seeded system ".repeat(600)
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "seeded prompt ".repeat(1200)
                        }
                    ]
                }
            ]
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-seed".into(),
            candidate_id: Some("cand-kiro-seed".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-seed".into(),
            endpoint_id: "endpoint-kiro-seed".into(),
            key_id: "key-kiro-seed".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut report_context = Some(json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        }));

        super::seed_kiro_report_context_input_tokens(&plan, &mut report_context);

        let input_tokens = report_context
            .as_ref()
            .and_then(|context| context.get("input_tokens"))
            .and_then(Value::as_u64)
            .expect("kiro input tokens should be seeded");
        assert!(input_tokens > 0);
    }

    #[tokio::test]
    async fn kiro_report_context_seeds_prompt_cache_usage_before_stream_rewrite() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "cache seed system ".repeat(600),
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "cache seed prompt ".repeat(1200),
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]
                }
            ]
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-cache-seed".into(),
            candidate_id: Some("cand-kiro-cache-seed".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-cache-seed".into(),
            endpoint_id: "endpoint-kiro-cache-seed".into(),
            key_id: "key-kiro-cache-seed".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut report_context = Some(json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        }));
        let state = AppState::new().expect("gateway state should build");

        super::seed_kiro_report_context_input_tokens(&plan, &mut report_context);
        super::seed_kiro_report_context_prompt_cache_usage(&state, &plan, &mut report_context)
            .await;

        let context = report_context.as_ref().expect("context should exist");
        assert!(context
            .get("input_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0));
        assert!(context
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0));
        assert_eq!(
            context
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64),
            Some(0)
        );
    }

    #[tokio::test]
    async fn kiro_report_context_skips_prompt_cache_usage_when_disabled() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "disabled cache system ".repeat(600),
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "disabled cache prompt ".repeat(1200),
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]
                }
            ]
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-cache-disabled".into(),
            candidate_id: Some("cand-kiro-cache-disabled".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-cache-disabled".into(),
            endpoint_id: "endpoint-kiro-cache-disabled".into(),
            key_id: "key-kiro-cache-disabled".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut report_context = Some(json!({
            "original_request_body": request_body,
        }));
        let state = AppState::new().expect("gateway state should build");

        super::seed_kiro_report_context_input_tokens(&plan, &mut report_context);
        super::seed_kiro_report_context_prompt_cache_usage(&state, &plan, &mut report_context)
            .await;

        let context = report_context.as_ref().expect("context should exist");
        assert!(context
            .get("input_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0));
        assert_eq!(context.get("cache_creation_input_tokens"), None);
        assert_eq!(context.get("cache_read_input_tokens"), None);
    }

    #[test]
    fn native_anthropic_event_stream_uses_bounded_precommit() {
        assert!(!should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            Some("text/event-stream"),
            "claude:messages",
            "claude:messages",
            false,
            false,
            false,
        ));
    }

    #[test]
    fn native_anthropic_terminal_error_uses_anthropic_sse_shape() {
        let plan = native_anthropic_stream_plan("anthropic-terminal-error-shape");
        let failure = build_stream_failure_report(
            "execution_runtime_stream_read_error",
            "upstream disconnected",
            502,
        );

        let event = encode_terminal_sse_error_event_for_plan(&plan, &failure)
            .expect("terminal event should encode");
        let event = String::from_utf8(event.to_vec()).expect("event should be utf8");

        assert!(event.starts_with("event: error\ndata: "));
        assert!(event.contains("\"type\":\"error\""));
        assert!(event.contains("\"type\":\"api_error\""));
        assert!(event.contains("upstream disconnected"));
        assert!(!event.contains("[DONE]"));
    }

    #[tokio::test]
    async fn native_anthropic_error_before_semantic_event_allows_failover() {
        let unknown = "event: future_event\ndata: {\"type\":\"future_event\",\"value\":1}\n\n";
        let upstream_error = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let outcome = execute_native_anthropic_prefetch_stream(
            "req-anthropic-precommit-error",
            vec![unknown.to_string(), upstream_error.to_string()],
        )
        .await;

        let AiAttemptExecutionOutcome::Retry {
            scope,
            fallback_response: Some(fallback_response),
        } = outcome
        else {
            panic!("precommit 529 should retry with the upstream response preserved")
        };
        assert_eq!(scope, AiAttemptRetryScope::Provider);
        assert_eq!(fallback_response.status(), StatusCode::OK);
        let fallback_body = to_bytes(fallback_response.into_body(), usize::MAX)
            .await
            .expect("fallback response body should read");
        assert_eq!(
            fallback_body.as_ref(),
            format!("{unknown}{upstream_error}").as_bytes()
        );
    }

    #[tokio::test]
    async fn native_anthropic_eof_before_semantic_event_allows_failover() {
        let outcome = execute_native_anthropic_prefetch_stream(
            "req-anthropic-precommit-eof",
            vec![": ping\n\n".to_string()],
        )
        .await;

        let AiAttemptExecutionOutcome::Retry {
            scope,
            fallback_response,
        } = outcome
        else {
            panic!("EOF before the first semantic event should retry another endpoint")
        };
        assert_eq!(scope, AiAttemptRetryScope::Endpoint);
        assert!(fallback_response.is_none());
    }

    #[tokio::test]
    async fn native_anthropic_auth_error_moves_to_the_next_credential() {
        let upstream_error = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"invalid credential\"}}\n\n",
        );
        let outcome = execute_native_anthropic_prefetch_stream(
            "req-anthropic-precommit-auth-error",
            vec![upstream_error.to_string()],
        )
        .await;

        let AiAttemptExecutionOutcome::Retry {
            scope,
            fallback_response: Some(fallback_response),
        } = outcome
        else {
            panic!("precommit authentication error should retry another credential")
        };
        assert_eq!(scope, AiAttemptRetryScope::Credential);
        let fallback_body = to_bytes(fallback_response.into_body(), usize::MAX)
            .await
            .expect("fallback response body should read");
        assert_eq!(fallback_body.as_ref(), upstream_error.as_bytes());
    }

    #[tokio::test]
    async fn native_anthropic_semantic_event_commits_before_later_error() {
        let raw = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"late\"}}\n\n",
        );
        let outcome = execute_native_anthropic_prefetch_stream(
            "req-anthropic-postcommit-error",
            vec![raw.to_string()],
        )
        .await;
        let AiAttemptExecutionOutcome::Responded(response) = outcome else {
            panic!("a semantic event should commit the selected candidate")
        };
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");

        assert_eq!(body.as_ref(), raw.as_bytes());
    }

    #[tokio::test]
    async fn native_anthropic_frame_error_after_commit_emits_anthropic_terminal_event() {
        let message_start = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let original_error = "upstream disconnected after message_start";
        let outcome = execute_native_anthropic_prefetch_stream_with_terminal_error(
            "req-anthropic-postcommit-frame-error",
            vec![message_start.to_string()],
            Some(original_error.to_string()),
        )
        .await;
        let AiAttemptExecutionOutcome::Responded(response) = outcome else {
            panic!("a semantic event should commit the selected candidate")
        };
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body = String::from_utf8(body.to_vec()).expect("body should be utf8");

        assert!(body.starts_with(message_start));
        assert!(body.contains("event: error\ndata: {\"type\":\"error\""));
        assert!(body.contains("\"type\":\"api_error\""));
        assert!(body.contains(original_error));
        assert!(!body.contains("[DONE]"));
    }

    #[tokio::test]
    async fn native_anthropic_eof_after_commit_emits_anthropic_terminal_event() {
        let message_start = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let outcome = execute_native_anthropic_prefetch_stream(
            "req-anthropic-postcommit-eof",
            vec![message_start.to_string()],
        )
        .await;
        let AiAttemptExecutionOutcome::Responded(response) = outcome else {
            panic!("a semantic event should commit the selected candidate")
        };
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body = String::from_utf8(body.to_vec()).expect("body should be utf8");

        assert!(body.starts_with(message_start));
        assert!(body.contains("event: error\ndata: {\"type\":\"error\""));
        assert!(body.contains("ended before message_stop"));
        assert!(!body.contains("[DONE]"));
    }

    #[tokio::test]
    async fn native_anthropic_done_marker_does_not_replace_message_stop() {
        let message_start = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let done = "data: [DONE]\n\n";
        let outcome = execute_native_anthropic_prefetch_stream(
            "req-anthropic-done-without-message-stop",
            vec![message_start.to_string(), done.to_string()],
        )
        .await;
        let AiAttemptExecutionOutcome::Responded(response) = outcome else {
            panic!("message_start should commit the selected candidate")
        };
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body = String::from_utf8(body.to_vec()).expect("body should be utf8");

        assert!(body.starts_with(message_start));
        assert!(body.contains(done));
        assert!(body.contains("event: error\ndata: {\"type\":\"error\""));
        assert!(body.contains("ended before message_stop"));
    }

    #[tokio::test]
    async fn native_anthropic_hanging_tail_does_not_delay_body_eof_and_is_bounded() {
        let request_id = "req-anthropic-hanging-tail";
        let plan = native_anthropic_stream_plan(request_id);
        let provider_catalog = provider_catalog_for_plan(&plan, None);
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_usage_repository_for_tests(Arc::clone(
                    &usage_repository,
                ))
                .with_provider_catalog_reader(Arc::new(provider_catalog))
                .with_encryption_key_for_tests("development-key"),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
        let message_start = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let message_stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let stream_dropped = Arc::new(AtomicBool::new(false));
        let drop_flag = StreamDropFlag(Arc::clone(&stream_dropped));
        let frame_stream = stream! {
            let _drop_flag = drop_flag;
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(message_start.to_string()),
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(message_stop.to_string()),
                },
            }));
            std::future::pending::<()>().await;
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-anthropic-hanging-tail",
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": request_id,
                "candidate_id": format!("candidate-{request_id}"),
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "claude:messages",
                "client_api_format": "claude:messages"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            false,
            frame_stream,
            None,
        )
        .await
        .expect("stream execution should succeed")
        .expect("stream execution should return a response");

        let body = tokio::time::timeout(
            Duration::from_secs(1),
            to_bytes(response.into_body(), usize::MAX),
        )
        .await
        .expect("client body EOF must not wait for the hanging producer tail")
        .expect("client body should read");
        assert_eq!(
            body.as_ref(),
            format!("{message_start}{message_stop}").as_bytes()
        );

        tokio::time::timeout(Duration::from_secs(1), async {
            while !stream_dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("producer tail should be dropped after the bounded drain window");

        let stored_usage = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let usage = usage_repository
                    .find_by_request_id(request_id)
                    .await
                    .expect("usage should read");
                if usage
                    .as_ref()
                    .is_some_and(|usage| usage.status == "completed")
                {
                    break usage.expect("completed usage should exist");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("bounded drain timeout should still settle usage successfully");
        assert_eq!(stored_usage.status_code, Some(200));
    }

    #[tokio::test]
    async fn native_anthropic_unterminated_oversized_tail_respects_read_budget() {
        const TAIL_CHUNK_BYTES: usize = 4 * 1024 * 1024;

        let request_id = "req-anthropic-oversized-tail";
        let plan = native_anthropic_stream_plan(request_id);
        let provider_catalog = provider_catalog_for_plan(&plan, None);
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_usage_repository_for_tests(Arc::clone(
                    &usage_repository,
                ))
                .with_provider_catalog_reader(Arc::new(provider_catalog))
                .with_encryption_key_for_tests("development-key"),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
        let message_start = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let message_stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let stream_dropped = Arc::new(AtomicBool::new(false));
        let drop_flag = StreamDropFlag(Arc::clone(&stream_dropped));
        let tail_chunks_polled = Arc::new(AtomicUsize::new(0));
        let tail_chunks_polled_for_stream = Arc::clone(&tail_chunks_polled);
        let tail_chunk = Bytes::from(vec![b'x'; TAIL_CHUNK_BYTES]);
        let tail_chunk_count = 32;
        let frame_stream = stream! {
            let _drop_flag = drop_flag;
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(message_start.to_string()),
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(message_stop.to_string()),
                },
            }));
            for _ in 0..tail_chunk_count {
                tail_chunks_polled_for_stream.fetch_add(1, Ordering::SeqCst);
                yield Ok::<Bytes, std::io::Error>(tail_chunk.clone());
            }
            std::future::pending::<()>().await;
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-anthropic-oversized-tail",
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": request_id,
                "candidate_id": format!("candidate-{request_id}"),
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "claude:messages",
                "client_api_format": "claude:messages"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            false,
            frame_stream,
            None,
        )
        .await
        .expect("stream execution should succeed")
        .expect("stream execution should return a response");

        let body = tokio::time::timeout(
            Duration::from_secs(1),
            to_bytes(response.into_body(), usize::MAX),
        )
        .await
        .expect("client body EOF must not wait for the oversized unterminated tail")
        .expect("client body should read");
        assert_eq!(
            body.as_ref(),
            format!("{message_start}{message_stop}").as_bytes()
        );

        tokio::time::timeout(Duration::from_secs(1), async {
            while !stream_dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("oversized tail producer should be released by the read budget");
        assert!(
            tail_chunks_polled.load(Ordering::SeqCst) <= 1,
            "post-stop drain must retain at most one atomic upstream stream item"
        );

        let stored_usage = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let usage = usage_repository
                    .find_by_request_id(request_id)
                    .await
                    .expect("usage should read");
                if usage
                    .as_ref()
                    .is_some_and(|usage| usage.status == "completed")
                {
                    break usage.expect("completed usage should exist");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("bounded oversized tail should still settle usage successfully");
        assert_eq!(stored_usage.status_code, Some(200));
    }

    #[tokio::test]
    async fn native_anthropic_frame_error_after_message_stop_is_ignored() {
        let message_start = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let message_stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let outcome = execute_native_anthropic_prefetch_stream_with_terminal_error(
            "req-anthropic-error-after-message-stop",
            vec![message_start.to_string(), message_stop.to_string()],
            Some("connection reset after message_stop".to_string()),
        )
        .await;
        let AiAttemptExecutionOutcome::Responded(response) = outcome else {
            panic!("message_stop should keep the selected candidate committed")
        };
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");

        assert_eq!(
            body.as_ref(),
            format!("{message_start}{message_stop}").as_bytes()
        );
    }

    #[tokio::test]
    async fn native_anthropic_same_chunk_stops_at_message_stop_record() {
        let message_start = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let message_stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let trailing_error = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"after stop\"}}\n\n",
        );
        let outcome = execute_native_anthropic_prefetch_stream(
            "req-anthropic-same-chunk-message-stop",
            vec![format!("{message_start}{message_stop}{trailing_error}")],
        )
        .await;
        let AiAttemptExecutionOutcome::Responded(response) = outcome else {
            panic!("message_stop should complete the selected candidate")
        };
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");

        assert_eq!(
            body.as_ref(),
            format!("{message_start}{message_stop}").as_bytes()
        );
    }

    #[tokio::test]
    async fn direct_anthropic_stops_at_message_stop_and_ignores_teardown_error() {
        let message_start = Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let message_stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let trailing_error = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"after stop\"}}\n\n",
        );
        let state = direct_anthropic_inline_state(
            "req-direct-anthropic-message-stop",
            vec![
                Ok(message_start.clone()),
                Ok(Bytes::from(format!("{message_stop}{trailing_error}"))),
                Err("connection reset after message_stop".to_string()),
            ],
        );

        let (first, state) = state
            .next_item()
            .await
            .expect("message_start should stream");
        assert_eq!(first.expect("message_start should succeed"), message_start);
        let (second, state) = state.next_item().await.expect("message_stop should stream");
        assert_eq!(
            second.expect("message_stop should succeed").as_ref(),
            message_stop.as_bytes()
        );
        assert!(state.next_item().await.is_none());
    }

    #[tokio::test]
    async fn direct_anthropic_clean_eof_after_message_start_emits_one_error() {
        let message_start = Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let state = direct_anthropic_inline_state(
            "req-direct-anthropic-premature-eof",
            vec![Ok(message_start.clone())],
        );

        let (first, state) = state
            .next_item()
            .await
            .expect("message_start should stream");
        assert_eq!(first.expect("message_start should succeed"), message_start);
        let (error, mut state) = state
            .next_item()
            .await
            .expect("premature EOF should emit an Anthropic error event");
        let error = String::from_utf8(error.expect("error event should succeed").to_vec())
            .expect("error event should be utf8");
        assert!(error.starts_with("event: error\ndata: "));
        assert!(error.contains("ended before message_stop"));
        discard_direct_test_finalizer(&mut state);
    }

    #[tokio::test]
    async fn direct_anthropic_provider_error_is_not_duplicated() {
        let message_start = Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let provider_error = Bytes::from_static(
            b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\n\n",
        );
        let state = direct_anthropic_inline_state(
            "req-direct-anthropic-provider-error",
            vec![Ok(message_start.clone()), Ok(provider_error.clone())],
        );

        let (first, state) = state
            .next_item()
            .await
            .expect("message_start should stream");
        assert_eq!(first.expect("message_start should succeed"), message_start);
        let (second, state) = state
            .next_item()
            .await
            .expect("provider error should stream");
        assert_eq!(
            second.expect("provider error should succeed"),
            provider_error
        );
        assert!(state.terminal_error_sent);
        assert!(state.next_item().await.is_none());
    }

    #[test]
    fn postcommit_anthropic_errors_use_the_precommit_status_taxonomy() {
        for (error_type, expected_status) in [
            ("request_too_large", 413),
            ("overloaded_error", 529),
            ("api_error", 500),
        ] {
            let body = json!({
                "type": "error",
                "error": { "type": error_type, "message": "upstream failure" }
            });
            assert_eq!(
                resolve_provider_stream_error_status_code("claude:messages", 200, &body),
                expected_status,
            );
        }
    }

    #[tokio::test]
    async fn native_anthropic_unknown_event_is_replayed_byte_for_byte() {
        let unknown = "event: future_event\ndata: {\"type\":\"future_event\",\"value\":1}\n\n";
        let message_start = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{}}\n\n",
        );
        let message_stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let expected = format!("{unknown}{message_start}{message_stop}");
        let outcome = execute_native_anthropic_prefetch_stream(
            "req-anthropic-unknown-replay",
            vec![
                unknown.to_string(),
                message_start.to_string(),
                message_stop.to_string(),
            ],
        )
        .await;
        let AiAttemptExecutionOutcome::Responded(response) = outcome else {
            panic!("unknown event should not terminate the stream")
        };
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");

        assert_eq!(body.as_ref(), expected.as_bytes());
    }

    #[test]
    fn skips_prefetch_for_same_format_passthrough_streams_without_content_type() {
        assert!(should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            None,
            "claude:messages",
            "claude:messages",
            false,
            false,
            false,
        ));
    }

    #[test]
    fn keeps_prefetch_for_same_format_json_streams() {
        assert!(!should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            Some("application/json"),
            "claude:messages",
            "claude:messages",
            false,
            false,
            false,
        ));
    }

    #[test]
    fn skips_prefetch_for_event_streams_even_when_cross_format_or_rewritten() {
        assert!(should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            Some("text/event-stream"),
            "openai:chat",
            "claude:messages",
            false,
            true,
            false,
        ));
    }

    #[test]
    fn cyber_failover_setting_forces_prefetch_for_event_streams() {
        assert!(!should_skip_direct_finalize_prefetch(
            Some("openai_responses_sync_finalize"),
            Some("text/event-stream"),
            "openai:responses",
            "openai:responses",
            false,
            false,
            true,
        ));
    }

    #[test]
    fn cyber_prefetch_waits_through_response_setup_until_output() {
        assert!(!prefetched_openai_responses_body_has_output_boundary(
            b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n"
        ));
        assert!(prefetched_openai_responses_body_has_output_boundary(
            b"event: response.created\ndata: {\"type\":\"response.created\"}\n\nevent: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n"
        ));
    }

    #[test]
    fn skips_success_failover_probe_for_event_streams() {
        assert!(!should_probe_success_failover_before_stream(
            &BTreeMap::from([(
                "content-type".to_string(),
                "text/event-stream; charset=utf-8".to_string(),
            )])
        ));
        assert!(should_probe_success_failover_before_stream(
            &BTreeMap::from([("content-type".to_string(), "application/json".to_string(),)])
        ));
    }

    #[test]
    fn limits_prefetch_for_openai_image_and_rewritten_streams() {
        assert!(should_limit_direct_finalize_prefetch(
            "openai_image_stream",
            false
        ));
        assert!(should_limit_direct_finalize_prefetch(
            "openai_chat_stream",
            true
        ));
        assert!(!should_limit_direct_finalize_prefetch(
            "openai_chat_stream",
            false
        ));
    }

    #[test]
    fn direct_passthrough_mode_defaults_inline_and_accepts_legacy() {
        assert_eq!(
            parse_direct_passthrough_mode(""),
            DirectPassthroughMode::Inline
        );
        assert_eq!(
            parse_direct_passthrough_mode("inline"),
            DirectPassthroughMode::Inline
        );
        assert_eq!(
            parse_direct_passthrough_mode("legacy"),
            DirectPassthroughMode::Legacy
        );
        assert_eq!(
            parse_direct_passthrough_mode("mpsc"),
            DirectPassthroughMode::Legacy
        );
    }

    #[test]
    fn openai_client_formats_disallow_proxy_generated_sse_control_blocks() {
        let mut plan = ExecutionPlan {
            request_id: "req-openai-keepalive".into(),
            candidate_id: Some("cand-openai-keepalive".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/v1/chat/completions".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        assert!(!client_format_allows_proxy_generated_sse_control_blocks(
            &plan
        ));
        plan.client_api_format = "openai:responses".into();
        assert!(!client_format_allows_proxy_generated_sse_control_blocks(
            &plan
        ));
        plan.client_api_format = "claude:messages".into();
        assert!(client_format_allows_proxy_generated_sse_control_blocks(
            &plan
        ));
    }

    #[tokio::test]
    async fn native_anthropic_sse_body_ends_at_message_stop_while_sender_is_alive() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let message_stop =
            Bytes::from_static(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        tx.send(Ok(message_stop.clone()))
            .await
            .expect("message_stop should send");
        let mut body_stream = Box::pin(build_sse_body_stream(
            Vec::new(),
            rx,
            true,
            false,
            true,
            Duration::from_secs(60),
        ));

        let chunk = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("message_stop should arrive immediately")
            .expect("stream should yield message_stop")
            .expect("message_stop should be successful");
        assert_eq!(chunk, message_stop);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), body_stream.next())
                .await
                .expect("body EOF must not wait for the producer")
                .is_none()
        );
        assert!(tx.is_closed(), "body EOF should drop the receiver");
    }

    #[tokio::test]
    async fn sse_body_stream_emits_initial_and_periodic_keepalive_without_business_chunks() {
        let (_tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            Vec::new(),
            rx,
            true,
            true,
            false,
            Duration::from_millis(10),
        ));

        let first = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("initial keepalive should be immediate")
            .expect("stream should yield initial keepalive")
            .expect("initial keepalive should be ok");
        assert_eq!(first.as_ref(), b": aether-keepalive\n\n");

        let second = tokio::time::timeout(Duration::from_millis(100), body_stream.next())
            .await
            .expect("periodic keepalive should arrive")
            .expect("stream should yield periodic keepalive")
            .expect("periodic keepalive should be ok");
        assert_eq!(second.as_ref(), b": aether-keepalive\n\n");
    }

    #[tokio::test]
    async fn sse_body_stream_filters_control_blocks_without_synthetic_keepalive() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            vec![Bytes::from_static(b": upstream-keepalive\n\n")],
            rx,
            true,
            false,
            false,
            Duration::from_millis(10),
        ));

        assert!(
            tokio::time::timeout(Duration::from_millis(30), body_stream.next())
                .await
                .is_err(),
            "control-only prefetched blocks should not produce client-visible chunks"
        );

        tx.send(Ok(Bytes::from_static(
            b"data: {\"id\":\"chatcmpl-no-keepalive\"}\n\n",
        )))
        .await
        .expect("business chunk should send");
        let chunk = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("business chunk should arrive")
            .expect("stream should yield business chunk")
            .expect("business chunk should be ok");
        assert_eq!(
            chunk.as_ref(),
            b"data: {\"id\":\"chatcmpl-no-keepalive\"}\n\n"
        );
    }

    #[tokio::test]
    async fn sse_body_stream_drops_upstream_control_only_blocks() {
        let (_tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            vec![
                Bytes::from_static(b": upstream-keepalive\n\n"),
                Bytes::from_static(b"event: ping\nid: 1\nretry: 1000\n\n"),
                Bytes::from_static(
                    b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
                ),
            ],
            rx,
            true,
            true,
            false,
            Duration::from_secs(60),
        ));

        let chunk = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("business chunk should arrive")
            .expect("stream should yield business chunk")
            .expect("business chunk should be ok");
        let text = std::str::from_utf8(chunk.as_ref()).expect("chunk should be utf8");
        assert!(text.contains("event: response.output_text.delta"));
        assert!(text.contains("data: {\"type\":\"response.output_text.delta\""));
        assert!(!text.contains("upstream-keepalive"));
        assert!(!text.contains("event: ping"));
        assert!(!text.contains("retry: 1000"));
    }

    #[tokio::test]
    async fn sse_body_stream_filters_control_blocks_across_chunk_boundaries() {
        let (_tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            vec![
                Bytes::from_static(b": upstream-keepalive\n"),
                Bytes::from_static(b"\n"),
                Bytes::from_static(b"event: response.created\n"),
                Bytes::from_static(b"data: {\"type\":\"response.created\"}\n\n"),
            ],
            rx,
            true,
            true,
            false,
            Duration::from_secs(60),
        ));

        let chunk = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("business chunk should arrive")
            .expect("stream should yield business chunk")
            .expect("business chunk should be ok");
        let text = std::str::from_utf8(chunk.as_ref()).expect("chunk should be utf8");
        assert_eq!(
            text,
            "event: response.created\ndata: {\"type\":\"response.created\"}\n\n"
        );
    }

    #[tokio::test]
    async fn sse_body_stream_forwards_data_line_before_block_boundary() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(4);
        let mut body_stream = Box::pin(build_sse_body_stream(
            Vec::new(),
            rx,
            true,
            true,
            false,
            Duration::from_secs(60),
        ));

        let keepalive = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("initial keepalive should be immediate")
            .expect("stream should yield initial keepalive")
            .expect("initial keepalive should be ok");
        assert_eq!(keepalive.as_ref(), b": aether-keepalive\n\n");

        tx.send(Ok(Bytes::from_static(
            b"event: response.output_text.delta\n",
        )))
        .await
        .expect("event line should send");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), body_stream.next())
                .await
                .is_err(),
            "event-only partial block should remain buffered"
        );

        tx.send(Ok(Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n",
        )))
        .await
        .expect("data line should send");
        let data_chunk = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("data-bearing block should stream before terminator")
            .expect("stream should yield data-bearing block")
            .expect("data-bearing block should be ok");
        assert_eq!(
            data_chunk.as_ref(),
            b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n"
        );

        tx.send(Ok(Bytes::from_static(b"\n")))
            .await
            .expect("terminator should send");
        let terminator = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("terminator should stream")
            .expect("stream should yield terminator")
            .expect("terminator should be ok");
        assert_eq!(terminator.as_ref(), b"\n");
    }

    #[tokio::test]
    async fn sse_body_stream_uses_local_keepalive_when_prefetched_blocks_are_control_only() {
        let (_tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            vec![Bytes::from_static(b": upstream-keepalive\n\n")],
            rx,
            true,
            true,
            false,
            Duration::from_secs(60),
        ));

        let first = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("local keepalive should arrive")
            .expect("stream should yield local keepalive")
            .expect("local keepalive should be ok");
        assert_eq!(first.as_ref(), b": aether-keepalive\n\n");
    }

    #[tokio::test]
    async fn execute_stream_from_frame_stream_does_not_finalize_rewritten_tool_call_after_midstream_error(
    ) {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
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
        let plan = ExecutionPlan {
            request_id: "req-responses-tool-midstream-error".into(),
            candidate_id: Some("cand-responses-tool-midstream-error".into()),
            provider_name: Some("openai".into()),
            provider_id: "provider-openai-responses".into(),
            endpoint_id: "endpoint-openai-responses".into(),
            key_id: "key-openai-responses".into(),
            method: "POST".into(),
            url: "https://api.openai.com/v1/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.5",
                "input": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "openai:responses".into(),
            model_name: Some("gpt-5.5".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let upstream_chunk = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_midstream_error\",\"model\":\"gpt-5.5\",\"status\":\"in_progress\"}}\n\n",
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"lookup\",\"arguments\":\"\",\"status\":\"in_progress\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"item_id\":\"fc_1\",\"call_id\":\"call_1\",\"delta\":\"{\\\"query\\\":\\\"abc\"}\n\n"
        );
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Headers,
                payload: StreamFramePayload::Headers {
                    status_code: 200,
                    headers: BTreeMap::from([(
                        "content-type".to_string(),
                        "text/event-stream".to_string(),
                    )]),
                    response_observation: None,
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Data,
                payload: StreamFramePayload::Data {
                    chunk_b64: None,
                    text: Some(upstream_chunk.to_string()),
                },
            }));
            yield Ok::<Bytes, std::io::Error>(ndjson_frame(StreamFrame {
                frame_type: StreamFrameType::Error,
                payload: StreamFramePayload::Error {
                    error: ExecutionError {
                        kind: ExecutionErrorKind::Internal,
                        phase: ExecutionPhase::StreamRead,
                        message: "error reading a body from connection: stream error received: unexpected internal error encountered".to_string(),
                        upstream_status: Some(200),
                        retryable: false,
                        failover_recommended: false,
                    },
                },
            }));
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-responses-tool-midstream-error",
            &test_decision(),
            "openai_responses_stream",
            Some("openai_responses_stream_success".to_string()),
            Some(json!({
                "request_id": "req-responses-tool-midstream-error",
                "candidate_id": "cand-responses-tool-midstream-error",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:responses",
                "client_api_format": "claude:messages",
                "needs_conversion": true,
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_text = String::from_utf8(body.to_vec()).expect("body should be utf8");
        assert!(body_text.contains("event: content_block_start"));
        assert!(body_text.contains("event: content_block_delta"));
        assert!(body_text.contains("\"type\":\"tool_use\""));
        assert!(!body_text.contains("event: content_block_stop"));
        assert!(!body_text.contains("event: message_delta"));
        assert!(!body_text.contains("event: message_stop"));
        assert!(!body_text.contains("\"stop_reason\":\"tool_use\""));
        assert!(body_text.contains("\"error\""));
        assert!(body_text.contains("unexpected internal error encountered"));
        assert!(body_text.contains("data: [DONE]"));

        let candidates = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = request_candidate_repository
                    .list_by_request_id("req-responses-tool-midstream-error")
                    .await
                    .expect("request candidates should read");
                if candidates
                    .first()
                    .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Failed)
                {
                    break candidates;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("candidate should be marked failed");
        assert_eq!(candidates[0].status_code, Some(200));
        assert_eq!(candidates[0].error_type.as_deref(), Some("internal"));
    }

    #[tokio::test]
    async fn openai_image_stream_ignores_plan_total_timeout() {
        let state = AppState::new().expect("app state should build");
        let plan = ExecutionPlan {
            request_id: "req-image-stream-timeout".into(),
            candidate_id: Some("cand-image-stream-timeout".into()),
            provider_name: Some("codex".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-image-1",
                "prompt": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:image".into(),
            provider_api_format: "openai:image".into(),
            model_name: Some("gpt-image-1".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                total_ms: Some(25),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/images/generations",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("image".to_string()),
            Some("openai:image".to_string()),
        )
        .with_execution_runtime_candidate(true);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
            ));
            std::future::pending::<()>().await;
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-image-stream-timeout",
            &decision,
            "openai_image_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:image",
                "client_api_format": "openai:image",
                "image_request": {
                    "operation": "generate"
                }
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let mut body_stream = response.into_body().into_data_stream();
        let next_chunk = tokio::time::timeout(Duration::from_millis(100), body_stream.next()).await;
        assert!(
            next_chunk.is_err(),
            "stream total_ms must not synthesize a keepalive, image failure, or close the response body"
        );
    }

    #[tokio::test]
    async fn execute_stream_from_frame_stream_treats_windsurf_connect_trailer_error_as_failure() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
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
        let plan = ExecutionPlan {
            request_id: "req-windsurf-connect-error".into(),
            candidate_id: Some("cand-windsurf-connect-error".into()),
            provider_name: Some("windsurf".into()),
            provider_id: "provider-windsurf".into(),
            endpoint_id: "endpoint-windsurf-chat".into(),
            key_id: "key-windsurf".into(),
            method: "POST".into(),
            url: "https://server.codeium.com/exa.api_server_pb.ApiServerService/GetChatMessage?beta=true".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/connect+json".into()),
                ("accept".into(), "application/connect+json".into()),
            ]),
            content_type: Some("application/connect+json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "claude-sonnet-4",
                "messages": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("claude-sonnet-4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let state = state.with_data_state_for_tests(
            crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                Arc::clone(&request_candidate_repository),
                Arc::clone(&usage_repository),
            )
            .with_provider_catalog_reader(Arc::new(provider_catalog_stop_429_for_plan(&plan)))
            .with_encryption_key_for_tests("development-key"),
        );
        let trailer_error = connect_json_frame(
            2,
            br#"{"error":{"code":"resource_exhausted","message":"an internal error occurred"}}"#,
        );
        let trailer_error_b64 = base64::engine::general_purpose::STANDARD.encode(trailer_error);
        let frame = format!(
            "{{\"type\":\"data\",\"payload\":{{\"kind\":\"data\",\"chunk_b64\":\"{trailer_error_b64}\"}}}}\n"
        );
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"application/connect+json\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from(frame));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n",
            ));
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-windsurf-connect-error",
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": "req-windsurf-connect-error",
                "candidate_id": "cand-windsurf-connect-error",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:chat",
                "client_api_format": "claude:messages",
                "needs_conversion": true,
                "has_envelope": true,
                "envelope_name": "windsurf:GetChatMessage"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_json: Value =
            serde_json::from_slice(&body).expect("response body should decode as json");
        assert_eq!(status.as_u16(), 429);
        assert_eq!(body_json["type"], json!("error"));
        assert_eq!(body_json["error"]["type"], json!("rate_limit_error"));
        assert_eq!(body_json["error"]["code"], json!("resource_exhausted"));
        assert_eq!(
            body_json["error"]["message"],
            json!("an internal error occurred")
        );

        let candidates = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = request_candidate_repository
                    .list_by_request_id("req-windsurf-connect-error")
                    .await
                    .expect("request candidates should read");
                if candidates
                    .first()
                    .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Failed)
                {
                    break candidates;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("candidate should be marked failed");
        assert_eq!(candidates[0].status_code, Some(429));
        assert_eq!(
            candidates[0].error_type.as_deref(),
            Some("resource_exhausted")
        );
    }

    #[tokio::test]
    async fn execute_stream_from_frame_stream_decodes_non_success_windsurf_connect_error_body() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
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
        let plan = ExecutionPlan {
            request_id: "req-windsurf-connect-429".into(),
            candidate_id: Some("cand-windsurf-connect-429".into()),
            provider_name: Some("windsurf".into()),
            provider_id: "provider-windsurf".into(),
            endpoint_id: "endpoint-windsurf-chat".into(),
            key_id: "key-windsurf".into(),
            method: "POST".into(),
            url: "https://server.codeium.com/exa.api_server_pb.ApiServerService/GetChatMessage?beta=true".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/connect+json".into()),
                ("accept".into(), "application/connect+json".into()),
            ]),
            content_type: Some("application/connect+json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "claude-sonnet-4",
                "messages": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("claude-sonnet-4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let state = state.with_data_state_for_tests(
            crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                Arc::clone(&request_candidate_repository),
                Arc::clone(&usage_repository),
            )
            .with_provider_catalog_reader(Arc::new(provider_catalog_stop_429_for_plan(&plan)))
            .with_encryption_key_for_tests("development-key"),
        );
        let connect_error = connect_json_frame(
            2,
            br#"{"error":{"code":"resource_exhausted","message":"quota exhausted"}}"#,
        );
        let connect_error_b64 = base64::engine::general_purpose::STANDARD.encode(connect_error);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":429,\"headers\":{\"content-type\":\"application/connect+json\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from(format!(
                "{{\"type\":\"data\",\"payload\":{{\"kind\":\"data\",\"chunk_b64\":\"{connect_error_b64}\"}}}}\n"
            )));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n",
            ));
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-windsurf-connect-429",
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": "req-windsurf-connect-429",
                "candidate_id": "cand-windsurf-connect-429",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:chat",
                "client_api_format": "claude:messages",
                "needs_conversion": true,
                "has_envelope": true,
                "envelope_name": "windsurf:GetChatMessage"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        assert_eq!(response.status().as_u16(), 429);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_json: Value =
            serde_json::from_slice(&body).expect("response body should decode as json");
        assert_eq!(body_json["type"], json!("error"));
        assert_eq!(body_json["error"]["type"], json!("rate_limit_error"));
        assert_eq!(body_json["error"]["code"], json!("resource_exhausted"));
        assert_eq!(body_json["error"]["message"], json!("quota exhausted"));

        let record = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(usage) = usage_repository
                    .find_by_request_id("req-windsurf-connect-429")
                    .await
                    .expect("usage should read")
                    .filter(|usage| usage.status == "failed")
                {
                    break usage;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be written");
        assert_eq!(record.status_code, Some(429));
        assert_eq!(
            record
                .response_body
                .as_ref()
                .and_then(|body| body.get("error"))
                .and_then(|error| error.get("code")),
            Some(&json!("resource_exhausted"))
        );
        assert!(record.response_body_ref.is_none());
    }

    #[tokio::test]
    async fn execute_stream_from_frame_stream_cancels_upstream_when_client_drops_body() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
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
        let plan = ExecutionPlan {
            request_id: "req-client-drop-cancels-upstream".into(),
            candidate_id: Some("cand-client-drop-cancels-upstream".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/v1/chat/completions".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "messages": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let release_terminal = Arc::new(Notify::new());
        let terminal_frame_drained = Arc::new(Notify::new());
        let release_terminal_for_stream = Arc::clone(&release_terminal);
        let terminal_frame_drained_for_stream = Arc::clone(&terminal_frame_drained);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"first\\\",\\\"choices\\\":[{\\\"index\\\":0,\\\"delta\\\":{\\\"content\\\":\\\"hello\\\"}}]}\\n\\n\"}}\n",
            ));
            release_terminal_for_stream.notified().await;
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"terminal\\\",\\\"object\\\":\\\"chat.completion.chunk\\\",\\\"model\\\":\\\"gpt-5.4\\\",\\\"choices\\\":[{\\\"index\\\":0,\\\"delta\\\":{},\\\"finish_reason\\\":\\\"stop\\\"}],\\\"usage\\\":{\\\"prompt_tokens\\\":7,\\\"completion_tokens\\\":11,\\\"total_tokens\\\":18}}\\n\\ndata: [DONE]\\n\\n\"}}\n",
            ));
            terminal_frame_drained_for_stream.notify_one();
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-client-drop-cancels-upstream",
            &test_decision(),
            "openai_chat_stream",
            None,
            Some(json!({
                "request_id": "req-client-drop-cancels-upstream",
                "candidate_id": "cand-client-drop-cancels-upstream",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:chat",
                "client_api_format": "openai:chat"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let mut body_stream = response.into_body().into_data_stream();
        let first = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let chunk = body_stream
                    .next()
                    .await
                    .expect("body should yield first chunk")
                    .expect("first chunk should be ok");
                if chunk.as_ref() != b": aether-keepalive\n\n" {
                    break chunk;
                }
            }
        })
        .await
        .expect("first business chunk should arrive");
        assert_eq!(
            first.as_ref(),
            b"data: {\"id\":\"first\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(body_stream);
        let candidates = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = request_candidate_repository
                    .list_by_request_id("req-client-drop-cancels-upstream")
                    .await
                    .expect("request candidates should read");
                if candidates
                    .first()
                    .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Cancelled)
                {
                    break candidates;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("candidate should be marked cancelled");
        assert_eq!(candidates[0].status_code, Some(499));
        assert_eq!(
            candidates[0].error_type.as_deref(),
            Some("downstream_disconnect")
        );

        let stored_usage = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let usage = usage_repository
                    .find_by_request_id("req-client-drop-cancels-upstream")
                    .await
                    .expect("usage should read");
                if usage
                    .as_ref()
                    .is_some_and(|usage| usage.status == "cancelled")
                {
                    break usage.expect("cancelled usage should exist");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be marked cancelled");
        assert_eq!(stored_usage.billing_status, "void");
        assert_eq!(stored_usage.status_code, Some(499));
        assert_eq!(stored_usage.input_tokens, 0);
        assert_eq!(stored_usage.output_tokens, 0);
        assert_eq!(stored_usage.total_tokens, 0);
        let first_byte_time_ms = stored_usage
            .first_byte_time_ms
            .expect("cancelled stream should retain first byte time");
        let response_time_ms = stored_usage
            .response_time_ms
            .expect("cancelled stream should record terminal duration");
        assert!(
            response_time_ms > first_byte_time_ms,
            "terminal duration should include time after the first byte"
        );

        release_terminal.notify_one();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                terminal_frame_drained.notified()
            )
            .await
            .is_err(),
            "upstream frame stream should stop when the client disconnects"
        );
    }

    #[tokio::test]
    async fn split_done_then_downstream_close_is_recorded_success() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
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
        let plan = ExecutionPlan {
            request_id: "req-split-done-close-success".into(),
            candidate_id: Some("cand-split-done-close-success".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/v1/chat/completions".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "messages": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let release_eof = Arc::new(Notify::new());
        let release_eof_for_stream = Arc::clone(&release_eof);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"first\\\",\\\"object\\\":\\\"chat.completion.chunk\\\",\\\"model\\\":\\\"gpt-5.4\\\",\\\"choices\\\":[{\\\"index\\\":0,\\\"delta\\\":{\\\"content\\\":\\\"hi\\\"},\\\"finish_reason\\\":null}]}\\n\\n\"}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"terminal\\\",\\\"object\\\":\\\"chat.completion.chunk\\\",\\\"model\\\":\\\"gpt-5.4\\\",\\\"choices\\\":[{\\\"index\\\":0,\\\"delta\\\":{},\\\"finish_reason\\\":\\\"stop\\\"}],\\\"usage\\\":{\\\"prompt_tokens\\\":7,\\\"completion_tokens\\\":11,\\\"total_tokens\\\":18}}\\n\\n\"}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DO\"}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"NE]\\n\\n\"}}\n",
            ));
            release_eof_for_stream.notified().await;
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-split-done-close-success",
            &test_decision(),
            "openai_chat_stream",
            None,
            Some(json!({
                "request_id": "req-split-done-close-success",
                "candidate_id": "cand-split-done-close-success",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:chat",
                "client_api_format": "openai:chat"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let mut body_stream = response.into_body().into_data_stream();
        let mut body = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !String::from_utf8_lossy(&body).contains("data: [DONE]") {
                let chunk = body_stream
                    .next()
                    .await
                    .expect("body should yield until done")
                    .expect("chunk should be ok");
                body.extend_from_slice(&chunk);
            }
        })
        .await
        .expect("final DONE should arrive");
        drop(body_stream);
        release_eof.notify_one();

        let candidates = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = request_candidate_repository
                    .list_by_request_id("req-split-done-close-success")
                    .await
                    .expect("request candidates should read");
                if candidates
                    .first()
                    .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Success)
                {
                    break candidates;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("candidate should be marked success");
        assert_eq!(candidates[0].status_code, Some(200));

        let stored_usage = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let usage = usage_repository
                    .find_by_request_id("req-split-done-close-success")
                    .await
                    .expect("usage should read");
                if usage
                    .as_ref()
                    .is_some_and(|usage| usage.status == "completed")
                {
                    break usage.expect("completed usage should exist");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be marked completed");
        assert_eq!(stored_usage.status_code, Some(200));
        assert_eq!(stored_usage.input_tokens, 7);
        assert_eq!(stored_usage.output_tokens, 11);
        assert_eq!(stored_usage.total_tokens, 18);
    }

    #[tokio::test]
    async fn image_stream_downstream_close_after_done_is_recorded_success() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
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
        let plan = ExecutionPlan {
            request_id: "req-image-done-close-success".into(),
            candidate_id: Some("cand-image-done-close-success".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/v1/images/generations".into(),
            headers: BTreeMap::from([("accept".into(), "text/event-stream".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-image-2",
                "prompt": "draw a small image",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:image".into(),
            model_name: Some("gpt-image-2".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"event: response.output_item.done\\ndata: {\\\"type\\\":\\\"response.output_item.done\\\",\\\"output_index\\\":0,\\\"item\\\":{\\\"id\\\":\\\"ig_1\\\",\\\"type\\\":\\\"image_generation_call\\\",\\\"result\\\":\\\"aGVsbG8=\\\"}}\\n\\nevent: response.completed\\ndata: {\\\"type\\\":\\\"response.completed\\\",\\\"response\\\":{\\\"id\\\":\\\"resp_1\\\",\\\"model\\\":\\\"gpt-image-2\\\",\\\"status\\\":\\\"completed\\\",\\\"usage\\\":null}}\\n\\n\"}}\n",
            ));
            std::future::pending::<()>().await;
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-image-done-close-success",
            &test_decision(),
            "openai_chat_stream",
            Some("openai_chat_stream_success".to_string()),
            Some(json!({
                "request_id": "req-image-done-close-success",
                "candidate_id": "cand-image-done-close-success",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:image",
                "client_api_format": "openai:chat",
                "image_request": {
                    "size": "1024x1024",
                    "quality": "medium"
                }
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            RequestStageTrace::from_env(),
            true,
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let mut body_stream = response.into_body().into_data_stream();
        let mut body = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !String::from_utf8_lossy(&body).contains("data: [DONE]") {
                let chunk = body_stream
                    .next()
                    .await
                    .expect("body should yield until done")
                    .expect("chunk should be ok");
                body.extend_from_slice(&chunk);
            }
        })
        .await
        .expect("final DONE should arrive");
        drop(body_stream);

        let candidates = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = request_candidate_repository
                    .list_by_request_id("req-image-done-close-success")
                    .await
                    .expect("request candidates should read");
                if candidates
                    .first()
                    .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Success)
                {
                    break candidates;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("candidate should be marked success");
        assert_eq!(candidates[0].status_code, Some(200));

        let stored_usage = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let usage = usage_repository
                    .find_by_request_id("req-image-done-close-success")
                    .await
                    .expect("usage should read");
                if usage
                    .as_ref()
                    .is_some_and(|usage| usage.status == "completed")
                {
                    break usage.expect("completed usage should exist");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be marked completed");
        assert_eq!(stored_usage.status_code, Some(200));
        assert!(stored_usage.total_tokens > 0);
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_records_first_data_as_streaming_before_terminal_telemetry(
    ) {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let first_data_seen = Arc::new(Notify::new());
        let release_terminal = Arc::new(Notify::new());
        let first_data_seen_for_route = Arc::clone(&first_data_seen);
        let release_terminal_for_route = Arc::clone(&release_terminal);
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(move |_request: Request| {
                    let first_data_seen = Arc::clone(&first_data_seen_for_route);
                    let release_terminal = Arc::clone(&release_terminal_for_route);
                    async move {
                        let frames = stream! {
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                            ));
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"event: response.output_text.delta\\ndata: {\\\"type\\\":\\\"response.output_text.delta\\\",\\\"delta\\\":\\\"hi\\\"}\\n\\n\"}}\n",
                            ));
                            first_data_seen.notify_one();
                            release_terminal.notified().await;
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"ttfb_ms\":123,\"elapsed_ms\":456}}}\n",
                            ));
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n",
                            ));
                        };
                        let mut response = axum::http::Response::new(Body::from_stream(frames));
                        response.headers_mut().insert(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("application/x-ndjson"),
                        );
                        response
                    }
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            })
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-live-stream-first-data".into(),
            candidate_id: Some("cand-live-stream-first-data".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "input": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:responses".into(),
            provider_api_format: "openai:responses".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("cli".to_string()),
            Some("openai:responses".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-live-stream-first-data",
            &decision,
            "openai_responses_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:responses",
                "client_api_format": "openai:responses",
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        first_data_seen.notified().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let live_usage = loop {
            let usage = usage_repository
                .find_by_request_id("req-live-stream-first-data")
                .await
                .expect("usage should read");
            if usage.as_ref().is_some_and(|usage| {
                usage.status == "streaming" && usage.first_byte_time_ms.is_some()
            }) {
                break usage.expect("live usage should exist");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "usage should record streaming status with first byte before terminal telemetry"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        assert_eq!(live_usage.status, "streaming");
        assert!(live_usage.first_byte_time_ms.is_some());

        release_terminal.notify_one();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let text = String::from_utf8(body.to_vec()).expect("response body should be utf8");
        assert!(text.contains("response.output_text.delta"));

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_records_first_stream_event_before_visible_text() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let first_event_seen = Arc::new(Notify::new());
        let release_text = Arc::new(Notify::new());
        let text_seen = Arc::new(Notify::new());
        let release_terminal = Arc::new(Notify::new());
        let first_event_seen_for_route = Arc::clone(&first_event_seen);
        let release_text_for_route = Arc::clone(&release_text);
        let text_seen_for_route = Arc::clone(&text_seen);
        let release_terminal_for_route = Arc::clone(&release_terminal);
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(move |_request: Request| {
                    let first_event_seen = Arc::clone(&first_event_seen_for_route);
                    let release_text = Arc::clone(&release_text_for_route);
                    let text_seen = Arc::clone(&text_seen_for_route);
                    let release_terminal = Arc::clone(&release_terminal_for_route);
                    async move {
                        let frames = stream! {
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                            ));
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"\"}}\n",
                            ));
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"ttfb_ms\":11,\"elapsed_ms\":12}}}\n",
                            ));
                            first_event_seen.notify_one();
                            release_text.notified().await;
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"choices\\\":[{\\\"delta\\\":{\\\"content\\\":\\\"hello\\\"}}]}\\n\\n\"}}\n",
                            ));
                            text_seen.notify_one();
                            release_terminal.notified().await;
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":50}}}\n",
                            ));
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n",
                            ));
                        };
                        let mut response = axum::http::Response::new(Body::from_stream(frames));
                        response.headers_mut().insert(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("application/x-ndjson"),
                        );
                        response
                    }
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            })
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-live-stream-first-event".into(),
            candidate_id: Some("cand-live-stream-first-event".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://api.openai.com/v1/chat/completions".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-live-stream-first-event",
            &decision,
            "openai_chat_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:chat",
                "client_api_format": "openai:chat",
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        first_event_seen.notified().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let first_event_usage = loop {
            let usage = usage_repository
                .find_by_request_id("req-live-stream-first-event")
                .await
                .expect("usage should read");
            if usage.as_ref().is_some_and(|usage| {
                usage.status == "streaming" && usage.first_byte_time_ms.is_some()
            }) {
                break usage.expect("streaming usage should exist");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "usage should record first byte on the first upstream stream event"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(first_event_usage.first_byte_time_ms.is_some());

        release_text.notify_one();
        text_seen.notified().await;

        release_terminal.notify_one();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let text = String::from_utf8(body.to_vec()).expect("response body should be utf8");
        assert!(text.contains("\"content\":\"hello\""));

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_bridges_sync_json_body_from_remote_runtime_to_sse() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(|_request: Request| async move {
                    let frames = concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"application/json\"}}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"{\\\"id\\\":\\\"resp-remote-runtime-sync-json-123\\\",\\\"object\\\":\\\"response\\\",\\\"model\\\":\\\"gpt-5.4\\\",\\\"status\\\":\\\"completed\\\",\\\"output\\\":[{\\\"type\\\":\\\"message\\\",\\\"id\\\":\\\"msg-remote-runtime-sync-json-123\\\",\\\"role\\\":\\\"assistant\\\",\\\"content\\\":[{\\\"type\\\":\\\"output_text\\\",\\\"text\\\":\\\"Hello from remote runtime sync json\\\",\\\"annotations\\\":[]}]}],\\\"usage\\\":{\\\"input_tokens\\\":1,\\\"output_tokens\\\":2,\\\"total_tokens\\\":3}}\"}}\n",
                        "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":41}}}\n",
                        "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                    );
                    let mut response = axum::http::Response::new(Body::from(frames));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/x-ndjson"),
                    );
                    response
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let state = AppState::new()
            .expect("app state should build")
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-remote-runtime-sync-json-stream".into(),
            candidate_id: Some("cand-remote-runtime-sync-json-stream".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "input": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:responses".into(),
            provider_api_format: "openai:responses".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("cli".to_string()),
            Some("openai:responses".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-remote-runtime-sync-json-stream",
            &decision,
            "openai_responses_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:responses",
                "client_api_format": "openai:responses",
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let text = String::from_utf8(body.to_vec()).expect("response body should be utf8");
        assert!(text.contains("event: response.output_text.delta"));
        assert!(text.contains("Hello from remote runtime sync json"));
        assert!(text.contains("event: response.completed"));

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_rewrites_redirect_to_structured_failure() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(|_request: Request| async move {
                    let frames = concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":302,\"headers\":{\"location\":\"/\",\"content-type\":\"text/html\",\"content-length\":\"0\"}}}\n",
                        "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                    );
                    let mut response = axum::http::Response::new(Body::from(frames));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/x-ndjson"),
                    );
                    response
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            })
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-remote-runtime-stream-redirect".into(),
            candidate_id: Some("cand-remote-runtime-stream-redirect".into()),
            provider_name: Some("ChatGPTWeb".into()),
            provider_id: "prov-redirect".into(),
            endpoint_id: "ep-redirect".into(),
            key_id: "key-redirect".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "input": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "gemini:generate_content".into(),
            provider_api_format: "openai:responses".into(),
            model_name: Some("gemini-3.1-flash-image-preview".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1beta/models/gemini-3.1-flash-image-preview:streamGenerateContent",
            Some("ai_public".to_string()),
            Some("gemini".to_string()),
            Some("generate_content".to_string()),
            Some("gemini:generate_content".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-remote-runtime-stream-redirect",
            &decision,
            "gemini_chat_stream",
            None,
            Some(json!({
                "request_id": "req-remote-runtime-stream-redirect",
                "candidate_id": "cand-remote-runtime-stream-redirect",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:responses",
                "client_api_format": "gemini:generate_content",
                "needs_conversion": true
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(
            response
                .headers()
                .get("x-aether-upstream-status")
                .and_then(|value| value.to_str().ok()),
            Some("302")
        );
        assert!(
            response.headers().get(header::LOCATION).is_none(),
            "redirect location should not be forwarded to AI clients"
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_json: Value =
            serde_json::from_slice(&body).expect("response body should decode as json");
        assert_eq!(
            body_json["error"]["type"],
            json!("execution_runtime_non_success_status")
        );
        assert_eq!(body_json["error"]["upstream_status"], json!(302));
        assert_eq!(body_json["error"]["location"], json!("/"));
        assert!(body_json["error"]["message"]
            .as_str()
            .is_some_and(|value| value.contains("non-success status 302")));

        let usage = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(usage) = usage_repository
                    .find_by_request_id("req-remote-runtime-stream-redirect")
                    .await
                    .expect("usage should read")
                    .filter(|usage| usage.status == "failed")
                {
                    break usage;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be written");

        assert_eq!(usage.status_code, Some(302));
        assert_eq!(usage.error_category.as_deref(), Some("redirect"));
        assert!(usage
            .error_message
            .as_deref()
            .is_some_and(|value| value.contains("non-success status 302")));
        assert_eq!(
            usage
                .client_response_headers
                .as_ref()
                .and_then(|headers| headers.get("x-aether-upstream-status")),
            Some(&json!("302"))
        );
        assert_eq!(
            usage
                .response_headers
                .as_ref()
                .and_then(|headers| headers.get("location")),
            Some(&json!("/"))
        );
        assert!(
            usage.response_body.is_none(),
            "upstream redirect did not include a body"
        );
        assert_eq!(
            usage
                .client_response_body
                .as_ref()
                .and_then(|body| body.pointer("/error/upstream_status")),
            Some(&json!(302))
        );
        let candidates = request_candidate_repository
            .list_by_request_id("req-remote-runtime-stream-redirect")
            .await
            .expect("candidate trace should read");
        let candidate_extra = candidates
            .first()
            .and_then(|candidate| candidate.extra_data.as_ref())
            .expect("failed candidate extra_data should exist");
        assert_eq!(
            candidate_extra["upstream_response"]["status_code"],
            json!(302)
        );
        assert_eq!(
            candidate_extra["upstream_response"]["headers"]["location"],
            json!("/")
        );
        assert!(candidate_extra["upstream_response"].get("body").is_none());
        assert!(candidate_extra.get("client_response").is_none());

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_bridges_openai_image_sync_json_from_remote_runtime_to_image_sse(
    ) {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(|_request: Request| async move {
                    let frames = concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"application/json\"}}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"{\\\"created\\\":1776972364,\\\"data\\\":[{\\\"b64_json\\\":\\\"aGVsbG8=\\\"}],\\\"usage\\\":{\\\"total_tokens\\\":100,\\\"input_tokens\\\":50,\\\"output_tokens\\\":50,\\\"input_tokens_details\\\":{\\\"text_tokens\\\":10,\\\"image_tokens\\\":40}}}\"}}\n",
                        "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":41}}}\n",
                        "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                    );
                    let mut response = axum::http::Response::new(Body::from(frames));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/x-ndjson"),
                    );
                    response
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let state = AppState::new()
            .expect("app state should build")
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-remote-runtime-image-sync-json-stream".into(),
            candidate_id: Some("cand-remote-runtime-image-sync-json-stream".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-image-1",
                "prompt": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:image".into(),
            provider_api_format: "openai:image".into(),
            model_name: Some("gpt-image-1".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/images/generations",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("image".to_string()),
            Some("openai:image".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-remote-runtime-image-sync-json-stream",
            &decision,
            "openai_image_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:image",
                "client_api_format": "openai:image",
                "mapped_model": "gpt-image-1",
                "image_request": {
                    "operation": "generate"
                }
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let text = String::from_utf8(body.to_vec()).expect("response body should be utf8");
        assert!(text.contains("event: image_generation.completed"));
        assert!(text.contains("\"type\":\"image_generation.completed\""));
        assert!(text.contains("\"b64_json\":\"aGVsbG8=\""));
        assert!(text.contains("\"total_tokens\":100"));

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_returns_client_error_with_local_tunnel_message_before_first_data(
    ) {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            901,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        )));

        let plan = ExecutionPlan {
            request_id: "req-client-stream-error-1".into(),
            candidate_id: Some("cand-client-stream-error-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = test_decision();

        let state_for_task = state.clone();
        let plan_for_task = plan.clone();
        let decision_for_task = decision.clone();
        let execution_task = tokio::spawn(async move {
            execute_execution_runtime_stream(
                &state_for_task,
                plan_for_task,
                "trace-local-stream-client-error",
                &decision_for_task,
                "openai_chat_stream",
                None,
                Some(json!({
                    "client_api_format": "openai:chat",
                    "provider_api_format": "openai:chat",
                })),
            )
            .await
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header = tunnel_protocol::FrameHeader::parse(&request_body)
            .expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, tunnel_protocol::REQUEST_BODY);

        let response_meta = tunnel_protocol::ResponseMeta {
            status: 200,
            // Use a non-SSE content type so direct finalize prefetch stays enabled and the
            // pre-body tunnel error is surfaced as a client-visible structured error response.
            headers: vec![("content-type".to_string(), "application/json".to_string())],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(901, &mut response_headers_frame)
            .await;

        let original_error = "proxy disconnected before first upstream event";
        let mut response_error_frame =
            tunnel_protocol::encode_stream_error(request_header.stream_id, original_error);
        tunnel_app
            .hub
            .handle_proxy_frame(901, &mut response_error_frame)
            .await;

        let response = execution_task
            .await
            .expect("execution task should complete")
            .expect("execution should succeed")
            .expect("execution should return a client response");

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_json: Value =
            serde_json::from_slice(&body).expect("response body should decode as json");

        let error_message = body_json
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .expect("response body should contain error.message");

        assert_eq!(error_message, original_error);
        assert!(
            !error_message.contains("unexpected EOF during chunk size line"),
            "client-facing response should preserve the original local tunnel error"
        );
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_emits_terminal_sse_error_event_after_body_started() {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            902,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        )));

        let plan = ExecutionPlan {
            request_id: "req-client-stream-sse-error-1".into(),
            candidate_id: Some("cand-client-stream-sse-error-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = test_decision();

        let state_for_task = state.clone();
        let plan_for_task = plan.clone();
        let decision_for_task = decision.clone();
        let execution_task = tokio::spawn(async move {
            execute_execution_runtime_stream(
                &state_for_task,
                plan_for_task,
                "trace-local-stream-sse-error",
                &decision_for_task,
                "openai_chat_stream",
                None,
                Some(json!({
                    "client_api_format": "openai:chat",
                    "provider_api_format": "openai:chat",
                })),
            )
            .await
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header = tunnel_protocol::FrameHeader::parse(&request_body)
            .expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, tunnel_protocol::REQUEST_BODY);

        let response_meta = tunnel_protocol::ResponseMeta {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(902, &mut response_headers_frame)
            .await;

        let mut response_body_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_BODY,
            0,
            b"data: hello\n\n",
        );
        tunnel_app
            .hub
            .handle_proxy_frame(902, &mut response_body_frame)
            .await;

        let response = execution_task
            .await
            .expect("execution task should complete")
            .expect("execution should succeed")
            .expect("execution should return a client response");
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );

        let body_task = tokio::spawn(async move {
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response body should read");
            String::from_utf8(body.to_vec()).expect("response body should be utf8")
        });

        let original_error = "proxy disconnected while forwarding upstream body";
        let mut response_error_frame =
            tunnel_protocol::encode_stream_error(request_header.stream_id, original_error);
        tunnel_app
            .hub
            .handle_proxy_frame(902, &mut response_error_frame)
            .await;

        let body = body_task.await.expect("body task should complete");
        assert!(body.contains("data: hello\n\n"));
        assert!(body.contains("data: {\"error\":"));
        assert!(body.contains(original_error));
        assert!(body.contains("data: [DONE]\n\n"));
        assert!(
            !body.contains("unexpected EOF during chunk size line"),
            "same-format SSE path should surface the original terminal error event"
        );
    }
}
