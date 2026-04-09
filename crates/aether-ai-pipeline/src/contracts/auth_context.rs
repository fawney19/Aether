use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExecutionRuntimeAuthContext {
    pub user_id: String,
    pub api_key_id: String,
    pub balance_remaining: Option<f64>,
    pub access_allowed: bool,
}
