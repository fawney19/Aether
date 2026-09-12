use aether_scheduler_core::{ClientSessionAffinity, SchedulerMinimalCandidateSelectionCandidate};
use std::time::Duration;

use super::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::constants::{
    API_KEY_CONCURRENCY_WAIT_POLL_INTERVAL_MS, API_KEY_CONCURRENCY_WAIT_TIMEOUT_MS,
};
use crate::scheduler::candidate::SchedulerSkippedCandidate;
use crate::scheduler::config::SchedulerOrderingConfig;
use crate::GatewayError;

impl<'a> PlannerAppState<'a> {
    /// `ordering_config` is the immutable scheduler snapshot derived from the
    /// request's resolved routing policy.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn list_selectable_candidates(
        self,
        api_format: &str,
        global_model_name: &str,
        require_streaming: bool,
        required_capabilities: Option<&serde_json::Value>,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        client_session_affinity: Option<&ClientSessionAffinity>,
        now_unix_secs: u64,
        enable_model_directives: bool,
        ordering_config: SchedulerOrderingConfig,
    ) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
        crate::scheduler::candidate::list_selectable_candidates(
            self.app().data.as_ref(),
            self.app(),
            api_format,
            global_model_name,
            require_streaming,
            required_capabilities,
            auth_snapshot,
            client_session_affinity,
            now_unix_secs,
            enable_model_directives,
            ordering_config,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn list_selectable_candidates_with_skip_reasons(
        self,
        api_format: &str,
        global_model_name: &str,
        require_streaming: bool,
        required_capabilities: Option<&serde_json::Value>,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        client_session_affinity: Option<&ClientSessionAffinity>,
        now_unix_secs: u64,
        enable_model_directives: bool,
        ordering_config: SchedulerOrderingConfig,
    ) -> Result<
        (
            Vec<SchedulerMinimalCandidateSelectionCandidate>,
            Vec<SchedulerSkippedCandidate>,
        ),
        GatewayError,
    > {
        self.list_selectable_candidates_with_skip_reasons_for_request_operation(
            api_format,
            global_model_name,
            require_streaming,
            required_capabilities,
            auth_snapshot,
            client_session_affinity,
            now_unix_secs,
            enable_model_directives,
            None,
            ordering_config,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn list_selectable_candidates_with_skip_reasons_for_request_operation(
        self,
        api_format: &str,
        global_model_name: &str,
        require_streaming: bool,
        required_capabilities: Option<&serde_json::Value>,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        client_session_affinity: Option<&ClientSessionAffinity>,
        now_unix_secs: u64,
        enable_model_directives: bool,
        request_operation: Option<&str>,
        ordering_config: SchedulerOrderingConfig,
    ) -> Result<
        (
            Vec<SchedulerMinimalCandidateSelectionCandidate>,
            Vec<SchedulerSkippedCandidate>,
        ),
        GatewayError,
    > {
        crate::scheduler::candidate::select_with_auth_concurrency_wait(
            self.app(),
            auth_snapshot,
            now_unix_secs,
            Duration::from_millis(API_KEY_CONCURRENCY_WAIT_TIMEOUT_MS),
            Duration::from_millis(API_KEY_CONCURRENCY_WAIT_POLL_INTERVAL_MS),
            |attempt_now_unix_secs| async move {
            let result = crate::scheduler::candidate::list_selectable_candidates_with_skip_reasons_for_request_operation(
                self.app().data.as_ref(),
                self.app(),
                api_format,
                global_model_name,
                require_streaming,
                required_capabilities,
                auth_snapshot,
                client_session_affinity,
                attempt_now_unix_secs,
                enable_model_directives,
                request_operation,
                ordering_config,
            )
            .await?;

            let auth_limit_blocked = crate::scheduler::candidate::is_exact_all_skipped_by_auth_limit(
                &result.0, &result.1,
            );
            Ok((result, auth_limit_blocked))
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn list_selectable_enumerated_candidates_with_skip_reasons(
        self,
        api_format: &str,
        global_model_name: &str,
        candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
        required_capabilities: Option<&serde_json::Value>,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        client_session_affinity: Option<&ClientSessionAffinity>,
        now_unix_secs: u64,
        ordering_config: SchedulerOrderingConfig,
    ) -> Result<
        (
            Vec<SchedulerMinimalCandidateSelectionCandidate>,
            Vec<SchedulerSkippedCandidate>,
        ),
        GatewayError,
    > {
        crate::scheduler::candidate::list_selectable_enumerated_candidates_with_skip_reasons(
            self.app(),
            api_format,
            global_model_name,
            candidates,
            required_capabilities,
            auth_snapshot,
            client_session_affinity,
            now_unix_secs,
            ordering_config,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn list_selectable_candidates_for_required_capability_without_requested_model(
        self,
        candidate_api_format: &str,
        required_capability: &str,
        require_streaming: bool,
        auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
        client_session_affinity: Option<&ClientSessionAffinity>,
        now_unix_secs: u64,
        ordering_config: SchedulerOrderingConfig,
    ) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
        crate::scheduler::candidate::select_with_auth_concurrency_wait(
            self.app(),
            auth_snapshot,
            now_unix_secs,
            Duration::from_millis(API_KEY_CONCURRENCY_WAIT_TIMEOUT_MS),
            Duration::from_millis(API_KEY_CONCURRENCY_WAIT_POLL_INTERVAL_MS),
            |attempt_now_unix_secs| {
                crate::scheduler::candidate::list_selectable_candidates_for_required_capability_without_requested_model_with_auth_limit_signal(
                self.app().data.as_ref(),
                self.app(),
                candidate_api_format,
                required_capability,
                require_streaming,
                auth_snapshot,
                client_session_affinity,
                attempt_now_unix_secs,
                ordering_config,
            )
            },
        )
        .await
    }
}
