use aether_ai_serving::{
    build_ai_execution_decision_from_plan, run_ai_stream_decision_path,
    AiExecutionDecisionFromPlanParts, AiStreamDecisionPathPort, AiStreamDecisionStep,
};
use async_trait::async_trait;

use crate::ai_serving::planner::common::{
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, OPENAI_VIDEO_CONTENT_PLAN_KIND,
};
use crate::ai_serving::planner::route::{
    is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
};
use crate::ai_serving::{resolve_decision_execution_runtime_auth_context, GatewayControlDecision};
use crate::{AiExecutionDecision, AppState, GatewayError};

pub(crate) async fn maybe_build_stream_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(plan_kind) = resolve_execution_runtime_stream_plan_kind(parts, decision) else {
        return Ok(None);
    };

    if !is_matching_stream_request(plan_kind, parts, body_json, body_base64) {
        return Ok(None);
    }

    let port = GatewayStreamDecisionPathPort {
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64,
        plan_kind,
    };

    run_ai_stream_decision_path(&port).await
}

struct GatewayStreamDecisionPathPort<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    body_base64: Option<&'a str>,
    plan_kind: &'a str,
}

#[async_trait]
impl AiStreamDecisionPathPort for GatewayStreamDecisionPathPort<'_> {
    type Decision = AiExecutionDecision;
    type Error = GatewayError;

    async fn build_stream_decision_step(
        &self,
        step: AiStreamDecisionStep,
    ) -> Result<Option<Self::Decision>, Self::Error> {
        match step {
            AiStreamDecisionStep::LocalVideoContent => {
                maybe_build_local_video_task_content_stream_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await
            }
            AiStreamDecisionStep::LocalImage => {
                super::maybe_build_stream_local_image_decision_payload(
                    self.state,
                    self.parts,
                    self.body_json,
                    self.body_base64,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await
            }
            AiStreamDecisionStep::LocalOpenAiChat => {
                super::maybe_build_stream_local_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await
            }
            AiStreamDecisionStep::LocalOpenAiResponses => {
                super::maybe_build_stream_local_openai_responses_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await
            }
            AiStreamDecisionStep::LocalStandardFamily => {
                super::maybe_build_stream_local_standard_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await
            }
            AiStreamDecisionStep::LocalSameFormatProvider => {
                super::maybe_build_stream_local_same_format_provider_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await
            }
            AiStreamDecisionStep::LocalGeminiFiles => {
                super::maybe_build_stream_local_gemini_files_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await
            }
        }
    }
}

async fn maybe_build_local_video_task_content_stream_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    if plan_kind != OPENAI_VIDEO_CONTENT_PLAN_KIND
        || decision.route_family.as_deref() != Some("openai")
    {
        return Ok(None);
    }

    let _ = state
        .hydrate_video_task_for_route(decision.route_family.as_deref(), parts.uri.path())
        .await?;

    let Some(action) = state.video_tasks.prepare_openai_content_stream_action(
        parts.uri.path(),
        parts.uri.query(),
        trace_id,
    ) else {
        return Ok(None);
    };

    let crate::video_tasks::LocalVideoTaskContentAction::StreamPlan(plan) = action else {
        return Ok(None);
    };
    let plan = *plan;

    Ok(Some(build_ai_execution_decision_from_plan(
        AiExecutionDecisionFromPlanParts {
            action: EXECUTION_RUNTIME_STREAM_DECISION_ACTION.to_string(),
            decision_kind: Some(plan_kind.to_string()),
            request_id: None,
            upstream_base_url: None,
            include_auth_pair: false,
            plan,
            report_kind: None,
            report_context: None,
            auth_context: resolve_decision_execution_runtime_auth_context(decision),
        },
    )))
}
