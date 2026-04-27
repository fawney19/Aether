use std::collections::BTreeMap;

use tracing::warn;

use crate::ai_pipeline::{
    request_candidate_api_format_preference, GatewayAuthApiKeySnapshot, PlannerAppState,
};
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::scheduler::config::{
    read_scheduler_ordering_config, SchedulerOrderingConfig, SchedulerSchedulingMode,
};
use aether_scheduler_core::{
    apply_scheduler_candidate_ranking, matches_affinity_target,
    requested_capability_priority_for_candidate, SchedulerAffinityTarget,
    SchedulerMinimalCandidateSelectionCandidate, SchedulerRankableCandidate,
    SchedulerRankingContext, SchedulerRankingMode,
};

use super::candidate_affinity_cache::read_cached_scheduler_affinity_target;
use super::candidate_resolution::EligibleLocalExecutionCandidate;
use super::candidate_transport_ranking_facts::{
    resolve_cached_transport_ranking_facts, CandidateTransportRankingFacts,
};

pub(crate) async fn rank_eligible_local_execution_candidates(
    state: PlannerAppState<'_>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    normalized_client_api_format: &str,
    requested_model: Option<&str>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    required_capabilities: Option<&serde_json::Value>,
) -> Vec<EligibleLocalExecutionCandidate> {
    let ordering_config = read_scheduler_ordering_config_or_default(state).await;
    let mut candidates = candidates;
    let affinity_requested_model = requested_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            candidates
                .first()
                .map(|candidate| candidate.candidate.global_model_name.as_str())
        });
    let cached_affinity_target = read_cached_scheduler_affinity_target(
        state,
        auth_snapshot,
        normalized_client_api_format,
        affinity_requested_model,
    );
    let mut rankables = Vec::with_capacity(candidates.len());
    let mut ordering_cache = BTreeMap::new();

    for (original_index, eligible) in candidates.iter().enumerate() {
        let ranking_facts = resolve_cached_transport_ranking_facts(
            state,
            &mut ordering_cache,
            &eligible.candidate,
            eligible.transport.as_ref(),
            ordering_config,
        )
        .await;
        rankables.push(rankable_candidate_from_candidate(
            &eligible.candidate,
            original_index,
            ranking_facts,
            normalized_client_api_format,
            eligible.provider_api_format.as_str(),
            required_capabilities,
            cached_affinity_target.as_ref().is_some_and(|target| {
                cached_affinity_matches_local_execution_scope(eligible, target)
            }),
        ));
    }

    drop(ordering_cache);
    let outcomes = apply_scheduler_candidate_ranking(
        &mut candidates,
        &rankables,
        planner_ranking_context(ordering_config),
    );
    for outcome in outcomes {
        let ranking_index = outcome.ranking_index;
        if let Some(candidate) = candidates.get_mut(ranking_index) {
            candidate.ranking = Some(outcome);
        }
    }
    candidates
}

fn rankable_candidate_from_candidate(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    original_index: usize,
    ranking_facts: CandidateTransportRankingFacts,
    normalized_client_api_format: &str,
    provider_api_format: &str,
    required_capabilities: Option<&serde_json::Value>,
    cached_affinity_match: bool,
) -> SchedulerRankableCandidate {
    let is_same_format = api_format_matches(provider_api_format, normalized_client_api_format);
    let mut rankable = SchedulerRankableCandidate::from_candidate(candidate, original_index);
    // The scheduler order is the upstream tie-breaker; pipeline only adds transport facts.
    rankable.provider_id.clear();
    rankable.endpoint_id.clear();
    rankable.key_id.clear();
    rankable.selected_provider_model_name.clear();

    rankable
        .with_capability_priority(requested_capability_priority_for_candidate(
            required_capabilities,
            candidate,
        ))
        .with_cached_affinity_match(cached_affinity_match)
        .with_tunnel_bucket(ranking_facts.tunnel_bucket)
        .with_format_state(
            !is_same_format && !ranking_facts.keep_priority_on_conversion,
            candidate_api_format_preference(normalized_client_api_format, provider_api_format),
        )
}

fn cached_affinity_matches_local_execution_scope(
    eligible: &EligibleLocalExecutionCandidate,
    target: &SchedulerAffinityTarget,
) -> bool {
    if local_execution_candidate_uses_pool(eligible) {
        return eligible.candidate.provider_id == target.provider_id
            && eligible.candidate.endpoint_id == target.endpoint_id;
    }

    matches_affinity_target(&eligible.candidate, target)
}

fn local_execution_candidate_uses_pool(eligible: &EligibleLocalExecutionCandidate) -> bool {
    admin_provider_pool_config_from_config_value(eligible.transport.provider.config.as_ref())
        .is_some()
}

fn planner_ranking_context(ordering_config: SchedulerOrderingConfig) -> SchedulerRankingContext {
    SchedulerRankingContext {
        priority_mode: ordering_config.priority_mode,
        ranking_mode: planner_ranking_mode(ordering_config.scheduling_mode),
        include_health: false,
        load_balance_seed: 0,
    }
}

fn planner_ranking_mode(mode: SchedulerSchedulingMode) -> SchedulerRankingMode {
    match mode {
        SchedulerSchedulingMode::FixedOrder => SchedulerRankingMode::FixedOrder,
        SchedulerSchedulingMode::CacheAffinity => SchedulerRankingMode::CacheAffinity,
        SchedulerSchedulingMode::LoadBalance => SchedulerRankingMode::LoadBalance,
    }
}

fn normalize_api_format_alias(value: &str) -> String {
    crate::ai_pipeline::normalize_legacy_openai_format_alias(value)
}

fn api_format_matches(left: &str, right: &str) -> bool {
    normalize_api_format_alias(left) == normalize_api_format_alias(right)
}

fn candidate_api_format_preference(client_api_format: &str, provider_api_format: &str) -> (u8, u8) {
    request_candidate_api_format_preference(client_api_format, provider_api_format)
        .unwrap_or((u8::MAX, u8::MAX))
}

async fn read_scheduler_ordering_config_or_default(
    state: PlannerAppState<'_>,
) -> SchedulerOrderingConfig {
    match read_scheduler_ordering_config(state.app()).await {
        Ok(config) => config,
        Err(error) => {
            warn!(
                event_name = "planner_scheduler_ordering_config_load_failed",
                log_type = "event",
                error = ?error,
                "failed to load scheduler ordering config while ranking local execution candidates"
            );
            SchedulerOrderingConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use aether_scheduler_core::{
        apply_scheduler_candidate_ranking, RANKING_REASON_CACHED_AFFINITY,
    };
    use serde_json::json;

    use super::super::candidate_affinity_cache::remember_scheduler_affinity_for_candidate;
    use super::super::candidate_transport_ranking_facts::resolve_cached_candidate_transport_ranking_facts;
    use super::{PlannerAppState, SchedulerMinimalCandidateSelectionCandidate};
    use crate::ai_pipeline::planner::candidate_resolution::resolve_and_rank_local_execution_candidates;
    use crate::data::auth::GatewayAuthApiKeySnapshot;
    use crate::data::GatewayDataState;
    use crate::tunnel::TunnelAttachmentRecord;
    use crate::{scheduler::affinity::SCHEDULER_AFFINITY_TTL, AppState};
    use aether_data::repository::auth::StoredAuthApiKeySnapshot;

    async fn rank_local_execution_candidates(
        state: PlannerAppState<'_>,
        candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
        client_api_format: &str,
        required_capabilities: Option<&serde_json::Value>,
    ) -> Vec<SchedulerMinimalCandidateSelectionCandidate> {
        let normalized_client_api_format = client_api_format.trim().to_ascii_lowercase();
        let ordering_config = super::read_scheduler_ordering_config_or_default(state).await;
        let mut candidates = candidates;
        let mut rankables = Vec::with_capacity(candidates.len());
        let mut ordering_cache = BTreeMap::new();

        for (original_index, candidate) in candidates.iter().enumerate() {
            let ranking_facts = resolve_cached_candidate_transport_ranking_facts(
                state,
                &mut ordering_cache,
                candidate,
                ordering_config,
            )
            .await;
            rankables.push(super::rankable_candidate_from_candidate(
                candidate,
                original_index,
                ranking_facts,
                normalized_client_api_format.as_str(),
                candidate.endpoint_api_format.as_str(),
                required_capabilities,
                false,
            ));
        }

        drop(ordering_cache);
        apply_scheduler_candidate_ranking(
            &mut candidates,
            &rankables,
            super::planner_ranking_context(ordering_config),
        );
        candidates
    }

    fn sample_candidate(
        endpoint_id: &str,
        key_id: &str,
    ) -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: "provider-1".to_string(),
            provider_name: "provider-1".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 0,
            endpoint_id: endpoint_id.to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "api_key".to_string(),
            key_internal_priority: 0,
            key_global_priority_for_format: Some(0),
            key_capabilities: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-4.1".to_string(),
            selected_provider_model_name: "gpt-4.1".to_string(),
            mapping_matched_model: None,
        }
    }

    fn sample_provider() -> StoredProviderCatalogProvider {
        sample_provider_with_options("provider-1", false, 0)
    }

    fn sample_provider_with_options(
        id: &str,
        keep_priority_on_conversion: bool,
        provider_priority: i32,
    ) -> StoredProviderCatalogProvider {
        sample_provider_with_config(id, keep_priority_on_conversion, provider_priority, None)
    }

    fn sample_provider_with_config(
        id: &str,
        keep_priority_on_conversion: bool,
        provider_priority: i32,
        config: Option<serde_json::Value>,
    ) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            id.to_string(),
            id.to_string(),
            Some("https://provider.example".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            keep_priority_on_conversion,
            false,
            None,
            None,
            None,
            None,
            None,
            config,
        )
        .with_routing_fields(provider_priority)
    }

    fn sample_endpoint(id: &str) -> StoredProviderCatalogEndpoint {
        sample_endpoint_for_provider("provider-1", id, "openai:chat")
    }

    fn sample_endpoint_for_provider(
        provider_id: &str,
        id: &str,
        api_format: &str,
    ) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            id.to_string(),
            provider_id.to_string(),
            api_format.to_string(),
            Some(
                api_format
                    .split(':')
                    .next()
                    .unwrap_or(api_format)
                    .to_string(),
            ),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.provider.example".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_key(id: &str, node_id: &str) -> StoredProviderCatalogKey {
        sample_key_for_provider("provider-1", id, node_id)
    }

    fn sample_key_for_provider(
        provider_id: &str,
        id: &str,
        node_id: &str,
    ) -> StoredProviderCatalogKey {
        sample_key_for_provider_with_options(
            provider_id,
            id,
            node_id,
            true,
            Some(json!(["openai:chat"])),
            None,
        )
    }

    fn sample_key_for_provider_with_options(
        provider_id: &str,
        id: &str,
        node_id: &str,
        is_active: bool,
        api_formats: Option<serde_json::Value>,
        allowed_models: Option<serde_json::Value>,
    ) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            id.to_string(),
            provider_id.to_string(),
            id.to_string(),
            "api_key".to_string(),
            None,
            is_active,
        )
        .expect("key should build")
        .with_transport_fields(
            api_formats,
            "plain-upstream-key".to_string(),
            None,
            None,
            Some(json!({"openai:chat": 1})),
            allowed_models,
            None,
            Some(json!({
                "enabled": true,
                "mode": "tunnel",
                "node_id": node_id,
            })),
            None,
        )
        .expect("key transport should build")
    }

    fn tunnel_attachment_key(node_id: &str) -> String {
        format!("tunnel.attachments.{node_id}")
    }

    fn current_unix_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn sample_auth_snapshot() -> GatewayAuthApiKeySnapshot {
        GatewayAuthApiKeySnapshot::from_stored(
            StoredAuthApiKeySnapshot::new(
                "user-1".to_string(),
                "alice".to_string(),
                Some("alice@example.com".to_string()),
                "user".to_string(),
                "local".to_string(),
                true,
                false,
                None,
                None,
                None,
                "api-key-1".to_string(),
                Some("default".to_string()),
                true,
                false,
                false,
                Some(60),
                Some(5),
                Some(4_102_444_800),
                None,
                None,
                None,
            )
            .expect("stored auth snapshot should build"),
            current_unix_secs(),
        )
    }

    fn sample_priority_candidate(
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        endpoint_api_format: &str,
        key_global_priority_for_format: Option<i32>,
        provider_priority: i32,
    ) -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: provider_id.to_string(),
            provider_name: provider_id.to_string(),
            provider_type: "custom".to_string(),
            provider_priority,
            endpoint_id: endpoint_id.to_string(),
            endpoint_api_format: endpoint_api_format.to_string(),
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "api_key".to_string(),
            key_internal_priority: 0,
            key_global_priority_for_format,
            key_capabilities: None,
            model_id: format!("model-{provider_id}"),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-4.1".to_string(),
            selected_provider_model_name: "gpt-4.1".to_string(),
            mapping_matched_model: None,
        }
    }

    #[tokio::test]
    async fn local_execution_ranking_keeps_provider_priority_before_tunnel_affinity() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-priority", false, 0),
                sample_provider_with_options("provider-local-tunnel", false, 10),
            ],
            vec![
                sample_endpoint_for_provider(
                    "provider-priority",
                    "endpoint-priority",
                    "openai:chat",
                ),
                sample_endpoint_for_provider(
                    "provider-local-tunnel",
                    "endpoint-local-tunnel",
                    "openai:chat",
                ),
            ],
            vec![
                sample_key_for_provider("provider-priority", "key-priority", "node-remote"),
                sample_key_for_provider("provider-local-tunnel", "key-local-tunnel", "node-local"),
            ],
        );
        let observed_at_unix_secs = current_unix_secs();
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        )
        .with_system_config_values_for_tests(vec![
            ("provider_priority_mode".to_string(), json!("provider")),
            (
                tunnel_attachment_key("node-remote"),
                serde_json::to_value(TunnelAttachmentRecord {
                    gateway_instance_id: "gateway-b".to_string(),
                    relay_base_url: "http://gateway-b:8080".to_string(),
                    conn_count: 1,
                    observed_at_unix_secs,
                })
                .expect("remote attachment should serialize"),
            ),
            (
                tunnel_attachment_key("node-local"),
                serde_json::to_value(TunnelAttachmentRecord {
                    gateway_instance_id: "gateway-a".to_string(),
                    relay_base_url: "http://gateway-a:8080".to_string(),
                    conn_count: 1,
                    observed_at_unix_secs,
                })
                .expect("local attachment should serialize"),
            ),
        ]);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_for_tests("gateway-a", Some("http://gateway-a:8080"));

        let ranked = rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-local-tunnel",
                    "endpoint-local-tunnel",
                    "key-local-tunnel",
                    "openai:chat",
                    Some(10),
                    10,
                ),
                sample_priority_candidate(
                    "provider-priority",
                    "endpoint-priority",
                    "key-priority",
                    "openai:chat",
                    Some(0),
                    0,
                ),
            ],
            "openai:chat",
            None,
        )
        .await;

        assert_eq!(ranked[0].provider_id, "provider-priority");
        assert_eq!(ranked[1].provider_id, "provider-local-tunnel");
    }

    #[tokio::test]
    async fn local_execution_ranking_demotes_cross_format_candidates_without_keep_priority() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-same", false, 0),
                sample_provider_with_options("provider-cross", false, 0),
            ],
            vec![
                sample_endpoint_for_provider("provider-same", "endpoint-same", "openai:chat"),
                sample_endpoint_for_provider("provider-cross", "endpoint-cross", "claude:chat"),
            ],
            vec![
                sample_key_for_provider("provider-same", "key-same", ""),
                sample_key_for_provider("provider-cross", "key-cross", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let ranked = rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-cross",
                    "endpoint-cross",
                    "key-cross",
                    "claude:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-same",
                    "endpoint-same",
                    "key-same",
                    "openai:chat",
                    Some(0),
                    0,
                ),
            ],
            "openai:chat",
            None,
        )
        .await;

        assert_eq!(ranked[0].endpoint_id, "endpoint-same");
        assert_eq!(ranked[1].endpoint_id, "endpoint-cross");
    }

    #[tokio::test]
    async fn fixed_order_local_execution_ranking_keeps_provider_priority_before_format_preference()
    {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-same", false, 10),
                sample_provider_with_options("provider-cross", false, 0),
            ],
            vec![
                sample_endpoint_for_provider("provider-same", "endpoint-same", "openai:chat"),
                sample_endpoint_for_provider("provider-cross", "endpoint-cross", "claude:chat"),
            ],
            vec![
                sample_key_for_provider("provider-same", "key-same", ""),
                sample_key_for_provider("provider-cross", "key-cross", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        )
        .with_system_config_values_for_tests(vec![(
            "scheduling_mode".to_string(),
            json!("fixed_order"),
        )]);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let ranked = rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-same",
                    "endpoint-same",
                    "key-same",
                    "openai:chat",
                    Some(10),
                    10,
                ),
                sample_priority_candidate(
                    "provider-cross",
                    "endpoint-cross",
                    "key-cross",
                    "claude:chat",
                    Some(0),
                    0,
                ),
            ],
            "openai:chat",
            None,
        )
        .await;

        assert_eq!(ranked[0].endpoint_id, "endpoint-cross");
        assert_eq!(ranked[1].endpoint_id, "endpoint-same");
    }

    #[tokio::test]
    async fn local_execution_ranking_keeps_cross_format_priority_when_enabled() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-same", false, 10),
                sample_provider_with_options("provider-cross", true, 0),
            ],
            vec![
                sample_endpoint_for_provider("provider-same", "endpoint-same", "openai:chat"),
                sample_endpoint_for_provider("provider-cross", "endpoint-cross", "claude:chat"),
            ],
            vec![
                sample_key_for_provider("provider-same", "key-same", ""),
                sample_key_for_provider("provider-cross", "key-cross", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let ranked = rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-cross",
                    "endpoint-cross",
                    "key-cross",
                    "claude:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-same",
                    "endpoint-same",
                    "key-same",
                    "openai:chat",
                    Some(10),
                    10,
                ),
            ],
            "openai:chat",
            None,
        )
        .await;

        assert_eq!(ranked[0].endpoint_id, "endpoint-cross");
        assert_eq!(ranked[1].endpoint_id, "endpoint-same");
    }

    #[tokio::test]
    async fn local_execution_ranking_keeps_cross_format_priority_when_global_override_is_enabled() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-same", false, 10),
                sample_provider_with_options("provider-cross", false, 0),
            ],
            vec![
                sample_endpoint_for_provider("provider-same", "endpoint-same", "openai:chat"),
                sample_endpoint_for_provider("provider-cross", "endpoint-cross", "claude:chat"),
            ],
            vec![
                sample_key_for_provider("provider-same", "key-same", ""),
                sample_key_for_provider("provider-cross", "key-cross", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        )
        .with_system_config_values_for_tests(vec![(
            "keep_priority_on_conversion".to_string(),
            json!(true),
        )]);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let ranked = rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-cross",
                    "endpoint-cross",
                    "key-cross",
                    "claude:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-same",
                    "endpoint-same",
                    "key-same",
                    "openai:chat",
                    Some(10),
                    10,
                ),
            ],
            "openai:chat",
            None,
        )
        .await;

        assert_eq!(ranked[0].endpoint_id, "endpoint-cross");
        assert_eq!(ranked[1].endpoint_id, "endpoint-same");
    }

    #[tokio::test]
    async fn local_execution_ranking_uses_provider_priority_mode_when_configured() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-provider-first", false, 0),
                sample_provider_with_options("provider-global-first", false, 10),
            ],
            vec![
                sample_endpoint_for_provider(
                    "provider-provider-first",
                    "endpoint-provider-first",
                    "openai:chat",
                ),
                sample_endpoint_for_provider(
                    "provider-global-first",
                    "endpoint-global-first",
                    "openai:chat",
                ),
            ],
            vec![
                sample_key_for_provider("provider-provider-first", "key-provider-first", ""),
                sample_key_for_provider("provider-global-first", "key-global-first", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        )
        .with_system_config_values_for_tests(vec![(
            "provider_priority_mode".to_string(),
            json!("provider"),
        )]);
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let ranked = rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-global-first",
                    "endpoint-global-first",
                    "key-global-first",
                    "openai:chat",
                    Some(0),
                    10,
                ),
                sample_priority_candidate(
                    "provider-provider-first",
                    "endpoint-provider-first",
                    "key-provider-first",
                    "openai:chat",
                    Some(10),
                    0,
                ),
            ],
            "openai:chat",
            None,
        )
        .await;

        assert_eq!(ranked[0].endpoint_id, "endpoint-provider-first");
        assert_eq!(ranked[1].endpoint_id, "endpoint-global-first");
    }

    #[tokio::test]
    async fn local_execution_ranking_prefers_same_kind_endpoint_for_same_key_candidates() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_with_options("provider-shared", false, 0)],
            vec![
                sample_endpoint_for_provider("provider-shared", "aaa-claude-chat", "claude:chat"),
                sample_endpoint_for_provider(
                    "provider-shared",
                    "zzz-openai-responses",
                    "openai:responses",
                ),
            ],
            vec![sample_key_for_provider_with_options(
                "provider-shared",
                "key-shared",
                "",
                true,
                Some(json!(["claude:chat", "openai:responses"])),
                None,
            )],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let ranked = rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-shared",
                    "aaa-claude-chat",
                    "key-shared",
                    "claude:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-shared",
                    "zzz-openai-responses",
                    "key-shared",
                    "openai:responses",
                    Some(0),
                    0,
                ),
            ],
            "claude:cli",
            None,
        )
        .await;

        assert_eq!(ranked[0].endpoint_id, "zzz-openai-responses");
        assert_eq!(ranked[1].endpoint_id, "aaa-claude-chat");
    }

    #[tokio::test]
    async fn local_execution_ranking_prefers_candidates_matching_requested_capabilities() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-miss", false, 0),
                sample_provider_with_options("provider-hit", false, 0),
            ],
            vec![
                sample_endpoint_for_provider("provider-miss", "endpoint-miss", "openai:chat"),
                sample_endpoint_for_provider("provider-hit", "endpoint-hit", "openai:chat"),
            ],
            vec![
                sample_key_for_provider("provider-miss", "key-miss", ""),
                sample_key_for_provider("provider-hit", "key-hit", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let mut candidate_miss = sample_priority_candidate(
            "provider-miss",
            "endpoint-miss",
            "key-miss",
            "openai:chat",
            Some(0),
            0,
        );
        let mut candidate_hit = sample_priority_candidate(
            "provider-hit",
            "endpoint-hit",
            "key-hit",
            "openai:chat",
            Some(0),
            0,
        );
        candidate_miss.key_capabilities = Some(json!({"cache_1h": false}));
        candidate_hit.key_capabilities = Some(json!({"cache_1h": true}));

        let required_capabilities = json!({"cache_1h": true});
        let ranked = rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![candidate_miss, candidate_hit],
            "openai:chat",
            Some(&required_capabilities),
        )
        .await;

        assert_eq!(ranked[0].endpoint_id, "endpoint-hit");
        assert_eq!(ranked[1].endpoint_id, "endpoint-miss");
    }

    #[tokio::test]
    async fn realtime_gate_skips_inactive_candidates_before_ranking() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-disabled", false, 0),
                sample_provider_with_options("provider-active", false, 10),
            ],
            vec![
                sample_endpoint_for_provider(
                    "provider-disabled",
                    "endpoint-disabled",
                    "openai:chat",
                ),
                sample_endpoint_for_provider("provider-active", "endpoint-active", "openai:chat"),
            ],
            vec![
                sample_key_for_provider_with_options(
                    "provider-disabled",
                    "key-disabled",
                    "",
                    false,
                    Some(json!(["openai:chat"])),
                    None,
                ),
                sample_key_for_provider_with_options(
                    "provider-active",
                    "key-active",
                    "",
                    true,
                    Some(json!(["openai:chat"])),
                    None,
                ),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-disabled",
                    "endpoint-disabled",
                    "key-disabled",
                    "openai:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-active",
                    "endpoint-active",
                    "key-active",
                    "openai:chat",
                    Some(10),
                    10,
                ),
            ],
            "openai:chat",
            "gpt-4.1",
            None,
            None,
            None,
        )
        .await;

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-active");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].candidate.endpoint_id, "endpoint-disabled");
        assert_eq!(skipped[0].skip_reason, "key_inactive");
    }

    #[tokio::test]
    async fn realtime_gate_skips_candidates_when_key_model_binding_is_disabled() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-restricted", false, 0),
                sample_provider_with_options("provider-open", false, 10),
            ],
            vec![
                sample_endpoint_for_provider(
                    "provider-restricted",
                    "endpoint-restricted",
                    "openai:chat",
                ),
                sample_endpoint_for_provider("provider-open", "endpoint-open", "openai:chat"),
            ],
            vec![
                sample_key_for_provider_with_options(
                    "provider-restricted",
                    "key-restricted",
                    "",
                    true,
                    Some(json!(["openai:chat"])),
                    Some(json!(["gpt-4o"])),
                ),
                sample_key_for_provider_with_options(
                    "provider-open",
                    "key-open",
                    "",
                    true,
                    Some(json!(["openai:chat"])),
                    None,
                ),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-restricted",
                    "endpoint-restricted",
                    "key-restricted",
                    "openai:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-open",
                    "endpoint-open",
                    "key-open",
                    "openai:chat",
                    Some(10),
                    10,
                ),
            ],
            "openai:chat",
            "gpt-4.1",
            None,
            None,
            None,
        )
        .await;

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-open");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].candidate.endpoint_id, "endpoint-restricted");
        assert_eq!(skipped[0].skip_reason, "key_model_disabled");
    }

    #[tokio::test]
    async fn realtime_gate_reports_cross_format_candidates_when_conversion_is_disabled() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-cross", true, 0),
                sample_provider_with_options("provider-same", false, 10),
            ],
            vec![
                sample_endpoint_for_provider("provider-cross", "endpoint-cross", "claude:chat"),
                sample_endpoint_for_provider("provider-same", "endpoint-same", "openai:chat"),
            ],
            vec![
                sample_key_for_provider_with_options(
                    "provider-cross",
                    "key-cross",
                    "",
                    true,
                    Some(json!(["claude:chat"])),
                    None,
                ),
                sample_key_for_provider_with_options(
                    "provider-same",
                    "key-same",
                    "",
                    true,
                    Some(json!(["openai:chat"])),
                    None,
                ),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-cross",
                    "endpoint-cross",
                    "key-cross",
                    "claude:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-same",
                    "endpoint-same",
                    "key-same",
                    "openai:chat",
                    Some(10),
                    10,
                ),
            ],
            "openai:chat",
            "gpt-4.1",
            None,
            None,
            None,
        )
        .await;

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-same");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].candidate.endpoint_id, "endpoint-cross");
        assert_eq!(skipped[0].skip_reason, "format_conversion_disabled");
    }

    #[tokio::test]
    async fn realtime_gate_reports_cross_format_disablement_when_same_key_has_exact_endpoint() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider_with_options("provider-shared", false, 0)],
            vec![
                sample_endpoint_for_provider("provider-shared", "endpoint-exact", "openai:chat"),
                sample_endpoint_for_provider("provider-shared", "endpoint-cross", "claude:chat"),
            ],
            vec![sample_key_for_provider_with_options(
                "provider-shared",
                "key-shared",
                "",
                true,
                Some(json!(["openai:chat", "claude:chat"])),
                None,
            )],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-shared",
                    "endpoint-exact",
                    "key-shared",
                    "openai:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-shared",
                    "endpoint-cross",
                    "key-shared",
                    "claude:chat",
                    Some(0),
                    0,
                ),
            ],
            "openai:chat",
            "gpt-4.1",
            None,
            None,
            None,
        )
        .await;

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-exact");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].candidate.endpoint_id, "endpoint-cross");
        assert_eq!(skipped[0].skip_reason, "format_conversion_disabled");
    }

    #[tokio::test]
    async fn realtime_gate_allows_cross_format_candidates_when_endpoint_acceptance_is_enabled() {
        let mut endpoint_cross =
            sample_endpoint_for_provider("provider-cross", "endpoint-cross", "claude:chat");
        endpoint_cross.format_acceptance_config = Some(json!({
            "enabled": true,
            "accept_formats": ["openai:chat"],
        }));

        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-cross", false, 0),
                sample_provider_with_options("provider-same", false, 10),
            ],
            vec![
                endpoint_cross,
                sample_endpoint_for_provider("provider-same", "endpoint-same", "openai:chat"),
            ],
            vec![
                sample_key_for_provider_with_options(
                    "provider-cross",
                    "key-cross",
                    "",
                    true,
                    Some(json!(["claude:chat"])),
                    None,
                ),
                sample_key_for_provider_with_options(
                    "provider-same",
                    "key-same",
                    "",
                    true,
                    Some(json!(["openai:chat"])),
                    None,
                ),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-cross",
                    "endpoint-cross",
                    "key-cross",
                    "claude:chat",
                    Some(0),
                    0,
                ),
                sample_priority_candidate(
                    "provider-same",
                    "endpoint-same",
                    "key-same",
                    "openai:chat",
                    Some(10),
                    10,
                ),
            ],
            "openai:chat",
            "gpt-4.1",
            None,
            None,
            None,
        )
        .await;

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-same");
        assert_eq!(ranked[1].candidate.endpoint_id, "endpoint-cross");
        assert!(skipped.is_empty());
    }

    #[tokio::test]
    async fn local_execution_ranking_reports_cached_affinity_promotion() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-priority", false, 0),
                sample_provider_with_options("provider-cached", false, 10),
            ],
            vec![
                sample_endpoint_for_provider(
                    "provider-priority",
                    "endpoint-priority",
                    "openai:chat",
                ),
                sample_endpoint_for_provider("provider-cached", "endpoint-cached", "openai:chat"),
            ],
            vec![
                sample_key_for_provider("provider-priority", "key-priority", ""),
                sample_key_for_provider("provider-cached", "key-cached", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = sample_auth_snapshot();
        let cached_candidate = sample_priority_candidate(
            "provider-cached",
            "endpoint-cached",
            "key-cached",
            "openai:chat",
            Some(10),
            10,
        );
        remember_scheduler_affinity_for_candidate(
            PlannerAppState::new(&state),
            Some(&auth_snapshot),
            "openai:chat",
            "gpt-4.1",
            &cached_candidate,
        );

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_priority_candidate(
                    "provider-priority",
                    "endpoint-priority",
                    "key-priority",
                    "openai:chat",
                    Some(0),
                    0,
                ),
                cached_candidate,
            ],
            "openai:chat",
            "gpt-4.1",
            Some(&auth_snapshot),
            None,
            None,
        )
        .await;

        assert!(skipped.is_empty());
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-cached");
        assert_eq!(
            ranked[0]
                .ranking
                .as_ref()
                .and_then(|ranking| ranking.promoted_by),
            Some(RANKING_REASON_CACHED_AFFINITY)
        );
    }

    #[tokio::test]
    async fn non_pool_key_affinity_does_not_promote_sibling_key_when_cached_key_is_inactive() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-priority", false, 0),
                sample_provider_with_options("provider-cached", false, 10),
            ],
            vec![
                sample_endpoint_for_provider(
                    "provider-priority",
                    "endpoint-priority",
                    "openai:chat",
                ),
                sample_endpoint_for_provider("provider-cached", "endpoint-cached", "openai:chat"),
            ],
            vec![
                sample_key_for_provider("provider-priority", "key-priority", ""),
                sample_key_for_provider_with_options(
                    "provider-cached",
                    "key-cached",
                    "",
                    false,
                    Some(json!(["openai:chat"])),
                    None,
                ),
                sample_key_for_provider("provider-cached", "key-sibling", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = sample_auth_snapshot();
        let cached_candidate = sample_priority_candidate(
            "provider-cached",
            "endpoint-cached",
            "key-cached",
            "openai:chat",
            Some(10),
            10,
        );
        remember_scheduler_affinity_for_candidate(
            PlannerAppState::new(&state),
            Some(&auth_snapshot),
            "openai:chat",
            "gpt-4.1",
            &cached_candidate,
        );

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                cached_candidate,
                sample_priority_candidate(
                    "provider-cached",
                    "endpoint-cached",
                    "key-sibling",
                    "openai:chat",
                    Some(10),
                    10,
                ),
                sample_priority_candidate(
                    "provider-priority",
                    "endpoint-priority",
                    "key-priority",
                    "openai:chat",
                    Some(0),
                    0,
                ),
            ],
            "openai:chat",
            "gpt-4.1",
            Some(&auth_snapshot),
            None,
            None,
        )
        .await;

        assert_eq!(ranked[0].candidate.key_id, "key-priority");
        assert_eq!(ranked[1].candidate.key_id, "key-sibling");
        assert!(ranked[1]
            .ranking
            .as_ref()
            .is_none_or(|ranking| ranking.promoted_by.is_none()));
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![("key-cached", "key_inactive")]
        );
    }

    #[tokio::test]
    async fn pool_key_affinity_promotes_pool_group_when_cached_key_is_inactive() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-priority", false, 0),
                sample_provider_with_config(
                    "provider-pool",
                    false,
                    10,
                    Some(json!({ "pool_advanced": {} })),
                ),
            ],
            vec![
                sample_endpoint_for_provider(
                    "provider-priority",
                    "endpoint-priority",
                    "openai:chat",
                ),
                sample_endpoint_for_provider("provider-pool", "endpoint-pool", "openai:chat"),
            ],
            vec![
                sample_key_for_provider("provider-priority", "key-priority", ""),
                sample_key_for_provider_with_options(
                    "provider-pool",
                    "key-cached",
                    "",
                    false,
                    Some(json!(["openai:chat"])),
                    None,
                ),
                sample_key_for_provider("provider-pool", "key-fallback", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = sample_auth_snapshot();
        let cached_candidate = sample_priority_candidate(
            "provider-pool",
            "endpoint-pool",
            "key-cached",
            "openai:chat",
            Some(10),
            10,
        );
        remember_scheduler_affinity_for_candidate(
            PlannerAppState::new(&state),
            Some(&auth_snapshot),
            "openai:chat",
            "gpt-4.1",
            &cached_candidate,
        );

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                cached_candidate,
                sample_priority_candidate(
                    "provider-pool",
                    "endpoint-pool",
                    "key-fallback",
                    "openai:chat",
                    Some(10),
                    10,
                ),
                sample_priority_candidate(
                    "provider-priority",
                    "endpoint-priority",
                    "key-priority",
                    "openai:chat",
                    Some(0),
                    0,
                ),
            ],
            "openai:chat",
            "gpt-4.1",
            Some(&auth_snapshot),
            None,
            None,
        )
        .await;

        assert_eq!(ranked[0].candidate.key_id, "key-fallback");
        assert_eq!(ranked[0].orchestration.pool_key_index, Some(0));
        assert_eq!(
            ranked[0]
                .ranking
                .as_ref()
                .and_then(|ranking| ranking.promoted_by),
            Some(RANKING_REASON_CACHED_AFFINITY)
        );
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![("key-cached", "key_inactive")]
        );
    }

    #[tokio::test]
    async fn pool_key_affinity_promotes_pool_group_when_cached_key_is_blocked() {
        let mut cached_key = sample_key_for_provider("provider-pool", "key-cached", "");
        cached_key.oauth_invalid_reason =
            Some("[ACCOUNT_BLOCK] account has been deactivated".to_string());

        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider_with_options("provider-priority", false, 0),
                sample_provider_with_config(
                    "provider-pool",
                    false,
                    10,
                    Some(json!({ "pool_advanced": {} })),
                ),
            ],
            vec![
                sample_endpoint_for_provider(
                    "provider-priority",
                    "endpoint-priority",
                    "openai:chat",
                ),
                sample_endpoint_for_provider("provider-pool", "endpoint-pool", "openai:chat"),
            ],
            vec![
                sample_key_for_provider("provider-priority", "key-priority", ""),
                cached_key,
                sample_key_for_provider("provider-pool", "key-fallback", ""),
            ],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let auth_snapshot = sample_auth_snapshot();
        let cached_candidate = sample_priority_candidate(
            "provider-pool",
            "endpoint-pool",
            "key-cached",
            "openai:chat",
            Some(10),
            10,
        );
        remember_scheduler_affinity_for_candidate(
            PlannerAppState::new(&state),
            Some(&auth_snapshot),
            "openai:chat",
            "gpt-4.1",
            &cached_candidate,
        );

        let (ranked, skipped) = resolve_and_rank_local_execution_candidates(
            PlannerAppState::new(&state),
            vec![
                cached_candidate,
                sample_priority_candidate(
                    "provider-pool",
                    "endpoint-pool",
                    "key-fallback",
                    "openai:chat",
                    Some(10),
                    10,
                ),
                sample_priority_candidate(
                    "provider-priority",
                    "endpoint-priority",
                    "key-priority",
                    "openai:chat",
                    Some(0),
                    0,
                ),
            ],
            "openai:chat",
            "gpt-4.1",
            Some(&auth_snapshot),
            None,
            None,
        )
        .await;

        assert_eq!(ranked[0].candidate.key_id, "key-fallback");
        assert_eq!(
            ranked[0]
                .ranking
                .as_ref()
                .and_then(|ranking| ranking.promoted_by),
            Some(RANKING_REASON_CACHED_AFFINITY)
        );
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![("key-cached", "pool_account_blocked")]
        );
    }

    #[tokio::test]
    async fn remembers_scheduler_affinity_for_candidate_using_requested_model_key() {
        let state = AppState::new().expect("state should build");
        let auth_snapshot = sample_auth_snapshot();
        let candidate = sample_candidate("endpoint-1", "key-1");

        remember_scheduler_affinity_for_candidate(
            PlannerAppState::new(&state),
            Some(&auth_snapshot),
            "openai:chat",
            "gpt-5",
            &candidate,
        );

        let remembered = state
            .read_scheduler_affinity_target(
                "scheduler_affinity:api-key-1:openai:chat:gpt-5",
                SCHEDULER_AFFINITY_TTL,
            )
            .expect("affinity target should be cached");
        assert_eq!(remembered.provider_id, "provider-1");
        assert_eq!(remembered.endpoint_id, "endpoint-1");
        assert_eq!(remembered.key_id, "key-1");
    }
}
