use std::collections::BTreeSet;

use aether_scheduler_core::{candidate_key, SchedulerAffinityTarget};

use super::runtime::{current_candidate_runtime_skip_reason, CandidateRuntimeSelectionSnapshot};
use super::{SchedulerMinimalCandidateSelectionCandidate, SchedulerSkippedCandidate};

pub(super) fn resolve_scheduler_candidate_selectability(
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    runtime_snapshot: &CandidateRuntimeSelectionSnapshot,
    now_unix_secs: u64,
    cached_affinity_target: Option<&SchedulerAffinityTarget>,
) -> (
    Vec<SchedulerMinimalCandidateSelectionCandidate>,
    Vec<SchedulerSkippedCandidate>,
) {
    let mut selected = Vec::with_capacity(candidates.len());
    let mut skipped = Vec::new();
    let mut emitted_selected_keys = BTreeSet::new();
    let mut emitted_skipped_keys = BTreeSet::new();

    for candidate in candidates {
        let key = candidate_key(&candidate);
        if let Some(skip_reason) = current_candidate_runtime_skip_reason(
            &candidate,
            runtime_snapshot,
            now_unix_secs,
            cached_affinity_target,
        ) {
            if emitted_skipped_keys.insert(key) {
                skipped.push(SchedulerSkippedCandidate {
                    candidate,
                    skip_reason,
                });
            }
            continue;
        }
        if emitted_selected_keys.insert(key) {
            selected.push(candidate);
        }
    }

    (selected, skipped)
}
