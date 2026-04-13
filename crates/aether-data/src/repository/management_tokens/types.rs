use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredManagementTokenUserSummary {
    pub id: String,
    pub email: Option<String>,
    pub username: String,
    pub role: String,
}

impl StoredManagementTokenUserSummary {
    pub fn new(
        id: String,
        email: Option<String>,
        username: String,
        role: String,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "users.id is empty".to_string(),
            ));
        }
        if username.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "users.username is empty".to_string(),
            ));
        }
        if role.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "users.role is empty".to_string(),
            ));
        }
        Ok(Self {
            id,
            email,
            username,
            role,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredManagementToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub description: Option<String>,
    pub token_prefix: Option<String>,
    pub allowed_ips: Option<serde_json::Value>,
    pub expires_at_unix_secs: Option<u64>,
    pub last_used_at_unix_secs: Option<u64>,
    pub last_used_ip: Option<String>,
    pub usage_count: u64,
    pub is_active: bool,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl StoredManagementToken {
    pub fn new(id: String, user_id: String, name: String) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "management_tokens.id is empty".to_string(),
            ));
        }
        if user_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "management_tokens.user_id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "management_tokens.name is empty".to_string(),
            ));
        }
        Ok(Self {
            id,
            user_id,
            name,
            description: None,
            token_prefix: None,
            allowed_ips: None,
            expires_at_unix_secs: None,
            last_used_at_unix_secs: None,
            last_used_ip: None,
            usage_count: 0,
            is_active: true,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
        })
    }

    pub fn with_display_fields(
        mut self,
        description: Option<String>,
        token_prefix: Option<String>,
        allowed_ips: Option<serde_json::Value>,
    ) -> Self {
        self.description = description;
        self.token_prefix = token_prefix;
        self.allowed_ips = allowed_ips;
        self
    }

    pub fn with_runtime_fields(
        mut self,
        expires_at_unix_secs: Option<u64>,
        last_used_at_unix_secs: Option<u64>,
        last_used_ip: Option<String>,
        usage_count: u64,
        is_active: bool,
    ) -> Self {
        self.expires_at_unix_secs = expires_at_unix_secs;
        self.last_used_at_unix_secs = last_used_at_unix_secs;
        self.last_used_ip = last_used_ip;
        self.usage_count = usage_count;
        self.is_active = is_active;
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

    pub fn token_display(&self) -> String {
        self.token_prefix
            .as_deref()
            .map(|prefix| format!("{prefix}...****"))
            .unwrap_or_else(|| "ae_****".to_string())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredManagementTokenWithUser {
    pub token: StoredManagementToken,
    pub user: StoredManagementTokenUserSummary,
}

impl StoredManagementTokenWithUser {
    pub fn new(token: StoredManagementToken, user: StoredManagementTokenUserSummary) -> Self {
        Self { token, user }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagementTokenListQuery {
    pub user_id: Option<String>,
    pub is_active: Option<bool>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateManagementTokenRecord {
    pub id: String,
    pub user_id: String,
    pub user: StoredManagementTokenUserSummary,
    pub token_hash: String,
    pub token_prefix: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub allowed_ips: Option<serde_json::Value>,
    pub expires_at_unix_secs: Option<u64>,
    pub is_active: bool,
}

impl CreateManagementTokenRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "token_id is required".to_string(),
            ));
        }
        if self.user_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "user_id is required".to_string(),
            ));
        }
        if self.user.id != self.user_id {
            return Err(crate::DataLayerError::InvalidInput(
                "management token user summary does not match user_id".to_string(),
            ));
        }
        if self.token_hash.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "token_hash is required".to_string(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "name is required".to_string(),
            ));
        }
        if let Some(allowed_ips) = &self.allowed_ips {
            let Some(items) = allowed_ips.as_array() else {
                return Err(crate::DataLayerError::InvalidInput(
                    "allowed_ips must be an array".to_string(),
                ));
            };
            if items.is_empty() {
                return Err(crate::DataLayerError::InvalidInput(
                    "allowed_ips must not be empty".to_string(),
                ));
            }
            if items.iter().any(|value| value.as_str().is_none()) {
                return Err(crate::DataLayerError::InvalidInput(
                    "allowed_ips must contain only strings".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpdateManagementTokenRecord {
    pub token_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub clear_description: bool,
    pub allowed_ips: Option<serde_json::Value>,
    pub clear_allowed_ips: bool,
    pub expires_at_unix_secs: Option<u64>,
    pub clear_expires_at: bool,
    pub is_active: Option<bool>,
}

impl UpdateManagementTokenRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.token_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "token_id is required".to_string(),
            ));
        }
        if let Some(name) = &self.name {
            if name.trim().is_empty() {
                return Err(crate::DataLayerError::InvalidInput(
                    "name must not be empty".to_string(),
                ));
            }
        }
        if let Some(allowed_ips) = &self.allowed_ips {
            let Some(items) = allowed_ips.as_array() else {
                return Err(crate::DataLayerError::InvalidInput(
                    "allowed_ips must be an array".to_string(),
                ));
            };
            if items.is_empty() {
                return Err(crate::DataLayerError::InvalidInput(
                    "allowed_ips must not be empty".to_string(),
                ));
            }
            if items.iter().any(|value| value.as_str().is_none()) {
                return Err(crate::DataLayerError::InvalidInput(
                    "allowed_ips must contain only strings".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegenerateManagementTokenSecret {
    pub token_id: String,
    pub token_hash: String,
    pub token_prefix: Option<String>,
}

impl RegenerateManagementTokenSecret {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.token_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "token_id is required".to_string(),
            ));
        }
        if self.token_hash.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "token_hash is required".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredManagementTokenListPage {
    pub items: Vec<StoredManagementTokenWithUser>,
    pub total: usize,
}

#[async_trait]
pub trait ManagementTokenReadRepository: Send + Sync {
    async fn list_management_tokens(
        &self,
        query: &ManagementTokenListQuery,
    ) -> Result<StoredManagementTokenListPage, crate::DataLayerError>;

    async fn get_management_token_with_user(
        &self,
        token_id: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, crate::DataLayerError>;

    async fn get_management_token_with_user_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, crate::DataLayerError>;
}

#[async_trait]
pub trait ManagementTokenWriteRepository: Send + Sync {
    async fn create_management_token(
        &self,
        record: &CreateManagementTokenRecord,
    ) -> Result<StoredManagementToken, crate::DataLayerError>;

    async fn update_management_token(
        &self,
        record: &UpdateManagementTokenRecord,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;

    async fn delete_management_token(&self, token_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn set_management_token_active(
        &self,
        token_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;

    async fn regenerate_management_token_secret(
        &self,
        mutation: &RegenerateManagementTokenSecret,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;

    async fn record_management_token_usage(
        &self,
        token_id: &str,
        last_used_ip: Option<&str>,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;
}
