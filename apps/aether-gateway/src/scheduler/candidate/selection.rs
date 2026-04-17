use std::collections::{BTreeMap, BTreeSet};

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_scheduler_core::{
    collect_selectable_candidates_from_keys,
    reorder_candidates_by_scheduler_health as reorder_candidates_by_scheduler_health_in_core,
    SchedulerPriorityMode,
};

use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::data::candidate_selection::{
    read_minimal_candidate_selection_with_priority_mode_and_affinity_key_and_required_capabilities,
    MinimalCandidateSelectionRowSource,
};
use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
use crate::scheduler::config::SchedulerSchedulingMode;
use crate::GatewayError;

use super::affinity::{
    build_scheduler_affinity_cache_key, candidate_key, remember_scheduler_affinity,
};
use super::runtime::{
    auth_snapshot_concurrency_limit_reached, current_candidate_runtime_skip_reason,
    read_candidate_runtime_selection_snapshot,
};
use super::{SchedulerMinimalCandidateSelectionCandidate, SchedulerRuntimeState};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SchedulerSkippedCandidate {
    pub(crate) candidate: SchedulerMinimalCandidateSelectionCandidate,
    pub(crate) skip_reason: &'static str,
}

pub(super) const API_KEY_CONCURRENCY_LIMIT_SKIP_REASON: &str = "api_key_concurrency_limit_reached";

pub(super) fn is_exact_all_skipped_by_auth_limit(
    selected: &[SchedulerMinimalCandidateSelectionCandidate],
    skipped: &[SchedulerSkippedCandidate],
) -> bool {
    selected.is_empty()
        && !skipped.is_empty()
        && skipped
            .iter()
            .all(|candidate| candidate.skip_reason == API_KEY_CONCURRENCY_LIMIT_SKIP_REASON)
}

pub(super) fn reorder_candidates_by_scheduler_health(
    candidates: &mut [SchedulerMinimalCandidateSelectionCandidate],
    provider_key_rpm_states: &BTreeMap<String, StoredProviderCatalogKey>,
    required_capabilities: Option<&serde_json::Value>,
    affinity_key: Option<&str>,
    priority_mode: SchedulerPriorityMode,
) {
    reorder_candidates_by_scheduler_health_in_core(
        candidates,
        provider_key_rpm_states,
        required_capabilities,
        affinity_key,
        priority_mode,
    );
}

fn apply_load_balance_rotation(
    candidates: &mut [SchedulerMinimalCandidateSelectionCandidate],
    priority_mode: SchedulerPriorityMode,
    now_unix_secs: u64,
) {
    if candidates.len() < 2 {
        return;
    }

    let mut start = 0usize;
    while start < candidates.len() {
        let mut end = start + 1;
        while end < candidates.len()
            && candidates_share_load_balance_group(
                &candidates[start],
                &candidates[end],
                priority_mode,
            )
        {
            end += 1;
        }

        let group_len = end - start;
        if group_len > 1 {
            let offset = usize::try_from(now_unix_secs).unwrap_or(0) % group_len;
            candidates[start..end].rotate_left(offset);
        }
        start = end;
    }
}

fn candidates_share_load_balance_group(
    left: &SchedulerMinimalCandidateSelectionCandidate,
    right: &SchedulerMinimalCandidateSelectionCandidate,
    priority_mode: SchedulerPriorityMode,
) -> bool {
    match priority_mode {
        SchedulerPriorityMode::Provider => {
            left.provider_priority == right.provider_priority
                && left.key_internal_priority == right.key_internal_priority
        }
        SchedulerPriorityMode::GlobalKey => {
            left.key_global_priority_for_format == right.key_global_priority_for_format
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) async fn select_minimal_candidate(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &impl SchedulerRuntimeState,
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<Option<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
    let affinity_cache_key =
        build_scheduler_affinity_cache_key(auth_snapshot, api_format, global_model_name);
    let selected = collect_selectable_candidates(
        selection_row_source,
        runtime_state,
        api_format,
        global_model_name,
        require_streaming,
        required_capabilities,
        auth_snapshot,
        now_unix_secs,
    )
    .await?
    .into_iter()
    .next();
    if let Some(candidate) = selected.as_ref() {
        remember_scheduler_affinity(affinity_cache_key.as_deref(), runtime_state, candidate);
    }
    Ok(selected)
}

pub(super) async fn collect_selectable_candidates(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &impl SchedulerRuntimeState,
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<Vec<SchedulerMinimalCandidateSelectionCandidate>, GatewayError> {
    Ok(collect_selectable_candidates_with_skip_reasons(
        selection_row_source,
        runtime_state,
        api_format,
        global_model_name,
        require_streaming,
        required_capabilities,
        auth_snapshot,
        now_unix_secs,
    )
    .await?
    .0)
}

pub(super) async fn collect_selectable_candidates_with_skip_reasons(
    selection_row_source: &(impl MinimalCandidateSelectionRowSource + Sync),
    runtime_state: &impl SchedulerRuntimeState,
    api_format: &str,
    global_model_name: &str,
    require_streaming: bool,
    required_capabilities: Option<&serde_json::Value>,
    auth_snapshot: Option<&GatewayAuthApiKeySnapshot>,
    now_unix_secs: u64,
) -> Result<
    (
        Vec<SchedulerMinimalCandidateSelectionCandidate>,
        Vec<SchedulerSkippedCandidate>,
    ),
    GatewayError,
> {
    let ordering_config = runtime_state.read_scheduler_ordering_config().await?;
    let priority_affinity_key =
        scheduling_priority_affinity_key(auth_snapshot, ordering_config.scheduling_mode);
    let mut candidates =
        read_minimal_candidate_selection_with_priority_mode_and_affinity_key_and_required_capabilities(
        selection_row_source,
        api_format,
        global_model_name,
        require_streaming,
        auth_snapshot,
        ordering_config.priority_mode,
        priority_affinity_key,
        required_capabilities,
    )
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let runtime_snapshot =
        read_candidate_runtime_selection_snapshot(runtime_state, &candidates, now_unix_secs)
            .await?;
    reorder_candidates_by_scheduler_health(
        &mut candidates,
        &runtime_snapshot.provider_key_rpm_states,
        required_capabilities,
        priority_affinity_key,
        ordering_config.priority_mode,
    );
    if ordering_config.scheduling_mode == SchedulerSchedulingMode::LoadBalance {
        apply_load_balance_rotation(
            &mut candidates,
            ordering_config.priority_mode,
            now_unix_secs,
        );
    }
    let affinity_cache_key =
        build_scheduler_affinity_cache_key(auth_snapshot, api_format, global_model_name);
    let cached_affinity_target = if ordering_config.scheduling_mode
        == SchedulerSchedulingMode::CacheAffinity
    {
        affinity_cache_key.as_deref().and_then(|cache_key| {
            runtime_state.read_cached_scheduler_affinity_target(cache_key, SCHEDULER_AFFINITY_TTL)
        })
    } else {
        None
    };

    if auth_snapshot_concurrency_limit_reached(auth_snapshot, &runtime_snapshot, now_unix_secs) {
        return Ok((
            Vec::new(),
            candidates
                .into_iter()
                .map(|candidate| SchedulerSkippedCandidate {
                    candidate,
                    skip_reason: API_KEY_CONCURRENCY_LIMIT_SKIP_REASON,
                })
                .collect(),
        ));
    }

    let mut selected_keys = BTreeSet::new();
    let mut skipped = Vec::new();
    let mut emitted_skipped_keys = BTreeSet::new();

    for candidate in &candidates {
        let key = candidate_key(candidate);
        if let Some(skip_reason) = current_candidate_runtime_skip_reason(
            candidate,
            &runtime_snapshot,
            now_unix_secs,
            cached_affinity_target.as_ref(),
        ) {
            if emitted_skipped_keys.insert(key) {
                skipped.push(SchedulerSkippedCandidate {
                    candidate: candidate.clone(),
                    skip_reason,
                });
            }
            continue;
        }
        selected_keys.insert(key);
    }

    Ok((
        collect_selectable_candidates_from_keys(
            candidates,
            &selected_keys,
            cached_affinity_target.as_ref(),
        ),
        skipped,
    ))
}

fn scheduling_priority_affinity_key<'a>(
    auth_snapshot: Option<&'a GatewayAuthApiKeySnapshot>,
    scheduling_mode: SchedulerSchedulingMode,
) -> Option<&'a str> {
    if scheduling_mode == SchedulerSchedulingMode::FixedOrder {
        return None;
    }

    auth_snapshot
        .map(|snapshot| snapshot.api_key_id.trim())
        .filter(|value| !value.is_empty())
}
