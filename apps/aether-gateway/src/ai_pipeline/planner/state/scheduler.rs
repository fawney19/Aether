use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use std::time::Duration;
use tokio::time::Instant;

use super::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_secs;
use crate::constants::{
    API_KEY_CONCURRENCY_WAIT_POLL_INTERVAL_MS, API_KEY_CONCURRENCY_WAIT_TIMEOUT_MS,
};
use crate::scheduler::candidate::SchedulerSkippedCandidate;
use crate::GatewayError;

impl<'a> PlannerAppState<'a> {
    pub(crate) async fn list_selectable_candidates(
        self,
        api_format: &str,
        global_model_name: &str,
        require_streaming: bool,
        required_capabilities: Option<&serde_json::Value>,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        now_unix_secs: u64,
    ) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
        crate::scheduler::candidate::list_selectable_candidates(
            self.app().data.as_ref(),
            self.app(),
            api_format,
            global_model_name,
            require_streaming,
            required_capabilities,
            auth_snapshot,
            now_unix_secs,
        )
        .await
    }

    pub(crate) async fn list_selectable_candidates_with_skip_reasons(
        self,
        api_format: &str,
        global_model_name: &str,
        require_streaming: bool,
        required_capabilities: Option<&serde_json::Value>,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        now_unix_secs: u64,
    ) -> Result<
        (
            Vec<SchedulerMinimalCandidateSelectionCandidate>,
            Vec<SchedulerSkippedCandidate>,
        ),
        GatewayError,
    > {
        let wait_timeout = Duration::from_millis(API_KEY_CONCURRENCY_WAIT_TIMEOUT_MS);
        let wait_interval = Duration::from_millis(API_KEY_CONCURRENCY_WAIT_POLL_INTERVAL_MS.max(1));
        let wait_deadline = Instant::now() + wait_timeout;
        let mut attempt_now_unix_secs = now_unix_secs;

        loop {
            let result = crate::scheduler::candidate::list_selectable_candidates_with_skip_reasons(
                self.app().data.as_ref(),
                self.app(),
                api_format,
                global_model_name,
                require_streaming,
                required_capabilities,
                auth_snapshot,
                attempt_now_unix_secs,
            )
            .await?;

            if !crate::scheduler::candidate::is_exact_all_skipped_by_auth_limit(
                &result.0, &result.1,
            ) {
                return Ok(result);
            }

            let now = Instant::now();
            if now >= wait_deadline {
                return Ok(result);
            }

            let remaining = wait_deadline.duration_since(now);
            tokio::time::sleep(wait_interval.min(remaining)).await;
            attempt_now_unix_secs = current_unix_secs();
        }
    }

    pub(crate) async fn list_selectable_candidates_for_required_capability_without_requested_model(
        self,
        candidate_api_format: &str,
        required_capability: &str,
        require_streaming: bool,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        now_unix_secs: u64,
    ) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
        let wait_timeout = Duration::from_millis(API_KEY_CONCURRENCY_WAIT_TIMEOUT_MS);
        let wait_interval = Duration::from_millis(API_KEY_CONCURRENCY_WAIT_POLL_INTERVAL_MS.max(1));
        let wait_deadline = Instant::now() + wait_timeout;
        let mut attempt_now_unix_secs = now_unix_secs;

        loop {
            let (result, auth_limit_blocked) = crate::scheduler::candidate::list_selectable_candidates_for_required_capability_without_requested_model_with_auth_limit_signal(
                self.app().data.as_ref(),
                self.app(),
                candidate_api_format,
                required_capability,
                require_streaming,
                auth_snapshot,
                attempt_now_unix_secs,
            )
            .await?;

            if !auth_limit_blocked {
                return Ok(result);
            }

            let now = Instant::now();
            if now >= wait_deadline {
                return Ok(result);
            }

            let remaining = wait_deadline.duration_since(now);
            tokio::time::sleep(wait_interval.min(remaining)).await;
            attempt_now_unix_secs = current_unix_secs();
        }
    }
}
