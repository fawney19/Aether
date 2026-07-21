use std::borrow::Borrow;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::RwLock;
use std::time::{Duration, Instant};

const INSERTION_ORDER_COMPACTION_SLACK: usize = 64;

#[derive(Debug, Clone)]
struct TimedEntry<V> {
    value: V,
    inserted_at: Instant,
    generation: u64,
}

#[derive(Debug)]
struct TimedKey<K> {
    key: K,
    inserted_at: Instant,
    generation: u64,
}

#[derive(Debug)]
struct ExpiringMapState<K, V> {
    entries: HashMap<K, TimedEntry<V>>,
    insertion_order: VecDeque<TimedKey<K>>,
    next_generation: u64,
}

impl<K, V> Default for ExpiringMapState<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            insertion_order: VecDeque::new(),
            next_generation: 0,
        }
    }
}

#[derive(Debug)]
pub struct ExpiringMap<K, V> {
    state: RwLock<ExpiringMapState<K, V>>,
}

#[derive(Debug, Clone)]
pub struct ExpiringMapFreshEntry<K, V> {
    pub key: K,
    pub value: V,
    pub age: Duration,
}

impl<K, V> Default for ExpiringMap<K, V> {
    fn default() -> Self {
        Self {
            state: RwLock::new(ExpiringMapState::default()),
        }
    }
}

impl<K, V> ExpiringMap<K, V>
where
    K: Eq + Hash + Clone,
{
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, key: K, value: V, ttl: Duration, max_entries: usize) {
        let Ok(mut state) = self.state.write() else {
            return;
        };

        let inserted_at = Instant::now();
        prune_expired(&mut state, ttl, inserted_at);
        evict_to_capacity(&mut state, max_entries);
        let generation = take_next_generation(&mut state);

        state.entries.insert(
            key.clone(),
            TimedEntry {
                value,
                inserted_at,
                generation,
            },
        );
        state.insertion_order.push_back(TimedKey {
            key,
            inserted_at,
            generation,
        });
        compact_insertion_order_if_needed(&mut state);
    }

    pub fn insert_if_absent_fresh(
        &self,
        key: K,
        value: V,
        ttl: Duration,
        max_entries: usize,
    ) -> bool {
        if ttl.is_zero() {
            return true;
        }

        let Ok(mut state) = self.state.write() else {
            return false;
        };

        let inserted_at = Instant::now();
        prune_expired(&mut state, ttl, inserted_at);
        if state.entries.contains_key(&key) {
            return false;
        }
        evict_to_capacity(&mut state, max_entries);
        let generation = take_next_generation(&mut state);

        state.entries.insert(
            key.clone(),
            TimedEntry {
                value,
                inserted_at,
                generation,
            },
        );
        state.insertion_order.push_back(TimedKey {
            key,
            inserted_at,
            generation,
        });
        compact_insertion_order_if_needed(&mut state);
        true
    }

    pub fn remove<Q>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let Ok(mut state) = self.state.write() else {
            return None;
        };
        let removed = state.entries.remove(key).map(|entry| entry.value);
        compact_insertion_order_if_needed(&mut state);
        removed
    }

    pub fn len(&self) -> usize {
        self.state
            .read()
            .map(|state| state.entries.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        let Ok(mut state) = self.state.write() else {
            return;
        };
        state.entries.clear();
        state.insertion_order.clear();
    }
}

impl<K, V> ExpiringMap<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn get_with_age<Q>(&self, key: &Q, max_age: Duration) -> Option<(V, Duration)>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        {
            let Ok(state) = self.state.read() else {
                return None;
            };
            let entry = state.entries.get(key)?;
            let age = Instant::now().saturating_duration_since(entry.inserted_at);
            if age <= max_age {
                return Some((entry.value.clone(), age));
            }
        }

        // Expiry is uncommon on the hot read path. Re-check after upgrading
        // to the write lock so a concurrent refresh is never removed.
        let Ok(mut state) = self.state.write() else {
            return None;
        };
        let entry = state.entries.get(key).cloned()?;
        let age = Instant::now().saturating_duration_since(entry.inserted_at);
        if age <= max_age {
            return Some((entry.value, age));
        }
        state.entries.remove(key);
        compact_insertion_order_if_needed(&mut state);
        None
    }

    pub fn get_fresh<Q>(&self, key: &Q, ttl: Duration) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.get_with_age(key, ttl).map(|(value, _age)| value)
    }

    pub fn contains_fresh<Q>(&self, key: &Q, ttl: Duration) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.get_fresh(key, ttl).is_some()
    }

    pub fn snapshot_fresh(&self, ttl: Duration) -> Vec<ExpiringMapFreshEntry<K, V>> {
        let Ok(mut state) = self.state.write() else {
            return Vec::new();
        };

        let now = Instant::now();
        prune_expired(&mut state, ttl, now);
        state
            .entries
            .iter()
            .map(|(key, entry)| ExpiringMapFreshEntry {
                key: key.clone(),
                value: entry.value.clone(),
                age: now.saturating_duration_since(entry.inserted_at),
            })
            .collect()
    }
}

fn prune_expired<K, V>(state: &mut ExpiringMapState<K, V>, ttl: Duration, now: Instant)
where
    K: Eq + Hash,
{
    if ttl.is_zero() {
        state.entries.clear();
        state.insertion_order.clear();
        return;
    }

    while state
        .insertion_order
        .front()
        .is_some_and(|entry| now.saturating_duration_since(entry.inserted_at) > ttl)
    {
        let Some(expired) = state.insertion_order.pop_front() else {
            break;
        };
        if state
            .entries
            .get(&expired.key)
            .is_some_and(|entry| entry.generation == expired.generation)
        {
            state.entries.remove(&expired.key);
        }
    }
}

fn evict_to_capacity<K, V>(state: &mut ExpiringMapState<K, V>, max_entries: usize)
where
    K: Eq + Hash + Clone,
{
    while max_entries > 0 && state.entries.len() >= max_entries {
        if remove_oldest_current_entry(state) {
            continue;
        }

        rebuild_insertion_order(state);
        if !remove_oldest_current_entry(state) {
            break;
        }
    }
}

fn remove_oldest_current_entry<K, V>(state: &mut ExpiringMapState<K, V>) -> bool
where
    K: Eq + Hash,
{
    while let Some(oldest) = state.insertion_order.pop_front() {
        if state
            .entries
            .get(&oldest.key)
            .is_some_and(|entry| entry.generation == oldest.generation)
        {
            state.entries.remove(&oldest.key);
            return true;
        }
    }
    false
}

fn compact_insertion_order_if_needed<K, V>(state: &mut ExpiringMapState<K, V>)
where
    K: Eq + Hash + Clone,
{
    if state.entries.is_empty() {
        state.insertion_order.clear();
        return;
    }

    let compact_at = state
        .entries
        .len()
        .saturating_mul(2)
        .saturating_add(INSERTION_ORDER_COMPACTION_SLACK);
    if state.insertion_order.len() > compact_at {
        rebuild_insertion_order(state);
    }
}

fn rebuild_insertion_order<K, V>(state: &mut ExpiringMapState<K, V>)
where
    K: Eq + Hash + Clone,
{
    let mut entries = state
        .entries
        .iter()
        .map(|(key, entry)| TimedKey {
            key: key.clone(),
            inserted_at: entry.inserted_at,
            generation: entry.generation,
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by_key(|entry| entry.inserted_at);
    state.insertion_order = entries.into();
}

fn take_next_generation<K, V>(state: &mut ExpiringMapState<K, V>) -> u64 {
    let generation = state.next_generation;
    state.next_generation = state
        .next_generation
        .checked_add(1)
        .expect("expiring map entry generation overflow");
    generation
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;

    use super::ExpiringMap;

    #[test]
    fn evicts_expired_entries_on_read() {
        let cache = ExpiringMap::new();
        cache.insert(
            "hello".to_string(),
            42_u32,
            std::time::Duration::from_millis(10),
            16,
        );

        sleep(std::time::Duration::from_millis(20));

        assert_eq!(
            cache.get_fresh(&"hello".to_string(), std::time::Duration::from_millis(10)),
            None
        );
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn evicts_oldest_entry_when_capacity_is_hit() {
        let cache = ExpiringMap::new();

        cache.insert(
            "one".to_string(),
            1_u32,
            std::time::Duration::from_secs(60),
            2,
        );
        sleep(std::time::Duration::from_millis(2));
        cache.insert(
            "two".to_string(),
            2_u32,
            std::time::Duration::from_secs(60),
            2,
        );
        sleep(std::time::Duration::from_millis(2));
        cache.insert(
            "three".to_string(),
            3_u32,
            std::time::Duration::from_secs(60),
            2,
        );

        assert_eq!(
            cache.get_fresh(&"one".to_string(), std::time::Duration::from_secs(60)),
            None
        );
        assert_eq!(
            cache.get_fresh(&"two".to_string(), std::time::Duration::from_secs(60)),
            Some(2)
        );
        assert_eq!(
            cache.get_fresh(&"three".to_string(), std::time::Duration::from_secs(60)),
            Some(3)
        );
    }

    #[test]
    fn insert_if_absent_fresh_rejects_fresh_duplicate() {
        let cache = ExpiringMap::new();

        assert!(cache.insert_if_absent_fresh(
            "hello".to_string(),
            1_u32,
            std::time::Duration::from_secs(60),
            16,
        ));
        assert!(!cache.insert_if_absent_fresh(
            "hello".to_string(),
            2_u32,
            std::time::Duration::from_secs(60),
            16,
        ));
        assert_eq!(
            cache.get_fresh(&"hello".to_string(), std::time::Duration::from_secs(60)),
            Some(1)
        );
    }

    #[test]
    fn insert_if_absent_fresh_allows_after_expiry() {
        let cache = ExpiringMap::new();

        assert!(cache.insert_if_absent_fresh(
            "hello".to_string(),
            1_u32,
            std::time::Duration::from_millis(10),
            16,
        ));
        sleep(std::time::Duration::from_millis(20));
        assert!(cache.insert_if_absent_fresh(
            "hello".to_string(),
            2_u32,
            std::time::Duration::from_millis(10),
            16,
        ));
        assert_eq!(
            cache.get_fresh(&"hello".to_string(), std::time::Duration::from_millis(10)),
            Some(2)
        );
    }

    #[test]
    fn string_keys_support_borrowed_str_lookup_and_remove() {
        let cache = ExpiringMap::new();
        cache.insert(
            "hello".to_string(),
            42_u32,
            std::time::Duration::from_secs(60),
            16,
        );

        assert_eq!(
            cache.get_fresh("hello", std::time::Duration::from_secs(60)),
            Some(42)
        );
        assert_eq!(cache.remove("hello"), Some(42));
        assert_eq!(
            cache.get_fresh("hello", std::time::Duration::from_secs(60)),
            None
        );
    }

    #[test]
    fn high_cardinality_inserts_keep_the_hard_capacity_bound() {
        let cache = ExpiringMap::new();
        let ttl = std::time::Duration::from_secs(60);

        for key in 0..20_000_u32 {
            cache.insert(key, key, ttl, 1_024);
        }

        assert_eq!(cache.len(), 1_024);
        assert_eq!(cache.get_fresh(&0, ttl), None);
        assert_eq!(cache.get_fresh(&19_999, ttl), Some(19_999));
    }

    #[test]
    fn repeated_overwrites_compact_stale_insertion_order_records() {
        let cache = ExpiringMap::new();
        let ttl = std::time::Duration::from_secs(60);

        for value in 0..1_000_u32 {
            cache.insert("same".to_string(), value, ttl, 16);
        }

        let state = cache.state.read().expect("cache state should lock");
        assert_eq!(state.entries.len(), 1);
        assert!(
            state.insertion_order.len()
                <= state
                    .entries
                    .len()
                    .saturating_mul(2)
                    .saturating_add(super::INSERTION_ORDER_COMPACTION_SLACK)
        );
        assert_eq!(
            state.entries.get("same").map(|entry| entry.value),
            Some(999)
        );
    }

    #[test]
    fn capacity_eviction_distinguishes_overwrites_with_equal_instants() {
        let cache = ExpiringMap::new();
        let ttl = std::time::Duration::from_secs(60);
        cache.insert("same".to_string(), 1_u32, ttl, 2);
        cache.insert("between".to_string(), 2_u32, ttl, 2);
        cache.insert("same".to_string(), 3_u32, ttl, 0);

        {
            let mut state = cache.state.write().expect("cache state should lock");
            let shared_instant = std::time::Instant::now();
            for entry in state.entries.values_mut() {
                entry.inserted_at = shared_instant;
            }
            for entry in &mut state.insertion_order {
                entry.inserted_at = shared_instant;
            }
        }

        cache.insert("new".to_string(), 4_u32, ttl, 2);

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get_fresh(&"same".to_string(), ttl), Some(3));
        assert_eq!(cache.get_fresh(&"between".to_string(), ttl), None);
        assert_eq!(cache.get_fresh(&"new".to_string(), ttl), Some(4));
    }
}
