use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use aether_cache::ExpiringMap;
use serde_json::Value;
use tokio::sync::futures::OwnedNotified;
use tokio::sync::Notify;

use crate::data::auth::GatewayAuthApiKeySnapshot;

const AUTH_RUNTIME_CACHE_MAX_ENTRIES: usize = 16_384;

#[derive(Debug)]
struct CacheSingleflight<K> {
    inflight: StdMutex<HashMap<K, std::sync::Arc<CacheInflightState>>>,
    // Invalidations advance this generation. A load that started before an
    // invalidation must not publish its stale result after the clear.
    generation: AtomicU64,
}

struct CacheInflightState {
    completed: AtomicBool,
    error: StdMutex<Option<Box<dyn Any + Send + Sync>>>,
    notify: std::sync::Arc<Notify>,
}

impl fmt::Debug for CacheInflightState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CacheInflightState")
            .field("completed", &self.completed.load(Ordering::Acquire))
            .field(
                "has_error",
                &self
                    .error
                    .lock()
                    .map(|error| error.is_some())
                    .unwrap_or(true),
            )
            .finish_non_exhaustive()
    }
}

impl CacheInflightState {
    fn new() -> Self {
        Self {
            completed: AtomicBool::new(false),
            error: StdMutex::new(None),
            notify: std::sync::Arc::new(Notify::new()),
        }
    }

    fn waiter(self: &std::sync::Arc<Self>) -> CacheInflightWaiter {
        CacheInflightWaiter {
            state: std::sync::Arc::clone(self),
            notified: std::sync::Arc::clone(&self.notify).notified_owned(),
        }
    }

    fn complete(&self) {
        self.completed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn fail<E>(&self, error: E)
    where
        E: Clone + Send + Sync + 'static,
    {
        if let Ok(mut current) = self.error.lock() {
            *current = Some(Box::new(error));
        }
        self.complete();
    }

    fn error<E>(&self) -> Option<E>
    where
        E: Clone + Send + Sync + 'static,
    {
        // Cache methods are generic over E, so retain the concrete error for
        // same-typed followers. A mismatched caller type safely falls back to
        // the existing retry path instead of receiving the wrong error type.
        self.error
            .lock()
            .ok()
            .and_then(|error| error.as_ref()?.downcast_ref::<E>().cloned())
    }
}

struct CacheInflightWaiter {
    state: std::sync::Arc<CacheInflightState>,
    notified: OwnedNotified,
}

impl CacheInflightWaiter {
    async fn wait<E>(self) -> Result<(), E>
    where
        E: Clone + Send + Sync + 'static,
    {
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

impl<K> Default for CacheSingleflight<K> {
    fn default() -> Self {
        Self {
            inflight: StdMutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
        }
    }
}

enum CacheInflightRegistration<'a, K: Eq + Hash> {
    Leader(CacheInflightGuard<'a, K>),
    Follower(CacheInflightWaiter),
    Bypass,
}

struct CacheInflightGuard<'a, K: Eq + Hash> {
    singleflight: &'a CacheSingleflight<K>,
    key: Option<K>,
    state: std::sync::Arc<CacheInflightState>,
    generation: u64,
}

struct CacheOwnedInflightGuard<K: Eq + Hash> {
    singleflight: std::sync::Arc<CacheSingleflight<K>>,
    key: Option<K>,
    state: std::sync::Arc<CacheInflightState>,
    generation: u64,
}

impl<K> CacheSingleflight<K>
where
    K: Eq + Hash,
{
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    fn generation_is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Acquire) == generation
    }

    fn finish(&self, key: &K, state: &std::sync::Arc<CacheInflightState>) {
        let removed = self
            .inflight
            .lock()
            .map(|mut inflight| {
                inflight
                    .get(key)
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, state))
                    && inflight.remove(key).is_some()
            })
            .unwrap_or(false);
        if removed {
            state.complete();
        }
    }

    fn fail<E>(&self, key: &K, state: &std::sync::Arc<CacheInflightState>, error: E)
    where
        E: Clone + Send + Sync + 'static,
    {
        let removed_current = self
            .inflight
            .lock()
            .map(|mut inflight| {
                if inflight
                    .get(key)
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, state))
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

    fn clear(&self) {
        let states = self
            .inflight
            .lock()
            .map(|mut inflight| {
                self.generation.fetch_add(1, Ordering::AcqRel);
                inflight.drain().map(|(_, state)| state).collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for state in states {
            state.complete();
        }
    }
}

impl<K> CacheSingleflight<K>
where
    K: Clone + Eq + Hash,
{
    fn register(&self, key: &K) -> CacheInflightRegistration<'_, K> {
        match self.inflight.lock() {
            Ok(mut inflight) => {
                if let Some(state) = inflight.get(key) {
                    // Register while the map lock is held so leader completion
                    // cannot race between lookup and waiter creation.
                    CacheInflightRegistration::Follower(state.waiter())
                } else {
                    let state = std::sync::Arc::new(CacheInflightState::new());
                    inflight.insert(key.clone(), std::sync::Arc::clone(&state));
                    CacheInflightRegistration::Leader(CacheInflightGuard {
                        singleflight: self,
                        key: Some(key.clone()),
                        state,
                        generation: self.generation(),
                    })
                }
            }
            Err(_) => CacheInflightRegistration::Bypass,
        }
    }

    fn try_register_owned_leader(
        self: &std::sync::Arc<Self>,
        key: &K,
    ) -> Option<CacheOwnedInflightGuard<K>> {
        let Ok(mut inflight) = self.inflight.lock() else {
            return None;
        };
        if inflight.contains_key(key) {
            return None;
        }

        let state = std::sync::Arc::new(CacheInflightState::new());
        inflight.insert(key.clone(), std::sync::Arc::clone(&state));
        Some(CacheOwnedInflightGuard {
            singleflight: std::sync::Arc::clone(self),
            key: Some(key.clone()),
            state,
            generation: self.generation(),
        })
    }
}

impl<K> Drop for CacheInflightGuard<'_, K>
where
    K: Eq + Hash,
{
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.singleflight.finish(&key, &self.state);
        }
    }
}

impl<K> CacheInflightGuard<'_, K>
where
    K: Eq + Hash,
{
    fn generation_is_current(&self) -> bool {
        self.singleflight.generation_is_current(self.generation)
    }

    fn fail<E>(&self, error: E)
    where
        E: Clone + Send + Sync + 'static,
    {
        if let Some(key) = self.key.as_ref() {
            self.singleflight.fail(key, &self.state, error);
        }
    }
}

impl<K> Drop for CacheOwnedInflightGuard<K>
where
    K: Eq + Hash,
{
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.singleflight.finish(&key, &self.state);
        }
    }
}

impl<K> CacheOwnedInflightGuard<K>
where
    K: Eq + Hash,
{
    fn fail<E>(&self, error: E)
    where
        E: Clone + Send + Sync + 'static,
    {
        if let Some(key) = self.key.as_ref() {
            self.singleflight.fail(key, &self.state, error);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AuthApiKeyIdentityCacheKey {
    user_id: String,
    api_key_id: String,
}

impl AuthApiKeyIdentityCacheKey {
    pub(crate) fn new(user_id: &str, api_key_id: &str) -> Self {
        Self {
            user_id: user_id.trim().to_string(),
            api_key_id: api_key_id.trim().to_string(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.user_id.is_empty() || self.api_key_id.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AuthApiKeyFeatureCacheKey {
    user_id: String,
    api_key_id: String,
    is_standalone: bool,
}

impl AuthApiKeyFeatureCacheKey {
    pub(crate) fn new(user_id: &str, api_key_id: &str, is_standalone: bool) -> Self {
        Self {
            user_id: user_id.trim().to_string(),
            api_key_id: api_key_id.trim().to_string(),
            is_standalone,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.api_key_id.is_empty() || (!self.is_standalone && self.user_id.is_empty())
    }
}

#[derive(Debug, Default)]
pub(crate) struct AuthSnapshotCache {
    entries: ExpiringMap<AuthSnapshotCacheKey, Option<GatewayAuthApiKeySnapshot>>,
    singleflight: CacheSingleflight<AuthSnapshotCacheKey>,
    mutation: StdMutex<()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum AuthSnapshotCacheKey {
    KeyHash(String),
    UserApiKeyIds(AuthApiKeyIdentityCacheKey),
}

impl AuthSnapshotCacheKey {
    pub(crate) fn key_hash(key_hash: &str) -> Self {
        Self::KeyHash(key_hash.trim().to_string())
    }

    pub(crate) fn user_api_key_ids(user_id: &str, api_key_id: &str) -> Self {
        Self::UserApiKeyIds(AuthApiKeyIdentityCacheKey::new(user_id, api_key_id))
    }

    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::KeyHash(key_hash) => key_hash.is_empty(),
            Self::UserApiKeyIds(key) => key.is_empty(),
        }
    }
}

impl AuthSnapshotCache {
    pub(crate) fn generation(&self) -> u64 {
        self.singleflight.generation()
    }

    pub(crate) fn get(
        &self,
        key: &AuthSnapshotCacheKey,
        ttl: Duration,
    ) -> Option<Option<GatewayAuthApiKeySnapshot>> {
        self.entries.get_fresh(key, ttl)
    }

    pub(crate) fn insert_if_generation(
        &self,
        key: AuthSnapshotCacheKey,
        value: Option<GatewayAuthApiKeySnapshot>,
        ttl: Duration,
        generation: u64,
    ) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        if !self.singleflight.generation_is_current(generation) {
            return;
        }
        self.entries
            .insert(key, value, ttl, AUTH_RUNTIME_CACHE_MAX_ENTRIES);
    }

    pub(crate) async fn get_or_load<E, F, Fut>(
        &self,
        key: AuthSnapshotCacheKey,
        ttl: Duration,
        mut load: F,
    ) -> Result<Option<GatewayAuthApiKeySnapshot>, E>
    where
        E: Clone + Send + Sync + 'static,
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Option<GatewayAuthApiKeySnapshot>, E>>,
    {
        if let Some(value) = self.get(&key, ttl) {
            return Ok(value);
        }

        loop {
            match self.singleflight.register(&key) {
                CacheInflightRegistration::Bypass => {
                    let generation = self.singleflight.generation();
                    let value = load().await?;
                    self.insert_if_generation(key, value.clone(), ttl, generation);
                    return Ok(value);
                }
                CacheInflightRegistration::Follower(waiter) => {
                    waiter.wait().await?;
                    if let Some(value) = self.get(&key, ttl) {
                        return Ok(value);
                    }
                }
                CacheInflightRegistration::Leader(guard) => {
                    let value = match load().await {
                        Ok(value) => value,
                        Err(error) => {
                            guard.fail(error.clone());
                            return Err(error);
                        }
                    };
                    if guard.generation_is_current() {
                        self.insert_if_generation(key, value.clone(), ttl, guard.generation);
                    }
                    return Ok(value);
                }
            }
        }
    }

    pub(crate) fn clear(&self) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.entries.clear();
        self.singleflight.clear();
    }
}

#[derive(Debug)]
pub(crate) struct JsonValueCache<K> {
    entries: ExpiringMap<K, Option<Value>>,
    singleflight: CacheSingleflight<K>,
    mutation: StdMutex<()>,
}

impl<K> Default for JsonValueCache<K> {
    fn default() -> Self {
        Self {
            entries: ExpiringMap::default(),
            singleflight: CacheSingleflight::default(),
            mutation: StdMutex::new(()),
        }
    }
}

impl<K> JsonValueCache<K>
where
    K: Clone + Eq + Hash,
{
    pub(crate) fn get(&self, key: &K, ttl: Duration) -> Option<Option<Value>> {
        self.entries.get_fresh(key, ttl)
    }

    pub(crate) fn insert(&self, key: K, value: Option<Value>, ttl: Duration) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.entries
            .insert(key, value, ttl, AUTH_RUNTIME_CACHE_MAX_ENTRIES);
        self.singleflight.clear();
    }

    fn insert_if_generation(&self, key: K, value: Option<Value>, ttl: Duration, generation: u64) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        if !self.singleflight.generation_is_current(generation) {
            return;
        }
        self.entries
            .insert(key, value, ttl, AUTH_RUNTIME_CACHE_MAX_ENTRIES);
    }

    pub(crate) async fn get_or_load<E, F, Fut>(
        &self,
        key: K,
        ttl: Duration,
        mut load: F,
    ) -> Result<Option<Value>, E>
    where
        E: Clone + Send + Sync + 'static,
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Option<Value>, E>>,
    {
        if let Some(value) = self.get(&key, ttl) {
            return Ok(value);
        }

        loop {
            match self.singleflight.register(&key) {
                CacheInflightRegistration::Bypass => {
                    let generation = self.singleflight.generation();
                    let value = load().await?;
                    self.insert_if_generation(key, value.clone(), ttl, generation);
                    return Ok(value);
                }
                CacheInflightRegistration::Follower(waiter) => {
                    waiter.wait().await?;
                    if let Some(value) = self.get(&key, ttl) {
                        return Ok(value);
                    }
                }
                CacheInflightRegistration::Leader(guard) => {
                    let value = match load().await {
                        Ok(value) => value,
                        Err(error) => {
                            guard.fail(error.clone());
                            return Err(error);
                        }
                    };
                    self.insert_if_generation(key, value.clone(), ttl, guard.generation);
                    return Ok(value);
                }
            }
        }
    }

    pub(crate) fn clear(&self) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.entries.clear();
        self.singleflight.clear();
    }
}

#[derive(Debug)]
pub(crate) struct ValueCache<K, V> {
    entries: ExpiringMap<K, Option<V>>,
    singleflight: std::sync::Arc<CacheSingleflight<K>>,
    mutation: StdMutex<()>,
}

impl<K, V> Default for ValueCache<K, V> {
    fn default() -> Self {
        Self {
            entries: ExpiringMap::default(),
            singleflight: std::sync::Arc::new(CacheSingleflight::default()),
            mutation: StdMutex::new(()),
        }
    }
}

impl<K, V> ValueCache<K, V>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub(crate) fn get(&self, key: &K, ttl: Duration) -> Option<Option<V>> {
        self.entries.get_fresh(key, ttl)
    }

    pub(crate) fn insert(&self, key: K, value: Option<V>, ttl: Duration) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.entries
            .insert(key, value, ttl, AUTH_RUNTIME_CACHE_MAX_ENTRIES);
        self.singleflight.clear();
    }

    fn insert_if_generation(&self, key: K, value: Option<V>, ttl: Duration, generation: u64) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        if !self.singleflight.generation_is_current(generation) {
            return;
        }
        self.entries
            .insert(key, value, ttl, AUTH_RUNTIME_CACHE_MAX_ENTRIES);
    }

    pub(crate) async fn get_or_load<E, F, Fut>(
        &self,
        key: K,
        ttl: Duration,
        mut load: F,
    ) -> Result<Option<V>, E>
    where
        E: Clone + Send + Sync + 'static,
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Option<V>, E>>,
    {
        if let Some(value) = self.get(&key, ttl) {
            return Ok(value);
        }

        loop {
            match self.singleflight.register(&key) {
                CacheInflightRegistration::Bypass => {
                    let generation = self.singleflight.generation();
                    let value = load().await?;
                    self.insert_if_generation(key, value.clone(), ttl, generation);
                    return Ok(value);
                }
                CacheInflightRegistration::Follower(waiter) => {
                    waiter.wait().await?;
                    if let Some(value) = self.get(&key, ttl) {
                        return Ok(value);
                    }
                }
                CacheInflightRegistration::Leader(guard) => {
                    let value = match load().await {
                        Ok(value) => value,
                        Err(error) => {
                            guard.fail(error.clone());
                            return Err(error);
                        }
                    };
                    self.insert_if_generation(key, value.clone(), ttl, guard.generation);
                    return Ok(value);
                }
            }
        }
    }

    pub(crate) async fn get_or_load_once<E, F, Fut>(
        &self,
        key: K,
        ttl: Duration,
        load: F,
    ) -> Result<Option<V>, E>
    where
        E: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<V>, E>>,
    {
        self.get_or_load_once_with_observer(key, ttl, load, CacheLoadObserver::default())
            .await
    }

    pub(crate) async fn get_or_load_once_with_observer<E, F, Fut>(
        &self,
        key: K,
        ttl: Duration,
        load: F,
        observer: CacheLoadObserver,
    ) -> Result<Option<V>, E>
    where
        E: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<V>, E>>,
    {
        if let Some(value) = self.get(&key, ttl) {
            observer.hit();
            return Ok(value);
        }
        observer.miss();

        let mut load = Some(load);
        loop {
            match self.singleflight.register(&key) {
                CacheInflightRegistration::Bypass => {
                    let generation = self.singleflight.generation();
                    observer.load();
                    let value =
                        load.take().expect("cache load closure should be available")().await?;
                    self.insert_if_generation(key, value.clone(), ttl, generation);
                    return Ok(value);
                }
                CacheInflightRegistration::Follower(waiter) => {
                    observer.follower_wait();
                    waiter.wait().await?;
                    if let Some(value) = self.get(&key, ttl) {
                        observer.hit();
                        return Ok(value);
                    }
                }
                CacheInflightRegistration::Leader(guard) => {
                    observer.load();
                    let value = match load.take().expect("cache load closure should be available")()
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => {
                            guard.fail(error.clone());
                            return Err(error);
                        }
                    };
                    self.insert_if_generation(key, value.clone(), ttl, guard.generation);
                    return Ok(value);
                }
            }
        }
    }

    pub(crate) async fn get_or_load_once_stale_while_refreshing<E, F, Fut>(
        &self,
        key: K,
        ttl: Duration,
        stale_ttl: Duration,
        load: F,
        observer: CacheLoadObserver,
    ) -> Result<Option<V>, E>
    where
        E: Clone + Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<V>, E>>,
    {
        if let Some((value, age)) = self.entries.get_with_age(&key, stale_ttl) {
            if age <= ttl {
                observer.hit();
                return Ok(value);
            }

            // Keep stale snapshots off the request critical path. The caller's
            // invalidation path clears entries when provider/catalog/routing
            // state changes, and the bounded stale TTL limits passive drift.
            observer.hit();
            return Ok(value);
        }

        observer.miss();
        let mut load = Some(load);
        loop {
            match self.singleflight.register(&key) {
                CacheInflightRegistration::Bypass => {
                    let generation = self.singleflight.generation();
                    observer.load();
                    let value =
                        load.take().expect("cache load closure should be available")().await?;
                    self.insert_if_generation(key, value.clone(), stale_ttl, generation);
                    return Ok(value);
                }
                CacheInflightRegistration::Follower(waiter) => {
                    observer.follower_wait();
                    waiter.wait().await?;
                    if let Some((value, _age)) = self.entries.get_with_age(&key, stale_ttl) {
                        observer.hit();
                        return Ok(value);
                    }
                }
                CacheInflightRegistration::Leader(guard) => {
                    observer.load();
                    let value = match load.take().expect("cache load closure should be available")()
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => {
                            guard.fail(error.clone());
                            return Err(error);
                        }
                    };
                    self.insert_if_generation(key, value.clone(), stale_ttl, guard.generation);
                    return Ok(value);
                }
            }
        }
    }

    pub(crate) async fn get_or_load_once_stale_while_revalidating<
        E,
        ColdLoad,
        ColdFuture,
        BackgroundRefresh,
        BackgroundFuture,
    >(
        self: &std::sync::Arc<Self>,
        key: K,
        ttl: Duration,
        stale_ttl: Duration,
        cold_load: ColdLoad,
        background_refresh: BackgroundRefresh,
        observer: CacheLoadObserver,
    ) -> Result<Option<V>, E>
    where
        K: Send + 'static,
        V: Send + 'static,
        E: Clone + Send + Sync + 'static,
        ColdLoad: FnOnce() -> ColdFuture,
        ColdFuture: Future<Output = Result<Option<V>, E>>,
        BackgroundRefresh: FnOnce() -> BackgroundFuture,
        BackgroundFuture: Future<Output = Result<Option<V>, E>> + Send + 'static,
    {
        let stale_ttl = stale_ttl.max(ttl);
        if let Some((value, age)) = self.entries.get_with_age(&key, stale_ttl) {
            observer.hit();
            if age > ttl {
                if let Some(guard) = self.singleflight.try_register_owned_leader(&key) {
                    observer.load();
                    let cache = std::sync::Arc::clone(self);
                    let generation = guard.generation;
                    // Build the owned refresh future only after this request
                    // wins stale revalidation. Fresh hits never clone the
                    // caller's plan/candidate inputs.
                    let refresh = background_refresh();
                    tokio::spawn(async move {
                        match refresh.await {
                            Ok(refreshed) => {
                                cache.insert_if_generation(key, refreshed, stale_ttl, generation);
                            }
                            Err(error) => guard.fail(error),
                        }
                        drop(guard);
                    });
                }
            }
            return Ok(value);
        }

        observer.miss();
        let mut cold_load = Some(cold_load);
        loop {
            match self.singleflight.register(&key) {
                CacheInflightRegistration::Bypass => {
                    let generation = self.singleflight.generation();
                    observer.load();
                    let cold_load = cold_load
                        .take()
                        .expect("cache cold-load closure should be available");
                    let value = cold_load().await?;
                    self.insert_if_generation(key, value.clone(), stale_ttl, generation);
                    return Ok(value);
                }
                CacheInflightRegistration::Follower(waiter) => {
                    observer.follower_wait();
                    waiter.wait().await?;
                    if let Some((value, _age)) = self.entries.get_with_age(&key, stale_ttl) {
                        observer.hit();
                        return Ok(value);
                    }
                }
                CacheInflightRegistration::Leader(guard) => {
                    observer.load();
                    let cold_load = cold_load
                        .take()
                        .expect("cache cold-load closure should be available");
                    let value = match cold_load().await {
                        Ok(value) => value,
                        Err(error) => {
                            guard.fail(error.clone());
                            return Err(error);
                        }
                    };
                    self.insert_if_generation(key, value.clone(), stale_ttl, guard.generation);
                    return Ok(value);
                }
            }
        }
    }

    pub(crate) fn clear(&self) {
        let Ok(_mutation) = self.mutation.lock() else {
            return;
        };
        self.entries.clear();
        self.singleflight.clear();
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct CacheLoadObserver {
    on_hit: Option<fn()>,
    on_miss: Option<fn()>,
    on_load: Option<fn()>,
    on_follower_wait: Option<fn()>,
}

impl CacheLoadObserver {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn on_hit(mut self, callback: fn()) -> Self {
        self.on_hit = Some(callback);
        self
    }

    pub(crate) fn on_miss(mut self, callback: fn()) -> Self {
        self.on_miss = Some(callback);
        self
    }

    pub(crate) fn on_load(mut self, callback: fn()) -> Self {
        self.on_load = Some(callback);
        self
    }

    pub(crate) fn on_follower_wait(mut self, callback: fn()) -> Self {
        self.on_follower_wait = Some(callback);
        self
    }

    fn hit(self) {
        if let Some(callback) = self.on_hit {
            callback();
        }
    }

    fn miss(self) {
        if let Some(callback) = self.on_miss {
            callback();
        }
    }

    fn load(self) {
        if let Some(callback) = self.on_load {
            callback();
        }
    }

    fn follower_wait(self) {
        if let Some(callback) = self.on_follower_wait {
            callback();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheInflightRegistration, CacheLoadObserver, CacheSingleflight, ValueCache};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn record_max(max_seen: &AtomicUsize, value: usize) {
        let mut current = max_seen.load(Ordering::Acquire);
        while value > current {
            match max_seen.compare_exchange(current, value, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
    }

    #[tokio::test]
    async fn cache_singleflight_notifies_only_followers_for_completed_key() {
        let singleflight = CacheSingleflight::<String>::default();
        let leader_a = match singleflight.register(&"key-a".to_string()) {
            CacheInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-a should register a leader"),
        };
        let leader_b = match singleflight.register(&"key-b".to_string()) {
            CacheInflightRegistration::Leader(guard) => guard,
            _ => panic!("key-b should register a leader"),
        };
        let follower_a = match singleflight.register(&"key-a".to_string()) {
            CacheInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second key-a registration should follow"),
        };
        let follower_b = match singleflight.register(&"key-b".to_string()) {
            CacheInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second key-b registration should follow"),
        };

        drop(leader_b);
        tokio::time::timeout(Duration::from_millis(100), follower_b.wait::<()>())
            .await
            .expect("key-b follower should wake when key-b completes")
            .expect("successful flight should not publish an error");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), follower_a.wait::<()>())
                .await
                .is_err(),
            "key-a follower must not wake when unrelated key-b completes"
        );

        drop(leader_a);
        assert!(singleflight.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cache_singleflight_leader_drop_before_follower_first_poll_does_not_lose_wakeup() {
        let singleflight = CacheSingleflight::<String>::default();
        let key = "before-first-poll".to_string();
        let leader = match singleflight.register(&key) {
            CacheInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let follower = match singleflight.register(&key) {
            CacheInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        // Drop before the waiter future is ever polled, as happens when a
        // loader completes between registration and task scheduling.
        drop(leader);
        tokio::time::timeout(Duration::from_millis(100), follower.wait::<()>())
            .await
            .expect("completion before first poll must still release the follower")
            .expect("successful flight should not publish an error");
    }

    #[tokio::test]
    async fn cache_singleflight_leader_drop_broadcasts_to_all_followers() {
        const FOLLOWER_COUNT: usize = 2_048;

        let singleflight = CacheSingleflight::<String>::default();
        let key = "broadcast".to_string();
        let leader = match singleflight.register(&key) {
            CacheInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let mut followers = Vec::with_capacity(FOLLOWER_COUNT);
        for _ in 0..FOLLOWER_COUNT {
            let waiter = match singleflight.register(&key) {
                CacheInflightRegistration::Follower(waiter) => waiter,
                _ => panic!("concurrent registration should follow"),
            };
            followers.push(tokio::spawn(waiter.wait::<()>()));
        }

        tokio::task::yield_now().await;
        drop(leader);
        tokio::time::timeout(Duration::from_secs(2), async {
            for follower in followers {
                follower
                    .await
                    .expect("follower task should join")
                    .expect("successful flight should not publish an error");
            }
        })
        .await
        .expect("one completion should broadcast to every follower");
        assert!(singleflight.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cache_singleflight_clear_wakes_old_followers_without_removing_replacement() {
        let singleflight = CacheSingleflight::<String>::default();
        let key = "clear-replacement".to_string();
        let old_generation = singleflight.generation();
        let old_leader = match singleflight.register(&key) {
            CacheInflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let old_follower = match singleflight.register(&key) {
            CacheInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("second registration should follow"),
        };

        singleflight.clear();
        assert_ne!(singleflight.generation(), old_generation);
        let replacement_leader = match singleflight.register(&key) {
            CacheInflightRegistration::Leader(guard) => guard,
            _ => panic!("clear should allow a replacement leader"),
        };
        let replacement_follower = match singleflight.register(&key) {
            CacheInflightRegistration::Follower(waiter) => waiter,
            _ => panic!("registration behind replacement should follow"),
        };

        drop(old_leader);
        assert_eq!(singleflight.inflight.lock().unwrap().len(), 1);
        tokio::time::timeout(Duration::from_millis(100), old_follower.wait::<()>())
            .await
            .expect("clear should wake followers of the invalidated flight")
            .expect("cache clear should allow an immediate retry");

        drop(replacement_leader);
        tokio::time::timeout(
            Duration::from_millis(100),
            replacement_follower.wait::<()>(),
        )
        .await
        .expect("old guard drop must not remove or strand the replacement flight")
        .expect("successful replacement should not publish an error");
        assert!(singleflight.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn value_cache_coalesces_concurrent_loads_for_same_key() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();

        for _ in 0..16 {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            tasks.push(tokio::spawn(async move {
                cache
                    .get_or_load::<(), _, _>(
                        "same-key".to_string(),
                        Duration::from_secs(60),
                        || async {
                            calls.fetch_add(1, Ordering::AcqRel);
                            tokio::time::sleep(Duration::from_millis(25)).await;
                            Ok(Some(42))
                        },
                    )
                    .await
                    .unwrap()
            }));
        }

        for task in tasks {
            assert_eq!(task.await.unwrap(), Some(42));
        }
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn value_cache_shares_leader_failure_without_follower_reload() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let leader_started = Arc::new(tokio::sync::Notify::new());
        let leader_release = Arc::new(tokio::sync::Notify::new());

        let leader_cache = Arc::clone(&cache);
        let leader_calls = Arc::clone(&calls);
        let started = Arc::clone(&leader_started);
        let release = Arc::clone(&leader_release);
        let leader = tokio::spawn(async move {
            leader_cache
                .get_or_load::<String, _, _>(
                    "failed-key".to_string(),
                    Duration::from_secs(60),
                    || {
                        let leader_calls = Arc::clone(&leader_calls);
                        let started = Arc::clone(&started);
                        let release = Arc::clone(&release);
                        async move {
                            leader_calls.fetch_add(1, Ordering::AcqRel);
                            started.notify_one();
                            release.notified().await;
                            Err("forced cache load failure".to_string())
                        }
                    },
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), leader_started.notified())
            .await
            .expect("leader load should start");

        let follower_cache = Arc::clone(&cache);
        let follower_calls = Arc::clone(&calls);
        let follower = tokio::spawn(async move {
            follower_cache
                .get_or_load::<String, _, _>(
                    "failed-key".to_string(),
                    Duration::from_secs(60),
                    || {
                        let follower_calls = Arc::clone(&follower_calls);
                        async move {
                            follower_calls.fetch_add(1, Ordering::AcqRel);
                            Err("follower loader must not run".to_string())
                        }
                    },
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let follower_registered = cache
                    .singleflight
                    .inflight
                    .lock()
                    .ok()
                    .and_then(|inflight| inflight.values().next().cloned())
                    .is_some_and(|state| Arc::strong_count(&state) >= 4);
                if follower_registered {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("follower should register behind the failed leader");

        leader_release.notify_one();
        assert_eq!(
            leader
                .await
                .expect("leader task should join")
                .expect_err("leader should return the injected failure"),
            "forced cache load failure"
        );
        assert_eq!(
            follower
                .await
                .expect("follower task should join")
                .expect_err("follower should receive the leader failure"),
            "forced cache load failure"
        );
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn value_cache_loads_different_keys_without_global_blocking() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();

        for (key, value) in [("key-a", 1_u64), ("key-b", 2_u64)] {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            tasks.push(tokio::spawn(async move {
                cache
                    .get_or_load::<(), _, _>(key.to_string(), Duration::from_secs(60), || {
                        let calls = Arc::clone(&calls);
                        let active = Arc::clone(&active);
                        let max_active = Arc::clone(&max_active);
                        async move {
                            calls.fetch_add(1, Ordering::AcqRel);
                            let current = active.fetch_add(1, Ordering::AcqRel) + 1;
                            record_max(&max_active, current);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            active.fetch_sub(1, Ordering::AcqRel);
                            Ok(Some(value))
                        }
                    })
                    .await
                    .unwrap()
            }));
        }

        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.unwrap());
        }
        results.sort_unstable();

        assert_eq!(results, vec![Some(1), Some(2)]);
        assert_eq!(calls.load(Ordering::Acquire), 2);
        assert_eq!(max_active.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn value_cache_legacy_stale_lookup_returns_without_invoking_loader() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let key = "hot-key".to_string();
        let calls = Arc::new(AtomicUsize::new(0));
        cache.insert(key.clone(), Some(1), Duration::from_millis(10));
        tokio::time::sleep(Duration::from_millis(20)).await;

        let first_cache = Arc::clone(&cache);
        let first_key = key.clone();
        let first_calls = Arc::clone(&calls);
        let first_started = Instant::now();
        let first = tokio::spawn(async move {
            first_cache
                .get_or_load_once_stale_while_refreshing::<(), _, _>(
                    first_key,
                    Duration::from_millis(10),
                    Duration::from_secs(1),
                    || async move {
                        first_calls.fetch_add(1, Ordering::AcqRel);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        Ok(Some(2))
                    },
                    CacheLoadObserver::default(),
                )
                .await
        });

        let follower_cache = Arc::clone(&cache);
        let follower_started = Instant::now();
        let follower_calls = Arc::clone(&calls);
        let follower = tokio::spawn(async move {
            follower_cache
                .get_or_load_once_stale_while_refreshing::<(), _, _>(
                    key,
                    Duration::from_millis(10),
                    Duration::from_secs(1),
                    || async move {
                        follower_calls.fetch_add(1, Ordering::AcqRel);
                        Ok(Some(3))
                    },
                    CacheLoadObserver::default(),
                )
                .await
        });

        assert_eq!(first.await.unwrap().unwrap(), Some(1));
        assert!(
            first_started.elapsed() < Duration::from_millis(80),
            "stale value should not wait for request-path refresh"
        );
        assert_eq!(follower.await.unwrap().unwrap(), Some(1));
        assert!(
            follower_started.elapsed() < Duration::from_millis(80),
            "follower should return stale value without waiting for refresh"
        );
        assert_eq!(calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn value_cache_stale_revalidation_returns_immediately_and_refreshes_in_background() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let key = "revalidate-key".to_string();
        let ttl = Duration::from_millis(5);
        let stale_ttl = Duration::from_secs(1);
        let cold_calls = Arc::new(AtomicUsize::new(0));
        let refresh_started = Arc::new(tokio::sync::Notify::new());
        let refresh_release = Arc::new(tokio::sync::Notify::new());

        cache.insert(key.clone(), Some(1), stale_ttl);
        tokio::time::sleep(Duration::from_millis(15)).await;

        let cold_calls_for_load = Arc::clone(&cold_calls);
        let refresh_started_for_load = Arc::clone(&refresh_started);
        let refresh_release_for_load = Arc::clone(&refresh_release);
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            cache.get_or_load_once_stale_while_revalidating::<(), _, _, _, _>(
                key.clone(),
                ttl,
                stale_ttl,
                || async move {
                    cold_calls_for_load.fetch_add(1, Ordering::AcqRel);
                    Ok(Some(99))
                },
                move || async move {
                    refresh_started_for_load.notify_one();
                    refresh_release_for_load.notified().await;
                    Ok(Some(2))
                },
                CacheLoadObserver::default(),
            ),
        )
        .await
        .expect("stale request should not wait for background refresh")
        .expect("stale lookup should succeed");

        assert_eq!(result, Some(1));
        assert_eq!(cold_calls.load(Ordering::Acquire), 0);
        tokio::time::timeout(Duration::from_secs(1), refresh_started.notified())
            .await
            .expect("background refresh should start");

        refresh_release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if cache.get(&key, stale_ttl) == Some(Some(2)) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background refresh should publish the new value");
    }

    #[tokio::test]
    async fn value_cache_fresh_hit_does_not_build_background_refresh_future() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let key = "fresh-lazy-refresh-key".to_string();
        let refresh_factory_calls = AtomicUsize::new(0);
        cache.insert(key.clone(), Some(1), Duration::from_secs(60));

        let result = cache
            .get_or_load_once_stale_while_revalidating::<(), _, _, _, _>(
                key,
                Duration::from_secs(30),
                Duration::from_secs(60),
                || async { panic!("cold loader must not run for a fresh hit") },
                || {
                    refresh_factory_calls.fetch_add(1, Ordering::AcqRel);
                    async { Ok(Some(2)) }
                },
                CacheLoadObserver::default(),
            )
            .await
            .expect("fresh lookup should succeed");

        assert_eq!(result, Some(1));
        assert_eq!(refresh_factory_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn value_cache_stale_revalidation_starts_only_one_refresh_per_key() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let key = "single-refresh-key".to_string();
        let ttl = Duration::from_millis(5);
        let stale_ttl = Duration::from_secs(1);
        let refresh_calls = Arc::new(AtomicUsize::new(0));
        let refresh_started = Arc::new(tokio::sync::Notify::new());
        let refresh_release = Arc::new(tokio::sync::Notify::new());

        cache.insert(key.clone(), Some(1), stale_ttl);
        tokio::time::sleep(Duration::from_millis(15)).await;

        let first_calls = Arc::clone(&refresh_calls);
        let first_started = Arc::clone(&refresh_started);
        let first_release = Arc::clone(&refresh_release);
        assert_eq!(
            cache
                .get_or_load_once_stale_while_revalidating::<(), _, _, _, _>(
                    key.clone(),
                    ttl,
                    stale_ttl,
                    || async { panic!("cold loader must not run for a stale hit") },
                    move || async move {
                        first_calls.fetch_add(1, Ordering::AcqRel);
                        first_started.notify_one();
                        first_release.notified().await;
                        Ok(Some(2))
                    },
                    CacheLoadObserver::default(),
                )
                .await
                .unwrap(),
            Some(1)
        );
        tokio::time::timeout(Duration::from_secs(1), refresh_started.notified())
            .await
            .expect("first refresh should start");

        for _ in 0..32 {
            let calls = Arc::clone(&refresh_calls);
            assert_eq!(
                cache
                    .get_or_load_once_stale_while_revalidating::<(), _, _, _, _>(
                        key.clone(),
                        ttl,
                        stale_ttl,
                        || async { panic!("cold loader must not run for a stale hit") },
                        move || async move {
                            calls.fetch_add(1, Ordering::AcqRel);
                            Ok(Some(3))
                        },
                        CacheLoadObserver::default(),
                    )
                    .await
                    .unwrap(),
                Some(1)
            );
        }
        assert_eq!(refresh_calls.load(Ordering::Acquire), 1);

        refresh_release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if cache.get(&key, stale_ttl) == Some(Some(2)) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the single refresh should publish");
    }

    #[tokio::test]
    async fn value_cache_clear_rejects_old_stale_revalidation_result() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let key = "revalidate-generation-key".to_string();
        let ttl = Duration::from_millis(5);
        let stale_ttl = Duration::from_secs(1);
        let refresh_started = Arc::new(tokio::sync::Notify::new());
        let refresh_release = Arc::new(tokio::sync::Notify::new());

        cache.insert(key.clone(), Some(1), stale_ttl);
        tokio::time::sleep(Duration::from_millis(15)).await;

        let started = Arc::clone(&refresh_started);
        let release = Arc::clone(&refresh_release);
        assert_eq!(
            cache
                .get_or_load_once_stale_while_revalidating::<(), _, _, _, _>(
                    key.clone(),
                    ttl,
                    stale_ttl,
                    || async { panic!("cold loader must not run for a stale hit") },
                    move || async move {
                        started.notify_one();
                        release.notified().await;
                        Ok(Some(2))
                    },
                    CacheLoadObserver::default(),
                )
                .await
                .unwrap(),
            Some(1)
        );
        tokio::time::timeout(Duration::from_secs(1), refresh_started.notified())
            .await
            .expect("refresh should start before clear");

        cache.clear();
        cache.insert(key.clone(), Some(3), stale_ttl);
        refresh_release.notify_one();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if Arc::strong_count(&cache.singleflight) == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("background refresh should finish");
        assert_eq!(cache.get(&key, stale_ttl), Some(Some(3)));
    }

    #[tokio::test]
    async fn value_cache_cold_stale_followers_do_not_reload_after_fresh_ttl() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let key = "cold-hot-key".to_string();
        let calls = Arc::new(AtomicUsize::new(0));

        let leader_cache = Arc::clone(&cache);
        let leader_key = key.clone();
        let leader_calls = Arc::clone(&calls);
        let leader = tokio::spawn(async move {
            leader_cache
                .get_or_load_once_stale_while_refreshing::<(), _, _>(
                    leader_key,
                    Duration::from_millis(10),
                    Duration::from_secs(1),
                    || async move {
                        leader_calls.fetch_add(1, Ordering::AcqRel);
                        Ok(Some(1))
                    },
                    CacheLoadObserver::default(),
                )
                .await
        });
        assert_eq!(leader.await.unwrap().unwrap(), Some(1));
        tokio::time::sleep(Duration::from_millis(25)).await;

        let follower_started = Instant::now();
        let follower_cache = Arc::clone(&cache);
        let follower_calls = Arc::clone(&calls);
        let follower = tokio::spawn(async move {
            follower_cache
                .get_or_load_once_stale_while_refreshing::<(), _, _>(
                    key,
                    Duration::from_millis(10),
                    Duration::from_secs(1),
                    || async move {
                        follower_calls.fetch_add(1, Ordering::AcqRel);
                        Ok(Some(2))
                    },
                    CacheLoadObserver::default(),
                )
                .await
        });

        assert_eq!(follower.await.unwrap().unwrap(), Some(1));
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert!(
            follower_started.elapsed() < Duration::from_millis(50),
            "follower should reuse cold-loaded stale value without reloading"
        );
    }

    #[tokio::test]
    async fn value_cache_clear_releases_same_key_followers() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let leader_cache = Arc::clone(&cache);
        let leader = tokio::spawn(async move {
            leader_cache
                .get_or_load::<(), _, _>(
                    "stuck-key".to_string(),
                    Duration::from_secs(60),
                    || async {
                        std::future::pending::<()>().await;
                        Ok(Some(1))
                    },
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        let follower_cache = Arc::clone(&cache);
        let follower = tokio::spawn(async move {
            follower_cache
                .get_or_load::<(), _, _>(
                    "stuck-key".to_string(),
                    Duration::from_secs(60),
                    || async { Ok(Some(2)) },
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        cache.clear();
        let value = tokio::time::timeout(Duration::from_millis(200), follower)
            .await
            .expect("clear should wake followers")
            .unwrap()
            .unwrap();
        assert_eq!(value, Some(2));

        leader.abort();
        let _ = leader.await;
    }

    #[tokio::test]
    async fn value_cache_direct_insert_rejects_older_inflight_publication() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let follower_loads = Arc::new(AtomicUsize::new(0));

        let leader_cache = Arc::clone(&cache);
        let leader_started = Arc::clone(&started);
        let leader_release = Arc::clone(&release);
        let leader = tokio::spawn(async move {
            leader_cache
                .get_or_load::<(), _, _>("insert-key".to_string(), Duration::from_secs(60), || {
                    let started = Arc::clone(&leader_started);
                    let release = Arc::clone(&leader_release);
                    async move {
                        started.notify_one();
                        release.notified().await;
                        Ok(Some(1))
                    }
                })
                .await
        });

        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("leader should start loading");

        let follower_cache = Arc::clone(&cache);
        let follower_loads_for_task = Arc::clone(&follower_loads);
        let follower = tokio::spawn(async move {
            follower_cache
                .get_or_load::<(), _, _>("insert-key".to_string(), Duration::from_secs(60), || {
                    let follower_loads = Arc::clone(&follower_loads_for_task);
                    async move {
                        follower_loads.fetch_add(1, Ordering::AcqRel);
                        Ok(Some(3))
                    }
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let follower_registered = cache
                    .singleflight
                    .inflight
                    .lock()
                    .ok()
                    .and_then(|inflight| inflight.values().next().cloned())
                    .is_some_and(|state| Arc::strong_count(&state) >= 4);
                if follower_registered {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("follower should register behind the old leader");

        cache.insert("insert-key".to_string(), Some(2), Duration::from_secs(60));
        assert_eq!(
            follower
                .await
                .expect("follower should join")
                .expect("follower should reuse the direct write"),
            Some(2)
        );
        assert_eq!(follower_loads.load(Ordering::Acquire), 0);

        release.notify_one();
        assert_eq!(leader.await.expect("leader should join").unwrap(), Some(1));
        assert_eq!(
            cache.get(&"insert-key".to_string(), Duration::from_secs(60)),
            Some(Some(2))
        );
    }

    #[tokio::test]
    async fn value_cache_clear_prevents_old_leader_from_reinserting_stale_value() {
        let cache = Arc::new(ValueCache::<String, u64>::default());
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());

        let leader_cache = Arc::clone(&cache);
        let leader_started = Arc::clone(&started);
        let leader_release = Arc::clone(&release);
        let leader = tokio::spawn(async move {
            leader_cache
                .get_or_load::<(), _, _>("epoch-key".to_string(), Duration::from_secs(60), || {
                    let started = Arc::clone(&leader_started);
                    let release = Arc::clone(&leader_release);
                    async move {
                        started.notify_one();
                        release.notified().await;
                        Ok(Some(1))
                    }
                })
                .await
        });

        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("leader should start loading");
        cache.clear();

        let fresh = cache
            .get_or_load::<(), _, _>("epoch-key".to_string(), Duration::from_secs(60), || async {
                Ok(Some(2))
            })
            .await
            .expect("fresh load should succeed");
        assert_eq!(fresh, Some(2));

        release.notify_one();
        assert_eq!(leader.await.expect("leader should join").unwrap(), Some(1));
        assert_eq!(
            cache.get(&"epoch-key".to_string(), Duration::from_secs(60)),
            Some(Some(2))
        );
    }
}
