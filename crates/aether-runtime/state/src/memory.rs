use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::{
    DataLayerError, RuntimeQueueEntry, RuntimeQueueReclaimConfig, RuntimeQueueReclaimPage,
    RuntimeQueueStats, RuntimeQueueTransferOutcome,
};
use crate::{ScoreWindowU64Stats, UsageLimitCheck, SCORE_WINDOW_AGGREGATION_MEMBER_LIMIT};

const MEMORY_RATE_LIMIT_COUNTER_SHARD_COUNT: usize = 64;
const MEMORY_RATE_LIMIT_COUNTER_PRUNE_INTERVAL: u64 = 256;
const MEMORY_USAGE_LIMIT_PRUNE_INTERVAL: u64 = 256;
const DEFAULT_MAX_USAGE_LIMIT_WINDOWS: usize = 10_000;
const DEFAULT_MAX_USAGE_LIMIT_EVENTS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRuntimeStateConfig {
    pub max_kv_entries: usize,
    /// Maximum number of active sliding-window keys retained by the memory backend.
    pub max_usage_limit_windows: usize,
    /// Maximum number of event identities retained across all usage-limit windows.
    pub max_usage_limit_events: usize,
}

impl Default for MemoryRuntimeStateConfig {
    fn default() -> Self {
        Self {
            max_kv_entries: 10_000,
            max_usage_limit_windows: DEFAULT_MAX_USAGE_LIMIT_WINDOWS,
            max_usage_limit_events: DEFAULT_MAX_USAGE_LIMIT_EVENTS,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryKvEntry {
    pub(crate) value: String,
    pub(crate) inserted_at: Instant,
    pub(crate) expires_at: Option<Instant>,
}

impl MemoryKvEntry {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }
}

#[derive(Debug, Default)]
pub(crate) struct MemoryRuntimeBackend {
    config: MemoryRuntimeStateConfig,
    kv: Mutex<HashMap<String, MemoryKvEntry>>,
    counters: MemoryRateLimitCounters,
    usage_limits: Mutex<MemoryUsageLimitState>,
    sets: Mutex<HashMap<String, MemorySetEntry>>,
    scores: Mutex<HashMap<String, MemoryScoreEntry>>,
    queues: Mutex<HashMap<String, MemoryQueueStream>>,
    queue_seq: AtomicU64,
    locks: Mutex<HashMap<String, MemoryLockEntry>>,
    lock_fencing_seq: AtomicU64,
    semaphores: Mutex<HashMap<String, BTreeMap<String, u64>>>,
}

#[derive(Debug, Clone)]
struct MemoryUsageLimitWindow {
    window_ms: u64,
    expires_at_unix_ms: u64,
    events: HashMap<String, u64>,
}

#[derive(Debug, Default)]
struct MemoryUsageLimitState {
    windows: HashMap<String, MemoryUsageLimitWindow>,
    total_events: usize,
    operations_since_prune: u64,
    next_expiry_unix_ms: Option<u64>,
}

impl MemoryUsageLimitState {
    fn amortized_prune(&mut self, now_unix_ms: u64) {
        self.operations_since_prune = self.operations_since_prune.saturating_add(1);
        if self.operations_since_prune < MEMORY_USAGE_LIMIT_PRUNE_INTERVAL {
            return;
        }
        self.operations_since_prune = 0;
        if self
            .next_expiry_unix_ms
            .is_some_and(|expires_at| expires_at <= now_unix_ms)
        {
            self.prune_all(now_unix_ms);
        }
    }

    fn prune_all(&mut self, now_unix_ms: u64) {
        self.operations_since_prune = 0;
        let mut total_events = 0_usize;
        let mut next_expiry_unix_ms = None;
        self.windows.retain(|_, window| {
            if window.expires_at_unix_ms <= now_unix_ms {
                return false;
            }
            prune_usage_limit_events(&mut window.events, now_unix_ms, window.window_ms);
            if window.events.is_empty() {
                return false;
            }
            total_events = total_events.saturating_add(window.events.len());
            update_earliest_expiry(&mut next_expiry_unix_ms, window.expires_at_unix_ms);
            for timestamp in window.events.values() {
                update_earliest_expiry(
                    &mut next_expiry_unix_ms,
                    timestamp.saturating_add(window.window_ms),
                );
            }
            true
        });
        self.total_events = total_events;
        self.next_expiry_unix_ms = next_expiry_unix_ms;
    }

    fn prune_rule_window(&mut self, key: &str, now_unix_ms: u64, window_ms: u64) {
        if self
            .windows
            .get(key)
            .is_some_and(|window| window.expires_at_unix_ms <= now_unix_ms)
        {
            if let Some(window) = self.windows.remove(key) {
                self.total_events = self.total_events.saturating_sub(window.events.len());
            }
            return;
        }
        let Some(window) = self.windows.get_mut(key) else {
            return;
        };
        let before = window.events.len();
        window.window_ms = window_ms;
        prune_usage_limit_events(&mut window.events, now_unix_ms, window_ms);
        self.total_events = self
            .total_events
            .saturating_sub(before.saturating_sub(window.events.len()));
        update_earliest_expiry(&mut self.next_expiry_unix_ms, window.expires_at_unix_ms);
        for timestamp in window.events.values() {
            update_earliest_expiry(
                &mut self.next_expiry_unix_ms,
                timestamp.saturating_add(window_ms),
            );
        }
        if window.events.is_empty() {
            self.windows.remove(key);
        }
    }

    fn additions_for(&self, input: crate::UsageLimitInput<'_>) -> (usize, usize) {
        input.rules.iter().fold(
            (0_usize, 0_usize),
            |(additional_windows, additional_events), rule| match self.windows.get(rule.key) {
                Some(window) if window.events.contains_key(input.event_id) => {
                    (additional_windows, additional_events)
                }
                Some(_) => (additional_windows, additional_events.saturating_add(1)),
                None => (
                    additional_windows.saturating_add(1),
                    additional_events.saturating_add(1),
                ),
            },
        )
    }
}

#[derive(Debug, Clone)]
struct MemoryCounterEntry {
    value: u32,
    bucket: u64,
    expires_at: Instant,
}

#[derive(Debug, Default)]
struct MemoryRateLimitCounterShard {
    entries: HashMap<String, MemoryCounterEntry>,
    operations_since_prune: u64,
}

impl MemoryRateLimitCounterShard {
    fn amortized_prune(&mut self, now: Instant) {
        self.operations_since_prune = self.operations_since_prune.saturating_add(1);
        if self.operations_since_prune < MEMORY_RATE_LIMIT_COUNTER_PRUNE_INTERVAL {
            return;
        }
        self.operations_since_prune = 0;
        self.entries.retain(|_, entry| entry.expires_at > now);
    }
}

#[derive(Debug)]
struct MemoryRateLimitCounters {
    shards: [StdMutex<MemoryRateLimitCounterShard>; MEMORY_RATE_LIMIT_COUNTER_SHARD_COUNT],
}

impl Default for MemoryRateLimitCounters {
    fn default() -> Self {
        Self {
            shards: std::array::from_fn(|_| StdMutex::new(MemoryRateLimitCounterShard::default())),
        }
    }
}

#[derive(Debug, Default)]
struct MemorySetEntry {
    members: BTreeSet<String>,
    expires_at: Option<Instant>,
}

#[derive(Debug, Default)]
struct MemoryScoreEntry {
    scores: BTreeMap<String, f64>,
    expires_at: Option<Instant>,
}

#[derive(Debug, Default)]
struct MemoryQueueStream {
    entries: VecDeque<MemoryQueuedEntry>,
    groups: HashMap<String, MemoryConsumerGroup>,
    expires_at: Option<Instant>,
}

trait MemoryExpiringKey {
    fn is_expired(&self, now: Instant) -> bool;
    fn set_expires_at(&mut self, expires_at: Instant);
}

impl MemoryExpiringKey for MemorySetEntry {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }

    fn set_expires_at(&mut self, expires_at: Instant) {
        self.expires_at = Some(expires_at);
    }
}

impl MemoryExpiringKey for MemoryScoreEntry {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }

    fn set_expires_at(&mut self, expires_at: Instant) {
        self.expires_at = Some(expires_at);
    }
}

impl MemoryExpiringKey for MemoryQueueStream {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }

    fn set_expires_at(&mut self, expires_at: Instant) {
        self.expires_at = Some(expires_at);
    }
}

#[derive(Debug, Clone)]
struct MemoryQueuedEntry {
    sequence: u64,
    entry: RuntimeQueueEntry,
}

#[derive(Debug, Default)]
struct MemoryConsumerGroup {
    last_delivered_sequence: u64,
    pending: BTreeMap<String, MemoryPendingQueueEntry>,
}

#[derive(Debug, Clone)]
struct MemoryPendingQueueEntry {
    sequence: u64,
    entry: RuntimeQueueEntry,
    consumer: String,
    delivered_at: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryLockEntry {
    pub(crate) token: String,
    #[allow(dead_code)]
    pub(crate) owner: String,
    pub(crate) expires_at: Instant,
}

impl MemoryRuntimeBackend {
    pub(crate) fn new(config: MemoryRuntimeStateConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    pub(crate) async fn kv_set(&self, key: &str, value: String, ttl: Option<Duration>) {
        let mut kv = self.kv.lock().await;
        let now = Instant::now();
        if ttl.is_some_and(|ttl| ttl.is_zero()) {
            kv.remove(key);
            return;
        }
        prune_kv(&mut kv, now);
        while kv.len() >= self.config.max_kv_entries.max(1) {
            let Some(oldest_key) = kv
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            kv.remove(&oldest_key);
        }
        kv.insert(
            key.to_string(),
            MemoryKvEntry {
                value,
                inserted_at: now,
                expires_at: ttl.map(|ttl| now + ttl),
            },
        );
    }

    pub(crate) async fn kv_set_if_absent(&self, key: &str, value: String, ttl: Duration) -> bool {
        let mut kv = self.kv.lock().await;
        let now = Instant::now();
        prune_kv(&mut kv, now);
        if kv.contains_key(key) {
            return false;
        }
        while kv.len() >= self.config.max_kv_entries.max(1) {
            let Some(oldest_key) = kv
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            kv.remove(&oldest_key);
        }
        kv.insert(
            key.to_string(),
            MemoryKvEntry {
                value,
                inserted_at: now,
                expires_at: Some(now + ttl),
            },
        );
        true
    }

    pub(crate) fn kv_set_nowait(&self, key: &str, value: String, ttl: Option<Duration>) -> bool {
        let Ok(mut kv) = self.kv.try_lock() else {
            return false;
        };
        let now = Instant::now();
        if ttl.is_some_and(|ttl| ttl.is_zero()) {
            kv.remove(key);
            return true;
        }
        prune_kv(&mut kv, now);
        while kv.len() >= self.config.max_kv_entries.max(1) {
            let Some(oldest_key) = kv
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            kv.remove(&oldest_key);
        }
        kv.insert(
            key.to_string(),
            MemoryKvEntry {
                value,
                inserted_at: now,
                expires_at: ttl.map(|ttl| now + ttl),
            },
        );
        true
    }

    pub(crate) async fn kv_get(&self, key: &str) -> Option<String> {
        let mut kv = self.kv.lock().await;
        get_fresh_locked(&mut kv, key, Instant::now())
    }

    pub(crate) async fn kv_take(&self, key: &str) -> Option<String> {
        let mut kv = self.kv.lock().await;
        let now = Instant::now();
        let entry = kv.remove(key)?;
        if entry.is_expired(now) {
            return None;
        }
        Some(entry.value)
    }

    pub(crate) async fn kv_delete(&self, key: &str) -> bool {
        let kv_deleted = self.kv.lock().await.remove(key).is_some();
        let set_deleted = self.sets.lock().await.remove(key).is_some();
        let score_deleted = self.scores.lock().await.remove(key).is_some();
        let queue_deleted = self.queues.lock().await.remove(key).is_some();
        kv_deleted || set_deleted || score_deleted || queue_deleted
    }

    pub(crate) async fn kv_delete_many(&self, keys: &[String]) -> usize {
        let keys = keys.iter().cloned().collect::<BTreeSet<_>>();
        let mut deleted = BTreeSet::new();
        let mut kv = self.kv.lock().await;
        for key in &keys {
            if kv.remove(key).is_some() {
                deleted.insert(key.clone());
            }
        }
        drop(kv);
        let mut sets = self.sets.lock().await;
        for key in &keys {
            if sets.remove(key).is_some() {
                deleted.insert(key.clone());
            }
        }
        drop(sets);
        let mut scores = self.scores.lock().await;
        for key in &keys {
            if scores.remove(key).is_some() {
                deleted.insert(key.clone());
            }
        }
        drop(scores);
        let mut queues = self.queues.lock().await;
        for key in &keys {
            if queues.remove(key).is_some() {
                deleted.insert(key.clone());
            }
        }
        deleted.len()
    }

    pub(crate) async fn kv_exists(&self, key: &str) -> bool {
        if self.kv_get(key).await.is_some() {
            return true;
        }
        let now = Instant::now();
        let mut sets = self.sets.lock().await;
        prune_memory_key(&mut sets, key, now);
        if sets.contains_key(key) {
            return true;
        }
        drop(sets);
        let mut scores = self.scores.lock().await;
        prune_memory_key(&mut scores, key, now);
        if scores.contains_key(key) {
            return true;
        }
        drop(scores);
        let mut queues = self.queues.lock().await;
        prune_memory_key(&mut queues, key, now);
        queues.contains_key(key)
    }

    pub(crate) async fn kv_ttl_seconds(&self, key: &str) -> Option<i64> {
        let mut kv = self.kv.lock().await;
        let now = Instant::now();
        let entry = kv.get(key).cloned()?;
        if entry.is_expired(now) {
            kv.remove(key);
            return None;
        }
        Some(
            entry
                .expires_at
                .map(|expires_at| {
                    expires_at
                        .saturating_duration_since(now)
                        .as_secs()
                        .try_into()
                        .unwrap_or(i64::MAX)
                })
                .unwrap_or(-1),
        )
    }

    pub(crate) async fn key_expire(&self, key: &str, ttl: Duration) -> bool {
        let now = Instant::now();
        if ttl.is_zero() {
            let kv_deleted = self.kv.lock().await.remove(key).is_some();
            let set_deleted = self.sets.lock().await.remove(key).is_some();
            let score_deleted = self.scores.lock().await.remove(key).is_some();
            let queue_deleted = self.queues.lock().await.remove(key).is_some();
            return kv_deleted || set_deleted || score_deleted || queue_deleted;
        }

        let expires_at = now + ttl;
        {
            let mut kv = self.kv.lock().await;
            if let Some(entry) = kv.get_mut(key) {
                if entry.is_expired(now) {
                    kv.remove(key);
                } else {
                    entry.expires_at = Some(expires_at);
                    return true;
                }
            }
        }
        if set_memory_key_expiry(&self.sets, key, expires_at, now).await {
            return true;
        }
        if set_memory_key_expiry(&self.scores, key, expires_at, now).await {
            return true;
        }
        if set_memory_key_expiry(&self.queues, key, expires_at, now).await {
            return true;
        }
        false
    }

    pub(crate) async fn kv_scan(&self, pattern: &str) -> Vec<String> {
        let now = Instant::now();
        let mut keys = BTreeSet::new();
        let mut kv = self.kv.lock().await;
        prune_kv(&mut kv, now);
        keys.extend(
            kv.keys()
                .filter(|key| key_matches_pattern(key, pattern))
                .cloned(),
        );
        drop(kv);
        let mut sets = self.sets.lock().await;
        prune_expiring_map(&mut sets, now);
        keys.extend(
            sets.keys()
                .filter(|key| key_matches_pattern(key, pattern))
                .cloned(),
        );
        drop(sets);
        let mut scores = self.scores.lock().await;
        prune_expiring_map(&mut scores, now);
        keys.extend(
            scores
                .keys()
                .filter(|key| key_matches_pattern(key, pattern))
                .cloned(),
        );
        drop(scores);
        let mut queues = self.queues.lock().await;
        prune_expiring_map(&mut queues, now);
        keys.extend(
            queues
                .keys()
                .filter(|key| key_matches_pattern(key, pattern))
                .cloned(),
        );
        keys.into_iter().collect()
    }

    pub(crate) async fn check_and_consume_rate_limit(
        &self,
        user_key: &str,
        key_key: &str,
        bucket: u64,
        user_limit: u32,
        key_limit: u32,
        ttl: Duration,
    ) -> Result<crate::RateLimitCheck, crate::DataLayerError> {
        // A user's API keys belong to the same rate-limit partition, so both
        // counters can be checked and updated atomically under one shard lock.
        let shard_index = memory_rate_limit_counter_shard_index(user_key);
        let mut shard = self.counters.shards[shard_index].lock().map_err(|_| {
            DataLayerError::UnexpectedValue("memory rate-limit counter lock poisoned".to_string())
        })?;
        let now = Instant::now();
        shard.amortized_prune(now);
        prune_rate_limit_counter(&mut shard.entries, user_key, bucket, now);
        prune_rate_limit_counter(&mut shard.entries, key_key, bucket, now);

        if user_limit > 0 {
            let user_count = shard
                .entries
                .get(user_key)
                .filter(|entry| entry.bucket == bucket)
                .map(|entry| entry.value)
                .unwrap_or_default();
            if user_count >= user_limit {
                return Ok(crate::RateLimitCheck::Rejected {
                    scope: crate::RateLimitScope::User,
                    limit: user_limit,
                });
            }
        }

        if key_limit > 0 {
            let key_count = shard
                .entries
                .get(key_key)
                .filter(|entry| entry.bucket == bucket)
                .map(|entry| entry.value)
                .unwrap_or_default();
            if key_count >= key_limit {
                return Ok(crate::RateLimitCheck::Rejected {
                    scope: crate::RateLimitScope::Key,
                    limit: key_limit,
                });
            }
        }

        let mut remaining = None::<u32>;
        let expires_at = now + ttl;
        if user_limit > 0 {
            let next = shard
                .entries
                .entry(user_key.to_string())
                .and_modify(|entry| {
                    entry.bucket = bucket;
                    entry.value = entry.value.saturating_add(1);
                    entry.expires_at = expires_at;
                })
                .or_insert(MemoryCounterEntry {
                    value: 1,
                    bucket,
                    expires_at,
                })
                .value;
            remaining = Some(user_limit.saturating_sub(next));
        }
        if key_limit > 0 {
            let next = shard
                .entries
                .entry(key_key.to_string())
                .and_modify(|entry| {
                    entry.bucket = bucket;
                    entry.value = entry.value.saturating_add(1);
                    entry.expires_at = expires_at;
                })
                .or_insert(MemoryCounterEntry {
                    value: 1,
                    bucket,
                    expires_at,
                })
                .value;
            let key_remaining = key_limit.saturating_sub(next);
            remaining = Some(remaining.map_or(key_remaining, |value| value.min(key_remaining)));
        }
        Ok(crate::RateLimitCheck::Allowed {
            remaining: remaining.unwrap_or(0),
        })
    }

    pub(crate) async fn check_and_consume_usage_limits(
        &self,
        input: crate::UsageLimitInput<'_>,
    ) -> Result<UsageLimitCheck, DataLayerError> {
        let mut state = self.usage_limits.lock().await;
        state.amortized_prune(input.now_unix_ms);

        for (index, rule) in input.rules.iter().enumerate() {
            let window_ms = rule.window_seconds.saturating_mul(1_000);
            state.prune_rule_window(rule.key, input.now_unix_ms, window_ms);
            let Some(window) = state.windows.get(rule.key) else {
                continue;
            };
            if window.events.contains_key(input.event_id) {
                continue;
            }
            if window.events.len() as u64 >= rule.limit {
                let earliest = window
                    .events
                    .values()
                    .copied()
                    .min()
                    .unwrap_or(input.now_unix_ms);
                let retry_after_ms = earliest
                    .saturating_add(window_ms)
                    .saturating_sub(input.now_unix_ms);
                return Ok(UsageLimitCheck::Rejected {
                    rule_index: index,
                    limit: rule.limit,
                    retry_after: retry_after_ms.saturating_add(999) / 1_000,
                });
            }
        }

        let (mut additional_windows, mut additional_events) = state.additions_for(input);
        if state.windows.len().saturating_add(additional_windows)
            > self.config.max_usage_limit_windows
            || state.total_events.saturating_add(additional_events)
                > self.config.max_usage_limit_events
        {
            // Redis drops an idle sorted-set key after its retention TTL. Force the equivalent full
            // cleanup before rejecting capacity so stale high-cardinality keys cannot pin memory.
            if state
                .next_expiry_unix_ms
                .is_some_and(|expires_at| expires_at <= input.now_unix_ms)
            {
                state.prune_all(input.now_unix_ms);
            }
            (additional_windows, additional_events) = state.additions_for(input);
        }
        if state.windows.len().saturating_add(additional_windows)
            > self.config.max_usage_limit_windows
            || state.total_events.saturating_add(additional_events)
                > self.config.max_usage_limit_events
        {
            return Err(DataLayerError::UnexpectedValue(format!(
                "runtime memory usage-limit capacity exhausted (windows {}/{}, events {}/{})",
                state.windows.len(),
                self.config.max_usage_limit_windows,
                state.total_events,
                self.config.max_usage_limit_events,
            )));
        }

        for rule in input.rules {
            let window_ms = rule.window_seconds.saturating_mul(1_000);
            let expires_at_unix_ms = input
                .now_unix_ms
                .saturating_add(rule.retention_seconds.saturating_mul(1_000));
            let inserted = match state.windows.entry(rule.key.to_string()) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let window = entry.get_mut();
                    window.window_ms = window_ms;
                    window.expires_at_unix_ms = expires_at_unix_ms;
                    match window.events.entry(input.event_id.to_string()) {
                        std::collections::hash_map::Entry::Occupied(_) => false,
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(input.now_unix_ms);
                            true
                        }
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(MemoryUsageLimitWindow {
                        window_ms,
                        expires_at_unix_ms,
                        events: HashMap::from([(input.event_id.to_string(), input.now_unix_ms)]),
                    });
                    true
                }
            };
            update_earliest_expiry(&mut state.next_expiry_unix_ms, expires_at_unix_ms);
            if inserted {
                state.total_events = state.total_events.saturating_add(1);
                update_earliest_expiry(
                    &mut state.next_expiry_unix_ms,
                    input.now_unix_ms.saturating_add(window_ms),
                );
            }
        }
        Ok(UsageLimitCheck::Allowed)
    }

    pub(crate) async fn release_usage_limits(
        &self,
        input: crate::UsageLimitReleaseInput<'_>,
    ) -> Result<(), DataLayerError> {
        let mut state = self.usage_limits.lock().await;
        for rule in input.rules {
            let mut remove_window = false;
            let mut removed_event = false;
            if let Some(window) = state.windows.get_mut(rule.key) {
                removed_event = window.events.remove(input.event_id).is_some();
                remove_window = window.events.is_empty();
            }
            if removed_event {
                state.total_events = state.total_events.saturating_sub(1);
            }
            if remove_window {
                state.windows.remove(rule.key);
            }
        }
        state.next_expiry_unix_ms = state
            .windows
            .values()
            .flat_map(|window| {
                std::iter::once(window.expires_at_unix_ms).chain(
                    window
                        .events
                        .values()
                        .map(|timestamp| timestamp.saturating_add(window.window_ms)),
                )
            })
            .min();
        Ok(())
    }

    pub(crate) fn rate_limit_count(&self, key: &str, bucket: u64) -> Result<u32, DataLayerError> {
        let now = Instant::now();
        let mut total = 0_u32;
        // Key counters are co-located with their owning user's shard. Count
        // reads are diagnostic-only, so scan shards without reintroducing a
        // global index or lock on the request hot path.
        for shard in &self.counters.shards {
            let mut shard = shard.lock().map_err(|_| {
                DataLayerError::UnexpectedValue(
                    "memory rate-limit counter lock poisoned".to_string(),
                )
            })?;
            shard.amortized_prune(now);
            prune_rate_limit_counter(&mut shard.entries, key, bucket, now);
            total = total.saturating_add(
                shard
                    .entries
                    .get(key)
                    .filter(|entry| entry.bucket == bucket)
                    .map(|entry| entry.value)
                    .unwrap_or_default(),
            );
        }
        Ok(total)
    }

    pub(crate) async fn set_add(&self, key: &str, member: &str) -> bool {
        let mut sets = self.sets.lock().await;
        prune_memory_key(&mut sets, key, Instant::now());
        sets.entry(key.to_string())
            .or_default()
            .members
            .insert(member.to_string())
    }

    pub(crate) fn set_add_nowait(&self, key: &str, member: &str) -> bool {
        let Ok(mut sets) = self.sets.try_lock() else {
            return false;
        };
        prune_memory_key(&mut sets, key, Instant::now());
        sets.entry(key.to_string())
            .or_default()
            .members
            .insert(member.to_string())
    }

    pub(crate) async fn set_remove(&self, key: &str, member: &str) -> bool {
        let mut sets = self.sets.lock().await;
        prune_memory_key(&mut sets, key, Instant::now());
        sets.get_mut(key)
            .is_some_and(|entry| entry.members.remove(member))
    }

    pub(crate) async fn set_members(&self, key: &str) -> Vec<String> {
        let mut sets = self.sets.lock().await;
        prune_memory_key(&mut sets, key, Instant::now());
        sets.get(key)
            .map(|entry| entry.members.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) async fn set_len(&self, key: &str) -> usize {
        let mut sets = self.sets.lock().await;
        prune_memory_key(&mut sets, key, Instant::now());
        sets.get(key).map_or(0, |entry| entry.members.len())
    }

    pub(crate) async fn score_set(&self, key: &str, member: &str, score: f64) {
        let mut scores = self.scores.lock().await;
        prune_memory_key(&mut scores, key, Instant::now());
        scores
            .entry(key.to_string())
            .or_default()
            .scores
            .insert(member.to_string(), score);
    }

    pub(crate) async fn score_many(&self, key: &str, members: &[String]) -> Vec<Option<f64>> {
        let mut scores = self.scores.lock().await;
        prune_memory_key(&mut scores, key, Instant::now());
        members
            .iter()
            .map(|member| {
                scores
                    .get(key)
                    .and_then(|entry| entry.scores.get(member))
                    .copied()
            })
            .collect()
    }

    pub(crate) async fn score_range_by_min(&self, key: &str, min_score: f64) -> Vec<String> {
        let mut scores = self.scores.lock().await;
        prune_memory_key(&mut scores, key, Instant::now());
        scores
            .get(key)
            .map(|entry| sorted_score_members(&entry.scores, |score| score >= min_score))
            .unwrap_or_default()
    }

    pub(crate) async fn score_window_u64_stats_by_min(
        &self,
        keys: &[String],
        min_score: f64,
    ) -> Vec<Option<ScoreWindowU64Stats>> {
        let mut scores = self.scores.lock().await;
        keys.iter()
            .map(|key| {
                prune_memory_key(&mut scores, key, Instant::now());
                let members = scores
                    .get(key)
                    .into_iter()
                    .flat_map(|entry| entry.scores.iter())
                    .filter(|(_, score)| **score >= min_score)
                    .take(SCORE_WINDOW_AGGREGATION_MEMBER_LIMIT + 1)
                    .map(|(member, _)| member.as_str())
                    .collect::<Vec<_>>();
                (members.len() <= SCORE_WINDOW_AGGREGATION_MEMBER_LIMIT)
                    .then(|| ScoreWindowU64Stats::from_members(members))
            })
            .collect()
    }

    pub(crate) async fn score_remove_by_score(&self, key: &str, max_score: f64) -> usize {
        let mut scores = self.scores.lock().await;
        prune_memory_key(&mut scores, key, Instant::now());
        let Some(entry) = scores.get_mut(key) else {
            return 0;
        };
        let before = entry.scores.len();
        entry.scores.retain(|_, score| *score > max_score);
        before.saturating_sub(entry.scores.len())
    }

    pub(crate) async fn score_remove(&self, key: &str, member: &str) -> bool {
        let mut scores = self.scores.lock().await;
        prune_memory_key(&mut scores, key, Instant::now());
        scores
            .get_mut(key)
            .is_some_and(|entry| entry.scores.remove(member).is_some())
    }

    pub(crate) async fn score_remove_by_rank(&self, key: &str, start: i64, stop: i64) -> usize {
        let mut scores = self.scores.lock().await;
        prune_memory_key(&mut scores, key, Instant::now());
        let Some(entry) = scores.get_mut(key) else {
            return 0;
        };
        let Some((start, stop)) = normalize_redis_rank_range(entry.scores.len(), start, stop)
        else {
            return 0;
        };
        let members = sorted_score_members(&entry.scores, |_| true);
        let remove = members
            .into_iter()
            .enumerate()
            .filter_map(|(index, member)| (index >= start && index <= stop).then_some(member))
            .collect::<Vec<_>>();
        let before = entry.scores.len();
        for member in remove {
            entry.scores.remove(&member);
        }
        before.saturating_sub(entry.scores.len())
    }

    pub(crate) async fn score_len(&self, key: &str) -> usize {
        let mut scores = self.scores.lock().await;
        prune_memory_key(&mut scores, key, Instant::now());
        scores.get(key).map_or(0, |entry| entry.scores.len())
    }

    pub(crate) async fn queue_append(
        &self,
        stream: &str,
        fields: BTreeMap<String, String>,
        maxlen: Option<usize>,
    ) -> String {
        // Stream IDs must follow insertion order, including when appenders wait for this lock.
        let mut queues = self.queues.lock().await;
        let sequence = self
            .queue_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let id = format!("{sequence}-0");
        prune_memory_key(&mut queues, stream, Instant::now());
        let stream_state = queues.entry(stream.to_string()).or_default();
        stream_state.entries.push_back(MemoryQueuedEntry {
            sequence,
            entry: RuntimeQueueEntry {
                id: id.clone(),
                fields,
            },
        });
        if let Some(maxlen) = maxlen.filter(|value| *value > 0) {
            while stream_state.entries.len() > maxlen {
                let Some(removed) = stream_state.entries.pop_front() else {
                    break;
                };
                remove_pending_from_all_groups(stream_state, &removed.entry.id);
            }
        }
        id
    }

    pub(crate) async fn queue_ensure_consumer_group(
        &self,
        stream: &str,
        group: &str,
        start_id: &str,
    ) -> Result<(), DataLayerError> {
        let mut queues = self.queues.lock().await;
        prune_memory_key(&mut queues, stream, Instant::now());
        let stream_state = queues.entry(stream.to_string()).or_default();
        if stream_state.groups.contains_key(group) {
            return Ok(());
        }
        let last_delivered_sequence = match start_id {
            "$" => stream_state
                .entries
                .back()
                .map(|entry| entry.sequence)
                .unwrap_or_default(),
            _ => parse_memory_stream_sequence(start_id)?,
        };
        stream_state.groups.insert(
            group.to_string(),
            MemoryConsumerGroup {
                last_delivered_sequence,
                pending: BTreeMap::new(),
            },
        );
        Ok(())
    }

    pub(crate) async fn queue_read(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        count: usize,
        block_ms: Option<u64>,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        let deadline = block_ms.map(|value| Instant::now() + Duration::from_millis(value.max(1)));
        loop {
            let entries = {
                let mut queues = self.queues.lock().await;
                prune_memory_key(&mut queues, stream, Instant::now());
                let Some(stream_state) = queues.get_mut(stream) else {
                    return Err(DataLayerError::InvalidInput(format!(
                        "runtime queue stream {stream} does not exist"
                    )));
                };
                let Some(group_state) = stream_state.groups.get_mut(group) else {
                    return Err(DataLayerError::InvalidInput(format!(
                        "runtime queue group {group} does not exist for stream {stream}"
                    )));
                };
                let now = Instant::now();
                let mut delivered = Vec::new();
                let last_delivered_sequence = group_state.last_delivered_sequence;
                for queued in stream_state
                    .entries
                    .iter()
                    .filter(|entry| entry.sequence > last_delivered_sequence)
                    .take(count.max(1))
                {
                    group_state.last_delivered_sequence = queued.sequence;
                    group_state.pending.insert(
                        queued.entry.id.clone(),
                        MemoryPendingQueueEntry {
                            sequence: queued.sequence,
                            entry: queued.entry.clone(),
                            consumer: consumer.to_string(),
                            delivered_at: now,
                        },
                    );
                    delivered.push(queued.entry.clone());
                }
                delivered
            };
            if !entries.is_empty() {
                return Ok(entries);
            }
            let Some(deadline) = deadline else {
                return Ok(Vec::new());
            };
            if Instant::now() >= deadline {
                return Ok(Vec::new());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[cfg(test)]
    async fn queue_claim_stale(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        start_id: &str,
        config: RuntimeQueueReclaimConfig,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        Ok(self
            .queue_claim_stale_page(stream, group, consumer, start_id, config)
            .await?
            .entries)
    }

    pub(crate) async fn queue_claim_stale_page(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        start_id: &str,
        config: RuntimeQueueReclaimConfig,
    ) -> Result<RuntimeQueueReclaimPage, DataLayerError> {
        let start_sequence = parse_memory_stream_sequence(start_id)?;
        let min_idle = Duration::from_millis(config.min_idle_ms.max(1));
        let now = Instant::now();
        let mut queues = self.queues.lock().await;
        prune_memory_key(&mut queues, stream, now);
        let Some(stream_state) = queues.get_mut(stream) else {
            return Err(DataLayerError::InvalidInput(format!(
                "runtime queue stream {stream} does not exist"
            )));
        };
        let Some(group_state) = stream_state.groups.get_mut(group) else {
            return Err(DataLayerError::InvalidInput(format!(
                "runtime queue group {group} does not exist for stream {stream}"
            )));
        };
        let ids = group_state
            .pending
            .values()
            .filter(|entry| entry.sequence >= start_sequence)
            .filter(|entry| now.saturating_duration_since(entry.delivered_at) >= min_idle)
            .map(|entry| (entry.sequence, entry.entry.id.clone()))
            .collect::<Vec<_>>();
        let mut ids = ids;
        ids.sort_by_key(|(sequence, _)| *sequence);
        let count = config.count.max(1);
        let next_start_id = ids
            .get(count)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| "0-0".to_string());

        let mut claimed = Vec::new();
        for (_, id) in ids.into_iter().take(count) {
            if let Some(pending) = group_state.pending.get_mut(&id) {
                pending.consumer = consumer.to_string();
                pending.delivered_at = now;
                claimed.push(pending.entry.clone());
            }
        }
        Ok(RuntimeQueueReclaimPage {
            next_start_id,
            entries: claimed,
            deleted_ids: Vec::new(),
        })
    }

    pub(crate) async fn queue_transfer_pending_to_stream(
        &self,
        source: &str,
        group: &str,
        entry_id: &str,
        destination: &str,
        destination_fields: &BTreeMap<String, String>,
    ) -> Result<RuntimeQueueTransferOutcome, DataLayerError> {
        crate::validate_runtime_queue_transfer(
            source,
            group,
            entry_id,
            destination,
            destination_fields,
        )?;
        // Match ordinary memory append ownership without copying a large payload while locked.
        let destination_fields = destination_fields.clone();
        let mut queues = self.queues.lock().await;
        let now = Instant::now();
        prune_memory_key(&mut queues, source, now);
        let source_state = queues.get(source).ok_or_else(|| {
            DataLayerError::InvalidInput(format!("runtime queue stream {source} does not exist"))
        })?;
        let group_state = source_state.groups.get(group).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "runtime queue group {group} does not exist for stream {source}"
            ))
        })?;
        // Memory trimming/deletion already removes PEL entries. Absence here cannot prove
        // archival, and must not delete an unread entry or append another dead letter.
        if !group_state.pending.contains_key(entry_id) {
            return Ok(RuntimeQueueTransferOutcome::NotPending);
        }

        let previous_sequence = self
            .queue_seq
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |sequence| {
                sequence.checked_add(1)
            })
            .map_err(|_| {
                DataLayerError::UnexpectedValue("runtime queue sequence exhausted".to_string())
            })?;
        let sequence = previous_sequence + 1;
        let destination_id = format!("{sequence}-0");
        prune_memory_key(&mut queues, destination, now);
        queues
            .entry(destination.to_string())
            .or_default()
            .entries
            .push_back(MemoryQueuedEntry {
                sequence,
                entry: RuntimeQueueEntry {
                    id: destination_id.clone(),
                    fields: destination_fields,
                },
            });

        // No await occurs between archive creation and source removal. Cancellation can only
        // happen while waiting for the mutex, so it cannot leave a half-completed transfer.
        let source_state = queues.get_mut(source).expect("validated source stream");
        let acked = usize::from(
            source_state
                .groups
                .get_mut(group)
                .expect("validated source group")
                .pending
                .remove(entry_id)
                .is_some(),
        );
        let before = source_state.entries.len();
        source_state
            .entries
            .retain(|entry| entry.entry.id != entry_id);
        remove_pending_from_all_groups(source_state, entry_id);
        Ok(RuntimeQueueTransferOutcome::Transferred {
            destination_id,
            acked,
            deleted: before.saturating_sub(source_state.entries.len()),
        })
    }

    pub(crate) async fn queue_ack(
        &self,
        stream: &str,
        group: &str,
        ids: &[String],
    ) -> Result<usize, DataLayerError> {
        let mut queues = self.queues.lock().await;
        prune_memory_key(&mut queues, stream, Instant::now());
        let Some(stream_state) = queues.get_mut(stream) else {
            return Err(DataLayerError::InvalidInput(format!(
                "runtime queue stream {stream} does not exist"
            )));
        };
        let Some(group_state) = stream_state.groups.get_mut(group) else {
            return Err(DataLayerError::InvalidInput(format!(
                "runtime queue group {group} does not exist for stream {stream}"
            )));
        };
        Ok(ids
            .iter()
            .filter(|id| group_state.pending.remove(*id).is_some())
            .count())
    }

    pub(crate) async fn queue_delete(&self, stream: &str, ids: &[String]) -> usize {
        let mut queues = self.queues.lock().await;
        prune_memory_key(&mut queues, stream, Instant::now());
        let Some(stream_state) = queues.get_mut(stream) else {
            return 0;
        };
        let ids = ids.iter().cloned().collect::<BTreeSet<_>>();
        let before = stream_state.entries.len();
        stream_state
            .entries
            .retain(|entry| !ids.contains(&entry.entry.id));
        for id in &ids {
            remove_pending_from_all_groups(stream_state, id);
        }
        before.saturating_sub(stream_state.entries.len())
    }

    pub(crate) async fn queue_stats(&self, stream: &str, group: Option<&str>) -> RuntimeQueueStats {
        let mut queues = self.queues.lock().await;
        let now = Instant::now();
        prune_memory_key(&mut queues, stream, now);
        let Some(stream_state) = queues.get(stream) else {
            return RuntimeQueueStats::default();
        };
        let stream_length = stream_state.entries.len() as u64;
        let Some(group_name) = group else {
            return RuntimeQueueStats {
                stream_length,
                ..RuntimeQueueStats::default()
            };
        };
        let Some(group_state) = stream_state.groups.get(group_name) else {
            return RuntimeQueueStats {
                stream_length,
                ..RuntimeQueueStats::default()
            };
        };
        let group_lag = stream_state
            .entries
            .iter()
            .filter(|entry| entry.sequence > group_state.last_delivered_sequence)
            .count() as u64;
        let oldest_pending_idle_ms = group_state
            .pending
            .values()
            .map(|entry| {
                now.saturating_duration_since(entry.delivered_at)
                    .as_millis() as u64
            })
            .max();

        RuntimeQueueStats {
            stream_length,
            group_pending: group_state.pending.len() as u64,
            group_lag: Some(group_lag),
            oldest_pending_idle_ms,
        }
    }

    pub(crate) async fn lock_try_acquire(
        &self,
        key: &str,
        owner: &str,
        token: String,
        ttl: Duration,
    ) -> Option<u64> {
        let mut locks = self.locks.lock().await;
        let now = Instant::now();
        locks.retain(|_, entry| entry.expires_at > now);
        if locks.contains_key(key) {
            return None;
        }
        let fencing_token = self
            .lock_fencing_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        locks.insert(
            key.to_string(),
            MemoryLockEntry {
                token,
                owner: owner.to_string(),
                expires_at: now + ttl,
            },
        );
        Some(fencing_token)
    }

    pub(crate) async fn lock_release(&self, key: &str, token: &str) -> bool {
        let mut locks = self.locks.lock().await;
        let now = Instant::now();
        if locks.get(key).is_some_and(|entry| entry.expires_at <= now) {
            locks.remove(key);
            return false;
        }
        if locks.get(key).is_some_and(|entry| entry.token == token) {
            locks.remove(key);
            return true;
        }
        false
    }

    pub(crate) async fn lock_renew(&self, key: &str, token: &str, ttl: Duration) -> bool {
        let mut locks = self.locks.lock().await;
        let now = Instant::now();
        if locks.get(key).is_some_and(|entry| entry.expires_at <= now) {
            locks.remove(key);
            return false;
        }
        if let Some(entry) = locks.get_mut(key) {
            if entry.token == token {
                entry.expires_at = now + ttl;
                return true;
            }
        }
        false
    }

    pub(crate) async fn semaphore_try_acquire(
        &self,
        key: &str,
        token: String,
        limit: usize,
        ttl_ms: u64,
    ) -> Result<usize, usize> {
        let now_ms = unix_time_ms();
        let expires_at = now_ms.saturating_add(ttl_ms);
        let mut semaphores = self.semaphores.lock().await;
        let holders = semaphores.entry(key.to_string()).or_default();
        holders.retain(|_, expires| *expires > now_ms);
        let count = holders.len();
        if count >= limit {
            return Err(count);
        }
        holders.insert(token, expires_at);
        Ok(holders.len())
    }

    pub(crate) async fn semaphore_renew(&self, key: &str, token: &str, ttl_ms: u64) -> bool {
        let now_ms = unix_time_ms();
        let mut semaphores = self.semaphores.lock().await;
        let Some(holders) = semaphores.get_mut(key) else {
            return false;
        };
        holders.retain(|_, expires| *expires > now_ms);
        if let Some(expires) = holders.get_mut(token) {
            *expires = now_ms.saturating_add(ttl_ms);
            return true;
        }
        false
    }

    pub(crate) async fn semaphore_release(&self, key: &str, token: &str) {
        let mut semaphores = self.semaphores.lock().await;
        if let Some(holders) = semaphores.get_mut(key) {
            holders.remove(token);
            if holders.is_empty() {
                semaphores.remove(key);
            }
        }
    }

    pub(crate) async fn semaphore_live_count(&self, key: &str) -> usize {
        let now_ms = unix_time_ms();
        let mut semaphores = self.semaphores.lock().await;
        let Some(holders) = semaphores.get_mut(key) else {
            return 0;
        };
        holders.retain(|_, expires| *expires > now_ms);
        holders.len()
    }
}

fn prune_usage_limit_events(events: &mut HashMap<String, u64>, now_unix_ms: u64, window_ms: u64) {
    let Some(cutoff) = now_unix_ms.checked_sub(window_ms) else {
        return;
    };
    events.retain(|_, timestamp| *timestamp > cutoff);
}

fn update_earliest_expiry(current: &mut Option<u64>, candidate: u64) {
    *current = Some(current.map_or(candidate, |existing| existing.min(candidate)));
}

fn get_fresh_locked(
    kv: &mut HashMap<String, MemoryKvEntry>,
    key: &str,
    now: Instant,
) -> Option<String> {
    let entry = kv.get(key).cloned()?;
    if entry.is_expired(now) {
        kv.remove(key);
        return None;
    }
    Some(entry.value)
}

fn prune_kv(kv: &mut HashMap<String, MemoryKvEntry>, now: Instant) {
    kv.retain(|_, entry| !entry.is_expired(now));
}

fn memory_rate_limit_counter_shard_index(key: &str) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % MEMORY_RATE_LIMIT_COUNTER_SHARD_COUNT
}

fn prune_rate_limit_counter(
    counters: &mut HashMap<String, MemoryCounterEntry>,
    key: &str,
    bucket: u64,
    now: Instant,
) {
    if counters
        .get(key)
        .is_some_and(|entry| entry.expires_at <= now || entry.bucket < bucket)
    {
        counters.remove(key);
    }
}

fn prune_memory_key<T>(values: &mut HashMap<String, T>, key: &str, now: Instant)
where
    T: MemoryExpiringKey,
{
    if values.get(key).is_some_and(|entry| entry.is_expired(now)) {
        values.remove(key);
    }
}

fn prune_expiring_map<T>(values: &mut HashMap<String, T>, now: Instant)
where
    T: MemoryExpiringKey,
{
    values.retain(|_, entry| !entry.is_expired(now));
}

async fn set_memory_key_expiry<T>(
    values: &Mutex<HashMap<String, T>>,
    key: &str,
    expires_at: Instant,
    now: Instant,
) -> bool
where
    T: MemoryExpiringKey,
{
    let mut values = values.lock().await;
    if values.get(key).is_some_and(|entry| entry.is_expired(now)) {
        values.remove(key);
        return false;
    }
    let Some(entry) = values.get_mut(key) else {
        return false;
    };
    entry.set_expires_at(expires_at);
    true
}

pub(crate) fn key_matches_pattern(key: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => key == pattern,
    }
}

fn sorted_score_members<F>(scores: &BTreeMap<String, f64>, include: F) -> Vec<String>
where
    F: Fn(f64) -> bool,
{
    let mut entries = scores
        .iter()
        .filter_map(|(member, score)| include(*score).then_some((member.clone(), *score)))
        .collect::<Vec<_>>();
    entries.sort_by(|(left_member, left_score), (right_member, right_score)| {
        left_score
            .total_cmp(right_score)
            .then_with(|| left_member.cmp(right_member))
    });
    entries.into_iter().map(|(member, _)| member).collect()
}

fn normalize_redis_rank_range(len: usize, start: i64, stop: i64) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let len = i64::try_from(len).ok()?;
    let mut start = if start < 0 { len + start } else { start };
    let mut stop = if stop < 0 { len + stop } else { stop };
    if start < 0 {
        start = 0;
    }
    if stop < 0 || start >= len || start > stop {
        return None;
    }
    if stop >= len {
        stop = len - 1;
    }
    Some((usize::try_from(start).ok()?, usize::try_from(stop).ok()?))
}

fn remove_pending_from_all_groups(stream: &mut MemoryQueueStream, id: &str) {
    for group in stream.groups.values_mut() {
        group.pending.remove(id);
    }
}

fn parse_memory_stream_sequence(id: &str) -> Result<u64, DataLayerError> {
    let Some((sequence, _)) = id.split_once('-') else {
        return Err(DataLayerError::InvalidInput(format!(
            "runtime queue stream id {id} must use redis stream id format"
        )));
    };
    sequence.parse::<u64>().map_err(|err| {
        DataLayerError::InvalidInput(format!("runtime queue stream id {id} is invalid: {err}"))
    })
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_queue_test_fields(index: usize) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "payload".to_string(),
                format!("record-{index}:{}\n\"\\\u{03bb}", "payload".repeat(8_192)),
            ),
            ("kind".to_string(), format!("event-{index}")),
            (String::new(), String::new()),
        ])
    }

    async fn age_memory_queue_pending(backend: &MemoryRuntimeBackend, stream: &str) {
        let stale = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .expect("test clock should support one second of history");
        let mut queues = backend.queues.lock().await;
        for group in queues
            .get_mut(stream)
            .expect("test stream")
            .groups
            .values_mut()
        {
            for pending in group.pending.values_mut() {
                pending.delivered_at = stale;
            }
        }
    }

    async fn memory_queue_transfer_fixture(
        backend: &MemoryRuntimeBackend,
        count: usize,
    ) -> Vec<RuntimeQueueEntry> {
        backend
            .queue_ensure_consumer_group("transfer:source", "workers", "0-0")
            .await
            .expect("source group");
        for index in 0..count {
            backend
                .queue_append("transfer:source", memory_queue_test_fields(index), None)
                .await;
        }
        backend
            .queue_read("transfer:source", "workers", "reader", count, None)
            .await
            .expect("pending source entries")
    }

    #[tokio::test]
    async fn memory_queue_transfer_preserves_fields_and_only_removes_the_target_entry() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let entries = memory_queue_transfer_fixture(&backend, 3).await;
        backend
            .queue_ensure_consumer_group("transfer:source", "other-workers", "0-0")
            .await
            .expect("second source group");
        backend
            .queue_read("transfer:source", "other-workers", "reader", 3, None)
            .await
            .expect("second group pending entries");
        let mut archived_fields = entries[1].fields.clone();
        archived_fields.insert("source_id".to_string(), entries[1].id.clone());
        archived_fields.insert(
            "error".to_string(),
            "invalid payload\noriginal retained".to_string(),
        );
        let outcome = backend
            .queue_transfer_pending_to_stream(
                "transfer:source",
                "workers",
                &entries[1].id,
                "transfer:archive",
                &archived_fields,
            )
            .await
            .expect("atomic transfer");
        let RuntimeQueueTransferOutcome::Transferred {
            destination_id,
            acked,
            deleted,
        } = outcome
        else {
            panic!("pending entry should transfer");
        };
        assert_eq!((acked, deleted), (1, 1));
        let queues = backend.queues.lock().await;
        let source = &queues["transfer:source"];
        let remaining = source
            .entries
            .iter()
            .map(|entry| &entry.entry)
            .collect::<Vec<_>>();
        assert_eq!(remaining, [&entries[0], &entries[2]]);
        for group in ["workers", "other-workers"] {
            let pending = &source.groups[group].pending;
            assert_eq!(pending.len(), 2);
            assert!(pending.contains_key(&entries[0].id));
            assert!(pending.contains_key(&entries[2].id));
        }
        let archive = &queues["transfer:archive"];
        assert_eq!(archive.entries.len(), 1);
        assert_eq!(archive.entries[0].entry.id, destination_id);
        assert_eq!(archive.entries[0].entry.fields, archived_fields);
    }

    #[tokio::test]
    async fn memory_queue_transfer_concurrent_and_repeated_attempts_archive_once() {
        let backend = std::sync::Arc::new(MemoryRuntimeBackend::new(
            MemoryRuntimeStateConfig::default(),
        ));
        let entries = memory_queue_transfer_fixture(&backend, 1).await;
        let fields = std::sync::Arc::new(entries[0].fields.clone());
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let backend = std::sync::Arc::clone(&backend);
            let fields = std::sync::Arc::clone(&fields);
            let entry_id = entries[0].id.clone();
            tasks.spawn(async move {
                backend
                    .queue_transfer_pending_to_stream(
                        "transfer:source",
                        "workers",
                        &entry_id,
                        "transfer:archive",
                        &fields,
                    )
                    .await
                    .expect("transfer attempt")
            });
        }
        let mut transferred = 0;
        let mut not_pending = 0;
        while let Some(outcome) = tasks.join_next().await {
            match outcome.expect("transfer task") {
                RuntimeQueueTransferOutcome::Transferred { acked, deleted, .. } => {
                    assert_eq!((acked, deleted), (1, 1));
                    transferred += 1;
                }
                RuntimeQueueTransferOutcome::NotPending => not_pending += 1,
            }
        }
        assert_eq!((transferred, not_pending), (1, 15));
        assert_eq!(
            backend
                .queue_transfer_pending_to_stream(
                    "transfer:source",
                    "workers",
                    &entries[0].id,
                    "transfer:archive",
                    &fields,
                )
                .await
                .expect("sequential retry"),
            RuntimeQueueTransferOutcome::NotPending
        );
        let stats = backend
            .queue_stats("transfer:source", Some("workers"))
            .await;
        assert_eq!((stats.stream_length, stats.group_pending), (0, 0));
        assert_eq!(
            backend
                .queue_stats("transfer:archive", None)
                .await
                .stream_length,
            1
        );
    }

    #[tokio::test]
    async fn memory_queue_transfer_retry_after_lost_response_does_not_archive_twice() {
        async fn commit_then_lose_response(
            backend: &MemoryRuntimeBackend,
            entry: &RuntimeQueueEntry,
        ) -> Result<RuntimeQueueTransferOutcome, DataLayerError> {
            backend
                .queue_transfer_pending_to_stream(
                    "transfer:source",
                    "workers",
                    &entry.id,
                    "transfer:archive",
                    &entry.fields,
                )
                .await?;
            Err(DataLayerError::TimedOut(
                "transfer response lost after commit".to_string(),
            ))
        }

        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let entries = memory_queue_transfer_fixture(&backend, 1).await;
        assert!(matches!(
            commit_then_lose_response(&backend, &entries[0]).await,
            Err(DataLayerError::TimedOut(_))
        ));
        assert_eq!(
            backend
                .queue_transfer_pending_to_stream(
                    "transfer:source",
                    "workers",
                    &entries[0].id,
                    "transfer:archive",
                    &entries[0].fields,
                )
                .await
                .expect("retry after lost response"),
            RuntimeQueueTransferOutcome::NotPending
        );
        let queues = backend.queues.lock().await;
        assert!(queues["transfer:source"].entries.is_empty());
        assert!(queues["transfer:source"].groups["workers"]
            .pending
            .is_empty());
        assert_eq!(queues["transfer:archive"].entries.len(), 1);
        assert_eq!(
            queues["transfer:archive"].entries[0].entry.fields,
            entries[0].fields
        );
    }

    #[tokio::test]
    async fn memory_queue_transfer_rejects_invalid_input_before_any_mutation() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let entries = memory_queue_transfer_fixture(&backend, 1).await;
        let entry = &entries[0];
        for invalid_id in [
            "",
            "1",
            "1-",
            "-1-0",
            "+1-0",
            "01-0",
            "1-00",
            "1-+0",
            "1-0x",
            "1-0-0",
            " 1-0",
            "1-0 ",
            "\u{0661}-0",
            "18446744073709551616-0",
            "1-18446744073709551616",
        ] {
            assert!(
                matches!(
                    backend
                        .queue_transfer_pending_to_stream(
                            "transfer:source",
                            "workers",
                            invalid_id,
                            "transfer:archive",
                            &entry.fields,
                        )
                        .await,
                    Err(DataLayerError::InvalidInput(_))
                ),
                "invalid entry id {invalid_id:?}"
            );
        }
        for (source, group, destination) in [
            ("", "workers", "transfer:archive"),
            ("transfer:source", " ", "transfer:archive"),
            ("transfer:source", "workers", ""),
            ("transfer:source", "workers", "transfer:source"),
            ("missing-source", "workers", "transfer:archive"),
            ("transfer:source", "missing-group", "transfer:archive"),
        ] {
            assert!(matches!(
                backend
                    .queue_transfer_pending_to_stream(
                        source,
                        group,
                        &entry.id,
                        destination,
                        &entry.fields
                    )
                    .await,
                Err(DataLayerError::InvalidInput(_))
            ));
        }
        assert!(matches!(
            backend
                .queue_transfer_pending_to_stream(
                    "transfer:source",
                    "workers",
                    &entry.id,
                    "transfer:archive",
                    &BTreeMap::new(),
                )
                .await,
            Err(DataLayerError::InvalidInput(_))
        ));
        let queues = backend.queues.lock().await;
        assert_eq!(queues.len(), 1);
        assert_eq!(queues["transfer:source"].entries[0].entry, *entry);
        assert!(queues["transfer:source"].groups["workers"]
            .pending
            .contains_key(&entry.id));
        assert_eq!(backend.queue_seq.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn memory_queue_transfer_archive_failure_keeps_source_pending() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let entries = memory_queue_transfer_fixture(&backend, 1).await;
        backend.queue_seq.store(u64::MAX, Ordering::Release);
        assert!(matches!(
            backend
                .queue_transfer_pending_to_stream(
                    "transfer:source",
                    "workers",
                    &entries[0].id,
                    "transfer:archive",
                    &entries[0].fields,
                )
                .await,
            Err(DataLayerError::UnexpectedValue(_))
        ));
        let queues = backend.queues.lock().await;
        assert_eq!(queues.len(), 1);
        assert_eq!(queues["transfer:source"].entries[0].entry, entries[0]);
        assert!(queues["transfer:source"].groups["workers"]
            .pending
            .contains_key(&entries[0].id));
    }

    #[tokio::test]
    async fn memory_queue_transfer_cancelled_lock_wait_has_no_side_effects() {
        use std::future::Future;
        use std::task::Poll;

        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let entries = memory_queue_transfer_fixture(&backend, 1).await;
        let queues = backend.queues.lock().await;
        let mut transfer = Box::pin(backend.queue_transfer_pending_to_stream(
            "transfer:source",
            "workers",
            &entries[0].id,
            "transfer:archive",
            &entries[0].fields,
        ));
        std::future::poll_fn(|cx| {
            assert!(transfer.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(transfer);
        assert_eq!(backend.queue_seq.load(Ordering::Acquire), 1);
        assert_eq!(queues.len(), 1);
        assert_eq!(queues["transfer:source"].entries[0].entry, entries[0]);
        assert!(queues["transfer:source"].groups["workers"]
            .pending
            .contains_key(&entries[0].id));
        drop(queues);
        assert_eq!(
            backend
                .queue_stats("transfer:archive", None)
                .await
                .stream_length,
            0
        );
    }

    #[tokio::test]
    async fn memory_queue_transfer_not_pending_does_not_archive_or_delete_unread_or_acked_entries()
    {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let entries = memory_queue_transfer_fixture(&backend, 1).await;
        let unread_id = backend
            .queue_append("transfer:source", memory_queue_test_fields(1), None)
            .await;
        backend
            .queue_ack("transfer:source", "workers", &[entries[0].id.clone()])
            .await
            .expect("ack without deleting");
        for id in [
            entries[0].id.as_str(),
            unread_id.as_str(),
            "1-1",
            "0-0",
            "18446744073709551615-18446744073709551615",
        ] {
            assert_eq!(
                backend
                    .queue_transfer_pending_to_stream(
                        "transfer:source",
                        "workers",
                        id,
                        "transfer:archive",
                        &entries[0].fields,
                    )
                    .await
                    .expect("valid but non-pending entry id"),
                RuntimeQueueTransferOutcome::NotPending
            );
        }
        let stats = backend
            .queue_stats("transfer:source", Some("workers"))
            .await;
        assert_eq!(
            (stats.stream_length, stats.group_pending, stats.group_lag),
            (2, 0, Some(1))
        );
        assert_eq!(
            backend
                .queue_stats("transfer:archive", None)
                .await
                .stream_length,
            0
        );
        assert_eq!(backend.queue_seq.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn memory_queue_transfer_retains_existing_trimmed_pending_semantics() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let entries = memory_queue_transfer_fixture(&backend, 1).await;
        backend
            .queue_append("transfer:source", memory_queue_test_fields(1), Some(1))
            .await;
        // Memory retention already removed both the original entry and its PEL copy.
        // NotPending does not claim that the retained caller copy was archived elsewhere.
        assert_eq!(
            backend
                .queue_transfer_pending_to_stream(
                    "transfer:source",
                    "workers",
                    &entries[0].id,
                    "transfer:archive",
                    &entries[0].fields,
                )
                .await
                .expect("trimmed entry"),
            RuntimeQueueTransferOutcome::NotPending
        );
        let stats = backend
            .queue_stats("transfer:source", Some("workers"))
            .await;
        assert_eq!((stats.stream_length, stats.group_pending), (1, 0));
        assert_eq!(
            backend
                .queue_stats("transfer:archive", None)
                .await
                .stream_length,
            0
        );
    }

    #[tokio::test]
    async fn memory_queue_waiting_append_does_not_reserve_an_out_of_order_sequence() {
        use std::future::Future;
        use std::task::Poll;

        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let stream = "queue:append-order";
        backend
            .queue_ensure_consumer_group(stream, "workers", "0-0")
            .await
            .expect("consumer group");
        let lock = backend.queues.lock().await;
        let mut first = Box::pin(backend.queue_append(
            stream,
            BTreeMap::from([("payload".to_string(), "first".to_string())]),
            None,
        ));
        let mut second = Box::pin(backend.queue_append(
            stream,
            BTreeMap::from([("payload".to_string(), "second".to_string())]),
            None,
        ));
        std::future::poll_fn(|cx| {
            assert!(first.as_mut().poll(cx).is_pending());
            assert!(second.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(
            backend.queue_seq.load(Ordering::Acquire),
            0,
            "an appender must own the insertion lock before assigning a stream sequence"
        );
        drop(lock);
        let (first_id, second_id) = tokio::join!(first, second);
        assert_eq!(first_id, "1-0");
        assert_eq!(second_id, "2-0");
        for (expected_id, expected_payload) in [(first_id, "first"), (second_id, "second")] {
            let entries = backend
                .queue_read(stream, "workers", "reader", 1, None)
                .await
                .expect("ordered delivery");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].id, expected_id);
            assert_eq!(entries[0].fields["payload"], expected_payload);
        }
        assert!(backend
            .queue_read(stream, "workers", "reader", 1, None)
            .await
            .expect("all entries delivered exactly once")
            .is_empty());
    }

    #[tokio::test]
    async fn memory_queue_reclaim_page_advances_and_rescans_after_reaching_the_end() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let stream = "queue:reclaim-page";
        backend
            .queue_ensure_consumer_group(stream, "workers", "0-0")
            .await
            .expect("consumer group");
        for index in 0..4 {
            backend
                .queue_append(stream, memory_queue_test_fields(index), None)
                .await;
        }
        let expected = backend
            .queue_read(stream, "workers", "reader", 4, None)
            .await
            .expect("initial delivery");
        age_memory_queue_pending(&backend, stream).await;
        backend
            .queues
            .lock()
            .await
            .get_mut(stream)
            .unwrap()
            .groups
            .get_mut("workers")
            .unwrap()
            .pending
            .get_mut(&expected[0].id)
            .unwrap()
            .delivered_at = Instant::now();
        let config = RuntimeQueueReclaimConfig {
            min_idle_ms: 500,
            count: 1,
        };
        let mut cursor = "0-0".to_string();
        for index in 1..4 {
            let page = backend
                .queue_claim_stale_page(stream, "workers", "reclaimer", &cursor, config)
                .await
                .expect("reclaim page");
            assert_eq!(page.entries.as_slice(), &expected[index..index + 1]);
            assert!(page.deleted_ids.is_empty());
            cursor = page.next_start_id;
            assert_eq!(
                cursor,
                expected
                    .get(index + 1)
                    .map_or("0-0", |entry| entry.id.as_str())
            );
        }
        let stale = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        backend
            .queues
            .lock()
            .await
            .get_mut(stream)
            .unwrap()
            .groups
            .get_mut("workers")
            .unwrap()
            .pending
            .get_mut(&expected[0].id)
            .unwrap()
            .delivered_at = stale;
        let page = backend
            .queue_claim_stale_page(stream, "workers", "reclaimer", &cursor, config)
            .await
            .expect("next scan rechecks the earlier fresh entry");
        assert_eq!(page.entries.as_slice(), &expected[..1]);
        assert_eq!(page.next_start_id, "0-0");
        assert!(page.deleted_ids.is_empty());
    }

    #[tokio::test]
    async fn memory_queue_read_batches_preserve_fields_and_independent_ownership() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let stream = "queue:read-ownership";
        backend
            .queue_ensure_consumer_group(stream, "workers", "0-0")
            .await
            .expect("consumer group");
        let mut expected = Vec::new();
        for index in 0..3 {
            let fields = memory_queue_test_fields(index);
            let id = backend.queue_append(stream, fields.clone(), None).await;
            expected.push(RuntimeQueueEntry { id, fields });
        }

        let mut first_batch = backend
            .queue_read(stream, "workers", "consumer-a", 2, None)
            .await
            .expect("first batch");
        assert_eq!(first_batch.as_slice(), &expected[..2]);
        let stats = backend.queue_stats(stream, Some("workers")).await;
        assert_eq!(stats.group_pending, 2);
        assert_eq!(stats.group_lag, Some(1));
        first_batch[0].id.clear();
        first_batch[0].fields.get_mut("payload").unwrap().clear();
        first_batch[0].fields.remove("kind");
        first_batch[1].fields.clear();

        let second_batch = backend
            .queue_read(stream, "workers", "consumer-a", 2, None)
            .await
            .expect("second batch");
        assert_eq!(second_batch.as_slice(), &expected[2..]);
        assert!(backend
            .queue_read(stream, "workers", "consumer-a", 2, None)
            .await
            .expect("all entries have been delivered")
            .is_empty());

        age_memory_queue_pending(&backend, stream).await;
        let mut reclaimed = backend
            .queue_claim_stale(
                stream,
                "workers",
                "consumer-b",
                "0-0",
                RuntimeQueueReclaimConfig {
                    min_idle_ms: 500,
                    count: 1,
                },
            )
            .await
            .expect("bounded reclaim");
        assert_eq!(reclaimed.as_slice(), &expected[..1]);
        reclaimed[0].fields.clear();
        age_memory_queue_pending(&backend, stream).await;
        assert_eq!(
            backend
                .queue_claim_stale(
                    stream,
                    "workers",
                    "consumer-c",
                    "0-0",
                    RuntimeQueueReclaimConfig {
                        min_idle_ms: 500,
                        count: 3
                    },
                )
                .await
                .expect("reclaim still owns original fields"),
            expected
        );

        backend
            .queue_ensure_consumer_group(stream, "later-group", "0-0")
            .await
            .expect("independent consumer group");
        assert_eq!(
            backend
                .queue_read(stream, "later-group", "consumer-d", 3, None)
                .await
                .expect("stream still owns original fields"),
            expected
        );
    }

    #[tokio::test]
    async fn memory_queue_read_ack_and_delete_preserve_pending_group_semantics() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let stream = "queue:ack-delete";
        let mut expected = Vec::new();
        for index in 0..3 {
            let fields = memory_queue_test_fields(index);
            let id = backend.queue_append(stream, fields.clone(), None).await;
            expected.push(RuntimeQueueEntry { id, fields });
        }
        for group in ["workers-a", "workers-b"] {
            backend
                .queue_ensure_consumer_group(stream, group, "0-0")
                .await
                .expect("consumer group");
            assert_eq!(
                backend
                    .queue_read(stream, group, "reader", 3, None)
                    .await
                    .expect("read batch"),
                expected
            );
        }
        assert_eq!(
            backend
                .queue_ack(stream, "workers-a", std::slice::from_ref(&expected[0].id))
                .await
                .expect("ack only the first group"),
            1
        );
        assert_eq!(
            backend
                .queue_delete(stream, &[expected[1].id.clone(), "missing-0".to_string()])
                .await,
            1
        );
        age_memory_queue_pending(&backend, stream).await;
        for (group, wanted) in [
            ("workers-a", vec![expected[2].clone()]),
            ("workers-b", vec![expected[0].clone(), expected[2].clone()]),
        ] {
            let stats = backend.queue_stats(stream, Some(group)).await;
            assert_eq!(stats.stream_length, 2);
            assert_eq!(stats.group_pending, wanted.len() as u64);
            assert_eq!(
                backend
                    .queue_claim_stale(
                        stream,
                        group,
                        "reclaimer",
                        "0-0",
                        RuntimeQueueReclaimConfig {
                            min_idle_ms: 500,
                            count: 3
                        },
                    )
                    .await
                    .expect("deleted entries cannot be reclaimed"),
                wanted
            );
        }
        assert_eq!(
            backend
                .queue_delete(stream, &[expected[0].id.clone(), expected[2].id.clone()])
                .await,
            2
        );
        for group in ["workers-a", "workers-b"] {
            let stats = backend.queue_stats(stream, Some(group)).await;
            assert_eq!(stats.stream_length, 0);
            assert_eq!(stats.group_pending, 0);
        }
    }

    #[tokio::test]
    async fn memory_queue_read_retains_returned_fields_after_pending_trim() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let stream = "queue:pending-trim";
        backend
            .queue_ensure_consumer_group(stream, "workers", "0-0")
            .await
            .expect("consumer group");
        let mut expected = Vec::new();
        for index in 0..2 {
            let fields = memory_queue_test_fields(index);
            let id = backend.queue_append(stream, fields.clone(), Some(2)).await;
            expected.push(RuntimeQueueEntry { id, fields });
        }
        let delivered = backend
            .queue_read(stream, "workers", "reader", 2, None)
            .await
            .expect("read batch before trim");
        let next_fields = memory_queue_test_fields(2);
        let next_id = backend
            .queue_append(stream, next_fields.clone(), Some(2))
            .await;
        assert_eq!(
            delivered, expected,
            "trimming must not invalidate returned entries"
        );
        age_memory_queue_pending(&backend, stream).await;
        assert_eq!(
            backend
                .queue_claim_stale(
                    stream,
                    "workers",
                    "reclaimer",
                    "0-0",
                    RuntimeQueueReclaimConfig {
                        min_idle_ms: 500,
                        count: 2
                    },
                )
                .await
                .expect("trimmed entry is removed from the PEL"),
            vec![expected[1].clone()]
        );
        assert_eq!(
            backend
                .queue_read(stream, "workers", "reader", 2, None)
                .await
                .expect("read the remaining new entry"),
            vec![RuntimeQueueEntry {
                id: next_id,
                fields: next_fields
            }]
        );
    }

    #[tokio::test]
    async fn rate_limit_shard_amortizes_expired_entry_cleanup() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig::default());
        let user_key = "rpm:user:cleanup:1";
        let shard_index = memory_rate_limit_counter_shard_index(user_key);
        {
            let mut shard = backend.counters.shards[shard_index]
                .lock()
                .expect("rate-limit shard should lock");
            shard.entries.insert(
                "expired-unrelated-key".to_string(),
                MemoryCounterEntry {
                    value: 1,
                    bucket: 1,
                    expires_at: Instant::now()
                        .checked_sub(Duration::from_secs(1))
                        .expect("test instant should support subtraction"),
                },
            );
            shard.operations_since_prune = MEMORY_RATE_LIMIT_COUNTER_PRUNE_INTERVAL - 1;
        }

        backend
            .check_and_consume_rate_limit(
                user_key,
                "rpm:key:cleanup:1",
                1,
                10,
                10,
                Duration::from_secs(60),
            )
            .await
            .expect("rate-limit check should succeed");

        let shard = backend.counters.shards[shard_index]
            .lock()
            .expect("rate-limit shard should lock");
        assert!(!shard.entries.contains_key("expired-unrelated-key"));
        assert_eq!(shard.operations_since_prune, 0);
    }

    #[tokio::test]
    async fn usage_limit_capacity_is_atomic_and_fail_closed() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig {
            max_usage_limit_windows: 2,
            max_usage_limit_events: 2,
            ..MemoryRuntimeStateConfig::default()
        });
        let first = [crate::UsageLimitRule {
            key: "usage:{user-1}:one",
            limit: 10,
            window_seconds: 60,
            retention_seconds: 60,
        }];
        backend
            .check_and_consume_usage_limits(crate::UsageLimitInput {
                rules: &first,
                event_id: "event-1",
                now_unix_ms: 1_000,
            })
            .await
            .expect("first event");

        let two_new_windows = [
            crate::UsageLimitRule {
                key: "usage:{user-1}:two",
                limit: 10,
                window_seconds: 60,
                retention_seconds: 60,
            },
            crate::UsageLimitRule {
                key: "usage:{user-1}:three",
                limit: 10,
                window_seconds: 60,
                retention_seconds: 60,
            },
        ];
        let error = backend
            .check_and_consume_usage_limits(crate::UsageLimitInput {
                rules: &two_new_windows,
                event_id: "event-2",
                now_unix_ms: 2_000,
            })
            .await
            .expect_err("capacity must fail closed");
        assert!(error.to_string().contains("capacity exhausted"));

        let state = backend.usage_limits.lock().await;
        assert_eq!(state.windows.len(), 1);
        assert_eq!(state.total_events, 1);
        assert!(!state.windows.contains_key(two_new_windows[0].key));
        assert!(!state.windows.contains_key(two_new_windows[1].key));
    }

    #[tokio::test]
    async fn usage_limit_capacity_reclaims_expired_windows_before_rejecting() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig {
            max_usage_limit_windows: 1,
            max_usage_limit_events: 1,
            ..MemoryRuntimeStateConfig::default()
        });
        let old = [crate::UsageLimitRule {
            key: "usage:{user-1}:old",
            limit: 1,
            window_seconds: 1,
            retention_seconds: 1,
        }];
        backend
            .check_and_consume_usage_limits(crate::UsageLimitInput {
                rules: &old,
                event_id: "event-old",
                now_unix_ms: 1_000,
            })
            .await
            .expect("old event");

        let current = [crate::UsageLimitRule {
            key: "usage:{user-1}:current",
            limit: 1,
            window_seconds: 1,
            retention_seconds: 1,
        }];
        assert_eq!(
            backend
                .check_and_consume_usage_limits(crate::UsageLimitInput {
                    rules: &current,
                    event_id: "event-current",
                    now_unix_ms: 2_000,
                })
                .await
                .expect("expired capacity should be reclaimed"),
            UsageLimitCheck::Allowed
        );

        let state = backend.usage_limits.lock().await;
        assert_eq!(state.windows.len(), 1);
        assert_eq!(state.total_events, 1);
        assert!(state.windows.contains_key(current[0].key));
    }

    #[tokio::test]
    async fn usage_limit_idempotent_replay_does_not_consume_event_capacity() {
        let backend = MemoryRuntimeBackend::new(MemoryRuntimeStateConfig {
            max_usage_limit_windows: 1,
            max_usage_limit_events: 1,
            ..MemoryRuntimeStateConfig::default()
        });
        let rules = [crate::UsageLimitRule {
            key: "usage:{user-1}:idempotent",
            limit: 10,
            window_seconds: 60,
            retention_seconds: 60,
        }];
        for now_unix_ms in [1_000, 2_000] {
            assert_eq!(
                backend
                    .check_and_consume_usage_limits(crate::UsageLimitInput {
                        rules: &rules,
                        event_id: "same-event",
                        now_unix_ms,
                    })
                    .await
                    .expect("idempotent replay"),
                UsageLimitCheck::Allowed
            );
        }

        let state = backend.usage_limits.lock().await;
        assert_eq!(state.total_events, 1);
        assert_eq!(
            state.windows[rules[0].key].events["same-event"], 1_000,
            "idempotent replay must preserve the original Redis ZADD NX timestamp"
        );
    }
}
