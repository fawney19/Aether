use aether_ai_serving::{
    run_ai_sync_execution_path, AiPlanFallbackReason, AiServingExecutionOutcome,
    AiSyncExecutionPathPort, AiSyncExecutionStep,
};
use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::http::Response;
use std::collections::BTreeMap;

use crate::ai_serving::api::{
    is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
    resolve_execution_runtime_sync_plan_kind, supports_sync_execution_decision_kind, AiSyncAttempt,
    GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND, OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::api::response::build_client_response_from_parts;
use crate::control::resolve_execution_runtime_auth_context;
use crate::control::GatewayControlDecision;
use crate::{AppState, GatewayError, GatewayFallbackReason};

use super::{
    build_direct_plan_bypass_cache_key, execute_sync_plan_and_reports,
    maybe_execute_sync_via_local_decision, maybe_execute_sync_via_local_gemini_files_decision,
    maybe_execute_sync_via_local_image_decision,
    maybe_execute_sync_via_local_openai_responses_decision,
    maybe_execute_sync_via_local_same_format_provider_decision,
    maybe_execute_sync_via_local_standard_decision, maybe_execute_sync_via_local_video_decision,
    maybe_execute_sync_via_plan_fallback, maybe_execute_sync_via_remote_decision,
    parse_local_request_body, should_skip_direct_plan, LocalExecutionRequestOutcome,
};

pub(crate) async fn maybe_execute_via_sync_decision_path(
    state: &AppState,
    parts: &http::request::Parts,
    body_bytes: &Bytes,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    if let LocalExecutionRequestOutcome::Responded(response) =
        maybe_build_local_video_task_read_response(state, parts, trace_id, decision).await?
    {
        return Ok(LocalExecutionRequestOutcome::Responded(response));
    }

    let Some(plan_kind) = resolve_execution_runtime_sync_plan_kind(parts, decision) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let Some((body_json, body_base64)) = parse_local_request_body(parts, body_bytes) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    if let Some(stream_plan_kind) = resolve_execution_runtime_stream_plan_kind(parts, decision) {
        if is_matching_stream_request(stream_plan_kind, parts, &body_json, body_base64.as_deref()) {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        }
    }

    let bypass_cache_key =
        build_direct_plan_bypass_cache_key(plan_kind, parts, body_bytes, decision);
    if should_skip_direct_plan(state, &bypass_cache_key) {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let port = GatewaySyncExecutionPathPort {
        state,
        parts,
        trace_id,
        decision,
        body_json: &body_json,
        body_base64,
        body_is_empty: body_bytes.is_empty(),
        plan_kind,
        bypass_cache_key,
        scheduler_supported: supports_sync_execution_decision_kind(plan_kind),
    };

    Ok(from_ai_serving_outcome(
        run_ai_sync_execution_path(&port).await?,
    ))
}

struct GatewaySyncExecutionPathPort<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    body_base64: Option<String>,
    body_is_empty: bool,
    plan_kind: &'a str,
    bypass_cache_key: String,
    scheduler_supported: bool,
}

#[async_trait]
impl AiSyncExecutionPathPort for GatewaySyncExecutionPathPort<'_> {
    type Response = Response<Body>;
    type Exhaustion = super::LocalExecutionExhaustion;
    type Error = GatewayError;

    fn scheduler_decision_supported(&self) -> bool {
        self.scheduler_supported
    }

    async fn execute_sync_step(
        &self,
        step: AiSyncExecutionStep,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error> {
        let outcome = match step {
            AiSyncExecutionStep::VideoTaskFollowUp => {
                maybe_execute_local_video_task_follow_up_sync(
                    self.state,
                    self.parts,
                    self.body_json,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await?
            }
            AiSyncExecutionStep::LocalVideo => {
                maybe_execute_sync_via_local_video_decision(
                    self.state,
                    self.parts,
                    self.body_json,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await?
            }
            AiSyncExecutionStep::LocalImage => {
                maybe_execute_sync_via_local_image_decision(
                    self.state,
                    self.parts,
                    self.body_json,
                    self.body_base64.as_deref(),
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await?
            }
            AiSyncExecutionStep::LocalOpenAiChat => {
                maybe_execute_sync_via_local_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
            }
            AiSyncExecutionStep::LocalOpenAiResponses => {
                maybe_execute_sync_via_local_openai_responses_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
            }
            AiSyncExecutionStep::LocalStandardFamily => {
                maybe_execute_sync_via_local_standard_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
            }
            AiSyncExecutionStep::LocalSameFormatProvider => {
                maybe_execute_sync_via_local_same_format_provider_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
            }
            AiSyncExecutionStep::LocalGeminiFiles => {
                maybe_execute_sync_via_local_gemini_files_decision(
                    self.state,
                    self.parts,
                    self.body_json,
                    self.body_base64.as_deref(),
                    self.body_is_empty,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await?
            }
            AiSyncExecutionStep::RemoteDecision => {
                if let Some(response) = maybe_execute_sync_via_remote_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
                {
                    LocalExecutionRequestOutcome::Responded(response)
                } else {
                    LocalExecutionRequestOutcome::NoPath
                }
            }
        };
        Ok(to_ai_serving_outcome(outcome))
    }

    async fn execute_sync_plan_fallback(
        &self,
        reason: AiPlanFallbackReason,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error> {
        let outcome = maybe_execute_sync_via_plan_fallback(
            self.state,
            self.parts,
            self.trace_id,
            self.decision,
            self.body_json,
            self.body_base64.clone(),
            self.plan_kind,
            self.bypass_cache_key.clone(),
            gateway_fallback_reason(reason),
        )
        .await?;
        Ok(to_ai_serving_outcome(outcome))
    }
}

fn to_ai_serving_outcome(
    outcome: LocalExecutionRequestOutcome,
) -> AiServingExecutionOutcome<Response<Body>, super::LocalExecutionExhaustion> {
    match outcome {
        LocalExecutionRequestOutcome::Responded(response) => {
            AiServingExecutionOutcome::Responded(response)
        }
        LocalExecutionRequestOutcome::Exhausted(outcome) => {
            AiServingExecutionOutcome::Exhausted(outcome)
        }
        LocalExecutionRequestOutcome::NoPath => AiServingExecutionOutcome::NoPath,
    }
}

fn from_ai_serving_outcome(
    outcome: AiServingExecutionOutcome<Response<Body>, super::LocalExecutionExhaustion>,
) -> LocalExecutionRequestOutcome {
    match outcome {
        AiServingExecutionOutcome::Responded(response) => {
            LocalExecutionRequestOutcome::Responded(response)
        }
        AiServingExecutionOutcome::Exhausted(outcome) => {
            LocalExecutionRequestOutcome::Exhausted(outcome)
        }
        AiServingExecutionOutcome::NoPath => LocalExecutionRequestOutcome::NoPath,
    }
}

fn gateway_fallback_reason(reason: AiPlanFallbackReason) -> GatewayFallbackReason {
    match reason {
        AiPlanFallbackReason::RemoteDecisionMiss => GatewayFallbackReason::RemoteDecisionMiss,
        AiPlanFallbackReason::SchedulerDecisionUnsupported => {
            GatewayFallbackReason::SchedulerDecisionUnsupported
        }
    }
}

async fn maybe_build_local_video_task_read_response(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    if parts.method != http::Method::GET || decision.route_kind.as_deref() != Some("video") {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let _ = state
        .hydrate_video_task_for_route(decision.route_family.as_deref(), parts.uri.path())
        .await?;

    let refresh_plan = state.video_tasks.prepare_read_refresh_sync_plan(
        decision.route_family.as_deref(),
        parts.uri.path(),
        trace_id,
    );

    if let Some(refresh_plan) = refresh_plan {
        state.execute_video_task_refresh_plan(&refresh_plan).await?;
    }

    let read_response = state
        .video_tasks
        .read_response(decision.route_family.as_deref(), parts.uri.path());
    let read_response = match read_response {
        Some(read_response) => Some(read_response),
        None => {
            state
                .read_data_backed_video_task_response(
                    decision.route_family.as_deref(),
                    parts.uri.path(),
                )
                .await?
        }
    };
    let Some(read_response) = read_response else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let body_bytes = serde_json::to_vec(&read_response.body_json)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("content-length".to_string(), body_bytes.len().to_string());

    Ok(LocalExecutionRequestOutcome::Responded(
        build_client_response_from_parts(
            read_response.status_code,
            &headers,
            Body::from(body_bytes),
            trace_id,
            Some(decision),
        )?,
    ))
}

async fn maybe_execute_local_video_task_follow_up_sync(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    if !matches!(
        plan_kind,
        OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND
            | OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND
            | OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND
            | GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND
    ) {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let _ = state
        .hydrate_video_task_for_route(decision.route_family.as_deref(), parts.uri.path())
        .await?;

    let auth_context = resolve_execution_runtime_auth_context(
        state,
        decision,
        &parts.headers,
        &parts.uri,
        trace_id,
    )
    .await?;
    let Some(follow_up) = state.video_tasks.prepare_follow_up_sync_plan(
        plan_kind,
        parts.uri.path(),
        Some(body_json),
        auth_context.as_ref(),
        trace_id,
    ) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        vec![AiSyncAttempt {
            plan: follow_up.plan,
            report_kind: follow_up.report_kind,
            report_context: follow_up.report_context,
        }],
    )
    .await
}
