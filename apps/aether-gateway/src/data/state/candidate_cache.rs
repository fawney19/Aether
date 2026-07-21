use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_cache::ExpiringMap;
use aether_data::DataLayerError;
use aether_data_contracts::repository::candidate_selection::{
    MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
    StoredPoolKeyCandidateOrder, StoredPoolKeyCandidateRowsByKeyIdsQuery,
    StoredPoolKeyCandidateRowsQuery, StoredRequestedModelCandidateRowsQuery,
};
use async_trait::async_trait;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;
use tracing::warn;

const CANDIDATE_SELECTION_CACHE_TTL: Duration = Duration::from_secs(5);
const CANDIDATE_SELECTION_CACHE_MAX_ENTRIES: usize = 4096;
const CANDIDATE_SELECTION_CACHE_MAX_INFLIGHT: usize = 4096;
#[cfg(not(test))]
const CANDIDATE_SELECTION_CACHE_LOAD_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const CANDIDATE_SELECTION_CACHE_LOAD_TIMEOUT: Duration = Duration::from_millis(50);
#[cfg(not(test))]
const CANDIDATE_SELECTION_CACHE_INFLIGHT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const CANDIDATE_SELECTION_CACHE_INFLIGHT_WAIT_TIMEOUT: Duration = Duration::from_millis(50);

pub(super) struct CachedMinimalCandidateSelectionReadRepository {
    inner: Arc<dyn MinimalCandidateSelectionReadRepository>,
    entries: ExpiringMap<CandidateSelectionCacheKey, Vec<StoredMinimalCandidateSelectionRow>>,
    inflight: Mutex<HashMap<CandidateSelectionCacheKey, Arc<InflightState>>>,
    epoch: AtomicU64,
    mutation: Mutex<()>,
    admission: Arc<Semaphore>,
}

impl CachedMinimalCandidateSelectionReadRepository {
    pub(super) fn new(inner: Arc<dyn MinimalCandidateSelectionReadRepository>) -> Self {
        Self {
            inner,
            entries: ExpiringMap::new(),
            inflight: Mutex::new(HashMap::new()),
            epoch: AtomicU64::new(0),
            mutation: Mutex::new(()),
            admission: Arc::new(Semaphore::new(CANDIDATE_SELECTION_CACHE_MAX_INFLIGHT)),
        }
    }

    async fn get_or_load<F, Fut>(
        &self,
        key: CandidateSelectionCacheKey,
        load: F,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>>,
    {
        if let Some(rows) = self.entries.get_fresh(&key, CANDIDATE_SELECTION_CACHE_TTL) {
            return Ok(rows);
        }

        self.load_after_cache_miss(key, load).await
    }

    async fn load_after_cache_miss<F, Fut>(
        &self,
        key: CandidateSelectionCacheKey,
        load: F,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>>,
    {
        loop {
            match self.register_inflight(&key) {
                InflightRegistration::Saturated => {
                    return Err(DataLayerError::TimedOut(format!(
                        "candidate selection cache admission saturated for {key:?}"
                    )));
                }
                InflightRegistration::Follower(state) => {
                    match timeout(
                        CANDIDATE_SELECTION_CACHE_INFLIGHT_WAIT_TIMEOUT,
                        state.wait(),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => return Err(error),
                        Err(_) => self.expire_inflight(&key, &state),
                    }
                    if let Some(rows) = self.entries.get_fresh(&key, CANDIDATE_SELECTION_CACHE_TTL)
                    {
                        return Ok(rows);
                    }
                    continue;
                }
                InflightRegistration::Leader(mut guard) => {
                    // A writer may have populated the cache after the first
                    // miss but before this flight was registered.
                    if let Some(rows) = self.entries.get_fresh(&key, CANDIDATE_SELECTION_CACHE_TTL)
                    {
                        return Ok(rows);
                    }

                    let result = load_candidate_selection_rows_with_timeout(&key, load()).await;
                    match &result {
                        Ok(rows) => guard.finish_loaded(rows.clone()),
                        Err(error) => guard.finish(Some(error.clone())),
                    }
                    return result;
                }
            }
        }
    }

    fn register_inflight(&self, key: &CandidateSelectionCacheKey) -> InflightRegistration<'_> {
        let mut inflight = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = inflight.get(key) {
            return InflightRegistration::Follower(Arc::clone(state));
        }
        if inflight.len() >= CANDIDATE_SELECTION_CACHE_MAX_INFLIGHT {
            return InflightRegistration::Saturated;
        }
        let Ok(admission) = Arc::clone(&self.admission).try_acquire_owned() else {
            return InflightRegistration::Saturated;
        };
        let state = Arc::new(InflightState {
            epoch: self.epoch.load(Ordering::Acquire),
            notify: Notify::new(),
            completed: AtomicBool::new(false),
            error: Mutex::new(None),
        });
        inflight.insert(key.clone(), Arc::clone(&state));
        InflightRegistration::Leader(InflightGuard::new(self, key.clone(), state, admission))
    }

    fn finish_inflight(
        &self,
        key: &CandidateSelectionCacheKey,
        state: &Arc<InflightState>,
        rows: Option<Vec<StoredMinimalCandidateSelectionRow>>,
        admission: Option<OwnedSemaphorePermit>,
    ) -> Option<Arc<InflightState>> {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut inflight = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(admission);
        if !inflight
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, state))
            || self.epoch.load(Ordering::Acquire) != state.epoch
        {
            return None;
        }
        if let Some(rows) = rows {
            self.entries.insert(
                key.clone(),
                rows,
                CANDIDATE_SELECTION_CACHE_TTL,
                CANDIDATE_SELECTION_CACHE_MAX_ENTRIES,
            );
        }
        let removed = inflight.remove(key);
        drop(inflight);
        drop(_mutation);
        removed
    }

    fn expire_inflight(&self, key: &CandidateSelectionCacheKey, state: &Arc<InflightState>) {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = {
            let mut inflight = self
                .inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if inflight
                .get(key)
                .is_some_and(|current| Arc::ptr_eq(current, state))
            {
                inflight.remove(key)
            } else {
                None
            }
        };
        drop(_mutation);
        if let Some(state) = removed {
            warn!(
                event_name = "candidate_selection_cache_inflight_expired",
                log_type = "ops",
                cache_key = ?key,
                wait_timeout_ms = CANDIDATE_SELECTION_CACHE_INFLIGHT_WAIT_TIMEOUT.as_millis() as u64,
                "gateway candidate selection cache expired stale inflight load"
            );
            state.complete(None);
        }
    }

    fn clear(&self) {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.entries.clear();
        let states = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|(_, state)| state)
            .collect::<Vec<_>>();
        if !states.is_empty() {
            warn!(
                event_name = "candidate_selection_cache_inflight_cleared",
                log_type = "ops",
                "gateway candidate selection cache cleared in-flight loads"
            );
            for state in states {
                state.complete(None);
            }
        }
    }
}

struct InflightState {
    epoch: u64,
    notify: Notify,
    completed: AtomicBool,
    error: Mutex<Option<DataLayerError>>,
}

impl InflightState {
    fn complete(&self, error: Option<DataLayerError>) {
        if let Some(error) = error {
            if let Ok(mut current) = self.error.lock() {
                *current = Some(error);
            }
        }
        if !self.completed.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    async fn wait(&self) -> Result<(), DataLayerError> {
        loop {
            if self.completed.load(Ordering::Acquire) {
                return self
                    .error
                    .lock()
                    .ok()
                    .and_then(|error| error.clone())
                    .map_or(Ok(()), Err);
            }

            // Register before checking completion a second time. Creating a
            // Notified future alone is insufficient because notify_waiters()
            // can otherwise run before the future's first poll.
            let mut notified = Box::pin(self.notify.notified());
            notified.as_mut().enable();
            if self.completed.load(Ordering::Acquire) {
                return self
                    .error
                    .lock()
                    .ok()
                    .and_then(|error| error.clone())
                    .map_or(Ok(()), Err);
            }
            notified.await;
        }
    }
}

enum InflightRegistration<'a> {
    Leader(InflightGuard<'a>),
    Follower(Arc<InflightState>),
    Saturated,
}

struct InflightGuard<'a> {
    cache: &'a CachedMinimalCandidateSelectionReadRepository,
    key: Option<CandidateSelectionCacheKey>,
    state: Arc<InflightState>,
    admission: Option<OwnedSemaphorePermit>,
}

impl<'a> InflightGuard<'a> {
    fn new(
        cache: &'a CachedMinimalCandidateSelectionReadRepository,
        key: CandidateSelectionCacheKey,
        state: Arc<InflightState>,
        admission: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            cache,
            key: Some(key),
            state,
            admission: Some(admission),
        }
    }

    fn epoch(&self) -> u64 {
        self.state.epoch
    }

    fn finish_loaded(&mut self, rows: Vec<StoredMinimalCandidateSelectionRow>) {
        let removed = self.key.take().and_then(|key| {
            self.cache
                .finish_inflight(&key, &self.state, Some(rows), self.admission.take())
        });
        self.admission.take();
        if let Some(removed) = removed {
            removed.complete(None);
        }
    }

    fn finish(&mut self, error: Option<DataLayerError>) {
        let removed = self.key.take().and_then(|key| {
            self.cache
                .finish_inflight(&key, &self.state, None, self.admission.take())
        });
        self.admission.take();
        if let Some(removed) = removed {
            removed.complete(error);
        }
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.finish(None);
    }
}

async fn load_candidate_selection_rows_with_timeout<Fut>(
    key: &CandidateSelectionCacheKey,
    load: Fut,
) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>
where
    Fut: Future<Output = Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>>,
{
    match timeout(CANDIDATE_SELECTION_CACHE_LOAD_TIMEOUT, load).await {
        Ok(result) => result,
        Err(_) => {
            warn!(
                event_name = "candidate_selection_cache_load_timeout",
                log_type = "ops",
                cache_key = ?key,
                timeout_ms = CANDIDATE_SELECTION_CACHE_LOAD_TIMEOUT.as_millis() as u64,
                "gateway candidate selection cache load timed out"
            );
            Err(DataLayerError::TimedOut(format!(
                "candidate selection cache load exceeded {}ms for {key:?}",
                CANDIDATE_SELECTION_CACHE_LOAD_TIMEOUT.as_millis()
            )))
        }
    }
}

#[async_trait]
impl MinimalCandidateSelectionReadRepository for CachedMinimalCandidateSelectionReadRepository {
    fn clear_local_cache(&self) {
        self.clear();
        self.inner.clear_local_cache();
    }

    async fn list_for_exact_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: normalize_api_format_key(api_format),
        };
        self.get_or_load(key, || self.inner.list_for_exact_api_format(api_format))
            .await
    }

    async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let key = CandidateSelectionCacheKey::ApiFormatAndGlobalModel {
            api_format: normalize_api_format_key(api_format),
            global_model_name: global_model_name.to_string(),
        };
        self.get_or_load(key, || {
            self.inner
                .list_for_exact_api_format_and_global_model(api_format, global_model_name)
        })
        .await
    }

    async fn list_for_exact_api_format_and_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let key = CandidateSelectionCacheKey::ApiFormatAndRequestedModel {
            api_format: normalize_api_format_key(api_format),
            requested_model_name: requested_model_name.to_string(),
        };
        self.get_or_load(key, || {
            self.inner
                .list_for_exact_api_format_and_requested_model(api_format, requested_model_name)
        })
        .await
    }

    async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let key = CandidateSelectionCacheKey::RequestedModelPage {
            api_format: normalize_api_format_key(&query.api_format),
            requested_model_name: query.requested_model_name.clone(),
            offset: query.offset,
            limit: query.limit,
        };
        self.get_or_load(key, || {
            self.inner
                .list_for_exact_api_format_and_requested_model_page(query)
        })
        .await
    }

    async fn list_pool_key_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let key = CandidateSelectionCacheKey::PoolKeyRowsForGroup {
            api_format: normalize_api_format_key(&query.api_format),
            provider_id: query.provider_id.clone(),
            endpoint_id: query.endpoint_id.clone(),
            model_id: query.model_id.clone(),
            selected_provider_model_name: query.selected_provider_model_name.clone(),
            order: CandidateSelectionPoolOrderKey::from(&query.order),
            offset: query.offset,
            limit: query.limit,
        };
        self.get_or_load(key, || self.inner.list_pool_key_rows_for_group(query))
            .await
    }

    async fn list_pool_key_rows_for_group_key_ids(
        &self,
        query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let key = CandidateSelectionCacheKey::PoolKeyRowsForGroupKeyIds {
            api_format: normalize_api_format_key(&query.api_format),
            provider_id: query.provider_id.clone(),
            endpoint_id: query.endpoint_id.clone(),
            model_id: query.model_id.clone(),
            selected_provider_model_name: query.selected_provider_model_name.clone(),
            key_ids: query.key_ids.clone(),
        };
        self.get_or_load(key, || {
            self.inner.list_pool_key_rows_for_group_key_ids(query)
        })
        .await
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CandidateSelectionCacheKey {
    ApiFormat {
        api_format: String,
    },
    ApiFormatAndGlobalModel {
        api_format: String,
        global_model_name: String,
    },
    ApiFormatAndRequestedModel {
        api_format: String,
        requested_model_name: String,
    },
    RequestedModelPage {
        api_format: String,
        requested_model_name: String,
        offset: u32,
        limit: u32,
    },
    PoolKeyRowsForGroup {
        api_format: String,
        provider_id: String,
        endpoint_id: String,
        model_id: String,
        selected_provider_model_name: String,
        order: CandidateSelectionPoolOrderKey,
        offset: u32,
        limit: u32,
    },
    PoolKeyRowsForGroupKeyIds {
        api_format: String,
        provider_id: String,
        endpoint_id: String,
        model_id: String,
        selected_provider_model_name: String,
        key_ids: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CandidateSelectionPoolOrderKey {
    InternalPriority,
    Lru,
    CacheAffinity,
    SingleAccount,
    LoadBalance { seed: String },
}

impl From<&StoredPoolKeyCandidateOrder> for CandidateSelectionPoolOrderKey {
    fn from(order: &StoredPoolKeyCandidateOrder) -> Self {
        match order {
            StoredPoolKeyCandidateOrder::InternalPriority => Self::InternalPriority,
            StoredPoolKeyCandidateOrder::Lru => Self::Lru,
            StoredPoolKeyCandidateOrder::CacheAffinity => Self::CacheAffinity,
            StoredPoolKeyCandidateOrder::SingleAccount => Self::SingleAccount,
            StoredPoolKeyCandidateOrder::LoadBalance { seed } => {
                Self::LoadBalance { seed: seed.clone() }
            }
        }
    }
}

fn normalize_api_format_key(api_format: &str) -> String {
    crate::ai_serving::normalize_api_format_alias(api_format.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::pending;
    use std::sync::atomic::AtomicUsize;

    struct StubCandidateSelectionRepository {
        calls: AtomicUsize,
        delay: Duration,
        rows: Vec<StoredMinimalCandidateSelectionRow>,
    }

    impl StubCandidateSelectionRepository {
        fn new(delay: Duration) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                delay,
                rows: Vec::new(),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        async fn load(&self) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(self.rows.clone())
        }
    }

    #[async_trait]
    impl MinimalCandidateSelectionReadRepository for StubCandidateSelectionRepository {
        async fn list_for_exact_api_format(
            &self,
            _api_format: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_for_exact_api_format_and_global_model(
            &self,
            _api_format: &str,
            _global_model_name: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_for_exact_api_format_and_requested_model(
            &self,
            _api_format: &str,
            _requested_model_name: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_for_exact_api_format_and_requested_model_page(
            &self,
            _query: &StoredRequestedModelCandidateRowsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_pool_key_rows_for_group(
            &self,
            _query: &StoredPoolKeyCandidateRowsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_pool_key_rows_for_group_key_ids(
            &self,
            _query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }
    }

    struct FirstLoadPendingThenFastRepository {
        calls: AtomicUsize,
    }

    impl FirstLoadPendingThenFastRepository {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        async fn load(&self) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                pending::<()>().await;
            }
            Ok(Vec::new())
        }
    }

    #[async_trait]
    impl MinimalCandidateSelectionReadRepository for FirstLoadPendingThenFastRepository {
        async fn list_for_exact_api_format(
            &self,
            _api_format: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_for_exact_api_format_and_global_model(
            &self,
            _api_format: &str,
            _global_model_name: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_for_exact_api_format_and_requested_model(
            &self,
            _api_format: &str,
            _requested_model_name: &str,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_for_exact_api_format_and_requested_model_page(
            &self,
            _query: &StoredRequestedModelCandidateRowsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_pool_key_rows_for_group(
            &self,
            _query: &StoredPoolKeyCandidateRowsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }

        async fn list_pool_key_rows_for_group_key_ids(
            &self,
            _query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
        ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
            self.load().await
        }
    }

    #[tokio::test]
    async fn candidate_selection_cache_coalesces_concurrent_loads() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(
            Duration::from_millis(25),
        ));
        let cache = Arc::new(CachedMinimalCandidateSelectionReadRepository::new(
            inner.clone(),
        ));
        let mut tasks = Vec::new();

        for _ in 0..16 {
            let cache = cache.clone();
            tasks.push(tokio::spawn(async move {
                cache.list_for_exact_api_format("openai").await.unwrap();
            }));
        }

        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(inner.calls(), 1);
    }

    #[tokio::test]
    async fn candidate_selection_cache_only_notifies_matching_inflight_key() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key_a = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let key_b = CandidateSelectionCacheKey::ApiFormat {
            api_format: "anthropic:messages".to_string(),
        };

        let mut leader_a = match cache.register_inflight(&key_a) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("first key registration should lead"),
        };
        let mut leader_b = match cache.register_inflight(&key_b) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("second key registration should lead independently"),
        };
        let state_a = match cache.register_inflight(&key_a) {
            InflightRegistration::Follower(state) => state,
            _ => panic!("duplicate key registration should follow"),
        };

        leader_b.finish(None);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), state_a.wait())
                .await
                .is_err(),
            "completing another key must not wake this follower"
        );
        leader_a.finish(None);
        tokio::time::timeout(Duration::from_millis(100), state_a.wait())
            .await
            .expect("completing the matching key must wake its follower")
            .expect("successful completion should not publish an error");
        assert!(cache.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn candidate_selection_cache_follower_observes_completion_before_first_poll() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };

        let mut leader = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let state = match cache.register_inflight(&key) {
            InflightRegistration::Follower(state) => state,
            _ => panic!("second registration should follow"),
        };

        // Complete before wait() is constructed or polled. notify_waiters()
        // alone would lose this notification and wait for the full timeout.
        leader.finish(None);
        tokio::time::timeout(Duration::from_millis(100), state.wait())
            .await
            .expect("completed follower must not miss the broadcast")
            .expect("successful completion should not publish an error");
    }

    #[tokio::test]
    async fn candidate_selection_cache_shares_leader_failure() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let mut leader = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let state = match cache.register_inflight(&key) {
            InflightRegistration::Follower(state) => state,
            _ => panic!("second registration should follow"),
        };

        leader.finish(Some(DataLayerError::Sql(
            "forced candidate cache load failure".to_string(),
        )));
        let error = tokio::time::timeout(Duration::from_millis(100), state.wait())
            .await
            .expect("failed load should release its follower")
            .expect_err("follower should observe the leader failure");
        assert_eq!(
            error.to_string(),
            "sql error: forced candidate cache load failure"
        );
        assert!(cache.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn candidate_selection_cache_broadcasts_to_all_same_key_followers() {
        const FOLLOWERS: usize = 64;

        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let mut leader = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };

        let mut tasks = Vec::with_capacity(FOLLOWERS);
        for _ in 0..FOLLOWERS {
            let state = match cache.register_inflight(&key) {
                InflightRegistration::Follower(state) => state,
                _ => panic!("same-key registration should follow"),
            };
            tasks.push(tokio::spawn(async move { state.wait().await }));
        }

        tokio::task::yield_now().await;
        leader.finish(None);
        for task in tasks {
            tokio::time::timeout(Duration::from_millis(250), task)
                .await
                .expect("all same-key followers should receive completion")
                .expect("follower task should finish")
                .expect("successful completion should not publish an error");
        }
    }

    #[tokio::test]
    async fn candidate_selection_cache_cancelled_guard_wakes_registered_follower() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let guard = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let state = match cache.register_inflight(&key) {
            InflightRegistration::Follower(state) => state,
            _ => panic!("second registration should follow"),
        };

        drop(guard);
        tokio::time::timeout(Duration::from_millis(100), state.wait())
            .await
            .expect("leader cancellation must wake an existing follower")
            .expect("leader cancellation should allow a retry");
        assert!(cache.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn candidate_selection_cache_clear_wakes_follower_and_old_guard_keeps_new_flight() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let old_guard = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let old_state = match cache.register_inflight(&key) {
            InflightRegistration::Follower(state) => state,
            _ => panic!("second registration should follow"),
        };
        let old_epoch = old_guard.epoch();

        cache.clear();
        assert!(cache.epoch.load(Ordering::Acquire) > old_epoch);
        let mut new_guard = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("registration after clear should lead"),
        };
        assert_ne!(old_guard.epoch(), new_guard.epoch());

        // Dropping the invalidated leader's RAII guard must not remove the
        // new flight created for the same key after clear().
        drop(old_guard);
        assert!(cache
            .inflight
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &new_guard.state)));
        tokio::time::timeout(Duration::from_millis(100), old_state.wait())
            .await
            .expect("clear must wake followers of the invalidated flight")
            .expect("clear should allow a retry");

        new_guard.finish(None);
        assert!(cache.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn candidate_selection_cache_clear_invalidates_entries() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner.clone());

        cache.list_for_exact_api_format("openai").await.unwrap();
        cache.list_for_exact_api_format("openai").await.unwrap();
        assert_eq!(inner.calls(), 1);

        cache.clear_local_cache();
        cache.list_for_exact_api_format("openai").await.unwrap();
        assert_eq!(inner.calls(), 2);
    }

    #[test]
    fn candidate_selection_cache_rejects_publication_from_pre_clear_flight() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let mut stale_leader = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };

        cache.clear();
        stale_leader.finish_loaded(Vec::new());

        assert!(cache
            .entries
            .get_fresh(&key, CANDIDATE_SELECTION_CACHE_TTL)
            .is_none());
    }

    #[test]
    fn candidate_selection_cache_clear_keeps_active_load_admission_bounded() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let mut cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        cache.admission = Arc::new(Semaphore::new(2));
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let mut detached_leaders = Vec::new();

        for _ in 0..2 {
            let leader = match cache.register_inflight(&key) {
                InflightRegistration::Leader(guard) => guard,
                _ => panic!("load below the hard active limit should lead"),
            };
            cache.clear();
            detached_leaders.push(leader);
        }

        assert_eq!(cache.admission.available_permits(), 0);
        assert!(matches!(
            cache.register_inflight(&key),
            InflightRegistration::Saturated
        ));
        drop(detached_leaders);
        assert_eq!(cache.admission.available_permits(), 2);
        assert!(matches!(
            cache.register_inflight(&key),
            InflightRegistration::Leader(_)
        ));
    }

    #[test]
    fn candidate_selection_cache_expired_leader_cannot_publish_over_replacement() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let mut old_leader = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        };
        let old_state = Arc::clone(&old_leader.state);
        cache.expire_inflight(&key, &old_state);
        let mut replacement = match cache.register_inflight(&key) {
            InflightRegistration::Leader(guard) => guard,
            _ => panic!("expiration should allow a replacement leader"),
        };
        assert_eq!(old_leader.epoch(), replacement.epoch());

        old_leader.finish_loaded(vec![sample_row("stale-key", 1)]);
        assert!(cache
            .entries
            .get_fresh(&key, CANDIDATE_SELECTION_CACHE_TTL)
            .is_none());

        replacement.finish_loaded(vec![sample_row("fresh-key", 2)]);
        let cached = cache
            .entries
            .get_fresh(&key, CANDIDATE_SELECTION_CACHE_TTL)
            .expect("replacement should publish");
        assert_eq!(cached[0].key_id, "fresh-key");
    }

    #[tokio::test]
    async fn candidate_selection_cache_rechecks_fresh_entry_after_leader_registration() {
        let inner = Arc::new(StubCandidateSelectionRepository::new(Duration::ZERO));
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner);
        let key = CandidateSelectionCacheKey::ApiFormat {
            api_format: "openai:chat".to_string(),
        };
        let expected = vec![sample_row("cached-key", 1)];
        cache.entries.insert(
            key.clone(),
            expected.clone(),
            CANDIDATE_SELECTION_CACHE_TTL,
            CANDIDATE_SELECTION_CACHE_MAX_ENTRIES,
        );
        let loads = AtomicUsize::new(0);

        // Exercise the post-initial-miss path directly to model a concurrent
        // writer filling the cache immediately before flight registration.
        let rows = cache
            .load_after_cache_miss(key, || async {
                loads.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            })
            .await
            .expect("fresh entry should satisfy the lookup");

        assert_eq!(rows, expected);
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert!(cache.inflight.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn candidate_selection_cache_releases_inflight_when_leader_is_cancelled() {
        let inner = Arc::new(FirstLoadPendingThenFastRepository::new());
        let cache = Arc::new(CachedMinimalCandidateSelectionReadRepository::new(
            inner.clone(),
        ));
        let leader_cache = cache.clone();
        let leader = tokio::spawn(async move {
            leader_cache
                .list_for_exact_api_format_and_requested_model_page(
                    &StoredRequestedModelCandidateRowsQuery {
                        api_format: "openai:chat".to_string(),
                        requested_model_name: "gpt-5.5".to_string(),
                        offset: 0,
                        limit: 64,
                    },
                )
                .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        leader.abort();
        let _ = leader.await;

        tokio::time::timeout(
            Duration::from_millis(200),
            cache.list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:chat".to_string(),
                    requested_model_name: "gpt-5.5".to_string(),
                    offset: 0,
                    limit: 64,
                },
            ),
        )
        .await
        .expect("cancelled leader must not leave a permanent inflight wait")
        .unwrap();
        assert_eq!(inner.calls(), 2);
    }

    #[tokio::test]
    async fn candidate_selection_cache_times_out_and_clears_stuck_load() {
        let inner = Arc::new(FirstLoadPendingThenFastRepository::new());
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner.clone());
        let err = cache
            .list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:chat".to_string(),
                    requested_model_name: "gpt-5.5".to_string(),
                    offset: 0,
                    limit: 64,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DataLayerError::TimedOut(_)));

        cache
            .list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:chat".to_string(),
                    requested_model_name: "gpt-5.5".to_string(),
                    offset: 0,
                    limit: 64,
                },
            )
            .await
            .unwrap();
        assert_eq!(inner.calls(), 2);
    }

    #[tokio::test]
    async fn candidate_selection_cache_clear_releases_inflight_waiters() {
        let inner = Arc::new(FirstLoadPendingThenFastRepository::new());
        let cache = Arc::new(CachedMinimalCandidateSelectionReadRepository::new(
            inner.clone(),
        ));
        let leader_cache = cache.clone();
        let leader = tokio::spawn(async move {
            leader_cache
                .list_for_exact_api_format_and_requested_model_page(
                    &StoredRequestedModelCandidateRowsQuery {
                        api_format: "openai:chat".to_string(),
                        requested_model_name: "gpt-5.5".to_string(),
                        offset: 0,
                        limit: 64,
                    },
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(10)).await;

        cache.clear_local_cache();
        tokio::time::timeout(
            Duration::from_millis(200),
            cache.list_for_exact_api_format_and_requested_model_page(
                &StoredRequestedModelCandidateRowsQuery {
                    api_format: "openai:chat".to_string(),
                    requested_model_name: "gpt-5.5".to_string(),
                    offset: 0,
                    limit: 64,
                },
            ),
        )
        .await
        .expect("cache clear must release stale inflight waiters")
        .unwrap();
        assert_eq!(inner.calls(), 2);
        leader.abort();
        let _ = leader.await;
    }

    #[tokio::test]
    async fn candidate_selection_load_balance_cache_keeps_seed_specific_entries() {
        let inner = Arc::new(StubCandidateSelectionRepository {
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
            rows: vec![
                sample_row("key-a", 1),
                sample_row("key-b", 2),
                sample_row("key-c", 3),
            ],
        });
        let cache = CachedMinimalCandidateSelectionReadRepository::new(inner.clone());
        let query = |seed: &str| StoredPoolKeyCandidateRowsQuery {
            api_format: "openai:chat".to_string(),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            model_id: "model-1".to_string(),
            selected_provider_model_name: "mock-model".to_string(),
            order: StoredPoolKeyCandidateOrder::LoadBalance {
                seed: seed.to_string(),
            },
            offset: 0,
            limit: 2,
        };

        let first = cache
            .list_pool_key_rows_for_group(&query("seed-a"))
            .await
            .unwrap();
        let second = cache
            .list_pool_key_rows_for_group(&query("seed-b"))
            .await
            .unwrap();

        assert_eq!(inner.calls(), 2);
        assert_eq!(first.len(), 3);
        assert_eq!(second.len(), 3);

        let third = cache
            .list_pool_key_rows_for_group(&query("seed-a"))
            .await
            .unwrap();
        assert_eq!(inner.calls(), 2);
        assert_eq!(third.len(), 3);
    }

    fn sample_row(key_id: &str, key_internal_priority: i32) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-1".to_string(),
            provider_name: "provider".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 1,
            provider_is_active: true,
            endpoint_id: "endpoint-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority,
            key_global_priority_by_format: None,
            model_id: "model-1".to_string(),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "mock-model".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "mock-model".to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }
}
