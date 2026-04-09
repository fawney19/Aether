use super::AdminAppState;

impl<'a> AdminAppState<'a> {
    pub(crate) fn http_client(&self) -> &reqwest::Client {
        &self.app.client
    }

    pub(crate) fn provider_oauth_token_url(
        &self,
        provider_type: &str,
        default_url: &str,
    ) -> String {
        self.app
            .provider_oauth_token_url(provider_type, default_url)
    }

    pub(crate) fn encryption_key(&self) -> Option<&str> {
        self.app.encryption_key()
    }

    pub(crate) fn encrypt_catalog_secret_with_fallbacks(&self, secret: &str) -> Option<String> {
        crate::handlers::admin::shared::encrypt_catalog_secret_with_fallbacks(self.app, secret)
    }

    pub(crate) fn decrypt_catalog_secret_with_fallbacks(&self, ciphertext: &str) -> Option<String> {
        crate::handlers::admin::shared::decrypt_catalog_secret_with_fallbacks(
            self.app.encryption_key(),
            ciphertext,
        )
    }

    pub(crate) fn parse_catalog_auth_config_json(
        &self,
        key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        crate::handlers::admin::shared::parse_catalog_auth_config_json(self.app, key)
    }

    pub(crate) fn has_provider_catalog_data_reader(&self) -> bool {
        self.app.has_provider_catalog_data_reader()
    }

    pub(crate) fn has_provider_catalog_data_writer(&self) -> bool {
        self.app.has_provider_catalog_data_writer()
    }

    pub(crate) fn has_request_candidate_data_reader(&self) -> bool {
        self.app.has_request_candidate_data_reader()
    }

    pub(crate) fn has_management_token_reader(&self) -> bool {
        self.app.has_management_token_reader()
    }

    pub(crate) fn has_management_token_writer(&self) -> bool {
        self.app.has_management_token_writer()
    }

    pub(crate) fn has_global_model_data_reader(&self) -> bool {
        self.app.has_global_model_data_reader()
    }

    pub(crate) fn has_global_model_data_writer(&self) -> bool {
        self.app.has_global_model_data_writer()
    }

    pub(crate) fn has_usage_data_reader(&self) -> bool {
        self.app.has_usage_data_reader()
    }

    pub(crate) fn has_gemini_file_mapping_data_reader(&self) -> bool {
        self.app.has_gemini_file_mapping_data_reader()
    }

    pub(crate) fn has_gemini_file_mapping_data_writer(&self) -> bool {
        self.app.has_gemini_file_mapping_data_writer()
    }

    pub(crate) fn has_auth_api_key_data_reader(&self) -> bool {
        self.app.has_auth_api_key_data_reader()
    }

    pub(crate) fn has_user_data_reader(&self) -> bool {
        self.app.has_user_data_reader()
    }

    pub(crate) fn has_proxy_node_reader(&self) -> bool {
        self.app.has_proxy_node_reader()
    }

    pub(crate) fn has_auth_api_key_writer(&self) -> bool {
        self.app.data.has_auth_api_key_writer()
    }

    pub(crate) fn has_auth_module_writer(&self) -> bool {
        self.app.has_auth_module_writer()
    }

    pub(crate) fn has_auth_user_write_capability(&self) -> bool {
        self.app.has_auth_user_write_capability()
    }

    pub(crate) fn has_auth_wallet_write_capability(&self) -> bool {
        self.app.has_auth_wallet_write_capability()
    }

    pub(crate) fn mark_provider_key_rpm_reset(&self, key_id: &str, now_unix_secs: u64) {
        self.app.mark_provider_key_rpm_reset(key_id, now_unix_secs)
    }

    pub(crate) fn redis_kv_runner(&self) -> Option<aether_data::redis::RedisKvRunner> {
        self.app.redis_kv_runner()
    }

    pub(crate) fn provider_key_rpm_reset_at(
        &self,
        key_id: &str,
        now_unix_secs: u64,
    ) -> Option<u64> {
        self.app.provider_key_rpm_reset_at(key_id, now_unix_secs)
    }

    pub(crate) fn has_postgres_pool(&self) -> bool {
        self.app.postgres_pool().is_some()
    }

    pub(crate) fn mark_admin_monitoring_error_stats_reset(&self, now_unix_secs: u64) {
        self.app
            .mark_admin_monitoring_error_stats_reset(now_unix_secs);
    }

    pub(crate) fn admin_monitoring_error_stats_reset_at(&self) -> Option<u64> {
        self.app.admin_monitoring_error_stats_reset_at()
    }
}
