use serde_json::Value;
use tracing::warn;

use crate::ai_serving::planner::common::{
    OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND,
};
use crate::ai_serving::planner::runtime_miss::set_local_runtime_execution_exhausted_diagnostic;
use crate::ai_serving::GatewayControlDecision;
use crate::{AiExecutionDecision, AppState, GatewayError};

mod decision;
mod plans;

use self::decision::{
    materialize_local_openai_chat_candidate_attempts,
    maybe_build_local_openai_chat_decision_payload_for_candidate, LocalOpenAiChatDecisionInput,
};
use self::plans::{
    build_local_openai_chat_stream_plan_and_reports, build_local_openai_chat_sync_plan_and_reports,
    list_local_openai_chat_candidates, resolve_local_openai_chat_decision_input,
    set_local_openai_chat_miss_diagnostic,
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
        state, trace_id, decision, body_json, plan_kind, false,
    )
    .await
    else {
        return Ok(None);
    };

    let (candidates, skipped_candidates) =
        match list_local_openai_chat_candidates(state, &input, false).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    event_name = "local_openai_chat_scheduler_selection_failed",
                    log_type = "event",
                    trace_id = %trace_id,
                    error = ?err,
                    "gateway local openai chat sync decision scheduler selection failed"
                );
                return Ok(None);
            }
        };

    let attempts = materialize_local_openai_chat_candidate_attempts(
        state,
        trace_id,
        &input,
        body_json,
        candidates,
        skipped_candidates,
    )
    .await;

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_openai_chat_decision_payload_for_candidate(
            state,
            parts,
            trace_id,
            body_json,
            &input,
            attempt,
            OPENAI_CHAT_SYNC_PLAN_KIND,
            "openai_chat_sync_success",
            false,
        )
        .await
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
        state, trace_id, decision, body_json, plan_kind, false,
    )
    .await
    else {
        return Ok(None);
    };

    let (candidates, skipped_candidates) =
        match list_local_openai_chat_candidates(state, &input, true).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    event_name = "local_openai_chat_scheduler_selection_failed",
                    log_type = "event",
                    trace_id = %trace_id,
                    error = ?err,
                    "gateway local openai chat stream decision scheduler selection failed"
                );
                return Ok(None);
            }
        };

    let attempts = materialize_local_openai_chat_candidate_attempts(
        state,
        trace_id,
        &input,
        body_json,
        candidates,
        skipped_candidates,
    )
    .await;

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_openai_chat_decision_payload_for_candidate(
            state,
            parts,
            trace_id,
            body_json,
            &input,
            attempt,
            OPENAI_CHAT_STREAM_PLAN_KIND,
            "openai_chat_stream_success",
            true,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}
