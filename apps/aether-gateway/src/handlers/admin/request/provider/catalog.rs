use super::*;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn read_provider_catalog_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        self.app.read_provider_catalog_keys_by_ids(key_ids).await
    }

    pub(crate) async fn read_provider_catalog_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider>,
        GatewayError,
    > {
        self.app
            .read_provider_catalog_providers_by_ids(provider_ids)
            .await
    }

    pub(crate) async fn read_provider_catalog_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint>,
        GatewayError,
    > {
        self.app
            .read_provider_catalog_endpoints_by_ids(endpoint_ids)
            .await
    }

    pub(crate) async fn list_provider_catalog_providers(
        &self,
        active_only: bool,
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider>,
        GatewayError,
    > {
        self.app.list_provider_catalog_providers(active_only).await
    }

    pub(crate) async fn list_provider_catalog_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint>,
        GatewayError,
    > {
        self.app
            .list_provider_catalog_endpoints_by_provider_ids(provider_ids)
            .await
    }

    pub(crate) async fn list_provider_catalog_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        self.app
            .list_provider_catalog_keys_by_provider_ids(provider_ids)
            .await
    }

    pub(crate) async fn list_provider_catalog_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        self.app
            .list_provider_catalog_key_summaries_by_provider_ids(provider_ids)
            .await
    }

    pub(crate) async fn list_provider_catalog_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        self.app.list_provider_catalog_keys_by_ids(key_ids).await
    }

    pub(crate) async fn list_provider_catalog_key_page(
        &self,
        query: &aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyListQuery,
    ) -> Result<
        aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKeyPage,
        GatewayError,
    > {
        self.app.list_provider_catalog_key_page(query).await
    }

    pub(crate) async fn list_provider_catalog_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKeyStats>,
        GatewayError,
    > {
        self.app
            .list_provider_catalog_key_stats_by_provider_ids(provider_ids)
            .await
    }

    pub(crate) async fn read_provider_quota_snapshots(
        &self,
        provider_ids: &[String],
    ) -> Result<
        Vec<aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot>,
        GatewayError,
    > {
        self.app.read_provider_quota_snapshots(provider_ids).await
    }

    pub(crate) async fn update_provider_catalog_key_health_state(
        &self,
        key_id: &str,
        is_active: bool,
        health_by_format: Option<&serde_json::Value>,
        circuit_breaker_by_format: Option<&serde_json::Value>,
    ) -> Result<bool, GatewayError> {
        self.app
            .update_provider_catalog_key_health_state(
                key_id,
                is_active,
                health_by_format,
                circuit_breaker_by_format,
            )
            .await
    }

    pub(crate) async fn create_provider_catalog_endpoint(
        &self,
        endpoint: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint>,
        GatewayError,
    > {
        self.app.create_provider_catalog_endpoint(endpoint).await
    }

    pub(crate) async fn update_provider_catalog_endpoint(
        &self,
        endpoint: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint>,
        GatewayError,
    > {
        self.app.update_provider_catalog_endpoint(endpoint).await
    }

    pub(crate) async fn delete_provider_catalog_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app.delete_provider_catalog_endpoint(endpoint_id).await
    }

    pub(crate) async fn update_provider_catalog_key(
        &self,
        key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        self.app.update_provider_catalog_key(key).await
    }

    pub(crate) async fn create_provider_catalog_key(
        &self,
        key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        self.app.create_provider_catalog_key(key).await
    }

    pub(crate) async fn delete_provider_catalog_key(
        &self,
        key_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app.delete_provider_catalog_key(key_id).await
    }

    pub(crate) async fn create_provider_catalog_provider(
        &self,
        provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider>,
        GatewayError,
    > {
        self.app
            .create_provider_catalog_provider(provider, shift_existing_priorities_from)
            .await
    }

    pub(crate) async fn update_provider_catalog_provider(
        &self,
        provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider>,
        GatewayError,
    > {
        self.app.update_provider_catalog_provider(provider).await
    }

    pub(crate) async fn cleanup_deleted_provider_catalog_refs(
        &self,
        provider_id: &str,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), GatewayError> {
        self.app
            .cleanup_deleted_provider_catalog_refs(provider_id, endpoint_ids, key_ids)
            .await
    }
}
