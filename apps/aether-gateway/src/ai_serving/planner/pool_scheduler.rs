use std::collections::{btree_map::Entry, BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Duration;

use aether_ai_serving::{
    run_ai_pool_scheduler, AiPoolCandidateFacts, AiPoolCandidateInput,
    AiPoolCandidateOrchestration, AiPoolCatalogKeyContext, AiPoolRuntimeState,
    AiPoolSchedulingConfig, AiPoolSchedulingPreset,
};
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredPoolKeyCandidateRowsQuery,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::{Map, Value};
use tracing::{debug, warn};

use crate::ai_serving::planner::candidate_resolution::{
    candidate_auth_channel_skip_reason, read_candidate_transport_snapshot,
    EligibleLocalExecutionCandidate, LocalExecutionCandidateKind, SkippedLocalExecutionCandidate,
};
use crate::ai_serving::{
    candidate_common_transport_skip_reason, CandidateTransportPolicyFacts, PlannerAppState,
};
use crate::cache::{CachedPoolCandidates, PoolCandidateCacheKey};
use crate::clock::current_unix_ms;
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::handlers::shared::provider_pool::read_admin_provider_pool_runtime_state;
use crate::handlers::shared::provider_pool::{
    AdminProviderPoolConfig, AdminProviderPoolRuntimeState,
};
use crate::handlers::shared::{
    parse_catalog_auth_config_json, provider_key_health_summary,
    provider_key_status_snapshot_payload,
};
use crate::maintenance::pool_preheat_probe_provider_is_supported;
use crate::orchestration::LocalExecutionCandidateMetadata;
use crate::provider_key_auth::provider_key_auth_semantics;

static LOAD_BALANCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const DEFAULT_POOL_KEY_PAGE_SIZE: u32 = 128;
const DEFAULT_POOL_MAX_SCANNED_KEYS: u32 = 1024;
const POOL_CANDIDATE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const POOL_CANDIDATE_CACHE_MAX_ENTRIES: usize = 1024;
const POOL_CANDIDATE_CACHE_MAX_ROWS: usize = 128;

type PoolCatalogKeyContext = AiPoolCatalogKeyContext;

struct PoolCandidateRowsPage {
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    next_offset: u32,
}

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
        runtime_by_provider.insert(provider_id, runtime);
    }

    apply_local_execution_pool_scheduler_with_runtime_map(
        candidates,
        &runtime_by_provider,
        &key_context_by_id,
    )
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
    next_offset: u32,
    scanned_keys: u32,
    page_size: u32,
    max_scanned_keys: u32,
    skip_reason_counts: BTreeMap<&'static str, u32>,
    next_pool_key_index: u32,
    queued_candidates: VecDeque<EligibleLocalExecutionCandidate>,
    skipped_candidates: Vec<SkippedLocalExecutionCandidate>,
    exhausted_logged: bool,
}

impl<'a> PoolKeyCursor<'a> {
    pub(crate) fn new(
        state: PlannerAppState<'a>,
        group: EligibleLocalExecutionCandidate,
        sticky_session_token: Option<&str>,
        requested_model: Option<&str>,
        request_auth_channel: Option<&str>,
    ) -> Self {
        Self {
            state,
            group,
            sticky_session_token: sticky_session_token.map(str::to_string),
            requested_model: requested_model.map(str::to_string),
            request_auth_channel: request_auth_channel.map(str::to_string),
            next_offset: 0,
            scanned_keys: 0,
            page_size: DEFAULT_POOL_KEY_PAGE_SIZE,
            max_scanned_keys: DEFAULT_POOL_MAX_SCANNED_KEYS,
            skip_reason_counts: BTreeMap::new(),
            next_pool_key_index: 0,
            queued_candidates: VecDeque::new(),
            skipped_candidates: Vec::new(),
            exhausted_logged: false,
        }
    }

    pub(crate) async fn next_key(&mut self) -> Option<EligibleLocalExecutionCandidate> {
        loop {
            if let Some(candidate) = self.queued_candidates.pop_front() {
                return Some(candidate);
            }

            let page_candidates = self.next_page_candidates().await?;
            let (mut page_scheduled, mut page_skipped) = schedule_pool_page_candidates(
                self.state,
                page_candidates,
                self.sticky_session_token.as_deref(),
            )
            .await;
            for skipped_candidate in &page_skipped {
                self.record_skip_reason(skipped_candidate.skip_reason);
            }
            for candidate in &mut page_scheduled {
                candidate.orchestration.pool_key_index = Some(self.next_pool_key_index);
                self.next_pool_key_index = self.next_pool_key_index.saturating_add(1);
            }
            self.queued_candidates.extend(page_scheduled);
            self.skipped_candidates.append(&mut page_skipped);
        }
    }

    pub(crate) fn take_skipped_candidates(&mut self) -> Vec<SkippedLocalExecutionCandidate> {
        std::mem::take(&mut self.skipped_candidates)
    }

    pub(crate) fn peek_queued_key_id(&self) -> Option<&str> {
        self.queued_candidates
            .front()
            .map(|candidate| candidate.candidate.key_id.as_str())
    }

    pub(crate) async fn promote_preheated_candidates_after_fast_fail(
        &mut self,
        provider_id: &str,
        pool_group_id: &str,
        attempted_key_ids: &BTreeSet<String>,
    ) -> Vec<EligibleLocalExecutionCandidate> {
        let provider_id = provider_id.trim();
        let pool_group_id = pool_group_id.trim();
        if provider_id.is_empty()
            || pool_group_id.is_empty()
            || self.group.candidate.provider_id != provider_id
            || !pool_preheat_enabled_for_candidate(&self.group)
        {
            return Vec::new();
        }

        let candidate_key_ids = self
            .queued_candidates
            .iter()
            .filter(|candidate| {
                candidate.orchestration.candidate_group_id.as_deref() == Some(pool_group_id)
            })
            .filter_map(|candidate| {
                let key_id = candidate.candidate.key_id.trim();
                if key_id.is_empty() || attempted_key_ids.contains(key_id) {
                    None
                } else {
                    Some(key_id.to_string())
                }
            })
            .collect::<Vec<_>>();
        if candidate_key_ids.is_empty() {
            return Vec::new();
        }

        let mut healthy_key_ids = BTreeSet::new();
        for key_id in candidate_key_ids {
            let stamp_key = preheat_last_stamp_key(provider_id, key_id.as_str());
            match self.state.app().runtime_state.kv_get(&stamp_key).await {
                Ok(Some(stamp)) if !stamp.trim().is_empty() => {
                    healthy_key_ids.insert(key_id);
                }
                Ok(_) => {}
                Err(error) => {
                    debug!(
                        event_name = "hedge_swap_runtime_state_read_failed",
                        log_type = "event",
                        provider_id,
                        pool_group_id,
                        error = ?error,
                        "gateway hedge swap failed open after preheat stamp read error"
                    );
                    return Vec::new();
                }
            }
        }
        if healthy_key_ids.is_empty() {
            return Vec::new();
        }

        let mut promoted_candidates = Vec::new();
        let mut remaining_candidates = VecDeque::new();
        let mut promoted_key_ids = BTreeSet::new();
        while let Some(candidate) = self.queued_candidates.pop_front() {
            let same_pool_group =
                candidate.orchestration.candidate_group_id.as_deref() == Some(pool_group_id);
            let key_id = candidate.candidate.key_id.clone();
            if same_pool_group
                && healthy_key_ids.contains(key_id.as_str())
                && promoted_key_ids.insert(key_id)
            {
                promoted_candidates.push(candidate);
            } else {
                remaining_candidates.push_back(candidate);
            }
        }
        self.queued_candidates = remaining_candidates;
        promoted_candidates
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
            skip_reason_counts = ?self.skip_reason_counts,
            "gateway pool scheduler exhausted pool group without a schedulable key"
        );
    }

    async fn next_page_candidates(&mut self) -> Option<Vec<EligibleLocalExecutionCandidate>> {
        if self.scanned_keys >= self.max_scanned_keys {
            return None;
        }

        let limit = self
            .page_size
            .min(self.max_scanned_keys - self.scanned_keys);
        let query = StoredPoolKeyCandidateRowsQuery {
            api_format: self.group.candidate.endpoint_api_format.clone(),
            provider_id: self.group.candidate.provider_id.clone(),
            endpoint_id: self.group.candidate.endpoint_id.clone(),
            model_id: self.group.candidate.model_id.clone(),
            selected_provider_model_name: self.group.candidate.selected_provider_model_name.clone(),
            offset: self.next_offset,
            limit,
        };
        let page = if query.offset == 0 {
            self.first_page_candidate_rows(&query).await?
        } else {
            self.query_candidate_rows_page(&query).await?
        };
        if page.rows.is_empty() {
            return None;
        }

        self.scanned_keys += page.rows.len() as u32;
        self.next_offset = page.next_offset;
        Some(self.build_page_eligible_candidates(page.rows).await)
    }

    async fn first_page_candidate_rows(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Option<PoolCandidateRowsPage> {
        let app = self.state.app();
        let cache_key = PoolCandidateCacheKey::from(query);
        let current_config_version = app.effective_pool_candidate_config_version().await;
        let cache = app.pool_candidate_cache();

        if let Some(cached) =
            cache.get_fresh(&cache_key, POOL_CANDIDATE_CACHE_TTL, current_config_version)
        {
            app.pool_preheat_metrics()
                .record_candidate_cache_operation("hit");
            return Some(PoolCandidateRowsPage {
                rows: cached.rows,
                next_offset: cached.next_db_offset,
            });
        }

        app.pool_preheat_metrics()
            .record_candidate_cache_operation("miss");
        let page = self.query_candidate_rows_page(query).await?;
        cache.insert(
            cache_key,
            CachedPoolCandidates::new(page.rows.clone(), current_config_version, page.next_offset),
            POOL_CANDIDATE_CACHE_TTL,
            POOL_CANDIDATE_CACHE_MAX_ENTRIES,
            POOL_CANDIDATE_CACHE_MAX_ROWS,
        );

        Some(page)
    }

    async fn query_candidate_rows_page(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Option<PoolCandidateRowsPage> {
        let rows = self.fetch_candidate_rows(query).await?;
        let next_offset = query.offset.saturating_add(rows.len() as u32);
        Some(PoolCandidateRowsPage { rows, next_offset })
    }

    async fn fetch_candidate_rows(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Option<Vec<StoredMinimalCandidateSelectionRow>> {
        match self
            .state
            .app()
            .list_pool_key_candidate_rows_for_group(query)
            .await
        {
            Ok(rows) => Some(rows),
            Err(err) => {
                warn!(
                    event_name = "pool_group_key_page_load_failed",
                    log_type = "event",
                    provider_id = %self.group.candidate.provider_id,
                    endpoint_id = %self.group.candidate.endpoint_id,
                    model_id = %self.group.candidate.model_id,
                    selected_provider_model_name = %self.group.candidate.selected_provider_model_name,
                    offset = query.offset,
                    limit = query.limit,
                    error = ?err,
                    "gateway pool scheduler failed to read pool key page"
                );
                None
            }
        }
    }

    async fn build_page_eligible_candidates(
        &mut self,
        rows: Vec<StoredMinimalCandidateSelectionRow>,
    ) -> Vec<EligibleLocalExecutionCandidate> {
        let mut candidates = Vec::with_capacity(rows.len());
        for row in rows {
            let candidate = pool_candidate_from_row(&self.group, row);
            let Some(transport) = read_candidate_transport_snapshot(self.state, &candidate).await
            else {
                self.record_skip_reason("transport_snapshot_missing");
                continue;
            };
            if let Some(skip_reason) =
                candidate_auth_channel_skip_reason(&transport, self.request_auth_channel.as_deref())
            {
                self.record_skip_reason(skip_reason);
                continue;
            }
            if let Some(skip_reason) = candidate_common_transport_skip_reason(
                &transport,
                pool_candidate_transport_policy_facts(&candidate),
                self.requested_model.as_deref(),
            ) {
                self.record_skip_reason(skip_reason);
                continue;
            }
            candidates.push(EligibleLocalExecutionCandidate {
                kind: LocalExecutionCandidateKind::SingleKey,
                candidate,
                provider_api_format: transport.endpoint.api_format.trim().to_ascii_lowercase(),
                transport: std::sync::Arc::new(transport),
                orchestration: LocalExecutionCandidateMetadata::default(),
                ranking: self.group.ranking.clone(),
            });
        }
        candidates
    }

    fn record_skip_reason(&mut self, reason: &'static str) {
        *self.skip_reason_counts.entry(reason).or_insert(0) += 1;
    }
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

    keys.into_iter()
        .map(|key| {
            let provider_type = provider_type_by_key_id
                .get(&key.id)
                .map(String::as_str)
                .unwrap_or_default();
            (
                key.id.clone(),
                build_pool_catalog_key_context(state, &key, provider_type),
            )
        })
        .collect()
}

fn build_pool_catalog_key_context(
    state: PlannerAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> PoolCatalogKeyContext {
    let status_snapshot = provider_key_status_snapshot_payload(key, provider_type);
    let quota_snapshot = status_snapshot
        .as_object()
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object);
    let account_snapshot = status_snapshot
        .as_object()
        .and_then(|snapshot| snapshot.get("account"))
        .and_then(Value::as_object);

    let (health_score, _, _, _, _) = provider_key_health_summary(key);
    let health_score = key
        .health_by_format
        .as_ref()
        .and_then(Value::as_object)
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

    PoolCatalogKeyContext {
        oauth_plan_type: quota_snapshot
            .and_then(|quota| quota.get("plan_type"))
            .and_then(Value::as_str)
            .and_then(|value| normalize_pool_plan_type(value, provider_type))
            .or_else(|| derive_pool_oauth_plan_type(state, key, provider_type)),
        quota_usage_ratio: quota_snapshot
            .and_then(|quota| quota.get("usage_ratio"))
            .and_then(json_f64)
            .map(|value| value.clamp(0.0, 1.0)),
        quota_reset_seconds: quota_snapshot
            .and_then(|quota| quota.get("reset_seconds"))
            .and_then(json_f64)
            .filter(|value| *value >= 0.0),
        account_blocked: account_snapshot
            .and_then(|account| account.get("blocked"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        quota_exhausted: quota_snapshot
            .and_then(|quota| quota.get("exhausted"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        health_score,
        latency_avg_ms,
        catalog_lru_score: Some(key.last_used_at_unix_secs.unwrap_or(0) as f64),
    }
}

fn derive_pool_oauth_plan_type(
    state: PlannerAppState<'_>,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<String> {
    if !provider_key_auth_semantics(key, provider_type).oauth_managed() {
        return None;
    }

    let provider_type_key = provider_type.trim().to_ascii_lowercase();
    if let Some(upstream_metadata) = key.upstream_metadata.as_ref().and_then(Value::as_object) {
        let provider_bucket = upstream_metadata
            .get(&provider_type_key)
            .and_then(Value::as_object);
        for source in provider_bucket
            .into_iter()
            .chain(std::iter::once(upstream_metadata))
        {
            if let Some(plan_type) = pool_plan_type_from_source(
                source,
                provider_type,
                &[
                    "plan_type",
                    "tier",
                    "subscription_title",
                    "subscription_plan",
                    "plan",
                ],
            ) {
                return Some(plan_type);
            }
        }
    }

    parse_catalog_auth_config_json(state.app(), key).and_then(|auth_config| {
        pool_plan_type_from_source(
            &auth_config,
            provider_type,
            &["plan_type", "tier", "plan", "subscription_plan"],
        )
    })
}

fn pool_plan_type_from_source(
    source: &Map<String, Value>,
    provider_type: &str,
    fields: &[&str],
) -> Option<String> {
    for field in fields {
        let Some(value) = source.get(*field).and_then(Value::as_str) else {
            continue;
        };
        if let Some(normalized) = normalize_pool_plan_type(value, provider_type) {
            return Some(normalized);
        }
    }
    None
}

fn normalize_pool_plan_type(value: &str, provider_type: &str) -> Option<String> {
    let mut normalized = value.trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    let provider_type = provider_type.trim().to_ascii_lowercase();
    if !provider_type.is_empty() && normalized.to_ascii_lowercase().starts_with(&provider_type) {
        normalized = normalized[provider_type.len()..]
            .trim_matches(|ch: char| [' ', ':', '-', '_'].contains(&ch))
            .to_string();
    }

    let normalized = normalized.trim().to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

fn json_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

fn apply_local_execution_pool_scheduler_with_runtime_map(
    candidates: Vec<EligibleLocalExecutionCandidate>,
    runtime_by_provider: &BTreeMap<String, AdminProviderPoolRuntimeState>,
    key_context_by_id: &BTreeMap<String, PoolCatalogKeyContext>,
) -> (
    Vec<EligibleLocalExecutionCandidate>,
    Vec<SkippedLocalExecutionCandidate>,
) {
    let runtime_by_provider = runtime_by_provider
        .iter()
        .map(|(provider_id, runtime)| (provider_id.clone(), ai_pool_runtime_state(runtime)))
        .collect::<BTreeMap<_, _>>();
    let inputs = candidates
        .into_iter()
        .map(|candidate| {
            let key_context = key_context_by_id
                .get(&candidate.candidate.key_id)
                .cloned()
                .unwrap_or_default();
            AiPoolCandidateInput {
                facts: ai_pool_candidate_facts(&candidate),
                pool_config: pool_config_for_candidate(&candidate).map(ai_pool_scheduling_config),
                key_context,
                candidate,
            }
        })
        .collect::<Vec<_>>();
    let outcome = run_ai_pool_scheduler(inputs, &runtime_by_provider, pool_sort_seed().as_str());

    let candidates = outcome
        .candidates
        .into_iter()
        .map(|scheduled| apply_ai_pool_orchestration(scheduled.candidate, scheduled.orchestration))
        .collect::<Vec<_>>();
    let skipped_candidates = outcome
        .skipped_candidates
        .into_iter()
        .map(|skipped| SkippedLocalExecutionCandidate {
            candidate: skipped.candidate.candidate,
            skip_reason: skipped.skip_reason,
            transport: Some(skipped.candidate.transport),
            ranking: skipped.candidate.ranking,
            extra_data: None,
        })
        .collect::<Vec<_>>();

    (candidates, skipped_candidates)
}

fn pool_config_for_candidate(
    candidate: &EligibleLocalExecutionCandidate,
) -> Option<AdminProviderPoolConfig> {
    admin_provider_pool_config_from_config_value(candidate.transport.provider.config.as_ref())
}

fn pool_preheat_enabled_for_candidate(candidate: &EligibleLocalExecutionCandidate) -> bool {
    pool_preheat_probe_provider_is_supported(&candidate.transport.provider.provider_type)
        && pool_config_for_candidate(candidate).is_some_and(|config| config.pre_probe.enabled)
}

fn preheat_last_stamp_key(provider_id: &str, key_id: &str) -> String {
    format!("ap:preheat:last:{provider_id}:{key_id}")
}

fn pool_sort_seed() -> String {
    let now_ms = current_unix_ms();
    let sequence = LOAD_BALANCE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    format!("{now_ms}:{sequence}")
}

fn ai_pool_candidate_facts(candidate: &EligibleLocalExecutionCandidate) -> AiPoolCandidateFacts {
    AiPoolCandidateFacts {
        provider_id: candidate.candidate.provider_id.clone(),
        endpoint_id: candidate.candidate.endpoint_id.clone(),
        model_id: candidate.candidate.model_id.clone(),
        selected_provider_model_name: candidate.candidate.selected_provider_model_name.clone(),
        provider_api_format: candidate.provider_api_format.clone(),
        provider_type: candidate.transport.provider.provider_type.clone(),
        key_id: candidate.candidate.key_id.clone(),
        key_internal_priority: candidate.candidate.key_internal_priority,
    }
}

fn ai_pool_scheduling_config(config: AdminProviderPoolConfig) -> AiPoolSchedulingConfig {
    AiPoolSchedulingConfig {
        scheduling_presets: config
            .scheduling_presets
            .into_iter()
            .map(|preset| AiPoolSchedulingPreset {
                preset: preset.preset,
                enabled: preset.enabled,
                mode: preset.mode,
            })
            .collect(),
        lru_enabled: config.lru_enabled,
        skip_exhausted_accounts: config.skip_exhausted_accounts,
        cost_limit_per_key_tokens: config.cost_limit_per_key_tokens,
    }
}

fn ai_pool_runtime_state(runtime: &AdminProviderPoolRuntimeState) -> AiPoolRuntimeState {
    AiPoolRuntimeState {
        sticky_bound_key_id: runtime.sticky_bound_key_id.clone(),
        cooldown_reason_by_key: runtime.cooldown_reason_by_key.clone(),
        cost_window_usage_by_key: runtime.cost_window_usage_by_key.clone(),
        latency_avg_ms_by_key: runtime.latency_avg_ms_by_key.clone(),
        lru_score_by_key: runtime.lru_score_by_key.clone(),
    }
}

fn apply_ai_pool_orchestration(
    mut candidate: EligibleLocalExecutionCandidate,
    orchestration: AiPoolCandidateOrchestration,
) -> EligibleLocalExecutionCandidate {
    candidate.orchestration = LocalExecutionCandidateMetadata {
        candidate_group_id: orchestration.candidate_group_id,
        pool_key_index: orchestration.pool_key_index,
    };
    candidate
}

#[cfg(test)]
mod tests {
    use super::{
        apply_local_execution_pool_scheduler_with_runtime_map, build_pool_catalog_key_context,
        PoolCatalogKeyContext, PoolKeyCursor, POOL_CANDIDATE_CACHE_TTL,
    };
    use crate::ai_serving::planner::candidate_resolution::{
        EligibleLocalExecutionCandidate, LocalExecutionCandidateKind,
    };
    use crate::ai_serving::PlannerAppState;
    use crate::cache::{CachedPoolCandidates, PoolCandidateCacheKey};
    use crate::data::GatewayDataState;
    use crate::handlers::shared::provider_pool::AdminProviderPoolRuntimeState;
    use crate::orchestration::LocalExecutionCandidateMetadata;
    use crate::AppState;
    use aether_ai_serving::{normalize_enabled_ai_pool_presets, AiPoolSchedulingPreset};
    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data::DataLayerError;
    use aether_data_contracts::repository::candidate_selection::{
        MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
        StoredPoolKeyCandidateRowsQuery, StoredRequestedModelCandidateRowsQuery,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider,
    };
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

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
    fn pool_scheduler_promotes_sticky_hit_regardless_distribution_mode() {
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
            vec!["key-a", "key-b"]
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
    fn pool_scheduler_applies_preset_hard_order_before_lru() {
        let key_a = sample_eligible_candidate(
            "provider-pool",
            "endpoint-1",
            "key-a",
            50,
            Some(json!({
                "pool_advanced": {
                    "scheduling_presets": [
                        {"preset": "priority_first", "enabled": true},
                        {"preset": "cache_affinity", "enabled": true}
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
                        {"preset": "priority_first", "enabled": true},
                        {"preset": "cache_affinity", "enabled": true}
                    ]
                }
            })),
        );

        let mut runtime_by_provider = BTreeMap::new();
        runtime_by_provider.insert(
            "provider-pool".to_string(),
            AdminProviderPoolRuntimeState {
                lru_score_by_key: BTreeMap::from([
                    ("key-a".to_string(), 5.0),
                    ("key-b".to_string(), 100.0),
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
            vec!["key-b", "key-a"]
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
                    oauth_plan_type: Some("free".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-plus".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("plus".to_string()),
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
                    oauth_plan_type: Some("plus".to_string()),
                    catalog_lru_score: Some(300.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-pro".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("pro".to_string()),
                    catalog_lru_score: Some(100.0),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-team".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("team".to_string()),
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
                    oauth_plan_type: Some("plus".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-pro".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("pro".to_string()),
                    ..PoolCatalogKeyContext::default()
                },
            ),
            (
                "key-team".to_string(),
                PoolCatalogKeyContext {
                    oauth_plan_type: Some("team".to_string()),
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
    fn normalizes_distribution_mutex_group_to_first_enabled_member() {
        let presets = normalize_enabled_ai_pool_presets(
            &[
                AiPoolSchedulingPreset {
                    preset: "lru".to_string(),
                    enabled: false,
                    mode: None,
                },
                AiPoolSchedulingPreset {
                    preset: "single_account".to_string(),
                    enabled: true,
                    mode: None,
                },
                AiPoolSchedulingPreset {
                    preset: "cache_affinity".to_string(),
                    enabled: true,
                    mode: None,
                },
                AiPoolSchedulingPreset {
                    preset: "priority_first".to_string(),
                    enabled: true,
                    mode: None,
                },
            ],
            "openai",
        );

        assert_eq!(presets, ["single_account", "priority_first"]);
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

        let context = build_pool_catalog_key_context(PlannerAppState::new(&app), &key, "codex");

        assert_eq!(context.oauth_plan_type.as_deref(), Some("team"));
        assert_eq!(context.quota_usage_ratio, Some(0.25));
        assert_eq!(context.quota_reset_seconds, Some(3600.0));
        assert_eq!(context.latency_avg_ms, Some(50.0));
        assert_eq!(context.catalog_lru_score, Some(1_711_000_123.0));
    }

    #[tokio::test]
    async fn pool_candidate_cache_miss_populates_first_page_rows() {
        let repository = Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
            pool_candidate_row("key-a", 10),
            pool_candidate_row("key-b", 20),
        ]));
        let app = app_with_candidate_repository(repository.clone());
        let mut cursor = pool_key_cursor(&app);

        let candidates = cursor
            .next_page_candidates()
            .await
            .expect("first page rows should be read");

        assert!(candidates.is_empty());
        assert_eq!(repository.pool_key_calls(), 1);
        assert_eq!(cursor.scanned_keys, 2);
        assert_eq!(cursor.next_offset, 2);
        assert_eq!(cache_operation_metric_value(&app, "miss"), 1);
        assert_eq!(cache_operation_metric_value(&app, "hit"), 0);

        let cached = app
            .pool_candidate_cache
            .get_fresh(&pool_candidate_cache_key(), POOL_CANDIDATE_CACHE_TTL, 0)
            .expect("first page rows should be cached");
        assert_eq!(cached.config_version, 0);
        assert_eq!(cached.next_db_offset, 2);
        assert_eq!(cached_key_ids(&cached), vec!["key-a", "key-b"]);
    }

    #[tokio::test]
    async fn pool_candidate_cache_hit_skips_first_page_candidate_row_query() {
        let repository = Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
            pool_candidate_row("db-key", 10),
        ]));
        let app = app_with_candidate_repository(repository.clone());
        app.pool_candidate_cache.insert(
            pool_candidate_cache_key(),
            CachedPoolCandidates::new(vec![pool_candidate_row("cached-key", 10)], 0, 1),
            POOL_CANDIDATE_CACHE_TTL,
            16,
            128,
        );
        let mut cursor = pool_key_cursor(&app);

        let candidates = cursor
            .next_page_candidates()
            .await
            .expect("cached first page rows should be read");

        assert!(candidates.is_empty());
        assert_eq!(repository.pool_key_calls(), 0);
        assert_eq!(cursor.scanned_keys, 1);
        assert_eq!(cursor.next_offset, 1);
        assert_eq!(cache_operation_metric_value(&app, "hit"), 1);
        assert_eq!(cache_operation_metric_value(&app, "miss"), 0);
    }

    #[tokio::test]
    async fn pool_candidate_cache_version_mismatch_repopulates_first_page() {
        let repository = Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
            pool_candidate_row("fresh-key", 10),
        ]));
        let app = app_with_candidate_repository(repository.clone());
        app.pool_candidate_cache.insert(
            pool_candidate_cache_key(),
            CachedPoolCandidates::new(vec![pool_candidate_row("stale-key", 10)], 0, 1),
            POOL_CANDIDATE_CACHE_TTL,
            16,
            128,
        );
        app.pool_candidate_config_version.store(1, Ordering::SeqCst);
        let mut cursor = pool_key_cursor(&app);

        let candidates = cursor
            .next_page_candidates()
            .await
            .expect("first page rows should be repopulated");

        assert!(candidates.is_empty());
        assert_eq!(repository.pool_key_calls(), 1);
        assert_eq!(cursor.scanned_keys, 1);
        assert_eq!(cursor.next_offset, 1);

        let cached = app
            .pool_candidate_cache
            .get_fresh(&pool_candidate_cache_key(), POOL_CANDIDATE_CACHE_TTL, 1)
            .expect("fresh version should be cached");
        assert_eq!(cached.config_version, 1);
        assert_eq!(cached_key_ids(&cached), vec!["fresh-key"]);
    }

    #[tokio::test]
    async fn pool_candidate_cache_local_bump_invalidates_cached_entry() {
        let repository = Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
            pool_candidate_row("fresh-key", 10),
        ]));
        let app = app_with_candidate_repository(repository.clone());
        app.pool_candidate_cache.insert(
            pool_candidate_cache_key(),
            CachedPoolCandidates::new(vec![pool_candidate_row("stale-key", 10)], 0, 1),
            POOL_CANDIDATE_CACHE_TTL,
            16,
            128,
        );

        let bumped_version = app.bump_pool_candidate_config_version().await;
        let mut cursor = pool_key_cursor(&app);

        let candidates = cursor
            .next_page_candidates()
            .await
            .expect("first page rows should be repopulated");

        assert!(candidates.is_empty());
        assert_eq!(bumped_version, 1);
        assert_eq!(repository.pool_key_calls(), 1);
        let cached = app
            .pool_candidate_cache
            .get_fresh(
                &pool_candidate_cache_key(),
                POOL_CANDIDATE_CACHE_TTL,
                bumped_version,
            )
            .expect("fresh version should be cached");
        assert_eq!(cached_key_ids(&cached), vec!["fresh-key"]);
    }

    #[tokio::test]
    async fn shared_pool_candidate_config_bump_makes_other_node_miss_cache() {
        let shared_runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let node_a_repository =
            Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
                pool_candidate_row("node-a-key", 10),
            ]));
        let node_b_repository =
            Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
                pool_candidate_row("node-b-fresh-key", 10),
            ]));
        let node_a = app_with_candidate_repository(node_a_repository)
            .with_runtime_state(shared_runtime.clone());
        let node_b = app_with_candidate_repository(node_b_repository.clone())
            .with_runtime_state(shared_runtime);
        node_b.pool_candidate_cache.insert(
            pool_candidate_cache_key(),
            CachedPoolCandidates::new(vec![pool_candidate_row("node-b-stale-key", 10)], 0, 1),
            POOL_CANDIDATE_CACHE_TTL,
            16,
            128,
        );

        assert_eq!(node_a.bump_pool_candidate_config_version().await, 1);
        let mut cursor = pool_key_cursor(&node_b);

        let candidates = cursor
            .next_page_candidates()
            .await
            .expect("node B should repopulate from shared version mismatch");

        assert!(candidates.is_empty());
        assert_eq!(node_b.pool_candidate_config_version(), 0);
        assert_eq!(node_b_repository.pool_key_calls(), 1);
        let cached = node_b
            .pool_candidate_cache
            .get_fresh(&pool_candidate_cache_key(), POOL_CANDIDATE_CACHE_TTL, 1)
            .expect("node B should cache the shared version");
        assert_eq!(cached_key_ids(&cached), vec!["node-b-fresh-key"]);
    }

    #[tokio::test]
    async fn pool_candidate_cache_does_not_cache_non_first_pages() {
        let repository = Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
            pool_candidate_row("key-a", 10),
            pool_candidate_row("key-b", 20),
        ]));
        let app = app_with_candidate_repository(repository.clone());
        app.pool_candidate_cache.insert(
            pool_candidate_cache_key(),
            CachedPoolCandidates::new(vec![pool_candidate_row("cached-first", 10)], 0, 1),
            POOL_CANDIDATE_CACHE_TTL,
            16,
            128,
        );
        let mut cursor = pool_key_cursor(&app);
        cursor.next_offset = 1;
        cursor.scanned_keys = 1;

        let candidates = cursor
            .next_page_candidates()
            .await
            .expect("second page rows should be read from repository");

        assert!(candidates.is_empty());
        assert_eq!(repository.pool_key_calls(), 1);
        assert_eq!(cursor.scanned_keys, 2);
        assert_eq!(cursor.next_offset, 2);
        assert_eq!(cache_operation_metric_value(&app, "hit"), 0);
        assert_eq!(cache_operation_metric_value(&app, "miss"), 0);

        let cached = app
            .pool_candidate_cache
            .get_fresh(&pool_candidate_cache_key(), POOL_CANDIDATE_CACHE_TTL, 0)
            .expect("first page cache entry should remain unchanged");
        assert_eq!(cached_key_ids(&cached), vec!["cached-first"]);
    }

    mod oauth_pool_preheat_integration {
        use super::*;

        #[tokio::test]
        async fn q1_cache_hit_no_sql_on_second_request() {
            let repository = Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
                pool_candidate_row("key-a", 10),
                pool_candidate_row("key-b", 20),
            ]));
            let app = app_with_candidate_repository(repository.clone());

            let mut first_cursor = pool_key_cursor(&app);
            first_cursor
                .next_page_candidates()
                .await
                .expect("first request should populate cache");
            assert_eq!(repository.pool_key_calls(), 1);
            assert_eq!(cache_operation_metric_value(&app, "miss"), 1);

            let mut second_cursor = pool_key_cursor(&app);
            second_cursor
                .next_page_candidates()
                .await
                .expect("second request should reuse cache");

            assert_eq!(repository.pool_key_calls(), 1);
            assert_eq!(second_cursor.scanned_keys, 2);
            assert_eq!(cache_operation_metric_value(&app, "hit"), 1);
            let cached = app
                .pool_candidate_cache
                .get_fresh(&pool_candidate_cache_key(), POOL_CANDIDATE_CACHE_TTL, 0)
                .expect("cache entry should remain fresh");
            assert_eq!(cached_key_ids(&cached), vec!["key-a", "key-b"]);
        }

        #[tokio::test]
        async fn q2_admin_mutation_invalidates_cache() {
            let repository = Arc::new(CountingMinimalCandidateSelectionReadRepository::seed(vec![
                pool_candidate_row("fresh-key", 10),
            ]));
            let app = app_with_candidate_repository(repository.clone());
            app.pool_candidate_cache.insert(
                pool_candidate_cache_key(),
                CachedPoolCandidates::new(vec![pool_candidate_row("stale-key", 10)], 0, 1),
                POOL_CANDIDATE_CACHE_TTL,
                16,
                128,
            );

            let bumped_version = app.bump_pool_candidate_config_version().await;
            let mut cursor = pool_key_cursor(&app);
            cursor
                .next_page_candidates()
                .await
                .expect("admin mutation should force a fresh first-page query");

            assert_eq!(bumped_version, 1);
            assert_eq!(repository.pool_key_calls(), 1);
            assert_eq!(cache_operation_metric_value(&app, "invalidate"), 1);
            assert_eq!(cache_operation_metric_value(&app, "miss"), 1);
            let cached = app
                .pool_candidate_cache
                .get_fresh(
                    &pool_candidate_cache_key(),
                    POOL_CANDIDATE_CACHE_TTL,
                    bumped_version,
                )
                .expect("fresh cache entry should use bumped version");
            assert_eq!(cached_key_ids(&cached), vec!["fresh-key"]);
        }
    }

    #[tokio::test]
    async fn hedge_swap_promotes_preheated_queued_pool_candidate() {
        let app = AppState::new().expect("state should build");
        let pool_group_id = pool_group_id();
        let mut cursor = pool_key_cursor_with_preheat(&app, true);
        cursor.queued_candidates = VecDeque::from([
            queued_pool_candidate("key-b", 1, true),
            queued_pool_candidate("key-c", 2, true),
            queued_pool_candidate("key-d", 3, true),
        ]);
        app.runtime_state
            .kv_set(
                "ap:preheat:last:provider-pool:key-b",
                "123",
                Some(Duration::from_secs(60)),
            )
            .await
            .expect("stamp write should succeed");
        app.runtime_state
            .kv_set(
                "ap:preheat:last:provider-pool:key-c",
                "123",
                Some(Duration::from_secs(60)),
            )
            .await
            .expect("stamp write should succeed");
        let attempted_key_ids = BTreeSet::from(["key-a".to_string(), "key-b".to_string()]);

        let promoted = cursor
            .promote_preheated_candidates_after_fast_fail(
                "provider-pool",
                pool_group_id.as_str(),
                &attempted_key_ids,
            )
            .await;

        assert_eq!(key_ids(&promoted), vec!["key-c"]);
        assert_eq!(queued_key_ids(&cursor), vec!["key-b", "key-d"]);
    }

    #[tokio::test]
    async fn hedge_swap_no_preheated_candidate_preserves_queue_order() {
        let app = AppState::new().expect("state should build");
        let pool_group_id = pool_group_id();
        let mut cursor = pool_key_cursor_with_preheat(&app, true);
        cursor.queued_candidates = VecDeque::from([
            queued_pool_candidate("key-b", 1, true),
            queued_pool_candidate("key-c", 2, true),
        ]);

        let promoted = cursor
            .promote_preheated_candidates_after_fast_fail(
                "provider-pool",
                pool_group_id.as_str(),
                &BTreeSet::from(["key-a".to_string()]),
            )
            .await;

        assert!(promoted.is_empty());
        assert_eq!(queued_key_ids(&cursor), vec!["key-b", "key-c"]);
    }

    #[tokio::test]
    async fn hedge_swap_switch_off_preserves_queue_order() {
        let app = AppState::new().expect("state should build");
        let pool_group_id = pool_group_id();
        let mut cursor = pool_key_cursor_with_preheat(&app, false);
        cursor.queued_candidates = VecDeque::from([
            queued_pool_candidate("key-b", 1, false),
            queued_pool_candidate("key-c", 2, false),
        ]);
        app.runtime_state
            .kv_set(
                "ap:preheat:last:provider-pool:key-c",
                "123",
                Some(Duration::from_secs(60)),
            )
            .await
            .expect("stamp write should succeed");

        let promoted = cursor
            .promote_preheated_candidates_after_fast_fail(
                "provider-pool",
                pool_group_id.as_str(),
                &BTreeSet::from(["key-a".to_string()]),
            )
            .await;

        assert!(promoted.is_empty());
        assert_eq!(queued_key_ids(&cursor), vec!["key-b", "key-c"]);
    }

    #[tokio::test]
    async fn hedge_swap_unsupported_provider_with_preheat_stamp_preserves_queue_order() {
        let app = AppState::new().expect("state should build");
        let pool_group_id = pool_group_id();
        let mut cursor = pool_key_cursor_with_preheat_and_provider_type(&app, true, "openai");
        cursor.queued_candidates = VecDeque::from([
            queued_pool_candidate_with_provider_type("key-b", 1, true, "openai"),
            queued_pool_candidate_with_provider_type("key-c", 2, true, "openai"),
        ]);
        app.runtime_state
            .kv_set(
                "ap:preheat:last:provider-pool:key-c",
                "123",
                Some(Duration::from_secs(60)),
            )
            .await
            .expect("stamp write should succeed");

        let promoted = cursor
            .promote_preheated_candidates_after_fast_fail(
                "provider-pool",
                pool_group_id.as_str(),
                &BTreeSet::from(["key-a".to_string()]),
            )
            .await;

        assert!(promoted.is_empty());
        assert_eq!(queued_key_ids(&cursor), vec!["key-b", "key-c"]);
    }

    fn sample_eligible_candidate(
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        internal_priority: i32,
        provider_config: Option<serde_json::Value>,
    ) -> EligibleLocalExecutionCandidate {
        sample_eligible_candidate_with_provider_type(
            provider_id,
            endpoint_id,
            key_id,
            internal_priority,
            "codex",
            provider_config,
        )
    }

    fn sample_eligible_candidate_with_provider_type(
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        internal_priority: i32,
        provider_type: &str,
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
                provider_type: provider_type.to_string(),
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
                    provider_type: provider_type.to_string(),
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
                    decrypted_api_key: "secret".to_string(),
                    decrypted_auth_config: None,
                },
            }),
        }
    }

    fn app_with_candidate_repository(
        repository: Arc<CountingMinimalCandidateSelectionReadRepository>,
    ) -> AppState {
        AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_minimal_candidate_selection_reader_for_tests(repository),
            )
    }

    fn pool_key_cursor<'a>(app: &'a AppState) -> PoolKeyCursor<'a> {
        PoolKeyCursor::new(
            PlannerAppState::new(app),
            sample_eligible_candidate(
                "provider-pool",
                "endpoint-1",
                "pool-group",
                10,
                Some(json!({ "pool_advanced": {} })),
            ),
            None,
            None,
            None,
        )
    }

    fn pool_key_cursor_with_preheat(app: &AppState, pre_probe_enabled: bool) -> PoolKeyCursor<'_> {
        pool_key_cursor_with_preheat_and_provider_type(app, pre_probe_enabled, "codex")
    }

    fn pool_key_cursor_with_preheat_and_provider_type<'a>(
        app: &'a AppState,
        pre_probe_enabled: bool,
        provider_type: &str,
    ) -> PoolKeyCursor<'a> {
        PoolKeyCursor::new(
            PlannerAppState::new(app),
            sample_eligible_candidate_with_provider_type(
                "provider-pool",
                "endpoint-1",
                "pool-group",
                10,
                provider_type,
                Some(pool_preheat_config(pre_probe_enabled)),
            ),
            None,
            None,
            None,
        )
    }

    fn queued_pool_candidate(
        key_id: &str,
        pool_key_index: u32,
        pre_probe_enabled: bool,
    ) -> EligibleLocalExecutionCandidate {
        queued_pool_candidate_with_provider_type(key_id, pool_key_index, pre_probe_enabled, "codex")
    }

    fn queued_pool_candidate_with_provider_type(
        key_id: &str,
        pool_key_index: u32,
        pre_probe_enabled: bool,
        provider_type: &str,
    ) -> EligibleLocalExecutionCandidate {
        let mut candidate = sample_eligible_candidate_with_provider_type(
            "provider-pool",
            "endpoint-1",
            key_id,
            10,
            provider_type,
            Some(pool_preheat_config(pre_probe_enabled)),
        );
        candidate.kind = LocalExecutionCandidateKind::SingleKey;
        candidate.orchestration = LocalExecutionCandidateMetadata {
            candidate_group_id: Some(pool_group_id()),
            pool_key_index: Some(pool_key_index),
        };
        candidate
    }

    fn pool_preheat_config(pre_probe_enabled: bool) -> serde_json::Value {
        json!({
            "pool_advanced": {
                "pre_probe": {
                    "enabled": pre_probe_enabled,
                    "top_n": 2,
                    "required_healthy": 1,
                    "cache_ttl_seconds": 300
                }
            }
        })
    }

    fn pool_group_id() -> String {
        "provider=provider-pool|endpoint=endpoint-1|model=model-1|selected_model=gpt-5|api_format=openai:chat|singleton_key=*".to_string()
    }

    fn key_ids(candidates: &[EligibleLocalExecutionCandidate]) -> Vec<&str> {
        candidates
            .iter()
            .map(|candidate| candidate.candidate.key_id.as_str())
            .collect()
    }

    fn queued_key_ids<'a>(cursor: &'a PoolKeyCursor<'_>) -> Vec<&'a str> {
        cursor
            .queued_candidates
            .iter()
            .map(|candidate| candidate.candidate.key_id.as_str())
            .collect()
    }

    fn pool_candidate_cache_key() -> PoolCandidateCacheKey {
        PoolCandidateCacheKey::new(
            "provider-pool",
            "endpoint-1",
            "model-1",
            "gpt-5",
            "openai:chat",
        )
    }

    fn cached_key_ids(cached: &CachedPoolCandidates) -> Vec<&str> {
        cached.rows.iter().map(|row| row.key_id.as_str()).collect()
    }

    fn cache_operation_metric_value(app: &AppState, operation: &str) -> u64 {
        app.pool_preheat_metrics()
            .metric_samples()
            .into_iter()
            .find(|sample| {
                sample.name == "aether_pool_candidate_cache_operations_total"
                    && sample
                        .labels
                        .iter()
                        .any(|label| label.key == "operation" && label.value == operation)
            })
            .map(|sample| sample.value)
            .unwrap_or(0)
    }

    fn pool_candidate_row(
        key_id: &str,
        internal_priority: i32,
    ) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-pool".to_string(),
            provider_name: "provider-pool".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: internal_priority,
            key_global_priority_by_format: Some(json!({ "openai:chat": internal_priority })),
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

    struct CountingMinimalCandidateSelectionReadRepository {
        inner: InMemoryMinimalCandidateSelectionReadRepository,
        pool_key_calls: AtomicUsize,
    }

    impl CountingMinimalCandidateSelectionReadRepository {
        fn seed(rows: Vec<StoredMinimalCandidateSelectionRow>) -> Self {
            Self {
                inner: InMemoryMinimalCandidateSelectionReadRepository::seed(rows),
                pool_key_calls: AtomicUsize::new(0),
            }
        }

        fn pool_key_calls(&self) -> usize {
            self.pool_key_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl MinimalCandidateSelectionReadRepository for CountingMinimalCandidateSelectionReadRepository {
        async fn list_for_exact_api_format(
            &self,
            api_format: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.inner.list_for_exact_api_format(api_format).await
        }

        async fn list_for_exact_api_format_and_global_model(
            &self,
            api_format: &str,
            global_model_name: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.inner
                .list_for_exact_api_format_and_global_model(api_format, global_model_name)
                .await
        }

        async fn list_for_exact_api_format_and_requested_model(
            &self,
            api_format: &str,
            requested_model_name: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.inner
                .list_for_exact_api_format_and_requested_model(api_format, requested_model_name)
                .await
        }

        async fn list_for_exact_api_format_and_requested_model_page(
            &self,
            query: &StoredRequestedModelCandidateRowsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.inner
                .list_for_exact_api_format_and_requested_model_page(query)
                .await
        }

        async fn list_pool_key_rows_for_group(
            &self,
            query: &StoredPoolKeyCandidateRowsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.pool_key_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.list_pool_key_rows_for_group(query).await
        }
    }
}
