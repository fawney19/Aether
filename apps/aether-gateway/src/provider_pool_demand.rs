use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_runtime_state::RuntimeState;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use tracing::debug;
use uuid::Uuid;

const PROVIDER_POOL_IN_FLIGHT_TOKENS_PREFIX: &str = "ap:provider_pool:in_flight";
const PROVIDER_POOL_DEMAND_SNAPSHOT_PREFIX: &str = "ap:provider_pool:demand";
const PROVIDER_POOL_BURST_PENDING_PREFIX: &str = "ap:quota_probe:burst_pending";
const PROVIDER_POOL_IN_FLIGHT_TOKEN_TTL_MS: u64 = 120_000;
const PROVIDER_POOL_IN_FLIGHT_RENEW_MS: u64 = 30_000;
const PROVIDER_POOL_DEMAND_SNAPSHOT_TTL_SECONDS: u64 = 6 * 60 * 60;
const PROVIDER_POOL_DEMAND_ALPHA: f64 = 0.2;
const PROVIDER_POOL_DEMAND_HEADROOM: f64 = 1.2;
const PROVIDER_POOL_DEMAND_FLOOR: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProviderPoolDemandSnapshot {
    pub(crate) in_flight: usize,
    pub(crate) ema_in_flight: f64,
    pub(crate) desired_hot: usize,
    pub(crate) sampled_at_unix_ms: u64,
}

pub(crate) struct ProviderPoolInFlightGuard {
    runtime: Arc<RuntimeState>,
    tokens_key: String,
    token: String,
    stop_renewal: Arc<AtomicBool>,
    renew_handle: Option<JoinHandle<()>>,
    released: bool,
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
        self.stop_renewal.store(true, Ordering::Release);
        if let Some(handle) = self.renew_handle.take() {
            handle.abort();
        }
        if let Err(err) = self
            .runtime
            .score_remove(&self.tokens_key, &self.token)
            .await
        {
            debug!(
                error = ?err,
                "gateway provider pool demand: failed to release in-flight token"
            );
        }
    }
}

impl Drop for ProviderPoolInFlightGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        self.stop_renewal.store(true, Ordering::Release);
        if let Some(handle) = self.renew_handle.take() {
            handle.abort();
        }

        let runtime = self.runtime.clone();
        let tokens_key = self.tokens_key.clone();
        let token = self.token.clone();
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

    let tokens_key = in_flight_tokens_key(provider_id);
    let token = build_in_flight_token(request_id, candidate_id, key_id);
    if let Err(err) = runtime
        .score_set(&tokens_key, &token, token_expiry_score(current_unix_ms()))
        .await
    {
        debug!(
            provider_id,
            error = ?err,
            "gateway provider pool demand: failed to acquire in-flight token"
        );
        return None;
    }

    let stop_renewal = Arc::new(AtomicBool::new(false));
    let renew_handle = spawn_in_flight_renewal(
        runtime.clone(),
        tokens_key.clone(),
        token.clone(),
        stop_renewal.clone(),
    );

    Some(ProviderPoolInFlightGuard {
        runtime,
        tokens_key,
        token,
        stop_renewal,
        renew_handle: Some(renew_handle),
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

    #[tokio::test]
    async fn in_flight_guard_tracks_and_releases_provider_tokens() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let guard = acquire_provider_pool_in_flight_guard(
            runtime.clone(),
            "provider-1",
            "request-1",
            Some("candidate-1"),
            "key-1",
        )
        .await
        .expect("guard should be acquired");

        assert_eq!(
            provider_pool_live_in_flight_count(runtime.as_ref(), "provider-1").await,
            1
        );

        guard.release().await;

        assert_eq!(
            provider_pool_live_in_flight_count(runtime.as_ref(), "provider-1").await,
            0
        );
    }

    #[tokio::test]
    async fn demand_snapshot_uses_instant_in_flight_for_fast_rise_and_ema_for_fall() {
        let runtime = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        for idx in 0..10 {
            let guard = acquire_provider_pool_in_flight_guard(
                Arc::new(runtime.clone()),
                "provider-1",
                "request-1",
                Some(&format!("candidate-{idx}")),
                "key-1",
            )
            .await
            .expect("guard");
            std::mem::forget(guard);
        }

        let high = sample_provider_pool_demand(&runtime, "provider-1", 100, 50).await;
        assert_eq!(high.in_flight, 10);
        assert_eq!(high.desired_hot, 12);

        let key = in_flight_tokens_key("provider-1");
        let _ = runtime.score_remove_by_score(&key, f64::INFINITY).await;
        let low = sample_provider_pool_demand(&runtime, "provider-1", 100, 50).await;
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
