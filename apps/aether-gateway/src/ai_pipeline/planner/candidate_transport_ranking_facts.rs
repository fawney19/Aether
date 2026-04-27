use std::collections::BTreeMap;

use aether_scheduler_core::{
    SchedulerMinimalCandidateSelectionCandidate, SchedulerTunnelAffinityBucket,
};
use tracing::warn;

use crate::ai_pipeline::{GatewayProviderTransportSnapshot, PlannerAppState};
use crate::scheduler::config::SchedulerOrderingConfig;

use super::candidate_resolution::read_candidate_transport_snapshot;

pub(super) type CandidateTransportIdentity<'a> = (&'a str, &'a str, &'a str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CandidateTransportRankingFacts {
    pub(super) tunnel_bucket: SchedulerTunnelAffinityBucket,
    pub(super) keep_priority_on_conversion: bool,
}

pub(super) async fn resolve_cached_candidate_transport_ranking_facts<'a>(
    state: PlannerAppState<'_>,
    cache: &mut BTreeMap<CandidateTransportIdentity<'a>, CandidateTransportRankingFacts>,
    candidate: &'a SchedulerMinimalCandidateSelectionCandidate,
    ordering_config: SchedulerOrderingConfig,
) -> CandidateTransportRankingFacts {
    let identity = candidate_transport_identity(candidate);
    if let Some(facts) = cache.get(&identity).copied() {
        return facts;
    }

    let facts = resolve_candidate_transport_ranking_facts(state, candidate, ordering_config).await;
    cache.insert(identity, facts);
    facts
}

pub(super) async fn resolve_cached_transport_ranking_facts<'a>(
    state: PlannerAppState<'_>,
    cache: &mut BTreeMap<CandidateTransportIdentity<'a>, CandidateTransportRankingFacts>,
    candidate: &'a SchedulerMinimalCandidateSelectionCandidate,
    transport: &GatewayProviderTransportSnapshot,
    ordering_config: SchedulerOrderingConfig,
) -> CandidateTransportRankingFacts {
    let identity = candidate_transport_identity(candidate);
    if let Some(facts) = cache.get(&identity).copied() {
        return facts;
    }

    let facts =
        resolve_candidate_transport_ranking_facts_from_transport(state, transport, ordering_config)
            .await;
    cache.insert(identity, facts);
    facts
}

async fn resolve_candidate_transport_ranking_facts(
    state: PlannerAppState<'_>,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    ordering_config: SchedulerOrderingConfig,
) -> CandidateTransportRankingFacts {
    let Some(transport) = read_candidate_transport_snapshot(state, candidate).await else {
        return CandidateTransportRankingFacts {
            tunnel_bucket: SchedulerTunnelAffinityBucket::Neutral,
            keep_priority_on_conversion: ordering_config.keep_priority_on_conversion,
        };
    };

    resolve_candidate_transport_ranking_facts_from_transport(state, &transport, ordering_config)
        .await
}

async fn resolve_candidate_transport_ranking_facts_from_transport(
    state: PlannerAppState<'_>,
    transport: &GatewayProviderTransportSnapshot,
    ordering_config: SchedulerOrderingConfig,
) -> CandidateTransportRankingFacts {
    CandidateTransportRankingFacts {
        tunnel_bucket: resolve_tunnel_owner_affinity_from_transport(state, transport).await,
        keep_priority_on_conversion: ordering_config.keep_priority_on_conversion
            || transport.provider.keep_priority_on_conversion,
    }
}

async fn resolve_tunnel_owner_affinity_from_transport(
    state: PlannerAppState<'_>,
    transport: &GatewayProviderTransportSnapshot,
) -> SchedulerTunnelAffinityBucket {
    let Some(proxy) = state
        .app()
        .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
        .await
    else {
        return SchedulerTunnelAffinityBucket::Neutral;
    };
    if proxy.enabled == Some(false) {
        return SchedulerTunnelAffinityBucket::Neutral;
    }
    let Some(node_id) = proxy
        .node_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return SchedulerTunnelAffinityBucket::Neutral;
    };

    if state.app().tunnel.has_local_proxy(node_id) {
        return SchedulerTunnelAffinityBucket::LocalTunnel;
    }

    match state
        .app()
        .tunnel
        .lookup_attachment_owner(state.app().data.as_ref(), node_id)
        .await
    {
        Ok(Some(owner)) if owner.gateway_instance_id == state.app().tunnel.local_instance_id() => {
            SchedulerTunnelAffinityBucket::LocalTunnel
        }
        Ok(Some(_)) => SchedulerTunnelAffinityBucket::RemoteTunnel,
        Ok(None) => SchedulerTunnelAffinityBucket::Neutral,
        Err(error) => {
            warn!(
                event_name = "candidate_transport_ranking_facts_tunnel_owner_lookup_failed",
                log_type = "event",
                node_id = node_id,
                error = %error,
                "failed to load tunnel attachment owner while evaluating candidate transport ranking facts"
            );
            SchedulerTunnelAffinityBucket::Neutral
        }
    }
}

fn candidate_transport_identity(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> CandidateTransportIdentity<'_> {
    (
        candidate.provider_id.as_str(),
        candidate.endpoint_id.as_str(),
        candidate.key_id.as_str(),
    )
}
