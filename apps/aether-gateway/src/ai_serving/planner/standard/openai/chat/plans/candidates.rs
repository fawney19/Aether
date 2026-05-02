use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use super::super::{GatewayError, LocalOpenAiChatDecisionInput};
use crate::ai_serving::planner::candidate_resolution::SkippedLocalExecutionCandidate;
use crate::ai_serving::planner::candidate_source::{
    preselect_local_execution_candidates_with_serving, LocalCandidatePreselectionKeyMode,
};
use crate::ai_serving::PlannerAppState;
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
    let outcome = preselect_local_execution_candidates_with_serving(
        PlannerAppState::new(state),
        "openai:chat",
        &input.requested_model,
        require_streaming,
        input.required_capabilities.as_ref(),
        &input.auth_snapshot,
        false,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel,
    )
    .await?;

    Ok((outcome.candidates, outcome.skipped_candidates))
}
