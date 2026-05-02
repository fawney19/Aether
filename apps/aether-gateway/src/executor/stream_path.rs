use aether_ai_serving::{
    run_ai_stream_execution_path, AiPlanFallbackReason, AiServingExecutionOutcome,
    AiStreamExecutionPathPort, AiStreamExecutionStep,
};
use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::http::Response;
use std::collections::BTreeMap;

use crate::ai_serving::api::{
    is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
    supports_stream_execution_decision_kind, AiStreamAttempt, OPENAI_VIDEO_CONTENT_PLAN_KIND,
};
use crate::api::response::build_client_response_from_parts;
use crate::control::GatewayControlDecision;
use crate::{AppState, GatewayError, GatewayFallbackReason};

use super::{
    build_direct_plan_bypass_cache_key, execute_stream_plan_and_reports,
    maybe_execute_stream_via_local_decision, maybe_execute_stream_via_local_gemini_files_decision,
    maybe_execute_stream_via_local_image_decision,
    maybe_execute_stream_via_local_openai_responses_decision,
    maybe_execute_stream_via_local_same_format_provider_decision,
    maybe_execute_stream_via_local_standard_decision, maybe_execute_stream_via_plan_fallback,
    maybe_execute_stream_via_remote_decision, parse_local_request_body, should_skip_direct_plan,
    LocalExecutionRequestOutcome,
};

pub(crate) async fn maybe_execute_via_stream_decision_path(
    state: &AppState,
    parts: &http::request::Parts,
    body_bytes: &Bytes,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some(plan_kind) = resolve_execution_runtime_stream_plan_kind(parts, decision) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let Some((body_json, body_base64)) = parse_local_request_body(parts, body_bytes) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    if !is_matching_stream_request(plan_kind, parts, &body_json, body_base64.as_deref()) {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let bypass_cache_key =
        build_direct_plan_bypass_cache_key(plan_kind, parts, body_bytes, decision);
    if should_skip_direct_plan(state, &bypass_cache_key) {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let port = GatewayStreamExecutionPathPort {
        state,
        parts,
        trace_id,
        decision,
        body_json: &body_json,
        body_base64,
        plan_kind,
        bypass_cache_key,
        scheduler_supported: supports_stream_execution_decision_kind(plan_kind),
    };

    Ok(from_ai_serving_outcome(
        run_ai_stream_execution_path(&port).await?,
    ))
}

struct GatewayStreamExecutionPathPort<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    body_base64: Option<String>,
    plan_kind: &'a str,
    bypass_cache_key: String,
    scheduler_supported: bool,
}

#[async_trait]
impl AiStreamExecutionPathPort for GatewayStreamExecutionPathPort<'_> {
    type Response = Response<Body>;
    type Exhaustion = super::LocalExecutionExhaustion;
    type Error = GatewayError;

    fn scheduler_decision_supported(&self) -> bool {
        self.scheduler_supported
    }

    async fn execute_stream_step(
        &self,
        step: AiStreamExecutionStep,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error> {
        let outcome = match step {
            AiStreamExecutionStep::LocalVideoContent => {
                maybe_execute_local_video_task_content_stream(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await?
            }
            AiStreamExecutionStep::LocalImage => {
                maybe_execute_stream_via_local_image_decision(
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
            AiStreamExecutionStep::LocalOpenAiChat => {
                maybe_execute_stream_via_local_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
            }
            AiStreamExecutionStep::LocalOpenAiResponses => {
                maybe_execute_stream_via_local_openai_responses_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
            }
            AiStreamExecutionStep::LocalStandardFamily => {
                maybe_execute_stream_via_local_standard_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
            }
            AiStreamExecutionStep::LocalSameFormatProvider => {
                maybe_execute_stream_via_local_same_format_provider_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await?
            }
            AiStreamExecutionStep::LocalGeminiFiles => {
                maybe_execute_stream_via_local_gemini_files_decision(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await?
            }
            AiStreamExecutionStep::RemoteDecision => {
                if let Some(response) = maybe_execute_stream_via_remote_decision(
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

    async fn execute_stream_plan_fallback(
        &self,
        reason: AiPlanFallbackReason,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error> {
        let outcome = maybe_execute_stream_via_plan_fallback(
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

async fn maybe_execute_local_video_task_content_stream(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    if plan_kind != OPENAI_VIDEO_CONTENT_PLAN_KIND
        || decision.route_family.as_deref() != Some("openai")
    {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let _ = state
        .hydrate_video_task_for_route(decision.route_family.as_deref(), parts.uri.path())
        .await?;

    if let Some(task_id) =
        crate::video_tasks::extract_openai_task_id_from_content_path(parts.uri.path())
    {
        let refresh_path = format!("/v1/videos/{task_id}");
        if let Some(refresh_plan) = state.video_tasks.prepare_read_refresh_sync_plan(
            Some("openai"),
            &refresh_path,
            trace_id,
        ) {
            state.execute_video_task_refresh_plan(&refresh_plan).await?;
        }
    }

    let Some(action) = state.video_tasks.prepare_openai_content_stream_action(
        parts.uri.path(),
        parts.uri.query(),
        trace_id,
    ) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    match action {
        crate::video_tasks::LocalVideoTaskContentAction::Immediate {
            status_code,
            body_json,
        } => Ok(LocalExecutionRequestOutcome::Responded(
            build_json_response(trace_id, decision, status_code, &body_json)?,
        )),
        crate::video_tasks::LocalVideoTaskContentAction::StreamPlan(plan) => {
            let plan = *plan;
            execute_stream_plan_and_reports(
                state,
                trace_id,
                decision,
                plan_kind,
                vec![AiStreamAttempt {
                    plan,
                    report_kind: None,
                    report_context: None,
                }],
            )
            .await
        }
    }
}

fn build_json_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    status_code: u16,
    body_json: &serde_json::Value,
) -> Result<Response<Body>, GatewayError> {
    let body_bytes =
        serde_json::to_vec(body_json).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("content-length".to_string(), body_bytes.len().to_string());
    build_client_response_from_parts(
        status_code,
        &headers,
        Body::from(body_bytes),
        trace_id,
        Some(decision),
    )
}
