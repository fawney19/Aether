use aether_ai_serving::{
    run_ai_candidate_preselection, AiCandidatePreselectionOutcome, AiCandidatePreselectionPort,
};
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use async_trait::async_trait;

use crate::ai_serving::planner::candidate_resolution::SkippedLocalExecutionCandidate;
use crate::ai_serving::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_secs;
use crate::scheduler::candidate::SchedulerSkippedCandidate;
use crate::GatewayError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalCandidatePreselectionKeyMode {
    ProviderEndpointKeyModel,
    ProviderEndpointKeyModelAndApiFormat,
}

struct GatewayLocalCandidatePreselectionPort<'a> {
    state: PlannerAppState<'a>,
    client_api_format: &'a str,
    requested_model: &'a str,
    require_streaming: bool,
    required_capabilities: Option<&'a serde_json::Value>,
    auth_snapshot: &'a GatewayAuthApiKeySnapshot,
    use_api_format_alias_match: bool,
    key_mode: LocalCandidatePreselectionKeyMode,
}

#[async_trait]
impl AiCandidatePreselectionPort for GatewayLocalCandidatePreselectionPort<'_> {
    type Candidate = SchedulerMinimalCandidateSelectionCandidate;
    type Skipped = SkippedLocalExecutionCandidate;
    type Error = GatewayError;

    fn candidate_api_formats(&self) -> Vec<String> {
        crate::ai_serving::request_candidate_api_formats(
            self.client_api_format,
            self.require_streaming,
        )
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn candidate_api_format_matches_client(&self, candidate_api_format: &str) -> bool {
        if self.use_api_format_alias_match {
            crate::ai_serving::api_format_alias_matches(
                candidate_api_format,
                self.client_api_format,
            )
        } else {
            candidate_api_format == self.client_api_format
        }
    }

    async fn list_candidates_for_api_format(
        &self,
        candidate_api_format: &str,
        matches_client_format: bool,
    ) -> Result<(Vec<Self::Candidate>, Vec<Self::Skipped>), Self::Error> {
        let auth_snapshot = matches_client_format.then_some(self.auth_snapshot);
        let (candidates, skipped_candidates) = self
            .state
            .list_selectable_candidates_with_skip_reasons(
                candidate_api_format,
                self.requested_model,
                self.require_streaming,
                self.required_capabilities,
                auth_snapshot,
                current_unix_secs(),
            )
            .await?;

        Ok((
            candidates,
            skipped_candidates
                .into_iter()
                .map(skipped_local_execution_candidate_from_scheduler_skip)
                .collect(),
        ))
    }

    fn candidate_allowed(
        &self,
        candidate: &Self::Candidate,
        _candidate_api_format: &str,
        matches_client_format: bool,
    ) -> bool {
        matches_client_format
            || auth_snapshot_allows_cross_format_candidate(
                self.auth_snapshot,
                self.requested_model,
                candidate,
            )
    }

    fn skipped_candidate_allowed(
        &self,
        skipped_candidate: &Self::Skipped,
        _candidate_api_format: &str,
        matches_client_format: bool,
    ) -> bool {
        matches_client_format
            || auth_snapshot_allows_cross_format_candidate(
                self.auth_snapshot,
                self.requested_model,
                &skipped_candidate.candidate,
            )
    }

    fn candidate_key(&self, candidate: &Self::Candidate) -> String {
        local_candidate_preselection_key(candidate, self.key_mode)
    }

    fn skipped_candidate_key(&self, skipped_candidate: &Self::Skipped) -> String {
        local_candidate_preselection_key(&skipped_candidate.candidate, self.key_mode)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn preselect_local_execution_candidates_with_serving(
    state: PlannerAppState<'_>,
    client_api_format: &str,
    requested_model: &str,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    use_api_format_alias_match: bool,
    key_mode: LocalCandidatePreselectionKeyMode,
) -> Result<
    AiCandidatePreselectionOutcome<
        SchedulerMinimalCandidateSelectionCandidate,
        SkippedLocalExecutionCandidate,
    >,
    GatewayError,
> {
    let port = GatewayLocalCandidatePreselectionPort {
        state,
        client_api_format,
        requested_model,
        require_streaming,
        required_capabilities,
        auth_snapshot,
        use_api_format_alias_match,
        key_mode,
    };

    run_ai_candidate_preselection(&port).await
}

fn skipped_local_execution_candidate_from_scheduler_skip(
    skipped_candidate: SchedulerSkippedCandidate,
) -> SkippedLocalExecutionCandidate {
    SkippedLocalExecutionCandidate {
        candidate: skipped_candidate.candidate,
        skip_reason: skipped_candidate.skip_reason,
        transport: None,
        ranking: None,
        extra_data: None,
    }
}

fn local_candidate_preselection_key(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    mode: LocalCandidatePreselectionKeyMode,
) -> String {
    match mode {
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModel => format!(
            "{}:{}:{}:{}:{}",
            candidate.provider_id,
            candidate.endpoint_id,
            candidate.key_id,
            candidate.model_id,
            candidate.selected_provider_model_name,
        ),
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat => format!(
            "{}:{}:{}:{}:{}:{}",
            candidate.provider_id,
            candidate.endpoint_id,
            candidate.key_id,
            candidate.model_id,
            candidate.selected_provider_model_name,
            candidate.endpoint_api_format,
        ),
    }
}

pub(crate) fn auth_snapshot_allows_cross_format_candidate(
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    requested_model: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> bool {
    if let Some(allowed_providers) = auth_snapshot.effective_allowed_providers() {
        let provider_allowed = allowed_providers.iter().any(|value| {
            aether_scheduler_core::provider_matches_allowed_value(
                value,
                &candidate.provider_id,
                &candidate.provider_name,
                &candidate.provider_type,
            )
        });
        if !provider_allowed {
            return false;
        }
    }

    if let Some(allowed_models) = auth_snapshot.effective_allowed_models() {
        let model_allowed = allowed_models
            .iter()
            .any(|value| value == requested_model || value == &candidate.global_model_name);
        if !model_allowed {
            return false;
        }
    }

    true
}
