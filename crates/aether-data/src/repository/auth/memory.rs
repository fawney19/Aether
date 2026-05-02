use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use super::types::{
    AuthApiKeyExportSummary, AuthApiKeyLookupKey, AuthApiKeyReadRepository,
    AuthApiKeyWriteRepository, CreateStandaloneApiKeyRecord, CreateUserApiKeyRecord,
    StandaloneApiKeyExportListQuery, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
    UpdateStandaloneApiKeyBasicRecord, UpdateUserApiKeyBasicRecord,
};
use crate::repository::usage::{ApiKeyUsageContribution, ApiKeyUsageDelta};
use crate::DataLayerError;

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Default)]
struct MemoryAuthApiKeyIndex {
    by_api_key_id: BTreeMap<String, StoredAuthApiKeySnapshot>,
    export_by_api_key_id: BTreeMap<String, StoredAuthApiKeyExportRecord>,
    by_key_hash: BTreeMap<String, String>,
    touch_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Default)]
pub struct InMemoryAuthApiKeySnapshotRepository {
    index: RwLock<MemoryAuthApiKeyIndex>,
}

impl InMemoryAuthApiKeySnapshotRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = (Option<String>, StoredAuthApiKeySnapshot)>,
    {
        let mut by_api_key_id = BTreeMap::new();
        let mut export_by_api_key_id = BTreeMap::new();
        let mut by_key_hash = BTreeMap::new();
        for (key_hash, snapshot) in items {
            let derived_key_hash = key_hash
                .clone()
                .unwrap_or_else(|| format!("memory-{}", snapshot.api_key_id));
            export_by_api_key_id.insert(
                snapshot.api_key_id.clone(),
                StoredAuthApiKeyExportRecord::new(
                    snapshot.user_id.clone(),
                    snapshot.api_key_id.clone(),
                    derived_key_hash.clone(),
                    None,
                    snapshot.api_key_name.clone(),
                    snapshot
                        .api_key_allowed_providers
                        .as_ref()
                        .map(|value| serde_json::json!(value)),
                    snapshot
                        .api_key_allowed_api_formats
                        .as_ref()
                        .map(|value| serde_json::json!(value)),
                    snapshot
                        .api_key_allowed_models
                        .as_ref()
                        .map(|value| serde_json::json!(value)),
                    snapshot.api_key_rate_limit,
                    snapshot.api_key_concurrent_limit,
                    None,
                    snapshot.api_key_is_active,
                    snapshot
                        .api_key_expires_at_unix_secs
                        .map(|value| value as i64),
                    false,
                    0,
                    0,
                    0.0,
                    snapshot.api_key_is_standalone,
                )
                .expect("derived auth api key export record should build"),
            );
            if let Some(key_hash) = key_hash {
                by_key_hash.insert(key_hash, snapshot.api_key_id.clone());
            }
            by_api_key_id.insert(snapshot.api_key_id.clone(), snapshot);
        }
        Self {
            index: RwLock::new(MemoryAuthApiKeyIndex {
                by_api_key_id,
                export_by_api_key_id,
                by_key_hash,
                touch_counts: BTreeMap::new(),
            }),
        }
    }

    pub fn with_export_records<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredAuthApiKeyExportRecord>,
    {
        let index = self
            .index
            .get_mut()
            .expect("auth api key snapshot repository lock");
        for item in items {
            index
                .export_by_api_key_id
                .insert(item.api_key_id.clone(), item);
        }
        self
    }

    pub fn touch_count(&self, api_key_id: &str) -> usize {
        self.index
            .read()
            .expect("auth api key snapshot repository lock")
            .touch_counts
            .get(api_key_id)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn apply_usage_stats_delta(
        &self,
        api_key_id: &str,
        delta: &ApiKeyUsageDelta,
        _recomputed_last_used_at_unix_secs: Option<u64>,
    ) {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(record) = index.export_by_api_key_id.get_mut(api_key_id) else {
            return;
        };

        record.total_requests = apply_i64_delta_to_u64(record.total_requests, delta.total_requests);
        record.total_tokens = apply_i64_delta_to_u64(record.total_tokens, delta.total_tokens);
        record.total_cost_usd = apply_f64_delta(record.total_cost_usd, delta.total_cost_usd);
    }

    pub(crate) fn rebuild_usage_stats(
        &self,
        contributions: &BTreeMap<String, ApiKeyUsageContribution>,
    ) {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        for record in index.export_by_api_key_id.values_mut() {
            record.total_requests = 0;
            record.total_tokens = 0;
            record.total_cost_usd = 0.0;
        }

        for (api_key_id, contribution) in contributions {
            let Some(record) = index.export_by_api_key_id.get_mut(api_key_id) else {
                continue;
            };
            record.total_requests = clamp_i64_to_u64(contribution.total_requests);
            record.total_tokens = clamp_i64_to_u64(contribution.total_tokens);
            record.total_cost_usd = contribution.total_cost_usd.max(0.0);
        }
    }
}

fn clamp_i64_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn apply_i64_delta_to_u64(current: u64, delta: i64) -> u64 {
    clamp_i64_to_u64(
        i64::try_from(current)
            .unwrap_or(i64::MAX)
            .saturating_add(delta),
    )
}

fn apply_f64_delta(current: f64, delta: f64) -> f64 {
    let next = current + delta;
    if next.is_finite() {
        next.max(0.0)
    } else {
        current.max(0.0)
    }
}

#[async_trait]
impl AuthApiKeyReadRepository for InMemoryAuthApiKeySnapshotRepository {
    async fn find_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(match key {
            AuthApiKeyLookupKey::KeyHash(key_hash) => index
                .by_key_hash
                .get(key_hash)
                .and_then(|api_key_id| index.by_api_key_id.get(api_key_id))
                .cloned(),
            AuthApiKeyLookupKey::ApiKeyId(api_key_id) => {
                index.by_api_key_id.get(api_key_id).cloned()
            }
            AuthApiKeyLookupKey::UserApiKeyIds {
                user_id,
                api_key_id,
            } => index
                .by_api_key_id
                .get(api_key_id)
                .filter(|snapshot| snapshot.user_id == user_id)
                .cloned(),
        })
    }

    async fn list_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(api_key_ids
            .iter()
            .filter_map(|api_key_id| index.by_api_key_id.get(api_key_id).cloned())
            .collect())
    }

    async fn list_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(index
            .export_by_api_key_id
            .values()
            .filter(|record| {
                !record.is_standalone && user_ids.iter().any(|id| id == &record.user_id)
            })
            .cloned()
            .collect())
    }

    async fn list_export_api_keys_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(api_key_ids
            .iter()
            .filter_map(|api_key_id| index.export_by_api_key_id.get(api_key_id).cloned())
            .collect())
    }

    async fn list_export_api_keys_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let name_search = name_search.trim().to_ascii_lowercase();
        if name_search.is_empty() {
            return Ok(Vec::new());
        }

        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(index
            .export_by_api_key_id
            .values()
            .filter(|record| {
                record
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&name_search)
            })
            .cloned()
            .collect())
    }

    async fn list_export_standalone_api_keys_page(
        &self,
        query: &StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(index
            .export_by_api_key_id
            .values()
            .filter(|record| {
                record.is_standalone
                    && query
                        .is_active
                        .is_none_or(|is_active| record.is_active == is_active)
            })
            .skip(query.skip)
            .take(query.limit)
            .cloned()
            .collect())
    }

    async fn count_export_standalone_api_keys(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(index
            .export_by_api_key_id
            .values()
            .filter(|record| {
                record.is_standalone
                    && is_active.is_none_or(|expected| record.is_active == expected)
            })
            .count() as u64)
    }

    async fn summarize_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        let mut summary = AuthApiKeyExportSummary::default();
        for record in index.export_by_api_key_id.values().filter(|record| {
            !record.is_standalone && user_ids.iter().any(|id| id == &record.user_id)
        }) {
            summary.total = summary.total.saturating_add(1);
            if record.is_active
                && record
                    .expires_at_unix_secs
                    .is_none_or(|expires_at_unix_secs| expires_at_unix_secs >= now_unix_secs)
            {
                summary.active = summary.active.saturating_add(1);
            }
        }
        Ok(summary)
    }

    async fn summarize_export_non_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        let mut summary = AuthApiKeyExportSummary::default();
        for record in index
            .export_by_api_key_id
            .values()
            .filter(|record| !record.is_standalone)
        {
            summary.total = summary.total.saturating_add(1);
            if record.is_active
                && record
                    .expires_at_unix_secs
                    .is_none_or(|expires_at_unix_secs| expires_at_unix_secs >= now_unix_secs)
            {
                summary.active = summary.active.saturating_add(1);
            }
        }
        Ok(summary)
    }

    async fn find_export_standalone_api_key_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(index
            .export_by_api_key_id
            .get(api_key_id)
            .filter(|record| record.is_standalone)
            .cloned())
    }

    async fn summarize_export_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        let mut summary = AuthApiKeyExportSummary::default();
        for record in index
            .export_by_api_key_id
            .values()
            .filter(|record| record.is_standalone)
        {
            summary.total = summary.total.saturating_add(1);
            if record.is_active
                && record
                    .expires_at_unix_secs
                    .is_none_or(|expires_at_unix_secs| expires_at_unix_secs >= now_unix_secs)
            {
                summary.active = summary.active.saturating_add(1);
            }
        }
        Ok(summary)
    }

    async fn list_export_standalone_api_keys(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let index = self
            .index
            .read()
            .expect("auth api key snapshot repository lock");
        Ok(index
            .export_by_api_key_id
            .values()
            .filter(|record| record.is_standalone)
            .cloned()
            .collect())
    }
}

#[async_trait]
impl AuthApiKeyWriteRepository for InMemoryAuthApiKeySnapshotRepository {
    async fn touch_last_used_at(&self, api_key_id: &str) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        if !index.by_api_key_id.contains_key(api_key_id) {
            return Ok(false);
        }
        let counter = index
            .touch_counts
            .entry(api_key_id.to_string())
            .or_insert(0);
        *counter += 1;
        Ok(true)
    }

    async fn create_user_api_key(
        &self,
        record: CreateUserApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        if index.by_api_key_id.contains_key(&record.api_key_id) {
            return Err(DataLayerError::UnexpectedValue(format!(
                "duplicate api_keys.id: {}",
                record.api_key_id
            )));
        }
        if index.by_key_hash.contains_key(&record.key_hash) {
            return Err(DataLayerError::UnexpectedValue(format!(
                "duplicate api_keys.key_hash: {}",
                record.key_hash
            )));
        }

        let template = index
            .by_api_key_id
            .values()
            .find(|snapshot| snapshot.user_id == record.user_id)
            .cloned();
        let snapshot = if let Some(template) = template {
            StoredAuthApiKeySnapshot {
                api_key_id: record.api_key_id.clone(),
                api_key_name: record.name.clone(),
                api_key_is_active: record.is_active,
                api_key_is_locked: false,
                api_key_is_standalone: false,
                api_key_rate_limit: Some(record.rate_limit),
                api_key_concurrent_limit: record.concurrent_limit,
                api_key_expires_at_unix_secs: record.expires_at_unix_secs,
                api_key_allowed_providers: record.allowed_providers.clone(),
                api_key_allowed_api_formats: record.allowed_api_formats.clone(),
                api_key_allowed_models: record.allowed_models.clone(),
                ..template
            }
        } else {
            StoredAuthApiKeySnapshot::new(
                record.user_id.clone(),
                format!(
                    "user-{}",
                    &record.user_id.chars().take(8).collect::<String>()
                ),
                None,
                "user".to_string(),
                "local".to_string(),
                true,
                false,
                None,
                None,
                None,
                record.api_key_id.clone(),
                record.name.clone(),
                record.is_active,
                false,
                false,
                Some(record.rate_limit),
                record.concurrent_limit,
                record.expires_at_unix_secs.map(|value| value as i64),
                record
                    .allowed_providers
                    .as_ref()
                    .map(|value| serde_json::json!(value)),
                record
                    .allowed_api_formats
                    .as_ref()
                    .map(|value| serde_json::json!(value)),
                record
                    .allowed_models
                    .as_ref()
                    .map(|value| serde_json::json!(value)),
            )?
        };

        let now_unix_secs = current_unix_secs() as i64;
        let export = StoredAuthApiKeyExportRecord::new(
            record.user_id.clone(),
            record.api_key_id.clone(),
            record.key_hash.clone(),
            record.key_encrypted,
            record.name,
            record
                .allowed_providers
                .as_ref()
                .map(|value| serde_json::json!(value)),
            record
                .allowed_api_formats
                .as_ref()
                .map(|value| serde_json::json!(value)),
            record
                .allowed_models
                .as_ref()
                .map(|value| serde_json::json!(value)),
            Some(record.rate_limit),
            record.concurrent_limit,
            record.force_capabilities,
            record.is_active,
            record.expires_at_unix_secs.map(|value| value as i64),
            record.auto_delete_on_expiry,
            record.total_requests as i64,
            record.total_tokens as i64,
            record.total_cost_usd,
            false,
        )?
        .with_activity_timestamps(None, Some(now_unix_secs), Some(now_unix_secs))?;

        index
            .by_key_hash
            .insert(record.key_hash, record.api_key_id.clone());
        index
            .by_api_key_id
            .insert(record.api_key_id.clone(), snapshot);
        index
            .export_by_api_key_id
            .insert(record.api_key_id, export.clone());
        Ok(Some(export))
    }

    async fn create_standalone_api_key(
        &self,
        record: CreateStandaloneApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        if index.by_api_key_id.contains_key(&record.api_key_id) {
            return Err(DataLayerError::UnexpectedValue(format!(
                "duplicate api_keys.id: {}",
                record.api_key_id
            )));
        }
        if index.by_key_hash.contains_key(&record.key_hash) {
            return Err(DataLayerError::UnexpectedValue(format!(
                "duplicate api_keys.key_hash: {}",
                record.key_hash
            )));
        }

        let template = index
            .by_api_key_id
            .values()
            .find(|snapshot| snapshot.user_id == record.user_id)
            .cloned();
        let snapshot = if let Some(template) = template {
            StoredAuthApiKeySnapshot {
                api_key_id: record.api_key_id.clone(),
                api_key_name: record.name.clone(),
                api_key_is_active: record.is_active,
                api_key_is_locked: false,
                api_key_is_standalone: true,
                api_key_rate_limit: record.rate_limit,
                api_key_concurrent_limit: record.concurrent_limit,
                api_key_expires_at_unix_secs: record.expires_at_unix_secs,
                api_key_allowed_providers: record.allowed_providers.clone(),
                api_key_allowed_api_formats: record.allowed_api_formats.clone(),
                api_key_allowed_models: record.allowed_models.clone(),
                ..template
            }
        } else {
            StoredAuthApiKeySnapshot::new(
                record.user_id.clone(),
                format!(
                    "admin-{}",
                    &record.user_id.chars().take(8).collect::<String>()
                ),
                None,
                "admin".to_string(),
                "local".to_string(),
                true,
                false,
                None,
                None,
                None,
                record.api_key_id.clone(),
                record.name.clone(),
                record.is_active,
                false,
                true,
                record.rate_limit,
                record.concurrent_limit,
                record.expires_at_unix_secs.map(|value| value as i64),
                record
                    .allowed_providers
                    .as_ref()
                    .map(|value| serde_json::json!(value)),
                record
                    .allowed_api_formats
                    .as_ref()
                    .map(|value| serde_json::json!(value)),
                record
                    .allowed_models
                    .as_ref()
                    .map(|value| serde_json::json!(value)),
            )?
        };

        let now_unix_secs = current_unix_secs() as i64;
        let export = StoredAuthApiKeyExportRecord::new(
            record.user_id.clone(),
            record.api_key_id.clone(),
            record.key_hash.clone(),
            record.key_encrypted,
            record.name,
            record
                .allowed_providers
                .as_ref()
                .map(|value| serde_json::json!(value)),
            record
                .allowed_api_formats
                .as_ref()
                .map(|value| serde_json::json!(value)),
            record
                .allowed_models
                .as_ref()
                .map(|value| serde_json::json!(value)),
            record.rate_limit,
            record.concurrent_limit,
            record.force_capabilities,
            record.is_active,
            record.expires_at_unix_secs.map(|value| value as i64),
            record.auto_delete_on_expiry,
            record.total_requests as i64,
            record.total_tokens as i64,
            record.total_cost_usd,
            true,
        )?
        .with_activity_timestamps(None, Some(now_unix_secs), Some(now_unix_secs))?;

        index
            .by_key_hash
            .insert(record.key_hash, record.api_key_id.clone());
        index
            .by_api_key_id
            .insert(record.api_key_id.clone(), snapshot);
        index
            .export_by_api_key_id
            .insert(record.api_key_id, export.clone());
        Ok(Some(export))
    }

    async fn update_user_api_key_basic(
        &self,
        record: UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(&record.api_key_id) else {
            return Ok(None);
        };
        if snapshot.user_id != record.user_id || snapshot.api_key_is_standalone {
            return Ok(None);
        }
        if let Some(name) = record.name {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_name = Some(name.clone());
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.name = Some(name);
            }
        }
        if let Some(rate_limit) = record.rate_limit {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_rate_limit = Some(rate_limit);
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.rate_limit = Some(rate_limit);
            }
        }
        if let Some(concurrent_limit) = record.concurrent_limit {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_concurrent_limit = Some(concurrent_limit);
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.concurrent_limit = Some(concurrent_limit);
            }
        }
        Ok(index.export_by_api_key_id.get(&record.api_key_id).cloned())
    }

    async fn update_standalone_api_key_basic(
        &self,
        record: UpdateStandaloneApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(&record.api_key_id) else {
            return Ok(None);
        };
        if !snapshot.api_key_is_standalone {
            return Ok(None);
        }
        if let Some(name) = record.name {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_name = Some(name.clone());
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.name = Some(name);
            }
        }
        if record.rate_limit_present {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_rate_limit = record.rate_limit;
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.rate_limit = record.rate_limit;
            }
        }
        if record.concurrent_limit_present {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_concurrent_limit = record.concurrent_limit;
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.concurrent_limit = record.concurrent_limit;
            }
        }
        if let Some(allowed_providers) = record.allowed_providers {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_allowed_providers = allowed_providers.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.allowed_providers = allowed_providers;
            }
        }
        if let Some(allowed_api_formats) = record.allowed_api_formats {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_allowed_api_formats = allowed_api_formats.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.allowed_api_formats = allowed_api_formats;
            }
        }
        if let Some(allowed_models) = record.allowed_models {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_allowed_models = allowed_models.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.allowed_models = allowed_models;
            }
        }
        if record.expires_at_present {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_expires_at_unix_secs = record.expires_at_unix_secs;
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.expires_at_unix_secs = record.expires_at_unix_secs;
            }
        }
        if record.auto_delete_on_expiry_present {
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.auto_delete_on_expiry = record.auto_delete_on_expiry;
            }
        }
        Ok(index.export_by_api_key_id.get(&record.api_key_id).cloned())
    }

    async fn set_user_api_key_active(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(api_key_id) else {
            return Ok(None);
        };
        if snapshot.user_id != user_id || snapshot.api_key_is_standalone {
            return Ok(None);
        }
        if let Some(snapshot) = index.by_api_key_id.get_mut(api_key_id) {
            snapshot.api_key_is_active = is_active;
        }
        if let Some(export) = index.export_by_api_key_id.get_mut(api_key_id) {
            export.is_active = is_active;
        }
        Ok(index.export_by_api_key_id.get(api_key_id).cloned())
    }

    async fn set_standalone_api_key_active(
        &self,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(api_key_id) else {
            return Ok(None);
        };
        if !snapshot.api_key_is_standalone {
            return Ok(None);
        }
        if let Some(snapshot) = index.by_api_key_id.get_mut(api_key_id) {
            snapshot.api_key_is_active = is_active;
        }
        if let Some(export) = index.export_by_api_key_id.get_mut(api_key_id) {
            export.is_active = is_active;
        }
        Ok(index.export_by_api_key_id.get(api_key_id).cloned())
    }

    async fn set_user_api_key_locked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_locked: bool,
    ) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(api_key_id) else {
            return Ok(false);
        };
        if snapshot.user_id != user_id || snapshot.api_key_is_standalone {
            return Ok(false);
        }
        if let Some(snapshot) = index.by_api_key_id.get_mut(api_key_id) {
            snapshot.api_key_is_locked = is_locked;
        }
        Ok(true)
    }

    async fn set_user_api_key_allowed_providers(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(api_key_id) else {
            return Ok(None);
        };
        if snapshot.user_id != user_id || snapshot.api_key_is_standalone {
            return Ok(None);
        }
        if let Some(snapshot) = index.by_api_key_id.get_mut(api_key_id) {
            snapshot.api_key_allowed_providers = allowed_providers.clone();
        }
        if let Some(export) = index.export_by_api_key_id.get_mut(api_key_id) {
            export.allowed_providers = allowed_providers;
        }
        Ok(index.export_by_api_key_id.get(api_key_id).cloned())
    }

    async fn set_user_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(api_key_id) else {
            return Ok(None);
        };
        if snapshot.user_id != user_id || snapshot.api_key_is_standalone {
            return Ok(None);
        }
        let Some(export) = index.export_by_api_key_id.get_mut(api_key_id) else {
            return Ok(None);
        };
        export.force_capabilities = force_capabilities;
        Ok(Some(export.clone()))
    }

    async fn delete_user_api_key(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(api_key_id) else {
            return Ok(false);
        };
        if snapshot.user_id != user_id || snapshot.api_key_is_standalone {
            return Ok(false);
        }
        index.by_api_key_id.remove(api_key_id);
        index.export_by_api_key_id.remove(api_key_id);
        index.by_key_hash.retain(|_, value| value != api_key_id);
        index.touch_counts.remove(api_key_id);
        Ok(true)
    }

    async fn delete_standalone_api_key(&self, api_key_id: &str) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(api_key_id) else {
            return Ok(false);
        };
        if !snapshot.api_key_is_standalone {
            return Ok(false);
        }
        index.by_api_key_id.remove(api_key_id);
        index.export_by_api_key_id.remove(api_key_id);
        index.by_key_hash.retain(|_, value| value != api_key_id);
        index.touch_counts.remove(api_key_id);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryAuthApiKeySnapshotRepository;
    use crate::repository::auth::{
        AuthApiKeyLookupKey, AuthApiKeyReadRepository, AuthApiKeyWriteRepository,
        StandaloneApiKeyExportListQuery, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
        UpdateStandaloneApiKeyBasicRecord, UpdateUserApiKeyBasicRecord,
    };

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
    async fn reads_auth_snapshot_by_all_supported_keys() {
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-1".to_string()),
            sample_snapshot("key-1", "user-1"),
        )]);

        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::KeyHash("hash-1"))
            .await
            .expect("find by hash should succeed")
            .is_some());
        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-1"))
            .await
            .expect("find by api key id should succeed")
            .is_some());
        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::UserApiKeyIds {
                user_id: "user-1",
                api_key_id: "key-1",
            })
            .await
            .expect("find by user/api key ids should succeed")
            .is_some());
        let snapshots = repository
            .list_api_key_snapshots_by_ids(&["key-1".to_string(), "missing".to_string()])
            .await
            .expect("batch lookup should succeed");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].api_key_id, "key-1");
    }

    #[tokio::test]
    async fn touches_last_used_for_existing_key() {
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-1".to_string()),
            sample_snapshot("key-1", "user-1"),
        )]);

        assert!(repository
            .touch_last_used_at("key-1")
            .await
            .expect("touch should succeed"));
        assert_eq!(repository.touch_count("key-1"), 1);
        assert!(!repository
            .touch_last_used_at("missing")
            .await
            .expect("missing touch should succeed"));
    }

    #[tokio::test]
    async fn lists_export_records_for_user_bound_and_standalone_keys() {
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![
            (
                Some("hash-user".to_string()),
                sample_snapshot("key-user", "user-1"),
            ),
            (
                Some("hash-standalone".to_string()),
                sample_snapshot("key-standalone", "admin-1"),
            ),
        ])
        .with_export_records(vec![
            StoredAuthApiKeyExportRecord::new(
                "user-1".to_string(),
                "key-user".to_string(),
                "hash-user".to_string(),
                Some("enc-user".to_string()),
                Some("default".to_string()),
                Some(serde_json::json!(["openai"])),
                Some(serde_json::json!(["openai:chat"])),
                Some(serde_json::json!(["gpt-5"])),
                Some(120),
                Some(7),
                Some(serde_json::json!({"cache_1h": true})),
                true,
                Some(200),
                false,
                14,
                1_400,
                1.5,
                false,
            )
            .expect("user export record should build"),
            StoredAuthApiKeyExportRecord::new(
                "admin-1".to_string(),
                "key-standalone".to_string(),
                "hash-standalone".to_string(),
                Some("enc-standalone".to_string()),
                Some("standalone".to_string()),
                None,
                None,
                None,
                None,
                Some(1),
                None,
                true,
                None,
                true,
                2,
                25,
                0.25,
                true,
            )
            .expect("standalone export record should build"),
        ]);

        let user_records = repository
            .list_export_api_keys_by_user_ids(&["user-1".to_string()])
            .await
            .expect("user export lookup should succeed");
        assert_eq!(user_records.len(), 1);
        assert_eq!(user_records[0].api_key_id, "key-user");
        assert_eq!(user_records[0].key_encrypted.as_deref(), Some("enc-user"));
        assert_eq!(user_records[0].total_requests, 14);

        let standalone_records = repository
            .list_export_standalone_api_keys()
            .await
            .expect("standalone export lookup should succeed");
        assert_eq!(standalone_records.len(), 1);
        assert_eq!(standalone_records[0].api_key_id, "key-standalone");
        assert!(standalone_records[0].is_standalone);

        let selected_records = repository
            .list_export_api_keys_by_ids(&[
                "key-standalone".to_string(),
                "missing".to_string(),
                "key-user".to_string(),
            ])
            .await
            .expect("api key id export lookup should succeed");
        assert_eq!(selected_records.len(), 2);
        assert_eq!(selected_records[0].api_key_id, "key-standalone");
        assert_eq!(selected_records[1].api_key_id, "key-user");

        let paged_records = repository
            .list_export_standalone_api_keys_page(&StandaloneApiKeyExportListQuery {
                skip: 0,
                limit: 10,
                is_active: Some(true),
            })
            .await
            .expect("standalone export page should succeed");
        assert_eq!(paged_records.len(), 1);
        assert_eq!(paged_records[0].api_key_id, "key-standalone");
        assert_eq!(
            repository
                .count_export_standalone_api_keys(Some(true))
                .await
                .expect("standalone export count should succeed"),
            1
        );
    }

    #[tokio::test]
    async fn update_user_api_key_basic_updates_concurrent_limit() {
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-1".to_string()),
            sample_snapshot("key-1", "user-1"),
        )]);

        let updated = repository
            .update_user_api_key_basic(UpdateUserApiKeyBasicRecord {
                user_id: "user-1".to_string(),
                api_key_id: "key-1".to_string(),
                name: None,
                rate_limit: None,
                concurrent_limit: Some(11),
            })
            .await
            .expect("update should succeed")
            .expect("record should exist");
        assert_eq!(updated.concurrent_limit, Some(11));

        let snapshot = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-1"))
            .await
            .expect("find should succeed")
            .expect("snapshot should exist");
        assert_eq!(snapshot.api_key_concurrent_limit, Some(11));
    }

    #[tokio::test]
    async fn update_standalone_api_key_basic_updates_concurrent_limit_when_present() {
        let mut standalone = sample_snapshot("key-standalone", "admin-1");
        standalone.api_key_is_standalone = true;
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-standalone".to_string()),
            standalone,
        )]);

        let updated = repository
            .update_standalone_api_key_basic(UpdateStandaloneApiKeyBasicRecord {
                api_key_id: "key-standalone".to_string(),
                name: None,
                rate_limit_present: false,
                rate_limit: None,
                concurrent_limit_present: true,
                concurrent_limit: Some(13),
                allowed_providers: None,
                allowed_api_formats: None,
                allowed_models: None,
                expires_at_present: false,
                expires_at_unix_secs: None,
                auto_delete_on_expiry_present: false,
                auto_delete_on_expiry: false,
            })
            .await
            .expect("update should succeed")
            .expect("record should exist");
        assert_eq!(updated.concurrent_limit, Some(13));

        let snapshot = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-standalone"))
            .await
            .expect("find should succeed")
            .expect("snapshot should exist");
        assert_eq!(snapshot.api_key_concurrent_limit, Some(13));
    }
}
