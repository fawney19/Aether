use std::collections::BTreeMap;

use aether_contracts::{ExecutionPlan, ExecutionResult, ExecutionTelemetry};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::{
    execution_error_details, parse_request_candidate_report_context,
    SchedulerRequestCandidateStatusUpdate,
};
use aether_usage_runtime::{
    build_lifecycle_usage_seed, build_sync_terminal_usage_payload_seed,
    build_terminal_usage_context_seed,
};
use axum::body::Body;
use axum::http::Response;
use base64::Engine as _;
use tracing::warn;

use crate::ai_pipeline_api::{
    implicit_sync_finalize_report_kind, maybe_build_sync_finalize_outcome,
    LocalCoreSyncFinalizeOutcome,
};
use crate::api::response::{
    attach_control_metadata_headers, build_client_response, build_client_response_from_parts,
};
use crate::clock::current_unix_ms as current_request_candidate_unix_ms;
use crate::constants::{CONTROL_CANDIDATE_ID_HEADER, CONTROL_REQUEST_ID_HEADER};
use crate::control::GatewayControlDecision;
#[cfg(test)]
use crate::execution_runtime::remote_compat::post_sync_plan_to_remote_execution_runtime;
use crate::execution_runtime::submission::submit_local_core_error_or_sync_finalize;
use crate::execution_runtime::transport::DirectSyncExecutionRuntime;
use crate::execution_runtime::{
    local_failover_response_text, resolve_core_sync_error_finalize_report_kind,
    should_fallback_to_control_sync, should_finalize_sync_response,
    should_retry_next_local_candidate_sync, should_stop_local_candidate_failover_sync,
};
use crate::log_ids::short_request_id;
use crate::request_candidate_runtime::{
    ensure_execution_request_candidate_slot, record_local_request_candidate_status,
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
    resolve_local_sync_success_background_report_kind, LocalVideoSyncSuccessOutcome,
};

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
        .record_sync_terminal(state.data.as_ref(), &context_seed, &payload_seed);
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
    ensure_execution_request_candidate_slot(state, &mut plan, &mut report_context).await;
    let plan_request_id = plan.request_id.as_str();
    let plan_request_id_for_log = short_request_id(plan_request_id);
    let plan_candidate_id = plan.candidate_id.as_deref();
    let provider_name = plan.provider_name.as_deref().unwrap_or("-");
    let endpoint_id = plan.endpoint_id.as_str();
    let key_id = plan.key_id.as_str();
    let model_name = plan.model_name.as_deref().unwrap_or("-");
    let candidate_index = parse_request_candidate_report_context(report_context.as_ref())
        .and_then(|context| context.candidate_index)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let candidate_started_unix_secs = current_request_candidate_unix_ms();
    let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
    state
        .usage_runtime
        .record_pending(state.data.as_ref(), &lifecycle_seed);
    #[cfg(not(test))]
    let result = {
        match DirectSyncExecutionRuntime::new()
            .execute_sync(plan.clone())
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
                    error = %err,
                    "gateway in-process sync execution unavailable"
                );
                return Ok(None);
            }
        }
    };
    #[cfg(test)]
    let result = {
        let remote_execution_runtime_base_url = state
            .execution_runtime_override_base_url()
            .unwrap_or_default();
        if remote_execution_runtime_base_url.trim().is_empty() {
            match DirectSyncExecutionRuntime::new()
                .execute_sync(plan.clone())
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
                        error = %err,
                        "gateway in-process sync execution unavailable"
                    );
                    return Ok(None);
                }
            }
        } else {
            let remote_outcome = execute_sync_via_remote_execution_runtime(
                state,
                remote_execution_runtime_base_url,
                trace_id,
                decision,
                &plan,
                plan_request_id,
                plan_candidate_id,
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
    let result_body_json = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref());
    let (result_error_type, result_error_message) =
        execution_error_details(result.error.as_ref(), result_body_json);
    let result_latency_ms = result
        .telemetry
        .as_ref()
        .and_then(|telemetry| telemetry.elapsed_ms);
    let mut headers = result.headers.clone();
    let (body_bytes, body_json, body_base64) = decode_execution_result_body(&result, &mut headers)?;
    let local_failover_response_text = local_failover_response_text(
        body_json.as_ref(),
        &body_bytes,
        result.error.as_ref().map(|error| error.message.as_str()),
    );
    let stop_local_failover = should_stop_local_candidate_failover_sync(
        state,
        &plan,
        plan_kind,
        report_context.as_ref(),
        &result,
        local_failover_response_text.as_deref(),
    )
    .await;
    if should_retry_next_local_candidate_sync(
        state,
        &plan,
        plan_kind,
        report_context.as_ref(),
        &result,
        local_failover_response_text.as_deref(),
    )
    .await
        && !stop_local_failover
    {
        let terminal_unix_secs = current_request_candidate_unix_ms();
        record_local_request_candidate_status(
            state,
            &plan,
            report_context.as_ref(),
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
    let request_id = (!result.request_id.trim().is_empty())
        .then_some(result.request_id.as_str())
        .or(Some(plan_request_id));
    let request_id_for_log = short_request_id(request_id.unwrap_or("-"));
    let candidate_id = result.candidate_id.as_deref().or(plan_candidate_id);
    let has_body_bytes = body_base64.is_some();
    let explicit_finalize = should_finalize_sync_response(report_kind.as_deref());
    let mapped_error_finalize_kind =
        resolve_core_sync_error_finalize_report_kind(plan_kind, &result, body_json.as_ref());
    let implicit_finalize = if explicit_finalize || mapped_error_finalize_kind.is_some() {
        None
    } else {
        maybe_build_implicit_sync_finalize_outcome(
            trace_id,
            decision,
            plan_kind,
            report_context.clone(),
            result.status_code,
            headers.clone(),
            body_json.clone(),
            body_base64.clone(),
            result.telemetry.clone(),
        )?
    };
    let finalize_report_kind = if explicit_finalize {
        report_kind.clone()
    } else if let Some(implicit_finalize) = implicit_finalize.as_ref() {
        Some(implicit_finalize.payload.report_kind.clone())
    } else {
        mapped_error_finalize_kind.clone()
    };

    if !stop_local_failover
        && should_fallback_to_control_sync(
            plan_kind,
            &result,
            body_json.as_ref(),
            has_body_bytes,
            explicit_finalize || implicit_finalize.is_some(),
            mapped_error_finalize_kind.is_some(),
        )
    {
        let terminal_unix_secs = current_request_candidate_unix_ms();
        record_local_request_candidate_status(
            state,
            &plan,
            report_context.as_ref(),
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
    record_local_request_candidate_status(
        state,
        &plan,
        report_context.as_ref(),
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

    let base_usage_payload = GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind: finalize_report_kind
            .clone()
            .or_else(|| report_kind.clone())
            .unwrap_or_default(),
        report_context: report_context.clone(),
        status_code: result.status_code,
        headers: headers.clone(),
        body_json: body_json.clone(),
        client_body_json: None,
        body_base64: body_base64.clone(),
        telemetry: result.telemetry.clone(),
    };

    if let Some(finalize_report_kind) = finalize_report_kind {
        if let Some(implicit_finalize) = implicit_finalize {
            let usage_payload = implicit_finalize
                .outcome
                .background_report
                .as_ref()
                .unwrap_or(&implicit_finalize.payload);
            record_sync_terminal_usage(state, &plan, report_context.as_ref(), usage_payload);
            if let Some(report_payload) = implicit_finalize.outcome.background_report {
                spawn_sync_report(state.clone(), trace_id.to_string(), report_payload);
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

        let payload = GatewaySyncReportRequest {
            trace_id: trace_id.to_string(),
            report_kind: finalize_report_kind,
            report_context,
            status_code: result.status_code,
            headers: headers.clone(),
            body_json: body_json.clone(),
            client_body_json: None,
            body_base64: body_base64.clone(),
            telemetry: result.telemetry.clone(),
        };
        if let Some(outcome) = maybe_build_sync_finalize_outcome(trace_id, decision, &payload)? {
            let usage_payload = outcome.background_report.as_ref().unwrap_or(&payload);
            record_sync_terminal_usage(
                state,
                &plan,
                payload.report_context.as_ref(),
                usage_payload,
            );
            if let Some(report_payload) = outcome.background_report {
                spawn_sync_report(state.clone(), trace_id.to_string(), report_payload);
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
        if let Some(outcome) = maybe_build_local_video_success_outcome(
            trace_id,
            decision,
            &payload,
            &state.video_tasks,
            &plan,
        )? {
            record_sync_terminal_usage(
                state,
                &plan,
                payload.report_context.as_ref(),
                &outcome.report_payload,
            );
            if let Some(snapshot) = outcome.local_task_snapshot.clone() {
                state.video_tasks.record_snapshot(snapshot.clone());
                let _ = state.upsert_video_task_snapshot(&snapshot).await?;
            }
            match outcome.report_mode {
                VideoTaskSyncReportMode::InlineSync => {
                    submit_sync_report(state, trace_id, outcome.report_payload).await?;
                }
                VideoTaskSyncReportMode::Background => {
                    spawn_sync_report(state.clone(), trace_id.to_string(), outcome.report_payload);
                }
            }
            return Ok(Some(attach_control_metadata_headers(
                outcome.response,
                request_id,
                candidate_id,
            )?));
        }
        if let Some(response) =
            maybe_build_local_sync_finalize_response(trace_id, decision, &payload)?
        {
            let usage_payload = if let Some(success_report_kind) =
                resolve_local_sync_success_background_report_kind(payload.report_kind.as_str())
            {
                let mut report_payload = payload.clone();
                report_payload.report_kind = success_report_kind.to_string();
                report_payload
            } else {
                payload.clone()
            };
            record_sync_terminal_usage(
                state,
                &plan,
                payload.report_context.as_ref(),
                &usage_payload,
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
            if let Some(success_report_kind) =
                resolve_local_sync_success_background_report_kind(payload.report_kind.as_str())
            {
                let mut report_payload = usage_payload;
                report_payload.report_kind = success_report_kind.to_string();
                spawn_sync_report(state.clone(), trace_id.to_string(), report_payload);
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
            let usage_payload = if let Some(error_report_kind) =
                resolve_local_sync_error_background_report_kind(payload.report_kind.as_str())
            {
                let mut report_payload = payload.clone();
                report_payload.report_kind = error_report_kind.to_string();
                report_payload
            } else {
                payload.clone()
            };
            record_sync_terminal_usage(
                state,
                &plan,
                payload.report_context.as_ref(),
                &usage_payload,
            );
            if let Some(error_report_kind) =
                resolve_local_sync_error_background_report_kind(payload.report_kind.as_str())
            {
                let mut report_payload = usage_payload;
                report_payload.report_kind = error_report_kind.to_string();
                spawn_sync_report(state.clone(), trace_id.to_string(), report_payload);
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
        record_sync_terminal_usage(state, &plan, payload.report_context.as_ref(), &payload);
        let response =
            submit_local_core_error_or_sync_finalize(state, trace_id, decision, payload).await?;
        return Ok(Some(attach_control_metadata_headers(
            response,
            request_id,
            candidate_id,
        )?));
    }

    record_sync_terminal_usage(state, &plan, report_context.as_ref(), &base_usage_payload);
    if let Some(report_kind) = report_kind {
        let report = GatewaySyncReportRequest {
            trace_id: trace_id.to_string(),
            report_kind,
            report_context,
            status_code: result.status_code,
            headers: headers.clone(),
            body_json: body_json.clone(),
            client_body_json: None,
            body_base64: body_base64.clone(),
            telemetry: result.telemetry.clone(),
        };
        spawn_sync_report(state.clone(), trace_id.to_string(), report);
    }

    let request_id_header: Option<&str> = request_id
        .map(str::trim)
        .filter(|value: &&str| !value.is_empty());
    if let Some(request_id) = request_id_header {
        headers.insert(
            CONTROL_REQUEST_ID_HEADER.to_string(),
            request_id.to_string(),
        );
    }

    let candidate_id_header: Option<&str> = candidate_id
        .map(str::trim)
        .filter(|value: &&str| !value.is_empty());
    if let Some(candidate_id) = candidate_id_header {
        headers.insert(
            CONTROL_CANDIDATE_ID_HEADER.to_string(),
            candidate_id.to_string(),
        );
    }

    Ok(Some(build_client_response_from_parts(
        result.status_code,
        &headers,
        Body::from(body_bytes),
        trace_id,
        Some(decision),
    )?))
}

#[allow(clippy::too_many_arguments)] // mirrors sync execution context
fn maybe_build_implicit_sync_finalize_outcome(
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_context: Option<serde_json::Value>,
    status_code: u16,
    headers: BTreeMap<String, String>,
    body_json: Option<serde_json::Value>,
    body_base64: Option<String>,
    telemetry: Option<ExecutionTelemetry>,
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
        report_context,
        status_code,
        headers,
        body_json,
        client_body_json: None,
        body_base64,
        telemetry,
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
