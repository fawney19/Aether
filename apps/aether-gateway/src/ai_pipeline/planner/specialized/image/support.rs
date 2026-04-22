use tracing::warn;

use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::planner::candidate_eligibility::{
    extract_pool_sticky_session_token, filter_and_rank_local_execution_candidates,
    SkippedLocalExecutionCandidate,
};
use crate::ai_pipeline::planner::candidate_materialization::{
    mark_skipped_local_execution_candidate,
    persist_available_local_execution_candidates_with_context,
    persist_skipped_local_execution_candidates_with_context,
    remember_first_local_candidate_affinity,
};
use crate::ai_pipeline::planner::candidate_metadata::{
    build_local_execution_candidate_metadata,
    build_local_execution_candidate_metadata_for_candidate, LocalExecutionCandidateMetadataParts,
};
use crate::ai_pipeline::planner::decision_input::{
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_pipeline::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_pipeline::planner::spec_metadata::local_openai_image_spec_metadata;
use crate::ai_pipeline::PlannerAppState;
use crate::ai_pipeline::{
    resolve_local_decision_execution_runtime_auth_context, GatewayControlDecision,
};
use crate::clock::current_unix_secs;
use crate::AppState;
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

pub(super) const OPENAI_IMAGE_DEFAULT_MODEL: &str = "gpt-image-2";

pub(super) use crate::ai_pipeline::planner::candidate_materialization::LocalExecutionCandidateAttempt as LocalOpenAiImageCandidateAttempt;
pub(super) use crate::ai_pipeline::planner::decision_input::LocalRequestedModelDecisionInput as LocalOpenAiImageDecisionInput;

pub(super) async fn resolve_local_openai_image_decision_input(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
) -> Option<LocalOpenAiImageDecisionInput> {
    let Some(auth_context) = resolve_local_openai_image_auth_context(decision) else {
        return None;
    };

    let requested_model = body_json
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(OPENAI_IMAGE_DEFAULT_MODEL)
        .to_string();

    let resolved_input = match resolve_local_authenticated_decision_input(
        state,
        auth_context,
        Some(requested_model.as_str()),
        None,
    )
    .await
    {
        Ok(Some(resolved_input)) => resolved_input,
        Ok(None) => return None,
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                error = ?err,
                "gateway local openai image decision auth snapshot read failed"
            );
            return None;
        }
    };

    Some(build_local_requested_model_decision_input(
        resolved_input,
        requested_model,
    ))
}

fn resolve_local_openai_image_auth_context(
    decision: &GatewayControlDecision,
) -> Option<ExecutionRuntimeAuthContext> {
    resolve_local_decision_execution_runtime_auth_context(decision)
}

pub(super) async fn list_local_openai_image_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    body_json: &serde_json::Value,
    api_format: &str,
    decision_kind: &str,
) -> Option<Vec<LocalOpenAiImageCandidateAttempt>> {
    let planner_state = PlannerAppState::new(state);
    let (candidates, preselection_skipped) = match planner_state
        .list_selectable_candidates_with_skip_reasons(
            api_format,
            &input.requested_model,
            false,
            input.required_capabilities.as_ref(),
            Some(&input.auth_snapshot),
            current_unix_secs(),
        )
        .await
    {
        Ok(candidates) => candidates,
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                decision_kind,
                error = ?err,
                "gateway local openai image decision scheduler selection failed"
            );
            return None;
        }
    };

    Some(
        materialize_local_openai_image_candidate_attempts(
            planner_state,
            trace_id,
            input,
            body_json,
            candidates,
            preselection_skipped
                .into_iter()
                .map(|item| SkippedLocalExecutionCandidate {
                    candidate: item.candidate,
                    skip_reason: item.skip_reason,
                    transport: None,
                    extra_data: None,
                })
                .collect(),
            api_format,
        )
        .await,
    )
}

async fn materialize_local_openai_image_candidate_attempts(
    state: PlannerAppState<'_>,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    body_json: &serde_json::Value,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    preselection_skipped: Vec<SkippedLocalExecutionCandidate>,
    api_format: &str,
) -> Vec<LocalOpenAiImageCandidateAttempt> {
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::ImageDecision,
    );
    let (candidates, skipped_candidates) = filter_and_rank_local_execution_candidates(
        state,
        candidates,
        api_format,
        &input.requested_model,
        input.required_capabilities.as_ref(),
        sticky_session_token.as_deref(),
    )
    .await;
    let skipped_candidates = preselection_skipped
        .into_iter()
        .chain(skipped_candidates)
        .collect::<Vec<_>>();
    remember_first_local_candidate_affinity(
        state,
        Some(&input.auth_snapshot),
        api_format,
        Some(&input.requested_model),
        &candidates,
    );
    let available_candidate_count = candidates.len() as u32;
    let attempts = persist_available_local_execution_candidates_with_context(
        state,
        trace_id,
        persistence_policy.available,
        candidates,
        |eligible| {
            Some(build_local_execution_candidate_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: api_format,
                    client_api_format: api_format,
                    extra_fields: serde_json::Map::new(),
                },
            ))
        },
    )
    .await;

    persist_skipped_local_execution_candidates_with_context(
        state.app(),
        trace_id,
        persistence_policy.skipped,
        available_candidate_count,
        skipped_candidates
            .into_iter()
            .map(|mut skipped_candidate| {
                skipped_candidate.extra_data =
                    Some(build_local_execution_candidate_metadata_for_candidate(
                        &skipped_candidate.candidate,
                        skipped_candidate.transport_ref(),
                        api_format,
                        api_format,
                        serde_json::Map::new(),
                    ));
                skipped_candidate
            })
            .collect(),
    )
    .await;

    attempts
}

pub(super) async fn mark_skipped_local_openai_image_candidate(
    state: &AppState,
    input: &LocalOpenAiImageDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::ImageDecision,
    );
    mark_skipped_local_execution_candidate(
        state,
        trace_id,
        persistence_policy.skipped,
        candidate,
        candidate_index,
        candidate_id,
        skip_reason,
    )
    .await;
}
