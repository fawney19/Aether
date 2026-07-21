use std::collections::BTreeMap;

#[cfg(feature = "mysql")]
use super::MysqlBackend;
#[cfg(feature = "postgres")]
use super::PostgresBackend;
#[cfg(feature = "sqlite")]
use super::SqliteBackend;
use crate::repository::system::{
    AdminSystemPurgeSummary, AdminSystemPurgeTarget, AdminSystemUsageAggregateImportMode,
    AdminSystemUsageAggregateImportSummary, AdminSystemUsageAggregateSnapshot,
    StoredSystemConfigEntry,
};
use crate::DataLayerError;

#[cfg(feature = "mysql")]
mod mysql;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

const ADMIN_CONFIG_PURGE_TABLES: &[&str] = &[
    "api_key_provider_mappings",
    "gemini_file_mappings",
    "provider_usage_tracking",
    "billing_rules",
    "dimension_collectors",
    "models",
    "provider_endpoints",
    "provider_api_keys",
    "providers",
    "global_models",
    "proxy_node_events",
    "proxy_nodes",
    "user_oauth_links",
    "ldap_configs",
    "oauth_providers",
    "auth_modules",
    "system_configs",
];

const ADMIN_STATS_PURGE_TABLES: &[&str] = &[
    "stats_user_daily_cost_savings_model_provider",
    "stats_user_daily_cost_savings_model",
    "stats_user_daily_cost_savings_provider",
    "stats_user_daily_cost_savings",
    "stats_daily_cost_savings_model_provider",
    "stats_daily_cost_savings_model",
    "stats_daily_cost_savings_provider",
    "stats_daily_cost_savings",
    "stats_user_daily_model_provider",
    "stats_daily_model_provider",
    "stats_user_daily_api_format",
    "stats_user_daily_provider",
    "stats_hourly_user_model",
    "stats_user_daily_model",
    "stats_user_summary",
    "stats_hourly_user",
    "stats_user_daily",
    "stats_daily_api_key",
    "stats_daily_error",
    "stats_daily_model",
    "stats_daily_provider",
    "stats_hourly_model",
    "stats_hourly_provider",
    "stats_summary",
    "stats_hourly",
    "stats_daily",
];

const ADMIN_USAGE_CHILD_TABLES: &[&str] = &[
    "usage_body_blobs",
    "usage_http_audits",
    "usage_routing_snapshots",
    "usage_settlement_snapshots",
];

const USAGE_BODY_FIELD_COLUMNS: &[&str] = &[
    "request_body",
    "response_body",
    "provider_request_body",
    "client_response_body",
    "request_body_compressed",
    "response_body_compressed",
    "provider_request_body_compressed",
    "client_response_body_compressed",
];

const ADMIN_USER_SCOPED_TABLES: &[&str] = &[
    "stats_user_daily_cost_savings_model_provider",
    "stats_user_daily_cost_savings_model",
    "stats_user_daily_cost_savings_provider",
    "stats_user_daily_cost_savings",
    "stats_user_daily_model_provider",
    "stats_user_daily_api_format",
    "stats_user_daily_provider",
    "stats_hourly_user_model",
    "stats_user_daily_model",
    "stats_user_summary",
    "stats_hourly_user",
    "stats_user_daily",
    "user_model_usage_counts",
    "announcement_reads",
    "management_tokens",
    "user_preferences",
    "user_sessions",
    "user_oauth_links",
];

fn checked_sql_identifier(value: &str) -> Result<&str, DataLayerError> {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Ok(value)
    } else {
        Err(DataLayerError::InvalidInput(format!(
            "invalid SQL identifier: {value}"
        )))
    }
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn i64_from_u64(value: u64, field_name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field_name} exceeds i64 range")))
}

fn optional_i64_from_u64(
    value: Option<u64>,
    field_name: &str,
) -> Result<Option<i64>, DataLayerError> {
    value
        .map(|value| i64_from_u64(value, field_name))
        .transpose()
}

fn u64_from_i64(value: i64) -> u64 {
    value.max(0) as u64
}

fn add_aggregate_import_count(
    counter: &mut crate::repository::system::AdminSystemUsageAggregateImportCounter,
    existed: bool,
) {
    if existed {
        counter.updated += 1;
    } else {
        counter.created += 1;
    }
}

fn should_skip_imported_aggregate(
    exists: bool,
    mode: AdminSystemUsageAggregateImportMode,
    table: &str,
    date_unix_secs: u64,
) -> Result<bool, DataLayerError> {
    if !exists {
        return Ok(false);
    }
    match mode {
        AdminSystemUsageAggregateImportMode::Skip => Ok(true),
        AdminSystemUsageAggregateImportMode::Overwrite => Ok(false),
        AdminSystemUsageAggregateImportMode::Error => Err(DataLayerError::InvalidInput(format!(
            "{table} aggregate already exists for date_unix_secs={date_unix_secs}"
        ))),
    }
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn serialize_json_value(value: &serde_json::Value) -> Result<String, DataLayerError> {
    serde_json::to_string(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("invalid system config JSON value: {err}"))
    })
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
fn parse_json_value(value: String) -> Result<serde_json::Value, DataLayerError> {
    serde_json::from_str(&value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("invalid system config JSON value: {err}"))
    })
}
