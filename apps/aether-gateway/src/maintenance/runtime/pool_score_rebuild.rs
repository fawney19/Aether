use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::pool_scores::GetPoolMemberScoresByIdsQuery;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use tracing::{debug, info, warn};

use crate::admin_api::admin_provider_pool_config;
use crate::ai_serving::build_provider_key_pool_score_upsert;
use crate::handlers::shared::provider_pool::AdminProviderPoolConfig;
use crate::{AppState, GatewayError};

const POOL_SCORE_REBUILD_DEFAULT_INTERVAL_SECONDS: u64 = 300;
const POOL_SCORE_REBUILD_MIN_INTERVAL_SECONDS: u64 = 30;
const POOL_SCORE_REBUILD_DEFAULT_MAX_UPSERTS_PER_TICK: usize = 20_000;
const POOL_SCORE_REBUILD_PROVIDER_CURSOR_KEY: &str = "ap:pool_score_rebuild:provider_cursor";
const POOL_SCORE_REBUILD_PROVIDER_OFFSET_PREFIX: &str = "ap:pool_score_rebuild:provider_offset";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolScoreRebuildRunSummary {
    pub(crate) providers_checked: usize,
    pub(crate) providers_scored: usize,
    pub(crate) keys_seen: usize,
    pub(crate) scores_upserted: usize,
}

impl PoolScoreRebuildRunSummary {
    const fn empty() -> Self {
        Self {
            providers_checked: 0,
            providers_scored: 0,
            keys_seen: 0,
            scores_upserted: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PoolScoreRebuildWorkerConfig {
    pub(crate) interval: Duration,
    pub(crate) max_upserts_per_tick: usize,
}

impl PoolScoreRebuildWorkerConfig {
    fn from_env() -> Self {
        let interval_seconds = env_u64(
            "POOL_SCORE_REBUILD_INTERVAL_SECONDS",
            POOL_SCORE_REBUILD_DEFAULT_INTERVAL_SECONDS,
        )
        .max(POOL_SCORE_REBUILD_MIN_INTERVAL_SECONDS);
        let max_upserts_per_tick = env_usize(
            "POOL_SCORE_REBUILD_MAX_UPSERTS_PER_TICK",
            POOL_SCORE_REBUILD_DEFAULT_MAX_UPSERTS_PER_TICK,
        )
        .max(1);
        Self {
            interval: Duration::from_secs(interval_seconds),
            max_upserts_per_tick,
        }
    }
}

fn env_u64(name: &str, default_value: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default_value)
}

fn env_usize(name: &str, default_value: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default_value)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn load_runtime_usize(state: &AppState, key: &str) -> usize {
    state
        .runtime_state
        .kv_get(key)
        .await
        .ok()
        .flatten()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0)
}

async fn store_runtime_usize(state: &AppState, key: &str, value: usize) {
    if let Err(err) = state
        .runtime_state
        .kv_set(key, value.to_string(), None)
        .await
    {
        debug!(
            key,
            error = ?err,
            "gateway pool score rebuild: failed to store cursor"
        );
    }
}

fn provider_offset_cursor_key(provider_id: &str) -> String {
    format!("{POOL_SCORE_REBUILD_PROVIDER_OFFSET_PREFIX}:{provider_id}")
}

pub(crate) async fn ensure_provider_key_pool_scores_for_keys(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    pool_config: &AdminProviderPoolConfig,
    _endpoints: &[StoredProviderCatalogEndpoint],
    keys: &[StoredProviderCatalogKey],
    now_unix_secs: u64,
    max_upserts: usize,
) -> Result<usize, GatewayError> {
    if max_upserts == 0
        || keys.is_empty()
        || !state.data.has_pool_score_reader()
        || !state.data.has_pool_score_writer()
    {
        return Ok(0);
    }

    let keys = keys
        .iter()
        .filter(|key| key.is_active && key.provider_id == provider.id)
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return Ok(0);
    }

    let build_items = keys
        .into_iter()
        .take(max_upserts)
        .map(|key| {
            let draft = build_provider_key_pool_score_upsert(
                key,
                provider.provider_type.as_str(),
                None,
                now_unix_secs,
                pool_config.score_rules,
            );
            (key, draft.id)
        })
        .collect::<Vec<_>>();
    if build_items.is_empty() {
        return Ok(0);
    }

    let existing_score_ids = state
        .data
        .get_pool_member_scores_by_ids(&GetPoolMemberScoresByIdsQuery {
            ids: build_items
                .iter()
                .map(|(_, score_id)| score_id.clone())
                .collect(),
        })
        .await
        .unwrap_or_else(|err| {
            debug!(
                provider_id = %provider.id,
                error = ?err,
                "gateway pool score ensure: failed to read existing scores by id"
            );
            Vec::new()
        })
        .into_iter()
        .map(|score| score.id)
        .collect::<std::collections::BTreeSet<_>>();

    let mut upserted = 0usize;
    for (key, score_id) in &build_items {
        if existing_score_ids.contains(score_id) {
            continue;
        }
        let upsert = build_provider_key_pool_score_upsert(
            key,
            provider.provider_type.as_str(),
            None,
            now_unix_secs,
            pool_config.score_rules,
        );
        if state
            .data
            .upsert_pool_member_score(upsert)
            .await
            .map_err(|err| GatewayError::Internal(format!("{err:?}")))?
            .is_some()
        {
            upserted = upserted.saturating_add(1);
        }
    }

    Ok(upserted)
}

pub(crate) async fn perform_pool_score_rebuild_once_with_config(
    state: &AppState,
    config: PoolScoreRebuildWorkerConfig,
) -> Result<PoolScoreRebuildRunSummary, GatewayError> {
    if !state.has_provider_catalog_data_reader()
        || !state.data.has_pool_score_reader()
        || !state.data.has_pool_score_writer()
    {
        return Ok(PoolScoreRebuildRunSummary::empty());
    }

    let mut providers = state
        .list_provider_catalog_providers(true)
        .await?
        .into_iter()
        .filter_map(|provider| {
            admin_provider_pool_config(&provider).map(|config| (provider, config))
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.0.id.cmp(&right.0.id));
    if providers.is_empty() {
        return Ok(PoolScoreRebuildRunSummary::empty());
    }

    let provider_ids = providers
        .iter()
        .map(|(provider, _)| provider.id.clone())
        .collect::<Vec<_>>();
    let mut key_ids_by_provider = BTreeMap::new();
    for summary in state
        .list_provider_catalog_key_maintenance_summaries_by_provider_ids(&provider_ids)
        .await?
    {
        if !summary.is_active {
            continue;
        }
        key_ids_by_provider
            .entry(summary.provider_id.clone())
            .or_insert_with(Vec::new)
            .push(summary.id);
    }
    for key_ids in key_ids_by_provider.values_mut() {
        key_ids.sort();
    }

    let now = now_unix_secs();
    let mut summary = PoolScoreRebuildRunSummary {
        providers_checked: providers.len(),
        ..PoolScoreRebuildRunSummary::empty()
    };

    let start_provider_index =
        load_runtime_usize(state, POOL_SCORE_REBUILD_PROVIDER_CURSOR_KEY).await % providers.len();
    let mut last_provider_index = None;
    for provider_index in
        (0..providers.len()).map(|offset| (start_provider_index + offset) % providers.len())
    {
        if summary.scores_upserted >= config.max_upserts_per_tick {
            break;
        }
        last_provider_index = Some(provider_index);
        let (provider, pool_config) = providers[provider_index].clone();
        let key_ids = key_ids_by_provider.remove(&provider.id).unwrap_or_default();
        if key_ids.is_empty() {
            continue;
        }
        let total_keys = key_ids.len();
        let provider_cursor_key = provider_offset_cursor_key(&provider.id);
        let provider_cursor = load_runtime_usize(state, &provider_cursor_key).await % total_keys;
        let remaining_budget = config
            .max_upserts_per_tick
            .saturating_sub(summary.scores_upserted);
        let provider_budget = remaining_budget.min(total_keys);
        let selected_ids = (0..provider_budget)
            .map(|offset| {
                let key_index = (provider_cursor + offset) % total_keys;
                key_ids[key_index].clone()
            })
            .collect::<Vec<_>>();
        let mut selected_keys_by_id = state
            .list_provider_catalog_keys_by_ids(&selected_ids)
            .await?
            .into_iter()
            .filter(|key| key.is_active && key.provider_id == provider.id)
            .map(|key| (key.id.clone(), key))
            .collect::<BTreeMap<_, _>>();
        let selected_keys = selected_ids
            .iter()
            .filter_map(|key_id| selected_keys_by_id.remove(key_id))
            .collect::<Vec<_>>();
        let mut build_items = Vec::with_capacity(selected_keys.len());
        for offset in 0..provider_budget {
            let Some(key) = selected_keys.get(offset) else {
                break;
            };
            let draft = build_provider_key_pool_score_upsert(
                key,
                provider.provider_type.as_str(),
                None,
                now,
                pool_config.score_rules,
            );
            build_items.push((offset, draft.id));
        }
        if build_items.is_empty() {
            store_runtime_usize(
                state,
                &provider_cursor_key,
                (provider_cursor + provider_budget) % total_keys,
            )
            .await;
            continue;
        }
        let existing_scores = state
            .data
            .get_pool_member_scores_by_ids(&GetPoolMemberScoresByIdsQuery {
                ids: build_items
                    .iter()
                    .map(|(_, score_id)| score_id.clone())
                    .collect(),
            })
            .await
            .unwrap_or_else(|err| {
                debug!(
                    provider_id = %provider.id,
                    error = ?err,
                    "gateway pool score rebuild: failed to read existing scores by id"
                );
                Vec::new()
            })
            .into_iter()
            .map(|score| (score.id.clone(), score))
            .collect::<BTreeMap<_, _>>();
        let mut provider_upserts = 0usize;
        summary.keys_seen = summary.keys_seen.saturating_add(total_keys);
        for (key_index, score_id) in &build_items {
            if summary.scores_upserted >= config.max_upserts_per_tick {
                break;
            }
            let key = &selected_keys[*key_index];
            let existing = existing_scores.get(score_id);
            let upsert = build_provider_key_pool_score_upsert(
                key,
                provider.provider_type.as_str(),
                existing,
                now,
                pool_config.score_rules,
            );
            if state
                .data
                .upsert_pool_member_score(upsert)
                .await
                .map_err(|err| GatewayError::Internal(format!("{err:?}")))?
                .is_some()
            {
                summary.scores_upserted = summary.scores_upserted.saturating_add(1);
                provider_upserts = provider_upserts.saturating_add(1);
            }
        }
        store_runtime_usize(
            state,
            &provider_cursor_key,
            (provider_cursor + provider_budget) % total_keys,
        )
        .await;
        if provider_upserts > 0 {
            summary.providers_scored = summary.providers_scored.saturating_add(1);
        }
    }
    if let Some(last_provider_index) = last_provider_index {
        store_runtime_usize(
            state,
            POOL_SCORE_REBUILD_PROVIDER_CURSOR_KEY,
            (last_provider_index + 1) % providers.len(),
        )
        .await;
    }

    Ok(summary)
}

pub(crate) async fn perform_pool_score_rebuild_once(
    state: &AppState,
) -> Result<PoolScoreRebuildRunSummary, GatewayError> {
    perform_pool_score_rebuild_once_with_config(state, PoolScoreRebuildWorkerConfig::from_env())
        .await
}

pub(crate) fn spawn_pool_score_rebuild_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    if !state.has_provider_catalog_data_reader()
        || !state.data.has_pool_score_reader()
        || !state.data.has_pool_score_writer()
    {
        return None;
    }

    let config = PoolScoreRebuildWorkerConfig::from_env();
    Some(crate::task_runtime::spawn_singleton_worker(
        state,
        crate::task_runtime::TASK_KEY_POOL_SCORE_REBUILD,
        move |state| async move {
            if let Err(err) = perform_pool_score_rebuild_once_with_config(&state, config).await {
                warn!(
                    error = ?err,
                    "gateway pool score rebuild initial tick failed"
                );
            }
            let mut interval = tokio::time::interval(config.interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if state
                    .data
                    .should_defer_maintenance_for_database_pool_pressure(&mut deferred_since)
                {
                    debug!(
                        event_name = "maintenance_worker_deferred",
                        log_type = "ops",
                        worker = "pool_score_rebuild",
                        "gateway pool score rebuild deferred because database pool has no idle reserve"
                    );
                    continue;
                }
                match perform_pool_score_rebuild_once_with_config(&state, config).await {
                    Ok(summary) if summary.scores_upserted > 0 => {
                        info!(
                            providers_checked = summary.providers_checked,
                            providers_scored = summary.providers_scored,
                            keys_seen = summary.keys_seen,
                            scores_upserted = summary.scores_upserted,
                            "gateway pool score rebuild completed"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!(
                            error = ?err,
                            "gateway pool score rebuild worker tick failed"
                        );
                    }
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use aether_data::repository::pool_scores::InMemoryPoolMemberScoreRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::pool_scores::PoolMemberIdentity;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogKeyListQuery, ProviderCatalogReadRepository,
        StoredProviderCatalogKeyMaintenanceSummary, StoredProviderCatalogKeyPage,
        StoredProviderCatalogKeyStats,
    };
    use aether_data_contracts::DataLayerError;
    use serde_json::json;

    use crate::data::GatewayDataState;

    struct NoWideKeyProviderCatalogReadRepository {
        inner: InMemoryProviderCatalogReadRepository,
    }

    impl NoWideKeyProviderCatalogReadRepository {
        fn seed(
            providers: Vec<StoredProviderCatalogProvider>,
            endpoints: Vec<StoredProviderCatalogEndpoint>,
            keys: Vec<StoredProviderCatalogKey>,
        ) -> Self {
            Self {
                inner: InMemoryProviderCatalogReadRepository::seed(providers, endpoints, keys),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProviderCatalogReadRepository for NoWideKeyProviderCatalogReadRepository {
        async fn list_providers(
            &self,
            active_only: bool,
        ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
            self.inner.list_providers(active_only).await
        }

        async fn list_providers_by_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
            self.inner.list_providers_by_ids(provider_ids).await
        }

        async fn list_endpoints_by_ids(
            &self,
            endpoint_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
            self.inner.list_endpoints_by_ids(endpoint_ids).await
        }

        async fn list_endpoints_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
            self.inner
                .list_endpoints_by_provider_ids(provider_ids)
                .await
        }

        async fn list_keys_by_ids(
            &self,
            key_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
            self.inner.list_keys_by_ids(key_ids).await
        }

        async fn list_keys_by_provider_ids(
            &self,
            _provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
            panic!("pool score rebuild should not read full provider key lists");
        }

        async fn list_key_summaries_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
            self.inner
                .list_key_summaries_by_provider_ids(provider_ids)
                .await
        }

        async fn list_key_maintenance_summaries_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKeyMaintenanceSummary>, DataLayerError> {
            self.inner
                .list_key_maintenance_summaries_by_provider_ids(provider_ids)
                .await
        }

        async fn list_keys_page(
            &self,
            query: &ProviderCatalogKeyListQuery,
        ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
            self.inner.list_keys_page(query).await
        }

        async fn list_key_stats_by_provider_ids(
            &self,
            provider_ids: &[String],
        ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
            self.inner
                .list_key_stats_by_provider_ids(provider_ids)
                .await
        }
    }

    fn provider(id: &str) -> StoredProviderCatalogProvider {
        let mut provider = StoredProviderCatalogProvider::new(
            id.to_string(),
            id.to_string(),
            None,
            "openai".to_string(),
        )
        .expect("provider should build");
        provider.config = Some(json!({ "pool_advanced": {} }));
        provider
    }

    fn key(id: &str, active: bool) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            id.to_string(),
            "provider-1".to_string(),
            id.to_string(),
            "oauth".to_string(),
            None,
            active,
        )
        .expect("key should build")
    }

    fn score_id(provider_id: &str, key_id: &str) -> String {
        let identity =
            PoolMemberIdentity::provider_api_key(provider_id.to_string(), key_id.to_string());
        crate::ai_serving::provider_key_pool_score_id(
            &identity,
            &crate::ai_serving::provider_key_pool_score_scope(),
        )
    }

    #[tokio::test]
    async fn pool_score_rebuild_uses_maintenance_summaries_before_full_key_load() {
        let provider = provider("provider-1");
        let provider_catalog_repository = Arc::new(NoWideKeyProviderCatalogReadRepository::seed(
            vec![provider],
            Vec::new(),
            vec![
                key("key-a", true),
                key("key-b", true),
                key("key-disabled", false),
            ],
        ));
        let pool_score_repository = Arc::new(InMemoryPoolMemberScoreRepository::default());
        let data =
            GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository)
                .with_pool_score_repository_for_tests(Arc::clone(&pool_score_repository));
        let state = AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data);

        let summary = perform_pool_score_rebuild_once_with_config(
            &state,
            PoolScoreRebuildWorkerConfig {
                interval: Duration::from_secs(60),
                max_upserts_per_tick: 1,
            },
        )
        .await
        .expect("rebuild should succeed");

        assert_eq!(
            summary,
            PoolScoreRebuildRunSummary {
                providers_checked: 1,
                providers_scored: 1,
                keys_seen: 2,
                scores_upserted: 1,
            }
        );

        let scores = state
            .data
            .get_pool_member_scores_by_ids(&GetPoolMemberScoresByIdsQuery {
                ids: vec![
                    score_id("provider-1", "key-a"),
                    score_id("provider-1", "key-b"),
                ],
            })
            .await
            .expect("scores should load");
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].member_id, "key-a");
    }
}
