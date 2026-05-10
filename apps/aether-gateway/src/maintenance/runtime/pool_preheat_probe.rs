use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::{Duration, Instant};

use aether_data_contracts::repository::candidate_selection::StoredPoolKeyCandidateRowsQuery;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_runtime_state::{
    DataLayerError, RateLimitCheck, RateLimitInput, RateLimitScope, RuntimeLockLease,
};
use serde::Serialize;
use serde_json::{json, Value};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::admin_api::{
    probe_oauth_provider_key_with_classification, provider_oauth_runtime_endpoint_for_provider,
    AdminAppState, OauthKeyProbeClassification, OauthKeyProbeOutcome,
};
use crate::cache::{CachedPoolCandidates, PoolCandidateCacheKey};
use crate::clock::current_unix_secs;
use crate::handlers::shared::provider_key_status_snapshot_payload;
use crate::handlers::shared::provider_pool::{
    admin_provider_pool_config_from_config_value, record_admin_provider_pool_error,
    AdminProviderPoolConfig,
};
use crate::orchestration::{
    project_local_failure_health, project_local_key_circuit_open, LocalFailoverClassification,
};
use crate::{AppState, GatewayError};

const POOL_PREHEAT_DEFAULT_COOLDOWN_SECS: u64 = 60;
const POOL_PREHEAT_RATE_LIMIT_COOLDOWN_STATUS_CODE: u16 = 429;
const POOL_PREHEAT_CANDIDATE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const POOL_PREHEAT_CANDIDATE_CACHE_MAX_ENTRIES: usize = 1024;
const POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS: usize = 128;
const POOL_PREHEAT_BACKFILL_SCAN_LIMIT: u32 = 128;
const POOL_PREHEAT_PROVIDER_RATE_LIMIT_WINDOW_SECS: u64 = 60;
const OAUTH_ACCOUNT_BLOCK_PREFIX: &str = "[ACCOUNT_BLOCK] ";
const OAUTH_EXPIRED_PREFIX: &str = "[OAUTH_EXPIRED] ";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PoolPreheatProbeTaskRequest {
    pub(crate) provider_id: String,
    pub(crate) pool_group_id: String,
    pub(crate) top_n_key_ids: Vec<String>,
}

impl PoolPreheatProbeTaskRequest {
    pub(crate) fn new(
        provider_id: impl Into<String>,
        pool_group_id: impl Into<String>,
        top_n_key_ids: Vec<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into().trim().to_string(),
            pool_group_id: pool_group_id.into().trim().to_string(),
            top_n_key_ids: normalize_pool_preheat_probe_key_ids(top_n_key_ids),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub(crate) struct PoolPreheatProbeRunSummary {
    pub(crate) provider_id: String,
    pub(crate) pool_group_id: String,
    pub(crate) requested_keys: usize,
    pub(crate) selected_keys: usize,
    pub(crate) skipped_keys: usize,
    pub(crate) deduped_keys: usize,
    pub(crate) probed_keys: usize,
    pub(crate) healthy: usize,
    pub(crate) oauth_invalid: usize,
    pub(crate) account_blocked: usize,
    pub(crate) rate_limited: usize,
    pub(crate) server_error: usize,
    pub(crate) transport_error: usize,
    pub(crate) errored: usize,
    pub(crate) coordination_fail_open: usize,
    pub(crate) rate_limit_remaining: Option<u32>,
    pub(crate) stopped_after_healthy: bool,
    pub(crate) skipped_reason: Option<String>,
}

impl PoolPreheatProbeRunSummary {
    fn new(request: &PoolPreheatProbeTaskRequest) -> Self {
        Self {
            provider_id: request.provider_id.clone(),
            pool_group_id: request.pool_group_id.clone(),
            ..Self::default()
        }
    }

    fn skipped(request: &PoolPreheatProbeTaskRequest, reason: impl Into<String>) -> Self {
        Self {
            skipped_reason: Some(reason.into()),
            ..Self::new(request)
        }
    }

    fn selected(
        request: &PoolPreheatProbeTaskRequest,
        requested_keys: usize,
        selected_keys: usize,
    ) -> Self {
        Self {
            requested_keys,
            selected_keys,
            skipped_keys: requested_keys.saturating_sub(selected_keys),
            ..Self::new(request)
        }
    }

    fn skipped_selected(
        request: &PoolPreheatProbeTaskRequest,
        requested_keys: usize,
        selected_keys: usize,
        deduped_keys: usize,
        coordination_fail_open: usize,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            deduped_keys,
            healthy: deduped_keys,
            coordination_fail_open,
            skipped_reason: Some(reason.into()),
            ..Self::selected(request, requested_keys, selected_keys)
        }
    }

    pub(crate) fn task_status_is_skipped(&self) -> bool {
        self.probed_keys == 0 && self.skipped_reason.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolPreheatProviderRateLimitDecision {
    Allowed {
        remaining: Option<u32>,
        fail_open: bool,
    },
    Rejected {
        scope: RateLimitScope,
        limit: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PoolPreheatProbeGroupContext {
    provider_id: String,
    endpoint_id: String,
    model_id: String,
    selected_provider_model_name: String,
    api_format: String,
}

pub(crate) fn pool_preheat_probe_provider_is_supported(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "codex" | "kiro" | "antigravity" | "chatgpt_web"
    )
}

fn normalize_pool_preheat_probe_key_ids(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter_map(|value| {
            let value = value.trim().to_string();
            if value.is_empty() || !seen.insert(value.clone()) {
                None
            } else {
                Some(value)
            }
        })
        .collect()
}

fn parse_pool_group_context(pool_group_id: &str) -> Option<PoolPreheatProbeGroupContext> {
    let mut parts = BTreeMap::<&str, &str>::new();
    for part in pool_group_id.split('|') {
        let (key, value) = part.split_once('=')?;
        parts.insert(key.trim(), value.trim());
    }

    let provider_id = parts.get("provider")?.trim();
    let endpoint_id = parts.get("endpoint")?.trim();
    let model_id = parts.get("model")?.trim();
    let selected_provider_model_name = parts.get("selected_model")?.trim();
    let api_format = parts.get("api_format")?.trim();
    if [
        provider_id,
        endpoint_id,
        model_id,
        selected_provider_model_name,
        api_format,
    ]
    .iter()
    .any(|value| value.is_empty())
    {
        return None;
    }

    Some(PoolPreheatProbeGroupContext {
        provider_id: provider_id.to_string(),
        endpoint_id: endpoint_id.to_string(),
        model_id: model_id.to_string(),
        selected_provider_model_name: selected_provider_model_name.to_string(),
        api_format: api_format.to_string(),
    })
}

fn pool_preheat_probe_cache_key(
    request: &PoolPreheatProbeTaskRequest,
) -> Option<PoolCandidateCacheKey> {
    let context = parse_pool_group_context(&request.pool_group_id)?;
    if context.provider_id != request.provider_id {
        return None;
    }
    Some(PoolCandidateCacheKey::new(
        context.provider_id,
        context.endpoint_id,
        context.model_id,
        context.selected_provider_model_name,
        context.api_format,
    ))
}

fn pool_preheat_probe_group_lock_key(request: &PoolPreheatProbeTaskRequest) -> String {
    format!(
        "ap:preheat:lock:{}:{}",
        request.provider_id, request.pool_group_id
    )
}

fn pool_preheat_provider_rate_limit_user_key(provider_id: &str) -> String {
    format!("ap:preheat:rpm:provider:{provider_id}")
}

fn pool_preheat_provider_rate_limit_key_key(provider_id: &str) -> String {
    format!("ap:preheat:rpm:key:{provider_id}")
}

fn pool_preheat_provider_circuit_samples_key(provider_id: &str) -> String {
    format!("ap:preheat:circuit:samples:{provider_id}")
}

fn pool_preheat_provider_circuit_suspend_key(provider_id: &str) -> String {
    format!("ap:preheat:circuit:suspend:{provider_id}")
}

fn pool_preheat_provider_rate_limit_bucket(now_unix_secs: u64) -> u64 {
    now_unix_secs / POOL_PREHEAT_PROVIDER_RATE_LIMIT_WINDOW_SECS
}

fn pool_preheat_provider_rate_limit_scope_label(scope: RateLimitScope) -> &'static str {
    match scope {
        RateLimitScope::User => "provider",
        RateLimitScope::Key => "key",
    }
}

fn pool_preheat_probe_trigger_label(request: &PoolPreheatProbeTaskRequest) -> &'static str {
    if request.top_n_key_ids.is_empty() {
        "on_demand"
    } else {
        "candidate_loop"
    }
}

fn pool_preheat_probe_provider_type_label(provider_type: &str) -> String {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    if provider_type.is_empty() {
        "unknown".to_string()
    } else {
        provider_type
    }
}

fn pool_preheat_probe_outcome_kind(classification: OauthKeyProbeClassification) -> &'static str {
    match classification {
        OauthKeyProbeClassification::Healthy => "healthy",
        OauthKeyProbeClassification::OAuthInvalid => "oauth_invalid",
        OauthKeyProbeClassification::AccountBlocked => "account_blocked",
        OauthKeyProbeClassification::RateLimited => "rate_limited",
        OauthKeyProbeClassification::ServerError => "server_error",
        OauthKeyProbeClassification::TransportError => "transport_error",
    }
}

fn pool_preheat_gateway_error_kind(error: &GatewayError) -> &'static str {
    match error {
        GatewayError::UpstreamUnavailable { .. } => "upstream_unavailable",
        GatewayError::ControlUnavailable { .. } => "control_unavailable",
        GatewayError::Internal(_) => "internal",
    }
}

fn elapsed_millis_u64(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn pool_preheat_probe_run_outcome(summary: &PoolPreheatProbeRunSummary) -> &'static str {
    if summary.task_status_is_skipped() {
        "skipped"
    } else if summary.errored > 0 {
        "completed_with_errors"
    } else {
        "completed"
    }
}

fn log_pool_preheat_probe_run_started(request: &PoolPreheatProbeTaskRequest, trigger: &str) {
    info!(
        event_name = "run_started",
        event = "run_started",
        log_type = "event",
        worker = "pool_preheat_probe",
        component = "pool_preheat_probe",
        trigger,
        provider_id = %request.provider_id,
        pool_group_id = %request.pool_group_id,
        top_n = request.top_n_key_ids.len(),
        top_n_key_count = request.top_n_key_ids.len(),
        "gateway pool preheat probe run started"
    );
}

fn log_pool_preheat_probe_outcome(
    request: &PoolPreheatProbeTaskRequest,
    provider_type: &str,
    key_id: &str,
    classification: OauthKeyProbeClassification,
    duration_ms: u64,
) {
    let outcome = pool_preheat_probe_outcome_kind(classification);
    info!(
        event_name = "probe_outcome",
        event = "probe_outcome",
        log_type = "event",
        worker = "pool_preheat_probe",
        component = "pool_preheat_probe",
        provider_id = %request.provider_id,
        provider_type,
        pool_group_id = %request.pool_group_id,
        key_id,
        outcome,
        outcome_kind = outcome,
        duration_ms,
        "gateway pool preheat probe classified key"
    );
}

fn log_pool_preheat_probe_error(
    request: &PoolPreheatProbeTaskRequest,
    provider_type: &str,
    key_id: &str,
    trigger_error_kind: &str,
    error: &GatewayError,
    duration_ms: u64,
) {
    warn!(
        event_name = "probe_error",
        event = "probe_error",
        log_type = "event",
        worker = "pool_preheat_probe",
        component = "pool_preheat_probe",
        provider_id = %request.provider_id,
        provider_type,
        pool_group_id = %request.pool_group_id,
        key_id,
        error = trigger_error_kind,
        trigger_error_kind,
        error_kind = pool_preheat_gateway_error_kind(error),
        duration_ms,
        "gateway pool preheat probe key processing failed"
    );
}

fn complete_pool_preheat_probe_run(
    state: &AppState,
    request: &PoolPreheatProbeTaskRequest,
    provider_type: &str,
    trigger: &str,
    summary: PoolPreheatProbeRunSummary,
) -> PoolPreheatProbeRunSummary {
    let provider_type = pool_preheat_probe_provider_type_label(provider_type);
    let outcome = pool_preheat_probe_run_outcome(&summary);
    state
        .pool_preheat_metrics()
        .record_probe_run(trigger, outcome);
    state
        .pool_preheat_metrics()
        .record_dedup_skipped(&provider_type, summary.deduped_keys as u64);
    let circuit_skipped =
        usize::from(summary.skipped_reason.as_deref() == Some("provider probe circuit open"));
    info!(
        event_name = "run_complete",
        event = "run_complete",
        log_type = "event",
        worker = "pool_preheat_probe",
        component = "pool_preheat_probe",
        trigger,
        outcome,
        provider_id = %summary.provider_id,
        provider_type = provider_type.as_str(),
        pool_group_id = %summary.pool_group_id,
        selected = summary.selected_keys,
        probed = summary.probed_keys,
        requested_keys = summary.requested_keys,
        selected_keys = summary.selected_keys,
        skipped_keys = summary.skipped_keys,
        dedup_skipped = summary.deduped_keys,
        circuit_skipped,
        deduped_keys = summary.deduped_keys,
        probed_keys = summary.probed_keys,
        healthy = summary.healthy,
        oauth_invalid = summary.oauth_invalid,
        account_blocked = summary.account_blocked,
        rate_limited = summary.rate_limited,
        server_error = summary.server_error,
        transport_error = summary.transport_error,
        errored = summary.errored,
        coordination_fail_open = summary.coordination_fail_open,
        rate_limit_remaining = ?summary.rate_limit_remaining,
        stopped_after_healthy = summary.stopped_after_healthy,
        skipped_reason = summary.skipped_reason.as_deref().unwrap_or("-"),
        "gateway pool preheat probe run completed"
    );
    summary
}

fn parse_probe_circuit_suspend_until(raw_value: Option<&str>) -> Option<u64> {
    raw_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
}

fn probe_circuit_sample_threshold(pool_config: &AdminProviderPoolConfig) -> usize {
    pool_config.pre_probe.five_xx_streak_threshold.max(1) as usize
}

fn probe_circuit_outcome_is_failure(outcome: &OauthKeyProbeOutcome) -> bool {
    matches!(
        outcome.classification(),
        OauthKeyProbeClassification::ServerError | OauthKeyProbeClassification::TransportError
    )
}

fn probe_circuit_sample_member(failed: bool, now_unix_secs: u64) -> String {
    format!(
        "{}:{now_unix_secs}:{}",
        u8::from(failed),
        Uuid::new_v4().simple()
    )
}

fn probe_circuit_sample_member_is_failure(member: &str) -> bool {
    member
        .split_once(':')
        .is_some_and(|(failed, _)| failed == "1")
}

fn probe_circuit_failure_rate_exceeds_threshold(
    failed_samples: usize,
    total_samples: usize,
    threshold_percent: u32,
) -> bool {
    if total_samples == 0 {
        return false;
    }
    (failed_samples as u128).saturating_mul(100)
        > (total_samples as u128).saturating_mul(u128::from(threshold_percent.max(1)))
}

async fn probe_circuit_allows_provider(
    state: &AppState,
    provider_id: &str,
    _pool_config: &AdminProviderPoolConfig,
    now_unix_secs: u64,
) -> bool {
    let suspend_key = pool_preheat_provider_circuit_suspend_key(provider_id);
    let raw_suspend_until = match state.runtime_state.kv_get(&suspend_key).await {
        Ok(value) => value,
        Err(error) => {
            warn!(
                provider_id,
                error = ?error,
                "gateway pool preheat probe provider circuit read unavailable; failing open"
            );
            return true;
        }
    };

    let Some(suspend_until) = parse_probe_circuit_suspend_until(raw_suspend_until.as_deref())
    else {
        return true;
    };
    if suspend_until > now_unix_secs {
        return false;
    }

    if let Err(error) = state.runtime_state.kv_delete(&suspend_key).await {
        warn!(
            provider_id,
            error = ?error,
            "gateway pool preheat probe provider circuit cleanup failed"
        );
    }
    true
}

async fn record_probe_outcome_for_circuit(
    state: &AppState,
    provider_id: &str,
    outcome: &OauthKeyProbeOutcome,
    pool_config: &AdminProviderPoolConfig,
    now_unix_secs: u64,
) {
    let samples_key = pool_preheat_provider_circuit_samples_key(provider_id);
    let window_seconds = pool_config.pre_probe.circuit_sample_window_seconds.max(1);
    let window_start = now_unix_secs.saturating_sub(window_seconds) as f64;
    if let Err(error) = state
        .runtime_state
        .score_remove_by_score(&samples_key, window_start)
        .await
    {
        warn!(
            provider_id,
            error = ?error,
            "gateway pool preheat probe provider circuit sample trim failed"
        );
        return;
    }

    let failed = probe_circuit_outcome_is_failure(outcome);
    let member = probe_circuit_sample_member(failed, now_unix_secs);
    if let Err(error) = state
        .runtime_state
        .score_set(&samples_key, &member, now_unix_secs as f64)
        .await
    {
        warn!(
            provider_id,
            error = ?error,
            "gateway pool preheat probe provider circuit sample write failed"
        );
        return;
    }

    let samples = match state
        .runtime_state
        .score_range_by_min(&samples_key, window_start)
        .await
    {
        Ok(samples) => samples,
        Err(error) => {
            warn!(
                provider_id,
                error = ?error,
                "gateway pool preheat probe provider circuit sample read failed"
            );
            return;
        }
    };
    let failed_samples = samples
        .iter()
        .filter(|member| probe_circuit_sample_member_is_failure(member))
        .count();
    let total_samples = samples.len();
    let _ = state
        .runtime_state
        .key_expire(
            &samples_key,
            Duration::from_secs(
                window_seconds.saturating_add(pool_config.pre_probe.circuit_suspend_seconds.max(1)),
            ),
        )
        .await;

    if total_samples < probe_circuit_sample_threshold(pool_config)
        || !probe_circuit_failure_rate_exceeds_threshold(
            failed_samples,
            total_samples,
            pool_config.pre_probe.circuit_failure_rate_threshold,
        )
    {
        return;
    }

    let suspend_seconds = pool_config.pre_probe.circuit_suspend_seconds.max(1);
    let suspend_until = now_unix_secs.saturating_add(suspend_seconds);
    if let Err(error) = state
        .runtime_state
        .kv_set(
            &pool_preheat_provider_circuit_suspend_key(provider_id),
            suspend_until.to_string(),
            Some(Duration::from_secs(suspend_seconds)),
        )
        .await
    {
        warn!(
            provider_id,
            error = ?error,
            "gateway pool preheat probe provider circuit suspend write failed"
        );
    }
}

fn parse_preheat_last_stamp(raw_value: Option<&str>) -> Option<u64> {
    raw_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
}

fn preheat_last_stamp_is_recent(stamp: u64, now_unix_secs: u64, dedup_window_secs: u64) -> bool {
    stamp >= now_unix_secs || now_unix_secs.saturating_sub(stamp) <= dedup_window_secs.max(1)
}

fn count_pool_preheat_dedup_hits(
    keys: &[StoredProviderCatalogKey],
    deduped_key_ids: &BTreeSet<String>,
) -> usize {
    keys.iter()
        .filter(|key| deduped_key_ids.contains(&key.id))
        .count()
}

fn pool_preheat_probe_needs_provider_probe(
    keys: &[StoredProviderCatalogKey],
    required_healthy: usize,
    deduped_key_ids: &BTreeSet<String>,
) -> bool {
    count_pool_preheat_dedup_hits(keys, deduped_key_ids) < required_healthy.max(1)
        && keys.iter().any(|key| !deduped_key_ids.contains(&key.id))
}

async fn acquire_pool_preheat_probe_group_lock(
    state: &AppState,
    request: &PoolPreheatProbeTaskRequest,
    pool_config: &AdminProviderPoolConfig,
) -> Result<Option<RuntimeLockLease>, ()> {
    match state
        .runtime_state
        .lock_try_acquire(
            &pool_preheat_probe_group_lock_key(request),
            state.tunnel.local_instance_id(),
            Duration::from_secs(pool_config.pre_probe.group_lock_ttl_seconds.max(1)),
        )
        .await
    {
        Ok(lease) => Ok(lease),
        Err(error) => {
            warn!(
                provider_id = %request.provider_id,
                pool_group_id = %request.pool_group_id,
                error = ?error,
                "gateway pool preheat probe group lock unavailable; failing open"
            );
            Err(())
        }
    }
}

async fn release_pool_preheat_probe_group_lock(state: &AppState, lease: Option<RuntimeLockLease>) {
    let Some(lease) = lease else {
        return;
    };
    if let Err(error) = state.runtime_state.lock_release(&lease).await {
        warn!(
            error = ?error,
            "gateway pool preheat probe group lock release failed"
        );
    }
}

async fn load_recent_preheat_dedup_hits(
    state: &AppState,
    provider_id: &str,
    keys: &[StoredProviderCatalogKey],
    now_unix_secs: u64,
    dedup_window_secs: u64,
) -> (BTreeSet<String>, bool) {
    if keys.is_empty() {
        return (BTreeSet::new(), false);
    }

    let runtime_keys = keys
        .iter()
        .map(|key| preheat_last_stamp_key(provider_id, &key.id))
        .collect::<Vec<_>>();
    let values = match state.runtime_state.kv_get_many(&runtime_keys).await {
        Ok(values) => values,
        Err(error) => {
            warn!(
                provider_id,
                error = ?error,
                "gateway pool preheat probe dedup read unavailable; failing open"
            );
            return (BTreeSet::new(), true);
        }
    };

    let deduped = keys
        .iter()
        .zip(values)
        .filter_map(|(key, raw)| {
            let stamp = parse_preheat_last_stamp(raw.as_deref())?;
            preheat_last_stamp_is_recent(stamp, now_unix_secs, dedup_window_secs)
                .then(|| key.id.clone())
        })
        .collect::<BTreeSet<_>>();
    (deduped, false)
}

fn pool_preheat_provider_rate_limit_decision_from_result(
    result: Result<RateLimitCheck, DataLayerError>,
) -> PoolPreheatProviderRateLimitDecision {
    match result {
        Ok(RateLimitCheck::Allowed { remaining }) => {
            PoolPreheatProviderRateLimitDecision::Allowed {
                remaining: Some(remaining),
                fail_open: false,
            }
        }
        Ok(RateLimitCheck::Rejected { scope, limit }) => {
            PoolPreheatProviderRateLimitDecision::Rejected { scope, limit }
        }
        Err(_) => PoolPreheatProviderRateLimitDecision::Allowed {
            remaining: None,
            fail_open: true,
        },
    }
}

async fn check_pool_preheat_provider_rate_limit(
    state: &AppState,
    provider_id: &str,
    limit_per_minute: u32,
    now_unix_secs: u64,
) -> PoolPreheatProviderRateLimitDecision {
    if limit_per_minute == 0 {
        return PoolPreheatProviderRateLimitDecision::Allowed {
            remaining: None,
            fail_open: false,
        };
    }

    let user_key = pool_preheat_provider_rate_limit_user_key(provider_id);
    let key_key = pool_preheat_provider_rate_limit_key_key(provider_id);
    let result = state
        .runtime_state
        .check_and_consume_rate_limit(RateLimitInput {
            user_key: &user_key,
            key_key: &key_key,
            bucket: pool_preheat_provider_rate_limit_bucket(now_unix_secs),
            user_limit: limit_per_minute,
            key_limit: 0,
            ttl_seconds: POOL_PREHEAT_PROVIDER_RATE_LIMIT_WINDOW_SECS,
        })
        .await;
    if let Err(error) = &result {
        warn!(
            provider_id,
            error = ?error,
            "gateway pool preheat probe provider rate limit unavailable; failing open"
        );
    }
    pool_preheat_provider_rate_limit_decision_from_result(result)
}

fn default_pool_preheat_config_value() -> Value {
    json!({ "pool_advanced": {} })
}

fn pool_preheat_probe_pool_config(
    provider: &StoredProviderCatalogProvider,
) -> AdminProviderPoolConfig {
    admin_provider_pool_config_from_config_value(provider.config.as_ref())
        .or_else(|| {
            let default_config = default_pool_preheat_config_value();
            admin_provider_pool_config_from_config_value(Some(&default_config))
        })
        .expect("default pool config should parse")
}

async fn candidate_key_ids_for_group(
    state: &AppState,
    request: &PoolPreheatProbeTaskRequest,
    required_healthy: usize,
) -> Result<Vec<String>, GatewayError> {
    if !request.top_n_key_ids.is_empty() {
        return Ok(request.top_n_key_ids.clone());
    }
    if !state.has_minimal_candidate_selection_reader() {
        return Ok(Vec::new());
    }

    let Some(context) = parse_pool_group_context(&request.pool_group_id) else {
        return Ok(Vec::new());
    };
    if context.provider_id != request.provider_id {
        return Ok(Vec::new());
    }

    let rows = state
        .list_pool_key_candidate_rows_for_group(&StoredPoolKeyCandidateRowsQuery {
            api_format: context.api_format,
            provider_id: context.provider_id,
            endpoint_id: context.endpoint_id,
            model_id: context.model_id,
            selected_provider_model_name: context.selected_provider_model_name,
            offset: 0,
            limit: required_healthy.max(1) as u32,
        })
        .await?;

    Ok(normalize_pool_preheat_probe_key_ids(
        rows.into_iter().map(|row| row.key_id).collect(),
    ))
}

fn endpoint_for_preheat_probe(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Option<StoredProviderCatalogEndpoint> {
    provider_oauth_runtime_endpoint_for_provider(provider_type, endpoints)
}

async fn load_pool_preheat_probe_context(
    state: &AppState,
    request: &PoolPreheatProbeTaskRequest,
) -> Result<
    Option<(
        StoredProviderCatalogProvider,
        StoredProviderCatalogEndpoint,
        Vec<StoredProviderCatalogKey>,
        usize,
        AdminProviderPoolConfig,
    )>,
    GatewayError,
> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(None);
    }

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&request.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };

    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !pool_preheat_probe_provider_is_supported(&provider_type) {
        return Ok(None);
    }

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let Some(endpoint) = endpoint_for_preheat_probe(&provider_type, &endpoints) else {
        return Ok(None);
    };

    let pool_config = pool_preheat_probe_pool_config(&provider);
    let required_healthy = pool_config.pre_probe.required_healthy as usize;

    let candidate_key_ids = candidate_key_ids_for_group(state, request, required_healthy).await?;
    if candidate_key_ids.is_empty() {
        return Ok(Some((provider, endpoint, Vec::new(), 0, pool_config)));
    }

    let mut keys_by_id = state
        .list_provider_catalog_keys_by_ids(&candidate_key_ids)
        .await?
        .into_iter()
        .filter(|key| key.provider_id == provider.id && key.is_active)
        .map(|key| (key.id.clone(), key))
        .collect::<BTreeMap<_, _>>();
    let keys = candidate_key_ids
        .iter()
        .filter_map(|key_id| keys_by_id.remove(key_id))
        .collect::<Vec<_>>();
    Ok(Some((
        provider,
        endpoint,
        keys,
        candidate_key_ids.len(),
        pool_config,
    )))
}

async fn classify_pool_preheat_probe_keys<F, Fut, W, WriteFut>(
    request: &PoolPreheatProbeTaskRequest,
    requested_keys: usize,
    keys: Vec<StoredProviderCatalogKey>,
    required_healthy: usize,
    mut classify_key: F,
    mut write_outcome: W,
) -> PoolPreheatProbeRunSummary
where
    F: FnMut(StoredProviderCatalogKey) -> Fut,
    Fut: Future<Output = Result<OauthKeyProbeOutcome, GatewayError>>,
    W: FnMut(String, OauthKeyProbeOutcome) -> WriteFut,
    WriteFut: Future<Output = Result<(), GatewayError>>,
{
    classify_pool_preheat_probe_keys_with_dedup_observed(
        request,
        requested_keys,
        keys,
        required_healthy,
        &BTreeSet::new(),
        &mut classify_key,
        &mut write_outcome,
        |_key_id, _classification, _duration_ms| {},
        |_key_id, _trigger_error_kind, _error, _duration_ms| {},
    )
    .await
}

async fn classify_pool_preheat_probe_keys_with_dedup<F, Fut, W, WriteFut>(
    request: &PoolPreheatProbeTaskRequest,
    requested_keys: usize,
    keys: Vec<StoredProviderCatalogKey>,
    required_healthy: usize,
    deduped_key_ids: &BTreeSet<String>,
    mut classify_key: F,
    mut write_outcome: W,
) -> PoolPreheatProbeRunSummary
where
    F: FnMut(StoredProviderCatalogKey) -> Fut,
    Fut: Future<Output = Result<OauthKeyProbeOutcome, GatewayError>>,
    W: FnMut(String, OauthKeyProbeOutcome) -> WriteFut,
    WriteFut: Future<Output = Result<(), GatewayError>>,
{
    classify_pool_preheat_probe_keys_with_dedup_observed(
        request,
        requested_keys,
        keys,
        required_healthy,
        deduped_key_ids,
        &mut classify_key,
        &mut write_outcome,
        |_key_id, _classification, _duration_ms| {},
        |_key_id, _trigger_error_kind, _error, _duration_ms| {},
    )
    .await
}

async fn classify_pool_preheat_probe_keys_with_dedup_observed<
    F,
    Fut,
    W,
    WriteFut,
    ObserveOutcome,
    ObserveError,
>(
    request: &PoolPreheatProbeTaskRequest,
    requested_keys: usize,
    keys: Vec<StoredProviderCatalogKey>,
    required_healthy: usize,
    deduped_key_ids: &BTreeSet<String>,
    mut classify_key: F,
    mut write_outcome: W,
    mut observe_outcome: ObserveOutcome,
    mut observe_error: ObserveError,
) -> PoolPreheatProbeRunSummary
where
    F: FnMut(StoredProviderCatalogKey) -> Fut,
    Fut: Future<Output = Result<OauthKeyProbeOutcome, GatewayError>>,
    W: FnMut(String, OauthKeyProbeOutcome) -> WriteFut,
    WriteFut: Future<Output = Result<(), GatewayError>>,
    ObserveOutcome: FnMut(&str, OauthKeyProbeClassification, u64),
    ObserveError: FnMut(&str, &'static str, &GatewayError, u64),
{
    let mut summary = PoolPreheatProbeRunSummary::selected(request, requested_keys, keys.len());
    if keys.is_empty() {
        summary.skipped_reason = Some("no candidate keys to probe".to_string());
        return summary;
    }

    let required_healthy = required_healthy.max(1);
    summary.deduped_keys = count_pool_preheat_dedup_hits(&keys, deduped_key_ids);
    summary.healthy = summary.deduped_keys;
    if summary.healthy >= required_healthy {
        summary.stopped_after_healthy = true;
        return summary;
    }

    for key in keys {
        let key_id = key.id.clone();
        if deduped_key_ids.contains(&key_id) {
            continue;
        }
        let probe_started_at = Instant::now();
        match classify_key(key).await {
            Ok(outcome) => {
                let duration_ms = elapsed_millis_u64(probe_started_at);
                let classification = outcome.classification();
                observe_outcome(key_id.as_str(), classification, duration_ms);
                summary.probed_keys += 1;
                match classification {
                    OauthKeyProbeClassification::Healthy => summary.healthy += 1,
                    OauthKeyProbeClassification::OAuthInvalid => summary.oauth_invalid += 1,
                    OauthKeyProbeClassification::AccountBlocked => summary.account_blocked += 1,
                    OauthKeyProbeClassification::RateLimited => summary.rate_limited += 1,
                    OauthKeyProbeClassification::ServerError => summary.server_error += 1,
                    OauthKeyProbeClassification::TransportError => summary.transport_error += 1,
                }
                if let Err(error) = write_outcome(key_id.clone(), outcome).await {
                    summary.errored += 1;
                    observe_error(
                        key_id.as_str(),
                        "outcome_writer_failed",
                        &error,
                        duration_ms,
                    );
                }
                if summary.healthy >= required_healthy {
                    summary.stopped_after_healthy = true;
                    break;
                }
            }
            Err(error) => {
                let duration_ms = elapsed_millis_u64(probe_started_at);
                summary.probed_keys += 1;
                summary.errored += 1;
                observe_error(key_id.as_str(), "classifier_failed", &error, duration_ms);
            }
        }
    }

    summary
}

fn preheat_last_stamp_key(provider_id: &str, key_id: &str) -> String {
    format!("ap:preheat:last:{provider_id}:{key_id}")
}

fn preheat_failure_streak_key(provider_id: &str, key_id: &str) -> String {
    format!("ap:preheat:5xx_streak:{provider_id}:{key_id}")
}

fn outcome_payload_string(outcome: &OauthKeyProbeOutcome, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        outcome
            .payload()
            .get(*field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn outcome_status_code(outcome: &OauthKeyProbeOutcome) -> u16 {
    if let Some(status_code) = outcome
        .payload()
        .get("status_code")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
    {
        return status_code;
    }

    match outcome.classification() {
        OauthKeyProbeClassification::Healthy => 200,
        OauthKeyProbeClassification::OAuthInvalid => 401,
        OauthKeyProbeClassification::AccountBlocked => 403,
        OauthKeyProbeClassification::RateLimited => 429,
        OauthKeyProbeClassification::ServerError => 500,
        OauthKeyProbeClassification::TransportError => 502,
    }
}

fn oauth_invalid_reason_for_outcome(outcome: &OauthKeyProbeOutcome) -> String {
    format!(
        "{OAUTH_EXPIRED_PREFIX}{}",
        circuit_reason_for_outcome(outcome)
    )
}

fn account_blocked_reason_for_outcome(outcome: &OauthKeyProbeOutcome) -> String {
    circuit_reason_for_outcome(outcome)
}

fn circuit_reason_for_outcome(outcome: &OauthKeyProbeOutcome) -> String {
    let status_code = outcome_status_code(outcome);
    match outcome.classification() {
        OauthKeyProbeClassification::OAuthInvalid => format!("oauth_invalid_{status_code}"),
        OauthKeyProbeClassification::AccountBlocked => format!("account_blocked_{status_code}"),
        OauthKeyProbeClassification::ServerError => format!("server_error_{status_code}"),
        OauthKeyProbeClassification::TransportError => format!("transport_error_{status_code}"),
        OauthKeyProbeClassification::Healthy | OauthKeyProbeClassification::RateLimited => {
            format!("preheat_probe_{status_code}")
        }
    }
}

fn cooldown_pool_config(pool_config: &AdminProviderPoolConfig) -> AdminProviderPoolConfig {
    let mut cooldown_config = pool_config.clone();
    cooldown_config.health_policy_enabled = true;
    cooldown_config.rate_limit_cooldown_seconds = POOL_PREHEAT_DEFAULT_COOLDOWN_SECS;
    cooldown_config.overload_cooldown_seconds = POOL_PREHEAT_DEFAULT_COOLDOWN_SECS;
    cooldown_config
}

async fn write_preheat_last_stamp(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
    now_unix_secs: u64,
    ttl_seconds: u64,
) -> Result<(), GatewayError> {
    state
        .runtime_state
        .kv_set(
            &preheat_last_stamp_key(provider_id, key_id),
            now_unix_secs.to_string(),
            Some(Duration::from_secs(ttl_seconds.max(1))),
        )
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

async fn reset_preheat_failure_streak(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
) -> Result<(), GatewayError> {
    state
        .runtime_state
        .kv_delete(&preheat_failure_streak_key(provider_id, key_id))
        .await
        .map(|_| ())
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

async fn increment_preheat_failure_streak(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
    ttl_seconds: u64,
) -> Result<u64, GatewayError> {
    let streak_key = preheat_failure_streak_key(provider_id, key_id);
    let current = state
        .runtime_state
        .kv_get(&streak_key)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let next = current.saturating_add(1);
    state
        .runtime_state
        .kv_set(
            &streak_key,
            next.to_string(),
            Some(Duration::from_secs(ttl_seconds.max(1))),
        )
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(next)
}

async fn record_preheat_runtime_cooldown(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
    pool_config: &AdminProviderPoolConfig,
    status_code: u16,
) {
    let cooldown_config = cooldown_pool_config(pool_config);
    record_admin_provider_pool_error(
        state.runtime_state.as_ref(),
        provider_id,
        key_id,
        &cooldown_config,
        status_code,
        None,
        None,
    )
    .await;
}

async fn select_one_backfill_row(
    state: &AppState,
    cache_key: &PoolCandidateCacheKey,
    mut offset: u32,
    excluded_key_ids: &BTreeSet<String>,
) -> Result<
    Option<(
        aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow,
        u32,
    )>,
    GatewayError,
> {
    if !state.has_minimal_candidate_selection_reader() {
        return Ok(None);
    }

    loop {
        let rows = state
            .list_pool_key_candidate_rows_for_group(&StoredPoolKeyCandidateRowsQuery {
                api_format: cache_key.api_format.clone(),
                provider_id: cache_key.provider_id.clone(),
                endpoint_id: cache_key.endpoint_id.clone(),
                model_id: cache_key.model_id.clone(),
                selected_provider_model_name: cache_key.selected_provider_model_name.clone(),
                offset,
                limit: POOL_PREHEAT_BACKFILL_SCAN_LIMIT,
            })
            .await?;
        if rows.is_empty() {
            return Ok(None);
        }

        let row_count = rows.len() as u32;
        for (index, row) in rows.into_iter().enumerate() {
            let next_offset = offset.saturating_add(index as u32).saturating_add(1);
            if !excluded_key_ids.contains(&row.key_id) {
                return Ok(Some((row, next_offset)));
            }
        }
        offset = offset.saturating_add(row_count);
    }
}

async fn prepare_probe_cache_eviction_and_backfill(
    state: &AppState,
    cache_key: &PoolCandidateCacheKey,
    key_id: &str,
) -> Result<Option<CachedPoolCandidates>, GatewayError> {
    let current_config_version = state.effective_pool_candidate_config_version().await;
    let Some(cached) = state.pool_candidate_cache().get_fresh(
        cache_key,
        POOL_PREHEAT_CANDIDATE_CACHE_TTL,
        current_config_version,
    ) else {
        state
            .pool_preheat_metrics()
            .record_candidate_cache_operation("miss");
        return Ok(None);
    };
    state
        .pool_preheat_metrics()
        .record_candidate_cache_operation("hit");

    let excluded_key_ids = cached
        .rows
        .iter()
        .map(|row| row.key_id.clone())
        .collect::<BTreeSet<_>>();
    let mut rows = cached.rows;
    let original_len = rows.len();
    rows.retain(|row| row.key_id != key_id);
    if rows.len() == original_len {
        return Ok(None);
    }
    state
        .pool_preheat_metrics()
        .record_candidate_cache_operation("evict");

    let mut next_db_offset = cached.next_db_offset;
    if rows.len() < POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS {
        if let Some((backfill, next_offset)) =
            select_one_backfill_row(state, cache_key, next_db_offset, &excluded_key_ids).await?
        {
            rows.push(backfill);
            next_db_offset = next_offset;
            state
                .pool_preheat_metrics()
                .record_candidate_cache_operation("backfill");
        }
    }

    Ok(Some(CachedPoolCandidates::new(
        rows,
        current_config_version,
        next_db_offset,
    )))
}

async fn prepare_probe_cache_preserve_current(
    state: &AppState,
    cache_key: &PoolCandidateCacheKey,
) -> Result<Option<CachedPoolCandidates>, GatewayError> {
    let current_config_version = state.effective_pool_candidate_config_version().await;
    let cached = state.pool_candidate_cache().get_fresh(
        cache_key,
        POOL_PREHEAT_CANDIDATE_CACHE_TTL,
        current_config_version,
    );
    state
        .pool_preheat_metrics()
        .record_candidate_cache_operation(if cached.is_some() { "hit" } else { "miss" });
    Ok(cached)
}

async fn commit_prepared_probe_cache(
    state: &AppState,
    cache_key: &PoolCandidateCacheKey,
    prepared: CachedPoolCandidates,
    max_expected_version_bumps: u64,
) {
    let current_config_version = state.effective_pool_candidate_config_version().await;
    if current_config_version
        > prepared
            .config_version
            .saturating_add(max_expected_version_bumps)
    {
        state
            .pool_preheat_metrics()
            .record_candidate_cache_operation("invalidate");
        return;
    }
    state.pool_candidate_cache().insert(
        cache_key.clone(),
        CachedPoolCandidates::new(
            prepared.rows,
            current_config_version,
            prepared.next_db_offset,
        ),
        POOL_PREHEAT_CANDIDATE_CACHE_TTL,
        POOL_PREHEAT_CANDIDATE_CACHE_MAX_ENTRIES,
        POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS,
    );
}

async fn open_probe_key_circuit(
    state: &AppState,
    cache_key: &PoolCandidateCacheKey,
    key_id: &str,
    reason: &str,
    now_unix_secs: u64,
) -> Result<(), GatewayError> {
    let Some(current_key) = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    let Some(circuit_breaker_by_format) = project_local_key_circuit_open(
        current_key.circuit_breaker_by_format.as_ref(),
        &cache_key.api_format,
        reason,
        now_unix_secs,
    ) else {
        return Ok(());
    };

    state
        .update_provider_catalog_key_health_state(
            key_id,
            current_key.is_active,
            current_key.health_by_format.as_ref(),
            Some(&circuit_breaker_by_format),
        )
        .await?;
    Ok(())
}

async fn degrade_probe_key_health(
    state: &AppState,
    cache_key: &PoolCandidateCacheKey,
    key_id: &str,
    outcome: &OauthKeyProbeOutcome,
    now_unix_secs: u64,
) -> Result<(), GatewayError> {
    let Some(current_key) = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    let Some(health_by_format) = project_local_failure_health(
        current_key.health_by_format.as_ref(),
        &cache_key.api_format,
        LocalFailoverClassification::RetryUpstreamFailure,
        outcome_status_code(outcome),
        now_unix_secs,
    ) else {
        return Ok(());
    };

    state
        .update_provider_catalog_key_format_health(key_id, &cache_key.api_format, &health_by_format)
        .await?;
    Ok(())
}

fn merge_account_blocked_metadata(
    current: Option<Value>,
    provider_type: &str,
    outcome: &OauthKeyProbeOutcome,
    now_unix_secs: u64,
) -> Value {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let reason = account_blocked_reason_for_outcome(outcome);
    let status = outcome_payload_string(outcome, &["status"]).unwrap_or_default();
    let mut root = current
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut bucket = root
        .get(&provider_type)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    bucket.insert("account_blocked".to_string(), json!(true));
    bucket.insert("account_blocked_reason".to_string(), json!(reason));
    bucket.insert("account_blocked_at".to_string(), json!(now_unix_secs));
    bucket.insert("updated_at".to_string(), json!(now_unix_secs));
    bucket.insert("source".to_string(), json!("preheat_probe"));
    if status.eq_ignore_ascii_case("banned") {
        bucket.insert("is_banned".to_string(), json!(true));
        bucket.insert("ban_reason".to_string(), json!(reason));
        bucket.insert("banned_at".to_string(), json!(now_unix_secs));
    } else {
        bucket.insert("is_forbidden".to_string(), json!(true));
        bucket.insert("forbidden_reason".to_string(), json!(reason));
        bucket.insert("forbidden_at".to_string(), json!(now_unix_secs));
    }
    root.insert(provider_type, Value::Object(bucket));
    Value::Object(root)
}

async fn mark_probe_account_blocked(
    state: &AppState,
    provider_type: &str,
    key_id: &str,
    outcome: &OauthKeyProbeOutcome,
    now_unix_secs: u64,
) -> Result<(), GatewayError> {
    let Some(mut key) = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(());
    };
    let reason = account_blocked_reason_for_outcome(outcome);
    key.upstream_metadata = Some(merge_account_blocked_metadata(
        key.upstream_metadata.take(),
        provider_type,
        outcome,
        now_unix_secs,
    ));
    key.oauth_invalid_at_unix_secs = Some(now_unix_secs);
    key.oauth_invalid_reason = Some(format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{reason}"));
    key.status_snapshot = Some(provider_key_status_snapshot_payload(&key, provider_type));
    key.updated_at_unix_secs = Some(now_unix_secs);
    state.update_provider_catalog_key(&key).await?;
    Ok(())
}

async fn apply_probe_outcome(
    state: &AppState,
    cache_key: Option<&PoolCandidateCacheKey>,
    key_id: &str,
    provider_type: &str,
    outcome: &OauthKeyProbeOutcome,
    now_unix_secs: u64,
    pool_config: &AdminProviderPoolConfig,
) -> Result<(), GatewayError> {
    let (prepared_cache, max_expected_cache_version_bumps) = match outcome.classification() {
        OauthKeyProbeClassification::OAuthInvalid => match cache_key {
            Some(cache_key) => (
                prepare_probe_cache_eviction_and_backfill(state, cache_key, key_id).await?,
                2,
            ),
            None => (None, 0),
        },
        OauthKeyProbeClassification::AccountBlocked => match cache_key {
            Some(cache_key) => (
                prepare_probe_cache_eviction_and_backfill(state, cache_key, key_id).await?,
                1,
            ),
            None => (None, 0),
        },
        OauthKeyProbeClassification::ServerError | OauthKeyProbeClassification::TransportError => {
            match cache_key {
                Some(cache_key) => (
                    prepare_probe_cache_preserve_current(state, cache_key).await?,
                    1,
                ),
                None => (None, 0),
            }
        }
        _ => (None, 0),
    };

    match outcome.classification() {
        OauthKeyProbeClassification::Healthy => {
            if let Some(cache_key) = cache_key {
                write_preheat_last_stamp(
                    state,
                    &cache_key.provider_id,
                    key_id,
                    now_unix_secs,
                    pool_config.pre_probe.dedup_window_secs,
                )
                .await?;
                reset_preheat_failure_streak(state, &cache_key.provider_id, key_id).await?;
            }
        }
        OauthKeyProbeClassification::OAuthInvalid => {
            let invalid_reason = oauth_invalid_reason_for_outcome(outcome);
            state
                .mark_provider_catalog_key_oauth_invalid(key_id, provider_type, &invalid_reason)
                .await?;
            if let Some(cache_key) = cache_key {
                open_probe_key_circuit(
                    state,
                    cache_key,
                    key_id,
                    &circuit_reason_for_outcome(outcome),
                    now_unix_secs,
                )
                .await?;
                reset_preheat_failure_streak(state, &cache_key.provider_id, key_id).await?;
            }
        }
        OauthKeyProbeClassification::AccountBlocked => {
            mark_probe_account_blocked(state, provider_type, key_id, outcome, now_unix_secs)
                .await?;
            if let Some(cache_key) = cache_key {
                reset_preheat_failure_streak(state, &cache_key.provider_id, key_id).await?;
            }
        }
        OauthKeyProbeClassification::RateLimited => {
            if let Some(cache_key) = cache_key {
                reset_preheat_failure_streak(state, &cache_key.provider_id, key_id).await?;
                record_preheat_runtime_cooldown(
                    state,
                    &cache_key.provider_id,
                    key_id,
                    pool_config,
                    POOL_PREHEAT_RATE_LIMIT_COOLDOWN_STATUS_CODE,
                )
                .await;
            }
        }
        OauthKeyProbeClassification::ServerError | OauthKeyProbeClassification::TransportError => {
            if let Some(cache_key) = cache_key {
                degrade_probe_key_health(state, cache_key, key_id, outcome, now_unix_secs).await?;
                let streak = increment_preheat_failure_streak(
                    state,
                    &cache_key.provider_id,
                    key_id,
                    pool_config.pre_probe.circuit_sample_window_seconds,
                )
                .await?;
                if streak >= pool_config.pre_probe.five_xx_streak_threshold.max(1) as u64 {
                    record_preheat_runtime_cooldown(
                        state,
                        &cache_key.provider_id,
                        key_id,
                        pool_config,
                        outcome_status_code(outcome),
                    )
                    .await;
                }
            }
        }
    }

    if let (Some(cache_key), Some(prepared_cache)) = (cache_key, prepared_cache) {
        commit_prepared_probe_cache(
            state,
            cache_key,
            prepared_cache,
            max_expected_cache_version_bumps,
        )
        .await;
    }
    Ok(())
}

pub(crate) async fn perform_pool_preheat_probe(
    state: &AppState,
    request: PoolPreheatProbeTaskRequest,
) -> Result<PoolPreheatProbeRunSummary, GatewayError> {
    let trigger = pool_preheat_probe_trigger_label(&request);
    log_pool_preheat_probe_run_started(&request, trigger);

    let Some((provider, endpoint, keys, requested_keys, pool_config)) =
        load_pool_preheat_probe_context(state, &request).await?
    else {
        let summary = PoolPreheatProbeRunSummary::skipped(
            &request,
            "missing or unsupported provider context",
        );
        return Ok(complete_pool_preheat_probe_run(
            state, &request, "unknown", trigger, summary,
        ));
    };
    let provider_type_label = pool_preheat_probe_provider_type_label(&provider.provider_type);

    let now_unix_secs = current_unix_secs();
    if !probe_circuit_allows_provider(state, &provider.id, &pool_config, now_unix_secs).await {
        state
            .pool_preheat_metrics()
            .record_circuit_suspended(provider_type_label.as_str());
        let summary = PoolPreheatProbeRunSummary::skipped_selected(
            &request,
            requested_keys,
            keys.len(),
            0,
            0,
            "provider probe circuit open",
        );
        return Ok(complete_pool_preheat_probe_run(
            state,
            &request,
            provider_type_label.as_str(),
            trigger,
            summary,
        ));
    }

    let required_healthy = pool_config.pre_probe.required_healthy as usize;
    let (group_lock, mut coordination_fail_open) =
        match acquire_pool_preheat_probe_group_lock(state, &request, &pool_config).await {
            Ok(Some(lease)) => (Some(lease), 0),
            Ok(None) => {
                let summary = PoolPreheatProbeRunSummary::skipped_selected(
                    &request,
                    requested_keys,
                    keys.len(),
                    0,
                    0,
                    "group lock already held",
                );
                return Ok(complete_pool_preheat_probe_run(
                    state,
                    &request,
                    provider_type_label.as_str(),
                    trigger,
                    summary,
                ));
            }
            Err(()) => (None, 1),
        };

    let (deduped_key_ids, dedup_fail_open) = load_recent_preheat_dedup_hits(
        state,
        &provider.id,
        &keys,
        now_unix_secs,
        pool_config.pre_probe.dedup_window_secs,
    )
    .await;
    if dedup_fail_open {
        coordination_fail_open += 1;
    }

    let mut rate_limit_remaining = None;
    if pool_preheat_probe_needs_provider_probe(&keys, required_healthy, &deduped_key_ids) {
        match check_pool_preheat_provider_rate_limit(
            state,
            &provider.id,
            pool_config.pre_probe.per_provider_rate_limit_per_minute,
            now_unix_secs,
        )
        .await
        {
            PoolPreheatProviderRateLimitDecision::Allowed {
                remaining,
                fail_open,
            } => {
                rate_limit_remaining = remaining;
                if fail_open {
                    coordination_fail_open += 1;
                }
            }
            PoolPreheatProviderRateLimitDecision::Rejected { scope, limit } => {
                state
                    .pool_preheat_metrics()
                    .record_rate_limit_rejected(provider_type_label.as_str());
                let deduped_keys = count_pool_preheat_dedup_hits(&keys, &deduped_key_ids);
                let mut summary = PoolPreheatProbeRunSummary::skipped_selected(
                    &request,
                    requested_keys,
                    keys.len(),
                    deduped_keys,
                    coordination_fail_open,
                    format!(
                        "provider rate limit exceeded: {} limit {limit}/min",
                        pool_preheat_provider_rate_limit_scope_label(scope)
                    ),
                );
                summary.rate_limited = 1;
                release_pool_preheat_probe_group_lock(state, group_lock).await;
                return Ok(complete_pool_preheat_probe_run(
                    state,
                    &request,
                    provider_type_label.as_str(),
                    trigger,
                    summary,
                ));
            }
        }
    }

    let admin_state = AdminAppState::new(state);
    let cache_key = pool_preheat_probe_cache_key(&request);
    let probe_timeout = Duration::from_secs(pool_config.pre_probe.probe_timeout_seconds.max(1));
    let mut summary = classify_pool_preheat_probe_keys_with_dedup_observed(
        &request,
        requested_keys,
        keys,
        required_healthy,
        &deduped_key_ids,
        |key| {
            let admin_state = &admin_state;
            let provider = &provider;
            let endpoint = &endpoint;
            async move {
                probe_oauth_provider_key_with_classification(
                    admin_state,
                    provider,
                    endpoint,
                    key,
                    None,
                    probe_timeout,
                    false,
                )
                .await
            }
        },
        |key_id, outcome| {
            let cache_key = cache_key.clone();
            let provider_id = provider.id.clone();
            let provider_type = provider.provider_type.clone();
            let pool_config = pool_config.clone();
            async move {
                let now_unix_secs = current_unix_secs();
                record_probe_outcome_for_circuit(
                    state,
                    provider_id.as_str(),
                    &outcome,
                    &pool_config,
                    now_unix_secs,
                )
                .await;
                apply_probe_outcome(
                    state,
                    cache_key.as_ref(),
                    key_id.as_str(),
                    provider_type.as_str(),
                    &outcome,
                    now_unix_secs,
                    &pool_config,
                )
                .await
            }
        },
        |key_id, classification, duration_ms| {
            state.pool_preheat_metrics().record_probe_outcome(
                provider_type_label.as_str(),
                pool_preheat_probe_outcome_kind(classification),
            );
            log_pool_preheat_probe_outcome(
                &request,
                provider_type_label.as_str(),
                key_id,
                classification,
                duration_ms,
            );
        },
        |key_id, trigger_error_kind, error, duration_ms| {
            state
                .pool_preheat_metrics()
                .record_probe_outcome(provider_type_label.as_str(), "probe_error");
            log_pool_preheat_probe_error(
                &request,
                provider_type_label.as_str(),
                key_id,
                trigger_error_kind,
                error,
                duration_ms,
            );
        },
    )
    .await;
    summary.coordination_fail_open = coordination_fail_open;
    summary.rate_limit_remaining = rate_limit_remaining;
    release_pool_preheat_probe_group_lock(state, group_lock).await;
    Ok(complete_pool_preheat_probe_run(
        state,
        &request,
        provider_type_label.as_str(),
        trigger,
        summary,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use serde_json::json;

    use crate::data::GatewayDataState;

    fn probe_outcome(classification: OauthKeyProbeClassification) -> OauthKeyProbeOutcome {
        OauthKeyProbeOutcome::from_payload(
            classification,
            classification == OauthKeyProbeClassification::Healthy,
            false,
            false,
            json!({ "status": format!("{:?}", classification).to_ascii_lowercase() }),
        )
    }

    fn key(id: &str, provider_id: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            id.to_string(),
            provider_id.to_string(),
            id.to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
    }

    fn provider_with_pre_probe_config(
        provider_type: &str,
        pre_probe: Value,
    ) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            provider_type.to_string(),
            Some("https://example.com".to_string()),
            provider_type.to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(1),
            None,
            None,
            None,
            Some(json!({ "pool_advanced": { "pre_probe": pre_probe } })),
        )
    }

    fn provider(provider_type: &str) -> StoredProviderCatalogProvider {
        provider_with_pre_probe_config(
            provider_type,
            json!({
                "dedup_window_secs": 300,
                "circuit_sample_window_seconds": 300,
                "5xx_streak_threshold": 2
            }),
        )
    }

    fn endpoint(provider_id: &str) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            provider_id.to_string(),
            "openai:responses".to_string(),
            Some("openai".to_string()),
            Some("responses".to_string()),
            true,
        )
        .expect("endpoint should build")
    }

    fn pool_config() -> AdminProviderPoolConfig {
        admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": {
                "pre_probe": {
                    "dedup_window_secs": 300,
                    "circuit_sample_window_seconds": 300,
                    "5xx_streak_threshold": 2
                }
            }
        })))
        .expect("pool config should parse")
    }

    fn pool_config_with_pre_probe(pre_probe: Value) -> AdminProviderPoolConfig {
        admin_provider_pool_config_from_config_value(Some(&json!({
            "pool_advanced": { "pre_probe": pre_probe }
        })))
        .expect("pool config should parse")
    }

    fn cache_key() -> PoolCandidateCacheKey {
        PoolCandidateCacheKey::new(
            "provider-1",
            "endpoint-1",
            "model-1",
            "gpt-5",
            "openai:responses",
        )
    }

    fn pool_group_id() -> String {
        "provider=provider-1|endpoint=endpoint-1|model=model-1|selected_model=gpt-5|api_format=openai:responses|singleton_key=*".to_string()
    }

    fn candidate_row(key_id: &str, priority: i32) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-1".to_string(),
            provider_name: "provider-1".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-1".to_string(),
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
            key_internal_priority: priority,
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
        }
    }

    fn state_with_rows(
        provider_type: &str,
        keys: Vec<StoredProviderCatalogKey>,
        rows: Vec<StoredMinimalCandidateSelectionRow>,
    ) -> AppState {
        state_with_provider_and_rows(provider(provider_type), keys, rows)
    }

    fn state_with_provider_and_rows(
        provider: StoredProviderCatalogProvider,
        mut keys: Vec<StoredProviderCatalogKey>,
        rows: Vec<StoredMinimalCandidateSelectionRow>,
    ) -> AppState {
        for key in &mut keys {
            key.provider_id = "provider-1".to_string();
            key.auth_type = "oauth".to_string();
        }
        let catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![endpoint("provider-1")],
            keys,
        ));
        let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(rows));
        AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(catalog)
                    .attach_minimal_candidate_selection_reader_for_tests(candidates),
            )
    }

    fn cached_key_ids(state: &AppState) -> Vec<String> {
        let current_config_version = state.pool_candidate_config_version();
        state
            .pool_candidate_cache()
            .get_fresh(
                &cache_key(),
                POOL_PREHEAT_CANDIDATE_CACHE_TTL,
                current_config_version,
            )
            .expect("cache entry should remain")
            .rows
            .into_iter()
            .map(|row| row.key_id)
            .collect()
    }

    #[test]
    fn pool_preheat_probe_normalizes_key_ids() {
        let request = PoolPreheatProbeTaskRequest::new(
            " provider-1 ",
            " pool-group ",
            vec![
                " key-a ".to_string(),
                "".to_string(),
                "key-b".to_string(),
                "key-a".to_string(),
            ],
        );

        assert_eq!(request.provider_id, "provider-1");
        assert_eq!(request.pool_group_id, "pool-group");
        assert_eq!(request.top_n_key_ids, vec!["key-a", "key-b"]);
    }

    #[test]
    fn pool_preheat_probe_parses_pool_group_context() {
        let context = parse_pool_group_context(&pool_group_id()).expect("context should parse");

        assert_eq!(context.provider_id, "provider-1");
        assert_eq!(context.endpoint_id, "endpoint-1");
        assert_eq!(context.model_id, "model-1");
        assert_eq!(context.selected_provider_model_name, "gpt-5");
        assert_eq!(context.api_format, "openai:responses");
    }

    #[test]
    fn probe_outcome_reasons_use_safe_codes_not_raw_messages() {
        let oauth_invalid = OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::OAuthInvalid,
            false,
            false,
            false,
            json!({"status_code":401,"message":"Bearer access_token=secret refresh_token=secret"}),
        );
        let account_blocked = OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::AccountBlocked,
            false,
            false,
            false,
            json!({"status_code":403,"message":"cookie=session-secret account blocked"}),
        );

        assert_eq!(
            oauth_invalid_reason_for_outcome(&oauth_invalid),
            "[OAUTH_EXPIRED] oauth_invalid_401"
        );
        assert_eq!(
            account_blocked_reason_for_outcome(&account_blocked),
            "account_blocked_403"
        );
    }

    #[tokio::test]
    async fn candidate_key_ids_for_group_reads_first_row_from_zero_offset() {
        let state = state_with_rows(
            "codex",
            vec![key("key-1", "provider-1"), key("key-2", "provider-1")],
            vec![candidate_row("key-1", 1), candidate_row("key-2", 2)],
        );
        let request = PoolPreheatProbeTaskRequest::new("provider-1", pool_group_id(), Vec::new());

        let key_ids = candidate_key_ids_for_group(&state, &request, 1)
            .await
            .expect("candidate key ids should load");

        assert_eq!(key_ids, vec!["key-1"]);
    }

    #[tokio::test]
    async fn pool_preheat_probe_classification_loop_stops_after_eight_healthy() {
        let request = PoolPreheatProbeTaskRequest::new(
            "provider-1",
            "pool-group",
            (0..10).map(|index| format!("key-{index}")).collect(),
        );
        let keys = (0..10)
            .map(|index| key(&format!("key-{index}"), "provider-1"))
            .collect::<Vec<_>>();

        let summary = classify_pool_preheat_probe_keys(
            &request,
            10,
            keys,
            8,
            |_key| async { Ok(probe_outcome(OauthKeyProbeClassification::Healthy)) },
            |_key_id, _outcome| async { Ok(()) },
        )
        .await;

        assert_eq!(summary.probed_keys, 8);
        assert_eq!(summary.healthy, 8);
        assert!(summary.stopped_after_healthy);
    }

    #[tokio::test]
    async fn pool_preheat_probe_classification_loop_counts_non_writing_outcomes() {
        let request = PoolPreheatProbeTaskRequest::new(
            "provider-1",
            "pool-group",
            vec![
                "key-a".to_string(),
                "key-b".to_string(),
                "key-c".to_string(),
            ],
        );
        let keys = ["key-a", "key-b", "key-c"]
            .into_iter()
            .map(|key_id| key(key_id, "provider-1"))
            .collect::<Vec<_>>();
        let outcomes = std::sync::Arc::new(std::sync::Mutex::new(vec![
            OauthKeyProbeClassification::RateLimited,
            OauthKeyProbeClassification::OAuthInvalid,
            OauthKeyProbeClassification::Healthy,
        ]));

        let summary = classify_pool_preheat_probe_keys(
            &request,
            3,
            keys,
            8,
            |key| {
                let outcomes = std::sync::Arc::clone(&outcomes);
                async move {
                    assert!(key.upstream_metadata.is_none());
                    Ok(probe_outcome(
                        outcomes.lock().expect("outcomes should lock").remove(0),
                    ))
                }
            },
            |_key_id, _outcome| async { Ok(()) },
        )
        .await;

        assert_eq!(summary.probed_keys, 3);
        assert_eq!(summary.rate_limited, 1);
        assert_eq!(summary.oauth_invalid, 1);
        assert_eq!(summary.healthy, 1);
        assert_eq!(summary.server_error, 0);
    }

    #[tokio::test]
    async fn pool_preheat_probe_empty_keys_skips_without_classification() {
        let request = PoolPreheatProbeTaskRequest::new("provider-1", "pool-group", Vec::new());

        let summary = classify_pool_preheat_probe_keys(
            &request,
            0,
            Vec::new(),
            8,
            |_key| async { Ok(probe_outcome(OauthKeyProbeClassification::Healthy)) },
            |_key_id, _outcome| async { Ok(()) },
        )
        .await;

        assert!(summary.task_status_is_skipped());
        assert_eq!(
            summary.skipped_reason.as_deref(),
            Some("no candidate keys to probe")
        );
    }

    #[tokio::test]
    async fn pool_preheat_probe_dedup_hit_counts_healthy_without_classification() {
        let request =
            PoolPreheatProbeTaskRequest::new("provider-1", "pool-group", vec!["key-1".to_string()]);
        let keys = vec![key("key-1", "provider-1")];
        let deduped_key_ids = BTreeSet::from(["key-1".to_string()]);

        let summary = classify_pool_preheat_probe_keys_with_dedup(
            &request,
            1,
            keys,
            1,
            &deduped_key_ids,
            |_key| async { panic!("deduped key should not be classified") },
            |_key_id, _outcome| async { panic!("deduped key should not write an outcome") },
        )
        .await;

        assert_eq!(summary.probed_keys, 0);
        assert_eq!(summary.deduped_keys, 1);
        assert_eq!(summary.healthy, 1);
        assert!(summary.stopped_after_healthy);
        assert!(!summary.task_status_is_skipped());
    }

    #[tokio::test]
    async fn record_probe_outcome_for_circuit_opens_after_ten_consecutive_5xx() {
        let state = state_with_rows("codex", Vec::new(), Vec::new());
        let pool_config = pool_config_with_pre_probe(json!({
            "circuit_failure_rate_threshold": 50,
            "circuit_sample_window_seconds": 300,
            "circuit_suspend_seconds": 600,
            "5xx_streak_threshold": 10
        }));
        let start = 1_760_000_000;
        let outcome = probe_outcome(OauthKeyProbeClassification::ServerError);

        for offset in 0..9 {
            record_probe_outcome_for_circuit(
                &state,
                "provider-1",
                &outcome,
                &pool_config,
                start + offset,
            )
            .await;
            assert!(
                probe_circuit_allows_provider(&state, "provider-1", &pool_config, start + offset)
                    .await
            );
        }

        record_probe_outcome_for_circuit(&state, "provider-1", &outcome, &pool_config, start + 9)
            .await;

        assert!(
            !probe_circuit_allows_provider(&state, "provider-1", &pool_config, start + 9).await
        );
        assert_eq!(
            state
                .runtime_state
                .kv_get(&pool_preheat_provider_circuit_suspend_key("provider-1"))
                .await
                .expect("suspend marker should read")
                .as_deref(),
            Some("1760000609")
        );
    }

    #[tokio::test]
    async fn probe_circuit_allows_provider_after_suspend_expiry() {
        let state = state_with_rows("codex", Vec::new(), Vec::new());
        let pool_config = pool_config_with_pre_probe(json!({
            "circuit_failure_rate_threshold": 50,
            "circuit_sample_window_seconds": 300,
            "circuit_suspend_seconds": 10,
            "5xx_streak_threshold": 3
        }));
        let start = 1_760_000_000;
        let outcome = probe_outcome(OauthKeyProbeClassification::TransportError);

        for offset in 0..3 {
            record_probe_outcome_for_circuit(
                &state,
                "provider-1",
                &outcome,
                &pool_config,
                start + offset,
            )
            .await;
        }

        assert!(
            !probe_circuit_allows_provider(&state, "provider-1", &pool_config, start + 2).await
        );
        assert!(
            probe_circuit_allows_provider(&state, "provider-1", &pool_config, start + 13).await
        );
        assert_eq!(
            state
                .runtime_state
                .kv_get(&pool_preheat_provider_circuit_suspend_key("provider-1"))
                .await
                .expect("suspend marker should read"),
            None
        );
    }

    #[tokio::test]
    async fn record_probe_outcome_for_circuit_counts_healthy_as_non_failure_sample() {
        let state = state_with_rows("codex", Vec::new(), Vec::new());
        let pool_config = pool_config_with_pre_probe(json!({
            "circuit_failure_rate_threshold": 50,
            "circuit_sample_window_seconds": 300,
            "circuit_suspend_seconds": 600,
            "5xx_streak_threshold": 2
        }));
        let start = 1_760_000_000;

        record_probe_outcome_for_circuit(
            &state,
            "provider-1",
            &probe_outcome(OauthKeyProbeClassification::ServerError),
            &pool_config,
            start,
        )
        .await;
        record_probe_outcome_for_circuit(
            &state,
            "provider-1",
            &probe_outcome(OauthKeyProbeClassification::Healthy),
            &pool_config,
            start + 1,
        )
        .await;

        let samples = state
            .runtime_state
            .score_range_by_min(
                &pool_preheat_provider_circuit_samples_key("provider-1"),
                start as f64,
            )
            .await
            .expect("samples should read");
        assert_eq!(samples.len(), 2);
        assert_eq!(
            samples
                .iter()
                .filter(|member| probe_circuit_sample_member_is_failure(member))
                .count(),
            1
        );
        assert!(probe_circuit_allows_provider(&state, "provider-1", &pool_config, start + 1).await);
    }

    #[tokio::test]
    async fn perform_pool_preheat_probe_dedup_hit_skips_oauth_classifier() {
        let state = state_with_provider_and_rows(
            provider_with_pre_probe_config(
                "codex",
                json!({
                    "required_healthy": 1,
                    "dedup_window_secs": 300,
                    "per_provider_rate_limit_per_minute": 1
                }),
            ),
            vec![key("key-1", "provider-1")],
            vec![candidate_row("key-1", 1)],
        );
        let now_unix_secs = current_unix_secs();
        state
            .runtime_state
            .kv_set(
                "ap:preheat:last:provider-1:key-1",
                now_unix_secs.to_string(),
                Some(Duration::from_secs(300)),
            )
            .await
            .expect("dedup stamp should write");

        let summary = perform_pool_preheat_probe(
            &state,
            PoolPreheatProbeTaskRequest::new(
                "provider-1",
                pool_group_id(),
                vec!["key-1".to_string()],
            ),
        )
        .await
        .expect("preheat probe should succeed");

        assert_eq!(summary.probed_keys, 0);
        assert_eq!(summary.deduped_keys, 1);
        assert_eq!(summary.healthy, 1);
        assert_eq!(summary.rate_limit_remaining, None);
        assert!(!summary.task_status_is_skipped());
    }

    #[tokio::test]
    async fn perform_pool_preheat_probe_group_lock_contention_returns_skipped_summary() {
        let state = state_with_rows(
            "codex",
            vec![key("key-1", "provider-1")],
            vec![candidate_row("key-1", 1)],
        );
        let request = PoolPreheatProbeTaskRequest::new(
            "provider-1",
            pool_group_id(),
            vec!["key-1".to_string()],
        );
        let lease = state
            .runtime_state
            .lock_try_acquire(
                &pool_preheat_probe_group_lock_key(&request),
                "other-node",
                Duration::from_secs(30),
            )
            .await
            .expect("lock acquire should run")
            .expect("lock should be held");

        let summary = perform_pool_preheat_probe(&state, request)
            .await
            .expect("preheat probe should return skipped summary");

        assert!(summary.task_status_is_skipped());
        assert_eq!(summary.probed_keys, 0);
        assert_eq!(summary.selected_keys, 1);
        assert_eq!(
            summary.skipped_reason.as_deref(),
            Some("group lock already held")
        );
        state
            .runtime_state
            .lock_release(&lease)
            .await
            .expect("lock release should run");
    }

    #[tokio::test]
    async fn perform_pool_preheat_probe_open_provider_circuit_skips_before_key_mutation() {
        let state = state_with_rows(
            "codex",
            vec![key("key-1", "provider-1"), key("key-2", "provider-1")],
            vec![candidate_row("key-1", 1), candidate_row("key-2", 2)],
        );
        state.pool_candidate_cache().insert(
            cache_key(),
            CachedPoolCandidates::new(
                vec![candidate_row("key-1", 1), candidate_row("key-2", 2)],
                0,
                2,
            ),
            POOL_PREHEAT_CANDIDATE_CACHE_TTL,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ENTRIES,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS,
        );
        let suspend_until = current_unix_secs().saturating_add(600);
        state
            .runtime_state
            .kv_set(
                &pool_preheat_provider_circuit_suspend_key("provider-1"),
                suspend_until.to_string(),
                Some(Duration::from_secs(600)),
            )
            .await
            .expect("suspend marker should write");

        let summary = perform_pool_preheat_probe(
            &state,
            PoolPreheatProbeTaskRequest::new(
                "provider-1",
                pool_group_id(),
                vec!["key-1".to_string()],
            ),
        )
        .await
        .expect("preheat probe should return skipped summary");

        assert!(summary.task_status_is_skipped());
        assert_eq!(summary.probed_keys, 0);
        assert_eq!(summary.selected_keys, 1);
        assert_eq!(
            summary.skipped_reason.as_deref(),
            Some("provider probe circuit open")
        );
        assert_eq!(cached_key_ids(&state), vec!["key-1", "key-2"]);
        let stored_key = state
            .read_provider_catalog_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should read")
            .remove(0);
        assert!(stored_key.health_by_format.is_none());
        assert!(stored_key.circuit_breaker_by_format.is_none());
        assert!(stored_key.oauth_invalid_at_unix_secs.is_none());
        assert_eq!(
            state
                .runtime_state
                .kv_get("ap:preheat:last:provider-1:key-1")
                .await
                .expect("dedup stamp should read"),
            None
        );
    }

    #[tokio::test]
    async fn perform_pool_preheat_probe_rate_limit_rejection_skips_without_key_mutation() {
        let state = state_with_provider_and_rows(
            provider_with_pre_probe_config(
                "codex",
                json!({
                    "required_healthy": 1,
                    "dedup_window_secs": 300,
                    "per_provider_rate_limit_per_minute": 1
                }),
            ),
            vec![key("key-1", "provider-1")],
            vec![candidate_row("key-1", 1)],
        );
        if current_unix_secs() % POOL_PREHEAT_PROVIDER_RATE_LIMIT_WINDOW_SECS
            >= POOL_PREHEAT_PROVIDER_RATE_LIMIT_WINDOW_SECS - 1
        {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let now_unix_secs = current_unix_secs();
        assert!(matches!(
            check_pool_preheat_provider_rate_limit(&state, "provider-1", 1, now_unix_secs).await,
            PoolPreheatProviderRateLimitDecision::Allowed { .. }
        ));

        let summary = perform_pool_preheat_probe(
            &state,
            PoolPreheatProbeTaskRequest::new(
                "provider-1",
                pool_group_id(),
                vec!["key-1".to_string()],
            ),
        )
        .await
        .expect("preheat probe should return skipped summary");

        assert!(summary.task_status_is_skipped());
        assert_eq!(summary.probed_keys, 0);
        assert_eq!(summary.rate_limited, 1);
        assert_eq!(
            summary.skipped_reason.as_deref(),
            Some("provider rate limit exceeded: provider limit 1/min")
        );
        let stored_key = state
            .read_provider_catalog_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should read")
            .remove(0);
        assert!(stored_key.health_by_format.is_none());
        assert!(stored_key.circuit_breaker_by_format.is_none());
        assert!(stored_key.oauth_invalid_at_unix_secs.is_none());
    }

    #[tokio::test]
    async fn pool_preheat_probe_provider_rate_limit_is_shared_across_nodes() {
        let shared_runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let node_a = state_with_provider_and_rows(
            provider_with_pre_probe_config(
                "codex",
                json!({ "per_provider_rate_limit_per_minute": 1 }),
            ),
            vec![key("key-node-a", "provider-1")],
            vec![candidate_row("key-node-a", 1)],
        )
        .with_runtime_state(shared_runtime.clone())
        .with_tunnel_identity_for_tests("node-a", None);
        let node_b = state_with_provider_and_rows(
            provider_with_pre_probe_config(
                "codex",
                json!({ "per_provider_rate_limit_per_minute": 1 }),
            ),
            vec![key("key-node-b", "provider-1")],
            vec![candidate_row("key-node-b", 1)],
        )
        .with_runtime_state(shared_runtime)
        .with_tunnel_identity_for_tests("node-b", None);

        // Passing the same provider/window through two AppStates exercises the shared backend counters.
        let now_unix_secs = current_unix_secs();
        assert!(matches!(
            check_pool_preheat_provider_rate_limit(&node_a, "provider-1", 1, now_unix_secs).await,
            PoolPreheatProviderRateLimitDecision::Allowed {
                fail_open: false,
                ..
            }
        ));
        assert_eq!(
            check_pool_preheat_provider_rate_limit(&node_b, "provider-1", 1, now_unix_secs).await,
            PoolPreheatProviderRateLimitDecision::Rejected {
                scope: RateLimitScope::User,
                limit: 1,
            }
        );
        let stored_key = node_b
            .read_provider_catalog_keys_by_ids(&["key-node-b".to_string()])
            .await
            .expect("node B key should read")
            .remove(0);
        assert!(stored_key.health_by_format.is_none());
        assert!(stored_key.circuit_breaker_by_format.is_none());
        assert!(stored_key.oauth_invalid_at_unix_secs.is_none());
        assert_eq!(
            node_b
                .runtime_state
                .kv_get("ap:preheat:last:provider-1:key-node-b")
                .await
                .expect("node B dedup stamp should read"),
            None
        );
    }

    #[test]
    fn pool_preheat_probe_rate_limit_runtime_error_fails_open() {
        assert_eq!(
            pool_preheat_provider_rate_limit_decision_from_result(Err(
                DataLayerError::InvalidInput("runtime unavailable".to_string())
            )),
            PoolPreheatProviderRateLimitDecision::Allowed {
                remaining: None,
                fail_open: true,
            }
        );
    }

    #[tokio::test]
    async fn apply_probe_outcome_healthy_writes_last_stamp_without_health_reset() {
        let mut healthy_key = key("key-1", "provider-1");
        healthy_key.health_by_format = Some(json!({
            "openai:responses": {
                "health_score": 0.4,
                "consecutive_failures": 3,
                "last_failure_at": "2026-01-01T00:00:00+00:00"
            }
        }));
        let expected_health = healthy_key.health_by_format.clone();
        let state = state_with_rows("codex", vec![healthy_key], Vec::new());
        let pool_config = pool_config();

        apply_probe_outcome(
            &state,
            Some(&cache_key()),
            "key-1",
            "codex",
            &probe_outcome(OauthKeyProbeClassification::Healthy),
            1_760_000_000,
            &pool_config,
        )
        .await
        .expect("healthy outcome should write");

        assert_eq!(
            state
                .runtime_state
                .kv_get("ap:preheat:last:provider-1:key-1")
                .await
                .expect("stamp should read")
                .as_deref(),
            Some("1760000000")
        );
        let ttl = state
            .runtime_state
            .kv_ttl_seconds("ap:preheat:last:provider-1:key-1")
            .await
            .expect("stamp ttl should read")
            .expect("stamp should have ttl");
        assert!(ttl <= 300 && ttl > 0);
        let stored_key = state
            .read_provider_catalog_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should read")
            .remove(0);
        assert_eq!(stored_key.health_by_format, expected_health);
    }

    #[tokio::test]
    async fn apply_probe_outcome_oauth_invalid_evicts_circuits_and_backfills_one() {
        let state = state_with_rows(
            "codex",
            vec![key("key-1", "provider-1"), key("key-2", "provider-1")],
            vec![
                candidate_row("key-1", 1),
                candidate_row("key-2", 2),
                candidate_row("key-3", 3),
                candidate_row("key-4", 4),
            ],
        );
        state.pool_candidate_cache().insert(
            cache_key(),
            CachedPoolCandidates::new(
                vec![candidate_row("key-1", 1), candidate_row("key-2", 2)],
                0,
                2,
            ),
            POOL_PREHEAT_CANDIDATE_CACHE_TTL,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ENTRIES,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS,
        );
        let outcome = OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::OAuthInvalid,
            false,
            false,
            false,
            json!({"status":"auth_invalid","status_code":401,"message":"expired token"}),
        );

        apply_probe_outcome(
            &state,
            Some(&cache_key()),
            "key-1",
            "codex",
            &outcome,
            1_760_000_000,
            &pool_config(),
        )
        .await
        .expect("oauth invalid outcome should write");

        assert_eq!(cached_key_ids(&state), vec!["key-2", "key-3"]);
        let stored_key = state
            .read_provider_catalog_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should read")
            .remove(0);
        assert!(stored_key.oauth_invalid_at_unix_secs.is_some());
        assert!(stored_key
            .circuit_breaker_by_format
            .as_ref()
            .and_then(|value| value.get("openai:responses"))
            .and_then(|value| value.get("open"))
            .and_then(Value::as_bool)
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn apply_probe_outcome_account_blocked_evicts_and_sets_provider_metadata() {
        let state = state_with_rows(
            "kiro",
            vec![key("key-1", "provider-1"), key("key-2", "provider-1")],
            vec![
                candidate_row("key-1", 1),
                candidate_row("key-2", 2),
                candidate_row("key-3", 3),
            ],
        );
        state.pool_candidate_cache().insert(
            cache_key(),
            CachedPoolCandidates::new(
                vec![candidate_row("key-1", 1), candidate_row("key-2", 2)],
                0,
                2,
            ),
            POOL_PREHEAT_CANDIDATE_CACHE_TTL,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ENTRIES,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS,
        );
        let outcome = OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::AccountBlocked,
            false,
            false,
            false,
            json!({"status":"banned","status_code":403,"message":"account banned"}),
        );

        apply_probe_outcome(
            &state,
            Some(&cache_key()),
            "key-1",
            "kiro",
            &outcome,
            1_760_000_000,
            &pool_config(),
        )
        .await
        .expect("account blocked outcome should write");

        assert_eq!(cached_key_ids(&state), vec!["key-2", "key-3"]);
        let stored_key = state
            .read_provider_catalog_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should read")
            .remove(0);
        let kiro_metadata = stored_key
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("kiro"))
            .expect("kiro metadata should exist");
        assert_eq!(kiro_metadata["account_blocked"], json!(true));
        assert_eq!(kiro_metadata["is_banned"], json!(true));
        assert!(stored_key
            .oauth_invalid_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX)));
    }

    #[tokio::test]
    async fn apply_probe_outcome_rate_limited_keeps_cache_and_sets_cooldown() {
        let state = state_with_rows(
            "codex",
            vec![key("key-1", "provider-1")],
            vec![candidate_row("key-1", 1)],
        );
        state.pool_candidate_cache().insert(
            cache_key(),
            CachedPoolCandidates::new(vec![candidate_row("key-1", 1)], 0, 1),
            POOL_PREHEAT_CANDIDATE_CACHE_TTL,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ENTRIES,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS,
        );
        let outcome = OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::RateLimited,
            false,
            false,
            false,
            json!({"status":"quota_exhausted","status_code":402}),
        );

        apply_probe_outcome(
            &state,
            Some(&cache_key()),
            "key-1",
            "codex",
            &outcome,
            1_760_000_000,
            &pool_config(),
        )
        .await
        .expect("rate limited outcome should write");

        assert_eq!(cached_key_ids(&state), vec!["key-1"]);
        assert_eq!(
            state
                .runtime_state
                .kv_get("ap:provider-1:cooldown:key-1")
                .await
                .expect("cooldown should read")
                .as_deref(),
            Some("rate_limited_429")
        );
        let ttl = state
            .runtime_state
            .kv_ttl_seconds("ap:provider-1:cooldown:key-1")
            .await
            .expect("cooldown ttl should read")
            .expect("cooldown should have ttl");
        assert!(ttl <= 60 && ttl > 0);
    }

    #[tokio::test]
    async fn commit_prepared_probe_cache_skips_unexpected_version_advance() {
        let state = state_with_rows(
            "codex",
            vec![key("key-1", "provider-1")],
            vec![candidate_row("key-1", 1)],
        );
        let prepared = CachedPoolCandidates::new(vec![candidate_row("key-1", 1)], 0, 1);
        state.bump_pool_candidate_config_version().await;
        state.bump_pool_candidate_config_version().await;

        commit_prepared_probe_cache(&state, &cache_key(), prepared, 1).await;

        let current_config_version = state.effective_pool_candidate_config_version().await;
        assert!(state
            .pool_candidate_cache()
            .get_fresh(
                &cache_key(),
                POOL_PREHEAT_CANDIDATE_CACHE_TTL,
                current_config_version,
            )
            .is_none());
    }

    #[tokio::test]
    async fn apply_probe_outcome_server_and_transport_errors_degrade_and_escalate_at_threshold() {
        let state = state_with_rows(
            "codex",
            vec![key("key-1", "provider-1")],
            vec![candidate_row("key-1", 1)],
        );
        state.pool_candidate_cache().insert(
            cache_key(),
            CachedPoolCandidates::new(vec![candidate_row("key-1", 1)], 0, 1),
            POOL_PREHEAT_CANDIDATE_CACHE_TTL,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ENTRIES,
            POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS,
        );
        let server_error = OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::ServerError,
            false,
            false,
            false,
            json!({"status":"error","status_code":500,"message":"upstream error"}),
        );
        let transport_error = OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::TransportError,
            false,
            false,
            false,
            json!({"status":"error","status_code":502,"message":"connect failed"}),
        );

        apply_probe_outcome(
            &state,
            Some(&cache_key()),
            "key-1",
            "codex",
            &server_error,
            1_760_000_000,
            &pool_config(),
        )
        .await
        .expect("server error outcome should write");

        assert_eq!(cached_key_ids(&state), vec!["key-1"]);
        assert_eq!(
            state
                .runtime_state
                .kv_get("ap:provider-1:cooldown:key-1")
                .await
                .expect("cooldown should read"),
            None
        );

        apply_probe_outcome(
            &state,
            Some(&cache_key()),
            "key-1",
            "codex",
            &transport_error,
            1_760_000_001,
            &pool_config(),
        )
        .await
        .expect("transport error outcome should write");

        assert_eq!(cached_key_ids(&state), vec!["key-1"]);
        assert_eq!(
            state
                .runtime_state
                .kv_get("ap:preheat:5xx_streak:provider-1:key-1")
                .await
                .expect("streak should read")
                .as_deref(),
            Some("2")
        );
        assert_eq!(
            state
                .runtime_state
                .kv_get("ap:provider-1:cooldown:key-1")
                .await
                .expect("cooldown should read")
                .as_deref(),
            Some("bad_gateway_502")
        );
        let stored_key = state
            .read_provider_catalog_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should read")
            .remove(0);
        assert_eq!(
            stored_key.health_by_format.as_ref().unwrap()["openai:responses"]
                ["consecutive_failures"],
            json!(2)
        );
    }

    #[test]
    fn pool_preheat_probe_summary_serializes_without_outcome_payloads() {
        let summary = PoolPreheatProbeRunSummary {
            provider_id: "provider-1".to_string(),
            pool_group_id: "pool-group".to_string(),
            requested_keys: 2,
            selected_keys: 2,
            probed_keys: 2,
            healthy: 1,
            rate_limited: 1,
            ..PoolPreheatProbeRunSummary::default()
        };

        assert_eq!(
            serde_json::to_value(&summary).expect("summary should serialize"),
            json!({
                "provider_id": "provider-1",
                "pool_group_id": "pool-group",
                "requested_keys": 2,
                "selected_keys": 2,
                "skipped_keys": 0,
                "deduped_keys": 0,
                "probed_keys": 2,
                "healthy": 1,
                "oauth_invalid": 0,
                "account_blocked": 0,
                "rate_limited": 1,
                "server_error": 0,
                "transport_error": 0,
                "errored": 0,
                "coordination_fail_open": 0,
                "rate_limit_remaining": null,
                "stopped_after_healthy": false,
                "skipped_reason": null,
            })
        );
    }

    mod oauth_pool_preheat_integration {
        use super::*;

        #[tokio::test]
        async fn q3_backfill_maintains_128_on_evict() {
            let key_ids = (0..=128)
                .map(|index| format!("key-{index:03}"))
                .collect::<Vec<_>>();
            let state = state_with_rows(
                "codex",
                key_ids
                    .iter()
                    .map(|key_id| key(key_id, "provider-1"))
                    .collect(),
                key_ids
                    .iter()
                    .enumerate()
                    .map(|(index, key_id)| candidate_row(key_id, index as i32))
                    .collect(),
            );
            state.pool_candidate_cache().insert(
                cache_key(),
                CachedPoolCandidates::new(
                    key_ids
                        .iter()
                        .take(128)
                        .enumerate()
                        .map(|(index, key_id)| candidate_row(key_id, index as i32))
                        .collect(),
                    state.pool_candidate_config_version(),
                    128,
                ),
                POOL_PREHEAT_CANDIDATE_CACHE_TTL,
                POOL_PREHEAT_CANDIDATE_CACHE_MAX_ENTRIES,
                POOL_PREHEAT_CANDIDATE_CACHE_MAX_ROWS,
            );
            let outcome = OauthKeyProbeOutcome::from_payload(
                OauthKeyProbeClassification::OAuthInvalid,
                false,
                false,
                false,
                json!({"status":"auth_invalid","status_code":401}),
            );

            apply_probe_outcome(
                &state,
                Some(&cache_key()),
                "key-000",
                "codex",
                &outcome,
                1_760_000_000,
                &pool_config(),
            )
            .await
            .expect("oauth invalid should evict and backfill cache");

            let cached = state
                .pool_candidate_cache()
                .get_fresh(
                    &cache_key(),
                    POOL_PREHEAT_CANDIDATE_CACHE_TTL,
                    state.pool_candidate_config_version(),
                )
                .expect("cache entry should remain after backfill");
            assert_eq!(cached.rows.len(), 128);
            assert_eq!(
                cached.rows.first().map(|row| row.key_id.as_str()),
                Some("key-001")
            );
            assert_eq!(
                cached.rows.last().map(|row| row.key_id.as_str()),
                Some("key-128")
            );
            assert_eq!(cached.next_db_offset, 129);
        }

        #[tokio::test]
        async fn q5_probe_dedup_skips_within_5min() {
            let state = state_with_provider_and_rows(
                provider_with_pre_probe_config(
                    "codex",
                    json!({
                        "required_healthy": 1,
                        "dedup_window_secs": 300,
                        "per_provider_rate_limit_per_minute": 1
                    }),
                ),
                vec![key("key-1", "provider-1")],
                vec![candidate_row("key-1", 1)],
            );
            let now_unix_secs = current_unix_secs();
            state
                .runtime_state
                .kv_set(
                    "ap:preheat:last:provider-1:key-1",
                    now_unix_secs.to_string(),
                    Some(Duration::from_secs(300)),
                )
                .await
                .expect("dedup stamp should write");

            let summary = perform_pool_preheat_probe(
                &state,
                PoolPreheatProbeTaskRequest::new(
                    "provider-1",
                    pool_group_id(),
                    vec!["key-1".to_string()],
                ),
            )
            .await
            .expect("preheat probe should dedup");

            assert_eq!(summary.probed_keys, 0);
            assert_eq!(summary.deduped_keys, 1);
            assert_eq!(summary.healthy, 1);
            assert_eq!(summary.rate_limit_remaining, None);
            assert!(!summary.task_status_is_skipped());
        }

        #[tokio::test]
        async fn q6_per_provider_rate_limit_crossnode() {
            let shared_runtime =
                Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
            let node_a = state_with_provider_and_rows(
                provider_with_pre_probe_config(
                    "codex",
                    json!({ "per_provider_rate_limit_per_minute": 1 }),
                ),
                vec![key("key-node-a", "provider-1")],
                vec![candidate_row("key-node-a", 1)],
            )
            .with_runtime_state(shared_runtime.clone())
            .with_tunnel_identity_for_tests("node-a", None);
            let node_b = state_with_provider_and_rows(
                provider_with_pre_probe_config(
                    "codex",
                    json!({ "per_provider_rate_limit_per_minute": 1 }),
                ),
                vec![key("key-node-b", "provider-1")],
                vec![candidate_row("key-node-b", 1)],
            )
            .with_runtime_state(shared_runtime)
            .with_tunnel_identity_for_tests("node-b", None);
            let now_unix_secs = current_unix_secs();

            assert!(matches!(
                check_pool_preheat_provider_rate_limit(&node_a, "provider-1", 1, now_unix_secs)
                    .await,
                PoolPreheatProviderRateLimitDecision::Allowed {
                    fail_open: false,
                    ..
                }
            ));
            assert_eq!(
                check_pool_preheat_provider_rate_limit(&node_b, "provider-1", 1, now_unix_secs)
                    .await,
                PoolPreheatProviderRateLimitDecision::Rejected {
                    scope: RateLimitScope::User,
                    limit: 1,
                }
            );
            assert_eq!(
                node_b
                    .runtime_state
                    .kv_get("ap:preheat:last:provider-1:key-node-b")
                    .await
                    .expect("node B dedup stamp should read"),
                None
            );
        }

        #[tokio::test]
        async fn q7_probe_circuit_on_global_5xx() {
            let state = state_with_provider_and_rows(
                provider_with_pre_probe_config(
                    "codex",
                    json!({
                        "required_healthy": 1,
                        "circuit_failure_rate_threshold": 50,
                        "circuit_sample_window_seconds": 300,
                        "circuit_suspend_seconds": 600,
                        "5xx_streak_threshold": 3
                    }),
                ),
                vec![key("key-1", "provider-1")],
                vec![candidate_row("key-1", 1)],
            );
            let pool_config = pool_config_with_pre_probe(json!({
                "circuit_failure_rate_threshold": 50,
                "circuit_sample_window_seconds": 300,
                "circuit_suspend_seconds": 600,
                "5xx_streak_threshold": 3
            }));
            let start = current_unix_secs();
            let outcome = probe_outcome(OauthKeyProbeClassification::ServerError);
            for offset in 0..3 {
                record_probe_outcome_for_circuit(
                    &state,
                    "provider-1",
                    &outcome,
                    &pool_config,
                    start + offset,
                )
                .await;
            }

            assert!(
                !probe_circuit_allows_provider(&state, "provider-1", &pool_config, start + 2).await
            );
            let summary = perform_pool_preheat_probe(
                &state,
                PoolPreheatProbeTaskRequest::new(
                    "provider-1",
                    pool_group_id(),
                    vec!["key-1".to_string()],
                ),
            )
            .await
            .expect("open provider circuit should skip probe");

            assert!(summary.task_status_is_skipped());
            assert_eq!(summary.probed_keys, 0);
            assert_eq!(
                summary.skipped_reason.as_deref(),
                Some("provider probe circuit open")
            );
            assert!(state
                .runtime_state
                .kv_get(&pool_preheat_provider_circuit_suspend_key("provider-1"))
                .await
                .expect("circuit marker should read")
                .is_some());
        }
    }
}
