use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::Value;

use super::PlannerAppState;

impl<'a> PlannerAppState<'a> {
    pub(crate) async fn resolve_request_candidate_required_capabilities(
        self,
        user_id: &str,
        api_key_id: &str,
        requested_model: Option<&str>,
        explicit_required_capabilities: Option<&Value>,
    ) -> Option<Value> {
        crate::request_candidate_runtime::resolve_request_candidate_required_capabilities(
            self.app(),
            user_id,
            api_key_id,
            requested_model,
            explicit_required_capabilities,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn persist_available_local_candidate(
        self,
        trace_id: &str,
        user_id: &str,
        api_key_id: &str,
        candidate: &SchedulerMinimalCandidateSelectionCandidate,
        candidate_index: u32,
        retry_index: u32,
        candidate_id: &str,
        required_capabilities: Option<&Value>,
        extra_data: Option<Value>,
        created_at_unix_ms: u64,
        error_context: &'static str,
    ) -> String {
        crate::request_candidate_runtime::persist_available_local_candidate(
            self.app(),
            trace_id,
            user_id,
            api_key_id,
            candidate,
            candidate_index,
            retry_index,
            candidate_id,
            required_capabilities,
            extra_data,
            created_at_unix_ms,
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
        retry_index: u32,
        candidate_id: &str,
        required_capabilities: Option<&Value>,
        skip_reason: &str,
        extra_data: Option<Value>,
        finished_at_unix_ms: u64,
        error_context: &'static str,
    ) {
        crate::request_candidate_runtime::persist_skipped_local_candidate(
            self.app(),
            trace_id,
            user_id,
            api_key_id,
            candidate,
            candidate_index,
            retry_index,
            candidate_id,
            required_capabilities,
            skip_reason,
            extra_data,
            finished_at_unix_ms,
            error_context,
        )
        .await
    }
}
