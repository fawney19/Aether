use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_cache::ExpiringMap;
use tokio::sync::futures::OwnedNotified;
use tokio::sync::Notify;

use crate::{control::GatewayControlAuthContext, GatewayError};

#[derive(Debug)]
pub(crate) struct AuthContextCache {
    entries: ExpiringMap<String, GatewayControlAuthContext>,
    inflight: std::sync::Mutex<HashMap<String, Arc<AuthContextInflightState>>>,
    // Guards cache publication across invalidation. A request that started
    // before a mutation must not repopulate the cache after it is cleared.
    generation: AtomicU64,
    mutation: std::sync::Mutex<()>,
    #[cfg(test)]
    refresh_interval_override_millis: AtomicU64,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthContextCacheGeneration {
    global: u64,
    state: Arc<AuthContextInflightState>,
}

#[derive(Debug)]
struct AuthContextInflightState {
    completed: AtomicBool,
    publishable: AtomicBool,
    error: std::sync::Mutex<Option<GatewayError>>,
    notify: Arc<Notify>,
}

impl AuthContextInflightState {
    fn new() -> Self {
        Self {
            completed: AtomicBool::new(false),
            publishable: AtomicBool::new(true),
            error: std::sync::Mutex::new(None),
            notify: Arc::new(Notify::new()),
        }
    }

    fn waiter(self: &Arc<Self>) -> AuthContextInflightWaiter {
        AuthContextInflightWaiter {
            state: Arc::clone(self),
            notified: Arc::clone(&self.notify).notified_owned(),
        }
    }

    fn complete(&self) {
        self.completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn invalidate(&self) {
        self.publishable.store(false, Ordering::Release);
        self.complete();
    }

    fn fail(&self, error: GatewayError) {
        self.publishable.store(false, Ordering::Release);
        if let Ok(mut current) = self.error.lock() {
            *current = Some(error);
        }
        self.complete();
    }

    fn error(&self) -> Option<GatewayError> {
        self.error.lock().ok().and_then(|error| error.clone())
    }
}

pub(crate) struct AuthContextInflightWaiter {
    state: Arc<AuthContextInflightState>,
    notified: OwnedNotified,
}

impl AuthContextInflightWaiter {
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

impl Default for AuthContextCache {
    fn default() -> Self {
        Self {
            entries: ExpiringMap::default(),
            inflight: std::sync::Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            mutation: std::sync::Mutex::new(()),
            #[cfg(test)]
            refresh_interval_override_millis: AtomicU64::new(0),
        }
    }
}

pub(crate) enum AuthContextInflightRegistration<'a> {
    Leader(AuthContextInflightGuard<'a>),
    Follower(AuthContextInflightWaiter),
    Bypass,
}

pub(crate) struct AuthContextInflightGuard<'a> {
    cache: &'a AuthContextCache,
    cache_key: Option<String>,
    state: Arc<AuthContextInflightState>,
    generation: AuthContextCacheGeneration,
}

pub(crate) struct AuthContextOwnedInflightGuard {
    cache: Arc<AuthContextCache>,
    cache_key: Option<String>,
    state: Arc<AuthContextInflightState>,
    generation: AuthContextCacheGeneration,
}

impl AuthContextInflightGuard<'_> {
    pub(crate) fn generation(&self) -> AuthContextCacheGeneration {
        self.generation.clone()
    }

    pub(crate) fn generation_is_current(&self) -> bool {
        self.cache.generation_is_current(&self.generation)
    }

    pub(crate) fn fail(&self, error: GatewayError) {
        if let Some(cache_key) = self.cache_key.as_deref() {
            self.cache.fail_inflight(cache_key, &self.state, error);
        }
    }
}

impl AuthContextOwnedInflightGuard {
    pub(crate) fn generation(&self) -> AuthContextCacheGeneration {
        self.generation.clone()
    }

    pub(crate) fn generation_is_current(&self) -> bool {
        self.cache.generation_is_current(&self.generation)
    }
}

impl Drop for AuthContextInflightGuard<'_> {
    fn drop(&mut self) {
        let Some(cache_key) = self.cache_key.take() else {
            return;
        };
        self.cache.finish_inflight(&cache_key, &self.state);
    }
}

impl Drop for AuthContextOwnedInflightGuard {
    fn drop(&mut self) {
        let Some(cache_key) = self.cache_key.take() else {
            return;
        };
        self.cache.finish_inflight(&cache_key, &self.state);
    }
}

impl AuthContextCache {
    fn finish_inflight(&self, cache_key: &str, state: &Arc<AuthContextInflightState>) {
        if let Ok(mut inflight) = self.inflight.lock() {
            // A clear may already have detached and completed this flight.
            // Keep drop idempotent so an old guard cannot affect its replacement.
            if inflight
                .get(cache_key)
                .is_some_and(|current| Arc::ptr_eq(current, state))
            {
                state.complete();
                inflight.remove(cache_key);
            }
        }
    }

    fn fail_inflight(
        &self,
        cache_key: &str,
        state: &Arc<AuthContextInflightState>,
        error: GatewayError,
    ) {
        let Ok(_mutation) = self.mutation.lock() else {
            state.fail(error);
            return;
        };
        let removed_current = match self.inflight.lock() {
            Ok(mut inflight) => {
                if inflight
                    .get(cache_key)
                    .is_some_and(|current| Arc::ptr_eq(current, state))
                {
                    inflight.remove(cache_key);
                    true
                } else {
                    false
                }
            }
            Err(_) => {
                state.fail(error);
                return;
            }
        };
        if removed_current {
            self.remove_entries_for_key(cache_key);
            state.fail(error);
        }
    }

    fn remove_entries_for_key(&self, cache_key: &str) {
        self.entries.remove(&cache_key.to_string());
        self.entries.remove(&format!("negative:{cache_key}"));
    }

    pub(crate) fn get_fresh(
        &self,
        cache_key: &str,
        ttl: Duration,
    ) -> Option<GatewayControlAuthContext> {
        self.entries.get_fresh(&cache_key.to_string(), ttl)
    }

    pub(crate) fn get_fresh_with_age(
        &self,
        cache_key: &str,
        ttl: Duration,
    ) -> Option<(GatewayControlAuthContext, Duration)> {
        self.entries.get_with_age(&cache_key.to_string(), ttl)
    }

    pub(crate) fn insert(
        &self,
        cache_key: String,
        auth_context: GatewayControlAuthContext,
        ttl: Duration,
        max_entries: usize,
    ) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.entries
            .insert(cache_key, auth_context, ttl, max_entries);
    }

    pub(crate) fn insert_if_generation(
        &self,
        cache_key: String,
        auth_context: GatewayControlAuthContext,
        ttl: Duration,
        max_entries: usize,
        generation: &AuthContextCacheGeneration,
    ) -> bool {
        let Ok(_mutation) = self.mutation.lock() else {
            return false;
        };
        if !self.generation_is_current(generation) {
            return false;
        }
        self.entries
            .insert(cache_key, auth_context, ttl, max_entries);
        true
    }

    fn generation_for_state(
        &self,
        state: &Arc<AuthContextInflightState>,
    ) -> AuthContextCacheGeneration {
        AuthContextCacheGeneration {
            global: self.generation.load(Ordering::Acquire),
            state: Arc::clone(state),
        }
    }

    fn generation_is_current(&self, generation: &AuthContextCacheGeneration) -> bool {
        self.generation.load(Ordering::Acquire) == generation.global
            && generation.state.publishable.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn set_refresh_interval_for_tests(&self, interval: Duration) {
        let millis = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
        self.refresh_interval_override_millis
            .store(millis.max(1), Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn refresh_interval_for_tests(&self) -> Option<Duration> {
        let millis = self
            .refresh_interval_override_millis
            .load(Ordering::Acquire);
        (millis > 0).then(|| Duration::from_millis(millis))
    }

    pub(crate) fn register_inflight(&self, cache_key: &str) -> AuthContextInflightRegistration<'_> {
        let cache_key = cache_key.trim();
        if cache_key.is_empty() {
            return AuthContextInflightRegistration::Bypass;
        }
        let Ok(_mutation) = self.mutation.lock() else {
            return AuthContextInflightRegistration::Bypass;
        };
        match self.inflight.lock() {
            Ok(mut inflight) => {
                if let Some(state) = inflight.get(cache_key) {
                    // Create the waiter while holding the map lock so leader
                    // completion cannot race between lookup and registration.
                    AuthContextInflightRegistration::Follower(state.waiter())
                } else {
                    let state = Arc::new(AuthContextInflightState::new());
                    let generation = self.generation_for_state(&state);
                    inflight.insert(cache_key.to_string(), Arc::clone(&state));
                    AuthContextInflightRegistration::Leader(AuthContextInflightGuard {
                        cache: self,
                        cache_key: Some(cache_key.to_string()),
                        state,
                        generation,
                    })
                }
            }
            Err(_) => AuthContextInflightRegistration::Bypass,
        }
    }

    pub(crate) fn try_register_owned_leader(
        self: &Arc<Self>,
        cache_key: &str,
    ) -> Option<AuthContextOwnedInflightGuard> {
        let cache_key = cache_key.trim();
        if cache_key.is_empty() {
            return None;
        }
        let Ok(_mutation) = self.mutation.lock() else {
            return None;
        };
        let Ok(mut inflight) = self.inflight.lock() else {
            return None;
        };
        if inflight.contains_key(cache_key) {
            return None;
        }

        let state = Arc::new(AuthContextInflightState::new());
        let generation = self.generation_for_state(&state);
        inflight.insert(cache_key.to_string(), Arc::clone(&state));
        Some(AuthContextOwnedInflightGuard {
            cache: Arc::clone(self),
            cache_key: Some(cache_key.to_string()),
            state,
            generation,
        })
    }

    pub(crate) fn clear(&self) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.entries.clear();
        let states = self
            .inflight
            .lock()
            .map(|mut inflight| inflight.drain().map(|(_, state)| state).collect::<Vec<_>>())
            .unwrap_or_default();
        for state in states {
            state.invalidate();
        }
    }

    pub(crate) fn invalidate(&self, cache_key: &str) {
        let cache_key = cache_key.trim();
        if cache_key.is_empty() {
            return;
        }
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.remove_entries_for_key(cache_key);
        let state = self
            .inflight
            .lock()
            .ok()
            .and_then(|mut inflight| inflight.remove(cache_key));
        if let Some(state) = state {
            state.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthContextCache, AuthContextInflightRegistration};
    use crate::{control::GatewayControlAuthContext, GatewayError};
    use std::sync::Arc;
    use std::time::Duration;

    fn context(api_key_id: &str) -> GatewayControlAuthContext {
        GatewayControlAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: api_key_id.to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            sell_rate_multiplier: 1.0,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            billing_group_id: None,
            billing_group_name: None,
        }
    }

    #[tokio::test]
    async fn auth_context_singleflight_notifies_only_matching_key() {
        let cache = AuthContextCache::default();
        let leader_a = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-a should register a leader"),
        };
        let leader_b = match cache.register_inflight("key-b") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-b should register a leader"),
        };
        let follower_a = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Follower(notified) => notified,
            _ => panic!("second key-a registration should follow"),
        };
        let follower_b = match cache.register_inflight("key-b") {
            AuthContextInflightRegistration::Follower(notified) => notified,
            _ => panic!("second key-b registration should follow"),
        };

        drop(leader_b);
        tokio::time::timeout(Duration::from_millis(100), follower_b.wait())
            .await
            .expect("key-b follower should wake when key-b completes")
            .expect("successful flight should not publish an error");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), follower_a.wait())
                .await
                .is_err(),
            "key-a follower must not wake when unrelated key-b completes"
        );

        drop(leader_a);
        assert!(cache.inflight.lock().unwrap().is_empty());
    }

    #[test]
    fn clear_rejects_publication_from_old_inflight_generation() {
        let cache = AuthContextCache::default();
        let leader = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-a should register a leader"),
        };
        let old_generation = leader.generation();

        cache.clear();
        assert!(!leader.generation_is_current());
        assert!(!cache.insert_if_generation(
            "key-a".to_string(),
            context("stale-key"),
            Duration::from_secs(60),
            10,
            &old_generation,
        ));
        assert!(cache.get_fresh("key-a", Duration::from_secs(60)).is_none());
    }

    #[tokio::test]
    async fn invalidate_removes_only_one_key_and_releases_its_followers() {
        let cache = AuthContextCache::default();
        cache.insert(
            "key-a".to_string(),
            context("api-key-a"),
            Duration::from_secs(60),
            10,
        );
        cache.insert(
            "key-b".to_string(),
            context("api-key-b"),
            Duration::from_secs(60),
            10,
        );
        cache.insert(
            "negative:key-a".to_string(),
            context("negative-api-key-a"),
            Duration::from_secs(60),
            10,
        );
        let old_leader = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-a should register a leader"),
        };
        let old_generation = old_leader.generation();
        let follower = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second key-a registration should follow"),
        };

        cache.invalidate("key-a");

        tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("invalidating key-a should release its follower")
            .expect("explicit invalidation should allow a retry");
        assert!(cache.get_fresh("key-a", Duration::from_secs(60)).is_none());
        assert!(cache
            .get_fresh("negative:key-a", Duration::from_secs(60))
            .is_none());
        assert_eq!(
            cache
                .get_fresh("key-b", Duration::from_secs(60))
                .expect("unrelated cache entry should survive")
                .api_key_id,
            "api-key-b"
        );
        assert!(!old_leader.generation_is_current());
        assert!(!cache.insert_if_generation(
            "key-a".to_string(),
            context("stale-key"),
            Duration::from_secs(60),
            10,
            &old_generation,
        ));

        let replacement = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("invalidated key should admit a replacement leader"),
        };
        drop(old_leader);
        assert!(matches!(
            cache.register_inflight("key-a"),
            AuthContextInflightRegistration::Follower(_)
        ));
        drop(replacement);
    }

    #[tokio::test]
    async fn leader_drop_before_follower_first_poll_does_not_lose_wakeup() {
        let cache = AuthContextCache::default();
        let leader = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        drop(leader);
        tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("completion before first poll must release the follower")
            .expect("successful flight should not publish an error");
    }

    #[tokio::test]
    async fn flight_failure_is_shared_and_cannot_remove_replacement() {
        let cache = AuthContextCache::default();
        cache.insert(
            "key-a".to_string(),
            context("stale-key"),
            Duration::from_secs(60),
            10,
        );
        let old_leader = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        old_leader.fail(GatewayError::Internal(
            "forced auth load failure".to_string(),
        ));
        let error = tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("failed flight should release its follower")
            .expect_err("follower should observe the leader error");
        assert_eq!(error.into_message(), "forced auth load failure");
        assert!(cache.get_fresh("key-a", Duration::from_secs(60)).is_none());

        let replacement = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("failed flight should allow a replacement"),
        };
        drop(old_leader);
        assert!(matches!(
            cache.register_inflight("key-a"),
            AuthContextInflightRegistration::Follower(_)
        ));
        drop(replacement);
    }

    #[tokio::test]
    async fn invalidation_wins_over_detached_leader_failure() {
        let cache = AuthContextCache::default();
        let old_leader = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        cache.invalidate("key-a");
        old_leader.fail(GatewayError::Internal(
            "superseded auth load failure".to_string(),
        ));
        follower
            .wait()
            .await
            .expect("invalidation should keep the detached flight retryable");
    }

    #[test]
    fn owned_refresh_allows_only_one_leader_per_key() {
        let cache = Arc::new(AuthContextCache::default());
        let leader = cache
            .try_register_owned_leader("key-a")
            .expect("first refresh should lead");
        assert!(cache.try_register_owned_leader("key-a").is_none());
        assert!(cache.try_register_owned_leader("key-b").is_some());

        drop(leader);
        assert!(cache.try_register_owned_leader("key-a").is_some());
    }

    #[test]
    fn clear_rejects_owned_refresh_publication_and_preserves_replacement() {
        let cache = Arc::new(AuthContextCache::default());
        let old_leader = cache
            .try_register_owned_leader("key-a")
            .expect("first refresh should lead");
        let old_generation = old_leader.generation();

        cache.clear();
        let replacement = cache
            .try_register_owned_leader("key-a")
            .expect("clear should allow a replacement refresh");
        assert!(!old_leader.generation_is_current());
        assert!(!cache.insert_if_generation(
            "key-a".to_string(),
            context("stale-key"),
            Duration::from_secs(60),
            10,
            &old_generation,
        ));

        drop(old_leader);
        assert!(cache.try_register_owned_leader("key-a").is_none());
        drop(replacement);
        assert!(cache.try_register_owned_leader("key-a").is_some());
    }

    #[tokio::test]
    async fn cancelled_owned_refresh_wakes_waiters_and_allows_retry() {
        let cache = Arc::new(AuthContextCache::default());
        let guard = cache
            .try_register_owned_leader("key-a")
            .expect("first refresh should lead");
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        let follower = match cache.register_inflight("key-a") {
            AuthContextInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("hard miss should follow the background refresh"),
        };

        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("cancellation must release existing waiters")
            .expect("cancellation should allow a retry");
        assert!(cache.try_register_owned_leader("key-a").is_some());
    }
}
