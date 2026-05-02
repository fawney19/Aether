use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery, ProviderCatalogReadRepository,
    ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogKeyPage, StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
use crate::repository::usage::{ProviderApiKeyUsageContribution, ProviderApiKeyUsageDelta};
use crate::DataLayerError;

#[derive(Debug, Default)]
struct MemoryProviderCatalogIndex {
    providers: BTreeMap<String, StoredProviderCatalogProvider>,
    endpoints: BTreeMap<String, StoredProviderCatalogEndpoint>,
    keys: BTreeMap<String, StoredProviderCatalogKey>,
}

#[derive(Debug, Default)]
pub struct InMemoryProviderCatalogReadRepository {
    index: RwLock<MemoryProviderCatalogIndex>,
}

impl InMemoryProviderCatalogReadRepository {
    pub fn seed(
        providers: Vec<StoredProviderCatalogProvider>,
        endpoints: Vec<StoredProviderCatalogEndpoint>,
        keys: Vec<StoredProviderCatalogKey>,
    ) -> Self {
        Self {
            index: RwLock::new(MemoryProviderCatalogIndex {
                providers: providers
                    .into_iter()
                    .map(|provider| (provider.id.clone(), provider))
                    .collect(),
                endpoints: endpoints
                    .into_iter()
                    .map(|endpoint| (endpoint.id.clone(), endpoint))
                    .collect(),
                keys: keys.into_iter().map(|key| (key.id.clone(), key)).collect(),
            }),
        }
    }

    pub(crate) fn apply_usage_stats_delta(
        &self,
        key_id: &str,
        delta: &ProviderApiKeyUsageDelta,
        recomputed_last_used_at_unix_secs: Option<u64>,
    ) {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return;
        };

        key.request_count = Some(apply_i64_delta_to_u32(
            key.request_count.unwrap_or_default(),
            delta.request_count,
        ));
        key.success_count = Some(apply_i64_delta_to_u32(
            key.success_count.unwrap_or_default(),
            delta.success_count,
        ));
        key.error_count = Some(apply_i64_delta_to_u32(
            key.error_count.unwrap_or_default(),
            delta.error_count,
        ));
        key.total_tokens = apply_i64_delta_to_u64(key.total_tokens, delta.total_tokens);
        key.total_cost_usd = apply_f64_delta(key.total_cost_usd, delta.total_cost_usd);
        key.total_response_time_ms = Some(apply_i64_delta_to_u32(
            key.total_response_time_ms.unwrap_or_default(),
            delta.total_response_time_ms,
        ));

        if let Some(candidate) = delta.candidate_last_used_at_unix_secs {
            key.last_used_at_unix_secs = Some(
                key.last_used_at_unix_secs
                    .map(|existing| existing.max(candidate))
                    .unwrap_or(candidate),
            );
        } else if delta.removed_last_used_at_unix_secs.is_some()
            && key.last_used_at_unix_secs == delta.removed_last_used_at_unix_secs
        {
            key.last_used_at_unix_secs = recomputed_last_used_at_unix_secs;
        }
    }

    pub(crate) fn rebuild_usage_stats(
        &self,
        contributions: &BTreeMap<String, ProviderApiKeyUsageContribution>,
    ) {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        for key in index.keys.values_mut() {
            key.request_count = Some(0);
            key.success_count = Some(0);
            key.error_count = Some(0);
            key.total_tokens = 0;
            key.total_cost_usd = 0.0;
            key.total_response_time_ms = Some(0);
            key.last_used_at_unix_secs = None;
        }

        for (key_id, contribution) in contributions {
            let Some(key) = index.keys.get_mut(key_id) else {
                continue;
            };
            key.request_count = Some(clamp_i64_to_u32(contribution.request_count));
            key.success_count = Some(clamp_i64_to_u32(contribution.success_count));
            key.error_count = Some(clamp_i64_to_u32(contribution.error_count));
            key.total_tokens = clamp_i64_to_u64(contribution.total_tokens);
            key.total_cost_usd = contribution.total_cost_usd.max(0.0);
            key.total_response_time_ms =
                Some(clamp_i64_to_u32(contribution.total_response_time_ms));
            key.last_used_at_unix_secs = contribution.last_used_at_unix_secs;
        }
    }
}

fn apply_i64_delta_to_u32(current: u32, delta: i64) -> u32 {
    clamp_i64_to_u32(i64::from(current).saturating_add(delta))
}

fn apply_i64_delta_to_u64(current: u64, delta: i64) -> u64 {
    if delta >= 0 {
        current.saturating_add(delta as u64)
    } else {
        current.saturating_sub(delta.unsigned_abs())
    }
}

fn clamp_i64_to_u32(value: i64) -> u32 {
    value.clamp(0, i64::from(u32::MAX)) as u32
}

fn clamp_i64_to_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn apply_f64_delta(current: f64, delta: f64) -> f64 {
    if !current.is_finite() && !delta.is_finite() {
        return 0.0;
    }
    let next = current.max(0.0) + delta;
    if next.is_finite() {
        next.max(0.0)
    } else {
        0.0
    }
}

#[async_trait]
impl ProviderCatalogReadRepository for InMemoryProviderCatalogReadRepository {
    async fn list_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        let index = self.index.read().expect("provider catalog repository lock");
        let mut providers = index
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
        Ok(providers)
    }

    async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        let index = self.index.read().expect("provider catalog repository lock");
        Ok(provider_ids
            .iter()
            .filter_map(|id| index.providers.get(id).cloned())
            .collect())
    }

    async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        let index = self.index.read().expect("provider catalog repository lock");
        Ok(endpoint_ids
            .iter()
            .filter_map(|id| index.endpoints.get(id).cloned())
            .collect())
    }

    async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        let index = self.index.read().expect("provider catalog repository lock");
        let mut endpoints = index
            .endpoints
            .values()
            .filter(|endpoint| {
                provider_ids
                    .iter()
                    .any(|provider_id| provider_id == &endpoint.provider_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        endpoints.sort_by(|left, right| {
            left.provider_id
                .cmp(&right.provider_id)
                .then(left.api_format.cmp(&right.api_format))
                .then(left.id.cmp(&right.id))
        });
        Ok(endpoints)
    }

    async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        let index = self.index.read().expect("provider catalog repository lock");
        Ok(key_ids
            .iter()
            .filter_map(|id| index.keys.get(id).cloned())
            .collect())
    }

    async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        let index = self.index.read().expect("provider catalog repository lock");
        let mut keys = index
            .keys
            .values()
            .filter(|key| {
                provider_ids
                    .iter()
                    .any(|provider_id| provider_id == &key.provider_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        keys.sort_by(|left, right| {
            left.provider_id
                .cmp(&right.provider_id)
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
        Ok(keys)
    }

    async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        Self::list_keys_by_provider_ids(self, provider_ids).await
    }

    async fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
        let index = self.index.read().expect("provider catalog repository lock");
        let mut keys = index
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
            .filter(|key| {
                query
                    .is_active
                    .is_none_or(|is_active| key.is_active == is_active)
            })
            .cloned()
            .collect::<Vec<_>>();
        match query.order {
            ProviderCatalogKeyListOrder::Name => {
                keys.sort_by(|left, right| {
                    left.internal_priority
                        .cmp(&right.internal_priority)
                        .then(left.name.cmp(&right.name))
                        .then(left.id.cmp(&right.id))
                });
            }
            ProviderCatalogKeyListOrder::CreatedAt => {
                keys.sort_by(|left, right| {
                    left.internal_priority
                        .cmp(&right.internal_priority)
                        .then(
                            left.created_at_unix_ms
                                .unwrap_or_default()
                                .cmp(&right.created_at_unix_ms.unwrap_or_default()),
                        )
                        .then(left.id.cmp(&right.id))
                });
            }
        }
        let total = keys.len();
        let items = keys
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(StoredProviderCatalogKeyPage { items, total })
    }

    async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        let index = self.index.read().expect("provider catalog repository lock");
        let mut stats = provider_ids
            .iter()
            .map(|provider_id| {
                let total_keys = index
                    .keys
                    .values()
                    .filter(|key| &key.provider_id == provider_id)
                    .count() as i64;
                let active_keys = index
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

#[async_trait]
impl ProviderCatalogWriteRepository for InMemoryProviderCatalogReadRepository {
    async fn create_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        if let Some(target_priority) = shift_existing_priorities_from {
            for existing in index.providers.values_mut() {
                if existing.provider_priority >= target_priority {
                    existing.provider_priority += 1;
                }
            }
        }
        index
            .providers
            .insert(provider.id.clone(), provider.clone());
        Ok(provider.clone())
    }

    async fn update_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<StoredProviderCatalogProvider, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(stored) = index.providers.get_mut(&provider.id) else {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog provider {} not found",
                provider.id
            )));
        };
        *stored = provider.clone();
        Ok(stored.clone())
    }

    async fn delete_provider(&self, provider_id: &str) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        Ok(index.providers.remove(provider_id).is_some())
    }

    async fn cleanup_deleted_provider_refs(
        &self,
        _provider_id: &str,
        _endpoint_ids: &[String],
        _key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        Ok(())
    }

    async fn create_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        index
            .endpoints
            .insert(endpoint.id.clone(), endpoint.clone());
        Ok(endpoint.clone())
    }

    async fn update_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(stored) = index.endpoints.get_mut(&endpoint.id) else {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog endpoint {} not found",
                endpoint.id
            )));
        };
        *stored = endpoint.clone();
        Ok(stored.clone())
    }

    async fn delete_endpoint(&self, endpoint_id: &str) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        Ok(index.endpoints.remove(endpoint_id).is_some())
    }

    async fn create_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        index.keys.insert(key.id.clone(), key.clone());
        Ok(key.clone())
    }

    async fn update_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(stored) = index.keys.get_mut(&key.id) else {
            return Err(DataLayerError::UnexpectedValue(format!(
                "provider catalog key {} not found",
                key.id
            )));
        };
        *stored = key.clone();
        Ok(stored.clone())
    }

    async fn update_key_upstream_metadata(
        &self,
        key_id: &str,
        upstream_metadata: Option<&serde_json::Value>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };

        key.upstream_metadata = upstream_metadata.cloned();
        key.updated_at_unix_secs = updated_at_unix_secs;
        Ok(true)
    }

    async fn delete_key(&self, key_id: &str) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        Ok(index.keys.remove(key_id).is_some())
    }

    async fn clear_key_oauth_invalid_marker(&self, key_id: &str) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };

        key.oauth_invalid_at_unix_secs = None;
        key.oauth_invalid_reason = None;
        Ok(true)
    }

    async fn update_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        if encrypted_api_key.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog oauth api_key is empty".to_string(),
            ));
        }

        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };

        key.encrypted_api_key = Some(encrypted_api_key.to_string());
        key.encrypted_auth_config = encrypted_auth_config.map(ToOwned::to_owned);
        key.expires_at_unix_secs = expires_at_unix_secs;
        Ok(true)
    }

    async fn update_key_health_state(
        &self,
        key_id: &str,
        is_active: bool,
        health_by_format: Option<&serde_json::Value>,
        circuit_breaker_by_format: Option<&serde_json::Value>,
    ) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };

        key.is_active = is_active;
        key.health_by_format = health_by_format.cloned();
        key.circuit_breaker_by_format = circuit_breaker_by_format.cloned();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryProviderCatalogReadRepository;
    use crate::repository::provider_catalog::{
        ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery, ProviderCatalogReadRepository,
        ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };

    fn sample_provider(id: &str) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            id.to_string(),
            format!("provider-{id}"),
            Some("https://example.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
    }

    fn sample_endpoint(id: &str, provider_id: &str) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            id.to_string(),
            provider_id.to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_health_score(0.9)
    }

    fn sample_key(id: &str, provider_id: &str) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            id.to_string(),
            provider_id.to_string(),
            "default".to_string(),
            "api_key".to_string(),
            Some(serde_json::json!({"cache_1h": true})),
            true,
        )
        .expect("key should build")
    }

    #[tokio::test]
    async fn reads_provider_catalog_items_by_id() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![sample_endpoint("endpoint-1", "provider-1")],
            vec![sample_key("key-1", "provider-1")],
        );

        assert_eq!(
            repository
                .list_providers_by_ids(&["provider-1".to_string()])
                .await
                .expect("providers should read")
                .len(),
            1
        );
        assert_eq!(
            repository
                .list_endpoints_by_ids(&["endpoint-1".to_string()])
                .await
                .expect("endpoints should read")
                .len(),
            1
        );
        assert_eq!(
            repository
                .list_keys_by_ids(&["key-1".to_string()])
                .await
                .expect("keys should read")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn lists_active_providers_in_priority_order() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider("provider-2").with_routing_fields(20),
                sample_provider("provider-1").with_routing_fields(10),
                sample_provider("provider-3")
                    .with_routing_fields(5)
                    .with_transport_fields(false, false, false, None, None, None, None, None, None),
            ],
            vec![],
            vec![],
        );

        let providers = repository
            .list_providers(true)
            .await
            .expect("providers should list");
        assert_eq!(
            providers
                .iter()
                .map(|provider| provider.id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider-1", "provider-2"]
        );
    }

    #[tokio::test]
    async fn updates_oauth_credentials_for_existing_key() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![sample_endpoint("endpoint-1", "provider-1")],
            vec![sample_key("key-1", "provider-1")
                .with_transport_fields(
                    None,
                    "ciphertext-placeholder".to_string(),
                    Some("ciphertext-auth-1".to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .expect("key transport should build")],
        );

        assert!(repository
            .update_key_oauth_credentials(
                "key-1",
                "ciphertext-updated-token",
                Some("ciphertext-auth-2"),
                Some(4_102_444_800),
            )
            .await
            .expect("update should succeed"));

        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("keys should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].encrypted_api_key.as_deref(),
            Some("ciphertext-updated-token")
        );
        assert_eq!(
            stored[0].encrypted_auth_config.as_deref(),
            Some("ciphertext-auth-2")
        );
        assert_eq!(stored[0].expires_at_unix_secs, Some(4_102_444_800));
    }

    #[tokio::test]
    async fn paginates_provider_keys_with_search_and_active_filter() {
        let mut alpha = sample_key("key-1", "provider-1");
        alpha.name = "alpha".to_string();
        alpha.internal_priority = 20;
        let mut beta = sample_key("key-2", "provider-1");
        beta.name = "beta".to_string();
        beta.internal_priority = 10;
        let mut gamma = sample_key("key-3", "provider-1");
        gamma.name = "gamma".to_string();
        gamma.internal_priority = 30;
        gamma.is_active = false;
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1"), sample_provider("provider-2")],
            vec![],
            vec![alpha, beta, gamma, sample_key("key-4", "provider-2")],
        );

        let page = repository
            .list_keys_page(&ProviderCatalogKeyListQuery {
                provider_id: "provider-1".to_string(),
                search: Some("a".to_string()),
                is_active: Some(true),
                offset: 0,
                limit: 10,
                order: ProviderCatalogKeyListOrder::Name,
            })
            .await
            .expect("keys should page");

        assert_eq!(page.total, 2);
        assert_eq!(page.items.len(), 2);
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["beta", "alpha"]
        );
    }

    #[tokio::test]
    async fn paginates_provider_keys_by_created_at_when_requested() {
        let mut early = sample_key("key-1", "provider-1");
        early.name = "zeta".to_string();
        early.internal_priority = 10;
        early.created_at_unix_ms = Some(10);
        let mut late = sample_key("key-2", "provider-1");
        late.name = "alpha".to_string();
        late.internal_priority = 10;
        late.created_at_unix_ms = Some(20);
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![late, early],
        );

        let page = repository
            .list_keys_page(&ProviderCatalogKeyListQuery {
                provider_id: "provider-1".to_string(),
                search: None,
                is_active: None,
                offset: 0,
                limit: 10,
                order: ProviderCatalogKeyListOrder::CreatedAt,
            })
            .await
            .expect("keys should page");

        assert_eq!(page.total, 2);
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-1", "key-2"]
        );
    }

    #[tokio::test]
    async fn summarizes_provider_key_stats() {
        let mut inactive = sample_key("key-2", "provider-1");
        inactive.is_active = false;
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1"), sample_provider("provider-2")],
            vec![],
            vec![
                sample_key("key-1", "provider-1"),
                inactive,
                sample_key("key-3", "provider-2"),
            ],
        );

        let stats = repository
            .list_key_stats_by_provider_ids(&["provider-1".to_string(), "provider-2".to_string()])
            .await
            .expect("stats should list");
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].provider_id, "provider-1");
        assert_eq!(stats[0].total_keys, 2);
        assert_eq!(stats[0].active_keys, 1);
        assert_eq!(stats[1].provider_id, "provider-2");
        assert_eq!(stats[1].total_keys, 1);
        assert_eq!(stats[1].active_keys, 1);
    }

    #[tokio::test]
    async fn creates_key() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![],
        );
        let key = sample_key("key-1", "provider-1");

        let created = repository
            .create_key(&key)
            .await
            .expect("key should create");

        assert_eq!(created.id, "key-1");
        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("keys should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].provider_id, "provider-1");
    }

    #[tokio::test]
    async fn provider_api_keys_concurrent_limit_defaults_and_round_trips_in_memory() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("test-provider-a")],
            vec![],
            vec![],
        );
        let mut key = sample_key("provider-key-a", "test-provider-a");
        assert_eq!(key.concurrent_limit, None);
        key.concurrent_limit = Some(1);

        let created = repository
            .create_key(&key)
            .await
            .expect("key should create");
        assert_eq!(created.concurrent_limit, Some(1));

        let mut updated = created.clone();
        updated.concurrent_limit = None;
        repository
            .update_key(&updated)
            .await
            .expect("key should update");
        let reloaded = repository
            .list_keys_by_ids(&["provider-key-a".to_string()])
            .await
            .expect("keys should read");
        assert_eq!(reloaded[0].concurrent_limit, None);
    }

    #[tokio::test]
    async fn creates_endpoint() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![],
        );
        let endpoint = sample_endpoint("endpoint-1", "provider-1");

        let created = repository
            .create_endpoint(&endpoint)
            .await
            .expect("endpoint should create");

        assert_eq!(created.id, "endpoint-1");
        let stored = repository
            .list_endpoints_by_ids(&["endpoint-1".to_string()])
            .await
            .expect("endpoints should read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].provider_id, "provider-1");
    }

    #[tokio::test]
    async fn updates_key() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![sample_key("key-1", "provider-1")],
        );
        let mut updated = sample_key("key-1", "provider-1");
        updated.name = "updated".to_string();
        updated.internal_priority = 7;

        let stored = repository
            .update_key(&updated)
            .await
            .expect("key should update");

        assert_eq!(stored.name, "updated");
        assert_eq!(stored.internal_priority, 7);
        let reloaded = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("keys should read");
        assert_eq!(reloaded[0].name, "updated");
        assert_eq!(reloaded[0].internal_priority, 7);
    }

    #[tokio::test]
    async fn updates_endpoint() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![sample_endpoint("endpoint-1", "provider-1")],
            vec![],
        );
        let updated = sample_endpoint("endpoint-1", "provider-1")
            .with_transport_fields(
                "https://updated.example".to_string(),
                None,
                None,
                Some(5),
                Some("/v1/chat/completions".to_string()),
                Some(serde_json::json!({"foo":"bar"})),
                None,
                None,
            )
            .expect("endpoint transport should build");

        let stored = repository
            .update_endpoint(&updated)
            .await
            .expect("endpoint should update");

        assert_eq!(stored.base_url, "https://updated.example");
        assert_eq!(stored.max_retries, Some(5));
        let reloaded = repository
            .list_endpoints_by_ids(&["endpoint-1".to_string()])
            .await
            .expect("endpoints should read");
        assert_eq!(reloaded[0].base_url, "https://updated.example");
        assert_eq!(reloaded[0].max_retries, Some(5));
    }

    #[tokio::test]
    async fn deletes_key() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![sample_key("key-1", "provider-1")],
        );

        assert!(repository
            .delete_key("key-1")
            .await
            .expect("delete should succeed"));
        let reloaded = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("keys should read");
        assert!(reloaded.is_empty());
    }

    #[tokio::test]
    async fn deletes_endpoint() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![sample_endpoint("endpoint-1", "provider-1")],
            vec![],
        );

        assert!(repository
            .delete_endpoint("endpoint-1")
            .await
            .expect("delete should succeed"));
        let reloaded = repository
            .list_endpoints_by_ids(&["endpoint-1".to_string()])
            .await
            .expect("endpoints should read");
        assert!(reloaded.is_empty());
    }
}
