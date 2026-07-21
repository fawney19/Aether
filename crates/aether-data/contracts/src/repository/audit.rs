use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const SUSPICIOUS_EVENT_TYPES: &[&str] = &[
    "suspicious_activity",
    "unauthorized_access",
    "login_failed",
    "request_rate_limited",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLogListQuery {
    pub cutoff_unix_secs: u64,
    pub username_pattern: Option<String>,
    pub event_type: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminAuditLog {
    pub id: String,
    pub event_type: String,
    pub user_id: Option<String>,
    pub user_email: Option<String>,
    pub user_username: Option<String>,
    pub description: Option<String>,
    pub ip_address: Option<String>,
    pub status_code: Option<i32>,
    pub error_message: Option<String>,
    pub metadata: Option<Value>,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredSuspiciousActivity {
    pub id: String,
    pub event_type: String,
    pub user_id: Option<String>,
    pub description: Option<String>,
    pub ip_address: Option<String>,
    pub metadata: Option<Value>,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUserAuditLog {
    pub id: String,
    pub event_type: String,
    pub description: Option<String>,
    pub ip_address: Option<String>,
    pub status_code: Option<i32>,
    pub created_at_unix_secs: u64,
}

fn unix_secs_to_rfc3339(secs: u64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(secs.min(i64::MAX as u64) as i64, 0)
        .map(|value| value.to_rfc3339())
}

impl StoredAdminAuditLog {
    pub fn created_at_rfc3339(&self) -> Option<String> {
        unix_secs_to_rfc3339(self.created_at_unix_secs)
    }
}

impl StoredSuspiciousActivity {
    pub fn created_at_rfc3339(&self) -> Option<String> {
        unix_secs_to_rfc3339(self.created_at_unix_secs)
    }
}

impl StoredUserAuditLog {
    pub fn created_at_rfc3339(&self) -> Option<String> {
        unix_secs_to_rfc3339(self.created_at_unix_secs)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredAdminAuditLogPage {
    pub items: Vec<StoredAdminAuditLog>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredUserAuditLogPage {
    pub items: Vec<StoredUserAuditLog>,
    pub total: u64,
}

#[async_trait]
pub trait AuditLogReadRepository: Send + Sync {
    async fn list_admin_audit_logs(
        &self,
        query: &AuditLogListQuery,
    ) -> Result<StoredAdminAuditLogPage, crate::DataLayerError>;

    async fn list_admin_suspicious_activities(
        &self,
        cutoff_unix_secs: u64,
    ) -> Result<Vec<StoredSuspiciousActivity>, crate::DataLayerError>;

    async fn read_admin_user_behavior_event_counts(
        &self,
        user_id: &str,
        cutoff_unix_secs: u64,
    ) -> Result<std::collections::BTreeMap<String, u64>, crate::DataLayerError>;

    async fn list_user_audit_logs(
        &self,
        user_id: &str,
        query: &AuditLogListQuery,
    ) -> Result<StoredUserAuditLogPage, crate::DataLayerError>;

    async fn delete_audit_logs_before(
        &self,
        cutoff_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, crate::DataLayerError>;
}

pub fn optional_json_from_text(
    value: Option<String>,
) -> Result<Option<Value>, crate::DataLayerError> {
    value
        .filter(|raw| !raw.trim().is_empty())
        .map(|raw| {
            serde_json::from_str(&raw).map_err(|err| {
                crate::DataLayerError::UnexpectedValue(format!(
                    "invalid audit log metadata json: {err}"
                ))
            })
        })
        .transpose()
}
