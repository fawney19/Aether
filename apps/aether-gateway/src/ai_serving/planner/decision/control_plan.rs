use crate::ai_serving::planner::common::{
    CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_STREAM_PLAN_KIND,
    CLAUDE_CLI_SYNC_PLAN_KIND, GEMINI_CHAT_STREAM_PLAN_KIND, GEMINI_CHAT_SYNC_PLAN_KIND,
    GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND, GEMINI_FILES_DELETE_PLAN_KIND,
    GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND, GEMINI_FILES_LIST_PLAN_KIND,
    GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND, GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND,
    OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND, OPENAI_IMAGE_STREAM_PLAN_KIND,
    OPENAI_IMAGE_SYNC_PLAN_KIND, OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND,
    OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND, OPENAI_RESPONSES_STREAM_PLAN_KIND,
    OPENAI_RESPONSES_SYNC_PLAN_KIND, OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CONTENT_PLAN_KIND, OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND, OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::ai_serving::planner::plan_builders::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_openai_chat_stream_plan_from_decision, build_openai_chat_sync_plan_from_decision,
    build_openai_responses_stream_plan_from_decision,
    build_openai_responses_sync_plan_from_decision, build_passthrough_stream_plan_from_decision,
    build_passthrough_sync_plan_from_decision, build_standard_stream_plan_from_decision,
    build_standard_sync_plan_from_decision,
};
use crate::ai_serving::planner::route::{
    resolve_execution_runtime_stream_plan_kind as resolve_stream_plan_kind,
    resolve_execution_runtime_sync_plan_kind as resolve_sync_plan_kind,
};
use crate::ai_serving::GatewayControlDecision;
use crate::{AiExecutionDecision, AiExecutionPlanPayload, AppState, GatewayError};
use aether_ai_serving::{
    build_ai_stream_execution_plan_payload, build_ai_sync_execution_plan_payload,
};

pub(crate) async fn maybe_build_sync_plan_payload_impl(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
) -> Result<Option<AiExecutionPlanPayload>, GatewayError> {
    let Some(plan_kind) = resolve_sync_plan_kind(parts, decision) else {
        return Ok(None);
    };
    let Some(payload) = super::maybe_build_sync_decision_payload(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64,
        body_is_empty,
    )
    .await?
    else {
        return Ok(None);
    };

    build_sync_plan_payload_from_decision(parts, body_json, plan_kind, payload)
}

pub(crate) async fn maybe_build_stream_plan_payload_impl(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
) -> Result<Option<AiExecutionPlanPayload>, GatewayError> {
    let Some(plan_kind) = resolve_stream_plan_kind(parts, decision) else {
        return Ok(None);
    };
    let Some(payload) = super::maybe_build_stream_decision_payload(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64,
    )
    .await?
    else {
        return Ok(None);
    };

    build_stream_plan_payload_from_decision(parts, body_json, plan_kind, payload)
}

fn build_sync_plan_payload_from_decision(
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    plan_kind: &str,
    mut payload: AiExecutionDecision,
) -> Result<Option<AiExecutionPlanPayload>, GatewayError> {
    let auth_context = payload.auth_context.take();
    let plan_and_report = match plan_kind {
        OPENAI_CHAT_SYNC_PLAN_KIND => {
            build_openai_chat_sync_plan_from_decision(parts, body_json, payload)?
        }
        OPENAI_RESPONSES_SYNC_PLAN_KIND => {
            build_openai_responses_sync_plan_from_decision(parts, body_json, payload, false)?
        }
        OPENAI_IMAGE_SYNC_PLAN_KIND => build_passthrough_sync_plan_from_decision(parts, payload)?,
        OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND => {
            build_openai_responses_sync_plan_from_decision(parts, body_json, payload, true)?
        }
        CLAUDE_CHAT_SYNC_PLAN_KIND | CLAUDE_CLI_SYNC_PLAN_KIND => {
            build_standard_sync_plan_from_decision(parts, body_json, payload)?
        }
        GEMINI_CHAT_SYNC_PLAN_KIND | GEMINI_CLI_SYNC_PLAN_KIND => {
            build_gemini_sync_plan_from_decision(parts, body_json, payload)?
        }
        OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND
        | OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND
        | OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND
        | OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND
        | GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND
        | GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND
        | GEMINI_FILES_LIST_PLAN_KIND
        | GEMINI_FILES_GET_PLAN_KIND
        | GEMINI_FILES_DELETE_PLAN_KIND => {
            build_passthrough_sync_plan_from_decision(parts, payload)?
        }
        _ => None,
    };

    Ok(plan_and_report
        .map(|value| build_ai_sync_execution_plan_payload(plan_kind, value, auth_context)))
}

fn build_stream_plan_payload_from_decision(
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    plan_kind: &str,
    mut payload: AiExecutionDecision,
) -> Result<Option<AiExecutionPlanPayload>, GatewayError> {
    let auth_context = payload.auth_context.take();
    let plan_and_report = match plan_kind {
        OPENAI_CHAT_STREAM_PLAN_KIND => {
            build_openai_chat_stream_plan_from_decision(parts, body_json, payload)?
        }
        OPENAI_RESPONSES_STREAM_PLAN_KIND => {
            build_openai_responses_stream_plan_from_decision(parts, body_json, payload, false)?
        }
        OPENAI_IMAGE_STREAM_PLAN_KIND => {
            build_standard_stream_plan_from_decision(parts, body_json, payload, false)?
        }
        OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND => {
            build_openai_responses_stream_plan_from_decision(parts, body_json, payload, true)?
        }
        CLAUDE_CHAT_STREAM_PLAN_KIND | CLAUDE_CLI_STREAM_PLAN_KIND => {
            build_standard_stream_plan_from_decision(parts, body_json, payload, true)?
        }
        GEMINI_CHAT_STREAM_PLAN_KIND | GEMINI_CLI_STREAM_PLAN_KIND => {
            build_gemini_stream_plan_from_decision(parts, body_json, payload)?
        }
        OPENAI_VIDEO_CONTENT_PLAN_KIND | GEMINI_FILES_DOWNLOAD_PLAN_KIND => {
            build_passthrough_stream_plan_from_decision(parts, payload)?
        }
        _ => None,
    };

    Ok(plan_and_report
        .map(|value| build_ai_stream_execution_plan_payload(plan_kind, value, auth_context)))
}
