use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::planner::candidate_affinity::prefer_local_tunnel_owner_candidates;
use crate::ai_pipeline::{
    resolve_local_decision_execution_runtime_auth_context, GatewayControlDecision,
};
use crate::ai_pipeline::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_secs;
use crate::{AppState, GatewayError};

pub(super) const GEMINI_FILES_CANDIDATE_API_FORMAT: &str = "gemini:chat";
pub(super) const GEMINI_FILES_CLIENT_API_FORMAT: &str = "gemini:files";
pub(super) const GEMINI_FILES_REQUIRED_CAPABILITY: &str = "gemini_files";

#[derive(Debug, Clone)]
pub(super) struct LocalGeminiFilesDecisionInput {
    pub(super) auth_context: ExecutionRuntimeAuthContext,
    pub(super) auth_snapshot: GatewayAuthApiKeySnapshot,
}

#[derive(Debug, Clone)]
pub(super) struct LocalGeminiFilesCandidateAttempt {
    pub(super) candidate: SchedulerMinimalCandidateSelectionCandidate,
    pub(super) candidate_index: u32,
    pub(super) candidate_id: String,
}

pub(super) async fn resolve_local_gemini_files_decision_input(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Option<LocalGeminiFilesDecisionInput> {
    let planner_state = PlannerAppState::new(state);
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return None;
    };

    let auth_snapshot = match planner_state
        .read_auth_api_key_snapshot(
            &auth_context.user_id,
            &auth_context.api_key_id,
            current_unix_secs(),
        )
        .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return None,
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                error = ?err,
                "gateway local gemini files decision auth snapshot read failed"
            );
            return None;
        }
    };

    Some(LocalGeminiFilesDecisionInput {
        auth_context,
        auth_snapshot,
    })
}

pub(super) async fn materialize_local_gemini_files_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalGeminiFilesDecisionInput,
) -> Result<Vec<LocalGeminiFilesCandidateAttempt>, GatewayError> {
    let planner_state = PlannerAppState::new(state);
    let candidates = planner_state
        .list_selectable_candidates_for_required_capability_without_requested_model(
            GEMINI_FILES_CANDIDATE_API_FORMAT,
            GEMINI_FILES_REQUIRED_CAPABILITY,
            false,
            Some(&input.auth_snapshot),
            current_unix_secs(),
        )
        .await?;
    let candidates = prefer_local_tunnel_owner_candidates(planner_state, candidates).await;

    let created_at_unix_secs = current_unix_secs();
    let mut attempts = Vec::with_capacity(candidates.len());
    for (candidate_index, candidate) in candidates.into_iter().enumerate() {
        let generated_candidate_id = Uuid::new_v4().to_string();
        let extra_data = json!({
            "provider_api_format": GEMINI_FILES_CLIENT_API_FORMAT,
            "client_api_format": GEMINI_FILES_CLIENT_API_FORMAT,
            "candidate_api_format": GEMINI_FILES_CANDIDATE_API_FORMAT,
            "global_model_id": candidate.global_model_id.clone(),
            "global_model_name": candidate.global_model_name.clone(),
            "model_id": candidate.model_id.clone(),
            "selected_provider_model_name": candidate.selected_provider_model_name.clone(),
            "mapping_matched_model": candidate.mapping_matched_model.clone(),
            "provider_name": candidate.provider_name.clone(),
            "key_name": candidate.key_name.clone(),
        });

        let candidate_id = planner_state
            .persist_available_local_candidate(
                trace_id,
                &input.auth_context.user_id,
                &input.auth_context.api_key_id,
                &candidate,
                candidate_index as u32,
                &generated_candidate_id,
                Some(extra_data),
                created_at_unix_secs,
                "gateway local gemini files request candidate upsert failed",
            )
            .await;

        attempts.push(LocalGeminiFilesCandidateAttempt {
            candidate,
            candidate_index: candidate_index as u32,
            candidate_id,
        });
    }

    Ok(attempts)
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
    PlannerAppState::new(state)
        .persist_skipped_local_candidate(
            trace_id,
            &input.auth_context.user_id,
            &input.auth_context.api_key_id,
            candidate,
            candidate_index,
            candidate_id,
            skip_reason,
            current_unix_secs(),
            "gateway local gemini files failed to persist skipped candidate",
        )
        .await;
}
