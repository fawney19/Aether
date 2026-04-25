use std::time::Duration;

use aether_cache::ExpiringMap;

const MAX_ENTRIES: usize = 512;

#[derive(Debug)]
pub(crate) struct SystemConfigCache {
    entries: ExpiringMap<String, Option<serde_json::Value>>,
}

impl Default for SystemConfigCache {
    fn default() -> Self {
        Self {
            entries: ExpiringMap::new(),
        }
    }
}

impl SystemConfigCache {
    pub(crate) fn get(&self, key: &str, ttl: Duration) -> Option<Option<serde_json::Value>> {
        self.entries.get_fresh(&key.to_string(), ttl)
    }

    pub(crate) fn insert(&self, key: String, value: Option<serde_json::Value>, ttl: Duration) {
        self.entries.insert(key, value, ttl, MAX_ENTRIES);
    }

    pub(crate) fn clear(&self) {
        self.entries.clear();
    }
}
