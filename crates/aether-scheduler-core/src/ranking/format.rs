use std::cmp::Ordering;

use super::types::SchedulerRankableCandidate;

pub(super) fn compare_format_state(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
) -> Ordering {
    left.demote_cross_format
        .cmp(&right.demote_cross_format)
        .then(left.format_preference.cmp(&right.format_preference))
}
