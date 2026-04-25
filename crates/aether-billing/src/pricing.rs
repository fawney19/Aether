use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingModelPricingSnapshot {
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

impl BillingModelPricingSnapshot {
    pub fn effective_tiered_pricing(&self) -> Option<&Value> {
        self.model_tiered_pricing
            .as_ref()
            .filter(|value| has_tiered_pricing_tiers(value))
            .or(self.default_tiered_pricing.as_ref())
    }

    pub fn effective_price_per_request(&self) -> Option<f64> {
        self.model_price_per_request
            .or(self.default_price_per_request)
    }

    pub fn pricing_source(&self) -> &'static str {
        if self
            .model_tiered_pricing
            .as_ref()
            .is_some_and(has_tiered_pricing_tiers)
            || self.model_price_per_request.is_some()
        {
            "provider_override"
        } else if self.default_tiered_pricing.is_some() || self.default_price_per_request.is_some()
        {
            "global_default"
        } else {
            "unpriced"
        }
    }

    pub fn is_free_tier(&self) -> bool {
        self.provider_billing_type
            .as_deref()
            .map(|value| value.eq_ignore_ascii_case("free_tier"))
            .unwrap_or(false)
    }

    pub fn rate_multiplier_for_api_format(&self, api_format: Option<&str>) -> f64 {
        let Some(api_format) = api_format.map(str::trim).filter(|value| !value.is_empty()) else {
            return 1.0;
        };
        let normalized = api_format.to_ascii_lowercase();
        let Some(mapping) = self
            .provider_api_key_rate_multipliers
            .as_ref()
            .and_then(Value::as_object)
        else {
            return 1.0;
        };
        mapping
            .get(&normalized)
            .and_then(|value| value.as_f64())
            .unwrap_or(1.0)
    }
}

fn has_tiered_pricing_tiers(value: &Value) -> bool {
    value
        .get("tiers")
        .and_then(Value::as_array)
        .is_some_and(|tiers| !tiers.is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::BillingModelPricingSnapshot;

    fn snapshot(
        model_tiered_pricing: Option<serde_json::Value>,
        default_tiered_pricing: Option<serde_json::Value>,
    ) -> BillingModelPricingSnapshot {
        BillingModelPricingSnapshot {
            provider_id: "provider-1".to_string(),
            provider_billing_type: None,
            provider_api_key_id: None,
            provider_api_key_rate_multipliers: None,
            provider_api_key_cache_ttl_minutes: None,
            global_model_id: "global-model-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_config: None,
            default_price_per_request: None,
            default_tiered_pricing,
            model_id: Some("model-1".to_string()),
            model_provider_model_name: Some("gpt-5-upstream".to_string()),
            model_config: None,
            model_price_per_request: None,
            model_tiered_pricing,
        }
    }

    #[test]
    fn empty_provider_tiered_pricing_inherits_global_default() {
        let default_pricing =
            json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]});
        let pricing = snapshot(Some(json!({})), Some(default_pricing.clone()));

        assert_eq!(pricing.effective_tiered_pricing(), Some(&default_pricing));

        let pricing = snapshot(Some(json!({"tiers": []})), Some(default_pricing.clone()));

        assert_eq!(pricing.effective_tiered_pricing(), Some(&default_pricing));
    }

    #[test]
    fn populated_provider_tiered_pricing_overrides_global_default() {
        let provider_pricing =
            json!({"tiers":[{"up_to":null,"input_price_per_1m":1.0,"output_price_per_1m":2.0}]});
        let default_pricing =
            json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]});
        let pricing = snapshot(Some(provider_pricing.clone()), Some(default_pricing));

        assert_eq!(pricing.effective_tiered_pricing(), Some(&provider_pricing));
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingUsageInput {
    pub task_type: String,
    pub api_format: Option<String>,
    pub request_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_creation_ephemeral_5m_tokens: i64,
    pub cache_creation_ephemeral_1h_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_ttl_minutes: Option<i64>,
}

impl BillingUsageInput {
    pub fn new(task_type: impl Into<String>) -> Self {
        Self {
            task_type: task_type.into(),
            api_format: None,
            request_count: 1,
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_creation_ephemeral_5m_tokens: 0,
            cache_creation_ephemeral_1h_tokens: 0,
            cache_read_tokens: 0,
            cache_ttl_minutes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillingComputation {
    pub cost_result: crate::CostResult,
    pub actual_total_cost: f64,
    pub rate_multiplier: f64,
    pub is_free_tier: bool,
}
