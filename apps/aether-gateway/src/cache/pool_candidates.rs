use std::collections::BTreeMap;
use std::time::Duration;

use aether_cache::ExpiringMap;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredPoolKeyCandidateRowsQuery,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PoolCandidateCacheKey {
    pub(crate) provider_id: String,
    pub(crate) endpoint_id: String,
    pub(crate) model_id: String,
    pub(crate) selected_provider_model_name: String,
    pub(crate) api_format: String,
}

impl PoolCandidateCacheKey {
    pub(crate) fn new(
        provider_id: impl Into<String>,
        endpoint_id: impl Into<String>,
        model_id: impl Into<String>,
        selected_provider_model_name: impl Into<String>,
        api_format: impl Into<String>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            endpoint_id: endpoint_id.into(),
            model_id: model_id.into(),
            selected_provider_model_name: selected_provider_model_name.into(),
            api_format: api_format.into(),
        }
    }

    pub(crate) fn from_pool_group_id(provider_id: &str, pool_group_id: &str) -> Option<Self> {
        let mut parts = BTreeMap::<&str, &str>::new();
        for part in pool_group_id.split('|') {
            let (key, value) = part.split_once('=')?;
            parts.insert(key.trim(), value.trim());
        }

        let provider_id = provider_id.trim();
        let group_provider_id = parts.get("provider")?.trim();
        let endpoint_id = parts.get("endpoint")?.trim();
        let model_id = parts.get("model")?.trim();
        let selected_provider_model_name = parts.get("selected_model")?.trim();
        let api_format = parts.get("api_format")?.trim();
        if [
            provider_id,
            group_provider_id,
            endpoint_id,
            model_id,
            selected_provider_model_name,
            api_format,
        ]
        .iter()
        .any(|value| value.is_empty())
            || provider_id != group_provider_id
        {
            return None;
        }

        Some(Self::new(
            group_provider_id,
            endpoint_id,
            model_id,
            selected_provider_model_name,
            api_format,
        ))
    }
}

impl From<&StoredPoolKeyCandidateRowsQuery> for PoolCandidateCacheKey {
    fn from(query: &StoredPoolKeyCandidateRowsQuery) -> Self {
        Self::new(
            query.provider_id.clone(),
            query.endpoint_id.clone(),
            query.model_id.clone(),
            query.selected_provider_model_name.clone(),
            query.api_format.clone(),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CachedPoolCandidates {
    pub(crate) rows: Vec<StoredMinimalCandidateSelectionRow>,
    pub(crate) config_version: u64,
    pub(crate) next_db_offset: u32,
}

impl CachedPoolCandidates {
    pub(crate) fn new(
        rows: Vec<StoredMinimalCandidateSelectionRow>,
        config_version: u64,
        next_db_offset: u32,
    ) -> Self {
        Self {
            rows,
            config_version,
            next_db_offset,
        }
    }

    fn bounded(mut self, max_rows: usize) -> Self {
        self.rows.truncate(max_rows);
        self
    }
}

#[derive(Debug, Default)]
pub(crate) struct PoolCandidateCache {
    entries: ExpiringMap<PoolCandidateCacheKey, CachedPoolCandidates>,
}

impl PoolCandidateCache {
    pub(crate) fn get_fresh(
        &self,
        key: &PoolCandidateCacheKey,
        ttl: Duration,
        current_config_version: u64,
    ) -> Option<CachedPoolCandidates> {
        let cached = self.entries.get_fresh(key, ttl)?;
        if cached.config_version != current_config_version {
            self.entries.remove(key);
            return None;
        }
        Some(cached)
    }

    pub(crate) fn insert(
        &self,
        key: PoolCandidateCacheKey,
        cached: CachedPoolCandidates,
        ttl: Duration,
        max_entries: usize,
        max_rows: usize,
    ) -> bool {
        if max_entries == 0 {
            self.entries.remove(&key);
            return false;
        }
        self.entries
            .insert(key, cached.bounded(max_rows), ttl, max_entries);
        true
    }

    pub(crate) fn evict_key_from_cache(
        &self,
        key: &PoolCandidateCacheKey,
        key_id: &str,
        ttl: Duration,
        current_config_version: u64,
        max_entries: usize,
        max_rows: usize,
    ) -> bool {
        let Some(mut cached) = self.get_fresh(key, ttl, current_config_version) else {
            return false;
        };
        let original_len = cached.rows.len();
        cached.rows.retain(|row| row.key_id != key_id);
        let removed = cached.rows.len() != original_len;
        if !removed {
            return false;
        }
        self.insert(key.clone(), cached, ttl, max_entries, max_rows);
        true
    }

    pub(crate) fn append_backfill_row(
        &self,
        key: &PoolCandidateCacheKey,
        row: StoredMinimalCandidateSelectionRow,
        next_db_offset: u32,
        ttl: Duration,
        current_config_version: u64,
        max_entries: usize,
        max_rows: usize,
    ) -> bool {
        let Some(mut cached) = self.get_fresh(key, ttl, current_config_version) else {
            return false;
        };
        let appended = cached.rows.len() < max_rows;
        if appended {
            cached.rows.push(row);
            cached.next_db_offset = next_db_offset;
            self.insert(key.clone(), cached, ttl, max_entries, max_rows);
        }
        appended
    }

    pub(crate) fn invalidate_exact(&self, key: &PoolCandidateCacheKey) -> bool {
        self.entries.remove(key).is_some()
    }

    pub(crate) fn clear_all(&self) {
        self.entries.clear();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::thread::sleep;

    use super::*;

    const CONFIG_VERSION: u64 = 7;
    const MAX_ENTRIES: usize = 16;
    const MAX_ROWS: usize = 8;

    fn ttl() -> Duration {
        Duration::from_secs(60)
    }

    fn key() -> PoolCandidateCacheKey {
        PoolCandidateCacheKey::new(
            "provider-a",
            "endpoint-a",
            "model-a",
            "gpt-upstream",
            "openai:chat",
        )
    }

    fn other_key() -> PoolCandidateCacheKey {
        PoolCandidateCacheKey::new(
            "provider-b",
            "endpoint-b",
            "model-b",
            "claude-upstream",
            "claude:messages",
        )
    }

    fn row(key_id: &str) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-a".to_string(),
            provider_name: "Provider A".to_string(),
            provider_type: "openai".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-a".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("sync".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: format!("Key {key_id}"),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 3,
            key_global_priority_by_format: None,
            model_id: "model-a".to_string(),
            global_model_id: "global-model-a".to_string(),
            global_model_name: "gpt-test".to_string(),
            global_model_mappings: Some(vec!["gpt-test".to_string()]),
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-upstream".to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    #[test]
    fn insert_and_get_fresh_returns_cached_candidates() {
        let cache = PoolCandidateCache::default();
        let key = key();
        let cached = CachedPoolCandidates::new(vec![row("key-1"), row("key-2")], CONFIG_VERSION, 2);

        assert!(cache.insert(key.clone(), cached.clone(), ttl(), MAX_ENTRIES, MAX_ROWS));

        assert_eq!(cache.get_fresh(&key, ttl(), CONFIG_VERSION), Some(cached));
    }

    #[test]
    fn cache_key_parses_pool_group_id() {
        let parsed = PoolCandidateCacheKey::from_pool_group_id(
            "provider-a",
            "provider=provider-a|endpoint=endpoint-a|model=model-a|selected_model=gpt-upstream|api_format=openai:chat|singleton_key=*",
        )
        .expect("pool group id should parse");

        assert_eq!(parsed, key());
        assert_eq!(
            PoolCandidateCacheKey::from_pool_group_id(
                "provider-b",
                "provider=provider-a|endpoint=endpoint-a|model=model-a|selected_model=gpt-upstream|api_format=openai:chat|singleton_key=*",
            ),
            None
        );
    }

    #[test]
    fn get_fresh_returns_none_for_config_version_mismatch() {
        let cache = PoolCandidateCache::default();
        let key = key();
        let cached = CachedPoolCandidates::new(vec![row("key-1")], CONFIG_VERSION, 1);
        cache.insert(key.clone(), cached, ttl(), MAX_ENTRIES, MAX_ROWS);

        assert_eq!(cache.get_fresh(&key, ttl(), CONFIG_VERSION + 1), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn get_fresh_returns_none_after_ttl_expiration() {
        let cache = PoolCandidateCache::default();
        let key = key();
        let cached = CachedPoolCandidates::new(vec![row("key-1")], CONFIG_VERSION, 1);
        cache.insert(
            key.clone(),
            cached,
            Duration::from_millis(5),
            MAX_ENTRIES,
            MAX_ROWS,
        );

        sleep(Duration::from_millis(20));

        assert_eq!(
            cache.get_fresh(&key, Duration::from_millis(5), CONFIG_VERSION),
            None
        );
        assert!(cache.is_empty());
    }

    #[test]
    fn evict_key_from_cache_removes_only_requested_key_id() {
        let cache = PoolCandidateCache::default();
        let key = key();
        let cached = CachedPoolCandidates::new(
            vec![row("key-1"), row("key-2"), row("key-3")],
            CONFIG_VERSION,
            3,
        );
        cache.insert(key.clone(), cached, ttl(), MAX_ENTRIES, MAX_ROWS);

        assert!(cache.evict_key_from_cache(
            &key,
            "key-2",
            ttl(),
            CONFIG_VERSION,
            MAX_ENTRIES,
            MAX_ROWS,
        ));

        let cached = cache
            .get_fresh(&key, ttl(), CONFIG_VERSION)
            .expect("cache entry should remain after key eviction");
        assert_eq!(cached.config_version, CONFIG_VERSION);
        assert_eq!(cached.next_db_offset, 3);
        assert_eq!(
            cached
                .rows
                .iter()
                .map(|row| row.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-1", "key-3"]
        );
    }

    #[test]
    fn append_backfill_row_pushes_to_end_without_reordering() {
        let cache = PoolCandidateCache::default();
        let key = key();
        let cached = CachedPoolCandidates::new(vec![row("key-1"), row("key-2")], CONFIG_VERSION, 2);
        cache.insert(key.clone(), cached, ttl(), MAX_ENTRIES, MAX_ROWS);

        assert!(cache.append_backfill_row(
            &key,
            row("key-3"),
            3,
            ttl(),
            CONFIG_VERSION,
            MAX_ENTRIES,
            MAX_ROWS,
        ));

        let cached = cache
            .get_fresh(&key, ttl(), CONFIG_VERSION)
            .expect("cache entry should remain after backfill");
        assert_eq!(cached.next_db_offset, 3);
        assert_eq!(
            cached
                .rows
                .iter()
                .map(|row| row.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-1", "key-2", "key-3"]
        );
    }

    #[test]
    fn append_backfill_row_respects_max_rows() {
        let cache = PoolCandidateCache::default();
        let key = key();
        let cached = CachedPoolCandidates::new(vec![row("key-1"), row("key-2")], CONFIG_VERSION, 2);
        cache.insert(key.clone(), cached, ttl(), MAX_ENTRIES, 2);

        assert!(!cache.append_backfill_row(
            &key,
            row("key-3"),
            3,
            ttl(),
            CONFIG_VERSION,
            MAX_ENTRIES,
            2,
        ));

        let cached = cache
            .get_fresh(&key, ttl(), CONFIG_VERSION)
            .expect("cache entry should remain when max rows blocks append");
        assert_eq!(cached.next_db_offset, 2);
        assert_eq!(
            cached
                .rows
                .iter()
                .map(|row| row.key_id.as_str())
                .collect::<Vec<_>>(),
            vec!["key-1", "key-2"]
        );
    }

    #[test]
    fn invalidate_exact_and_clear_all_remove_only_requested_scopes() {
        let cache = PoolCandidateCache::default();
        let key = key();
        let other_key = other_key();
        cache.insert(
            key.clone(),
            CachedPoolCandidates::new(vec![row("key-1")], CONFIG_VERSION, 1),
            ttl(),
            MAX_ENTRIES,
            MAX_ROWS,
        );
        cache.insert(
            other_key.clone(),
            CachedPoolCandidates::new(vec![row("key-2")], CONFIG_VERSION, 1),
            ttl(),
            MAX_ENTRIES,
            MAX_ROWS,
        );

        assert!(cache.invalidate_exact(&key));
        assert_eq!(cache.get_fresh(&key, ttl(), CONFIG_VERSION), None);
        assert!(cache.get_fresh(&other_key, ttl(), CONFIG_VERSION).is_some());

        cache.clear_all();

        assert!(cache.is_empty());
    }

    #[test]
    fn insert_bounds_rows_and_max_entries() {
        let cache = PoolCandidateCache::default();
        let key = key();
        let other_key = other_key();
        cache.insert(
            key.clone(),
            CachedPoolCandidates::new(
                vec![row("key-1"), row("key-2"), row("key-3")],
                CONFIG_VERSION,
                3,
            ),
            ttl(),
            1,
            2,
        );
        let cached = cache
            .get_fresh(&key, ttl(), CONFIG_VERSION)
            .expect("bounded cache entry should be readable");
        assert_eq!(cached.rows.len(), 2);

        sleep(Duration::from_millis(2));
        cache.insert(
            other_key.clone(),
            CachedPoolCandidates::new(vec![row("key-4")], CONFIG_VERSION, 1),
            ttl(),
            1,
            MAX_ROWS,
        );

        assert_eq!(cache.get_fresh(&key, ttl(), CONFIG_VERSION), None);
        assert!(cache.get_fresh(&other_key, ttl(), CONFIG_VERSION).is_some());
        assert_eq!(cache.len(), 1);
    }
}
