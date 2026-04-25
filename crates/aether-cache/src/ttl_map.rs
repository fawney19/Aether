use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct TimedEntry<V> {
    value: V,
    inserted_at: Instant,
}

#[derive(Debug)]
pub struct ExpiringMap<K, V> {
    entries: Mutex<HashMap<K, TimedEntry<V>>>,
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
            entries: Mutex::new(HashMap::new()),
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
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };

        prune_expired(&mut entries, ttl);
        while max_entries > 0 && entries.len() >= max_entries {
            let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            entries.remove(&oldest_key);
        }

        entries.insert(
            key,
            TimedEntry {
                value,
                inserted_at: Instant::now(),
            },
        );
    }

    pub fn remove(&self, key: &K) -> Option<V> {
        let Ok(mut entries) = self.entries.lock() else {
            return None;
        };
        entries.remove(key).map(|entry| entry.value)
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&self) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        entries.clear();
    }
}

impl<K, V> ExpiringMap<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub fn get_fresh(&self, key: &K, ttl: Duration) -> Option<V> {
        let Ok(mut entries) = self.entries.lock() else {
            return None;
        };

        let entry = entries.get(key).cloned()?;

        if entry.inserted_at.elapsed() > ttl {
            entries.remove(key);
            return None;
        }

        Some(entry.value)
    }

    pub fn contains_fresh(&self, key: &K, ttl: Duration) -> bool {
        self.get_fresh(key, ttl).is_some()
    }

    pub fn snapshot_fresh(&self, ttl: Duration) -> Vec<ExpiringMapFreshEntry<K, V>> {
        let Ok(mut entries) = self.entries.lock() else {
            return Vec::new();
        };

        prune_expired(&mut entries, ttl);
        entries
            .iter()
            .map(|(key, entry)| ExpiringMapFreshEntry {
                key: key.clone(),
                value: entry.value.clone(),
                age: entry.inserted_at.elapsed(),
            })
            .collect()
    }
}

fn prune_expired<K, V>(entries: &mut HashMap<K, TimedEntry<V>>, ttl: Duration)
where
    K: Eq + Hash,
{
    if ttl.is_zero() {
        entries.clear();
        return;
    }
    entries.retain(|_, entry| entry.inserted_at.elapsed() <= ttl);
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
}
