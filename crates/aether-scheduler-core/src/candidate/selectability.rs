use std::collections::BTreeMap;

use aether_data_contracts::repository::candidates::StoredRequestCandidate;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

use super::types::SchedulerMinimalCandidateSelectionCandidate;

pub fn auth_api_key_concurrency_limit_reached(
    recent_candidates: &[StoredRequestCandidate],
    now_unix_secs: u64,
    api_key_id: &str,
    concurrent_limit: usize,
) -> bool {
    if api_key_id.trim().is_empty() || concurrent_limit == 0 {
        return false;
    }

    crate::count_recent_active_requests_for_api_key(recent_candidates, api_key_id, now_unix_secs)
        >= concurrent_limit
}

#[derive(Clone, Copy, Debug)]
pub struct CandidateRuntimeSelectabilityInput<'a> {
    pub candidate: &'a SchedulerMinimalCandidateSelectionCandidate,
    pub recent_candidates: &'a [StoredRequestCandidate],
    pub provider_concurrent_limits: &'a BTreeMap<String, usize>,
    pub provider_key_rpm_states: &'a BTreeMap<String, StoredProviderCatalogKey>,
    pub now_unix_secs: u64,
    pub cached_affinity_target: Option<&'a crate::SchedulerAffinityTarget>,
    pub provider_quota_blocks_requests: bool,
    pub account_quota_exhausted: bool,
    pub oauth_invalid: bool,
    pub rpm_reset_at: Option<u64>,
}

pub fn candidate_is_selectable_with_runtime_state(
    input: CandidateRuntimeSelectabilityInput<'_>,
) -> bool {
    candidate_runtime_skip_reason_with_state(input).is_none()
}

pub fn candidate_runtime_skip_reason_with_state(
    input: CandidateRuntimeSelectabilityInput<'_>,
) -> Option<&'static str> {
    let CandidateRuntimeSelectabilityInput {
        candidate,
        recent_candidates,
        provider_concurrent_limits,
        provider_key_rpm_states,
        now_unix_secs,
        cached_affinity_target,
        provider_quota_blocks_requests,
        account_quota_exhausted,
        oauth_invalid,
        rpm_reset_at,
    } = input;

    if provider_quota_blocks_requests {
        return Some("provider_quota_blocked");
    }
    if account_quota_exhausted {
        return Some("account_quota_exhausted");
    }
    if oauth_invalid {
        return Some("oauth_invalid");
    }
    if crate::is_candidate_in_recent_failure_cooldown(
        recent_candidates,
        candidate.provider_id.as_str(),
        candidate.endpoint_id.as_str(),
        candidate.key_id.as_str(),
        now_unix_secs,
    ) {
        return Some("recent_failure_cooldown");
    }
    if provider_concurrent_limits
        .get(&candidate.provider_id)
        .is_some_and(|limit| {
            crate::count_recent_active_requests_for_provider(
                recent_candidates,
                candidate.provider_id.as_str(),
                now_unix_secs,
            ) >= *limit
        })
    {
        return Some("provider_concurrency_limit_reached");
    }

    let provider_key = provider_key_rpm_states.get(&candidate.key_id);
    if let Some(provider_key) = provider_key {
        if let Some(limit) = provider_key
            .concurrent_limit
            .filter(|limit| *limit > 0)
            .and_then(|limit| usize::try_from(limit).ok())
        {
            if crate::count_recent_active_requests_for_provider_key(
                recent_candidates,
                candidate.key_id.as_str(),
                now_unix_secs,
            ) >= limit
            {
                return Some("provider_key_concurrency_limit_reached");
            }
        }
    }

    let is_cached_user = cached_affinity_target
        .is_some_and(|target| crate::matches_affinity_target(candidate, target));
    if let Some(provider_key) = provider_key {
        if crate::is_provider_key_circuit_open(provider_key, candidate.endpoint_api_format.as_str())
        {
            return Some("key_circuit_open");
        }
        if crate::provider_key_health_score(provider_key, candidate.endpoint_api_format.as_str())
            .is_some_and(|score| score <= 0.0)
        {
            return Some("key_health_score_zero");
        }
        if !crate::provider_key_rpm_allows_request_since(
            provider_key,
            recent_candidates,
            now_unix_secs,
            is_cached_user,
            rpm_reset_at,
        ) {
            return Some("key_rpm_exhausted");
        }
    }

    None
}
