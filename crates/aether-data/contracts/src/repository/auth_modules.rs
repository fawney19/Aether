use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredOAuthProviderModuleConfig {
    pub provider_type: String,
    pub display_name: String,
    pub client_id: String,
    pub client_secret_encrypted: Option<String>,
    pub redirect_uri: String,
}

impl StoredOAuthProviderModuleConfig {
    pub fn new(
        provider_type: String,
        display_name: String,
        client_id: String,
        client_secret_encrypted: Option<String>,
        redirect_uri: String,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_type.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "oauth_providers.provider_type is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "oauth_providers.display_name is empty".to_string(),
            ));
        }
        Ok(Self {
            provider_type,
            display_name,
            client_id,
            client_secret_encrypted,
            redirect_uri,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredLdapModuleConfig {
    pub server_url: String,
    pub bind_dn: String,
    pub bind_password_encrypted: Option<String>,
    pub base_dn: String,
    pub user_search_filter: Option<String>,
    pub username_attr: Option<String>,
    pub email_attr: Option<String>,
    pub display_name_attr: Option<String>,
    pub is_enabled: bool,
    pub is_exclusive: bool,
    pub use_starttls: bool,
    pub connect_timeout: Option<i32>,
}

#[async_trait]
pub trait AuthModuleReadRepository: Send + Sync {
    async fn list_enabled_oauth_providers(
        &self,
    ) -> Result<Vec<StoredOAuthProviderModuleConfig>, crate::DataLayerError>;

    async fn get_ldap_config(
        &self,
    ) -> Result<Option<StoredLdapModuleConfig>, crate::DataLayerError>;
}

#[async_trait]
pub trait AuthModuleWriteRepository: Send + Sync {
    async fn upsert_ldap_config(
        &self,
        config: &StoredLdapModuleConfig,
    ) -> Result<Option<StoredLdapModuleConfig>, crate::DataLayerError>;
}
