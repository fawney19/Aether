use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use super::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::GatewayError;

impl<'a> PlannerAppState<'a> {
    pub(crate) async fn list_selectable_candidates(
        self,
        api_format: &str,
        global_model_name: &str,
        require_streaming: bool,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        now_unix_secs: u64,
    ) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
        crate::scheduler::candidate::list_selectable_candidates(
            self.app().data.as_ref(),
            self.app(),
            api_format,
            global_model_name,
            require_streaming,
            auth_snapshot,
            now_unix_secs,
        )
        .await
    }

    pub(crate) async fn list_selectable_candidates_for_required_capability_without_requested_model(
        self,
        candidate_api_format: &str,
        required_capability: &str,
        require_streaming: bool,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        now_unix_secs: u64,
    ) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
        crate::scheduler::candidate::list_selectable_candidates_for_required_capability_without_requested_model(
            self.app().data.as_ref(),
            self.app(),
            candidate_api_format,
            required_capability,
            require_streaming,
            auth_snapshot,
            now_unix_secs,
        )
        .await
    }
}
