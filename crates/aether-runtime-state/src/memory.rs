use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::{RuntimeQueueEntry, RuntimeQueueReclaimConfig};

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
    atomic_counters: Mutex<HashMap<String, u64>>,
    counters: Mutex<HashMap<String, MemoryCounterEntry>>,
    sets: Mutex<HashMap<String, BTreeSet<String>>>,
    scores: Mutex<HashMap<String, BTreeMap<String, f64>>>,
    queues: Mutex<HashMap<String, VecDeque<RuntimeQueueEntry>>>,
    queue_seq: AtomicU64,
    locks: Mutex<HashMap<String, MemoryLockEntry>>,
    semaphores: Mutex<HashMap<String, BTreeMap<String, u64>>>,
}

#[derive(Debug, Clone)]
struct MemoryCounterEntry {
    value: u32,
    bucket: u64,
    expires_at: Instant,
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
        self.kv.lock().await.remove(key).is_some()
    }

    pub(crate) async fn kv_delete_many(&self, keys: &[String]) -> usize {
        let mut kv = self.kv.lock().await;
        keys.iter().filter(|key| kv.remove(*key).is_some()).count()
    }

    pub(crate) async fn kv_exists(&self, key: &str) -> bool {
        self.kv_get(key).await.is_some()
    }

    pub(crate) async fn counter_increment(&self, key: &str) -> u64 {
        let mut counters = self.atomic_counters.lock().await;
        let next = counters
            .get(key)
            .copied()
            .unwrap_or_default()
            .saturating_add(1);
        counters.insert(key.to_string(), next);
        next
    }

    pub(crate) async fn counter_get(&self, key: &str) -> u64 {
        self.atomic_counters
            .lock()
            .await
            .get(key)
            .copied()
            .unwrap_or_default()
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

    pub(crate) async fn kv_scan(&self, pattern: &str) -> Vec<String> {
        let mut kv = self.kv.lock().await;
        prune_kv(&mut kv, Instant::now());
        let mut keys = kv
            .keys()
            .filter(|key| key_matches_pattern(key, pattern))
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        keys
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
        let mut counters = self.counters.lock().await;
        let now = Instant::now();
        counters.retain(|_, entry| entry.expires_at > now && entry.bucket >= bucket);

        if user_limit > 0 {
            let user_count = counters
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
            let key_count = counters
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
            let next = counters
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
            let next = counters
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

    pub(crate) async fn set_add(&self, key: &str, member: &str) -> bool {
        self.sets
            .lock()
            .await
            .entry(key.to_string())
            .or_default()
            .insert(member.to_string())
    }

    pub(crate) fn set_add_nowait(&self, key: &str, member: &str) -> bool {
        let Ok(mut sets) = self.sets.try_lock() else {
            return false;
        };
        sets.entry(key.to_string())
            .or_default()
            .insert(member.to_string())
    }

    pub(crate) async fn set_remove(&self, key: &str, member: &str) -> bool {
        self.sets
            .lock()
            .await
            .get_mut(key)
            .is_some_and(|set| set.remove(member))
    }

    pub(crate) async fn set_members(&self, key: &str) -> Vec<String> {
        self.sets
            .lock()
            .await
            .get(key)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) async fn set_len(&self, key: &str) -> usize {
        self.sets.lock().await.get(key).map_or(0, BTreeSet::len)
    }

    pub(crate) async fn score_set(&self, key: &str, member: &str, score: f64) {
        self.scores
            .lock()
            .await
            .entry(key.to_string())
            .or_default()
            .insert(member.to_string(), score);
    }

    pub(crate) async fn score_many(&self, key: &str, members: &[String]) -> Vec<Option<f64>> {
        let scores = self.scores.lock().await;
        members
            .iter()
            .map(|member| scores.get(key).and_then(|set| set.get(member)).copied())
            .collect()
    }

    pub(crate) async fn score_range_by_min(&self, key: &str, min_score: f64) -> Vec<String> {
        let scores = self.scores.lock().await;
        scores
            .get(key)
            .map(|set| {
                set.iter()
                    .filter(|(_, score)| **score >= min_score)
                    .map(|(member, _)| member.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) async fn score_remove_by_score(&self, key: &str, max_score: f64) -> usize {
        let mut scores = self.scores.lock().await;
        let Some(set) = scores.get_mut(key) else {
            return 0;
        };
        let before = set.len();
        set.retain(|_, score| *score > max_score);
        before.saturating_sub(set.len())
    }

    pub(crate) async fn score_len(&self, key: &str) -> usize {
        self.scores.lock().await.get(key).map_or(0, BTreeMap::len)
    }

    pub(crate) async fn queue_append(
        &self,
        stream: &str,
        fields: BTreeMap<String, String>,
        maxlen: Option<usize>,
    ) -> String {
        let id = format!(
            "{}-0",
            self.queue_seq
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1)
        );
        let mut queues = self.queues.lock().await;
        let queue = queues.entry(stream.to_string()).or_default();
        queue.push_back(RuntimeQueueEntry {
            id: id.clone(),
            fields,
        });
        if let Some(maxlen) = maxlen.filter(|value| *value > 0) {
            while queue.len() > maxlen {
                queue.pop_front();
            }
        }
        id
    }

    pub(crate) async fn queue_read(&self, stream: &str, count: usize) -> Vec<RuntimeQueueEntry> {
        let mut queues = self.queues.lock().await;
        let Some(queue) = queues.get_mut(stream) else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        for _ in 0..count.max(1) {
            let Some(entry) = queue.pop_front() else {
                break;
            };
            entries.push(entry);
        }
        entries
    }

    pub(crate) async fn queue_claim_stale(
        &self,
        _stream: &str,
        _config: RuntimeQueueReclaimConfig,
    ) -> Vec<RuntimeQueueEntry> {
        Vec::new()
    }

    pub(crate) async fn queue_delete(&self, _stream: &str, _ids: &[String]) -> usize {
        0
    }

    pub(crate) async fn lock_try_acquire(
        &self,
        key: &str,
        owner: &str,
        token: String,
        ttl: Duration,
    ) -> bool {
        let mut locks = self.locks.lock().await;
        let now = Instant::now();
        locks.retain(|_, entry| entry.expires_at > now);
        if locks.contains_key(key) {
            return false;
        }
        locks.insert(
            key.to_string(),
            MemoryLockEntry {
                token,
                owner: owner.to_string(),
                expires_at: now + ttl,
            },
        );
        true
    }

    pub(crate) async fn lock_release(&self, key: &str, token: &str) -> bool {
        let mut locks = self.locks.lock().await;
        if locks.get(key).is_some_and(|entry| entry.token == token) {
            locks.remove(key);
            return true;
        }
        false
    }

    pub(crate) async fn lock_renew(&self, key: &str, token: &str, ttl: Duration) -> bool {
        let mut locks = self.locks.lock().await;
        if let Some(entry) = locks.get_mut(key) {
            if entry.token == token {
                entry.expires_at = Instant::now() + ttl;
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

pub(crate) fn key_matches_pattern(key: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => key == pattern,
    }
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
