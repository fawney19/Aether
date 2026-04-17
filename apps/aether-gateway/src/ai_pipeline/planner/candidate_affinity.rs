use tracing::warn;

use crate::ai_pipeline::{
    request_candidate_api_format_preference, GatewayAuthApiKeySnapshot,
    GatewayProviderTransportSnapshot, PlannerAppState,
};
use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
use crate::scheduler::config::{read_scheduler_ordering_config, SchedulerOrderingConfig};
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use aether_scheduler_core::{
    build_scheduler_affinity_cache_key_for_api_key_id, compare_candidates_by_priority_mode,
    requested_capability_priority_for_candidate, SchedulerAffinityTarget, SchedulerPriorityMode,
};

use super::candidate_eligibility::{
    read_candidate_transport_snapshot, EligibleLocalExecutionCandidate,
};

const PLANNER_SCHEDULER_AFFINITY_MAX_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TunnelOwnerAffinityBucket {
    LocalTunnel = 0,
    Neutral = 1,
    RemoteTunnel = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CandidateExecutionOrdering {
    tunnel_bucket: TunnelOwnerAffinityBucket,
    keep_priority_on_conversion: bool,
}

pub(crate) async fn prefer_local_tunnel_owner_candidates(
    state: PlannerAppState<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
) -> Vec<SchedulerMinimalCandidateSelectionCandidate> {
    let mut ranked = Vec::with_capacity(candidates.len());
    for (original_index, candidate) in candidates.into_iter().enumerate() {
        let bucket = resolve_candidate_tunnel_owner_affinity(state, &candidate).await;
        ranked.push((bucket, original_index, candidate));
    }
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    ranked
        .into_iter()
        .map(|(_, _, candidate)| candidate)
        .collect()
}

#[cfg(test)]
async fn rank_local_execution_candidates(
    state: PlannerAppState<'_>,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    client_api_format: &str,
    required_capabilities: Option<&serde_json::Value>,
) -> Vec<SchedulerMinimalCandidateSelectionCandidate> {
    let normalized_client_api_format = client_api_format.trim().to_ascii_lowercase();
    let ordering_config = read_scheduler_ordering_config_or_default(state).await;
    let mut ranked = Vec::with_capacity(candidates.len());

    for (original_index, candidate) in candidates.into_iter().enumerate() {
        let ordering =
            resolve_candidate_execution_ordering(state, &candidate, ordering_config).await;
        let is_same_format = candidate
            .endpoint_api_format
            .trim()
            .eq_ignore_ascii_case(normalized_client_api_format.as_str());
        let demote_cross_format = !is_same_format && !ordering.keep_priority_on_conversion;
        let format_preference = candidate_api_format_preference(
            normalized_client_api_format.as_str(),
            candidate.endpoint_api_format.as_str(),
        );
        let capability_priority =
            requested_capability_priority_for_candidate(required_capabilities, &candidate);
        ranked.push((
            capability_priority.0,
            capability_priority.1,
            ordering.tunnel_bucket,
            demote_cross_format,
            format_preference,
            original_index,
            candidate,
        ));
    }

    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
            .then(left.3.cmp(&right.3))
            .then_with(|| {
                compare_candidate_priority_slot(&left.6, &right.6, ordering_config.priority_mode)
            })
            .then(left.4.cmp(&right.4))
            .then_with(|| {
                compare_candidates_by_priority_mode(
                    &left.6,
                    &right.6,
                    ordering_config.priority_mode,
                    None,
                )
            })
            .then(left.5.cmp(&right.5))
    });

    ranked
        .into_iter()
        .map(|(_, _, _, _, _, _, candidate)| candidate)
        .collect()
}

pub(crate) async fn rank_eligible_local_execution_candidates(
    state: PlannerAppState<'_>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    client_api_format: &str,
    required_capabilities: Option<&serde_json::Value>,
) -> Vec<EligibleLocalExecutionCandidate> {
    let normalized_client_api_format = client_api_format.trim().to_ascii_lowercase();
    let ordering_config = read_scheduler_ordering_config_or_default(state).await;
    let mut ranked = Vec::with_capacity(candidates.len());

    for (original_index, eligible) in candidates.into_iter().enumerate() {
        let ordering = resolve_candidate_execution_ordering_from_transport(
            state,
            &eligible.transport,
            ordering_config,
        )
        .await;
        let is_same_format = eligible
            .provider_api_format
            .eq_ignore_ascii_case(normalized_client_api_format.as_str());
        let demote_cross_format = !is_same_format && !ordering.keep_priority_on_conversion;
        let format_preference = candidate_api_format_preference(
            normalized_client_api_format.as_str(),
            eligible.provider_api_format.as_str(),
        );
        let capability_priority =
            requested_capability_priority_for_candidate(required_capabilities, &eligible.candidate);
        ranked.push((
            capability_priority.0,
            capability_priority.1,
            ordering.tunnel_bucket,
            demote_cross_format,
            format_preference,
            original_index,
            eligible,
        ));
    }

    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then(left.2.cmp(&right.2))
            .then(left.3.cmp(&right.3))
            .then_with(|| {
                compare_candidate_priority_slot(
                    &left.6.candidate,
                    &right.6.candidate,
                    ordering_config.priority_mode,
                )
            })
            .then(left.4.cmp(&right.4))
            .then_with(|| {
                compare_candidates_by_priority_mode(
                    &left.6.candidate,
                    &right.6.candidate,
                    ordering_config.priority_mode,
                    None,
                )
            })
            .then(left.5.cmp(&right.5))
    });

    ranked
        .into_iter()
        .map(|(_, _, _, _, _, _, eligible)| eligible)
        .collect()
}

pub(crate) fn remember_scheduler_affinity_for_candidate(
    state: PlannerAppState<'_>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    client_api_format: &str,
    requested_model: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) {
    let Some(api_key_id) = auth_snapshot
        .map(|snapshot| snapshot.api_key_id.trim())
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let Some(cache_key) = build_scheduler_affinity_cache_key_for_api_key_id(
        api_key_id,
        client_api_format,
        requested_model,
    ) else {
        return;
    };

    state.app().remember_scheduler_affinity_target(
        &cache_key,
        SchedulerAffinityTarget {
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
        },
        SCHEDULER_AFFINITY_TTL,
        PLANNER_SCHEDULER_AFFINITY_MAX_ENTRIES,
    );
}

async fn resolve_candidate_tunnel_owner_affinity(
    state: PlannerAppState<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> TunnelOwnerAffinityBucket {
    let Some(transport) = read_candidate_transport_snapshot(state, candidate).await else {
        return TunnelOwnerAffinityBucket::Neutral;
    };

    resolve_tunnel_owner_affinity_from_transport(state, &transport).await
}

async fn resolve_candidate_execution_ordering(
    state: PlannerAppState<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    ordering_config: SchedulerOrderingConfig,
) -> CandidateExecutionOrdering {
    let Some(transport) = read_candidate_transport_snapshot(state, candidate).await else {
        return CandidateExecutionOrdering {
            tunnel_bucket: TunnelOwnerAffinityBucket::Neutral,
            keep_priority_on_conversion: ordering_config.keep_priority_on_conversion,
        };
    };

    resolve_candidate_execution_ordering_from_transport(state, &transport, ordering_config).await
}

async fn resolve_candidate_execution_ordering_from_transport(
    state: PlannerAppState<'_>,
    transport: &GatewayProviderTransportSnapshot,
    ordering_config: SchedulerOrderingConfig,
) -> CandidateExecutionOrdering {
    CandidateExecutionOrdering {
        tunnel_bucket: resolve_tunnel_owner_affinity_from_transport(state, transport).await,
        keep_priority_on_conversion: ordering_config.keep_priority_on_conversion
            || transport.provider.keep_priority_on_conversion,
    }
}

async fn resolve_tunnel_owner_affinity_from_transport(
    state: PlannerAppState<'_>,
    transport: &GatewayProviderTransportSnapshot,
) -> TunnelOwnerAffinityBucket {
    let Some(proxy) = state
        .app()
        .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
        .await
    else {
        return TunnelOwnerAffinityBucket::Neutral;
    };
    if proxy.enabled == Some(false) {
        return TunnelOwnerAffinityBucket::Neutral;
    }
    let Some(node_id) = proxy
        .node_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return TunnelOwnerAffinityBucket::Neutral;
    };

    if state.app().tunnel.has_local_proxy(node_id) {
        return TunnelOwnerAffinityBucket::LocalTunnel;
    }

    match state
        .app()
        .tunnel
        .lookup_attachment_owner(state.app().data.as_ref(), node_id)
        .await
    {
        Ok(Some(owner)) if owner.gateway_instance_id == state.app().tunnel.local_instance_id() => {
            TunnelOwnerAffinityBucket::LocalTunnel
        }
        Ok(Some(_)) => TunnelOwnerAffinityBucket::RemoteTunnel,
        Ok(None) => TunnelOwnerAffinityBucket::Neutral,
        Err(error) => {
            warn!(
                event_name = "candidate_affinity_tunnel_owner_lookup_failed",
                log_type = "event",
                node_id = node_id,
                error = %error,
                "failed to load tunnel attachment owner while evaluating scheduler affinity"
            );
            TunnelOwnerAffinityBucket::Neutral
        }
    }
}

fn candidate_api_format_preference(client_api_format: &str, provider_api_format: &str) -> (u8, u8) {
    request_candidate_api_format_preference(client_api_format, provider_api_format)
        .unwrap_or((u8::MAX, u8::MAX))
}

fn compare_candidate_priority_slot(
    left: &SchedulerMinimalCandidateSelectionCandidate,
    right: &SchedulerMinimalCandidateSelectionCandidate,
    priority_mode: SchedulerPriorityMode,
) -> std::cmp::Ordering {
    match priority_mode {
        SchedulerPriorityMode::Provider => left
            .provider_priority
            .cmp(&right.provider_priority)
            .then(left.key_internal_priority.cmp(&right.key_internal_priority)),
        SchedulerPriorityMode::GlobalKey => left
            .key_global_priority_for_format
            .unwrap_or(i32::MAX)
            .cmp(&right.key_global_priority_for_format.unwrap_or(i32::MAX))
            .then(left.provider_priority.cmp(&right.provider_priority))
            .then(left.key_internal_priority.cmp(&right.key_internal_priority)),
    }
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::json;

    use super::{
        prefer_local_tunnel_owner_candidates, rank_local_execution_candidates,
        remember_scheduler_affinity_for_candidate, PlannerAppState,
        SchedulerMinimalCandidateSelectionCandidate,
    };
    use crate::ai_pipeline::planner::candidate_eligibility::filter_and_rank_local_execution_candidates;
    use crate::data::auth::GatewayAuthApiKeySnapshot;
    use crate::data::GatewayDataState;
    use crate::tunnel::TunnelAttachmentRecord;
    use crate::{scheduler::affinity::SCHEDULER_AFFINITY_TTL, AppState};
    use aether_data::repository::auth::StoredAuthApiKeySnapshot;

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
            None,
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
    async fn prefers_local_tunnel_owner_candidates_before_remote_tunnel_candidates() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider()],
            vec![
                sample_endpoint("endpoint-remote"),
                sample_endpoint("endpoint-local"),
            ],
            vec![
                sample_key("key-remote", "node-remote"),
                sample_key("key-local", "node-local"),
            ],
        );
        let observed_at_unix_secs = current_unix_secs();
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        )
        .with_system_config_values_for_tests(vec![
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

        let reordered = prefer_local_tunnel_owner_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_candidate("endpoint-remote", "key-remote"),
                sample_candidate("endpoint-local", "key-local"),
            ],
        )
        .await;

        assert_eq!(reordered[0].endpoint_id, "endpoint-local");
        assert_eq!(reordered[1].endpoint_id, "endpoint-remote");
    }

    #[tokio::test]
    async fn leaves_candidate_order_unchanged_when_transport_has_no_tunnel_proxy() {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider()],
            vec![sample_endpoint("endpoint-a"), sample_endpoint("endpoint-b")],
            vec![sample_key("key-a", ""), sample_key("key-b", "")],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_for_tests("gateway-a", Some("http://gateway-a:8080"));

        let reordered = prefer_local_tunnel_owner_candidates(
            PlannerAppState::new(&state),
            vec![
                sample_candidate("endpoint-a", "key-a"),
                sample_candidate("endpoint-b", "key-b"),
            ],
        )
        .await;

        assert_eq!(reordered[0].endpoint_id, "endpoint-a");
        assert_eq!(reordered[1].endpoint_id, "endpoint-b");
    }

    #[tokio::test]
    async fn local_execution_ranking_demotes_cross_format_candidates_without_keep_priority() {
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

        assert_eq!(ranked[0].endpoint_id, "endpoint-same");
        assert_eq!(ranked[1].endpoint_id, "endpoint-cross");
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
                sample_endpoint_for_provider("provider-shared", "zzz-openai-cli", "openai:cli"),
            ],
            vec![sample_key_for_provider_with_options(
                "provider-shared",
                "key-shared",
                "",
                true,
                Some(json!(["claude:chat", "openai:cli"])),
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
                    "zzz-openai-cli",
                    "key-shared",
                    "openai:cli",
                    Some(0),
                    0,
                ),
            ],
            "claude:cli",
            None,
        )
        .await;

        assert_eq!(ranked[0].endpoint_id, "zzz-openai-cli");
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

        let (ranked, skipped) = filter_and_rank_local_execution_candidates(
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

        let (ranked, skipped) = filter_and_rank_local_execution_candidates(
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
        )
        .await;

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-open");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].candidate.endpoint_id, "endpoint-restricted");
        assert_eq!(skipped[0].skip_reason, "key_model_disabled");
    }

    #[tokio::test]
    async fn realtime_gate_skips_cross_format_candidates_when_conversion_is_disabled() {
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

        let (ranked, skipped) = filter_and_rank_local_execution_candidates(
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
        )
        .await;

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-same");
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

        let (ranked, skipped) = filter_and_rank_local_execution_candidates(
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
        )
        .await;

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].candidate.endpoint_id, "endpoint-same");
        assert_eq!(ranked[1].candidate.endpoint_id, "endpoint-cross");
        assert!(skipped.is_empty());
    }

    #[tokio::test]
    async fn remembers_scheduler_affinity_for_candidate_using_requested_model_key() {
        let state = AppState::new().expect("state should build");
        let auth_snapshot = GatewayAuthApiKeySnapshot::from_stored(
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
        );
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
