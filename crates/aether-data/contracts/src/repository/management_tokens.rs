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
    pub permissions: Option<serde_json::Value>,
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
            permissions: None,
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

    pub fn with_permissions(mut self, permissions: Option<serde_json::Value>) -> Self {
        self.permissions = permissions;
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
            .unwrap_or_else(|| "ae-****".to_string())
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

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateManagementTokenRecord {
    pub id: String,
    pub user_id: String,
    pub user: StoredManagementTokenUserSummary,
    pub token_hash: String,
    pub token_prefix: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub allowed_ips: Option<serde_json::Value>,
    pub permissions: Option<serde_json::Value>,
    pub expires_at_unix_secs: Option<u64>,
    pub is_active: bool,
}

impl std::fmt::Debug for CreateManagementTokenRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreateManagementTokenRecord")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("token_hash", &"[REDACTED]")
            .field(
                "token_prefix",
                &self.token_prefix.as_ref().map(|_| "[REDACTED]"),
            )
            .field("name", &self.name)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .field("is_active", &self.is_active)
            .finish_non_exhaustive()
    }
}

fn validate_management_token_unix_secs_storage_range(
    value: u64,
    field_name: &str,
) -> Result<(), crate::DataLayerError> {
    if i64::try_from(value).is_err() {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field_name} exceeds supported storage range"
        )));
    }
    Ok(())
}

fn validate_optional_management_token_unix_secs_storage_range(
    value: Option<u64>,
    field_name: &str,
) -> Result<(), crate::DataLayerError> {
    match value {
        Some(value) => validate_management_token_unix_secs_storage_range(value, field_name),
        None => Ok(()),
    }
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
                    "IP 限制规则必须是数组".to_string(),
                ));
            };
            if items.is_empty() {
                return Err(crate::DataLayerError::InvalidInput(
                    "IP 限制规则不能为空".to_string(),
                ));
            }
            if items.iter().any(|value| value.as_str().is_none()) {
                return Err(crate::DataLayerError::InvalidInput(
                    "IP 限制规则只能包含字符串".to_string(),
                ));
            }
        }
        validate_management_token_permissions(self.permissions.as_ref())?;
        validate_optional_management_token_unix_secs_storage_range(
            self.expires_at_unix_secs,
            "expires_at_unix_secs",
        )?;
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
    pub permissions: Option<serde_json::Value>,
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
        if self.clear_description && self.description.is_some() {
            return Err(crate::DataLayerError::InvalidInput(
                "description and clear_description are mutually exclusive".to_string(),
            ));
        }
        if self.clear_allowed_ips && self.allowed_ips.is_some() {
            return Err(crate::DataLayerError::InvalidInput(
                "allowed_ips and clear_allowed_ips are mutually exclusive".to_string(),
            ));
        }
        if self.clear_expires_at && self.expires_at_unix_secs.is_some() {
            return Err(crate::DataLayerError::InvalidInput(
                "expires_at_unix_secs and clear_expires_at are mutually exclusive".to_string(),
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
                    "IP 限制规则必须是数组".to_string(),
                ));
            };
            if items.is_empty() {
                return Err(crate::DataLayerError::InvalidInput(
                    "IP 限制规则不能为空".to_string(),
                ));
            }
            if items.iter().any(|value| value.as_str().is_none()) {
                return Err(crate::DataLayerError::InvalidInput(
                    "IP 限制规则只能包含字符串".to_string(),
                ));
            }
        }
        validate_management_token_permissions(self.permissions.as_ref())?;
        validate_optional_management_token_unix_secs_storage_range(
            self.expires_at_unix_secs,
            "expires_at_unix_secs",
        )?;
        Ok(())
    }
}

fn validate_management_token_permissions(
    permissions: Option<&serde_json::Value>,
) -> Result<(), crate::DataLayerError> {
    let Some(permissions) = permissions else {
        return Ok(());
    };
    let Some(items) = permissions.as_array() else {
        return Err(crate::DataLayerError::InvalidInput(
            "permissions must be an array".to_string(),
        ));
    };
    if items.is_empty() {
        return Err(crate::DataLayerError::InvalidInput(
            "permissions must not be empty".to_string(),
        ));
    }
    if items.iter().any(|value| value.as_str().is_none()) {
        return Err(crate::DataLayerError::InvalidInput(
            "permissions must contain only strings".to_string(),
        ));
    }
    Ok(())
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegenerateManagementTokenSecret {
    pub token_id: String,
    pub token_hash: String,
    pub token_prefix: Option<String>,
}

impl std::fmt::Debug for RegenerateManagementTokenSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RegenerateManagementTokenSecret")
            .field("token_id", &self.token_id)
            .field("token_hash", &"[REDACTED]")
            .field(
                "token_prefix",
                &self.token_prefix.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActivateManagementTokenIfMatches {
    pub expected_token: StoredManagementToken,
    pub token_hash: String,
    pub expected_user_security_version: i64,
    pub now_unix_secs: u64,
}

impl std::fmt::Debug for ActivateManagementTokenIfMatches {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivateManagementTokenIfMatches")
            .field("expected_token", &self.expected_token)
            .field("token_hash", &"[REDACTED]")
            .field(
                "expected_user_security_version",
                &self.expected_user_security_version,
            )
            .field("now_unix_secs", &self.now_unix_secs)
            .finish()
    }
}

impl ActivateManagementTokenIfMatches {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.expected_token.id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "token_id is required".to_string(),
            ));
        }
        if self.expected_token.user_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "user_id is required".to_string(),
            ));
        }
        if self.expected_token.name.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "token name is required".to_string(),
            ));
        }
        if self.expected_token.is_active {
            return Err(crate::DataLayerError::InvalidInput(
                "activation snapshot must be inactive".to_string(),
            ));
        }
        if self.token_hash.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "token_hash is required".to_string(),
            ));
        }
        if self.expected_user_security_version < 0 {
            return Err(crate::DataLayerError::InvalidInput(
                "expected_user_security_version must not be negative".to_string(),
            ));
        }
        validate_management_token_unix_secs_storage_range(self.now_unix_secs, "now_unix_secs")?;
        validate_optional_management_token_unix_secs_storage_range(
            self.expected_token.expires_at_unix_secs,
            "expires_at_unix_secs",
        )?;
        validate_optional_management_token_unix_secs_storage_range(
            self.expected_token.last_used_at_unix_secs,
            "last_used_at_unix_secs",
        )?;
        validate_optional_management_token_unix_secs_storage_range(
            self.expected_token.created_at_unix_ms,
            "created_at_unix_ms",
        )?;
        validate_optional_management_token_unix_secs_storage_range(
            self.expected_token.updated_at_unix_secs,
            "updated_at_unix_secs",
        )?;
        validate_management_token_unix_secs_storage_range(
            self.expected_token.usage_count,
            "usage_count",
        )?;
        if let Some(allowed_ips) = &self.expected_token.allowed_ips {
            let Some(items) = allowed_ips.as_array() else {
                return Err(crate::DataLayerError::InvalidInput(
                    "IP 限制规则必须是数组".to_string(),
                ));
            };
            if items.is_empty() || items.iter().any(|value| value.as_str().is_none()) {
                return Err(crate::DataLayerError::InvalidInput(
                    "IP 限制规则只能是非空字符串数组".to_string(),
                ));
            }
        }
        validate_management_token_permissions(self.expected_token.permissions.as_ref())
    }

    pub fn matches_locked_token_snapshot(
        &self,
        current: &StoredManagementToken,
        current_token_hash: &str,
    ) -> bool {
        current_token_hash == self.token_hash && current == &self.expected_token
    }
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

    /// Update a token only while it still belongs to `user_id`.
    ///
    /// Self-service callers must use this owner-scoped mutation instead of
    /// relying on a preceding read-side ownership check.
    async fn update_management_token_for_user(
        &self,
        record: &UpdateManagementTokenRecord,
        user_id: &str,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;

    async fn delete_management_token(&self, token_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn delete_management_token_for_user(
        &self,
        token_id: &str,
        user_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn set_management_token_active(
        &self,
        token_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;

    async fn set_management_token_active_for_user(
        &self,
        token_id: &str,
        user_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;

    /// Atomically activate an inactive one-time-install token only while all
    /// security-relevant fields still match the session that issued it.
    async fn activate_management_token_if_matches(
        &self,
        mutation: &ActivateManagementTokenIfMatches,
    ) -> Result<bool, crate::DataLayerError>;

    /// Delete an unclaimed one-time-install token only while it still matches
    /// the session that created it and remains inactive.
    async fn delete_inactive_management_token_if_matches(
        &self,
        mutation: &ActivateManagementTokenIfMatches,
    ) -> Result<bool, crate::DataLayerError>;

    async fn regenerate_management_token_secret(
        &self,
        mutation: &RegenerateManagementTokenSecret,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;

    async fn regenerate_management_token_secret_for_user(
        &self,
        mutation: &RegenerateManagementTokenSecret,
        user_id: &str,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;

    async fn record_management_token_usage(
        &self,
        token_id: &str,
        last_used_ip: Option<&str>,
    ) -> Result<Option<StoredManagementToken>, crate::DataLayerError>;
}

#[cfg(test)]
mod tests {
    use super::{
        ActivateManagementTokenIfMatches, CreateManagementTokenRecord,
        RegenerateManagementTokenSecret, StoredManagementTokenUserSummary,
        UpdateManagementTokenRecord,
    };

    fn activation() -> ActivateManagementTokenIfMatches {
        ActivateManagementTokenIfMatches {
            expected_token: super::StoredManagementToken::new(
                "token-1".to_string(),
                "user-1".to_string(),
                "install token".to_string(),
            )
            .expect("token should build")
            .with_display_fields(None, Some("ae_install".to_string()), None)
            .with_permissions(Some(serde_json::json!(["admin:proxy_nodes:write"])))
            .with_runtime_fields(Some(2_000_000_000), None, None, 0, false)
            .with_timestamps(Some(1_800_000_000), Some(1_800_000_000)),
            token_hash: "hash-1".to_string(),
            expected_user_security_version: 7,
            now_unix_secs: 1_900_000_000,
        }
    }

    #[test]
    fn activation_rejects_timestamps_outside_sql_storage_range() {
        let mut mutation = activation();
        mutation.expected_token.expires_at_unix_secs = Some(i64::MAX as u64 + 1);
        assert!(mutation.validate().is_err());

        let mut mutation = activation();
        mutation.now_unix_secs = i64::MAX as u64 + 1;
        assert!(mutation.validate().is_err());
    }

    #[test]
    fn activation_requires_non_negative_user_security_version() {
        let mut mutation = activation();
        mutation.expected_user_security_version = -1;
        assert!(mutation.validate().is_err());
    }

    #[test]
    fn activation_matches_the_complete_canonical_token_snapshot() {
        let mutation = activation();
        assert!(
            mutation.matches_locked_token_snapshot(&mutation.expected_token, &mutation.token_hash,)
        );

        let mut changed = mutation.expected_token.clone();
        changed.description = Some("changed after session creation".to_string());
        assert!(!mutation.matches_locked_token_snapshot(&changed, &mutation.token_hash));

        let mut changed = mutation.expected_token.clone();
        changed.updated_at_unix_secs = changed.updated_at_unix_secs.map(|value| value + 1);
        assert!(!mutation.matches_locked_token_snapshot(&changed, &mutation.token_hash));

        assert!(
            !mutation.matches_locked_token_snapshot(&mutation.expected_token, "different-hash",)
        );
    }

    #[test]
    fn management_token_secret_debug_output_is_redacted() {
        let activation = ActivateManagementTokenIfMatches {
            token_hash: "activation-token-hash-canary".to_string(),
            ..activation()
        };
        let activation_debug = format!("{activation:?}");
        assert!(activation_debug.contains("[REDACTED]"));
        assert!(!activation_debug.contains("activation-token-hash-canary"));

        let regenerate = RegenerateManagementTokenSecret {
            token_id: "token-1".to_string(),
            token_hash: "regenerate-token-hash-canary".to_string(),
            token_prefix: Some("regenerate-token-prefix-canary".to_string()),
        };
        let regenerate_debug = format!("{regenerate:?}");
        assert!(regenerate_debug.contains("[REDACTED]"));
        assert!(!regenerate_debug.contains("regenerate-token-hash-canary"));
        assert!(!regenerate_debug.contains("regenerate-token-prefix-canary"));

        let user = StoredManagementTokenUserSummary::new(
            "user-1".to_string(),
            None,
            "admin".to_string(),
            "admin".to_string(),
        )
        .expect("user summary should build");
        let create = CreateManagementTokenRecord {
            id: "token-1".to_string(),
            user_id: user.id.clone(),
            user,
            token_hash: "create-token-hash-canary".to_string(),
            token_prefix: Some("create-token-prefix-canary".to_string()),
            name: "token".to_string(),
            description: None,
            allowed_ips: None,
            permissions: None,
            expires_at_unix_secs: None,
            is_active: true,
        };
        let create_debug = format!("{create:?}");
        assert!(create_debug.contains("[REDACTED]"));
        assert!(!create_debug.contains("create-token-hash-canary"));
        assert!(!create_debug.contains("create-token-prefix-canary"));
    }

    #[test]
    fn activation_rejects_json_null_without_conflating_it_with_sql_null() {
        let mutation = activation();

        let mut json_null_allowed_ips = mutation.expected_token.clone();
        json_null_allowed_ips.allowed_ips = Some(serde_json::Value::Null);
        assert!(
            !mutation.matches_locked_token_snapshot(&json_null_allowed_ips, &mutation.token_hash)
        );
        let mut invalid = mutation.clone();
        invalid.expected_token = json_null_allowed_ips;
        assert!(invalid.validate().is_err());

        let mut json_null_permissions = mutation.expected_token.clone();
        json_null_permissions.permissions = Some(serde_json::Value::Null);
        assert!(
            !mutation.matches_locked_token_snapshot(&json_null_permissions, &mutation.token_hash)
        );
        let mut invalid = mutation;
        invalid.expected_token = json_null_permissions;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn create_and_update_reject_expiry_outside_sql_storage_range() {
        let unsupported_expiry = i64::MAX as u64 + 1;
        let user = StoredManagementTokenUserSummary::new(
            "user-1".to_string(),
            None,
            "admin".to_string(),
            "admin".to_string(),
        )
        .expect("user summary should build");
        let create = CreateManagementTokenRecord {
            id: "token-1".to_string(),
            user_id: user.id.clone(),
            user,
            token_hash: "hash-1".to_string(),
            token_prefix: Some("ae_1234".to_string()),
            name: "token".to_string(),
            description: None,
            allowed_ips: None,
            permissions: Some(serde_json::json!(["admin:proxy_nodes:write"])),
            expires_at_unix_secs: Some(unsupported_expiry),
            is_active: false,
        };
        assert!(create.validate().is_err());

        let update = UpdateManagementTokenRecord {
            token_id: "token-1".to_string(),
            name: None,
            description: None,
            clear_description: false,
            allowed_ips: None,
            clear_allowed_ips: false,
            permissions: None,
            expires_at_unix_secs: Some(unsupported_expiry),
            clear_expires_at: false,
            is_active: None,
        };
        assert!(update.validate().is_err());
    }

    #[test]
    fn management_token_update_rejects_ambiguous_set_and_clear_operations() {
        let base = UpdateManagementTokenRecord {
            token_id: "token-1".to_string(),
            name: None,
            description: None,
            clear_description: false,
            allowed_ips: None,
            clear_allowed_ips: false,
            permissions: None,
            expires_at_unix_secs: None,
            clear_expires_at: false,
            is_active: None,
        };

        assert!(UpdateManagementTokenRecord {
            description: Some("description".to_string()),
            clear_description: true,
            ..base.clone()
        }
        .validate()
        .is_err());
        assert!(UpdateManagementTokenRecord {
            allowed_ips: Some(serde_json::json!(["127.0.0.1"])),
            clear_allowed_ips: true,
            ..base.clone()
        }
        .validate()
        .is_err());
        assert!(UpdateManagementTokenRecord {
            expires_at_unix_secs: Some(100),
            clear_expires_at: true,
            ..base
        }
        .validate()
        .is_err());
    }
}
