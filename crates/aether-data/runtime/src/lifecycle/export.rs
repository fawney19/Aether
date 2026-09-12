use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;

use aether_data_contracts::repository::candidates::{
    sanitize_request_candidate_error_type, sanitize_request_candidate_extra_data,
    sanitize_request_candidate_required_capabilities, sanitize_request_candidate_skip_reason,
};

use crate::error::SqlResultExt;
use crate::{DataLayerError, DatabaseDriver, SqlDatabaseConfig};

#[cfg(feature = "postgres")]
mod postgres;

#[cfg(all(test, feature = "postgres"))]
mod tests;

#[cfg(feature = "postgres")]
pub use postgres::{
    export_postgres_core_jsonl, export_postgres_jsonl, import_postgres_jsonl, import_postgres_plan,
};

#[cfg(all(test, feature = "postgres"))]
use postgres::normalize_postgres_import_payload;

pub const EXPORT_FORMAT_VERSION: u32 = 2;
const MIN_SUPPORTED_EXPORT_FORMAT_VERSION: u32 = 1;

// JSONL imports are ultimately materialized as a `DataImportPlan`, so an
// attacker-controlled document can otherwise consume memory in both the input
// string and the parsed row/payload vectors. Keep these bounds deliberately
// separate from HTTP request limits: database exports may contain large body
// blobs, while still needing a finite parser budget. The total budget is kept
// below the gateway's 256 MiB request-body ceiling because parsing duplicates
// portions of the input in serde values and the import plan.
pub const MAX_JSONL_INPUT_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_JSONL_LINE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_JSONL_RECORDS: usize = 1_000_000;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ExportDomain {
    Users,
    ApiKeys,
    Providers,
    ProviderKeys,
    Endpoints,
    GlobalModels,
    Models,
    AuthModules,
    OAuthProviders,
    UserOAuthLinks,
    UserGroups,
    UserGroupMembers,
    ProxyNodes,
    SystemConfigs,
    Wallets,
    Usage,
    Billing,
    Auxiliary,
}

impl ExportDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Users => "users",
            Self::ApiKeys => "api_keys",
            Self::Providers => "providers",
            Self::ProviderKeys => "provider_keys",
            Self::Endpoints => "endpoints",
            Self::Models => "models",
            Self::GlobalModels => "global_models",
            Self::AuthModules => "auth_modules",
            Self::OAuthProviders => "oauth_providers",
            Self::UserOAuthLinks => "user_oauth_links",
            Self::UserGroups => "user_groups",
            Self::UserGroupMembers => "user_group_members",
            Self::ProxyNodes => "proxy_nodes",
            Self::SystemConfigs => "system_configs",
            Self::Wallets => "wallets",
            Self::Usage => "usage",
            Self::Billing => "billing",
            Self::Auxiliary => "auxiliary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AuxiliaryTable {
    name: &'static str,
    primary_key: &'static [&'static str],
}

const AUXILIARY_TABLES: &[AuxiliaryTable] = &[
    AuxiliaryTable {
        name: "audit_logs",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "announcements",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "announcement_reads",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "management_tokens",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "user_preferences",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "user_sessions",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "ldap_configs",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "pool_member_scores",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "api_key_provider_mappings",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "provider_usage_tracking",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "gemini_file_mappings",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "routing_groups",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "routing_group_versions",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "routing_group_bindings",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "proxy_node_events",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "proxy_node_metrics_1m",
        primary_key: &["node_id", "bucket_start_unix_secs"],
    },
    AuxiliaryTable {
        name: "proxy_node_metrics_1h",
        primary_key: &["node_id", "bucket_start_unix_secs"],
    },
    AuxiliaryTable {
        name: "user_invite_codes",
        primary_key: &["user_id"],
    },
    AuxiliaryTable {
        name: "user_referrals",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "referral_rewards",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "payment_gateway_configs",
        primary_key: &["provider"],
    },
    AuxiliaryTable {
        name: "billing_plans",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "user_plan_entitlements",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "entitlement_usage_ledgers",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "request_candidates",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "video_tasks",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "usage_body_blobs",
        primary_key: &["body_ref"],
    },
    AuxiliaryTable {
        name: "usage_http_audits",
        primary_key: &["request_id"],
    },
    AuxiliaryTable {
        name: "usage_routing_snapshots",
        primary_key: &["request_id"],
    },
    AuxiliaryTable {
        name: "usage_counter_deltas",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "usage_cost_reservations",
        primary_key: &["reservation_token"],
    },
    AuxiliaryTable {
        name: "usage_request_admissions",
        primary_key: &["event_token"],
    },
    AuxiliaryTable {
        name: "background_task_runs",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "background_task_events",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_hourly",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_summary",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_hourly_user",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_hourly_user_model",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "user_model_usage_counts",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_hourly_model",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_hourly_provider",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_model",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_provider",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_api_key",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_error",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_summary",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily_model",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily_provider",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily_api_format",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_model_provider",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily_model_provider",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_cost_savings",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_cost_savings_provider",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_cost_savings_model",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_daily_cost_savings_model_provider",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily_cost_savings",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily_cost_savings_provider",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily_cost_savings_model",
        primary_key: &["id"],
    },
    AuxiliaryTable {
        name: "stats_user_daily_cost_savings_model_provider",
        primary_key: &["id"],
    },
];

fn auxiliary_table(table_name: &str) -> Result<AuxiliaryTable, DataLayerError> {
    AUXILIARY_TABLES
        .iter()
        .copied()
        .find(|table| table.name == table_name)
        .ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "unsupported auxiliary export table '{table_name}'"
            ))
        })
}

fn auxiliary_row_id(table: AuxiliaryTable, payload: &Value) -> Result<String, DataLayerError> {
    let object = payload.as_object().ok_or_else(|| {
        DataLayerError::UnexpectedValue(format!(
            "auxiliary export row in table '{}' is not a JSON object",
            table.name
        ))
    })?;
    let key = table
        .primary_key
        .iter()
        .map(|column| {
            object
                .get(*column)
                .filter(|value| !value.is_null())
                .cloned()
                .ok_or_else(|| {
                    DataLayerError::UnexpectedValue(format!(
                        "auxiliary export row in table '{}' has null or missing primary key column '{}'",
                        table.name, column
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let encoded = serde_json::to_string(&key)
        .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
    Ok(format!("{}:{encoded}", table.name))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DataExportManifest {
    pub format_version: u32,
    pub created_at_unix_secs: u64,
    pub source_driver: Option<DatabaseDriver>,
    pub domains: Vec<ExportDomain>,
}

impl DataExportManifest {
    pub fn new(
        created_at_unix_secs: u64,
        source_driver: Option<DatabaseDriver>,
        domains: Vec<ExportDomain>,
    ) -> Self {
        let mut domains = domains;
        domains.sort();
        domains.dedup();
        Self {
            format_version: EXPORT_FORMAT_VERSION,
            created_at_unix_secs,
            source_driver,
            domains,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
pub enum DataExportRecord {
    Manifest {
        manifest: DataExportManifest,
    },
    Row {
        domain: ExportDomain,
        id: String,
        payload: Value,
    },
}

impl DataExportRecord {
    pub fn manifest(manifest: DataExportManifest) -> Self {
        Self::Manifest { manifest }
    }

    pub fn row(domain: ExportDomain, id: impl Into<String>, payload: Value) -> Self {
        Self::Row {
            domain,
            id: id.into(),
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataImportPlan {
    pub manifest: DataExportManifest,
    pub rows_by_domain: BTreeMap<ExportDomain, Vec<ExportRow>>,
}

impl DataImportPlan {
    pub fn rows(&self, domain: ExportDomain) -> &[ExportRow] {
        self.rows_by_domain
            .get(&domain)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn imports_domain(&self, domain: ExportDomain) -> bool {
        self.manifest.domains.contains(&domain)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportRow {
    pub id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct IdentityImportScope {
    user_ids: Vec<String>,
    oauth_link_ids: Vec<String>,
    oauth_provider_types: Vec<String>,
    finalizes_oauth_links: bool,
    validates_oauth_login_methods: bool,
}

impl IdentityImportScope {
    fn from_plan(plan: &DataImportPlan) -> Result<Self, DataLayerError> {
        let scope = Self {
            user_ids: imported_payload_ids(plan, ExportDomain::Users, "id")?,
            oauth_link_ids: imported_payload_ids(plan, ExportDomain::UserOAuthLinks, "id")?,
            oauth_provider_types: imported_payload_ids(
                plan,
                ExportDomain::OAuthProviders,
                "provider_type",
            )?,
            finalizes_oauth_links: plan.imports_domain(ExportDomain::UserOAuthLinks),
            validates_oauth_login_methods: plan.imports_domain(ExportDomain::UserOAuthLinks)
                || plan.imports_domain(ExportDomain::OAuthProviders),
        };
        if let Some(provider_type) = scope.oauth_provider_types.iter().find(|provider_type| {
            provider_type.is_empty()
                || provider_type.as_str() != provider_type.trim().to_ascii_lowercase()
        }) {
            return Err(DataLayerError::InvalidInput(format!(
                "OAuth provider import has non-canonical provider_type '{provider_type}'"
            )));
        }
        Ok(scope)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct IdentityImportState {
    affected_user_ids: BTreeSet<String>,
}

fn imported_payload_ids(
    plan: &DataImportPlan,
    domain: ExportDomain,
    payload_field: &str,
) -> Result<Vec<String>, DataLayerError> {
    plan.rows(domain)
        .iter()
        .map(|row| {
            let payload_id = row
                .payload
                .as_object()
                .and_then(|payload| payload.get(payload_field))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    DataLayerError::InvalidInput(format!(
                        "{} export row '{}' must contain a non-empty string {}",
                        domain.as_str(),
                        row.id,
                        payload_field
                    ))
                })?;
            if payload_id != row.id {
                return Err(DataLayerError::InvalidInput(format!(
                    "{} export row id '{}' does not match payload {} '{}'",
                    domain.as_str(),
                    row.id,
                    payload_field,
                    payload_id
                )));
            }
            Ok(payload_id.to_string())
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataImportOptions {
    pub preserve_credentials: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DataCopyOptions {
    pub omit_request_body_details: bool,
    pub preserve_credentials: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(feature = "postgres")]
struct PostgresImportColumn {
    data_type: String,
    udt_name: String,
    is_nullable: bool,
    has_default: bool,
}

#[cfg(feature = "postgres")]
type PostgresImportColumns = BTreeMap<String, PostgresImportColumn>;

const IMPORTED_CREDENTIAL_REVOKE_REASON: &str = "imported_credentials_revoked";

fn imported_credential_tombstone() -> String {
    format!("{:x}", Sha256::digest(uuid::Uuid::new_v4().as_bytes()))
}

fn set_supported_import_value(
    object: &mut serde_json::Map<String, Value>,
    target_has_column: &impl Fn(&str) -> bool,
    column: &str,
    value: Value,
) {
    if target_has_column(column) {
        object.insert(column.to_string(), value);
    }
}

fn apply_import_credential_policy(
    table_name: &str,
    object: &mut serde_json::Map<String, Value>,
    target_has_column: impl Fn(&str) -> bool,
    options: DataImportOptions,
) {
    let normalized_table = table_name
        .rsplit('.')
        .next()
        .unwrap_or(table_name)
        .trim_matches(|character| matches!(character, '"' | '`'));
    if options.preserve_credentials
        && matches!(normalized_table, "users" | "api_keys" | "management_tokens")
    {
        return;
    }
    deactivate_imported_credentials(table_name, object, target_has_column);
}

fn deactivate_imported_credentials(
    table_name: &str,
    object: &mut serde_json::Map<String, Value>,
    target_has_column: impl Fn(&str) -> bool,
) {
    let table_name = table_name
        .rsplit('.')
        .next()
        .unwrap_or(table_name)
        .trim_matches(|ch| matches!(ch, '"' | '`'));

    match table_name {
        "users"
            if object
                .get("password_hash")
                .is_some_and(|value| !value.is_null()) =>
        {
            set_supported_import_value(
                object,
                &target_has_column,
                "password_hash",
                Value::String(format!(
                    "$aether-import-revoked${}",
                    imported_credential_tombstone()
                )),
            );
        }
        "users" => {}
        "api_keys" => {
            if object.contains_key("key_hash") {
                set_supported_import_value(
                    object,
                    &target_has_column,
                    "key_hash",
                    Value::String(imported_credential_tombstone()),
                );
            }
            set_supported_import_value(object, &target_has_column, "key_encrypted", Value::Null);
            set_supported_import_value(
                object,
                &target_has_column,
                "status",
                Value::String("disabled".to_string()),
            );
            set_supported_import_value(object, &target_has_column, "is_active", Value::Bool(false));
            set_supported_import_value(object, &target_has_column, "is_locked", Value::Bool(true));
        }
        "management_tokens" => {
            if object.contains_key("token_hash") {
                set_supported_import_value(
                    object,
                    &target_has_column,
                    "token_hash",
                    Value::String(imported_credential_tombstone()),
                );
            }
            set_supported_import_value(object, &target_has_column, "is_active", Value::Bool(false));
        }
        "user_sessions" => {
            if object.contains_key("refresh_token_hash") {
                set_supported_import_value(
                    object,
                    &target_has_column,
                    "refresh_token_hash",
                    Value::String(imported_credential_tombstone()),
                );
            }
            set_supported_import_value(
                object,
                &target_has_column,
                "prev_refresh_token_hash",
                Value::Null,
            );
            set_supported_import_value(
                object,
                &target_has_column,
                "revoked_at",
                Value::from(chrono::Utc::now().timestamp()),
            );
            set_supported_import_value(
                object,
                &target_has_column,
                "revoke_reason",
                Value::String(IMPORTED_CREDENTIAL_REVOKE_REASON.to_string()),
            );
        }
        "proxy_nodes" => {
            set_supported_import_value(
                object,
                &target_has_column,
                "tunnel_generation",
                Value::String(uuid::Uuid::new_v4().to_string()),
            );
            set_supported_import_value(
                object,
                &target_has_column,
                "tunnel_connected",
                Value::Bool(false),
            );
            set_supported_import_value(
                object,
                &target_has_column,
                "status",
                Value::String("offline".to_string()),
            );
            set_supported_import_value(
                object,
                &target_has_column,
                "active_connections",
                Value::from(0),
            );
        }
        _ => {}
    }
}

const USAGE_REQUEST_BODY_DETAIL_COLUMNS: &[&str] = &[
    "request_body",
    "response_body",
    "provider_request_body",
    "client_response_body",
    "request_body_compressed",
    "response_body_compressed",
    "provider_request_body_compressed",
    "client_response_body_compressed",
];

const USAGE_HTTP_BODY_DETAIL_COLUMNS: &[&str] = &[
    "request_body_ref",
    "provider_request_body_ref",
    "response_body_ref",
    "client_response_body_ref",
    "request_body_state",
    "provider_request_body_state",
    "response_body_state",
    "client_response_body_state",
    "body_capture_mode",
];

#[cfg(feature = "postgres")]
fn import_column_stores_timestamp(column_name: &str) -> bool {
    column_name.ends_with("_at")
        || column_name.ends_with("_unix_secs")
        || column_name.ends_with("_unix_ms")
        || column_name.ends_with("_date")
        || matches!(
            column_name,
            "start_time" | "end_time" | "window_start" | "window_end" | "hour_utc" | "date"
        )
}

#[cfg(feature = "postgres")]
fn import_timestamp_uses_millis(table_name: &str, column_name: &str) -> bool {
    if !column_name.ends_with("_unix_ms") {
        return false;
    }

    // This legacy field is named `_unix_ms`, but every repository and API path
    // has always stored and consumed it as Unix seconds.
    let relation_name = table_name
        .rsplit('.')
        .next()
        .unwrap_or(table_name)
        .trim_matches(['"', '`']);
    !(relation_name == "usage" && column_name == "created_at_unix_ms")
}

#[cfg(feature = "postgres")]
fn normalize_imported_integer_timestamp(
    driver_name: &str,
    table_name: &str,
    column_name: &str,
    value: &Value,
) -> Result<Option<i64>, DataLayerError> {
    let invalid = || {
        DataLayerError::InvalidInput(format!(
            "{driver_name} import timestamp column '{column_name}' must contain an integer or supported datetime"
        ))
    };

    let timestamp = match value {
        Value::Null => return Ok(None),
        Value::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .ok_or_else(invalid)?,
        Value::String(value) => {
            if let Ok(timestamp) = value.trim().parse::<i64>() {
                timestamp
            } else {
                let datetime = parse_imported_datetime(value).ok_or_else(invalid)?;
                if import_timestamp_uses_millis(table_name, column_name) {
                    datetime.timestamp_millis()
                } else {
                    datetime.timestamp()
                }
            }
        }
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => return Err(invalid()),
    };
    Ok(Some(timestamp))
}

#[cfg(feature = "postgres")]
fn parse_imported_datetime(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let value = value.trim();
    if let Ok(datetime) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(datetime.with_timezone(&chrono::Utc));
    }
    if let Ok(datetime) = chrono::DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%:z") {
        return Some(datetime.with_timezone(&chrono::Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(datetime) = chrono::NaiveDateTime::parse_from_str(value, format) {
            return Some(datetime.and_utc());
        }
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|datetime| datetime.and_utc())
}

#[cfg(feature = "postgres")]
fn normalize_imported_binary(
    driver_name: &str,
    column_name: &str,
    value: &Value,
) -> Result<Option<Vec<u8>>, DataLayerError> {
    let invalid = |detail: &str| {
        DataLayerError::InvalidInput(format!(
            "{driver_name} import binary column '{column_name}' {detail}"
        ))
    };
    match value {
        Value::Null => Ok(None),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(|| invalid("contains a non-byte array value"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Value::String(value) => {
            let encoded = value
                .trim()
                .strip_prefix("\\x")
                .ok_or_else(|| invalid("must use PostgreSQL \\x hex encoding"))?;
            if !encoded.len().is_multiple_of(2) {
                return Err(invalid("contains odd-length hex data"));
            }
            let mut bytes = Vec::with_capacity(encoded.len() / 2);
            for index in (0..encoded.len()).step_by(2) {
                let byte = u8::from_str_radix(&encoded[index..index + 2], 16).map_err(|err| {
                    invalid(&format!(
                        "contains invalid hex data at byte {}: {err}",
                        index / 2
                    ))
                })?;
                bytes.push(byte);
            }
            Ok(Some(bytes))
        }
        Value::Bool(_) | Value::Number(_) | Value::Object(_) => {
            Err(invalid("must contain a byte array or PostgreSQL hex value"))
        }
    }
}

#[cfg(feature = "postgres")]
fn postgres_bytea_json_value(column_name: &str, value: &Value) -> Result<Value, DataLayerError> {
    let Some(bytes) = normalize_imported_binary("postgres", column_name, value)? else {
        return Ok(Value::Null);
    };
    let mut encoded = String::with_capacity(2 + bytes.len() * 2);
    encoded.push_str("\\x");
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}")
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
    }
    Ok(Value::String(encoded))
}

pub fn encode_jsonl(records: &[DataExportRecord]) -> Result<String, DataLayerError> {
    validate_export_records(records)?;

    let mut output = String::new();
    for record in records {
        let line = serde_json::to_string(record)
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        if line.len() > MAX_JSONL_LINE_BYTES {
            return Err(DataLayerError::InvalidInput(format!(
                "export JSONL record exceeds the {} byte line limit",
                MAX_JSONL_LINE_BYTES
            )));
        }
        let output_len = output
            .len()
            .checked_add(line.len())
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| {
                DataLayerError::InvalidInput(
                    "export JSONL exceeds the input size limit".to_string(),
                )
            })?;
        if output_len > MAX_JSONL_INPUT_BYTES {
            return Err(DataLayerError::InvalidInput(format!(
                "export JSONL exceeds the {} byte input limit",
                MAX_JSONL_INPUT_BYTES
            )));
        }
        output.push_str(&line);
        output.push('\n');
    }
    Ok(output)
}

pub fn decode_jsonl(input: &str) -> Result<Vec<DataExportRecord>, DataLayerError> {
    decode_jsonl_with_limits(
        input,
        MAX_JSONL_INPUT_BYTES,
        MAX_JSONL_LINE_BYTES,
        MAX_JSONL_RECORDS,
    )
}

fn decode_jsonl_with_limits(
    input: &str,
    max_input_bytes: usize,
    max_line_bytes: usize,
    max_records: usize,
) -> Result<Vec<DataExportRecord>, DataLayerError> {
    if input.len() > max_input_bytes {
        return Err(DataLayerError::InvalidInput(format!(
            "export JSONL exceeds the {max_input_bytes} byte input limit"
        )));
    }

    let mut records = Vec::new();
    for (line_index, line) in input.lines().enumerate() {
        if line.len() > max_line_bytes {
            return Err(DataLayerError::InvalidInput(format!(
                "export JSONL record on line {} exceeds the {max_line_bytes} byte line limit",
                line_index + 1,
            )));
        }
        if line.trim().is_empty() {
            continue;
        }
        if records.len() >= max_records {
            return Err(DataLayerError::InvalidInput(format!(
                "export JSONL exceeds the {max_records} record limit"
            )));
        }
        let record = serde_json::from_str::<DataExportRecord>(line).map_err(|err| {
            DataLayerError::InvalidInput(format!(
                "invalid export JSONL record on line {}: {err}",
                line_index + 1
            ))
        })?;
        records.push(record);
    }
    validate_export_records(&records)?;
    Ok(records)
}

pub fn build_import_plan(input: &str) -> Result<DataImportPlan, DataLayerError> {
    let records = decode_jsonl(input)?;
    let manifest = match records.first() {
        Some(DataExportRecord::Manifest { manifest }) => manifest.clone(),
        _ => unreachable!("decode_jsonl validates the manifest record"),
    };
    let mut rows_by_domain = BTreeMap::<ExportDomain, Vec<ExportRow>>::new();
    for record in records.into_iter().skip(1) {
        let DataExportRecord::Row {
            domain,
            id,
            payload,
        } = record
        else {
            return Err(DataLayerError::InvalidInput(
                "export manifest must appear only as the first record".to_string(),
            ));
        };
        rows_by_domain
            .entry(domain)
            .or_default()
            .push(ExportRow { id, payload });
    }
    Ok(DataImportPlan {
        manifest,
        rows_by_domain,
    })
}

pub fn validate_export_records(records: &[DataExportRecord]) -> Result<(), DataLayerError> {
    if records.len() > MAX_JSONL_RECORDS {
        return Err(DataLayerError::InvalidInput(format!(
            "export JSONL exceeds the {} record limit",
            MAX_JSONL_RECORDS
        )));
    }
    let Some(DataExportRecord::Manifest { manifest }) = records.first() else {
        return Err(DataLayerError::InvalidInput(
            "export JSONL must start with a manifest record".to_string(),
        ));
    };
    if !(MIN_SUPPORTED_EXPORT_FORMAT_VERSION..=EXPORT_FORMAT_VERSION)
        .contains(&manifest.format_version)
    {
        return Err(DataLayerError::InvalidInput(format!(
            "unsupported export format version {}; supported versions are {} through {}",
            manifest.format_version, MIN_SUPPORTED_EXPORT_FORMAT_VERSION, EXPORT_FORMAT_VERSION
        )));
    }

    let allowed_domains = manifest.domains.iter().copied().collect::<BTreeSet<_>>();
    let mut seen_ids = BTreeSet::<(ExportDomain, String)>::new();
    for (index, record) in records.iter().enumerate().skip(1) {
        match record {
            DataExportRecord::Manifest { .. } => {
                return Err(DataLayerError::InvalidInput(format!(
                    "export manifest appears more than once at record {}",
                    index + 1
                )));
            }
            DataExportRecord::Row {
                domain,
                id,
                payload: _,
            } => {
                if !allowed_domains.contains(domain) {
                    return Err(DataLayerError::InvalidInput(format!(
                        "record {} uses domain '{}' not declared in manifest",
                        index + 1,
                        domain.as_str()
                    )));
                }
                if id.trim().is_empty() {
                    return Err(DataLayerError::InvalidInput(format!(
                        "record {} has an empty id",
                        index + 1
                    )));
                }
                let key = (*domain, id.clone());
                if !seen_ids.insert(key) {
                    return Err(DataLayerError::InvalidInput(format!(
                        "duplicate '{}' export id '{}' at record {}",
                        domain.as_str(),
                        id,
                        index + 1
                    )));
                }
            }
        }
    }
    Ok(())
}

pub fn postgres_core_export_domains() -> Vec<ExportDomain> {
    vec![
        ExportDomain::Users,
        ExportDomain::ApiKeys,
        ExportDomain::Providers,
        ExportDomain::ProviderKeys,
        ExportDomain::Endpoints,
        ExportDomain::GlobalModels,
        ExportDomain::Models,
        ExportDomain::AuthModules,
        ExportDomain::OAuthProviders,
        ExportDomain::UserOAuthLinks,
        ExportDomain::UserGroups,
        ExportDomain::UserGroupMembers,
        ExportDomain::ProxyNodes,
        ExportDomain::SystemConfigs,
        ExportDomain::Wallets,
        ExportDomain::Usage,
        ExportDomain::Billing,
        ExportDomain::Auxiliary,
    ]
}

pub async fn export_database_jsonl(
    database: SqlDatabaseConfig,
    domains: Vec<ExportDomain>,
    created_at_unix_secs: u64,
) -> Result<String, DataLayerError> {
    match database.driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => {
            let pool =
                crate::driver::postgres::PostgresPoolFactory::new(database.to_postgres_config()?)?
                    .connect_lazy()?;
            if domains.is_empty() {
                export_postgres_core_jsonl(&pool, created_at_unix_secs).await
            } else {
                export_postgres_jsonl(&pool, domains, created_at_unix_secs).await
            }
        }
        #[cfg(not(feature = "postgres"))]
        DatabaseDriver::Postgres => Err(DataLayerError::InvalidInput(
            "PostgreSQL driver is not enabled for aether-data".to_string(),
        )),
    }
}

pub async fn import_database_jsonl(
    database: SqlDatabaseConfig,
    input: &str,
) -> Result<usize, DataLayerError> {
    import_database_jsonl_with_options(database, input, DataImportOptions::default()).await
}

pub async fn import_database_jsonl_with_options(
    database: SqlDatabaseConfig,
    input: &str,
    options: DataImportOptions,
) -> Result<usize, DataLayerError> {
    match database.driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => {
            let pool =
                crate::driver::postgres::PostgresPoolFactory::new(database.to_postgres_config()?)?
                    .connect_lazy()?;
            postgres::import_postgres_jsonl_with_options(&pool, input, options).await
        }
        #[cfg(not(feature = "postgres"))]
        DatabaseDriver::Postgres => Err(DataLayerError::InvalidInput(
            "PostgreSQL driver is not enabled for aether-data".to_string(),
        )),
    }
}

pub async fn copy_database_records(
    source: SqlDatabaseConfig,
    target: SqlDatabaseConfig,
    domains: Vec<ExportDomain>,
    created_at_unix_secs: u64,
    options: DataCopyOptions,
) -> Result<usize, DataLayerError> {
    let mut records =
        decode_jsonl(&export_database_jsonl(source, domains, created_at_unix_secs).await?)?;
    if options.omit_request_body_details {
        omit_request_body_details_from_records(&mut records);
    }
    import_database_jsonl_with_options(
        target,
        &encode_jsonl(&records)?,
        DataImportOptions {
            preserve_credentials: options.preserve_credentials,
        },
    )
    .await
}

fn omit_request_body_details_from_records(records: &mut Vec<DataExportRecord>) {
    records.retain_mut(|record| {
        let DataExportRecord::Row {
            domain, payload, ..
        } = record
        else {
            return true;
        };
        let Some(object) = payload.as_object_mut() else {
            return true;
        };
        match *domain {
            ExportDomain::Usage => {
                for column_name in USAGE_REQUEST_BODY_DETAIL_COLUMNS {
                    object.remove(*column_name);
                }
            }
            ExportDomain::Auxiliary
                if object.get("__table").and_then(Value::as_str) == Some("usage_body_blobs") =>
            {
                return false;
            }
            ExportDomain::Auxiliary
                if object.get("__table").and_then(Value::as_str) == Some("usage_http_audits") =>
            {
                for column_name in USAGE_HTTP_BODY_DETAIL_COLUMNS {
                    object.remove(*column_name);
                }
            }
            _ => {}
        }
        true
    });
}

#[cfg(feature = "postgres")]
fn is_postgres_bytea_column(column: &PostgresImportColumn) -> bool {
    column.data_type == "bytea" || column.udt_name == "bytea"
}

fn export_order_by(domain: ExportDomain, id_column: &str) -> String {
    if domain == ExportDomain::UserGroupMembers {
        "group_id ASC, user_id ASC".to_string()
    } else {
        format!("{id_column} ASC")
    }
}

#[cfg(feature = "postgres")]
pub(super) fn postgres_quote_identifier(identifier: &str) -> Result<String, DataLayerError> {
    if identifier.trim().is_empty() {
        return Err(DataLayerError::InvalidInput(
            "postgres import column name cannot be empty".to_string(),
        ));
    }
    if !identifier
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(DataLayerError::InvalidInput(format!(
            "postgres import column name '{identifier}' contains unsupported characters"
        )));
    }
    Ok(format!(r#""{identifier}""#))
}

fn payload_with_table(payload: Value, table_name: &str) -> Result<Value, DataLayerError> {
    let mut object = payload.as_object().cloned().ok_or_else(|| {
        DataLayerError::UnexpectedValue("export row payload must be a JSON object".to_string())
    })?;
    normalize_billing_payload(table_name, &mut object)?;
    sanitize_request_candidate_auxiliary_payload(table_name, &mut object);
    sanitize_payment_security_payload(table_name, &mut object);
    object.insert("__table".to_string(), Value::String(table_name.to_string()));
    Ok(Value::Object(object))
}

fn sanitize_request_candidate_auxiliary_payload(
    table_name: &str,
    object: &mut serde_json::Map<String, Value>,
) {
    if table_name != "request_candidates" {
        return;
    }

    object.insert("error_message".to_string(), Value::Null);
    sanitize_request_candidate_auxiliary_string(
        object,
        "skip_reason",
        sanitize_request_candidate_skip_reason,
    );
    sanitize_request_candidate_auxiliary_string(
        object,
        "error_type",
        sanitize_request_candidate_error_type,
    );
    sanitize_request_candidate_auxiliary_json(
        object,
        "extra_data",
        sanitize_request_candidate_extra_data,
    );
    sanitize_request_candidate_auxiliary_json(
        object,
        "required_capabilities",
        sanitize_request_candidate_required_capabilities,
    );
}

fn sanitize_payment_security_payload(
    table_name: &str,
    object: &mut serde_json::Map<String, Value>,
) {
    match table_name {
        "payment_orders" => {
            object.insert("gateway_response".to_string(), Value::Null);
        }
        "payment_callbacks" => {
            object.insert("payload".to_string(), Value::Null);
        }
        _ => {}
    }
}

fn sanitize_request_candidate_auxiliary_string(
    object: &mut serde_json::Map<String, Value>,
    field: &str,
    sanitize: fn(Option<String>) -> Option<String>,
) {
    let value = object
        .remove(field)
        .and_then(|value| value.as_str().map(ToOwned::to_owned));
    object.insert(
        field.to_string(),
        sanitize(value).map_or(Value::Null, Value::String),
    );
}

fn sanitize_request_candidate_auxiliary_json(
    object: &mut serde_json::Map<String, Value>,
    field: &str,
    sanitize: fn(Option<Value>) -> Option<Value>,
) {
    let value = object.remove(field).and_then(|value| match value {
        Value::Null => None,
        Value::String(raw) => serde_json::from_str::<Value>(&raw).ok(),
        value => Some(value),
    });
    object.insert(field.to_string(), sanitize(value).unwrap_or(Value::Null));
}

fn normalize_billing_payload(
    table_name: &str,
    object: &mut serde_json::Map<String, Value>,
) -> Result<(), DataLayerError> {
    if table_name != "billing_rules" {
        return Ok(());
    }
    for field_name in ["variables", "dimension_mappings"] {
        let Some(Value::String(raw)) = object.get(field_name) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        let parsed = serde_json::from_str::<Value>(raw).map_err(|err| {
            DataLayerError::UnexpectedValue(format!(
                "billing_rules.{field_name} contains invalid JSON: {err}"
            ))
        })?;
        object.insert(field_name.to_string(), parsed);
    }
    Ok(())
}

fn billing_payload_table(row: &ExportRow) -> Result<(String, Value), DataLayerError> {
    domain_payload_table(row, "billing", None)
}

fn domain_payload_table(
    row: &ExportRow,
    domain_label: &str,
    default_table: Option<&str>,
) -> Result<(String, Value), DataLayerError> {
    let mut object = row.payload.as_object().cloned().ok_or_else(|| {
        DataLayerError::InvalidInput(format!(
            "{domain_label} export row '{}' payload must be a JSON object",
            row.id,
        ))
    })?;
    let table_name = match object.remove("__table") {
        Some(value) => value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "{domain_label} export row '{}' has non-string __table",
                row.id
            ))
        })?,
        None => default_table.map(str::to_string).ok_or_else(|| {
            DataLayerError::InvalidInput(format!(
                "{domain_label} export row '{}' is missing string __table",
                row.id
            ))
        })?,
    };
    sanitize_request_candidate_auxiliary_payload(&table_name, &mut object);
    sanitize_payment_security_payload(&table_name, &mut object);
    Ok((table_name, Value::Object(object)))
}

#[cfg(test)]
mod payment_export_security_tests {
    use serde_json::json;

    use super::{domain_payload_table, payload_with_table, ExportRow};

    #[test]
    fn wallet_exports_and_imports_drop_payment_capabilities_and_raw_callbacks() {
        let order = payload_with_table(
            json!({
                "id": "order-1",
                "gateway_response": {
                    "client_secret": "pi_1_secret_replayable",
                    "_stripe_client_secret_encrypted": "ciphertext",
                    "customer": {"email": "payer@example.com"},
                    "payment_url": "https://pay.example/checkout?token=secret",
                },
            }),
            "payment_orders",
        )
        .expect("payment order export should sanitize");
        assert!(order["gateway_response"].is_null());

        let callback = ExportRow {
            id: "payment_callbacks:callback-1".to_string(),
            payload: json!({
                "__table": "payment_callbacks",
                "id": "callback-1",
                "payload": {
                    "client_secret": "pi_1_secret_replayable",
                    "customer_email": "payer@example.com",
                },
            }),
        };
        let (table, callback) = domain_payload_table(&callback, "wallet", Some("wallets"))
            .expect("payment callback import should sanitize");
        assert_eq!(table, "payment_callbacks");
        assert!(callback["payload"].is_null());

        let encoded = format!("{order}{callback}");
        for forbidden in [
            "client_secret",
            "replayable",
            "ciphertext",
            "customer",
            "payer@example.com",
            "token=secret",
        ] {
            assert!(!encoded.contains(forbidden), "exported {forbidden}");
        }
    }
}

#[cfg(test)]
mod request_candidate_export_security_tests {
    use serde_json::json;

    use super::{domain_payload_table, payload_with_table, ExportRow};

    #[test]
    fn request_candidate_auxiliary_export_and_import_drop_sensitive_diagnostics() {
        let raw = json!({
            "id": "candidate-1",
            "error_message": "Bearer export-secret",
            "skip_reason": "secret skip reason",
            "error_type": "secret error type",
            "extra_data": "{\"upstream_url\":\"https://user:pass@example.com/private/export-secret?token=secret\",\"unknown\":\"secret\",\"header_rules\":[{\"id\":\"secret-rule\",\"action\":\"set\",\"name\":\"authorization\",\"value\":\"secret\"}]}",
            "required_capabilities": "{\"cache_1h\":\"true\",\"tenant_secret\":\"secret\"}"
        });

        let exported = payload_with_table(raw, "request_candidates")
            .expect("candidate export payload should sanitize");
        assert!(exported["error_message"].is_null());
        assert_eq!(exported["skip_reason"], "unclassified_skip");
        assert_eq!(exported["error_type"], "unclassified_error");
        assert_eq!(
            exported["extra_data"]["upstream_url"],
            "https://example.com/"
        );
        assert_eq!(exported["extra_data"]["header_rules"]["count"], 1);
        assert_eq!(exported["required_capabilities"]["cache_1h"], true);
        let encoded = exported.to_string();
        for sensitive in [
            "export-secret",
            "user:pass",
            "secret-rule",
            "authorization",
            "tenant_secret",
        ] {
            assert!(!encoded.contains(sensitive));
        }

        let imported_row = ExportRow {
            id: "request_candidates:[\"candidate-1\"]".to_string(),
            payload: json!({
                "__table": "request_candidates",
                "id": "candidate-1",
                "error_message": "Bearer import-secret",
                "extra_data": {"free_text": "import-secret"},
                "required_capabilities": {"vision": 1, "secret": "import-secret"}
            }),
        };
        let (table, imported) = domain_payload_table(&imported_row, "auxiliary", None)
            .expect("candidate import payload should sanitize");
        assert_eq!(table, "request_candidates");
        assert!(imported["error_message"].is_null());
        assert!(imported["extra_data"].is_null());
        assert_eq!(imported["required_capabilities"], json!({"vision": true}));
        assert!(!imported.to_string().contains("import-secret"));
    }
}
