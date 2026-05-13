mod format;
mod modes;
mod priority;
mod reasons;
mod types;

use modes::compare_rankable_candidates;
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

    fn ranked_keys(
        candidates: &[SchedulerRankableCandidate],
        context: SchedulerRankingContext,
    ) -> Vec<String> {
        scheduler_candidate_ranking_order(candidates, context)
            .into_iter()
            .map(|index| candidates[index].key_id.clone())
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
    fn fixed_order_demotes_cross_format_before_priority() {
        let lower_priority_same_format = candidate("same", 10, 0, Some(10));

        let mut higher_priority_cross_format = candidate("cross", 0, 0, Some(0));
        higher_priority_cross_format.demote_cross_format = true;

        assert_eq!(
            ranked_ids(
                &[higher_priority_cross_format, lower_priority_same_format],
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::FixedOrder,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-same", "provider-cross"]
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
    fn cache_affinity_promotes_cached_candidate_before_cross_format_demotion() {
        let same_format = candidate("same", 10, 0, Some(10));
        let mut cached_cross_format = candidate("cross", 0, 0, Some(0));
        cached_cross_format.cached_affinity_match = true;
        cached_cross_format.demote_cross_format = true;

        let outcomes = scheduler_ranking_outcomes(
            &[cached_cross_format, same_format],
            SchedulerRankingContext {
                priority_mode: SchedulerPriorityMode::Provider,
                ranking_mode: SchedulerRankingMode::CacheAffinity,
                include_health: false,
                load_balance_seed: 0,
            },
        );

        assert_eq!(outcomes[0].original_index, 0);
        assert_eq!(
            outcomes[0].promoted_by,
            Some(RANKING_REASON_CACHED_AFFINITY)
        );
        assert_eq!(outcomes[0].demoted_by, Some(RANKING_REASON_CROSS_FORMAT));
        assert_eq!(outcomes[1].original_index, 1);
    }

    #[test]
    fn demoted_cross_format_candidates_follow_format_preference_before_priority() {
        let mut openai_responses_high_priority = candidate("responses", 0, 0, Some(0));
        openai_responses_high_priority.demote_cross_format = true;
        openai_responses_high_priority.format_preference = (3, 1);

        let mut openai_chat_low_priority = candidate("chat", 10, 0, Some(10));
        openai_chat_low_priority.demote_cross_format = true;
        openai_chat_low_priority.format_preference = (3, 0);

        assert_eq!(
            ranked_ids(
                &[openai_responses_high_priority, openai_chat_low_priority],
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::CacheAffinity,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-chat", "provider-responses"]
        );
    }

    #[test]
    fn fixed_order_randomizes_equal_priority_ties_without_crossing_priority_slots() {
        let first = candidate("first", 0, 0, Some(0));
        let second = candidate("second", 0, 0, Some(0));
        let lower_priority = candidate("lower", 10, 0, Some(10));

        let first_seed_order = ranked_ids(
            &[first.clone(), second.clone(), lower_priority.clone()],
            SchedulerRankingContext {
                priority_mode: SchedulerPriorityMode::Provider,
                ranking_mode: SchedulerRankingMode::FixedOrder,
                include_health: false,
                load_balance_seed: 0,
            },
        );
        let alternate_order = (1..128)
            .map(|seed| {
                ranked_ids(
                    &[first.clone(), second.clone(), lower_priority.clone()],
                    SchedulerRankingContext {
                        priority_mode: SchedulerPriorityMode::Provider,
                        ranking_mode: SchedulerRankingMode::FixedOrder,
                        include_health: false,
                        load_balance_seed: seed,
                    },
                )
            })
            .find(|order| order[0] != first_seed_order[0])
            .expect("equal priority tie should vary by seed");

        assert_eq!(first_seed_order[2], "provider-lower");
        assert_eq!(alternate_order[2], "provider-lower");
    }

    #[test]
    fn load_balance_does_not_rotate_across_cross_format_demotion_group() {
        let same_format = candidate("same", 0, 0, Some(0));
        let mut cross_format = candidate("cross", 0, 0, Some(0));
        cross_format.demote_cross_format = true;

        assert_eq!(
            ranked_ids(
                &[same_format, cross_format],
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::Provider,
                    ranking_mode: SchedulerRankingMode::LoadBalance,
                    include_health: false,
                    load_balance_seed: 1,
                },
            ),
            vec!["provider-same", "provider-cross"]
        );
    }

    #[test]
    fn load_balance_provider_mode_keeps_provider_priority_slots_and_uses_internal_key_priority() {
        let mut provider_a_primary = candidate("a-primary", 0, 0, Some(0));
        provider_a_primary.provider_id = "provider-a".to_string();
        provider_a_primary.key_id = "key-a-primary".to_string();
        let mut provider_a_secondary = candidate("a-secondary", 0, 10, Some(10));
        provider_a_secondary.provider_id = "provider-a".to_string();
        provider_a_secondary.key_id = "key-a-secondary".to_string();
        let mut provider_b = candidate("b", 100, 0, Some(100));
        provider_b.provider_id = "provider-b".to_string();
        provider_b.key_id = "key-b".to_string();
        let candidates = vec![provider_a_secondary, provider_b, provider_a_primary];

        let order = ranked_keys(
            &candidates,
            SchedulerRankingContext {
                priority_mode: SchedulerPriorityMode::Provider,
                ranking_mode: SchedulerRankingMode::LoadBalance,
                include_health: false,
                load_balance_seed: 0,
            },
        );

        assert_eq!(order[2], "key-b");
        let primary_index = order
            .iter()
            .position(|key| key == "key-a-primary")
            .expect("primary key should be ranked");
        let secondary_index = order
            .iter()
            .position(|key| key == "key-a-secondary")
            .expect("secondary key should be ranked");
        assert!(primary_index < secondary_index);
    }

    #[test]
    fn load_balance_global_key_mode_keeps_global_priority_slots() {
        let high_priority = candidate("high", 100, 0, Some(0));
        let low_priority = candidate("low", 0, 0, Some(100));
        let candidates = vec![high_priority, low_priority];

        assert_eq!(
            ranked_ids(
                &candidates,
                SchedulerRankingContext {
                    priority_mode: SchedulerPriorityMode::GlobalKey,
                    ranking_mode: SchedulerRankingMode::LoadBalance,
                    include_health: false,
                    load_balance_seed: 0,
                },
            ),
            vec!["provider-high", "provider-low"]
        );
    }
}
