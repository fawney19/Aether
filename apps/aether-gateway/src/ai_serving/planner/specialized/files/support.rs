use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::json;
use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::{
    build_local_execution_candidate_attempt_source_with_serving,
    mark_skipped_local_execution_candidate,
    mark_skipped_local_execution_candidate_with_failure_diagnostic,
    materialize_local_execution_candidates_with_serving, LocalCandidateResolutionMode,
    LocalExecutionCandidateAttemptSource,
};
use crate::ai_serving::planner::candidate_metadata::{
    build_local_execution_candidate_metadata,
    build_local_execution_candidate_metadata_for_candidate, LocalExecutionCandidateMetadataParts,
};
use crate::ai_serving::planner::decision_input::{
    attach_routing_policy_to_local_requested_model_input,
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::{
    resolve_local_decision_execution_runtime_auth_context, CandidateFailureDiagnostic,
    ExecutionRuntimeAuthContext, GatewayControlDecision, PlannerAppState,
};
use crate::client_session_affinity::client_session_affinity_from_parts;
use crate::clock::current_unix_secs;
use crate::{AppState, GatewayError};

pub(super) use crate::ai_serving::planner::candidate_materialization::LocalExecutionCandidateAttempt as LocalGeminiFilesCandidateAttempt;
pub(super) use crate::ai_serving::planner::candidate_materialization::LocalExecutionCandidateAttemptSource as LocalGeminiFilesCandidateAttemptSource;
pub(super) use crate::ai_serving::planner::decision_input::LocalRequestedModelDecisionInput as LocalGeminiFilesDecisionInput;

pub(super) const GEMINI_FILES_CANDIDATE_API_FORMAT: &str = "gemini:files";
pub(super) const GEMINI_FILES_CLIENT_API_FORMAT: &str = "gemini:files";
pub(super) const GEMINI_FILES_REQUIRED_CAPABILITY: &str = "gemini_files";
pub(super) const GEMINI_FILES_ROUTING_MODEL: &str = "gemini-files";

pub(super) async fn resolve_local_gemini_files_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: Option<&serde_json::Value>,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<Option<LocalGeminiFilesDecisionInput>, GatewayError> {
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return Ok(None);
    };

    let explicit_required_capabilities = json!({ "gemini_files": true });
    let resolved_input = match resolve_local_authenticated_decision_input(
        state,
        auth_context,
        None,
        decision.auth_endpoint_signature.as_deref(),
        Some(&explicit_required_capabilities),
        &decision.model_directive_policy,
    )
    .await
    {
        Ok(Some(resolved_input)) => resolved_input,
        Ok(None) => return Ok(None),
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                error = ?err,
                "gateway local gemini files decision auth snapshot read failed"
            );
            return Err(err);
        }
    };

    let routing_body_json = body_json.cloned().unwrap_or(serde_json::Value::Null);
    let mut input = build_local_requested_model_decision_input(
        resolved_input,
        GEMINI_FILES_ROUTING_MODEL.to_string(),
    );
    input.request_auth_channel = decision.request_auth_channel.clone();
    input.client_session_affinity = client_session_affinity_from_parts(parts, body_json);
    attach_routing_policy_to_local_requested_model_input(
        state,
        parts,
        &mut input,
        &routing_body_json,
        GEMINI_FILES_CLIENT_API_FORMAT,
    )
    .await?;
    Ok(Some(input))
}

pub(super) async fn materialize_local_gemini_files_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalGeminiFilesDecisionInput,
) -> Result<Vec<LocalGeminiFilesCandidateAttempt>, GatewayError> {
    let planner_state = PlannerAppState::new(state);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::GeminiFilesDecision,
    );
    let candidates = planner_state
        .list_selectable_candidates_for_required_capability_without_requested_model(
            GEMINI_FILES_CANDIDATE_API_FORMAT,
            GEMINI_FILES_REQUIRED_CAPABILITY,
            false,
            Some(&input.auth_snapshot),
            input.client_session_affinity.as_ref(),
            current_unix_secs(),
        )
        .await?;
    let outcome = materialize_local_execution_candidates_with_serving(
        planner_state,
        trace_id,
        GEMINI_FILES_CLIENT_API_FORMAT,
        None,
        Some(&input.auth_snapshot),
        input.client_session_affinity.as_ref(),
        input.required_capabilities.as_ref(),
        input.routing_policy.as_ref(),
        None,
        input.request_auth_channel.as_deref(),
        persistence_policy,
        candidates,
        Vec::new(),
        LocalCandidateResolutionMode::WithoutTransportPairGate,
        |eligible| {
            let mut extra_fields = serde_json::Map::new();
            extra_fields.insert(
                "candidate_api_format".to_string(),
                json!(GEMINI_FILES_CANDIDATE_API_FORMAT),
            );
            Some(build_local_execution_candidate_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: GEMINI_FILES_CLIENT_API_FORMAT,
                    client_api_format: GEMINI_FILES_CLIENT_API_FORMAT,
                    extra_fields,
                },
            ))
        },
        |mut skipped_candidate| {
            let mut extra_fields = serde_json::Map::new();
            extra_fields.insert(
                "candidate_api_format".to_string(),
                json!(GEMINI_FILES_CANDIDATE_API_FORMAT),
            );
            skipped_candidate.extra_data =
                Some(build_local_execution_candidate_metadata_for_candidate(
                    &skipped_candidate.candidate,
                    skipped_candidate.transport_ref(),
                    GEMINI_FILES_CLIENT_API_FORMAT,
                    GEMINI_FILES_CLIENT_API_FORMAT,
                    extra_fields,
                ));
            skipped_candidate
        },
    )
    .await;

    Ok(outcome.attempts)
}

pub(super) async fn build_local_gemini_files_candidate_attempt_source<'a>(
    state: &'a AppState,
    trace_id: &str,
    input: &LocalGeminiFilesDecisionInput,
) -> Result<(LocalGeminiFilesCandidateAttemptSource<'a>, usize), GatewayError> {
    let planner_state = PlannerAppState::new(state);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::GeminiFilesDecision,
    );
    let candidates = planner_state
        .list_selectable_candidates_for_required_capability_without_requested_model(
            GEMINI_FILES_CANDIDATE_API_FORMAT,
            GEMINI_FILES_REQUIRED_CAPABILITY,
            false,
            Some(&input.auth_snapshot),
            input.client_session_affinity.as_ref(),
            current_unix_secs(),
        )
        .await?;
    Ok(build_local_execution_candidate_attempt_source_with_serving(
        planner_state,
        trace_id,
        GEMINI_FILES_CLIENT_API_FORMAT,
        None,
        Some(&input.auth_snapshot),
        input.client_session_affinity.as_ref(),
        input.required_capabilities.as_ref(),
        input.routing_policy.as_ref(),
        None,
        input.request_auth_channel.as_deref(),
        persistence_policy,
        candidates,
        Vec::new(),
        LocalCandidateResolutionMode::WithoutTransportPairGate,
        |eligible| {
            let mut extra_fields = serde_json::Map::new();
            extra_fields.insert(
                "candidate_api_format".to_string(),
                json!(GEMINI_FILES_CANDIDATE_API_FORMAT),
            );
            Some(build_local_execution_candidate_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: GEMINI_FILES_CLIENT_API_FORMAT,
                    client_api_format: GEMINI_FILES_CLIENT_API_FORMAT,
                    extra_fields,
                },
            ))
        },
        |mut skipped_candidate| {
            let mut extra_fields = serde_json::Map::new();
            extra_fields.insert(
                "candidate_api_format".to_string(),
                json!(GEMINI_FILES_CANDIDATE_API_FORMAT),
            );
            skipped_candidate.extra_data =
                Some(build_local_execution_candidate_metadata_for_candidate(
                    &skipped_candidate.candidate,
                    skipped_candidate.transport_ref(),
                    GEMINI_FILES_CLIENT_API_FORMAT,
                    GEMINI_FILES_CLIENT_API_FORMAT,
                    extra_fields,
                ));
            skipped_candidate
        },
    )
    .await)
}

pub(super) async fn mark_skipped_local_gemini_files_candidate(
    state: &AppState,
    input: &LocalGeminiFilesDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::GeminiFilesDecision,
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

pub(super) async fn mark_skipped_local_gemini_files_candidate_with_failure_diagnostic(
    state: &AppState,
    input: &LocalGeminiFilesDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    diagnostic: CandidateFailureDiagnostic,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::GeminiFilesDecision,
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
