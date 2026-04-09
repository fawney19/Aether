use tracing::warn;

use super::super::{
    materialize_local_openai_chat_candidate_attempts,
    maybe_build_local_openai_chat_decision_payload_for_candidate, AppState, GatewayControlDecision,
    GatewayError, LocalExecutionRuntimeMissDiagnostic,
};
use super::candidates::list_local_openai_chat_candidates;
use super::diagnostic::{
    build_local_openai_chat_miss_diagnostic, set_local_openai_chat_miss_diagnostic,
};
use super::resolve::resolve_local_openai_chat_decision_input;
use crate::ai_pipeline::planner::common::OPENAI_CHAT_SYNC_PLAN_KIND;
use crate::ai_pipeline::planner::plan_builders::{
    build_openai_chat_sync_plan_from_decision, LocalSyncPlanAndReport,
};

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

    let candidates = match list_local_openai_chat_candidates(state, &input, false).await {
        Ok(candidates) => candidates,
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
    if candidates.is_empty() {
        state.set_local_execution_runtime_miss_diagnostic(
            trace_id,
            LocalExecutionRuntimeMissDiagnostic {
                candidate_count: Some(0),
                ..build_local_openai_chat_miss_diagnostic(
                    decision,
                    plan_kind,
                    Some(input.requested_model.as_str()),
                    "candidate_list_empty",
                )
            },
        );
        return Ok(Vec::new());
    }
    state.set_local_execution_runtime_miss_diagnostic(
        trace_id,
        LocalExecutionRuntimeMissDiagnostic {
            candidate_count: Some(candidates.len()),
            ..build_local_openai_chat_miss_diagnostic(
                decision,
                plan_kind,
                Some(input.requested_model.as_str()),
                "candidate_evaluation_incomplete",
            )
        },
    );

    let attempts =
        materialize_local_openai_chat_candidate_attempts(state, trace_id, &input, candidates).await;

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_openai_chat_decision_payload_for_candidate(
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

    state.mutate_local_execution_runtime_miss_diagnostic(trace_id, |diagnostic| {
        let candidate_count = diagnostic.candidate_count.unwrap_or(0);
        let skipped_candidate_count = diagnostic.skipped_candidate_count.unwrap_or(0);
        diagnostic.reason = if candidate_count > 0 && skipped_candidate_count >= candidate_count {
            "all_candidates_skipped".to_string()
        } else {
            "no_local_sync_plans".to_string()
        };
    });

    Ok(plans)
}
