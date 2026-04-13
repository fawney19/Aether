use std::collections::{BTreeMap, BTreeSet};

use aether_data::repository::auth::{AuthApiKeyLookupKey, ResolvedAuthApiKeySnapshotReader};

use crate::{AppState, GatewayError};

use super::super::super::{AUTH_API_KEY_LAST_USED_MAX_ENTRIES, AUTH_API_KEY_LAST_USED_TTL};

impl AppState {
    pub(crate) async fn resolve_auth_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeySnapshot>, GatewayError> {
        if !self.has_auth_api_key_data_reader() {
            return Ok(Vec::new());
        }

        let api_key_ids = api_key_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut snapshots = self
            .data
            .list_auth_api_key_snapshots_by_ids(&api_key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .into_iter()
            .map(|snapshot| (snapshot.api_key_id.clone(), snapshot))
            .collect::<BTreeMap<_, _>>();

        for api_key_id in &api_key_ids {
            if snapshots.contains_key(api_key_id) {
                continue;
            }
            let snapshot = self
                .data
                .find_stored_auth_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId(api_key_id))
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            if let Some(snapshot) = snapshot {
                snapshots.insert(api_key_id.clone(), snapshot);
            }
        }

        Ok(snapshots.into_values().collect())
    }

    pub(crate) async fn resolve_auth_api_key_names_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<BTreeMap<String, String>, GatewayError> {
        if !self.has_auth_api_key_data_reader() {
            return Ok(BTreeMap::new());
        }

        let api_key_ids = api_key_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if api_key_ids.is_empty() {
            return Ok(BTreeMap::new());
        }

        let mut names = self
            .data
            .list_auth_api_key_snapshots_by_ids(&api_key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
            .into_iter()
            .filter_map(|snapshot| {
                snapshot
                    .api_key_name
                    .map(|name| (snapshot.api_key_id, name))
            })
            .collect::<BTreeMap<_, _>>();

        for api_key_id in &api_key_ids {
            if names.contains_key(api_key_id) {
                continue;
            }
            let snapshot = self
                .data
                .find_stored_auth_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId(api_key_id))
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            if let Some(name) = snapshot.and_then(|snapshot| snapshot.api_key_name) {
                names.insert(api_key_id.clone(), name);
            }
        }

        Ok(names)
    }

    pub(crate) async fn touch_auth_api_key_last_used_best_effort(&self, api_key_id: &str) {
        let api_key_id = api_key_id.trim();
        if api_key_id.is_empty() || !self.data.has_auth_api_key_writer() {
            return;
        }
        if !self.auth_api_key_last_used_cache.should_touch(
            api_key_id,
            AUTH_API_KEY_LAST_USED_TTL,
            AUTH_API_KEY_LAST_USED_MAX_ENTRIES,
        ) {
            return;
        }
        if let Err(err) = self.data.touch_auth_api_key_last_used(api_key_id).await {
            tracing::warn!(
                api_key_id = %api_key_id,
                error = ?err,
                "gateway auth api key last_used_at touch failed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use aether_data::repository::auth::{
        AuthApiKeyExportSummary, AuthApiKeyLookupKey, AuthApiKeyReadRepository,
        InMemoryAuthApiKeySnapshotRepository, StandaloneApiKeyExportListQuery,
        StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
    };
    use async_trait::async_trait;

    use crate::AppState;

    #[derive(Debug)]
    struct PartialListAuthApiKeyRepository {
        lookup: InMemoryAuthApiKeySnapshotRepository,
    }

    #[async_trait]
    impl AuthApiKeyReadRepository for PartialListAuthApiKeyRepository {
        async fn find_api_key_snapshot(
            &self,
            key: AuthApiKeyLookupKey<'_>,
        ) -> Result<Option<StoredAuthApiKeySnapshot>, aether_data::DataLayerError> {
            self.lookup.find_api_key_snapshot(key).await
        }

        async fn list_api_key_snapshots_by_ids(
            &self,
            _api_key_ids: &[String],
        ) -> Result<Vec<StoredAuthApiKeySnapshot>, aether_data::DataLayerError> {
            Ok(Vec::new())
        }

        async fn list_export_api_keys_by_user_ids(
            &self,
            user_ids: &[String],
        ) -> Result<Vec<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
            self.lookup.list_export_api_keys_by_user_ids(user_ids).await
        }

        async fn list_export_api_keys_by_ids(
            &self,
            api_key_ids: &[String],
        ) -> Result<Vec<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
            self.lookup.list_export_api_keys_by_ids(api_key_ids).await
        }

        async fn list_export_standalone_api_keys_page(
            &self,
            query: &StandaloneApiKeyExportListQuery,
        ) -> Result<Vec<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
            self.lookup
                .list_export_standalone_api_keys_page(query)
                .await
        }

        async fn count_export_standalone_api_keys(
            &self,
            is_active: Option<bool>,
        ) -> Result<u64, aether_data::DataLayerError> {
            self.lookup
                .count_export_standalone_api_keys(is_active)
                .await
        }

        async fn summarize_export_api_keys_by_user_ids(
            &self,
            user_ids: &[String],
            now_unix_secs: u64,
        ) -> Result<AuthApiKeyExportSummary, aether_data::DataLayerError> {
            self.lookup
                .summarize_export_api_keys_by_user_ids(user_ids, now_unix_secs)
                .await
        }

        async fn summarize_export_non_standalone_api_keys(
            &self,
            now_unix_secs: u64,
        ) -> Result<AuthApiKeyExportSummary, aether_data::DataLayerError> {
            self.lookup
                .summarize_export_non_standalone_api_keys(now_unix_secs)
                .await
        }

        async fn summarize_export_standalone_api_keys(
            &self,
            now_unix_secs: u64,
        ) -> Result<AuthApiKeyExportSummary, aether_data::DataLayerError> {
            self.lookup
                .summarize_export_standalone_api_keys(now_unix_secs)
                .await
        }

        async fn find_export_standalone_api_key_by_id(
            &self,
            api_key_id: &str,
        ) -> Result<Option<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
            self.lookup
                .find_export_standalone_api_key_by_id(api_key_id)
                .await
        }

        async fn list_export_standalone_api_keys(
            &self,
        ) -> Result<Vec<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
            self.lookup.list_export_standalone_api_keys().await
        }
    }

    fn sample_usage_auth_snapshot(
        api_key_id: &str,
        user_id: &str,
        api_key_name: &str,
    ) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            None,
            None,
            None,
            api_key_id.to_string(),
            Some(api_key_name.to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            None,
            None,
            None,
            None,
        )
        .expect("auth api key snapshot should build")
    }

    #[tokio::test]
    async fn resolve_auth_api_key_names_by_ids_falls_back_to_single_lookup_for_missing_list_rows() {
        let repository = Arc::new(PartialListAuthApiKeyRepository {
            lookup: InMemoryAuthApiKeySnapshotRepository::seed(vec![(
                None,
                sample_usage_auth_snapshot("key-1", "user-1", "fresh-default"),
            )]),
        });
        let state = AppState::new()
            .expect("state should build")
            .with_auth_api_key_data_reader_for_tests(repository);

        let names = state
            .resolve_auth_api_key_names_by_ids(&["key-1".to_string()])
            .await
            .expect("api key name resolution should succeed");

        assert_eq!(
            names,
            BTreeMap::from([("key-1".to_string(), "fresh-default".to_string())])
        );
    }

    #[tokio::test]
    async fn resolve_auth_api_key_snapshots_by_ids_falls_back_to_single_lookup_for_missing_list_rows(
    ) {
        let repository = Arc::new(PartialListAuthApiKeyRepository {
            lookup: InMemoryAuthApiKeySnapshotRepository::seed(vec![(
                None,
                sample_usage_auth_snapshot("key-1", "user-1", "fresh-default"),
            )]),
        });
        let state = AppState::new()
            .expect("state should build")
            .with_auth_api_key_data_reader_for_tests(repository);

        let snapshots = state
            .resolve_auth_api_key_snapshots_by_ids(&["key-1".to_string()])
            .await
            .expect("api key snapshot resolution should succeed");

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].api_key_id, "key-1");
        assert_eq!(snapshots[0].api_key_name.as_deref(), Some("fresh-default"));
    }
}
