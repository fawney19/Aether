use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredAuthApiKeySnapshot {
    pub user_id: String,
    pub username: String,
    pub email: Option<String>,
    pub user_role: String,
    pub user_auth_source: String,
    pub user_is_active: bool,
    pub user_is_deleted: bool,
    pub user_rate_limit: Option<i32>,
    pub user_allowed_providers: Option<Vec<String>>,
    pub user_allowed_api_formats: Option<Vec<String>>,
    pub user_allowed_models: Option<Vec<String>>,
    pub api_key_id: String,
    pub api_key_name: Option<String>,
    pub api_key_is_active: bool,
    pub api_key_is_locked: bool,
    pub api_key_is_standalone: bool,
    pub api_key_rate_limit: Option<i32>,
    pub api_key_concurrent_limit: Option<i32>,
    pub api_key_expires_at_unix_secs: Option<u64>,
    pub api_key_allowed_providers: Option<Vec<String>>,
    pub api_key_allowed_api_formats: Option<Vec<String>>,
    pub api_key_allowed_models: Option<Vec<String>>,
}

impl StoredAuthApiKeySnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_id: String,
        username: String,
        email: Option<String>,
        user_role: String,
        user_auth_source: String,
        user_is_active: bool,
        user_is_deleted: bool,
        user_allowed_providers: Option<serde_json::Value>,
        user_allowed_api_formats: Option<serde_json::Value>,
        user_allowed_models: Option<serde_json::Value>,
        api_key_id: String,
        api_key_name: Option<String>,
        api_key_is_active: bool,
        api_key_is_locked: bool,
        api_key_is_standalone: bool,
        api_key_rate_limit: Option<i32>,
        api_key_concurrent_limit: Option<i32>,
        api_key_expires_at_unix_secs: Option<i64>,
        api_key_allowed_providers: Option<serde_json::Value>,
        api_key_allowed_api_formats: Option<serde_json::Value>,
        api_key_allowed_models: Option<serde_json::Value>,
    ) -> Result<Self, crate::DataLayerError> {
        Ok(Self {
            user_id,
            username,
            email,
            user_role,
            user_auth_source,
            user_is_active,
            user_is_deleted,
            user_rate_limit: None,
            user_allowed_providers: parse_string_list(
                user_allowed_providers,
                "users.allowed_providers",
            )?,
            user_allowed_api_formats: parse_string_list(
                user_allowed_api_formats,
                "users.allowed_api_formats",
            )?,
            user_allowed_models: parse_string_list(user_allowed_models, "users.allowed_models")?,
            api_key_id,
            api_key_name,
            api_key_is_active,
            api_key_is_locked,
            api_key_is_standalone,
            api_key_rate_limit,
            api_key_concurrent_limit,
            api_key_expires_at_unix_secs: api_key_expires_at_unix_secs
                .map(|value| {
                    u64::try_from(value).map_err(|_| {
                        crate::DataLayerError::UnexpectedValue(format!(
                            "invalid api_keys.expires_at_unix_secs: {value}"
                        ))
                    })
                })
                .transpose()?,
            api_key_allowed_providers: parse_string_list(
                api_key_allowed_providers,
                "api_keys.allowed_providers",
            )?,
            api_key_allowed_api_formats: parse_string_list(
                api_key_allowed_api_formats,
                "api_keys.allowed_api_formats",
            )?,
            api_key_allowed_models: parse_string_list(
                api_key_allowed_models,
                "api_keys.allowed_models",
            )?,
        })
    }

    pub fn is_currently_usable(&self, now_unix_secs: u64) -> bool {
        if !self.user_is_active || self.user_is_deleted {
            return false;
        }
        if !self.api_key_is_active {
            return false;
        }
        if self.api_key_is_locked && !self.api_key_is_standalone {
            return false;
        }
        if let Some(expires_at_unix_secs) = self.api_key_expires_at_unix_secs {
            if expires_at_unix_secs < now_unix_secs {
                return false;
            }
        }
        true
    }

    pub fn with_user_rate_limit(mut self, user_rate_limit: Option<i32>) -> Self {
        self.user_rate_limit = user_rate_limit;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedAuthApiKeySnapshot {
    pub user_id: String,
    pub username: String,
    pub email: Option<String>,
    pub user_role: String,
    pub user_auth_source: String,
    pub user_is_active: bool,
    pub user_is_deleted: bool,
    pub user_rate_limit: Option<i32>,
    pub user_allowed_providers: Option<Vec<String>>,
    pub user_allowed_api_formats: Option<Vec<String>>,
    pub user_allowed_models: Option<Vec<String>>,
    pub api_key_id: String,
    pub api_key_name: Option<String>,
    pub api_key_is_active: bool,
    pub api_key_is_locked: bool,
    pub api_key_is_standalone: bool,
    pub api_key_rate_limit: Option<i32>,
    pub api_key_concurrent_limit: Option<i32>,
    pub api_key_expires_at_unix_secs: Option<u64>,
    pub api_key_allowed_providers: Option<Vec<String>>,
    pub api_key_allowed_api_formats: Option<Vec<String>>,
    pub api_key_allowed_models: Option<Vec<String>>,
    pub currently_usable: bool,
}

impl ResolvedAuthApiKeySnapshot {
    pub fn from_stored(snapshot: StoredAuthApiKeySnapshot, now_unix_secs: u64) -> Self {
        let currently_usable = snapshot.is_currently_usable(now_unix_secs);
        Self {
            user_id: snapshot.user_id,
            username: snapshot.username,
            email: snapshot.email,
            user_role: snapshot.user_role,
            user_auth_source: snapshot.user_auth_source,
            user_is_active: snapshot.user_is_active,
            user_is_deleted: snapshot.user_is_deleted,
            user_rate_limit: snapshot.user_rate_limit,
            user_allowed_providers: snapshot.user_allowed_providers,
            user_allowed_api_formats: snapshot.user_allowed_api_formats,
            user_allowed_models: snapshot.user_allowed_models,
            api_key_id: snapshot.api_key_id,
            api_key_name: snapshot.api_key_name,
            api_key_is_active: snapshot.api_key_is_active,
            api_key_is_locked: snapshot.api_key_is_locked,
            api_key_is_standalone: snapshot.api_key_is_standalone,
            api_key_rate_limit: snapshot.api_key_rate_limit,
            api_key_concurrent_limit: snapshot.api_key_concurrent_limit,
            api_key_expires_at_unix_secs: snapshot.api_key_expires_at_unix_secs,
            api_key_allowed_providers: snapshot.api_key_allowed_providers,
            api_key_allowed_api_formats: snapshot.api_key_allowed_api_formats,
            api_key_allowed_models: snapshot.api_key_allowed_models,
            currently_usable,
        }
    }

    pub fn effective_allowed_providers(&self) -> Option<&[String]> {
        self.api_key_allowed_providers
            .as_deref()
            .or(self.user_allowed_providers.as_deref())
    }

    pub fn effective_allowed_api_formats(&self) -> Option<&[String]> {
        self.api_key_allowed_api_formats
            .as_deref()
            .or(self.user_allowed_api_formats.as_deref())
    }

    pub fn effective_allowed_models(&self) -> Option<&[String]> {
        self.api_key_allowed_models
            .as_deref()
            .or(self.user_allowed_models.as_deref())
    }
}

#[async_trait]
pub trait ResolvedAuthApiKeySnapshotReader: Send + Sync {
    async fn find_stored_auth_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, crate::DataLayerError>;
}

pub async fn read_resolved_auth_api_key_snapshot(
    reader: &impl ResolvedAuthApiKeySnapshotReader,
    key: AuthApiKeyLookupKey<'_>,
    now_unix_secs: u64,
) -> Result<Option<ResolvedAuthApiKeySnapshot>, crate::DataLayerError> {
    Ok(reader
        .find_stored_auth_api_key_snapshot(key)
        .await?
        .map(|snapshot| ResolvedAuthApiKeySnapshot::from_stored(snapshot, now_unix_secs)))
}

pub async fn read_resolved_auth_api_key_snapshot_by_key_hash(
    reader: &impl ResolvedAuthApiKeySnapshotReader,
    key_hash: &str,
    now_unix_secs: u64,
) -> Result<Option<ResolvedAuthApiKeySnapshot>, crate::DataLayerError> {
    read_resolved_auth_api_key_snapshot(
        reader,
        AuthApiKeyLookupKey::KeyHash(key_hash),
        now_unix_secs,
    )
    .await
}

pub async fn read_resolved_auth_api_key_snapshot_by_user_api_key_ids(
    reader: &impl ResolvedAuthApiKeySnapshotReader,
    user_id: &str,
    api_key_id: &str,
    now_unix_secs: u64,
) -> Result<Option<ResolvedAuthApiKeySnapshot>, crate::DataLayerError> {
    read_resolved_auth_api_key_snapshot(
        reader,
        AuthApiKeyLookupKey::UserApiKeyIds {
            user_id,
            api_key_id,
        },
        now_unix_secs,
    )
    .await
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAuthApiKeyExportRecord {
    pub user_id: String,
    pub api_key_id: String,
    pub key_hash: String,
    pub key_encrypted: Option<String>,
    pub name: Option<String>,
    pub allowed_providers: Option<Vec<String>>,
    pub allowed_api_formats: Option<Vec<String>>,
    pub allowed_models: Option<Vec<String>>,
    pub rate_limit: Option<i32>,
    pub concurrent_limit: Option<i32>,
    pub force_capabilities: Option<serde_json::Value>,
    pub is_active: bool,
    pub expires_at_unix_secs: Option<u64>,
    pub auto_delete_on_expiry: bool,
    pub total_requests: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub last_used_at_unix_secs: Option<u64>,
    pub created_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    pub is_standalone: bool,
}

impl StoredAuthApiKeyExportRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_id: String,
        api_key_id: String,
        key_hash: String,
        key_encrypted: Option<String>,
        name: Option<String>,
        allowed_providers: Option<serde_json::Value>,
        allowed_api_formats: Option<serde_json::Value>,
        allowed_models: Option<serde_json::Value>,
        rate_limit: Option<i32>,
        concurrent_limit: Option<i32>,
        force_capabilities: Option<serde_json::Value>,
        is_active: bool,
        expires_at_unix_secs: Option<i64>,
        auto_delete_on_expiry: bool,
        total_requests: i64,
        total_tokens: i64,
        total_cost_usd: f64,
        is_standalone: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if user_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "api_keys.user_id is empty".to_string(),
            ));
        }
        if api_key_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "api_keys.id is empty".to_string(),
            ));
        }
        if key_hash.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "api_keys.key_hash is empty".to_string(),
            ));
        }
        if !total_cost_usd.is_finite() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "api_keys.total_cost_usd is not finite".to_string(),
            ));
        }

        Ok(Self {
            user_id,
            api_key_id,
            key_hash,
            key_encrypted,
            name,
            allowed_providers: parse_string_list(allowed_providers, "api_keys.allowed_providers")?,
            allowed_api_formats: parse_string_list(
                allowed_api_formats,
                "api_keys.allowed_api_formats",
            )?,
            allowed_models: parse_string_list(allowed_models, "api_keys.allowed_models")?,
            rate_limit,
            concurrent_limit,
            force_capabilities,
            is_active,
            expires_at_unix_secs: expires_at_unix_secs
                .map(|value| parse_u64_i64(value, "api_keys.expires_at_unix_secs"))
                .transpose()?,
            auto_delete_on_expiry,
            total_requests: parse_u64_i64(total_requests, "api_keys.total_requests")?,
            total_tokens: parse_u64_i64(total_tokens, "api_keys.total_tokens")?,
            total_cost_usd,
            last_used_at_unix_secs: None,
            created_at_unix_secs: None,
            updated_at_unix_secs: None,
            is_standalone,
        })
    }

    pub fn with_activity_timestamps(
        mut self,
        last_used_at_unix_secs: Option<i64>,
        created_at_unix_secs: Option<i64>,
        updated_at_unix_secs: Option<i64>,
    ) -> Result<Self, crate::DataLayerError> {
        self.last_used_at_unix_secs = last_used_at_unix_secs
            .map(|value| parse_u64_i64(value, "api_keys.last_used_at_unix_secs"))
            .transpose()?;
        self.created_at_unix_secs = created_at_unix_secs
            .map(|value| parse_u64_i64(value, "api_keys.created_at_unix_secs"))
            .transpose()?;
        self.updated_at_unix_secs = updated_at_unix_secs
            .map(|value| parse_u64_i64(value, "api_keys.updated_at_unix_secs"))
            .transpose()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AuthApiKeyExportSummary {
    pub total: u64,
    pub active: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StandaloneApiKeyExportListQuery {
    pub skip: usize,
    pub limit: usize,
    pub is_active: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserApiKeyRecord {
    pub user_id: String,
    pub api_key_id: String,
    pub key_hash: String,
    pub key_encrypted: Option<String>,
    pub name: Option<String>,
    pub allowed_providers: Option<Vec<String>>,
    pub allowed_api_formats: Option<Vec<String>>,
    pub allowed_models: Option<Vec<String>>,
    pub rate_limit: i32,
    pub concurrent_limit: Option<i32>,
    pub force_capabilities: Option<serde_json::Value>,
    pub is_active: bool,
    pub expires_at_unix_secs: Option<u64>,
    pub auto_delete_on_expiry: bool,
    pub total_requests: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateUserApiKeyBasicRecord {
    pub user_id: String,
    pub api_key_id: String,
    pub name: Option<String>,
    pub rate_limit: Option<i32>,
    pub concurrent_limit: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateStandaloneApiKeyRecord {
    pub user_id: String,
    pub api_key_id: String,
    pub key_hash: String,
    pub key_encrypted: Option<String>,
    pub name: Option<String>,
    pub allowed_providers: Option<Vec<String>>,
    pub allowed_api_formats: Option<Vec<String>>,
    pub allowed_models: Option<Vec<String>>,
    pub rate_limit: Option<i32>,
    pub concurrent_limit: Option<i32>,
    pub force_capabilities: Option<serde_json::Value>,
    pub is_active: bool,
    pub expires_at_unix_secs: Option<u64>,
    pub auto_delete_on_expiry: bool,
    pub total_requests: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStandaloneApiKeyBasicRecord {
    pub api_key_id: String,
    pub name: Option<String>,
    pub rate_limit_present: bool,
    pub rate_limit: Option<i32>,
    pub concurrent_limit_present: bool,
    pub concurrent_limit: Option<i32>,
    pub allowed_providers: Option<Option<Vec<String>>>,
    pub allowed_api_formats: Option<Option<Vec<String>>>,
    pub allowed_models: Option<Option<Vec<String>>>,
    pub expires_at_present: bool,
    pub expires_at_unix_secs: Option<u64>,
    pub auto_delete_on_expiry_present: bool,
    pub auto_delete_on_expiry: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthApiKeyLookupKey<'a> {
    KeyHash(&'a str),
    ApiKeyId(&'a str),
    UserApiKeyIds {
        user_id: &'a str,
        api_key_id: &'a str,
    },
}

#[async_trait]
pub trait AuthApiKeyReadRepository: Send + Sync {
    async fn find_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, crate::DataLayerError>;

    async fn list_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, crate::DataLayerError>;

    async fn list_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn list_export_api_keys_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn list_export_api_keys_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn list_export_standalone_api_keys_page(
        &self,
        query: &StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn count_export_standalone_api_keys(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, crate::DataLayerError>;

    async fn summarize_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, crate::DataLayerError>;

    async fn summarize_export_non_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, crate::DataLayerError>;

    async fn summarize_export_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, crate::DataLayerError>;

    async fn find_export_standalone_api_key_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn list_export_standalone_api_keys(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;
}

#[async_trait]
pub trait AuthApiKeyWriteRepository: Send + Sync {
    async fn touch_last_used_at(&self, api_key_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn create_user_api_key(
        &self,
        record: CreateUserApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn create_standalone_api_key(
        &self,
        record: CreateStandaloneApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn update_user_api_key_basic(
        &self,
        record: UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn update_standalone_api_key_basic(
        &self,
        record: UpdateStandaloneApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn set_user_api_key_active(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn set_standalone_api_key_active(
        &self,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn set_user_api_key_locked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_locked: bool,
    ) -> Result<bool, crate::DataLayerError>;

    async fn set_user_api_key_allowed_providers(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn set_user_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, crate::DataLayerError>;

    async fn delete_user_api_key(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn delete_standalone_api_key(
        &self,
        api_key_id: &str,
    ) -> Result<bool, crate::DataLayerError>;
}

pub trait AuthRepository:
    AuthApiKeyReadRepository + AuthApiKeyWriteRepository + Send + Sync
{
}

impl<T> AuthRepository for T where
    T: AuthApiKeyReadRepository + AuthApiKeyWriteRepository + Send + Sync
{
}

fn parse_string_list(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<Vec<String>>, crate::DataLayerError> {
    let Some(value) = value else {
        return Ok(None);
    };
    parse_string_list_value(&value, field_name)
}

fn parse_string_list_value(
    value: &serde_json::Value,
    field_name: &str,
) -> Result<Option<Vec<String>>, crate::DataLayerError> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Array(array) => parse_string_list_array(array, field_name).map(Some),
        serde_json::Value::String(raw) => parse_embedded_string_list(raw, field_name),
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

    if let Ok(decoded) = serde_json::from_str::<serde_json::Value>(raw) {
        return parse_string_list_value(&decoded, field_name);
    }

    Ok(Some(vec![raw.to_string()]))
}

fn parse_string_list_array(
    array: &[serde_json::Value],
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

fn parse_u64_i64(value: i64, field_name: &str) -> Result<u64, crate::DataLayerError> {
    u64::try_from(value).map_err(|_| {
        crate::DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}"))
    })
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::{
        read_resolved_auth_api_key_snapshot_by_key_hash,
        read_resolved_auth_api_key_snapshot_by_user_api_key_ids, AuthApiKeyLookupKey,
        ResolvedAuthApiKeySnapshot, ResolvedAuthApiKeySnapshotReader, StoredAuthApiKeyExportRecord,
        StoredAuthApiKeySnapshot,
    };

    #[test]
    fn rejects_non_array_allowed_providers() {
        assert!(StoredAuthApiKeySnapshot::new(
            "user-1".to_string(),
            "alice".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!({"bad": true})),
            None,
            None,
            "key-1".to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            None,
            None,
            None,
            None,
        )
        .is_err());
    }

    #[test]
    fn accepts_stringified_allowed_provider_array() {
        let snapshot = StoredAuthApiKeySnapshot::new(
            "user-1".to_string(),
            "alice".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!("[\"openai\", \" gemini \"]")),
            None,
            None,
            "key-1".to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            None,
            None,
            None,
            None,
        )
        .expect("snapshot should build");

        assert_eq!(
            snapshot.user_allowed_providers,
            Some(vec!["openai".to_string(), "gemini".to_string()])
        );
    }

    #[test]
    fn accepts_single_string_allowed_provider() {
        let snapshot = StoredAuthApiKeySnapshot::new(
            "user-1".to_string(),
            "alice".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!("openai")),
            None,
            None,
            "key-1".to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            None,
            None,
            None,
            None,
        )
        .expect("snapshot should build");

        assert_eq!(
            snapshot.user_allowed_providers,
            Some(vec!["openai".to_string()])
        );
    }

    #[test]
    fn expired_non_standalone_key_is_not_usable() {
        let snapshot = StoredAuthApiKeySnapshot::new(
            "user-1".to_string(),
            "alice".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            None,
            None,
            None,
            "key-1".to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(100),
            None,
            None,
            None,
        )
        .expect("snapshot should build");

        assert!(!snapshot.is_currently_usable(101));
    }

    #[test]
    fn resolved_snapshot_prefers_api_key_lists_over_user_lists() {
        let snapshot = StoredAuthApiKeySnapshot::new(
            "user-1".to_string(),
            "alice".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            "key-1".to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(200),
            Some(serde_json::json!(["anthropic"])),
            None,
            None,
        )
        .expect("snapshot should build");

        let resolved = ResolvedAuthApiKeySnapshot::from_stored(snapshot, 150);

        assert!(resolved.currently_usable);
        assert_eq!(
            resolved.effective_allowed_providers(),
            Some(&["anthropic".to_string()][..])
        );
        assert_eq!(
            resolved.effective_allowed_api_formats(),
            Some(&["openai:chat".to_string()][..])
        );
        assert_eq!(
            resolved.effective_allowed_models(),
            Some(&["gpt-4.1".to_string()][..])
        );
    }

    #[test]
    fn export_record_rejects_negative_totals() {
        assert!(StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-1".to_string(),
            Some("enc".to_string()),
            Some("default".to_string()),
            None,
            None,
            None,
            Some(60),
            Some(5),
            None,
            true,
            None,
            false,
            -1,
            0,
            0.0,
            false,
        )
        .is_err());
    }

    #[test]
    fn export_record_accepts_stringified_allowed_models() {
        let record = StoredAuthApiKeyExportRecord::new(
            "user-1".to_string(),
            "key-1".to_string(),
            "hash-1".to_string(),
            Some("enc".to_string()),
            Some("default".to_string()),
            None,
            None,
            Some(serde_json::json!("[\"gpt-5\", \" gpt-4.1 \"]")),
            Some(60),
            Some(5),
            Some(serde_json::json!({"cache_1h": true})),
            true,
            Some(200),
            false,
            12,
            480,
            1.25,
            false,
        )
        .expect("export record should build");

        assert_eq!(
            record.allowed_models,
            Some(vec!["gpt-5".to_string(), "gpt-4.1".to_string()])
        );
        assert_eq!(record.total_requests, 12);
        assert_eq!(record.total_cost_usd, 1.25);
    }

    #[derive(Default)]
    struct FakeResolvedAuthApiKeySnapshotReader {
        stored: Option<StoredAuthApiKeySnapshot>,
    }

    #[async_trait]
    impl ResolvedAuthApiKeySnapshotReader for FakeResolvedAuthApiKeySnapshotReader {
        async fn find_stored_auth_api_key_snapshot(
            &self,
            _key: AuthApiKeyLookupKey<'_>,
        ) -> Result<Option<StoredAuthApiKeySnapshot>, crate::DataLayerError> {
            Ok(self.stored.clone())
        }
    }

    #[tokio::test]
    async fn reads_resolved_auth_snapshot_by_user_api_key_ids() {
        let reader = FakeResolvedAuthApiKeySnapshotReader {
            stored: Some(sample_auth_snapshot("key-1", "user-1")),
        };

        let snapshot = read_resolved_auth_api_key_snapshot_by_user_api_key_ids(
            &reader, "user-1", "key-1", 150,
        )
        .await
        .expect("snapshot should read")
        .expect("snapshot should exist");

        assert_eq!(snapshot.user_id, "user-1");
        assert_eq!(snapshot.api_key_id, "key-1");
        assert!(snapshot.currently_usable);
    }

    #[tokio::test]
    async fn reads_resolved_auth_snapshot_by_key_hash() {
        let reader = FakeResolvedAuthApiKeySnapshotReader {
            stored: Some(sample_auth_snapshot("key-1", "user-1")),
        };

        let snapshot = read_resolved_auth_api_key_snapshot_by_key_hash(&reader, "hash-lookup", 150)
            .await
            .expect("snapshot should read")
            .expect("snapshot should exist");

        assert_eq!(
            snapshot.effective_allowed_providers(),
            Some(&["openai".to_string()][..])
        );
        assert_eq!(
            snapshot.effective_allowed_api_formats(),
            Some(&["openai:chat".to_string()][..])
        );
        assert_eq!(
            snapshot.effective_allowed_models(),
            Some(&["gpt-4.1".to_string()][..])
        );
    }

    fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(200),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
        )
        .expect("snapshot should build")
    }
}
