use tracing::warn;

use super::super::{
    materialize_local_openai_chat_candidate_attempts,
    maybe_build_local_openai_chat_decision_payload_for_candidate, AppState, GatewayControlDecision,
    GatewayError,
};
use super::candidates::list_local_openai_chat_candidates;
use super::diagnostic::{
    set_local_openai_chat_candidate_evaluation_diagnostic, set_local_openai_chat_miss_diagnostic,
};
use super::resolve::resolve_local_openai_chat_decision_input;
use crate::ai_pipeline::planner::common::{
    force_upstream_streaming_for_provider, OPENAI_CHAT_SYNC_PLAN_KIND,
};
use crate::ai_pipeline::planner::plan_builders::{
    build_openai_chat_sync_plan_from_decision, LocalSyncPlanAndReport,
};
use crate::ai_pipeline::planner::runtime_miss::apply_local_runtime_candidate_terminal_reason;

fn openai_chat_sync_upstream_is_stream_for_candidate(
    provider_type: &str,
    provider_api_format: &str,
) -> bool {
    force_upstream_streaming_for_provider(provider_type, provider_api_format)
}

pub(crate) async fn build_local_openai_chat_sync_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Vec<LocalSyncPlanAndReport>, GatewayError> {
    if plan_kind != OPENAI_CHAT_SYNC_PLAN_KIND {
        return Ok(Vec::new());
    }

    let Some(input) = resolve_local_openai_chat_decision_input(
        state, trace_id, decision, body_json, plan_kind, true,
    )
    .await
    else {
        return Ok(Vec::new());
    };

    let (candidates, skipped_candidates) =
        match list_local_openai_chat_candidates(state, &input, false).await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    error = ?err,
                    "gateway local openai chat sync decision scheduler selection failed"
                );
                set_local_openai_chat_miss_diagnostic(
                    state,
                    trace_id,
                    decision,
                    plan_kind,
                    Some(input.requested_model.as_str()),
                    "scheduler_selection_failed",
                );
                return Ok(Vec::new());
            }
        };
    if candidates.is_empty() && skipped_candidates.is_empty() {
        set_local_openai_chat_candidate_evaluation_diagnostic(
            state,
            trace_id,
            decision,
            plan_kind,
            Some(input.requested_model.as_str()),
            0,
        );
        return Ok(Vec::new());
    }
    set_local_openai_chat_candidate_evaluation_diagnostic(
        state,
        trace_id,
        decision,
        plan_kind,
        Some(input.requested_model.as_str()),
        candidates.len() + skipped_candidates.len(),
    );

    let attempts = materialize_local_openai_chat_candidate_attempts(
        state,
        trace_id,
        &input,
        body_json,
        candidates,
        skipped_candidates,
    )
    .await;

    let mut plans = Vec::new();
    for attempt in attempts {
        let upstream_is_stream = openai_chat_sync_upstream_is_stream_for_candidate(
            attempt.eligible.transport.provider.provider_type.as_str(),
            attempt.eligible.provider_api_format.as_str(),
        );
        let Some(payload) = maybe_build_local_openai_chat_decision_payload_for_candidate(
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
        .await
        else {
            continue;
        };

        match build_openai_chat_sync_plan_from_decision(parts, body_json, payload) {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    error = ?err,
                    "gateway local openai chat sync decision plan build failed"
                );
            }
        }
    }

    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_sync_plans");

    Ok(plans)
}

#[cfg(test)]
mod tests {
    use super::openai_chat_sync_upstream_is_stream_for_candidate;

    #[test]
    fn openai_chat_sync_forces_streaming_for_codex_openai_cli_candidates() {
        assert!(openai_chat_sync_upstream_is_stream_for_candidate(
            "codex",
            "openai:cli"
        ));
        assert!(!openai_chat_sync_upstream_is_stream_for_candidate(
            "openai",
            "openai:cli"
        ));
        assert!(!openai_chat_sync_upstream_is_stream_for_candidate(
            "codex",
            "openai:chat"
        ));
    }
}
