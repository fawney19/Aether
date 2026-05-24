use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
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
use tokio::sync::Notify;
use tokio::time::timeout;
use tracing::warn;

const CANDIDATE_SELECTION_CACHE_TTL: Duration = Duration::from_secs(5);
const CANDIDATE_SELECTION_CACHE_MAX_ENTRIES: usize = 4096;
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
    inflight: Mutex<HashMap<CandidateSelectionCacheKey, u64>>,
    inflight_notify: Notify,
    next_inflight_token: AtomicU64,
    epoch: AtomicU64,
}

impl CachedMinimalCandidateSelectionReadRepository {
    pub(super) fn new(inner: Arc<dyn MinimalCandidateSelectionReadRepository>) -> Self {
        Self {
            inner,
            entries: ExpiringMap::new(),
            inflight: Mutex::new(HashMap::new()),
            inflight_notify: Notify::new(),
            next_inflight_token: AtomicU64::new(1),
            epoch: AtomicU64::new(0),
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

        loop {
            let notified = self.inflight_notify.notified();
            match self.register_inflight(&key) {
                InflightRegistration::Bypass => {
                    return load_candidate_selection_rows_with_timeout(&key, load()).await;
                }
                InflightRegistration::Follower => {
                    if timeout(CANDIDATE_SELECTION_CACHE_INFLIGHT_WAIT_TIMEOUT, notified)
                        .await
                        .is_err()
                    {
                        self.expire_inflight(&key);
                    }
                    if let Some(rows) = self.entries.get_fresh(&key, CANDIDATE_SELECTION_CACHE_TTL)
                    {
                        return Ok(rows);
                    }
                    continue;
                }
                InflightRegistration::Leader(token) => {
                    let mut guard = InflightGuard::new(self, key.clone(), token);
                    let load_epoch = self.epoch.load(Ordering::Acquire);
                    let result = load_candidate_selection_rows_with_timeout(&key, load()).await;
                    if let Ok(rows) = &result {
                        if load_epoch == self.epoch.load(Ordering::Acquire) {
                            self.entries.insert(
                                key.clone(),
                                rows.clone(),
                                CANDIDATE_SELECTION_CACHE_TTL,
                                CANDIDATE_SELECTION_CACHE_MAX_ENTRIES,
                            );
                        }
                    }
                    guard.finish();
                    return result;
                }
            }
        }
    }

    fn register_inflight(&self, key: &CandidateSelectionCacheKey) -> InflightRegistration {
        match self.inflight.lock() {
            Ok(mut inflight) => {
                if inflight.contains_key(key) {
                    return InflightRegistration::Follower;
                }
                let token = self.next_inflight_token.fetch_add(1, Ordering::AcqRel);
                inflight.insert(key.clone(), token);
                InflightRegistration::Leader(token)
            }
            Err(_) => InflightRegistration::Bypass,
        }
    }

    fn finish_inflight(&self, key: &CandidateSelectionCacheKey, token: u64) {
        let mut removed = false;
        if let Ok(mut inflight) = self.inflight.lock() {
            if inflight.get(key).copied() == Some(token) {
                inflight.remove(key);
                removed = true;
            }
        }
        if removed {
            self.inflight_notify.notify_waiters();
        }
    }

    fn expire_inflight(&self, key: &CandidateSelectionCacheKey) {
        let mut removed = false;
        if let Ok(mut inflight) = self.inflight.lock() {
            removed = inflight.remove(key).is_some();
        }
        if removed {
            warn!(
                event_name = "candidate_selection_cache_inflight_expired",
                log_type = "ops",
                cache_key = ?key,
                wait_timeout_ms = CANDIDATE_SELECTION_CACHE_INFLIGHT_WAIT_TIMEOUT.as_millis() as u64,
                "gateway candidate selection cache expired stale inflight load"
            );
            self.inflight_notify.notify_waiters();
        }
    }

    fn clear(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.entries.clear();
        let mut cleared_inflight = false;
        if let Ok(mut inflight) = self.inflight.lock() {
            cleared_inflight = !inflight.is_empty();
            inflight.clear();
        }
        if cleared_inflight {
            warn!(
                event_name = "candidate_selection_cache_inflight_cleared",
                log_type = "ops",
                "gateway candidate selection cache cleared in-flight loads"
            );
            self.inflight_notify.notify_waiters();
        }
    }
}

enum InflightRegistration {
    Leader(u64),
    Follower,
    Bypass,
}

struct InflightGuard<'a> {
    cache: &'a CachedMinimalCandidateSelectionReadRepository,
    key: Option<CandidateSelectionCacheKey>,
    token: u64,
}

impl<'a> InflightGuard<'a> {
    fn new(
        cache: &'a CachedMinimalCandidateSelectionReadRepository,
        key: CandidateSelectionCacheKey,
        token: u64,
    ) -> Self {
        Self {
            cache,
            key: Some(key),
            token,
        }
    }

    fn finish(&mut self) {
        if let Some(key) = self.key.take() {
            self.cache.finish_inflight(&key, self.token);
        }
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.finish();
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
    }

    impl StubCandidateSelectionRepository {
        fn new(delay: Duration) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                delay,
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
            Ok(Vec::new())
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
}
