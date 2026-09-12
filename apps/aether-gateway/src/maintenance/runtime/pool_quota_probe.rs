use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::pool_scores::{
    ListPoolMemberProbeCandidatesQuery, ListPoolMemberScoresQuery, PoolMemberHardState,
    PoolMemberIdentity, PoolMemberProbeAttempt, PoolMemberProbeResult, PoolMemberProbeStatus,
    StoredPoolMemberScore, POOL_KIND_PROVIDER_KEY_POOL,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_pool::provider_pool_quota_metadata_updated_at;
use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use futures_util::{stream, StreamExt};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::admin_api::{
    admin_provider_pool_config, provider_quota_refresh_endpoint_for_provider,
    provider_type_supports_quota_refresh, reconcile_admin_fixed_provider_template_endpoints,
    refresh_provider_pool_quota_locally, AdminAppState,
};
use crate::{AppState, GatewayError};

use crate::ai_serving::provider_key_pool_score_scope;
use crate::handlers::shared::provider_pool::{
    admin_provider_pool_quota_probe_active_members_key, AdminProviderPoolConfig,
};
use crate::provider_pool_demand::{
    provider_pool_burst_pending_key, read_provider_pool_demand_snapshot,
    sample_provider_pool_demand, ProviderPoolDemandSnapshot,
};

use super::pool_score_rebuild::ensure_provider_key_pool_scores_for_keys;

const POOL_QUOTA_PROBE_REDIS_PREFIX: &str = "ap:quota_probe:last";
const POOL_QUOTA_PROBE_DEFAULT_SCAN_INTERVAL_SECONDS: u64 = 60;
const POOL_QUOTA_PROBE_MIN_SCAN_INTERVAL_SECONDS: u64 = 15;
const POOL_QUOTA_PROBE_UNLIMITED_MAX_KEYS_PER_PROVIDER: usize = usize::MAX;
const POOL_QUOTA_PROBE_PROVIDER_SCORE_READ_LIMIT: usize = 100_000;
const POOL_QUOTA_PROBE_DEFAULT_GLOBAL_CONCURRENCY: usize = 16;
const POOL_QUOTA_PROBE_PROVIDER_LOCK_TTL_MS: u64 = 30_000;
const POOL_QUOTA_PROBE_BURST_TRIGGER_LOCK_TTL_MS: u64 = 30_000;
const POOL_QUOTA_PROBE_BURST_PENDING_PREFIX: &str = "ap:quota_probe:burst_pending";
const POOL_QUOTA_PROBE_BURST_PENDING_TTL_SECONDS: u64 = 30;
const POOL_QUOTA_PROBE_BURST_RETRY_GUARD_SECONDS: u64 = 15;
const POOL_QUOTA_PROBE_AUTO_MIN_INTERVAL_SECONDS: u64 = 30;
const POOL_QUOTA_PROBE_AUTO_MAX_INTERVAL_SECONDS: u64 = 10 * 60;
const POOL_QUOTA_PROBE_AUTO_MAX_PRESSURE: u64 = 64;
const POOL_QUOTA_PROBE_LOCAL_MAX_PROVIDERS: usize = 1024;

#[derive(Debug)]
pub(crate) struct PoolQuotaProbeReplenishCoordinator {
    capacity: usize,
    state: Mutex<PoolQuotaProbeReplenishState>,
}

#[derive(Debug, Default)]
struct PoolQuotaProbeReplenishState {
    providers: HashMap<String, PoolQuotaProbeReplenishEntry>,
    started_total: u64,
    coalesced_total: u64,
    capacity_rejected_total: u64,
}

#[derive(Debug)]
struct PoolQuotaProbeReplenishEntry {
    identity: Arc<()>,
    pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolQuotaProbeReplenishSnapshot {
    pub(crate) capacity: usize,
    pub(crate) active: usize,
    pub(crate) started_total: u64,
    pub(crate) coalesced_total: u64,
    pub(crate) capacity_rejected_total: u64,
}

impl Default for PoolQuotaProbeReplenishCoordinator {
    fn default() -> Self {
        Self::new(POOL_QUOTA_PROBE_LOCAL_MAX_PROVIDERS)
    }
}

impl PoolQuotaProbeReplenishCoordinator {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(PoolQuotaProbeReplenishState::default()),
        }
    }

    pub(crate) fn snapshot(&self) -> PoolQuotaProbeReplenishSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        PoolQuotaProbeReplenishSnapshot {
            capacity: self.capacity,
            active: state.providers.len(),
            started_total: state.started_total,
            coalesced_total: state.coalesced_total,
            capacity_rejected_total: state.capacity_rejected_total,
        }
    }

    fn request(self: &Arc<Self>, provider_id: String) -> Option<PoolQuotaProbeReplenishGuard> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = state.providers.get_mut(&provider_id) {
            entry.pending = true;
            state.coalesced_total = state.coalesced_total.saturating_add(1);
            return None;
        }
        if state.providers.len() >= self.capacity {
            // Replenishment is best effort; the periodic base scan remains available.
            state.capacity_rejected_total = state.capacity_rejected_total.saturating_add(1);
            return None;
        }
        let identity = Arc::new(());
        state.providers.insert(
            provider_id.clone(),
            PoolQuotaProbeReplenishEntry {
                identity: Arc::clone(&identity),
                pending: true,
            },
        );
        state.started_total = state.started_total.saturating_add(1);
        Some(PoolQuotaProbeReplenishGuard {
            coordinator: Arc::clone(self),
            provider_id,
            identity,
            finished: false,
        })
    }

    fn spawn<F, Fut>(
        self: &Arc<Self>,
        provider_id: String,
        mut replenish: F,
    ) -> Option<tokio::task::JoinHandle<()>>
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        // Own the guard before spawn so cancellation before the first poll also cleans up.
        let mut guard = self.request(provider_id)?;
        Some(tokio::spawn(async move {
            while guard.next_pass() {
                replenish().await;
            }
        }))
    }
}

struct PoolQuotaProbeReplenishGuard {
    coordinator: Arc<PoolQuotaProbeReplenishCoordinator>,
    provider_id: String,
    identity: Arc<()>,
    finished: bool,
}

impl PoolQuotaProbeReplenishGuard {
    fn next_pass(&mut self) -> bool {
        let mut state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = state
            .providers
            .get_mut(&self.provider_id)
            .filter(|entry| Arc::ptr_eq(&entry.identity, &self.identity))
        {
            if entry.pending {
                entry.pending = false;
                return true;
            }
            // Check for a follow-up and release ownership in one critical section.
            state.providers.remove(&self.provider_id);
        }
        self.finished = true;
        false
    }
}

impl Drop for PoolQuotaProbeReplenishGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state
            .providers
            .get(&self.provider_id)
            .is_some_and(|entry| Arc::ptr_eq(&entry.identity, &self.identity))
        {
            state.providers.remove(&self.provider_id);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolQuotaProbeMode {
    Base,
    Burst,
}

impl PoolQuotaProbeMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Burst => "burst",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolQuotaProbeRunSummary {
    pub(crate) providers_checked: usize,
    pub(crate) providers_probed: usize,
    pub(crate) providers_skipped: usize,
    pub(crate) providers_busy: usize,
    pub(crate) selected_keys: usize,
    pub(crate) succeeded: usize,
    pub(crate) failed: usize,
    pub(crate) auto_removed: usize,
}

impl PoolQuotaProbeRunSummary {
    const fn empty() -> Self {
        Self {
            providers_checked: 0,
            providers_probed: 0,
            providers_skipped: 0,
            providers_busy: 0,
            selected_keys: 0,
            succeeded: 0,
            failed: 0,
            auto_removed: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PoolQuotaProbeWorkerConfig {
    pub(crate) scan_interval: Duration,
    pub(crate) max_keys_per_provider: usize,
    pub(crate) global_concurrency: usize,
}

impl PoolQuotaProbeWorkerConfig {
    pub(crate) fn from_env() -> Self {
        let scan_interval_seconds = env_u64(
            "POOL_QUOTA_PROBE_SCAN_INTERVAL_SECONDS",
            POOL_QUOTA_PROBE_DEFAULT_SCAN_INTERVAL_SECONDS,
        )
        .max(POOL_QUOTA_PROBE_MIN_SCAN_INTERVAL_SECONDS);
        let max_keys_per_provider = env_usize(
            "POOL_QUOTA_PROBE_MAX_KEYS_PER_PROVIDER",
            POOL_QUOTA_PROBE_UNLIMITED_MAX_KEYS_PER_PROVIDER,
        );
        let global_concurrency = env_usize(
            "POOL_QUOTA_PROBE_GLOBAL_CONCURRENCY",
            POOL_QUOTA_PROBE_DEFAULT_GLOBAL_CONCURRENCY,
        )
        .clamp(1, 256);
        Self {
            scan_interval: Duration::from_secs(scan_interval_seconds),
            max_keys_per_provider,
            global_concurrency,
        }
    }
}

fn env_u64(name: &str, default_value: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default_value)
}

fn env_usize(name: &str, default_value: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default_value)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn provider_supports_quota_probe(provider_type: &str) -> bool {
    provider_type_supports_quota_refresh(provider_type)
}

fn parse_probe_stamp(raw_value: Option<&str>) -> Option<u64> {
    let parsed = raw_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<f64>().ok())?;
    if parsed <= 0.0 {
        return None;
    }
    Some(parsed as u64)
}

pub(crate) fn pool_quota_probe_target_count(
    total_active_keys: usize,
    target_percent: Option<f64>,
    target_count: Option<u64>,
) -> usize {
    if total_active_keys == 0 {
        return 0;
    }
    let by_percent = target_percent
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| ((total_active_keys as f64) * (value.clamp(0.0, 100.0) / 100.0)).ceil())
        .map(|value| value as usize)
        .unwrap_or(0);
    let by_count = target_count
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    by_percent.max(by_count).min(total_active_keys)
}

fn pool_quota_probe_burst_batch_size(
    pool_config: &AdminProviderPoolConfig,
    config: PoolQuotaProbeWorkerConfig,
) -> usize {
    if config.max_keys_per_provider == 0 {
        return 0;
    }
    let concurrency_batch = (pool_config.probe_concurrency.clamp(1, 64) as usize).saturating_mul(2);
    concurrency_batch.min(config.max_keys_per_provider)
}

fn pool_quota_probe_target_count_for_mode(
    total_active_keys: usize,
    pool_config: &AdminProviderPoolConfig,
    auto_target: usize,
    config: PoolQuotaProbeWorkerConfig,
    mode: PoolQuotaProbeMode,
) -> usize {
    if total_active_keys == 0 || auto_target == 0 {
        return 0;
    }
    match mode {
        PoolQuotaProbeMode::Base => auto_target.min(total_active_keys),
        PoolQuotaProbeMode::Burst => auto_target
            .saturating_add(pool_quota_probe_burst_batch_size(pool_config, config))
            .min(total_active_keys)
            .min(config.max_keys_per_provider),
    }
}

fn pool_quota_probe_selection_limit_for_mode(
    pool_config: &AdminProviderPoolConfig,
    config: PoolQuotaProbeWorkerConfig,
    mode: PoolQuotaProbeMode,
) -> usize {
    match mode {
        PoolQuotaProbeMode::Base => config.max_keys_per_provider,
        PoolQuotaProbeMode::Burst => pool_quota_probe_burst_batch_size(pool_config, config),
    }
}

fn pool_quota_probe_auto_interval_seconds(demand_snapshot: &ProviderPoolDemandSnapshot) -> u64 {
    let live_pressure = u64::try_from(demand_snapshot.in_flight).unwrap_or(u64::MAX);
    let ema_pressure =
        if demand_snapshot.ema_in_flight.is_finite() && demand_snapshot.ema_in_flight > 0.0 {
            demand_snapshot
                .ema_in_flight
                .ceil()
                .clamp(0.0, POOL_QUOTA_PROBE_AUTO_MAX_PRESSURE as f64) as u64
        } else {
            0
        };
    let request_pressure = live_pressure.max(ema_pressure);
    if request_pressure == 0 {
        return POOL_QUOTA_PROBE_AUTO_MAX_INTERVAL_SECONDS;
    }

    let hot_pressure = u64::try_from(demand_snapshot.desired_hot).unwrap_or(u64::MAX);
    let pressure = request_pressure
        .max(hot_pressure)
        .clamp(1, POOL_QUOTA_PROBE_AUTO_MAX_PRESSURE);
    POOL_QUOTA_PROBE_AUTO_MAX_INTERVAL_SECONDS
        .saturating_div(pressure)
        .clamp(
            POOL_QUOTA_PROBE_AUTO_MIN_INTERVAL_SECONDS,
            POOL_QUOTA_PROBE_AUTO_MAX_INTERVAL_SECONDS,
        )
}

fn active_probe_member_remains_valid(score: Option<&StoredPoolMemberScore>) -> bool {
    match score.map(|score| score.hard_state) {
        Some(PoolMemberHardState::Available | PoolMemberHardState::Unknown) | None => true,
        Some(
            PoolMemberHardState::Cooldown
            | PoolMemberHardState::QuotaExhausted
            | PoolMemberHardState::AuthInvalid
            | PoolMemberHardState::Banned
            | PoolMemberHardState::Inactive,
        ) => false,
    }
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|raw| u64::try_from(raw).ok()))
        .or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse::<u64>().ok())
        })
}

fn score_last_self_check_success_at(score: &StoredPoolMemberScore) -> Option<u64> {
    if let Some(last_self_check) = score.score_reason.get("last_self_check") {
        let status = last_self_check
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if status == "success" {
            return last_self_check
                .get("attempted_at")
                .and_then(json_u64)
                .or(score.last_probe_success_at);
        }
    }

    let last_probe = score.score_reason.get("last_probe")?;
    let source = last_probe
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let status = last_probe
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    (source == "account_self_check" && status == "success")
        .then_some(score.last_probe_success_at)
        .flatten()
}

pub(crate) fn select_pool_quota_probe_key_ids(
    keys: &[StoredProviderCatalogKey],
    provider_type: &str,
    now_ts: u64,
    interval_seconds: u64,
    last_probe_timestamps: &BTreeMap<String, u64>,
    limit: usize,
) -> Vec<String> {
    let mut stale = Vec::<(u64, String)>::new();
    for key in keys {
        if key.id.trim().is_empty() {
            continue;
        }
        let quota_updated_ts =
            provider_pool_quota_metadata_updated_at(key.upstream_metadata.as_ref(), provider_type);
        let last_probe_ts = last_probe_timestamps.get(&key.id).copied();
        let anchor_ts = quota_updated_ts
            .unwrap_or(0)
            .max(last_probe_ts.unwrap_or(0));
        if anchor_ts == 0 || now_ts.saturating_sub(anchor_ts) >= interval_seconds {
            stale.push((anchor_ts, key.id.clone()));
        }
    }

    stale.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    if limit > 0 && stale.len() > limit {
        stale.truncate(limit);
    }
    stale.into_iter().map(|(_, key_id)| key_id).collect()
}

async fn select_score_probe_key_ids(
    state: &AppState,
    provider_id: &str,
    now_ts: u64,
    interval_seconds: u64,
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let stale_before_unix_secs = now_ts.saturating_sub(interval_seconds);
    let query = ListPoolMemberProbeCandidatesQuery {
        pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
        pool_id: provider_id.to_string(),
        capability: None,
        stale_before_unix_secs,
        limit: limit.saturating_mul(4).max(limit),
    };
    let scores = match state.data.list_pool_member_probe_candidates(&query).await {
        Ok(scores) => scores,
        Err(err) => {
            debug!(
                provider_id,
                error = ?err,
                "gateway pool quota probe: failed to read score probe candidates"
            );
            return Vec::new();
        }
    };
    let mut selected = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for score in scores {
        if !seen.insert(score.member_id.clone()) {
            continue;
        }
        selected.push(score.member_id);
        if selected.len() >= limit {
            break;
        }
    }
    selected
}

async fn load_provider_key_account_scores(
    state: &AppState,
    provider_id: &str,
    key_ids: &[String],
) -> BTreeMap<String, StoredPoolMemberScore> {
    if key_ids.is_empty() || !state.data.has_pool_score_reader() {
        return BTreeMap::new();
    }
    let scope = provider_key_pool_score_scope();
    let key_ids = key_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    match state
        .data
        .list_pool_member_scores(&ListPoolMemberScoresQuery {
            pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
            pool_id: provider_id.to_string(),
            capability: Some(scope.capability),
            scope_kind: Some(scope.scope_kind),
            scope_id: scope.scope_id,
            hard_states: Vec::new(),
            probe_statuses: None,
            offset: 0,
            limit: POOL_QUOTA_PROBE_PROVIDER_SCORE_READ_LIMIT,
        })
        .await
    {
        Ok(scores) => scores
            .into_iter()
            .filter(|score| key_ids.contains(score.member_id.as_str()))
            .map(|score| (score.member_id.clone(), score))
            .collect(),
        Err(err) => {
            debug!(
                provider_id,
                error = ?err,
                "gateway pool quota probe: failed to read provider key account scores"
            );
            BTreeMap::new()
        }
    }
}

pub(crate) fn select_pool_quota_probe_ids_for_active_target(
    key_ids: &[String],
    active_member_ids: &BTreeSet<String>,
    scores_by_key: &BTreeMap<String, StoredPoolMemberScore>,
    prefer_latest_self_check: bool,
    target_active_count: usize,
    limit: usize,
) -> Vec<String> {
    if limit == 0 || target_active_count == 0 || key_ids.is_empty() {
        return Vec::new();
    }

    let active_count = active_member_ids.len();
    let deficit = target_active_count.saturating_sub(active_count);
    if deficit == 0 {
        return Vec::new();
    }

    let mut candidates = key_ids
        .iter()
        .filter_map(|key_id| {
            if active_member_ids.contains(key_id.as_str()) {
                return None;
            }
            let score = scores_by_key.get(key_id.as_str());
            let self_check_success_at = if prefer_latest_self_check {
                score
                    .filter(|score| {
                        matches!(
                            score.hard_state,
                            PoolMemberHardState::Available | PoolMemberHardState::Unknown
                        )
                    })
                    .and_then(score_last_self_check_success_at)
                    .unwrap_or(0)
            } else {
                0
            };
            let self_check_priority =
                u8::from(prefer_latest_self_check && self_check_success_at == 0);
            let priority = match score.map(|score| score.hard_state) {
                Some(PoolMemberHardState::Unknown) | None => 0u8,
                Some(PoolMemberHardState::Available) => 1,
                Some(PoolMemberHardState::Cooldown) => 2,
                Some(PoolMemberHardState::QuotaExhausted) => 3,
                Some(
                    PoolMemberHardState::AuthInvalid
                    | PoolMemberHardState::Banned
                    | PoolMemberHardState::Inactive,
                ) => {
                    return None;
                }
            };
            let last_success = score
                .and_then(|score| score.last_probe_success_at)
                .unwrap_or(0);
            let last_attempt = score
                .and_then(|score| score.last_probe_attempt_at)
                .unwrap_or(0);
            let rank_score = score.map(|score| score.score).unwrap_or(1.0);
            Some((
                self_check_priority,
                priority,
                self_check_success_at,
                rank_score,
                last_success,
                last_attempt,
                key_id.clone(),
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| right.3.total_cmp(&left.3))
            .then_with(|| left.4.cmp(&right.4))
            .then_with(|| left.5.cmp(&right.5))
            .then_with(|| left.6.cmp(&right.6))
    });
    candidates
        .into_iter()
        .take(deficit.min(limit))
        .map(|(_, _, _, _, _, _, key_id)| key_id)
        .collect()
}

fn probe_stamp_key(provider_id: &str, key_id: &str) -> String {
    format!("{POOL_QUOTA_PROBE_REDIS_PREFIX}:{provider_id}:{key_id}")
}

fn prune_pool_quota_probe_active_member_ids(
    key_ids: &[String],
    active_member_ids: &BTreeSet<String>,
    scores_by_key: &BTreeMap<String, StoredPoolMemberScore>,
) -> (BTreeSet<String>, Vec<String>) {
    if active_member_ids.is_empty() {
        return (BTreeSet::new(), Vec::new());
    }

    let known_key_ids = key_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut retained = BTreeSet::new();
    let mut removed = Vec::new();
    for key_id in active_member_ids {
        if !known_key_ids.contains(key_id.as_str())
            || !active_probe_member_remains_valid(scores_by_key.get(key_id.as_str()))
        {
            removed.push(key_id.clone());
        } else {
            retained.insert(key_id.clone());
        }
    }
    (retained, removed)
}

fn trim_pool_quota_probe_active_member_ids_to_target(
    active_member_ids: &BTreeSet<String>,
    scores_by_key: &BTreeMap<String, StoredPoolMemberScore>,
    target_active_count: usize,
) -> Vec<String> {
    if active_member_ids.len() <= target_active_count {
        return Vec::new();
    }

    let mut candidates = active_member_ids
        .iter()
        .map(|key_id| {
            let score = scores_by_key.get(key_id.as_str());
            let trim_priority = match score.map(|score| score.hard_state) {
                Some(PoolMemberHardState::Unknown) | None => 0u8,
                Some(PoolMemberHardState::Available) => 1,
                Some(PoolMemberHardState::Cooldown) => 2,
                Some(PoolMemberHardState::QuotaExhausted) => 3,
                Some(
                    PoolMemberHardState::AuthInvalid
                    | PoolMemberHardState::Banned
                    | PoolMemberHardState::Inactive,
                ) => 4,
            };
            let rank_score = score.map(|score| score.score).unwrap_or(0.0);
            let last_success = score
                .and_then(|score| score.last_probe_success_at)
                .unwrap_or(0);
            let last_attempt = score
                .and_then(|score| score.last_probe_attempt_at)
                .unwrap_or(0);
            let last_ranked = score.and_then(|score| score.last_ranked_at).unwrap_or(0);
            let last_scheduled = score.and_then(|score| score.last_scheduled_at).unwrap_or(0);
            (
                trim_priority,
                rank_score,
                last_success,
                last_ranked,
                last_scheduled,
                last_attempt,
                key_id.clone(),
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.total_cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
            .then_with(|| left.4.cmp(&right.4))
            .then_with(|| left.5.cmp(&right.5))
            .then_with(|| left.6.cmp(&right.6))
    });

    candidates
        .into_iter()
        .take(active_member_ids.len().saturating_sub(target_active_count))
        .map(|(_, _, _, _, _, _, key_id)| key_id)
        .collect()
}

async fn load_active_probe_member_ids(
    runtime: &RuntimeState,
    provider_id: &str,
) -> BTreeSet<String> {
    match runtime
        .set_members(&admin_provider_pool_quota_probe_active_members_key(
            provider_id,
        ))
        .await
    {
        Ok(values) => values
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        Err(err) => {
            debug!(
                provider_id,
                error = ?err,
                "gateway pool quota probe: failed to read active member set"
            );
            BTreeSet::new()
        }
    }
}

async fn remove_active_probe_member_ids(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &[String],
) {
    if key_ids.is_empty() {
        return;
    }
    let set_key = admin_provider_pool_quota_probe_active_members_key(provider_id);
    for key_id in key_ids {
        if let Err(err) = runtime.set_remove(&set_key, key_id).await {
            debug!(
                provider_id,
                key_id,
                error = ?err,
                "gateway pool quota probe: failed to remove active member"
            );
        }
    }
}

async fn add_active_probe_member_ids(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &BTreeSet<String>,
) {
    if key_ids.is_empty() {
        return;
    }
    let set_key = admin_provider_pool_quota_probe_active_members_key(provider_id);
    for key_id in key_ids {
        if let Err(err) = runtime.set_add(&set_key, key_id).await {
            debug!(
                provider_id,
                key_id,
                error = ?err,
                "gateway pool quota probe: failed to add active member"
            );
        }
    }
}

async fn load_pruned_active_probe_member_ids(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &[String],
    scores_by_key: &BTreeMap<String, StoredPoolMemberScore>,
) -> BTreeSet<String> {
    let active_member_ids = load_active_probe_member_ids(runtime, provider_id).await;
    let (active_member_ids, removed_ids) =
        prune_pool_quota_probe_active_member_ids(key_ids, &active_member_ids, scores_by_key);
    remove_active_probe_member_ids(runtime, provider_id, &removed_ids).await;
    active_member_ids
}

fn probe_burst_pending_key(provider_id: &str) -> String {
    provider_pool_burst_pending_key(provider_id)
}

async fn mark_probe_burst_pending(runtime: &RuntimeState, provider_id: &str) {
    if let Err(err) = runtime
        .kv_set(
            &probe_burst_pending_key(provider_id),
            "1".to_string(),
            Some(Duration::from_secs(
                POOL_QUOTA_PROBE_BURST_PENDING_TTL_SECONDS,
            )),
        )
        .await
    {
        debug!(
            provider_id,
            error = ?err,
            "gateway pool quota probe: failed to mark burst pending"
        );
    }
}

async fn acquire_pool_quota_probe_burst_trigger_lock(
    runtime: &RuntimeState,
    provider_id: &str,
) -> Option<RuntimeLockLease> {
    let owner = format!("aether-gateway-pool-probe-burst-{}", std::process::id());
    match runtime
        .lock_try_acquire(
            &format!("pool_quota_probe_burst:{provider_id}"),
            &owner,
            Duration::from_millis(POOL_QUOTA_PROBE_BURST_TRIGGER_LOCK_TTL_MS),
        )
        .await
    {
        Ok(lease) => lease,
        Err(err) => {
            debug!(
                provider_id,
                error = %err,
                "gateway pool quota probe: failed to acquire burst trigger lock"
            );
            None
        }
    }
}

async fn release_pool_quota_probe_burst_trigger_lock(
    runtime: &RuntimeState,
    lease: Option<RuntimeLockLease>,
) {
    let Some(lease) = lease else {
        return;
    };
    if let Err(err) = runtime.lock_release(&lease).await {
        debug!(
            error = %err,
            "gateway pool quota probe: failed to release burst trigger lock"
        );
    }
}

async fn load_probe_timestamps(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &[String],
) -> BTreeMap<String, u64> {
    if key_ids.is_empty() {
        return BTreeMap::new();
    }

    let runtime_keys = key_ids
        .iter()
        .map(|key_id| probe_stamp_key(provider_id, key_id))
        .collect::<Vec<_>>();
    let Ok(values) = runtime.kv_get_many(&runtime_keys).await else {
        debug!("gateway pool quota probe: failed to read runtime probe stamps");
        return BTreeMap::new();
    };

    key_ids
        .iter()
        .zip(values)
        .filter_map(|(key_id, raw)| {
            parse_probe_stamp(raw.as_deref()).map(|ts| (key_id.clone(), ts))
        })
        .collect()
}

async fn mark_probe_timestamps(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &[String],
    now_ts: u64,
    interval_seconds: u64,
) {
    if key_ids.is_empty() {
        return;
    }

    let ttl_seconds = interval_seconds.saturating_mul(2).max(120);
    let value = now_ts.to_string();
    for key_id in key_ids {
        if runtime
            .kv_set(
                &probe_stamp_key(provider_id, key_id),
                value.clone(),
                Some(Duration::from_secs(ttl_seconds)),
            )
            .await
            .is_err()
        {
            debug!("gateway pool quota probe: failed to write runtime probe stamp");
        }
    }
}

async fn acquire_provider_probe_lock(
    runtime: &RuntimeState,
    provider_id: &str,
) -> Option<RuntimeLockLease> {
    let owner = format!("aether-gateway-pool-probe-{}", std::process::id());
    match runtime
        .lock_try_acquire(
            &format!("pool_quota_probe:{provider_id}"),
            &owner,
            Duration::from_millis(POOL_QUOTA_PROBE_PROVIDER_LOCK_TTL_MS),
        )
        .await
    {
        Ok(lease) => lease,
        Err(err) => {
            debug!(
                provider_id,
                error = %err,
                "gateway pool quota probe: failed to acquire runtime provider lock"
            );
            None
        }
    }
}

async fn release_provider_probe_lock(runtime: &RuntimeState, lease: Option<RuntimeLockLease>) {
    let Some(lease) = lease else {
        return;
    };
    if let Err(err) = runtime.lock_release(&lease).await {
        debug!(
            error = %err,
            "gateway pool quota probe: failed to release runtime provider lock"
        );
    }
}

enum PoolQuotaProbeSelectionOutcome {
    Selected(Vec<StoredProviderCatalogKey>),
    Busy,
    Empty,
}

async fn select_keys_for_provider(
    state: &AppState,
    runtime: &RuntimeState,
    provider: &StoredProviderCatalogProvider,
    pool_config: &AdminProviderPoolConfig,
    config: PoolQuotaProbeWorkerConfig,
    mode: PoolQuotaProbeMode,
    now_ts: u64,
) -> Result<PoolQuotaProbeSelectionOutcome, GatewayError> {
    let lease = acquire_provider_probe_lock(runtime, &provider.id).await;
    if lease.is_none() {
        return Ok(PoolQuotaProbeSelectionOutcome::Busy);
    }

    let result = async {
        let summaries = state
            .list_provider_catalog_key_maintenance_summaries_by_provider_ids(std::slice::from_ref(
                &provider.id,
            ))
            .await?
            .into_iter()
            .filter(|summary| summary.is_active)
            .collect::<Vec<_>>();
        if summaries.is_empty() {
            return Ok(PoolQuotaProbeSelectionOutcome::Empty);
        }

        let demand_snapshot = match mode {
            PoolQuotaProbeMode::Base => {
                sample_provider_pool_demand(
                    runtime,
                    &provider.id,
                    summaries.len(),
                    config.max_keys_per_provider,
                )
                .await
            }
            PoolQuotaProbeMode::Burst => {
                read_provider_pool_demand_snapshot(
                    runtime,
                    &provider.id,
                    summaries.len(),
                    config.max_keys_per_provider,
                )
                .await
            }
        };
        let target_active_count = pool_quota_probe_target_count_for_mode(
            summaries.len(),
            pool_config,
            demand_snapshot.desired_hot,
            config,
            mode,
        );
        if target_active_count == 0 {
            return Ok(PoolQuotaProbeSelectionOutcome::Empty);
        }
        let selection_limit = pool_quota_probe_selection_limit_for_mode(pool_config, config, mode);
        if selection_limit == 0 {
            return Ok(PoolQuotaProbeSelectionOutcome::Empty);
        }

        let key_ids = summaries
            .iter()
            .map(|summary| summary.id.clone())
            .collect::<Vec<_>>();
        let scores_by_key = load_provider_key_account_scores(state, &provider.id, &key_ids).await;
        let mut active_member_ids =
            load_pruned_active_probe_member_ids(runtime, &provider.id, &key_ids, &scores_by_key)
                .await;
        if matches!(mode, PoolQuotaProbeMode::Base) {
            let trimmed_ids = trim_pool_quota_probe_active_member_ids_to_target(
                &active_member_ids,
                &scores_by_key,
                target_active_count,
            );
            if !trimmed_ids.is_empty() {
                remove_active_probe_member_ids(runtime, &provider.id, &trimmed_ids).await;
                for key_id in trimmed_ids {
                    active_member_ids.remove(&key_id);
                }
            }
        }
        let probe_stamps = load_probe_timestamps(runtime, &provider.id, &key_ids).await;
        let probe_eligible_key_ids = key_ids
            .iter()
            .filter(|key_id| {
                if active_member_ids.contains(key_id.as_str()) {
                    return false;
                }
                match mode {
                    PoolQuotaProbeMode::Base => {
                        probe_stamps
                            .get(key_id.as_str())
                            .is_none_or(|last_probe_ts| {
                                now_ts.saturating_sub(*last_probe_ts)
                                    >= pool_quota_probe_auto_interval_seconds(&demand_snapshot)
                            })
                    }
                    PoolQuotaProbeMode::Burst => {
                        probe_stamps
                            .get(key_id.as_str())
                            .is_none_or(|last_probe_ts| {
                                now_ts.saturating_sub(*last_probe_ts)
                                    >= POOL_QUOTA_PROBE_BURST_RETRY_GUARD_SECONDS
                            })
                    }
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut selected_ids = select_pool_quota_probe_ids_for_active_target(
            &probe_eligible_key_ids,
            &active_member_ids,
            &scores_by_key,
            pool_config.account_self_check_enabled,
            target_active_count,
            selection_limit,
        );
        if selected_ids.is_empty() {
            return Ok(PoolQuotaProbeSelectionOutcome::Empty);
        }

        let stamp_interval_seconds = match mode {
            PoolQuotaProbeMode::Base => pool_quota_probe_auto_interval_seconds(&demand_snapshot),
            PoolQuotaProbeMode::Burst => POOL_QUOTA_PROBE_BURST_PENDING_TTL_SECONDS,
        };
        mark_probe_timestamps(
            runtime,
            &provider.id,
            &selected_ids,
            now_ts,
            stamp_interval_seconds,
        )
        .await;

        let mut keys_by_id = state
            .list_provider_catalog_keys_by_ids(&selected_ids)
            .await?
            .into_iter()
            .map(|key| (key.id.clone(), key))
            .collect::<BTreeMap<_, _>>();
        Ok(PoolQuotaProbeSelectionOutcome::Selected(
            selected_ids
                .into_iter()
                .filter_map(|key_id| keys_by_id.remove(&key_id))
                .collect::<Vec<_>>(),
        ))
    }
    .await;

    release_provider_probe_lock(runtime, lease).await;
    result
}

fn endpoint_for_probe(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Option<StoredProviderCatalogEndpoint> {
    provider_quota_refresh_endpoint_for_provider(provider_type, endpoints, true)
}

async fn endpoint_for_probe_with_reconcile(
    state: &AppState,
    admin_state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    provider_type: &str,
    endpoints_by_provider: &mut BTreeMap<String, Vec<StoredProviderCatalogEndpoint>>,
) -> Result<Option<StoredProviderCatalogEndpoint>, GatewayError> {
    let endpoints = endpoints_by_provider
        .get(&provider.id)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if let Some(endpoint) = endpoint_for_probe(provider_type, endpoints) {
        return Ok(Some(endpoint));
    }

    if admin_state
        .fixed_provider_template(&provider.provider_type)
        .is_none()
    {
        return Ok(None);
    }

    reconcile_admin_fixed_provider_template_endpoints(admin_state, provider).await?;
    let refreshed = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let endpoint = endpoint_for_probe(provider_type, &refreshed);
    endpoints_by_provider.insert(provider.id.clone(), refreshed);
    Ok(endpoint)
}

async fn refresh_provider_probe_keys(
    admin_state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    provider_type: &str,
    keys: Vec<StoredProviderCatalogKey>,
) -> Result<Option<Value>, GatewayError> {
    refresh_provider_pool_quota_locally(admin_state, provider, endpoint, provider_type, keys, None)
        .await
}

fn pool_quota_probe_worker_error_score_reason(_error: &GatewayError) -> Value {
    serde_json::json!({
        "last_probe": {
            "source": "pool_quota_probe",
            "status": "worker_error",
            "category": "worker_error",
            "message": "Provider quota probe worker failed"
        }
    })
}

fn update_summary_from_payload(
    summary: &mut PoolQuotaProbeRunSummary,
    selected_count: usize,
    payload: Option<&Value>,
) {
    let Some(payload) = payload else {
        summary.failed += selected_count;
        return;
    };
    summary.succeeded += payload.get("success").and_then(Value::as_u64).unwrap_or(0) as usize;
    summary.failed += payload.get("failed").and_then(Value::as_u64).unwrap_or(0) as usize;
    summary.auto_removed += payload
        .get("auto_removed")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
}

async fn record_score_probe_results_from_payload(
    state: &AppState,
    provider_id: &str,
    selected_key_ids: &[String],
    payload: Option<&Value>,
    attempted_at: u64,
) -> BTreeSet<String> {
    let mut recorded = std::collections::BTreeSet::new();
    let mut succeeded = BTreeSet::new();
    if let Some(results) = payload
        .and_then(|value| value.get("results"))
        .and_then(Value::as_array)
    {
        for item in results {
            let Some(key_id) = item
                .get("key_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            recorded.insert(key_id.to_string());
            if probe_result_succeeded(item) {
                succeeded.insert(key_id.to_string());
            }
            record_score_probe_result_for_key(
                state,
                provider_id,
                key_id,
                attempted_at,
                probe_result_succeeded(item),
                probe_result_hard_state(item).or_else(|| {
                    (!probe_result_succeeded(item)).then_some(PoolMemberHardState::Cooldown)
                }),
                serde_json::json!({
                    "last_probe": {
                        "source": "pool_quota_probe",
                        "status": item.get("status").cloned().unwrap_or(Value::Null),
                        "status_code": item.get("status_code").cloned().unwrap_or(Value::Null),
                        "message": item.get("message").cloned().unwrap_or(Value::Null),
                        "auto_removed": item.get("auto_removed").cloned().unwrap_or(Value::Null)
                    }
                }),
            )
            .await;
        }
    }

    for key_id in selected_key_ids {
        if recorded.contains(key_id) {
            continue;
        }
        record_score_probe_result_for_key(
            state,
            provider_id,
            key_id,
            attempted_at,
            false,
            Some(PoolMemberHardState::Cooldown),
            serde_json::json!({
                "last_probe": {
                    "source": "pool_quota_probe",
                    "status": "missing_result"
                }
            }),
        )
        .await;
    }
    succeeded
}

async fn record_score_probe_result_for_key(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
    attempted_at: u64,
    succeeded: bool,
    hard_state: Option<PoolMemberHardState>,
    score_reason_patch: Value,
) {
    let result = PoolMemberProbeResult {
        identity: PoolMemberIdentity::provider_api_key(provider_id.to_string(), key_id.to_string()),
        scope: None,
        attempted_at,
        succeeded,
        hard_state,
        probe_status: if succeeded {
            PoolMemberProbeStatus::Ok
        } else {
            PoolMemberProbeStatus::Failed
        },
        score_reason_patch: Some(score_reason_patch),
    };
    if let Err(err) = state.data.record_pool_member_probe_result(result).await {
        debug!(
            provider_id,
            key_id,
            error = ?err,
            "gateway pool quota probe: failed to record score probe result"
        );
    }
}

async fn record_score_probe_in_progress_for_key(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
    attempted_at: u64,
) {
    let attempt = PoolMemberProbeAttempt {
        identity: PoolMemberIdentity::provider_api_key(provider_id.to_string(), key_id.to_string()),
        scope: None,
        attempted_at,
        score_reason_patch: Some(serde_json::json!({
            "last_probe": {
                "source": "pool_quota_probe",
                "status": "in_progress"
            }
        })),
    };
    if let Err(err) = state.data.mark_pool_member_probe_in_progress(attempt).await {
        debug!(
            provider_id,
            key_id,
            error = ?err,
            "gateway pool quota probe: failed to mark score probe in progress"
        );
    }
}

fn probe_result_succeeded(item: &Value) -> bool {
    item.get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status == "success")
}

fn probe_result_hard_state(item: &Value) -> Option<PoolMemberHardState> {
    if probe_result_succeeded(item) {
        return Some(PoolMemberHardState::Available);
    }
    if item
        .get("auto_removed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some(PoolMemberHardState::Banned);
    }
    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    // Providers frequently encode an exhausted account as HTTP 429 with a
    // provider-specific status/message (for example RESOURCE_EXHAUSTED or
    // `quota exceeded`) rather than the normalized `quota_exhausted` status.
    // Preserve that distinction in the score so the member stays out of the
    // scheduler until a successful quota probe observes a reset.
    let serialized_item = item.to_string().to_ascii_lowercase();
    let status_code = item.get("status_code").and_then(Value::as_u64);
    if status == "quota_exhausted"
        || (status_code == Some(429) && contains_quota_exhaustion_marker(&serialized_item))
    {
        return Some(PoolMemberHardState::QuotaExhausted);
    }
    match status.as_str() {
        "auth_invalid" | "forbidden" => Some(PoolMemberHardState::AuthInvalid),
        "workspace_deactivated" => Some(PoolMemberHardState::Banned),
        "quota_exhausted" => Some(PoolMemberHardState::QuotaExhausted),
        _ => match item.get("status_code").and_then(Value::as_u64) {
            Some(401 | 403) => Some(PoolMemberHardState::AuthInvalid),
            Some(402) => Some(PoolMemberHardState::QuotaExhausted),
            Some(429 | 500..=599) => Some(PoolMemberHardState::Cooldown),
            _ => None,
        },
    }
}

fn contains_quota_exhaustion_marker(value: &str) -> bool {
    [
        "quota exhausted",
        "quota_exhausted",
        "quota exceeded",
        "quota_exceeded",
        "insufficient_quota",
        "resource exhausted",
        "resource has been exhausted",
        "resource_exhausted",
        "usage_limit_reached",
        "limit_reached",
        "quota limit reached",
        "credits exhausted",
        "insufficient credits",
    ]
    .iter()
    .any(|marker| value.contains(marker))
}

async fn perform_pool_quota_probe_for_provider(
    state: &AppState,
    admin_state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    provider_type: &str,
    pool_config: &AdminProviderPoolConfig,
    endpoints_by_provider: &mut BTreeMap<String, Vec<StoredProviderCatalogEndpoint>>,
    config: PoolQuotaProbeWorkerConfig,
    mode: PoolQuotaProbeMode,
    now_ts: u64,
) -> Result<PoolQuotaProbeRunSummary, GatewayError> {
    let mut summary = PoolQuotaProbeRunSummary::empty();
    let provider_short_id = provider.id.chars().take(8).collect::<String>();

    if aether_admin::provider::quota::provider_auto_remove_banned_keys(provider.config.as_ref()) {
        let auto_removed = admin_state
            .cleanup_known_banned_provider_catalog_keys(provider)
            .await?;
        if auto_removed > 0 {
            summary.auto_removed += auto_removed;
            info!(
                event_name = "auto_removed_hard_banned",
                provider_id = %provider_short_id,
                provider_type,
                auto_removed,
                "gateway pool quota probe auto-cleaned known abnormal provider keys"
            );
        }
    }
    if aether_admin::provider::quota::provider_auto_remove_quota_exhausted_keys(
        provider.config.as_ref(),
    ) {
        let auto_removed = admin_state
            .cleanup_quota_exhausted_provider_catalog_keys(provider, provider_type)
            .await?;
        if auto_removed > 0 {
            summary.auto_removed += auto_removed;
            info!(
                event_name = "auto_removed_quota_exhausted",
                provider_id = %provider_short_id,
                provider_type,
                auto_removed,
                "gateway pool quota probe auto-cleaned quota-exhausted provider keys"
            );
        }
    }

    let Some(endpoint) = endpoint_for_probe_with_reconcile(
        state,
        admin_state,
        provider,
        provider_type,
        endpoints_by_provider,
    )
    .await?
    else {
        summary.providers_skipped += 1;
        debug!(
            provider_id = %provider.id,
            provider_type,
            "gateway pool quota probe skipped provider without active quota endpoint"
        );
        return Ok(summary);
    };
    let endpoints = endpoints_by_provider
        .remove(&provider.id)
        .unwrap_or_else(|| vec![endpoint.clone()]);

    let keys = match select_keys_for_provider(
        state,
        state.runtime_state.as_ref(),
        provider,
        pool_config,
        config,
        mode,
        now_ts,
    )
    .await?
    {
        PoolQuotaProbeSelectionOutcome::Selected(keys) => keys,
        PoolQuotaProbeSelectionOutcome::Busy => {
            summary.providers_busy += 1;
            return Ok(summary);
        }
        PoolQuotaProbeSelectionOutcome::Empty => return Ok(summary),
    };

    let selected_count = keys.len();
    summary.providers_probed += 1;
    summary.selected_keys += selected_count;

    let selected_key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
    let score_ensure_budget = (pool_config.score_fallback_scan_limit as usize)
        .min(50_000)
        .max(selected_count.min(50_000));
    match ensure_provider_key_pool_scores_for_keys(
        state,
        provider,
        pool_config,
        &endpoints,
        &keys,
        now_ts,
        score_ensure_budget,
    )
    .await
    {
        Ok(upserted) if upserted > 0 => {
            debug!(
                provider_id = %provider.id,
                key_count = selected_count,
                scores_upserted = upserted,
                "gateway pool quota probe: ensured score rows for selected probe keys"
            );
        }
        Ok(_) => {}
        Err(err) => {
            warn!(
                provider_id = %provider.id,
                key_count = selected_count,
                error = ?err,
                "gateway pool quota probe: failed to ensure score rows for selected probe keys"
            );
        }
    }
    for key_id in &selected_key_ids {
        record_score_probe_in_progress_for_key(state, &provider.id, key_id, now_ts).await;
    }

    let probe_concurrency = pool_config.probe_concurrency.clamp(1, 64) as usize;
    let probe_concurrency = probe_concurrency.min(config.global_concurrency).max(1);
    let probe_results = stream::iter(keys.into_iter().map(|key| {
        let key_id = key.id.clone();
        let endpoint = &endpoint;
        let provider_type = provider_type.to_string();
        async move {
            let result = refresh_provider_probe_keys(
                admin_state,
                provider,
                endpoint,
                provider_type.as_str(),
                vec![key],
            )
            .await;
            (key_id, result)
        }
    }))
    .buffer_unordered(probe_concurrency)
    .collect::<Vec<_>>()
    .await;

    let mut probe_success = 0usize;
    let mut probe_failed = 0usize;
    for (key_id, result) in probe_results {
        match result {
            Ok(payload) => {
                update_summary_from_payload(&mut summary, 1, payload.as_ref());
                probe_success += payload
                    .as_ref()
                    .and_then(|value| value.get("success"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                probe_failed += payload
                    .as_ref()
                    .and_then(|value| value.get("failed"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                let successful_key_ids = record_score_probe_results_from_payload(
                    state,
                    &provider.id,
                    std::slice::from_ref(&key_id),
                    payload.as_ref(),
                    now_ts,
                )
                .await;
                add_active_probe_member_ids(
                    state.runtime_state.as_ref(),
                    &provider.id,
                    &successful_key_ids,
                )
                .await;
            }
            Err(err) => {
                summary.failed += 1;
                probe_failed += 1;
                record_score_probe_result_for_key(
                    state,
                    &provider.id,
                    &key_id,
                    now_ts,
                    false,
                    Some(PoolMemberHardState::Cooldown),
                    pool_quota_probe_worker_error_score_reason(&err),
                )
                .await;
                warn!(
                    provider_id = %provider_short_id,
                    provider_type,
                    key_id,
                    error_category = "provider_quota_probe_failed",
                    "gateway pool quota probe failed"
                );
            }
        }
    }
    info!(
        provider_id = %provider_short_id,
        provider_type,
        mode = mode.as_str(),
        selected = selected_count,
        success = probe_success,
        failed = probe_failed,
        concurrency = probe_concurrency,
        "gateway pool quota probe completed"
    );

    Ok(summary)
}

pub(crate) async fn perform_pool_quota_probe_once_with_config(
    state: &AppState,
    config: PoolQuotaProbeWorkerConfig,
) -> Result<PoolQuotaProbeRunSummary, GatewayError> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(PoolQuotaProbeRunSummary::empty());
    }

    let providers = state
        .list_provider_catalog_providers(true)
        .await?
        .into_iter()
        .filter_map(|provider| {
            let provider_type = provider.provider_type.trim().to_ascii_lowercase();
            if provider_supports_quota_probe(&provider_type) {
                Some((provider, provider_type))
            } else {
                None
            }
        })
        .filter_map(|(provider, provider_type)| {
            let pool_config = admin_provider_pool_config(&provider)?;
            if pool_config.probing_enabled {
                Some((provider, provider_type, pool_config))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if providers.is_empty() {
        return Ok(PoolQuotaProbeRunSummary::empty());
    }

    let provider_ids = providers
        .iter()
        .map(|(provider, _, _)| provider.id.clone())
        .collect::<Vec<_>>();
    let mut endpoints_by_provider = BTreeMap::<String, Vec<StoredProviderCatalogEndpoint>>::new();
    for endpoint in state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await?
    {
        endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }

    let admin_state = AdminAppState::new(state);
    let now_ts = now_unix_secs();
    let mut summary = PoolQuotaProbeRunSummary {
        providers_checked: providers.len(),
        ..PoolQuotaProbeRunSummary::empty()
    };

    for (provider, provider_type, pool_config) in providers {
        let provider_summary = perform_pool_quota_probe_for_provider(
            state,
            &admin_state,
            &provider,
            &provider_type,
            &pool_config,
            &mut endpoints_by_provider,
            config,
            PoolQuotaProbeMode::Base,
            now_ts,
        )
        .await?;
        summary.providers_skipped += provider_summary.providers_skipped;
        summary.providers_probed += provider_summary.providers_probed;
        summary.providers_busy += provider_summary.providers_busy;
        summary.selected_keys += provider_summary.selected_keys;
        summary.succeeded += provider_summary.succeeded;
        summary.failed += provider_summary.failed;
        summary.auto_removed += provider_summary.auto_removed;
    }

    Ok(summary)
}

pub(crate) async fn perform_pool_quota_probe_once(
    state: &AppState,
) -> Result<PoolQuotaProbeRunSummary, GatewayError> {
    perform_pool_quota_probe_once_with_config(state, PoolQuotaProbeWorkerConfig::from_env()).await
}

async fn perform_pool_quota_probe_once_for_provider_with_mode(
    state: &AppState,
    provider_id: &str,
    config: PoolQuotaProbeWorkerConfig,
    mode: PoolQuotaProbeMode,
) -> Result<PoolQuotaProbeRunSummary, GatewayError> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(PoolQuotaProbeRunSummary::empty());
    }

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(PoolQuotaProbeRunSummary::empty());
    };
    if !provider.is_active {
        return Ok(PoolQuotaProbeRunSummary {
            providers_checked: 1,
            ..PoolQuotaProbeRunSummary::empty()
        });
    }
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !provider_supports_quota_probe(&provider_type) {
        return Ok(PoolQuotaProbeRunSummary {
            providers_checked: 1,
            ..PoolQuotaProbeRunSummary::empty()
        });
    }
    let Some(pool_config) =
        admin_provider_pool_config(&provider).filter(|config| config.probing_enabled)
    else {
        return Ok(PoolQuotaProbeRunSummary {
            providers_checked: 1,
            ..PoolQuotaProbeRunSummary::empty()
        });
    };

    let mut endpoints_by_provider = BTreeMap::<String, Vec<StoredProviderCatalogEndpoint>>::new();
    for endpoint in state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?
    {
        endpoints_by_provider
            .entry(endpoint.provider_id.clone())
            .or_default()
            .push(endpoint);
    }

    let admin_state = AdminAppState::new(state);
    let provider_summary = perform_pool_quota_probe_for_provider(
        state,
        &admin_state,
        &provider,
        &provider_type,
        &pool_config,
        &mut endpoints_by_provider,
        config,
        mode,
        now_unix_secs(),
    )
    .await?;
    Ok(PoolQuotaProbeRunSummary {
        providers_checked: 1,
        providers_probed: provider_summary.providers_probed,
        providers_skipped: provider_summary.providers_skipped,
        providers_busy: provider_summary.providers_busy,
        selected_keys: provider_summary.selected_keys,
        succeeded: provider_summary.succeeded,
        failed: provider_summary.failed,
        auto_removed: provider_summary.auto_removed,
    })
}

pub(crate) async fn perform_pool_quota_probe_once_for_provider_with_config(
    state: &AppState,
    provider_id: &str,
    config: PoolQuotaProbeWorkerConfig,
) -> Result<PoolQuotaProbeRunSummary, GatewayError> {
    perform_pool_quota_probe_once_for_provider_with_mode(
        state,
        provider_id,
        config,
        PoolQuotaProbeMode::Base,
    )
    .await
}

pub(crate) fn spawn_pool_quota_probe_replenish_for_request(
    state: AppState,
    provider_id: String,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return None;
    }

    let coordinator = Arc::clone(&state.pool_quota_probe_replenish);
    coordinator.spawn(provider_id.clone(), move || {
        run_pool_quota_probe_replenish(state.clone(), provider_id.clone())
    })
}

async fn run_pool_quota_probe_replenish(state: AppState, provider_id: String) {
    let runtime = state.runtime_state.clone();
    let runtime = runtime.as_ref();
    let config = PoolQuotaProbeWorkerConfig::from_env();
    run_pool_quota_probe_replenish_with(
        runtime,
        &provider_id,
        || {
            perform_pool_quota_probe_once_for_provider_with_mode(
                &state,
                &provider_id,
                config,
                PoolQuotaProbeMode::Burst,
            )
        },
        |lease| release_pool_quota_probe_burst_trigger_lock(runtime, Some(lease)),
    )
    .await;
}

async fn run_pool_quota_probe_replenish_with<Probe, ProbeFuture, Release, ReleaseFuture>(
    runtime: &RuntimeState,
    provider_id: &str,
    mut probe: Probe,
    mut release: Release,
) where
    Probe: FnMut() -> ProbeFuture,
    ProbeFuture: Future<Output = Result<PoolQuotaProbeRunSummary, GatewayError>>,
    Release: FnMut(RuntimeLockLease) -> ReleaseFuture,
    ReleaseFuture: Future<Output = ()>,
{
    mark_probe_burst_pending(runtime, provider_id).await;
    loop {
        let Some(lease) = acquire_pool_quota_probe_burst_trigger_lock(runtime, provider_id).await
        else {
            return;
        };
        let recheck_after_release = loop {
            let pending = match runtime.kv_take(&probe_burst_pending_key(provider_id)).await {
                Ok(pending) => pending.is_some(),
                Err(_) => break false,
            };
            if !pending {
                break true;
            }

            match probe().await {
                Ok(summary) => {
                    if summary.providers_busy > 0 {
                        mark_probe_burst_pending(runtime, provider_id).await;
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        continue;
                    }
                }
                Err(err) => {
                    warn!(
                        provider_id,
                        error = ?err,
                        "gateway pool quota probe request-triggered replenish failed"
                    );
                }
            }

            match runtime
                .kv_exists(&probe_burst_pending_key(provider_id))
                .await
            {
                Ok(true) => {}
                Ok(false) => break true,
                Err(_) => break false,
            }
        };

        release(lease).await;
        // A different instance may publish after the final pending check and fail
        // to acquire our old lease. Recheck after release, then acquire a fresh token
        // before consuming that signal. Read failures terminate instead of spinning.
        if !recheck_after_release
            || !runtime
                .kv_exists(&probe_burst_pending_key(provider_id))
                .await
                .unwrap_or(false)
        {
            return;
        }
        tokio::task::yield_now().await;
    }
}

pub(crate) fn spawn_pool_quota_probe_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return None;
    }

    let config = PoolQuotaProbeWorkerConfig::from_env();
    Some(crate::task_runtime::spawn_singleton_worker(
        state,
        crate::task_runtime::TASK_KEY_POOL_QUOTA_PROBE,
        move |state| async move {
            let mut interval = tokio::time::interval(config.scan_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                if perform_pool_quota_probe_once_with_config(&state, config)
                    .await
                    .is_err()
                {
                    warn!(
                        error_category = "pool_quota_probe_worker_failed",
                        "gateway pool quota probe worker tick failed"
                    );
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aether_runtime_state::MemoryRuntimeStateConfig;
    use serde_json::json;
    use tokio::sync::Notify;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pool_quota_probe_local_coalesces_before_spawn_and_keeps_one_follow_up() {
        let coordinator = Arc::new(PoolQuotaProbeReplenishCoordinator::new(8));
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let probe_calls = Arc::new(AtomicUsize::new(0));
        let release_calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let finish_first = Arc::new(Notify::new());
        let leader = coordinator
            .spawn("provider".to_string(), {
                let runtime = Arc::clone(&runtime);
                let probe_calls = Arc::clone(&probe_calls);
                let release_calls = Arc::clone(&release_calls);
                let started = Arc::clone(&started);
                let finish_first = Arc::clone(&finish_first);
                move || {
                    let runtime = Arc::clone(&runtime);
                    let probe_calls = Arc::clone(&probe_calls);
                    let release_calls = Arc::clone(&release_calls);
                    let started = Arc::clone(&started);
                    let finish_first = Arc::clone(&finish_first);
                    async move {
                        run_pool_quota_probe_replenish_with(
                            runtime.as_ref(),
                            "provider",
                            || async {
                                if probe_calls.fetch_add(1, Ordering::AcqRel) == 0 {
                                    started.notify_one();
                                    finish_first.notified().await;
                                }
                                Ok(PoolQuotaProbeRunSummary::empty())
                            },
                            |lease| {
                                release_calls.fetch_add(1, Ordering::AcqRel);
                                release_pool_quota_probe_burst_trigger_lock(
                                    runtime.as_ref(),
                                    Some(lease),
                                )
                            },
                        )
                        .await;
                    }
                }
            })
            .expect("one leader");
        tokio::time::timeout(Duration::from_secs(2), started.notified())
            .await
            .expect("first probe starts");
        let barrier = Arc::new(tokio::sync::Barrier::new(65));
        let mut triggers = tokio::task::JoinSet::new();
        for _ in 0..64 {
            let coordinator = Arc::clone(&coordinator);
            let barrier = Arc::clone(&barrier);
            triggers.spawn(async move {
                barrier.wait().await;
                assert!(coordinator
                    .spawn("provider".to_string(), || async {
                        panic!("a duplicate trigger must not spawn work")
                    })
                    .is_none());
            });
        }
        barrier.wait().await;
        while let Some(result) = triggers.join_next().await {
            result.expect("concurrent trigger");
        }
        assert_eq!(coordinator.snapshot().started_total, 1);
        assert_eq!(coordinator.snapshot().coalesced_total, 64);
        assert_eq!(probe_calls.load(Ordering::Acquire), 1);
        assert!(
            !runtime
                .kv_exists(&probe_burst_pending_key("provider"))
                .await
                .expect("pending read"),
            "local duplicates must not each write Redis pending while the leader is running"
        );
        finish_first.notify_one();
        tokio::time::timeout(Duration::from_secs(2), leader)
            .await
            .expect("leader finishes")
            .expect("leader task");
        assert_eq!(probe_calls.load(Ordering::Acquire), 2);
        assert_eq!(
            release_calls.load(Ordering::Acquire),
            2,
            "64 retriggers produce one additional Redis lock/drain cycle"
        );
        assert_eq!(coordinator.snapshot().active, 0);
    }

    #[tokio::test]
    async fn pool_quota_probe_local_different_providers_run_independently() {
        let coordinator = Arc::new(PoolQuotaProbeReplenishCoordinator::new(2));
        let started = Arc::new(tokio::sync::Semaphore::new(0));
        let finish = Arc::new(tokio::sync::Semaphore::new(0));
        let mut tasks = Vec::new();
        for provider_id in ["provider-a", "provider-b"] {
            tasks.push(
                coordinator
                    .spawn(provider_id.to_string(), {
                        let started = Arc::clone(&started);
                        let finish = Arc::clone(&finish);
                        move || {
                            let started = Arc::clone(&started);
                            let finish = Arc::clone(&finish);
                            async move {
                                started.add_permits(1);
                                finish.acquire().await.expect("finish signal").forget();
                            }
                        }
                    })
                    .expect("independent provider leader"),
            );
        }
        tokio::time::timeout(Duration::from_secs(2), started.acquire_many(2))
            .await
            .expect("both providers start")
            .expect("started permits")
            .forget();
        assert_eq!(coordinator.snapshot().active, 2);
        finish.add_permits(2);
        for task in tasks {
            task.await.expect("provider finishes");
        }
        assert_eq!(coordinator.snapshot().active, 0);
    }

    #[test]
    fn pool_quota_probe_local_exit_handoff_keeps_exactly_one_owner() {
        let coordinator = Arc::new(PoolQuotaProbeReplenishCoordinator::new(1));
        for _ in 0..100 {
            let mut first = coordinator
                .request("provider".to_string())
                .expect("first owner");
            assert!(first.next_pass());
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let (mut first, continues, replacement) = std::thread::scope(|scope| {
                let first_barrier = Arc::clone(&barrier);
                let exit = scope.spawn(move || {
                    first_barrier.wait();
                    let continues = first.next_pass();
                    (first, continues)
                });
                let trigger = scope.spawn(|| {
                    barrier.wait();
                    coordinator.request("provider".to_string())
                });
                let (first, continues) = exit.join().expect("exit thread");
                (first, continues, trigger.join().expect("trigger thread"))
            });
            assert_ne!(
                continues,
                replacement.is_some(),
                "the signal is consumed by exactly one owner"
            );
            assert_eq!(coordinator.snapshot().active, 1);
            if continues {
                assert!(!first.next_pass());
            }
            drop(first);
            if replacement.is_some() {
                assert_eq!(
                    coordinator.snapshot().active,
                    1,
                    "old cleanup must not delete the replacement"
                );
            }
            drop(replacement);
            assert_eq!(coordinator.snapshot().active, 0);
        }
    }

    #[tokio::test]
    async fn pool_quota_probe_local_abort_and_panic_release_admission() {
        let coordinator = Arc::new(PoolQuotaProbeReplenishCoordinator::new(1));
        let unpolled = coordinator
            .spawn("provider".to_string(), || async {
                std::future::pending::<()>().await;
            })
            .expect("unpolled owner");
        unpolled.abort();
        assert!(unpolled.await.expect_err("aborted").is_cancelled());
        assert_eq!(coordinator.snapshot().active, 0);

        let started = Arc::new(Notify::new());
        let running = coordinator
            .spawn("provider".to_string(), {
                let started = Arc::clone(&started);
                move || {
                    let started = Arc::clone(&started);
                    async move {
                        started.notify_one();
                        std::future::pending::<()>().await;
                    }
                }
            })
            .expect("running owner");
        started.notified().await;
        assert!(coordinator.request("provider".to_string()).is_none());
        running.abort();
        assert!(running.await.expect_err("aborted").is_cancelled());
        assert_eq!(coordinator.snapshot().active, 0);

        let panicked = coordinator
            .spawn("provider".to_string(), || async {
                panic!("probe panicked")
            })
            .expect("panic owner");
        assert!(panicked.await.expect_err("probe panic").is_panic());
        assert_eq!(coordinator.snapshot().active, 0);
        coordinator
            .spawn("provider".to_string(), || std::future::ready(()))
            .expect("later trigger can run")
            .await
            .expect("recovered probe");
        assert_eq!(coordinator.snapshot().active, 0);
    }

    #[test]
    fn pool_quota_probe_local_capacity_is_bounded_and_completed_keys_are_removed() {
        let coordinator = Arc::new(PoolQuotaProbeReplenishCoordinator::new(2));
        let first = coordinator.request("a".to_string()).expect("first");
        let second = coordinator.request("b".to_string()).expect("second");
        assert!(coordinator.request("c".to_string()).is_none());
        assert!(coordinator.request("a".to_string()).is_none());
        assert_eq!(coordinator.snapshot().capacity_rejected_total, 1);
        assert_eq!(coordinator.snapshot().coalesced_total, 1);
        drop(first);
        let replacement = coordinator
            .request("c".to_string())
            .expect("freed capacity");
        drop((second, replacement));
        for index in 0..1000 {
            drop(
                coordinator
                    .request(format!("provider-{index}"))
                    .expect("new provider"),
            );
            assert_eq!(coordinator.snapshot().active, 0);
        }
        assert_eq!(coordinator.snapshot().started_total, 1003);
    }

    #[tokio::test]
    async fn pool_quota_probe_local_state_clones_share_only_the_same_runtime_binding() {
        let state = AppState::new().expect("state");
        let cloned = state.clone();
        assert!(Arc::ptr_eq(
            &state.pool_quota_probe_replenish,
            &cloned.pool_quota_probe_replenish
        ));
        let rebound_same = cloned.with_runtime_state(Arc::clone(&state.runtime_state));
        assert!(Arc::ptr_eq(
            &state.pool_quota_probe_replenish,
            &rebound_same.pool_quota_probe_replenish
        ));
        let rebound = state
            .clone()
            .with_runtime_state(Arc::new(RuntimeState::memory(
                MemoryRuntimeStateConfig::default(),
            )));
        assert!(!Arc::ptr_eq(
            &state.pool_quota_probe_replenish,
            &rebound.pool_quota_probe_replenish
        ));
        let first = state
            .pool_quota_probe_replenish
            .request("provider".to_string())
            .expect("first runtime");
        assert!(rebound_same
            .pool_quota_probe_replenish
            .request("provider".to_string())
            .is_none());
        let second = rebound
            .pool_quota_probe_replenish
            .request("provider".to_string())
            .expect("other runtime is independent");
        drop((first, second));
    }

    #[tokio::test]
    async fn pool_quota_probe_replenish_rechecks_remote_pending_after_unlock() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let first = Arc::new(PoolQuotaProbeReplenishCoordinator::new(1));
        let second = Arc::new(PoolQuotaProbeReplenishCoordinator::new(1));
        let before_unlock = Arc::new(Notify::new());
        let finish_unlock = Arc::new(Notify::new());
        let first_probes = Arc::new(AtomicUsize::new(0));
        let first_releases = Arc::new(AtomicUsize::new(0));
        let first_task = first
            .spawn("provider".to_string(), {
                let runtime = Arc::clone(&runtime);
                let before_unlock = Arc::clone(&before_unlock);
                let finish_unlock = Arc::clone(&finish_unlock);
                let first_probes = Arc::clone(&first_probes);
                let first_releases = Arc::clone(&first_releases);
                move || {
                    let runtime = Arc::clone(&runtime);
                    let before_unlock = Arc::clone(&before_unlock);
                    let finish_unlock = Arc::clone(&finish_unlock);
                    let first_probes = Arc::clone(&first_probes);
                    let first_releases = Arc::clone(&first_releases);
                    async move {
                        run_pool_quota_probe_replenish_with(
                            runtime.as_ref(),
                            "provider",
                            || {
                                first_probes.fetch_add(1, Ordering::AcqRel);
                                std::future::ready(Ok(PoolQuotaProbeRunSummary::empty()))
                            },
                            |lease| {
                                let runtime = Arc::clone(&runtime);
                                let before_unlock = Arc::clone(&before_unlock);
                                let finish_unlock = Arc::clone(&finish_unlock);
                                let first_release =
                                    first_releases.fetch_add(1, Ordering::AcqRel) == 0;
                                async move {
                                    if first_release {
                                        before_unlock.notify_one();
                                        finish_unlock.notified().await;
                                    }
                                    release_pool_quota_probe_burst_trigger_lock(
                                        runtime.as_ref(),
                                        Some(lease),
                                    )
                                    .await;
                                }
                            },
                        )
                        .await;
                    }
                }
            })
            .expect("first instance leader");
        tokio::time::timeout(Duration::from_secs(2), before_unlock.notified())
            .await
            .expect("first instance drained but still owns Redis lease");
        assert_eq!(first_probes.load(Ordering::Acquire), 1);
        assert!(!runtime
            .kv_exists(&probe_burst_pending_key("provider"))
            .await
            .expect("drained pending"));
        let second_task = second.spawn("provider".to_string(), {
            let runtime = Arc::clone(&runtime);
            move || {
                let runtime = Arc::clone(&runtime);
                async move {
                    run_pool_quota_probe_replenish_with(
                        runtime.as_ref(), "provider",
                        || async { panic!("second instance must not consume pending without the Redis lease") },
                        |lease| release_pool_quota_probe_burst_trigger_lock(runtime.as_ref(), Some(lease)),
                    ).await;
                }
            }
        }).expect("independent second instance leader");
        second_task
            .await
            .expect("second instance leaves pending for the current owner");
        assert_eq!(second.snapshot().active, 0);
        assert!(runtime
            .kv_exists(&probe_burst_pending_key("provider"))
            .await
            .expect("new pending signal"));
        finish_unlock.notify_one();
        tokio::time::timeout(Duration::from_secs(2), first_task)
            .await
            .expect("handoff drains")
            .expect("first instance finishes");
        assert_eq!(
            first_probes.load(Ordering::Acquire),
            2,
            "the cross-instance exit signal must trigger a second probe"
        );
        assert_eq!(first_releases.load(Ordering::Acquire), 2);
        assert_eq!(first.snapshot().active, 0);
        assert!(!runtime
            .kv_exists(&probe_burst_pending_key("provider"))
            .await
            .expect("all pending consumed"));
        let lease = acquire_pool_quota_probe_burst_trigger_lock(runtime.as_ref(), "provider").await;
        assert!(lease.is_some(), "the replacement Redis lease is released");
        release_pool_quota_probe_burst_trigger_lock(runtime.as_ref(), lease).await;
    }

    #[tokio::test]
    async fn pool_quota_probe_replenish_public_spawn_returns_none_for_merged_triggers() {
        let repository = Arc::new(
            aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository::seed(
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
        );
        let state = AppState::new().expect("state").with_data_state_for_tests(
            crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(repository),
        );
        let leader =
            spawn_pool_quota_probe_replenish_for_request(state.clone(), "provider".to_string())
                .expect("leader handle");
        for _ in 0..64 {
            assert!(spawn_pool_quota_probe_replenish_for_request(
                state.clone(),
                "provider".to_string()
            )
            .is_none());
        }
        leader.await.expect("leader completes");
        assert_eq!(state.pool_quota_probe_replenish.snapshot().started_total, 1);
        assert_eq!(
            state.pool_quota_probe_replenish.snapshot().coalesced_total,
            64
        );
        assert_eq!(state.pool_quota_probe_replenish.snapshot().active, 0);
        spawn_pool_quota_probe_replenish_for_request(state.clone(), "provider".to_string())
            .expect("later leader handle")
            .await
            .expect("later leader finishes");
    }

    #[test]
    fn worker_error_score_reason_drops_runtime_error_details() {
        let secret = "postgresql://admin:db-secret@db.internal/aether; Authorization: Bearer quota-secret; https://user:password@upstream.test/quota?q=secret";
        let patch =
            pool_quota_probe_worker_error_score_reason(&GatewayError::Internal(secret.to_string()));
        let serialized = patch.to_string();

        assert_eq!(patch["last_probe"]["status"], "worker_error");
        assert_eq!(patch["last_probe"]["category"], "worker_error");
        assert_eq!(
            patch["last_probe"]["message"],
            "Provider quota probe worker failed"
        );
        for sensitive in [
            "db-secret",
            "quota-secret",
            "user:password",
            "db.internal",
            "upstream.test",
        ] {
            assert!(!serialized.contains(sensitive));
        }
    }

    fn key(
        id: &str,
        provider_id: &str,
        upstream_metadata: Option<Value>,
    ) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            id.to_string(),
            provider_id.to_string(),
            id.to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.upstream_metadata = upstream_metadata;
        key
    }

    #[test]
    fn selects_stale_probe_keys_by_oldest_anchor() {
        let keys = vec![
            key(
                "fresh",
                "provider-1",
                Some(json!({ "codex": { "updated_at": 1_990 } })),
            ),
            key(
                "old",
                "provider-1",
                Some(json!({ "codex": { "updated_at": 1_000 } })),
            ),
            key("never", "provider-1", None),
            key(
                "stamped",
                "provider-1",
                Some(json!({ "codex": { "updated_at": 900 } })),
            ),
        ];
        let stamps = BTreeMap::from([("stamped".to_string(), 1_950)]);

        let selected = select_pool_quota_probe_key_ids(&keys, "codex", 2_000, 600, &stamps, 2);

        assert_eq!(selected, vec!["never".to_string(), "old".to_string()]);
    }

    #[test]
    fn quota_markers_in_429_probe_results_are_hard_exhaustion() {
        assert_eq!(
            probe_result_hard_state(&json!({
                "status": "rate_limited",
                "status_code": 429,
                "message": "RESOURCE_EXHAUSTED: quota exceeded"
            })),
            Some(PoolMemberHardState::QuotaExhausted)
        );
        assert_eq!(
            probe_result_hard_state(&json!({
                "status": "rate_limited",
                "status_code": 429,
                "message": "temporary rate limit"
            })),
            Some(PoolMemberHardState::Cooldown)
        );
    }

    fn score(
        member_id: &str,
        hard_state: PoolMemberHardState,
        probe_status: PoolMemberProbeStatus,
        last_probe_success_at: Option<u64>,
        last_probe_attempt_at: Option<u64>,
    ) -> StoredPoolMemberScore {
        score_with_value(
            member_id,
            hard_state,
            probe_status,
            last_probe_success_at,
            last_probe_attempt_at,
            1.0,
        )
    }

    fn score_with_value(
        member_id: &str,
        hard_state: PoolMemberHardState,
        probe_status: PoolMemberProbeStatus,
        last_probe_success_at: Option<u64>,
        last_probe_attempt_at: Option<u64>,
        score_value: f64,
    ) -> StoredPoolMemberScore {
        StoredPoolMemberScore {
            id: format!("score-{member_id}"),
            pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
            pool_id: "provider-1".to_string(),
            member_kind: "provider_api_key".to_string(),
            member_id: member_id.to_string(),
            capability: "account".to_string(),
            scope_kind: "account".to_string(),
            scope_id: None,
            score: score_value,
            hard_state,
            score_version: 1,
            score_reason: json!({}),
            last_ranked_at: None,
            last_scheduled_at: None,
            last_success_at: None,
            last_failure_at: None,
            failure_count: 0,
            last_probe_attempt_at,
            last_probe_success_at,
            last_probe_failure_at: None,
            probe_failure_count: 0,
            probe_status,
            updated_at: 0,
        }
    }

    #[test]
    fn active_probe_target_uses_larger_of_percent_and_count() {
        assert_eq!(pool_quota_probe_target_count(10, Some(20.0), None), 2);
        assert_eq!(pool_quota_probe_target_count(10, Some(20.0), Some(5)), 5);
        assert_eq!(pool_quota_probe_target_count(3, Some(80.0), Some(10)), 3);
    }

    #[test]
    fn pool_quota_probe_auto_interval_tracks_request_pressure() {
        let idle = ProviderPoolDemandSnapshot {
            in_flight: 0,
            ema_in_flight: 0.0,
            desired_hot: 2,
            sampled_at_unix_ms: 0,
        };
        assert_eq!(
            pool_quota_probe_auto_interval_seconds(&idle),
            POOL_QUOTA_PROBE_AUTO_MAX_INTERVAL_SECONDS
        );

        let active = ProviderPoolDemandSnapshot {
            in_flight: 1,
            ema_in_flight: 1.0,
            desired_hot: 2,
            sampled_at_unix_ms: 0,
        };
        assert!(
            pool_quota_probe_auto_interval_seconds(&active)
                < POOL_QUOTA_PROBE_AUTO_MAX_INTERVAL_SECONDS
        );

        let saturated = ProviderPoolDemandSnapshot {
            in_flight: 128,
            ema_in_flight: 128.0,
            desired_hot: 128,
            sampled_at_unix_ms: 0,
        };
        assert_eq!(
            pool_quota_probe_auto_interval_seconds(&saturated),
            POOL_QUOTA_PROBE_AUTO_MIN_INTERVAL_SECONDS
        );
    }

    #[test]
    fn selects_only_pool_out_keys_to_fill_active_probe_target() {
        let key_ids = vec![
            "active".to_string(),
            "never".to_string(),
            "stale".to_string(),
            "banned".to_string(),
        ];
        let active_member_ids = vec!["active".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let scores = BTreeMap::from([
            (
                "active".to_string(),
                score(
                    "active",
                    PoolMemberHardState::Available,
                    PoolMemberProbeStatus::Ok,
                    Some(1_900),
                    Some(1_900),
                ),
            ),
            (
                "stale".to_string(),
                score(
                    "stale",
                    PoolMemberHardState::Available,
                    PoolMemberProbeStatus::Failed,
                    None,
                    Some(500),
                ),
            ),
            (
                "banned".to_string(),
                score(
                    "banned",
                    PoolMemberHardState::Banned,
                    PoolMemberProbeStatus::Failed,
                    None,
                    Some(1_800),
                ),
            ),
        ]);

        let selected = select_pool_quota_probe_ids_for_active_target(
            &key_ids,
            &active_member_ids,
            &scores,
            false,
            3,
            10,
        );

        assert_eq!(selected, vec!["never".to_string(), "stale".to_string()]);
    }

    #[test]
    fn active_probe_target_prefers_latest_successful_self_check_when_enabled() {
        let key_ids = vec![
            "older".to_string(),
            "latest".to_string(),
            "unchecked".to_string(),
            "failed".to_string(),
        ];
        let active_member_ids = BTreeSet::new();
        let mut older = score_with_value(
            "older",
            PoolMemberHardState::Available,
            PoolMemberProbeStatus::Ok,
            Some(100),
            Some(100),
            9.0,
        );
        older.score_reason = json!({
            "last_self_check": {
                "source": "account_self_check",
                "status": "success",
                "attempted_at": 100
            }
        });
        let mut latest = score_with_value(
            "latest",
            PoolMemberHardState::Available,
            PoolMemberProbeStatus::Ok,
            Some(200),
            Some(200),
            1.0,
        );
        latest.score_reason = json!({
            "last_self_check": {
                "source": "account_self_check",
                "status": "success",
                "attempted_at": 200
            }
        });
        let unchecked = score_with_value(
            "unchecked",
            PoolMemberHardState::Available,
            PoolMemberProbeStatus::Never,
            None,
            None,
            99.0,
        );
        let mut failed = score_with_value(
            "failed",
            PoolMemberHardState::Available,
            PoolMemberProbeStatus::Failed,
            None,
            Some(300),
            100.0,
        );
        failed.score_reason = json!({
            "last_self_check": {
                "source": "account_self_check",
                "status": "failed",
                "attempted_at": 300
            }
        });
        let scores = BTreeMap::from([
            ("older".to_string(), older),
            ("latest".to_string(), latest),
            ("unchecked".to_string(), unchecked),
            ("failed".to_string(), failed),
        ]);

        let selected = select_pool_quota_probe_ids_for_active_target(
            &key_ids,
            &active_member_ids,
            &scores,
            true,
            1,
            10,
        );

        assert_eq!(selected, vec!["latest".to_string()]);
    }

    #[test]
    fn active_probe_target_uses_unchecked_keys_when_self_check_history_is_missing() {
        let key_ids = vec!["fresh".to_string(), "latest".to_string()];
        let active_member_ids = BTreeSet::new();
        let mut latest = score_with_value(
            "latest",
            PoolMemberHardState::Available,
            PoolMemberProbeStatus::Ok,
            Some(200),
            Some(200),
            1.0,
        );
        latest.score_reason = json!({
            "last_self_check": {
                "source": "account_self_check",
                "status": "success",
                "attempted_at": 200
            }
        });
        let scores = BTreeMap::from([("latest".to_string(), latest)]);

        let selected = select_pool_quota_probe_ids_for_active_target(
            &key_ids,
            &active_member_ids,
            &scores,
            true,
            2,
            10,
        );

        assert_eq!(selected, vec!["latest".to_string(), "fresh".to_string()]);
    }

    #[test]
    fn active_probe_target_does_not_expire_confirmed_accounts_by_time() {
        let key_ids = vec!["old_success".to_string(), "candidate".to_string()];
        let active_member_ids = vec!["old_success".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let scores = BTreeMap::from([(
            "old_success".to_string(),
            score(
                "old_success",
                PoolMemberHardState::Available,
                PoolMemberProbeStatus::Ok,
                Some(10),
                Some(10),
            ),
        )]);

        let selected = select_pool_quota_probe_ids_for_active_target(
            &key_ids,
            &active_member_ids,
            &scores,
            false,
            1,
            10,
        );

        assert!(selected.is_empty());
    }

    #[test]
    fn active_probe_members_drop_only_when_missing_or_explicitly_unusable() {
        let key_ids = vec![
            "kept".to_string(),
            "cooldown".to_string(),
            "unknown".to_string(),
        ];
        let active_member_ids = vec![
            "kept".to_string(),
            "cooldown".to_string(),
            "missing".to_string(),
            "unknown".to_string(),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        let scores = BTreeMap::from([
            (
                "kept".to_string(),
                score(
                    "kept",
                    PoolMemberHardState::Available,
                    PoolMemberProbeStatus::Ok,
                    Some(2_000),
                    Some(2_000),
                ),
            ),
            (
                "cooldown".to_string(),
                score(
                    "cooldown",
                    PoolMemberHardState::Cooldown,
                    PoolMemberProbeStatus::Failed,
                    Some(1_000),
                    Some(2_000),
                ),
            ),
            (
                "unknown".to_string(),
                score(
                    "unknown",
                    PoolMemberHardState::Unknown,
                    PoolMemberProbeStatus::Never,
                    None,
                    None,
                ),
            ),
        ]);

        let (retained, removed) =
            prune_pool_quota_probe_active_member_ids(&key_ids, &active_member_ids, &scores);

        assert_eq!(
            retained,
            vec!["kept".to_string(), "unknown".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(removed, vec!["cooldown".to_string(), "missing".to_string()]);
    }

    #[test]
    fn active_probe_base_trim_removes_lowest_ranked_extra_members() {
        let active_member_ids = vec!["kept".to_string(), "dropped".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let scores = BTreeMap::from([
            (
                "kept".to_string(),
                score_with_value(
                    "kept",
                    PoolMemberHardState::Available,
                    PoolMemberProbeStatus::Ok,
                    Some(2_000),
                    Some(2_000),
                    9.0,
                ),
            ),
            (
                "dropped".to_string(),
                score_with_value(
                    "dropped",
                    PoolMemberHardState::Available,
                    PoolMemberProbeStatus::Ok,
                    Some(2_000),
                    Some(2_000),
                    1.0,
                ),
            ),
        ]);

        let trimmed =
            trim_pool_quota_probe_active_member_ids_to_target(&active_member_ids, &scores, 1);

        assert_eq!(trimmed, vec!["dropped".to_string()]);
    }

    #[test]
    fn active_probe_target_keeps_existing_members_even_if_pool_out_scores_higher() {
        let key_ids = vec!["kept".to_string(), "candidate".to_string()];
        let active_member_ids = vec!["kept".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let scores = BTreeMap::from([
            (
                "kept".to_string(),
                score_with_value(
                    "kept",
                    PoolMemberHardState::Available,
                    PoolMemberProbeStatus::Ok,
                    Some(2_000),
                    Some(2_000),
                    0.1,
                ),
            ),
            (
                "candidate".to_string(),
                score_with_value(
                    "candidate",
                    PoolMemberHardState::Available,
                    PoolMemberProbeStatus::Never,
                    None,
                    None,
                    9.0,
                ),
            ),
        ]);

        let selected = select_pool_quota_probe_ids_for_active_target(
            &key_ids,
            &active_member_ids,
            &scores,
            false,
            1,
            10,
        );

        assert!(selected.is_empty());
    }

    #[test]
    fn parses_quota_updated_at_seconds_and_milliseconds() {
        assert_eq!(
            provider_pool_quota_metadata_updated_at(
                Some(&json!({ "codex": { "updated_at": 1_700_000_000 } })),
                "codex"
            ),
            Some(1_700_000_000)
        );
        assert_eq!(
            provider_pool_quota_metadata_updated_at(
                Some(&json!({ "kiro": { "updated_at": 1_700_000_000_000_u64 } })),
                "kiro"
            ),
            Some(1_700_000_000)
        );
    }
}
