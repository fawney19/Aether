use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionRuntimeAuthContext {
    pub user_id: String,
    pub api_key_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_name: Option<String>,
    #[serde(default = "default_api_key_billing_multiplier")]
    pub api_key_billing_multiplier: f64,
    pub balance_remaining: Option<f64>,
    pub access_allowed: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub api_key_is_standalone: bool,
}

fn default_api_key_billing_multiplier() -> f64 {
    1.0
}
