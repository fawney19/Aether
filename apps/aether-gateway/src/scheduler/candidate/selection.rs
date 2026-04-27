use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::data::candidate_selection::MinimalCandidateSelectionRowSource;
use crate::scheduler::affinity::SCHEDULER_AFFINITY_TTL;
use crate::scheduler::config::SchedulerSchedulingMode;
use crate::GatewayError;

use super::affinity::{build_scheduler_affinity_cache_key, remember_scheduler_affinity};
use super::enumeration::enumerate_scheduler_candidates;
use super::ranking::rank_scheduler_candidates;
use super::resolution::resolve_scheduler_candidate_selectability;
use super::runtime::{
    auth_snapshot_concurrency_limit_reached, read_candidate_runtime_selection_snapshot,
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
    let mut candidates = enumerate_scheduler_candidates(
        selection_row_source,
        api_format,
        global_model_name,
        require_streaming,
        required_capabilities,
        auth_snapshot,
    )
    .await?;
    let runtime_snapshot =
        read_candidate_runtime_selection_snapshot(runtime_state, &candidates, now_unix_secs)
            .await?;
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
        rank_scheduler_candidates(
            &mut candidates,
            &runtime_snapshot,
            ordering_config,
            required_capabilities,
            priority_affinity_key,
            cached_affinity_target.as_ref(),
            now_unix_secs,
        );
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

    let (mut selected, skipped) = resolve_scheduler_candidate_selectability(
        candidates,
        &runtime_snapshot,
        now_unix_secs,
        cached_affinity_target.as_ref(),
    );
    rank_scheduler_candidates(
        &mut selected,
        &runtime_snapshot,
        ordering_config,
        required_capabilities,
        priority_affinity_key,
        cached_affinity_target.as_ref(),
        now_unix_secs,
    );

    Ok((selected, skipped))
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
