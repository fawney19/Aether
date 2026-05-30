use std::collections::{btree_map::Entry, BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    atomic::{AtomicU64, Ordering as AtomicOrdering},
    Arc, LazyLock,
};

use aether_admin::provider::pool as admin_provider_pool_pure;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredPoolKeyCandidateOrder,
    StoredPoolKeyCandidateRowsByKeyIdsQuery, StoredPoolKeyCandidateRowsQuery,
};
use aether_data_contracts::repository::pool_scores::{
    ListRankedPoolMembersQuery, PoolMemberHardState, PoolMemberIdentity,
    PoolMemberScheduleFeedback, PoolScoreScope, StoredPoolMemberScore, POOL_KIND_PROVIDER_KEY_POOL,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_pool_core::{
    run_pool_scheduler, PoolCandidateFacts, PoolCandidateInput, PoolCandidateOrchestration,
    PoolMemberSignals, PoolRuntimeState, PoolSchedulingConfig, PoolSchedulingPreset,
    POOL_ACCOUNT_BLOCKED_SKIP_REASON, POOL_ACCOUNT_EXHAUSTED_SKIP_REASON,
    POOL_COOLDOWN_SKIP_REASON, POOL_COST_LIMIT_REACHED_SKIP_REASON,
};
use aether_provider_pool::ProviderPoolService;
use aether_routing_core::{RankingOverlay, ResolvedRoutingPolicy};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::ai_serving::{
    candidate_auth_channel_skip_reason, candidate_common_transport_skip_reason,
    provider_key_pool_score_scope, read_candidate_transport_snapshot,
    record_local_runtime_candidate_skip_reason, CandidateTransportPolicyFacts,
    EligibleLocalExecutionCandidate, LocalExecutionCandidateKind, PlannerAppState,
    SkippedLocalExecutionCandidate,
};
use crate::clock::current_unix_ms;
use crate::handlers::shared::provider_pool::read_admin_provider_pool_runtime_state;
use crate::handlers::shared::provider_pool::{
    admin_provider_pool_cache_affinity_enabled, admin_provider_pool_config_from_config_value,
};
use crate::handlers::shared::provider_pool::{
    admin_provider_pool_quota_probe_active_members_key,
    read_admin_provider_pool_key_cooldown_reason, AdminProviderPoolConfig,
    AdminProviderPoolRuntimeState,
};
use crate::handlers::shared::{parse_catalog_auth_config_json, provider_key_health_summary};
use crate::maintenance::spawn_pool_quota_probe_replenish_for_request;
use crate::orchestration::LocalExecutionCandidateMetadata;

static LOAD_BALANCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static POOL_SCORE_SCHEDULE_INTEREST_SEMAPHORE: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(POOL_SCORE_SCHEDULE_INTEREST_CONCURRENCY)));
const POOL_ACTIVE_PROBE_SEALED_SKIP_REASON: &str = "pool_active_probe_sealed";
const ROUTING_PROFILE_DISALLOWED_KEY_SKIP_REASON: &str = "routing_profile_disallowed_key";
const POOL_SCORE_SCHEDULE_INTEREST_CONCURRENCY: usize = 4;
const POOL_SCORE_SCHEDULE_INTEREST_MAX_PER_BATCH: usize = 16;
const POOL_SCORE_SCHEDULE_INTEREST_MIN_INTERVAL_SECS: u64 = 60;

type PoolCatalogKeyContext = PoolMemberSignals;

pub(crate) async fn apply_local_execution_pool_scheduler(
    state: PlannerAppState<'_>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    sticky_session_token: Option<&str>,
    requested_model: Option<&str>,
    request_auth_channel: Option<&str>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    if candidates.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let sticky_session_token = sticky_session_token
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let mut scheduled = Vec::new();
    let mut skipped = Vec::new();
    for candidate in candidates {
        if candidate.kind == LocalExecutionCandidateKind::PoolGroup {
            let mut expanded = expand_pool_group_candidate(
                state,
                candidate,
                sticky_session_token,
                requested_model,
                request_auth_channel,
            )
            .await;
            scheduled.append(&mut expanded.0);
            skipped.append(&mut expanded.1);
        } else {
            scheduled.push(candidate);
        }
    }

    (scheduled, skipped)
}

async fn schedule_pool_page_candidates(
    state: PlannerAppState<'_>,
    candidates: Vec<EligibleLocalExecutionCandidate>,
    sticky_session_token: Option<&str>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    if candidates.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut provider_runtime_requirements =
        BTreeMap::<String, (AdminProviderPoolConfig, BTreeSet<String>)>::new();
    for candidate in &candidates {
        let Some(pool_config) = pool_config_for_candidate(candidate) else {
            continue;
        };
        let entry = provider_runtime_requirements
            .entry(candidate.candidate.provider_id.clone())
            .or_insert_with(|| (pool_config.clone(), BTreeSet::new()));
        entry.1.insert(candidate.candidate.key_id.clone());
    }

    let key_context_by_id = read_pool_catalog_key_contexts_by_id(state, &candidates).await;

    let mut runtime_by_provider = BTreeMap::new();
    let mut pool_config_by_provider = BTreeMap::new();
    let mut burst_provider_ids = BTreeSet::<String>::new();
    for (provider_id, (pool_config, key_ids)) in provider_runtime_requirements {
        let key_ids = key_ids.into_iter().collect::<Vec<_>>();
        let runtime = if key_ids.is_empty() {
            AdminProviderPoolRuntimeState::default()
        } else {
            read_admin_provider_pool_runtime_state(
                state.app().runtime_state.as_ref(),
                provider_id.as_str(),
                &key_ids,
                &pool_config,
                sticky_session_token,
            )
            .await
        };
        pool_config_by_provider.insert(provider_id.clone(), pool_config);
        runtime_by_provider.insert(provider_id, runtime);
    }

    let preflight_evictions = prune_unschedulable_active_probe_members_for_request(
        &mut runtime_by_provider,
        &candidates,
        &key_context_by_id,
    );
    spawn_active_probe_member_evictions_for_request(state, &preflight_evictions);
    burst_provider_ids.extend(preflight_evictions.keys().cloned());

    for (provider_id, pool_config) in &pool_config_by_provider {
        let Some(runtime) = runtime_by_provider.get(provider_id) else {
            continue;
        };
        if should_trigger_active_probe_burst_for_request(pool_config, runtime) {
            burst_provider_ids.insert(provider_id.clone());
        }
    }

    let outcome = apply_local_execution_pool_scheduler_with_runtime_map_outcome(
        candidates,
        &runtime_by_provider,
        &key_context_by_id,
    );
    let scheduled = outcome.candidates;
    let skipped = outcome.skipped;
    burst_provider_ids.extend(outcome.active_probe_seal_fallback_provider_ids);
    spawn_active_probe_member_evictions_for_request(
        state,
        &outcome.active_probe_evicted_members_by_provider,
    );
    burst_provider_ids.extend(
        outcome
            .active_probe_evicted_members_by_provider
            .keys()
            .cloned(),
    );

    for skipped_candidate in &skipped {
        if skipped_candidate.skip_reason == POOL_ACTIVE_PROBE_SEALED_SKIP_REASON {
            burst_provider_ids.insert(skipped_candidate.candidate.provider_id.clone());
        }
    }

    for provider_id in burst_provider_ids {
        let _ = spawn_pool_quota_probe_replenish_for_request(state.app().clone(), provider_id);
    }

    (scheduled, skipped)
}

async fn remove_active_probe_members_for_request(
    state: PlannerAppState<'_>,
    evicted_members_by_provider: &BTreeMap<String, BTreeSet<String>>,
) {
    remove_active_probe_members(state.app().clone(), evicted_members_by_provider).await;
}

fn spawn_active_probe_member_evictions_for_request(
    state: PlannerAppState<'_>,
    evicted_members_by_provider: &BTreeMap<String, BTreeSet<String>>,
) {
    if evicted_members_by_provider.is_empty() {
        return;
    }
    let app = state.app().clone();
    let evicted_members_by_provider = evicted_members_by_provider.clone();
    tokio::spawn(async move {
        remove_active_probe_members(app, &evicted_members_by_provider).await;
    });
}

async fn remove_active_probe_members(
    app: crate::AppState,
    evicted_members_by_provider: &BTreeMap<String, BTreeSet<String>>,
) {
    for (provider_id, key_ids) in evicted_members_by_provider {
        let set_key = admin_provider_pool_quota_probe_active_members_key(provider_id);
        for key_id in key_ids {
            if let Err(err) = app
                .runtime_state
                .as_ref()
                .set_remove(&set_key, key_id)
                .await
            {
                warn!(
                    event_name = "pool_active_probe_member_evict_failed",
                    log_type = "event",
                    provider_id,
                    key_id,
                    error = ?err,
                    "gateway pool scheduler failed to evict unschedulable active probe member"
                );
            }
        }
    }
}

fn prune_unschedulable_active_probe_members_for_request(
    runtime_by_provider: &mut BTreeMap<String, AdminProviderPoolRuntimeState>,
    candidates: &[EligibleLocalExecutionCandidate],
    key_context_by_id: &BTreeMap<String, PoolCatalogKeyContext>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut evicted = BTreeMap::<String, BTreeSet<String>>::new();
    for candidate in candidates {
        let Some(pool_config) = pool_config_for_candidate(candidate) else {
            continue;
        };
        if !should_enforce_active_probe_sealed_pool(&pool_config) {
            continue;
        }
        let provider_id = candidate.candidate.provider_id.as_str();
        let key_id = candidate.candidate.key_id.as_str();
        let Some(runtime) = runtime_by_provider.get_mut(provider_id) else {
            continue;
        };
        if !runtime.active_probe_member_ids.contains(key_id) {
            continue;
        }
        if !active_probe_member_is_unschedulable_for_request(
            &pool_config,
            runtime,
            key_id,
            key_context_by_id.get(key_id),
        ) {
            continue;
        }
        runtime.active_probe_member_ids.remove(key_id);
        evicted
            .entry(provider_id.to_string())
            .or_default()
            .insert(key_id.to_string());
    }
    evicted
}

fn active_probe_member_is_unschedulable_for_request(
    pool_config: &AdminProviderPoolConfig,
    runtime: &AdminProviderPoolRuntimeState,
    key_id: &str,
    key_context: Option<&PoolCatalogKeyContext>,
) -> bool {
    if runtime.cooldown_reason_by_key.contains_key(key_id) {
        return true;
    }
    if pool_config.cost_limit_per_key_tokens.is_some_and(|limit| {
        runtime
            .cost_window_usage_by_key
            .get(key_id)
            .copied()
            .unwrap_or(0)
            >= limit
    }) {
        return true;
    }
    key_context.is_some_and(|context| context.account_blocked || context.quota_exhausted)
}

async fn expand_pool_group_candidate(
    state: PlannerAppState<'_>,
    group: EligibleLocalExecutionCandidate,
    sticky_session_token: Option<&str>,
    requested_model: Option<&str>,
    request_auth_channel: Option<&str>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let mut cursor = PoolKeyCursor::new(
        state,
        group,
        sticky_session_token,
        requested_model,
        request_auth_channel,
    );
    let mut scheduled = Vec::new();
    let mut skipped = Vec::new();

    while let Some(candidate) = cursor.next_key().await {
        scheduled.push(candidate);
    }
    skipped.append(&mut cursor.take_skipped_candidates());

    if scheduled.is_empty() {
        cursor.log_exhausted();
    }
    (scheduled, skipped)
}

pub(crate) struct PoolKeyCursor<'a> {
    state: PlannerAppState<'a>,
    group: EligibleLocalExecutionCandidate,
    sticky_session_token: Option<String>,
    requested_model: Option<String>,
    request_auth_channel: Option<String>,
    routing_overlay: Option<RankingOverlay>,
    runtime_miss_trace_id: Option<String>,
    record_runtime_miss_diagnostic: bool,
    pool_key_order: StoredPoolKeyCandidateOrder,
    next_offset: u32,
    scanned_keys: u32,
    budget_scanned_keys: u32,
    window_size: u32,
    page_size: u32,
    max_scanned_keys: u32,
    absolute_max_scanned_keys: u32,
    score_top_n: u32,
    score_phase_loaded: bool,
    skip_reason_counts: BTreeMap<&'static str, u32>,
    next_pool_key_index: u32,
    sticky_candidate_loaded: bool,
    seen_key_ids: BTreeSet<String>,
    queued_candidates: VecDeque<EligibleLocalExecutionCandidate>,
    skipped_candidates: Vec<SkippedLocalExecutionCandidate>,
    exhausted_logged: bool,
    returned_key_count: u32,
    exhaustion_skip_recorded: bool,
}

impl<'a> PoolKeyCursor<'a> {
    pub(crate) fn new(
        state: PlannerAppState<'a>,
        group: EligibleLocalExecutionCandidate,
        sticky_session_token: Option<&str>,
        requested_model: Option<&str>,
        request_auth_channel: Option<&str>,
    ) -> Self {
        Self::new_with_routing_policy(
            state,
            group,
            sticky_session_token,
            requested_model,
            request_auth_channel,
            None,
        )
    }

    pub(crate) fn new_with_routing_policy(
        state: PlannerAppState<'a>,
        group: EligibleLocalExecutionCandidate,
        sticky_session_token: Option<&str>,
        requested_model: Option<&str>,
        request_auth_channel: Option<&str>,
        routing_policy: Option<&ResolvedRoutingPolicy>,
    ) -> Self {
        let pool_key_order = pool_key_candidate_order_for_group(&group, routing_policy);
        let routing_overlay = routing_policy.map(|policy| policy.ranking_overlay.clone());
        let pool_config = pool_config_for_candidate(&group);
        let score_top_n = pool_config
            .as_ref()
            .map(|config| config.score_top_n)
            .unwrap_or(u64::from(aether_dispatch_core::DEFAULT_POOL_PAGE_SIZE))
            .clamp(1, u64::from(u32::MAX)) as u32;
        let configured_max_scanned_keys = pool_config
            .as_ref()
            .map(|config| config.score_fallback_scan_limit)
            .unwrap_or(u64::from(aether_dispatch_core::DEFAULT_POOL_MAX_SCAN))
            .clamp(1, u64::from(u32::MAX)) as u32;
        let window_config = crate::dispatch::pool::default_pool_window_config().normalized();
        let max_scanned_keys = configured_max_scanned_keys.min(window_config.max_scan);
        let absolute_max_scanned_keys = configured_max_scanned_keys.max(max_scanned_keys);
        Self {
            state,
            group,
            sticky_session_token: sticky_session_token.map(str::to_string),
            requested_model: requested_model.map(str::to_string),
            request_auth_channel: request_auth_channel.map(str::to_string),
            routing_overlay,
            runtime_miss_trace_id: None,
            record_runtime_miss_diagnostic: false,
            pool_key_order,
            next_offset: 0,
            scanned_keys: 0,
            budget_scanned_keys: 0,
            window_size: window_config.window_size,
            page_size: window_config.page_size,
            max_scanned_keys: max_scanned_keys.max(window_config.window_size),
            absolute_max_scanned_keys: absolute_max_scanned_keys.max(window_config.window_size),
            score_top_n,
            score_phase_loaded: false,
            skip_reason_counts: BTreeMap::new(),
            next_pool_key_index: 0,
            sticky_candidate_loaded: false,
            seen_key_ids: BTreeSet::new(),
            queued_candidates: VecDeque::new(),
            skipped_candidates: Vec::new(),
            exhausted_logged: false,
            returned_key_count: 0,
            exhaustion_skip_recorded: false,
        }
    }

    pub(crate) fn with_runtime_miss_diagnostic(
        mut self,
        trace_id: &str,
        record_runtime_miss_diagnostic: bool,
    ) -> Self {
        if record_runtime_miss_diagnostic {
            self.runtime_miss_trace_id = Some(trace_id.to_string());
            self.record_runtime_miss_diagnostic = true;
        }
        self
    }

    pub(crate) async fn next_key(&mut self) -> Option<EligibleLocalExecutionCandidate> {
        loop {
            if let Some(candidate) = self.next_queued_candidate().await {
                self.returned_key_count = self.returned_key_count.saturating_add(1);
                return Some(candidate);
            }

            if !self.sticky_candidate_loaded {
                self.sticky_candidate_loaded = true;
                if let Some(candidate) = self.sticky_candidate().await {
                    self.queued_candidates.push_back(candidate);
                    continue;
                }
            }

            if !self.refill_queued_candidates().await {
                return None;
            }
        }
    }

    pub(crate) fn take_skipped_candidates(&mut self) -> Vec<SkippedLocalExecutionCandidate> {
        std::mem::take(&mut self.skipped_candidates)
    }

    pub(crate) fn exhausted_group_skipped_candidate(
        &self,
    ) -> Option<SkippedLocalExecutionCandidate> {
        if self.returned_key_count > 0 {
            return None;
        }

        let skip_reason_counts = self
            .skip_reason_counts
            .iter()
            .map(|(reason, count)| ((*reason).to_string(), serde_json::json!(count)))
            .collect::<serde_json::Map<String, serde_json::Value>>();
        Some(SkippedLocalExecutionCandidate {
            candidate: self.group.candidate.clone(),
            skip_reason: self.runtime_miss_pool_exhaustion_skip_reason(),
            transport: Some(self.group.transport.clone()),
            ranking: self.group.ranking.clone(),
            extra_data: Some(serde_json::json!({
                "pool_group_exhaustion": {
                    "scanned_keys": self.scanned_keys,
                    "budget_scanned_keys": self.budget_scanned_keys,
                    "skip_reason_counts": skip_reason_counts,
                }
            })),
        })
    }

    pub(crate) fn log_exhausted(&mut self) {
        if self.exhausted_logged {
            return;
        }
        self.exhausted_logged = true;
        warn!(
            event_name = "pool_group_exhausted",
            log_type = "event",
            provider_id = %self.group.candidate.provider_id,
            endpoint_id = %self.group.candidate.endpoint_id,
            model_id = %self.group.candidate.model_id,
            scanned_keys = self.scanned_keys,
            budget_scanned_keys = self.budget_scanned_keys,
            max_scanned_keys = self.max_scanned_keys,
            absolute_max_scanned_keys = self.absolute_max_scanned_keys,
            skip_reason_counts = ?self.skip_reason_counts,
            "gateway pool scheduler exhausted pool group without a schedulable key"
        );
        self.record_runtime_miss_pool_exhaustion_skip_reason();
    }

    fn record_runtime_miss_pool_exhaustion_skip_reason(&mut self) {
        if self.exhaustion_skip_recorded
            || !self.record_runtime_miss_diagnostic
            || self.returned_key_count > 0
        {
            return;
        }
        let Some(trace_id) = self.runtime_miss_trace_id.as_deref() else {
            return;
        };
        self.exhaustion_skip_recorded = true;
        record_local_runtime_candidate_skip_reason(
            self.state.app(),
            trace_id,
            self.runtime_miss_pool_exhaustion_skip_reason(),
        );
    }

    fn runtime_miss_pool_exhaustion_skip_reason(&self) -> &'static str {
        let mut selected_reason = "pool_group_exhausted";
        let mut selected_count = 0;
        for (reason, count) in &self.skip_reason_counts {
            if *count > selected_count {
                selected_reason = *reason;
                selected_count = *count;
            }
        }
        selected_reason
    }

    async fn next_page_candidates(&mut self) -> Option<Vec<EligibleLocalExecutionCandidate>> {
        if !self.score_phase_loaded {
            self.score_phase_loaded = true;
            if let Some(score_candidates) = self.next_score_candidates().await {
                return Some(score_candidates);
            }
        }

        if self.budget_scanned_keys >= self.max_scanned_keys
            || self.scanned_keys >= self.absolute_max_scanned_keys
        {
            return None;
        }

        let limit = self
            .page_size
            .min(self.max_scanned_keys - self.budget_scanned_keys)
            .min(self.absolute_max_scanned_keys - self.scanned_keys);
        let query = StoredPoolKeyCandidateRowsQuery {
            api_format: self.group.candidate.endpoint_api_format.clone(),
            provider_id: self.group.candidate.provider_id.clone(),
            endpoint_id: self.group.candidate.endpoint_id.clone(),
            model_id: self.group.candidate.model_id.clone(),
            selected_provider_model_name: self.group.candidate.selected_provider_model_name.clone(),
            order: self.pool_key_order.clone(),
            offset: self.next_offset,
            limit,
        };
        let rows = match self
            .state
            .app()
            .list_pool_key_candidate_rows_for_group(&query)
            .await
        {
            Ok(rows) => rows,
            Err(err) => {
                warn!(
                    event_name = "pool_group_key_page_load_failed",
                    log_type = "event",
                    provider_id = %self.group.candidate.provider_id,
                    endpoint_id = %self.group.candidate.endpoint_id,
                    model_id = %self.group.candidate.model_id,
                    selected_provider_model_name = %self.group.candidate.selected_provider_model_name,
                    offset = self.next_offset,
                    limit,
                    error = ?err,
                    "gateway pool scheduler failed to read pool key page"
                );
                return None;
            }
        };
        if rows.is_empty() {
            return None;
        }

        self.scanned_keys += rows.len() as u32;
        self.budget_scanned_keys += rows.len() as u32;
        self.next_offset = self.next_offset.saturating_add(rows.len() as u32);
        Some(self.build_page_eligible_candidates(rows).await)
    }

    async fn next_score_candidates(&mut self) -> Option<Vec<EligibleLocalExecutionCandidate>> {
        if self.scanned_keys >= self.absolute_max_scanned_keys {
            return None;
        }
        let limit = self
            .score_top_n
            .min(self.absolute_max_scanned_keys - self.scanned_keys);
        if limit == 0 {
            return None;
        }
        let scope = provider_key_pool_score_scope();
        let query = ListRankedPoolMembersQuery {
            pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
            pool_id: self.group.candidate.provider_id.clone(),
            capability: scope.capability.clone(),
            scope_kind: scope.scope_kind.clone(),
            scope_id: scope.scope_id.clone(),
            hard_states: vec![PoolMemberHardState::Available, PoolMemberHardState::Unknown],
            probe_statuses: None,
            offset: 0,
            limit: limit as usize,
        };
        let scores = match self.state.app().data.list_ranked_pool_members(&query).await {
            Ok(scores) => scores,
            Err(err) => {
                warn!(
                    event_name = "pool_group_score_load_failed",
                    log_type = "event",
                    provider_id = %self.group.candidate.provider_id,
                    endpoint_id = %self.group.candidate.endpoint_id,
                    model_id = %self.group.candidate.model_id,
                    selected_provider_model_name = %self.group.candidate.selected_provider_model_name,
                    error = ?err,
                    "gateway pool scheduler failed to read ranked pool member scores"
                );
                return None;
            }
        };
        if scores.is_empty() {
            return None;
        }

        self.spawn_score_schedule_interest_recording(&scores);

        let key_ids = scores
            .iter()
            .map(|score| score.member_id.clone())
            .collect::<Vec<_>>();
        let rows_query = StoredPoolKeyCandidateRowsByKeyIdsQuery {
            api_format: self.group.candidate.endpoint_api_format.clone(),
            provider_id: self.group.candidate.provider_id.clone(),
            endpoint_id: self.group.candidate.endpoint_id.clone(),
            model_id: self.group.candidate.model_id.clone(),
            selected_provider_model_name: self.group.candidate.selected_provider_model_name.clone(),
            key_ids,
        };
        let rows = match self
            .state
            .app()
            .list_pool_key_candidate_rows_for_group_key_ids(&rows_query)
            .await
        {
            Ok(rows) => rows,
            Err(err) => {
                warn!(
                    event_name = "pool_group_score_key_load_failed",
                    log_type = "event",
                    provider_id = %self.group.candidate.provider_id,
                    endpoint_id = %self.group.candidate.endpoint_id,
                    model_id = %self.group.candidate.model_id,
                    selected_provider_model_name = %self.group.candidate.selected_provider_model_name,
                    score_count = scores.len(),
                    error = ?err,
                    "gateway pool scheduler failed to materialize ranked pool keys"
                );
                return None;
            }
        };
        let materialized_row_count = rows.len() as u32;
        let missing_score_count = scores.len().saturating_sub(rows.len());
        if missing_score_count > 0 {
            *self
                .skip_reason_counts
                .entry("pool_score_member_missing")
                .or_insert(0) += u32::try_from(missing_score_count).unwrap_or(u32::MAX);
        }
        self.scanned_keys = self.scanned_keys.saturating_add(materialized_row_count);
        self.budget_scanned_keys = self
            .budget_scanned_keys
            .saturating_add(materialized_row_count);
        Some(self.build_page_eligible_candidates(rows).await)
    }

    async fn sticky_candidate(&mut self) -> Option<EligibleLocalExecutionCandidate> {
        let pool_config = pool_config_for_candidate(&self.group)?;
        if !admin_provider_pool_cache_affinity_enabled(&pool_config) {
            return None;
        }
        let runtime = read_admin_provider_pool_runtime_state(
            self.state.app().runtime_state.as_ref(),
            self.group.candidate.provider_id.as_str(),
            &[],
            &pool_config,
            self.sticky_session_token.as_deref(),
        )
        .await;
        let sticky_key_id = runtime.sticky_bound_key_id?;
        if self.seen_key_ids.contains(&sticky_key_id) {
            return None;
        }

        let key = match self
            .state
            .app()
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&sticky_key_id))
            .await
        {
            Ok(mut keys) => keys.pop()?,
            Err(err) => {
                warn!(
                    event_name = "pool_group_sticky_key_load_failed",
                    log_type = "event",
                    provider_id = %self.group.candidate.provider_id,
                    endpoint_id = %self.group.candidate.endpoint_id,
                    model_id = %self.group.candidate.model_id,
                    key_id = %sticky_key_id,
                    error = ?err,
                    "gateway pool scheduler failed to read sticky pool key"
                );
                return None;
            }
        };
        if key.provider_id != self.group.candidate.provider_id {
            return None;
        }

        let candidate = pool_candidate_from_catalog_key(&self.group, key);
        self.build_eligible_candidate(candidate).await
    }

    async fn refill_queued_candidates(&mut self) -> bool {
        let refill_target = self.window_size.max(1) as usize;

        loop {
            let mut candidates = Vec::new();
            // Keep pool expansion bounded; the cursor freezes one small window at a time.
            while candidates.len() < refill_target {
                let Some(mut page_candidates) = self.next_page_candidates().await else {
                    break;
                };
                candidates.append(&mut page_candidates);
            }

            if candidates.is_empty() {
                return false;
            }

            let (mut scheduled, mut skipped) = schedule_pool_page_candidates(
                self.state,
                candidates,
                self.sticky_session_token.as_deref(),
            )
            .await;
            self.record_skipped_candidates(&skipped);
            self.skipped_candidates.append(&mut skipped);

            if scheduled.is_empty() {
                continue;
            }

            scheduled.truncate(refill_target);
            self.queued_candidates.extend(scheduled.drain(..));
            return true;
        }
    }

    async fn next_queued_candidate(&mut self) -> Option<EligibleLocalExecutionCandidate> {
        while let Some(candidate) = self.queued_candidates.pop_front() {
            let mut candidate = candidate;
            if self.skip_candidate_if_routing_profile_disallowed(&candidate) {
                continue;
            }
            if self.skip_candidate_if_runtime_cooldown(&candidate).await {
                continue;
            }
            if candidate.orchestration.candidate_group_id.is_none() {
                candidate.orchestration.candidate_group_id =
                    Some(pool_cursor_candidate_group_id(&self.group));
            }
            candidate.orchestration.pool_key_index = Some(self.next_pool_key_index);
            self.next_pool_key_index = self.next_pool_key_index.saturating_add(1);
            return Some(candidate);
        }

        None
    }

    fn skip_candidate_if_routing_profile_disallowed(
        &mut self,
        candidate: &EligibleLocalExecutionCandidate,
    ) -> bool {
        let Some(overlay) = self.routing_overlay.as_ref() else {
            return false;
        };
        if overlay.key_allowed(candidate.candidate.key_id.as_str()) {
            return false;
        }
        self.record_skip_reason(ROUTING_PROFILE_DISALLOWED_KEY_SKIP_REASON);
        self.skipped_candidates
            .push(SkippedLocalExecutionCandidate {
                candidate: candidate.candidate.clone(),
                skip_reason: ROUTING_PROFILE_DISALLOWED_KEY_SKIP_REASON,
                transport: Some(candidate.transport.clone()),
                ranking: candidate.ranking.clone(),
                extra_data: None,
            });
        true
    }

    async fn skip_candidate_if_runtime_cooldown(
        &mut self,
        candidate: &EligibleLocalExecutionCandidate,
    ) -> bool {
        match read_admin_provider_pool_key_cooldown_reason(
            self.state.app().runtime_state.as_ref(),
            candidate.candidate.provider_id.as_str(),
            candidate.candidate.key_id.as_str(),
        )
        .await
        {
            Ok(Some(_)) => {
                self.record_skip_reason("pool_cooldown");
                self.skipped_candidates
                    .push(SkippedLocalExecutionCandidate {
                        candidate: candidate.candidate.clone(),
                        skip_reason: "pool_cooldown",
                        transport: Some(candidate.transport.clone()),
                        ranking: candidate.ranking.clone(),
                        extra_data: None,
                    });
                self.spawn_active_probe_member_eviction_and_replenish(candidate);
                true
            }
            Ok(None) => false,
            Err(err) => {
                warn!(
                    event_name = "pool_key_cooldown_check_failed",
                    log_type = "event",
                    provider_id = %candidate.candidate.provider_id,
                    endpoint_id = %candidate.candidate.endpoint_id,
                    model_id = %candidate.candidate.model_id,
                    key_id = %candidate.candidate.key_id,
                    error = ?err,
                    "gateway pool scheduler failed to read pool key cooldown; scheduling key"
                );
                false
            }
        }
    }

    fn spawn_active_probe_member_eviction_and_replenish(
        &self,
        candidate: &EligibleLocalExecutionCandidate,
    ) {
        let Some(config) = pool_config_for_candidate(candidate) else {
            return;
        };
        if !should_enforce_active_probe_sealed_pool(&config) {
            return;
        }
        let provider_id = candidate.candidate.provider_id.as_str();
        let key_id = candidate.candidate.key_id.as_str();
        spawn_active_probe_member_evictions_for_request(
            self.state,
            &BTreeMap::from([(
                provider_id.to_string(),
                BTreeSet::from([key_id.to_string()]),
            )]),
        );
        let _ = spawn_pool_quota_probe_replenish_for_request(
            self.state.app().clone(),
            provider_id.to_string(),
        );
    }

    fn spawn_score_schedule_interest_recording(&self, scores: &[StoredPoolMemberScore]) {
        if scores.is_empty() || !self.state.app().data.has_pool_score_writer() {
            return;
        }

        let scheduled_at = current_unix_ms() / 1000;
        let provider_id = self.group.candidate.provider_id.clone();
        let endpoint_id = self.group.candidate.endpoint_id.clone();
        let model_id = self.group.candidate.model_id.clone();
        let feedback = scores
            .iter()
            .filter(|score| {
                score.last_scheduled_at.is_none_or(|last_scheduled_at| {
                    scheduled_at.saturating_sub(last_scheduled_at)
                        >= POOL_SCORE_SCHEDULE_INTEREST_MIN_INTERVAL_SECS
                })
            })
            .take(POOL_SCORE_SCHEDULE_INTEREST_MAX_PER_BATCH)
            .map(|score| PoolMemberScheduleFeedback {
                identity: PoolMemberIdentity {
                    pool_kind: score.pool_kind.clone(),
                    pool_id: score.pool_id.clone(),
                    member_kind: score.member_kind.clone(),
                    member_id: score.member_id.clone(),
                },
                scope: Some(PoolScoreScope {
                    capability: score.capability.clone(),
                    scope_kind: score.scope_kind.clone(),
                    scope_id: score.scope_id.clone(),
                }),
                scheduled_at,
                succeeded: None,
                hard_state: None,
                score_delta: None,
                score_reason_patch: Some(serde_json::json!({
                    "last_schedule_interest": {
                        "provider_id": provider_id.as_str(),
                        "endpoint_id": endpoint_id.as_str(),
                        "model_id": model_id.as_str()
                    }
                })),
            })
            .collect::<Vec<_>>();
        let score_count = feedback.len();
        if feedback.is_empty() {
            return;
        }

        let Ok(permit) = POOL_SCORE_SCHEDULE_INTEREST_SEMAPHORE
            .clone()
            .try_acquire_owned()
        else {
            debug!(
                event_name = "pool_group_score_interest_dropped",
                log_type = "event",
                provider_id = %self.group.candidate.provider_id,
                endpoint_id = %self.group.candidate.endpoint_id,
                model_id = %self.group.candidate.model_id,
                score_count = scores.len(),
                "gateway pool scheduler dropped score schedule interest because the background writer is saturated"
            );
            return;
        };

        let app = self.state.app().clone();

        tokio::spawn(async move {
            let _permit = permit;
            let mut failed = 0usize;
            for feedback in feedback {
                let result = app
                    .data
                    .record_pool_member_schedule_feedback(feedback)
                    .await;
                if result.is_err() {
                    failed += 1;
                }
            }
            if failed > 0 {
                warn!(
                    event_name = "pool_group_score_interest_update_failed",
                    log_type = "event",
                    provider_id = %provider_id,
                    endpoint_id = %endpoint_id,
                    model_id = %model_id,
                    failed_count = failed,
                    score_count,
                    "gateway pool scheduler failed to record some pool score schedule interests"
                );
            }
        });
    }

    async fn build_page_eligible_candidates(
        &mut self,
        rows: Vec<StoredMinimalCandidateSelectionRow>,
    ) -> Vec<EligibleLocalExecutionCandidate> {
        let mut candidates = Vec::with_capacity(rows.len());
        for row in rows {
            let candidate = pool_candidate_from_row(&self.group, row);
            if let Some(candidate) = self.build_eligible_candidate(candidate).await {
                candidates.push(candidate);
            }
        }
        candidates
    }

    async fn build_eligible_candidate(
        &mut self,
        candidate: aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    ) -> Option<EligibleLocalExecutionCandidate> {
        if !self.seen_key_ids.insert(candidate.key_id.clone()) {
            return None;
        }

        let Some(transport) = read_candidate_transport_snapshot(self.state, &candidate).await
        else {
            self.record_skip_reason("transport_snapshot_missing");
            return None;
        };
        if let Some(skip_reason) =
            candidate_auth_channel_skip_reason(&transport, self.request_auth_channel.as_deref())
        {
            self.record_skip_reason(skip_reason);
            return None;
        }
        if let Some(skip_reason) = candidate_common_transport_skip_reason(
            &transport,
            pool_candidate_transport_policy_facts(&candidate),
            self.requested_model.as_deref(),
        ) {
            self.record_skip_reason(skip_reason);
            return None;
        }
        Some(EligibleLocalExecutionCandidate {
            kind: LocalExecutionCandidateKind::SingleKey,
            candidate,
            provider_api_format: transport.endpoint.api_format.trim().to_ascii_lowercase(),
            transport: std::sync::Arc::new(transport),
            orchestration: LocalExecutionCandidateMetadata::default(),
            ranking: self.group.ranking.clone(),
        })
    }

    fn record_skip_reason(&mut self, reason: &'static str) {
        *self.skip_reason_counts.entry(reason).or_insert(0) += 1;
    }

    fn record_skipped_candidates(&mut self, skipped_candidates: &[SkippedLocalExecutionCandidate]) {
        for skipped_candidate in skipped_candidates {
            self.record_skip_reason(skipped_candidate.skip_reason);
        }
        let prefiltered_count = skipped_candidates
            .iter()
            .filter(|candidate| pool_skip_reason_releases_scan_budget(candidate.skip_reason))
            .count();
        self.budget_scanned_keys = self
            .budget_scanned_keys
            .saturating_sub(u32::try_from(prefiltered_count).unwrap_or(u32::MAX));
    }
}

fn pool_skip_reason_releases_scan_budget(skip_reason: &str) -> bool {
    matches!(
        skip_reason,
        POOL_ACCOUNT_EXHAUSTED_SKIP_REASON | POOL_ACCOUNT_BLOCKED_SKIP_REASON
    )
}

fn pool_candidate_transport_policy_facts(
    candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
) -> CandidateTransportPolicyFacts<'_> {
    CandidateTransportPolicyFacts {
        endpoint_api_format: candidate.endpoint_api_format.as_str(),
        global_model_name: candidate.global_model_name.as_str(),
        selected_provider_model_name: candidate.selected_provider_model_name.as_str(),
        mapping_matched_model: candidate.mapping_matched_model.as_deref(),
    }
}

fn pool_candidate_from_row(
    group: &EligibleLocalExecutionCandidate,
    row: StoredMinimalCandidateSelectionRow,
) -> aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate {
    let mut candidate = group.candidate.clone();
    candidate.key_id = row.key_id;
    candidate.key_name = row.key_name;
    candidate.key_auth_type = row.key_auth_type;
    candidate.key_internal_priority = row.key_internal_priority;
    candidate.key_global_priority_for_format =
        aether_scheduler_core::extract_global_priority_for_format(
            row.key_global_priority_by_format.as_ref(),
            group.candidate.endpoint_api_format.as_str(),
        )
        .ok()
        .flatten();
    candidate.key_capabilities = row.key_capabilities;
    candidate
}

fn pool_candidate_from_catalog_key(
    group: &EligibleLocalExecutionCandidate,
    key: StoredProviderCatalogKey,
) -> aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate {
    let mut candidate = group.candidate.clone();
    candidate.key_id = key.id;
    candidate.key_name = key.name;
    candidate.key_auth_type = key.auth_type;
    candidate.key_internal_priority = key.internal_priority;
    candidate.key_global_priority_for_format =
        aether_scheduler_core::extract_global_priority_for_format(
            key.global_priority_by_format.as_ref(),
            group.candidate.endpoint_api_format.as_str(),
        )
        .ok()
        .flatten();
    candidate.key_capabilities = key.capabilities;
    candidate
}

async fn read_pool_catalog_key_contexts_by_id(
    state: PlannerAppState<'_>,
    candidates: &[EligibleLocalExecutionCandidate],
) -> BTreeMap<String, PoolCatalogKeyContext> {
    let mut key_ids = Vec::new();
    let mut provider_type_by_key_id = BTreeMap::<String, String>::new();

    for candidate in candidates {
        if pool_config_for_candidate(candidate).is_none() {
            continue;
        }
        let key_id = candidate.candidate.key_id.clone();
        if let Entry::Vacant(entry) = provider_type_by_key_id.entry(key_id.clone()) {
            entry.insert(candidate.transport.provider.provider_type.clone());
            key_ids.push(key_id);
        }
    }

    if key_ids.is_empty() {
        return BTreeMap::new();
    }

    let keys = match state
        .app()
        .read_provider_catalog_keys_by_ids(&key_ids)
        .await
    {
        Ok(keys) => keys,
        Err(err) => {
            warn!(
                error = ?err,
                key_count = key_ids.len(),
                "gateway pool scheduler: failed to read catalog key metadata"
            );
            return BTreeMap::new();
        }
    };

    let provider_pool_service = ProviderPoolService::with_builtin_adapters();

    keys.into_iter()
        .map(|key| {
            let provider_type = provider_type_by_key_id
                .get(&key.id)
                .map(String::as_str)
                .unwrap_or_default();
            (
                key.id.clone(),
                build_pool_catalog_key_context(state, &provider_pool_service, &key, provider_type),
            )
        })
        .collect()
}

fn build_pool_catalog_key_context(
    state: PlannerAppState<'_>,
    provider_pool_service: &ProviderPoolService,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> PoolCatalogKeyContext {
    let (health_score, _, _, _, _) = provider_key_health_summary(key);
    let health_score = key
        .health_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .filter(|payload| !payload.is_empty())
        .map(|_| health_score);
    let latency_avg_ms = key
        .success_count
        .filter(|count| *count > 0)
        .zip(key.total_response_time_ms)
        .map(|(success_count, total_response_time_ms)| {
            f64::from(total_response_time_ms) / f64::from(success_count)
        })
        .filter(|value| value.is_finite() && *value >= 0.0);

    let auth_config = parse_catalog_auth_config_json(state.app(), key);
    let mut signals =
        provider_pool_service.member_signals(provider_type, key, auth_config.as_ref());
    signals.account_blocked |= admin_provider_pool_pure::admin_pool_key_is_known_banned(key);
    signals.account_blocked |=
        pool_key_requires_reauth_for_scheduling(key, current_unix_ms().saturating_div(1000));
    signals.health_score = health_score;
    signals.latency_avg_ms = latency_avg_ms;
    signals.catalog_lru_score = Some(key.last_used_at_unix_secs.unwrap_or(0) as f64);
    signals
}

fn pool_key_requires_reauth_for_scheduling(
    key: &StoredProviderCatalogKey,
    now_unix_secs: u64,
) -> bool {
    if !key.auth_type.trim().eq_ignore_ascii_case("oauth") {
        return false;
    }

    let invalid_reason = key
        .oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if !invalid_reason.is_empty() {
        if pool_oauth_reason_has_tag(invalid_reason, "[OAUTH_EXPIRED]")
            || pool_oauth_reason_has_tag(invalid_reason, "[ACCOUNT_BLOCK]")
        {
            return true;
        }
        if pool_oauth_reason_has_tag(invalid_reason, "[REQUEST_FAILED]") {
            return false;
        }
        if pool_oauth_reason_has_tag(invalid_reason, "[REFRESH_FAILED]") {
            return key
                .expires_at_unix_secs
                .is_none_or(|expires_at| expires_at == 0 || expires_at <= now_unix_secs);
        }
        return true;
    }

    key.oauth_invalid_at_unix_secs.is_some()
}

fn pool_oauth_reason_has_tag(reason: &str, tag: &str) -> bool {
    reason
        .lines()
        .map(str::trim)
        .any(|line| line.starts_with(tag))
}

fn apply_local_execution_pool_scheduler_with_runtime_map(
    candidates: Vec<EligibleLocalExecutionCandidate>,
    runtime_by_provider: &BTreeMap<String, AdminProviderPoolRuntimeState>,
    key_context_by_id: &BTreeMap<String, PoolCatalogKeyContext>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let outcome = apply_local_execution_pool_scheduler_with_runtime_map_outcome(
        candidates,
        runtime_by_provider,
        key_context_by_id,
    );
    (outcome.candidates, outcome.skipped)
}

struct PoolSchedulerApplyOutcome {
    candidates: Vec<EligibleLocalExecutionCandidate>,
    skipped: Vec<SkippedLocalExecutionCandidate>,
    active_probe_seal_fallback_provider_ids: BTreeSet<String>,
    active_probe_evicted_members_by_provider: BTreeMap<String, BTreeSet<String>>,
}

fn apply_local_execution_pool_scheduler_with_runtime_map_outcome(
    candidates: Vec<EligibleLocalExecutionCandidate>,
    runtime_by_provider: &BTreeMap<String, AdminProviderPoolRuntimeState>,
    key_context_by_id: &BTreeMap<String, PoolCatalogKeyContext>,
) -> PoolSchedulerApplyOutcome {
    let (scheduled, skipped) = run_local_execution_pool_scheduler_with_runtime_map(
        candidates.clone(),
        runtime_by_provider,
        key_context_by_id,
        true,
    );
    let mut active_probe_evicted_members_by_provider =
        active_probe_evicted_members_from_skipped(&skipped, runtime_by_provider);
    let active_probe_seal_fallback_provider_ids = if scheduled.is_empty() {
        skipped
            .iter()
            .filter(|skipped| skipped.skip_reason == POOL_ACTIVE_PROBE_SEALED_SKIP_REASON)
            .map(|skipped| skipped.candidate.provider_id.clone())
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };

    if active_probe_seal_fallback_provider_ids.is_empty() {
        return PoolSchedulerApplyOutcome {
            candidates: scheduled,
            skipped,
            active_probe_seal_fallback_provider_ids,
            active_probe_evicted_members_by_provider,
        };
    }

    let (scheduled, skipped) = run_local_execution_pool_scheduler_with_runtime_map(
        candidates,
        runtime_by_provider,
        key_context_by_id,
        false,
    );
    merge_active_probe_evictions(
        &mut active_probe_evicted_members_by_provider,
        active_probe_evicted_members_from_skipped(&skipped, runtime_by_provider),
    );
    PoolSchedulerApplyOutcome {
        candidates: scheduled,
        skipped,
        active_probe_seal_fallback_provider_ids,
        active_probe_evicted_members_by_provider,
    }
}

fn merge_active_probe_evictions(
    target: &mut BTreeMap<String, BTreeSet<String>>,
    source: BTreeMap<String, BTreeSet<String>>,
) {
    for (provider_id, key_ids) in source {
        target.entry(provider_id).or_default().extend(key_ids);
    }
}

fn active_probe_evicted_members_from_skipped(
    skipped: &[SkippedLocalExecutionCandidate],
    runtime_by_provider: &BTreeMap<String, AdminProviderPoolRuntimeState>,
) -> BTreeMap<String, BTreeSet<String>> {
    if skipped.is_empty() {
        return BTreeMap::new();
    }

    let mut evicted = BTreeMap::<String, BTreeSet<String>>::new();
    for skipped_candidate in skipped {
        if !matches!(
            skipped_candidate.skip_reason,
            POOL_ACCOUNT_BLOCKED_SKIP_REASON
                | POOL_ACCOUNT_EXHAUSTED_SKIP_REASON
                | POOL_COOLDOWN_SKIP_REASON
                | POOL_COST_LIMIT_REACHED_SKIP_REASON
        ) {
            continue;
        }
        let Some(runtime) = runtime_by_provider.get(&skipped_candidate.candidate.provider_id)
        else {
            continue;
        };
        if runtime
            .active_probe_member_ids
            .contains(&skipped_candidate.candidate.key_id)
        {
            evicted
                .entry(skipped_candidate.candidate.provider_id.clone())
                .or_default()
                .insert(skipped_candidate.candidate.key_id.clone());
        }
    }
    evicted
}

fn run_local_execution_pool_scheduler_with_runtime_map(
    candidates: Vec<EligibleLocalExecutionCandidate>,
    runtime_by_provider: &BTreeMap<String, AdminProviderPoolRuntimeState>,
    key_context_by_id: &BTreeMap<String, PoolCatalogKeyContext>,
    enforce_active_probe_seal: bool,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let scheduler_runtime_by_provider = runtime_by_provider
        .iter()
        .map(|(provider_id, runtime)| (provider_id.clone(), pool_runtime_state(runtime)))
        .collect::<BTreeMap<_, _>>();
    let mut inputs = Vec::new();
    let mut skipped_candidates = Vec::new();
    for candidate in candidates {
        let key_context = key_context_by_id
            .get(&candidate.candidate.key_id)
            .cloned()
            .unwrap_or_default();
        let admin_pool_config = pool_config_for_candidate(&candidate);

        if let Some(config) = admin_pool_config.as_ref() {
            if enforce_active_probe_seal && should_enforce_active_probe_sealed_pool(config) {
                let active_member_ids = runtime_by_provider
                    .get(&candidate.candidate.provider_id)
                    .map(|runtime| &runtime.active_probe_member_ids);
                let should_seal_cold_member = active_member_ids.is_some_and(|members| {
                    !members.is_empty() && !members.contains(&candidate.candidate.key_id)
                });
                if should_seal_cold_member {
                    skipped_candidates.push(SkippedLocalExecutionCandidate {
                        candidate: candidate.candidate.clone(),
                        skip_reason: POOL_ACTIVE_PROBE_SEALED_SKIP_REASON,
                        transport: Some(candidate.transport.clone()),
                        ranking: candidate.ranking.clone(),
                        extra_data: None,
                    });
                    continue;
                }
            }
        }

        let pool_config = admin_pool_config.map(|config| {
            pool_scheduling_config(config, candidate.transport.provider.provider_type.as_str())
        });
        inputs.push(PoolCandidateInput {
            facts: pool_candidate_facts(&candidate),
            pool_config,
            key_context,
            candidate,
        });
    }
    let outcome = run_pool_scheduler(
        inputs,
        &scheduler_runtime_by_provider,
        pool_sort_seed().as_str(),
    );

    let candidates = outcome
        .candidates
        .into_iter()
        .map(|scheduled| apply_pool_orchestration(scheduled.candidate, scheduled.orchestration))
        .collect::<Vec<_>>();
    skipped_candidates.extend(outcome.skipped_candidates.into_iter().map(|skipped| {
        SkippedLocalExecutionCandidate {
            candidate: skipped.candidate.candidate,
            skip_reason: skipped.skip_reason,
            transport: Some(skipped.candidate.transport),
            ranking: skipped.candidate.ranking,
            extra_data: None,
        }
    }));

    (candidates, skipped_candidates)
}

fn pool_config_for_candidate(
    candidate: &EligibleLocalExecutionCandidate,
) -> Option<AdminProviderPoolConfig> {
    admin_provider_pool_config_from_config_value(candidate.transport.provider.config.as_ref())
}

fn should_enforce_active_probe_sealed_pool(pool_config: &AdminProviderPoolConfig) -> bool {
    pool_config.probing_enabled
}

fn should_trigger_active_probe_burst_for_request(
    pool_config: &AdminProviderPoolConfig,
    runtime: &AdminProviderPoolRuntimeState,
) -> bool {
    if !should_enforce_active_probe_sealed_pool(pool_config) {
        return false;
    }
    if runtime.provider_burst_pending {
        return false;
    }
    let active_count = runtime.active_probe_member_ids.len();
    runtime.provider_desired_hot > 0 && active_count < runtime.provider_desired_hot
}

fn pool_key_candidate_order_for_group(
    group: &EligibleLocalExecutionCandidate,
    routing_policy: Option<&ResolvedRoutingPolicy>,
) -> StoredPoolKeyCandidateOrder {
    let Some(pool_config) = pool_config_for_candidate(group) else {
        return StoredPoolKeyCandidateOrder::InternalPriority;
    };
    let override_presets = routing_policy
        .and_then(|policy| {
            policy
                .pool_policy_overrides
                .get(group.candidate.provider_id.as_str())
        })
        .filter(|override_policy| !override_policy.scheduling_presets.is_empty());
    let presets = match override_presets {
        Some(override_policy) => override_policy
            .scheduling_presets
            .iter()
            .map(|preset| PoolSchedulingPreset {
                preset: preset.preset.clone(),
                enabled: preset.enabled,
                mode: preset.mode.clone(),
            })
            .collect::<Vec<_>>(),
        None => pool_config
            .scheduling_presets
            .iter()
            .map(|preset| PoolSchedulingPreset {
                preset: preset.preset.clone(),
                enabled: preset.enabled,
                mode: preset.mode.clone(),
            })
            .collect::<Vec<_>>(),
    };
    let active_presets = ProviderPoolService::with_builtin_adapters()
        .normalize_scheduling_presets(group.transport.provider.provider_type.as_str(), &presets)
        .into_iter()
        .map(|preset| preset.preset)
        .collect::<Vec<_>>();
    if let Some(distribution_mode) = active_presets
        .iter()
        .find(|preset| pool_distribution_mode_preset(preset.as_str()))
        .map(String::as_str)
    {
        return match distribution_mode {
            "cache_affinity" => StoredPoolKeyCandidateOrder::CacheAffinity,
            "load_balance" => StoredPoolKeyCandidateOrder::LoadBalance {
                seed: pool_sort_seed(),
            },
            "single_account" => StoredPoolKeyCandidateOrder::SingleAccount,
            _ => StoredPoolKeyCandidateOrder::InternalPriority,
        };
    }
    if pool_config.lru_enabled {
        return StoredPoolKeyCandidateOrder::Lru;
    }
    StoredPoolKeyCandidateOrder::InternalPriority
}

fn pool_distribution_mode_preset(preset: &str) -> bool {
    matches!(preset, "cache_affinity" | "load_balance" | "single_account")
}

fn pool_sort_seed() -> String {
    let now_ms = current_unix_ms();
    let sequence = LOAD_BALANCE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    format!("{now_ms}:{sequence}")
}

fn pool_candidate_facts(candidate: &EligibleLocalExecutionCandidate) -> PoolCandidateFacts {
    PoolCandidateFacts {
        provider_id: candidate.candidate.provider_id.clone(),
        endpoint_id: candidate.candidate.endpoint_id.clone(),
        model_id: candidate.candidate.model_id.clone(),
        selected_provider_model_name: candidate.candidate.selected_provider_model_name.clone(),
        provider_api_format: candidate.provider_api_format.clone(),
        key_id: candidate.candidate.key_id.clone(),
        key_internal_priority: candidate.candidate.key_internal_priority,
    }
}

fn pool_cursor_candidate_group_id(group: &EligibleLocalExecutionCandidate) -> String {
    format!(
        "provider={}|endpoint={}|model={}|selected_model={}|api_format={}|singleton_key=*",
        group.candidate.provider_id,
        group.candidate.endpoint_id,
        group.candidate.model_id,
        group.candidate.selected_provider_model_name,
        group.provider_api_format,
    )
}

fn pool_scheduling_config(
    config: AdminProviderPoolConfig,
    provider_type: &str,
) -> PoolSchedulingConfig {
    let service = ProviderPoolService::with_builtin_adapters();
    let scheduling_presets = config
        .scheduling_presets
        .into_iter()
        .map(|preset| PoolSchedulingPreset {
            preset: preset.preset,
            enabled: preset.enabled,
            mode: preset.mode,
        })
        .collect::<Vec<_>>();
    PoolSchedulingConfig {
        scheduling_presets: service
            .normalize_scheduling_presets(provider_type, &scheduling_presets),
        lru_enabled: config.lru_enabled,
        skip_exhausted_accounts: config.skip_exhausted_accounts,
        cost_limit_per_key_tokens: config.cost_limit_per_key_tokens,
    }
}

fn pool_runtime_state(runtime: &AdminProviderPoolRuntimeState) -> PoolRuntimeState {
    PoolRuntimeState {
        sticky_bound_key_id: runtime.sticky_bound_key_id.clone(),
        cooldown_reason_by_key: runtime.cooldown_reason_by_key.clone(),
        cost_window_usage_by_key: runtime.cost_window_usage_by_key.clone(),
        latency_avg_ms_by_key: runtime.latency_avg_ms_by_key.clone(),
        lru_score_by_key: runtime.lru_score_by_key.clone(),
    }
}

fn apply_pool_orchestration(
    mut candidate: EligibleLocalExecutionCandidate,
    orchestration: PoolCandidateOrchestration,
) -> EligibleLocalExecutionCandidate {
    let scheduler_affinity_epoch = candidate.orchestration.scheduler_affinity_epoch;
    candidate.orchestration = LocalExecutionCandidateMetadata {
        candidate_group_id: orchestration.candidate_group_id,
        pool_key_index: orchestration.pool_key_index,
        pool_key_lease: None,
        scheduler_affinity_epoch,
    };
    candidate
}

#[cfg(test)]
mod tests {
    use super::{
        admin_provider_pool_quota_probe_active_members_key, apply_local_execution_pool_scheduler,
        apply_local_execution_pool_scheduler_with_runtime_map,
        apply_local_execution_pool_scheduler_with_runtime_map_outcome,
        build_pool_catalog_key_context, pool_config_for_candidate,
        pool_key_requires_reauth_for_scheduling,
        prune_unschedulable_active_probe_members_for_request,
        remove_active_probe_members_for_request, should_trigger_active_probe_burst_for_request,
        PoolCatalogKeyContext, PoolKeyCursor, POOL_ACTIVE_PROBE_SEALED_SKIP_REASON,
        ROUTING_PROFILE_DISALLOWED_KEY_SKIP_REASON,
    };
    use crate::ai_serving::{
        apply_local_runtime_candidate_terminal_reason, provider_key_pool_score_id,
        provider_key_pool_score_scope, EligibleLocalExecutionCandidate,
        LocalExecutionCandidateKind, PlannerAppState,
    };
    use crate::data::GatewayDataState;
    use crate::handlers::shared::provider_pool::{
        record_admin_provider_pool_error, AdminProviderPoolRuntimeState,
    };
    use crate::orchestration::LocalExecutionCandidateMetadata;
    use crate::{AppState, LocalExecutionRuntimeMissDiagnostic};
    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data::repository::pool_scores::InMemoryPoolMemberScoreRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::candidate_selection::{
        StoredMinimalCandidateSelectionRow, StoredPoolKeyCandidateOrder,
    };
    use aether_data_contracts::repository::pool_scores::{
        PoolMemberHardState, PoolMemberIdentity, PoolMemberProbeStatus, StoredPoolMemberScore,
    };
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use aether_pool_core::PoolSchedulingPreset;
    use aether_provider_pool::ProviderPoolService;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use aether_routing_core::{
        RankingOverlay, ResolvedRoutingPolicy, RoutingSchedulingMode, RoutingSetPriorityMode,
    };
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use std::sync::Arc;

    #[test]
    fn pool_scheduler_groups_interleaved_candidates_and_reorders_internal_keys() {
        let pool_first = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pool-a",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );
        let other =
            sample_eligible_candidate("provider-other", "endpoint-2", "key-other", 10, None);
        let pool_second = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pool-b",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-pool-a".to_string(), 20.0),
                    ("key-pool-b".to_string(), 10.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![pool_first, other, pool_second],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-pool-b", "key-pool-a", "key-other"]
        );
    }

    #[test]
    fn pool_scheduler_uses_catalog_last_used_when_runtime_lru_is_missing() {
        let recent_key = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-recent",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );
        let older_key = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-older",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );

        let key_context_by_id = BTreeMap::from([
            (
                "key-recent".to_string(),
                PoolCatalogKeyContext {
                    catalog_lru_score: Some(200.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-older".to_string(),
                PoolCatalogKeyContext {
                    catalog_lru_score: Some(100.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
        ]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![recent_key, older_key],
            &BTreeMap::new(),
            &key_context_by_id,
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-older", "key-recent"]
        );
    }

    #[test]
    fn pool_scheduler_attaches_group_and_pool_metadata_to_ranked_candidates() {
        let pool_first = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pool-a",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );
        let other =
            sample_eligible_candidate("provider-other", "endpoint-2", "key-other", 10, None);
        let pool_second = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pool-b",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-pool-a".to_string(), 20.0),
                    ("key-pool-b".to_string(), 10.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![pool_first, other, pool_second],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(reordered.len(), 3);
        assert_eq!(
            reordered[0].orchestration,
            LocalExecutionCandidateMetadata {
                candidate_group_id: Some(
                    "provider=provider-pool|endpoint=endpoint-1|model=model-1|selected_model=gpt-5|api_format=openai:chat|singleton_key=*"
                        .to_string(),
                ),
                pool_key_index: Some(0),
                pool_key_lease: None,
                scheduler_affinity_epoch: None,
            }
        );
        assert_eq!(reordered[1].orchestration.pool_key_index, Some(1));
        assert_eq!(
            reordered[1].orchestration.candidate_group_id,
            reordered[0].orchestration.candidate_group_id
        );
        assert_eq!(
            reordered[2].orchestration,
            LocalExecutionCandidateMetadata {
                candidate_group_id: Some(
                    "provider=provider-other|endpoint=endpoint-2|model=model-1|selected_model=gpt-5|api_format=openai:chat|singleton_key=key-other"
                        .to_string(),
                ),
                pool_key_index: None,
                pool_key_lease: None,
                scheduler_affinity_epoch: None,
            }
        );
    }

    #[test]
    fn pool_scheduler_promotes_sticky_hit_before_other_sorted_keys() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "cache_affinity", "enabled": true}]
                }
            })),
        );
        let key_b = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-b",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "cache_affinity", "enabled": true}]
                }
            })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                sticky_bound_key_id: Some("key-a".to_string()),
                lru_score_by_key: BTreeMap::from([
                    ("key-a".to_string(), 50.0),
                    ("key-b".to_string(), 10.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_a, key_b],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-a", "key-b"]
        );
    }

    #[test]
    fn pool_scheduler_ignores_sticky_hit_without_cache_affinity() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "quota_balanced", "enabled": true}]
                }
            })),
        );
        let key_b = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-b",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "quota_balanced", "enabled": true}]
                }
            })),
        );

        let runtime_by_provider = BTreeMap::from([(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                sticky_bound_key_id: Some("key-a".to_string()),
                ..AdminProviderPoolRuntimeState::default()
            },
        )]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_b, key_a],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-b", "key-a"]
        );
    }

    #[test]
    fn pool_scheduler_skips_cooldown_and_cost_exhausted_keys() {
        let key_ready = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-ready",
            10,
            Some(json!({
                "pool_advanced": {
                    "cost_limit_per_key_tokens": 100
                }
            })),
        );
        let key_cooldown = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-cooldown",
            10,
            Some(json!({
                "pool_advanced": {
                    "cost_limit_per_key_tokens": 100
                }
            })),
        );
        let key_cost = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-cost",
            10,
            Some(json!({
                "pool_advanced": {
                    "cost_limit_per_key_tokens": 100
                }
            })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                cooldown_reason_by_key: BTreeMap::from([(
                    "key-cooldown".to_string(),
                    "429".to_string(),
                )]),
                cost_window_usage_by_key: BTreeMap::from([("key-cost".to_string(), 100)]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_ready, key_cooldown, key_cost],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-ready"]
        );
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![
                ("key-cooldown", "pool_cooldown"),
                ("key-cost", "pool_cost_limit_reached"),
            ]
        );
    }

    #[test]
    fn pool_scheduler_uses_only_active_probe_members_when_active_probe_enabled() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true
            }
        }));
        let key_active = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-active",
            10,
            provider_config.clone(),
        );
        let key_sealed = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-sealed",
            10,
            provider_config,
        );

        let runtime_by_provider = BTreeMap::from([(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                active_probe_member_ids: BTreeSet::from(["key-active".to_string()]),
                ..AdminProviderPoolRuntimeState::default()
            },
        )]);

        let (scheduled, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_sealed, key_active],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert_eq!(
            scheduled
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-active"]
        );
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![("key-sealed", POOL_ACTIVE_PROBE_SEALED_SKIP_REASON)]
        );
    }

    #[test]
    fn pool_scheduler_falls_back_when_active_probe_members_are_unschedulable() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true
            }
        }));
        let key_hot = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-hot",
            10,
            provider_config.clone(),
        );
        let key_cold = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-cold",
            10,
            provider_config,
        );

        let runtime_by_provider = BTreeMap::from([(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                active_probe_member_ids: BTreeSet::from(["key-hot".to_string()]),
                cooldown_reason_by_key: BTreeMap::from([(
                    "key-hot".to_string(),
                    "429".to_string(),
                )]),
                provider_desired_hot: 1,
                ..AdminProviderPoolRuntimeState::default()
            },
        )]);

        let outcome = apply_local_execution_pool_scheduler_with_runtime_map_outcome(
            vec![key_hot, key_cold],
            &runtime_by_provider,
            &BTreeMap::new(),
        );
        let scheduled = outcome.candidates;
        let skipped = outcome.skipped;

        assert_eq!(
            scheduled
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-cold"]
        );
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![("key-hot", "pool_cooldown")]
        );
        assert_eq!(
            outcome
                .active_probe_evicted_members_by_provider
                .get("provider-pool"),
            Some(&BTreeSet::from(["key-hot".to_string()]))
        );
    }

    #[test]
    fn pool_scheduler_prunes_cold_active_probe_members_before_scheduling() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true
            }
        }));
        let key_hot = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-hot",
            10,
            provider_config,
        );
        let mut runtime_by_provider = BTreeMap::from([(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                active_probe_member_ids: BTreeSet::from(["key-hot".to_string()]),
                cooldown_reason_by_key: BTreeMap::from([(
                    "key-hot".to_string(),
                    "429".to_string(),
                )]),
                provider_desired_hot: 1,
                ..AdminProviderPoolRuntimeState::default()
            },
        )]);

        let evicted = prune_unschedulable_active_probe_members_for_request(
            &mut runtime_by_provider,
            &[key_hot],
            &BTreeMap::new(),
        );

        assert_eq!(
            evicted.get("provider-pool"),
            Some(&BTreeSet::from(["key-hot".to_string()]))
        );
        assert!(runtime_by_provider
            .get("provider-pool")
            .expect("runtime should exist")
            .active_probe_member_ids
            .is_empty());
    }

    #[tokio::test]
    async fn pool_scheduler_removes_unschedulable_member_from_active_probe_set() {
        let app = AppState::new().expect("state should build");
        let set_key = admin_provider_pool_quota_probe_active_members_key("provider-pool");
        app.runtime_state
            .set_add(&set_key, "key-hot")
            .await
            .expect("active member should insert");

        remove_active_probe_members_for_request(
            PlannerAppState::new(&app),
            &BTreeMap::from([(
                "provider-pool".to_string(),
                BTreeSet::from(["key-hot".to_string()]),
            )]),
        )
        .await;

        let members = app
            .runtime_state
            .set_members(&set_key)
            .await
            .expect("active members should read");
        assert!(members.is_empty());
    }

    #[test]
    fn pool_scheduler_allows_cold_start_when_active_probe_pool_is_empty() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true
            }
        }));
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            10,
            provider_config.clone(),
        );
        let key_b =
            sample_eligible_candidate("provider-pool", "endpoint-1", "key-b", 10, provider_config);

        let runtime_by_provider = BTreeMap::from([(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState::default(),
        )]);

        let (scheduled, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_a, key_b],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert_eq!(
            scheduled
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-a", "key-b"]
        );
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            Vec::<(&str, &str)>::new()
        );
    }

    #[test]
    fn pool_scheduler_triggers_burst_when_auto_hot_target_has_gap() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true
            }
        }));
        let candidate =
            sample_eligible_candidate("provider-pool", "endpoint-1", "key-a", 10, provider_config);
        let pool_config = pool_config_for_candidate(&candidate).expect("pool config should parse");
        let runtime = AdminProviderPoolRuntimeState {
            active_probe_member_ids: BTreeSet::from(["key-a".to_string()]),
            provider_desired_hot: 3,
            ..AdminProviderPoolRuntimeState::default()
        };

        assert!(should_trigger_active_probe_burst_for_request(
            &pool_config,
            &runtime
        ));
    }

    #[test]
    fn pool_scheduler_ignores_legacy_threshold_fields_for_burst_target() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true,
                "probing_target_percent": 60,
                "probing_target_count": 10
            }
        }));
        let candidate =
            sample_eligible_candidate("provider-pool", "endpoint-1", "key-a", 10, provider_config);
        let pool_config = pool_config_for_candidate(&candidate).expect("pool config should parse");
        let runtime = AdminProviderPoolRuntimeState {
            active_probe_member_ids: BTreeSet::from(["key-a".to_string(), "key-b".to_string()]),
            provider_desired_hot: 2,
            ..AdminProviderPoolRuntimeState::default()
        };

        assert!(!should_trigger_active_probe_burst_for_request(
            &pool_config,
            &runtime
        ));
    }

    #[test]
    fn pool_scheduler_skips_burst_when_active_probe_target_is_met() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true,
                "probing_target_count": 1
            }
        }));
        let candidate =
            sample_eligible_candidate("provider-pool", "endpoint-1", "key-a", 10, provider_config);
        let pool_config = pool_config_for_candidate(&candidate).expect("pool config should parse");
        let runtime = AdminProviderPoolRuntimeState {
            active_probe_member_ids: BTreeSet::from(["key-a".to_string()]),
            provider_desired_hot: 1,
            ..AdminProviderPoolRuntimeState::default()
        };

        assert!(!should_trigger_active_probe_burst_for_request(
            &pool_config,
            &runtime
        ));
    }

    #[test]
    fn pool_scheduler_applies_distribution_mode_before_strategy_presets() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            50,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [
                        {"preset": "cache_affinity", "enabled": true},
                        {"preset": "priority_first", "enabled": true}
                    ]
                }
            })),
        );
        let key_b = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-b",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [
                        {"preset": "cache_affinity", "enabled": true},
                        {"preset": "priority_first", "enabled": true}
                    ]
                }
            })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-a".to_string(), 100.0),
                    ("key-b".to_string(), 5.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        );

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_a, key_b],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-a", "key-b"]
        );
    }

    #[test]
    fn pool_scheduler_uses_plan_preset_with_catalog_context() {
        let key_free = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-free",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );
        let key_plus = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-plus",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );

        let key_context_by_id = BTreeMap::from([
            (
                "key-free".to_string(),
                PoolCatalogKeyContext {
                    plan_tier: Some("free".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-plus".to_string(),
                PoolCatalogKeyContext {
                    plan_tier: Some("plus".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
        ]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_free, key_plus],
            &BTreeMap::new(),
            &key_context_by_id,
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-plus", "key-free"]
        );
    }

    #[test]
    fn pool_scheduler_plus_first_treats_plus_and_pro_as_top_tier() {
        let key_plus = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-plus",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );
        let key_pro = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pro",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );
        let key_team = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-team",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "plus_first", "enabled": true}]
                }
            })),
        );

        let key_context_by_id = BTreeMap::from([
            (
                "key-plus".to_string(),
                PoolCatalogKeyContext {
                    plan_tier: Some("plus".to_string()),
                    catalog_lru_score: Some(300.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-pro".to_string(),
                PoolCatalogKeyContext {
                    plan_tier: Some("pro".to_string()),
                    catalog_lru_score: Some(100.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-team".to_string(),
                PoolCatalogKeyContext {
                    plan_tier: Some("team".to_string()),
                    catalog_lru_score: Some(50.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
        ]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_plus, key_pro, key_team],
            &BTreeMap::new(),
            &key_context_by_id,
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-plus", "key-pro", "key-team"]
        );
    }

    #[test]
    fn pool_scheduler_supports_pro_first_plan_preset() {
        let key_plus = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-plus",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "pro_first", "enabled": true}]
                }
            })),
        );
        let key_pro = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-pro",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "pro_first", "enabled": true}]
                }
            })),
        );
        let key_team = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-team",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [{"preset": "pro_first", "enabled": true}]
                }
            })),
        );

        let key_context_by_id = BTreeMap::from([
            (
                "key-plus".to_string(),
                PoolCatalogKeyContext {
                    plan_tier: Some("plus".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-pro".to_string(),
                PoolCatalogKeyContext {
                    plan_tier: Some("pro".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-team".to_string(),
                PoolCatalogKeyContext {
                    plan_tier: Some("team".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
        ]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_plus, key_team, key_pro],
            &BTreeMap::new(),
            &key_context_by_id,
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-pro", "key-plus", "key-team"]
        );
    }

    #[test]
    fn pool_scheduler_defaults_empty_pool_advanced_to_cache_affinity() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            10,
            Some(json!({ "pool_advanced": {} })),
        );
        let key_b = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-b",
            10,
            Some(json!({ "pool_advanced": {} })),
        );

        let runtime_by_provider = BTreeMap::from([(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-a".to_string(), 10.0),
                    ("key-b".to_string(), 200.0),
                ]),
                ..AdminProviderPoolRuntimeState::default()
            },
        )]);

        let (reordered, skipped) = apply_local_execution_pool_scheduler_with_runtime_map(
            vec![key_a, key_b],
            &runtime_by_provider,
            &BTreeMap::new(),
        );

        assert!(skipped.is_empty());
        assert_eq!(
            reordered
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-b", "key-a"]
        );
    }

    #[test]
    fn normalizes_distribution_mode_before_strategy_presets() {
        let presets = ProviderPoolService::with_builtin_adapters()
            .normalize_scheduling_presets(
                "openai",
                &[
                    PoolSchedulingPreset {
                        preset: "lru".to_string(),
                        enabled: false,
                        mode: None,
                    },
                    PoolSchedulingPreset {
                        preset: "single_account".to_string(),
                        enabled: true,
                        mode: None,
                    },
                    PoolSchedulingPreset {
                        preset: "cache_affinity".to_string(),
                        enabled: true,
                        mode: None,
                    },
                    PoolSchedulingPreset {
                        preset: "priority_first".to_string(),
                        enabled: true,
                        mode: None,
                    },
                ],
            )
            .into_iter()
            .map(|preset| preset.preset)
            .collect::<Vec<_>>();

        assert_eq!(presets, ["single_account", "priority_first"]);
    }

    #[test]
    fn pool_key_cursor_uses_distribution_order_for_page_queries() {
        let app = AppState::new().expect("state should build");
        let load_balance_group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [
                        {"preset": "load_balance", "enabled": true},
                        {"preset": "priority_first", "enabled": true}
                    ]
                }
            })),
        );
        let load_balance_cursor = PoolKeyCursor::new(
            PlannerAppState::new(&app),
            load_balance_group,
            None,
            None,
            None,
        );
        assert!(matches!(
            load_balance_cursor.pool_key_order,
            StoredPoolKeyCandidateOrder::LoadBalance { ref seed } if !seed.is_empty()
        ));

        let lru_group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );
        let lru_cursor =
            PoolKeyCursor::new(PlannerAppState::new(&app), lru_group, None, None, None);
        assert_eq!(lru_cursor.pool_key_order, StoredPoolKeyCandidateOrder::Lru);
    }

    #[test]
    fn pool_key_cursor_records_runtime_miss_when_exhausted_without_returning_key() {
        let app = AppState::new().expect("state should build");
        let trace_id = "trace-pool-exhausted-runtime-miss";
        app.set_local_execution_runtime_miss_diagnostic(
            trace_id,
            LocalExecutionRuntimeMissDiagnostic {
                reason: "candidate_evaluation_incomplete".to_string(),
                requested_model: Some("gpt-5".to_string()),
                candidate_count: Some(1),
                ..LocalExecutionRuntimeMissDiagnostic::default()
            },
        );
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            Some(json!({ "pool_advanced": { "lru_enabled": true } })),
        );
        let mut cursor = PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None)
            .with_runtime_miss_diagnostic(trace_id, true);
        cursor.record_skip_reason("pool_cooldown");
        cursor.record_skip_reason("pool_cooldown");
        cursor.record_skip_reason("transport_snapshot_missing");

        cursor.log_exhausted();
        apply_local_runtime_candidate_terminal_reason(&app, trace_id, "no_local_sync_plans");

        let diagnostic = app
            .take_local_execution_runtime_miss_diagnostic(trace_id)
            .expect("runtime miss diagnostic should exist");
        assert_eq!(diagnostic.reason, "all_candidates_skipped");
        assert_eq!(diagnostic.skipped_candidate_count, Some(1));
        assert_eq!(diagnostic.skip_reasons.get("pool_cooldown"), Some(&1));
    }

    #[tokio::test]
    async fn pool_key_cursor_rechecks_cooldown_for_frozen_window_candidates() {
        let app = AppState::new().expect("state should build");
        let provider_config = Some(json!({
            "pool_advanced": {
                "rate_limit_cooldown_seconds": 300
            }
        }));
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config.clone(),
        );
        let mut cursor =
            PoolKeyCursor::new(PlannerAppState::new(&app), group.clone(), None, None, None);
        cursor.queued_candidates = VecDeque::from([
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "key-a",
                10,
                provider_config.clone(),
            ),
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "key-b",
                10,
                provider_config.clone(),
            ),
            sample_eligible_candidate("provider-pool", "endpoint-1", "key-c", 10, provider_config),
        ]);

        let first = cursor.next_key().await.expect("first key should schedule");
        assert_eq!(first.candidate.key_id, "key-a");

        let pool_config = pool_config_for_candidate(&group).expect("pool config should parse");
        record_admin_provider_pool_error(
            app.runtime_state.as_ref(),
            "provider-pool",
            "key-b",
            &pool_config,
            429,
            None,
            None,
        )
        .await;

        let second = cursor
            .next_key()
            .await
            .expect("second key should skip the cooled-down frozen key");
        assert_eq!(second.candidate.key_id, "key-c");
        assert_eq!(second.orchestration.pool_key_index, Some(1));

        let skipped = cursor.take_skipped_candidates();
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![("key-b", "pool_cooldown")]
        );
    }

    #[tokio::test]
    async fn pool_key_cursor_filters_expanded_keys_by_routing_profile_allowed_keys() {
        let app = AppState::new().expect("state should build");
        let provider_config = Some(json!({ "pool_advanced": { "lru_enabled": true } }));
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config.clone(),
        );
        let routing_policy = routing_policy_with_allowed_keys(["key-b"]);
        let mut cursor = PoolKeyCursor::new_with_routing_policy(
            PlannerAppState::new(&app),
            group,
            None,
            None,
            None,
            Some(&routing_policy),
        );
        cursor.queued_candidates = VecDeque::from([
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "key-a",
                10,
                provider_config.clone(),
            ),
            sample_eligible_candidate("provider-pool", "endpoint-1", "key-b", 10, provider_config),
        ]);

        let candidate = cursor
            .next_key()
            .await
            .expect("cursor should skip disallowed pool key and return allowed key");
        assert_eq!(candidate.candidate.key_id, "key-b");
        assert_eq!(candidate.orchestration.pool_key_index, Some(0));
        assert_eq!(
            candidate.orchestration.candidate_group_id.as_deref(),
            Some(
                "provider=provider-pool|endpoint=endpoint-1|model=model-1|selected_model=gpt-5|api_format=openai:chat|singleton_key=*"
            )
        );
        assert_eq!(
            cursor
                .skip_reason_counts
                .get(ROUTING_PROFILE_DISALLOWED_KEY_SKIP_REASON),
            Some(&1)
        );
        let skipped = cursor.take_skipped_candidates();
        assert_eq!(
            skipped
                .iter()
                .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
                .collect::<Vec<_>>(),
            vec![("key-a", ROUTING_PROFILE_DISALLOWED_KEY_SKIP_REASON)]
        );
    }

    #[tokio::test]
    async fn pool_key_cursor_allows_parallel_requests_to_use_same_healthy_key() {
        let app = AppState::new().expect("state should build");
        let provider_config = Some(json!({ "pool_advanced": { "lru_enabled": true } }));
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config.clone(),
        );
        let mut cursor = PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None);
        cursor.queued_candidates = VecDeque::from([
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "key-a",
                10,
                provider_config.clone(),
            ),
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "key-b",
                10,
                provider_config.clone(),
            ),
        ]);

        let candidate = cursor
            .next_key()
            .await
            .expect("cursor should return the first healthy key");
        assert_eq!(candidate.candidate.key_id, "key-a");
        assert_eq!(candidate.orchestration.pool_key_index, Some(0));
        assert!(candidate.orchestration.pool_key_lease.is_none());
        assert!(!cursor
            .skip_reason_counts
            .contains_key("pool_key_lease_busy"));

        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config,
        );
        let mut second_cursor =
            PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None);
        second_cursor.queued_candidates = VecDeque::from([
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "key-a",
                10,
                Some(json!({ "pool_advanced": { "lru_enabled": true } })),
            ),
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "key-b",
                10,
                Some(json!({ "pool_advanced": { "lru_enabled": true } })),
            ),
        ]);
        let second_candidate = second_cursor
            .next_key()
            .await
            .expect("second request should also be allowed to pick the same healthy key");
        assert_eq!(second_candidate.candidate.key_id, "key-a");
        assert!(second_candidate.orchestration.pool_key_lease.is_none());
    }

    #[tokio::test]
    async fn pool_key_cursor_skips_key_after_account_cooldown_is_recorded() {
        let app = AppState::new().expect("state should build");
        let provider_config = Some(json!({ "pool_advanced": { "lru_enabled": true } }));
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config.clone(),
        );
        let pool_config = pool_config_for_candidate(&group).expect("pool config should parse");
        let mut cursor = PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None);
        cursor.queued_candidates = VecDeque::from([
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "key-a",
                10,
                provider_config.clone(),
            ),
            sample_eligible_candidate("provider-pool", "endpoint-1", "key-b", 10, provider_config),
        ]);

        record_admin_provider_pool_error(
            app.runtime_state.as_ref(),
            "provider-pool",
            "key-a",
            &pool_config,
            429,
            None,
            None,
        )
        .await;

        let candidate = cursor
            .next_key()
            .await
            .expect("cursor should skip cooled-down key and return next key");
        assert_eq!(candidate.candidate.key_id, "key-b");
        assert_eq!(candidate.orchestration.pool_key_index, Some(0));
        assert!(candidate.orchestration.pool_key_lease.is_none());
        assert_eq!(cursor.skip_reason_counts.get("pool_cooldown"), Some(&1));
    }

    #[tokio::test]
    async fn pool_key_cursor_continues_after_exhausted_window() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "skip_exhausted_accounts": true
            }
        }));
        let (provider, endpoint, mut keys, rows) = large_pool_fixture(3, provider_config.clone());
        for key in keys.iter_mut().take(2) {
            key.status_snapshot = Some(json!({
                "quota": {
                    "provider_type": "openai",
                    "exhausted": true,
                    "usage_ratio": 1.0,
                    "windows": [
                        {
                            "code": "daily",
                            "used_ratio": 1.0,
                            "remaining_ratio": 0.0
                        }
                    ]
                }
            }));
        }

        let data_state =
            GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    vec![provider],
                    vec![endpoint],
                    keys,
                )),
                Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows)),
            )
            .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY);
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config,
        );

        let mut cursor = PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None);
        cursor.window_size = 2;
        cursor.page_size = 2;
        cursor.max_scanned_keys = 4;

        let candidate = cursor
            .next_key()
            .await
            .expect("cursor should scan past an exhausted window");
        assert_eq!(candidate.candidate.key_id, "key-00002");
        assert_eq!(candidate.orchestration.pool_key_index, Some(0));
        assert!(candidate.orchestration.pool_key_lease.is_none());
        assert_eq!(
            cursor
                .skip_reason_counts
                .get(aether_pool_core::POOL_ACCOUNT_EXHAUSTED_SKIP_REASON),
            Some(&2)
        );

        let skipped = cursor.take_skipped_candidates();
        assert_eq!(skipped.len(), 2);
        assert!(skipped.iter().all(|candidate| {
            candidate.skip_reason == aether_pool_core::POOL_ACCOUNT_EXHAUSTED_SKIP_REASON
        }));
    }

    #[tokio::test]
    async fn pool_key_cursor_does_not_spend_effective_scan_budget_on_exhausted_accounts() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "skip_exhausted_accounts": true
            }
        }));
        let (provider, endpoint, mut keys, rows) = large_pool_fixture(700, provider_config.clone());
        for key in keys.iter_mut().take(600) {
            key.status_snapshot = Some(json!({
                "quota": {
                    "provider_type": "openai",
                    "exhausted": true,
                    "usage_ratio": 1.0,
                    "windows": [
                        {
                            "code": "daily",
                            "used_ratio": 1.0,
                            "remaining_ratio": 0.0
                        }
                    ]
                }
            }));
        }

        let data_state =
            GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    vec![provider],
                    vec![endpoint],
                    keys,
                )),
                Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows)),
            )
            .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY);
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config,
        );

        let mut cursor = PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None);
        assert_eq!(
            cursor.max_scanned_keys,
            aether_dispatch_core::DEFAULT_POOL_MAX_SCAN
        );
        assert!(
            cursor.absolute_max_scanned_keys > cursor.max_scanned_keys,
            "pool config scan limit should be retained as the absolute cap"
        );

        let candidate = cursor
            .next_key()
            .await
            .expect("cursor should scan past exhausted accounts within the absolute cap");
        let key_index = candidate
            .candidate
            .key_id
            .strip_prefix("key-")
            .and_then(|value| value.parse::<usize>().ok())
            .expect("fixture key id should contain a numeric suffix");
        assert!(
            key_index >= 600,
            "cursor should not return one of the exhausted leading keys"
        );
        assert_eq!(candidate.orchestration.pool_key_index, Some(0));
        assert_eq!(cursor.scanned_keys, 640);
        assert_eq!(cursor.budget_scanned_keys, 40);
        assert_eq!(
            cursor
                .skip_reason_counts
                .get(aether_pool_core::POOL_ACCOUNT_EXHAUSTED_SKIP_REASON),
            Some(&600)
        );
    }

    #[tokio::test]
    async fn pool_key_cursor_does_not_spend_effective_scan_budget_on_blocked_accounts() {
        const BLOCKED_COUNT: usize = 1_600;
        let provider_config = Some(json!({ "pool_advanced": {} }));
        let (provider, endpoint, mut keys, rows) =
            large_pool_fixture(BLOCKED_COUNT + 100, provider_config.clone());
        for key in keys.iter_mut().take(BLOCKED_COUNT) {
            key.oauth_invalid_reason = Some("blocked account".to_string());
        }

        let data_state =
            GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    vec![provider],
                    vec![endpoint],
                    keys,
                )),
                Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows)),
            )
            .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY);
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config,
        );

        let mut cursor = PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None);
        assert_eq!(
            cursor.max_scanned_keys,
            aether_dispatch_core::DEFAULT_POOL_MAX_SCAN
        );
        assert!(
            cursor.absolute_max_scanned_keys >= u32::try_from(BLOCKED_COUNT + 1).unwrap(),
            "default absolute scan cap should allow scanning past a large blocked prefix"
        );

        let candidate = cursor
            .next_key()
            .await
            .expect("cursor should scan past blocked accounts within the absolute cap");
        let key_index = candidate
            .candidate
            .key_id
            .strip_prefix("key-")
            .and_then(|value| value.parse::<usize>().ok())
            .expect("fixture key id should contain a numeric suffix");
        assert!(
            key_index >= BLOCKED_COUNT,
            "cursor should not return one of the blocked leading keys"
        );
        assert_eq!(candidate.orchestration.pool_key_index, Some(0));
        assert!(
            cursor.budget_scanned_keys <= aether_dispatch_core::DEFAULT_POOL_PAGE_SIZE,
            "blocked accounts should not consume effective scan budget"
        );
        assert_eq!(
            cursor
                .skip_reason_counts
                .get(aether_pool_core::POOL_ACCOUNT_BLOCKED_SKIP_REASON),
            Some(&(BLOCKED_COUNT as u32))
        );
    }

    #[tokio::test]
    async fn pool_key_cursor_does_not_spend_scan_budget_on_missing_score_rows() {
        let provider_config = Some(json!({
            "pool_advanced": {
                "score_top_n": 128
            }
        }));
        let (provider, endpoint, keys, rows) = large_pool_fixture(1, provider_config.clone());
        let scores = (0..128)
            .map(|index| {
                sample_provider_key_pool_score(
                    "provider-pool",
                    &format!("missing-key-{index:03}"),
                    1_000.0 - index as f64,
                )
            })
            .collect::<Vec<_>>();

        let data_state =
            GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    vec![provider],
                    vec![endpoint],
                    keys,
                )),
                Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows)),
            )
            .with_pool_score_repository_for_tests(Arc::new(
                InMemoryPoolMemberScoreRepository::seed(scores),
            ))
            .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY);
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config,
        );

        let mut cursor = PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None);
        let candidate = cursor
            .next_key()
            .await
            .expect("cursor should fall back to catalog rows after stale scores");

        assert_eq!(candidate.candidate.key_id, "key-00000");
        assert_eq!(cursor.scanned_keys, 1);
        assert_eq!(cursor.budget_scanned_keys, 1);
        assert_eq!(
            cursor.skip_reason_counts.get("pool_score_member_missing"),
            Some(&128)
        );
    }

    #[tokio::test]
    async fn pool_scheduler_skips_invalid_and_exhausted_high_priority_hot_pool_before_fallback_provider(
    ) {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true,
                "skip_exhausted_accounts": true,
                "scheduling_presets": [
                    {"preset": "single_account", "enabled": true}
                ]
            }
        }));
        let provider_a = sample_codex_pool_provider("provider-a", 0, provider_config.clone());
        let provider_b = sample_codex_pool_provider("provider-b", 10, provider_config.clone());
        let endpoint_a = sample_codex_pool_endpoint("provider-a", "endpoint-a");
        let endpoint_b = sample_codex_pool_endpoint("provider-b", "endpoint-b");

        let mut key_a_invalid = sample_codex_pool_key("provider-a", "key-a-invalid");
        key_a_invalid.oauth_invalid_at_unix_secs = Some(1_710_000_000);
        key_a_invalid.oauth_invalid_reason =
            Some("[OAUTH_EXPIRED] Codex Token 无效或已过期 (401)".to_string());
        let exhausted_status_snapshot = json!({
            "quota": {
                "provider_type": "codex",
                "exhausted": true,
                "usage_ratio": 1.0,
                "windows": [
                    {
                        "code": "daily",
                        "used_ratio": 1.0,
                        "remaining_ratio": 0.0
                    }
                ]
            }
        });
        key_a_invalid.status_snapshot = Some(exhausted_status_snapshot.clone());
        let mut key_a_exhausted = sample_codex_pool_key("provider-a", "key-a-exhausted");
        key_a_exhausted.status_snapshot = Some(exhausted_status_snapshot);
        let key_b_ready = sample_codex_pool_key("provider-b", "key-b-ready");

        let rows = vec![
            sample_codex_pool_row("provider-a", "endpoint-a", "key-a-invalid", 0),
            sample_codex_pool_row("provider-a", "endpoint-a", "key-a-exhausted", 0),
            sample_codex_pool_row("provider-b", "endpoint-b", "key-b-ready", 10),
        ];

        let data_state =
            GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    vec![provider_a, provider_b],
                    vec![endpoint_a, endpoint_b],
                    vec![key_a_invalid, key_a_exhausted, key_b_ready],
                )),
                Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows)),
            )
            .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY);
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        app.runtime_state
            .set_add(
                &admin_provider_pool_quota_probe_active_members_key("provider-a"),
                "key-a-invalid",
            )
            .await
            .expect("provider-a hot member should insert");
        app.runtime_state
            .set_add(
                &admin_provider_pool_quota_probe_active_members_key("provider-b"),
                "key-b-ready",
            )
            .await
            .expect("provider-b hot member should insert");

        let group_a =
            sample_codex_pool_group("provider-a", "endpoint-a", 0, provider_config.clone());
        let group_b = sample_codex_pool_group("provider-b", "endpoint-b", 10, provider_config);

        let (scheduled, skipped) = apply_local_execution_pool_scheduler(
            PlannerAppState::new(&app),
            vec![group_a, group_b],
            None,
            Some("gpt-5"),
            None,
        )
        .await;

        assert_eq!(
            scheduled
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-b-ready"]
        );
        let skipped_pairs = skipped
            .iter()
            .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
            .collect::<Vec<_>>();
        assert!(skipped_pairs.contains(&("key-a-invalid", "pool_account_blocked")));
        assert!(skipped_pairs.contains(&(
            "key-a-exhausted",
            aether_pool_core::POOL_ACCOUNT_EXHAUSTED_SKIP_REASON
        )));
    }

    #[tokio::test]
    async fn pool_scheduler_skips_invalid_high_priority_hot_pool_account_even_with_remaining_quota()
    {
        let provider_config = Some(json!({
            "pool_advanced": {
                "probing_enabled": true,
                "skip_exhausted_accounts": true,
                "scheduling_presets": [
                    {"preset": "single_account", "enabled": true}
                ]
            }
        }));
        let provider_a = sample_codex_pool_provider("provider-a", 0, provider_config.clone());
        let provider_b = sample_codex_pool_provider("provider-b", 10, provider_config.clone());
        let endpoint_a = sample_codex_pool_endpoint("provider-a", "endpoint-a");
        let endpoint_b = sample_codex_pool_endpoint("provider-b", "endpoint-b");

        let mut key_a_invalid = sample_codex_pool_key("provider-a", "key-a-invalid");
        key_a_invalid.oauth_invalid_at_unix_secs = Some(1_710_000_000);
        key_a_invalid.oauth_invalid_reason =
            Some("[OAUTH_EXPIRED] Codex Token 无效或已过期 (401)".to_string());
        key_a_invalid.status_snapshot = Some(json!({
            "quota": {
                "provider_type": "codex",
                "exhausted": false,
                "usage_ratio": 0.25,
                "windows": [
                    {
                        "code": "daily",
                        "used_ratio": 0.25,
                        "remaining_ratio": 0.75
                    }
                ]
            }
        }));
        let key_b_ready = sample_codex_pool_key("provider-b", "key-b-ready");

        let rows = vec![
            sample_codex_pool_row("provider-a", "endpoint-a", "key-a-invalid", 0),
            sample_codex_pool_row("provider-b", "endpoint-b", "key-b-ready", 10),
        ];

        let data_state =
            GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    vec![provider_a, provider_b],
                    vec![endpoint_a, endpoint_b],
                    vec![key_a_invalid, key_b_ready],
                )),
                Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows)),
            )
            .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY);
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        app.runtime_state
            .set_add(
                &admin_provider_pool_quota_probe_active_members_key("provider-a"),
                "key-a-invalid",
            )
            .await
            .expect("provider-a hot member should insert");
        app.runtime_state
            .set_add(
                &admin_provider_pool_quota_probe_active_members_key("provider-b"),
                "key-b-ready",
            )
            .await
            .expect("provider-b hot member should insert");

        let group_a =
            sample_codex_pool_group("provider-a", "endpoint-a", 0, provider_config.clone());
        let group_b = sample_codex_pool_group("provider-b", "endpoint-b", 10, provider_config);

        let (scheduled, skipped) = apply_local_execution_pool_scheduler(
            PlannerAppState::new(&app),
            vec![group_a, group_b],
            None,
            Some("gpt-5"),
            None,
        )
        .await;

        assert_eq!(
            scheduled
                .iter()
                .map(|item| item.candidate.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-b-ready"]
        );
        let skipped_pairs = skipped
            .iter()
            .map(|item| (item.candidate.key_id.as_str(), item.skip_reason))
            .collect::<Vec<_>>();
        assert!(skipped_pairs.contains(&("key-a-invalid", "pool_account_blocked")));
    }

    #[test]
    fn pool_key_reauth_scheduling_keeps_recoverable_oauth_markers_usable() {
        let mut key = sample_codex_pool_key("provider-a", "key-refresh-failed");
        key.expires_at_unix_secs = Some(200);
        key.oauth_invalid_reason = Some(
            "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 已被使用并轮换，请重新登录授权"
                .to_string(),
        );

        assert!(!pool_key_requires_reauth_for_scheduling(&key, 100));
        assert!(pool_key_requires_reauth_for_scheduling(&key, 200));

        key.oauth_invalid_reason = Some("[REQUEST_FAILED] 账号状态检查失败".to_string());
        key.oauth_invalid_at_unix_secs = Some(100);
        assert!(!pool_key_requires_reauth_for_scheduling(&key, 300));
    }

    #[test]
    fn pool_key_reauth_scheduling_blocks_invalid_oauth_markers_without_affecting_non_oauth_keys() {
        let mut key = sample_codex_pool_key("provider-a", "key-invalid");
        key.oauth_invalid_reason = Some("[ACCOUNT_BLOCK] account has been deactivated".to_string());
        assert!(pool_key_requires_reauth_for_scheduling(&key, 100));

        key.oauth_invalid_reason = Some("Kiro Token 无效或已过期".to_string());
        key.oauth_invalid_at_unix_secs = None;
        assert!(pool_key_requires_reauth_for_scheduling(&key, 100));

        key.oauth_invalid_reason = None;
        key.oauth_invalid_at_unix_secs = Some(100);
        assert!(pool_key_requires_reauth_for_scheduling(&key, 100));

        key.auth_type = "api_key".to_string();
        assert!(!pool_key_requires_reauth_for_scheduling(&key, 100));
    }

    #[tokio::test]
    async fn pool_key_cursor_simulates_large_lru_pool_with_lazy_pages_and_dynamic_skips() {
        const KEY_COUNT: usize = 2048;
        let provider_config = Some(json!({
            "pool_advanced": {
                "lru_enabled": true,
                "rate_limit_cooldown_seconds": 300
            }
        }));
        let (provider, endpoint, keys, rows) =
            large_pool_fixture(KEY_COUNT, provider_config.clone());
        let data_state =
            GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    vec![provider],
                    vec![endpoint],
                    keys,
                )),
                Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows)),
            )
            .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY);
        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state);
        let group = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "pool-group",
            10,
            provider_config.clone(),
        );
        let pool_config = pool_config_for_candidate(&group).expect("pool config should parse");
        for key_id in ["key-00000", "key-00001"] {
            record_admin_provider_pool_error(
                app.runtime_state.as_ref(),
                "provider-pool",
                key_id,
                &pool_config,
                429,
                None,
                None,
            )
            .await;
        }

        let mut cursor = PoolKeyCursor::new(PlannerAppState::new(&app), group, None, None, None);
        assert_eq!(
            cursor.window_size,
            aether_dispatch_core::DEFAULT_POOL_WINDOW_SIZE
        );
        assert_eq!(
            cursor.page_size,
            aether_dispatch_core::DEFAULT_POOL_PAGE_SIZE
        );
        assert_eq!(
            cursor.max_scanned_keys,
            aether_dispatch_core::DEFAULT_POOL_MAX_SCAN
        );

        let mut returned_ids = Vec::new();
        for _ in 0..10 {
            let candidate = cursor
                .next_key()
                .await
                .expect("large pool should return first page candidates");
            returned_ids.push(candidate.candidate.key_id.clone());
            assert!(candidate.orchestration.pool_key_lease.is_none());
        }
        assert_eq!(returned_ids.first().map(String::as_str), Some("key-00002"));
        assert_eq!(returned_ids.last().map(String::as_str), Some("key-00011"));
        assert_eq!(cursor.scanned_keys, 64);
        assert!(
            cursor.queued_candidates.len() <= cursor.window_size as usize,
            "cursor should only retain the current page window"
        );

        record_admin_provider_pool_error(
            app.runtime_state.as_ref(),
            "provider-pool",
            "key-00014",
            &pool_config,
            429,
            None,
            None,
        )
        .await;

        let candidate = cursor
            .next_key()
            .await
            .expect("cursor should keep the frozen window despite later runtime changes");
        assert_eq!(candidate.candidate.key_id, "key-00012");
        returned_ids.push(candidate.candidate.key_id.clone());
        assert!(candidate.orchestration.pool_key_lease.is_none());

        while let Some(candidate) = cursor.next_key().await {
            returned_ids.push(candidate.candidate.key_id.clone());
            assert!(candidate.orchestration.pool_key_lease.is_none());
        }

        let max_returned_windows = aether_dispatch_core::DEFAULT_POOL_MAX_SCAN
            / aether_dispatch_core::DEFAULT_POOL_PAGE_SIZE;
        assert!(
            returned_ids.len()
                <= (max_returned_windows * aether_dispatch_core::DEFAULT_POOL_WINDOW_SIZE) as usize,
            "cursor should only return bounded frozen windows per request"
        );
        assert_eq!(returned_ids.len(), 127);
        assert_eq!(returned_ids.last().map(String::as_str), Some("key-00463"));
        assert_eq!(cursor.scanned_keys, 512);
        assert_eq!(cursor.skip_reason_counts.get("pool_cooldown"), Some(&3));
        assert!(!cursor
            .skip_reason_counts
            .contains_key("pool_key_lease_busy"));
        for skipped in ["key-00000", "key-00001", "key-00014"] {
            assert!(
                !returned_ids.iter().any(|key_id| key_id == skipped),
                "{skipped} should have been skipped"
            );
        }
        assert!(
            returned_ids.iter().any(|key_id| key_id == "key-00015"),
            "key-00015 should not be blocked by request-scoped leases"
        );
    }

    #[test]
    fn builds_pool_catalog_context_from_status_snapshot_and_auth_config() {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            None,
            "secret".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("transport fields should build");
        key.status_snapshot = Some(json!({
            "account": {"blocked": false},
            "quota": {
                "usage_ratio": 0.25,
                "reset_seconds": 3600,
                "exhausted": false,
                "plan_type": "team"
            }
        }));
        key.success_count = Some(4);
        key.total_response_time_ms = Some(200);
        key.last_used_at_unix_secs = Some(1_711_000_123);

        let app = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    Vec::new(),
                    Vec::new(),
                    vec![key.clone()],
                )),
            ));

        let context = build_pool_catalog_key_context(
            PlannerAppState::new(&app),
            &ProviderPoolService::with_builtin_adapters(),
            &key,
            "codex",
        );

        assert_eq!(context.plan_tier.as_deref(), Some("team"));
        assert_eq!(context.quota_usage_ratio, Some(0.25));
        assert_eq!(context.quota_reset_seconds, Some(3600.0));
        assert_eq!(context.latency_avg_ms, Some(50.0));
        assert_eq!(context.catalog_lru_score, Some(1_711_000_123.0));
    }

    #[test]
    fn pool_catalog_context_ignores_stale_codex_exhausted_snapshot_when_windows_have_capacity() {
        let mut key = sample_catalog_oauth_key("key-stale-exhausted");
        key.upstream_metadata = Some(json!({
            "codex": {
                "primary_used_percent": 100.0
            }
        }));
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "code": "exhausted",
                "exhausted": true,
                "usage_ratio": 0.0,
                "windows": [
                    {
                        "code": "weekly",
                        "used_ratio": 0.0,
                        "remaining_ratio": 1.0
                    },
                    {
                        "code": "5h",
                        "used_ratio": 0.0,
                        "remaining_ratio": 1.0
                    }
                ]
            }
        }));

        let app = app_state_with_catalog_key(key.clone());
        let context = build_pool_catalog_key_context(
            PlannerAppState::new(&app),
            &ProviderPoolService::with_builtin_adapters(),
            &key,
            "codex",
        );

        assert!(!context.quota_exhausted);
    }

    #[test]
    fn pool_catalog_context_marks_codex_metadata_exhausted() {
        let mut key = sample_catalog_oauth_key("key-metadata-exhausted");
        key.upstream_metadata = Some(json!({
            "codex": {
                "secondary_used_percent": 100.0
            }
        }));

        let app = app_state_with_catalog_key(key.clone());
        let context = build_pool_catalog_key_context(
            PlannerAppState::new(&app),
            &ProviderPoolService::with_builtin_adapters(),
            &key,
            "codex",
        );

        assert!(context.quota_exhausted);
    }

    #[test]
    fn pool_catalog_context_preserves_snapshot_exhaustion_for_snapshot_only_providers() {
        let mut key = sample_catalog_oauth_key("key-antigravity-exhausted");
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "antigravity",
                "code": "exhausted",
                "exhausted": true,
                "windows": [
                    {
                        "code": "gemini-2.5-pro",
                        "used_ratio": 1.0,
                        "remaining_ratio": 0.0
                    }
                ]
            }
        }));

        let app = app_state_with_catalog_key(key.clone());
        let context = build_pool_catalog_key_context(
            PlannerAppState::new(&app),
            &ProviderPoolService::with_builtin_adapters(),
            &key,
            "antigravity",
        );

        assert!(context.quota_exhausted);
    }

    #[test]
    fn pool_catalog_context_marks_known_banned_account_from_metadata() {
        let mut key = sample_catalog_oauth_key("key-account-banned");
        key.upstream_metadata = Some(json!({
            "codex": {
                "account_disabled": true,
                "reason": "deactivated_workspace"
            }
        }));

        let app = app_state_with_catalog_key(key.clone());
        let context = build_pool_catalog_key_context(
            PlannerAppState::new(&app),
            &ProviderPoolService::with_builtin_adapters(),
            &key,
            "codex",
        );

        assert!(context.account_blocked);
    }

    fn sample_catalog_oauth_key(key_id: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            key_id.to_string(),
            "provider-1".to_string(),
            key_id.to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            None,
            "secret".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("transport fields should build")
    }

    fn app_state_with_catalog_key(key: StoredProviderCatalogKey) -> AppState {
        AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                Arc::new(InMemoryProviderCatalogReadRepository::seed(
                    Vec::new(),
                    Vec::new(),
                    vec![key],
                )),
            ))
    }

    fn large_pool_fixture(
        key_count: usize,
        provider_config: Option<serde_json::Value>,
    ) -> (
        StoredProviderCatalogProvider,
        StoredProviderCatalogEndpoint,
        Vec<StoredProviderCatalogKey>,
        Vec<StoredMinimalCandidateSelectionRow>,
    ) {
        let provider = StoredProviderCatalogProvider::new(
            "provider-pool".to_string(),
            "provider-pool".to_string(),
            Some("https://example.com".to_string()),
            "openai".to_string(),
        )
        .expect("provider should build")
        .with_routing_fields(0)
        .with_transport_fields(
            true,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            provider_config,
        );
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-pool".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_health_score(1.0)
        .with_transport_fields(
            "https://example.com/v1/chat/completions".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build");

        let mut keys = Vec::with_capacity(key_count);
        let mut rows = Vec::with_capacity(key_count);
        for index in 0..key_count {
            let key_id = format!("key-{index:05}");
            let mut key = StoredProviderCatalogKey::new(
                key_id.clone(),
                "provider-pool".to_string(),
                key_id.clone(),
                "api_key".to_string(),
                None,
                true,
            )
            .expect("key should build")
            .with_transport_fields(
                Some(json!(["openai:chat"])),
                Some(format!("secret-{index}")),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("key transport should build");
            key.internal_priority = 10;
            key.last_used_at_unix_secs = Some(index as u64);
            keys.push(key);
            rows.push(StoredMinimalCandidateSelectionRow {
                provider_id: "provider-pool".to_string(),
                provider_name: "provider-pool".to_string(),
                provider_type: "openai".to_string(),
                provider_priority: 0,
                provider_is_active: true,
                endpoint_id: "endpoint-1".to_string(),
                endpoint_api_format: "openai:chat".to_string(),
                endpoint_api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_is_active: true,
                key_id: key_id.clone(),
                key_name: key_id,
                key_auth_type: "api_key".to_string(),
                key_is_active: true,
                key_api_formats: Some(vec!["openai:chat".to_string()]),
                key_allowed_models: None,
                key_capabilities: None,
                key_internal_priority: 10,
                key_global_priority_by_format: None,
                model_id: "model-1".to_string(),
                global_model_id: "global-model-1".to_string(),
                global_model_name: "gpt-5".to_string(),
                global_model_mappings: None,
                global_model_supports_streaming: Some(true),
                model_provider_model_name: "gpt-5".to_string(),
                model_provider_model_mappings: None,
                model_supports_streaming: Some(true),
                model_is_active: true,
                model_is_available: true,
            });
        }
        (provider, endpoint, keys, rows)
    }

    fn sample_provider_key_pool_score(
        provider_id: &str,
        key_id: &str,
        score: f64,
    ) -> StoredPoolMemberScore {
        let identity = PoolMemberIdentity::provider_api_key(provider_id, key_id);
        let scope = provider_key_pool_score_scope();
        StoredPoolMemberScore {
            id: provider_key_pool_score_id(&identity, &scope),
            pool_kind: identity.pool_kind,
            pool_id: identity.pool_id,
            member_kind: identity.member_kind,
            member_id: identity.member_id,
            capability: scope.capability,
            scope_kind: scope.scope_kind,
            scope_id: scope.scope_id,
            score,
            hard_state: PoolMemberHardState::Available,
            score_version: 1,
            score_reason: json!({}),
            last_ranked_at: Some(1_000),
            last_scheduled_at: None,
            last_success_at: None,
            last_failure_at: None,
            failure_count: 0,
            last_probe_attempt_at: None,
            last_probe_success_at: None,
            last_probe_failure_at: None,
            probe_failure_count: 0,
            probe_status: PoolMemberProbeStatus::Ok,
            updated_at: 1_000,
        }
    }

    fn sample_codex_pool_provider(
        provider_id: &str,
        provider_priority: i32,
        provider_config: Option<serde_json::Value>,
    ) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            provider_id.to_string(),
            provider_id.to_string(),
            Some("https://example.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")
        .with_routing_fields(provider_priority)
        .with_transport_fields(
            true,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            provider_config,
        )
    }

    fn sample_codex_pool_endpoint(
        provider_id: &str,
        endpoint_id: &str,
    ) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            endpoint_id.to_string(),
            provider_id.to_string(),
            "openai:responses".to_string(),
            Some("openai".to_string()),
            Some("responses".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_health_score(1.0)
        .with_transport_fields(
            "https://example.com/v1/responses".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_codex_pool_key(provider_id: &str, key_id: &str) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider_id.to_string(),
            key_id.to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:responses"])),
            Some(format!("secret-{key_id}")),
            None,
            None,
            Some(json!({"openai:responses": 1})),
            None,
            Some(4_102_444_800),
            None,
            None,
        )
        .expect("key transport should build");
        key.internal_priority = 10;
        key
    }

    fn sample_codex_pool_row(
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        provider_priority: i32,
    ) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: provider_id.to_string(),
            provider_name: provider_id.to_string(),
            provider_type: "codex".to_string(),
            provider_priority,
            provider_is_active: true,
            endpoint_id: endpoint_id.to_string(),
            endpoint_api_format: "openai:responses".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("responses".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "oauth".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:responses".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 10,
            key_global_priority_by_format: Some(json!({"openai:responses": 1})),
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5".to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_codex_pool_group(
        provider_id: &str,
        endpoint_id: &str,
        provider_priority: i32,
        provider_config: Option<serde_json::Value>,
    ) -> EligibleLocalExecutionCandidate {
        EligibleLocalExecutionCandidate {
            kind: LocalExecutionCandidateKind::PoolGroup,
            candidate: SchedulerMinimalCandidateSelectionCandidate {
                provider_id: provider_id.to_string(),
                provider_name: provider_id.to_string(),
                provider_type: "codex".to_string(),
                provider_priority,
                endpoint_id: endpoint_id.to_string(),
                endpoint_api_format: "openai:responses".to_string(),
                key_id: format!("{provider_id}-pool-group"),
                key_name: format!("{provider_id}-pool-group"),
                key_auth_type: "oauth".to_string(),
                key_internal_priority: 10,
                key_global_priority_for_format: Some(1),
                key_capabilities: None,
                model_id: "model-1".to_string(),
                global_model_id: "global-model-1".to_string(),
                global_model_name: "gpt-5".to_string(),
                selected_provider_model_name: "gpt-5".to_string(),
                mapping_matched_model: None,
            },
            provider_api_format: "openai:responses".to_string(),
            orchestration: LocalExecutionCandidateMetadata::default(),
            ranking: None,
            transport: Arc::new(crate::ai_serving::GatewayProviderTransportSnapshot {
                provider: GatewayProviderTransportProvider {
                    id: provider_id.to_string(),
                    name: provider_id.to_string(),
                    provider_type: "codex".to_string(),
                    website: None,
                    is_active: true,
                    keep_priority_on_conversion: false,
                    enable_format_conversion: false,
                    concurrent_limit: None,
                    max_retries: None,
                    proxy: None,
                    request_timeout_secs: None,
                    stream_first_byte_timeout_secs: None,
                    config: provider_config,
                },
                endpoint: GatewayProviderTransportEndpoint {
                    id: endpoint_id.to_string(),
                    provider_id: provider_id.to_string(),
                    api_format: "openai:responses".to_string(),
                    api_family: Some("openai".to_string()),
                    endpoint_kind: Some("responses".to_string()),
                    is_active: true,
                    base_url: "https://example.com/v1/responses".to_string(),
                    header_rules: None,
                    body_rules: None,
                    max_retries: None,
                    custom_path: None,
                    config: None,
                    format_acceptance_config: None,
                    proxy: None,
                },
                key: GatewayProviderTransportKey {
                    id: format!("{provider_id}-pool-group"),
                    provider_id: provider_id.to_string(),
                    name: format!("{provider_id}-pool-group"),
                    auth_type: "oauth".to_string(),
                    is_active: true,
                    api_formats: Some(vec!["openai:responses".to_string()]),
                    auth_type_by_format: None,
                    allow_auth_channel_mismatch_formats: None,
                    allowed_models: None,
                    capabilities: None,
                    rate_multipliers: None,
                    global_priority_by_format: None,
                    expires_at_unix_secs: None,
                    proxy: None,
                    fingerprint: None,
                    upstream_metadata: None,
                    decrypted_api_key: "secret".to_string(),
                    decrypted_auth_config: None,
                },
            }),
        }
    }

    fn routing_policy_with_allowed_keys<const N: usize>(
        key_ids: [&str; N],
    ) -> ResolvedRoutingPolicy {
        ResolvedRoutingPolicy {
            group_id: Some("routing-group-1".to_string()),
            group_version: Some(1),
            selection_source: "test".to_string(),
            requested_model: "gpt-5".to_string(),
            resolved_model: "gpt-5".to_string(),
            priority_mode: RoutingSetPriorityMode::Provider,
            scheduling_mode: RoutingSchedulingMode::CacheAffinity,
            keep_priority_on_conversion: false,
            ranking_overlay: RankingOverlay {
                allowed_keys: key_ids.into_iter().map(str::to_string).collect(),
                ..RankingOverlay::default()
            },
            mutation_plan: Default::default(),
            pool_policy_overrides: BTreeMap::new(),
            matched_rules: Vec::new(),
        }
    }

    fn sample_eligible_candidate(
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        internal_priority: i32,
        provider_config: Option<serde_json::Value>,
    ) -> EligibleLocalExecutionCandidate {
        EligibleLocalExecutionCandidate {
            kind: if provider_config.is_some() {
                LocalExecutionCandidateKind::PoolGroup
            } else {
                LocalExecutionCandidateKind::SingleKey
            },
            candidate: SchedulerMinimalCandidateSelectionCandidate {
                provider_id: provider_id.to_string(),
                provider_name: provider_id.to_string(),
                provider_type: "codex".to_string(),
                provider_priority: 10,
                endpoint_id: endpoint_id.to_string(),
                endpoint_api_format: "openai:chat".to_string(),
                key_id: key_id.to_string(),
                key_name: key_id.to_string(),
                key_auth_type: "api_key".to_string(),
                key_internal_priority: internal_priority,
                key_global_priority_for_format: Some(1),
                key_capabilities: None,
                model_id: "model-1".to_string(),
                global_model_id: "global-model-1".to_string(),
                global_model_name: "gpt-5".to_string(),
                selected_provider_model_name: "gpt-5".to_string(),
                mapping_matched_model: None,
            },
            provider_api_format: "openai:chat".to_string(),
            orchestration: LocalExecutionCandidateMetadata::default(),
            ranking: None,
            transport: Arc::new(crate::ai_serving::GatewayProviderTransportSnapshot {
                provider: GatewayProviderTransportProvider {
                    id: provider_id.to_string(),
                    name: provider_id.to_string(),
                    provider_type: "codex".to_string(),
                    website: None,
                    is_active: true,
                    keep_priority_on_conversion: false,
                    enable_format_conversion: false,
                    concurrent_limit: None,
                    max_retries: None,
                    proxy: None,
                    request_timeout_secs: None,
                    stream_first_byte_timeout_secs: None,
                    config: provider_config,
                },
                endpoint: GatewayProviderTransportEndpoint {
                    id: endpoint_id.to_string(),
                    provider_id: provider_id.to_string(),
                    api_format: "openai:chat".to_string(),
                    api_family: Some("openai".to_string()),
                    endpoint_kind: Some("chat".to_string()),
                    is_active: true,
                    base_url: "https://example.com".to_string(),
                    header_rules: None,
                    body_rules: None,
                    max_retries: None,
                    custom_path: None,
                    config: None,
                    format_acceptance_config: None,
                    proxy: None,
                },
                key: GatewayProviderTransportKey {
                    id: key_id.to_string(),
                    provider_id: provider_id.to_string(),
                    name: key_id.to_string(),
                    auth_type: "api_key".to_string(),
                    is_active: true,
                    api_formats: Some(vec!["openai:chat".to_string()]),
                    auth_type_by_format: None,
                    allow_auth_channel_mismatch_formats: None,

                    allowed_models: None,
                    capabilities: None,
                    rate_multipliers: None,
                    global_priority_by_format: None,
                    expires_at_unix_secs: None,
                    proxy: None,
                    fingerprint: None,
                    upstream_metadata: None,
                    decrypted_api_key: "secret".to_string(),
                    decrypted_auth_config: None,
                },
            }),
        }
    }
}
