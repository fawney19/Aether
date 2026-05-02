use aether_ai_serving::{
    build_ai_execution_decision_from_plan, infer_ai_upstream_base_url, run_ai_sync_decision_path,
    AiExecutionDecisionFromPlanParts, AiSyncDecisionPathPort, AiSyncDecisionStep,
};
use async_trait::async_trait;
use tracing::debug;

use crate::ai_serving::planner::common::{
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_FILES_DELETE_PLAN_KIND,
    GEMINI_FILES_GET_PLAN_KIND, GEMINI_FILES_LIST_PLAN_KIND, GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::ai_serving::planner::route::resolve_execution_runtime_sync_plan_kind;
use crate::ai_serving::{
    build_execution_runtime_auth_context, resolve_execution_runtime_auth_context,
    GatewayControlDecision,
};
use crate::{AiExecutionDecision, AppState, GatewayError};

pub(crate) async fn maybe_build_sync_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(plan_kind) = resolve_execution_runtime_sync_plan_kind(parts, decision) else {
        return Ok(None);
    };

    let port = GatewaySyncDecisionPathPort {
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64,
        body_is_empty,
        plan_kind,
    };

    run_ai_sync_decision_path(&port).await
}

struct GatewaySyncDecisionPathPort<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    body_base64: Option<&'a str>,
    body_is_empty: bool,
    plan_kind: &'a str,
}

#[async_trait]
impl AiSyncDecisionPathPort for GatewaySyncDecisionPathPort<'_> {
    type Decision = AiExecutionDecision;
    type Error = GatewayError;

    fn sync_decision_step_enabled(&self, step: AiSyncDecisionStep) -> bool {
        if step == AiSyncDecisionStep::LocalGeminiFiles {
            return matches!(
                self.plan_kind,
                GEMINI_FILES_LIST_PLAN_KIND
                    | GEMINI_FILES_GET_PLAN_KIND
                    | GEMINI_FILES_DELETE_PLAN_KIND
            );
        }
        true
    }

    async fn build_sync_decision_step(
        &self,
        step: AiSyncDecisionStep,
    ) -> Result<Option<Self::Decision>, Self::Error> {
        match step {
            AiSyncDecisionStep::VideoTaskFollowUp => {
                maybe_build_local_video_task_follow_up_sync_decision_payload(
                    self.state,
                    self.parts,
                    self.body_json,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await
            }
            AiSyncDecisionStep::LocalVideo => {
                super::maybe_build_sync_local_video_decision_payload(
                    self.state,
                    self.parts,
                    self.body_json,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await
            }
            AiSyncDecisionStep::LocalImage => {
                super::maybe_build_sync_local_image_decision_payload(
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
            AiSyncDecisionStep::LocalOpenAiChat => {
                super::maybe_build_sync_local_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await
            }
            AiSyncDecisionStep::LocalOpenAiResponses => {
                super::maybe_build_sync_local_openai_responses_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await
            }
            AiSyncDecisionStep::LocalStandardFamily => {
                super::maybe_build_sync_local_standard_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await
            }
            AiSyncDecisionStep::LocalSameFormatProvider => {
                super::maybe_build_sync_local_same_format_provider_decision_payload(
                    self.state,
                    self.parts,
                    self.trace_id,
                    self.decision,
                    self.body_json,
                    self.plan_kind,
                )
                .await
            }
            AiSyncDecisionStep::LocalGeminiFiles => {
                super::maybe_build_sync_local_gemini_files_decision_payload(
                    self.state,
                    self.parts,
                    self.body_json,
                    self.body_base64,
                    self.body_is_empty,
                    self.trace_id,
                    self.decision,
                    self.plan_kind,
                )
                .await
            }
        }
    }
}

async fn maybe_build_local_video_task_follow_up_sync_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    if !matches!(
        plan_kind,
        OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND
            | OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND
            | OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND
            | GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND
    ) {
        return Ok(None);
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
    let Some(auth_context) = auth_context else {
        return Ok(None);
    };
    let Some(follow_up) = state.video_tasks.prepare_follow_up_sync_plan(
        plan_kind,
        parts.uri.path(),
        Some(body_json),
        Some(&auth_context),
        trace_id,
    ) else {
        return Ok(None);
    };

    let aether_video_tasks_core::LocalVideoTaskFollowUpPlan {
        plan,
        report_kind,
        report_context,
    } = follow_up;
    let upstream_base_url = infer_ai_upstream_base_url(&plan.url);

    debug!(
        event_name = "local_video_follow_up_sync_decision_payload_built",
        log_type = "debug",
        trace_id = %trace_id,
        request_id = %trace_id,
        candidate_id = ?plan.candidate_id,
        provider_id = %plan.provider_id,
        endpoint_id = %plan.endpoint_id,
        key_id = %plan.key_id,
        plan_kind,
        downstream_path = %parts.uri.path(),
        provider_api_format = %plan.provider_api_format,
        client_api_format = %plan.client_api_format,
        upstream_base_url = ?upstream_base_url,
        upstream_url = %plan.url,
        "gateway built local video follow-up sync decision payload"
    );

    Ok(Some(build_ai_execution_decision_from_plan(
        AiExecutionDecisionFromPlanParts {
            action: EXECUTION_RUNTIME_SYNC_DECISION_ACTION.to_string(),
            decision_kind: Some(plan_kind.to_string()),
            request_id: Some(trace_id.to_string()),
            upstream_base_url,
            include_auth_pair: true,
            plan,
            report_kind,
            report_context,
            auth_context: Some(build_execution_runtime_auth_context(&auth_context)),
        },
    )))
}
