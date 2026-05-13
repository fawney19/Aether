use crate::ai_serving::planner::common::{
    OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND,
};
use crate::ai_serving::planner::runtime_miss::set_local_runtime_execution_exhausted_diagnostic;
use crate::ai_serving::GatewayControlDecision;
use crate::{AiExecutionDecision, AppState, GatewayError};
use tracing::warn;

mod decision;
mod plans;

use self::decision::{
    build_lazy_local_openai_chat_candidate_attempt_source,
    maybe_build_local_openai_chat_decision_payload_for_candidate, LocalOpenAiChatCandidateAttempt,
    LocalOpenAiChatCandidateAttemptSource, LocalOpenAiChatDecisionInput,
};
use self::plans::{
    build_local_openai_chat_stream_attempt_source, build_local_openai_chat_stream_plan_and_reports,
    build_local_openai_chat_sync_attempt_source, build_local_openai_chat_sync_plan_and_reports,
    resolve_local_openai_chat_decision_input,
};

pub(crate) async fn build_local_openai_chat_sync_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Vec<crate::ai_serving::planner::plan_builders::AiSyncAttempt>, GatewayError> {
    build_local_openai_chat_sync_plan_and_reports(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await
}

pub(crate) async fn build_local_openai_chat_stream_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Vec<crate::ai_serving::planner::plan_builders::AiStreamAttempt>, GatewayError> {
    build_local_openai_chat_stream_plan_and_reports(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await
}

pub(crate) async fn build_local_openai_chat_sync_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    plan_kind: &str,
) -> Result<
    Option<(
        impl crate::ai_serving::planner::LocalExecutionAttemptSource<
                crate::ai_serving::planner::plan_builders::AiSyncAttempt,
            > + 'a,
        usize,
    )>,
    GatewayError,
> {
    build_local_openai_chat_sync_attempt_source(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await
}

pub(crate) async fn build_local_openai_chat_stream_attempt_source_for_kind<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    body_json: &'a serde_json::Value,
    plan_kind: &str,
) -> Result<
    Option<(
        impl crate::ai_serving::planner::LocalExecutionAttemptSource<
                crate::ai_serving::planner::plan_builders::AiStreamAttempt,
            > + 'a,
        usize,
    )>,
    GatewayError,
> {
    build_local_openai_chat_stream_attempt_source(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await
}

pub(crate) fn set_local_openai_chat_execution_exhausted_diagnostic(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    body_json: &serde_json::Value,
    plan_count: usize,
) {
    warn!(
        event_name = "local_openai_chat_candidates_exhausted",
        log_type = "event",
        trace_id = %trace_id,
        plan_kind,
        route_class = decision.route_class.as_deref().unwrap_or("passthrough"),
        route_family = decision.route_family.as_deref().unwrap_or("unknown"),
        candidate_count = plan_count,
        model = body_json.get("model").and_then(|value| value.as_str()).unwrap_or(""),
        "gateway local openai chat execution exhausted all candidates"
    );
    set_local_runtime_execution_exhausted_diagnostic(
        state,
        trace_id,
        decision,
        plan_kind,
        body_json.get("model").and_then(|value| value.as_str()),
        plan_count,
    );
}

pub(crate) async fn maybe_build_sync_local_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    if plan_kind != OPENAI_CHAT_SYNC_PLAN_KIND {
        return Ok(None);
    }

    let Some(input) = resolve_local_openai_chat_decision_input(
        state, parts, trace_id, decision, body_json, plan_kind, false,
    )
    .await
    else {
        return Ok(None);
    };

    let (mut source, _) = build_lazy_local_openai_chat_candidate_attempt_source(
        state, trace_id, &input, body_json, false,
    )
    .await;

    while let Some(attempt) = source.next_attempt().await {
        let upstream_is_stream = self::plans::openai_chat_upstream_is_stream_for_candidate(
            &attempt.eligible.transport,
            attempt.eligible.provider_api_format.as_str(),
            false,
        );
        if let Some(payload) = maybe_build_local_openai_chat_decision_payload_for_candidate(
            state,
            parts,
            trace_id,
            body_json,
            &input,
            attempt,
            OPENAI_CHAT_SYNC_PLAN_KIND,
            "openai_chat_sync_success",
            upstream_is_stream,
        )
        .await?
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

pub(crate) async fn maybe_build_stream_local_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    if plan_kind != OPENAI_CHAT_STREAM_PLAN_KIND {
        return Ok(None);
    }

    let Some(input) = resolve_local_openai_chat_decision_input(
        state, parts, trace_id, decision, body_json, plan_kind, false,
    )
    .await
    else {
        return Ok(None);
    };

    let (mut source, _) = build_lazy_local_openai_chat_candidate_attempt_source(
        state, trace_id, &input, body_json, true,
    )
    .await;

    while let Some(attempt) = source.next_attempt().await {
        let upstream_is_stream = self::plans::openai_chat_upstream_is_stream_for_candidate(
            &attempt.eligible.transport,
            attempt.eligible.provider_api_format.as_str(),
            true,
        );
        if let Some(payload) = maybe_build_local_openai_chat_decision_payload_for_candidate(
            state,
            parts,
            trace_id,
            body_json,
            &input,
            attempt,
            OPENAI_CHAT_STREAM_PLAN_KIND,
            "openai_chat_stream_success",
            upstream_is_stream,
        )
        .await?
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}
