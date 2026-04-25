use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::Value;
use uuid::Uuid;

use crate::ai_pipeline::planner::candidate_affinity::remember_scheduler_affinity_for_candidate;
use crate::ai_pipeline::planner::candidate_eligibility::{
    EligibleLocalExecutionCandidate, SkippedLocalExecutionCandidate,
};
use crate::ai_pipeline::planner::runtime_miss::record_local_runtime_candidate_skip_reason;
use crate::ai_pipeline::{GatewayAuthApiKeySnapshot, PlannerAppState};
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
    eligible.orchestration.pool_key_index.is_none()
}

fn should_persist_skipped_local_candidate(candidate: &SkippedLocalExecutionCandidate) -> bool {
    candidate.transport.as_ref().is_none_or(|transport| {
        admin_provider_pool_config_from_config_value(transport.provider.config.as_ref()).is_none()
    })
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
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value>,
{
    let created_at_unix_ms = current_unix_ms();
    let total_attempts = candidates
        .iter()
        .map(|eligible| local_attempt_slot_count(&eligible.transport) as usize)
        .sum();
    let mut materialized = Vec::with_capacity(total_attempts);

    for (candidate_index, eligible) in candidates.into_iter().enumerate() {
        let candidate_index = candidate_index as u32;
        let attempt_slots = local_attempt_slot_count(&eligible.transport);
        let pool_key_index = eligible.orchestration.pool_key_index;
        let extra_data = build_extra_data(&eligible);
        let mut owned_eligible = Some(eligible);

        for retry_index in 0..attempt_slots {
            let eligible = owned_eligible
                .as_ref()
                .expect("eligible candidate should remain available until final retry");
            let attempt_identity = ExecutionAttemptIdentity::new(candidate_index, retry_index)
                .with_pool_key_index(pool_key_index);
            let generated_candidate_id = Uuid::new_v4().to_string();
            let candidate_id = if should_persist_available_local_candidate(eligible) {
                state
                    .persist_available_local_candidate(
                        trace_id,
                        user_id,
                        api_key_id,
                        &eligible.candidate,
                        attempt_identity.candidate_index,
                        attempt_identity.retry_index,
                        &generated_candidate_id,
                        required_capabilities,
                        extra_data.clone(),
                        created_at_unix_ms,
                        error_context,
                    )
                    .await
            } else {
                generated_candidate_id
            };

            let eligible = if retry_index + 1 == attempt_slots {
                owned_eligible
                    .take()
                    .expect("final retry should consume owned eligible candidate")
            } else {
                eligible.clone()
            };
            materialized.push(LocalExecutionCandidateAttempt {
                eligible,
                candidate_index: attempt_identity.candidate_index,
                retry_index: attempt_identity.retry_index,
                candidate_id,
            });
        }
    }

    materialized
}

pub(crate) async fn persist_available_local_execution_candidates_with_context<F>(
    state: PlannerAppState<'_>,
    trace_id: &str,
    context: LocalAvailableCandidatePersistenceContext<'_>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    build_extra_data: F,
) -> Vec<LocalExecutionCandidateAttempt>
where
    F: Fn(&EligibleLocalExecutionCandidate) -> Option<Value>,
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
    let mut next_candidate_index = starting_candidate_index;
    for skipped_candidate in skipped_candidates {
        if !should_persist_skipped_local_candidate(&skipped_candidate) {
            continue;
        }
        let generated_candidate_id = Uuid::new_v4().to_string();
        persist_skipped_local_execution_candidate(
            state,
            trace_id,
            user_id,
            api_key_id,
            &skipped_candidate.candidate,
            next_candidate_index,
            &generated_candidate_id,
            required_capabilities,
            skipped_candidate.skip_reason,
            skipped_candidate.extra_data,
            error_context,
            record_runtime_miss_diagnostic,
        )
        .await;
        next_candidate_index = next_candidate_index.saturating_add(1);
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
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
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
    ) -> Arc<crate::ai_pipeline::GatewayProviderTransportSnapshot> {
        Arc::new(crate::ai_pipeline::GatewayProviderTransportSnapshot {
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
        }
    }

    #[tokio::test]
    async fn pool_candidates_are_not_persisted_as_available_before_attempt() {
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
                sample_eligible("normal-key", None),
            ],
            "persist should not fail",
            |_| None,
        )
        .await;

        assert_eq!(attempts.len(), 2);
        let stored = app
            .read_request_candidates_by_request_id("trace-pool-lazy")
            .await
            .expect("request candidates should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].key_id.as_deref(), Some("normal-key"));
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
                    extra_data: None,
                },
                SkippedLocalExecutionCandidate {
                    candidate: sample_candidate("normal-skipped"),
                    skip_reason: "key_inactive",
                    transport: None,
                    extra_data: None,
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
    }
}
