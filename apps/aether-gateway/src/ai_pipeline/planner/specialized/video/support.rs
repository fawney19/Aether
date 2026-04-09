use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

use super::{LocalVideoCreateFamily, LocalVideoCreateSpec};
use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::planner::candidate_affinity::prefer_local_tunnel_owner_candidates;
use crate::ai_pipeline::{
    resolve_local_decision_execution_runtime_auth_context, GatewayControlDecision,
};
use crate::ai_pipeline::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_secs;
use crate::AppState;

#[derive(Debug, Clone)]
pub(super) struct LocalVideoCreateDecisionInput {
    pub(super) auth_context: ExecutionRuntimeAuthContext,
    pub(super) requested_model: String,
    pub(super) auth_snapshot: GatewayAuthApiKeySnapshot,
}

#[derive(Debug, Clone)]
pub(super) struct LocalVideoCreateCandidateAttempt {
    pub(super) candidate: SchedulerMinimalCandidateSelectionCandidate,
    pub(super) candidate_index: u32,
    pub(super) candidate_id: String,
}

pub(super) async fn resolve_local_video_create_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalVideoCreateSpec,
) -> Option<LocalVideoCreateDecisionInput> {
    let planner_state = PlannerAppState::new(state);
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return None;
    };

    let requested_model = match spec.family {
        LocalVideoCreateFamily::OpenAi => body_json
            .get("model")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)?,
        LocalVideoCreateFamily::Gemini => extract_gemini_video_model_from_path(parts.uri.path())?,
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
                decision_kind = spec.decision_kind,
                error = ?err,
                "gateway local video decision auth snapshot read failed"
            );
            return None;
        }
    };

    Some(LocalVideoCreateDecisionInput {
        auth_context,
        requested_model,
        auth_snapshot,
    })
}

pub(super) async fn list_local_video_create_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalVideoCreateDecisionInput,
    api_format: &str,
    decision_kind: &str,
) -> Option<Vec<LocalVideoCreateCandidateAttempt>> {
    let planner_state = PlannerAppState::new(state);
    let candidates = match planner_state
        .list_selectable_candidates(
            api_format,
            &input.requested_model,
            false,
            Some(&input.auth_snapshot),
            current_unix_secs(),
        )
        .await
    {
        Ok(candidates) => candidates,
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                decision_kind = decision_kind,
                error = ?err,
                "gateway local video decision scheduler selection failed"
            );
            return None;
        }
    };

    Some(
        materialize_local_video_create_candidate_attempts(
            planner_state,
            trace_id,
            input,
            candidates,
            api_format,
        )
        .await,
    )
}

async fn materialize_local_video_create_candidate_attempts(
    state: PlannerAppState<'_>,
    trace_id: &str,
    input: &LocalVideoCreateDecisionInput,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    api_format: &str,
) -> Vec<LocalVideoCreateCandidateAttempt> {
    let candidates = prefer_local_tunnel_owner_candidates(state, candidates).await;
    let created_at_unix_secs = current_unix_secs();
    let mut attempts = Vec::with_capacity(candidates.len());

    for (candidate_index, candidate) in candidates.into_iter().enumerate() {
        let generated_candidate_id = Uuid::new_v4().to_string();
        let extra_data = json!({
            "provider_api_format": api_format,
            "client_api_format": api_format,
            "global_model_id": candidate.global_model_id.clone(),
            "global_model_name": candidate.global_model_name.clone(),
            "model_id": candidate.model_id.clone(),
            "selected_provider_model_name": candidate.selected_provider_model_name.clone(),
            "mapping_matched_model": candidate.mapping_matched_model.clone(),
            "provider_name": candidate.provider_name.clone(),
            "key_name": candidate.key_name.clone(),
        });

        let candidate_id = state
            .persist_available_local_candidate(
                trace_id,
                &input.auth_context.user_id,
                &input.auth_context.api_key_id,
                &candidate,
                candidate_index as u32,
                &generated_candidate_id,
                Some(extra_data),
                created_at_unix_secs,
                "gateway local video decision request candidate upsert failed",
            )
            .await;

        attempts.push(LocalVideoCreateCandidateAttempt {
            candidate,
            candidate_index: candidate_index as u32,
            candidate_id,
        });
    }

    attempts
}

pub(super) async fn mark_skipped_local_video_candidate(
    state: &AppState,
    input: &LocalVideoCreateDecisionInput,
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
            "gateway local video decision failed to persist skipped candidate",
        )
        .await;
}

fn extract_gemini_video_model_from_path(path: &str) -> Option<String> {
    let suffix = path.strip_prefix("/v1beta/models/")?;
    let model = suffix.split(':').next()?.trim();
    if model.is_empty() {
        return None;
    }
    Some(model.to_string())
}
