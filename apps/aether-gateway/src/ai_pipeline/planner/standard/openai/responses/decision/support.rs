use std::collections::BTreeSet;

use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use tracing::warn;

use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::conversion::{request_candidate_api_formats, request_conversion_kind};
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
use crate::ai_pipeline::planner::candidate_source::auth_snapshot_allows_cross_format_candidate;
use crate::ai_pipeline::planner::common::extract_standard_requested_model;
use crate::ai_pipeline::planner::decision_input::{
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_pipeline::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_pipeline::planner::runtime_miss::set_local_runtime_miss_diagnostic_reason;
use crate::ai_pipeline::planner::spec_metadata::local_openai_responses_spec_metadata;
use crate::ai_pipeline::planner::CandidateFailureDiagnostic;
use crate::ai_pipeline::PlannerAppState;
use crate::ai_pipeline::{
    resolve_local_decision_execution_runtime_auth_context, ConversionMode, ExecutionStrategy,
    GatewayControlDecision,
};
use crate::clock::current_unix_secs;
use crate::{AppState, GatewayError};

use super::LocalOpenAiResponsesSpec;

pub(crate) use crate::ai_pipeline::planner::candidate_materialization::LocalExecutionCandidateAttempt as LocalOpenAiResponsesCandidateAttempt;
pub(crate) use crate::ai_pipeline::planner::decision_input::LocalRequestedModelDecisionInput as LocalOpenAiResponsesDecisionInput;

pub(crate) async fn resolve_local_openai_responses_decision_input(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Option<LocalOpenAiResponsesDecisionInput> {
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        warn!(
            trace_id = %trace_id,
            route_class = ?decision.route_class,
            route_family = ?decision.route_family,
            route_kind = ?decision.route_kind,
            "gateway local openai responses decision skipped: missing_auth_context"
        );
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            plan_kind,
            extract_standard_requested_model(body_json).as_deref(),
            "missing_auth_context",
        );
        return None;
    };

    let Some(requested_model) = extract_standard_requested_model(body_json) else {
        warn!(
            trace_id = %trace_id,
            "gateway local openai responses decision skipped: missing_requested_model"
        );
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            plan_kind,
            None,
            "missing_requested_model",
        );
        return None;
    };

    let resolved_input = match resolve_local_authenticated_decision_input(
        state,
        auth_context.clone(),
        Some(requested_model.as_str()),
        None,
    )
    .await
    {
        Ok(Some(resolved_input)) => resolved_input,
        Ok(None) => {
            warn!(
                trace_id = %trace_id,
                user_id = %auth_context.user_id,
                api_key_id = %auth_context.api_key_id,
                "gateway local openai responses decision skipped: auth_snapshot_missing"
            );
            set_local_runtime_miss_diagnostic_reason(
                state,
                trace_id,
                decision,
                plan_kind,
                Some(requested_model.as_str()),
                "auth_snapshot_missing",
            );
            return None;
        }
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                error = ?err,
                "gateway local openai responses decision auth snapshot read failed"
            );
            set_local_runtime_miss_diagnostic_reason(
                state,
                trace_id,
                decision,
                plan_kind,
                Some(requested_model.as_str()),
                "auth_snapshot_read_failed",
            );
            return None;
        }
    };

    Some(build_local_requested_model_decision_input(
        resolved_input,
        requested_model,
    ))
}

pub(crate) async fn materialize_local_openai_responses_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalOpenAiResponsesDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalOpenAiResponsesSpec,
) -> Result<(Vec<LocalOpenAiResponsesCandidateAttempt>, usize), GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let client_api_format = spec_metadata.api_format.trim().to_ascii_lowercase();
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
    );
    let mut seen_candidates = BTreeSet::new();
    let mut seen_skipped_candidates = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut preselection_skipped = Vec::new();
    for candidate_api_format in
        request_candidate_api_formats(spec_metadata.api_format, spec_metadata.require_streaming)
    {
        let auth_snapshot =
            if api_format_alias_matches(candidate_api_format, spec_metadata.api_format) {
                Some(&input.auth_snapshot)
            } else {
                None
            };
        let (mut selected_candidates, skipped_candidates) = planner_state
            .list_selectable_candidates_with_skip_reasons(
                candidate_api_format,
                &input.requested_model,
                spec_metadata.require_streaming,
                input.required_capabilities.as_ref(),
                auth_snapshot,
                current_unix_secs(),
            )
            .await?;
        if auth_snapshot.is_none() {
            selected_candidates.retain(|candidate| {
                auth_snapshot_allows_cross_format_candidate(
                    &input.auth_snapshot,
                    &input.requested_model,
                    candidate,
                )
            });
        }
        for skipped_candidate in skipped_candidates {
            if auth_snapshot.is_none()
                && !auth_snapshot_allows_cross_format_candidate(
                    &input.auth_snapshot,
                    &input.requested_model,
                    &skipped_candidate.candidate,
                )
            {
                continue;
            }
            let candidate_key = format!(
                "{}:{}:{}:{}:{}:{}",
                skipped_candidate.candidate.provider_id,
                skipped_candidate.candidate.endpoint_id,
                skipped_candidate.candidate.key_id,
                skipped_candidate.candidate.model_id,
                skipped_candidate.candidate.selected_provider_model_name,
                skipped_candidate.candidate.endpoint_api_format,
            );
            if seen_skipped_candidates.insert(candidate_key) {
                preselection_skipped.push(SkippedLocalExecutionCandidate {
                    candidate: skipped_candidate.candidate,
                    skip_reason: skipped_candidate.skip_reason,
                    transport: None,
                    ranking: None,
                    extra_data: None,
                });
            }
        }
        for candidate in selected_candidates {
            let candidate_key = format!(
                "{}:{}:{}:{}:{}:{}",
                candidate.provider_id,
                candidate.endpoint_id,
                candidate.key_id,
                candidate.model_id,
                candidate.selected_provider_model_name,
                candidate.endpoint_api_format,
            );
            if seen_candidates.insert(candidate_key) {
                candidates.push(candidate);
            }
        }
    }
    let (candidates, skipped_candidates) = resolve_and_rank_local_execution_candidates(
        planner_state,
        candidates,
        spec_metadata.api_format,
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
            let execution_strategy = if provider_api_format == client_api_format {
                ExecutionStrategy::LocalSameFormat
            } else {
                ExecutionStrategy::LocalCrossFormat
            };
            let conversion_mode =
                if request_conversion_kind(spec_metadata.api_format, provider_api_format.as_str())
                    .is_some()
                {
                    ConversionMode::Bidirectional
                } else {
                    ConversionMode::None
                };
            skipped_candidate.extra_data = Some(
                build_local_execution_candidate_contract_metadata_for_candidate(
                    &skipped_candidate.candidate,
                    skipped_candidate.transport_ref(),
                    provider_api_format.as_str(),
                    spec_metadata.api_format,
                    serde_json::Map::new(),
                    execution_strategy,
                    conversion_mode,
                    provider_api_format.as_str(),
                ),
            );
            skipped_candidate
        })
        .collect::<Vec<_>>();

    let candidate_count = candidates.len() + skipped_candidates.len();

    remember_first_local_candidate_affinity(
        planner_state,
        Some(&input.auth_snapshot),
        spec_metadata.api_format,
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
            let execution_strategy = if provider_api_format == client_api_format {
                ExecutionStrategy::LocalSameFormat
            } else {
                ExecutionStrategy::LocalCrossFormat
            };
            let conversion_mode =
                if request_conversion_kind(spec_metadata.api_format, provider_api_format.as_str())
                    .is_some()
                {
                    ConversionMode::Bidirectional
                } else {
                    ConversionMode::None
                };
            Some(build_local_execution_candidate_contract_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: provider_api_format.as_str(),
                    client_api_format: spec_metadata.api_format,
                    extra_fields: serde_json::Map::new(),
                },
                execution_strategy,
                conversion_mode,
                eligible.candidate.endpoint_api_format.as_str(),
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

    Ok((attempts, candidate_count))
}

fn api_format_alias_matches(left: &str, right: &str) -> bool {
    crate::ai_pipeline::legacy_openai_format_alias_matches(left, right)
}
pub(crate) async fn mark_skipped_local_openai_responses_candidate(
    state: &AppState,
    input: &LocalOpenAiResponsesDecisionInput,
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
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
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
pub(crate) async fn mark_skipped_local_openai_responses_candidate_with_extra_data(
    state: &AppState,
    input: &LocalOpenAiResponsesDecisionInput,
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
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
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
pub(crate) async fn mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
    state: &AppState,
    input: &LocalOpenAiResponsesDecisionInput,
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
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
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
