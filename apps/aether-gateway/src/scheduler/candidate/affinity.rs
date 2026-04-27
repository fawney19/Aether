use aether_scheduler_core::{
    build_scheduler_affinity_cache_key_for_api_key_id, SchedulerAffinityTarget,
};

use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;

use super::{
    SchedulerMinimalCandidateSelectionCandidate, SchedulerRuntimeState,
    SCHEDULER_AFFINITY_MAX_ENTRIES,
};

pub(super) fn build_scheduler_affinity_cache_key(
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    api_format: &str,
    global_model_name: &str,
) -> Option<String> {
    let api_key_id = auth_snapshot
        .map(|snapshot| snapshot.api_key_id.trim())
        .filter(|value| !value.is_empty())?;
    build_scheduler_affinity_cache_key_for_api_key_id(api_key_id, api_format, global_model_name)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn remember_scheduler_affinity(
    affinity_cache_key: Option<&str>,
    state: &(impl SchedulerRuntimeState + ?Sized),
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) {
    let Some(cache_key) = affinity_cache_key else {
        return;
    };

    state.remember_scheduler_affinity_target(
        cache_key,
        SchedulerAffinityTarget {
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
        },
        SCHEDULER_AFFINITY_TTL,
        SCHEDULER_AFFINITY_MAX_ENTRIES,
    );
}
