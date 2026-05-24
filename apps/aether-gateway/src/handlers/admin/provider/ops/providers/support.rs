use serde::Deserialize;
use std::collections::BTreeMap;

pub(super) use aether_admin::provider::ops::ProviderOpsCheckinOutcome as AdminProviderOpsCheckinOutcome;

pub(super) const ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS: &[&str] = &[
    "api_key",
    "password",
    "refresh_token",
    "_cached_access_token",
    "session_token",
    "session_cookie",
    "token_cookie",
    "auth_cookie",
    "cookie_string",
    "cookie",
];
pub(super) const ADMIN_PROVIDER_OPS_CONNECT_RUST_ONLY_MESSAGE: &str =
    "Provider 连接仅支持 Rust execution runtime";
pub(super) const ADMIN_PROVIDER_OPS_ACTION_RUST_ONLY_MESSAGE: &str =
    "Provider 操作仅支持 Rust execution runtime";
pub(super) const ADMIN_PROVIDER_OPS_VERIFY_RUST_ONLY_MESSAGE: &str =
    "认证验证仅支持 Rust execution runtime";

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOpsSaveConfigRequest {
    #[serde(default = "default_admin_provider_ops_architecture_id")]
    pub(crate) architecture_id: String,
    #[serde(default)]
    pub(crate) base_url: Option<String>,
    pub(crate) connector: AdminProviderOpsConnectorConfigRequest,
    #[serde(default)]
    pub(crate) actions: BTreeMap<String, AdminProviderOpsActionConfigRequest>,
    #[serde(default)]
    pub(crate) schedule: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) quota_alert: Option<AdminProviderOpsQuotaAlertConfigRequest>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOpsConnectorConfigRequest {
    pub(crate) auth_type: String,
    #[serde(default)]
    pub(crate) config: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub(crate) credentials: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOpsActionConfigRequest {
    #[serde(default = "default_admin_provider_ops_action_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) config: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOpsQuotaAlertConfigRequest {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default, deserialize_with = "deserialize_optional_f64_from_number")]
    pub(crate) threshold_amount: Option<f64>,
    #[serde(default)]
    pub(crate) fetch_interval_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOpsConnectRequest {
    #[serde(default)]
    pub(crate) credentials: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOpsExecuteActionRequest {
    #[serde(default)]
    pub(crate) config: Option<serde_json::Map<String, serde_json::Value>>,
}

fn default_admin_provider_ops_architecture_id() -> String {
    "generic_api".to_string()
}

fn default_admin_provider_ops_action_enabled() -> bool {
    true
}

fn deserialize_optional_f64_from_number<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(number)) => number
            .as_f64()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("expected a finite number")),
        Some(serde_json::Value::String(raw)) => raw
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom("expected a finite number or numeric string")),
        Some(_) => Err(serde::de::Error::custom(
            "expected a finite number or numeric string",
        )),
    }
}
