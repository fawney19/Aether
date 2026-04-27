use aether_scheduler_core::{
    apply_scheduler_candidate_ranking, effective_provider_key_health_score,
    provider_key_health_bucket, requested_capability_priority_for_candidate,
    SchedulerAffinityTarget, SchedulerRankableCandidate, SchedulerRankingContext,
    SchedulerRankingMode,
};

use crate::scheduler::config::{SchedulerOrderingConfig, SchedulerSchedulingMode};

use super::affinity::{
    scheduler_candidate_affinity_hash, scheduler_candidate_matches_affinity_target,
};
use super::runtime::CandidateRuntimeSelectionSnapshot;
use super::SchedulerMinimalCandidateSelectionCandidate;

pub(super) fn rank_scheduler_candidates(
    candidates: &mut [SchedulerMinimalCandidateSelectionCandidate],
    runtime_snapshot: &CandidateRuntimeSelectionSnapshot,
    ordering_config: SchedulerOrderingConfig,
    required_capabilities: Option<&serde_json::Value>,
    priority_affinity_key: Option<&str>,
    cached_affinity_target: Option<&SchedulerAffinityTarget>,
    now_unix_secs: u64,
) {
    let rankables = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let provider_key = runtime_snapshot
                .provider_key_rpm_states
                .get(&candidate.key_id);
            SchedulerRankableCandidate::from_candidate(candidate, index)
                .with_capability_priority(requested_capability_priority_for_candidate(
                    required_capabilities,
                    candidate,
                ))
                .with_cached_affinity_match(cached_affinity_target.is_some_and(|target| {
                    scheduler_candidate_matches_affinity_target(candidate, target)
                }))
                .with_affinity_hash(
                    priority_affinity_key
                        .map(|key| scheduler_candidate_affinity_hash(key, candidate)),
                )
                .with_health(
                    provider_key.and_then(|key| {
                        provider_key_health_bucket(key, candidate.endpoint_api_format.as_str())
                    }),
                    provider_key
                        .and_then(|key| {
                            effective_provider_key_health_score(
                                key,
                                candidate.endpoint_api_format.as_str(),
                            )
                        })
                        .unwrap_or(1.0),
                )
        })
        .collect::<Vec<_>>();

    apply_scheduler_candidate_ranking(
        candidates,
        &rankables,
        SchedulerRankingContext {
            priority_mode: ordering_config.priority_mode,
            ranking_mode: scheduler_ranking_mode(ordering_config.scheduling_mode),
            include_health: true,
            load_balance_seed: now_unix_secs,
        },
    );
}

fn scheduler_ranking_mode(mode: SchedulerSchedulingMode) -> SchedulerRankingMode {
    match mode {
        SchedulerSchedulingMode::FixedOrder => SchedulerRankingMode::FixedOrder,
        SchedulerSchedulingMode::CacheAffinity => SchedulerRankingMode::CacheAffinity,
        SchedulerSchedulingMode::LoadBalance => SchedulerRankingMode::LoadBalance,
    }
}
