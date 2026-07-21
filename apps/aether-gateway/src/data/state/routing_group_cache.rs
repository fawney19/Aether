use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use aether_cache::ExpiringMap;
use aether_data::DataLayerError;
use aether_data_contracts::repository::routing_profiles::{
    RoutingGroupBindingQuery, RoutingGroupBindingSubject, RoutingGroupLookupKey,
    RoutingGroupReadRepository, StoredRoutingGroup, StoredRoutingGroupBinding,
    StoredRoutingGroupVersion,
};
use async_trait::async_trait;
use tokio::sync::Notify;

// Routing profile writes clear this local cache. Keep the fallback TTL bounded
// so a missed cross-node invalidation cannot route traffic stale for minutes.
const ROUTING_GROUP_CACHE_STALE_TTL: Duration = Duration::from_secs(60);
const ROUTING_GROUP_CACHE_MAX_ENTRIES: usize = 4_096;
const ROUTING_GROUP_CACHE_MAX_INFLIGHT: usize = 4_096;
const ROUTING_GROUP_CACHE_LOAD_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct CachedRoutingGroupReadRepository {
    inner: Arc<dyn RoutingGroupReadRepository>,
    entries: ExpiringMap<RoutingGroupCacheKey, RoutingGroupCacheValue>,
    inflight: Mutex<HashMap<RoutingGroupCacheKey, Arc<RoutingGroupInflightState>>>,
    admission: Arc<tokio::sync::Semaphore>,
    generation: AtomicU64,
    mutation: Mutex<()>,
}

impl CachedRoutingGroupReadRepository {
    pub(super) fn new(inner: Arc<dyn RoutingGroupReadRepository>) -> Self {
        Self {
            inner,
            entries: ExpiringMap::new(),
            inflight: Mutex::new(HashMap::new()),
            admission: Arc::new(tokio::sync::Semaphore::new(
                ROUTING_GROUP_CACHE_MAX_INFLIGHT,
            )),
            generation: AtomicU64::new(0),
            mutation: Mutex::new(()),
        }
    }

    fn clear(&self) {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.entries.clear();
        let states = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|(_, state)| {
                let _ = state
                    .completion
                    .set(RoutingGroupInflightCompletion::Invalidated);
                state
            })
            .collect::<Vec<_>>();
        drop(_mutation);
        for state in states {
            state.notify.notify_waiters();
        }
    }

    async fn get_or_load<F, Fut>(
        &self,
        key: RoutingGroupCacheKey,
        mut load: F,
    ) -> Result<RoutingGroupCacheValue, DataLayerError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<RoutingGroupCacheValue, DataLayerError>>,
    {
        loop {
            if let Some(value) = self.cached_value(&key) {
                return Ok(value);
            }

            match self.register_inflight(&key) {
                RoutingGroupInflightRegistration::Saturated => {
                    return Err(DataLayerError::TimedOut(format!(
                        "routing group cache admission saturated for {key:?}"
                    )));
                }
                RoutingGroupInflightRegistration::Leader(mut guard) => {
                    // A previous leader can publish between the optimistic
                    // cache read and this registration.
                    if let Some(value) = self.cached_value(&key) {
                        guard.finish(RoutingGroupInflightCompletion::Loaded(value.clone()));
                        return Ok(value);
                    }

                    let result = match tokio::time::timeout(
                        ROUTING_GROUP_CACHE_LOAD_TIMEOUT,
                        load(),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(DataLayerError::TimedOut(format!(
                            "routing group cache load exceeded {}ms for {key:?}",
                            ROUTING_GROUP_CACHE_LOAD_TIMEOUT.as_millis()
                        ))),
                    };
                    match result {
                        Ok(value) => {
                            self.insert_if_generation(key.clone(), value.clone(), guard.generation);
                            guard.finish(RoutingGroupInflightCompletion::Loaded(value.clone()));
                            return Ok(value);
                        }
                        Err(error) => {
                            guard.finish(RoutingGroupInflightCompletion::Failed(
                                SharedDataLayerError::from(&error),
                            ));
                            return Err(error);
                        }
                    }
                }
                RoutingGroupInflightRegistration::Follower(state) => {
                    state.wait().await;
                    match self.follower_completion(&state) {
                        Some(RoutingGroupInflightCompletion::Loaded(value)) => return Ok(value),
                        Some(RoutingGroupInflightCompletion::Failed(error)) => {
                            return Err(error.into_data_layer_error());
                        }
                        Some(
                            RoutingGroupInflightCompletion::Cancelled
                            | RoutingGroupInflightCompletion::Invalidated,
                        )
                        | None => continue,
                    }
                }
            }
        }
    }

    fn cached_value(&self, key: &RoutingGroupCacheKey) -> Option<RoutingGroupCacheValue> {
        self.entries
            .get_with_age(key, ROUTING_GROUP_CACHE_STALE_TTL)
            .map(|(value, _age)| value)
    }

    fn follower_completion(
        &self,
        state: &RoutingGroupInflightState,
    ) -> Option<RoutingGroupInflightCompletion> {
        // Double-check the generation around the completion read. This keeps
        // the hot follower path lock-free while ensuring clear() cannot race
        // between an old-generation check and returning a loaded value.
        let before = self.generation.load(Ordering::Acquire);
        let completion = state.completion.get().cloned();
        let after = self.generation.load(Ordering::Acquire);
        (before == state.generation && before == after)
            .then_some(completion)
            .flatten()
    }

    fn insert_if_generation(
        &self,
        key: RoutingGroupCacheKey,
        value: RoutingGroupCacheValue,
        generation: u64,
    ) {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        self.entries.insert(
            key,
            value,
            ROUTING_GROUP_CACHE_STALE_TTL,
            ROUTING_GROUP_CACHE_MAX_ENTRIES,
        );
    }

    fn register_inflight(
        &self,
        key: &RoutingGroupCacheKey,
    ) -> RoutingGroupInflightRegistration<'_> {
        // Existing followers only touch the per-key map. Avoid taking the
        // mutation lock on the 20k-request hot path; that lock is reserved
        // for leader insertion and invalidation ordering.
        {
            let inflight = self
                .inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(state) = inflight.get(key) {
                return RoutingGroupInflightRegistration::Follower(Arc::clone(state));
            }
        }

        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut inflight = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = inflight.get(key) {
            return RoutingGroupInflightRegistration::Follower(Arc::clone(state));
        }
        if inflight.len() >= ROUTING_GROUP_CACHE_MAX_INFLIGHT {
            return RoutingGroupInflightRegistration::Saturated;
        }
        let Ok(admission) = Arc::clone(&self.admission).try_acquire_owned() else {
            return RoutingGroupInflightRegistration::Saturated;
        };

        let state = Arc::new(RoutingGroupInflightState {
            notify: Arc::new(Notify::new()),
            completion: OnceLock::new(),
            generation: self.generation.load(Ordering::Acquire),
        });
        let generation = state.generation;
        inflight.insert(key.clone(), Arc::clone(&state));
        RoutingGroupInflightRegistration::Leader(RoutingGroupInflightGuard {
            cache: self,
            key: Some(key.clone()),
            state,
            generation,
            admission: Some(admission),
        })
    }

    fn finish_inflight(
        &self,
        key: &RoutingGroupCacheKey,
        state: &Arc<RoutingGroupInflightState>,
        admission: tokio::sync::OwnedSemaphorePermit,
        completion: RoutingGroupInflightCompletion,
    ) {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = {
            let mut inflight = self
                .inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            drop(admission);
            debug_assert!(self.admission.available_permits() > 0);
            if inflight
                .get(key)
                .is_some_and(|current| Arc::ptr_eq(current, state))
            {
                let _ = state.completion.set(completion);
                inflight.remove(key);
                true
            } else {
                false
            }
        };
        drop(_mutation);
        if removed {
            state.notify.notify_waiters();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RoutingGroupCacheKey {
    ListGroups,
    HasAnyBinding,
    FindById(String),
    FindByName(String),
    FindSystemDefault,
    Bindings {
        group_id: Option<String>,
        subject_type: Option<&'static str>,
        subject_id: Option<String>,
    },
    Versions(String),
}

#[derive(Debug, Clone)]
enum RoutingGroupCacheValue {
    Groups(Vec<StoredRoutingGroup>),
    Bool(bool),
    Group(Option<StoredRoutingGroup>),
    Bindings(Vec<StoredRoutingGroupBinding>),
    Versions(Vec<StoredRoutingGroupVersion>),
}

struct RoutingGroupInflightState {
    notify: Arc<Notify>,
    completion: OnceLock<RoutingGroupInflightCompletion>,
    generation: u64,
}

impl RoutingGroupInflightState {
    async fn wait(&self) {
        loop {
            if self.completion.get().is_some() {
                return;
            }

            // Register before checking completion a second time. This closes
            // the completion/notify race even if the follower is not polled
            // until after the leader has broadcast with notify_waiters().
            let mut notified = Box::pin(self.notify.notified());
            notified.as_mut().enable();
            if self.completion.get().is_some() {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Clone)]
enum RoutingGroupInflightCompletion {
    Loaded(RoutingGroupCacheValue),
    Failed(SharedDataLayerError),
    Cancelled,
    Invalidated,
}

enum RoutingGroupInflightRegistration<'a> {
    Leader(RoutingGroupInflightGuard<'a>),
    Follower(Arc<RoutingGroupInflightState>),
    Saturated,
}

struct RoutingGroupInflightGuard<'a> {
    cache: &'a CachedRoutingGroupReadRepository,
    key: Option<RoutingGroupCacheKey>,
    state: Arc<RoutingGroupInflightState>,
    generation: u64,
    admission: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl RoutingGroupInflightGuard<'_> {
    fn finish(&mut self, completion: RoutingGroupInflightCompletion) {
        if let Some(key) = self.key.take() {
            let admission = self
                .admission
                .take()
                .expect("active routing group leader must own admission");
            self.cache
                .finish_inflight(&key, &self.state, admission, completion);
        }
    }
}

impl Drop for RoutingGroupInflightGuard<'_> {
    fn drop(&mut self) {
        self.finish(RoutingGroupInflightCompletion::Cancelled);
    }
}

#[derive(Clone)]
enum SharedDataLayerError {
    InvalidConfiguration(String),
    InvalidInput(String),
    Postgres(String),
    Redis(String),
    Sql(String),
    TimedOut(String),
    UnexpectedValue(String),
}

impl From<&DataLayerError> for SharedDataLayerError {
    fn from(error: &DataLayerError) -> Self {
        match error {
            DataLayerError::InvalidConfiguration(message) => {
                Self::InvalidConfiguration(message.clone())
            }
            DataLayerError::InvalidInput(message) => Self::InvalidInput(message.clone()),
            DataLayerError::Postgres(message) => Self::Postgres(message.clone()),
            DataLayerError::Redis(message) => Self::Redis(message.clone()),
            DataLayerError::Sql(message) => Self::Sql(message.clone()),
            DataLayerError::TimedOut(message) => Self::TimedOut(message.clone()),
            DataLayerError::UnexpectedValue(message) => Self::UnexpectedValue(message.clone()),
        }
    }
}

impl SharedDataLayerError {
    fn into_data_layer_error(self) -> DataLayerError {
        match self {
            Self::InvalidConfiguration(message) => DataLayerError::InvalidConfiguration(message),
            Self::InvalidInput(message) => DataLayerError::InvalidInput(message),
            Self::Postgres(message) => DataLayerError::Postgres(message),
            Self::Redis(message) => DataLayerError::Redis(message),
            Self::Sql(message) => DataLayerError::Sql(message),
            Self::TimedOut(message) => DataLayerError::TimedOut(message),
            Self::UnexpectedValue(message) => DataLayerError::UnexpectedValue(message),
        }
    }
}

fn lookup_cache_key(lookup: &RoutingGroupLookupKey<'_>) -> RoutingGroupCacheKey {
    match lookup {
        RoutingGroupLookupKey::Id(id) => RoutingGroupCacheKey::FindById((*id).to_string()),
        RoutingGroupLookupKey::Name(name) => RoutingGroupCacheKey::FindByName((*name).to_string()),
        RoutingGroupLookupKey::SystemDefault => RoutingGroupCacheKey::FindSystemDefault,
    }
}

fn subject_cache_key(subject: Option<RoutingGroupBindingSubject>) -> Option<&'static str> {
    match subject {
        Some(RoutingGroupBindingSubject::User) => Some("user"),
        Some(RoutingGroupBindingSubject::ApiKey) => Some("api_key"),
        Some(RoutingGroupBindingSubject::UserGroup) => Some("user_group"),
        None => None,
    }
}

#[async_trait]
impl RoutingGroupReadRepository for CachedRoutingGroupReadRepository {
    fn clear_local_cache(&self) {
        self.clear();
    }

    async fn list_routing_groups(&self) -> Result<Vec<StoredRoutingGroup>, DataLayerError> {
        match self
            .get_or_load(RoutingGroupCacheKey::ListGroups, || async {
                self.inner
                    .list_routing_groups()
                    .await
                    .map(RoutingGroupCacheValue::Groups)
            })
            .await?
        {
            RoutingGroupCacheValue::Groups(groups) => Ok(groups),
            _ => Ok(Vec::new()),
        }
    }

    async fn find_routing_group(
        &self,
        lookup: RoutingGroupLookupKey<'_>,
    ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
        let key = lookup_cache_key(&lookup);
        let lookup_for_load = lookup.clone();
        match self
            .get_or_load(key, move || {
                let lookup = lookup_for_load.clone();
                async move {
                    self.inner
                        .find_routing_group(lookup)
                        .await
                        .map(RoutingGroupCacheValue::Group)
                }
            })
            .await?
        {
            RoutingGroupCacheValue::Group(group) => Ok(group),
            _ => Ok(None),
        }
    }

    async fn list_routing_group_bindings(
        &self,
        query: &RoutingGroupBindingQuery,
    ) -> Result<Vec<StoredRoutingGroupBinding>, DataLayerError> {
        let key = RoutingGroupCacheKey::Bindings {
            group_id: query.group_id.clone(),
            subject_type: subject_cache_key(query.subject_type),
            subject_id: query.subject_id.clone(),
        };
        match self
            .get_or_load(key, || async {
                self.inner
                    .list_routing_group_bindings(query)
                    .await
                    .map(RoutingGroupCacheValue::Bindings)
            })
            .await?
        {
            RoutingGroupCacheValue::Bindings(bindings) => Ok(bindings),
            _ => Ok(Vec::new()),
        }
    }

    async fn has_any_routing_group_binding(&self) -> Result<bool, DataLayerError> {
        match self
            .get_or_load(RoutingGroupCacheKey::HasAnyBinding, || async {
                self.inner
                    .has_any_routing_group_binding()
                    .await
                    .map(RoutingGroupCacheValue::Bool)
            })
            .await?
        {
            RoutingGroupCacheValue::Bool(value) => Ok(value),
            _ => Ok(false),
        }
    }

    async fn list_routing_group_versions(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredRoutingGroupVersion>, DataLayerError> {
        let key = RoutingGroupCacheKey::Versions(group_id.to_string());
        match self
            .get_or_load(key, || async {
                self.inner
                    .list_routing_group_versions(group_id)
                    .await
                    .map(RoutingGroupCacheValue::Versions)
            })
            .await?
        {
            RoutingGroupCacheValue::Versions(versions) => Ok(versions),
            _ => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::{Barrier, Semaphore};

    use super::*;

    #[derive(Default)]
    struct CountingRoutingGroupReadRepository {
        list_calls: AtomicUsize,
        has_any_binding_calls: AtomicUsize,
    }

    #[async_trait]
    impl RoutingGroupReadRepository for CountingRoutingGroupReadRepository {
        async fn list_routing_groups(&self) -> Result<Vec<StoredRoutingGroup>, DataLayerError> {
            self.list_calls.fetch_add(1, Ordering::AcqRel);
            Ok(Vec::new())
        }

        async fn find_routing_group(
            &self,
            _lookup: RoutingGroupLookupKey<'_>,
        ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
            Ok(None)
        }

        async fn list_routing_group_bindings(
            &self,
            _query: &RoutingGroupBindingQuery,
        ) -> Result<Vec<StoredRoutingGroupBinding>, DataLayerError> {
            Ok(Vec::new())
        }

        async fn has_any_routing_group_binding(&self) -> Result<bool, DataLayerError> {
            self.has_any_binding_calls.fetch_add(1, Ordering::AcqRel);
            Ok(false)
        }

        async fn list_routing_group_versions(
            &self,
            _group_id: &str,
        ) -> Result<Vec<StoredRoutingGroupVersion>, DataLayerError> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone, Copy)]
    enum ControlledHasAnyBindingBehavior {
        WaitThenSuccess(bool),
        WaitThenError,
        FirstWaitFalseThenTrue,
        FirstPendingThenTrue,
    }

    struct ControlledRoutingGroupReadRepository {
        behavior: ControlledHasAnyBindingBehavior,
        has_any_binding_calls: AtomicUsize,
        started: Semaphore,
        release: Semaphore,
    }

    impl ControlledRoutingGroupReadRepository {
        fn new(behavior: ControlledHasAnyBindingBehavior) -> Self {
            Self {
                behavior,
                has_any_binding_calls: AtomicUsize::new(0),
                started: Semaphore::new(0),
                release: Semaphore::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.has_any_binding_calls.load(Ordering::Acquire)
        }

        async fn wait_until_started(&self) {
            self.started
                .acquire()
                .await
                .expect("started semaphore should remain open")
                .forget();
        }

        async fn wait_for_release(&self) {
            self.release
                .acquire()
                .await
                .expect("release semaphore should remain open")
                .forget();
        }
    }

    #[async_trait]
    impl RoutingGroupReadRepository for ControlledRoutingGroupReadRepository {
        async fn list_routing_groups(&self) -> Result<Vec<StoredRoutingGroup>, DataLayerError> {
            Ok(Vec::new())
        }

        async fn find_routing_group(
            &self,
            _lookup: RoutingGroupLookupKey<'_>,
        ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
            Ok(None)
        }

        async fn list_routing_group_bindings(
            &self,
            _query: &RoutingGroupBindingQuery,
        ) -> Result<Vec<StoredRoutingGroupBinding>, DataLayerError> {
            Ok(Vec::new())
        }

        async fn has_any_routing_group_binding(&self) -> Result<bool, DataLayerError> {
            let call = self.has_any_binding_calls.fetch_add(1, Ordering::AcqRel) + 1;
            self.started.add_permits(1);
            match self.behavior {
                ControlledHasAnyBindingBehavior::WaitThenSuccess(value) => {
                    self.wait_for_release().await;
                    Ok(value)
                }
                ControlledHasAnyBindingBehavior::WaitThenError => {
                    self.wait_for_release().await;
                    Err(DataLayerError::TimedOut(
                        "shared routing load error".to_string(),
                    ))
                }
                ControlledHasAnyBindingBehavior::FirstWaitFalseThenTrue if call == 1 => {
                    self.wait_for_release().await;
                    Ok(false)
                }
                ControlledHasAnyBindingBehavior::FirstPendingThenTrue if call == 1 => {
                    std::future::pending().await
                }
                ControlledHasAnyBindingBehavior::FirstWaitFalseThenTrue
                | ControlledHasAnyBindingBehavior::FirstPendingThenTrue => Ok(true),
            }
        }

        async fn list_routing_group_versions(
            &self,
            _group_id: &str,
        ) -> Result<Vec<StoredRoutingGroupVersion>, DataLayerError> {
            Ok(Vec::new())
        }
    }

    async fn wait_for_same_key_inflight_participants(
        repository: &CachedRoutingGroupReadRepository,
        participants: usize,
    ) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let participant_count = repository
                    .inflight
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&RoutingGroupCacheKey::HasAnyBinding)
                    .map(Arc::strong_count)
                    .unwrap_or_default();
                // One Arc is retained by the map and every active request
                // owns one through its leader guard or follower state.
                if participant_count >= participants + 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all same-key requests should join one inflight load");
    }

    #[tokio::test]
    async fn clear_local_cache_forces_next_load() {
        let inner = Arc::new(CountingRoutingGroupReadRepository::default());
        let repository = CachedRoutingGroupReadRepository::new(inner.clone());

        repository
            .list_routing_groups()
            .await
            .expect("initial list should load");
        repository
            .list_routing_groups()
            .await
            .expect("cached list should load");
        assert_eq!(inner.list_calls.load(Ordering::Acquire), 1);

        assert!(!repository
            .has_any_routing_group_binding()
            .await
            .expect("initial binding existence should load"));
        assert!(!repository
            .has_any_routing_group_binding()
            .await
            .expect("cached binding existence should load"));
        assert_eq!(inner.has_any_binding_calls.load(Ordering::Acquire), 1);

        repository.clear_local_cache();
        repository
            .list_routing_groups()
            .await
            .expect("cleared list should reload");
        assert_eq!(inner.list_calls.load(Ordering::Acquire), 2);
        assert!(!repository
            .has_any_routing_group_binding()
            .await
            .expect("cleared binding existence should reload"));
        assert_eq!(inner.has_any_binding_calls.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn follower_does_not_miss_completion_before_first_poll() {
        let inner = Arc::new(CountingRoutingGroupReadRepository::default());
        let repository = CachedRoutingGroupReadRepository::new(inner);
        let key = RoutingGroupCacheKey::HasAnyBinding;
        let mut leader = match repository.register_inflight(&key) {
            RoutingGroupInflightRegistration::Leader(leader) => leader,
            _ => panic!("first registration should lead"),
        };
        let follower = match repository.register_inflight(&key) {
            RoutingGroupInflightRegistration::Follower(state) => state,
            _ => panic!("second registration should follow"),
        };

        // Complete before the follower wait future is created or polled. A
        // bare notify_waiters().await sequence would sleep forever here.
        leader.finish(RoutingGroupInflightCompletion::Loaded(
            RoutingGroupCacheValue::Bool(true),
        ));
        tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("completed follower should not miss the broadcast");
        assert!(matches!(
            repository.follower_completion(&follower),
            Some(RoutingGroupInflightCompletion::Loaded(
                RoutingGroupCacheValue::Bool(true)
            ))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_same_key_requests_share_one_completed_load() {
        const TASKS: usize = 64;

        let inner = Arc::new(ControlledRoutingGroupReadRepository::new(
            ControlledHasAnyBindingBehavior::WaitThenSuccess(true),
        ));
        let repository = Arc::new(CachedRoutingGroupReadRepository::new(inner.clone()));
        let barrier = Arc::new(Barrier::new(TASKS + 1));
        let mut tasks = Vec::with_capacity(TASKS);
        for _ in 0..TASKS {
            let repository = Arc::clone(&repository);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                repository.has_any_routing_group_binding().await
            }));
        }

        barrier.wait().await;
        inner.wait_until_started().await;
        wait_for_same_key_inflight_participants(&repository, TASKS).await;
        inner.release.add_permits(TASKS);

        for task in tasks {
            assert!(task
                .await
                .expect("request task should finish")
                .expect("shared load should succeed"));
        }
        assert_eq!(inner.calls(), 1);
        assert!(repository
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_same_key_followers_share_leader_error() {
        const TASKS: usize = 32;

        let inner = Arc::new(ControlledRoutingGroupReadRepository::new(
            ControlledHasAnyBindingBehavior::WaitThenError,
        ));
        let repository = Arc::new(CachedRoutingGroupReadRepository::new(inner.clone()));
        let barrier = Arc::new(Barrier::new(TASKS + 1));
        let mut tasks = Vec::with_capacity(TASKS);
        for _ in 0..TASKS {
            let repository = Arc::clone(&repository);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                repository.has_any_routing_group_binding().await
            }));
        }

        barrier.wait().await;
        inner.wait_until_started().await;
        wait_for_same_key_inflight_participants(&repository, TASKS).await;
        inner.release.add_permits(TASKS);

        for task in tasks {
            let error = task
                .await
                .expect("request task should finish")
                .expect_err("shared load should fail");
            assert!(matches!(
                error,
                DataLayerError::TimedOut(message) if message == "shared routing load error"
            ));
        }
        assert_eq!(inner.calls(), 1);
    }

    #[tokio::test]
    async fn cancelled_leader_wakes_follower_for_retry() {
        let inner = Arc::new(ControlledRoutingGroupReadRepository::new(
            ControlledHasAnyBindingBehavior::FirstPendingThenTrue,
        ));
        let repository = Arc::new(CachedRoutingGroupReadRepository::new(inner.clone()));
        let leader_repository = Arc::clone(&repository);
        let leader =
            tokio::spawn(async move { leader_repository.has_any_routing_group_binding().await });
        inner.wait_until_started().await;

        let follower_repository = Arc::clone(&repository);
        let follower =
            tokio::spawn(async move { follower_repository.has_any_routing_group_binding().await });
        wait_for_same_key_inflight_participants(&repository, 2).await;
        leader.abort();
        let _ = leader.await;

        assert!(tokio::time::timeout(Duration::from_secs(1), follower)
            .await
            .expect("follower should not remain stuck after leader cancellation")
            .expect("follower task should finish")
            .expect("retried load should succeed"));
        assert_eq!(inner.calls(), 2);
    }

    #[tokio::test]
    async fn clear_wakes_followers_and_rejects_old_generation_publication() {
        let inner = Arc::new(ControlledRoutingGroupReadRepository::new(
            ControlledHasAnyBindingBehavior::FirstWaitFalseThenTrue,
        ));
        let repository = Arc::new(CachedRoutingGroupReadRepository::new(inner.clone()));
        let leader_repository = Arc::clone(&repository);
        let leader =
            tokio::spawn(async move { leader_repository.has_any_routing_group_binding().await });
        inner.wait_until_started().await;

        let follower_repository = Arc::clone(&repository);
        let follower =
            tokio::spawn(async move { follower_repository.has_any_routing_group_binding().await });
        wait_for_same_key_inflight_participants(&repository, 2).await;
        repository.clear_local_cache();

        assert!(tokio::time::timeout(Duration::from_secs(1), follower)
            .await
            .expect("clear should wake the follower")
            .expect("follower task should finish")
            .expect("new-generation load should succeed"));
        assert_eq!(inner.calls(), 2);

        inner.release.add_permits(1);
        assert!(!leader
            .await
            .expect("old leader task should finish")
            .expect("old leader load should succeed"));
        assert!(repository
            .has_any_routing_group_binding()
            .await
            .expect("new-generation value should remain cached"));
        assert_eq!(inner.calls(), 2);
    }

    #[test]
    fn capacity_full_cancelled_follower_can_retry_after_repeated_clear() {
        let inner = Arc::new(CountingRoutingGroupReadRepository::default());
        let repository = CachedRoutingGroupReadRepository::new(inner);
        let key = RoutingGroupCacheKey::HasAnyBinding;
        let mut active = Vec::with_capacity(ROUTING_GROUP_CACHE_MAX_INFLIGHT);

        for _ in 0..ROUTING_GROUP_CACHE_MAX_INFLIGHT - 1 {
            let leader = match repository.register_inflight(&key) {
                RoutingGroupInflightRegistration::Leader(guard) => guard,
                _ => panic!("each available permit should admit one leader"),
            };
            active.push(leader);
            repository.clear();
        }

        let current = match repository.register_inflight(&key) {
            RoutingGroupInflightRegistration::Leader(guard) => guard,
            _ => panic!("the final available permit should admit a leader"),
        };
        let follower = match repository.register_inflight(&key) {
            RoutingGroupInflightRegistration::Follower(state) => state,
            _ => panic!("the same-key request should follow at full capacity"),
        };
        assert_eq!(repository.admission.available_permits(), 0);
        assert!(matches!(
            repository.register_inflight(&RoutingGroupCacheKey::ListGroups),
            RoutingGroupInflightRegistration::Saturated
        ));

        drop(current);
        assert!(matches!(
            repository.follower_completion(&follower),
            Some(RoutingGroupInflightCompletion::Cancelled)
        ));
        let mut replacement = match repository.register_inflight(&key) {
            RoutingGroupInflightRegistration::Leader(guard) => guard,
            _ => panic!("cancelled follower retry should use the released permit"),
        };
        replacement.finish(RoutingGroupInflightCompletion::Cancelled);
        assert_eq!(repository.admission.available_permits(), 1);
    }
}
