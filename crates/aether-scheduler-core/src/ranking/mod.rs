mod format;
mod modes;
mod priority;
mod reasons;
mod types;

use modes::{apply_load_balance_rotation, compare_rankable_candidates};
use priority::candidate_priority_slot;
use reasons::{demoted_by as ranking_demoted_by, promoted_by as ranking_promoted_by};
pub use reasons::{
    RANKING_REASON_CACHED_AFFINITY, RANKING_REASON_CROSS_FORMAT, RANKING_REASON_LOCAL_TUNNEL,
};
pub use types::{
    SchedulerRankableCandidate, SchedulerRankingContext, SchedulerRankingMode,
    SchedulerRankingOutcome, SchedulerTunnelAffinityBucket,
};

fn compare_candidate_identity_for_ranking(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
) -> std::cmp::Ordering {
    left.provider_id
        .cmp(&right.provider_id)
        .then(left.endpoint_id.cmp(&right.endpoint_id))
        .then(left.key_id.cmp(&right.key_id))
        .then(
            left.selected_provider_model_name
                .cmp(&right.selected_provider_model_name),
        )
}

fn scheduler_candidate_ranking_order(
    candidates: &[SchedulerRankableCandidate],
    context: SchedulerRankingContext,
) -> Vec<usize> {
    let mut order = (0..candidates.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        compare_rankable_candidates(&candidates[*left], &candidates[*right], context)
    });
    apply_load_balance_rotation(&mut order, candidates, context);
    order
}

fn scheduler_ranking_outcomes(
    candidates: &[SchedulerRankableCandidate],
    context: SchedulerRankingContext,
) -> Vec<SchedulerRankingOutcome> {
    scheduler_candidate_ranking_order(candidates, context)
        .into_iter()
        .enumerate()
        .map(|(ranking_index, original_index)| {
            let candidate = &candidates[original_index];
            SchedulerRankingOutcome {
                original_index,
                ranking_index,
                priority_mode: context.priority_mode,
                ranking_mode: context.ranking_mode,
                priority_slot: candidate_priority_slot(candidate, context.priority_mode),
                promoted_by: ranking_promoted_by(candidate, context.ranking_mode),
                demoted_by: ranking_demoted_by(candidate),
            }
        })
        .collect()
}

pub fn apply_scheduler_candidate_ranking<T>(
    items: &mut [T],
    candidates: &[SchedulerRankableCandidate],
    context: SchedulerRankingContext,
) -> Vec<SchedulerRankingOutcome> {
    let outcomes = scheduler_ranking_outcomes(candidates, context);
    apply_order(
        items,
        outcomes
            .iter()
            .map(|outcome| outcome.original_index)
            .collect(),
    );
    outcomes
}

fn apply_order<T>(items: &mut [T], sorted_old_indices: Vec<usize>) {
    if items.len() < 2 {
        return;
    }

    let mut target_positions = vec![0usize; sorted_old_indices.len()];
    for (new_position, old_position) in sorted_old_indices.into_iter().enumerate() {
        target_positions[old_position] = new_position;
    }

    for index in 0..items.len() {
        while target_positions[index] != index {
            let target = target_positions[index];
            items.swap(index, target);
            target_positions.swap(index, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SchedulerPriorityMode, SchedulerTunnelAffinityBucket};

    fn candidate(
        id: &str,
        provider_priority: i32,
        key_priority: i32,
        global_key_priority: Option<i32>,
    ) -> SchedulerRankableCandidate {
        SchedulerRankableCandidate {
            provider_id: format!("provider-{id}"),
            endpoint_id: format!("endpoint-{id}"),
            key_id: format!("key-{id}"),
            selected_provider_model_name: "gpt-5".to_string(),
            provider_priority,
            key_internal_priority: key_priority,
            key_global_priority_for_format: global_key_priority,
            capability_priority: (0, 0),
            cached_affinity_match: false,
            affinity_hash: None,
            tunnel_bucket: SchedulerTunnelAffinityBucket::Neutral,
            demote_cross_format: false,
            format_preference: (0, 0),
            health_bucket: None,
            health_score: 1.0,
            original_index: 0,
        }
    }

    fn ranked_ids(
        candidates: &[SchedulerRankableCandidate],
        context: SchedulerRankingContext,
    ) -> Vec<String> {
        scheduler_candidate_ranking_order(candidates, context)
            .into_iter()
            .map(|index| candidates[index].provider_id.clone())
            .collect()
    }

    #[test]
    fn provider_priority_mode_prefers_provider_priority_slot() {
        let candidates = vec![
            candidate("global", 10, 0, Some(0)),
            candidate("provider", 0, 10, Some(10)),
        ];

        assert_eq!(
            ranked_ids(
                &candidates,
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::FixedOrder,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-provider", "provider-global"]
        );
    }

    #[test]
    fn global_key_priority_mode_prefers_global_key_priority_slot() {
        let candidates = vec![
            candidate("provider", 0, 10, Some(10)),
            candidate("global", 10, 0, Some(0)),
        ];

        assert_eq!(
            ranked_ids(
                &candidates,
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::GlobalKey,
                    ranking_mode: SchedulerRankingMode::FixedOrder,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-global", "provider-provider"]
        );
    }

    #[test]
    fn fixed_order_keeps_priority_before_affinity_tunnel_and_format_preference() {
        let mut lower_priority = candidate("lower", 10, 0, Some(10));
        lower_priority.cached_affinity_match = true;
        lower_priority.tunnel_bucket = SchedulerTunnelAffinityBucket::LocalTunnel;
        lower_priority.format_preference = (0, 0);

        let mut higher_priority = candidate("higher", 0, 0, Some(0));
        higher_priority.demote_cross_format = true;
        higher_priority.format_preference = (9, 9);

        assert_eq!(
            ranked_ids(
                &[lower_priority, higher_priority],
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::FixedOrder,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-higher", "provider-lower"]
        );
    }

    #[test]
    fn capability_priority_precedes_provider_priority() {
        let matching_capability = candidate("matching", 10, 0, None);
        let mut missing_compatible_capability = candidate("missing", 0, 0, None);
        missing_compatible_capability.capability_priority = (0, 1);

        assert_eq!(
            ranked_ids(
                &[missing_compatible_capability, matching_capability],
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::FixedOrder,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-matching", "provider-missing"]
        );
    }

    #[test]
    fn cache_affinity_can_promote_cached_candidate_and_reports_reason() {
        let high_priority = candidate("high", 0, 0, Some(0));
        let mut cached = candidate("cached", 10, 0, Some(10));
        cached.cached_affinity_match = true;
        let candidates = vec![high_priority, cached];
        let context = SchedulerRankingContext {
            priority_mode: SchedulerPriorityMode::Provider,
            ranking_mode: SchedulerRankingMode::CacheAffinity,
            include_health: false,
            load_balance_seed: 0,
        };

        let outcomes = scheduler_ranking_outcomes(&candidates, context);
        assert_eq!(outcomes[0].original_index, 1);
        assert_eq!(
            outcomes[0].promoted_by,
            Some(RANKING_REASON_CACHED_AFFINITY)
        );
    }

    #[test]
    fn cache_affinity_without_cache_hit_keeps_priority_before_tunnel() {
        let mut higher_priority = candidate("higher", 0, 0, Some(0));
        higher_priority.tunnel_bucket = SchedulerTunnelAffinityBucket::RemoteTunnel;

        let mut lower_priority = candidate("lower", 10, 0, Some(10));
        lower_priority.tunnel_bucket = SchedulerTunnelAffinityBucket::LocalTunnel;

        assert_eq!(
            ranked_ids(
                &[lower_priority, higher_priority],
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::CacheAffinity,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-higher", "provider-lower"]
        );
    }

    #[test]
    fn cache_affinity_keeps_cross_format_demotion_before_priority() {
        let same_format_low_priority = candidate("same", 10, 0, Some(10));
        let mut cross_format_high_priority = candidate("cross", 0, 0, Some(0));
        cross_format_high_priority.demote_cross_format = true;

        assert_eq!(
            ranked_ids(
                &[cross_format_high_priority, same_format_low_priority],
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::CacheAffinity,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-same", "provider-cross"]
        );
    }

    #[test]
    fn load_balance_rotates_only_within_same_priority_group() {
        let first = candidate("first", 0, 0, Some(0));
        let second = candidate("second", 0, 0, Some(0));
        let third = candidate("third", 10, 0, Some(10));

        assert_eq!(
            ranked_ids(
                &[first, second, third],
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::LoadBalance,
                    include_health: false,
                    load_balance_seed: 1,
                },
            ),
            vec!["provider-second", "provider-first", "provider-third"]
        );
    }
}
