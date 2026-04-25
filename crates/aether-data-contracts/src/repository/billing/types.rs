use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredBillingModelContext {
    pub provider_id: String,
    pub provider_billing_type: Option<String>,
    pub provider_api_key_id: Option<String>,
    pub provider_api_key_rate_multipliers: Option<Value>,
    pub provider_api_key_cache_ttl_minutes: Option<i64>,
    pub global_model_id: String,
    pub global_model_name: String,
    pub global_model_config: Option<Value>,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub model_id: Option<String>,
    pub model_provider_model_name: Option<String>,
    pub model_config: Option<Value>,
    pub model_price_per_request: Option<f64>,
    pub model_tiered_pricing: Option<Value>,
}

impl StoredBillingModelContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider_id: String,
        provider_billing_type: Option<String>,
        provider_api_key_id: Option<String>,
        provider_api_key_rate_multipliers: Option<Value>,
        provider_api_key_cache_ttl_minutes: Option<i64>,
        global_model_id: String,
        global_model_name: String,
        global_model_config: Option<Value>,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        model_id: Option<String>,
        model_provider_model_name: Option<String>,
        model_config: Option<Value>,
        model_price_per_request: Option<f64>,
        model_tiered_pricing: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "billing.provider_id is empty".to_string(),
            ));
        }
        if global_model_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "billing.global_model_id is empty".to_string(),
            ));
        }
        if global_model_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "billing.global_model_name is empty".to_string(),
            ));
        }
        Ok(Self {
            provider_id,
            provider_billing_type,
            provider_api_key_id,
            provider_api_key_rate_multipliers,
            provider_api_key_cache_ttl_minutes,
            global_model_id,
            global_model_name,
            global_model_config,
            default_price_per_request,
            default_tiered_pricing,
            model_id,
            model_provider_model_name,
            model_config,
            model_price_per_request,
            model_tiered_pricing,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminBillingRuleRecord {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub global_model_id: Option<String>,
    pub model_id: Option<String>,
    pub expression: String,
    pub variables: Value,
    pub dimension_mappings: Value,
    pub is_enabled: bool,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminBillingRuleWriteInput {
    pub name: String,
    pub task_type: String,
    pub global_model_id: Option<String>,
    pub model_id: Option<String>,
    pub expression: String,
    pub variables: Value,
    pub dimension_mappings: Value,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminBillingCollectorRecord {
    pub id: String,
    pub api_format: String,
    pub task_type: String,
    pub dimension_name: String,
    pub source_type: String,
    pub source_path: Option<String>,
    pub value_type: String,
    pub transform_expression: Option<String>,
    pub default_value: Option<String>,
    pub priority: i32,
    pub is_enabled: bool,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminBillingCollectorWriteInput {
    pub api_format: String,
    pub task_type: String,
    pub dimension_name: String,
    pub source_type: String,
    pub source_path: Option<String>,
    pub value_type: String,
    pub transform_expression: Option<String>,
    pub default_value: Option<String>,
    pub priority: i32,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminBillingPresetApplyResult {
    pub preset: String,
    pub mode: String,
    pub created: u64,
    pub updated: u64,
    pub skipped: u64,
    pub errors: Vec<String>,
}

#[async_trait]
pub trait BillingReadRepository: Send + Sync {
    async fn find_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, crate::DataLayerError>;

    async fn find_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, crate::DataLayerError> {
        let _ = (provider_id, provider_api_key_id, model_id);
        Ok(None)
    }
}
