use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::pool_scores::{
    PoolMemberHardState, PoolMemberIdentity, PoolMemberProbeAttempt, PoolMemberProbeResult,
    PoolMemberProbeStatus,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_runtime_state::{RuntimeLockLease, RuntimeState};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::admin_api::{
    admin_provider_pool_config, provider_quota_refresh_endpoint_for_provider,
    provider_type_supports_quota_refresh, refresh_provider_pool_quota_locally, AdminAppState,
};
use crate::{AppState, GatewayError};

const ACCOUNT_SELF_CHECK_REDIS_PREFIX: &str = "ap:account_self_check:last";
const ACCOUNT_SELF_CHECK_LOCK_TTL_MS: u64 = 30_000;
const ACCOUNT_SELF_CHECK_DEFAULT_SCAN_INTERVAL_SECONDS: u64 = 60;
const ACCOUNT_SELF_CHECK_MIN_SCAN_INTERVAL_SECONDS: u64 = 15;
const ACCOUNT_SELF_CHECK_DEFAULT_MAX_KEYS_PER_PROVIDER: usize = 200;
const ACCOUNT_SELF_CHECK_DEFAULT_GLOBAL_CONCURRENCY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct AccountSelfCheckRunSummary {
    pub(crate) providers_checked: usize,
    pub(crate) providers_checked_with_keys: usize,
    pub(crate) providers_skipped: usize,
    pub(crate) scanned_keys: usize,
    pub(crate) selected_keys: usize,
    pub(crate) succeeded: usize,
    pub(crate) blocked: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
    pub(crate) auto_removed: usize,
}

impl AccountSelfCheckRunSummary {
    const fn empty() -> Self {
        Self {
            providers_checked: 0,
            providers_checked_with_keys: 0,
            providers_skipped: 0,
            scanned_keys: 0,
            selected_keys: 0,
            succeeded: 0,
            blocked: 0,
            failed: 0,
            skipped: 0,
            auto_removed: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AccountSelfCheckWorkerConfig {
    pub(crate) scan_interval: Duration,
    pub(crate) max_keys_per_provider: usize,
    pub(crate) global_concurrency: usize,
}

impl AccountSelfCheckWorkerConfig {
    fn from_env() -> Self {
        let scan_interval_seconds = env_u64(
            "ACCOUNT_SELF_CHECK_SCAN_INTERVAL_SECONDS",
            ACCOUNT_SELF_CHECK_DEFAULT_SCAN_INTERVAL_SECONDS,
        )
        .max(ACCOUNT_SELF_CHECK_MIN_SCAN_INTERVAL_SECONDS);
        let max_keys_per_provider = env_usize(
            "ACCOUNT_SELF_CHECK_MAX_KEYS_PER_PROVIDER",
            ACCOUNT_SELF_CHECK_DEFAULT_MAX_KEYS_PER_PROVIDER,
        )
        .max(1);
        let global_concurrency = env_usize(
            "ACCOUNT_SELF_CHECK_GLOBAL_CONCURRENCY",
            ACCOUNT_SELF_CHECK_DEFAULT_GLOBAL_CONCURRENCY,
        )
        .clamp(1, 256);
        Self {
            scan_interval: Duration::from_secs(scan_interval_seconds),
            max_keys_per_provider,
            global_concurrency,
        }
    }
}

enum AccountSelfCheckOutcome {
    Success {
        status_code: Option<u16>,
        message: Option<String>,
    },
    Blocked {
        status_code: Option<u16>,
        message: String,
    },
    AutoRemoved {
        status_code: Option<u16>,
        message: String,
    },
    Failed {
        status_code: Option<u16>,
        message: String,
    },
    Skipped {
        message: String,
    },
}

impl AccountSelfCheckOutcome {
    fn score_status(&self) -> &'static str {
        match self {
            Self::Success { .. } => "success",
            Self::Blocked { .. } => "blocked",
            Self::AutoRemoved { .. } => "auto_removed",
            Self::Failed { .. } => "failed",
            Self::Skipped { .. } => "skipped",
        }
    }

    fn status_code(&self) -> Option<u16> {
        match self {
            Self::Success { status_code, .. }
            | Self::Blocked { status_code, .. }
            | Self::AutoRemoved { status_code, .. }
            | Self::Failed { status_code, .. } => *status_code,
            Self::Skipped { .. } => None,
        }
    }

    fn message(&self) -> Option<&str> {
        match self {
            Self::Success { message, .. } => message.as_deref(),
            Self::Blocked { message, .. }
            | Self::AutoRemoved { message, .. }
            | Self::Failed { message, .. }
            | Self::Skipped { message, .. } => Some(message.as_str()),
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
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn parse_check_stamp(raw_value: Option<&str>) -> Option<u64> {
    let parsed = raw_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<f64>().ok())?;
    if parsed <= 0.0 {
        return None;
    }
    Some(parsed as u64)
}

fn check_stamp_key(provider_id: &str, key_id: &str) -> String {
    format!("{ACCOUNT_SELF_CHECK_REDIS_PREFIX}:{provider_id}:{key_id}")
}

async fn load_check_timestamps(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &[String],
) -> BTreeMap<String, u64> {
    if key_ids.is_empty() {
        return BTreeMap::new();
    }

    let runtime_keys = key_ids
        .iter()
        .map(|key_id| check_stamp_key(provider_id, key_id))
        .collect::<Vec<_>>();
    let Ok(values) = runtime.kv_get_many(&runtime_keys).await else {
        debug!("gateway account self-check: failed to read runtime check stamps");
        return BTreeMap::new();
    };

    key_ids
        .iter()
        .zip(values)
        .filter_map(|(key_id, raw)| {
            parse_check_stamp(raw.as_deref()).map(|ts| (key_id.clone(), ts))
        })
        .collect()
}

async fn mark_check_timestamps(
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
                &check_stamp_key(provider_id, key_id),
                value.clone(),
                Some(Duration::from_secs(ttl_seconds)),
            )
            .await
            .is_err()
        {
            debug!("gateway account self-check: failed to write runtime check stamp");
        }
    }
}

async fn acquire_provider_self_check_lock(
    runtime: &RuntimeState,
    provider_id: &str,
) -> Option<RuntimeLockLease> {
    let owner = format!("aether-gateway-account-self-check-{}", std::process::id());
    match runtime
        .lock_try_acquire(
            &format!("account_self_check:{provider_id}"),
            &owner,
            Duration::from_millis(ACCOUNT_SELF_CHECK_LOCK_TTL_MS),
        )
        .await
    {
        Ok(lease) => lease,
        Err(err) => {
            debug!(
                provider_id,
                error = %err,
                "gateway account self-check: failed to acquire runtime provider lock"
            );
            None
        }
    }
}

async fn release_provider_self_check_lock(runtime: &RuntimeState, lease: Option<RuntimeLockLease>) {
    let Some(lease) = lease else {
        return;
    };
    if let Err(err) = runtime.lock_release(&lease).await {
        debug!(
            error = %err,
            "gateway account self-check: failed to release runtime provider lock"
        );
    }
}

pub(crate) fn select_account_self_check_key_ids(
    key_ids: &[String],
    now_ts: u64,
    interval_seconds: u64,
    last_check_timestamps: &BTreeMap<String, u64>,
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }

    let mut stale = key_ids
        .iter()
        .filter_map(|key_id| {
            let last_check_ts = last_check_timestamps.get(key_id).copied().unwrap_or(0);
            (last_check_ts == 0 || now_ts.saturating_sub(last_check_ts) >= interval_seconds)
                .then(|| (last_check_ts, key_id.clone()))
        })
        .collect::<Vec<_>>();
    stale.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    stale
        .into_iter()
        .take(limit)
        .map(|(_, key_id)| key_id)
        .collect()
}

async fn select_keys_for_provider(
    state: &AppState,
    runtime: &RuntimeState,
    provider: &StoredProviderCatalogProvider,
    interval_seconds: u64,
    max_keys_per_provider: usize,
    now_ts: u64,
) -> Result<Vec<StoredProviderCatalogKey>, GatewayError> {
    let lease = acquire_provider_self_check_lock(runtime, &provider.id).await;
    if lease.is_none() {
        return Ok(Vec::new());
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
            return Ok(Vec::new());
        }

        let key_ids = summaries
            .iter()
            .map(|summary| summary.id.clone())
            .collect::<Vec<_>>();
        let check_stamps = load_check_timestamps(runtime, &provider.id, &key_ids).await;
        let selected_ids = select_account_self_check_key_ids(
            &key_ids,
            now_ts,
            interval_seconds,
            &check_stamps,
            max_keys_per_provider,
        );
        if selected_ids.is_empty() {
            return Ok(Vec::new());
        }

        mark_check_timestamps(
            runtime,
            &provider.id,
            &selected_ids,
            now_ts,
            interval_seconds,
        )
        .await;

        let mut keys_by_id = state
            .list_provider_catalog_keys_by_ids(&selected_ids)
            .await?
            .into_iter()
            .map(|key| (key.id.clone(), key))
            .collect::<BTreeMap<_, _>>();
        Ok(selected_ids
            .into_iter()
            .filter_map(|key_id| keys_by_id.remove(&key_id))
            .collect::<Vec<_>>())
    }
    .await;

    release_provider_self_check_lock(runtime, lease).await;
    result
}

fn quota_payload_result_for_key(key_id: &str, payload: Option<Value>) -> AccountSelfCheckOutcome {
    let Some(payload) = payload else {
        return AccountSelfCheckOutcome::Failed {
            status_code: None,
            message: "quota refresh returned no payload".to_string(),
        };
    };
    let Some(results) = payload.get("results").and_then(Value::as_array) else {
        return AccountSelfCheckOutcome::Failed {
            status_code: None,
            message: "quota refresh returned no result list".to_string(),
        };
    };
    let Some(item) = results.iter().find(|item| {
        item.get("key_id")
            .and_then(Value::as_str)
            .is_some_and(|value| value == key_id)
    }) else {
        return AccountSelfCheckOutcome::Failed {
            status_code: None,
            message: "quota refresh result missing key".to_string(),
        };
    };

    let status = item
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let status_code = item
        .get("status_code")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok());
    let message = item
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let auto_removed = item
        .get("auto_removed")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if status == "success" {
        return AccountSelfCheckOutcome::Success {
            status_code,
            message,
        };
    }
    if auto_removed {
        return AccountSelfCheckOutcome::AutoRemoved {
            status_code,
            message: message.unwrap_or_else(|| "已自动删除".to_string()),
        };
    }
    if quota_result_status_is_blocked(&status, status_code, message.as_deref()) {
        return AccountSelfCheckOutcome::Blocked {
            status_code,
            message: message.unwrap_or_else(|| status.clone()),
        };
    }
    AccountSelfCheckOutcome::Failed {
        status_code,
        message: message.unwrap_or_else(|| status.clone()),
    }
}

fn quota_result_status_is_blocked(
    status: &str,
    status_code: Option<u16>,
    message: Option<&str>,
) -> bool {
    matches!(
        status,
        "banned" | "forbidden" | "workspace_deactivated" | "auth_invalid"
    ) || matches!(status_code, Some(401 | 403 | 423))
        || aether_admin::provider::status::resolve_pool_account_state(None, None, message).blocked
}

async fn perform_quota_refresh_check(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    provider_type: &str,
    key: StoredProviderCatalogKey,
) -> Result<AccountSelfCheckOutcome, GatewayError> {
    let key_id = key.id.clone();
    let payload = refresh_provider_pool_quota_locally(
        state,
        provider,
        endpoint,
        provider_type,
        vec![key],
        None,
    )
    .await?;
    Ok(quota_payload_result_for_key(&key_id, payload))
}

async fn record_score_probe_in_progress_for_key(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
    attempted_at: u64,
) {
    if !state.data.has_pool_score_writer() {
        return;
    }
    let attempt = PoolMemberProbeAttempt {
        identity: PoolMemberIdentity::provider_api_key(provider_id.to_string(), key_id.to_string()),
        scope: None,
        attempted_at,
        score_reason_patch: Some(json!({
            "last_probe": {
                "source": "account_self_check",
                "status": "in_progress"
            },
            "last_self_check": {
                "source": "account_self_check",
                "status": "in_progress",
                "attempted_at": attempted_at
            }
        })),
    };
    if let Err(err) = state.data.mark_pool_member_probe_in_progress(attempt).await {
        debug!(
            provider_id,
            key_id,
            error = ?err,
            "gateway account self-check: failed to mark score probe in progress"
        );
    }
}

async fn record_score_probe_result_for_key(
    state: &AppState,
    provider_id: &str,
    key_id: &str,
    attempted_at: u64,
    outcome: &AccountSelfCheckOutcome,
) {
    if !state.data.has_pool_score_writer() {
        return;
    }
    let (succeeded, hard_state, probe_status) = match outcome {
        AccountSelfCheckOutcome::Success { .. } => (
            true,
            Some(PoolMemberHardState::Available),
            PoolMemberProbeStatus::Ok,
        ),
        AccountSelfCheckOutcome::Blocked { .. } => (
            false,
            Some(PoolMemberHardState::Banned),
            PoolMemberProbeStatus::Failed,
        ),
        AccountSelfCheckOutcome::AutoRemoved { .. } => (
            false,
            Some(PoolMemberHardState::Banned),
            PoolMemberProbeStatus::Failed,
        ),
        AccountSelfCheckOutcome::Failed { .. } => (
            false,
            Some(PoolMemberHardState::Cooldown),
            PoolMemberProbeStatus::Failed,
        ),
        AccountSelfCheckOutcome::Skipped { .. } => (
            false,
            Some(PoolMemberHardState::Unknown),
            PoolMemberProbeStatus::Never,
        ),
    };
    let result = PoolMemberProbeResult {
        identity: PoolMemberIdentity::provider_api_key(provider_id.to_string(), key_id.to_string()),
        scope: None,
        attempted_at,
        succeeded,
        hard_state,
        probe_status,
        score_reason_patch: Some(json!({
            "last_probe": {
                "source": "account_self_check",
                "status": outcome.score_status(),
                "status_code": outcome.status_code(),
                "message": outcome.message()
            },
            "last_self_check": {
                "source": "account_self_check",
                "status": outcome.score_status(),
                "status_code": outcome.status_code(),
                "message": outcome.message(),
                "attempted_at": attempted_at
            }
        })),
    };
    if let Err(err) = state.data.record_pool_member_probe_result(result).await {
        debug!(
            provider_id,
            key_id,
            error = ?err,
            "gateway account self-check: failed to record score probe result"
        );
    }
}

fn endpoint_for_self_check(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Option<StoredProviderCatalogEndpoint> {
    provider_quota_refresh_endpoint_for_provider(provider_type, endpoints, true)
}

fn gateway_error_message(err: GatewayError) -> String {
    err.into_message()
}

fn update_summary_from_outcome(
    summary: &mut AccountSelfCheckRunSummary,
    outcome: &AccountSelfCheckOutcome,
) {
    match outcome {
        AccountSelfCheckOutcome::Success { .. } => {
            summary.succeeded = summary.succeeded.saturating_add(1);
        }
        AccountSelfCheckOutcome::Blocked { .. } => {
            summary.blocked = summary.blocked.saturating_add(1);
        }
        AccountSelfCheckOutcome::AutoRemoved { .. } => {
            summary.auto_removed = summary.auto_removed.saturating_add(1);
        }
        AccountSelfCheckOutcome::Failed { .. } => {
            summary.failed = summary.failed.saturating_add(1);
        }
        AccountSelfCheckOutcome::Skipped { .. } => {
            summary.skipped = summary.skipped.saturating_add(1);
        }
    }
}

pub(crate) async fn perform_account_self_check_once_with_config(
    state: &AppState,
    config: AccountSelfCheckWorkerConfig,
) -> Result<AccountSelfCheckRunSummary, GatewayError> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(AccountSelfCheckRunSummary::empty());
    }

    let providers = state
        .list_provider_catalog_providers(true)
        .await?
        .into_iter()
        .filter_map(|provider| {
            let provider_type = provider.provider_type.trim().to_ascii_lowercase();
            let pool_config = admin_provider_pool_config(&provider)?;
            if pool_config.account_self_check_enabled {
                Some((provider, provider_type, pool_config))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if providers.is_empty() {
        return Ok(AccountSelfCheckRunSummary::empty());
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
    let mut summary = AccountSelfCheckRunSummary {
        providers_checked: providers.len(),
        ..AccountSelfCheckRunSummary::empty()
    };

    for (provider, provider_type, pool_config) in providers {
        let provider_endpoints = endpoints_by_provider
            .get(&provider.id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let Some(endpoint) = endpoint_for_self_check(&provider_type, provider_endpoints) else {
            summary.providers_skipped = summary.providers_skipped.saturating_add(1);
            continue;
        };
        if !provider_type_supports_quota_refresh(&provider_type) {
            summary.providers_skipped = summary.providers_skipped.saturating_add(1);
            continue;
        }

        let interval_seconds = pool_config
            .account_self_check_interval_minutes
            .clamp(1, 1440)
            .saturating_mul(60);
        let provider_limit = config.max_keys_per_provider;
        let keys = select_keys_for_provider(
            state,
            state.runtime_state.as_ref(),
            &provider,
            interval_seconds,
            provider_limit,
            now_ts,
        )
        .await?;
        if keys.is_empty() {
            continue;
        }

        let selected_count = keys.len();
        summary.scanned_keys = summary.scanned_keys.saturating_add(selected_count);
        summary.selected_keys = summary.selected_keys.saturating_add(selected_count);
        summary.providers_checked_with_keys = summary.providers_checked_with_keys.saturating_add(1);
        for key in &keys {
            record_score_probe_in_progress_for_key(state, &provider.id, &key.id, now_ts).await;
        }

        let provider_short_id = provider.id.chars().take(8).collect::<String>();
        let concurrency = (pool_config.account_self_check_concurrency as usize)
            .clamp(1, 64)
            .min(config.global_concurrency)
            .max(1);
        let check_results = stream::iter(keys.into_iter().map(|key| {
            let admin_state = &admin_state;
            let provider = &provider;
            let endpoint = &endpoint;
            let provider_type = provider_type.as_str();
            async move {
                let key_for_check = key.clone();
                let result = perform_quota_refresh_check(
                    admin_state,
                    provider,
                    endpoint,
                    provider_type,
                    key_for_check,
                )
                .await;
                (key, result)
            }
        }))
        .buffer_unordered(concurrency)
        .collect::<Vec<_>>()
        .await;

        for (key, result) in check_results {
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(err) => AccountSelfCheckOutcome::Failed {
                    status_code: None,
                    message: gateway_error_message(err),
                },
            };
            record_score_probe_result_for_key(state, &provider.id, &key.id, now_ts, &outcome).await;
            update_summary_from_outcome(&mut summary, &outcome);
        }

        info!(
            provider_id = %provider_short_id,
            provider_type,
            selected = selected_count,
            concurrency,
            "gateway account self-check completed"
        );
    }

    Ok(summary)
}

pub(crate) async fn perform_account_self_check_once(
    state: &AppState,
) -> Result<AccountSelfCheckRunSummary, GatewayError> {
    perform_account_self_check_once_with_config(state, AccountSelfCheckWorkerConfig::from_env())
        .await
}

pub(crate) fn spawn_account_self_check_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return None;
    }

    let config = AccountSelfCheckWorkerConfig::from_env();
    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.scan_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        let mut deferred_since = None;
        loop {
            interval.tick().await;
            if state
                .data
                .should_defer_maintenance_for_database_pool_pressure(&mut deferred_since)
            {
                debug!(
                    event_name = "maintenance_worker_deferred",
                    log_type = "ops",
                    worker = "account_self_check",
                    "gateway account self-check deferred because database pool has no idle reserve"
                );
                continue;
            }
            if let Err(err) = perform_account_self_check_once_with_config(&state, config).await {
                warn!(
                    error = ?err,
                    "gateway account self-check worker tick failed"
                );
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::select_account_self_check_key_ids;
    use std::collections::BTreeMap;

    #[test]
    fn selects_never_and_stale_self_check_keys_first() {
        let key_ids = vec![
            "fresh".to_string(),
            "never".to_string(),
            "stale".to_string(),
        ];
        let stamps = BTreeMap::from([("fresh".to_string(), 1_950), ("stale".to_string(), 1_000)]);

        let selected = select_account_self_check_key_ids(&key_ids, 2_000, 600, &stamps, 2);

        assert_eq!(selected, vec!["never".to_string(), "stale".to_string()]);
    }
}
