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
    supports_stream_execution_decision_kind, AiStreamAttempt, CLAUDE_CHAT_STREAM_PLAN_KIND,
    CLAUDE_CLI_STREAM_PLAN_KIND, GEMINI_CHAT_STREAM_PLAN_KIND, GEMINI_CLI_STREAM_PLAN_KIND,
    GEMINI_FILES_DOWNLOAD_PLAN_KIND, OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_IMAGE_STREAM_PLAN_KIND,
    OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND, OPENAI_RESPONSES_STREAM_PLAN_KIND,
    OPENAI_VIDEO_CONTENT_PLAN_KIND,
};
use crate::api::response::build_client_response_from_parts;
use crate::control::GatewayControlDecision;
use crate::stage_metrics::observe_gateway_stage_ms;
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
    let plan_kind_started_at = std::time::Instant::now();
    let Some(plan_kind) = resolve_execution_runtime_stream_plan_kind(parts, decision) else {
        observe_gateway_stage_ms(
            "frontdoor_stream_plan_kind",
            plan_kind_started_at.elapsed().as_millis() as u64,
        );
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };
    observe_gateway_stage_ms(
        "frontdoor_stream_plan_kind",
        plan_kind_started_at.elapsed().as_millis() as u64,
    );

    let parse_started_at = std::time::Instant::now();
    let Some((body_json, body_base64)) = parse_local_request_body(parts, body_bytes) else {
        observe_gateway_stage_ms(
            "frontdoor_stream_parse",
            parse_started_at.elapsed().as_millis() as u64,
        );
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };
    observe_gateway_stage_ms(
        "frontdoor_stream_parse",
        parse_started_at.elapsed().as_millis() as u64,
    );

    let match_started_at = std::time::Instant::now();
    let stream_matches =
        is_matching_stream_request(plan_kind, parts, &body_json, body_base64.as_deref());
    observe_gateway_stage_ms(
        "frontdoor_stream_match",
        match_started_at.elapsed().as_millis() as u64,
    );
    if !stream_matches {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let bypass_started_at = std::time::Instant::now();
    let bypass_cache_key =
        build_direct_plan_bypass_cache_key(plan_kind, parts, body_bytes, decision);
    let skip_direct_plan = should_skip_direct_plan(state, &bypass_cache_key);
    observe_gateway_stage_ms(
        "frontdoor_stream_bypass",
        bypass_started_at.elapsed().as_millis() as u64,
    );
    if skip_direct_plan {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    if plan_kind == OPENAI_CHAT_STREAM_PLAN_KIND
        && supports_stream_execution_decision_kind(plan_kind)
        && decision.route_family.as_deref() == Some("openai")
    {
        let fast_path_started_at = std::time::Instant::now();
        let outcome = execute_openai_chat_stream_fast_path(
            state,
            parts,
            trace_id,
            decision,
            &body_json,
            body_base64,
            plan_kind,
            bypass_cache_key,
        )
        .await;
        observe_gateway_stage_ms(
            "frontdoor_stream_fast_path_total",
            fast_path_started_at.elapsed().as_millis() as u64,
        );
        return outcome;
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

#[allow(clippy::too_many_arguments)]
async fn execute_openai_chat_stream_fast_path(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<String>,
    plan_kind: &str,
    bypass_cache_key: String,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let started_at = std::time::Instant::now();
    let local_outcome = maybe_execute_stream_via_local_decision(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?;
    observe_gateway_stage_ms(
        "stream_openai_chat_local_decision",
        started_at.elapsed().as_millis() as u64,
    );
    match local_outcome {
        LocalExecutionRequestOutcome::Responded(response) => {
            return Ok(LocalExecutionRequestOutcome::Responded(response));
        }
        LocalExecutionRequestOutcome::Exhausted(outcome) => {
            return Ok(LocalExecutionRequestOutcome::Exhausted(outcome));
        }
        LocalExecutionRequestOutcome::NoPath => {}
    }

    if let Some(response) = maybe_execute_stream_via_remote_decision(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(LocalExecutionRequestOutcome::Responded(response));
    }

    let fallback_started_at = std::time::Instant::now();
    let fallback_outcome = maybe_execute_stream_via_plan_fallback(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64,
        plan_kind,
        bypass_cache_key,
        GatewayFallbackReason::RemoteDecisionMiss,
    )
    .await?;
    observe_gateway_stage_ms(
        "frontdoor_stream_fast_path",
        fallback_started_at.elapsed().as_millis() as u64,
    );
    Ok(fallback_outcome)
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

    fn stream_execution_steps(&self) -> &'static [AiStreamExecutionStep] {
        stream_execution_steps_for_plan_kind(self.plan_kind)
    }

    async fn execute_stream_step(
        &self,
        step: AiStreamExecutionStep,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error> {
        let step_started_at = std::time::Instant::now();
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
        observe_gateway_stage_ms(
            "stream_path_step",
            step_started_at.elapsed().as_millis() as u64,
        );
        observe_gateway_stage_ms(
            stream_path_stage_name(step),
            step_started_at.elapsed().as_millis() as u64,
        );
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

const STREAM_STEPS_VIDEO_CONTENT: &[AiStreamExecutionStep] = &[
    AiStreamExecutionStep::LocalVideoContent,
    AiStreamExecutionStep::RemoteDecision,
];
const STREAM_STEPS_OPENAI_IMAGE: &[AiStreamExecutionStep] = &[
    AiStreamExecutionStep::LocalImage,
    AiStreamExecutionStep::RemoteDecision,
];
const STREAM_STEPS_OPENAI_CHAT: &[AiStreamExecutionStep] = &[
    AiStreamExecutionStep::LocalOpenAiChat,
    AiStreamExecutionStep::RemoteDecision,
];
const STREAM_STEPS_OPENAI_RESPONSES: &[AiStreamExecutionStep] = &[
    AiStreamExecutionStep::LocalOpenAiResponses,
    AiStreamExecutionStep::RemoteDecision,
];
const STREAM_STEPS_STANDARD_TEXT: &[AiStreamExecutionStep] = &[
    AiStreamExecutionStep::LocalStandardFamily,
    AiStreamExecutionStep::LocalSameFormatProvider,
    AiStreamExecutionStep::RemoteDecision,
];
const STREAM_STEPS_GEMINI_FILES: &[AiStreamExecutionStep] = &[
    AiStreamExecutionStep::LocalGeminiFiles,
    AiStreamExecutionStep::RemoteDecision,
];

fn stream_execution_steps_for_plan_kind(plan_kind: &str) -> &'static [AiStreamExecutionStep] {
    match plan_kind {
        OPENAI_VIDEO_CONTENT_PLAN_KIND => STREAM_STEPS_VIDEO_CONTENT,
        OPENAI_IMAGE_STREAM_PLAN_KIND => STREAM_STEPS_OPENAI_IMAGE,
        OPENAI_CHAT_STREAM_PLAN_KIND => STREAM_STEPS_OPENAI_CHAT,
        OPENAI_RESPONSES_STREAM_PLAN_KIND | OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND => {
            STREAM_STEPS_OPENAI_RESPONSES
        }
        CLAUDE_CHAT_STREAM_PLAN_KIND
        | CLAUDE_CLI_STREAM_PLAN_KIND
        | GEMINI_CHAT_STREAM_PLAN_KIND
        | GEMINI_CLI_STREAM_PLAN_KIND => STREAM_STEPS_STANDARD_TEXT,
        GEMINI_FILES_DOWNLOAD_PLAN_KIND => STREAM_STEPS_GEMINI_FILES,
        _ => aether_ai_serving::DEFAULT_STREAM_EXECUTION_STEPS,
    }
}

fn stream_path_stage_name(step: AiStreamExecutionStep) -> &'static str {
    match step {
        AiStreamExecutionStep::LocalVideoContent => "stream_path_step_video_content",
        AiStreamExecutionStep::LocalImage => "stream_path_step_image",
        AiStreamExecutionStep::LocalOpenAiChat => "stream_path_step_openai_chat",
        AiStreamExecutionStep::LocalOpenAiResponses => "stream_path_step_openai_responses",
        AiStreamExecutionStep::LocalStandardFamily => "stream_path_step_standard_family",
        AiStreamExecutionStep::LocalSameFormatProvider => "stream_path_step_same_format_provider",
        AiStreamExecutionStep::LocalGeminiFiles => "stream_path_step_gemini_files",
        AiStreamExecutionStep::RemoteDecision => "stream_path_step_remote_decision",
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
