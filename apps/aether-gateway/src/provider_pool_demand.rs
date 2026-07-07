use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, OnceLock,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_runtime_state::RuntimeState;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tracing::debug;
use uuid::Uuid;

const PROVIDER_POOL_IN_FLIGHT_TOKENS_PREFIX: &str = "ap:provider_pool:in_flight";
const PROVIDER_POOL_DEMAND_SNAPSHOT_PREFIX: &str = "ap:provider_pool:demand";
const PROVIDER_POOL_BURST_PENDING_PREFIX: &str = "ap:quota_probe:burst_pending";
const PROVIDER_POOL_IN_FLIGHT_TOKEN_TTL_MS: u64 = 120_000;
const PROVIDER_POOL_IN_FLIGHT_RENEW_MS: u64 = 30_000;
const PROVIDER_POOL_IN_FLIGHT_ACQUIRE_TIMEOUT_ENV: &str =
    "AETHER_GATEWAY_PROVIDER_POOL_IN_FLIGHT_ACQUIRE_TIMEOUT_MS";
const PROVIDER_POOL_IN_FLIGHT_MODE_ENV: &str = "AETHER_GATEWAY_PROVIDER_POOL_IN_FLIGHT_MODE";
const DEFAULT_PROVIDER_POOL_IN_FLIGHT_ACQUIRE_TIMEOUT_MS: u64 = 10;
const PROVIDER_POOL_DEMAND_SNAPSHOT_TTL_SECONDS: u64 = 6 * 60 * 60;
const PROVIDER_POOL_DEMAND_ALPHA: f64 = 0.2;
const PROVIDER_POOL_DEMAND_HEADROOM: f64 = 1.2;
const PROVIDER_POOL_DEMAND_FLOOR: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderPoolInFlightMode {
    Local,
    Runtime,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProviderPoolDemandSnapshot {
    pub(crate) in_flight: usize,
    pub(crate) ema_in_flight: f64,
    pub(crate) desired_hot: usize,
    pub(crate) sampled_at_unix_ms: u64,
}

pub(crate) struct ProviderPoolInFlightGuard {
    kind: ProviderPoolInFlightGuardKind,
    released: bool,
}

enum ProviderPoolInFlightGuardKind {
    Local {
        provider_id: String,
        counter: Arc<AtomicUsize>,
    },
    Runtime {
        runtime: Arc<RuntimeState>,
        tokens_key: String,
        token: String,
        stop_renewal: Arc<AtomicBool>,
        renew_handle: Option<JoinHandle<()>>,
    },
}

impl ProviderPoolInFlightGuard {
    pub(crate) async fn release(mut self) {
        self.release_inner().await;
    }

    async fn release_inner(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        match &mut self.kind {
            ProviderPoolInFlightGuardKind::Local {
                provider_id,
                counter,
            } => decrement_local_provider_in_flight(provider_id, counter),
            ProviderPoolInFlightGuardKind::Runtime {
                runtime,
                tokens_key,
                token,
                stop_renewal,
                renew_handle,
            } => {
                stop_renewal.store(true, Ordering::Release);
                if let Some(handle) = renew_handle.take() {
                    handle.abort();
                }
                if let Err(err) = runtime.score_remove(tokens_key, token).await {
                    debug!(
                        error = ?err,
                        "gateway provider pool demand: failed to release in-flight token"
                    );
                }
            }
        }
    }
}

impl Drop for ProviderPoolInFlightGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        match &mut self.kind {
            ProviderPoolInFlightGuardKind::Local {
                provider_id,
                counter,
            } => decrement_local_provider_in_flight(provider_id, counter),
            ProviderPoolInFlightGuardKind::Runtime {
                runtime,
                tokens_key,
                token,
                stop_renewal,
                renew_handle,
            } => {
                stop_renewal.store(true, Ordering::Release);
                if let Some(handle) = renew_handle.take() {
                    handle.abort();
                }

                let runtime = runtime.clone();
                let tokens_key = tokens_key.clone();
                let token = token.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        if let Err(err) = runtime.score_remove(&tokens_key, &token).await {
                            debug!(
                                error = ?err,
                                "gateway provider pool demand: failed to release dropped in-flight token"
                            );
                        }
                    });
                }
            }
        }
    }
}

fn current_unix_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn in_flight_tokens_key(provider_id: &str) -> String {
    format!("{PROVIDER_POOL_IN_FLIGHT_TOKENS_PREFIX}:{provider_id}")
}

fn local_provider_in_flight_counts() -> &'static DashMap<String, Arc<AtomicUsize>> {
    static COUNTS: std::sync::OnceLock<DashMap<String, Arc<AtomicUsize>>> =
        std::sync::OnceLock::new();
    COUNTS.get_or_init(DashMap::new)
}

fn increment_local_provider_in_flight(provider_id: &str) -> Arc<AtomicUsize> {
    let counter_ref = local_provider_in_flight_counts()
        .entry(provider_id.to_string())
        .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
    counter_ref.fetch_add(1, Ordering::AcqRel);
    counter_ref.clone()
}

fn decrement_local_provider_in_flight(provider_id: &str, counter: &AtomicUsize) {
    let mut current = counter.load(Ordering::Acquire);
    while current > 0 {
        match counter.compare_exchange_weak(
            current,
            current - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
    if counter.load(Ordering::Acquire) == 0 {
        let _ = local_provider_in_flight_counts()
            .remove_if(provider_id, |_, stored| stored.load(Ordering::Acquire) == 0);
    }
}

fn local_provider_live_in_flight_count(provider_id: &str) -> usize {
    local_provider_in_flight_counts()
        .get(provider_id)
        .map(|counter| counter.load(Ordering::Acquire))
        .unwrap_or(0)
}

fn demand_snapshot_key(provider_id: &str) -> String {
    format!("{PROVIDER_POOL_DEMAND_SNAPSHOT_PREFIX}:{provider_id}")
}

pub(crate) fn provider_pool_burst_pending_key(provider_id: &str) -> String {
    format!("{PROVIDER_POOL_BURST_PENDING_PREFIX}:{provider_id}")
}

fn build_in_flight_token(request_id: &str, candidate_id: Option<&str>, key_id: &str) -> String {
    let request_id = request_id.trim();
    let candidate_id = candidate_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-");
    let key_id = key_id.trim();
    format!(
        "{}:{}:{}:{}",
        current_unix_ms(),
        request_id,
        candidate_id,
        if key_id.is_empty() { "-" } else { key_id },
    ) + &format!(":{}", Uuid::new_v4())
}

fn token_expiry_score(now_ms: u64) -> f64 {
    now_ms.saturating_add(PROVIDER_POOL_IN_FLIGHT_TOKEN_TTL_MS) as f64
}

fn provider_pool_in_flight_acquire_timeout() -> Duration {
    static TIMEOUT: OnceLock<Duration> = OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        let millis = std::env::var(PROVIDER_POOL_IN_FLIGHT_ACQUIRE_TIMEOUT_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_PROVIDER_POOL_IN_FLIGHT_ACQUIRE_TIMEOUT_MS);
        Duration::from_millis(millis)
    })
}

fn provider_pool_in_flight_mode() -> ProviderPoolInFlightMode {
    static MODE: OnceLock<ProviderPoolInFlightMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        parse_provider_pool_in_flight_mode(
            std::env::var(PROVIDER_POOL_IN_FLIGHT_MODE_ENV)
                .ok()
                .as_deref(),
        )
    })
}

fn parse_provider_pool_in_flight_mode(value: Option<&str>) -> ProviderPoolInFlightMode {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("runtime" | "redis" | "distributed") => ProviderPoolInFlightMode::Runtime,
        Some("off" | "none" | "false" | "0" | "disabled") => ProviderPoolInFlightMode::Off,
        _ => ProviderPoolInFlightMode::Local,
    }
}

fn spawn_in_flight_renewal(
    runtime: Arc<RuntimeState>,
    tokens_key: String,
    token: String,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_millis(PROVIDER_POOL_IN_FLIGHT_RENEW_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if stop.load(Ordering::Acquire) {
                break;
            }
            if let Err(err) = runtime
                .score_set(&tokens_key, &token, token_expiry_score(current_unix_ms()))
                .await
            {
                debug!(
                    error = ?err,
                    "gateway provider pool demand: failed to renew in-flight token"
                );
            }
        }
    })
}

pub(crate) async fn acquire_provider_pool_in_flight_guard(
    runtime: Arc<RuntimeState>,
    provider_id: &str,
    request_id: &str,
    candidate_id: Option<&str>,
    key_id: &str,
) -> Option<ProviderPoolInFlightGuard> {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return None;
    }

    match provider_pool_in_flight_mode() {
        ProviderPoolInFlightMode::Off => return None,
        ProviderPoolInFlightMode::Local => {
            let counter = increment_local_provider_in_flight(provider_id);
            return Some(ProviderPoolInFlightGuard {
                kind: ProviderPoolInFlightGuardKind::Local {
                    provider_id: provider_id.to_string(),
                    counter,
                },
                released: false,
            });
        }
        ProviderPoolInFlightMode::Runtime if runtime.is_memory() => {
            let counter = increment_local_provider_in_flight(provider_id);
            return Some(ProviderPoolInFlightGuard {
                kind: ProviderPoolInFlightGuardKind::Local {
                    provider_id: provider_id.to_string(),
                    counter,
                },
                released: false,
            });
        }
        ProviderPoolInFlightMode::Runtime => {}
    }

    let tokens_key = in_flight_tokens_key(provider_id);
    let token = build_in_flight_token(request_id, candidate_id, key_id);
    match tokio::time::timeout(
        provider_pool_in_flight_acquire_timeout(),
        runtime.score_set(&tokens_key, &token, token_expiry_score(current_unix_ms())),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            debug!(
                provider_id,
                error = ?err,
                "gateway provider pool demand: failed to acquire in-flight token"
            );
            return None;
        }
        Err(_) => {
            debug!(
                provider_id,
                timeout_ms = provider_pool_in_flight_acquire_timeout().as_millis() as u64,
                "gateway provider pool demand: skipped in-flight token after acquire timeout"
            );
            return None;
        }
    }

    let stop_renewal = Arc::new(AtomicBool::new(false));
    let renew_handle = spawn_in_flight_renewal(
        runtime.clone(),
        tokens_key.clone(),
        token.clone(),
        stop_renewal.clone(),
    );

    Some(ProviderPoolInFlightGuard {
        kind: ProviderPoolInFlightGuardKind::Runtime {
            runtime,
            tokens_key,
            token,
            stop_renewal,
            renew_handle: Some(renew_handle),
        },
        released: false,
    })
}

pub(crate) async fn provider_pool_live_in_flight_count(
    runtime: &RuntimeState,
    provider_id: &str,
) -> usize {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return 0;
    }
    match provider_pool_in_flight_mode() {
        ProviderPoolInFlightMode::Off => return 0,
        ProviderPoolInFlightMode::Local => return local_provider_live_in_flight_count(provider_id),
        ProviderPoolInFlightMode::Runtime if runtime.is_memory() => {
            return local_provider_live_in_flight_count(provider_id);
        }
        ProviderPoolInFlightMode::Runtime => {}
    }
    let key = in_flight_tokens_key(provider_id);
    let now_ms = current_unix_ms() as f64;
    if let Err(err) = runtime.score_remove_by_score(&key, now_ms).await {
        debug!(
            provider_id,
            error = ?err,
            "gateway provider pool demand: failed to prune expired in-flight tokens"
        );
    }
    runtime.score_len(&key).await.unwrap_or(0)
}

pub(crate) async fn provider_pool_burst_pending(runtime: &RuntimeState, provider_id: &str) -> bool {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return false;
    }
    runtime
        .kv_exists(&provider_pool_burst_pending_key(provider_id))
        .await
        .unwrap_or(false)
}

pub(crate) fn provider_pool_desired_hot(
    in_flight: usize,
    ema_in_flight: f64,
    total_active_keys: usize,
    max_keys_per_provider: usize,
) -> usize {
    let cap = total_active_keys.min(max_keys_per_provider);
    if cap == 0 {
        return 0;
    }
    let signal = ema_in_flight
        .max(in_flight as f64)
        .max(0.0)
        .min(usize::MAX as f64);
    let desired = (signal * PROVIDER_POOL_DEMAND_HEADROOM).ceil() as usize;
    desired.max(PROVIDER_POOL_DEMAND_FLOOR.min(cap)).min(cap)
}

fn parse_stored_demand_snapshot(raw: Option<String>) -> Option<ProviderPoolDemandSnapshot> {
    let mut snapshot: ProviderPoolDemandSnapshot = serde_json::from_str(&raw?).ok()?;
    if !snapshot.ema_in_flight.is_finite() || snapshot.ema_in_flight < 0.0 {
        snapshot.ema_in_flight = 0.0;
    }
    Some(snapshot)
}

async fn stored_demand_snapshot(
    runtime: &RuntimeState,
    provider_id: &str,
) -> Option<ProviderPoolDemandSnapshot> {
    runtime
        .kv_get(&demand_snapshot_key(provider_id))
        .await
        .ok()
        .and_then(parse_stored_demand_snapshot)
}

pub(crate) async fn read_provider_pool_demand_snapshot(
    runtime: &RuntimeState,
    provider_id: &str,
    total_active_keys: usize,
    max_keys_per_provider: usize,
) -> ProviderPoolDemandSnapshot {
    let in_flight = provider_pool_live_in_flight_count(runtime, provider_id).await;
    let stored = stored_demand_snapshot(runtime, provider_id).await;
    let ema_in_flight = stored
        .as_ref()
        .map(|snapshot| snapshot.ema_in_flight)
        .unwrap_or(in_flight as f64);
    ProviderPoolDemandSnapshot {
        in_flight,
        ema_in_flight,
        desired_hot: provider_pool_desired_hot(
            in_flight,
            ema_in_flight,
            total_active_keys,
            max_keys_per_provider,
        ),
        sampled_at_unix_ms: stored
            .map(|snapshot| snapshot.sampled_at_unix_ms)
            .unwrap_or(0),
    }
}

pub(crate) async fn sample_provider_pool_demand(
    runtime: &RuntimeState,
    provider_id: &str,
    total_active_keys: usize,
    max_keys_per_provider: usize,
) -> ProviderPoolDemandSnapshot {
    let in_flight = provider_pool_live_in_flight_count(runtime, provider_id).await;
    let previous = stored_demand_snapshot(runtime, provider_id).await;
    let previous_ema = previous
        .as_ref()
        .map(|snapshot| snapshot.ema_in_flight)
        .unwrap_or(in_flight as f64);
    let ema_in_flight = if previous.is_some() {
        previous_ema.mul_add(
            1.0 - PROVIDER_POOL_DEMAND_ALPHA,
            in_flight as f64 * PROVIDER_POOL_DEMAND_ALPHA,
        )
    } else {
        in_flight as f64
    }
    .max(0.0);
    let sampled_at_unix_ms = current_unix_ms();
    let snapshot = ProviderPoolDemandSnapshot {
        in_flight,
        ema_in_flight,
        desired_hot: provider_pool_desired_hot(
            in_flight,
            ema_in_flight,
            total_active_keys,
            max_keys_per_provider,
        ),
        sampled_at_unix_ms,
    };

    if let Ok(serialized) = serde_json::to_string(&snapshot) {
        if let Err(err) = runtime
            .kv_set(
                &demand_snapshot_key(provider_id),
                serialized,
                Some(Duration::from_secs(
                    PROVIDER_POOL_DEMAND_SNAPSHOT_TTL_SECONDS,
                )),
            )
            .await
        {
            debug!(
                provider_id,
                error = ?err,
                "gateway provider pool demand: failed to persist demand snapshot"
            );
        }
    }

    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};

    #[test]
    fn provider_pool_in_flight_mode_defaults_local_and_allows_runtime_or_off() {
        assert_eq!(
            parse_provider_pool_in_flight_mode(None),
            ProviderPoolInFlightMode::Local
        );
        assert_eq!(
            parse_provider_pool_in_flight_mode(Some("")),
            ProviderPoolInFlightMode::Local
        );
        assert_eq!(
            parse_provider_pool_in_flight_mode(Some("runtime")),
            ProviderPoolInFlightMode::Runtime
        );
        assert_eq!(
            parse_provider_pool_in_flight_mode(Some("redis")),
            ProviderPoolInFlightMode::Runtime
        );
        assert_eq!(
            parse_provider_pool_in_flight_mode(Some("off")),
            ProviderPoolInFlightMode::Off
        );
        assert_eq!(
            parse_provider_pool_in_flight_mode(Some("0")),
            ProviderPoolInFlightMode::Off
        );
    }

    #[tokio::test]
    async fn in_flight_guard_tracks_and_releases_provider_tokens() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let provider_id = "provider-guard-release";
        let guard = acquire_provider_pool_in_flight_guard(
            runtime.clone(),
            provider_id,
            "request-1",
            Some("candidate-1"),
            "key-1",
        )
        .await
        .expect("guard should be acquired");

        assert_eq!(
            provider_pool_live_in_flight_count(runtime.as_ref(), provider_id).await,
            1
        );

        guard.release().await;

        assert_eq!(
            provider_pool_live_in_flight_count(runtime.as_ref(), provider_id).await,
            0
        );
    }

    #[tokio::test]
    async fn demand_snapshot_uses_instant_in_flight_for_fast_rise_and_ema_for_fall() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let provider_id = "provider-demand-snapshot";
        let mut guards = Vec::new();
        for idx in 0..10 {
            let guard = acquire_provider_pool_in_flight_guard(
                Arc::new(runtime.clone()),
                provider_id,
                "request-1",
                Some(&format!("candidate-{idx}")),
                "key-1",
            )
            .await
            .expect("guard");
            guards.push(guard);
        }

        let high = sample_provider_pool_demand(&runtime, provider_id, 100, 50).await;
        assert_eq!(high.in_flight, 10);
        assert_eq!(high.desired_hot, 12);

        drop(guards);
        let low = sample_provider_pool_demand(&runtime, provider_id, 100, 50).await;
        assert_eq!(low.in_flight, 0);
        assert!(low.ema_in_flight > 0.0);
        assert!(low.desired_hot >= PROVIDER_POOL_DEMAND_FLOOR);
        assert!(low.desired_hot < high.desired_hot);
    }

    #[test]
    fn desired_hot_without_provider_limit_is_capped_by_active_keys() {
        assert_eq!(provider_pool_desired_hot(100, 100.0, 37, usize::MAX), 37);
    }
}
