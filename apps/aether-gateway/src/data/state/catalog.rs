use super::{
    DataLayerError, GatewayDataState, GeminiFileMappingListQuery, GeminiFileMappingStats,
    ProviderCatalogKeyListQuery, PublicHealthStatusCount, PublicHealthTimelineBucket,
    StoredGeminiFileMapping, StoredGeminiFileMappingListPage, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogKeyPage, StoredProviderCatalogKeyStats,
    StoredProviderCatalogProvider, StoredRequestCandidate, UpsertGeminiFileMappingRecord,
    UpsertRequestCandidateRecord,
};

impl GatewayDataState {
    pub(crate) async fn list_request_candidates_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        match &self.request_candidate_reader {
            Some(repository) => repository.list_by_request_id(request_id).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_request_candidates_by_provider_id(
        &self,
        provider_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        match &self.request_candidate_reader {
            Some(repository) => repository.list_by_provider_id(provider_id, limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_recent_request_candidates(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        match &self.request_candidate_reader {
            Some(repository) => repository.list_recent(limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_finalized_request_candidates_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        match &self.request_candidate_reader {
            Some(repository) => {
                repository
                    .list_finalized_by_endpoint_ids_since(endpoint_ids, since_unix_secs, limit)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_finalized_request_candidate_statuses_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
    ) -> Result<Vec<PublicHealthStatusCount>, DataLayerError> {
        match &self.request_candidate_reader {
            Some(repository) => {
                repository
                    .count_finalized_statuses_by_endpoint_ids_since(endpoint_ids, since_unix_secs)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        until_unix_secs: u64,
        segments: u32,
    ) -> Result<Vec<PublicHealthTimelineBucket>, DataLayerError> {
        match &self.request_candidate_reader {
            Some(repository) => {
                repository
                    .aggregate_finalized_timeline_by_endpoint_ids_since(
                        endpoint_ids,
                        since_unix_secs,
                        until_unix_secs,
                        segments,
                    )
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn upsert_request_candidate(
        &self,
        candidate: UpsertRequestCandidateRecord,
    ) -> Result<Option<StoredRequestCandidate>, DataLayerError> {
        match &self.request_candidate_writer {
            Some(repository) => repository.upsert(candidate).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_request_candidates_created_before(
        &self,
        created_before_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        match &self.request_candidate_writer {
            Some(repository) => {
                repository
                    .delete_created_before(created_before_unix_secs, limit)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn touch_auth_api_key_last_used(
        &self,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.touch_last_used_at(api_key_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn upsert_gemini_file_mapping(
        &self,
        record: UpsertGeminiFileMappingRecord,
    ) -> Result<Option<StoredGeminiFileMapping>, DataLayerError> {
        match &self.gemini_file_mapping_writer {
            Some(repository) => repository.upsert(record).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn list_gemini_file_mappings(
        &self,
        query: &GeminiFileMappingListQuery,
    ) -> Result<StoredGeminiFileMappingListPage, DataLayerError> {
        match &self.gemini_file_mapping_reader {
            Some(repository) => repository.list_mappings(query).await,
            None => Ok(StoredGeminiFileMappingListPage {
                items: Vec::new(),
                total: 0,
            }),
        }
    }

    pub(crate) async fn summarize_gemini_file_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<GeminiFileMappingStats, DataLayerError> {
        match &self.gemini_file_mapping_reader {
            Some(repository) => repository.summarize_mappings(now_unix_secs).await,
            None => Ok(GeminiFileMappingStats {
                total_mappings: 0,
                active_mappings: 0,
                expired_mappings: 0,
                by_mime_type: Vec::new(),
            }),
        }
    }

    pub(crate) async fn delete_gemini_file_mapping_by_file_name(
        &self,
        file_name: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.gemini_file_mapping_writer {
            Some(repository) => repository.delete_by_file_name(file_name).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn delete_gemini_file_mapping_by_id(
        &self,
        mapping_id: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, DataLayerError> {
        match &self.gemini_file_mapping_writer {
            Some(repository) => repository.delete_by_id(mapping_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_expired_gemini_file_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<usize, DataLayerError> {
        match &self.gemini_file_mapping_writer {
            Some(repository) => repository.delete_expired_before(now_unix_secs).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn cache_set_string_with_ttl(
        &self,
        key: &str,
        value: &str,
        ttl_seconds: u64,
    ) -> Result<(), DataLayerError> {
        let Some(runner) = self.kv_runner() else {
            return Ok(());
        };
        runner.setex(key, value, Some(ttl_seconds)).await?;
        Ok(())
    }

    pub(crate) async fn cache_delete_key(&self, key: &str) -> Result<(), DataLayerError> {
        let Some(runner) = self.kv_runner() else {
            return Ok(());
        };
        let _deleted = runner.del(key).await?;
        Ok(())
    }

    pub(crate) async fn list_provider_catalog_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => repository.list_providers_by_ids(provider_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_provider_catalog_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => repository.list_providers(active_only).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_provider_catalog_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => repository.list_endpoints_by_ids(endpoint_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_provider_catalog_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => {
                repository
                    .list_endpoints_by_provider_ids(provider_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_provider_catalog_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => repository.list_keys_by_ids(key_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_provider_catalog_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => repository.list_keys_by_provider_ids(provider_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_provider_catalog_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => {
                repository
                    .list_key_summaries_by_provider_ids(provider_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_provider_catalog_key_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => repository.list_keys_page(query).await,
            None => Ok(StoredProviderCatalogKeyPage {
                items: Vec::new(),
                total: 0,
            }),
        }
    }

    pub(crate) async fn list_provider_catalog_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, DataLayerError> {
        match &self.provider_catalog_reader {
            Some(repository) => {
                repository
                    .list_key_stats_by_provider_ids(provider_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn update_provider_catalog_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => {
                repository
                    .update_key_oauth_credentials(
                        key_id,
                        encrypted_api_key,
                        encrypted_auth_config,
                        expires_at_unix_secs,
                    )
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn create_provider_catalog_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<Option<StoredProviderCatalogKey>, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.create_key(key).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn create_provider_catalog_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<Option<StoredProviderCatalogProvider>, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository
                .create_provider(provider, shift_existing_priorities_from)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn update_provider_catalog_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<Option<StoredProviderCatalogProvider>, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.update_provider(provider).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_provider_catalog_provider(
        &self,
        provider_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.delete_provider(provider_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn cleanup_deleted_provider_catalog_refs(
        &self,
        provider_id: &str,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => {
                repository
                    .cleanup_deleted_provider_refs(provider_id, endpoint_ids, key_ids)
                    .await
            }
            None => Ok(()),
        }
    }

    pub(crate) async fn create_provider_catalog_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<Option<StoredProviderCatalogEndpoint>, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.create_endpoint(endpoint).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn update_provider_catalog_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<Option<StoredProviderCatalogEndpoint>, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.update_endpoint(endpoint).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_provider_catalog_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.delete_endpoint(endpoint_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn update_provider_catalog_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<Option<StoredProviderCatalogKey>, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.update_key(key).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_provider_catalog_key(
        &self,
        key_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.delete_key(key_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn clear_provider_catalog_key_oauth_invalid_marker(
        &self,
        key_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => repository.clear_key_oauth_invalid_marker(key_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn update_provider_catalog_key_health_state(
        &self,
        key_id: &str,
        is_active: bool,
        health_by_format: Option<&serde_json::Value>,
        circuit_breaker_by_format: Option<&serde_json::Value>,
    ) -> Result<bool, DataLayerError> {
        match &self.provider_catalog_writer {
            Some(repository) => {
                repository
                    .update_key_health_state(
                        key_id,
                        is_active,
                        health_by_format,
                        circuit_breaker_by_format,
                    )
                    .await
            }
            None => Ok(false),
        }
    }
}
