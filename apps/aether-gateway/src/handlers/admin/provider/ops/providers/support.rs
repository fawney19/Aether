use serde::Deserialize;
use std::collections::BTreeMap;

pub(super) const ADMIN_PROVIDER_OPS_SENSITIVE_FIELDS: &[&str] = &[
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
pub(super) struct AdminProviderOpsConnectRequest {
    #[serde(default)]
    pub(crate) credentials: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AdminProviderOpsExecuteActionRequest {
    #[serde(default)]
    pub(crate) config: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone)]
pub(super) struct AdminProviderOpsCheckinOutcome {
    pub(crate) success: Option<bool>,
    pub(crate) message: String,
    pub(crate) cookie_expired: bool,
}

fn default_admin_provider_ops_architecture_id() -> String {
    "generic_api".to_string()
}

fn default_admin_provider_ops_action_enabled() -> bool {
    true
}
