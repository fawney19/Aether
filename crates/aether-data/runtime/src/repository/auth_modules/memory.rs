use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    AuthModuleReadRepository, AuthModuleWriteRepository, StoredLdapModuleConfig,
    StoredOAuthProviderModuleConfig,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryAuthModuleReadRepository {
    oauth_providers: RwLock<Vec<StoredOAuthProviderModuleConfig>>,
    ldap_config: RwLock<Option<StoredLdapModuleConfig>>,
}

impl InMemoryAuthModuleReadRepository {
    pub fn seed<I>(oauth_providers: I, ldap_config: Option<StoredLdapModuleConfig>) -> Self
    where
        I: IntoIterator<Item = StoredOAuthProviderModuleConfig>,
    {
        Self {
            oauth_providers: RwLock::new(oauth_providers.into_iter().collect()),
            ldap_config: RwLock::new(ldap_config),
        }
    }
}

#[async_trait]
impl AuthModuleReadRepository for InMemoryAuthModuleReadRepository {
    async fn list_enabled_oauth_providers(
        &self,
    ) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
        Ok(self
            .oauth_providers
            .read()
            .expect("auth module oauth provider repository lock")
            .clone())
    }

    async fn get_ldap_config(&self) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        Ok(self
            .ldap_config
            .read()
            .expect("auth module ldap repository lock")
            .clone())
    }
}

#[async_trait]
impl AuthModuleWriteRepository for InMemoryAuthModuleReadRepository {
    async fn upsert_ldap_config(
        &self,
        config: &StoredLdapModuleConfig,
    ) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        self.ldap_config
            .write()
            .expect("auth module ldap repository lock")
            .replace(config.clone());
        Ok(Some(config.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryAuthModuleReadRepository;
    use crate::repository::auth_modules::{
        AuthModuleReadRepository, StoredLdapModuleConfig, StoredOAuthProviderModuleConfig,
    };

    #[tokio::test]
    async fn reads_seeded_auth_module_configs() {
        let repository = InMemoryAuthModuleReadRepository::seed(
            vec![StoredOAuthProviderModuleConfig::new(
                "linuxdo".to_string(),
                "Linux DO".to_string(),
                "client-id".to_string(),
                Some("encrypted".to_string()),
                "https://example.com/callback".to_string(),
            )
            .expect("oauth provider should build")],
            Some(StoredLdapModuleConfig {
                server_url: "ldaps://ldap.example.com".to_string(),
                bind_dn: "cn=admin,dc=example,dc=com".to_string(),
                bind_password_encrypted: Some("encrypted-password".to_string()),
                base_dn: "dc=example,dc=com".to_string(),
                user_search_filter: Some("(uid={username})".to_string()),
                username_attr: Some("uid".to_string()),
                email_attr: Some("mail".to_string()),
                display_name_attr: Some("displayName".to_string()),
                is_enabled: true,
                is_exclusive: false,
                use_starttls: true,
                connect_timeout: Some(10),
            }),
        );

        let oauth = repository
            .list_enabled_oauth_providers()
            .await
            .expect("oauth providers should load");
        let ldap = repository
            .get_ldap_config()
            .await
            .expect("ldap config should load");

        assert_eq!(oauth.len(), 1);
        assert_eq!(oauth[0].provider_type, "linuxdo");
        assert_eq!(
            ldap.expect("ldap config should exist").server_url,
            "ldaps://ldap.example.com"
        );
    }
}
