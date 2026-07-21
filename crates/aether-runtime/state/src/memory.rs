use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::{DataLayerError, RuntimeQueueEntry, RuntimeQueueReclaimConfig, RuntimeQueueStats};

const MEMORY_RATE_LIMIT_COUNTER_SHARD_COUNT: usize = 64;
const MEMORY_RATE_LIMIT_COUNTER_PRUNE_INTERVAL: u64 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRuntimeStateConfig {
    pub max_kv_entries: usize,
}

impl Default for MemoryRuntimeStateConfig {
    fn default() -> Self {
        Self {
            max_kv_entries: 10_000,
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
    sets: Mutex<HashMap<String, MemorySetEntry>>,
    scores: Mutex<HashMap<String, MemoryScoreEntry>>,
    queues: Mutex<HashMap<String, MemoryQueueStream>>,
    queue_seq: AtomicU64,
    locks: Mutex<HashMap<String, MemoryLockEntry>>,
    lock_fencing_seq: AtomicU64,
    semaphores: Mutex<HashMap<String, BTreeMap<String, u64>>>,
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
        let sequence = self
            .queue_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let id = format!("{sequence}-0");
        let mut queues = self.queues.lock().await;
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
                let queued_entries = stream_state
                    .entries
                    .iter()
                    .filter(|entry| entry.sequence > last_delivered_sequence)
                    .take(count.max(1))
                    .cloned()
                    .collect::<Vec<_>>();
                for queued in queued_entries {
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

    pub(crate) async fn queue_claim_stale(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        start_id: &str,
        config: RuntimeQueueReclaimConfig,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
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

        let mut claimed = Vec::new();
        for (_, id) in ids.into_iter().take(config.count.max(1)) {
            if let Some(pending) = group_state.pending.get_mut(&id) {
                pending.consumer = consumer.to_string();
                pending.delivered_at = now;
                claimed.push(pending.entry.clone());
            }
        }
        Ok(claimed)
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
}
