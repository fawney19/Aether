use std::collections::BTreeSet;

use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use super::super::{GatewayError, LocalOpenAiChatDecisionInput};
use crate::ai_pipeline::conversion::request_candidate_api_formats;
use crate::ai_pipeline::planner::candidate_resolution::SkippedLocalExecutionCandidate;
use crate::ai_pipeline::planner::candidate_source::auth_snapshot_allows_cross_format_candidate;
use crate::ai_pipeline::PlannerAppState;
use crate::clock::current_unix_secs;
use crate::AppState;

pub(crate) async fn list_local_openai_chat_candidates(
    state: &AppState,
    input: &LocalOpenAiChatDecisionInput,
    require_streaming: bool,
) -> Result<
    (
        Vec<SchedulerMinimalCandidateSelectionCandidate>,
        Vec<SkippedLocalExecutionCandidate>,
    ),
    GatewayError,
> {
    let planner_state = PlannerAppState::new(state);
    let now_unix_secs = current_unix_secs();
    let mut combined = Vec::new();
    let mut seen = BTreeSet::new();
    let mut skipped = Vec::new();
    let mut seen_skipped = BTreeSet::new();

    let api_formats = request_candidate_api_formats("openai:chat", require_streaming);

    for api_format in api_formats {
        let auth_snapshot = if api_format == "openai:chat" {
            Some(&input.auth_snapshot)
        } else {
            None
        };
        let (mut candidates, skipped_candidates) = planner_state
            .list_selectable_candidates_with_skip_reasons(
                api_format,
                &input.requested_model,
                require_streaming,
                input.required_capabilities.as_ref(),
                auth_snapshot,
                now_unix_secs,
            )
            .await?;
        if api_format != "openai:chat" {
            candidates.retain(|candidate| {
                auth_snapshot_allows_cross_format_candidate(
                    &input.auth_snapshot,
                    &input.requested_model,
                    candidate,
                )
            });
        }
        for skipped_candidate in skipped_candidates {
            if api_format != "openai:chat"
                && !auth_snapshot_allows_cross_format_candidate(
                    &input.auth_snapshot,
                    &input.requested_model,
                    &skipped_candidate.candidate,
                )
            {
                continue;
            }
            let candidate_key = format!(
                "{}:{}:{}:{}:{}",
                skipped_candidate.candidate.provider_id,
                skipped_candidate.candidate.endpoint_id,
                skipped_candidate.candidate.key_id,
                skipped_candidate.candidate.model_id,
                skipped_candidate.candidate.selected_provider_model_name,
            );
            if seen_skipped.insert(candidate_key) {
                skipped.push(SkippedLocalExecutionCandidate {
                    candidate: skipped_candidate.candidate,
                    skip_reason: skipped_candidate.skip_reason,
                    transport: None,
                    ranking: None,
                    extra_data: None,
                });
            }
        }
        for candidate in candidates {
            let candidate_key = format!(
                "{}:{}:{}:{}:{}",
                candidate.provider_id,
                candidate.endpoint_id,
                candidate.key_id,
                candidate.model_id,
                candidate.selected_provider_model_name,
            );
            if seen.insert(candidate_key) {
                combined.push(candidate);
            }
        }
    }

    Ok((combined, skipped))
}
