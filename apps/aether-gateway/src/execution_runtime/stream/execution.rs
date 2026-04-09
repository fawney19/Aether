use std::io::Error as IoError;

use aether_contracts::{ExecutionPlan, ExecutionTelemetry, StreamFramePayload};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::SchedulerRequestCandidateStatusUpdate;
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
use crate::ai_pipeline_api::{
    maybe_build_provider_private_stream_normalizer, maybe_build_stream_response_rewriter,
    normalize_provider_private_report_context,
};
use crate::api::response::{
    attach_control_metadata_headers, build_client_response, build_client_response_from_parts,
};
use crate::clock::current_unix_secs as current_request_candidate_unix_secs;
use crate::constants::{CONTROL_CANDIDATE_ID_HEADER, CONTROL_REQUEST_ID_HEADER};
use crate::control::GatewayControlDecision;
use crate::execution_runtime::build_direct_execution_frame_stream;
#[cfg(test)]
use crate::execution_runtime::remote_compat::post_stream_plan_to_remote_execution_runtime;
use crate::execution_runtime::submission::{
    resolve_core_error_background_report_kind, submit_local_core_error_or_sync_finalize,
};
use crate::execution_runtime::transport::{
    DirectSyncExecutionRuntime, DirectUpstreamStreamExecution,
};
use crate::execution_runtime::{
    resolve_core_stream_direct_finalize_report_kind,
    resolve_core_stream_error_finalize_report_kind, should_fallback_to_control_stream,
    should_retry_next_local_candidate_stream,
};
use crate::execution_runtime::{MAX_STREAM_PREFETCH_BYTES, MAX_STREAM_PREFETCH_FRAMES};
use crate::log_ids::short_request_id;
use crate::request_candidate_runtime::{
    ensure_execution_request_candidate_slot, record_local_request_candidate_status,
};
use crate::usage::submit_stream_report;
use crate::usage::{GatewayStreamReportRequest, GatewaySyncReportRequest};
use crate::{AppState, GatewayError};

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
    ensure_execution_request_candidate_slot(state, &mut plan, &mut report_context).await;
    let plan_request_id_for_log = short_request_id(plan.request_id.as_str());
    #[cfg(not(test))]
    {
        let execution = match DirectSyncExecutionRuntime::new()
            .execute_stream(plan.clone())
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
                    error = %err,
                    "gateway in-process stream execution unavailable"
                );
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
            let execution = match DirectSyncExecutionRuntime::new()
                .execute_stream(plan.clone())
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
                        error = %err,
                        "gateway in-process stream execution unavailable"
                    );
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
                return Ok(None);
            }
        };

        if response.status() != http::StatusCode::OK {
            let terminal_unix_secs = current_request_candidate_unix_secs();
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
                    started_at_unix_secs: Some(terminal_unix_secs),
                    finished_at_unix_secs: Some(terminal_unix_secs),
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
            frame_stream,
        )
        .await;
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
    frame_stream: BoxStream<'static, Result<Bytes, IoError>>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let request_id = plan.request_id.as_str();
    let request_id_for_log = short_request_id(request_id);
    let candidate_id = plan.candidate_id.as_deref();
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

    if should_retry_next_local_candidate_stream(plan_kind, report_context.as_ref(), status_code) {
        let terminal_unix_secs = current_request_candidate_unix_secs();
        record_local_request_candidate_status(
            state,
            &plan,
            report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(status_code),
                error_type: Some("retryable_upstream_status".to_string()),
                error_message: Some(format!(
                    "execution runtime stream returned retryable status {status_code}"
                )),
                latency_ms: None,
                started_at_unix_secs: Some(terminal_unix_secs),
                finished_at_unix_secs: Some(terminal_unix_secs),
            },
        )
        .await;
        warn!(
            event_name = "local_stream_candidate_retry_scheduled",
            log_type = "event",
            trace_id = %trace_id,
            request_id = %request_id_for_log,
            status_code,
            "gateway local stream decision retrying next candidate after retryable execution runtime status"
        );
        return Ok(None);
    }

    let stream_error_finalize_kind =
        resolve_core_stream_error_finalize_report_kind(plan_kind, status_code);

    if should_fallback_to_control_stream(
        plan_kind,
        status_code,
        stream_error_finalize_kind.is_some(),
    ) {
        let terminal_unix_secs = current_request_candidate_unix_secs();
        record_local_request_candidate_status(
            state,
            &plan,
            report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(status_code),
                error_type: Some("control_fallback".to_string()),
                error_message: Some(format!(
                    "stream decision fell back to control after status {status_code}"
                )),
                latency_ms: None,
                started_at_unix_secs: Some(terminal_unix_secs),
                finished_at_unix_secs: Some(terminal_unix_secs),
            },
        )
        .await;
        return Ok(None);
    }

    if status_code >= 400 {
        let error_body = collect_error_body(&mut lines).await?;
        let (body_json, body_base64) = decode_stream_error_body(&headers, &error_body);
        let usage_report_kind = stream_error_finalize_kind
            .clone()
            .or_else(|| report_kind.clone())
            .unwrap_or_default();
        let usage_payload = GatewaySyncReportRequest {
            trace_id: trace_id.to_string(),
            report_kind: usage_report_kind,
            report_context: report_context.clone(),
            status_code,
            headers: headers.clone(),
            body_json: body_json.clone(),
            client_body_json: None,
            body_base64: body_base64.clone(),
            telemetry: None,
        };
        state
            .usage_runtime
            .record_sync_terminal(
                state.data.as_ref(),
                &plan,
                report_context.as_ref(),
                &usage_payload,
            )
            .await;
        let terminal_unix_secs = current_request_candidate_unix_secs();
        record_local_request_candidate_status(
            state,
            &plan,
            report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(status_code),
                error_type: Some("execution_runtime_stream_error".to_string()),
                error_message: Some(format!(
                    "execution runtime stream returned error status {status_code}"
                )),
                latency_ms: None,
                started_at_unix_secs: Some(terminal_unix_secs),
                finished_at_unix_secs: Some(terminal_unix_secs),
            },
        )
        .await;
        if let Some(report_kind) = stream_error_finalize_kind {
            let payload = GatewaySyncReportRequest {
                trace_id: trace_id.to_string(),
                report_kind,
                report_context,
                status_code,
                headers: headers.clone(),
                body_json,
                client_body_json: None,
                body_base64,
                telemetry: None,
            };
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
                headers,
                error_body,
            )?,
            Some(request_id),
            candidate_id,
        )?));
    }

    let direct_stream_finalize_kind = resolve_core_stream_direct_finalize_report_kind(plan_kind);
    let normalized_stream_report_context =
        normalize_provider_private_report_context(report_context.as_ref());
    let mut private_stream_normalizer =
        maybe_build_provider_private_stream_normalizer(report_context.as_ref());
    let mut local_stream_rewriter =
        maybe_build_stream_response_rewriter(normalized_stream_report_context.as_ref());
    if private_stream_normalizer.is_some() || local_stream_rewriter.is_some() {
        headers.remove("content-encoding");
        headers.remove("content-length");
        headers.insert("content-type".to_string(), "text/event-stream".to_string());
    }
    let mut prefetched_chunks: Vec<Bytes> = Vec::new();
    let mut provider_prefetched_body = Vec::new();
    let mut prefetched_body = Vec::new();
    let mut prefetched_inspection_body = Vec::new();
    let mut prefetched_telemetry: Option<ExecutionTelemetry> = None;
    let mut reached_eof = false;
    if let Some(ref report_kind) = direct_stream_finalize_kind {
        while prefetched_chunks.len() < MAX_STREAM_PREFETCH_FRAMES
            && prefetched_inspection_body.len() < MAX_STREAM_PREFETCH_BYTES
        {
            let Some(frame) = (match read_next_frame(&mut lines).await {
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
                        report_context.clone(),
                        request_id,
                        candidate_id,
                        report_kind,
                        &headers,
                        prefetched_telemetry.clone(),
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
                    let chunk = if let Some(chunk_b64) = chunk_b64 {
                        match base64::engine::general_purpose::STANDARD.decode(chunk_b64) {
                            Ok(decoded) => decoded,
                            Err(err) => {
                                let failure = build_stream_failure_report(
                                    "execution_runtime_stream_chunk_decode_error",
                                    format!(
                                        "failed to decode execution runtime stream chunk: {err}"
                                    ),
                                    502,
                                );
                                return handle_prefetch_stream_failure(
                                    state,
                                    trace_id,
                                    decision,
                                    &plan,
                                    report_context.clone(),
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    &headers,
                                    prefetched_telemetry.clone(),
                                    &prefetched_body,
                                    failure,
                                )
                                .await;
                            }
                        }
                    } else if let Some(text) = text {
                        text.into_bytes()
                    } else {
                        Vec::new()
                    };

                    if chunk.is_empty() {
                        continue;
                    }

                    provider_prefetched_body.extend_from_slice(&chunk);
                    prefetched_inspection_body.extend_from_slice(&chunk);

                    let inspection =
                        inspect_prefetched_stream_body(&headers, &prefetched_inspection_body);
                    match inspection {
                        StreamPrefetchInspection::EmbeddedError(body_json) => {
                            let payload = GatewaySyncReportRequest {
                                trace_id: trace_id.to_string(),
                                report_kind: report_kind.clone(),
                                report_context: report_context.clone(),
                                status_code,
                                headers: headers.clone(),
                                body_json: Some(body_json),
                                client_body_json: None,
                                body_base64: None,
                                telemetry: prefetched_telemetry.clone(),
                            };
                            state
                                .usage_runtime
                                .record_sync_terminal(
                                    state.data.as_ref(),
                                    &plan,
                                    report_context.as_ref(),
                                    &payload,
                                )
                                .await;
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
                                    report_context.clone(),
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    &headers,
                                    prefetched_telemetry.clone(),
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
                                    report_context.clone(),
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    &headers,
                                    prefetched_telemetry.clone(),
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
                StreamFramePayload::Eof { .. } => {
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
                        report_context.clone(),
                        request_id,
                        candidate_id,
                        report_kind,
                        &headers,
                        prefetched_telemetry.clone(),
                        &provider_prefetched_body,
                        build_stream_failure_from_execution_error(&error),
                    )
                    .await;
                }
                StreamFramePayload::Headers { .. } => {}
            }
        }
    }

    let candidate_started_unix_secs = current_request_candidate_unix_secs();
    state
        .usage_runtime
        .record_pending(state.data.as_ref(), &plan, report_context.as_ref())
        .await;
    state
        .usage_runtime
        .record_stream_started(
            state.data.as_ref(),
            &plan,
            report_context.as_ref(),
            status_code,
            &headers,
            prefetched_telemetry.as_ref(),
        )
        .await;
    record_local_request_candidate_status(
        state,
        &plan,
        report_context.as_ref(),
        SchedulerRequestCandidateStatusUpdate {
            status: RequestCandidateStatus::Streaming,
            status_code: Some(status_code),
            error_type: None,
            error_message: None,
            latency_ms: prefetched_telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.elapsed_ms),
            started_at_unix_secs: Some(candidate_started_unix_secs),
            finished_at_unix_secs: None,
        },
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<Result<Bytes, IoError>>(16);
    let state_for_report = state.clone();
    let plan_for_report = plan.clone();
    let trace_id_owned = trace_id.to_string();
    let headers_for_report = headers.clone();
    let report_kind_owned = report_kind.clone();
    let report_context_owned = report_context.clone();
    let provider_prefetched_body_for_report = provider_prefetched_body.clone();
    let prefetched_body_for_report = prefetched_body.clone();
    let prefetched_chunks_for_body = prefetched_chunks.clone();
    let initial_telemetry = prefetched_telemetry.clone();
    let initial_reached_eof = reached_eof;
    let direct_stream_finalize_kind_owned = direct_stream_finalize_kind.clone();
    let candidate_started_unix_secs_for_report = candidate_started_unix_secs;
    let request_id_for_report = request_id.to_string();
    let request_id_for_report_log = short_request_id(request_id);
    let candidate_id_for_report = candidate_id.map(ToOwned::to_owned);
    tokio::spawn(async move {
        let mut provider_buffered_body = provider_prefetched_body_for_report;
        let mut buffered_body = prefetched_body_for_report;
        let mut telemetry: Option<ExecutionTelemetry> = initial_telemetry;
        let reached_eof = initial_reached_eof;
        let mut downstream_dropped = false;
        let mut terminal_failure: Option<StreamFailureReport> = None;

        if !reached_eof {
            loop {
                let next_frame = match read_next_frame(&mut lines).await {
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
                        let chunk = if let Some(chunk_b64) = chunk_b64 {
                            match base64::engine::general_purpose::STANDARD.decode(chunk_b64) {
                                Ok(decoded) => decoded,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_chunk_decode_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = %err,
                                        "gateway failed to decode execution runtime chunk"
                                    );
                                    terminal_failure = Some(build_stream_failure_report(
                                        "execution_runtime_stream_chunk_decode_error",
                                        format!("failed to decode execution runtime stream chunk: {err}"),
                                        502,
                                    ));
                                    break;
                                }
                            }
                        } else if let Some(text) = text {
                            text.into_bytes()
                        } else {
                            Vec::new()
                        };

                        if chunk.is_empty() {
                            continue;
                        }

                        provider_buffered_body.extend_from_slice(&chunk);
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

                        buffered_body.extend_from_slice(&rewritten_chunk);
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
                        telemetry = Some(frame_telemetry);
                    }
                    StreamFramePayload::Eof { .. } => {
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
                            buffered_body.extend_from_slice(&rewritten_chunk);
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
                            buffered_body.extend_from_slice(&flushed_chunk);
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

        drop(tx);

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
            state_for_report
                .usage_runtime
                .record_stream_terminal(
                    state_for_report.data.as_ref(),
                    &plan_for_report,
                    report_context_owned.as_ref(),
                    &GatewayStreamReportRequest {
                        trace_id: trace_id_owned.clone(),
                        report_kind: report_kind_owned.clone().unwrap_or_default(),
                        report_context: report_context_owned.clone(),
                        status_code: 499,
                        headers: headers_for_report.clone(),
                        provider_body_base64: (!provider_buffered_body.is_empty()).then(|| {
                            base64::engine::general_purpose::STANDARD
                                .encode(&provider_buffered_body)
                        }),
                        client_body_base64: (!buffered_body.is_empty()).then(|| {
                            base64::engine::general_purpose::STANDARD.encode(&buffered_body)
                        }),
                        telemetry: telemetry.clone(),
                    },
                    true,
                )
                .await;
            record_local_request_candidate_status(
                &state_for_report,
                &plan_for_report,
                report_context_owned.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Cancelled,
                    status_code: Some(499),
                    error_type: Some("downstream_disconnect".to_string()),
                    error_message: Some("client disconnected before stream completion".to_string()),
                    latency_ms: telemetry.as_ref().and_then(|value| value.elapsed_ms),
                    started_at_unix_secs: Some(candidate_started_unix_secs_for_report),
                    finished_at_unix_secs: Some(current_request_candidate_unix_secs()),
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
                report_context_owned.as_ref(),
                &headers_for_report,
                telemetry.clone(),
                &provider_buffered_body,
                candidate_started_unix_secs_for_report,
                failure,
            )
            .await;
            return;
        }

        let usage_payload = GatewayStreamReportRequest {
            trace_id: trace_id_owned.clone(),
            report_kind: report_kind_owned.clone().unwrap_or_default(),
            report_context: report_context_owned.clone(),
            status_code,
            headers: headers_for_report.clone(),
            provider_body_base64: (!provider_buffered_body.is_empty())
                .then(|| base64::engine::general_purpose::STANDARD.encode(&provider_buffered_body)),
            client_body_base64: (!buffered_body.is_empty())
                .then(|| base64::engine::general_purpose::STANDARD.encode(&buffered_body)),
            telemetry: telemetry.clone(),
        };
        state_for_report
            .usage_runtime
            .record_stream_terminal(
                state_for_report.data.as_ref(),
                &plan_for_report,
                report_context_owned.as_ref(),
                &usage_payload,
                false,
            )
            .await;
        record_local_request_candidate_status(
            &state_for_report,
            &plan_for_report,
            report_context_owned.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Success,
                status_code: Some(status_code),
                error_type: None,
                error_message: None,
                latency_ms: telemetry.as_ref().and_then(|value| value.elapsed_ms),
                started_at_unix_secs: Some(candidate_started_unix_secs_for_report),
                finished_at_unix_secs: Some(current_request_candidate_unix_secs()),
            },
        )
        .await;

        if let Some(report_kind) = report_kind_owned {
            let mut report = usage_payload;
            report.report_kind = report_kind;
            if let Err(err) = submit_stream_report(&state_for_report, &trace_id_owned, report).await
            {
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

    headers.insert(
        CONTROL_REQUEST_ID_HEADER.to_string(),
        request_id.to_string(),
    );

    if let Some(candidate_id) = candidate_id
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
