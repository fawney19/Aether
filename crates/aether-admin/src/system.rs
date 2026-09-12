use aether_data::repository::{
    auth_modules::{StoredLdapModuleConfig, StoredOAuthProviderModuleConfig},
    proxy_nodes::{
        ProxyNodeMetricsStep, StoredProxyFleetMetricsBucket, StoredProxyNode, StoredProxyNodeEvent,
        StoredProxyNodeMetricsBucket,
    },
    system::StoredSystemConfigEntry,
    wallet::StoredWalletSnapshot,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use axum::http;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use regex::Regex;
use semver::Version;
use serde::{de, de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use url::Url;

#[derive(Debug, Clone)]
pub struct AdminSystemSettingsUpdate {
    pub default_provider: Option<Option<String>>,
    pub default_model: Option<Option<String>>,
    pub enable_usage_tracking: Option<bool>,
    pub password_policy_level: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AdminSystemConfigUpdate {
    pub normalized_key: String,
    pub value: serde_json::Value,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AdminEmailTemplateUpdate {
    pub subject: Option<String>,
    pub html: Option<String>,
}

/// Email templates are administrator-configured, but they are rendered on
/// authentication and notification paths. Keep their resource and protocol
/// surface bounded before values reach storage or an SMTP header/body.
pub const ADMIN_EMAIL_TEMPLATE_MAX_SUBJECT_BYTES: usize = 512;
pub const ADMIN_EMAIL_TEMPLATE_MAX_HTML_BYTES: usize = 256 * 1024;
pub const ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_BYTES: usize = 512 * 1024;
pub const ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_VALUE_BYTES: usize = 64 * 1024;

pub fn admin_email_template_subject_is_valid(value: &str) -> bool {
    value.len() <= ADMIN_EMAIL_TEMPLATE_MAX_SUBJECT_BYTES
        && !value.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
}

pub fn admin_email_template_html_is_valid(value: &str) -> bool {
    value.len() <= ADMIN_EMAIL_TEMPLATE_MAX_HTML_BYTES
        && !value.bytes().any(|byte| {
            byte == 0 || byte == 0x7f || (byte < 0x20 && !matches!(byte, b'\r' | b'\n' | b'\t'))
        })
}

pub const ADMIN_SYSTEM_CONFIG_EXPORT_VERSION: &str = "2.3";
pub const ADMIN_SYSTEM_CONFIG_SUPPORTED_VERSIONS: &[&str] =
    &["2.0", "2.1", "2.2", ADMIN_SYSTEM_CONFIG_EXPORT_VERSION];
pub const ADMIN_SYSTEM_USERS_EXPORT_VERSION: &str = "1.7";
pub const ADMIN_SYSTEM_USERS_SUPPORTED_VERSIONS: &[&str] = &[
    "1.3",
    "1.4",
    "1.5",
    "1.6",
    ADMIN_SYSTEM_USERS_EXPORT_VERSION,
];
pub const ADMIN_SYSTEM_PROVIDER_OPS_SENSITIVE_CREDENTIAL_FIELDS: &[&str] = &[
    "api_key",
    "password",
    "refresh_token",
    "session_token",
    "session_cookie",
    "token_cookie",
    "auth_cookie",
    "cookie_string",
    "cookie",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminSystemUpdateRelease {
    pub version: String,
    pub release_url: Option<String>,
    pub release_notes: Option<String>,
    pub published_at: Option<String>,
    pub tarball_url: Option<String>,
    pub sha256sums_url: Option<String>,
}

fn default_true() -> bool {
    true
}

fn chat_pii_redaction_default_rules() -> serde_json::Value {
    json!([
        {
            "id": "email",
            "name": "邮箱",
            "pattern": "(?i)[A-Z0-9._%+-]{1,64}@[A-Z0-9.-]{1,253}\\.[A-Z]{2,63}",
            "enabled": true,
            "features": {"validator": "email"},
            "system": true
        },
        {
            "id": "cn_phone",
            "name": "手机号",
            "pattern": "(?:\\+?86[- ]?)?(?:1[3-9]\\d[- ]?\\d{4}[- ]?\\d{4}|0\\d{2,3}[- ]\\d{7,8}(?:-\\d{1,6})?)",
            "enabled": true,
            "features": {"validator": "cn_phone"},
            "system": true
        },
        {
            "id": "global_phone",
            "name": "国际号码",
            "pattern": "\\+[1-9]\\d(?:[ -]?\\d){6,13}\\d",
            "enabled": true,
            "features": {"validator": "global_phone"},
            "system": true
        },
        {
            "id": "cn_id",
            "name": "身份证号",
            "pattern": "(?i)\\b\\d{17}[\\dX]\\b",
            "enabled": true,
            "features": {"validator": "cn_id"},
            "system": true
        },
        {
            "id": "payment_card",
            "name": "银行卡号",
            "pattern": "\\b(?:\\d[ -]?){12,18}\\d\\b",
            "enabled": true,
            "features": {"validator": "payment_card"},
            "system": true
        },
        {
            "id": "ipv4",
            "name": "IPv4",
            "pattern": "\\b(?:\\d{1,3}\\.){3}\\d{1,3}\\b",
            "enabled": true,
            "features": {"validator": "ipv4"},
            "system": true
        },
        {
            "id": "ipv6",
            "name": "IPv6",
            "pattern": "\\b(?:[0-9A-Fa-f]{1,4}:){2,7}[0-9A-Fa-f:.]{1,39}\\b",
            "enabled": true,
            "features": {"validator": "ipv6"},
            "system": true
        },
        {
            "id": "api_key",
            "name": "API Key",
            "pattern": "\\b(?:sk-(?:proj-)?[A-Za-z0-9_-]{20,}|sk-ant-[A-Za-z0-9_-]{20,}|(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})|xox[baprs]-[A-Za-z0-9-]{20,}|(?:AKIA|ASIA)[0-9A-Z]{16}|[A-Za-z0-9_-]{32,})\\b",
            "enabled": true,
            "features": {"validator": "api_key"},
            "system": true
        },
        {
            "id": "access_token",
            "name": "Access Token",
            "pattern": "(?i)\\baccess[_-]?token\\s*[:=]\\s*[\"']?[A-Za-z0-9._~+/=-]{20,}",
            "enabled": true,
            "features": {"validator": "access_token"},
            "system": true
        },
        {
            "id": "secret_key",
            "name": "Secret Key",
            "pattern": "(?i)\\bsecret[_-]?key\\s*[:=]\\s*[\"']?[A-Za-z0-9._~+/=-]{20,}",
            "enabled": true,
            "features": {"validator": "secret_key"},
            "system": true
        },
        {
            "id": "bearer_token",
            "name": "Bearer Token",
            "pattern": "(?i)\\bBearer\\s+[A-Za-z0-9._~+/=-]{20,}",
            "enabled": true,
            "features": {"validator": "bearer_token"},
            "system": true
        },
        {
            "id": "jwt",
            "name": "JWT",
            "pattern": "\\b[A-Za-z0-9_-]{10,}\\.[A-Za-z0-9_-]{10,}\\.[A-Za-z0-9_-]{10,}\\b",
            "enabled": true,
            "features": {"validator": "jwt"},
            "system": true
        },
    ])
}

fn notification_service_default_items() -> serde_json::Value {
    json!([
        {
            "key": "provider_quota_alert",
            "name": "号池额度不足",
            "enabled": true,
            "channel": "global",
            "title_template": "",
            "markdown_template": "",
            "text_template": "",
            "user_email_enabled": false,
            "system": true
        },
        {
            "key": "provider_pool_abnormal",
            "name": "号池异常",
            "enabled": true,
            "channel": "global",
            "title_template": "号池异常：{provider_name}",
            "markdown_template": "号池 `{provider_name}` 出现异常，请检查服务状态。",
            "text_template": "号池 {provider_name} 出现异常，请检查服务状态。",
            "user_email_enabled": false,
            "system": true
        },
        {
            "key": "user_balance_low",
            "name": "用户余额不足",
            "enabled": true,
            "channel": "email",
            "title_template": "余额不足提醒",
            "markdown_template": "你的账户余额已低于提醒阈值，请及时处理。",
            "text_template": "你的账户余额已低于提醒阈值，请及时处理。",
            "user_email_enabled": true,
            "system": true
        }
    ])
}

fn normalize_chat_pii_redaction_placeholder_prefix(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || value.len() > 32 {
        return None;
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(value.to_ascii_uppercase())
}

fn invalid_request(detail: impl Into<String>) -> (http::StatusCode, serde_json::Value) {
    (
        http::StatusCode::BAD_REQUEST,
        json!({ "detail": detail.into() }),
    )
}

fn parse_finite_f64_import_value<E>(raw: &str) -> Result<f64, E>
where
    E: de::Error,
{
    let value = raw
        .trim()
        .parse::<f64>()
        .map_err(|_| E::custom("expected a finite number or numeric string"))?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(E::custom("expected a finite number or numeric string"))
    }
}

fn deserialize_optional_f64_from_number<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| de::Error::custom("expected a finite number or numeric string")),
        Some(Value::String(raw)) if !raw.trim().is_empty() => {
            parse_finite_f64_import_value::<D::Error>(&raw).map(Some)
        }
        Some(_) => Err(de::Error::custom(
            "expected a finite number or numeric string",
        )),
    }
}

fn deserialize_optional_u64_from_number<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .map(Some)
            .ok_or_else(|| de::Error::custom("expected a non-negative integer or numeric string")),
        Some(Value::String(raw)) if !raw.trim().is_empty() => raw
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| de::Error::custom("expected a non-negative integer or numeric string")),
        Some(_) => Err(de::Error::custom(
            "expected a non-negative integer or numeric string",
        )),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminImportMergeMode {
    #[default]
    Skip,
    Overwrite,
    Error,
}

impl AdminImportMergeMode {
    fn parse_json_value(
        value: Option<&serde_json::Value>,
    ) -> Result<Self, (http::StatusCode, serde_json::Value)> {
        match value
            .and_then(serde_json::Value::as_str)
            .unwrap_or("skip")
            .trim()
        {
            "" | "skip" => Ok(Self::Skip),
            "overwrite" => Ok(Self::Overwrite),
            "error" => Ok(Self::Error),
            _ => Err(invalid_request(
                "merge_mode 仅支持 skip / overwrite / error",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for AdminImportMergeMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Option::<serde_json::Value>::deserialize(deserializer)?;
        match value {
            None | Some(serde_json::Value::Null) => Ok(Self::Skip),
            Some(serde_json::Value::String(raw)) => match raw.trim() {
                "" | "skip" => Ok(Self::Skip),
                "overwrite" => Ok(Self::Overwrite),
                "error" => Ok(Self::Error),
                _ => Err(de::Error::custom(
                    "merge_mode 仅支持 skip / overwrite / error",
                )),
            },
            Some(_) => Err(de::Error::custom(
                "merge_mode 仅支持 skip / overwrite / error",
            )),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AdminSystemConfigImportCounter {
    pub created: u64,
    pub updated: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AdminSystemConfigImportStats {
    pub global_models: AdminSystemConfigImportCounter,
    pub proxy_nodes: AdminSystemConfigImportCounter,
    pub providers: AdminSystemConfigImportCounter,
    pub endpoints: AdminSystemConfigImportCounter,
    pub keys: AdminSystemConfigImportCounter,
    pub models: AdminSystemConfigImportCounter,
    pub ldap: AdminSystemConfigImportCounter,
    pub oauth: AdminSystemConfigImportCounter,
    pub system_configs: AdminSystemConfigImportCounter,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigGlobalModel {
    pub name: String,
    pub display_name: String,
    #[serde(default, deserialize_with = "deserialize_optional_u64_from_number")]
    pub usage_count: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64_from_number")]
    pub default_price_per_request: Option<f64>,
    #[serde(default)]
    pub default_tiered_pricing: Option<Value>,
    #[serde(default)]
    pub supported_capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default = "default_true")]
    pub is_active: bool,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigEndpoint {
    pub api_format: String,
    pub base_url: String,
    #[serde(default)]
    pub header_rules: Option<Value>,
    #[serde(default)]
    pub body_rules: Option<Value>,
    #[serde(default)]
    pub max_retries: Option<i32>,
    #[serde(default = "default_true")]
    pub is_active: bool,
    #[serde(default)]
    pub custom_path: Option<String>,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub format_acceptance_config: Option<Value>,
    #[serde(default)]
    pub proxy: Option<Value>,
}

impl std::fmt::Debug for AdminSystemConfigEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSystemConfigEndpoint")
            .field("api_format", &self.api_format)
            .field("base_url", &self.base_url)
            .field(
                "header_rules",
                &self.header_rules.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "body_rules",
                &self.body_rules.as_ref().map(|_| "[REDACTED]"),
            )
            .field("max_retries", &self.max_retries)
            .field("is_active", &self.is_active)
            .field("custom_path", &self.custom_path)
            .field("config", &self.config.as_ref().map(|_| "[REDACTED]"))
            .field(
                "format_acceptance_config",
                &self.format_acceptance_config.as_ref().map(|_| "[REDACTED]"),
            )
            .field("proxy", &self.proxy.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigProviderKey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_config: Option<Value>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub api_formats: Option<Vec<String>>,
    #[serde(default)]
    pub supported_endpoints: Option<Vec<String>>,
    #[serde(default)]
    pub rate_multipliers: Option<Value>,
    #[serde(default)]
    pub internal_priority: Option<i32>,
    #[serde(default)]
    pub global_priority_by_format: Option<Value>,
    #[serde(default)]
    pub auth_type_by_format: Option<Value>,
    #[serde(default)]
    pub allow_auth_channel_mismatch_formats: Option<Vec<String>>,
    #[serde(default)]
    pub rpm_limit: Option<u32>,
    #[serde(default)]
    pub allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub capabilities: Option<Value>,
    #[serde(default)]
    pub cache_ttl_minutes: Option<i32>,
    #[serde(default)]
    pub max_probe_interval_minutes: Option<i32>,
    #[serde(default)]
    pub auto_fetch_models: Option<bool>,
    #[serde(default)]
    pub locked_models: Option<Vec<String>>,
    #[serde(default)]
    pub model_include_patterns: Option<Vec<String>>,
    #[serde(default)]
    pub model_exclude_patterns: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub is_active: bool,
    #[serde(default)]
    pub proxy: Option<Value>,
    #[serde(default)]
    pub fingerprint: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_state: Option<String>,
}

impl std::fmt::Debug for AdminSystemConfigProviderKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSystemConfigProviderKey")
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("auth_type", &self.auth_type)
            .field(
                "auth_config",
                &self.auth_config.as_ref().map(|_| "[REDACTED]"),
            )
            .field("name", &self.name)
            .field("note", &self.note)
            .field("api_formats", &self.api_formats)
            .field("is_active", &self.is_active)
            .field("credential_state", &self.credential_state)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigProviderModel {
    #[serde(default)]
    pub global_model_name: Option<String>,
    pub provider_model_name: String,
    #[serde(default)]
    pub provider_model_mappings: Option<Value>,
    #[serde(default, deserialize_with = "deserialize_optional_f64_from_number")]
    pub price_per_request: Option<f64>,
    #[serde(default)]
    pub tiered_pricing: Option<Value>,
    #[serde(default)]
    pub supports_vision: Option<bool>,
    #[serde(default)]
    pub supports_function_calling: Option<bool>,
    #[serde(default)]
    pub supports_streaming: Option<bool>,
    #[serde(default)]
    pub supports_extended_thinking: Option<bool>,
    #[serde(default)]
    pub supports_image_generation: Option<bool>,
    #[serde(default = "default_true")]
    pub is_active: bool,
    #[serde(default)]
    pub config: Option<Value>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigProvider {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub provider_type: Option<String>,
    #[serde(default)]
    pub billing_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64_from_number")]
    pub monthly_quota_usd: Option<f64>,
    #[serde(default)]
    pub quota_reset_day: Option<u64>,
    #[serde(default)]
    pub provider_priority: Option<i32>,
    #[serde(default)]
    pub keep_priority_on_conversion: Option<bool>,
    #[serde(default)]
    pub enable_format_conversion: Option<bool>,
    #[serde(default = "default_true")]
    pub is_active: bool,
    #[serde(default)]
    pub concurrent_limit: Option<i32>,
    #[serde(default)]
    pub max_retries: Option<i32>,
    #[serde(default, deserialize_with = "deserialize_optional_f64_from_number")]
    pub stream_first_byte_timeout: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64_from_number")]
    pub request_timeout: Option<f64>,
    #[serde(default)]
    pub proxy: Option<Value>,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub endpoints: Vec<AdminSystemConfigEndpoint>,
    #[serde(default)]
    pub api_keys: Vec<AdminSystemConfigProviderKey>,
    #[serde(default)]
    pub models: Vec<AdminSystemConfigProviderModel>,
}

impl std::fmt::Debug for AdminSystemConfigProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSystemConfigProvider")
            .field("name", &self.name)
            .field("provider_type", &self.provider_type)
            .field("billing_type", &self.billing_type)
            .field("is_active", &self.is_active)
            .field("proxy", &self.proxy.as_ref().map(|_| "[REDACTED]"))
            .field("config", &self.config.as_ref().map(|_| "[REDACTED]"))
            .field("endpoints", &self.endpoints)
            .field("api_keys", &self.api_keys)
            .field("models", &self.models.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigProxyNode {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub port: Option<i32>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub is_manual: Option<bool>,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<String>,
    #[serde(default)]
    pub tunnel_mode: Option<bool>,
    #[serde(default)]
    pub heartbeat_interval: Option<i32>,
    #[serde(default)]
    pub remote_config: Option<Value>,
    #[serde(default)]
    pub config_version: Option<i32>,
}

impl std::fmt::Debug for AdminSystemConfigProxyNode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSystemConfigProxyNode")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("ip", &self.ip)
            .field("port", &self.port)
            .field("region", &self.region)
            .field("is_manual", &self.is_manual)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .field("proxy_username", &self.proxy_username)
            .field(
                "proxy_password",
                &self.proxy_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("tunnel_mode", &self.tunnel_mode)
            .field("heartbeat_interval", &self.heartbeat_interval)
            .field(
                "remote_config",
                &self.remote_config.as_ref().map(|_| "[REDACTED]"),
            )
            .field("config_version", &self.config_version)
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigLdap {
    pub server_url: String,
    pub bind_dn: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_password: Option<String>,
    pub base_dn: String,
    #[serde(default)]
    pub user_search_filter: Option<String>,
    #[serde(default)]
    pub username_attr: Option<String>,
    #[serde(default)]
    pub email_attr: Option<String>,
    #[serde(default)]
    pub display_name_attr: Option<String>,
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default)]
    pub is_exclusive: bool,
    #[serde(default)]
    pub use_starttls: bool,
    #[serde(default)]
    pub connect_timeout: Option<i32>,
}

impl std::fmt::Debug for AdminSystemConfigLdap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSystemConfigLdap")
            .field("server_url", &self.server_url)
            .field("bind_dn", &self.bind_dn)
            .field(
                "bind_password",
                &self.bind_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("base_dn", &self.base_dn)
            .field("is_enabled", &self.is_enabled)
            .field("is_exclusive", &self.is_exclusive)
            .field("use_starttls", &self.use_starttls)
            .field("connect_timeout", &self.connect_timeout)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigOAuthProvider {
    pub provider_type: String,
    pub display_name: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub authorization_url_override: Option<String>,
    #[serde(default)]
    pub token_url_override: Option<String>,
    #[serde(default)]
    pub userinfo_url_override: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    pub redirect_uri: String,
    pub frontend_callback_url: String,
    #[serde(default)]
    pub attribute_mapping: Option<Value>,
    #[serde(default)]
    pub extra_config: Option<Value>,
    #[serde(default)]
    pub is_enabled: bool,
}

impl std::fmt::Debug for AdminSystemConfigOAuthProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSystemConfigOAuthProvider")
            .field("provider_type", &self.provider_type)
            .field("display_name", &self.display_name)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("redirect_uri", &self.redirect_uri)
            .field("frontend_callback_url", &self.frontend_callback_url)
            .field("is_enabled", &self.is_enabled)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigEntry {
    pub key: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub description: Option<String>,
}

impl std::fmt::Debug for AdminSystemConfigEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = formatter.debug_struct("AdminSystemConfigEntry");
        debug.field("key", &self.key);
        if is_sensitive_admin_system_config_key(&self.key) {
            debug.field("value", &"[REDACTED]");
        } else {
            debug.field("value", &self.value);
        }
        debug.field("description", &self.description).finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigDocument {
    pub version: String,
    #[serde(default)]
    pub exported_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_state: Option<String>,
    #[serde(default)]
    pub global_models: Vec<AdminSystemConfigGlobalModel>,
    #[serde(default)]
    pub providers: Vec<AdminSystemConfigProvider>,
    #[serde(default)]
    pub proxy_nodes: Vec<AdminSystemConfigProxyNode>,
    #[serde(default)]
    pub ldap_config: Option<AdminSystemConfigLdap>,
    #[serde(default)]
    pub oauth_providers: Vec<AdminSystemConfigOAuthProvider>,
    #[serde(default)]
    pub system_configs: Vec<AdminSystemConfigEntry>,
}

impl std::fmt::Debug for AdminSystemConfigDocument {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminSystemConfigDocument")
            .field("version", &self.version)
            .field("exported_at", &self.exported_at)
            .field("credential_state", &self.credential_state)
            .field("global_models", &self.global_models.len())
            .field("providers", &self.providers)
            .field("proxy_nodes", &self.proxy_nodes)
            .field("ldap_config", &self.ldap_config)
            .field("oauth_providers", &self.oauth_providers)
            .field("system_configs", &self.system_configs)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigImportRequest {
    #[serde(flatten)]
    pub document: AdminSystemConfigDocument,
    #[serde(default)]
    pub merge_mode: AdminImportMergeMode,
}

#[derive(Clone)]
pub struct ParsedAdminSystemConfigImportRequest {
    pub request: AdminSystemConfigImportRequest,
    pub root: Map<String, Value>,
}

impl std::fmt::Debug for ParsedAdminSystemConfigImportRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ParsedAdminSystemConfigImportRequest")
            .field("request", &self.request)
            .field("root", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone)]
pub struct ParsedAdminSystemConfigObject<T> {
    pub raw: Map<String, Value>,
    pub value: T,
}

impl<T: std::fmt::Debug> std::fmt::Debug for ParsedAdminSystemConfigObject<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ParsedAdminSystemConfigObject")
            .field("raw", &"[REDACTED]")
            .field("value", &self.value)
            .finish()
    }
}

impl<T> ParsedAdminSystemConfigObject<T> {
    pub fn into_parts(self) -> (Map<String, Value>, T) {
        (self.raw, self.value)
    }
}

fn parse_admin_system_config_object<T: DeserializeOwned>(
    item: Value,
    field_name: &str,
) -> Result<ParsedAdminSystemConfigObject<T>, (http::StatusCode, Value)> {
    let raw = item
        .as_object()
        .cloned()
        .ok_or_else(|| invalid_request(format!("{field_name} 项必须是对象")))?;
    let value = serde_json::from_value::<T>(Value::Object(raw.clone()))
        .map_err(|_| invalid_request(format!("{field_name} 项格式无效")))?;
    Ok(ParsedAdminSystemConfigObject { raw, value })
}

pub fn parse_admin_system_config_array<T: DeserializeOwned>(
    root: &Map<String, Value>,
    field_name: &str,
) -> Result<Vec<ParsedAdminSystemConfigObject<T>>, (http::StatusCode, Value)> {
    let Some(value) = root.get(field_name) else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| invalid_request(format!("{field_name} 必须是数组")))?;
    items
        .iter()
        .cloned()
        .map(|item| parse_admin_system_config_object(item, field_name))
        .collect()
}

pub fn parse_admin_system_config_optional_object<T: DeserializeOwned>(
    root: &Map<String, Value>,
    field_name: &str,
) -> Result<Option<ParsedAdminSystemConfigObject<T>>, (http::StatusCode, Value)> {
    let Some(value) = root.get(field_name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    parse_admin_system_config_object(value.clone(), field_name).map(Some)
}

pub fn parse_admin_system_config_nested_array<T: DeserializeOwned>(
    parent: &Map<String, Value>,
    field_name: &str,
) -> Result<Vec<ParsedAdminSystemConfigObject<T>>, (http::StatusCode, Value)> {
    let Some(value) = parent.get(field_name) else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| invalid_request(format!("{field_name} 必须是数组")))?;
    items
        .iter()
        .cloned()
        .map(|item| parse_admin_system_config_object(item, field_name))
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct AdminApiFormatDefinition {
    value: &'static str,
    label: &'static str,
    default_path: &'static str,
    aliases: &'static [&'static str],
}

const REQUEST_RECORD_LEVEL_KEY: &str = "request_record_level";
const LEGACY_REQUEST_LOG_LEVEL_KEY: &str = "request_log_level";
const DEFAULT_BARK_API_BASE: &str = "https://api.day.app";
const SENSITIVE_SYSTEM_CONFIG_KEYS: &[&str] = &[
    "smtp_password",
    "turnstile_secret_key",
    "backup_s3_secret_access_key",
    "module.server_chan_push.send_key",
    "module.important_notification.server_chan_send_key",
    "module.bark_push.device_key",
];
const ADMIN_API_FORMAT_DEFINITIONS: &[AdminApiFormatDefinition] = &[
    AdminApiFormatDefinition {
        value: "openai:chat",
        label: "OpenAI Chat",
        default_path: "/v1/chat/completions",
        aliases: &[
            "openai",
            "openai_compatible",
            "deepseek",
            "grok",
            "moonshot",
            "zhipu",
            "qwen",
            "baichuan",
            "minimax",
        ],
    },
    AdminApiFormatDefinition {
        value: "openai:responses",
        label: "OpenAI Responses",
        default_path: "/v1/responses",
        aliases: &["responses"],
    },
    AdminApiFormatDefinition {
        value: "openai:responses:compact",
        label: "OpenAI Responses Compact",
        default_path: "/v1/responses/compact",
        aliases: &["responses_compact"],
    },
    AdminApiFormatDefinition {
        value: "openai:realtime",
        label: "OpenAI Realtime",
        default_path: "/v1/realtime",
        aliases: &["openai_realtime", "realtime"],
    },
    AdminApiFormatDefinition {
        value: "openai:search",
        label: "OpenAI Search",
        default_path: "/v1/alpha/search",
        aliases: &["openai_search", "search"],
    },
    AdminApiFormatDefinition {
        value: "openai:embedding",
        label: "OpenAI Embedding",
        default_path: "/v1/embeddings",
        aliases: &["openai_embedding", "embeddings"],
    },
    AdminApiFormatDefinition {
        value: "openai:rerank",
        label: "OpenAI Rerank",
        default_path: "/v1/rerank",
        aliases: &["openai_rerank", "rerank"],
    },
    AdminApiFormatDefinition {
        value: "openai:image",
        label: "OpenAI Image",
        default_path: "/v1/images/generations",
        aliases: &["openai_image", "images"],
    },
    AdminApiFormatDefinition {
        value: "openai:video",
        label: "OpenAI Video",
        default_path: "/v1/videos",
        aliases: &["openai_video", "sora"],
    },
    AdminApiFormatDefinition {
        value: "codex:live",
        label: "OpenAI Live",
        default_path: "/v1/live",
        aliases: &["codex_live", "live"],
    },
    AdminApiFormatDefinition {
        value: "claude:messages",
        label: "Claude Messages",
        default_path: "/v1/messages",
        aliases: &["claude", "claude_compatible"],
    },
    AdminApiFormatDefinition {
        value: "gemini:generate_content",
        label: "Gemini Generate Content",
        default_path: "/v1beta/models/{model}:{action}",
        aliases: &["gemini", "google", "vertex"],
    },
    AdminApiFormatDefinition {
        value: "gemini:interactions",
        label: "Gemini Interactions",
        default_path: "/v1/interactions",
        aliases: &["gemini_interactions", "interactions"],
    },
    AdminApiFormatDefinition {
        value: "gemini:embedding",
        label: "Gemini Embedding",
        default_path: "/v1beta/models/{model}:{action}",
        aliases: &["gemini_embedding"],
    },
    AdminApiFormatDefinition {
        value: "gemini:video",
        label: "Gemini Video",
        default_path: "/v1beta/models/{model}:predictLongRunning",
        aliases: &["gemini_video", "veo"],
    },
    AdminApiFormatDefinition {
        value: "jina:embedding",
        label: "Jina Embedding",
        default_path: "/v1/embeddings",
        aliases: &["jina_embedding"],
    },
    AdminApiFormatDefinition {
        value: "jina:rerank",
        label: "Jina Rerank",
        default_path: "/v1/rerank",
        aliases: &["jina_rerank"],
    },
    AdminApiFormatDefinition {
        value: "doubao:embedding",
        label: "Doubao Embedding",
        default_path: "/v1/embeddings",
        aliases: &["doubao_embedding"],
    },
    AdminApiFormatDefinition {
        value: "aliyun:multimodal_embedding",
        label: "Aliyun Multimodal Embedding",
        default_path: "/api/v1/services/embeddings/multimodal-embedding/multimodal-embedding",
        aliases: &[
            "aliyun_embedding",
            "aliyun_multimodal_embedding",
            "dashscope_embedding",
            "dashscope_multimodal_embedding",
            "dashscope:multimodal_embedding",
        ],
    },
];

pub fn build_admin_system_check_update_payload(current_version: String) -> serde_json::Value {
    build_admin_system_check_update_payload_with_release(
        current_version,
        None,
        Some("检查更新需要 Rust 管理后端".to_string()),
    )
}

pub fn build_admin_system_check_update_payload_with_release(
    current_version: String,
    latest_release: Option<AdminSystemUpdateRelease>,
    error: Option<String>,
) -> serde_json::Value {
    let has_update = latest_release
        .as_ref()
        .is_some_and(|release| admin_system_update_available(&current_version, &release.version));
    let update_blocker = latest_release
        .as_ref()
        .and_then(admin_system_update_blocker);
    let updatable = latest_release
        .as_ref()
        .is_some_and(|release| admin_system_update_blocker(release).is_none());

    json!({
        "current_version": current_version,
        "latest_version": latest_release.as_ref().map(|release| release.version.clone()),
        "has_update": has_update,
        "updatable": updatable,
        "update_blocker": update_blocker,
        "release_url": latest_release.as_ref().and_then(|release| release.release_url.clone()),
        "release_notes": latest_release.as_ref().and_then(|release| release.release_notes.clone()),
        "published_at": latest_release.as_ref().and_then(|release| release.published_at.clone()),
        "error": error,
    })
}

pub fn build_admin_system_releases_payload(
    current_version: String,
    releases: Vec<AdminSystemUpdateRelease>,
    error: Option<String>,
) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = releases
        .iter()
        .map(|release| {
            let is_current = {
                let norm_current = normalized_admin_system_version(&current_version);
                let norm_release = normalized_admin_system_version(&release.version);
                norm_current == norm_release
            };
            let is_newer = admin_system_update_available(&current_version, &release.version);
            let update_blocker = admin_system_update_blocker(release);
            json!({
                "version": release.version,
                "release_url": release.release_url,
                "release_notes": release.release_notes,
                "published_at": release.published_at,
                "tarball_url": release.tarball_url,
                "sha256sums_url": release.sha256sums_url,
                "is_current": is_current,
                "is_newer": is_newer,
                "updatable": update_blocker.is_none(),
                "update_blocker": update_blocker,
            })
        })
        .collect();

    json!({
        "current_version": current_version,
        "releases": entries,
        "error": error,
    })
}

fn admin_system_update_blocker(release: &AdminSystemUpdateRelease) -> Option<&'static str> {
    if release.tarball_url.is_none() {
        Some("当前平台暂无安装包")
    } else if release.sha256sums_url.is_none() {
        Some("缺少 SHA256SUMS 校验文件")
    } else {
        None
    }
}

fn normalized_admin_system_version(version: &str) -> String {
    let trimmed = version.trim();
    trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed)
        .to_string()
}

fn admin_system_update_available(current_version: &str, latest_release_version: &str) -> bool {
    match (
        parse_admin_system_version_for_update(current_version),
        parse_admin_system_version_for_update(latest_release_version),
    ) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

fn parse_admin_system_version_for_update(version: &str) -> Option<Version> {
    let base = admin_system_release_base_version(version);
    let normalized = normalize_admin_system_rc_prerelease(&base);
    Version::parse(&normalized).ok()
}

fn admin_system_release_base_version(version: &str) -> String {
    let normalized = normalized_admin_system_version(version);
    let without_dirty = normalized.strip_suffix("-dirty").unwrap_or(&normalized);
    git_describe_base_version(without_dirty)
        .unwrap_or(without_dirty)
        .to_string()
}

fn git_describe_base_version(version: &str) -> Option<&str> {
    let (before_hash, hash) = version.rsplit_once("-g")?;
    if hash.is_empty() || !hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }

    let (base, commit_count) = before_hash.rsplit_once('-')?;
    if commit_count.is_empty() || !commit_count.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    Some(base)
}

fn normalize_admin_system_rc_prerelease(version: &str) -> String {
    let Some((core, prerelease)) = version.split_once('-') else {
        return version.to_string();
    };
    let Some(rc_number) = prerelease.strip_prefix("rc") else {
        return version.to_string();
    };
    if rc_number.is_empty() || !rc_number.chars().all(|ch| ch.is_ascii_digit()) {
        return version.to_string();
    }

    format!("{core}-rc.{rc_number}")
}

pub fn build_admin_system_stats_payload(
    total_users: u64,
    active_users: u64,
    total_providers: u64,
    active_providers: u64,
    total_api_keys: u64,
    total_requests: u64,
    usage_counter: serde_json::Value,
) -> serde_json::Value {
    json!({
        "users": {
            "total": total_users,
            "active": active_users,
        },
        "providers": {
            "total": total_providers,
            "active": active_providers,
        },
        "api_keys": total_api_keys,
        "requests": total_requests,
        "usage_counter": usage_counter,
    })
}

pub fn build_admin_system_settings_payload(
    default_provider: Option<String>,
    default_model: Option<String>,
    enable_usage_tracking: bool,
    password_policy_level: String,
) -> serde_json::Value {
    json!({
        "default_provider": default_provider,
        "default_model": default_model,
        "enable_usage_tracking": enable_usage_tracking,
        "password_policy_level": password_policy_level,
    })
}

pub fn parse_admin_system_settings_update(
    request_body: &[u8],
) -> Result<AdminSystemSettingsUpdate, (http::StatusCode, serde_json::Value)> {
    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(payload)) => payload,
        Ok(_) | Err(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };

    let default_provider = match payload.get("default_provider") {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Some(None)
            } else {
                Some(Some(value.to_string()))
            }
        }
        Some(serde_json::Value::Null) => Some(None),
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        None => None,
    };

    let default_model = match payload.get("default_model") {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Some(None)
            } else {
                Some(Some(value.to_string()))
            }
        }
        Some(serde_json::Value::Null) => Some(None),
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        None => None,
    };

    let enable_usage_tracking = match payload.get("enable_usage_tracking") {
        Some(serde_json::Value::Bool(value)) => Some(*value),
        Some(serde_json::Value::Null) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        None => None,
    };

    let password_policy_level = match payload.get("password_policy_level") {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if matches!(value, "weak" | "medium" | "strong") {
                Some(value.to_string())
            } else {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        }
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        None => None,
    };

    Ok(AdminSystemSettingsUpdate {
        default_provider,
        default_model,
        enable_usage_tracking,
        password_policy_level,
    })
}

pub fn build_admin_system_settings_updated_payload() -> serde_json::Value {
    json!({ "message": "系统设置更新成功" })
}

pub fn build_admin_email_templates_payload(templates: Vec<serde_json::Value>) -> serde_json::Value {
    json!({ "templates": templates })
}

pub fn admin_email_template_not_found_error(
    template_type: &str,
) -> (http::StatusCode, serde_json::Value) {
    (
        http::StatusCode::NOT_FOUND,
        json!({ "detail": format!("模板类型 '{template_type}' 不存在") }),
    )
}

pub fn parse_admin_email_template_update(
    request_body: &[u8],
) -> Result<AdminEmailTemplateUpdate, (http::StatusCode, serde_json::Value)> {
    if request_body.len() > ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_BYTES {
        return Err((
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": "模板请求体超过允许大小" }),
        ));
    }
    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(payload)) => payload,
        _ => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };

    let subject = match payload.get("subject") {
        Some(serde_json::Value::String(value)) if admin_email_template_subject_is_valid(value) => {
            Some(value.clone())
        }
        Some(serde_json::Value::String(_)) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "模板 subject 超过大小限制或包含非法控制字符" }),
            ));
        }
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };
    let html = match payload.get("html") {
        Some(serde_json::Value::String(value)) if admin_email_template_html_is_valid(value) => {
            Some(value.clone())
        }
        Some(serde_json::Value::String(_)) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "模板 html 超过大小限制或包含非法控制字符" }),
            ));
        }
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };

    if subject.is_none() && html.is_none() {
        return Err((
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": "请提供 subject 或 html" }),
        ));
    }

    Ok(AdminEmailTemplateUpdate { subject, html })
}

pub fn parse_admin_email_template_preview_payload(
    request_body: Option<&[u8]>,
) -> Result<serde_json::Map<String, serde_json::Value>, (http::StatusCode, serde_json::Value)> {
    match request_body {
        Some(bytes) if bytes.len() > ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_BYTES => Err((
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": "模板预览请求体超过允许大小" }),
        )),
        Some(bytes) => match serde_json::from_slice::<serde_json::Value>(bytes) {
            Ok(serde_json::Value::Object(payload)) => {
                if let Some(serde_json::Value::String(value)) = payload.get("html") {
                    if !admin_email_template_html_is_valid(value) {
                        return Err((
                            http::StatusCode::BAD_REQUEST,
                            json!({ "detail": "模板 html 超过大小限制或包含非法控制字符" }),
                        ));
                    }
                }
                if let Some(serde_json::Value::String(value)) = payload.get("subject") {
                    if !admin_email_template_subject_is_valid(value) {
                        return Err((
                            http::StatusCode::BAD_REQUEST,
                            json!({ "detail": "模板 subject 超过大小限制或包含非法控制字符" }),
                        ));
                    }
                }
                if payload.values().any(|value| {
                    value.as_str().is_some_and(|value| {
                        value.len() > ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_VALUE_BYTES
                    })
                }) {
                    return Err((
                        http::StatusCode::BAD_REQUEST,
                        json!({ "detail": "模板预览变量超过允许大小" }),
                    ));
                }
                Ok(payload)
            }
            Ok(serde_json::Value::Null) => Ok(serde_json::Map::new()),
            _ => Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            )),
        },
        None => Ok(serde_json::Map::new()),
    }
}

pub fn build_admin_email_template_saved_payload() -> serde_json::Value {
    json!({ "message": "模板保存成功" })
}

pub fn build_admin_email_template_preview_payload(
    rendered_html: String,
    preview_variables: std::collections::BTreeMap<String, String>,
) -> serde_json::Value {
    json!({
        "html": rendered_html,
        "variables": preview_variables,
    })
}

pub fn build_admin_email_template_reset_payload(
    template_type: &str,
    name: &str,
    default_subject: &str,
    default_html: &str,
) -> serde_json::Value {
    json!({
        "message": "模板已重置为默认值",
        "template": {
            "type": template_type,
            "name": name,
            "subject": default_subject,
            "html": default_html,
        }
    })
}

pub fn build_admin_api_formats_payload() -> serde_json::Value {
    json!({
        "formats": ADMIN_API_FORMAT_DEFINITIONS
            .iter()
            .map(|definition| json!({
                "value": definition.value,
                "label": definition.label,
                "default_path": definition.default_path,
                "aliases": definition.aliases,
            }))
            .collect::<Vec<_>>(),
    })
}

pub fn admin_module_name_from_status_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/modules/status/")
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .map(ToOwned::to_owned)
}

pub fn admin_module_name_from_enabled_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/modules/status/")
        .and_then(|value| value.strip_suffix("/enabled"))
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .map(ToOwned::to_owned)
}

pub fn oauth_module_config_is_valid(providers: &[StoredOAuthProviderModuleConfig]) -> bool {
    !providers.is_empty()
        && providers.iter().all(|provider| {
            !provider.client_id.trim().is_empty()
                && provider
                    .client_secret_encrypted
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some()
                && !provider.redirect_uri.trim().is_empty()
        })
}

pub fn ldap_module_config_is_valid(config: Option<&StoredLdapModuleConfig>) -> bool {
    let Some(config) = config else {
        return false;
    };
    normalize_ldap_transport_server_url(&config.server_url, config.use_starttls).is_some()
        && ldap_module_config_fields_are_valid(config)
}

/// Validate LDAP configuration fields other than the transport endpoint.
///
/// Keeping this separate lets gateway test builds use their in-process
/// `mockldap` transport while sharing every production data-shape check.
pub fn ldap_module_config_fields_are_valid(config: &StoredLdapModuleConfig) -> bool {
    let search_filter = config
        .user_search_filter
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("(uid={username})");
    let username_attr = config
        .username_attr
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("uid");
    let email_attr = config
        .email_attr
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("mail");
    let display_name_attr = config
        .display_name_attr
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cn");

    ldap_distinguished_name_is_valid(&config.bind_dn)
        && ldap_distinguished_name_is_valid(&config.base_dn)
        && ldap_search_filter_is_valid(search_filter)
        && ldap_attribute_description_is_valid(username_attr)
        && ldap_attribute_description_is_valid(email_attr)
        && ldap_attribute_description_is_valid(display_name_attr)
        && config
            .bind_password_encrypted
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
}

/// Validate the bounded LDAP user filter shape used by the gateway login
/// implementation.  This pure helper is shared by admin status/validation
/// code so an imported malformed filter cannot be reported as active.
pub fn ldap_search_filter_is_valid(value: &str) -> bool {
    if value.chars().any(char::is_control) {
        return false;
    }
    let value = value.trim();
    if value.is_empty()
        || value.len() > 200
        || !value.contains("{username}")
        || !value.starts_with('(')
        || !value.ends_with(')')
    {
        return false;
    }

    let mut depth = 0i32;
    let mut max_depth = 0i32;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '(' => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
                // A valid LDAP filter has one outer pair. Reaching depth zero
                // before EOF would allow a second top-level expression.
                if depth == 0 && chars.peek().is_some() {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0 && max_depth <= 5
}

/// Validate a DN as bounded, protocol-safe configuration data.
///
/// DN strings are encoded as their own LDAP protocol fields, so reparsing the
/// full RFC 4514 grammar here would add compatibility risk without preventing
/// filter injection. Raw control characters and unbounded values are the
/// relevant configuration hazards at this boundary.
pub fn ldap_distinguished_name_is_valid(value: &str) -> bool {
    if value.chars().any(char::is_control) {
        return false;
    }
    let value = value.trim();
    !value.is_empty() && value.len() <= 4096
}

/// Validate an RFC 4512 AttributeDescription used both for requested LDAP
/// attributes and, for the default username lookup, as filter syntax.
pub fn ldap_attribute_description_is_valid(value: &str) -> bool {
    if value.chars().any(char::is_control) {
        return false;
    }
    let value = value.trim();
    if value.is_empty() || value.len() > 128 || !value.is_ascii() {
        return false;
    }

    let mut parts = value.split(';');
    let Some(attribute_type) = parts.next() else {
        return false;
    };
    if !ldap_attribute_type_is_valid(attribute_type) {
        return false;
    }
    parts.all(|option| {
        !option.is_empty()
            && option
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

fn ldap_attribute_type_is_valid(value: &str) -> bool {
    let mut bytes = value.bytes();
    if value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
    {
        return bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    }

    let mut components = value.split('.');
    let Some(first) = components.next() else {
        return false;
    };
    let Some(second) = components.next() else {
        return false;
    };
    ldap_oid_component_is_valid(first)
        && ldap_oid_component_is_valid(second)
        && components.all(ldap_oid_component_is_valid)
}

fn ldap_oid_component_is_valid(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value.len() == 1 || !value.starts_with('0'))
}

/// Parse and canonicalize an LDAP transport endpoint.
///
/// LDAP simple binds carry credentials, therefore plaintext `ldap://` is only
/// accepted when StartTLS is explicitly enabled.  This helper validates the
/// transport URL shape and removes URL-controlled request data (credentials,
/// query strings, fragments, and paths).  It intentionally does not apply an
/// IP allow/deny policy: LDAP servers are commonly deployed on private
/// networks, and network reachability policy belongs to the outbound
/// transport layer rather than this pure configuration validator.
pub fn normalize_ldap_transport_server_url(raw: &str, use_starttls: bool) -> Option<String> {
    normalize_ldap_transport_server_url_inner(raw, use_starttls, false)
}

/// Test-only variant used by gateway fixtures that emulate an LDAP endpoint
/// with the `mockldap://` scheme.  Production callers must use
/// [`normalize_ldap_transport_server_url`].
#[doc(hidden)]
pub fn normalize_ldap_transport_server_url_for_tests(
    raw: &str,
    use_starttls: bool,
) -> Option<String> {
    normalize_ldap_transport_server_url_inner(raw, use_starttls, true)
}

fn normalize_ldap_transport_server_url_inner(
    raw: &str,
    use_starttls: bool,
    allow_mockldap: bool,
) -> Option<String> {
    if raw.chars().any(char::is_control) {
        return None;
    }
    let raw = raw.trim();
    if raw.is_empty() || raw.contains('@') {
        return None;
    }
    let candidate = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("ldap://{raw}")
    };
    let Ok(mut parsed) = Url::parse(&candidate) else {
        return None;
    };
    let scheme = parsed.scheme().to_ascii_lowercase();
    let secure_transport = match scheme.as_str() {
        "ldaps" => true,
        "ldap" => use_starttls,
        "mockldap" => allow_mockldap,
        _ => false,
    };
    if !secure_transport
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || (parsed.path() != "" && parsed.path() != "/")
    {
        return None;
    }
    let host = parsed
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    parsed.set_host(Some(&host)).ok()?;
    let default_port = match scheme.as_str() {
        "ldaps" => Some(636),
        "ldap" => Some(389),
        "mockldap" => None,
        _ => None,
    };
    if default_port.is_some_and(|port| parsed.port() == Some(port)) {
        parsed.set_port(None).ok()?;
    }
    Some(parsed.to_string().trim_end_matches('/').to_string())
}

pub struct AdminModuleValidationInput<'a> {
    pub module_name: &'a str,
    pub oauth_providers: &'a [StoredOAuthProviderModuleConfig],
    pub ldap_config: Option<&'a StoredLdapModuleConfig>,
    pub gemini_files_has_capable_key: bool,
    pub important_notification_configured: bool,
    pub server_chan_push_configured: bool,
    pub bark_push_configured: bool,
    pub s3_backup_configured: bool,
}

pub fn build_admin_module_validation_result(
    input: AdminModuleValidationInput<'_>,
) -> (bool, Option<String>) {
    let AdminModuleValidationInput {
        module_name,
        oauth_providers,
        ldap_config,
        gemini_files_has_capable_key,
        important_notification_configured,
        server_chan_push_configured,
        bark_push_configured,
        s3_backup_configured,
    } = input;

    match module_name {
        "oauth" => {
            if oauth_providers.is_empty() {
                return (
                    false,
                    Some("请先配置并启用至少一个 OAuth Provider".to_string()),
                );
            }
            for provider in oauth_providers {
                if provider.client_id.trim().is_empty() {
                    return (
                        false,
                        Some(format!(
                            "Provider [{}] 未配置 Client ID",
                            provider.display_name
                        )),
                    );
                }
                if provider
                    .client_secret_encrypted
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
                {
                    return (
                        false,
                        Some(format!(
                            "Provider [{}] 未配置 Client Secret",
                            provider.display_name
                        )),
                    );
                }
                if provider.redirect_uri.trim().is_empty() {
                    return (
                        false,
                        Some(format!(
                            "Provider [{}] 未配置回调地址",
                            provider.display_name
                        )),
                    );
                }
            }
            (true, None)
        }
        "ldap" => {
            let Some(config) = ldap_config else {
                return (false, Some("请先配置 LDAP 连接信息".to_string()));
            };
            if normalize_ldap_transport_server_url(&config.server_url, config.use_starttls)
                .is_none()
            {
                return (false, Some("请配置 LDAP 服务器地址".to_string()));
            }
            if !ldap_distinguished_name_is_valid(&config.bind_dn) {
                return (false, Some("请配置有效的绑定 DN".to_string()));
            }
            if !ldap_distinguished_name_is_valid(&config.base_dn) {
                return (false, Some("请配置有效的搜索基准 DN".to_string()));
            }
            let search_filter = config
                .user_search_filter
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("(uid={username})");
            if !ldap_search_filter_is_valid(search_filter) {
                return (false, Some("请配置有效的 LDAP 搜索过滤器".to_string()));
            }
            for (attribute, label) in [
                (
                    config.username_attr.as_deref().unwrap_or("uid"),
                    "用户名属性",
                ),
                (config.email_attr.as_deref().unwrap_or("mail"), "邮箱属性"),
                (
                    config.display_name_attr.as_deref().unwrap_or("cn"),
                    "显示名称属性",
                ),
            ] {
                let attribute = attribute.trim();
                let attribute = if attribute.is_empty() {
                    match label {
                        "用户名属性" => "uid",
                        "邮箱属性" => "mail",
                        _ => "cn",
                    }
                } else {
                    attribute
                };
                if !ldap_attribute_description_is_valid(attribute) {
                    return (false, Some(format!("请配置有效的 LDAP {label}")));
                }
            }
            if config
                .bind_password_encrypted
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return (false, Some("请配置绑定密码".to_string()));
            }
            (true, None)
        }
        "important_notification" | "notification_email" => {
            if important_notification_configured {
                (true, None)
            } else {
                (false, Some("请先完成通知服务推送渠道配置".to_string()))
            }
        }
        "server_chan_push" => {
            if server_chan_push_configured {
                (true, None)
            } else {
                (false, Some("请先配置 Server 酱 SendKey".to_string()))
            }
        }
        "bark_push" => {
            if bark_push_configured {
                (true, None)
            } else {
                (false, Some("请先配置 Bark Device Key".to_string()))
            }
        }
        "s3_backup" => {
            if s3_backup_configured {
                (true, None)
            } else {
                (false, Some("请先完成 S3 备份配置".to_string()))
            }
        }
        "gemini_files" => {
            if gemini_files_has_capable_key {
                (true, None)
            } else {
                (
                    false,
                    Some("至少启用一个具有「Gemini 文件 API」能力的 Key".to_string()),
                )
            }
        }
        "management_tokens" | "model_directives" | "proxy_nodes" => (true, None),
        _ => (true, None),
    }
}

pub fn build_admin_module_health(
    module_name: &str,
    gemini_files_has_capable_key: bool,
) -> &'static str {
    match module_name {
        "management_tokens"
        | "model_directives"
        | "proxy_nodes"
        | "important_notification"
        | "bark_push"
        | "server_chan_push"
        | "s3_backup" => "healthy",
        "gemini_files" => {
            if gemini_files_has_capable_key {
                "healthy"
            } else {
                "degraded"
            }
        }
        _ => "unknown",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_admin_module_status_payload(
    name: &str,
    display_name: &str,
    description: &str,
    category: &str,
    admin_route: Option<&str>,
    admin_menu_icon: Option<&str>,
    admin_menu_group: Option<&str>,
    admin_menu_order: i32,
    available: bool,
    enabled: bool,
    config_validated: bool,
    config_error: Option<String>,
    health: &str,
) -> serde_json::Value {
    let active = available && enabled && config_validated;
    json!({
        "name": name,
        "available": available,
        "enabled": enabled,
        "active": active,
        "config_validated": config_validated,
        "config_error": if config_validated { serde_json::Value::Null } else { json!(config_error) },
        "display_name": display_name,
        "description": description,
        "category": category,
        "admin_route": if available { json!(admin_route) } else { serde_json::Value::Null },
        "admin_menu_icon": admin_menu_icon,
        "admin_menu_group": admin_menu_group,
        "admin_menu_order": admin_menu_order,
        "health": health,
    })
}

pub fn normalize_admin_system_export_api_formats(
    raw_formats: Option<&serde_json::Value>,
    mut signature_for: impl FnMut(&str) -> Option<String>,
) -> Vec<String> {
    let Some(raw_formats) = raw_formats.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in raw_formats {
        let Some(value) = raw
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(signature) = signature_for(value) else {
            continue;
        };
        if seen.insert(signature.clone()) {
            normalized.push(signature);
        }
    }
    normalized
}

pub fn resolve_admin_system_export_key_api_formats(
    raw_formats: Option<&serde_json::Value>,
    provider_endpoint_formats: &[String],
    signature_for: impl FnMut(&str) -> Option<String>,
) -> Vec<String> {
    let normalized = normalize_admin_system_export_api_formats(raw_formats, signature_for);
    if !normalized.is_empty() {
        return normalized;
    }
    if raw_formats.is_none() {
        return provider_endpoint_formats.to_vec();
    }
    Vec::new()
}

pub fn collect_admin_system_export_provider_endpoint_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
    mut signature_for: impl FnMut(&str) -> Option<String>,
) -> Vec<String> {
    endpoints
        .iter()
        .filter_map(|endpoint| signature_for(&endpoint.api_format))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn serialize_admin_system_users_export_wallet(
    wallet: Option<&StoredWalletSnapshot>,
) -> Option<serde_json::Value> {
    let wallet = wallet?;
    let recharge_balance = wallet.balance;
    let gift_balance = wallet.gift_balance;
    let spendable_balance = recharge_balance + gift_balance;
    let unlimited = wallet.limit_mode.eq_ignore_ascii_case("unlimited");

    Some(json!({
        "id": wallet.id.clone(),
        "balance": spendable_balance,
        "recharge_balance": recharge_balance,
        "gift_balance": gift_balance,
        "refundable_balance": recharge_balance,
        "currency": wallet.currency.clone(),
        "status": wallet.status.clone(),
        "limit_mode": wallet.limit_mode.clone(),
        "unlimited": unlimited,
        "total_recharged": wallet.total_recharged,
        "total_consumed": wallet.total_consumed,
        "total_refunded": wallet.total_refunded,
        "total_adjusted": wallet.total_adjusted,
        "updated_at": unix_secs_to_rfc3339(wallet.updated_at_unix_secs),
    }))
}

pub fn parse_admin_system_config_import_request(
    request_body: &[u8],
) -> Result<ParsedAdminSystemConfigImportRequest, (http::StatusCode, serde_json::Value)> {
    let root = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(root)) => root,
        _ => return Err(invalid_request("请求数据验证失败")),
    };

    let version = root
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_request("version 为必填字段"))?;
    if !ADMIN_SYSTEM_CONFIG_SUPPORTED_VERSIONS.contains(&version) {
        return Err(invalid_request(format!(
            "不支持的配置版本: {version}，支持的版本: {}",
            ADMIN_SYSTEM_CONFIG_SUPPORTED_VERSIONS.join(", ")
        )));
    }

    let merge_mode = AdminImportMergeMode::parse_json_value(root.get("merge_mode"))?;
    let document = serde_path_to_error::deserialize::<_, AdminSystemConfigDocument>(
        serde_json::Value::Object(root.clone()),
    )
    .map_err(|err| {
        let path = err.path().to_string();
        let inner = err.into_inner();
        let detail = if path.is_empty() {
            format!("配置文件格式无效: {inner}")
        } else {
            format!("配置文件格式无效: {path}: {inner}")
        };
        invalid_request(detail)
    })?;

    Ok(ParsedAdminSystemConfigImportRequest {
        request: AdminSystemConfigImportRequest {
            document,
            merge_mode,
        },
        root,
    })
}

pub fn normalize_admin_system_config_key(requested_key: &str) -> String {
    let trimmed = requested_key.trim();
    if trimmed.eq_ignore_ascii_case(LEGACY_REQUEST_LOG_LEVEL_KEY) {
        REQUEST_RECORD_LEVEL_KEY.to_string()
    } else if trimmed.eq_ignore_ascii_case("module.notification_email.enabled") {
        "module.important_notification.enabled".to_string()
    } else if trimmed.eq_ignore_ascii_case("module.important_notification.server_chan_enabled") {
        "module.server_chan_push.enabled".to_string()
    } else if trimmed.eq_ignore_ascii_case("module.important_notification.server_chan_send_key") {
        "module.server_chan_push.send_key".to_string()
    } else if trimmed.eq_ignore_ascii_case("module.important_notification.server_chan_template") {
        "module.server_chan_push.template".to_string()
    } else if let Some(canonical) = SENSITIVE_SYSTEM_CONFIG_KEYS
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(trimmed))
    {
        (*canonical).to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn admin_system_config_delete_keys(requested_key: &str) -> Vec<String> {
    let normalized = normalize_admin_system_config_key(requested_key);
    if normalized == REQUEST_RECORD_LEVEL_KEY {
        vec![
            REQUEST_RECORD_LEVEL_KEY.to_string(),
            LEGACY_REQUEST_LOG_LEVEL_KEY.to_string(),
        ]
    } else if normalized == "module.important_notification.enabled" {
        vec![
            "module.important_notification.enabled".to_string(),
            "module.notification_email.enabled".to_string(),
        ]
    } else if normalized == "module.server_chan_push.enabled" {
        vec![
            "module.server_chan_push.enabled".to_string(),
            "module.important_notification.server_chan_enabled".to_string(),
        ]
    } else if normalized == "module.server_chan_push.send_key" {
        vec![
            "module.server_chan_push.send_key".to_string(),
            "module.important_notification.server_chan_send_key".to_string(),
        ]
    } else if normalized == "module.server_chan_push.template" {
        vec![
            "module.server_chan_push.template".to_string(),
            "module.important_notification.server_chan_template".to_string(),
        ]
    } else {
        vec![normalized]
    }
}

pub fn is_sensitive_admin_system_config_key(key: &str) -> bool {
    SENSITIVE_SYSTEM_CONFIG_KEYS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(key))
}

pub fn admin_system_config_default_value(key: &str) -> Option<serde_json::Value> {
    match key {
        "site_name" => Some(json!("Aether")),
        "site_subtitle" => Some(json!("AI Gateway")),
        "default_user_initial_gift_usd" => Some(json!(10.0)),
        "password_policy_level" => Some(json!("weak")),
        REQUEST_RECORD_LEVEL_KEY => Some(json!("basic")),
        "max_request_body_size" => Some(json!(0)),
        "max_response_body_size" => Some(json!(0)),
        "sensitive_headers" => Some(json!([
            "authorization",
            "x-api-key",
            "api-key",
            "cookie",
            "set-cookie"
        ])),
        "detail_log_retention_days" => Some(json!(7)),
        "compressed_log_retention_days" => Some(json!(30)),
        "header_retention_days" => Some(json!(90)),
        "log_retention_days" => Some(json!(365)),
        "enable_auto_cleanup" => Some(json!(true)),
        "cleanup_batch_size" => Some(json!(1000)),
        "request_candidates_retention_days" => Some(json!(30)),
        "request_candidates_cleanup_batch_size" => Some(json!(5000)),
        "proxy_node_metrics_1m_retention_days" => Some(json!(30)),
        "proxy_node_metrics_1h_retention_days" => Some(json!(180)),
        "proxy_node_metrics_cleanup_batch_size" => Some(json!(5000)),
        "enable_provider_checkin" => Some(json!(true)),
        "provider_checkin_time" => Some(json!("01:05")),
        "auto_delete_expired_keys" => Some(json!(false)),
        "turnstile_enabled" => Some(json!(false)),
        "turnstile_site_key" => Some(serde_json::Value::Null),
        "turnstile_secret_key" => Some(serde_json::Value::Null),
        "turnstile_allowed_hostnames" => Some(json!([])),
        "backup_s3_enabled" => Some(json!(false)),
        "backup_s3_scope" => Some(json!("data")),
        "backup_s3_endpoint" => Some(serde_json::Value::Null),
        "backup_s3_region" => Some(json!("auto")),
        "backup_s3_user_agent" => Some(json!("rclone/v1.68.0")),
        "backup_s3_bucket" => Some(serde_json::Value::Null),
        "backup_s3_prefix" => Some(json!("aether/backups/")),
        "backup_s3_access_key_id" => Some(serde_json::Value::Null),
        "backup_s3_secret_access_key" => Some(serde_json::Value::Null),
        "backup_s3_path_style" => Some(json!(true)),
        "backup_s3_compression" => Some(json!("zstd")),
        "backup_s3_schedule_unit" => Some(json!("days")),
        "backup_s3_schedule_interval" => Some(json!(1)),
        "backup_s3_schedule_minute" => Some(json!(0)),
        "backup_s3_schedule_hour" => Some(json!(3)),
        "backup_s3_schedule_weekday" => Some(json!(1)),
        "backup_s3_schedule_month_day" => Some(json!(1)),
        "backup_s3_retention_count" => Some(json!(7)),
        "backup_s3_last_slot" => Some(serde_json::Value::Null),
        "email_suffix_mode" => Some(json!("none")),
        "email_suffix_list" => Some(json!([])),
        "enable_format_conversion" => Some(json!(false)),
        "enable_model_directives" => Some(json!(false)),
        // Failover after a provider-side Cyber policy refusal is an explicit
        // opt-in.  Keep the system-config fallback aligned with the routing
        // policy default so an unset value cannot accidentally enable it.
        "cyber_continue_failover" => Some(json!(false)),
        "model_directives" => Some(aether_ai_formats::default_model_directives_config()),
        "audit_log_retention_days" => Some(json!(30)),
        "enable_db_maintenance" => Some(json!(true)),
        "system_proxy_node_id" => Some(serde_json::Value::Null),
        "external_models_proxy_node_id" => Some(serde_json::Value::Null),
        "smtp_host" => Some(serde_json::Value::Null),
        "smtp_port" => Some(json!(587)),
        "smtp_user" => Some(serde_json::Value::Null),
        "smtp_password" => Some(serde_json::Value::Null),
        "smtp_use_tls" => Some(json!(true)),
        "smtp_use_ssl" => Some(json!(false)),
        "smtp_from_email" => Some(serde_json::Value::Null),
        "smtp_from_name" => Some(json!("Aether")),
        "enable_oauth_token_refresh" => Some(json!(true)),
        "module.important_notification.enabled" => Some(json!(false)),
        "module.important_notification.email_enabled" => Some(json!(false)),
        "module.important_notification.email_recipients" => Some(json!("")),
        "module.important_notification.default_channel" => Some(json!("all")),
        "module.important_notification.items" => Some(notification_service_default_items()),
        "module.server_chan_push.enabled" => Some(json!(false)),
        "module.server_chan_push.send_key" => Some(serde_json::Value::Null),
        "module.server_chan_push.template" => Some(json!("")),
        "module.bark_push.enabled" => Some(json!(false)),
        "module.bark_push.device_key" => Some(serde_json::Value::Null),
        "module.bark_push.server_url" => Some(json!(DEFAULT_BARK_API_BASE)),
        "module.bark_push.template" => Some(json!("")),
        "module.chat_pii_redaction.enabled" => Some(json!(false)),
        "module.chat_pii_redaction.rules" => Some(chat_pii_redaction_default_rules()),
        "module.chat_pii_redaction.cache_ttl_seconds" => Some(json!(300)),
        "module.chat_pii_redaction.placeholder_prefix" => Some(json!("AETHER")),
        _ => None,
    }
}

pub fn build_admin_system_configs_payload(
    entries: &[StoredSystemConfigEntry],
) -> serde_json::Value {
    let canonical_keys = entries
        .iter()
        .filter_map(|entry| {
            let normalized = normalize_admin_system_config_key(&entry.key);
            entry
                .key
                .eq_ignore_ascii_case(&normalized)
                .then(|| normalized.to_ascii_lowercase())
        })
        .collect::<BTreeSet<_>>();
    json!(entries
        .iter()
        .filter_map(|entry| {
            let normalized_key = normalize_admin_system_config_key(&entry.key);
            let is_legacy = !entry.key.eq_ignore_ascii_case(&normalized_key);
            if is_legacy && canonical_keys.contains(&normalized_key.to_ascii_lowercase()) {
                return None;
            }
            Some(build_admin_system_config_list_item(
                &normalized_key,
                &entry.value,
                entry.description.as_deref(),
                entry.updated_at_unix_secs,
            ))
        })
        .collect::<Vec<_>>())
}

pub fn build_admin_system_config_detail_payload(
    requested_key: &str,
    value: Option<serde_json::Value>,
) -> Result<serde_json::Value, (http::StatusCode, serde_json::Value)> {
    let normalized_key = normalize_admin_system_config_key(requested_key);
    let value = value.or_else(|| admin_system_config_default_value(&normalized_key));
    let Some(value) = value else {
        return Err((
            http::StatusCode::NOT_FOUND,
            json!({ "detail": format!("配置项 '{requested_key}' 不存在") }),
        ));
    };
    if is_sensitive_admin_system_config_key(&normalized_key) {
        return Ok(json!({
            "key": requested_key,
            "value": serde_json::Value::Null,
            "is_set": system_config_is_set(&value),
        }));
    }
    Ok(json!({
        "key": requested_key,
        "value": value,
    }))
}

fn normalize_chat_pii_redaction_rules_value(
    value: serde_json::Value,
) -> Result<serde_json::Value, ()> {
    let Some(raw_rules) = value.as_array() else {
        return Err(());
    };
    let mut rules = Vec::with_capacity(raw_rules.len());
    for raw_rule in raw_rules {
        let Some(raw_rule) = raw_rule.as_object() else {
            return Err(());
        };
        let id = raw_rule
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(())?;
        let name = raw_rule
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(())?;
        let pattern = raw_rule
            .get("pattern")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(())?;
        Regex::new(pattern).map_err(|_| ())?;
        let enabled = raw_rule
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let system = raw_rule
            .get("system")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let features = normalize_chat_pii_redaction_rule_features(raw_rule)?;
        rules.push(json!({
            "id": id,
            "name": name,
            "pattern": pattern,
            "enabled": enabled,
            "system": system,
            "features": features,
        }));
    }
    Ok(serde_json::Value::Array(rules))
}

fn normalize_chat_pii_redaction_rule_features(
    raw_rule: &Map<String, Value>,
) -> Result<serde_json::Value, ()> {
    let mut features = match raw_rule.get("features") {
        Some(Value::Object(features)) => features.clone(),
        Some(Value::Null) | None => Map::new(),
        Some(_) => return Err(()),
    };

    if !features.contains_key("validator") {
        if let Some(Value::String(value)) = raw_rule.get("kind") {
            let value = value.trim();
            if !value.is_empty() {
                features.insert("validator".to_string(), json!(value));
            }
        } else if raw_rule.get("kind").is_some_and(|value| !value.is_null()) {
            return Err(());
        }
    }

    match features.get("validator") {
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                features.remove("validator");
            } else {
                features.insert("validator".to_string(), json!(value));
            }
        }
        Some(Value::Null) => {
            features.remove("validator");
        }
        Some(_) => return Err(()),
        None => {}
    }

    Ok(Value::Object(features))
}

fn normalize_string_list_config_value(value: serde_json::Value) -> Result<serde_json::Value, ()> {
    match value {
        Value::Null => Ok(json!("")),
        Value::String(raw) => Ok(json!(raw.trim())),
        Value::Array(items) => {
            let mut normalized = Vec::with_capacity(items.len());
            for item in items {
                let Some(raw) = item.as_str() else {
                    return Err(());
                };
                let raw = raw.trim();
                if !raw.is_empty() {
                    normalized.push(raw.to_string());
                }
            }
            Ok(json!(normalized))
        }
        _ => Err(()),
    }
}

fn normalize_nullable_string_config_value(
    value: serde_json::Value,
) -> Result<serde_json::Value, ()> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::String(raw) => {
            let raw = raw.trim();
            if raw.is_empty() {
                Ok(Value::Null)
            } else {
                Ok(json!(raw))
            }
        }
        _ => Err(()),
    }
}

fn normalize_smtp_control_config_value(value: serde_json::Value) -> Result<serde_json::Value, ()> {
    let normalized = normalize_nullable_string_config_value(value)?;
    if normalized
        .as_str()
        .is_some_and(|raw| raw.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0)))
    {
        return Err(());
    }
    Ok(normalized)
}

fn normalize_bark_server_url_config_value(
    value: serde_json::Value,
) -> Result<serde_json::Value, ()> {
    match value {
        Value::Null => Ok(json!(DEFAULT_BARK_API_BASE)),
        Value::String(raw) => {
            let raw = raw.trim().trim_end_matches('/');
            if raw.is_empty() {
                return Ok(json!(DEFAULT_BARK_API_BASE));
            }
            if raw.len() > 2_048 {
                return Err(());
            }
            let parsed = url::Url::parse(raw).map_err(|_| ())?;
            if !matches!(parsed.scheme(), "https" | "http")
                || parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return Err(());
            }
            Ok(json!(parsed.as_str().trim_end_matches('/')))
        }
        _ => Err(()),
    }
}

fn normalize_notification_channel_value(value: serde_json::Value) -> Result<serde_json::Value, ()> {
    match value {
        Value::Null => Ok(json!("all")),
        Value::String(raw) => {
            let normalized = normalize_notification_channel(raw.trim(), false)?;
            Ok(json!(normalized))
        }
        _ => Err(()),
    }
}

fn normalize_notification_channel(raw: &str, allow_global: bool) -> Result<&'static str, ()> {
    match raw.to_ascii_lowercase().as_str() {
        "all" => Ok("all"),
        "email" => Ok("email"),
        "server_chan" | "serverchan" | "serve_chan" => Ok("server_chan"),
        "bark" => Ok("bark"),
        "global" | "" if allow_global => Ok("global"),
        _ => Err(()),
    }
}

fn normalize_notification_service_items_value(
    value: serde_json::Value,
) -> Result<serde_json::Value, ()> {
    let Value::Array(items) = value else {
        return Err(());
    };
    let mut normalized_items = Vec::with_capacity(items.len());
    let mut keys = BTreeSet::new();
    for item in items {
        let Value::Object(raw_item) = item else {
            return Err(());
        };
        let key = normalize_notification_item_key(raw_item.get("key"))?;
        if !keys.insert(key.clone()) {
            return Err(());
        }
        let name = normalize_optional_bounded_string(raw_item.get("name"), 80)?
            .unwrap_or_else(|| key.clone());
        let enabled = raw_item
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let channel = raw_item
            .get("channel")
            .and_then(Value::as_str)
            .map(|raw| normalize_notification_channel(raw.trim(), true))
            .transpose()?
            .unwrap_or("global");
        let title_template =
            normalize_optional_bounded_string(raw_item.get("title_template"), 256)?
                .unwrap_or_default();
        let markdown_template =
            normalize_optional_bounded_string(raw_item.get("markdown_template"), 8_000)?
                .unwrap_or_default();
        let text_template =
            normalize_optional_bounded_string(raw_item.get("text_template"), 8_000)?
                .unwrap_or_default();
        let user_email_enabled = raw_item
            .get("user_email_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let system = raw_item
            .get("system")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        normalized_items.push(json!({
            "key": key,
            "name": name,
            "enabled": enabled,
            "channel": channel,
            "title_template": title_template,
            "markdown_template": markdown_template,
            "text_template": text_template,
            "user_email_enabled": user_email_enabled,
            "system": system,
        }));
    }
    Ok(Value::Array(normalized_items))
}

fn normalize_notification_item_key(value: Option<&Value>) -> Result<String, ()> {
    let Some(raw) = value.and_then(Value::as_str).map(str::trim) else {
        return Err(());
    };
    if raw.is_empty() || raw.len() > 64 {
        return Err(());
    }
    if !raw
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
    {
        return Err(());
    }
    Ok(raw.to_string())
}

fn normalize_optional_bounded_string(
    value: Option<&Value>,
    max_len: usize,
) -> Result<Option<String>, ()> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.len() > max_len {
                return Err(());
            }
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(_) => Err(()),
    }
}

fn validate_model_directives_config_value(value: &Value) -> Result<(), ()> {
    let root = value.as_object().ok_or(())?;
    let reasoning = root
        .get("reasoning_effort")
        .and_then(Value::as_object)
        .ok_or(())?;
    if reasoning
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(());
    }

    let Some(api_formats) = reasoning.get("api_formats") else {
        return Ok(());
    };
    let api_formats = api_formats.as_object().ok_or(())?;
    let mut canonical_api_formats = BTreeSet::new();
    for (api_format, raw_config) in api_formats {
        if api_format.trim().is_empty() {
            return Err(());
        }
        let canonical_api_format = aether_ai_formats::normalize_api_format_alias(api_format);
        if !canonical_api_formats.insert(canonical_api_format) {
            return Err(());
        }
        if raw_config.is_boolean() {
            continue;
        }
        let config = raw_config.as_object().ok_or(())?;
        if config
            .get("enabled")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(());
        }
        if let Some(suffixes) = config.get("suffixes") {
            let suffixes = suffixes.as_array().ok_or(())?;
            let mut canonical_suffixes = BTreeSet::new();
            for suffix in suffixes {
                let suffix = suffix.as_str().map(str::trim).ok_or(())?;
                if suffix.is_empty()
                    || suffix.starts_with('-')
                    || suffix.ends_with('-')
                    || !canonical_suffixes.insert(suffix.to_ascii_lowercase())
                {
                    return Err(());
                }
            }
        }
        if let Some(mappings) = config.get("mappings") {
            let mappings = mappings.as_object().ok_or(())?;
            let mut canonical_suffixes = BTreeSet::new();
            for (suffix, mapping) in mappings {
                let suffix = suffix.trim();
                if suffix.is_empty()
                    || suffix.starts_with('-')
                    || suffix.ends_with('-')
                    || !canonical_suffixes.insert(suffix.to_ascii_lowercase())
                    || !mapping.is_object()
                {
                    return Err(());
                }
            }
        }
    }
    Ok(())
}

pub fn parse_admin_system_config_update(
    requested_key: &str,
    request_body: &[u8],
) -> Result<AdminSystemConfigUpdate, (http::StatusCode, serde_json::Value)> {
    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(payload)) => payload,
        _ => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };
    let normalized_key = normalize_admin_system_config_key(requested_key);
    let mut value = payload
        .get("value")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let description = match payload.get("description") {
        Some(serde_json::Value::String(value)) => Some(value.trim().to_string()),
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };

    if normalized_key == "password_policy_level" {
        match value.as_str().map(str::trim) {
            Some("weak" | "medium" | "strong") => {
                value = json!(value.as_str().unwrap().trim());
            }
            Some(_) => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
            None if value.is_null() => {
                value = json!("weak");
            }
            None => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        }
    }

    match normalized_key.as_str() {
        "enable_model_directives"
        | "cyber_continue_failover"
        | "module.important_notification.enabled"
        | "module.important_notification.email_enabled"
        | "module.server_chan_push.enabled"
        | "module.bark_push.enabled" => match value.as_bool() {
            Some(enabled) => value = json!(enabled),
            None if value.is_null() => {
                value = admin_system_config_default_value(&normalized_key).unwrap_or(json!(false));
            }
            None => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        },
        "module.important_notification.email_recipients" => {
            value = normalize_string_list_config_value(value).map_err(|_| {
                (
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                )
            })?;
        }
        "module.important_notification.default_channel" => {
            value = normalize_notification_channel_value(value).map_err(|_| {
                (
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                )
            })?;
        }
        "module.important_notification.items" => {
            if value.is_null() {
                value = notification_service_default_items();
            } else {
                value = normalize_notification_service_items_value(value).map_err(|_| {
                    (
                        http::StatusCode::BAD_REQUEST,
                        json!({ "detail": "请求数据验证失败" }),
                    )
                })?;
            }
        }
        "module.server_chan_push.send_key" => {
            value = normalize_nullable_string_config_value(value).map_err(|_| {
                (
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                )
            })?;
        }
        "module.server_chan_push.template" => {
            value = match value {
                Value::Null => json!(""),
                Value::String(raw) => json!(raw),
                _ => {
                    return Err((
                        http::StatusCode::BAD_REQUEST,
                        json!({ "detail": "请求数据验证失败" }),
                    ));
                }
            };
        }
        "module.bark_push.device_key" => {
            value = normalize_nullable_string_config_value(value).map_err(|_| {
                (
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                )
            })?;
        }
        "module.bark_push.server_url" => {
            value = normalize_bark_server_url_config_value(value).map_err(|_| {
                (
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                )
            })?;
        }
        "module.bark_push.template" => {
            value = match value {
                Value::Null => json!(""),
                Value::String(raw) => json!(raw),
                _ => {
                    return Err((
                        http::StatusCode::BAD_REQUEST,
                        json!({ "detail": "请求数据验证失败" }),
                    ));
                }
            };
        }
        "smtp_host" | "smtp_from_email" | "smtp_from_name" => {
            value = normalize_smtp_control_config_value(value).map_err(|_| {
                (
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                )
            })?;
        }
        "model_directives" => {
            if value.is_null() {
                value = aether_ai_formats::default_model_directives_config();
            } else {
                validate_model_directives_config_value(&value).map_err(|_| {
                    (
                        http::StatusCode::BAD_REQUEST,
                        json!({ "detail": "模型后缀参数配置格式无效" }),
                    )
                })?;
            }
        }
        "module.chat_pii_redaction.enabled" => match value.as_bool() {
            Some(enabled) => value = json!(enabled),
            None if value.is_null() => {
                value = admin_system_config_default_value(&normalized_key).unwrap();
            }
            None => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        },
        "module.chat_pii_redaction.rules" => {
            if value.is_null() {
                value = chat_pii_redaction_default_rules();
            } else {
                value = normalize_chat_pii_redaction_rules_value(value).map_err(|_| {
                    (
                        http::StatusCode::BAD_REQUEST,
                        json!({ "detail": "请求数据验证失败" }),
                    )
                })?;
            }
        }
        "module.chat_pii_redaction.cache_ttl_seconds" => match value.as_u64() {
            Some(300 | 3600) => value = json!(value.as_u64().unwrap()),
            Some(_) => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
            None if value.is_null() => value = json!(300),
            None => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        },
        "module.chat_pii_redaction.placeholder_prefix" => match value.as_str() {
            Some(raw) => {
                let Some(normalized) = normalize_chat_pii_redaction_placeholder_prefix(raw) else {
                    return Err((
                        http::StatusCode::BAD_REQUEST,
                        json!({ "detail": "请求数据验证失败" }),
                    ));
                };
                value = json!(normalized);
            }
            None if value.is_null() => value = json!("AETHER"),
            None => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        },
        _ => {}
    }

    if is_sensitive_admin_system_config_key(&normalized_key)
        && !matches!(&value, Value::Null | Value::String(_))
    {
        return Err((
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": "请求数据验证失败" }),
        ));
    }

    Ok(AdminSystemConfigUpdate {
        normalized_key,
        value,
        description,
    })
}

pub fn build_admin_system_config_updated_payload(
    key: String,
    value: serde_json::Value,
    description: Option<String>,
    updated_at_unix_secs: Option<u64>,
) -> serde_json::Value {
    json!({
        "key": key,
        "value": value,
        "description": description,
        "updated_at": updated_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

pub fn build_admin_system_config_deleted_payload(requested_key: &str) -> serde_json::Value {
    json!({
        "message": format!("配置项 '{}' 已删除", requested_key.trim()),
    })
}

pub fn is_admin_management_tokens_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/management-tokens" | "/api/admin/management-tokens/"
    )
}

pub fn is_admin_system_configs_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/system/configs" | "/api/admin/system/configs/"
    )
}

pub fn is_admin_system_email_templates_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/system/email/templates" | "/api/admin/system/email/templates/"
    )
}

pub fn admin_system_config_key_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/system/configs/")
}

pub fn admin_system_email_template_type_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/system/email/templates/")
}

pub fn admin_system_email_template_preview_type_from_path(request_path: &str) -> Option<String> {
    suffixed_path_identifier_from_path(
        request_path,
        "/api/admin/system/email/templates/",
        "/preview",
    )
}

pub fn admin_system_email_template_reset_type_from_path(request_path: &str) -> Option<String> {
    suffixed_path_identifier_from_path(request_path, "/api/admin/system/email/templates/", "/reset")
}

pub fn admin_management_token_id_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/management-tokens/")
}

pub fn admin_management_token_status_id_from_path(request_path: &str) -> Option<String> {
    suffixed_path_identifier_from_path(request_path, "/api/admin/management-tokens/", "/status")
}

pub fn admin_management_token_regenerate_id_from_path(request_path: &str) -> Option<String> {
    suffixed_path_identifier_from_path(request_path, "/api/admin/management-tokens/", "/regenerate")
}

pub fn admin_adaptive_effective_limit(key: &StoredProviderCatalogKey) -> Option<u32> {
    if key.rpm_limit.is_none() {
        key.learned_rpm_limit
    } else {
        key.rpm_limit
    }
}

pub fn admin_adaptive_adjustment_items(
    value: Option<&serde_json::Value>,
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_object)
        .cloned()
        .collect()
}

pub fn admin_adaptive_key_payload(key: &StoredProviderCatalogKey) -> serde_json::Value {
    json!({
        "id": key.id,
        "name": key.name,
        "provider_id": key.provider_id,
        "api_formats": key
            .api_formats
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "is_adaptive": key.rpm_limit.is_none(),
        "rpm_limit": key.rpm_limit,
        "effective_limit": admin_adaptive_effective_limit(key),
        "learned_rpm_limit": key.learned_rpm_limit,
        "concurrent_429_count": key.concurrent_429_count.unwrap_or(0),
        "rpm_429_count": key.rpm_429_count.unwrap_or(0),
    })
}

pub fn build_admin_adaptive_summary_payload(
    keys: &[StoredProviderCatalogKey],
) -> serde_json::Value {
    let adaptive_keys = keys
        .iter()
        .filter(|key| key.rpm_limit.is_none())
        .collect::<Vec<_>>();

    let total_keys = adaptive_keys.len() as u64;
    let total_concurrent_429 = adaptive_keys
        .iter()
        .map(|key| u64::from(key.concurrent_429_count.unwrap_or(0)))
        .sum::<u64>();
    let total_rpm_429 = adaptive_keys
        .iter()
        .map(|key| u64::from(key.rpm_429_count.unwrap_or(0)))
        .sum::<u64>();

    let mut recent_adjustments = Vec::new();
    let mut total_adjustments = 0usize;
    for key in adaptive_keys {
        let history = admin_adaptive_adjustment_items(key.adjustment_history.as_ref());
        total_adjustments += history.len();
        for adjustment in history.into_iter().rev().take(3) {
            let mut payload = adjustment;
            payload.insert("key_id".to_string(), json!(key.id));
            payload.insert("key_name".to_string(), json!(key.name));
            recent_adjustments.push(serde_json::Value::Object(payload));
        }
    }

    recent_adjustments.sort_by(|left, right| {
        let lhs = left
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let rhs = right
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        rhs.cmp(lhs)
    });

    json!({
        "total_adaptive_keys": total_keys,
        "total_concurrent_429_errors": total_concurrent_429,
        "total_rpm_429_errors": total_rpm_429,
        "total_adjustments": total_adjustments,
        "recent_adjustments": recent_adjustments.into_iter().take(10).collect::<Vec<_>>(),
    })
}

pub fn build_admin_adaptive_stats_payload(key: &StoredProviderCatalogKey) -> serde_json::Value {
    let status_snapshot = key
        .status_snapshot
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let adjustments = admin_adaptive_adjustment_items(key.adjustment_history.as_ref());
    let adjustment_count = adjustments.len();
    let recent_adjustments = adjustments
        .into_iter()
        .rev()
        .take(10)
        .map(serde_json::Value::Object)
        .collect::<Vec<_>>();

    json!({
        "adaptive_mode": key.rpm_limit.is_none(),
        "rpm_limit": key.rpm_limit,
        "effective_limit": admin_adaptive_effective_limit(key),
        "learned_limit": key.learned_rpm_limit,
        "concurrent_429_count": key.concurrent_429_count.unwrap_or(0),
        "rpm_429_count": key.rpm_429_count.unwrap_or(0),
        "last_429_at": key.last_429_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "last_429_type": key.last_429_type,
        "adjustment_count": adjustment_count,
        "recent_adjustments": recent_adjustments,
        "learning_confidence": status_snapshot.and_then(|value| value.get("learning_confidence")).cloned(),
        "enforcement_active": status_snapshot.and_then(|value| value.get("enforcement_active")).cloned(),
        "observation_count": status_snapshot
            .and_then(|value| value.get("observation_count"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        "header_observation_count": status_snapshot
            .and_then(|value| value.get("header_observation_count"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        "latest_upstream_limit": status_snapshot
            .and_then(|value| value.get("latest_upstream_limit"))
            .and_then(serde_json::Value::as_u64),
    })
}

pub fn build_admin_adaptive_toggle_mode_payload(
    updated: &StoredProviderCatalogKey,
    message: String,
) -> serde_json::Value {
    json!({
        "message": message,
        "key_id": updated.id,
        "is_adaptive": updated.rpm_limit.is_none(),
        "rpm_limit": updated.rpm_limit,
        "effective_limit": admin_adaptive_effective_limit(updated),
    })
}

pub fn build_admin_adaptive_set_limit_payload(
    updated: &StoredProviderCatalogKey,
    was_adaptive: bool,
    limit: u32,
) -> serde_json::Value {
    json!({
        "message": format!("已设置为固定限制模式，RPM 限制为 {limit}"),
        "key_id": updated.id,
        "is_adaptive": false,
        "rpm_limit": updated.rpm_limit,
        "previous_mode": if was_adaptive { "adaptive" } else { "fixed" },
    })
}

pub fn build_admin_adaptive_reset_learning_payload(key_id: &str) -> serde_json::Value {
    json!({
        "message": "学习状态已重置",
        "key_id": key_id,
    })
}

pub fn admin_adaptive_key_not_found_response(key_id: &str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": format!("Key {key_id} 不存在") })),
    )
        .into_response()
}

pub fn admin_adaptive_dispatcher_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Adaptive route not found" })),
    )
        .into_response()
}

pub fn admin_adaptive_key_id_from_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let mut segments = normalized.split('/').filter(|segment| !segment.is_empty());
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some("api"), Some("admin"), Some("adaptive"), Some("keys"), Some(key_id))
            if !key_id.is_empty() =>
        {
            Some(key_id.to_string())
        }
        _ => None,
    }
}

pub const ADMIN_PROXY_NODES_DATA_UNAVAILABLE_DETAIL: &str = "Admin proxy nodes data unavailable";

pub fn build_admin_proxy_nodes_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_PROXY_NODES_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub fn build_admin_proxy_nodes_invalid_status_response() -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({
            "detail": "status 必须是以下之一: ['offline', 'online']"
        })),
    )
        .into_response()
}

pub fn build_admin_proxy_nodes_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Proxy node 不存在" })),
    )
        .into_response()
}

pub fn build_admin_proxy_node_payload(node: &StoredProxyNode) -> serde_json::Value {
    // Keep operational tunnel metadata, but never expose either legacy plaintext
    // or encrypted credential material through an admin response.
    let proxy_metadata = redact_admin_proxy_node_metadata(node.proxy_metadata.as_ref());
    let mut payload = serde_json::Map::from_iter([
        ("id".to_string(), json!(node.id)),
        ("name".to_string(), json!(node.name)),
        ("ip".to_string(), json!(node.ip)),
        ("port".to_string(), json!(node.port)),
        ("region".to_string(), json!(node.region)),
        ("status".to_string(), json!(node.status)),
        ("is_manual".to_string(), json!(node.is_manual)),
        ("tunnel_mode".to_string(), json!(node.tunnel_mode)),
        ("tunnel_connected".to_string(), json!(node.tunnel_connected)),
        (
            "tunnel_connected_at".to_string(),
            json!(node
                .tunnel_connected_at_unix_secs
                .and_then(unix_secs_to_rfc3339)),
        ),
        ("registered_by".to_string(), json!(node.registered_by)),
        (
            "last_heartbeat_at".to_string(),
            json!(node
                .last_heartbeat_at_unix_secs
                .and_then(unix_secs_to_rfc3339)),
        ),
        (
            "heartbeat_interval".to_string(),
            json!(node.heartbeat_interval),
        ),
        (
            "active_connections".to_string(),
            json!(node.active_connections),
        ),
        ("total_requests".to_string(), json!(node.total_requests)),
        ("avg_latency_ms".to_string(), json!(node.avg_latency_ms)),
        ("failed_requests".to_string(), json!(node.failed_requests)),
        ("dns_failures".to_string(), json!(node.dns_failures)),
        ("stream_errors".to_string(), json!(node.stream_errors)),
        ("proxy_metadata".to_string(), json!(proxy_metadata)),
        ("hardware_info".to_string(), json!(node.hardware_info)),
        (
            "estimated_max_concurrency".to_string(),
            json!(node.estimated_max_concurrency),
        ),
        ("remote_config".to_string(), json!(node.remote_config)),
        ("config_version".to_string(), json!(node.config_version)),
        (
            "created_at".to_string(),
            json!(node.created_at_unix_ms.and_then(unix_secs_to_rfc3339)),
        ),
        (
            "updated_at".to_string(),
            json!(node.updated_at_unix_secs.and_then(unix_secs_to_rfc3339)),
        ),
    ]);

    if node.is_manual {
        payload.insert("proxy_url".to_string(), json!(node.proxy_url));
        payload.insert("proxy_username".to_string(), json!(node.proxy_username));
    }
    payload.insert(
        "has_proxy_password".to_string(),
        json!(node
            .proxy_password
            .as_deref()
            .is_some_and(|password| !password.is_empty())),
    );

    serde_json::Value::Object(payload)
}

fn redact_admin_proxy_node_metadata(
    metadata: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let metadata = metadata?;
    let mut metadata = metadata.clone();
    if let Some(tunnel_security) = metadata
        .as_object_mut()
        .and_then(|object| object.get_mut("tunnel_security"))
        .and_then(serde_json::Value::as_object_mut)
    {
        tunnel_security.remove("encryption_key");
        tunnel_security.remove("encryption_key_encrypted");
    }
    Some(metadata)
}

pub fn build_admin_proxy_node_event_payload(event: &StoredProxyNodeEvent) -> serde_json::Value {
    json!({
        "id": event.id,
        "event_type": event.event_type,
        "detail": event.detail,
        "event_metadata": event.event_metadata,
        "created_at": event.created_at_unix_ms.and_then(unix_secs_to_rfc3339),
    })
}

pub fn admin_proxy_node_event_node_id_from_path(request_path: &str) -> Option<&str> {
    let request_path = request_path.trim_end_matches('/');
    let node_id = request_path.strip_prefix("/api/admin/proxy-nodes/")?;
    let node_id = node_id.strip_suffix("/events")?;
    if node_id.is_empty() || node_id.contains('/') {
        None
    } else {
        Some(node_id)
    }
}

pub fn admin_proxy_node_metrics_node_id_from_path(request_path: &str) -> Option<&str> {
    let request_path = request_path.trim_end_matches('/');
    let node_id = request_path.strip_prefix("/api/admin/proxy-nodes/")?;
    let node_id = node_id.strip_suffix("/metrics")?;
    if node_id.is_empty() || node_id.contains('/') {
        None
    } else {
        Some(node_id)
    }
}

pub fn build_admin_proxy_node_metrics_payload_response(
    step: ProxyNodeMetricsStep,
    from_unix_secs: u64,
    to_unix_secs: u64,
    items: Vec<StoredProxyNodeMetricsBucket>,
) -> Response<Body> {
    let summary = summarize_proxy_node_metric_buckets(items.iter().map(|item| {
        (
            item.samples,
            item.uptime_samples,
            item.active_connections_sum,
            item.active_connections_max,
            item.heartbeat_rtt_ms_sum,
            item.heartbeat_rtt_ms_max,
            item.connect_errors_delta,
            item.disconnects_delta,
            item.error_events_delta,
            item.ws_in_bytes_delta,
            item.ws_out_bytes_delta,
            item.ws_in_frames_delta,
            item.ws_out_frames_delta,
        )
    }));
    let items = items
        .into_iter()
        .map(build_admin_proxy_node_metrics_bucket_payload)
        .collect::<Vec<_>>();
    Json(json!({
        "step": step.as_api_value(),
        "from": from_unix_secs,
        "to": to_unix_secs,
        "items": items,
        "summary": summary,
    }))
    .into_response()
}

pub fn build_admin_proxy_fleet_metrics_payload_response(
    step: ProxyNodeMetricsStep,
    from_unix_secs: u64,
    to_unix_secs: u64,
    items: Vec<StoredProxyFleetMetricsBucket>,
) -> Response<Body> {
    let summary = summarize_proxy_node_metric_buckets(items.iter().map(|item| {
        (
            item.samples,
            item.uptime_samples,
            item.active_connections_sum,
            item.active_connections_max,
            item.heartbeat_rtt_ms_sum,
            item.heartbeat_rtt_ms_max,
            item.connect_errors_delta,
            item.disconnects_delta,
            item.error_events_delta,
            item.ws_in_bytes_delta,
            item.ws_out_bytes_delta,
            item.ws_in_frames_delta,
            item.ws_out_frames_delta,
        )
    }));
    let items = items
        .into_iter()
        .map(build_admin_proxy_fleet_metrics_bucket_payload)
        .collect::<Vec<_>>();
    Json(json!({
        "step": step.as_api_value(),
        "from": from_unix_secs,
        "to": to_unix_secs,
        "items": items,
        "summary": summary,
    }))
    .into_response()
}

fn build_admin_proxy_node_metrics_bucket_payload(
    item: StoredProxyNodeMetricsBucket,
) -> serde_json::Value {
    json!({
        "node_id": item.node_id,
        "bucket_start_unix_secs": item.bucket_start_unix_secs,
        "bucket_start": unix_secs_to_rfc3339(item.bucket_start_unix_secs),
        "samples": item.samples,
        "uptime_samples": item.uptime_samples,
        "uptime_ratio": ratio(item.uptime_samples, item.samples),
        "active_connections_sum": item.active_connections_sum,
        "active_connections_max": item.active_connections_max,
        "active_connections_avg": ratio(item.active_connections_sum, item.samples),
        "heartbeat_rtt_ms_sum": item.heartbeat_rtt_ms_sum,
        "heartbeat_rtt_ms_max": item.heartbeat_rtt_ms_max,
        "heartbeat_rtt_ms_avg": ratio(item.heartbeat_rtt_ms_sum, item.samples),
        "connect_errors_delta": item.connect_errors_delta,
        "disconnects_delta": item.disconnects_delta,
        "error_events_delta": item.error_events_delta,
        "ws_in_bytes_delta": item.ws_in_bytes_delta,
        "ws_out_bytes_delta": item.ws_out_bytes_delta,
        "ws_in_frames_delta": item.ws_in_frames_delta,
        "ws_out_frames_delta": item.ws_out_frames_delta,
    })
}

fn build_admin_proxy_fleet_metrics_bucket_payload(
    item: StoredProxyFleetMetricsBucket,
) -> serde_json::Value {
    json!({
        "bucket_start_unix_secs": item.bucket_start_unix_secs,
        "bucket_start": unix_secs_to_rfc3339(item.bucket_start_unix_secs),
        "samples": item.samples,
        "uptime_samples": item.uptime_samples,
        "uptime_ratio": ratio(item.uptime_samples, item.samples),
        "active_connections_sum": item.active_connections_sum,
        "active_connections_max": item.active_connections_max,
        "active_connections_avg": ratio(item.active_connections_sum, item.samples),
        "heartbeat_rtt_ms_sum": item.heartbeat_rtt_ms_sum,
        "heartbeat_rtt_ms_max": item.heartbeat_rtt_ms_max,
        "heartbeat_rtt_ms_avg": ratio(item.heartbeat_rtt_ms_sum, item.samples),
        "connect_errors_delta": item.connect_errors_delta,
        "disconnects_delta": item.disconnects_delta,
        "error_events_delta": item.error_events_delta,
        "ws_in_bytes_delta": item.ws_in_bytes_delta,
        "ws_out_bytes_delta": item.ws_out_bytes_delta,
        "ws_in_frames_delta": item.ws_in_frames_delta,
        "ws_out_frames_delta": item.ws_out_frames_delta,
    })
}

fn summarize_proxy_node_metric_buckets<I>(items: I) -> serde_json::Value
where
    I: IntoIterator<
        Item = (
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ),
    >,
{
    let mut samples = 0;
    let mut uptime_samples = 0;
    let mut active_connections_sum = 0;
    let mut active_connections_max = 0;
    let mut heartbeat_rtt_ms_sum = 0;
    let mut heartbeat_rtt_ms_max = 0;
    let mut connect_errors_delta = 0;
    let mut disconnects_delta = 0;
    let mut error_events_delta = 0;
    let mut ws_in_bytes_delta = 0;
    let mut ws_out_bytes_delta = 0;
    let mut ws_in_frames_delta = 0;
    let mut ws_out_frames_delta = 0;

    for item in items {
        samples += item.0;
        uptime_samples += item.1;
        active_connections_sum += item.2;
        active_connections_max = active_connections_max.max(item.3);
        heartbeat_rtt_ms_sum += item.4;
        heartbeat_rtt_ms_max = heartbeat_rtt_ms_max.max(item.5);
        connect_errors_delta += item.6;
        disconnects_delta += item.7;
        error_events_delta += item.8;
        ws_in_bytes_delta += item.9;
        ws_out_bytes_delta += item.10;
        ws_in_frames_delta += item.11;
        ws_out_frames_delta += item.12;
    }

    json!({
        "samples": samples,
        "uptime_samples": uptime_samples,
        "uptime_ratio": ratio(uptime_samples, samples),
        "active_connections_sum": active_connections_sum,
        "active_connections_max": active_connections_max,
        "active_connections_avg": ratio(active_connections_sum, samples),
        "heartbeat_rtt_ms_sum": heartbeat_rtt_ms_sum,
        "heartbeat_rtt_ms_max": heartbeat_rtt_ms_max,
        "heartbeat_rtt_ms_avg": ratio(heartbeat_rtt_ms_sum, samples),
        "connect_errors_delta": connect_errors_delta,
        "disconnects_delta": disconnects_delta,
        "error_events_delta": error_events_delta,
        "ws_in_bytes_delta": ws_in_bytes_delta,
        "ws_out_bytes_delta": ws_out_bytes_delta,
        "ws_in_frames_delta": ws_in_frames_delta,
        "ws_out_frames_delta": ws_out_frames_delta,
    })
}

fn ratio(numerator: i64, denominator: i64) -> Option<f64> {
    if denominator <= 0 {
        return None;
    }
    Some(numerator as f64 / denominator as f64)
}

pub fn build_admin_proxy_nodes_list_payload_response(
    items: Vec<serde_json::Value>,
    total: usize,
    skip: usize,
    limit: usize,
    rollout: Option<serde_json::Value>,
) -> Response<Body> {
    Json(json!({
        "items": items,
        "total": total,
        "skip": skip,
        "limit": limit,
        "rollout": rollout,
    }))
    .into_response()
}

pub fn build_admin_proxy_node_events_payload_response(
    items: Vec<serde_json::Value>,
) -> Response<Body> {
    Json(json!({ "items": items })).into_response()
}

fn system_config_is_set(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(|value| value != 0)
            .or_else(|| value.as_u64().map(|value| value != 0))
            .or_else(|| value.as_f64().map(|value| value != 0.0))
            .unwrap_or(false),
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
    }
}

fn build_admin_system_config_list_item(
    key: &str,
    value: &serde_json::Value,
    description: Option<&str>,
    updated_at_unix_secs: Option<u64>,
) -> serde_json::Value {
    let masked_value = if is_sensitive_admin_system_config_key(key) {
        serde_json::Value::Null
    } else {
        value.clone()
    };
    let is_set = is_sensitive_admin_system_config_key(key).then(|| system_config_is_set(value));
    let mut payload = json!({
        "key": key,
        "description": description,
        "updated_at": updated_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "value": masked_value,
    });
    if let Some(is_set) = is_set {
        payload["is_set"] = json!(is_set);
    }
    payload
}

fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_secs as i64, 0)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn path_identifier_from_path(request_path: &str, prefix: &str) -> Option<String> {
    let value = request_path
        .strip_prefix(prefix)?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

fn suffixed_path_identifier_from_path(
    request_path: &str,
    prefix: &str,
    suffix: &str,
) -> Option<String> {
    request_path
        .strip_prefix(prefix)?
        .strip_suffix(suffix)
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_template_update_rejects_oversized_and_control_fields() {
        let oversized_subject = serde_json::json!({
            "subject": "x".repeat(ADMIN_EMAIL_TEMPLATE_MAX_SUBJECT_BYTES + 1)
        });
        assert!(
            parse_admin_email_template_update(oversized_subject.to_string().as_bytes()).is_err()
        );

        let control_html = serde_json::json!({ "html": "<p>ok</p>\u{0001}" });
        assert!(parse_admin_email_template_update(control_html.to_string().as_bytes()).is_err());
    }

    #[test]
    fn email_template_preview_bounds_html_and_variable_values() {
        let oversized_html = serde_json::json!({
            "html": "x".repeat(ADMIN_EMAIL_TEMPLATE_MAX_HTML_BYTES + 1)
        });
        assert!(parse_admin_email_template_preview_payload(Some(
            oversized_html.to_string().as_bytes()
        ))
        .is_err());

        let oversized_value = serde_json::json!({
            "app_name": "x".repeat(ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_VALUE_BYTES + 1)
        });
        assert!(parse_admin_email_template_preview_payload(Some(
            oversized_value.to_string().as_bytes()
        ))
        .is_err());
    }

    #[test]
    fn email_template_field_validators_allow_normal_markup_and_subjects() {
        assert!(admin_email_template_subject_is_valid("验证码"));
        assert!(admin_email_template_html_is_valid("<p>{{app_name}}</p>\n"));
        assert!(!admin_email_template_html_is_valid("<p>bad\u{007f}</p>"));
    }

    #[test]
    fn ldap_module_validation_requires_an_encrypted_transport() {
        let config = |server_url: &str, use_starttls: bool| StoredLdapModuleConfig {
            server_url: server_url.to_string(),
            bind_dn: "cn=bind,dc=example,dc=com".to_string(),
            bind_password_encrypted: Some("sealed-password".to_string()),
            base_dn: "dc=example,dc=com".to_string(),
            user_search_filter: Some("(uid={username})".to_string()),
            username_attr: Some("uid".to_string()),
            email_attr: Some("mail".to_string()),
            display_name_attr: Some("cn".to_string()),
            is_enabled: true,
            is_exclusive: false,
            use_starttls,
            connect_timeout: Some(10),
        };

        assert!(ldap_module_config_is_valid(Some(&config(
            "ldaps://ldap.internal.example:636",
            false,
        ))));
        assert!(ldap_module_config_is_valid(Some(&config(
            "ldap://10.0.0.7:389",
            true,
        ))));
        assert!(!ldap_module_config_is_valid(Some(&config(
            "ldap://10.0.0.7:389",
            false,
        ))));
        assert!(!ldap_module_config_is_valid(Some(&config(
            "ldaps://user:password@ldap.internal.example",
            false,
        ))));
        assert!(!ldap_module_config_is_valid(Some(&config(
            "ldaps://ldap.internal.example?secret=1",
            false,
        ))));
        assert!(!ldap_module_config_is_valid(Some(&config(
            "ldaps://ldap.internal.example#fragment",
            false,
        ))));
        assert!(
            normalize_ldap_transport_server_url("mockldap://ldap.internal.example", false,)
                .is_none()
        );
        assert!(normalize_ldap_transport_server_url_for_tests(
            "mockldap://ldap.internal.example",
            false,
        )
        .is_some());
    }

    #[test]
    fn ldap_search_filter_requires_one_outer_expression() {
        assert!(ldap_search_filter_is_valid("(uid={username})"));
        assert!(ldap_search_filter_is_valid(
            "(&(objectClass=person)(uid={username}))"
        ));

        assert!(!ldap_search_filter_is_valid(
            "(uid={username})(objectClass=person)"
        ));
        assert!(!ldap_search_filter_is_valid(
            "(uid={username}) (objectClass=person)"
        ));
        assert!(!ldap_search_filter_is_valid("(uid={username}))"));
        assert!(!ldap_search_filter_is_valid("((uid={username})"));
        assert!(!ldap_search_filter_is_valid("(uid=alice)"));
    }

    #[test]
    fn ldap_attribute_descriptions_follow_bounded_rfc4512_shape() {
        for valid in [
            "uid",
            "sAMAccountName",
            "display-name",
            "mail;lang-en",
            "1.2.840.113556.1.4.221",
        ] {
            assert!(ldap_attribute_description_is_valid(valid), "{valid}");
        }
        for invalid in [
            "",
            "uid)(|(objectClass=*)",
            "uid\nmail",
            "-uid",
            "01.2.3",
            "1",
            "uid;",
            "uid;lang=en",
            "用户名",
        ] {
            assert!(!ldap_attribute_description_is_valid(invalid), "{invalid}");
        }
        assert!(!ldap_attribute_description_is_valid(&"a".repeat(129)));
    }

    #[test]
    fn ldap_distinguished_names_reject_control_and_unbounded_data() {
        assert!(ldap_distinguished_name_is_valid(
            "cn=service\\, account,ou=users,dc=example,dc=com"
        ));
        assert!(!ldap_distinguished_name_is_valid(""));
        assert!(!ldap_distinguished_name_is_valid(
            "cn=service\n,dc=example,dc=com"
        ));
        assert!(!ldap_distinguished_name_is_valid(&"a".repeat(4097)));
    }

    #[test]
    fn ldap_module_field_validation_rejects_filter_and_attribute_injection() {
        let mut config = StoredLdapModuleConfig {
            server_url: "ldaps://ldap.internal.example".to_string(),
            bind_dn: "cn=bind,dc=example,dc=com".to_string(),
            bind_password_encrypted: Some("sealed-password".to_string()),
            base_dn: "dc=example,dc=com".to_string(),
            user_search_filter: Some("(uid={username})".to_string()),
            username_attr: Some("uid".to_string()),
            email_attr: Some("mail".to_string()),
            display_name_attr: Some("cn".to_string()),
            is_enabled: true,
            is_exclusive: false,
            use_starttls: false,
            connect_timeout: Some(10),
        };
        assert!(ldap_module_config_fields_are_valid(&config));

        config.user_search_filter = Some("(uid={username})(objectClass=*)".to_string());
        assert!(!ldap_module_config_fields_are_valid(&config));
        config.user_search_filter = Some("(uid={username})".to_string());
        config.username_attr = Some("uid)(|(objectClass=*)".to_string());
        assert!(!ldap_module_config_fields_are_valid(&config));
        config.username_attr = Some("uid".to_string());
        config.base_dn = "dc=example,dc=com\n".to_string();
        assert!(!ldap_module_config_fields_are_valid(&config));
    }

    #[test]
    fn admin_proxy_node_payload_redacts_tunnel_psk_and_password() {
        let mut node = StoredProxyNode::new(
            "node-1".to_string(),
            "edge-1".to_string(),
            "127.0.0.1".to_string(),
            0,
            true,
            "online".to_string(),
            30,
            0,
            0,
            0,
            0,
            0,
            false,
            false,
            0,
        )
        .expect("proxy node should build")
        .with_manual_proxy_fields(
            Some("http://proxy.example:8080".to_string()),
            Some("alice".to_string()),
            Some("supersecret".to_string()),
        );
        node.proxy_metadata = Some(json!({
            "version": "1.2.3",
            "tunnel_security": {
                "mode": "non_tls_required",
                "encryption_key": "secret-psk",
                "encryption_key_encrypted": "sealed-secret-psk",
                "rotation_id": "rotation-1"
            }
        }));

        let payload = build_admin_proxy_node_payload(&node);

        assert_eq!(payload["has_proxy_password"], json!(true));
        assert!(payload.get("proxy_password").is_none());
        assert_eq!(
            payload["proxy_metadata"]["tunnel_security"]["mode"],
            json!("non_tls_required")
        );
        assert_eq!(
            payload["proxy_metadata"]["tunnel_security"]["rotation_id"],
            json!("rotation-1")
        );
        assert!(payload["proxy_metadata"]["tunnel_security"]
            .get("encryption_key")
            .is_none());
        assert!(payload["proxy_metadata"]["tunnel_security"]
            .get("encryption_key_encrypted")
            .is_none());
        assert_eq!(
            node.proxy_metadata
                .as_ref()
                .and_then(|value| value.pointer("/tunnel_security/encryption_key"))
                .and_then(serde_json::Value::as_str),
            Some("secret-psk")
        );
    }

    #[test]
    fn api_formats_payload_exposes_realtime_and_codex_live_separately() {
        let payload = build_admin_api_formats_payload();
        let formats = payload["formats"]
            .as_array()
            .expect("formats payload should be an array");
        let realtime = formats
            .iter()
            .find(|format| format["value"] == "openai:realtime")
            .expect("OpenAI Realtime format should be registered");
        let live = formats
            .iter()
            .find(|format| format["value"] == "codex:live")
            .expect("OpenAI Live format should be registered");

        assert_eq!(realtime["label"], "OpenAI Realtime");
        assert_eq!(realtime["default_path"], "/v1/realtime");
        assert_eq!(
            realtime["aliases"],
            serde_json::json!(["openai_realtime", "realtime"])
        );
        assert_eq!(live["label"], "OpenAI Live");
        assert_eq!(live["default_path"], "/v1/live");
        assert_eq!(live["aliases"], serde_json::json!(["codex_live", "live"]));
    }

    #[test]
    fn build_admin_system_check_update_payload_reports_available_release() {
        let payload = build_admin_system_check_update_payload_with_release(
            "0.7.0-rc27".to_string(),
            Some(AdminSystemUpdateRelease {
                version: "v0.7.0-rc28".to_string(),
                release_url: Some(
                    "https://github.com/fawney19/Aether/releases/tag/v0.7.0-rc28".to_string(),
                ),
                release_notes: Some("release notes".to_string()),
                published_at: Some("2026-05-13T00:00:00Z".to_string()),
                tarball_url: None,
                sha256sums_url: None,
            }),
            None,
        );

        assert_eq!(payload["current_version"], "0.7.0-rc27");
        assert_eq!(payload["latest_version"], "v0.7.0-rc28");
        assert_eq!(payload["has_update"], true);
        assert_eq!(
            payload["release_url"],
            "https://github.com/fawney19/Aether/releases/tag/v0.7.0-rc28"
        );
        assert_eq!(payload["release_notes"], "release notes");
        assert_eq!(payload["published_at"], "2026-05-13T00:00:00Z");
        assert_eq!(payload["updatable"], false);
        assert_eq!(payload["update_blocker"], "当前平台暂无安装包");
        assert_eq!(payload["error"], serde_json::Value::Null);
    }

    #[test]
    fn build_admin_system_check_update_payload_reports_updatable_release() {
        let payload = build_admin_system_check_update_payload_with_release(
            "0.7.0-rc27".to_string(),
            Some(AdminSystemUpdateRelease {
                version: "v0.7.0-rc28".to_string(),
                release_url: None,
                release_notes: None,
                published_at: None,
                tarball_url: Some("https://github.com/fawney19/Aether/releases/download/v0.7.0-rc28/aether.tar.gz".to_string()),
                sha256sums_url: Some("https://github.com/fawney19/Aether/releases/download/v0.7.0-rc28/SHA256SUMS".to_string()),
            }),
            None,
        );

        assert_eq!(payload["has_update"], true);
        assert_eq!(payload["updatable"], true);
        assert_eq!(payload["update_blocker"], serde_json::Value::Null);
    }

    #[test]
    fn build_admin_system_check_update_payload_normalizes_v_prefix() {
        let payload = build_admin_system_check_update_payload_with_release(
            "0.7.0-rc28".to_string(),
            Some(AdminSystemUpdateRelease {
                version: "v0.7.0-rc28".to_string(),
                release_url: None,
                release_notes: None,
                published_at: None,
                tarball_url: None,
                sha256sums_url: None,
            }),
            None,
        );

        assert_eq!(payload["has_update"], false);
        assert_eq!(payload["error"], serde_json::Value::Null);
    }

    #[test]
    fn build_admin_system_check_update_payload_ignores_git_describe_build_on_latest_release() {
        let payload = build_admin_system_check_update_payload_with_release(
            "0.7.0-rc28-11-g63149fe2-dirty".to_string(),
            Some(AdminSystemUpdateRelease {
                version: "v0.7.0-rc28".to_string(),
                release_url: None,
                release_notes: None,
                published_at: None,
                tarball_url: None,
                sha256sums_url: None,
            }),
            None,
        );

        assert_eq!(payload["has_update"], false);
        assert_eq!(payload["error"], serde_json::Value::Null);
    }

    #[test]
    fn build_admin_system_check_update_payload_ignores_newer_local_release() {
        let payload = build_admin_system_check_update_payload_with_release(
            "0.7.0-rc29".to_string(),
            Some(AdminSystemUpdateRelease {
                version: "v0.7.0-rc28".to_string(),
                release_url: None,
                release_notes: None,
                published_at: None,
                tarball_url: None,
                sha256sums_url: None,
            }),
            None,
        );

        assert_eq!(payload["has_update"], false);
        assert_eq!(payload["error"], serde_json::Value::Null);
    }

    #[test]
    fn build_admin_system_check_update_payload_compares_rc_versions_numerically() {
        let payload = build_admin_system_check_update_payload_with_release(
            "0.7.0-rc9".to_string(),
            Some(AdminSystemUpdateRelease {
                version: "v0.7.0-rc10".to_string(),
                release_url: None,
                release_notes: None,
                published_at: None,
                tarball_url: None,
                sha256sums_url: None,
            }),
            None,
        );

        assert_eq!(payload["has_update"], true);
        assert_eq!(payload["error"], serde_json::Value::Null);
    }

    #[test]
    fn parse_admin_system_config_import_request_accepts_supported_versions() {
        for version in ADMIN_SYSTEM_CONFIG_SUPPORTED_VERSIONS {
            let parsed = parse_admin_system_config_import_request(
                json!({
                    "version": version,
                    "global_models": [],
                    "providers": [],
                })
                .to_string()
                .as_bytes(),
            )
            .expect("supported version should parse");

            assert_eq!(parsed.request.document.version, *version);
            assert_eq!(parsed.request.merge_mode, AdminImportMergeMode::Skip);
            assert!(parsed.request.document.oauth_providers.is_empty());
            assert!(parsed.request.document.system_configs.is_empty());
            assert!(parsed.request.document.ldap_config.is_none());
        }
    }

    #[test]
    fn system_config_debug_output_redacts_exported_credentials() {
        let secrets = [
            "debug-provider-api-key",
            "debug-provider-auth-config",
            "debug-proxy-password",
            "debug-ldap-password",
            "debug-oauth-client-secret",
            "debug-smtp-password",
        ];
        let document = serde_json::from_value::<AdminSystemConfigDocument>(json!({
            "version": "2.2",
            "providers": [{
                "name": "provider",
                "api_keys": [{
                    "api_key": secrets[0],
                    "auth_config": {"refresh_token": secrets[1]}
                }]
            }],
            "proxy_nodes": [{
                "name": "proxy",
                "proxy_password": secrets[2]
            }],
            "ldap_config": {
                "server_url": "ldaps://ldap.example.com",
                "bind_dn": "cn=admin,dc=example,dc=com",
                "bind_password": secrets[3],
                "base_dn": "dc=example,dc=com"
            },
            "oauth_providers": [{
                "provider_type": "linuxdo",
                "display_name": "Linux.do",
                "client_id": "client-id",
                "client_secret": secrets[4],
                "redirect_uri": "https://gateway.example/api/oauth/linuxdo/callback",
                "frontend_callback_url": "https://frontend.example/auth/callback"
            }],
            "system_configs": [{
                "key": "smtp_password",
                "value": secrets[5]
            }]
        }))
        .expect("system config document should deserialize");
        let rendered = format!("{document:?}");

        for secret in secrets {
            assert!(!rendered.contains(secret), "Debug output leaked {secret}");
        }
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn parse_admin_system_config_import_request_rejects_unknown_versions() {
        for version in ["1.9", "2.4"] {
            let err = parse_admin_system_config_import_request(
                json!({
                    "version": version,
                    "global_models": [],
                    "providers": [],
                })
                .to_string()
                .as_bytes(),
            )
            .expect_err("unknown versions should fail");

            assert_eq!(err.0, http::StatusCode::BAD_REQUEST);
            assert_eq!(
                err.1["detail"],
                format!(
                    "不支持的配置版本: {version}，支持的版本: {}",
                    ADMIN_SYSTEM_CONFIG_SUPPORTED_VERSIONS.join(", ")
                )
            );
        }
    }

    #[test]
    fn parse_admin_system_config_import_request_rejects_invalid_merge_mode() {
        let err = parse_admin_system_config_import_request(
            json!({
                "version": "2.2",
                "merge_mode": "replace_all",
            })
            .to_string()
            .as_bytes(),
        )
        .expect_err("invalid merge mode should fail");

        assert_eq!(err.0, http::StatusCode::BAD_REQUEST);
        assert_eq!(
            err.1["detail"],
            "merge_mode 仅支持 skip / overwrite / error"
        );
    }

    #[test]
    fn parse_admin_system_config_import_request_reports_field_path_for_shape_errors() {
        let err = parse_admin_system_config_import_request(
            json!({
                "version": "2.2",
                "global_models": [],
                "providers": [{
                    "name": "import-openai",
                    "endpoints": [{
                        "api_format": "openai:chat",
                        "base_url": "https://api.example.com",
                        "is_active": "yes"
                    }]
                }],
            })
            .to_string()
            .as_bytes(),
        )
        .expect_err("invalid endpoint shape should fail");

        assert_eq!(err.0, http::StatusCode::BAD_REQUEST);
        let detail = err.1["detail"].as_str().expect("detail should be a string");
        assert!(detail.contains("配置文件格式无效"));
        assert!(detail.contains("providers[0].endpoints[0].is_active"));
    }

    #[test]
    fn parse_admin_system_config_import_request_accepts_numeric_string_fields() {
        let parsed = parse_admin_system_config_import_request(
            json!({
                "version": "2.2",
                "global_models": [{
                    "name": "veo3.1",
                    "display_name": "Veo 3.1",
                    "usage_count": "42",
                    "default_price_per_request": "1.80000000",
                }],
                "providers": [{
                    "name": "undyapi",
                    "monthly_quota_usd": "12.50",
                    "stream_first_byte_timeout": "60",
                    "request_timeout": "120",
                    "models": [{
                        "global_model_name": "veo3.1",
                        "provider_model_name": "veo3.1",
                        "price_per_request": "0.70000000",
                    }]
                }],
            })
            .to_string()
            .as_bytes(),
        )
        .expect("numeric string fields from Python exports should parse");

        let global_model = &parsed.request.document.global_models[0];
        assert_eq!(global_model.usage_count, Some(42));
        assert_eq!(global_model.default_price_per_request, Some(1.8));

        let provider = &parsed.request.document.providers[0];
        assert_eq!(provider.monthly_quota_usd, Some(12.5));
        assert_eq!(provider.stream_first_byte_timeout, Some(60.0));
        assert_eq!(provider.request_timeout, Some(120.0));
        assert_eq!(provider.models[0].price_per_request, Some(0.7));
    }

    #[test]
    fn parse_admin_system_config_import_request_rejects_invalid_numeric_string_fields() {
        let err = parse_admin_system_config_import_request(
            json!({
                "version": "2.2",
                "global_models": [{
                    "name": "veo3.1",
                    "display_name": "Veo 3.1",
                    "default_price_per_request": "not-a-number",
                }],
                "providers": [],
            })
            .to_string()
            .as_bytes(),
        )
        .expect_err("invalid numeric string fields should fail");

        assert_eq!(err.0, http::StatusCode::BAD_REQUEST);
        let detail = err.1["detail"].as_str().expect("detail should be a string");
        assert!(detail.contains("配置文件格式无效"));
        assert!(detail.contains("default_price_per_request"));
    }

    #[test]
    fn resolve_admin_system_export_key_api_formats_uses_endpoint_fallback() {
        let provider_formats = vec!["openai:chat".to_string(), "claude:messages".to_string()];
        let resolved =
            resolve_admin_system_export_key_api_formats(None, &provider_formats, |value| {
                Some(value.to_string())
            });

        assert_eq!(resolved, provider_formats);
    }

    #[test]
    fn sensitive_admin_system_config_keys_are_case_insensitive() {
        assert!(is_sensitive_admin_system_config_key("smtp_password"));
        assert!(is_sensitive_admin_system_config_key("SMTP_PASSWORD"));
        assert!(is_sensitive_admin_system_config_key("turnstile_secret_key"));
        assert!(is_sensitive_admin_system_config_key("TURNSTILE_SECRET_KEY"));
        assert!(is_sensitive_admin_system_config_key(
            "module.server_chan_push.send_key"
        ));
        assert!(is_sensitive_admin_system_config_key(
            "module.important_notification.server_chan_send_key"
        ));
        assert!(is_sensitive_admin_system_config_key(
            "module.bark_push.device_key"
        ));
        assert!(!is_sensitive_admin_system_config_key("site_name"));
    }

    #[test]
    fn sensitive_admin_system_config_keys_use_canonical_storage_spelling() {
        assert_eq!(
            normalize_admin_system_config_key(" SMTP_PASSWORD "),
            "smtp_password"
        );
        assert_eq!(
            normalize_admin_system_config_key("BACKUP_S3_SECRET_ACCESS_KEY"),
            "backup_s3_secret_access_key"
        );
        assert_eq!(
            normalize_admin_system_config_key("MODULE.BARK_PUSH.DEVICE_KEY"),
            "module.bark_push.device_key"
        );
    }

    #[test]
    fn sensitive_admin_system_config_values_reject_non_strings() {
        for key in SENSITIVE_SYSTEM_CONFIG_KEYS {
            for value in [
                json!(123),
                json!(true),
                json!(["secret"]),
                json!({"secret": true}),
            ] {
                let body = serde_json::to_vec(&json!({ "value": value }))
                    .expect("sensitive config body should serialize");
                let error = parse_admin_system_config_update(key, &body)
                    .expect_err("non-string sensitive value must fail closed");
                assert_eq!(error.0, http::StatusCode::BAD_REQUEST, "key={key}");
            }
            assert!(parse_admin_system_config_update(key, br#"{"value":null}"#).is_ok());
            assert!(parse_admin_system_config_update(key, br#"{"value":"secret"}"#).is_ok());
        }
    }

    #[test]
    fn s3_backup_secret_access_key_is_sensitive() {
        assert!(is_sensitive_admin_system_config_key(
            "backup_s3_secret_access_key"
        ));
        assert!(is_sensitive_admin_system_config_key(
            "BACKUP_S3_SECRET_ACCESS_KEY"
        ));
        assert!(!is_sensitive_admin_system_config_key("backup_s3_bucket"));
    }

    #[test]
    fn s3_backup_defaults_match_admin_ui_contract() {
        assert_eq!(
            admin_system_config_default_value("backup_s3_scope"),
            Some(json!("data"))
        );
        assert_eq!(
            admin_system_config_default_value("backup_s3_schedule_unit"),
            Some(json!("days"))
        );
        assert_eq!(
            admin_system_config_default_value("backup_s3_schedule_interval"),
            Some(json!(1))
        );
        assert_eq!(
            admin_system_config_default_value("backup_s3_retention_count"),
            Some(json!(7))
        );
        assert_eq!(
            admin_system_config_default_value("backup_s3_path_style"),
            Some(json!(true))
        );
        assert_eq!(
            admin_system_config_default_value("backup_s3_user_agent"),
            Some(json!("rclone/v1.68.0"))
        );
    }

    #[test]
    fn cyber_continue_failover_defaults_to_disabled() {
        assert_eq!(
            admin_system_config_default_value("cyber_continue_failover"),
            Some(json!(false))
        );
    }

    #[test]
    fn cyber_continue_failover_update_requires_a_boolean() {
        let update =
            parse_admin_system_config_update("cyber_continue_failover", br#"{"value":true}"#)
                .expect("boolean Cyber failover setting should parse");
        assert_eq!(update.value, json!(true));

        assert!(parse_admin_system_config_update(
            "cyber_continue_failover",
            br#"{"value":"true"}"#,
        )
        .is_err());
    }

    #[test]
    fn smtp_control_fields_reject_header_and_command_injection() {
        for key in ["smtp_host", "smtp_from_email", "smtp_from_name"] {
            for value in [
                "safe\r\nX-Injected: yes",
                "safe\nMAIL FROM:<attacker>",
                "safe\0tail",
            ] {
                let body = serde_json::to_vec(&json!({ "value": value }))
                    .expect("SMTP config body should serialize");
                let error = parse_admin_system_config_update(key, &body)
                    .expect_err("SMTP control characters must be rejected");
                assert_eq!(error.0, http::StatusCode::BAD_REQUEST);
            }
        }

        let update =
            parse_admin_system_config_update("smtp_from_name", br#"{"value":" Aether Mail "}"#)
                .expect("normal SMTP display name should parse");
        assert_eq!(update.value, json!("Aether Mail"));
    }

    #[test]
    fn model_directives_update_accepts_legacy_and_current_config_shapes() {
        for body in [
            r#"{
                "value": {
                    "reasoning_effort": {
                        "enabled": true,
                        "api_formats": {
                            "openai:responses": {
                                "enabled": true,
                                "mappings": {
                                    "low": { "reasoning": { "effort": "low" } }
                                }
                            }
                        }
                    }
                }
            }"#,
            r#"{
                "value": {
                    "reasoning_effort": {
                        "api_formats": {
                            "openai:responses": {
                                "suffixes": ["low", "VendorFuture"],
                                "mappings": {
                                    "VendorFuture": { "reasoning": { "context": "all_turns" } }
                                }
                            }
                        }
                    }
                }
            }"#,
            r#"{
                "value": {
                    "reasoning_effort": {
                        "api_formats": {
                            "/v1/responses": {
                                "suffixes": ["low"],
                                "mappings": {}
                            }
                        }
                    }
                }
            }"#,
        ] {
            let update = parse_admin_system_config_update("model_directives", body.as_bytes())
                .expect("valid model directive config should parse");
            assert!(update.value["reasoning_effort"].is_object());
        }

        let reset = parse_admin_system_config_update("model_directives", br#"{"value":null}"#)
            .expect("null should reset model directives to defaults");
        assert_eq!(
            reset.value,
            aether_ai_formats::default_model_directives_config()
        );
    }

    #[test]
    fn model_directives_update_rejects_unsafe_mapping_shapes() {
        for body in [
            r#"{"value":[]}"#,
            r#"{"value":{"reasoning_effort":true}}"#,
            r#"{"value":{"reasoning_effort":{"enabled":"true"}}}"#,
            r#"{"value":{"reasoning_effort":{"api_formats":[]}}}"#,
            r#"{"value":{"reasoning_effort":{"api_formats":{"openai:responses":{"suffixes":["low",42]}}}}}"#,
            r#"{"value":{"reasoning_effort":{"api_formats":{"openai:responses":{"mappings":{"low":"replace-body"}}}}}}"#,
        ] {
            let error = parse_admin_system_config_update("model_directives", body.as_bytes())
                .expect_err("invalid model directive config should fail");
            assert_eq!(error.0, http::StatusCode::BAD_REQUEST);
            assert_eq!(error.1["detail"], "模型后缀参数配置格式无效");
        }
    }

    #[test]
    fn model_directives_update_rejects_ambiguous_aliases_and_suffixes() {
        for body in [
            r#"{"value":{"reasoning_effort":{"api_formats":{"openai:responses":true,"/v1/responses":false}}}}"#,
            r#"{"value":{"reasoning_effort":{"api_formats":{"openai:responses":{"suffixes":["low","LOW"]}}}}}"#,
            r#"{"value":{"reasoning_effort":{"api_formats":{"openai:responses":{"mappings":{"VendorFuture":{},"vendorfuture":{}}}}}}}"#,
        ] {
            let error = parse_admin_system_config_update("model_directives", body.as_bytes())
                .expect_err("ambiguous model directive config should fail");
            assert_eq!(error.0, http::StatusCode::BAD_REQUEST);
            assert_eq!(error.1["detail"], "模型后缀参数配置格式无效");
        }
    }

    #[test]
    fn enable_model_directives_update_requires_a_boolean() {
        let update =
            parse_admin_system_config_update("enable_model_directives", br#"{"value":true}"#)
                .expect("boolean model directives flag should parse");
        assert_eq!(update.value, json!(true));
        assert!(parse_admin_system_config_update(
            "enable_model_directives",
            br#"{"value":"true"}"#,
        )
        .is_err());
    }

    #[test]
    fn legacy_notification_email_config_key_normalizes_to_important_notification() {
        assert_eq!(
            normalize_admin_system_config_key("module.notification_email.enabled"),
            "module.important_notification.enabled"
        );
        assert_eq!(
            admin_system_config_delete_keys("module.important_notification.enabled"),
            vec![
                "module.important_notification.enabled".to_string(),
                "module.notification_email.enabled".to_string(),
            ]
        );
    }

    #[test]
    fn s3_backup_secret_detail_is_write_only() {
        let payload = build_admin_system_config_detail_payload(
            "backup_s3_secret_access_key",
            Some(json!("encrypted-secret")),
        )
        .expect("sensitive backup key should render");

        assert_eq!(payload["key"], json!("backup_s3_secret_access_key"));
        assert_eq!(payload["value"], serde_json::Value::Null);
        assert_eq!(payload["is_set"], json!(true));
    }

    #[test]
    fn legacy_server_chan_config_keys_normalize_to_push_module() {
        assert_eq!(
            normalize_admin_system_config_key("module.important_notification.server_chan_send_key"),
            "module.server_chan_push.send_key"
        );
        assert_eq!(
            admin_system_config_delete_keys("module.server_chan_push.send_key"),
            vec![
                "module.server_chan_push.send_key".to_string(),
                "module.important_notification.server_chan_send_key".to_string(),
            ]
        );
    }

    #[test]
    fn system_config_list_normalizes_legacy_keys_and_prefers_canonical_rows() {
        let entries = vec![
            StoredSystemConfigEntry {
                key: "module.important_notification.server_chan_send_key".to_string(),
                value: json!("legacy-secret"),
                description: None,
                updated_at_unix_secs: None,
            },
            StoredSystemConfigEntry {
                key: "module.server_chan_push.send_key".to_string(),
                value: json!("canonical-secret"),
                description: None,
                updated_at_unix_secs: None,
            },
            StoredSystemConfigEntry {
                key: "module.notification_email.enabled".to_string(),
                value: json!(true),
                description: None,
                updated_at_unix_secs: None,
            },
        ];

        let payload = build_admin_system_configs_payload(&entries);
        let rows = payload.as_array().expect("config list should be an array");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| {
            row["key"] == json!("module.server_chan_push.send_key") && row["is_set"] == json!(true)
        }));
        assert!(rows.iter().any(|row| {
            row["key"] == json!("module.important_notification.enabled")
                && row["value"] == json!(true)
        }));
    }

    #[test]
    fn notification_service_items_are_normalized() {
        let update = parse_admin_system_config_update(
            "module.important_notification.items",
            r#"{
                "value": [
                    {
                        "key": "user_balance_low",
                        "name": " 用户余额不足 ",
                        "enabled": true,
                        "channel": "serverchan",
                        "title_template": " 余额提醒 ",
                        "markdown_template": " {body} ",
                        "text_template": null,
                        "user_email_enabled": true,
                        "system": true
                    }
                ]
            }"#
            .as_bytes(),
        )
        .expect("items should parse");

        assert_eq!(update.normalized_key, "module.important_notification.items");
        assert_eq!(update.value[0]["channel"], json!("server_chan"));
        assert_eq!(update.value[0]["name"], json!("用户余额不足"));
        assert_eq!(update.value[0]["text_template"], json!(""));
        assert_eq!(update.value[0]["user_email_enabled"], json!(true));
    }

    #[test]
    fn bark_push_config_values_are_normalized() {
        let update = parse_admin_system_config_update(
            "module.bark_push.server_url",
            r#"{ "value": " https://api.day.app/ " }"#.as_bytes(),
        )
        .expect("server url should parse");

        assert_eq!(update.normalized_key, "module.bark_push.server_url");
        assert_eq!(update.value, json!("https://api.day.app"));

        let err = parse_admin_system_config_update(
            "module.bark_push.server_url",
            r#"{ "value": "api.day.app" }"#.as_bytes(),
        )
        .expect_err("server url without scheme should fail");
        assert_eq!(err.0, http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn build_admin_system_config_detail_masks_turnstile_secret_key() {
        let payload = build_admin_system_config_detail_payload(
            "turnstile_secret_key",
            Some(json!("encrypted-turnstile-secret")),
        )
        .expect("turnstile secret detail should build");

        assert_eq!(payload["key"], "turnstile_secret_key");
        assert_eq!(payload["value"], serde_json::Value::Null);
        assert_eq!(payload["is_set"], json!(true));
    }
}
