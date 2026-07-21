use aether_cache::CacheKeyNamespace;

use crate::redis::{RedisLockKey, RedisStreamName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisKeyspace {
    namespace: CacheKeyNamespace,
}

impl RedisKeyspace {
    pub fn new(prefix: Option<&str>) -> Self {
        let normalized = prefix.unwrap_or_default().trim().trim_matches(':');
        Self {
            namespace: CacheKeyNamespace::new(normalized),
        }
    }

    pub fn key(&self, raw_key: &str) -> String {
        self.namespace.key(raw_key)
    }

    pub fn lock_key(&self, raw_key: &str) -> RedisLockKey {
        RedisLockKey(self.namespace.child("lock").key(raw_key))
    }

    pub fn stream_name(&self, raw_name: &str) -> RedisStreamName {
        RedisStreamName(self.namespace.child("stream").key(raw_name))
    }
}

#[cfg(test)]
mod tests {
    use super::RedisKeyspace;

    #[test]
    fn composes_prefixed_lock_and_stream_names() {
        let keyspace = RedisKeyspace::new(Some("aether"));

        assert_eq!(keyspace.key("auth:user"), "aether:auth:user");
        assert_eq!(keyspace.lock_key("poller").0, "aether:lock:poller");
        assert_eq!(keyspace.stream_name("audit").0, "aether:stream:audit");
    }
}
