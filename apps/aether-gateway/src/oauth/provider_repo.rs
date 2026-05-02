use crate::admin_api::{
    create_provider_oauth_catalog_key, find_duplicate_provider_oauth_key,
    refresh_provider_oauth_account_state_after_update, update_existing_provider_oauth_catalog_key,
    AdminAppState, AdminGatewayProviderTransportSnapshot, AdminLocalOAuthRefreshError,
};
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ProviderOAuthRepository;

impl ProviderOAuthRepository {
    pub(crate) async fn update_provider_catalog_key_oauth_credentials(
        state: &AdminAppState<'_>,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, GatewayError> {
        state
            .app()
            .update_provider_catalog_key_oauth_credentials(
                key_id,
                encrypted_api_key,
                encrypted_auth_config,
                expires_at_unix_secs,
            )
            .await
    }

    pub(crate) async fn clear_provider_catalog_key_oauth_invalid_marker(
        state: &AdminAppState<'_>,
        key_id: &str,
    ) -> Result<bool, GatewayError> {
        state
            .app()
            .clear_provider_catalog_key_oauth_invalid_marker(key_id)
            .await
    }

    pub(crate) async fn force_local_oauth_refresh_entry(
        state: &AdminAppState<'_>,
        transport: &AdminGatewayProviderTransportSnapshot,
    ) -> Result<Option<crate::provider_transport::CachedOAuthEntry>, AdminLocalOAuthRefreshError>
    {
        state.app().force_local_oauth_refresh_entry(transport).await
    }

    pub(crate) async fn find_duplicate_provider_oauth_key(
        state: &AdminAppState<'_>,
        provider_id: &str,
        auth_config: &serde_json::Map<String, serde_json::Value>,
        exclude_key_id: Option<&str>,
    ) -> Result<Option<StoredProviderCatalogKey>, String> {
        find_duplicate_provider_oauth_key(state, provider_id, auth_config, exclude_key_id).await
    }

    pub(crate) async fn create_provider_oauth_catalog_key(
        state: &AdminAppState<'_>,
        provider_id: &str,
        provider_type: &str,
        name: &str,
        access_token: &str,
        auth_config: &serde_json::Map<String, serde_json::Value>,
        api_formats: &[String],
        proxy: Option<serde_json::Value>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
        create_provider_oauth_catalog_key(
            state,
            provider_id,
            provider_type,
            name,
            access_token,
            auth_config,
            api_formats,
            proxy,
            expires_at_unix_secs,
        )
        .await
    }

    pub(crate) async fn update_existing_provider_oauth_catalog_key(
        state: &AdminAppState<'_>,
        existing_key: &StoredProviderCatalogKey,
        provider_type: &str,
        access_token: &str,
        auth_config: &serde_json::Map<String, serde_json::Value>,
        api_formats: &[String],
        proxy: Option<serde_json::Value>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
        update_existing_provider_oauth_catalog_key(
            state,
            existing_key,
            provider_type,
            access_token,
            auth_config,
            api_formats,
            proxy,
            expires_at_unix_secs,
        )
        .await
    }

    pub(crate) async fn refresh_provider_oauth_account_state_after_update(
        state: &AdminAppState<'_>,
        provider: &StoredProviderCatalogProvider,
        key_id: &str,
        proxy_override: Option<&ProxySnapshot>,
    ) -> Result<(bool, Option<String>), GatewayError> {
        refresh_provider_oauth_account_state_after_update(state, provider, key_id, proxy_override)
            .await
    }

    pub(crate) fn clear_transport_cache_after_write(state: &AdminAppState<'_>) {
        state.app().clear_provider_transport_snapshot_cache();
    }
}
