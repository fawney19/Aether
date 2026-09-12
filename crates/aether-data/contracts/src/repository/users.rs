use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

fn redacted_optional_secret<T>(value: &Option<T>) -> Option<&'static str> {
    value.as_ref().map(|_| "[REDACTED]")
}

pub const LAST_ACTIVE_ADMIN_UPDATE_DENIED: &str = "last_active_admin_update_denied";
pub const LAST_ACTIVE_ADMIN_DELETE_DENIED: &str = "last_active_admin_delete_denied";

pub fn is_last_active_admin_update_denied(error: &crate::DataLayerError) -> bool {
    matches!(error, crate::DataLayerError::InvalidInput(message) if message == LAST_ACTIVE_ADMIN_UPDATE_DENIED)
}

pub fn is_last_active_admin_delete_denied(error: &crate::DataLayerError) -> bool {
    matches!(error, crate::DataLayerError::InvalidInput(message) if message == LAST_ACTIVE_ADMIN_DELETE_DENIED)
}

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

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    #[serde(default)]
    pub security_version: i64,
    pub created_at: Option<DateTime<Utc>>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for StoredUserAuthRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredUserAuthRecord")
            .field("id", &self.id)
            .field("email", &self.email)
            .field("email_verified", &self.email_verified)
            .field("username", &self.username)
            .field(
                "password_hash",
                &redacted_optional_secret(&self.password_hash),
            )
            .field("role", &self.role)
            .field("auth_source", &self.auth_source)
            .field("allowed_providers", &self.allowed_providers)
            .field("allowed_providers_mode", &self.allowed_providers_mode)
            .field("allowed_api_formats", &self.allowed_api_formats)
            .field("allowed_api_formats_mode", &self.allowed_api_formats_mode)
            .field("allowed_models", &self.allowed_models)
            .field("allowed_models_mode", &self.allowed_models_mode)
            .field("is_active", &self.is_active)
            .field("is_deleted", &self.is_deleted)
            .field("security_version", &self.security_version)
            .field("created_at", &self.created_at)
            .field("last_login_at", &self.last_login_at)
            .finish()
    }
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
            security_version: 0,
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

    pub fn with_security_version(
        mut self,
        security_version: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if security_version < 0 {
            return Err(crate::DataLayerError::UnexpectedValue(
                "users.security_version is negative".to_string(),
            ));
        }
        self.security_version = security_version;
        Ok(self)
    }

    /// Compare the user fields that an aggregate import is allowed to restore.
    /// Passwords, security versions, and timestamps are intentionally excluded:
    /// password restoration has its own nullable CAS operation and the remaining
    /// fields are server-managed concurrency markers.
    pub fn matches_restore_state(&self, expected: &Self) -> bool {
        self.id == expected.id
            && self.email == expected.email
            && self.email_verified == expected.email_verified
            && self.username == expected.username
            && self.role == expected.role
            && self.auth_source == expected.auth_source
            && self.allowed_providers == expected.allowed_providers
            && self.allowed_providers_mode == expected.allowed_providers_mode
            && self.allowed_api_formats == expected.allowed_api_formats
            && self.allowed_api_formats_mode == expected.allowed_api_formats_mode
            && self.allowed_models == expected.allowed_models
            && self.allowed_models_mode == expected.allowed_models_mode
            && self.is_active == expected.is_active
            && self.is_deleted == expected.is_deleted
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteUserOAuthLinkOutcome {
    Deleted,
    NotFound,
    LastOAuthBinding,
    LastLoginMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindUserOAuthLinkOutcome {
    Bound,
    IdentityAlreadyBoundToUser,
    IdentityBoundToAnotherUser,
    UserAlreadyLinkedProvider,
    UserNotFound,
    SessionUnavailable,
    ProviderNotFound,
    ProviderDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindUserOAuthLinkSessionExpectation {
    pub session_id: String,
    pub client_device_id: String,
    pub security_version: i64,
    pub checked_at: DateTime<Utc>,
}

impl BindUserOAuthLinkSessionExpectation {
    pub fn new(
        session_id: impl Into<String>,
        client_device_id: impl Into<String>,
        security_version: i64,
        checked_at: DateTime<Utc>,
    ) -> Result<Self, crate::DataLayerError> {
        let session_id = session_id.into();
        let client_device_id = client_device_id.into();
        if session_id.trim().is_empty()
            || session_id != session_id.trim()
            || client_device_id.trim().is_empty()
            || client_device_id != client_device_id.trim()
            || security_version < 0
        {
            return Err(crate::DataLayerError::InvalidInput(
                "OAuth link session expectation is invalid".to_string(),
            ));
        }
        Ok(Self {
            session_id,
            client_device_id,
            security_version,
            checked_at,
        })
    }
}

// Returning the full linked-user record avoids a second repository lookup and
// is the established public contract. Keep the success value inline rather
// than changing every adapter/caller to an allocated box.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ResolveOAuthLinkedUserOutcome {
    Linked(StoredUserAuthRecord),
    NotLinked,
    ProviderUnavailable,
}

pub fn last_oauth_unbind_denial(
    auth_source: &str,
    password_hash: Option<&str>,
    local_password_login_allowed: bool,
) -> Option<DeleteUserOAuthLinkOutcome> {
    if auth_source.eq_ignore_ascii_case("oauth") {
        return Some(DeleteUserOAuthLinkOutcome::LastOAuthBinding);
    }
    if !auth_source.eq_ignore_ascii_case("local")
        || !local_password_login_allowed
        || !password_hash.is_some_and(is_valid_bcrypt_hash)
    {
        return Some(DeleteUserOAuthLinkOutcome::LastLoginMethod);
    }
    None
}

pub fn is_valid_bcrypt_hash(value: &str) -> bool {
    use base64::Engine as _;
    use std::str::FromStr;

    let bytes = value.as_bytes();
    if value.len() != 60
        || !matches!(value.get(0..4), Some("$2a$") | Some("$2b$") | Some("$2y$"))
        || !bytes.get(4).is_some_and(u8::is_ascii_digit)
        || !bytes.get(5).is_some_and(u8::is_ascii_digit)
        || bytes.get(6) != Some(&b'$')
    {
        return false;
    }
    let Ok(parts) = bcrypt::HashParts::from_str(value) else {
        return false;
    };
    if !(4..=31).contains(&parts.get_cost()) {
        return false;
    }
    let Some(payload) = value.get(7..) else {
        return false;
    };
    if !payload.bytes().all(is_bcrypt_base64_byte) {
        return false;
    }
    bcrypt::BASE_64
        .decode(&payload[..22])
        .is_ok_and(|salt| salt.len() == 16)
        && bcrypt::BASE_64
            .decode(&payload[22..])
            .is_ok_and(|hash| hash.len() == 23)
}

fn is_bcrypt_base64_byte(value: u8) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, b'.' | b'/')
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

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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

impl std::fmt::Debug for StoredUserExportRow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredUserExportRow")
            .field("id", &self.id)
            .field("email", &self.email)
            .field("email_verified", &self.email_verified)
            .field("username", &self.username)
            .field(
                "password_hash",
                &redacted_optional_secret(&self.password_hash),
            )
            .field("role", &self.role)
            .field("auth_source", &self.auth_source)
            .field("allowed_providers", &self.allowed_providers)
            .field("allowed_providers_mode", &self.allowed_providers_mode)
            .field("allowed_api_formats", &self.allowed_api_formats)
            .field("allowed_api_formats_mode", &self.allowed_api_formats_mode)
            .field("allowed_models", &self.allowed_models)
            .field("allowed_models_mode", &self.allowed_models_mode)
            .field("rate_limit", &self.rate_limit)
            .field("rate_limit_mode", &self.rate_limit_mode)
            .field("model_capability_settings", &self.model_capability_settings)
            .field("feature_settings", &self.feature_settings)
            .field("is_active", &self.is_active)
            .finish()
    }
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

    /// Compare the identity and policy fields that an aggregate import may
    /// restore. Passwords, rate/settings payloads, and server timestamps are
    /// checked separately by the rollback operation.
    pub fn matches_restore_state(&self, expected: &Self) -> bool {
        self.id == expected.id
            && self.email == expected.email
            && self.email_verified == expected.email_verified
            && self.username == expected.username
            && self.role == expected.role
            && self.auth_source == expected.auth_source
            && self.allowed_providers == expected.allowed_providers
            && self.allowed_providers_mode == expected.allowed_providers_mode
            && self.allowed_api_formats == expected.allowed_api_formats
            && self.allowed_api_formats_mode == expected.allowed_api_formats_mode
            && self.allowed_models == expected.allowed_models
            && self.allowed_models_mode == expected.allowed_models_mode
            && self.is_active == expected.is_active
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

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    #[serde(skip)]
    pub security_version: i64,
}

impl std::fmt::Debug for StoredUserSessionRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredUserSessionRecord")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("client_device_id", &self.client_device_id)
            .field("device_label", &self.device_label)
            .field("refresh_token_hash", &"[REDACTED]")
            .field(
                "prev_refresh_token_hash",
                &redacted_optional_secret(&self.prev_refresh_token_hash),
            )
            .field("rotated_at", &self.rotated_at)
            .field("last_seen_at", &self.last_seen_at)
            .field("expires_at", &self.expires_at)
            .field("revoked_at", &self.revoked_at)
            .field("revoke_reason", &self.revoke_reason)
            .field("ip_address", &self.ip_address)
            .field("user_agent", &self.user_agent)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("security_version", &self.security_version)
            .finish()
    }
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
            security_version: 0,
        })
    }

    pub fn with_security_version(
        mut self,
        security_version: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if security_version < 0 {
            return Err(crate::DataLayerError::UnexpectedValue(
                "user_sessions.security_version is negative".to_string(),
            ));
        }
        self.security_version = security_version;
        Ok(self)
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
        let age = now.signed_duration_since(rotated_at);
        if prev_hash == &token_hash
            && age >= chrono::Duration::zero()
            && age <= chrono::Duration::seconds(Self::REFRESH_GRACE_SECONDS)
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
    pub provider_key_policies: std::collections::BTreeMap<String, Vec<String>>,
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
        provider_key_policies: Option<Value>,
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
            provider_key_policies: parse_provider_key_policies(
                provider_key_policies,
                "user_groups.provider_key_policies",
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
    pub provider_key_policies: std::collections::BTreeMap<String, Vec<String>>,
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
    pub sort_by: UserExportSortBy,
    pub sort_order: UserExportSortOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserExportSortBy {
    #[default]
    Id,
    CreatedAt,
}

impl UserExportSortBy {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "created_at" => Some(Self::CreatedAt),
            "id" => Some(Self::Id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserExportSortOrder {
    #[default]
    Asc,
    Desc,
}

impl UserExportSortOrder {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "asc" => Some(Self::Asc),
            "desc" => Some(Self::Desc),
            _ => None,
        }
    }

    pub fn is_desc(self) -> bool {
        matches!(self, Self::Desc)
    }
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

    async fn count_export_users(
        &self,
        query: &UserExportListQuery,
    ) -> Result<u64, crate::DataLayerError>;

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

    /// Restore an existing user group only when its complete stored snapshot
    /// still equals `expected`. The compare and replacement must be atomic so
    /// an import rollback cannot overwrite a concurrent administrator update.
    /// `false` means the row is missing, the identities differ, or the current
    /// snapshot no longer matches the expected post-import state.
    async fn restore_user_group_if_matches(
        &self,
        expected: &StoredUserGroup,
        restored: &StoredUserGroup,
    ) -> Result<bool, crate::DataLayerError> {
        let _ = (expected, restored);
        Err(crate::DataLayerError::InvalidInput(
            "atomic user group restore is not available".to_string(),
        ))
    }

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

    /// Restore a user's group memberships only when the current set still
    /// equals the post-import set. The compare and replacement are atomic.
    async fn restore_user_groups_if_matches(
        &self,
        user_id: &str,
        expected_group_ids: &[String],
        restored_group_ids: &[String],
    ) -> Result<bool, crate::DataLayerError> {
        let _ = (user_id, expected_group_ids, restored_group_ids);
        Err(crate::DataLayerError::InvalidInput(
            "atomic user group restore is not available".to_string(),
        ))
    }

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

    #[allow(clippy::too_many_arguments)]
    async fn resolve_enabled_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<Value>,
        verified_email: Option<&str>,
        touched_at: DateTime<Utc>,
        provider_enabled_snapshot: bool,
    ) -> Result<ResolveOAuthLinkedUserOutcome, crate::DataLayerError>;

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
        email_verified: bool,
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

    async fn has_oauth_links_for_provider(
        &self,
        provider_type: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn count_locked_users_if_oauth_provider_disabled(
        &self,
        _provider_type: &str,
        _enabled_provider_types_snapshot: &[String],
        _ldap_exclusive: bool,
    ) -> Result<usize, crate::DataLayerError> {
        Ok(0)
    }

    #[allow(clippy::too_many_arguments)]
    async fn bind_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<Value>,
        linked_at: DateTime<Utc>,
    ) -> Result<BindUserOAuthLinkOutcome, crate::DataLayerError> {
        self.bind_user_oauth_link_if_provider_enabled(
            user_id,
            provider_type,
            provider_user_id,
            provider_username,
            provider_email,
            extra_data,
            linked_at,
            true,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn bind_user_oauth_link_if_provider_enabled(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<Value>,
        linked_at: DateTime<Utc>,
        provider_enabled_snapshot: bool,
        session_expectation: Option<&BindUserOAuthLinkSessionExpectation>,
    ) -> Result<BindUserOAuthLinkOutcome, crate::DataLayerError>;

    /// Marks an email as verified only if the user's current normalized email still matches.
    async fn upgrade_oauth_email_verification_if_matches(
        &self,
        user_id: &str,
        verified_email: &str,
        verified_at: DateTime<Utc>,
    ) -> Result<bool, crate::DataLayerError>;

    /// Atomically deletes the requested link only when doing so leaves a valid login method.
    async fn delete_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        local_password_login_allowed: bool,
        enabled_provider_types_snapshot: &[String],
    ) -> Result<DeleteUserOAuthLinkOutcome, crate::DataLayerError>;

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
        email_present: bool,
        email: Option<String>,
        email_verified: Option<bool>,
        username: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    /// Restore all user fields represented by an aggregate import checkpoint in
    /// one compare-and-write transaction. The password hash is deliberately
    /// excluded and must be restored through the nullable password CAS method.
    /// Implementations must not write anything when the current state differs
    /// from `expected_auth` (or the exported rate/settings fields).
    #[allow(clippy::too_many_arguments)]
    async fn restore_local_auth_user_state_if_matches(
        &self,
        expected_auth: &StoredUserAuthRecord,
        restored_auth: &StoredUserAuthRecord,
        expected_export: &StoredUserExportRow,
        restored_export: &StoredUserExportRow,
        expected_model_capability_settings: Option<&Value>,
        restored_model_capability_settings: Option<Value>,
        expected_feature_settings: Option<&Value>,
        restored_feature_settings: Option<Value>,
    ) -> Result<bool, crate::DataLayerError> {
        let _ = (
            expected_auth,
            restored_auth,
            expected_export,
            restored_export,
            expected_model_capability_settings,
            restored_model_capability_settings,
            expected_feature_settings,
            restored_feature_settings,
        );
        Err(crate::DataLayerError::InvalidInput(
            "atomic user state restore is not available".to_string(),
        ))
    }

    async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: DateTime<Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, crate::DataLayerError>;

    /// Replace a user's password hash, including an explicit `NULL`, only when the current hash
    /// still equals `expected_password_hash`. The compare and write must be atomic.
    async fn restore_local_auth_user_password_hash_if_matches(
        &self,
        user_id: &str,
        expected_password_hash: Option<&str>,
        password_hash: Option<String>,
        updated_at: DateTime<Utc>,
    ) -> Result<bool, crate::DataLayerError> {
        let _ = (user_id, expected_password_hash, password_hash, updated_at);
        Err(crate::DataLayerError::InvalidInput(
            "atomic nullable password restore is not available".to_string(),
        ))
    }

    async fn reset_local_auth_user_password_and_revoke_sessions(
        &self,
        user_id: &str,
        password_hash: String,
        changed_at: DateTime<Utc>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn change_local_auth_password_and_revoke_sessions(
        &self,
        user_id: &str,
        current_session_id: &str,
        expected_password_hash: Option<&str>,
        next_password_hash: String,
        changed_at: DateTime<Utc>,
    ) -> Result<bool, crate::DataLayerError>;

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

    /// Delete a local-auth user only when no wallet currently belongs to the user.
    ///
    /// Authentication provisioning compensation uses this guard after removing a
    /// wallet it can prove ownership of.  Implementations must evaluate the
    /// wallet absence check in the same database transaction as the user delete;
    /// a default implementation is deliberately fail-closed for repositories
    /// that cannot provide that atomicity.
    async fn delete_local_auth_user_if_wallet_absent(
        &self,
        user_id: &str,
    ) -> Result<bool, crate::DataLayerError> {
        let _ = user_id;
        Err(crate::DataLayerError::InvalidInput(
            "atomic user deletion without a wallet is not available".to_string(),
        ))
    }

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

    async fn create_user_session_if_password_matches(
        &self,
        session: &StoredUserSessionRecord,
        expected_password_hash: &str,
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
        expected_refresh_token_hash: &str,
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

fn parse_provider_key_policies(
    value: Option<Value>,
    field_name: &str,
) -> Result<std::collections::BTreeMap<String, Vec<String>>, crate::DataLayerError> {
    let Some(value) = value else {
        return Ok(std::collections::BTreeMap::new());
    };
    if value.is_null() {
        return Ok(std::collections::BTreeMap::new());
    }
    let value = match value {
        Value::String(raw) => serde_json::from_str::<Value>(raw.trim()).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!("{field_name} is not a JSON object"))
        })?,
        other => other,
    };
    let Value::Object(object) = value else {
        return Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} is not a JSON object"
        )));
    };

    let mut policies = std::collections::BTreeMap::new();
    for (provider_id, key_values) in object {
        let provider_id = provider_id.trim();
        if provider_id.is_empty() {
            continue;
        }
        let Value::Array(key_values) = key_values else {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name}.{provider_id} is not a JSON array"
            )));
        };
        let mut key_ids = key_values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        crate::DataLayerError::UnexpectedValue(format!(
                            "{field_name}.{provider_id} contains a non-string item"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(ToOwned::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        key_ids.sort();
        policies.insert(provider_id.to_string(), key_ids);
    }
    Ok(policies)
}

fn parse_string_list_value(
    value: &Value,
    field_name: &str,
) -> Result<Option<Vec<String>>, crate::DataLayerError> {
    match value {
        Value::Null => Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} contains JSON null; use SQL NULL for an unset policy"
        ))),
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
    if raw.is_empty() {
        return Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} contains an empty string"
        )));
    }
    if raw.eq_ignore_ascii_case("null") {
        return Err(crate::DataLayerError::UnexpectedValue(format!(
            "{field_name} contains stringified JSON null; use SQL NULL for an unset policy"
        )));
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
        if item.is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "{field_name} contains an empty item"
            )));
        }
        items.push(item.to_string());
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use serde_json::Value;

    use super::{
        is_valid_bcrypt_hash, last_oauth_unbind_denial, legacy_list_policy_mode,
        DeleteUserOAuthLinkOutcome, StoredUserAuthRecord, StoredUserExportRow, StoredUserGroup,
        StoredUserPreferenceRecord, StoredUserSessionRecord,
    };

    fn stored_user_group_with_provider_key_policies(
        value: Option<Value>,
    ) -> Result<StoredUserGroup, crate::DataLayerError> {
        StoredUserGroup::new(
            "group-1".to_string(),
            "Team".to_string(),
            "team".to_string(),
            None,
            0,
            Some(serde_json::json!(["provider-1"])),
            "specific".to_string(),
            value,
            None,
            "unrestricted".to_string(),
            None,
            "unrestricted".to_string(),
            None,
            "system".to_string(),
            None,
            None,
        )
    }

    #[test]
    fn null_provider_key_policies_are_backward_compatible() {
        let group = stored_user_group_with_provider_key_policies(None)
            .expect("legacy NULL policy should be accepted");

        assert!(group.provider_key_policies.is_empty());
    }

    #[test]
    fn provider_key_policies_are_trimmed_deduplicated_and_sorted() {
        let group = stored_user_group_with_provider_key_policies(Some(serde_json::json!({
            " provider-1 ": [" key-b ", "key-a", "key-b"]
        })))
        .expect("policy should parse");

        assert_eq!(
            group.provider_key_policies,
            std::collections::BTreeMap::from([(
                "provider-1".to_string(),
                vec!["key-a".to_string(), "key-b".to_string()],
            )])
        );
    }

    #[test]
    fn classifies_last_oauth_unbind_by_remaining_login_method() {
        let valid_hash =
            bcrypt::hash("Secret123!", bcrypt::DEFAULT_COST).expect("bcrypt fixture should hash");

        assert_eq!(
            last_oauth_unbind_denial("oauth", None, false),
            Some(DeleteUserOAuthLinkOutcome::LastOAuthBinding)
        );
        assert_eq!(
            last_oauth_unbind_denial("local", None, true),
            Some(DeleteUserOAuthLinkOutcome::LastLoginMethod)
        );
        assert_eq!(
            last_oauth_unbind_denial("local", Some("not-a-password-hash"), true),
            Some(DeleteUserOAuthLinkOutcome::LastLoginMethod)
        );
        assert_eq!(
            last_oauth_unbind_denial("local", Some(&valid_hash), false),
            Some(DeleteUserOAuthLinkOutcome::LastLoginMethod)
        );
        assert_eq!(
            last_oauth_unbind_denial("local", Some(&valid_hash), true),
            None
        );
    }

    #[test]
    fn validates_complete_bcrypt_encoding_and_cost() {
        let valid_hash =
            bcrypt::hash("Secret123!", bcrypt::DEFAULT_COST).expect("bcrypt fixture should hash");
        assert!(is_valid_bcrypt_hash(&valid_hash));

        for invalid in [
            format!("$2b$99${}", &valid_hash[7..]),
            format!("$2x$12${}", &valid_hash[7..]),
            format!("$2b$12${}", "!".repeat(53)),
            valid_hash[..59].to_string(),
        ] {
            assert!(!is_valid_bcrypt_hash(&invalid), "accepted {invalid}");
        }
    }

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
            None,
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
    fn stored_user_security_lists_distinguish_sql_null_from_malformed_json_null() {
        assert_eq!(
            super::parse_string_list(None, "users.allowed_providers")
                .expect("SQL NULL should remain an unset policy"),
            None
        );
        assert!(
            super::parse_string_list(Some(serde_json::Value::Null), "users.allowed_providers")
                .is_err()
        );
        assert!(super::parse_string_list(
            Some(serde_json::json!("null")),
            "users.allowed_providers"
        )
        .is_err());
        assert!(super::parse_string_list(
            Some(serde_json::json!([" "])),
            "users.allowed_providers"
        )
        .is_err());
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
    fn user_record_debug_output_redacts_password_and_refresh_hashes() {
        let password_hash = "debug-secret-password-hash";
        let auth = StoredUserAuthRecord::new(
            "user-debug".to_string(),
            Some("debug@example.com".to_string()),
            true,
            "debug-user".to_string(),
            Some(password_hash.to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("auth record should build");
        let export = StoredUserExportRow::new(
            "user-debug".to_string(),
            Some("debug@example.com".to_string()),
            true,
            "debug-user".to_string(),
            Some(password_hash.to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            None,
            None,
            true,
        )
        .expect("export record should build");
        let current_hash = "debug-secret-current-refresh-hash";
        let previous_hash = "debug-secret-previous-refresh-hash";
        let session = StoredUserSessionRecord::new(
            "session-debug".to_string(),
            "user-debug".to_string(),
            "device-debug".to_string(),
            None,
            current_hash.to_string(),
            Some(previous_hash.to_string()),
            None,
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

        for rendered in [format!("{auth:?}"), format!("{export:?}")] {
            assert!(!rendered.contains(password_hash));
            assert!(rendered.contains("[REDACTED]"));
        }
        let rendered = format!("{session:?}");
        assert!(!rendered.contains(current_hash));
        assert!(!rendered.contains(previous_hash));
        assert!(rendered.contains("[REDACTED]"));
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

        let future_rotation = StoredUserSessionRecord::new(
            "session-2".to_string(),
            "user-1".to_string(),
            "device-1".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("current-token"),
            Some(StoredUserSessionRecord::hash_refresh_token("prev-token")),
            Some(now + Duration::seconds(1)),
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
            future_rotation.verify_refresh_token("prev-token", now),
            (false, false)
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
