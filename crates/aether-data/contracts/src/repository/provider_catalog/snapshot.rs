use std::cmp::Ordering;
use std::collections::BTreeMap;

use super::{
    ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogKeyMaintenanceSummary,
    StoredProviderCatalogKeyPage, StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use crate::DataLayerError;

/// Immutable provider-catalog view used by memory and driver adapters.
#[derive(Debug, Clone, Default)]
pub struct ProviderCatalogSnapshot {
    providers: BTreeMap<String, StoredProviderCatalogProvider>,
    endpoints: BTreeMap<String, StoredProviderCatalogEndpoint>,
    keys: BTreeMap<String, StoredProviderCatalogKey>,
}

impl ProviderCatalogSnapshot {
    pub fn new(
        providers: Vec<StoredProviderCatalogProvider>,
        endpoints: Vec<StoredProviderCatalogEndpoint>,
        keys: Vec<StoredProviderCatalogKey>,
    ) -> Self {
        Self {
            providers: providers
                .into_iter()
                .map(|item| (item.id.clone(), item))
                .collect(),
            endpoints: endpoints
                .into_iter()
                .map(|item| (item.id.clone(), item))
                .collect(),
            keys: keys
                .into_iter()
                .map(|item| (item.id.clone(), item))
                .collect(),
        }
    }

    pub fn list_providers(&self, active_only: bool) -> Vec<StoredProviderCatalogProvider> {
        let mut providers = self
            .providers
            .values()
            .filter(|provider| !active_only || provider.is_active)
            .cloned()
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| {
            left.provider_priority
                .cmp(&right.provider_priority)
                .then(left.name.cmp(&right.name))
        });
        providers
    }

    pub fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Vec<StoredProviderCatalogProvider> {
        provider_ids
            .iter()
            .filter_map(|id| self.providers.get(id).cloned())
            .collect()
    }

    pub fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Vec<StoredProviderCatalogEndpoint> {
        endpoint_ids
            .iter()
            .filter_map(|id| self.endpoints.get(id).cloned())
            .collect()
    }

    pub fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Vec<StoredProviderCatalogEndpoint> {
        let mut endpoints = self
            .endpoints
            .values()
            .filter(|endpoint| provider_ids.contains(&endpoint.provider_id))
            .cloned()
            .collect::<Vec<_>>();
        endpoints.sort_by(|left, right| {
            left.provider_id
                .cmp(&right.provider_id)
                .then(left.api_format.cmp(&right.api_format))
                .then(left.id.cmp(&right.id))
        });
        endpoints
    }

    pub fn list_keys_by_ids(&self, key_ids: &[String]) -> Vec<StoredProviderCatalogKey> {
        key_ids
            .iter()
            .filter_map(|id| self.keys.get(id).cloned())
            .collect()
    }

    pub fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Vec<StoredProviderCatalogKey> {
        let mut keys = self
            .keys
            .values()
            .filter(|key| provider_ids.contains(&key.provider_id))
            .cloned()
            .collect::<Vec<_>>();
        keys.sort_by(|left, right| {
            left.provider_id
                .cmp(&right.provider_id)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        keys
    }

    pub fn list_key_maintenance_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Vec<StoredProviderCatalogKeyMaintenanceSummary> {
        let mut keys = self
            .keys
            .values()
            .filter(|key| provider_ids.contains(&key.provider_id))
            .map(|key| StoredProviderCatalogKeyMaintenanceSummary {
                id: key.id.clone(),
                provider_id: key.provider_id.clone(),
                is_active: key.is_active,
                upstream_metadata: key.upstream_metadata.clone(),
            })
            .collect::<Vec<_>>();
        keys.sort_by(|left, right| {
            left.provider_id
                .cmp(&right.provider_id)
                .then(left.id.cmp(&right.id))
        });
        keys
    }

    pub fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> StoredProviderCatalogKeyPage {
        let mut keys = self
            .keys
            .values()
            .filter(|key| key.provider_id == query.provider_id)
            .filter(|key| {
                query.search.as_ref().is_none_or(|keyword| {
                    let keyword = keyword.trim().to_ascii_lowercase();
                    keyword.is_empty()
                        || key.name.to_ascii_lowercase().contains(&keyword)
                        || key.id.to_ascii_lowercase().contains(&keyword)
                })
            })
            .filter(|key| query.is_active.is_none_or(|value| key.is_active == value))
            .cloned()
            .collect::<Vec<_>>();
        sort_key_page(&mut keys, query.order.clone());
        let total = keys.len();
        let items = keys
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        StoredProviderCatalogKeyPage { items, total }
    }

    pub fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        let mut stats = provider_ids
            .iter()
            .map(|provider_id| {
                let total_keys = self
                    .keys
                    .values()
                    .filter(|key| &key.provider_id == provider_id)
                    .count() as i64;
                let active_keys = self
                    .keys
                    .values()
                    .filter(|key| &key.provider_id == provider_id && key.is_active)
                    .count() as i64;
                StoredProviderCatalogKeyStats::new(provider_id.clone(), total_keys, active_keys)
            })
            .collect::<Result<Vec<_>, _>>()?;
        stats.retain(|item| item.total_keys > 0);
        Ok(stats)
    }
}

fn compare_optional_u64_null_last(
    left: Option<u64>,
    right: Option<u64>,
    descending: bool,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) if descending => right.cmp(&left),
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn sort_key_page(items: &mut [StoredProviderCatalogKey], order: ProviderCatalogKeyListOrder) {
    items.sort_by(|left, right| match order {
        ProviderCatalogKeyListOrder::Name => left
            .internal_priority
            .cmp(&right.internal_priority)
            .then(left.name.cmp(&right.name))
            .then(left.id.cmp(&right.id)),
        ProviderCatalogKeyListOrder::CreatedAt => left
            .internal_priority
            .cmp(&right.internal_priority)
            .then(
                left.created_at_unix_ms
                    .unwrap_or_default()
                    .cmp(&right.created_at_unix_ms.unwrap_or_default()),
            )
            .then(left.id.cmp(&right.id)),
        ProviderCatalogKeyListOrder::CreatedAtAsc => {
            compare_optional_u64_null_last(left.created_at_unix_ms, right.created_at_unix_ms, false)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        }
        ProviderCatalogKeyListOrder::CreatedAtDesc => {
            compare_optional_u64_null_last(left.created_at_unix_ms, right.created_at_unix_ms, true)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        }
        ProviderCatalogKeyListOrder::LastUsedAtAsc => compare_optional_u64_null_last(
            left.last_used_at_unix_secs,
            right.last_used_at_unix_secs,
            false,
        )
        .then(left.name.cmp(&right.name))
        .then(left.id.cmp(&right.id)),
        ProviderCatalogKeyListOrder::LastUsedAtDesc => compare_optional_u64_null_last(
            left.last_used_at_unix_secs,
            right.last_used_at_unix_secs,
            true,
        )
        .then(left.name.cmp(&right.name))
        .then(left.id.cmp(&right.id)),
    });
}
