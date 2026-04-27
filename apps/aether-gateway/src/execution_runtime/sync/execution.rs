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
use crate::control::GatewayControlDecision;
use crate::execution_runtime::oauth_retry::refresh_oauth_plan_auth_for_retry;
#[cfg(test)]
use crate::execution_runtime::remote_compat::post_sync_plan_to_remote_execution_runtime;
use crate::execution_runtime::submission::submit_local_core_error_or_sync_finalize;
use crate::execution_runtime::transport::DirectSyncExecutionRuntime;
use crate::execution_runtime::{
    analyze_local_candidate_failover_sync, local_failover_response_text,
    resolve_core_sync_error_finalize_report_kind, should_fallback_to_control_sync,
    should_finalize_sync_response, LocalFailoverDecision,
};
use crate::log_ids::short_request_id;
use crate::orchestration::{
    apply_local_execution_effect, build_local_error_flow_metadata, with_error_flow_report_context,
    LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect, LocalAttemptFailureEffect,
    LocalExecutionEffect, LocalExecutionEffectContext, LocalHealthFailureEffect,
    LocalHealthSuccessEffect, LocalOAuthInvalidationEffect, LocalPoolErrorEffect,
};
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
    resolve_local_sync_success_background_report_kind, LocalVideoSyncSuccessBuild,
    LocalVideoSyncSuccessOutcome,
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
        .record_sync_terminal(state.data.as_ref(), context_seed, payload_seed);
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
    #[cfg(not(test))]
    let mut result = {
        match DirectSyncExecutionRuntime::new().execute_sync(&plan).await {
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
            match DirectSyncExecutionRuntime::new().execute_sync(&plan).await {
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
        let mut headers = std::mem::take(&mut result.headers);
        let (body_bytes, body_json, body_base64) =
            decode_execution_result_body(result.body.take(), &mut headers)?;
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
        let error_flow_report_context = with_error_flow_report_context(
            report_context.as_ref(),
            build_local_error_flow_metadata(
                result.status_code,
                local_failover_response_text.as_deref(),
                local_failover_analysis,
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
            &headers,
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
        let error_flow_report_context = with_error_flow_report_context(
            report_context.as_ref(),
            build_local_error_flow_metadata(
                result.status_code,
                local_failover_response_text.as_deref(),
                local_failover_analysis,
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
            with_error_flow_report_context(
                report_context.as_ref(),
                build_local_error_flow_metadata(
                    result.status_code,
                    local_failover_response_text.as_deref(),
                    local_failover_analysis,
                ),
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
        record_sync_terminal_usage(
            state,
            &plan,
            implicit_finalize.payload.report_context.as_ref(),
            usage_payload,
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
            headers,
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
            record_sync_terminal_usage(
                state,
                &plan,
                payload.report_context.as_ref(),
                usage_payload,
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
                record_sync_terminal_usage(
                    state,
                    &plan,
                    original_report_context.as_ref(),
                    &report_payload,
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
            record_sync_terminal_usage(state, &plan, payload.report_context.as_ref(), &payload);
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
            record_sync_terminal_usage(state, &plan, payload.report_context.as_ref(), &payload);
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
        record_sync_terminal_usage(state, &plan, payload.report_context.as_ref(), &payload);
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
        headers,
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
    record_sync_terminal_usage(
        state,
        &plan,
        usage_payload.report_context.as_ref(),
        &usage_payload,
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
