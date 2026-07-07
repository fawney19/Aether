use std::sync::Arc;
use std::time::Duration;

use aether_cache::ExpiringMap;
use aether_data::DataLayerError;
use aether_data_contracts::repository::routing_profiles::{
    RoutingGroupBindingQuery, RoutingGroupBindingSubject, RoutingGroupLookupKey,
    RoutingGroupReadRepository, StoredRoutingGroup, StoredRoutingGroupBinding,
    StoredRoutingGroupVersion,
};
use async_trait::async_trait;
use dashmap::DashMap;

const ROUTING_GROUP_CACHE_STALE_TTL: Duration = Duration::from_secs(60);
const ROUTING_GROUP_CACHE_MAX_ENTRIES: usize = 4_096;
const ROUTING_GROUP_CACHE_MAX_LOAD_GUARDS: usize = 4_096;

pub(super) struct CachedRoutingGroupReadRepository {
    inner: Arc<dyn RoutingGroupReadRepository>,
    entries: ExpiringMap<RoutingGroupCacheKey, RoutingGroupCacheValue>,
    load_guards: DashMap<RoutingGroupCacheKey, Arc<tokio::sync::Mutex<()>>>,
}

impl CachedRoutingGroupReadRepository {
    pub(super) fn new(inner: Arc<dyn RoutingGroupReadRepository>) -> Self {
        Self {
            inner,
            entries: ExpiringMap::new(),
            load_guards: DashMap::new(),
        }
    }

    fn clear(&self) {
        self.entries.clear();
        self.load_guards.clear();
    }

    async fn get_or_load(
        &self,
        key: RoutingGroupCacheKey,
        load: impl std::future::Future<Output = Result<RoutingGroupCacheValue, DataLayerError>>,
    ) -> Result<RoutingGroupCacheValue, DataLayerError> {
        if let Some((value, _age)) = self
            .entries
            .get_with_age(&key, ROUTING_GROUP_CACHE_STALE_TTL)
        {
            return Ok(value);
        }
        let load_guard = self.load_guard_for(&key);
        let _guard = load_guard.lock().await;
        if let Some((value, _age)) = self
            .entries
            .get_with_age(&key, ROUTING_GROUP_CACHE_STALE_TTL)
        {
            return Ok(value);
        }
        let value = load.await?;
        self.entries.insert(
            key,
            value.clone(),
            ROUTING_GROUP_CACHE_STALE_TTL,
            ROUTING_GROUP_CACHE_MAX_ENTRIES,
        );
        Ok(value)
    }

    fn load_guard_for(&self, key: &RoutingGroupCacheKey) -> Arc<tokio::sync::Mutex<()>> {
        if self.load_guards.len() > ROUTING_GROUP_CACHE_MAX_LOAD_GUARDS {
            self.load_guards.clear();
        }
        self.load_guards
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RoutingGroupCacheKey {
    ListGroups,
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
    Group(Option<StoredRoutingGroup>),
    Bindings(Vec<StoredRoutingGroupBinding>),
    Versions(Vec<StoredRoutingGroupVersion>),
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
            .get_or_load(RoutingGroupCacheKey::ListGroups, async {
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
        match self
            .get_or_load(key, async {
                self.inner
                    .find_routing_group(lookup)
                    .await
                    .map(RoutingGroupCacheValue::Group)
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
            .get_or_load(key, async {
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

    async fn list_routing_group_versions(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredRoutingGroupVersion>, DataLayerError> {
        let key = RoutingGroupCacheKey::Versions(group_id.to_string());
        match self
            .get_or_load(key, async {
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

    use super::*;

    #[derive(Default)]
    struct CountingRoutingGroupReadRepository {
        list_calls: AtomicUsize,
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

        async fn list_routing_group_versions(
            &self,
            _group_id: &str,
        ) -> Result<Vec<StoredRoutingGroupVersion>, DataLayerError> {
            Ok(Vec::new())
        }
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

        repository.clear_local_cache();
        repository
            .list_routing_groups()
            .await
            .expect("cleared list should reload");
        assert_eq!(inner.list_calls.load(Ordering::Acquire), 2);
    }
}
