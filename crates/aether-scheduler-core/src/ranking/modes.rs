use std::cmp::Ordering;

use super::compare_candidate_identity_for_ranking;
use super::format::{
    compare_cross_format_demotion, compare_demoted_format_preference, compare_format_preference,
};
use super::priority::{candidates_share_priority_group, compare_candidate_priority_slot};
use super::types::{SchedulerRankableCandidate, SchedulerRankingContext, SchedulerRankingMode};

pub(super) fn compare_rankable_candidates(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    context: SchedulerRankingContext,
) -> Ordering {
    match context.ranking_mode {
        SchedulerRankingMode::FixedOrder => compare_fixed_order(left, right, context),
        SchedulerRankingMode::CacheAffinity => compare_cache_affinity(left, right, context),
        SchedulerRankingMode::LoadBalance => compare_load_balance_base(left, right, context),
    }
}

fn compare_fixed_order(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    context: SchedulerRankingContext,
) -> Ordering {
    left.capability_priority
        .cmp(&right.capability_priority)
        .then_with(|| compare_cross_format_demotion(left, right))
        .then_with(|| compare_demoted_format_preference(left, right))
        .then_with(|| compare_candidate_priority_slot(left, right, context.priority_mode))
        .then_with(|| compare_format_preference(left, right))
        .then_with(|| compare_candidate_identity_for_ranking(left, right))
        .then(left.original_index.cmp(&right.original_index))
}

fn compare_cache_affinity(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    context: SchedulerRankingContext,
) -> Ordering {
    left.capability_priority
        .cmp(&right.capability_priority)
        .then_with(|| right.cached_affinity_match.cmp(&left.cached_affinity_match))
        .then_with(|| compare_cross_format_demotion(left, right))
        .then_with(|| compare_demoted_format_preference(left, right))
        .then_with(|| compare_candidate_priority_slot(left, right, context.priority_mode))
        .then(left.tunnel_bucket.cmp(&right.tunnel_bucket))
        .then_with(|| compare_format_preference(left, right))
        .then_with(|| compare_health(left, right, context.include_health))
        .then(left.affinity_hash.cmp(&right.affinity_hash))
        .then_with(|| compare_candidate_identity_for_ranking(left, right))
        .then(left.original_index.cmp(&right.original_index))
}

fn compare_load_balance_base(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    context: SchedulerRankingContext,
) -> Ordering {
    left.capability_priority
        .cmp(&right.capability_priority)
        .then_with(|| compare_cross_format_demotion(left, right))
        .then_with(|| compare_demoted_format_preference(left, right))
        .then_with(|| compare_candidate_priority_slot(left, right, context.priority_mode))
        .then_with(|| compare_format_preference(left, right))
        .then_with(|| compare_health(left, right, context.include_health))
        .then(left.affinity_hash.cmp(&right.affinity_hash))
        .then_with(|| compare_candidate_identity_for_ranking(left, right))
        .then(left.original_index.cmp(&right.original_index))
}

fn compare_health(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    include_health: bool,
) -> Ordering {
    if !include_health {
        return Ordering::Equal;
    }
    right
        .health_bucket
        .cmp(&left.health_bucket)
        .then_with(|| right.health_score.total_cmp(&left.health_score))
}

pub(super) fn apply_load_balance_rotation(
    sorted_indices: &mut [usize],
    candidates: &[SchedulerRankableCandidate],
    context: SchedulerRankingContext,
) {
    if context.ranking_mode != SchedulerRankingMode::LoadBalance || sorted_indices.len() < 2 {
        return;
    }

    let mut start = 0usize;
    while start < sorted_indices.len() {
        let mut end = start + 1;
        while end < sorted_indices.len()
            && candidates_share_load_balance_rotation_group(
                &candidates[sorted_indices[start]],
                &candidates[sorted_indices[end]],
                context.priority_mode,
            )
        {
            end += 1;
        }

        let group_len = end - start;
        if group_len > 1 {
            let offset = usize::try_from(context.load_balance_seed).unwrap_or(0) % group_len;
            sorted_indices[start..end].rotate_left(offset);
        }
        start = end;
    }
}

fn candidates_share_load_balance_rotation_group(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    priority_mode: crate::SchedulerPriorityMode,
) -> bool {
    candidates_share_priority_group(left, right, priority_mode)
        && left.capability_priority == right.capability_priority
        && left.demote_cross_format == right.demote_cross_format
        && left.format_preference == right.format_preference
}
