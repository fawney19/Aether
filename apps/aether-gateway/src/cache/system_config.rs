use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_cache::ExpiringMap;
use tokio::sync::futures::OwnedNotified;
use tokio::sync::Notify;

use crate::GatewayError;

const MAX_ENTRIES: usize = 512;

#[derive(Debug)]
pub(crate) struct SystemConfigCache {
    entries: ExpiringMap<String, Option<serde_json::Value>>,
    inflight: std::sync::Mutex<HashMap<String, Arc<SystemConfigInflightState>>>,
    generation: AtomicU64,
    mutation: std::sync::Mutex<()>,
}

#[derive(Debug)]
struct SystemConfigInflightState {
    completed: AtomicBool,
    error: std::sync::Mutex<Option<GatewayError>>,
    notify: Arc<Notify>,
}

impl SystemConfigInflightState {
    fn new() -> Self {
        Self {
            completed: AtomicBool::new(false),
            error: std::sync::Mutex::new(None),
            notify: Arc::new(Notify::new()),
        }
    }

    fn waiter(self: &Arc<Self>) -> SystemConfigInflightWaiter {
        SystemConfigInflightWaiter {
            state: Arc::clone(self),
            notified: Arc::clone(&self.notify).notified_owned(),
        }
    }

    fn complete(&self) {
        self.completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn fail(&self, error: GatewayError) {
        if let Ok(mut current) = self.error.lock() {
            *current = Some(error);
        }
        self.complete();
    }

    fn error(&self) -> Option<GatewayError> {
        self.error.lock().ok().and_then(|error| error.clone())
    }
}

impl Default for SystemConfigCache {
    fn default() -> Self {
        Self {
            entries: ExpiringMap::new(),
            inflight: std::sync::Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            mutation: std::sync::Mutex::new(()),
        }
    }
}

pub(crate) enum SystemConfigInflightRegistration<'a> {
    Leader(SystemConfigInflightGuard<'a>),
    Follower(SystemConfigInflightWaiter),
    Bypass,
}

pub(crate) struct SystemConfigInflightWaiter {
    state: Arc<SystemConfigInflightState>,
    notified: OwnedNotified,
}

impl SystemConfigInflightWaiter {
    pub(crate) async fn wait(self) -> Result<(), GatewayError> {
        let Self { state, notified } = self;
        if state.completed.load(Ordering::Acquire) {
            return state.error().map_or(Ok(()), Err);
        }

        tokio::pin!(notified);
        if notified.as_mut().enable() || state.completed.load(Ordering::Acquire) {
            return state.error().map_or(Ok(()), Err);
        }
        notified.await;
        state.error().map_or(Ok(()), Err)
    }
}

pub(crate) struct SystemConfigInflightGuard<'a> {
    cache: &'a SystemConfigCache,
    key: Option<String>,
    state: Arc<SystemConfigInflightState>,
    generation: u64,
}

pub(crate) struct SystemConfigOwnedInflightGuard {
    cache: Arc<SystemConfigCache>,
    key: Option<String>,
    state: Arc<SystemConfigInflightState>,
    generation: u64,
}

impl SystemConfigInflightGuard<'_> {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn fail(&self, error: GatewayError) {
        if let Some(key) = self.key.as_deref() {
            self.cache.fail_load(key, &self.state, error);
        }
    }
}

impl SystemConfigOwnedInflightGuard {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn fail(&self, error: GatewayError) {
        if let Some(key) = self.key.as_deref() {
            self.cache.fail_load(key, &self.state, error);
        }
    }
}

impl Drop for SystemConfigInflightGuard<'_> {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        self.cache.finish_load(&key, &self.state);
    }
}

impl Drop for SystemConfigOwnedInflightGuard {
    fn drop(&mut self) {
        let Some(key) = self.key.take() else {
            return;
        };
        self.cache.finish_load(&key, &self.state);
    }
}

impl SystemConfigCache {
    pub(crate) fn get_with_age(
        &self,
        key: &str,
        max_age: Duration,
    ) -> Option<(Option<serde_json::Value>, Duration)> {
        self.entries.get_with_age(&key.to_string(), max_age)
    }

    /// Publishes a value written through the application and invalidates any
    /// older loads before they can overwrite it.
    pub(crate) fn insert(&self, key: String, value: Option<serde_json::Value>, max_age: Duration) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.entries.insert(key, value, max_age, MAX_ENTRIES);
        self.detach_all_loads();
    }

    pub(crate) fn insert_if_generation(
        &self,
        key: String,
        value: Option<serde_json::Value>,
        max_age: Duration,
        generation: u64,
    ) -> bool {
        let Ok(_mutation) = self.mutation.lock() else {
            return false;
        };
        if self.generation.load(Ordering::Acquire) != generation {
            return false;
        }
        self.entries.insert(key, value, max_age, MAX_ENTRIES);
        true
    }

    pub(crate) fn register_load(&self, key: &str) -> SystemConfigInflightRegistration<'_> {
        let key = key.trim();
        if key.is_empty() {
            return SystemConfigInflightRegistration::Bypass;
        }
        let Ok(_mutation) = self.mutation.lock() else {
            return SystemConfigInflightRegistration::Bypass;
        };
        let generation = self.generation.load(Ordering::Acquire);
        match self.inflight.lock() {
            Ok(mut inflight) => {
                if let Some(state) = inflight.get(key) {
                    SystemConfigInflightRegistration::Follower(state.waiter())
                } else {
                    let state = Arc::new(SystemConfigInflightState::new());
                    inflight.insert(key.to_string(), Arc::clone(&state));
                    SystemConfigInflightRegistration::Leader(SystemConfigInflightGuard {
                        cache: self,
                        key: Some(key.to_string()),
                        state,
                        generation,
                    })
                }
            }
            Err(_) => SystemConfigInflightRegistration::Bypass,
        }
    }

    pub(crate) fn try_register_owned_leader(
        self: &Arc<Self>,
        key: &str,
    ) -> Option<SystemConfigOwnedInflightGuard> {
        let key = key.trim();
        if key.is_empty() {
            return None;
        }
        let Ok(_mutation) = self.mutation.lock() else {
            return None;
        };
        let generation = self.generation.load(Ordering::Acquire);
        let Ok(mut inflight) = self.inflight.lock() else {
            return None;
        };
        if inflight.contains_key(key) {
            return None;
        }

        let state = Arc::new(SystemConfigInflightState::new());
        inflight.insert(key.to_string(), Arc::clone(&state));
        Some(SystemConfigOwnedInflightGuard {
            cache: Arc::clone(self),
            key: Some(key.to_string()),
            state,
            generation,
        })
    }

    fn finish_load(&self, key: &str, state: &Arc<SystemConfigInflightState>) {
        let removed = self
            .inflight
            .lock()
            .map(|mut inflight| {
                inflight
                    .get(key)
                    .is_some_and(|current| Arc::ptr_eq(current, state))
                    && inflight.remove(key).is_some()
            })
            .unwrap_or(false);
        if removed {
            state.complete();
        }
    }

    fn fail_load(&self, key: &str, state: &Arc<SystemConfigInflightState>, error: GatewayError) {
        let Ok(_mutation) = self.mutation.lock() else {
            state.fail(error);
            return;
        };
        let removed_current = self
            .inflight
            .lock()
            .map(|mut inflight| {
                if inflight
                    .get(key)
                    .is_some_and(|current| Arc::ptr_eq(current, state))
                {
                    inflight.remove(key);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if removed_current {
            state.fail(error);
        }
    }

    fn detach_all_loads(&self) {
        let states = self
            .inflight
            .lock()
            .map(|mut inflight| inflight.drain().map(|(_, state)| state).collect::<Vec<_>>())
            .unwrap_or_default();
        for state in states {
            state.complete();
        }
    }

    pub(crate) fn clear(&self) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.entries.clear();
        self.detach_all_loads();
    }
}

#[cfg(test)]
mod tests {
    use super::{SystemConfigCache, SystemConfigInflightRegistration};
    use crate::GatewayError;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn singleflight_notifies_only_followers_for_the_matching_key() {
        let cache = SystemConfigCache::default();
        let leader_a = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-a should register a leader"),
        };
        let leader_b = match cache.register_load("key-b") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-b should register a leader"),
        };
        let follower_a = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second key-a registration should follow"),
        };
        let follower_b = match cache.register_load("key-b") {
            SystemConfigInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second key-b registration should follow"),
        };

        drop(leader_b);
        tokio::time::timeout(Duration::from_millis(100), follower_b.wait())
            .await
            .expect("key-b follower should wake when key-b completes")
            .expect("successful load should not publish an error");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), follower_a.wait())
                .await
                .is_err(),
            "key-a follower must not wake for unrelated key-b"
        );

        drop(leader_a);
        assert!(cache.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn leader_completion_before_follower_poll_does_not_lose_wakeup() {
        let cache = SystemConfigCache::default();
        let leader = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        drop(leader);
        tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("completion before first poll must release the follower")
            .expect("successful load should not publish an error");
    }

    #[tokio::test]
    async fn failed_load_is_shared_and_old_guard_preserves_replacement() {
        let cache = SystemConfigCache::default();
        let old_leader = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        old_leader.fail(GatewayError::Internal(
            "forced system config load failure".to_string(),
        ));
        let error = tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("failed load should release its follower")
            .expect_err("follower should observe the leader error");
        assert_eq!(error.into_message(), "forced system config load failure");

        let replacement = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("failed load should allow a replacement"),
        };
        drop(old_leader);
        assert!(matches!(
            cache.register_load("key-a"),
            SystemConfigInflightRegistration::Follower(_)
        ));
        drop(replacement);
    }

    #[tokio::test]
    async fn application_write_wins_over_detached_load_failure() {
        let cache = SystemConfigCache::default();
        let old_leader = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        cache.insert(
            "key-a".to_string(),
            Some(json!("written")),
            Duration::from_secs(60),
        );
        old_leader.fail(GatewayError::Internal(
            "superseded system config load failure".to_string(),
        ));
        follower
            .wait()
            .await
            .expect("detached followers should observe the application write");
        assert_eq!(
            cache
                .get_with_age("key-a", Duration::from_secs(60))
                .map(|(value, _)| value),
            Some(Some(json!("written")))
        );
    }

    #[test]
    fn clear_rejects_old_publication_and_old_guard_preserves_replacement() {
        let cache = SystemConfigCache::default();
        let old_leader = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let old_generation = old_leader.generation();

        cache.clear();
        let replacement = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("clear should allow an immediate replacement"),
        };
        assert!(!cache.insert_if_generation(
            "key-a".to_string(),
            Some(json!("stale")),
            Duration::from_secs(60),
            old_generation,
        ));

        drop(old_leader);
        assert!(matches!(
            cache.register_load("key-a"),
            SystemConfigInflightRegistration::Follower(_)
        ));
        drop(replacement);
        assert!(matches!(
            cache.register_load("key-a"),
            SystemConfigInflightRegistration::Leader(_)
        ));
    }

    #[test]
    fn owned_refresh_allows_only_one_leader_per_key() {
        let cache = Arc::new(SystemConfigCache::default());
        let leader = cache
            .try_register_owned_leader("key-a")
            .expect("first refresh should lead");
        assert!(cache.try_register_owned_leader("key-a").is_none());
        assert!(cache.try_register_owned_leader("key-b").is_some());

        drop(leader);
        assert!(cache.try_register_owned_leader("key-a").is_some());
    }

    #[test]
    fn application_write_rejects_an_older_refresh() {
        let cache = SystemConfigCache::default();
        let old_leader = match cache.register_load("key-a") {
            SystemConfigInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let old_generation = old_leader.generation();

        cache.insert(
            "key-a".to_string(),
            Some(json!("written")),
            Duration::from_secs(60),
        );
        assert!(!cache.insert_if_generation(
            "key-a".to_string(),
            Some(json!("old-refresh")),
            Duration::from_secs(60),
            old_generation,
        ));
        assert_eq!(
            cache
                .get_with_age("key-a", Duration::from_secs(60))
                .map(|(value, _)| value),
            Some(Some(json!("written")))
        );
    }
}
