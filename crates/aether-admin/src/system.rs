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
use serde::{de, de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

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

pub const ADMIN_SYSTEM_CONFIG_EXPORT_VERSION: &str = "2.2";
pub const ADMIN_SYSTEM_CONFIG_SUPPORTED_VERSIONS: &[&str] = &[ADMIN_SYSTEM_CONFIG_EXPORT_VERSION];
pub const ADMIN_SYSTEM_USERS_EXPORT_VERSION: &str = "1.4";
pub const ADMIN_SYSTEM_USERS_SUPPORTED_VERSIONS: &[&str] =
    &["1.3", ADMIN_SYSTEM_USERS_EXPORT_VERSION];
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

fn default_true() -> bool {
    true
}

const CHAT_PII_REDACTION_ENTITY_KEYS: &[&str] = &[
    "email",
    "cn_phone",
    "global_phone",
    "cn_id",
    "payment_card",
    "ipv4",
    "ipv6",
    "api_key",
    "access_token",
    "secret_key",
    "bearer_token",
    "jwt",
];

fn chat_pii_redaction_default_entities() -> serde_json::Value {
    json!(CHAT_PII_REDACTION_ENTITY_KEYS)
}

fn invalid_request(detail: impl Into<String>) -> (http::StatusCode, serde_json::Value) {
    (
        http::StatusCode::BAD_REQUEST,
        json!({ "detail": detail.into() }),
    )
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
            .ok_or_else(|| de::Error::custom("expected a finite number")),
        Some(_) => Err(de::Error::custom("expected a finite number")),
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigProviderKey {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub auth_type: Option<String>,
    #[serde(default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[serde(default)]
    pub proxy_username: Option<String>,
    #[serde(default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigLdap {
    pub server_url: String,
    pub bind_dn: String,
    #[serde(default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigOAuthProvider {
    pub provider_type: String,
    pub display_name: String,
    pub client_id: String,
    #[serde(default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigEntry {
    pub key: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigDocument {
    pub version: String,
    #[serde(default)]
    pub exported_at: String,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminSystemConfigImportRequest {
    #[serde(flatten)]
    pub document: AdminSystemConfigDocument,
    #[serde(default)]
    pub merge_mode: AdminImportMergeMode,
}

#[derive(Debug, Clone)]
pub struct ParsedAdminSystemConfigImportRequest {
    pub request: AdminSystemConfigImportRequest,
    pub root: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct ParsedAdminSystemConfigObject<T> {
    pub raw: Map<String, Value>,
    pub value: T,
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
const SENSITIVE_SYSTEM_CONFIG_KEYS: &[&str] = &["smtp_password"];
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
        value: "gemini:embedding",
        label: "Gemini Embedding",
        default_path: "/v1/embeddings",
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
];

pub fn build_admin_system_check_update_payload(current_version: String) -> serde_json::Value {
    json!({
        "current_version": current_version,
        "latest_version": serde_json::Value::Null,
        "has_update": false,
        "release_url": serde_json::Value::Null,
        "release_notes": serde_json::Value::Null,
        "published_at": serde_json::Value::Null,
        "error": "检查更新需要 Rust 管理后端",
    })
}

pub fn build_admin_system_stats_payload(
    total_users: u64,
    active_users: u64,
    total_providers: u64,
    active_providers: u64,
    total_api_keys: u64,
    total_requests: u64,
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
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };
    let html = match payload.get("html") {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
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
        Some(bytes) => match serde_json::from_slice::<serde_json::Value>(bytes) {
            Ok(serde_json::Value::Object(payload)) => Ok(payload),
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
    !config.server_url.trim().is_empty()
        && !config.bind_dn.trim().is_empty()
        && !config.base_dn.trim().is_empty()
        && config
            .bind_password_encrypted
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
}

pub fn build_admin_module_validation_result(
    module_name: &str,
    oauth_providers: &[StoredOAuthProviderModuleConfig],
    ldap_config: Option<&StoredLdapModuleConfig>,
    gemini_files_has_capable_key: bool,
    smtp_configured: bool,
) -> (bool, Option<String>) {
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
            if config.server_url.trim().is_empty() {
                return (false, Some("请配置 LDAP 服务器地址".to_string()));
            }
            if config.bind_dn.trim().is_empty() {
                return (false, Some("请配置绑定 DN".to_string()));
            }
            if config.base_dn.trim().is_empty() {
                return (false, Some("请配置搜索基准 DN".to_string()));
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
        "notification_email" => {
            if smtp_configured {
                (true, None)
            } else {
                (false, Some("请先完成邮件配置（SMTP）".to_string()))
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
        "management_tokens" | "model_directives" | "proxy_nodes" => "healthy",
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
        REQUEST_RECORD_LEVEL_KEY => Some(json!("full")),
        "max_request_body_size" => Some(json!(5_242_880)),
        "max_response_body_size" => Some(json!(5_242_880)),
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
        "provider_priority_mode" => Some(json!("provider")),
        "scheduling_mode" => Some(json!("cache_affinity")),
        "auto_delete_expired_keys" => Some(json!(false)),
        "email_suffix_mode" => Some(json!("none")),
        "email_suffix_list" => Some(json!([])),
        "enable_format_conversion" => Some(json!(false)),
        "enable_model_directives" => Some(json!(false)),
        "model_directives" => Some(json!({
            "reasoning_effort": {
                "enabled": true,
                "api_formats": {
                    "openai:chat": {
                        "enabled": true,
                        "mappings": {
                            "low": { "reasoning_effort": "low" },
                            "medium": { "reasoning_effort": "medium" },
                            "high": { "reasoning_effort": "high" },
                            "xhigh": { "reasoning_effort": "xhigh" },
                            "max": { "reasoning_effort": "xhigh" }
                        }
                    },
                    "openai:responses": {
                        "enabled": true,
                        "mappings": {
                            "low": { "reasoning": { "effort": "low" } },
                            "medium": { "reasoning": { "effort": "medium" } },
                            "high": { "reasoning": { "effort": "high" } },
                            "xhigh": { "reasoning": { "effort": "xhigh" } },
                            "max": { "reasoning": { "effort": "xhigh" } }
                        }
                    },
                    "openai:responses:compact": {
                        "enabled": true,
                        "mappings": {
                            "low": { "reasoning": { "effort": "low" } },
                            "medium": { "reasoning": { "effort": "medium" } },
                            "high": { "reasoning": { "effort": "high" } },
                            "xhigh": { "reasoning": { "effort": "xhigh" } },
                            "max": { "reasoning": { "effort": "xhigh" } }
                        }
                    },
                    "claude:messages": {
                        "enabled": true,
                        "mappings": {
                            "low": { "thinking": { "type": "enabled", "budget_tokens": 1024 } },
                            "medium": { "thinking": { "type": "enabled", "budget_tokens": 4096 } },
                            "high": { "thinking": { "type": "enabled", "budget_tokens": 8192 } },
                            "xhigh": { "thinking": { "type": "enabled", "budget_tokens": 16384 } },
                            "max": { "thinking": { "type": "enabled", "budget_tokens": 32768 } }
                        }
                    },
                    "gemini:generate_content": {
                        "enabled": true,
                        "mappings": {
                            "low": { "generationConfig": { "thinkingConfig": { "thinkingBudget": 1024 } } },
                            "medium": { "generationConfig": { "thinkingConfig": { "thinkingBudget": 4096 } } },
                            "high": { "generationConfig": { "thinkingConfig": { "thinkingBudget": 8192 } } },
                            "xhigh": { "generationConfig": { "thinkingConfig": { "thinkingBudget": 16384 } } },
                            "max": { "generationConfig": { "thinkingConfig": { "thinkingBudget": -1 } } }
                        }
                    }
                }
            }
        })),
        "keep_priority_on_conversion" => Some(json!(false)),
        "audit_log_retention_days" => Some(json!(30)),
        "enable_db_maintenance" => Some(json!(true)),
        "system_proxy_node_id" => Some(serde_json::Value::Null),
        "smtp_host" => Some(serde_json::Value::Null),
        "smtp_port" => Some(json!(587)),
        "smtp_user" => Some(serde_json::Value::Null),
        "smtp_password" => Some(serde_json::Value::Null),
        "smtp_use_tls" => Some(json!(true)),
        "smtp_use_ssl" => Some(json!(false)),
        "smtp_from_email" => Some(serde_json::Value::Null),
        "smtp_from_name" => Some(json!("Aether")),
        "enable_oauth_token_refresh" => Some(json!(true)),
        "module.chat_pii_redaction.enabled" => Some(json!(false)),
        "module.chat_pii_redaction.provider_scope" => Some(json!("selected_providers")),
        "module.chat_pii_redaction.entities" => Some(chat_pii_redaction_default_entities()),
        "module.chat_pii_redaction.cache_ttl_seconds" => Some(json!(300)),
        "module.chat_pii_redaction.inject_model_instruction" => Some(json!(true)),
        _ => None,
    }
}

pub fn build_admin_system_configs_payload(
    entries: &[StoredSystemConfigEntry],
) -> serde_json::Value {
    let has_request_record_level = entries
        .iter()
        .any(|entry| entry.key == REQUEST_RECORD_LEVEL_KEY);
    json!(entries
        .iter()
        .filter_map(|entry| {
            if entry.key == LEGACY_REQUEST_LOG_LEVEL_KEY && has_request_record_level {
                return None;
            }
            let key = if entry.key == LEGACY_REQUEST_LOG_LEVEL_KEY {
                REQUEST_RECORD_LEVEL_KEY
            } else {
                entry.key.as_str()
            };
            Some(build_admin_system_config_list_item(
                key,
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
        "module.chat_pii_redaction.enabled"
        | "module.chat_pii_redaction.inject_model_instruction" => match value.as_bool() {
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
        "module.chat_pii_redaction.provider_scope" => match value.as_str().map(str::trim) {
            Some("all_providers" | "selected_providers") => {
                value = json!(value.as_str().unwrap().trim());
            }
            Some(_) => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
            None if value.is_null() => value = json!("selected_providers"),
            None => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        },
        "module.chat_pii_redaction.entities" => match value.as_array() {
            Some(raw_entities) => {
                let requested = raw_entities
                    .iter()
                    .map(|entity| entity.as_str().map(str::trim))
                    .collect::<Option<BTreeSet<_>>>()
                    .ok_or_else(|| {
                        (
                            http::StatusCode::BAD_REQUEST,
                            json!({ "detail": "请求数据验证失败" }),
                        )
                    })?;
                let allowed = CHAT_PII_REDACTION_ENTITY_KEYS
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>();
                if !requested.is_subset(&allowed) {
                    return Err((
                        http::StatusCode::BAD_REQUEST,
                        json!({ "detail": "请求数据验证失败" }),
                    ));
                }
                value = json!(CHAT_PII_REDACTION_ENTITY_KEYS
                    .iter()
                    .copied()
                    .filter(|entity| requested.contains(entity))
                    .collect::<Vec<_>>());
            }
            None if value.is_null() => value = chat_pii_redaction_default_entities(),
            None => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        },
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
        _ => {}
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
        ("proxy_metadata".to_string(), json!(node.proxy_metadata)),
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
        payload.insert(
            "proxy_password".to_string(),
            json!(mask_admin_proxy_node_password(
                node.proxy_password.as_deref()
            )),
        );
    }

    serde_json::Value::Object(payload)
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

fn mask_admin_proxy_node_password(password: Option<&str>) -> Option<String> {
    let password = password?;
    if password.is_empty() {
        return None;
    }
    if password.len() < 8 {
        return Some("****".to_string());
    }
    Some(format!(
        "{}****{}",
        &password[..2],
        &password[password.len() - 2..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_admin_system_config_import_request_accepts_supported_versions() {
        let parsed = parse_admin_system_config_import_request(
            json!({
                "version": ADMIN_SYSTEM_CONFIG_EXPORT_VERSION,
                "global_models": [],
                "providers": [],
            })
            .to_string()
            .as_bytes(),
        )
        .expect("current version should parse");

        assert_eq!(
            parsed.request.document.version,
            ADMIN_SYSTEM_CONFIG_EXPORT_VERSION
        );
        assert_eq!(parsed.request.merge_mode, AdminImportMergeMode::Skip);
        assert!(parsed.request.document.oauth_providers.is_empty());
        assert!(parsed.request.document.system_configs.is_empty());
        assert!(parsed.request.document.ldap_config.is_none());
    }

    #[test]
    fn parse_admin_system_config_import_request_rejects_removed_versions() {
        for version in ["2.0", "2.1"] {
            let err = parse_admin_system_config_import_request(
                json!({
                    "version": version,
                    "global_models": [],
                    "providers": [],
                })
                .to_string()
                .as_bytes(),
            )
            .expect_err("removed versions should fail");

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
    fn parse_admin_system_config_import_request_rejects_numeric_string_fields() {
        let err = parse_admin_system_config_import_request(
            json!({
                "version": "2.2",
                "global_models": [{
                    "name": "veo3.1",
                    "display_name": "Veo 3.1",
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
        .expect_err("numeric string fields should fail");

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
        assert!(!is_sensitive_admin_system_config_key("site_name"));
    }
}
