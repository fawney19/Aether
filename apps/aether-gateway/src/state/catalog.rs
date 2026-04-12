use super::{AppState, GatewayError, LocalMutationOutcome, LocalProviderDeleteTaskState};
use aether_data_contracts::repository::{candidates, global_models, provider_catalog};

impl AppState {
    pub fn has_provider_catalog_data_reader(&self) -> bool {
        self.data.has_provider_catalog_reader()
    }

    pub(crate) fn has_provider_catalog_data_writer(&self) -> bool {
        self.data.has_provider_catalog_writer()
    }

    pub(crate) fn has_global_model_data_reader(&self) -> bool {
        self.data.has_global_model_reader()
    }

    pub(crate) fn has_global_model_data_writer(&self) -> bool {
        self.data.has_global_model_writer()
    }

    pub(crate) fn has_minimal_candidate_selection_reader(&self) -> bool {
        self.data.has_minimal_candidate_selection_reader()
    }

    pub(crate) fn has_management_token_reader(&self) -> bool {
        self.data.has_management_token_reader()
    }

    pub(crate) fn has_management_token_writer(&self) -> bool {
        self.data.has_management_token_writer()
    }

    pub(crate) async fn list_provider_catalog_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogProvider>, GatewayError> {
        self.data
            .list_provider_catalog_providers(active_only)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_provider_catalog_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogEndpoint>, GatewayError> {
        self.data
            .list_provider_catalog_endpoints_by_provider_ids(provider_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_public_global_models(
        &self,
        query: &global_models::PublicGlobalModelQuery,
    ) -> Result<global_models::StoredPublicGlobalModelPage, GatewayError> {
        self.data
            .list_public_global_models(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_management_tokens(
        &self,
        query: &aether_data::repository::management_tokens::ManagementTokenListQuery,
    ) -> Result<
        aether_data::repository::management_tokens::StoredManagementTokenListPage,
        GatewayError,
    > {
        self.data
            .list_management_tokens(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn get_management_token_with_user(
        &self,
        token_id: &str,
    ) -> Result<
        Option<aether_data::repository::management_tokens::StoredManagementTokenWithUser>,
        GatewayError,
    > {
        self.data
            .get_management_token_with_user(token_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn get_management_token_with_user_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<
        Option<aether_data::repository::management_tokens::StoredManagementTokenWithUser>,
        GatewayError,
    > {
        self.data
            .get_management_token_with_user_by_hash(token_hash)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_management_token(
        &self,
        record: &aether_data::repository::management_tokens::CreateManagementTokenRecord,
    ) -> Result<
        LocalMutationOutcome<aether_data::repository::management_tokens::StoredManagementToken>,
        GatewayError,
    > {
        self.data
            .create_management_token(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_management_token(
        &self,
        record: &aether_data::repository::management_tokens::UpdateManagementTokenRecord,
    ) -> Result<
        LocalMutationOutcome<aether_data::repository::management_tokens::StoredManagementToken>,
        GatewayError,
    > {
        self.data
            .update_management_token(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_management_token(
        &self,
        token_id: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .delete_management_token(token_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn record_management_token_usage(
        &self,
        token_id: &str,
        last_used_ip: Option<&str>,
    ) -> Result<
        Option<aether_data::repository::management_tokens::StoredManagementToken>,
        GatewayError,
    > {
        self.data
            .record_management_token_usage(token_id, last_used_ip)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn set_management_token_active(
        &self,
        token_id: &str,
        is_active: bool,
    ) -> Result<
        Option<aether_data::repository::management_tokens::StoredManagementToken>,
        GatewayError,
    > {
        self.data
            .set_management_token_active(token_id, is_active)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn regenerate_management_token_secret(
        &self,
        mutation: &aether_data::repository::management_tokens::RegenerateManagementTokenSecret,
    ) -> Result<
        LocalMutationOutcome<aether_data::repository::management_tokens::StoredManagementToken>,
        GatewayError,
    > {
        self.data
            .regenerate_management_token_secret(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn get_public_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<global_models::StoredPublicGlobalModel>, GatewayError> {
        self.data
            .get_public_global_model_by_name(model_name)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_public_catalog_models(
        &self,
        query: &global_models::PublicCatalogModelListQuery,
    ) -> Result<Vec<global_models::StoredPublicCatalogModel>, GatewayError> {
        self.data
            .list_public_catalog_models(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn search_public_catalog_models(
        &self,
        query: &global_models::PublicCatalogModelSearchQuery,
    ) -> Result<Vec<global_models::StoredPublicCatalogModel>, GatewayError> {
        self.data
            .search_public_catalog_models(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_admin_provider_models(
        &self,
        query: &global_models::AdminProviderModelListQuery,
    ) -> Result<Vec<global_models::StoredAdminProviderModel>, GatewayError> {
        self.data
            .list_admin_provider_models(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_admin_global_models(
        &self,
        query: &global_models::AdminGlobalModelListQuery,
    ) -> Result<global_models::StoredAdminGlobalModelPage, GatewayError> {
        self.data
            .list_admin_global_models(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<global_models::StoredAdminProviderModel>, GatewayError> {
        self.data
            .get_admin_provider_model(provider_id, model_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<global_models::StoredAdminProviderModel>, GatewayError> {
        self.data
            .list_admin_provider_available_source_models(provider_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<global_models::StoredAdminGlobalModel>, GatewayError> {
        self.data
            .get_admin_global_model_by_id(global_model_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<global_models::StoredAdminGlobalModel>, GatewayError> {
        self.data
            .get_admin_global_model_by_name(model_name)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<global_models::StoredAdminProviderModel>, GatewayError> {
        self.data
            .list_admin_provider_models_by_global_model_id(global_model_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_admin_provider_model(
        &self,
        record: &global_models::UpsertAdminProviderModelRecord,
    ) -> Result<Option<global_models::StoredAdminProviderModel>, GatewayError> {
        self.data
            .create_admin_provider_model(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_admin_provider_model(
        &self,
        record: &global_models::UpsertAdminProviderModelRecord,
    ) -> Result<Option<global_models::StoredAdminProviderModel>, GatewayError> {
        self.data
            .update_admin_provider_model(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .delete_admin_provider_model(provider_id, model_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_admin_global_model(
        &self,
        record: &global_models::CreateAdminGlobalModelRecord,
    ) -> Result<Option<global_models::StoredAdminGlobalModel>, GatewayError> {
        self.data
            .create_admin_global_model(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_admin_global_model(
        &self,
        record: &global_models::UpdateAdminGlobalModelRecord,
    ) -> Result<Option<global_models::StoredAdminGlobalModel>, GatewayError> {
        self.data
            .update_admin_global_model(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .delete_admin_global_model(global_model_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<global_models::StoredProviderModelStats>, GatewayError> {
        self.data
            .list_provider_model_stats(provider_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<global_models::StoredProviderActiveGlobalModel>, GatewayError> {
        self.data
            .list_active_global_model_ids_by_provider_ids(provider_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_finalized_request_candidates_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<candidates::StoredRequestCandidate>, GatewayError> {
        self.data
            .list_finalized_request_candidates_by_endpoint_ids_since(
                endpoint_ids,
                since_unix_secs,
                limit,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_finalized_request_candidate_statuses_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
    ) -> Result<Vec<candidates::PublicHealthStatusCount>, GatewayError> {
        self.data
            .count_finalized_request_candidate_statuses_by_endpoint_ids_since(
                endpoint_ids,
                since_unix_secs,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        until_unix_secs: u64,
        segments: u32,
    ) -> Result<Vec<candidates::PublicHealthTimelineBucket>, GatewayError> {
        self.data
            .aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(
                endpoint_ids,
                since_unix_secs,
                until_unix_secs,
                segments,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_provider_catalog_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogKey>, GatewayError> {
        self.data
            .list_provider_catalog_keys_by_provider_ids(provider_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_provider_catalog_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogKey>, GatewayError> {
        self.data
            .list_provider_catalog_key_summaries_by_provider_ids(provider_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_provider_catalog_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogKey>, GatewayError> {
        self.data
            .list_provider_catalog_keys_by_ids(key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_provider_catalog_key_page(
        &self,
        query: &provider_catalog::ProviderCatalogKeyListQuery,
    ) -> Result<provider_catalog::StoredProviderCatalogKeyPage, GatewayError> {
        self.data
            .list_provider_catalog_key_page(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_provider_catalog_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogKeyStats>, GatewayError> {
        self.data
            .list_provider_catalog_key_stats_by_provider_ids(provider_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_provider_catalog_key(
        &self,
        key: &provider_catalog::StoredProviderCatalogKey,
    ) -> Result<Option<provider_catalog::StoredProviderCatalogKey>, GatewayError> {
        let created = self
            .data
            .create_provider_catalog_key(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if created.is_some() {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(created)
    }

    pub(crate) async fn create_provider_catalog_provider(
        &self,
        provider: &provider_catalog::StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<Option<provider_catalog::StoredProviderCatalogProvider>, GatewayError> {
        let created = self
            .data
            .create_provider_catalog_provider(provider, shift_existing_priorities_from)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if created.is_some() {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(created)
    }

    pub(crate) async fn update_provider_catalog_provider(
        &self,
        provider: &provider_catalog::StoredProviderCatalogProvider,
    ) -> Result<Option<provider_catalog::StoredProviderCatalogProvider>, GatewayError> {
        let updated = self
            .data
            .update_provider_catalog_provider(provider)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated.is_some() {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(updated)
    }

    pub(crate) async fn delete_provider_catalog_provider(
        &self,
        provider_id: &str,
    ) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_provider_catalog_provider(provider_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if deleted {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(deleted)
    }

    pub(crate) async fn cleanup_deleted_provider_catalog_refs(
        &self,
        provider_id: &str,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), GatewayError> {
        self.data
            .cleanup_deleted_provider_catalog_refs(provider_id, endpoint_ids, key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if !endpoint_ids.is_empty() || !key_ids.is_empty() {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(())
    }

    pub(crate) async fn create_provider_catalog_endpoint(
        &self,
        endpoint: &provider_catalog::StoredProviderCatalogEndpoint,
    ) -> Result<Option<provider_catalog::StoredProviderCatalogEndpoint>, GatewayError> {
        let created = self
            .data
            .create_provider_catalog_endpoint(endpoint)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if created.is_some() {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(created)
    }

    pub(crate) async fn update_provider_catalog_endpoint(
        &self,
        endpoint: &provider_catalog::StoredProviderCatalogEndpoint,
    ) -> Result<Option<provider_catalog::StoredProviderCatalogEndpoint>, GatewayError> {
        let updated = self
            .data
            .update_provider_catalog_endpoint(endpoint)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated.is_some() {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(updated)
    }

    pub(crate) async fn delete_provider_catalog_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_provider_catalog_endpoint(endpoint_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if deleted {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(deleted)
    }

    pub(crate) async fn update_provider_catalog_key(
        &self,
        key: &provider_catalog::StoredProviderCatalogKey,
    ) -> Result<Option<provider_catalog::StoredProviderCatalogKey>, GatewayError> {
        let updated = self
            .data
            .update_provider_catalog_key(key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated.is_some() {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(updated)
    }

    pub(crate) async fn delete_provider_catalog_key(
        &self,
        key_id: &str,
    ) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_provider_catalog_key(key_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if deleted {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(deleted)
    }

    pub(crate) async fn clear_provider_catalog_key_oauth_invalid_marker(
        &self,
        key_id: &str,
    ) -> Result<bool, GatewayError> {
        self.data
            .clear_provider_catalog_key_oauth_invalid_marker(key_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) fn put_provider_delete_task(&self, task: LocalProviderDeleteTaskState) {
        let mut tasks = self
            .provider_delete_tasks
            .lock()
            .expect("provider delete tasks cache should lock");
        tasks.insert(task.task_id.clone(), task);
    }

    pub(crate) fn get_provider_delete_task(
        &self,
        task_id: &str,
    ) -> Option<LocalProviderDeleteTaskState> {
        let tasks = self
            .provider_delete_tasks
            .lock()
            .expect("provider delete tasks cache should lock");
        tasks.get(task_id).cloned()
    }

    pub(crate) async fn read_provider_catalog_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogProvider>, GatewayError> {
        self.data
            .list_provider_catalog_providers_by_ids(provider_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn read_provider_catalog_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogEndpoint>, GatewayError> {
        self.data
            .list_provider_catalog_endpoints_by_ids(endpoint_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn read_provider_catalog_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<provider_catalog::StoredProviderCatalogKey>, GatewayError> {
        self.data
            .list_provider_catalog_keys_by_ids(key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_provider_catalog_key_health_state(
        &self,
        key_id: &str,
        is_active: bool,
        health_by_format: Option<&serde_json::Value>,
        circuit_breaker_by_format: Option<&serde_json::Value>,
    ) -> Result<bool, GatewayError> {
        let updated = self
            .data
            .update_provider_catalog_key_health_state(
                key_id,
                is_active,
                health_by_format,
                circuit_breaker_by_format,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated {
            self.clear_provider_transport_snapshot_cache();
        }
        Ok(updated)
    }
}
