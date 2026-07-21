use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};

use tokio::sync::Mutex;

const DEFAULT_PRUNE_THRESHOLD: usize = 8_192;

#[derive(Debug)]
pub(crate) struct KeyedAsyncLockPool {
    entries: StdMutex<HashMap<String, Weak<Mutex<()>>>>,
    prune_threshold: usize,
}

impl Default for KeyedAsyncLockPool {
    fn default() -> Self {
        Self {
            entries: StdMutex::new(HashMap::new()),
            prune_threshold: DEFAULT_PRUNE_THRESHOLD,
        }
    }
}

impl KeyedAsyncLockPool {
    pub(crate) fn lock_for(&self, key: &str) -> Arc<Mutex<()>> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = entries.get(key).and_then(Weak::upgrade) {
            return existing;
        }
        if entries.len() >= self.prune_threshold {
            entries.retain(|_, lock| lock.strong_count() > 0);
        }
        let lock = Arc::new(Mutex::new(()));
        entries.insert(key.to_string(), Arc::downgrade(&lock));
        lock
    }

    #[cfg(test)]
    fn tracked_keys(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::KeyedAsyncLockPool;
    use std::sync::Arc;

    #[test]
    fn reuses_active_key_without_serializing_different_keys() {
        let pool = KeyedAsyncLockPool::default();
        let first = pool.lock_for("request-a");
        let same = pool.lock_for("request-a");
        let different = pool.lock_for("request-b");

        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &different));
        assert_eq!(pool.tracked_keys(), 2);
    }
}
