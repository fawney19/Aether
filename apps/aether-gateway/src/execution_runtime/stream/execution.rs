use std::collections::{BTreeMap, VecDeque};
use std::io::Error as IoError;
use std::time::Instant;

use aether_contracts::{
    ExecutionPlan, ExecutionStreamTerminalSummary, ExecutionTelemetry, StreamFrame,
    StreamFramePayload,
};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use aether_scheduler_core::{
    parse_request_candidate_report_context, SchedulerRequestCandidateStatusUpdate,
};
use aether_usage_runtime::{
    build_lifecycle_usage_seed, build_stream_terminal_usage_payload_seed,
    build_sync_terminal_usage_payload_seed, build_terminal_usage_context_seed,
    UsageBodyCapturePolicy, UsageRequestRecordLevel, UsageRuntimeAccess,
    DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
};
use async_stream::stream;
use axum::body::{Body, Bytes};
use axum::http::Response;
use base64::Engine as _;
use futures_util::stream::BoxStream;
use futures_util::{StreamExt, TryStreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;
use tracing::{debug, info, warn};

use super::error::{
    build_execution_runtime_error_response, collect_error_body, decode_stream_error_body,
    inspect_prefetched_stream_body, read_next_frame, StreamPrefetchInspection,
};
#[path = "execution_failures.rs"]
mod execution_failures;
use self::execution_failures::{
    build_stream_failure_from_execution_error, build_stream_failure_report,
    handle_prefetch_stream_failure, submit_midstream_stream_failure, StreamFailureReport,
};
use crate::ai_serving::api::{
    maybe_bridge_standard_sync_json_to_stream, maybe_build_provider_private_stream_normalizer,
    maybe_build_stream_response_rewriter, normalize_provider_private_report_context,
    StreamingStandardTerminalObserver,
};
use crate::api::response::{
    attach_control_metadata_headers, build_client_response, build_client_response_from_parts,
};
use crate::clock::current_unix_ms as current_request_candidate_unix_ms;
use crate::constants::{CONTROL_CANDIDATE_ID_HEADER, CONTROL_REQUEST_ID_HEADER};
use crate::control::GatewayControlDecision;
use crate::execution_runtime::build_direct_execution_frame_stream;
use crate::execution_runtime::kiro_web_search::maybe_execute_kiro_web_search_stream;
use crate::execution_runtime::oauth_retry::refresh_oauth_plan_auth_for_retry;
#[cfg(test)]
use crate::execution_runtime::remote_compat::post_stream_plan_to_remote_execution_runtime;
use crate::execution_runtime::submission::{
    resolve_core_error_background_report_kind, strip_utf8_bom_and_ws,
    submit_local_core_error_or_sync_finalize,
};
use crate::execution_runtime::transport::{
    execute_stream_plan_via_local_tunnel, DirectSyncExecutionRuntime,
    DirectUpstreamStreamExecution, ExecutionRuntimeTransportError,
};
use crate::execution_runtime::{
    local_failover_response_text, resolve_core_stream_direct_finalize_report_kind,
    resolve_core_stream_error_finalize_report_kind,
    resolve_local_candidate_failover_analysis_stream, should_fallback_to_control_stream,
    should_retry_next_local_candidate_stream, LocalFailoverDecision,
};
use crate::execution_runtime::{MAX_STREAM_PREFETCH_BYTES, MAX_STREAM_PREFETCH_FRAMES};
use crate::log_ids::short_request_id;
use crate::orchestration::{
    apply_local_execution_effect, build_local_error_flow_metadata, with_error_flow_report_context,
    LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect, LocalAttemptFailureEffect,
    LocalExecutionEffect, LocalExecutionEffectContext, LocalHealthFailureEffect,
    LocalHealthSuccessEffect, LocalOAuthInvalidationEffect, LocalPoolErrorEffect,
};
use crate::request_candidate_runtime::{
    ensure_execution_request_candidate_slot, record_local_request_candidate_status,
    record_local_request_candidate_status_snapshot, snapshot_local_request_candidate_status,
};
use crate::usage::submit_stream_report;
use crate::usage::{GatewayStreamReportRequest, GatewaySyncReportRequest};
use crate::{AppState, GatewayError};

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

fn record_stream_terminal_usage(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewayStreamReportRequest,
    cancelled: bool,
) {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_stream_terminal_usage_payload_seed(payload);
    state.usage_runtime.record_stream_terminal(
        state.data.as_ref(),
        context_seed,
        payload_seed,
        cancelled,
    );
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
    if chunk.is_empty() {
        return;
    }

    buffered.extend_from_slice(chunk);
    while let Some(line_end) = buffered.iter().position(|byte| *byte == b'\n') {
        let line = buffered.drain(..=line_end).collect::<Vec<_>>();
        if let Err(err) = observer.push_line(report_context, line) {
            observer.disable_with_error(err.to_string());
            buffered.clear();
            break;
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

async fn execute_in_process_stream(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<DirectUpstreamStreamExecution, ExecutionRuntimeTransportError> {
    if let Some(execution) = execute_stream_plan_via_local_tunnel(state, plan).await? {
        return Ok(execution);
    }

    DirectSyncExecutionRuntime::new().execute_stream(plan).await
}

async fn execute_in_process_stream_with_oauth_retry(
    state: &AppState,
    plan: &mut ExecutionPlan,
    trace_id: &str,
    report_context: Option<&Value>,
) -> Result<DirectUpstreamStreamExecution, ExecutionRuntimeTransportError> {
    let mut execution = execute_in_process_stream(state, plan).await?;
    apply_stream_summary_report_context(&mut execution, report_context);
    if execution.status_code >= 400
        && refresh_oauth_plan_auth_for_retry(state, plan, execution.status_code, None, trace_id)
            .await
    {
        execution = execute_in_process_stream(state, plan).await?;
        apply_stream_summary_report_context(&mut execution, report_context);
    }
    Ok(execution)
}

#[allow(clippy::too_many_arguments)] // internal function, grouping would add unnecessary indirection
pub(crate) async fn execute_execution_runtime_stream(
    state: &AppState,
    mut plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    mut report_context: Option<serde_json::Value>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let stream_started_at = Instant::now();
    ensure_execution_request_candidate_slot(state, &mut plan, &mut report_context).await;
    let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
    let request_candidate_status_snapshot =
        snapshot_local_request_candidate_status(&plan, report_context.as_ref());
    state
        .usage_runtime
        .record_pending(state.data.as_ref(), lifecycle_seed.clone());
    let candidate_started_unix_secs = current_request_candidate_unix_ms();
    if let Some(snapshot) = request_candidate_status_snapshot.clone() {
        let state_bg = state.clone();
        tokio::spawn(async move {
            record_local_request_candidate_status_snapshot(
                &state_bg,
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
        });
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
    match maybe_execute_kiro_web_search_stream(state, &plan, report_context.as_ref()).await {
        Ok(Some(kiro_web_search)) => {
            return execute_stream_from_frame_stream(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                kiro_web_search.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                kiro_web_search.frame_stream,
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
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
    #[cfg(not(test))]
    {
        let execution = match execute_in_process_stream_with_oauth_retry(
            state,
            &mut plan,
            trace_id,
            report_context.as_ref(),
        )
        .await
        {
            Ok(execution) => execution,
            Err(err) => {
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
                        error_message: Some(format!("{err:?}")),
                        latency_ms: None,
                        started_at_unix_ms: Some(candidate_started_unix_secs),
                        finished_at_unix_ms: Some(terminal_unix_secs),
                    },
                )
                .await;
                return Ok(None);
            }
        };
        let frame_stream = build_direct_execution_frame_stream(execution).boxed();
        return execute_stream_from_frame_stream(
            state,
            plan,
            trace_id,
            decision,
            plan_kind,
            report_kind,
            report_context,
            candidate_started_unix_secs,
            stream_started_at,
            frame_stream,
        )
        .await;
    }
    #[cfg(test)]
    {
        let remote_execution_runtime_base_url = state
            .execution_runtime_override_base_url()
            .unwrap_or_default();
        if remote_execution_runtime_base_url.trim().is_empty() {
            let execution = match execute_in_process_stream_with_oauth_retry(
                state,
                &mut plan,
                trace_id,
                report_context.as_ref(),
            )
            .await
            {
                Ok(execution) => execution,
                Err(err) => {
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
                            error_message: Some(err.to_string()),
                            latency_ms: None,
                            started_at_unix_ms: Some(candidate_started_unix_secs),
                            finished_at_unix_ms: Some(terminal_unix_secs),
                        },
                    )
                    .await;
                    return Ok(None);
                }
            };
            let frame_stream = build_direct_execution_frame_stream(execution).boxed();
            return execute_stream_from_frame_stream(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                report_context,
                candidate_started_unix_secs,
                stream_started_at,
                frame_stream,
            )
            .await;
        }

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
                        error_message: Some(format!("{err:?}")),
                        latency_ms: None,
                        started_at_unix_ms: Some(candidate_started_unix_secs),
                        finished_at_unix_ms: Some(terminal_unix_secs),
                    },
                )
                .await;
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

        let frame_stream = response
            .bytes_stream()
            .map_err(|err| IoError::other(err.to_string()))
            .boxed();
        return execute_stream_from_frame_stream(
            state,
            plan,
            trace_id,
            decision,
            plan_kind,
            report_kind,
            report_context,
            candidate_started_unix_secs,
            stream_started_at,
            frame_stream,
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

fn encode_terminal_sse_error_event(failure: &StreamFailureReport) -> Result<Bytes, std::io::Error> {
    let payload = failure
        .to_json_string()
        .map_err(|err| IoError::other(err.to_string()))?;
    let mut event = String::from("event: aether.error\n");
    for line in payload.lines() {
        event.push_str("data: ");
        event.push_str(line);
        event.push('\n');
    }
    event.push('\n');
    Ok(Bytes::from(event))
}

async fn next_stream_frame<R>(
    buffered_frames: &mut VecDeque<StreamFrame>,
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Option<StreamFrame>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    if let Some(frame) = buffered_frames.pop_front() {
        return Ok(Some(frame));
    }
    read_next_frame(lines).await
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

fn should_skip_direct_finalize_prefetch(
    direct_stream_finalize_kind: Option<&str>,
    content_type: Option<&str>,
    provider_api_format: &str,
    client_api_format: &str,
    has_private_stream_normalizer: bool,
    has_local_stream_rewriter: bool,
) -> bool {
    if direct_stream_finalize_kind.is_none() {
        return false;
    }

    let content_type = content_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.contains("text/event-stream") {
        return true;
    }

    if has_private_stream_normalizer || has_local_stream_rewriter {
        return false;
    }

    if !provider_api_format.eq_ignore_ascii_case(client_api_format) {
        return false;
    }

    if content_type.is_empty() {
        return true;
    }

    !(content_type.contains("json") || content_type.ends_with("+json"))
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
    buffered_frames: &mut VecDeque<StreamFrame>,
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Option<String>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    while let Some(frame) = read_next_frame(lines).await? {
        let probe_text = match &frame.payload {
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
        buffered_frames.push_back(frame);
        if probe_text.is_some() {
            return Ok(probe_text);
        }
    }

    Ok(None)
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
    frame_stream: BoxStream<'static, Result<Bytes, IoError>>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let request_id = plan.request_id.as_str();
    let request_id_for_log = short_request_id(request_id);
    let candidate_id = plan.candidate_id.as_deref();
    let provider_name = plan.provider_name.as_deref().unwrap_or("-");
    let model_name = plan.model_name.as_deref().unwrap_or("-");
    let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
    let request_candidate_status_snapshot =
        snapshot_local_request_candidate_status(&plan, report_context.as_ref());
    let candidate_index = parse_request_candidate_report_context(report_context.as_ref())
        .and_then(|context| context.candidate_index)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let reader = StreamReader::new(frame_stream);
    let mut lines = FramedRead::new(reader, LinesCodec::new());

    let first_frame = read_next_frame(&mut lines).await?.ok_or_else(|| {
        GatewayError::Internal("execution runtime stream ended before headers frame".to_string())
    })?;
    let StreamFramePayload::Headers {
        status_code,
        mut headers,
    } = first_frame.payload
    else {
        return Err(GatewayError::Internal(
            "execution runtime stream must start with headers frame".to_string(),
        ));
    };
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

    if status_code >= 400 {
        let error_body = collect_error_body(&mut lines).await?;
        let (body_json, body_base64) = decode_stream_error_body(&headers, &error_body);
        let error_response_text =
            local_failover_response_text(body_json.as_ref(), &error_body, None);
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
            let terminal_unix_secs = current_request_candidate_unix_ms();
            let error_flow_report_context = with_error_flow_report_context(
                report_context.as_ref(),
                build_local_error_flow_metadata(
                    status_code,
                    error_response_text.as_deref(),
                    failover_analysis,
                ),
            );
            record_local_request_candidate_status(
                state,
                &plan,
                error_flow_report_context
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
            let error_flow_report_context = with_error_flow_report_context(
                report_context.as_ref(),
                build_local_error_flow_metadata(
                    status_code,
                    error_response_text.as_deref(),
                    failover_analysis,
                ),
            );
            record_local_request_candidate_status(
                state,
                &plan,
                error_flow_report_context
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

        let payload = build_stream_sync_payload(
            trace_id,
            stream_error_finalize_kind
                .as_deref()
                .or(report_kind.as_deref())
                .unwrap_or_default()
                .to_string(),
            report_context,
            status_code,
            headers,
            body_json,
            body_base64,
            None,
        );
        record_sync_terminal_usage(state, &plan, payload.report_context.as_ref(), &payload);
        let terminal_unix_secs = current_request_candidate_unix_ms();
        let error_flow_report_context = with_error_flow_report_context(
            payload.report_context.as_ref(),
            build_local_error_flow_metadata(
                status_code,
                error_response_text.as_deref(),
                failover_analysis,
            ),
        );
        record_local_request_candidate_status(
            state,
            &plan,
            error_flow_report_context
                .as_ref()
                .or(payload.report_context.as_ref()),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(status_code),
                error_type: Some("execution_runtime_stream_error".to_string()),
                error_message: Some(format!(
                    "execution runtime stream returned error status {status_code}"
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
            build_execution_runtime_error_response(
                trace_id,
                decision,
                plan_kind,
                status_code,
                payload.headers,
                error_body,
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
    let skip_direct_finalize_prefetch = should_skip_direct_finalize_prefetch(
        direct_stream_finalize_kind.as_deref(),
        upstream_content_type,
        plan.provider_api_format.as_str(),
        plan.client_api_format.as_str(),
        private_stream_normalizer.is_some(),
        local_stream_rewriter.is_some(),
    );
    let mut prefetched_chunks: Vec<Bytes> = Vec::new();
    let mut provider_prefetched_body = Vec::new();
    let mut prefetched_body = Vec::new();
    let mut prefetched_inspection_body = Vec::new();
    let mut prefetched_telemetry: Option<ExecutionTelemetry> = None;
    let mut reached_eof = false;
    let mut sync_json_stream_bridge_active = false;
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
        while prefetched_chunks.len() < MAX_STREAM_PREFETCH_FRAMES
            && prefetched_inspection_body.len() < MAX_STREAM_PREFETCH_BYTES
        {
            let Some(frame) = (match next_stream_frame(&mut buffered_frames, &mut lines).await {
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
                        prefetched_telemetry,
                        &provider_prefetched_body,
                        failure,
                    )
                    .await;
                }
            }) else {
                reached_eof = true;
                break;
            };
            match frame.payload {
                StreamFramePayload::Data { chunk_b64, text } => {
                    let chunk =
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
                                    prefetched_telemetry,
                                    &prefetched_body,
                                    failure,
                                )
                                .await;
                            }
                        };

                    if chunk.is_empty() {
                        continue;
                    }

                    provider_prefetched_body.extend_from_slice(&chunk);
                    prefetched_inspection_body.extend_from_slice(&chunk);

                    let inspection = inspect_prefetched_stream_body(
                        &upstream_headers,
                        &prefetched_inspection_body,
                    );
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
                            let payload = build_stream_sync_payload(
                                trace_id,
                                report_kind.clone(),
                                report_context,
                                status_code,
                                headers,
                                Some(body_json),
                                None,
                                prefetched_telemetry,
                            );
                            record_sync_terminal_usage(
                                state,
                                &plan,
                                payload.report_context.as_ref(),
                                &payload,
                            );
                            let response = submit_local_core_error_or_sync_finalize(
                                state, trace_id, decision, payload,
                            )
                            .await?;
                            return Ok(Some(attach_control_metadata_headers(
                                response,
                                Some(request_id),
                                candidate_id,
                            )?));
                        }
                        StreamPrefetchInspection::NeedMore => {}
                        StreamPrefetchInspection::NonError => {}
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
                                        prefetched_telemetry,
                                        &provider_prefetched_body,
                                        failure,
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
                                    prefetched_telemetry,
                                    &provider_prefetched_body,
                                    failure,
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
                                    prefetched_telemetry,
                                    &provider_prefetched_body,
                                    failure,
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

                    if matches!(inspection, StreamPrefetchInspection::NonError) {
                        break;
                    }
                }
                StreamFramePayload::Telemetry {
                    telemetry: frame_telemetry,
                } => {
                    prefetched_telemetry = Some(frame_telemetry);
                }
                StreamFramePayload::Eof { summary } => {
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
                        prefetched_telemetry,
                        &provider_prefetched_body,
                        build_stream_failure_from_execution_error(&error),
                    )
                    .await;
                }
                StreamFramePayload::Headers { .. } => {}
            }
        }
    }
    drop(private_stream_normalizer);
    drop(local_stream_rewriter);

    state.usage_runtime.record_stream_started(
        state.data.as_ref(),
        &lifecycle_seed,
        status_code,
        prefetched_telemetry.as_ref(),
    );
    if let Some(snapshot) = request_candidate_status_snapshot {
        let state_bg = state.clone();
        let latency_ms = prefetched_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.elapsed_ms);
        tokio::spawn(async move {
            record_local_request_candidate_status_snapshot(
                &state_bg,
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
        });
    }

    let request_id = request_id.to_string();
    let candidate_id = candidate_id.map(ToOwned::to_owned);
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, IoError>>(16);
    let state_for_report = state.clone();
    let plan_for_report = plan;
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
    let emit_passthrough_sse_terminal_error =
        skip_direct_finalize_prefetch && response_headers_indicate_sse(&upstream_headers);
    let body_capture_policy = match UsageRuntimeAccess::body_capture_policy(state.data.as_ref())
        .await
    {
        Ok(policy) => policy,
        Err(err) => {
            warn!(
                event_name = "stream_execution_body_capture_policy_read_failed",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %request_id_for_report_log,
                candidate_id = ?candidate_id_for_report.as_deref(),
                error = %err,
                fallback_request_body_bytes = DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
                "gateway failed to read body capture policy; falling back to default stream capture limits"
            );
            UsageBodyCapturePolicy::default()
        }
    };
    let max_stream_body_buffer_bytes = if matches!(
        body_capture_policy.record_level,
        UsageRequestRecordLevel::Basic
    ) {
        0
    } else {
        body_capture_policy
            .max_response_body_bytes
            .unwrap_or(DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES)
    };
    tokio::spawn(async move {
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
        let mut usage_stream_telemetry: Option<ExecutionTelemetry> = initial_telemetry.clone();
        let mut telemetry: Option<ExecutionTelemetry> = initial_telemetry;
        let reached_eof = initial_reached_eof;
        let mut downstream_dropped = false;
        let mut terminal_failure: Option<StreamFailureReport> = None;
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
        }

        if terminal_failure.is_none() && !reached_eof {
            loop {
                let next_frame = match next_stream_frame(&mut buffered_frames, &mut lines).await {
                    Ok(frame) => frame,
                    Err(err) => {
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
                let Some(frame) = next_frame else {
                    break;
                };
                match frame.payload {
                    StreamFramePayload::Data { chunk_b64, text } => {
                        if sync_json_stream_bridge_active_for_report {
                            continue;
                        }
                        let chunk =
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

                        if rewritten_chunk.is_empty() {
                            continue;
                        }

                        if usage_stream_telemetry
                            .as_ref()
                            .and_then(|telemetry| telemetry.ttfb_ms)
                            .is_none()
                        {
                            let first_data_elapsed_ms = stream_started_at
                                .elapsed()
                                .as_millis()
                                .min(u128::from(u64::MAX))
                                as u64;
                            let first_data_telemetry = ExecutionTelemetry {
                                ttfb_ms: Some(first_data_elapsed_ms),
                                elapsed_ms: Some(first_data_elapsed_ms),
                                upstream_bytes: telemetry
                                    .as_ref()
                                    .and_then(|telemetry| telemetry.upstream_bytes),
                            };
                            state_for_report.usage_runtime.record_stream_started(
                                state_for_report.data.as_ref(),
                                &lifecycle_seed_for_report,
                                status_code,
                                Some(&first_data_telemetry),
                            );
                            usage_stream_telemetry = Some(first_data_telemetry);
                        }

                        append_stream_capture_bytes(
                            &mut buffered_body,
                            &rewritten_chunk,
                            max_stream_body_buffer_bytes,
                            &mut client_body_truncated,
                        );
                        if tx.send(Ok(Bytes::from(rewritten_chunk))).await.is_err() {
                            warn!(
                                event_name = "stream_execution_downstream_disconnected",
                                log_type = "ops",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                "gateway stream downstream dropped; stopping execution runtime stream forwarding"
                            );
                            downstream_dropped = true;
                            break;
                        }
                    }
                    StreamFramePayload::Telemetry {
                        telemetry: frame_telemetry,
                    } => {
                        let should_refresh_stream_usage = should_refresh_stream_usage_telemetry(
                            usage_stream_telemetry.as_ref(),
                            &frame_telemetry,
                        );
                        if should_refresh_stream_usage {
                            state_for_report.usage_runtime.record_stream_started(
                                state_for_report.data.as_ref(),
                                &lifecycle_seed_for_report,
                                status_code,
                                Some(&frame_telemetry),
                            );
                            usage_stream_telemetry = Some(frame_telemetry.clone());
                        }
                        telemetry = Some(frame_telemetry);
                    }
                    StreamFramePayload::Eof { summary } => {
                        if summary.is_some() {
                            stream_terminal_summary = summary;
                        }
                        break;
                    }
                    StreamFramePayload::Error { error } => {
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

        if downstream_dropped {
            debug!(
                event_name = "execution_runtime_stream_flush_skipped",
                log_type = "debug",
                debug_context = "redacted",
                stream_status = "downstream_disconnected",
                trace_id = %trace_id_owned,
                "gateway skipped local stream flush after downstream disconnect"
            );
        } else {
            if let Some(normalizer) = private_stream_normalizer.as_mut() {
                match normalizer.finish() {
                    Ok(normalized_chunk) if !normalized_chunk.is_empty() => {
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
                                        event_name = "stream_execution_normalized_flush_rewrite_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to rewrite normalized private stream chunk during flush"
                                    );
                                    terminal_failure.get_or_insert_with(|| {
                                        build_stream_failure_report(
                                            "execution_runtime_stream_rewrite_flush_error",
                                            format!("failed to rewrite normalized private stream chunk during flush: {err:?}"),
                                            502,
                                        )
                                    });
                                    Vec::new()
                                }
                            }
                        } else {
                            normalized_chunk
                        };
                        if !rewritten_chunk.is_empty() {
                            append_stream_capture_bytes(
                                &mut buffered_body,
                                &rewritten_chunk,
                                max_stream_body_buffer_bytes,
                                &mut client_body_truncated,
                            );
                            if tx.send(Ok(Bytes::from(rewritten_chunk))).await.is_err() {
                                warn!(
                                    event_name = "stream_execution_downstream_flush_disconnected",
                                    log_type = "ops",
                                    trace_id = %trace_id_owned,
                                    request_id = %request_id_for_report_log,
                                    candidate_id = ?candidate_id_for_report.as_deref(),
                                    "gateway stream downstream dropped while flushing private stream normalization"
                                );
                                downstream_dropped = true;
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
            if !downstream_dropped {
                if let Some(rewriter) = local_stream_rewriter.as_mut() {
                    match rewriter.finish() {
                        Ok(flushed_chunk) if !flushed_chunk.is_empty() => {
                            append_stream_capture_bytes(
                                &mut buffered_body,
                                &flushed_chunk,
                                max_stream_body_buffer_bytes,
                                &mut client_body_truncated,
                            );
                            if tx.send(Ok(Bytes::from(flushed_chunk))).await.is_err() {
                                warn!(
                                    event_name = "stream_execution_downstream_rewrite_flush_disconnected",
                                    log_type = "ops",
                                    trace_id = %trace_id_owned,
                                    request_id = %request_id_for_report_log,
                                    candidate_id = ?candidate_id_for_report.as_deref(),
                                    "gateway stream downstream dropped while flushing local stream rewrite"
                                );
                                downstream_dropped = true;
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
        }

        if !downstream_dropped && emit_passthrough_sse_terminal_error {
            if let Some(failure) = terminal_failure.as_ref() {
                match encode_terminal_sse_error_event(failure) {
                    Ok(error_event) => {
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

        drop(tx);

        stream_terminal_summary = merge_stream_terminal_summary(
            stream_terminal_summary,
            finalize_stream_usage_observer(
                &mut stream_usage_observer,
                stream_usage_report_context.as_ref(),
                &mut stream_usage_observer_buffered,
            ),
        );

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
            let usage_payload = build_stream_usage_payload(
                trace_id_owned,
                report_kind_owned.unwrap_or_default(),
                report_context_owned,
                499,
                headers_for_report,
                &provider_buffered_body,
                provider_body_truncated,
                &buffered_body,
                client_body_truncated,
                stream_terminal_summary,
                telemetry,
            );
            record_stream_terminal_usage(
                &state_for_report,
                &plan_for_report,
                usage_payload.report_context.as_ref(),
                &usage_payload,
                true,
            );
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
            submit_midstream_stream_failure(
                &state_for_report,
                &trace_id_owned,
                &plan_for_report,
                direct_stream_finalize_kind_owned.as_deref(),
                report_context_owned,
                headers_for_report,
                telemetry,
                &provider_buffered_body,
                candidate_started_unix_secs_for_report,
                failure,
            )
            .await;
            return;
        }

        let should_submit_report = report_kind_owned.is_some();
        let usage_payload = build_stream_usage_payload(
            trace_id_owned.clone(),
            report_kind_owned.unwrap_or_default(),
            report_context_owned,
            status_code,
            headers_for_report,
            &provider_buffered_body,
            provider_body_truncated,
            &buffered_body,
            client_body_truncated,
            stream_terminal_summary,
            telemetry,
        );
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
        record_stream_terminal_usage(
            &state_for_report,
            &plan_for_report,
            usage_payload.report_context.as_ref(),
            &usage_payload,
            false,
        );
        record_local_request_candidate_status(
            &state_for_report,
            &plan_for_report,
            usage_payload.report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Success,
                status_code: Some(status_code),
                error_type: None,
                error_message: None,
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

    let body_stream = stream! {
        for chunk in prefetched_chunks_for_body {
            yield Ok(chunk);
        }
        while let Some(item) = rx.recv().await {
            yield item;
        }
    };

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
    use std::sync::Arc;
    use std::time::Duration;

    use aether_contracts::{
        ExecutionPlan, ExecutionStreamTerminalSummary, ExecutionTimeouts, RequestBody,
        StandardizedUsage,
    };
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::usage::UsageReadRepository;
    use aether_usage_runtime::UsageRuntimeConfig;
    use async_stream::stream;
    use axum::body::{to_bytes, Body, Bytes};
    use axum::extract::ws::Message;
    use axum::extract::Request;
    use axum::routing::any;
    use axum::{http::header, http::HeaderValue, Router};
    use serde_json::{json, Value};
    use tokio::sync::{watch, Notify};

    use super::{
        execute_execution_runtime_stream, merge_stream_terminal_summary,
        should_probe_success_failover_before_stream, should_skip_direct_finalize_prefetch,
    };
    use crate::control::GatewayControlDecision;
    use crate::tunnel::{tunnel_protocol, TunnelProxyConn};
    use crate::AppState;

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
                unknown_event_count: 1,
                ..ExecutionStreamTerminalSummary::default()
            }),
            Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(observed_usage),
                response_id: Some("resp_123".to_string()),
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
        assert!(merged.observed_finish);
        assert_eq!(merged.unknown_event_count, 3);
    }

    #[test]
    fn skips_prefetch_for_same_format_passthrough_event_streams() {
        assert!(should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            Some("text/event-stream"),
            "claude:messages",
            "claude:messages",
            false,
            false,
        ));
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
        let state = AppState::new()
            .expect("app state should build")
            .with_usage_data_repository_for_tests(Arc::clone(&usage_repository))
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
            tls_profile: None,
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
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
            tls_profile: None,
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
            tls_profile: None,
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
            tls_profile: None,
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
            tls_profile: None,
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
        assert!(body.contains("event: aether.error\n"));
        assert!(body.contains(original_error));
        assert!(
            !body.contains("unexpected EOF during chunk size line"),
            "same-format SSE path should surface the original terminal error event"
        );
    }
}
