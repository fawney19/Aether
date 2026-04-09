use super::AdminAppState;
use crate::GatewayError;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn get_ldap_module_config(
        &self,
    ) -> Result<Option<aether_data::repository::auth_modules::StoredLdapModuleConfig>, GatewayError>
    {
        self.app.get_ldap_module_config().await
    }

    pub(crate) async fn list_oauth_provider_configs(
        &self,
    ) -> Result<
        Vec<aether_data::repository::oauth_providers::StoredOAuthProviderConfig>,
        GatewayError,
    > {
        self.app.list_oauth_provider_configs().await
    }

    pub(crate) async fn get_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<
        Option<aether_data::repository::oauth_providers::StoredOAuthProviderConfig>,
        GatewayError,
    > {
        self.app.get_oauth_provider_config(provider_type).await
    }

    pub(crate) async fn upsert_oauth_provider_config(
        &self,
        record: &aether_data::repository::oauth_providers::UpsertOAuthProviderConfigRecord,
    ) -> Result<
        Option<aether_data::repository::oauth_providers::StoredOAuthProviderConfig>,
        GatewayError,
    > {
        self.app.upsert_oauth_provider_config(record).await
    }

    pub(crate) async fn delete_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<bool, GatewayError> {
        self.app.delete_oauth_provider_config(provider_type).await
    }

    pub(crate) async fn count_locked_users_if_oauth_provider_disabled(
        &self,
        provider_type: &str,
        ldap_exclusive: bool,
    ) -> Result<usize, GatewayError> {
        self.app
            .count_locked_users_if_oauth_provider_disabled(provider_type, ldap_exclusive)
            .await
    }

    pub(crate) async fn get_management_token_with_user(
        &self,
        token_id: &str,
    ) -> Result<
        Option<aether_data::repository::management_tokens::StoredManagementTokenWithUser>,
        GatewayError,
    > {
        self.app.get_management_token_with_user(token_id).await
    }

    pub(crate) async fn delete_management_token(
        &self,
        token_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app.delete_management_token(token_id).await
    }

    pub(crate) async fn list_management_tokens(
        &self,
        query: &aether_data::repository::management_tokens::ManagementTokenListQuery,
    ) -> Result<
        aether_data::repository::management_tokens::StoredManagementTokenListPage,
        GatewayError,
    > {
        self.app.list_management_tokens(query).await
    }

    pub(crate) async fn set_management_token_active(
        &self,
        token_id: &str,
        is_active: bool,
    ) -> Result<
        Option<aether_data::repository::management_tokens::StoredManagementToken>,
        GatewayError,
    > {
        self.app
            .set_management_token_active(token_id, is_active)
            .await
    }

    pub(crate) async fn remove_admin_security_blacklist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        self.app.remove_admin_security_blacklist(ip_address).await
    }

    pub(crate) async fn add_admin_security_blacklist(
        &self,
        ip_address: &str,
        reason: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<bool, GatewayError> {
        self.app
            .add_admin_security_blacklist(ip_address, reason, ttl_seconds)
            .await
    }

    pub(crate) async fn admin_security_blacklist_stats(
        &self,
    ) -> Result<(bool, usize, Option<String>), GatewayError> {
        self.app.admin_security_blacklist_stats().await
    }

    pub(crate) async fn list_admin_security_blacklist(
        &self,
    ) -> Result<Vec<crate::state::AdminSecurityBlacklistEntry>, GatewayError> {
        self.app.list_admin_security_blacklist().await
    }

    pub(crate) async fn add_admin_security_whitelist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        self.app.add_admin_security_whitelist(ip_address).await
    }

    pub(crate) async fn remove_admin_security_whitelist(
        &self,
        ip_address: &str,
    ) -> Result<bool, GatewayError> {
        self.app.remove_admin_security_whitelist(ip_address).await
    }

    pub(crate) async fn list_admin_security_whitelist(&self) -> Result<Vec<String>, GatewayError> {
        self.app.list_admin_security_whitelist().await
    }

    pub(crate) async fn upsert_ldap_module_config(
        &self,
        config: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
    ) -> Result<Option<aether_data::repository::auth_modules::StoredLdapModuleConfig>, GatewayError>
    {
        self.app.upsert_ldap_module_config(config).await
    }

    pub(crate) async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, GatewayError> {
        self.app
            .count_active_local_admin_users_with_valid_password()
            .await
    }

    pub(crate) async fn list_enabled_oauth_module_providers(
        &self,
    ) -> Result<
        Vec<aether_data::repository::auth_modules::StoredOAuthProviderModuleConfig>,
        GatewayError,
    > {
        self.app.list_enabled_oauth_module_providers().await
    }
}
