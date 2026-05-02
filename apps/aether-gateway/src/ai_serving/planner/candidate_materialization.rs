use aether_ai_serving::{
    ai_candidate_extra_data_with_ranking, ai_should_persist_available_candidate_for_pool_key,
    ai_should_persist_skipped_candidate_for_pool_membership,
    run_ai_available_candidate_persistence, run_ai_candidate_materialization,
    run_ai_skipped_candidate_persistence, AiAvailableCandidatePersistencePort,
    AiCandidateMaterializationOutcome, AiCandidateMaterializationPort, AiCandidateResolutionMode,
    AiSkippedCandidatePersistencePort,
};
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use async_trait::async_trait;
use serde_json::Value;
use std::convert::Infallible;
use uuid::Uuid;

use crate::ai_serving::planner::candidate_affinity_cache::remember_scheduler_affinity_for_candidate;
use crate::ai_serving::planner::candidate_resolution::{
    resolve_and_rank_local_execution_candidates,
    resolve_and_rank_local_execution_candidates_without_transport_pair_gate,
    EligibleLocalExecutionCandidate, SkippedLocalExecutionCandidate,
};
use crate::ai_serving::planner::materialization_policy::LocalCandidatePersistencePolicy;
use crate::ai_serving::planner::runtime_miss::record_local_runtime_candidate_skip_reason;
use crate::ai_serving::planner::CandidateFailureDiagnostic;
use crate::ai_serving::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_ms;
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::orchestration::{local_attempt_slot_count, ExecutionAttemptIdentity};
use crate::AppState;

#[derive(Debug, Clone)]
pub(crate) struct LocalExecutionCandidateAttempt {
    pub(crate) eligible: EligibleLocalExecutionCandidate,
    pub(crate) candidate_index: u32,
    pub(crate) retry_index: u32,
    pub(crate) candidate_id: String,
}

impl LocalExecutionCandidateAttempt {
    pub(crate) fn attempt_identity(&self) -> ExecutionAttemptIdentity {
        ExecutionAttemptIdentity::new(self.candidate_index, self.retry_index)
            .with_pool_key_index(self.eligible.orchestration.pool_key_index)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalAvailableCandidatePersistenceContext<'a> {
    pub(crate) user_id: &'a str,
    pub(crate) api_key_id: &'a str,
    pub(crate) required_capabilities: Option<&'a Value>,
    pub(crate) error_context: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LocalSkippedCandidatePersistenceContext<'a> {
    pub(crate) user_id: &'a str,
    pub(crate) api_key_id: &'a str,
    pub(crate) required_capabilities: Option<&'a Value>,
    pub(crate) error_context: &'static str,
    pub(crate) record_runtime_miss_diagnostic: bool,
}

pub(crate) use aether_ai_serving::AiCandidateResolutionMode as LocalCandidateResolutionMode;

struct GatewayLocalCandidateMaterializationPort<'a, F, G> {
    state: PlannerAppState<'a>,
    trace_id: &'a str,
    client_api_format: &'a str,
    requested_model: Option<&'a str>,
    auth_snapshot: Option<&'a GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&'a Value>,
    sticky_session_token: Option<&'a str>,
    request_auth_channel: Option<&'a str>,
    persistence_policy: LocalCandidatePersistencePolicy<'a>,
    resolution_mode: LocalCandidateResolutionMode,
    build_available_extra_data: F,
    decorate_skipped_candidate: G,
}

struct GatewayAvailableCandidatePersistencePort<'a, F> {
    state: PlannerAppState<'a>,
    trace_id: &'a str,
    user_id: &'a str,
    api_key_id: &'a str,
    required_capabilities: Option<&'a Value>,
    error_context: &'static str,
    created_at_unix_ms: u64,
    build_extra_data: F,
}

struct GatewaySkippedCandidatePersistencePort<'a> {
    state: &'a AppState,
    trace_id: &'a str,
    user_id: &'a str,
    api_key_id: &'a str,
    required_capabilities: Option<&'a Value>,
    error_context: &'static str,
    record_runtime_miss_diagnostic: bool,
}

#[async_trait]
impl<F, G> AiCandidateMaterializationPort for GatewayLocalCandidateMaterializationPort<'_, F, G>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
    G: Fn(SkippedLocalExecutionCandidate) -> SkippedLocalExecutionCandidate + Send + Sync,
{
    type Candidate = SchedulerMinimalCandidateSelectionCandidate;
    type Eligible = EligibleLocalExecutionCandidate;
    type Skipped = SkippedLocalExecutionCandidate;
    type Attempt = LocalExecutionCandidateAttempt;
    type Error = Infallible;

    async fn resolve_and_rank_candidates(
        &self,
        candidates: Vec<Self::Candidate>,
    ) -> Result<(Vec<Self::Eligible>, Vec<Self::Skipped>), Self::Error> {
        let requested_model = self.requested_model.map(str::to_string);
        let resolved = match self.resolution_mode {
            AiCandidateResolutionMode::Standard => {
                resolve_and_rank_local_execution_candidates(
                    self.state,
                    candidates,
                    self.client_api_format,
                    requested_model.as_deref().unwrap_or_default(),
                    self.auth_snapshot,
                    self.required_capabilities,
                    self.sticky_session_token,
                    self.request_auth_channel,
                )
                .await
            }
            AiCandidateResolutionMode::WithoutTransportPairGate => {
                resolve_and_rank_local_execution_candidates_without_transport_pair_gate(
                    self.state,
                    candidates,
                    self.client_api_format,
                    requested_model.as_deref(),
                    self.auth_snapshot,
                    self.required_capabilities,
                    self.sticky_session_token,
                    self.request_auth_channel,
                )
                .await
            }
        };
        Ok(resolved)
    }

    fn decorate_skipped_candidate(&self, skipped: Self::Skipped) -> Self::Skipped {
        (self.decorate_skipped_candidate)(skipped)
    }

    fn remember_first_candidate_affinity(&self, candidates: &[Self::Eligible]) {
        remember_first_local_candidate_affinity(
            self.state,
            self.auth_snapshot,
            self.client_api_format,
            self.requested_model,
            candidates,
        );
    }

    async fn persist_available_candidates(
        &self,
        candidates: Vec<Self::Eligible>,
    ) -> Result<Vec<Self::Attempt>, Self::Error> {
        Ok(persist_available_local_execution_candidates_with_context(
            self.state,
            self.trace_id,
            self.persistence_policy.available,
            candidates,
            &self.build_available_extra_data,
        )
        .await)
    }

    async fn persist_skipped_candidates(
        &self,
        starting_candidate_index: u32,
        skipped_candidates: Vec<Self::Skipped>,
    ) -> Result<(), Self::Error> {
        persist_skipped_local_execution_candidates_with_context(
            self.state.app(),
            self.trace_id,
            self.persistence_policy.skipped,
            starting_candidate_index,
            skipped_candidates,
        )
        .await;
        Ok(())
    }
}

#[async_trait]
impl<F> AiAvailableCandidatePersistencePort for GatewayAvailableCandidatePersistencePort<'_, F>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    type Candidate = EligibleLocalExecutionCandidate;
    type Attempt = LocalExecutionCandidateAttempt;
    type ExtraData = Value;
    type Error = Infallible;

    fn attempt_slot_count(&self, candidate: &Self::Candidate) -> u32 {
        local_attempt_slot_count(&candidate.transport)
    }

    fn build_extra_data(&self, candidate: &Self::Candidate) -> Option<Self::ExtraData> {
        ai_candidate_extra_data_with_ranking(
            (self.build_extra_data)(candidate),
            candidate.ranking.as_ref(),
        )
    }

    fn generate_candidate_id(&self) -> String {
        Uuid::new_v4().to_string()
    }

    fn should_persist_available_candidate(&self, candidate: &Self::Candidate) -> bool {
        should_persist_available_local_candidate(candidate)
    }

    async fn persist_available_candidate(
        &self,
        candidate: &Self::Candidate,
        candidate_index: u32,
        retry_index: u32,
        generated_candidate_id: &str,
        extra_data: Option<Self::ExtraData>,
    ) -> Result<String, Self::Error> {
        Ok(self
            .state
            .persist_available_local_candidate(
                self.trace_id,
                self.user_id,
                self.api_key_id,
                &candidate.candidate,
                candidate_index,
                retry_index,
                generated_candidate_id,
                self.required_capabilities,
                extra_data,
                self.created_at_unix_ms,
                self.error_context,
            )
            .await)
    }

    fn build_attempt(
        &self,
        candidate: Self::Candidate,
        candidate_index: u32,
        retry_index: u32,
        candidate_id: String,
    ) -> Self::Attempt {
        LocalExecutionCandidateAttempt {
            eligible: candidate,
            candidate_index,
            retry_index,
            candidate_id,
        }
    }
}

#[async_trait]
impl AiSkippedCandidatePersistencePort for GatewaySkippedCandidatePersistencePort<'_> {
    type Skipped = SkippedLocalExecutionCandidate;
    type ExtraData = Value;
    type Error = Infallible;

    fn should_persist_skipped_candidate(&self, candidate: &Self::Skipped) -> bool {
        should_persist_skipped_local_candidate(candidate)
    }

    fn build_extra_data(&self, candidate: &Self::Skipped) -> Option<Self::ExtraData> {
        ai_candidate_extra_data_with_ranking(
            candidate.extra_data.clone(),
            candidate.ranking.as_ref(),
        )
    }

    fn generate_candidate_id(&self) -> String {
        Uuid::new_v4().to_string()
    }

    async fn persist_skipped_candidate(
        &self,
        candidate: &Self::Skipped,
        candidate_index: u32,
        generated_candidate_id: &str,
        extra_data: Option<Self::ExtraData>,
    ) -> Result<(), Self::Error> {
        persist_skipped_local_execution_candidate(
            self.state,
            self.trace_id,
            self.user_id,
            self.api_key_id,
            &candidate.candidate,
            candidate_index,
            generated_candidate_id,
            self.required_capabilities,
            candidate.skip_reason,
            extra_data,
            self.error_context,
            self.record_runtime_miss_diagnostic,
        )
        .await;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn materialize_local_execution_candidates_with_serving<F, G>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    client_api_format: &str,
    requested_model: Option<&str>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&Value>,
    sticky_session_token: Option<&str>,
    request_auth_channel: Option<&str>,
    persistence_policy: LocalCandidatePersistencePolicy<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    preselection_skipped: Vec<SkippedLocalExecutionCandidate>,
    resolution_mode: LocalCandidateResolutionMode,
    build_available_extra_data: F,
    decorate_skipped_candidate: G,
) -> AiCandidateMaterializationOutcome<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
    G: Fn(SkippedLocalExecutionCandidate) -> SkippedLocalExecutionCandidate + Send + Sync,
{
    let port = GatewayLocalCandidateMaterializationPort {
        state,
        trace_id,
        client_api_format,
        requested_model,
        auth_snapshot,
        required_capabilities,
        sticky_session_token,
        request_auth_channel,
        persistence_policy,
        resolution_mode,
        build_available_extra_data,
        decorate_skipped_candidate,
    };

    match run_ai_candidate_materialization(&port, candidates, preselection_skipped).await {
        Ok(outcome) => outcome,
        Err(error) => match error {},
    }
}

pub(crate) fn remember_first_local_candidate_affinity(
    state: PlannerAppState<'_>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    client_api_format: &str,
    requested_model: Option<&str>,
    candidates: &[EligibleLocalExecutionCandidate],
) {
    let Some(first_candidate) = candidates.first() else {
        return;
    };
    let affinity_requested_model = requested_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(first_candidate.candidate.global_model_name.as_str());
    remember_scheduler_affinity_for_candidate(
        state,
        auth_snapshot,
        client_api_format,
        affinity_requested_model,
        &first_candidate.candidate,
    );
}

fn should_persist_available_local_candidate(eligible: &EligibleLocalExecutionCandidate) -> bool {
    ai_should_persist_available_candidate_for_pool_key(eligible.orchestration.pool_key_index)
}

fn should_persist_skipped_local_candidate(candidate: &SkippedLocalExecutionCandidate) -> bool {
    let is_pool_candidate = candidate.transport.as_ref().is_some_and(|transport| {
        admin_provider_pool_config_from_config_value(transport.provider.config.as_ref()).is_some()
    });
    ai_should_persist_skipped_candidate_for_pool_membership(is_pool_candidate)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_available_local_execution_candidates<F>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    user_id: &str,
    api_key_id: &str,
    required_capabilities: Option<&Value>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    error_context: &'static str,
    build_extra_data: F,
) -> Vec<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    let port = GatewayAvailableCandidatePersistencePort {
        state,
        trace_id,
        user_id,
        api_key_id,
        required_capabilities,
        error_context,
        created_at_unix_ms: current_unix_ms(),
        build_extra_data,
    };

    match run_ai_available_candidate_persistence(&port, candidates).await {
        Ok(attempts) => attempts,
        Err(error) => match error {},
    }
}

pub(crate) async fn persist_available_local_execution_candidates_with_context<F>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    context: LocalAvailableCandidatePersistenceContext<'_>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    build_extra_data: F,
) -> Vec<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value> + Send + Sync,
{
    persist_available_local_execution_candidates(
        state,
        trace_id,
        context.user_id,
        context.api_key_id,
        context.required_capabilities,
        candidates,
        context.error_context,
        build_extra_data,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_skipped_local_execution_candidate(
    state: &AppState,
    trace_id: &str,
    user_id: &str,
    api_key_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    required_capabilities: Option<&Value>,
    skip_reason: &'static str,
    extra_data: Option<Value>,
    error_context: &'static str,
    record_runtime_miss_diagnostic: bool,
) {
    if record_runtime_miss_diagnostic {
        record_local_runtime_candidate_skip_reason(state, trace_id, skip_reason);
    }

    PlannerAppState::new(state)
        .persist_skipped_local_candidate(
            trace_id,
            user_id,
            api_key_id,
            candidate,
            candidate_index,
            0,
            candidate_id,
            required_capabilities,
            skip_reason,
            extra_data,
            current_unix_ms(),
            error_context,
        )
        .await;
}

pub(crate) async fn mark_skipped_local_execution_candidate(
    state: &AppState,
    trace_id: &str,
    context: LocalSkippedCandidatePersistenceContext<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    persist_skipped_local_execution_candidate(
        state,
        trace_id,
        context.user_id,
        context.api_key_id,
        candidate,
        candidate_index,
        candidate_id,
        context.required_capabilities,
        skip_reason,
        None,
        context.error_context,
        context.record_runtime_miss_diagnostic,
    )
    .await;
}

pub(crate) async fn mark_skipped_local_execution_candidate_with_extra_data(
    state: &AppState,
    trace_id: &str,
    context: LocalSkippedCandidatePersistenceContext<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    extra_data: Option<Value>,
) {
    persist_skipped_local_execution_candidate(
        state,
        trace_id,
        context.user_id,
        context.api_key_id,
        candidate,
        candidate_index,
        candidate_id,
        context.required_capabilities,
        skip_reason,
        extra_data,
        context.error_context,
        context.record_runtime_miss_diagnostic,
    )
    .await;
}

pub(crate) async fn mark_skipped_local_execution_candidate_with_failure_diagnostic(
    state: &AppState,
    trace_id: &str,
    context: LocalSkippedCandidatePersistenceContext<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    diagnostic: CandidateFailureDiagnostic,
) {
    mark_skipped_local_execution_candidate_with_extra_data(
        state,
        trace_id,
        context,
        candidate,
        candidate_index,
        candidate_id,
        skip_reason,
        Some(diagnostic.to_extra_data()),
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_skipped_local_execution_candidates(
    state: &AppState,
    trace_id: &str,
    user_id: &str,
    api_key_id: &str,
    required_capabilities: Option<&Value>,
    starting_candidate_index: u32,
    skipped_candidates: Vec<SkippedLocalExecutionCandidate>,
    error_context: &'static str,
    record_runtime_miss_diagnostic: bool,
) {
    let port = GatewaySkippedCandidatePersistencePort {
        state,
        trace_id,
        user_id,
        api_key_id,
        required_capabilities,
        error_context,
        record_runtime_miss_diagnostic,
    };

    match run_ai_skipped_candidate_persistence(&port, starting_candidate_index, skipped_candidates)
        .await
    {
        Ok(()) => {}
        Err(error) => match error {},
    }
}

pub(crate) async fn persist_skipped_local_execution_candidates_with_context(
    state: &AppState,
    trace_id: &str,
    context: LocalSkippedCandidatePersistenceContext<'_>,
    starting_candidate_index: u32,
    skipped_candidates: Vec<SkippedLocalExecutionCandidate>,
) {
    persist_skipped_local_execution_candidates(
        state,
        trace_id,
        context.user_id,
        context.api_key_id,
        context.required_capabilities,
        starting_candidate_index,
        skipped_candidates,
        context.error_context,
        context.record_runtime_miss_diagnostic,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use aether_scheduler_core::{
        SchedulerMinimalCandidateSelectionCandidate, SchedulerPriorityMode, SchedulerRankingMode,
        SchedulerRankingOutcome,
    };
    use serde_json::json;

    use super::*;
    use crate::data::GatewayDataState;
    use crate::orchestration::LocalExecutionCandidateMetadata;

    fn sample_candidate(key_id: &str) -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: "provider-1".to_string(),
            provider_name: "provider-1".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 10,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "api_key".to_string(),
            key_internal_priority: 10,
            key_global_priority_for_format: Some(10),
            key_capabilities: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            selected_provider_model_name: "gpt-5".to_string(),
            mapping_matched_model: None,
        }
    }

    fn sample_transport(
        key_id: &str,
        provider_config: Option<serde_json::Value>,
    ) -> Arc<crate::ai_serving::GatewayProviderTransportSnapshot> {
        Arc::new(crate::ai_serving::GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider-1".to_string(),
                provider_type: "codex".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: provider_config,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:chat".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: key_id.to_string(),
                provider_id: "provider-1".to_string(),
                name: key_id.to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec!["openai:chat".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        })
    }

    fn sample_eligible(
        key_id: &str,
        pool_key_index: Option<u32>,
    ) -> EligibleLocalExecutionCandidate {
        EligibleLocalExecutionCandidate {
            candidate: sample_candidate(key_id),
            transport: sample_transport(
                key_id,
                pool_key_index.map(|_| json!({ "pool_advanced": {} })),
            ),
            provider_api_format: "openai:chat".to_string(),
            orchestration: LocalExecutionCandidateMetadata {
                candidate_group_id: pool_key_index.map(|_| "pool-group".to_string()),
                pool_key_index,
            },
            ranking: None,
        }
    }

    #[tokio::test]
    async fn pool_group_representatives_are_persisted_as_available_before_attempt() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_repository_for_tests(Arc::clone(
                    &repository,
                )),
            );

        let attempts = persist_available_local_execution_candidates(
            PlannerAppState::new(&app),
            "trace-pool-lazy",
            "user-1",
            "api-key-1",
            None,
            vec![
                sample_eligible("pool-key", Some(0)),
                sample_eligible("pool-key-internal", Some(1)),
                sample_eligible("normal-key", None),
            ],
            "persist should not fail",
            |_| None,
        )
        .await;

        assert_eq!(attempts.len(), 3);
        let stored = app
            .read_request_candidates_by_request_id("trace-pool-lazy")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].key_id.as_deref(), Some("pool-key"));
        assert_eq!(stored[0].candidate_index, 0);
        assert_eq!(stored[1].key_id.as_deref(), Some("normal-key"));
        assert_eq!(stored[1].candidate_index, 2);
    }

    #[tokio::test]
    async fn available_candidates_persist_ranking_metadata_in_extra_data() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_repository_for_tests(Arc::clone(
                    &repository,
                )),
            );
        let mut eligible = sample_eligible("ranked-key", None);
        eligible.ranking = Some(SchedulerRankingOutcome {
            original_index: 1,
            ranking_index: 0,
            priority_mode: SchedulerPriorityMode::Provider,
            ranking_mode: SchedulerRankingMode::CacheAffinity,
            priority_slot: 7,
            promoted_by: Some("cached_affinity"),
            demoted_by: Some("cross_format"),
        });

        persist_available_local_execution_candidates(
            PlannerAppState::new(&app),
            "trace-ranking-extra-data",
            "user-1",
            "api-key-1",
            None,
            vec![eligible],
            "persist should not fail",
            |_| Some(json!({ "existing": "value" })),
        )
        .await;

        let stored = app
            .read_request_candidates_by_request_id("trace-ranking-extra-data")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        let extra_data = stored[0]
            .extra_data
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .expect("ranking metadata should persist as object extra data");
        assert_eq!(extra_data.get("existing"), Some(&json!("value")));
        assert_eq!(
            extra_data.get("ranking_mode"),
            Some(&json!("CacheAffinity"))
        );
        assert_eq!(extra_data.get("priority_mode"), Some(&json!("Provider")));
        assert_eq!(extra_data.get("ranking_index"), Some(&json!(0)));
        assert_eq!(extra_data.get("priority_slot"), Some(&json!(7)));
        assert_eq!(
            extra_data.get("promoted_by"),
            Some(&json!("cached_affinity"))
        );
        assert_eq!(extra_data.get("demoted_by"), Some(&json!("cross_format")));
    }

    #[tokio::test]
    async fn pool_internal_skipped_candidates_are_not_persisted() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_repository_for_tests(Arc::clone(
                    &repository,
                )),
            );

        persist_skipped_local_execution_candidates(
            &app,
            "trace-pool-skipped",
            "user-1",
            "api-key-1",
            None,
            0,
            vec![
                SkippedLocalExecutionCandidate {
                    candidate: sample_candidate("pool-skipped"),
                    skip_reason: "pool_cooldown",
                    transport: Some(sample_transport(
                        "pool-skipped",
                        Some(json!({ "pool_advanced": {} })),
                    )),
                    ranking: None,
                    extra_data: None,
                },
                SkippedLocalExecutionCandidate {
                    candidate: sample_candidate("normal-skipped"),
                    skip_reason: "key_inactive",
                    transport: None,
                    ranking: Some(SchedulerRankingOutcome {
                        original_index: 2,
                        ranking_index: 1,
                        priority_mode: SchedulerPriorityMode::Provider,
                        ranking_mode: SchedulerRankingMode::CacheAffinity,
                        priority_slot: 9,
                        promoted_by: None,
                        demoted_by: Some("cross_format"),
                    }),
                    extra_data: Some(json!({ "existing": "value" })),
                },
            ],
            "persist skipped should not fail",
            false,
        )
        .await;

        let stored = app
            .read_request_candidates_by_request_id("trace-pool-skipped")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].key_id.as_deref(), Some("normal-skipped"));
        assert_eq!(stored[0].candidate_index, 0);
        let extra_data = stored[0]
            .extra_data
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .expect("skipped ranking metadata should persist");
        assert_eq!(extra_data.get("existing"), Some(&json!("value")));
        assert_eq!(
            extra_data.get("ranking_mode"),
            Some(&json!("CacheAffinity"))
        );
        assert_eq!(extra_data.get("priority_mode"), Some(&json!("Provider")));
        assert_eq!(extra_data.get("ranking_index"), Some(&json!(1)));
        assert_eq!(extra_data.get("priority_slot"), Some(&json!(9)));
        assert_eq!(extra_data.get("demoted_by"), Some(&json!("cross_format")));
    }
}
