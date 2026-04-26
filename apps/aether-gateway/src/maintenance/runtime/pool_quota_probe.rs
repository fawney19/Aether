use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_data::redis::{RedisKvRunner, RedisLockLease, RedisLockRunner};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::admin_api::{
    admin_provider_pool_config, provider_oauth_runtime_endpoint_for_provider,
    refresh_antigravity_provider_quota_locally, refresh_codex_provider_quota_locally,
    refresh_kiro_provider_quota_locally, AdminAppState,
};
use crate::{AppState, GatewayError};

const POOL_QUOTA_PROBE_REDIS_PREFIX: &str = "ap:quota_probe:last";
const POOL_QUOTA_PROBE_DEFAULT_SCAN_INTERVAL_SECONDS: u64 = 60;
const POOL_QUOTA_PROBE_MIN_SCAN_INTERVAL_SECONDS: u64 = 15;
const POOL_QUOTA_PROBE_DEFAULT_MAX_KEYS_PER_PROVIDER: usize = 50;
const POOL_QUOTA_PROBE_PROVIDER_LOCK_TTL_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolQuotaProbeRunSummary {
    pub(crate) providers_checked: usize,
    pub(crate) providers_probed: usize,
    pub(crate) providers_skipped: usize,
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
}

impl PoolQuotaProbeWorkerConfig {
    fn from_env() -> Self {
        let scan_interval_seconds = env_u64(
            "POOL_QUOTA_PROBE_SCAN_INTERVAL_SECONDS",
            POOL_QUOTA_PROBE_DEFAULT_SCAN_INTERVAL_SECONDS,
        )
        .max(POOL_QUOTA_PROBE_MIN_SCAN_INTERVAL_SECONDS);
        let max_keys_per_provider = env_usize(
            "POOL_QUOTA_PROBE_MAX_KEYS_PER_PROVIDER",
            POOL_QUOTA_PROBE_DEFAULT_MAX_KEYS_PER_PROVIDER,
        );
        Self {
            scan_interval: Duration::from_secs(scan_interval_seconds),
            max_keys_per_provider,
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
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "codex" | "kiro" | "antigravity"
    )
}

fn json_number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<f64>().ok())
}

fn extract_quota_updated_at(provider_type: &str, upstream_metadata: Option<&Value>) -> Option<u64> {
    let metadata = upstream_metadata?.as_object()?;
    let bucket_name = match provider_type.trim().to_ascii_lowercase().as_str() {
        "codex" => "codex",
        "kiro" => "kiro",
        "antigravity" => "antigravity",
        _ => return None,
    };
    let bucket = metadata.get(bucket_name)?.as_object()?;
    let mut updated_at = json_number(bucket.get("updated_at"))?;
    if updated_at <= 0.0 {
        return None;
    }
    if updated_at > 1_000_000_000_000.0 {
        updated_at /= 1000.0;
    }
    Some(updated_at as u64)
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
            extract_quota_updated_at(provider_type, key.upstream_metadata.as_ref());
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

fn probe_stamp_key(provider_id: &str, key_id: &str) -> String {
    format!("{POOL_QUOTA_PROBE_REDIS_PREFIX}:{provider_id}:{key_id}")
}

async fn load_probe_timestamps(
    runner: Option<&RedisKvRunner>,
    provider_id: &str,
    key_ids: &[String],
) -> BTreeMap<String, u64> {
    let Some(runner) = runner else {
        return BTreeMap::new();
    };
    if key_ids.is_empty() {
        return BTreeMap::new();
    }

    let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
        debug!("gateway pool quota probe: failed to connect redis for stamp read");
        return BTreeMap::new();
    };
    let redis_keys = key_ids
        .iter()
        .map(|key_id| runner.keyspace().key(&probe_stamp_key(provider_id, key_id)))
        .collect::<Vec<_>>();
    let Ok(values) = redis::cmd("MGET")
        .arg(redis_keys)
        .query_async::<Vec<Option<String>>>(&mut connection)
        .await
    else {
        debug!("gateway pool quota probe: failed to read redis probe stamps");
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
    runner: Option<&RedisKvRunner>,
    provider_id: &str,
    key_ids: &[String],
    now_ts: u64,
    interval_seconds: u64,
) {
    let Some(runner) = runner else {
        return;
    };
    if key_ids.is_empty() {
        return;
    }

    let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
        debug!("gateway pool quota probe: failed to connect redis for stamp write");
        return;
    };
    let ttl_seconds = interval_seconds.saturating_mul(2).max(120);
    let value = now_ts.to_string();
    let mut pipeline = redis::pipe();
    for key_id in key_ids {
        pipeline
            .cmd("SETEX")
            .arg(runner.keyspace().key(&probe_stamp_key(provider_id, key_id)))
            .arg(ttl_seconds)
            .arg(&value)
            .ignore();
    }
    if pipeline.query_async::<()>(&mut connection).await.is_err() {
        debug!("gateway pool quota probe: failed to write redis probe stamps");
    }
}

async fn acquire_provider_probe_lock(
    runner: Option<&RedisLockRunner>,
    provider_id: &str,
) -> Option<RedisLockLease> {
    let Some(runner) = runner else {
        return None;
    };
    let lock_key = runner
        .keyspace()
        .lock_key(&format!("pool_quota_probe:{provider_id}"));
    let owner = format!("aether-gateway-pool-probe-{}", std::process::id());
    match runner
        .try_acquire(
            &lock_key,
            &owner,
            Some(POOL_QUOTA_PROBE_PROVIDER_LOCK_TTL_MS),
        )
        .await
    {
        Ok(lease) => lease,
        Err(err) => {
            debug!(
                provider_id,
                error = %err,
                "gateway pool quota probe: failed to acquire redis provider lock"
            );
            None
        }
    }
}

async fn release_provider_probe_lock(
    runner: Option<&RedisLockRunner>,
    lease: Option<RedisLockLease>,
) {
    let (Some(runner), Some(lease)) = (runner, lease) else {
        return;
    };
    if let Err(err) = runner.release(&lease).await {
        debug!(
            error = %err,
            "gateway pool quota probe: failed to release redis provider lock"
        );
    }
}

async fn select_keys_for_provider(
    state: &AppState,
    redis_kv: Option<&RedisKvRunner>,
    redis_lock: Option<&RedisLockRunner>,
    provider: &StoredProviderCatalogProvider,
    provider_type: &str,
    interval_seconds: u64,
    max_keys_per_provider: usize,
    now_ts: u64,
) -> Result<Vec<StoredProviderCatalogKey>, GatewayError> {
    let lease = acquire_provider_probe_lock(redis_lock, &provider.id).await;
    if redis_lock.is_some() && lease.is_none() {
        return Ok(Vec::new());
    }

    let result = async {
        let keys = state
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?
            .into_iter()
            .filter(|key| key.is_active)
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
        let probe_stamps = load_probe_timestamps(redis_kv, &provider.id, &key_ids).await;
        let selected_ids = select_pool_quota_probe_key_ids(
            &keys,
            provider_type,
            now_ts,
            interval_seconds,
            &probe_stamps,
            max_keys_per_provider,
        );
        if selected_ids.is_empty() {
            return Ok(Vec::new());
        }

        mark_probe_timestamps(
            redis_kv,
            &provider.id,
            &selected_ids,
            now_ts,
            interval_seconds,
        )
        .await;

        let mut keys_by_id = keys
            .into_iter()
            .map(|key| (key.id.clone(), key))
            .collect::<BTreeMap<_, _>>();
        Ok(selected_ids
            .into_iter()
            .filter_map(|key_id| keys_by_id.remove(&key_id))
            .collect::<Vec<_>>())
    }
    .await;

    release_provider_probe_lock(redis_lock, lease).await;
    result
}

fn endpoint_for_probe(
    provider_type: &str,
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Option<StoredProviderCatalogEndpoint> {
    provider_oauth_runtime_endpoint_for_provider(provider_type, endpoints)
}

async fn refresh_provider_probe_keys(
    admin_state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    provider_type: &str,
    keys: Vec<StoredProviderCatalogKey>,
) -> Result<Option<Value>, GatewayError> {
    match provider_type {
        "codex" => {
            refresh_codex_provider_quota_locally(admin_state, provider, endpoint, keys, None).await
        }
        "kiro" => {
            refresh_kiro_provider_quota_locally(admin_state, provider, endpoint, keys, None).await
        }
        "antigravity" => {
            refresh_antigravity_provider_quota_locally(admin_state, provider, endpoint, keys, None)
                .await
        }
        _ => Ok(None),
    }
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
                Some((
                    provider,
                    provider_type,
                    pool_config.probing_interval_minutes,
                ))
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

    let redis_kv = state.redis_kv_runner();
    let redis_lock = state.data.oauth_refresh_lock_runner();
    let admin_state = AdminAppState::new(state);
    let now_ts = now_unix_secs();
    let mut summary = PoolQuotaProbeRunSummary {
        providers_checked: providers.len(),
        ..PoolQuotaProbeRunSummary::empty()
    };

    for (provider, provider_type, interval_minutes) in providers {
        let endpoints = endpoints_by_provider
            .remove(&provider.id)
            .unwrap_or_default();
        let Some(endpoint) = endpoint_for_probe(&provider_type, &endpoints) else {
            summary.providers_skipped += 1;
            debug!(
                provider_id = %provider.id,
                provider_type,
                "gateway pool quota probe skipped provider without active quota endpoint"
            );
            continue;
        };

        let interval_seconds = interval_minutes.clamp(1, 1440).saturating_mul(60);
        let keys = select_keys_for_provider(
            state,
            redis_kv.as_ref(),
            redis_lock.as_ref(),
            &provider,
            &provider_type,
            interval_seconds,
            config.max_keys_per_provider,
            now_ts,
        )
        .await?;
        if keys.is_empty() {
            continue;
        }

        let selected_count = keys.len();
        summary.providers_probed += 1;
        summary.selected_keys += selected_count;

        let provider_short_id = provider.id.chars().take(8).collect::<String>();
        match refresh_provider_probe_keys(&admin_state, &provider, &endpoint, &provider_type, keys)
            .await
        {
            Ok(payload) => {
                update_summary_from_payload(&mut summary, selected_count, payload.as_ref());
                let probe_success = payload
                    .as_ref()
                    .and_then(|value| value.get("success"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let probe_failed = payload
                    .as_ref()
                    .and_then(|value| value.get("failed"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                info!(
                    provider_id = %provider_short_id,
                    provider_type,
                    selected = selected_count,
                    success = probe_success,
                    failed = probe_failed,
                    "gateway pool quota probe completed"
                );
            }
            Err(err) => {
                summary.failed += selected_count;
                warn!(
                    provider_id = %provider_short_id,
                    provider_type,
                    selected = selected_count,
                    error = ?err,
                    "gateway pool quota probe failed"
                );
            }
        }
    }

    Ok(summary)
}

pub(crate) async fn perform_pool_quota_probe_once(
    state: &AppState,
) -> Result<PoolQuotaProbeRunSummary, GatewayError> {
    perform_pool_quota_probe_once_with_config(state, PoolQuotaProbeWorkerConfig::from_env()).await
}

pub(crate) fn spawn_pool_quota_probe_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return None;
    }

    let config = PoolQuotaProbeWorkerConfig::from_env();
    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.scan_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(err) = perform_pool_quota_probe_once_with_config(&state, config).await {
                warn!(
                    error = ?err,
                    "gateway pool quota probe worker tick failed"
                );
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn parses_quota_updated_at_seconds_and_milliseconds() {
        assert_eq!(
            extract_quota_updated_at(
                "codex",
                Some(&json!({ "codex": { "updated_at": 1_700_000_000 } }))
            ),
            Some(1_700_000_000)
        );
        assert_eq!(
            extract_quota_updated_at(
                "kiro",
                Some(&json!({ "kiro": { "updated_at": 1_700_000_000_000_u64 } }))
            ),
            Some(1_700_000_000)
        );
    }
}
