use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::Value;

use super::PlannerAppState;

impl<'a> PlannerAppState<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn persist_available_local_candidate(
        self,
        trace_id: &str,
        user_id: &str,
        api_key_id: &str,
        candidate: &SchedulerMinimalCandidateSelectionCandidate,
        candidate_index: u32,
        candidate_id: &str,
        extra_data: Option<Value>,
        created_at_unix_secs: u64,
        error_context: &'static str,
    ) -> String {
        crate::request_candidate_runtime::persist_available_local_candidate(
            self.app(),
            trace_id,
            user_id,
            api_key_id,
            candidate,
            candidate_index,
            candidate_id,
            extra_data,
            created_at_unix_secs,
            error_context,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn persist_skipped_local_candidate(
        self,
        trace_id: &str,
        user_id: &str,
        api_key_id: &str,
        candidate: &SchedulerMinimalCandidateSelectionCandidate,
        candidate_index: u32,
        candidate_id: &str,
        skip_reason: &str,
        finished_at_unix_secs: u64,
        error_context: &'static str,
    ) {
        crate::request_candidate_runtime::persist_skipped_local_candidate(
            self.app(),
            trace_id,
            user_id,
            api_key_id,
            candidate,
            candidate_index,
            candidate_id,
            skip_reason,
            finished_at_unix_secs,
            error_context,
        )
        .await
    }
}
