use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_cache::ExpiringMap;
use aether_data::repository::auth::*;
use aether_data::DataLayerError;
use async_trait::async_trait;
use tokio::sync::futures::OwnedNotified;
use tokio::sync::Notify;

// Security revalidation bypasses this throughput-oriented read cache.
const AUTH_API_KEY_SNAPSHOT_CACHE_TTL: Duration = Duration::from_secs(30);
const AUTH_API_KEY_SNAPSHOT_CACHE_MAX_ENTRIES: usize = 16_384;

tokio::task_local! {
    static AUTH_API_KEY_READ_CACHE_BYPASS: ();
}

pub(super) struct CachedAuthApiKeyReadRepository {
    inner: Arc<dyn AuthApiKeyReadRepository>,
    snapshots: ExpiringMap<AuthApiKeySnapshotCacheKey, Option<StoredAuthApiKeySnapshot>>,
    // Loads for unrelated identities must not queue behind one global mutex. A
    // per-key notification keeps same-key loads singleflight without retaining
    // an async mutex (or a cancelled waiter) in the map.
    inflight: std::sync::Mutex<HashMap<AuthApiKeySnapshotCacheKey, Arc<AuthApiKeyInflightState>>>,
    generation: AtomicU64,
    mutation: std::sync::Mutex<()>,
}

impl CachedAuthApiKeyReadRepository {
    pub(super) fn new(inner: Arc<dyn AuthApiKeyReadRepository>) -> Self {
        Self {
            inner,
            snapshots: ExpiringMap::new(),
            inflight: std::sync::Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            mutation: std::sync::Mutex::new(()),
        }
    }

    fn insert_if_generation(
        &self,
        cache_key: AuthApiKeySnapshotCacheKey,
        value: Option<StoredAuthApiKeySnapshot>,
        generation: u64,
    ) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        self.snapshots.insert(
            cache_key,
            value,
            AUTH_API_KEY_SNAPSHOT_CACHE_TTL,
            AUTH_API_KEY_SNAPSHOT_CACHE_MAX_ENTRIES,
        );
    }

    pub(super) fn clear_cache(&self) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.snapshots.clear();
        let states = self
            .inflight
            .lock()
            .map(|mut inflight| inflight.drain().map(|(_, state)| state).collect::<Vec<_>>())
            .unwrap_or_default();
        for state in states {
            state.complete();
        }
    }

    fn register_inflight(
        &self,
        cache_key: &AuthApiKeySnapshotCacheKey,
    ) -> AuthApiKeyInflightRegistration<'_> {
        match self.inflight.lock() {
            Ok(mut inflight) => {
                if let Some(state) = inflight.get(cache_key) {
                    AuthApiKeyInflightRegistration::Follower(state.waiter())
                } else {
                    let state = Arc::new(AuthApiKeyInflightState::new());
                    inflight.insert(cache_key.clone(), Arc::clone(&state));
                    AuthApiKeyInflightRegistration::Leader(AuthApiKeyInflightGuard {
                        cache: self,
                        cache_key: Some(cache_key.clone()),
                        state,
                        generation: self.generation.load(Ordering::Acquire),
                    })
                }
            }
            Err(_) => AuthApiKeyInflightRegistration::Bypass,
        }
    }

    fn cache_key(key: AuthApiKeyLookupKey<'_>) -> AuthApiKeySnapshotCacheKey {
        match key {
            AuthApiKeyLookupKey::KeyHash(value) => {
                AuthApiKeySnapshotCacheKey::KeyHash(value.to_string())
            }
            AuthApiKeyLookupKey::ApiKeyId(value) => {
                AuthApiKeySnapshotCacheKey::ApiKeyId(value.to_string())
            }
            AuthApiKeyLookupKey::UserApiKeyIds {
                user_id,
                api_key_id,
            } => AuthApiKeySnapshotCacheKey::UserApiKeyIds {
                user_id: user_id.to_string(),
                api_key_id: api_key_id.to_string(),
            },
        }
    }
}

impl super::GatewayDataState {
    pub(crate) fn clear_auth_api_key_read_cache(&self) {
        if let Some(repository) = self.auth_api_key_reader.as_ref() {
            repository.clear_cache();
        }
    }

    #[cfg(test)]
    pub(crate) fn with_cached_auth_api_key_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: AuthRepository + 'static,
    {
        let inner: Arc<dyn AuthApiKeyReadRepository> = repository.clone();
        let cached: Arc<dyn AuthApiKeyReadRepository> =
            Arc::new(CachedAuthApiKeyReadRepository::new(inner));
        let mut state = Self::with_auth_api_key_repository_for_tests(repository);
        state.auth_api_key_reader = Some(cached);
        state
    }

    pub(crate) async fn read_auth_api_key_snapshot_strong(
        &self,
        user_id: &str,
        api_key_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<crate::data::auth::GatewayAuthApiKeySnapshot>, DataLayerError> {
        AUTH_API_KEY_READ_CACHE_BYPASS
            .scope(
                (),
                self.read_auth_api_key_snapshot(user_id, api_key_id, now_unix_secs),
            )
            .await
    }

    pub(crate) async fn read_auth_api_key_snapshot_by_key_hash_strong(
        &self,
        key_hash: &str,
        now_unix_secs: u64,
    ) -> Result<Option<crate::data::auth::GatewayAuthApiKeySnapshot>, DataLayerError> {
        AUTH_API_KEY_READ_CACHE_BYPASS
            .scope(
                (),
                self.read_auth_api_key_snapshot_by_key_hash(key_hash, now_unix_secs),
            )
            .await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum AuthApiKeySnapshotCacheKey {
    KeyHash(String),
    ApiKeyId(String),
    UserApiKeyIds { user_id: String, api_key_id: String },
}

enum AuthApiKeyInflightRegistration<'a> {
    Leader(AuthApiKeyInflightGuard<'a>),
    Follower(AuthApiKeyInflightWaiter),
    Bypass,
}

struct AuthApiKeyInflightState {
    completed: AtomicBool,
    error: std::sync::Mutex<Option<DataLayerError>>,
    notify: Arc<Notify>,
}

impl AuthApiKeyInflightState {
    fn new() -> Self {
        Self {
            completed: AtomicBool::new(false),
            error: std::sync::Mutex::new(None),
            notify: Arc::new(Notify::new()),
        }
    }

    fn waiter(self: &Arc<Self>) -> AuthApiKeyInflightWaiter {
        AuthApiKeyInflightWaiter {
            state: Arc::clone(self),
            notified: Arc::clone(&self.notify).notified_owned(),
        }
    }

    fn complete(&self) {
        if !self.completed.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    fn fail(&self, error: DataLayerError) {
        if let Ok(mut current) = self.error.lock() {
            *current = Some(error);
        }
        self.complete();
    }

    fn error(&self) -> Option<DataLayerError> {
        self.error.lock().ok().and_then(|error| error.clone())
    }
}

struct AuthApiKeyInflightWaiter {
    state: Arc<AuthApiKeyInflightState>,
    notified: OwnedNotified,
}

impl AuthApiKeyInflightWaiter {
    async fn wait(self) -> Result<(), DataLayerError> {
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

struct AuthApiKeyInflightGuard<'a> {
    cache: &'a CachedAuthApiKeyReadRepository,
    cache_key: Option<AuthApiKeySnapshotCacheKey>,
    state: Arc<AuthApiKeyInflightState>,
    generation: u64,
}

impl AuthApiKeyInflightGuard<'_> {
    fn fail(&self, error: DataLayerError) {
        let Some(cache_key) = self.cache_key.as_ref() else {
            return;
        };
        let removed_current = self
            .cache
            .inflight
            .lock()
            .map(|mut inflight| {
                if inflight
                    .get(cache_key)
                    .is_some_and(|current| Arc::ptr_eq(current, &self.state))
                {
                    inflight.remove(cache_key);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if removed_current {
            self.state.fail(error);
        }
    }
}

impl Drop for AuthApiKeyInflightGuard<'_> {
    fn drop(&mut self) {
        let Some(cache_key) = self.cache_key.take() else {
            return;
        };
        let removed = self
            .cache
            .inflight
            .lock()
            .map(|mut inflight| {
                inflight
                    .get(&cache_key)
                    .is_some_and(|current| Arc::ptr_eq(current, &self.state))
                    && inflight.remove(&cache_key).is_some()
            })
            .unwrap_or(false);
        if removed {
            self.state.complete();
        }
    }
}

#[async_trait]
impl AuthApiKeyReadRepository for CachedAuthApiKeyReadRepository {
    async fn find_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, DataLayerError> {
        if AUTH_API_KEY_READ_CACHE_BYPASS
            .try_with(|_| true)
            .unwrap_or(false)
        {
            return self.inner.find_api_key_snapshot(key).await;
        }

        let cache_key = Self::cache_key(key);
        if let Some(value) = self
            .snapshots
            .get_fresh(&cache_key, AUTH_API_KEY_SNAPSHOT_CACHE_TTL)
        {
            return Ok(value);
        }

        loop {
            match self.register_inflight(&cache_key) {
                AuthApiKeyInflightRegistration::Leader(guard) => {
                    if let Some(value) = self
                        .snapshots
                        .get_fresh(&cache_key, AUTH_API_KEY_SNAPSHOT_CACHE_TTL)
                    {
                        return Ok(value);
                    }

                    let value = match self.inner.find_api_key_snapshot(key).await {
                        Ok(value) => value,
                        Err(error) => {
                            guard.fail(error.clone());
                            return Err(error);
                        }
                    };
                    self.insert_if_generation(cache_key.clone(), value.clone(), guard.generation);
                    return Ok(value);
                }
                AuthApiKeyInflightRegistration::Follower(waiter) => {
                    waiter.wait().await?;
                    if let Some(value) = self
                        .snapshots
                        .get_fresh(&cache_key, AUTH_API_KEY_SNAPSHOT_CACHE_TTL)
                    {
                        return Ok(value);
                    }
                }
                AuthApiKeyInflightRegistration::Bypass => {
                    let generation = self.generation.load(Ordering::Acquire);
                    let value = self.inner.find_api_key_snapshot(key).await?;
                    self.insert_if_generation(cache_key.clone(), value.clone(), generation);
                    return Ok(value);
                }
            }
        }
    }

    async fn list_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, DataLayerError> {
        let mut snapshots = Vec::with_capacity(api_key_ids.len());
        for api_key_id in api_key_ids {
            if let Some(snapshot) = self
                .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId(api_key_id))
                .await?
            {
                snapshots.push(snapshot);
            }
        }
        Ok(snapshots)
    }

    fn clear_cache(&self) {
        CachedAuthApiKeyReadRepository::clear_cache(self);
    }

    async fn list_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.inner.list_export_api_keys_by_user_ids(user_ids).await
    }

    async fn list_export_api_keys_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.inner.list_export_api_keys_by_ids(api_key_ids).await
    }

    async fn list_export_api_keys_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.inner
            .list_export_api_keys_by_name_search(name_search)
            .await
    }

    async fn list_export_standalone_api_keys_page(
        &self,
        query: &StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.inner.list_export_standalone_api_keys_page(query).await
    }

    async fn count_export_standalone_api_keys(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, DataLayerError> {
        self.inner.count_export_standalone_api_keys(is_active).await
    }

    async fn summarize_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        self.inner
            .summarize_export_api_keys_by_user_ids(user_ids, now_unix_secs)
            .await
    }

    async fn summarize_export_non_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        self.inner
            .summarize_export_non_standalone_api_keys(now_unix_secs)
            .await
    }

    async fn summarize_export_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        self.inner
            .summarize_export_standalone_api_keys(now_unix_secs)
            .await
    }

    async fn find_export_standalone_api_key_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.inner
            .find_export_standalone_api_key_by_id(api_key_id)
            .await
    }

    async fn list_export_standalone_api_keys(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.inner.list_export_standalone_api_keys().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data::repository::auth::InMemoryAuthApiKeySnapshotRepository;

    fn sample_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(200),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
        )
        .expect("snapshot should build")
    }

    #[tokio::test]
    async fn concurrent_same_key_loads_once_and_reuses_cached_snapshot() {
        let inner = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::seed([(
                None,
                sample_snapshot("key-a", "user-a"),
            )])
            .with_lookup_delay_for_tests(Duration::from_millis(25)),
        );
        let repository = Arc::new(CachedAuthApiKeyReadRepository::new(inner.clone()));
        let mut tasks = Vec::new();

        for _ in 0..32 {
            let repository = Arc::clone(&repository);
            tasks.push(tokio::spawn(async move {
                repository
                    .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-a"))
                    .await
                    .expect("cached lookup should succeed")
                    .expect("snapshot should exist")
            }));
        }

        for task in tasks {
            assert_eq!(
                task.await.expect("lookup task should join").api_key_id,
                "key-a"
            );
        }
        assert_eq!(inner.snapshot_lookup_count("key-a"), 1);
        assert!(repository
            .inflight
            .lock()
            .expect("inflight lock should not be poisoned")
            .is_empty());
    }

    #[tokio::test]
    async fn different_keys_use_independent_inflight_notifications() {
        let inner = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed([]));
        let repository = CachedAuthApiKeyReadRepository::new(inner);
        let key_a = AuthApiKeySnapshotCacheKey::ApiKeyId("key-a".to_string());
        let key_b = AuthApiKeySnapshotCacheKey::ApiKeyId("key-b".to_string());

        let leader_a = repository.register_inflight(&key_a);
        let follower_a = repository.register_inflight(&key_a);
        let leader_b = repository.register_inflight(&key_b);

        assert!(matches!(
            leader_a,
            AuthApiKeyInflightRegistration::Leader(_)
        ));
        assert!(matches!(
            follower_a,
            AuthApiKeyInflightRegistration::Follower(_)
        ));
        assert!(matches!(
            leader_b,
            AuthApiKeyInflightRegistration::Leader(_)
        ));
    }

    #[tokio::test]
    async fn cancelled_leader_releases_inflight_key() {
        let inner = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::seed([])
                .with_lookup_delay_for_tests(Duration::from_secs(30)),
        );
        let repository = Arc::new(CachedAuthApiKeyReadRepository::new(inner));
        let lookup_repository = Arc::clone(&repository);
        let task = tokio::spawn(async move {
            lookup_repository
                .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("cancelled-key"))
                .await
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if repository
                    .inflight
                    .lock()
                    .expect("inflight lock should not be poisoned")
                    .contains_key(&AuthApiKeySnapshotCacheKey::ApiKeyId(
                        "cancelled-key".to_string(),
                    ))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("lookup should register its inflight key");

        task.abort();
        let _ = task.await;
        assert!(repository
            .inflight
            .lock()
            .expect("inflight lock should not be poisoned")
            .is_empty());
    }

    #[tokio::test]
    async fn follower_observes_completion_before_first_poll() {
        let inner = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed([]));
        let repository = CachedAuthApiKeyReadRepository::new(inner);
        let key = AuthApiKeySnapshotCacheKey::ApiKeyId("key-a".to_string());
        let leader = match repository.register_inflight(&key) {
            AuthApiKeyInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match repository.register_inflight(&key) {
            AuthApiKeyInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        // Complete before the OwnedNotified is polled. A bare notify_waiters()
        // broadcast would be lost in this ordering.
        drop(leader);
        tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("completed follower must not miss the broadcast")
            .expect("successful flight should not publish an error");
    }

    #[tokio::test]
    async fn failed_flight_shares_error_and_preserves_replacement() {
        let inner = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed([]));
        let repository = CachedAuthApiKeyReadRepository::new(inner);
        let key = AuthApiKeySnapshotCacheKey::ApiKeyId("key-a".to_string());
        let old_leader = match repository.register_inflight(&key) {
            AuthApiKeyInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match repository.register_inflight(&key) {
            AuthApiKeyInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        old_leader.fail(DataLayerError::Sql(
            "forced auth snapshot load failure".to_string(),
        ));
        let error = tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("failed flight should release its follower")
            .expect_err("follower should observe the leader error");
        assert_eq!(
            error.to_string(),
            "sql error: forced auth snapshot load failure"
        );

        let replacement = match repository.register_inflight(&key) {
            AuthApiKeyInflightRegistration::Leader(guard) => guard,
            _ => panic!("failed flight should allow a replacement"),
        };
        drop(old_leader);
        assert!(matches!(
            repository.register_inflight(&key),
            AuthApiKeyInflightRegistration::Follower(_)
        ));
        drop(replacement);
    }

    #[tokio::test]
    async fn strong_read_bypasses_a_fresh_cached_allow_snapshot() {
        let inner = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed([(
            None,
            sample_snapshot("key-a", "user-a"),
        )]));
        let repository = CachedAuthApiKeyReadRepository::new(inner.clone());
        let lookup = AuthApiKeyLookupKey::UserApiKeyIds {
            user_id: "user-a",
            api_key_id: "key-a",
        };

        let cached = repository
            .find_api_key_snapshot(lookup)
            .await
            .expect("initial lookup should succeed")
            .expect("snapshot should exist");
        assert!(!cached.api_key_is_locked);
        assert!(inner
            .set_user_api_key_locked("user-a", "key-a", true)
            .await
            .expect("cross-node lock should succeed"));

        let still_cached = repository
            .find_api_key_snapshot(lookup)
            .await
            .expect("cached lookup should succeed")
            .expect("snapshot should exist");
        assert!(!still_cached.api_key_is_locked);

        let strong = AUTH_API_KEY_READ_CACHE_BYPASS
            .scope((), repository.find_api_key_snapshot(lookup))
            .await
            .expect("strong lookup should succeed")
            .expect("snapshot should exist");
        assert!(strong.api_key_is_locked);
        assert_eq!(inner.snapshot_lookup_count("key-a"), 2);
    }

    #[test]
    fn clear_cache_rejects_old_leader_publication() {
        let inner = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed([]));
        let repository = CachedAuthApiKeyReadRepository::new(inner);
        let key = AuthApiKeySnapshotCacheKey::ApiKeyId("key-a".to_string());
        let leader = match repository.register_inflight(&key) {
            AuthApiKeyInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-a should register a leader"),
        };
        let old_generation = leader.generation;

        repository.clear_cache();
        repository.insert_if_generation(
            key.clone(),
            Some(sample_snapshot("stale-key", "user-a")),
            old_generation,
        );
        assert!(repository
            .snapshots
            .get_fresh(&key, AUTH_API_KEY_SNAPSHOT_CACHE_TTL)
            .is_none());
    }
}
