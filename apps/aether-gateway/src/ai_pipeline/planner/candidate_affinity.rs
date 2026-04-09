use tracing::warn;

use crate::ai_pipeline::transport::resolve_transport_proxy_snapshot;
use crate::ai_pipeline::PlannerAppState;
use crate::AppState;
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TunnelOwnerAffinityBucket {
    LocalTunnel = 0,
    Neutral = 1,
    RemoteTunnel = 2,
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

async fn resolve_candidate_tunnel_owner_affinity(
    state: PlannerAppState<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> TunnelOwnerAffinityBucket {
    let transport = match state
        .read_provider_transport_snapshot(
            &candidate.provider_id,
            &candidate.endpoint_id,
            &candidate.key_id,
        )
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return TunnelOwnerAffinityBucket::Neutral,
        Err(error) => {
            warn!(
                event_name = "candidate_affinity_transport_load_failed",
                log_type = "event",
                provider_id = %candidate.provider_id,
                endpoint_id = %candidate.endpoint_id,
                key_id = %candidate.key_id,
                error = ?error,
                "failed to load provider transport while evaluating tunnel owner affinity"
            );
            return TunnelOwnerAffinityBucket::Neutral;
        }
    };

    let Some(proxy) = resolve_transport_proxy_snapshot(&transport) else {
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

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::json;

    use super::{
        prefer_local_tunnel_owner_candidates, AppState, PlannerAppState,
        SchedulerMinimalCandidateSelectionCandidate,
    };
    use crate::data::GatewayDataState;
    use crate::tunnel::TunnelAttachmentRecord;

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
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "provider-1".to_string(),
            Some("https://provider.example".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(true, false, false, None, None, None, None, None, None)
    }

    fn sample_endpoint(id: &str) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            id.to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
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
        StoredProviderCatalogKey::new(
            id.to_string(),
            "provider-1".to_string(),
            id.to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:chat"])),
            "plain-upstream-key".to_string(),
            None,
            None,
            Some(json!({"openai:chat": 1})),
            None,
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
}
