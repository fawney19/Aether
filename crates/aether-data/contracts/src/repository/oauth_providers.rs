use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredOAuthProviderConfig {
    pub provider_type: String,
    pub display_name: String,
    pub client_id: String,
    pub client_secret_encrypted: Option<String>,
    pub authorization_url_override: Option<String>,
    pub token_url_override: Option<String>,
    pub userinfo_url_override: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub redirect_uri: String,
    pub frontend_callback_url: String,
    pub attribute_mapping: Option<serde_json::Value>,
    pub extra_config: Option<serde_json::Value>,
    pub icon_url: Option<String>,
    pub is_enabled: bool,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl StoredOAuthProviderConfig {
    pub fn new(
        provider_type: String,
        display_name: String,
        client_id: String,
        redirect_uri: String,
        frontend_callback_url: String,
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
        if client_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "oauth_providers.client_id is empty".to_string(),
            ));
        }
        if redirect_uri.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "oauth_providers.redirect_uri is empty".to_string(),
            ));
        }
        if frontend_callback_url.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "oauth_providers.frontend_callback_url is empty".to_string(),
            ));
        }

        Ok(Self {
            provider_type,
            display_name,
            client_id,
            client_secret_encrypted: None,
            authorization_url_override: None,
            token_url_override: None,
            userinfo_url_override: None,
            scopes: None,
            redirect_uri,
            frontend_callback_url,
            attribute_mapping: None,
            extra_config: None,
            icon_url: None,
            is_enabled: false,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_config_fields(
        mut self,
        client_secret_encrypted: Option<String>,
        authorization_url_override: Option<String>,
        token_url_override: Option<String>,
        userinfo_url_override: Option<String>,
        scopes: Option<Vec<String>>,
        attribute_mapping: Option<serde_json::Value>,
        extra_config: Option<serde_json::Value>,
        icon_url: Option<String>,
        is_enabled: bool,
    ) -> Self {
        self.client_secret_encrypted = client_secret_encrypted;
        self.authorization_url_override = authorization_url_override;
        self.token_url_override = token_url_override;
        self.userinfo_url_override = userinfo_url_override;
        self.scopes = scopes;
        self.attribute_mapping = attribute_mapping;
        self.extra_config = extra_config;
        self.icon_url = icon_url;
        self.is_enabled = is_enabled;
        self
    }

    pub fn with_timestamps(
        mut self,
        created_at_unix_ms: Option<u64>,
        updated_at_unix_secs: Option<u64>,
    ) -> Self {
        self.created_at_unix_ms = created_at_unix_ms;
        self.updated_at_unix_secs = updated_at_unix_secs;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum EncryptedSecretUpdate {
    #[default]
    Preserve,
    Clear,
    Set(String),
}

impl EncryptedSecretUpdate {
    pub fn mode_name(&self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Clear => "clear",
            Self::Set(_) => "set",
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            Self::Set(value) => Some(value.as_str()),
            Self::Preserve | Self::Clear => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertOAuthProviderConfigRecord {
    pub provider_type: String,
    pub display_name: String,
    pub client_id: String,
    pub client_secret_encrypted: EncryptedSecretUpdate,
    pub authorization_url_override: Option<String>,
    pub token_url_override: Option<String>,
    pub userinfo_url_override: Option<String>,
    pub scopes: Option<Vec<String>>,
    pub redirect_uri: String,
    pub frontend_callback_url: String,
    pub attribute_mapping: Option<serde_json::Value>,
    pub extra_config: Option<serde_json::Value>,
    pub icon_url: Option<String>,
    pub is_enabled: bool,
}

impl UpsertOAuthProviderConfigRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.provider_type.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "provider_type is required".to_string(),
            ));
        }
        if self.display_name.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "display_name is required".to_string(),
            ));
        }
        if self.client_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "client_id is required".to_string(),
            ));
        }
        if self.redirect_uri.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "redirect_uri is required".to_string(),
            ));
        }
        if self.frontend_callback_url.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "frontend_callback_url is required".to_string(),
            ));
        }
        if let Some(scopes) = &self.scopes {
            for scope in scopes {
                if scope.trim().is_empty() {
                    return Err(crate::DataLayerError::InvalidInput(
                        "scopes must not contain empty values".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
pub trait OAuthProviderReadRepository: Send + Sync {
    async fn list_oauth_provider_configs(
        &self,
    ) -> Result<Vec<StoredOAuthProviderConfig>, crate::DataLayerError>;

    async fn get_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<Option<StoredOAuthProviderConfig>, crate::DataLayerError>;

    async fn count_locked_users_if_provider_disabled(
        &self,
        provider_type: &str,
        ldap_exclusive: bool,
    ) -> Result<usize, crate::DataLayerError>;
}

#[async_trait]
pub trait OAuthProviderWriteRepository: Send + Sync {
    async fn upsert_oauth_provider_config(
        &self,
        record: &UpsertOAuthProviderConfigRecord,
    ) -> Result<StoredOAuthProviderConfig, crate::DataLayerError>;

    async fn delete_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<bool, crate::DataLayerError>;
}

pub trait OAuthProviderRepository:
    OAuthProviderReadRepository + OAuthProviderWriteRepository + Send + Sync
{
}

impl<T> OAuthProviderRepository for T where
    T: OAuthProviderReadRepository + OAuthProviderWriteRepository + Send + Sync
{
}
