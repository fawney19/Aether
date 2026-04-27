use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::planner::candidate_materialization::{
    mark_skipped_local_execution_candidate, mark_skipped_local_execution_candidate_with_extra_data,
    mark_skipped_local_execution_candidate_with_failure_diagnostic,
    persist_available_local_execution_candidates_with_context,
    persist_skipped_local_execution_candidates_with_context,
    remember_first_local_candidate_affinity,
};
use crate::ai_pipeline::planner::candidate_metadata::{
    build_local_execution_candidate_contract_metadata,
    build_local_execution_candidate_contract_metadata_for_candidate,
    LocalExecutionCandidateMetadataParts,
};
use crate::ai_pipeline::planner::candidate_resolution::{
    extract_pool_sticky_session_token, resolve_and_rank_local_execution_candidates,
    SkippedLocalExecutionCandidate,
};
use crate::ai_pipeline::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_pipeline::planner::CandidateFailureDiagnostic;
use crate::ai_pipeline::{ConversionMode, ExecutionStrategy, PlannerAppState};
use crate::AppState;

pub(crate) use crate::ai_pipeline::planner::candidate_materialization::LocalExecutionCandidateAttempt as LocalOpenAiChatCandidateAttempt;
pub(crate) use crate::ai_pipeline::planner::decision_input::LocalRequestedModelDecisionInput as LocalOpenAiChatDecisionInput;

pub(crate) async fn mark_skipped_local_openai_chat_candidate(
    state: &AppState,
    input: &LocalOpenAiChatDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiChatDecision,
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn mark_skipped_local_openai_chat_candidate_with_extra_data(
    state: &AppState,
    input: &LocalOpenAiChatDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    extra_data: Option<serde_json::Value>,
) {
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiChatDecision,
    );
    mark_skipped_local_execution_candidate_with_extra_data(
        state,
        trace_id,
        persistence_policy.skipped,
        candidate,
        candidate_index,
        candidate_id,
        skip_reason,
        extra_data,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
    state: &AppState,
    input: &LocalOpenAiChatDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    diagnostic: CandidateFailureDiagnostic,
) {
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiChatDecision,
    );
    mark_skipped_local_execution_candidate_with_failure_diagnostic(
        state,
        trace_id,
        persistence_policy.skipped,
        candidate,
        candidate_index,
        candidate_id,
        skip_reason,
        diagnostic,
    )
    .await;
}

pub(crate) async fn materialize_local_openai_chat_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalOpenAiChatDecisionInput,
    body_json: &serde_json::Value,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    preselection_skipped: Vec<SkippedLocalExecutionCandidate>,
) -> Vec<LocalOpenAiChatCandidateAttempt> {
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiChatDecision,
    );
    let (candidates, skipped_candidates) = resolve_and_rank_local_execution_candidates(
        planner_state,
        candidates,
        "openai:chat",
        &input.requested_model,
        Some(&input.auth_snapshot),
        input.required_capabilities.as_ref(),
        sticky_session_token.as_deref(),
    )
    .await;
    let skipped_candidates = preselection_skipped
        .into_iter()
        .chain(skipped_candidates)
        .map(|mut skipped_candidate| {
            let provider_api_format = skipped_candidate
                .transport
                .as_ref()
                .map(|transport| transport.endpoint.api_format.trim().to_ascii_lowercase())
                .unwrap_or_else(|| {
                    skipped_candidate
                        .candidate
                        .endpoint_api_format
                        .trim()
                        .to_ascii_lowercase()
                });
            let (execution_strategy, conversion_mode) = if provider_api_format == "openai:chat" {
                (ExecutionStrategy::LocalSameFormat, ConversionMode::None)
            } else {
                (
                    ExecutionStrategy::LocalCrossFormat,
                    ConversionMode::Bidirectional,
                )
            };
            skipped_candidate.extra_data = Some(
                build_local_execution_candidate_contract_metadata_for_candidate(
                    &skipped_candidate.candidate,
                    skipped_candidate.transport_ref(),
                    provider_api_format.as_str(),
                    "openai:chat",
                    serde_json::Map::new(),
                    execution_strategy,
                    conversion_mode,
                    provider_api_format.as_str(),
                ),
            );
            skipped_candidate
        })
        .collect::<Vec<_>>();
    remember_first_local_candidate_affinity(
        planner_state,
        Some(&input.auth_snapshot),
        "openai:chat",
        Some(&input.requested_model),
        &candidates,
    );
    let available_candidate_count = candidates.len() as u32;
    let attempts = persist_available_local_execution_candidates_with_context(
        planner_state,
        trace_id,
        persistence_policy.available,
        candidates,
        |eligible| {
            let provider_api_format = eligible.provider_api_format.clone();
            let (execution_strategy, conversion_mode) = if provider_api_format == "openai:chat" {
                (ExecutionStrategy::LocalSameFormat, ConversionMode::None)
            } else {
                (
                    ExecutionStrategy::LocalCrossFormat,
                    ConversionMode::Bidirectional,
                )
            };
            Some(build_local_execution_candidate_contract_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: provider_api_format.as_str(),
                    client_api_format: "openai:chat",
                    extra_fields: serde_json::Map::new(),
                },
                execution_strategy,
                conversion_mode,
                eligible.candidate.endpoint_api_format.trim(),
            ))
        },
    )
    .await;

    persist_skipped_local_execution_candidates_with_context(
        state,
        trace_id,
        persistence_policy.skipped,
        available_candidate_count,
        skipped_candidates,
    )
    .await;

    attempts
}
