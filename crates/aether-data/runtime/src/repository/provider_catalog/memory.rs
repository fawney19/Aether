use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use super::{
    ProviderCatalogKeyAdaptiveState, ProviderCatalogKeyAdaptiveStateUpdate,
    ProviderCatalogKeyHealthStateUpdate, ProviderCatalogKeyListQuery,
    ProviderCatalogKeyRuntimeMetadataUpdate, ProviderCatalogKeyStatusSnapshotUpdate,
    ProviderCatalogReadRepository, ProviderCatalogSnapshot,
    ProviderCatalogUpstreamMetadataNamespaceUpdate, ProviderCatalogWriteRepository,
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogKeyMaintenanceSummary, StoredProviderCatalogKeyPage,
    StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
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

    fn snapshot(&self) -> ProviderCatalogSnapshot {
        let index = self.index.read().expect("provider catalog repository lock");
        ProviderCatalogSnapshot::new(
            index.providers.values().cloned().collect(),
            index.endpoints.values().cloned().collect(),
            index.keys.values().cloned().collect(),
        )
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
        key.total_response_time_ms = Some(apply_i64_delta_to_u64(
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

        apply_codex_window_usage_stats_delta(&mut key.status_snapshot, delta);
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
                Some(clamp_i64_to_u64(contribution.total_response_time_ms));
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

fn default_codex_window_minutes(code: &str) -> Option<u64> {
    if code.eq_ignore_ascii_case("5h") {
        Some(300)
    } else if code.eq_ignore_ascii_case("weekly") {
        Some(10_080)
    } else {
        None
    }
}

fn json_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value.as_u64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
    })
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value.as_i64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
    })
}

fn json_f64(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| {
        value.as_f64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<f64>().ok())
        })
    })
}

fn apply_i64_delta_to_json_u64(current: u64, delta: i64) -> u64 {
    if delta >= 0 {
        current.saturating_add(delta as u64)
    } else {
        current.saturating_sub(delta.unsigned_abs())
    }
}

fn apply_f64_delta_to_json_cost(current: f64, delta: f64) -> f64 {
    let current = if current.is_finite() { current } else { 0.0 };
    let delta = if delta.is_finite() { delta } else { 0.0 };
    (current + delta).max(0.0)
}

fn codex_window_matches_usage_time(window: &Map<String, Value>, usage_created_at: u64) -> bool {
    let code = window
        .get("code")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !code.eq_ignore_ascii_case("5h") && !code.eq_ignore_ascii_case("weekly") {
        return false;
    }

    let Some(reset_at) = json_u64(window.get("reset_at")) else {
        return false;
    };
    let Some(window_minutes) =
        json_u64(window.get("window_minutes")).or_else(|| default_codex_window_minutes(code))
    else {
        return false;
    };
    let Some(window_seconds) = window_minutes.checked_mul(60) else {
        return false;
    };
    let Some(window_start) = reset_at.checked_sub(window_seconds) else {
        return false;
    };
    let usage_reset_at = json_u64(window.get("usage_reset_at")).unwrap_or(0);
    let start = window_start.max(usage_reset_at);
    usage_created_at >= start && usage_created_at < reset_at
}

fn apply_codex_window_usage_stats_delta(
    status_snapshot: &mut Option<Value>,
    delta: &ProviderApiKeyUsageDelta,
) {
    let Some(usage_created_at) = delta.usage_created_at_unix_secs else {
        return;
    };
    if delta.request_count == 0 && delta.total_tokens == 0 && delta.total_cost_usd == 0.0 {
        return;
    }

    let Some(quota) = status_snapshot
        .as_mut()
        .and_then(Value::as_object_mut)
        .and_then(|snapshot| snapshot.get_mut("quota"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let quota_provider_type = quota
        .get("provider_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !quota_provider_type.eq_ignore_ascii_case("codex") {
        return;
    }
    let Some(windows) = quota.get_mut("windows").and_then(Value::as_array_mut) else {
        return;
    };

    for window in windows.iter_mut().filter_map(Value::as_object_mut) {
        if !codex_window_matches_usage_time(window, usage_created_at) {
            continue;
        }

        let usage = window
            .entry("usage".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut();
        let Some(usage) = usage else {
            window.insert("usage".to_string(), json!({}));
            let Some(usage) = window.get_mut("usage").and_then(Value::as_object_mut) else {
                continue;
            };
            let request_count = apply_i64_delta_to_json_u64(0, delta.request_count);
            let total_tokens = apply_i64_delta_to_json_u64(0, delta.total_tokens);
            let total_cost_usd = apply_f64_delta_to_json_cost(0.0, delta.total_cost_usd);
            usage.insert("request_count".to_string(), json!(request_count));
            usage.insert("total_tokens".to_string(), json!(total_tokens));
            usage.insert(
                "total_cost_usd".to_string(),
                json!(format!("{total_cost_usd:.8}")),
            );
            continue;
        };

        let request_count = apply_i64_delta_to_json_u64(
            json_i64(usage.get("request_count")).unwrap_or(0).max(0) as u64,
            delta.request_count,
        );
        let total_tokens = apply_i64_delta_to_json_u64(
            json_i64(usage.get("total_tokens")).unwrap_or(0).max(0) as u64,
            delta.total_tokens,
        );
        let total_cost_usd = apply_f64_delta_to_json_cost(
            json_f64(usage.get("total_cost_usd")).unwrap_or(0.0),
            delta.total_cost_usd,
        );

        usage.insert("request_count".to_string(), json!(request_count));
        usage.insert("total_tokens".to_string(), json!(total_tokens));
        usage.insert(
            "total_cost_usd".to_string(),
            json!(format!("{total_cost_usd:.8}")),
        );
    }
}

#[async_trait]
impl ProviderCatalogReadRepository for InMemoryProviderCatalogReadRepository {
    async fn list_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        Ok(self.snapshot().list_providers(active_only))
    }

    async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        Ok(self.snapshot().list_providers_by_ids(provider_ids))
    }

    async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        Ok(self.snapshot().list_endpoints_by_ids(endpoint_ids))
    }

    async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        Ok(self.snapshot().list_endpoints_by_provider_ids(provider_ids))
    }

    async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        Ok(self.snapshot().list_keys_by_ids(key_ids))
    }

    async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        Ok(self.snapshot().list_keys_by_provider_ids(provider_ids))
    }

    async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        Ok(self.snapshot().list_keys_by_provider_ids(provider_ids))
    }

    async fn list_key_maintenance_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, DataLayerError> {
        Ok(self
            .snapshot()
            .list_key_maintenance_summaries_by_provider_ids(provider_ids))
    }

    async fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
        Ok(self.snapshot().list_keys_page(query))
    }

    async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        self.snapshot().list_key_stats_by_provider_ids(provider_ids)
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
        _provider_deleted: bool,
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
        *stored = merge_admin_key_update(stored, key);
        Ok(stored.clone())
    }

    async fn update_keys(
        &self,
        keys: &[StoredProviderCatalogKey],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        for key in keys {
            if !index.keys.contains_key(&key.id) {
                return Err(DataLayerError::UnexpectedValue(format!(
                    "provider catalog key {} not found",
                    key.id
                )));
            }
        }
        for key in keys {
            let stored = index
                .keys
                .get_mut(&key.id)
                .expect("provider catalog key existence was validated");
            *stored = merge_admin_key_update(stored, key);
        }
        Ok(keys
            .iter()
            .filter_map(|key| index.keys.get(&key.id).cloned())
            .collect())
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
        key.updated_at_unix_secs = Some(updated_at_unix_secs.unwrap_or_else(current_unix_secs));
        Ok(true)
    }

    async fn upsert_key_upstream_metadata_namespace(
        &self,
        key_id: &str,
        namespace: &str,
        value: &serde_json::Value,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        if namespace.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog upstream metadata namespace is empty".to_string(),
            ));
        }
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };
        let metadata = key
            .upstream_metadata
            .get_or_insert_with(|| serde_json::json!({}));
        let Some(metadata) = metadata.as_object_mut() else {
            return Err(DataLayerError::UnexpectedValue(
                "provider catalog upstream metadata must be an object".to_string(),
            ));
        };
        metadata.insert(namespace.to_string(), value.clone());
        key.updated_at_unix_secs = Some(updated_at_unix_secs.unwrap_or_else(current_unix_secs));
        Ok(true)
    }

    async fn update_key_model_fetch_state(
        &self,
        key_id: &str,
        allowed_models: Option<&serde_json::Value>,
        last_models_fetch_at_unix_secs: Option<u64>,
        last_models_fetch_error: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };
        key.allowed_models = allowed_models.cloned();
        key.last_models_fetch_at_unix_secs = last_models_fetch_at_unix_secs;
        key.last_models_fetch_error = last_models_fetch_error.map(str::to_string);
        key.updated_at_unix_secs = Some(updated_at_unix_secs.unwrap_or_else(current_unix_secs));
        Ok(true)
    }

    async fn update_key_model_fetch_success(
        &self,
        key_id: &str,
        allowed_models: Option<&serde_json::Value>,
        last_models_fetch_at_unix_secs: u64,
        upstream_metadata_updates: &[ProviderCatalogUpstreamMetadataNamespaceUpdate],
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        if upstream_metadata_updates
            .iter()
            .any(|update| update.namespace.trim().is_empty())
        {
            return Err(DataLayerError::InvalidInput(
                "provider catalog upstream metadata namespace is empty".to_string(),
            ));
        }
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };
        if !upstream_metadata_updates.is_empty()
            && key
                .upstream_metadata
                .as_ref()
                .is_some_and(|metadata| !metadata.is_object())
        {
            return Err(DataLayerError::UnexpectedValue(
                "provider catalog upstream metadata must be an object".to_string(),
            ));
        }

        key.allowed_models = allowed_models.cloned();
        key.last_models_fetch_at_unix_secs = Some(last_models_fetch_at_unix_secs);
        key.last_models_fetch_error = None;
        key.updated_at_unix_secs = Some(updated_at_unix_secs.unwrap_or_else(current_unix_secs));
        if !upstream_metadata_updates.is_empty() {
            let metadata = key
                .upstream_metadata
                .get_or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .expect("upstream metadata object was validated");
            for update in upstream_metadata_updates {
                metadata.insert(update.namespace.clone(), update.value.clone());
            }
        }
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
        key.updated_at_unix_secs = Some(current_unix_secs());
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
        key.updated_at_unix_secs = Some(current_unix_secs());
        Ok(true)
    }

    async fn update_key_oauth_runtime_state(
        &self,
        key_id: &str,
        oauth_invalid_at_unix_secs: Option<u64>,
        oauth_invalid_reason: Option<&str>,
        encrypted_auth_config_update: Option<&str>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };

        key.oauth_invalid_at_unix_secs = oauth_invalid_at_unix_secs;
        key.oauth_invalid_reason = oauth_invalid_reason.map(ToOwned::to_owned);
        if let Some(encrypted_auth_config) = encrypted_auth_config_update {
            key.encrypted_auth_config = Some(encrypted_auth_config.to_string());
        }
        key.updated_at_unix_secs = Some(updated_at_unix_secs.unwrap_or_else(current_unix_secs));
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
        key.updated_at_unix_secs = Some(current_unix_secs());
        Ok(true)
    }

    async fn reset_key_error_count(&self, key_id: &str) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(key_id) else {
            return Ok(false);
        };

        key.error_count = Some(0);
        key.updated_at_unix_secs = Some(current_unix_secs());
        Ok(true)
    }

    async fn compare_and_update_key_adaptive_state(
        &self,
        update: &ProviderCatalogKeyAdaptiveStateUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }
        let patch = adaptive_status_snapshot_patch(&update.status_snapshot_patch)?;
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(&update.key_id) else {
            return Ok(false);
        };
        let expected = update.expected.canonicalized();
        let next = update.next.canonicalized();
        if ProviderCatalogKeyAdaptiveState::from(&*key) != expected {
            return Ok(false);
        }
        let status_snapshot = json_object_for_merge(
            key.status_snapshot.as_ref(),
            "provider catalog status snapshot",
        )?;
        key.learned_rpm_limit = next.learned_rpm_limit;
        key.concurrent_429_count = next.concurrent_429_count;
        key.rpm_429_count = next.rpm_429_count;
        key.last_429_at_unix_secs = next.last_429_at_unix_secs;
        key.last_429_type.clone_from(&next.last_429_type);
        key.adjustment_history.clone_from(&next.adjustment_history);
        key.utilization_samples
            .clone_from(&next.utilization_samples);
        key.last_probe_increase_at_unix_secs = next.last_probe_increase_at_unix_secs;
        key.last_rpm_peak = next.last_rpm_peak;
        key.status_snapshot = Some(Value::Object(merge_json_objects(status_snapshot, patch)));
        key.updated_at_unix_secs = Some(
            update
                .updated_at_unix_secs
                .unwrap_or_else(current_unix_secs),
        );
        Ok(true)
    }

    async fn update_key_runtime_metadata(
        &self,
        update: &ProviderCatalogKeyRuntimeMetadataUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.key_id.trim().is_empty() || update.namespace.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id and runtime metadata namespace are required".to_string(),
            ));
        }
        let status_patch = update
            .status_snapshot_patch
            .as_object()
            .cloned()
            .ok_or_else(|| {
                DataLayerError::InvalidInput(
                    "provider catalog runtime status snapshot patch must be an object".to_string(),
                )
            })?;
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(&update.key_id) else {
            return Ok(false);
        };
        let current_namespace = key
            .upstream_metadata
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get(&update.namespace))
            .cloned();
        if current_namespace != update.expected_upstream_metadata_value {
            return Ok(false);
        }
        let mut metadata = json_object_for_merge(
            key.upstream_metadata.as_ref(),
            "provider catalog upstream metadata",
        )?;
        let status_snapshot = json_object_for_merge(
            key.status_snapshot.as_ref(),
            "provider catalog status snapshot",
        )?;
        metadata.insert(
            update.namespace.clone(),
            update.upstream_metadata_value.clone(),
        );
        key.upstream_metadata = Some(Value::Object(metadata));
        key.status_snapshot = Some(Value::Object(merge_json_objects(
            status_snapshot,
            status_patch,
        )));
        key.updated_at_unix_secs = Some(
            update
                .updated_at_unix_secs
                .unwrap_or_else(current_unix_secs),
        );
        Ok(true)
    }

    async fn update_key_status_snapshot(
        &self,
        update: &ProviderCatalogKeyStatusSnapshotUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }
        let patch = update
            .status_snapshot_patch
            .as_object()
            .cloned()
            .ok_or_else(|| {
                DataLayerError::InvalidInput(
                    "provider catalog status snapshot patch must be an object".to_string(),
                )
            })?;
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(&update.key_id) else {
            return Ok(false);
        };
        let status_snapshot = json_object_for_merge(
            key.status_snapshot.as_ref(),
            "provider catalog status snapshot",
        )?;
        key.status_snapshot = Some(Value::Object(merge_json_objects(status_snapshot, patch)));
        key.updated_at_unix_secs = Some(
            update
                .updated_at_unix_secs
                .unwrap_or_else(current_unix_secs),
        );
        Ok(true)
    }

    async fn compare_and_update_key_health_state(
        &self,
        update: &ProviderCatalogKeyHealthStateUpdate,
    ) -> Result<bool, DataLayerError> {
        if update.key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "provider catalog key_id is empty".to_string(),
            ));
        }
        let mut index = self
            .index
            .write()
            .expect("provider catalog repository lock");
        let Some(key) = index.keys.get_mut(&update.key_id) else {
            return Ok(false);
        };
        if key.health_by_format != update.expected_health_by_format
            || key.circuit_breaker_by_format != update.expected_circuit_breaker_by_format
        {
            return Ok(false);
        }
        key.health_by_format.clone_from(&update.health_by_format);
        key.circuit_breaker_by_format
            .clone_from(&update.circuit_breaker_by_format);
        key.updated_at_unix_secs = Some(current_unix_secs());
        Ok(true)
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn merge_admin_key_update(
    stored: &StoredProviderCatalogKey,
    requested: &StoredProviderCatalogKey,
) -> StoredProviderCatalogKey {
    let mut merged = requested.clone();

    // Catalog edits own configuration, not live observations. Keep operational
    // fields from the current row so stale admin snapshots cannot undo runtime writes.
    merged.learned_rpm_limit = stored.learned_rpm_limit;
    merged.concurrent_429_count = stored.concurrent_429_count;
    merged.rpm_429_count = stored.rpm_429_count;
    merged.last_429_at_unix_secs = stored.last_429_at_unix_secs;
    merged.last_429_type.clone_from(&stored.last_429_type);
    merged
        .adjustment_history
        .clone_from(&stored.adjustment_history);
    merged
        .utilization_samples
        .clone_from(&stored.utilization_samples);
    merged.last_probe_increase_at_unix_secs = stored.last_probe_increase_at_unix_secs;
    merged.last_rpm_peak = stored.last_rpm_peak;
    merged.last_models_fetch_at_unix_secs = stored.last_models_fetch_at_unix_secs;
    merged
        .last_models_fetch_error
        .clone_from(&stored.last_models_fetch_error);
    merged
        .upstream_metadata
        .clone_from(&stored.upstream_metadata);
    merged.oauth_invalid_at_unix_secs = stored.oauth_invalid_at_unix_secs;
    merged
        .oauth_invalid_reason
        .clone_from(&stored.oauth_invalid_reason);
    merged.status_snapshot.clone_from(&stored.status_snapshot);
    merged.health_by_format.clone_from(&stored.health_by_format);
    merged
        .circuit_breaker_by_format
        .clone_from(&stored.circuit_breaker_by_format);
    merged.request_count = stored.request_count;
    merged.total_tokens = stored.total_tokens;
    merged.total_cost_usd = stored.total_cost_usd;
    merged.success_count = stored.success_count;
    merged.error_count = stored.error_count;
    merged.total_response_time_ms = stored.total_response_time_ms;
    merged.last_used_at_unix_secs = stored.last_used_at_unix_secs;
    merged.created_at_unix_ms = stored.created_at_unix_ms;
    merged
}

fn adaptive_status_snapshot_patch(patch: &Value) -> Result<Map<String, Value>, DataLayerError> {
    const OWNED_FIELDS: [&str; 6] = [
        "observation_count",
        "header_observation_count",
        "latest_upstream_limit",
        "learning_confidence",
        "enforcement_active",
        "known_boundary",
    ];
    let object = patch.as_object().ok_or_else(|| {
        DataLayerError::InvalidInput(
            "provider catalog adaptive status snapshot patch must be an object".to_string(),
        )
    })?;
    Ok(OWNED_FIELDS
        .into_iter()
        .filter_map(|field| {
            object
                .get(field)
                .cloned()
                .map(|value| (field.to_string(), value))
        })
        .collect())
}

fn json_object_for_merge(
    value: Option<&Value>,
    field_name: &str,
) -> Result<Map<String, Value>, DataLayerError> {
    match value {
        None => Ok(Map::new()),
        Some(Value::Object(object)) => Ok(object.clone()),
        Some(_) => Err(DataLayerError::UnexpectedValue(format!(
            "{field_name} must be an object"
        ))),
    }
}

fn merge_json_objects(
    mut current: Map<String, Value>,
    patch: Map<String, Value>,
) -> Map<String, Value> {
    current.extend(patch);
    current
}

#[cfg(test)]
mod tests {
    use super::InMemoryProviderCatalogReadRepository;
    use crate::repository::provider_catalog::{
        ProviderCatalogKeyAdaptiveState, ProviderCatalogKeyAdaptiveStateUpdate,
        ProviderCatalogKeyHealthStateUpdate, ProviderCatalogKeyListOrder,
        ProviderCatalogKeyListQuery, ProviderCatalogKeyRuntimeMetadataUpdate,
        ProviderCatalogReadRepository, ProviderCatalogWriteRepository,
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use crate::repository::usage::ProviderApiKeyUsageDelta;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tokio::sync::Barrier;

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
    async fn materializes_codex_window_usage_stats_delta_in_memory() {
        let mut key = sample_key("key-1", "provider-1");
        key.status_snapshot = Some(json!({
            "quota": {
                "provider_type": "codex",
                "windows": [
                    {
                        "code": "5h",
                        "reset_at": 120_000u64,
                        "window_minutes": 300u64,
                        "usage": {
                            "request_count": 1,
                            "total_tokens": 10,
                            "total_cost_usd": "0.10000000"
                        }
                    },
                    {
                        "code": "weekly",
                        "reset_at": 700_000u64
                    }
                ]
            }
        }));
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![key],
        );

        repository.apply_usage_stats_delta(
            "key-1",
            &ProviderApiKeyUsageDelta {
                request_count: 2,
                total_tokens: 25,
                total_cost_usd: 0.25,
                usage_created_at_unix_secs: Some(110_000),
                ..ProviderApiKeyUsageDelta::default()
            },
            None,
        );

        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("keys should read");
        let windows = stored[0].status_snapshot.as_ref().expect("snapshot")["quota"]["windows"]
            .as_array()
            .expect("windows");
        let five_h = windows
            .iter()
            .find(|window| window["code"] == json!("5h"))
            .expect("5h window should exist");
        let weekly = windows
            .iter()
            .find(|window| window["code"] == json!("weekly"))
            .expect("weekly window should exist");

        assert_eq!(five_h["usage"]["request_count"], json!(3));
        assert_eq!(five_h["usage"]["total_tokens"], json!(35));
        assert_eq!(five_h["usage"]["total_cost_usd"], json!("0.35000000"));
        assert_eq!(weekly["usage"]["request_count"], json!(2));
        assert_eq!(weekly["usage"]["total_tokens"], json!(25));
        assert_eq!(weekly["usage"]["total_cost_usd"], json!("0.25000000"));
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
    async fn paginates_provider_keys_by_pool_sort_fields() {
        let mut old = sample_key("key-1", "provider-1");
        old.name = "old".to_string();
        old.created_at_unix_ms = Some(10);
        old.last_used_at_unix_secs = Some(30);
        let mut fresh = sample_key("key-2", "provider-1");
        fresh.name = "fresh".to_string();
        fresh.created_at_unix_ms = Some(20);
        fresh.last_used_at_unix_secs = Some(10);
        let mut unused = sample_key("key-3", "provider-1");
        unused.name = "unused".to_string();
        unused.created_at_unix_ms = None;
        unused.last_used_at_unix_secs = None;
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![old, fresh, unused],
        );

        let imported = repository
            .list_keys_page(&ProviderCatalogKeyListQuery {
                provider_id: "provider-1".to_string(),
                search: None,
                is_active: None,
                offset: 0,
                limit: 2,
                order: ProviderCatalogKeyListOrder::CreatedAtDesc,
            })
            .await
            .expect("keys should page");

        assert_eq!(imported.total, 3);
        assert_eq!(
            imported
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-2", "key-1"]
        );

        let last_used = repository
            .list_keys_page(&ProviderCatalogKeyListQuery {
                provider_id: "provider-1".to_string(),
                search: None,
                is_active: None,
                offset: 0,
                limit: 3,
                order: ProviderCatalogKeyListOrder::LastUsedAtDesc,
            })
            .await
            .expect("keys should page");

        assert_eq!(
            last_used
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-1", "key-2", "key-3"]
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
    async fn runtime_health_cas_preserves_admin_activation_and_rejects_stale_state() {
        let mut key = sample_key("key-1", "provider-1");
        key.is_active = false;
        key.health_by_format = Some(json!({"openai:chat":{"consecutive_failures":1}}));
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![key],
        );
        let update = ProviderCatalogKeyHealthStateUpdate {
            key_id: "key-1".to_string(),
            expected_health_by_format: Some(json!({"openai:chat":{"consecutive_failures":1}})),
            expected_circuit_breaker_by_format: None,
            health_by_format: Some(json!({"openai:chat":{"consecutive_failures":2}})),
            circuit_breaker_by_format: None,
        };

        assert!(repository
            .compare_and_update_key_health_state(&update)
            .await
            .expect("health CAS should succeed"));
        assert!(!repository
            .compare_and_update_key_health_state(&update)
            .await
            .expect("stale health CAS should report a conflict"));

        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert!(!stored.is_active);
        assert_eq!(
            stored.health_by_format,
            Some(json!({"openai:chat":{"consecutive_failures":2}}))
        );
    }

    #[tokio::test]
    async fn adaptive_cas_detects_conflicts_and_merges_only_owned_status_fields() {
        let mut key = sample_key("key-1", "provider-1");
        key.learned_rpm_limit = Some(10);
        key.rpm_429_count = Some(1);
        key.status_snapshot = Some(json!({
            "quota": {"remaining": 9},
            "oauth": {"invalid": false},
            "observation_count": 1
        }));
        let expected = ProviderCatalogKeyAdaptiveState::from(&key);
        let mut next = expected.clone();
        next.learned_rpm_limit = Some(8);
        next.rpm_429_count = Some(2);
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![key],
        );
        let update = ProviderCatalogKeyAdaptiveStateUpdate {
            key_id: "key-1".to_string(),
            expected,
            next,
            status_snapshot_patch: json!({
                "observation_count": 2,
                "learning_confidence": 0.5,
                "quota": {"remaining": 0}
            }),
            updated_at_unix_secs: Some(10),
        };

        assert!(repository
            .compare_and_update_key_adaptive_state(&update)
            .await
            .expect("adaptive CAS should succeed"));
        assert!(!repository
            .compare_and_update_key_adaptive_state(&update)
            .await
            .expect("stale adaptive CAS should report a conflict"));

        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        let status = stored.status_snapshot.expect("status should exist");
        assert_eq!(stored.learned_rpm_limit, Some(8));
        assert_eq!(stored.rpm_429_count, Some(2));
        assert_eq!(status["quota"], json!({"remaining": 9}));
        assert_eq!(status["oauth"], json!({"invalid": false}));
        assert_eq!(status["observation_count"], json!(2));
        assert_eq!(status["learning_confidence"], json!(0.5));
    }

    #[tokio::test]
    async fn runtime_metadata_update_preserves_adaptive_state_and_other_namespaces() {
        let mut key = sample_key("key-1", "provider-1");
        key.learned_rpm_limit = Some(12);
        key.upstream_metadata = Some(json!({
            "codex": {"remaining": 5},
            "grok": {"remaining": 7}
        }));
        key.status_snapshot = Some(json!({
            "quota": {"remaining": 5},
            "observation_count": 4,
            "oauth": {"invalid": false}
        }));
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![key],
        );

        assert!(repository
            .update_key_runtime_metadata(&ProviderCatalogKeyRuntimeMetadataUpdate {
                key_id: "key-1".to_string(),
                namespace: "codex".to_string(),
                expected_upstream_metadata_value: Some(json!({"remaining": 5})),
                upstream_metadata_value: json!({"remaining": 3}),
                status_snapshot_patch: json!({"quota":{"remaining":3}}),
                updated_at_unix_secs: Some(20),
            })
            .await
            .expect("runtime metadata should update"));

        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.learned_rpm_limit, Some(12));
        assert_eq!(
            stored.upstream_metadata,
            Some(json!({
                "codex": {"remaining": 3},
                "grok": {"remaining": 7}
            }))
        );
        let status = stored.status_snapshot.expect("status should exist");
        assert_eq!(status["quota"], json!({"remaining": 3}));
        assert_eq!(status["observation_count"], json!(4));
        assert_eq!(status["oauth"], json!({"invalid": false}));
    }

    #[tokio::test]
    async fn runtime_metadata_namespace_cas_serializes_concurrent_read_modify_writes() {
        let mut key = sample_key("key-1", "provider-1");
        key.upstream_metadata = Some(json!({"grok": {"remaining": 10, "updates": 0}}));
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![key],
        ));
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let repository = Arc::clone(&repository);
            let barrier = Arc::clone(&barrier);
            handles.push(tokio::spawn(async move {
                let observed = repository
                    .list_keys_by_ids(&["key-1".to_string()])
                    .await
                    .expect("key should read")
                    .pop()
                    .expect("key should exist");
                let expected = observed
                    .upstream_metadata
                    .as_ref()
                    .and_then(Value::as_object)
                    .and_then(|metadata| metadata.get("grok"))
                    .cloned();
                let mut next = expected
                    .as_ref()
                    .and_then(Value::as_object)
                    .cloned()
                    .expect("grok namespace should be an object");
                let updates = next
                    .get("updates")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                next.insert("updates".to_string(), json!(updates + 1));
                barrier.wait().await;

                let first = repository
                    .update_key_runtime_metadata(&ProviderCatalogKeyRuntimeMetadataUpdate {
                        key_id: "key-1".to_string(),
                        namespace: "grok".to_string(),
                        expected_upstream_metadata_value: expected,
                        upstream_metadata_value: Value::Object(next),
                        status_snapshot_patch: json!({}),
                        updated_at_unix_secs: Some(100),
                    })
                    .await
                    .expect("CAS should run");
                if first {
                    return (true, true);
                }

                // A stale writer must reload and retry from the winner's value;
                // otherwise one of two concurrent quota deltas is lost.
                let refreshed = repository
                    .list_keys_by_ids(&["key-1".to_string()])
                    .await
                    .expect("key should reload")
                    .pop()
                    .expect("key should exist");
                let expected = refreshed
                    .upstream_metadata
                    .as_ref()
                    .and_then(Value::as_object)
                    .and_then(|metadata| metadata.get("grok"))
                    .cloned();
                let mut next = expected
                    .as_ref()
                    .and_then(Value::as_object)
                    .cloned()
                    .expect("grok namespace should remain an object");
                let updates = next
                    .get("updates")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                next.insert("updates".to_string(), json!(updates + 1));
                let retried = repository
                    .update_key_runtime_metadata(&ProviderCatalogKeyRuntimeMetadataUpdate {
                        key_id: "key-1".to_string(),
                        namespace: "grok".to_string(),
                        expected_upstream_metadata_value: expected,
                        upstream_metadata_value: Value::Object(next),
                        status_snapshot_patch: json!({}),
                        updated_at_unix_secs: Some(101),
                    })
                    .await
                    .expect("retry CAS should run");
                (false, retried)
            }));
        }

        let outcomes = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|result| result.expect("CAS task should finish"))
            .collect::<Vec<_>>();
        assert_eq!(outcomes.iter().filter(|(first, _)| !*first).count(), 1);
        assert!(outcomes.iter().all(|(_, persisted)| *persisted));
        let stored = repository
            .list_keys_by_ids(&["key-1".to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(
            stored.upstream_metadata.unwrap()["grok"]["updates"],
            json!(2)
        );
    }

    #[tokio::test]
    async fn runtime_metadata_namespace_cas_distinguishes_missing_and_stale_values() {
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![sample_key("key-1", "provider-1")],
        );
        let create = ProviderCatalogKeyRuntimeMetadataUpdate {
            key_id: "key-1".to_string(),
            namespace: "new_namespace".to_string(),
            expected_upstream_metadata_value: None,
            upstream_metadata_value: json!({"value": 1}),
            status_snapshot_patch: json!({}),
            updated_at_unix_secs: Some(1),
        };
        assert!(repository
            .update_key_runtime_metadata(&create)
            .await
            .expect("missing namespace CAS should succeed"));
        assert!(!repository
            .update_key_runtime_metadata(&create)
            .await
            .expect("stale missing namespace CAS should conflict"));
        let stale = ProviderCatalogKeyRuntimeMetadataUpdate {
            expected_upstream_metadata_value: Some(json!({"value": 1})),
            upstream_metadata_value: json!({"value": 2}),
            ..create
        };
        assert!(repository
            .update_key_runtime_metadata(&stale)
            .await
            .expect("matching namespace CAS should succeed"));
    }

    #[tokio::test]
    async fn stale_admin_update_preserves_concurrent_runtime_owned_fields() {
        let mut key = sample_key("key-1", "provider-1");
        key.learned_rpm_limit = Some(10);
        key.rpm_429_count = Some(1);
        key.health_by_format = Some(json!({"openai:chat":{"consecutive_failures":1}}));
        key.upstream_metadata = Some(json!({"codex":{"remaining":5}}));
        key.status_snapshot = Some(json!({
            "quota":{"remaining":5},
            "observation_count":1
        }));
        let mut stale_admin_key = key.clone();
        let repository = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-1")],
            vec![],
            vec![key.clone()],
        );

        repository
            .compare_and_update_key_health_state(&ProviderCatalogKeyHealthStateUpdate {
                key_id: key.id.clone(),
                expected_health_by_format: key.health_by_format.clone(),
                expected_circuit_breaker_by_format: None,
                health_by_format: Some(json!({"openai:chat":{"consecutive_failures":2}})),
                circuit_breaker_by_format: None,
            })
            .await
            .expect("health CAS should run");
        let expected = ProviderCatalogKeyAdaptiveState::from(&key);
        let mut next = expected.clone();
        next.learned_rpm_limit = Some(8);
        next.rpm_429_count = Some(2);
        repository
            .compare_and_update_key_adaptive_state(&ProviderCatalogKeyAdaptiveStateUpdate {
                key_id: key.id.clone(),
                expected,
                next,
                status_snapshot_patch: json!({"observation_count":2}),
                updated_at_unix_secs: Some(20),
            })
            .await
            .expect("adaptive CAS should run");
        repository
            .update_key_runtime_metadata(&ProviderCatalogKeyRuntimeMetadataUpdate {
                key_id: key.id.clone(),
                namespace: "codex".to_string(),
                expected_upstream_metadata_value: Some(json!({"remaining": 5})),
                upstream_metadata_value: json!({"remaining":3}),
                status_snapshot_patch: json!({"quota":{"remaining":3}}),
                updated_at_unix_secs: Some(21),
            })
            .await
            .expect("runtime metadata should update");

        stale_admin_key.name = "admin-renamed".to_string();
        stale_admin_key.is_active = false;
        repository
            .update_key(&stale_admin_key)
            .await
            .expect("admin update should succeed");

        let stored = repository
            .list_keys_by_ids(&[key.id])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.name, "admin-renamed");
        assert!(!stored.is_active);
        assert_eq!(stored.learned_rpm_limit, Some(8));
        assert_eq!(stored.rpm_429_count, Some(2));
        assert_eq!(
            stored.health_by_format,
            Some(json!({"openai:chat":{"consecutive_failures":2}}))
        );
        assert_eq!(
            stored.upstream_metadata,
            Some(json!({"codex":{"remaining":3}}))
        );
        assert_eq!(
            stored.status_snapshot,
            Some(json!({"quota":{"remaining":3},"observation_count":2}))
        );
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
