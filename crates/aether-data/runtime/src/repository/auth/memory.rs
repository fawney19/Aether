use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use super::{
    AuthApiKeyExportSummary, AuthApiKeyLookupKey, AuthApiKeyReadRepository,
    AuthApiKeyWriteRepository, CompareAndSwapAuthApiKeyCiphertext, CreateStandaloneApiKeyRecord,
    CreateUserApiKeyRecord, StandaloneApiKeyExportListQuery, StoredAuthApiKeyExportRecord,
    StoredAuthApiKeySnapshot, UpdateStandaloneApiKeyBasicRecord, UpdateUserApiKeyBasicRecord,
};
use crate::repository::usage::{ApiKeyUsageContribution, ApiKeyUsageDelta};
use crate::repository::users::StoredUserAuthRecord;
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
    owner_by_user_id: BTreeMap<String, MemoryAuthApiKeyOwnerRegistryEntry>,
    touch_counts: BTreeMap<String, usize>,
    snapshot_lookup_counts: BTreeMap<String, usize>,
    key_hash_lookup_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryAuthApiKeyOwnerSnapshot {
    user_id: String,
    username: String,
    email: Option<String>,
    user_role: String,
    user_auth_source: String,
    user_is_active: bool,
    user_is_deleted: bool,
    user_rate_limit: Option<i32>,
    user_allowed_providers: Option<Vec<String>>,
    user_allowed_api_formats: Option<Vec<String>>,
    user_allowed_models: Option<Vec<String>>,
}

impl From<&StoredAuthApiKeySnapshot> for MemoryAuthApiKeyOwnerSnapshot {
    fn from(snapshot: &StoredAuthApiKeySnapshot) -> Self {
        Self {
            user_id: snapshot.user_id.clone(),
            username: snapshot.username.clone(),
            email: snapshot.email.clone(),
            user_role: snapshot.user_role.clone(),
            user_auth_source: snapshot.user_auth_source.clone(),
            user_is_active: snapshot.user_is_active,
            user_is_deleted: snapshot.user_is_deleted,
            user_rate_limit: snapshot.user_rate_limit,
            user_allowed_providers: snapshot.user_allowed_providers.clone(),
            user_allowed_api_formats: snapshot.user_allowed_api_formats.clone(),
            user_allowed_models: snapshot.user_allowed_models.clone(),
        }
    }
}

impl From<&StoredUserAuthRecord> for MemoryAuthApiKeyOwnerSnapshot {
    fn from(user: &StoredUserAuthRecord) -> Self {
        Self {
            user_id: user.id.clone(),
            username: user.username.clone(),
            email: user.email.clone(),
            user_role: user.role.clone(),
            user_auth_source: user.auth_source.clone(),
            user_is_active: user.is_active,
            user_is_deleted: user.is_deleted,
            user_rate_limit: None,
            user_allowed_providers: user.allowed_providers.clone(),
            user_allowed_api_formats: user.allowed_api_formats.clone(),
            user_allowed_models: user.allowed_models.clone(),
        }
    }
}

// The trusted snapshot is returned on the common path so callers can use the
// complete immutable view without another lookup. Preserve this established
// in-memory registry representation rather than introducing heap allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum MemoryAuthApiKeyOwnerRegistryEntry {
    Trusted(MemoryAuthApiKeyOwnerSnapshot),
    Conflicted,
}

#[derive(Debug, Default)]
pub struct InMemoryAuthApiKeySnapshotRepository {
    index: RwLock<MemoryAuthApiKeyIndex>,
    lookup_delay: Option<Duration>,
}

impl InMemoryAuthApiKeySnapshotRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = (Option<String>, StoredAuthApiKeySnapshot)>,
    {
        let mut by_api_key_id = BTreeMap::new();
        let mut export_by_api_key_id = BTreeMap::new();
        let mut by_key_hash = BTreeMap::new();
        let mut owner_by_user_id = BTreeMap::new();
        for (key_hash, snapshot) in items {
            Self::register_owner_snapshot(
                &mut owner_by_user_id,
                MemoryAuthApiKeyOwnerSnapshot::from(&snapshot),
            );
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
                .and_then(|record| {
                    record.with_ip_rules(
                        snapshot
                            .api_key_ip_rules
                            .as_ref()
                            .map(|value| serde_json::json!(value)),
                    )
                })
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
                owner_by_user_id,
                touch_counts: BTreeMap::new(),
                snapshot_lookup_counts: BTreeMap::new(),
                key_hash_lookup_counts: BTreeMap::new(),
            }),
            lookup_delay: None,
        }
    }

    /// Registers trusted owner state without inserting an API key.  Only the
    /// owner fields are retained; the API-key fields of each fixture snapshot
    /// are intentionally ignored.
    pub fn with_owner_snapshots<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredAuthApiKeySnapshot>,
    {
        let index = self
            .index
            .get_mut()
            .expect("auth api key snapshot repository lock");
        for snapshot in items {
            Self::register_owner_snapshot(
                &mut index.owner_by_user_id,
                MemoryAuthApiKeyOwnerSnapshot::from(&snapshot),
            );
        }
        self
    }

    pub fn with_lookup_delay_for_tests(mut self, delay: Duration) -> Self {
        self.lookup_delay = Some(delay);
        self
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

    pub fn snapshot_lookup_count(&self, api_key_id: &str) -> usize {
        self.index
            .read()
            .expect("auth api key snapshot repository lock")
            .snapshot_lookup_counts
            .get(api_key_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn key_hash_lookup_count(&self, key_hash: &str) -> usize {
        self.index
            .read()
            .expect("auth api key snapshot repository lock")
            .key_hash_lookup_counts
            .get(key_hash)
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

    fn remove_api_key(index: &mut MemoryAuthApiKeyIndex, api_key_id: &str) {
        let key_hashes = index
            .by_key_hash
            .iter()
            .filter(|(_, mapped_api_key_id)| mapped_api_key_id.as_str() == api_key_id)
            .map(|(key_hash, _)| key_hash.clone())
            .collect::<Vec<_>>();
        index.by_api_key_id.remove(api_key_id);
        index.export_by_api_key_id.remove(api_key_id);
        index.by_key_hash.retain(|_, value| value != api_key_id);
        index.touch_counts.remove(api_key_id);
        index.snapshot_lookup_counts.remove(api_key_id);
        for key_hash in key_hashes {
            index.key_hash_lookup_counts.remove(&key_hash);
        }
    }

    fn register_owner_snapshot(
        owners: &mut BTreeMap<String, MemoryAuthApiKeyOwnerRegistryEntry>,
        owner: MemoryAuthApiKeyOwnerSnapshot,
    ) {
        use std::collections::btree_map::Entry;

        match owners.entry(owner.user_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(MemoryAuthApiKeyOwnerRegistryEntry::Trusted(owner));
            }
            Entry::Occupied(mut entry) => {
                let matches_existing = matches!(
                    entry.get(),
                    MemoryAuthApiKeyOwnerRegistryEntry::Trusted(existing) if existing == &owner
                );
                if !matches_existing {
                    entry.insert(MemoryAuthApiKeyOwnerRegistryEntry::Conflicted);
                }
            }
        }
    }
}

fn clamp_i64_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn i64_from_u64(value: u64, field_name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field_name} exceeds i64: {value}")))
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
        if let Some(delay) = self.lookup_delay {
            tokio::time::sleep(delay).await;
        }
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        Ok(match key {
            AuthApiKeyLookupKey::KeyHash(key_hash) => {
                *index
                    .key_hash_lookup_counts
                    .entry(key_hash.to_string())
                    .or_insert(0) += 1;
                index
                    .by_key_hash
                    .get(key_hash)
                    .and_then(|api_key_id| index.by_api_key_id.get(api_key_id))
                    .cloned()
            }
            AuthApiKeyLookupKey::ApiKeyId(api_key_id) => {
                *index
                    .snapshot_lookup_counts
                    .entry(api_key_id.to_string())
                    .or_insert(0) += 1;
                index.by_api_key_id.get(api_key_id).cloned()
            }
            AuthApiKeyLookupKey::UserApiKeyIds {
                user_id,
                api_key_id,
            } => {
                *index
                    .snapshot_lookup_counts
                    .entry(api_key_id.to_string())
                    .or_insert(0) += 1;
                index
                    .by_api_key_id
                    .get(api_key_id)
                    .filter(|snapshot| snapshot.user_id == user_id)
                    .cloned()
            }
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
        i64_from_u64(now_unix_secs, "api_keys.summary_now")?;
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
        i64_from_u64(now_unix_secs, "api_keys.summary_now")?;
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
        i64_from_u64(now_unix_secs, "api_keys.summary_now")?;
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

    async fn synchronize_user_api_key_owner_for_tests(
        &self,
        user: &StoredUserAuthRecord,
    ) -> Result<(), DataLayerError> {
        if user.id.trim().is_empty()
            || user.username.trim().is_empty()
            || user.role.trim().is_empty()
            || user.auth_source.trim().is_empty()
            || user.security_version < 0
        {
            return Err(DataLayerError::InvalidInput(
                "invalid authoritative API-key owner snapshot".to_string(),
            ));
        }

        let owner = MemoryAuthApiKeyOwnerSnapshot::from(user);
        self.index
            .write()
            .expect("auth api key snapshot repository lock")
            .owner_by_user_id
            .insert(
                owner.user_id.clone(),
                MemoryAuthApiKeyOwnerRegistryEntry::Trusted(owner),
            );
        Ok(())
    }

    async fn create_user_api_key(
        &self,
        record: CreateUserApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        // Match the database adapters: inactive owners may retain or receive keys for
        // administrative restore workflows, but unknown/deleted owners must fail closed.  The
        // resulting snapshot remains unusable while its trusted owner is inactive.
        let owner = match index.owner_by_user_id.get(&record.user_id) {
            Some(MemoryAuthApiKeyOwnerRegistryEntry::Trusted(owner)) if !owner.user_is_deleted => {
                owner.clone()
            }
            Some(
                MemoryAuthApiKeyOwnerRegistryEntry::Trusted(_)
                | MemoryAuthApiKeyOwnerRegistryEntry::Conflicted,
            )
            | None => return Ok(None),
        };
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
        let snapshot = StoredAuthApiKeySnapshot {
            user_id: owner.user_id,
            username: owner.username,
            email: owner.email,
            user_role: owner.user_role,
            user_auth_source: owner.user_auth_source,
            user_is_active: owner.user_is_active,
            user_is_deleted: owner.user_is_deleted,
            user_rate_limit: owner.user_rate_limit,
            user_allowed_providers: owner.user_allowed_providers,
            user_allowed_api_formats: owner.user_allowed_api_formats,
            user_allowed_models: owner.user_allowed_models,
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
            api_key_ip_rules: record.ip_rules.clone(),
            user_provider_key_policies: std::collections::BTreeMap::new(),
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
            record
                .expires_at_unix_secs
                .map(|value| i64_from_u64(value, "api_keys.expires_at"))
                .transpose()?,
            record.auto_delete_on_expiry,
            i64_from_u64(record.total_requests, "api_keys.total_requests")?,
            i64_from_u64(record.total_tokens, "api_keys.total_tokens")?,
            record.total_cost_usd,
            false,
        )?
        .with_ip_rules(
            record
                .ip_rules
                .as_ref()
                .map(|value| serde_json::json!(value)),
        )?
        .with_feature_settings(record.feature_settings)
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
                api_key_ip_rules: record.ip_rules.clone(),
                user_provider_key_policies: std::collections::BTreeMap::new(),
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
                record
                    .expires_at_unix_secs
                    .map(|value| i64_from_u64(value, "api_keys.expires_at"))
                    .transpose()?,
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
            .with_api_key_ip_rules(
                record
                    .ip_rules
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
            record
                .expires_at_unix_secs
                .map(|value| i64_from_u64(value, "api_keys.expires_at"))
                .transpose()?,
            record.auto_delete_on_expiry,
            i64_from_u64(record.total_requests, "api_keys.total_requests")?,
            i64_from_u64(record.total_tokens, "api_keys.total_tokens")?,
            record.total_cost_usd,
            true,
        )?
        .with_ip_rules(
            record
                .ip_rules
                .as_ref()
                .map(|value| serde_json::json!(value)),
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

    async fn compare_and_swap_api_key_ciphertext(
        &self,
        mutation: &CompareAndSwapAuthApiKeyCiphertext,
    ) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(current) = index.export_by_api_key_id.get_mut(&mutation.api_key_id) else {
            return Ok(false);
        };
        if current.user_id != mutation.user_id
            || current.key_hash != mutation.key_hash
            || current.is_standalone != mutation.is_standalone
            || current.key_encrypted.as_deref() != Some(mutation.expected_key_encrypted.as_str())
        {
            return Ok(false);
        }
        current.key_encrypted = Some(mutation.key_encrypted.clone());
        Ok(true)
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
        if record.key_encrypted_present {
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.key_encrypted = record.key_encrypted.clone();
            }
        }
        if record.name_present {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_name = record.name.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.name = record.name.clone();
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
        if let Some(ip_rules) = record.ip_rules {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_ip_rules = ip_rules.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.ip_rules = ip_rules;
            }
        }
        if let Some(feature_settings) = record.feature_settings {
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.feature_settings = match feature_settings {
                    Some(serde_json::Value::Null) | None => None,
                    Some(value) => Some(value),
                };
            }
        }
        Ok(index.export_by_api_key_id.get(&record.api_key_id).cloned())
    }

    async fn update_user_api_key_basic_if_unlocked(
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
        if snapshot.user_id != record.user_id
            || snapshot.api_key_is_standalone
            || snapshot.api_key_is_locked
        {
            return Ok(None);
        }
        if record.key_encrypted_present {
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.key_encrypted = record.key_encrypted.clone();
            }
        }
        if record.name_present {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_name = record.name.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.name = record.name.clone();
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
        if let Some(ip_rules) = record.ip_rules {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_ip_rules = ip_rules.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.ip_rules = ip_rules;
            }
        }
        if let Some(feature_settings) = record.feature_settings {
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.feature_settings = match feature_settings {
                    Some(serde_json::Value::Null) | None => None,
                    Some(value) => Some(value),
                };
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
        if record.key_encrypted_present {
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.key_encrypted = record.key_encrypted.clone();
            }
        }
        if record.name_present {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_name = record.name.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.name = record.name.clone();
            }
        }
        if let Some(force_capabilities) = record.force_capabilities {
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.force_capabilities = force_capabilities;
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
        if let Some(ip_rules) = record.ip_rules {
            if let Some(snapshot) = index.by_api_key_id.get_mut(&record.api_key_id) {
                snapshot.api_key_ip_rules = ip_rules.clone();
            }
            if let Some(export) = index.export_by_api_key_id.get_mut(&record.api_key_id) {
                export.ip_rules = ip_rules;
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

    async fn restore_api_key_if_matches(
        &self,
        expected: &StoredAuthApiKeyExportRecord,
        restored: &StoredAuthApiKeyExportRecord,
    ) -> Result<bool, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(current) = index.export_by_api_key_id.get(&expected.api_key_id) else {
            return Ok(false);
        };
        // Immutable identity must never be changed by compensation.  The complete exported row
        // comparison below is the in-memory equivalent of the database CAS predicate.
        if current != expected
            || restored.api_key_id != expected.api_key_id
            || restored.user_id != expected.user_id
            || restored.key_hash != expected.key_hash
            || restored.is_standalone != expected.is_standalone
        {
            return Ok(false);
        }
        index
            .export_by_api_key_id
            .insert(expected.api_key_id.clone(), restored.clone());
        if let Some(snapshot) = index.by_api_key_id.get_mut(&expected.api_key_id) {
            snapshot.api_key_name = restored.name.clone();
            snapshot.api_key_is_active = restored.is_active;
            snapshot.api_key_rate_limit = restored.rate_limit;
            snapshot.api_key_concurrent_limit = restored.concurrent_limit;
            snapshot.api_key_expires_at_unix_secs = restored.expires_at_unix_secs;
            snapshot.api_key_allowed_providers = restored.allowed_providers.clone();
            snapshot.api_key_allowed_api_formats = restored.allowed_api_formats.clone();
            snapshot.api_key_allowed_models = restored.allowed_models.clone();
            snapshot.api_key_ip_rules = restored.ip_rules.clone();
        }
        Ok(true)
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

    async fn set_user_api_key_active_if_unlocked(
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
        if snapshot.user_id != user_id
            || snapshot.api_key_is_standalone
            || snapshot.api_key_is_locked
        {
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

    async fn set_user_api_key_allowed_providers_if_unlocked(
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
        if snapshot.user_id != user_id
            || snapshot.api_key_is_standalone
            || snapshot.api_key_is_locked
        {
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

    async fn set_user_api_key_force_capabilities_if_unlocked(
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
        if snapshot.user_id != user_id
            || snapshot.api_key_is_standalone
            || snapshot.api_key_is_locked
        {
            return Ok(None);
        }
        let Some(export) = index.export_by_api_key_id.get_mut(api_key_id) else {
            return Ok(None);
        };
        export.force_capabilities = force_capabilities;
        Ok(Some(export.clone()))
    }

    async fn set_user_api_key_feature_settings(
        &self,
        user_id: &str,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
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
        export.feature_settings = match feature_settings {
            Some(serde_json::Value::Null) | None => None,
            Some(value) => Some(value),
        };
        Ok(Some(export.clone()))
    }

    async fn set_user_api_key_feature_settings_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(snapshot) = index.by_api_key_id.get(api_key_id) else {
            return Ok(None);
        };
        if snapshot.user_id != user_id
            || snapshot.api_key_is_standalone
            || snapshot.api_key_is_locked
        {
            return Ok(None);
        }
        let Some(export) = index.export_by_api_key_id.get_mut(api_key_id) else {
            return Ok(None);
        };
        export.feature_settings = match feature_settings {
            Some(serde_json::Value::Null) | None => None,
            Some(value) => Some(value),
        };
        Ok(Some(export.clone()))
    }

    async fn set_api_key_usage_totals(
        &self,
        api_key_id: &str,
        total_requests: u64,
        total_tokens: u64,
        total_cost_usd: f64,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        i64_from_u64(total_requests, "api_keys.total_requests")?;
        i64_from_u64(total_tokens, "api_keys.total_tokens")?;
        if !total_cost_usd.is_finite() {
            return Err(DataLayerError::InvalidInput(
                "api_keys.total_cost_usd is not finite".to_string(),
            ));
        }
        let mut index = self
            .index
            .write()
            .expect("auth api key snapshot repository lock");
        let Some(export) = index.export_by_api_key_id.get_mut(api_key_id) else {
            return Ok(None);
        };
        export.total_requests = total_requests;
        export.total_tokens = total_tokens;
        export.total_cost_usd = total_cost_usd;
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
        Self::remove_api_key(&mut index, api_key_id);
        Ok(true)
    }

    async fn delete_user_api_key_if_unlocked(
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
        if snapshot.user_id != user_id
            || snapshot.api_key_is_standalone
            || snapshot.api_key_is_locked
        {
            return Ok(false);
        }
        Self::remove_api_key(&mut index, api_key_id);
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
        Self::remove_api_key(&mut index, api_key_id);
        Ok(true)
    }

    async fn set_standalone_api_key_feature_settings(
        &self,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
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
        let Some(export) = index.export_by_api_key_id.get_mut(api_key_id) else {
            return Ok(None);
        };
        export.feature_settings = match feature_settings {
            Some(serde_json::Value::Null) | None => None,
            Some(value) => Some(value),
        };
        Ok(Some(export.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryAuthApiKeySnapshotRepository;
    use crate::repository::auth::{
        AuthApiKeyLookupKey, AuthApiKeyReadRepository, AuthApiKeyWriteRepository,
        CompareAndSwapAuthApiKeyCiphertext, CreateUserApiKeyRecord,
        StandaloneApiKeyExportListQuery, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
        UpdateStandaloneApiKeyBasicRecord, UpdateUserApiKeyBasicRecord,
    };
    use crate::repository::users::StoredUserAuthRecord;

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

    fn sample_create_user_api_key_record(
        user_id: &str,
        api_key_id: &str,
    ) -> CreateUserApiKeyRecord {
        CreateUserApiKeyRecord {
            user_id: user_id.to_string(),
            api_key_id: api_key_id.to_string(),
            key_hash: format!("hash-{api_key_id}"),
            key_encrypted: Some(format!("encrypted-{api_key_id}")),
            name: Some("Created".to_string()),
            allowed_providers: None,
            allowed_api_formats: None,
            allowed_models: None,
            ip_rules: None,
            rate_limit: 0,
            concurrent_limit: None,
            force_capabilities: None,
            feature_settings: None,
            is_active: true,
            expires_at_unix_secs: None,
            auto_delete_on_expiry: false,
            total_requests: 0,
            total_tokens: 0,
            total_cost_usd: 0.0,
        }
    }

    fn sample_authoritative_user(
        user_id: &str,
        is_active: bool,
        is_deleted: bool,
    ) -> StoredUserAuthRecord {
        StoredUserAuthRecord::new(
            user_id.to_string(),
            Some(format!("{user_id}@example.com")),
            true,
            format!("owner-{user_id}"),
            Some("server-managed-password-hash".to_string()),
            "admin".to_string(),
            "oauth".to_string(),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5"])),
            is_active,
            is_deleted,
            None,
            None,
        )
        .expect("authoritative user should build")
        .with_security_version(37)
        .expect("security version should be valid")
    }

    #[tokio::test]
    async fn authoritative_owner_sync_allows_first_key_without_synthesizing_owner_fields() {
        let repository = InMemoryAuthApiKeySnapshotRepository::default();
        let user = sample_authoritative_user("authoritative-user", true, false);
        repository
            .synchronize_user_api_key_owner_for_tests(&user)
            .await
            .expect("authoritative owner should synchronize");

        let mut record =
            sample_create_user_api_key_record("authoritative-user", "authoritative-key");
        record.allowed_providers = Some(vec!["anthropic".to_string()]);
        repository
            .create_user_api_key(record)
            .await
            .expect("first key creation should resolve")
            .expect("active authoritative owner should allow its first key");

        let snapshot = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("authoritative-key"))
            .await
            .expect("created snapshot lookup should resolve")
            .expect("created snapshot should exist");
        assert_eq!(snapshot.username, user.username);
        assert_eq!(snapshot.email, user.email);
        assert_eq!(snapshot.user_role, "admin");
        assert_eq!(snapshot.user_auth_source, "oauth");
        assert!(snapshot.user_is_active);
        assert!(!snapshot.user_is_deleted);
        assert_eq!(
            snapshot.user_allowed_providers,
            Some(vec!["openai".to_string()])
        );
        assert_eq!(
            snapshot.api_key_allowed_providers,
            Some(vec!["anthropic".to_string()])
        );
        assert_eq!(user.security_version, 37);
    }

    #[tokio::test]
    async fn api_key_ciphertext_cas_fences_complete_identity_and_exact_old_value() {
        let repository = InMemoryAuthApiKeySnapshotRepository::default();
        repository
            .synchronize_user_api_key_owner_for_tests(&sample_authoritative_user(
                "cipher-owner",
                true,
                false,
            ))
            .await
            .expect("owner should synchronize");
        repository
            .create_user_api_key(sample_create_user_api_key_record(
                "cipher-owner",
                "cipher-key",
            ))
            .await
            .expect("key creation should succeed")
            .expect("key should be created");

        let expected = CompareAndSwapAuthApiKeyCiphertext {
            user_id: "cipher-owner".to_string(),
            api_key_id: "cipher-key".to_string(),
            key_hash: "hash-cipher-key".to_string(),
            is_standalone: false,
            expected_key_encrypted: "encrypted-cipher-key".to_string(),
            key_encrypted: "bound-ciphertext".to_string(),
        };
        for mutation in [
            CompareAndSwapAuthApiKeyCiphertext {
                user_id: "other-owner".to_string(),
                ..expected.clone()
            },
            CompareAndSwapAuthApiKeyCiphertext {
                key_hash: "other-hash".to_string(),
                ..expected.clone()
            },
            CompareAndSwapAuthApiKeyCiphertext {
                is_standalone: true,
                ..expected.clone()
            },
            CompareAndSwapAuthApiKeyCiphertext {
                expected_key_encrypted: "ENCRYPTED-cipher-key".to_string(),
                ..expected.clone()
            },
        ] {
            assert!(!repository
                .compare_and_swap_api_key_ciphertext(&mutation)
                .await
                .expect("CAS should execute"));
        }
        assert!(repository
            .compare_and_swap_api_key_ciphertext(&expected)
            .await
            .expect("matching CAS should execute"));
        assert_eq!(
            repository
                .list_export_api_keys_by_ids(&["cipher-key".to_string()])
                .await
                .expect("key should reload")[0]
                .key_encrypted
                .as_deref(),
            Some("bound-ciphertext")
        );
        assert!(!repository
            .compare_and_swap_api_key_ciphertext(&expected)
            .await
            .expect("stale CAS should execute"));
    }

    #[tokio::test]
    async fn authoritative_owner_sync_preserves_inactive_state_and_rejects_deleted_owner() {
        let repository = InMemoryAuthApiKeySnapshotRepository::default();
        let inactive = sample_authoritative_user("inactive-user", false, false);
        repository
            .synchronize_user_api_key_owner_for_tests(&inactive)
            .await
            .expect("inactive owner should synchronize without privilege elevation");
        repository
            .create_user_api_key(sample_create_user_api_key_record(
                "inactive-user",
                "inactive-key",
            ))
            .await
            .expect("inactive owner creation should resolve")
            .expect("low-level repository should retain database write parity");
        let inactive_snapshot = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("inactive-key"))
            .await
            .expect("inactive key lookup should resolve")
            .expect("inactive key should be stored");
        assert!(!inactive_snapshot.user_is_active);
        assert!(!inactive_snapshot.is_currently_usable(0));

        let deleted = sample_authoritative_user("deleted-user", true, true);
        repository
            .synchronize_user_api_key_owner_for_tests(&deleted)
            .await
            .expect("deleted owner tombstone should synchronize");
        assert!(repository
            .create_user_api_key(sample_create_user_api_key_record(
                "deleted-user",
                "deleted-key",
            ))
            .await
            .expect("deleted owner creation should resolve")
            .is_none());
        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("deleted-key"))
            .await
            .expect("rejected deleted key lookup should resolve")
            .is_none());
    }

    #[tokio::test]
    async fn create_user_api_key_persists_feature_settings_in_initial_record() {
        let repository = InMemoryAuthApiKeySnapshotRepository::default()
            .with_owner_snapshots([sample_snapshot("owner-fixture", "user-1")]);
        let mut record = sample_create_user_api_key_record("user-1", "key-created");
        record.feature_settings = Some(serde_json::json!({"compact": true}));
        let created = repository
            .create_user_api_key(record)
            .await
            .expect("create should succeed")
            .expect("created key should be returned");

        assert_eq!(
            created.feature_settings,
            Some(serde_json::json!({"compact": true}))
        );
        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("owner-fixture"))
            .await
            .expect("owner fixture lookup should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn create_user_api_key_rejects_unknown_deleted_and_conflicted_owners() {
        let mut deleted_owner = sample_snapshot("deleted-owner-fixture", "deleted-user");
        deleted_owner.user_is_deleted = true;
        let conflicted_owner = sample_snapshot("conflicted-owner-fixture-a", "conflicted-user");
        let mut conflicting_owner =
            sample_snapshot("conflicted-owner-fixture-b", "conflicted-user");
        conflicting_owner.username = "different-owner-state".to_string();
        let cases = [
            (
                "unknown",
                "unknown-user",
                InMemoryAuthApiKeySnapshotRepository::default(),
            ),
            (
                "deleted",
                "deleted-user",
                InMemoryAuthApiKeySnapshotRepository::default()
                    .with_owner_snapshots([deleted_owner]),
            ),
            (
                "conflicted",
                "conflicted-user",
                InMemoryAuthApiKeySnapshotRepository::default()
                    .with_owner_snapshots([conflicted_owner, conflicting_owner]),
            ),
        ];

        for (case, user_id, repository) in cases {
            let api_key_id = format!("key-{case}");
            let key_hash = format!("hash-{api_key_id}");
            let record = sample_create_user_api_key_record(user_id, &api_key_id);
            assert!(repository
                .create_user_api_key(record)
                .await
                .expect("fail-closed creation should resolve")
                .is_none());
            assert!(repository
                .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId(&api_key_id))
                .await
                .expect("rejected key lookup should resolve")
                .is_none());
            assert!(repository
                .find_api_key_snapshot(AuthApiKeyLookupKey::KeyHash(&key_hash))
                .await
                .expect("rejected hash lookup should resolve")
                .is_none());
            assert!(repository
                .list_export_api_keys_by_user_ids(&[user_id.to_string()])
                .await
                .expect("rejected exports should list")
                .is_empty());
        }
    }

    #[tokio::test]
    async fn create_user_api_key_preserves_disabled_owner_state() {
        let mut disabled_owner = sample_snapshot("disabled-owner-fixture", "disabled-user");
        disabled_owner.user_is_active = false;
        let repository =
            InMemoryAuthApiKeySnapshotRepository::default().with_owner_snapshots([disabled_owner]);

        repository
            .create_user_api_key(sample_create_user_api_key_record(
                "disabled-user",
                "key-disabled",
            ))
            .await
            .expect("disabled-owner creation should resolve")
            .expect("non-deleted disabled owner should retain database parity");

        let snapshot = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-disabled"))
            .await
            .expect("created key lookup should resolve")
            .expect("created key snapshot should exist");
        assert!(!snapshot.user_is_active);
        assert!(!snapshot.is_currently_usable(0));
    }

    #[tokio::test]
    async fn invalid_owner_cannot_probe_duplicate_credential_indexes() {
        let repository = InMemoryAuthApiKeySnapshotRepository::seed([(
            Some("hash-key-existing".to_string()),
            sample_snapshot("key-existing", "trusted-user"),
        )]);

        assert!(repository
            .create_user_api_key(sample_create_user_api_key_record(
                "unknown-user",
                "key-existing",
            ))
            .await
            .expect("invalid owner should fail closed before duplicate checks")
            .is_none());

        let existing = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-existing"))
            .await
            .expect("existing key lookup should resolve")
            .expect("existing key should remain unchanged");
        assert_eq!(existing.user_id, "trusted-user");
    }

    #[tokio::test]
    async fn usage_total_replacement_rejects_unpersistable_values_without_mutation() {
        let repository = InMemoryAuthApiKeySnapshotRepository::seed([(
            Some("hash-key-existing".to_string()),
            sample_snapshot("key-existing", "trusted-user"),
        )]);
        let before = repository
            .list_export_api_keys_by_ids(&["key-existing".to_string()])
            .await
            .expect("existing export should load")
            .pop()
            .expect("existing export should exist");

        for (total_requests, total_tokens, total_cost_usd) in [
            (u64::MAX, 0, 0.0),
            (0, u64::MAX, 0.0),
            (0, 0, f64::NAN),
            (0, 0, f64::INFINITY),
        ] {
            assert!(matches!(
                repository
                    .set_api_key_usage_totals(
                        "key-existing",
                        total_requests,
                        total_tokens,
                        total_cost_usd,
                    )
                    .await,
                Err(crate::DataLayerError::InvalidInput(_))
            ));
        }

        let after = repository
            .list_export_api_keys_by_ids(&["key-existing".to_string()])
            .await
            .expect("existing export should reload")
            .pop()
            .expect("existing export should remain");
        assert_eq!(after, before);
    }

    #[tokio::test]
    async fn unlocked_user_mutations_atomically_reject_locked_keys() {
        let mut snapshot = sample_snapshot("key-locked", "user-1");
        snapshot.api_key_is_locked = true;
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-locked".to_string()),
            snapshot,
        )]);

        assert!(repository
            .update_user_api_key_basic_if_unlocked(UpdateUserApiKeyBasicRecord {
                user_id: "user-1".to_string(),
                api_key_id: "key-locked".to_string(),
                key_encrypted: None,
                key_encrypted_present: false,
                name: Some("must-not-change".to_string()),
                name_present: true,
                rate_limit: None,
                rate_limit_present: false,
                concurrent_limit: None,
                concurrent_limit_present: false,
                ip_rules: None,
                feature_settings: Some(Some(serde_json::json!({"must_not_change": true}))),
            })
            .await
            .expect("locked basic update should resolve")
            .is_none());
        assert!(repository
            .set_user_api_key_active_if_unlocked("user-1", "key-locked", false)
            .await
            .expect("locked status update should resolve")
            .is_none());
        assert!(repository
            .set_user_api_key_allowed_providers_if_unlocked(
                "user-1",
                "key-locked",
                Some(vec!["must-not-change".to_string()]),
            )
            .await
            .expect("locked provider update should resolve")
            .is_none());
        assert!(repository
            .set_user_api_key_force_capabilities_if_unlocked(
                "user-1",
                "key-locked",
                Some(serde_json::json!({"must_not_change": true})),
            )
            .await
            .expect("locked capability update should resolve")
            .is_none());
        assert!(repository
            .set_user_api_key_feature_settings_if_unlocked(
                "user-1",
                "key-locked",
                Some(serde_json::json!({"must_not_change": true})),
            )
            .await
            .expect("locked feature update should resolve")
            .is_none());
        assert!(!repository
            .delete_user_api_key_if_unlocked("user-1", "key-locked")
            .await
            .expect("locked deletion should resolve"));

        let unchanged = repository
            .list_export_api_keys_by_ids(&["key-locked".to_string()])
            .await
            .expect("locked key should reload")
            .pop()
            .expect("locked key should remain");
        assert_ne!(unchanged.name.as_deref(), Some("must-not-change"));
        assert!(unchanged.is_active);
        assert!(unchanged.feature_settings.is_none());

        // Administrative repository operations deliberately retain authority
        // over locked keys; only the self-service variants enforce the fence.
        let admin_updated = repository
            .update_user_api_key_basic(UpdateUserApiKeyBasicRecord {
                user_id: "user-1".to_string(),
                api_key_id: "key-locked".to_string(),
                key_encrypted: None,
                key_encrypted_present: false,
                name: Some("admin-change".to_string()),
                name_present: true,
                rate_limit: None,
                rate_limit_present: false,
                concurrent_limit: None,
                concurrent_limit_present: false,
                ip_rules: None,
                feature_settings: Some(Some(serde_json::json!({"admin": true}))),
            })
            .await
            .expect("administrator update should resolve")
            .expect("administrator may update a locked key");
        assert_eq!(admin_updated.name.as_deref(), Some("admin-change"));
        assert_eq!(
            admin_updated.feature_settings,
            Some(serde_json::json!({"admin": true}))
        );
        assert!(repository
            .set_user_api_key_active("user-1", "key-locked", false)
            .await
            .expect("admin status update should resolve")
            .is_some());
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
    async fn delete_is_owner_scoped_and_removes_all_credential_indexes() {
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-1".to_string()),
            sample_snapshot("key-1", "user-1"),
        )]);

        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::KeyHash("hash-1"))
            .await
            .expect("hash lookup should succeed")
            .is_some());
        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-1"))
            .await
            .expect("id lookup should succeed")
            .is_some());
        assert!(repository
            .touch_last_used_at("key-1")
            .await
            .expect("touch should succeed"));

        assert!(!repository
            .delete_user_api_key("other-user", "key-1")
            .await
            .expect("wrong-owner delete should resolve"));
        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-1"))
            .await
            .expect("wrong-owner delete must preserve the key")
            .is_some());

        assert!(repository
            .delete_user_api_key("user-1", "key-1")
            .await
            .expect("owner-scoped delete should succeed"));
        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-1"))
            .await
            .expect("deleted id lookup should resolve")
            .is_none());
        assert!(repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::KeyHash("hash-1"))
            .await
            .expect("deleted hash lookup should resolve")
            .is_none());
        assert_eq!(repository.touch_count("key-1"), 0);
        assert_eq!(repository.snapshot_lookup_count("key-1"), 1);
        assert_eq!(repository.key_hash_lookup_count("hash-1"), 1);
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
                key_encrypted: None,
                key_encrypted_present: false,
                name: None,
                name_present: false,
                rate_limit: None,
                rate_limit_present: false,
                concurrent_limit: Some(11),
                concurrent_limit_present: true,
                ip_rules: None,
                feature_settings: None,
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
    async fn update_user_api_key_basic_restores_nullable_values_and_zero() {
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-1".to_string()),
            sample_snapshot("key-1", "user-1"),
        )]);

        let cleared = repository
            .update_user_api_key_basic(UpdateUserApiKeyBasicRecord {
                user_id: "user-1".to_string(),
                api_key_id: "key-1".to_string(),
                key_encrypted: None,
                key_encrypted_present: true,
                name: None,
                name_present: true,
                rate_limit: None,
                rate_limit_present: true,
                concurrent_limit: None,
                concurrent_limit_present: true,
                ip_rules: None,
                feature_settings: None,
            })
            .await
            .expect("nullable values should clear")
            .expect("record should exist");
        assert!(cleared.name.is_none());
        assert!(cleared.rate_limit.is_none());
        assert!(cleared.concurrent_limit.is_none());

        let zero = repository
            .update_user_api_key_basic(UpdateUserApiKeyBasicRecord {
                user_id: "user-1".to_string(),
                api_key_id: "key-1".to_string(),
                key_encrypted: None,
                key_encrypted_present: false,
                name: None,
                name_present: false,
                rate_limit: Some(0),
                rate_limit_present: true,
                concurrent_limit: None,
                concurrent_limit_present: false,
                ip_rules: None,
                feature_settings: None,
            })
            .await
            .expect("zero rate limit should persist")
            .expect("record should exist");
        assert_eq!(zero.rate_limit, Some(0));
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
                key_encrypted: None,
                key_encrypted_present: false,
                name: None,
                name_present: false,
                force_capabilities: None,
                rate_limit_present: false,
                rate_limit: None,
                concurrent_limit_present: true,
                concurrent_limit: Some(13),
                allowed_providers: None,
                allowed_api_formats: None,
                allowed_models: None,
                ip_rules: None,
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

    #[tokio::test]
    async fn restores_standalone_nullable_fields_and_force_capabilities_atomically() {
        let mut standalone = sample_snapshot("key-standalone", "admin-1");
        standalone.api_key_is_standalone = true;
        let before = StoredAuthApiKeyExportRecord::new(
            "admin-1".to_string(),
            "key-standalone".to_string(),
            "hash-standalone".to_string(),
            None,
            None,
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            Some(60),
            Some(5),
            None,
            true,
            Some(200),
            false,
            2,
            25,
            0.25,
            true,
        )
        .expect("before export should build")
        .with_ip_rules(Some(serde_json::json!(["10.0.0.0/24"])))
        .expect("before ip rules should build")
        .with_feature_settings(Some(serde_json::json!({"compact": true})));
        let repository = InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-standalone".to_string()),
            standalone,
        )])
        .with_export_records([before.clone()]);

        let after = repository
            .update_standalone_api_key_basic(UpdateStandaloneApiKeyBasicRecord {
                api_key_id: "key-standalone".to_string(),
                key_encrypted: Some("enc-after".to_string()),
                key_encrypted_present: true,
                name: Some("after".to_string()),
                name_present: true,
                force_capabilities: Some(Some(serde_json::json!({"vision": true}))),
                rate_limit_present: true,
                rate_limit: Some(99),
                concurrent_limit_present: true,
                concurrent_limit: Some(8),
                allowed_providers: Some(Some(vec!["anthropic".to_string()])),
                allowed_api_formats: None,
                allowed_models: None,
                ip_rules: None,
                expires_at_present: false,
                expires_at_unix_secs: None,
                auto_delete_on_expiry_present: false,
                auto_delete_on_expiry: false,
            })
            .await
            .expect("after update should succeed")
            .expect("after record should exist");
        assert!(repository
            .restore_api_key_if_matches(&after, &before)
            .await
            .expect("restore should succeed"));
        let restored = repository
            .list_export_api_keys_by_ids(&["key-standalone".to_string()])
            .await
            .expect("restored export should load")
            .pop()
            .expect("restored key should exist");
        assert_eq!(restored, before);

        // A changed post-state must make the CAS fail without touching the newer value.
        let concurrent = repository
            .update_standalone_api_key_basic(UpdateStandaloneApiKeyBasicRecord {
                api_key_id: "key-standalone".to_string(),
                key_encrypted: None,
                key_encrypted_present: false,
                name: Some("concurrent".to_string()),
                name_present: true,
                force_capabilities: Some(None),
                rate_limit_present: false,
                rate_limit: None,
                concurrent_limit_present: false,
                concurrent_limit: None,
                allowed_providers: None,
                allowed_api_formats: None,
                allowed_models: None,
                ip_rules: None,
                expires_at_present: false,
                expires_at_unix_secs: None,
                auto_delete_on_expiry_present: false,
                auto_delete_on_expiry: false,
            })
            .await
            .expect("concurrent update should succeed")
            .expect("concurrent record should exist");
        assert!(!repository
            .restore_api_key_if_matches(&after, &before)
            .await
            .expect("conflicting restore should return false"));
        assert_eq!(
            repository
                .list_export_api_keys_by_ids(&["key-standalone".to_string()])
                .await
                .expect("current export should load")
                .pop()
                .expect("current key should exist")
                .name,
            concurrent.name
        );
    }
}
