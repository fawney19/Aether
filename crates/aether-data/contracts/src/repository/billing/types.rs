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

#[derive(Debug, Clone, PartialEq)]
pub enum AdminBillingMutationOutcome<T> {
    Applied(T),
    NotFound,
    Invalid(String),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PaymentGatewayConfigRecord {
    pub provider: String,
    pub enabled: bool,
    pub endpoint_url: String,
    pub callback_base_url: Option<String>,
    pub merchant_id: String,
    pub merchant_key_encrypted: Option<String>,
    pub pay_currency: String,
    pub usd_exchange_rate: f64,
    pub min_recharge_usd: f64,
    pub channels_json: Value,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaymentGatewayConfigWriteInput {
    pub provider: String,
    pub enabled: bool,
    pub endpoint_url: String,
    pub callback_base_url: Option<String>,
    pub merchant_id: String,
    pub merchant_key_encrypted: Option<String>,
    pub preserve_existing_secret: bool,
    pub pay_currency: String,
    pub usd_exchange_rate: f64,
    pub min_recharge_usd: f64,
    pub channels_json: Value,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BillingPlanRecord {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub price_amount: f64,
    pub price_currency: String,
    pub duration_unit: String,
    pub duration_value: i64,
    pub enabled: bool,
    pub sort_order: i64,
    pub max_active_per_user: i64,
    pub purchase_limit_scope: String,
    pub entitlements_json: Value,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BillingPlanWriteInput {
    pub title: String,
    pub description: Option<String>,
    pub price_amount: f64,
    pub price_currency: String,
    pub duration_unit: String,
    pub duration_value: i64,
    pub enabled: bool,
    pub sort_order: i64,
    pub max_active_per_user: i64,
    pub purchase_limit_scope: String,
    pub entitlements_json: Value,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserPlanEntitlementRecord {
    pub id: String,
    pub user_id: String,
    pub plan_id: String,
    pub payment_order_id: String,
    pub status: String,
    pub starts_at_unix_secs: u64,
    pub expires_at_unix_secs: u64,
    pub entitlements_snapshot: Value,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserDailyQuotaAvailabilityRecord {
    pub has_active_daily_quota: bool,
    pub total_quota_usd: f64,
    pub used_usd: f64,
    pub remaining_usd: f64,
    pub allow_wallet_overage: bool,
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

    async fn admin_billing_enabled_default_value_exists(
        &self,
        api_format: &str,
        task_type: &str,
        dimension_name: &str,
        existing_id: Option<&str>,
    ) -> Result<Option<bool>, crate::DataLayerError> {
        let _ = (api_format, task_type, dimension_name, existing_id);
        Ok(None)
    }

    async fn create_admin_billing_rule(
        &self,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingRuleRecord>, crate::DataLayerError> {
        let _ = input;
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn list_admin_billing_rules(
        &self,
        task_type: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingRuleRecord>, u64)>, crate::DataLayerError> {
        let _ = (task_type, is_enabled, page, page_size);
        Ok(None)
    }

    async fn find_admin_billing_rule(
        &self,
        rule_id: &str,
    ) -> Result<Option<AdminBillingRuleRecord>, crate::DataLayerError> {
        let _ = rule_id;
        Ok(None)
    }

    async fn update_admin_billing_rule(
        &self,
        rule_id: &str,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingRuleRecord>, crate::DataLayerError> {
        let _ = (rule_id, input);
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn create_admin_billing_collector(
        &self,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingCollectorRecord>, crate::DataLayerError>
    {
        let _ = input;
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn list_admin_billing_collectors(
        &self,
        api_format: Option<&str>,
        task_type: Option<&str>,
        dimension_name: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingCollectorRecord>, u64)>, crate::DataLayerError> {
        let _ = (
            api_format,
            task_type,
            dimension_name,
            is_enabled,
            page,
            page_size,
        );
        Ok(None)
    }

    async fn find_admin_billing_collector(
        &self,
        collector_id: &str,
    ) -> Result<Option<AdminBillingCollectorRecord>, crate::DataLayerError> {
        let _ = collector_id;
        Ok(None)
    }

    async fn update_admin_billing_collector(
        &self,
        collector_id: &str,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<AdminBillingMutationOutcome<AdminBillingCollectorRecord>, crate::DataLayerError>
    {
        let _ = (collector_id, input);
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn apply_admin_billing_preset(
        &self,
        preset: &str,
        mode: &str,
        collectors: &[AdminBillingCollectorWriteInput],
    ) -> Result<AdminBillingMutationOutcome<AdminBillingPresetApplyResult>, crate::DataLayerError>
    {
        let _ = (preset, mode, collectors);
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn find_payment_gateway_config(
        &self,
        provider: &str,
    ) -> Result<Option<PaymentGatewayConfigRecord>, crate::DataLayerError> {
        let _ = provider;
        Ok(None)
    }

    async fn upsert_payment_gateway_config(
        &self,
        input: &PaymentGatewayConfigWriteInput,
    ) -> Result<AdminBillingMutationOutcome<PaymentGatewayConfigRecord>, crate::DataLayerError>
    {
        let _ = input;
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn list_billing_plans(
        &self,
        include_disabled: bool,
    ) -> Result<Option<Vec<BillingPlanRecord>>, crate::DataLayerError> {
        let _ = include_disabled;
        Ok(None)
    }

    async fn find_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<Option<BillingPlanRecord>, crate::DataLayerError> {
        let _ = plan_id;
        Ok(None)
    }

    async fn create_billing_plan(
        &self,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, crate::DataLayerError> {
        let _ = input;
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn update_billing_plan(
        &self,
        plan_id: &str,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, crate::DataLayerError> {
        let _ = (plan_id, input);
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn set_billing_plan_enabled(
        &self,
        plan_id: &str,
        enabled: bool,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, crate::DataLayerError> {
        let _ = (plan_id, enabled);
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn delete_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<AdminBillingMutationOutcome<()>, crate::DataLayerError> {
        let _ = plan_id;
        Ok(AdminBillingMutationOutcome::Unavailable)
    }

    async fn list_user_plan_entitlements(
        &self,
        user_id: &str,
    ) -> Result<Option<Vec<UserPlanEntitlementRecord>>, crate::DataLayerError> {
        let _ = user_id;
        Ok(None)
    }

    async fn find_user_daily_quota_availability(
        &self,
        user_id: &str,
    ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, crate::DataLayerError> {
        let _ = user_id;
        Ok(None)
    }
}
