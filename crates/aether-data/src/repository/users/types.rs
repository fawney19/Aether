use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserSummary {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub role: String,
    pub is_active: bool,
    pub is_deleted: bool,
}

impl StoredUserSummary {
    pub fn new(
        id: String,
        username: String,
        email: Option<String>,
        role: String,
        is_active: bool,
        is_deleted: bool,
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
            username,
            email,
            role,
            is_active,
            is_deleted,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserAuthRecord {
    pub id: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub username: String,
    pub password_hash: Option<String>,
    pub role: String,
    pub auth_source: String,
    pub allowed_providers: Option<Vec<String>>,
    pub allowed_providers_mode: String,
    pub allowed_api_formats: Option<Vec<String>>,
    pub allowed_api_formats_mode: String,
    pub allowed_models: Option<Vec<String>>,
    pub allowed_models_mode: String,
    pub is_active: bool,
    pub is_deleted: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl StoredUserAuthRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: Option<String>,
        role: String,
        auth_source: String,
        allowed_providers: Option<Value>,
        allowed_api_formats: Option<Value>,
        allowed_models: Option<Value>,
        is_active: bool,
        is_deleted: bool,
        created_at: Option<DateTime<Utc>>,
        last_login_at: Option<DateTime<Utc>>,
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
        if auth_source.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "users.auth_source is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            email,
            email_verified,
            username,
            password_hash,
            role,
            auth_source,
            allowed_providers: parse_string_list(allowed_providers, "users.allowed_providers")?,
            allowed_providers_mode: "unrestricted".to_string(),
            allowed_api_formats: parse_string_list(
                allowed_api_formats,
                "users.allowed_api_formats",
            )?,
            allowed_api_formats_mode: "unrestricted".to_string(),
            allowed_models: parse_string_list(allowed_models, "users.allowed_models")?,
            allowed_models_mode: "unrestricted".to_string(),
            is_active,
            is_deleted,
            created_at,
            last_login_at,
        })
        .map(|record| record.with_legacy_policy_modes())
    }

    pub fn with_policy_modes(
        mut self,
        allowed_providers_mode: String,
        allowed_api_formats_mode: String,
        allowed_models_mode: String,
    ) -> Result<Self, crate::DataLayerError> {
        self.allowed_providers_mode =
            normalize_list_policy_mode(&allowed_providers_mode, "users.allowed_providers_mode")?;
        self.allowed_api_formats_mode = normalize_list_policy_mode(
            &allowed_api_formats_mode,
            "users.allowed_api_formats_mode",
        )?;
        self.allowed_models_mode =
            normalize_list_policy_mode(&allowed_models_mode, "users.allowed_models_mode")?;
        Ok(self)
    }

    fn with_legacy_policy_modes(mut self) -> Self {
        self.allowed_providers_mode = legacy_list_policy_mode(&self.allowed_providers);
        self.allowed_api_formats_mode = legacy_list_policy_mode(&self.allowed_api_formats);
        self.allowed_models_mode = legacy_list_policy_mode(&self.allowed_models);
        self
    }

    pub fn to_summary(&self) -> Result<StoredUserSummary, crate::DataLayerError> {
        StoredUserSummary::new(
            self.id.clone(),
            self.username.clone(),
            self.email.clone(),
            self.role.clone(),
            self.is_active,
            self.is_deleted,
        )
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LdapAuthUserProvisioningOutcome {
    pub user: StoredUserAuthRecord,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserOAuthLinkSummary {
    pub provider_type: String,
    pub display_name: String,
    pub provider_username: Option<String>,
    pub provider_email: Option<String>,
    pub linked_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub provider_enabled: bool,
}

impl StoredUserOAuthLinkSummary {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider_type: String,
        display_name: String,
        provider_username: Option<String>,
        provider_email: Option<String>,
        linked_at: Option<DateTime<Utc>>,
        last_login_at: Option<DateTime<Utc>>,
        provider_enabled: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_type.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_oauth_links.provider_type is empty".to_string(),
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
            provider_username,
            provider_email,
            linked_at,
            last_login_at,
            provider_enabled,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserExportRow {
    pub id: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub username: String,
    pub password_hash: Option<String>,
    pub role: String,
    pub auth_source: String,
    pub allowed_providers: Option<Vec<String>>,
    pub allowed_providers_mode: String,
    pub allowed_api_formats: Option<Vec<String>>,
    pub allowed_api_formats_mode: String,
    pub allowed_models: Option<Vec<String>>,
    pub allowed_models_mode: String,
    pub rate_limit: Option<i32>,
    pub rate_limit_mode: String,
    pub model_capability_settings: Option<Value>,
    pub feature_settings: Option<Value>,
    pub is_active: bool,
}

impl StoredUserExportRow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: Option<String>,
        role: String,
        auth_source: String,
        allowed_providers: Option<Value>,
        allowed_api_formats: Option<Value>,
        allowed_models: Option<Value>,
        rate_limit: Option<i32>,
        model_capability_settings: Option<Value>,
        is_active: bool,
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
        if auth_source.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "users.auth_source is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            email,
            email_verified,
            username,
            password_hash,
            role,
            auth_source,
            allowed_providers: parse_string_list(allowed_providers, "users.allowed_providers")?,
            allowed_providers_mode: "unrestricted".to_string(),
            allowed_api_formats: parse_string_list(
                allowed_api_formats,
                "users.allowed_api_formats",
            )?,
            allowed_api_formats_mode: "unrestricted".to_string(),
            allowed_models: parse_string_list(allowed_models, "users.allowed_models")?,
            allowed_models_mode: "unrestricted".to_string(),
            rate_limit,
            rate_limit_mode: "system".to_string(),
            model_capability_settings: normalize_optional_json(model_capability_settings),
            feature_settings: None,
            is_active,
        })
        .map(|record| record.with_legacy_policy_modes())
    }

    pub fn with_policy_modes(
        mut self,
        allowed_providers_mode: String,
        allowed_api_formats_mode: String,
        allowed_models_mode: String,
        rate_limit_mode: String,
    ) -> Result<Self, crate::DataLayerError> {
        self.allowed_providers_mode =
            normalize_list_policy_mode(&allowed_providers_mode, "users.allowed_providers_mode")?;
        self.allowed_api_formats_mode = normalize_list_policy_mode(
            &allowed_api_formats_mode,
            "users.allowed_api_formats_mode",
        )?;
        self.allowed_models_mode =
            normalize_list_policy_mode(&allowed_models_mode, "users.allowed_models_mode")?;
        self.rate_limit_mode =
            normalize_rate_limit_policy_mode(&rate_limit_mode, "users.rate_limit_mode")?;
        Ok(self)
    }

    pub fn with_feature_settings(mut self, feature_settings: Option<Value>) -> Self {
        self.feature_settings = normalize_optional_json(feature_settings);
        self
    }

    fn with_legacy_policy_modes(mut self) -> Self {
        self.allowed_providers_mode = legacy_list_policy_mode(&self.allowed_providers);
        self.allowed_api_formats_mode = legacy_list_policy_mode(&self.allowed_api_formats);
        self.allowed_models_mode = legacy_list_policy_mode(&self.allowed_models);
        self.rate_limit_mode = if self.rate_limit.is_some() {
            "custom".to_string()
        } else {
            "system".to_string()
        };
        self
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserSessionRecord {
    pub id: String,
    pub user_id: String,
    pub client_device_id: String,
    pub device_label: Option<String>,
    pub refresh_token_hash: String,
    pub prev_refresh_token_hash: Option<String>,
    pub rotated_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoke_reason: Option<String>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl StoredUserSessionRecord {
    pub const REFRESH_GRACE_SECONDS: i64 = 10;
    pub const TOUCH_INTERVAL_SECONDS: i64 = 300;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        user_id: String,
        client_device_id: String,
        device_label: Option<String>,
        refresh_token_hash: String,
        prev_refresh_token_hash: Option<String>,
        rotated_at: Option<DateTime<Utc>>,
        last_seen_at: Option<DateTime<Utc>>,
        expires_at: Option<DateTime<Utc>>,
        revoked_at: Option<DateTime<Utc>>,
        revoke_reason: Option<String>,
        ip_address: Option<String>,
        user_agent: Option<String>,
        created_at: Option<DateTime<Utc>>,
        updated_at: Option<DateTime<Utc>>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_sessions.id is empty".to_string(),
            ));
        }
        if user_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_sessions.user_id is empty".to_string(),
            ));
        }
        if client_device_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_sessions.client_device_id is empty".to_string(),
            ));
        }
        if refresh_token_hash.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_sessions.refresh_token_hash is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            user_id,
            client_device_id,
            device_label,
            refresh_token_hash,
            prev_refresh_token_hash,
            rotated_at,
            last_seen_at,
            expires_at,
            revoked_at,
            revoke_reason,
            ip_address,
            user_agent,
            created_at,
            updated_at,
        })
    }

    pub fn hash_refresh_token(token: &str) -> String {
        use sha2::Digest;

        let mut hasher = sha2::Sha256::new();
        hasher.update(token.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn verify_refresh_token(&self, token: &str, now: DateTime<Utc>) -> (bool, bool) {
        let token_hash = Self::hash_refresh_token(token);
        if self.refresh_token_hash == token_hash {
            return (true, false);
        }
        let Some(prev_hash) = self.prev_refresh_token_hash.as_ref() else {
            return (false, false);
        };
        let Some(rotated_at) = self.rotated_at else {
            return (false, false);
        };
        if prev_hash == &token_hash
            && now.signed_duration_since(rotated_at).num_seconds() <= Self::REFRESH_GRACE_SECONDS
        {
            return (true, true);
        }
        (false, false)
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_none_or(|expires_at| expires_at <= now)
    }

    pub fn should_touch(&self, now: DateTime<Utc>) -> bool {
        self.last_seen_at
            .map(|last_seen_at| {
                now.signed_duration_since(last_seen_at).num_seconds()
                    >= Self::TOUCH_INTERVAL_SECONDS
            })
            .unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserPreferenceRecord {
    pub user_id: String,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub default_provider_id: Option<String>,
    pub default_provider_name: Option<String>,
    pub theme: String,
    pub language: String,
    pub timezone: String,
    pub email_notifications: bool,
    pub usage_alerts: bool,
    pub announcement_notifications: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserGroup {
    pub id: String,
    pub name: String,
    pub normalized_name: String,
    pub description: Option<String>,
    pub priority: i32,
    pub allowed_providers: Option<Vec<String>>,
    pub allowed_providers_mode: String,
    pub allowed_api_formats: Option<Vec<String>>,
    pub allowed_api_formats_mode: String,
    pub allowed_models: Option<Vec<String>>,
    pub allowed_models_mode: String,
    pub rate_limit: Option<i32>,
    pub rate_limit_mode: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl StoredUserGroup {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        normalized_name: String,
        description: Option<String>,
        priority: i32,
        allowed_providers: Option<Value>,
        allowed_providers_mode: String,
        allowed_api_formats: Option<Value>,
        allowed_api_formats_mode: String,
        allowed_models: Option<Value>,
        allowed_models_mode: String,
        rate_limit: Option<i32>,
        rate_limit_mode: String,
        created_at: Option<DateTime<Utc>>,
        updated_at: Option<DateTime<Utc>>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_groups.id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_groups.name is empty".to_string(),
            ));
        }
        if normalized_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_groups.normalized_name is empty".to_string(),
            ));
        }
        Ok(Self {
            id,
            name,
            normalized_name,
            description,
            priority,
            allowed_providers: parse_string_list(
                allowed_providers,
                "user_groups.allowed_providers",
            )?,
            allowed_providers_mode: normalize_list_policy_mode(
                &allowed_providers_mode,
                "user_groups.allowed_providers_mode",
            )?,
            allowed_api_formats: parse_string_list(
                allowed_api_formats,
                "user_groups.allowed_api_formats",
            )?,
            allowed_api_formats_mode: normalize_list_policy_mode(
                &allowed_api_formats_mode,
                "user_groups.allowed_api_formats_mode",
            )?,
            allowed_models: parse_string_list(allowed_models, "user_groups.allowed_models")?,
            allowed_models_mode: normalize_list_policy_mode(
                &allowed_models_mode,
                "user_groups.allowed_models_mode",
            )?,
            rate_limit,
            rate_limit_mode: normalize_rate_limit_policy_mode(
                &rate_limit_mode,
                "user_groups.rate_limit_mode",
            )?,
            created_at,
            updated_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserGroupMember {
    pub group_id: String,
    pub user_id: String,
    pub username: String,
    pub email: Option<String>,
    pub role: String,
    pub is_active: bool,
    pub is_deleted: bool,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserGroupMembership {
    pub user_id: String,
    pub group_id: String,
    pub group_name: String,
    pub group_priority: i32,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertUserGroupRecord {
    pub name: String,
    pub description: Option<String>,
    pub priority: i32,
    pub allowed_providers: Option<Vec<String>>,
    pub allowed_providers_mode: String,
    pub allowed_api_formats: Option<Vec<String>>,
    pub allowed_api_formats_mode: String,
    pub allowed_models: Option<Vec<String>>,
    pub allowed_models_mode: String,
    pub rate_limit: Option<i32>,
    pub rate_limit_mode: String,
}

impl UpsertUserGroupRecord {
    pub fn normalized_name(&self) -> String {
        normalize_user_group_name(&self.name).to_ascii_lowercase()
    }
}

impl StoredUserPreferenceRecord {
    pub fn default_for_user(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            avatar_url: None,
            bio: None,
            default_provider_id: None,
            default_provider_name: None,
            theme: "light".to_string(),
            language: "zh-CN".to_string(),
            timezone: "Asia/Shanghai".to_string(),
            email_notifications: true,
            usage_alerts: true,
            announcement_notifications: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserExportListQuery {
    pub skip: usize,
    pub limit: usize,
    pub role: Option<String>,
    pub is_active: Option<bool>,
    pub search: Option<String>,
    pub group_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct UserExportSummary {
    pub total: u64,
    pub active: u64,
}

#[async_trait]
pub trait UserReadRepository: Send + Sync {
    async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserSummary>, crate::DataLayerError>;

    async fn list_users_by_username_search(
        &self,
        username_search: &str,
    ) -> Result<Vec<StoredUserSummary>, crate::DataLayerError>;

    async fn list_export_users(&self) -> Result<Vec<StoredUserExportRow>, crate::DataLayerError>;

    async fn list_export_users_page(
        &self,
        query: &UserExportListQuery,
    ) -> Result<Vec<StoredUserExportRow>, crate::DataLayerError>;

    async fn summarize_export_users(&self) -> Result<UserExportSummary, crate::DataLayerError>;

    async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserExportRow>, crate::DataLayerError>;

    async fn list_user_groups(&self) -> Result<Vec<StoredUserGroup>, crate::DataLayerError>;

    async fn find_user_group_by_id(
        &self,
        group_id: &str,
    ) -> Result<Option<StoredUserGroup>, crate::DataLayerError>;

    async fn list_user_groups_by_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, crate::DataLayerError>;

    async fn create_user_group(
        &self,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, crate::DataLayerError>;

    async fn update_user_group(
        &self,
        group_id: &str,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, crate::DataLayerError>;

    async fn delete_user_group(&self, group_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn list_user_group_members(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredUserGroupMember>, crate::DataLayerError>;

    async fn replace_user_group_members(
        &self,
        group_id: &str,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMember>, crate::DataLayerError>;

    async fn list_user_groups_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserGroup>, crate::DataLayerError>;

    async fn list_user_group_memberships_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMembership>, crate::DataLayerError>;

    async fn replace_user_groups_for_user(
        &self,
        user_id: &str,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, crate::DataLayerError>;

    async fn add_user_to_group(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn list_non_admin_export_users(
        &self,
    ) -> Result<Vec<StoredUserExportRow>, crate::DataLayerError>;

    async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn list_user_auth_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn find_user_auth_by_email(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn find_active_user_auth_by_email_ci(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn find_user_auth_by_username(
        &self,
        username: &str,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn list_user_oauth_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserOAuthLinkSummary>, crate::DataLayerError>;

    async fn find_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn touch_oauth_link(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<Value>,
        touched_at: DateTime<Utc>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn create_oauth_auth_user(
        &self,
        email: Option<String>,
        username: String,
        created_at: DateTime<Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn find_oauth_link_owner(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<String>, crate::DataLayerError>;

    async fn has_user_oauth_provider_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn count_user_oauth_links(&self, user_id: &str) -> Result<u64, crate::DataLayerError>;

    #[allow(clippy::too_many_arguments)]
    async fn upsert_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<Value>,
        linked_at: DateTime<Utc>,
    ) -> Result<(), crate::DataLayerError>;

    async fn delete_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn get_or_create_ldap_auth_user(
        &self,
        email: String,
        username: String,
        ldap_dn: Option<String>,
        ldap_username: Option<String>,
        logged_in_at: DateTime<Utc>,
    ) -> Result<Option<LdapAuthUserProvisioningOutcome>, crate::DataLayerError>;

    async fn touch_auth_user_last_login(
        &self,
        user_id: &str,
        logged_in_at: DateTime<Utc>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email: Option<String>,
        username: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: DateTime<Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    #[allow(clippy::too_many_arguments)]
    async fn update_local_auth_user_admin_fields(
        &self,
        user_id: &str,
        role: Option<String>,
        allowed_providers_present: bool,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats_present: bool,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models_present: bool,
        allowed_models: Option<Vec<String>>,
        rate_limit_present: bool,
        rate_limit: Option<i32>,
        is_active: Option<bool>,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn update_local_auth_user_policy_modes(
        &self,
        user_id: &str,
        allowed_providers_mode: Option<String>,
        allowed_api_formats_mode: Option<String>,
        allowed_models_mode: Option<String>,
        rate_limit_mode: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<Value>,
    ) -> Result<Option<Value>, crate::DataLayerError>;

    async fn update_user_feature_settings(
        &self,
        user_id: &str,
        settings: Option<Value>,
    ) -> Result<Option<Value>, crate::DataLayerError>;

    async fn create_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    #[allow(clippy::too_many_arguments)]
    async fn create_local_auth_user_with_settings(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        role: String,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models: Option<Vec<String>>,
        rate_limit: Option<i32>,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    async fn delete_local_auth_user(&self, user_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn read_user_preferences(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserPreferenceRecord>, crate::DataLayerError>;

    async fn write_user_preferences(
        &self,
        preferences: &StoredUserPreferenceRecord,
    ) -> Result<Option<StoredUserPreferenceRecord>, crate::DataLayerError>;

    async fn find_user_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredUserSessionRecord>, crate::DataLayerError>;

    async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserSessionRecord>, crate::DataLayerError>;

    async fn create_user_session(
        &self,
        session: &StoredUserSessionRecord,
    ) -> Result<Option<StoredUserSessionRecord>, crate::DataLayerError>;

    async fn touch_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        touched_at: DateTime<Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn update_user_session_device_label(
        &self,
        user_id: &str,
        session_id: &str,
        device_label: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<bool, crate::DataLayerError>;

    #[allow(clippy::too_many_arguments)]
    async fn rotate_user_session_refresh_token(
        &self,
        user_id: &str,
        session_id: &str,
        previous_refresh_token_hash: &str,
        next_refresh_token_hash: &str,
        rotated_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn revoke_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: DateTime<Utc>,
        reason: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn revoke_all_user_sessions(
        &self,
        user_id: &str,
        revoked_at: DateTime<Utc>,
        reason: &str,
    ) -> Result<u64, crate::DataLayerError>;

    async fn count_active_admin_users(&self) -> Result<u64, crate::DataLayerError>;

    async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, crate::DataLayerError>;
}

fn normalize_optional_json(value: Option<Value>) -> Option<Value> {
    match value {
        Some(Value::Null) | None => None,
        Some(value) => Some(value),
    }
}

pub fn normalize_user_group_name(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn normalize_list_policy_mode(
    value: &str,
    field_name: &str,
) -> Result<String, crate::DataLayerError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "inherit" => Ok("inherit".to_string()),
        "unrestricted" => Ok("unrestricted".to_string()),
        "specific" => Ok("specific".to_string()),
        "deny_all" => Ok("deny_all".to_string()),
        _ => Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} is not a valid list policy mode"
        ))),
    }
}

pub fn normalize_rate_limit_policy_mode(
    value: &str,
    field_name: &str,
) -> Result<String, crate::DataLayerError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "inherit" => Ok("inherit".to_string()),
        "system" => Ok("system".to_string()),
        "custom" => Ok("custom".to_string()),
        _ => Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} is not a valid rate limit policy mode"
        ))),
    }
}

fn legacy_list_policy_mode(values: &Option<Vec<String>>) -> String {
    if values.as_ref().is_some_and(|items| !items.is_empty()) {
        "specific".to_string()
    } else {
        "unrestricted".to_string()
    }
}

fn parse_string_list(
    value: Option<Value>,
    field_name: &str,
) -> Result<Option<Vec<String>>, crate::DataLayerError> {
    let Some(value) = value else {
        return Ok(None);
    };
    parse_string_list_value(&value, field_name)
}

fn parse_string_list_value(
    value: &Value,
    field_name: &str,
) -> Result<Option<Vec<String>>, crate::DataLayerError> {
    match value {
        Value::Null => Ok(None),
        Value::Array(array) => parse_string_list_array(array, field_name).map(Some),
        Value::String(raw) => parse_embedded_string_list(raw, field_name),
        _ => Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} is not a JSON array"
        ))),
    }
}

fn parse_embedded_string_list(
    raw: &str,
    field_name: &str,
) -> Result<Option<Vec<String>>, crate::DataLayerError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("null") {
        return Ok(None);
    }

    if let Ok(decoded) = serde_json::from_str::<Value>(raw) {
        return parse_string_list_value(&decoded, field_name);
    }

    Ok(Some(vec![raw.to_string()]))
}

fn parse_string_list_array(
    array: &[Value],
    field_name: &str,
) -> Result<Vec<String>, crate::DataLayerError> {
    let mut items = Vec::with_capacity(array.len());
    for item in array {
        let Some(item) = item.as_str() else {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name} contains a non-string item"
            )));
        };
        let item = item.trim();
        if !item.is_empty() {
            items.push(item.to_string());
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use serde_json::Value;

    use super::{
        legacy_list_policy_mode, StoredUserAuthRecord, StoredUserExportRow,
        StoredUserPreferenceRecord, StoredUserSessionRecord,
    };

    #[test]
    fn builds_user_export_row_with_allowed_lists() {
        let row = StoredUserExportRow::new(
            "user-1".to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            Some(serde_json::json!(["openai", "anthropic"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            Some(60),
            Some(serde_json::json!({"gpt-4.1": {"cache_1h": true}})),
            true,
        )
        .expect("row should build");

        assert_eq!(
            row.allowed_providers,
            Some(vec!["openai".to_string(), "anthropic".to_string()])
        );
        assert_eq!(
            row.allowed_api_formats,
            Some(vec!["openai:chat".to_string()])
        );
        assert_eq!(row.allowed_models, Some(vec!["gpt-4.1".to_string()]));
        assert_eq!(
            row.model_capability_settings,
            Some(serde_json::json!({"gpt-4.1": {"cache_1h": true}}))
        );
    }

    #[test]
    fn accepts_embedded_string_lists_for_user_export_row() {
        let row = StoredUserExportRow::new(
            "user-1".to_string(),
            None,
            false,
            "alice".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            Some(serde_json::json!("[\"openai\"]")),
            Some(serde_json::json!("null")),
            Some(serde_json::json!("gpt-4.1")),
            None,
            Some(Value::Null),
            true,
        )
        .expect("row should build");

        assert_eq!(row.allowed_providers, Some(vec!["openai".to_string()]));
        assert_eq!(row.allowed_api_formats, None);
        assert_eq!(row.allowed_models, Some(vec!["gpt-4.1".to_string()]));
        assert_eq!(row.model_capability_settings, None);
    }

    #[test]
    fn rejects_object_allowed_providers_for_user_export_row() {
        let result = StoredUserExportRow::new(
            "user-1".to_string(),
            None,
            false,
            "alice".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            Some(serde_json::json!({"bad": true})),
            None,
            None,
            None,
            None,
            true,
        );

        assert!(result.is_err());
    }

    #[test]
    fn builds_user_auth_record_with_allowed_lists() {
        let row = StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            true,
            false,
            None,
            None,
        )
        .expect("auth row should build");

        assert_eq!(row.allowed_providers, Some(vec!["openai".to_string()]));
        assert_eq!(
            row.allowed_api_formats,
            Some(vec!["openai:chat".to_string()])
        );
        assert_eq!(row.allowed_models, Some(vec!["gpt-4.1".to_string()]));
    }

    #[test]
    fn legacy_policy_mode_treats_empty_lists_as_unrestricted() {
        assert_eq!(legacy_list_policy_mode(&None), "unrestricted");
        assert_eq!(legacy_list_policy_mode(&Some(Vec::new())), "unrestricted");
        assert_eq!(
            legacy_list_policy_mode(&Some(vec!["openai".to_string()])),
            "specific"
        );
    }

    #[test]
    fn user_session_previous_refresh_token_has_grace_window() {
        let now = Utc::now();
        let session = StoredUserSessionRecord::new(
            "session-1".to_string(),
            "user-1".to_string(),
            "device-1".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("current-token"),
            Some(StoredUserSessionRecord::hash_refresh_token("prev-token")),
            Some(now - Duration::seconds(StoredUserSessionRecord::REFRESH_GRACE_SECONDS - 1)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("session should build");

        assert_eq!(
            session.verify_refresh_token("prev-token", now),
            (true, true)
        );
        assert_eq!(
            session.verify_refresh_token("current-token", now),
            (true, false)
        );
    }

    #[test]
    fn user_preference_defaults_match_gateway_expectations() {
        let record = StoredUserPreferenceRecord::default_for_user("user-1");

        assert_eq!(record.user_id, "user-1");
        assert_eq!(record.theme, "light");
        assert_eq!(record.language, "zh-CN");
        assert_eq!(record.timezone, "Asia/Shanghai");
        assert!(record.email_notifications);
        assert!(record.usage_alerts);
        assert!(record.announcement_notifications);
    }
}
