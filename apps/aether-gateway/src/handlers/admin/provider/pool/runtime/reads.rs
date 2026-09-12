use super::keys::{
    pool_cooldown_index_key, pool_cooldown_key, pool_cooldown_keys, pool_cost_keys,
    pool_latency_keys, pool_lru_key, pool_sticky_key, pool_sticky_pattern,
};
use crate::handlers::admin::provider::pool::config::admin_provider_pool_cache_affinity_enabled;
use crate::handlers::admin::provider::shared::support::{
    admin_provider_pool_quota_probe_active_members_key, AdminProviderPoolConfig,
    AdminProviderPoolRuntimeState,
};
use crate::maintenance::PoolQuotaProbeWorkerConfig;
use crate::provider_pool_demand::{
    provider_pool_burst_pending, read_provider_pool_demand_snapshot,
};
use aether_pool_core::{normalize_enabled_pool_presets, PoolSchedulingPreset};
use aether_runtime_state::{DataLayerError, RuntimeState, ScoreWindowU64Stats};
use futures_util::future::join_all;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

const DEFAULT_POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT: usize = 512;
const MAX_POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT: usize = 10_000;
const POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT_ENV: &str =
    "AETHER_GATEWAY_ADMIN_POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT";

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn should_load_active_probe_members(pool_config: &AdminProviderPoolConfig) -> bool {
    pool_config.probing_enabled
}

fn pool_runtime_window_metric_key_limit() -> usize {
    std::env::var(POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT)
        .clamp(1, MAX_POOL_RUNTIME_WINDOW_METRIC_KEY_LIMIT)
}

fn bounded_runtime_window_metric_key_ids(key_ids: &[String], limit: usize) -> &[String] {
    let end = key_ids.len().min(limit.max(1));
    &key_ids[..end]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PoolRuntimeReadPurpose {
    Admin,
    Scheduling,
}

fn scheduling_window_metrics(pool_config: &AdminProviderPoolConfig) -> (bool, bool) {
    let presets = pool_config
        .scheduling_presets
        .iter()
        .map(|preset| PoolSchedulingPreset {
            preset: preset.preset.clone(),
            enabled: preset.enabled,
            mode: preset.mode.clone(),
        })
        .collect::<Vec<_>>();
    let active = normalize_enabled_pool_presets(&presets);
    let cost = pool_config.cost_limit_per_key_tokens.is_some()
        || active
            .iter()
            .any(|preset| matches!(preset.as_str(), "cost_first" | "quota_balanced"));
    let latency = active.iter().any(|preset| preset == "latency_first");
    (cost, latency)
}

async fn read_window_stats(
    runtime: &RuntimeState,
    keys: &[String],
    min_score: f64,
) -> Vec<ScoreWindowU64Stats> {
    let aggregates = match runtime.score_window_u64_stats_by_min(keys, min_score).await {
        Ok(values) => values,
        Err(err) => {
            warn!(
                "gateway provider pool: bounded window aggregation failed, using exact range reads: {err:?}"
            );
            vec![None; keys.len()]
        }
    };
    join_all(keys.iter().zip(aggregates).map(|(key, stats)| async move {
        match stats {
            Some(stats) => stats,
            // Large windows and failed aggregation retain the original exact
            // read. A missing aggregate must never be treated as zero cost.
            None => {
                let members = runtime
                    .score_range_by_min(key, min_score)
                    .await
                    .unwrap_or_default();
                ScoreWindowU64Stats::from_members(members.iter().map(String::as_str))
            }
        }
    }))
    .await
}

pub(crate) async fn read_provider_pool_sticky_bound_key_id(
    runtime: &RuntimeState,
    provider_id: &str,
    pool_config: &AdminProviderPoolConfig,
    sticky_session_token: Option<&str>,
) -> Option<String> {
    if pool_config.sticky_session_ttl_seconds == 0
        || !admin_provider_pool_cache_affinity_enabled(pool_config)
    {
        return None;
    }
    let sticky_session_token = sticky_session_token
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let sticky_key = pool_sticky_key(provider_id, sticky_session_token);
    let bound_key_id = runtime.kv_get(&sticky_key).await.ok().flatten()?;
    let cooldown_key = pool_cooldown_key(provider_id, &bound_key_id);
    match runtime.kv_exists(&cooldown_key).await {
        Ok(false) => {
            let _ = runtime
                .key_expire(
                    &sticky_key,
                    std::time::Duration::from_secs(pool_config.sticky_session_ttl_seconds),
                )
                .await;
            Some(bound_key_id)
        }
        Ok(true) => {
            let _ = runtime.kv_delete(&sticky_key).await;
            None
        }
        Err(err) => {
            warn!(
                "gateway admin provider pool: failed to validate sticky cooldown for provider {provider_id}: {:?}",
                err
            );
            Some(bound_key_id)
        }
    }
}

pub(crate) async fn read_admin_provider_pool_cooldown_counts(
    runtime: &RuntimeState,
    provider_ids: &[String],
) -> BTreeMap<String, usize> {
    join_all(provider_ids.iter().map(|provider_id| async move {
        let count = runtime
            .set_len(&pool_cooldown_index_key(provider_id))
            .await
            .unwrap_or(0);
        (provider_id.clone(), count)
    }))
    .await
    .into_iter()
    .collect()
}

pub(crate) async fn read_admin_provider_pool_runtime_state(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &[String],
    pool_config: &AdminProviderPoolConfig,
    sticky_session_token: Option<&str>,
) -> AdminProviderPoolRuntimeState {
    read_provider_pool_runtime_state(
        runtime,
        provider_id,
        key_ids,
        pool_config,
        sticky_session_token,
        PoolRuntimeReadPurpose::Admin,
    )
    .await
}

pub(crate) async fn read_provider_pool_scheduling_runtime_state(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &[String],
    pool_config: &AdminProviderPoolConfig,
    sticky_session_token: Option<&str>,
) -> AdminProviderPoolRuntimeState {
    read_provider_pool_runtime_state(
        runtime,
        provider_id,
        key_ids,
        pool_config,
        sticky_session_token,
        PoolRuntimeReadPurpose::Scheduling,
    )
    .await
}

async fn read_provider_pool_runtime_state(
    runtime: &RuntimeState,
    provider_id: &str,
    key_ids: &[String],
    pool_config: &AdminProviderPoolConfig,
    sticky_session_token: Option<&str>,
    purpose: PoolRuntimeReadPurpose,
) -> AdminProviderPoolRuntimeState {
    let include_admin_metrics = purpose == PoolRuntimeReadPurpose::Admin;
    let mut state = AdminProviderPoolRuntimeState::default();
    let cooldown_keys = pool_cooldown_keys(provider_id, key_ids);
    let metric_key_limit = if include_admin_metrics {
        pool_runtime_window_metric_key_limit()
    } else {
        key_ids.len()
    };
    // The admin display cap must not hide a candidate's strict cost limit.
    let metric_key_ids = bounded_runtime_window_metric_key_ids(key_ids, metric_key_limit);
    if metric_key_ids.len() < key_ids.len() {
        info!(
            event_name = "admin_pool_runtime_window_metrics_truncated",
            log_type = "event",
            provider_id,
            total_key_count = key_ids.len(),
            scanned_key_count = metric_key_ids.len(),
            metric_key_limit,
            "gateway limited admin pool runtime cost/latency window reads"
        );
    }
    let (load_cost, load_latency) = if include_admin_metrics {
        (true, true)
    } else {
        scheduling_window_metrics(pool_config)
    };
    let cost_keys = if load_cost {
        pool_cost_keys(provider_id, metric_key_ids)
    } else {
        Vec::new()
    };
    let latency_keys = if load_latency {
        pool_latency_keys(provider_id, metric_key_ids)
    } else {
        Vec::new()
    };
    let sticky_sessions_enabled = pool_config.sticky_session_ttl_seconds > 0
        && admin_provider_pool_cache_affinity_enabled(pool_config);

    state.sticky_bound_key_id = read_provider_pool_sticky_bound_key_id(
        runtime,
        provider_id,
        pool_config,
        sticky_session_token,
    )
    .await;

    if include_admin_metrics && sticky_sessions_enabled {
        let sticky_keys = runtime
            .scan_keys(&pool_sticky_pattern(provider_id), 200)
            .await
            .unwrap_or_default();
        state.total_sticky_sessions = sticky_keys.len();
        if !sticky_keys.is_empty() {
            let raw_keys = sticky_keys
                .iter()
                .map(|key| runtime.strip_namespace(key).to_string())
                .collect::<Vec<_>>();
            if let Ok(values) = runtime.kv_get_many(&raw_keys).await {
                for bound_key_id in values.into_iter().flatten() {
                    *state
                        .sticky_sessions_by_key
                        .entry(bound_key_id)
                        .or_insert(0) += 1;
                }
            }
        }
    }

    if should_load_active_probe_members(pool_config) {
        state.active_probe_member_ids = runtime
            .set_members(&admin_provider_pool_quota_probe_active_members_key(
                provider_id,
            ))
            .await
            .map(|values| {
                values
                    .into_iter()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
    }

    if include_admin_metrics || pool_config.probing_enabled {
        let probe_config = PoolQuotaProbeWorkerConfig::from_env();
        let demand_snapshot = read_provider_pool_demand_snapshot(
            runtime,
            provider_id,
            key_ids.len(),
            probe_config.max_keys_per_provider,
        )
        .await;
        state.provider_in_flight = demand_snapshot.in_flight;
        state.provider_ema_in_flight = demand_snapshot.ema_in_flight;
        state.provider_desired_hot = if pool_config.probing_enabled {
            demand_snapshot.desired_hot
        } else {
            0
        };
        state.provider_burst_pending =
            pool_config.probing_enabled && provider_pool_burst_pending(runtime, provider_id).await;
    }

    if !cooldown_keys.is_empty() {
        let cooldown_reasons = runtime
            .kv_get_many(&cooldown_keys)
            .await
            .unwrap_or_else(|_| vec![None; cooldown_keys.len()]);
        for (key_id, (cooldown_key, reason)) in key_ids
            .iter()
            .zip(cooldown_keys.iter().zip(cooldown_reasons))
        {
            if let Some(reason) = reason {
                state.cooldown_reason_by_key.insert(key_id.clone(), reason);
                if include_admin_metrics {
                    if let Ok(Some(ttl)) = runtime.kv_ttl_seconds(cooldown_key).await {
                        if let Ok(ttl_seconds) = u64::try_from(ttl) {
                            if ttl_seconds > 0 {
                                state
                                    .cooldown_ttl_by_key
                                    .insert(key_id.clone(), ttl_seconds);
                            }
                        }
                    }
                }
            }
        }
    }

    let now = current_unix_secs();
    let cost_window_start = now.saturating_sub(pool_config.cost_window_seconds) as f64;
    let latency_window_start = now.saturating_sub(pool_config.latency_window_seconds) as f64;
    let (cost_results, latency_results) = tokio::join!(
        read_window_stats(runtime, &cost_keys, cost_window_start),
        read_window_stats(runtime, &latency_keys, latency_window_start),
    );
    for (key_id, stats) in metric_key_ids.iter().zip(cost_results) {
        if stats.sum > 0 {
            state
                .cost_window_usage_by_key
                .insert(key_id.clone(), stats.sum);
        }
    }

    for (key_id, stats) in metric_key_ids.iter().zip(latency_results) {
        if stats.positive_count == 0 {
            continue;
        }
        let average = stats.sum as f64 / stats.positive_count as f64;
        if average.is_finite() && average >= 0.0 {
            state.latency_avg_ms_by_key.insert(key_id.clone(), average);
        }
    }

    if (pool_config.lru_enabled
        || pool_config
            .scheduling_presets
            .iter()
            .any(|item| item.enabled))
        && !key_ids.is_empty()
    {
        if let Ok(scores) = runtime
            .score_many(&pool_lru_key(provider_id), key_ids)
            .await
        {
            for (key_id, score) in key_ids.iter().zip(scores) {
                if let Some(score) = score {
                    state.lru_score_by_key.insert(key_id.clone(), score);
                }
            }
        }
    }

    state
}

pub(crate) async fn read_admin_provider_pool_cooldown_count(
    runtime: &RuntimeState,
    provider_id: &str,
) -> usize {
    runtime
        .set_len(&pool_cooldown_index_key(provider_id))
        .await
        .unwrap_or(0)
}

pub(crate) async fn read_admin_provider_pool_cooldown_key_ids(
    runtime: &RuntimeState,
    provider_id: &str,
) -> Vec<String> {
    runtime
        .set_members(&pool_cooldown_index_key(provider_id))
        .await
        .unwrap_or_default()
}

pub(crate) async fn read_admin_provider_pool_key_cooldown_reason(
    runtime: &RuntimeState,
    provider_id: &str,
    key_id: &str,
) -> Result<Option<String>, DataLayerError> {
    runtime
        .kv_get(&pool_cooldown_key(provider_id, key_id))
        .await
}

#[cfg(test)]
mod tests {
    use super::super::keys::{pool_cooldown_key, pool_cost_key, pool_latency_key, pool_sticky_key};
    use super::{
        bounded_runtime_window_metric_key_ids, current_unix_secs,
        read_admin_provider_pool_runtime_state, read_provider_pool_scheduling_runtime_state,
        read_provider_pool_sticky_bound_key_id,
    };
    use crate::handlers::admin::provider::pool::config::admin_provider_pool_config_from_config_value;
    use crate::handlers::admin::provider::shared::support::AdminProviderPoolConfig;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RedisClientConfig, RuntimeState};
    use aether_test_support::ManagedRedisServer;
    use serde_json::json;
    use std::time::Duration;

    fn config(value: serde_json::Value) -> AdminProviderPoolConfig {
        admin_provider_pool_config_from_config_value(Some(&json!({ "pool_advanced": value })))
            .expect("pool config")
    }

    async fn seed_window_metrics(runtime: &RuntimeState, provider_id: &str, key_id: &str) {
        let now = current_unix_secs() as f64;
        for (key, member, timestamp) in [
            (pool_cost_key(provider_id, key_id), "current:70", now),
            (pool_cost_key(provider_id, key_id), "earlier:30", now - 1.0),
            (
                pool_cost_key(provider_id, key_id),
                "expired:999",
                now - 20_000.0,
            ),
            (pool_latency_key(provider_id, key_id), "first:10", now),
            (
                pool_latency_key(provider_id, key_id),
                "second:30",
                now - 1.0,
            ),
        ] {
            runtime
                .score_set(&key, member, timestamp)
                .await
                .expect("seed window");
        }
    }

    async fn admin_command_count(runtime: &RuntimeState) -> u64 {
        runtime
            .redis_diagnostics()
            .await
            .expect("diagnostics")
            .expect("Redis runtime")
            .lanes
            .into_iter()
            .find(|lane| lane.lane == "admin")
            .expect("admin lane")
            .command_count
    }

    #[tokio::test]
    async fn scheduling_runtime_aggregates_bounded_windows_and_falls_back_for_large_windows() {
        let redis = match ManagedRedisServer::start().await {
            Ok(server) => server,
            Err(err) if err.to_string().contains("No such file or directory") => {
                eprintln!("skipping redis-backed scheduling runtime test: {err}");
                return;
            }
            Err(err) => panic!("start Redis: {err}"),
        };
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: redis.redis_url().to_string(),
                key_prefix: Some("pool-window-aggregation-test".to_string()),
            },
            Some(1_000),
        )
        .await
        .expect("runtime Redis");
        let keys = vec!["bounded".to_string(), "large".to_string()];
        let now = current_unix_secs() as f64;
        for (key_id, count) in [(&keys[0], 512), (&keys[1], 2048)] {
            let cost_key = pool_cost_key("pool", key_id);
            for index in 0..count {
                runtime
                    .score_set(&cost_key, &format!("{index}:100"), now)
                    .await
                    .expect("seed cost window");
            }
            runtime
                .score_set(&cost_key, "expired:9999999", now - 20_000.0)
                .await
                .expect("expired cost");
            for (member, score) in [("first:10", now), ("second:30", now), ("zero:0", now)] {
                runtime
                    .score_set(&pool_latency_key("pool", key_id), member, score)
                    .await
                    .expect("seed latency");
            }
        }
        let pool_config = config(json!({
            "cost_limit_per_key_tokens": 50_000,
            "cost_window_seconds": 600,
            "latency_window_seconds": 600,
            "scheduling_presets": [{"preset": "latency_first", "enabled": true}]
        }));
        let scheduled = read_provider_pool_scheduling_runtime_state(
            &runtime,
            "pool",
            &keys,
            &pool_config,
            None,
        )
        .await;
        assert_eq!(
            scheduled.cost_window_usage_by_key.get("bounded"),
            Some(&51_200)
        );
        assert_eq!(
            scheduled.cost_window_usage_by_key.get("large"),
            Some(&204_800)
        );
        assert_eq!(scheduled.latency_avg_ms_by_key.get("bounded"), Some(&20.0));
        assert_eq!(scheduled.latency_avg_ms_by_key.get("large"), Some(&20.0));

        runtime
            .score_remove_by_score(&pool_cost_key("pool", "large"), f64::INFINITY)
            .await
            .expect("reset window");
        runtime
            .score_set(&pool_cost_key("pool", "large"), "after-reset:75", now)
            .await
            .expect("post-reset cost");
        let reset = read_provider_pool_scheduling_runtime_state(
            &runtime,
            "pool",
            &keys,
            &pool_config,
            None,
        )
        .await;
        assert_eq!(reset.cost_window_usage_by_key.get("large"), Some(&75));

        runtime
            .kv_set(&pool_cost_key("pool", "large"), "wrong-type", None)
            .await
            .expect("simulate invalid metric key");
        let partial = read_provider_pool_scheduling_runtime_state(
            &runtime,
            "pool",
            &keys,
            &pool_config,
            None,
        )
        .await;
        assert_eq!(
            partial.cost_window_usage_by_key.get("bounded"),
            Some(&51_200),
            "one failed aggregate must not discard another key's strict cost check"
        );
    }

    #[tokio::test]
    async fn scheduling_runtime_skips_admin_scan_and_unused_window_queries() {
        let redis = match ManagedRedisServer::start().await {
            Ok(server) => server,
            Err(err) if err.to_string().contains("No such file or directory") => {
                eprintln!("skipping redis-backed scheduling runtime test: {err}");
                return;
            }
            Err(err) => panic!("Redis server should start: {err}"),
        };
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: redis.redis_url().to_string(),
                key_prefix: Some("scheduling-runtime-reads".to_string()),
            },
            Some(2_000),
        )
        .await
        .expect("Redis runtime");
        let pool_config = config(json!({}));
        let keys = vec!["ready".to_string(), "cooling".to_string()];
        seed_window_metrics(&runtime, "pool", "ready").await;
        for session in ["current", "other"] {
            runtime
                .kv_set(
                    &pool_sticky_key("pool", session),
                    "ready".to_string(),
                    Some(Duration::from_secs(60)),
                )
                .await
                .expect("seed sticky session");
        }
        runtime
            .kv_set(
                &pool_cooldown_key("pool", "cooling"),
                "rate_limit".to_string(),
                Some(Duration::from_secs(60)),
            )
            .await
            .expect("seed cooldown");

        let before = admin_command_count(&runtime).await;
        let scheduled = read_provider_pool_scheduling_runtime_state(
            &runtime,
            "pool",
            &keys,
            &pool_config,
            Some("current"),
        )
        .await;
        let after = admin_command_count(&runtime).await;

        assert_eq!(
            after - before,
            1,
            "only the diagnostics INFO may use the admin lane"
        );
        assert_eq!(scheduled.sticky_bound_key_id.as_deref(), Some("ready"));
        assert_eq!(
            scheduled
                .cooldown_reason_by_key
                .get("cooling")
                .map(String::as_str),
            Some("rate_limit")
        );
        assert!(scheduled.cooldown_ttl_by_key.is_empty());
        assert!(scheduled.cost_window_usage_by_key.is_empty());
        assert!(scheduled.latency_avg_ms_by_key.is_empty());
        assert_eq!(scheduled.total_sticky_sessions, 0);

        let admin = read_admin_provider_pool_runtime_state(
            &runtime,
            "pool",
            &keys,
            &pool_config,
            Some("current"),
        )
        .await;
        assert_eq!(admin.total_sticky_sessions, 2);
        assert_eq!(admin.sticky_sessions_by_key.get("ready"), Some(&2));
        assert_eq!(admin.cost_window_usage_by_key.get("ready"), Some(&100));
        assert_eq!(admin.latency_avg_ms_by_key.get("ready"), Some(&20.0));
        assert!(admin
            .cooldown_ttl_by_key
            .get("cooling")
            .is_some_and(|ttl| *ttl > 0));
    }

    #[tokio::test]
    async fn scheduling_runtime_loads_only_metrics_used_by_enabled_strategies() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let keys = vec!["key".to_string()];
        seed_window_metrics(&runtime, "pool", "key").await;
        for (value, expected_cost, expected_latency) in [
            (json!({}), false, false),
            (json!({"cost_limit_per_key_tokens": 100}), true, false),
            (json!({"cost_limit_per_key_tokens": 0}), true, false),
            (
                json!({"scheduling_presets": [{"preset": "cost_first", "enabled": true}]}),
                true,
                false,
            ),
            (
                json!({"scheduling_presets": [{"preset": "quota_balanced", "enabled": true}]}),
                true,
                false,
            ),
            (
                json!({"scheduling_presets": [{"preset": "latency_first", "enabled": true}]}),
                false,
                true,
            ),
            (
                json!({"scheduling_presets": [
                    {"preset": "cost_first", "enabled": false},
                    {"preset": "latency_first", "enabled": false}
                ]}),
                false,
                false,
            ),
        ] {
            let pool_config = config(value.clone());
            let snapshot = read_provider_pool_scheduling_runtime_state(
                &runtime,
                "pool",
                &keys,
                &pool_config,
                None,
            )
            .await;
            assert_eq!(
                snapshot.cost_window_usage_by_key.get("key").copied(),
                expected_cost.then_some(100),
                "config: {value}"
            );
            assert_eq!(
                snapshot.latency_avg_ms_by_key.get("key").copied(),
                expected_latency.then_some(20.0),
                "config: {value}"
            );
        }
    }

    #[tokio::test]
    async fn scheduling_runtime_checks_cost_for_candidates_beyond_admin_display_limit() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let keys = (0..513)
            .map(|index| format!("key-{index}"))
            .collect::<Vec<_>>();
        seed_window_metrics(&runtime, "pool", &keys[512]).await;
        let pool_config = config(json!({ "cost_limit_per_key_tokens": 100 }));
        let snapshot = read_provider_pool_scheduling_runtime_state(
            &runtime,
            "pool",
            &keys,
            &pool_config,
            None,
        )
        .await;
        assert_eq!(
            snapshot.cost_window_usage_by_key.get(&keys[512]),
            Some(&100)
        );
    }

    #[tokio::test]
    async fn scheduling_sticky_lookup_invalidates_a_cooled_down_binding() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let pool_config = config(json!({}));
        let sticky_key = pool_sticky_key("pool", "session");
        runtime
            .kv_set(&sticky_key, "key".to_string(), None)
            .await
            .expect("sticky session");
        runtime
            .kv_set(
                &pool_cooldown_key("pool", "key"),
                "rate_limit".to_string(),
                None,
            )
            .await
            .expect("cooldown");
        assert!(read_provider_pool_sticky_bound_key_id(
            &runtime,
            "pool",
            &pool_config,
            Some("session")
        )
        .await
        .is_none());
        assert!(!runtime
            .kv_exists(&sticky_key)
            .await
            .expect("sticky existence"));
    }

    #[test]
    fn runtime_window_metric_key_ids_are_bounded() {
        let key_ids = vec![
            "key-1".to_string(),
            "key-2".to_string(),
            "key-3".to_string(),
        ];

        let bounded = bounded_runtime_window_metric_key_ids(&key_ids, 2);

        assert_eq!(bounded, &key_ids[..2]);
    }

    #[test]
    fn runtime_window_metric_key_ids_keep_at_least_one_key() {
        let key_ids = vec!["key-1".to_string(), "key-2".to_string()];

        let bounded = bounded_runtime_window_metric_key_ids(&key_ids, 0);

        assert_eq!(bounded, &key_ids[..1]);
    }
}
