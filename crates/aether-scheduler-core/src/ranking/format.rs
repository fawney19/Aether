use std::cmp::Ordering;

use super::types::SchedulerRankableCandidate;

pub(super) fn compare_cross_format_demotion(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
) -> Ordering {
    left.demote_cross_format.cmp(&right.demote_cross_format)
}

pub(super) fn compare_format_preference(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
) -> Ordering {
    left.format_preference.cmp(&right.format_preference)
}

pub(super) fn compare_demoted_format_preference(
    left: &SchedulerRankableCandidate,
    right: &SchedulerRankableCandidate,
) -> Ordering {
    if left.demote_cross_format && right.demote_cross_format {
        compare_format_preference(left, right)
    } else {
        Ordering::Equal
    }
}
