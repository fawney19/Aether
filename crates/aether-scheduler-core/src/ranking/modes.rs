use std::cmp::Ordering;

use sha2::{Digest, Sha256};

use super::compare_candidate_identity_for_ranking;
use super::format::{
    compare_cross_format_demotion, compare_demoted_format_preference, compare_format_preference,
};
use super::priority::{candidate_priority_slot, compare_candidate_priority_slot};
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
        .then_with(|| compare_seeded_candidate_hash(left, right, context.load_balance_seed, "tie"))
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
        .then_with(|| compare_affinity_or_seeded_hash(left, right, context.load_balance_seed))
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
        .then_with(|| compare_load_balance_priority_slot(left, right, context))
        .then_with(|| compare_format_preference(left, right))
        .then_with(|| compare_load_balance_distribution(left, right, context))
        .then_with(|| compare_candidate_identity_for_ranking(left, right))
        .then(left.original_index.cmp(&right.original_index))
}

fn compare_load_balance_priority_slot(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    context: SchedulerRankingContext,
) -> Ordering {
    candidate_priority_slot(left, context.priority_mode)
        .cmp(&candidate_priority_slot(right, context.priority_mode))
}

fn compare_load_balance_distribution(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    context: SchedulerRankingContext,
) -> Ordering {
    match context.priority_mode {
        crate::SchedulerPriorityMode::Provider => {
            compare_seeded_provider_hash(left, right, context.load_balance_seed)
                .then_with(|| {
                    if left.provider_id == right.provider_id {
                        left.key_internal_priority.cmp(&right.key_internal_priority)
                    } else {
                        Ordering::Equal
                    }
                })
                .then_with(|| {
                    compare_seeded_candidate_hash(left, right, context.load_balance_seed, "key")
                })
        }
        crate::SchedulerPriorityMode::GlobalKey => {
            compare_seeded_candidate_hash(left, right, context.load_balance_seed, "global-key")
        }
    }
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

fn compare_affinity_or_seeded_hash(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    seed: u64,
) -> Ordering {
    match (left.affinity_hash, right.affinity_hash) {
        (Some(left_hash), Some(right_hash)) => left_hash.cmp(&right_hash),
        _ => compare_seeded_candidate_hash(left, right, seed, "tie"),
    }
}

fn compare_seeded_provider_hash(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    seed: u64,
) -> Ordering {
    seeded_rank_hash(seed, "provider", [left.provider_id.as_str()], 0).cmp(&seeded_rank_hash(
        seed,
        "provider",
        [right.provider_id.as_str()],
        0,
    ))
}

fn compare_seeded_candidate_hash(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
    seed: u64,
    salt: &str,
) -> Ordering {
    seeded_rank_hash(
        seed,
        salt,
        [
            left.provider_id.as_str(),
            left.endpoint_id.as_str(),
            left.key_id.as_str(),
            left.selected_provider_model_name.as_str(),
        ],
        left.original_index,
    )
    .cmp(&seeded_rank_hash(
        seed,
        salt,
        [
            right.provider_id.as_str(),
            right.endpoint_id.as_str(),
            right.key_id.as_str(),
            right.selected_provider_model_name.as_str(),
        ],
        right.original_index,
    ))
}

fn seeded_rank_hash<'a>(
    seed: u64,
    salt: &str,
    parts: impl IntoIterator<Item = &'a str>,
    original_index: usize,
) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(seed.to_be_bytes());
    hasher.update(b":");
    hasher.update(salt.as_bytes());
    for part in parts {
        hasher.update(b":");
        hasher.update(part.as_bytes());
    }
    hasher.update(b":");
    hasher.update(original_index.to_be_bytes());
    let digest = hasher.finalize();
    u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}
