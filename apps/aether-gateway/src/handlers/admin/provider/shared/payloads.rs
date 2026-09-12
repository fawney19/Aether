use crate::handlers::admin::shared::{
    deserialize_optional_f64_from_number_or_string, AdminTypedObjectPatch,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct AdminProviderKeyCreateRequest {
    #[serde(default)]
    pub(crate) api_formats: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) api_key: Option<String>,
    #[serde(default)]
    pub(crate) auth_type: Option<String>,
    #[serde(default)]
    pub(crate) auth_type_by_format: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) allow_auth_channel_mismatch_formats: Option<Option<Vec<String>>>,
    #[serde(default)]
    pub(crate) auth_config: Option<serde_json::Value>,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) rate_multipliers: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) internal_priority: Option<i32>,
    #[serde(default)]
    pub(crate) rpm_limit: Option<u32>,
    #[serde(default)]
    pub(crate) concurrent_limit: Option<i32>,
    #[serde(default)]
    pub(crate) allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) capabilities: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) cache_ttl_minutes: Option<i32>,
    #[serde(default)]
    pub(crate) max_probe_interval_minutes: Option<i32>,
    #[serde(default)]
    pub(crate) note: Option<String>,
    #[serde(default)]
    pub(crate) auto_fetch_models: Option<bool>,
    #[serde(default)]
    pub(crate) locked_models: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) model_include_patterns: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) model_exclude_patterns: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) fingerprint: Option<serde_json::Value>,
}

impl std::fmt::Debug for AdminProviderKeyCreateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminProviderKeyCreateRequest")
            .field("api_formats", &self.api_formats)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("auth_type", &self.auth_type)
            .field(
                "auth_config",
                &self.auth_config.as_ref().map(|_| "[REDACTED]"),
            )
            .field("name", &self.name)
            .field("internal_priority", &self.internal_priority)
            .field("rpm_limit", &self.rpm_limit)
            .field("concurrent_limit", &self.concurrent_limit)
            .field("auto_fetch_models", &self.auto_fetch_models)
            .field(
                "fingerprint",
                &self.fingerprint.as_ref().map(|_| "[REDACTED]"),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
pub(crate) struct AdminProviderKeyUpdateRequest {
    #[serde(default)]
    pub(crate) api_formats: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) api_key: Option<String>,
    #[serde(default)]
    pub(crate) auth_type: Option<String>,
    #[serde(default)]
    pub(crate) auth_type_by_format: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) allow_auth_channel_mismatch_formats: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) auth_config: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) rate_multipliers: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) internal_priority: Option<i32>,
    #[serde(default)]
    pub(crate) global_priority_by_format: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) rpm_limit: Option<u32>,
    #[serde(default)]
    pub(crate) concurrent_limit: Option<i32>,
    #[serde(default)]
    pub(crate) allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) capabilities: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) cache_ttl_minutes: Option<i32>,
    #[serde(default)]
    pub(crate) max_probe_interval_minutes: Option<i32>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
    #[serde(default)]
    pub(crate) note: Option<String>,
    #[serde(default)]
    pub(crate) auto_fetch_models: Option<bool>,
    #[serde(default)]
    pub(crate) locked_models: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) model_include_patterns: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) model_exclude_patterns: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) proxy: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) fingerprint: Option<serde_json::Value>,
}

impl std::fmt::Debug for AdminProviderKeyUpdateRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminProviderKeyUpdateRequest")
            .field("api_formats", &self.api_formats)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("auth_type", &self.auth_type)
            .field(
                "auth_config",
                &self.auth_config.as_ref().map(|_| "[REDACTED]"),
            )
            .field("name", &self.name)
            .field("internal_priority", &self.internal_priority)
            .field("rpm_limit", &self.rpm_limit)
            .field("concurrent_limit", &self.concurrent_limit)
            .field("is_active", &self.is_active)
            .field("auto_fetch_models", &self.auto_fetch_models)
            .field("proxy", &self.proxy.as_ref().map(|_| "[REDACTED]"))
            .field(
                "fingerprint",
                &self.fingerprint.as_ref().map(|_| "[REDACTED]"),
            )
            .finish_non_exhaustive()
    }
}

pub(crate) type AdminProviderKeyUpdatePatch = AdminTypedObjectPatch<AdminProviderKeyUpdateRequest>;

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderKeyBatchUpdateRequest {
    pub(crate) key_ids: Vec<String>,
    pub(crate) patch: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderKeyBatchDeleteRequest {
    pub(crate) ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderQuotaRefreshRequest {
    #[serde(default)]
    pub(crate) key_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminCodexResetCreditConsumeRequest {
    pub(crate) idempotency_key: String,
    pub(crate) expected_credential_generation: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderCreateRequest {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) provider_type: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) website: Option<String>,
    #[serde(default)]
    pub(crate) billing_type: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) monthly_quota_usd: Option<f64>,
    #[serde(default)]
    pub(crate) quota_reset_day: Option<u64>,
    #[serde(default)]
    pub(crate) quota_last_reset_at: Option<String>,
    #[serde(default)]
    pub(crate) quota_expires_at: Option<String>,
    #[serde(default)]
    pub(crate) provider_priority: Option<i32>,
    #[serde(default)]
    pub(crate) keep_priority_on_conversion: Option<bool>,
    #[serde(default)]
    pub(crate) codex_fingerprint_convergence_enabled: Option<bool>,
    #[serde(default)]
    pub(crate) responses_websocket_enabled: Option<bool>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
    #[serde(default)]
    pub(crate) concurrent_limit: Option<i32>,
    #[serde(default)]
    pub(crate) max_retries: Option<i32>,
    #[serde(default)]
    pub(crate) max_transfer_count: Option<i64>,
    #[serde(default)]
    pub(crate) max_transfer_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub(crate) proxy: Option<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) stream_first_byte_timeout: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) request_timeout: Option<f64>,
    #[serde(default)]
    pub(crate) pool_advanced: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) claude_code_advanced: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) failover_rules: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderUpdateRequest {
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) provider_type: Option<String>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) website: Option<String>,
    #[serde(default)]
    pub(crate) billing_type: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) monthly_quota_usd: Option<f64>,
    #[serde(default)]
    pub(crate) quota_reset_day: Option<u64>,
    #[serde(default)]
    pub(crate) quota_last_reset_at: Option<String>,
    #[serde(default)]
    pub(crate) quota_expires_at: Option<String>,
    #[serde(default)]
    pub(crate) provider_priority: Option<i32>,
    #[serde(default)]
    pub(crate) keep_priority_on_conversion: Option<bool>,
    #[serde(default)]
    pub(crate) codex_fingerprint_convergence_enabled: Option<bool>,
    #[serde(default)]
    pub(crate) responses_websocket_enabled: Option<bool>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
    #[serde(default)]
    pub(crate) concurrent_limit: Option<i32>,
    #[serde(default)]
    pub(crate) max_retries: Option<i32>,
    #[serde(default)]
    pub(crate) max_transfer_count: Option<i64>,
    #[serde(default)]
    pub(crate) max_transfer_timeout_seconds: Option<i64>,
    #[serde(default)]
    pub(crate) proxy: Option<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) stream_first_byte_timeout: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) request_timeout: Option<f64>,
    #[serde(default)]
    pub(crate) pool_advanced: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) claude_code_advanced: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) failover_rules: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) enable_format_conversion: Option<bool>,
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
}

pub(crate) type AdminProviderUpdatePatch = AdminTypedObjectPatch<AdminProviderUpdateRequest>;

pub(crate) const CODEX_WHAM_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
pub(crate) const KIRO_USAGE_LIMITS_PATH: &str = "/getUsageLimits";
pub(crate) const KIRO_USAGE_SDK_VERSION: &str = "1.0.0";
pub(crate) const ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";
pub(crate) const OAUTH_ACCOUNT_BLOCK_PREFIX: &str = "[ACCOUNT_BLOCK] ";
pub(crate) const OAUTH_REFRESH_FAILED_PREFIX: &str = "[REFRESH_FAILED] ";
pub(crate) const OAUTH_EXPIRED_PREFIX: &str = "[OAUTH_EXPIRED] ";
pub(crate) const OAUTH_REQUEST_FAILED_PREFIX: &str = "[REQUEST_FAILED] ";

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderModelCreateRequest {
    pub(crate) provider_model_name: String,
    #[serde(default)]
    pub(crate) provider_model_mappings: Option<serde_json::Value>,
    pub(crate) global_model_id: String,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) price_per_request: Option<f64>,
    #[serde(default)]
    pub(crate) tiered_pricing: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) supports_vision: Option<bool>,
    #[serde(default)]
    pub(crate) supports_function_calling: Option<bool>,
    #[serde(default)]
    pub(crate) supports_streaming: Option<bool>,
    #[serde(default)]
    pub(crate) supports_extended_thinking: Option<bool>,
    #[serde(default)]
    pub(crate) supports_image_generation: Option<bool>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminProviderModelUpdateRequest {
    #[serde(default)]
    pub(crate) provider_model_name: Option<String>,
    #[serde(default)]
    pub(crate) provider_model_mappings: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) global_model_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) price_per_request: Option<f64>,
    #[serde(default)]
    pub(crate) tiered_pricing: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) supports_vision: Option<bool>,
    #[serde(default)]
    pub(crate) supports_function_calling: Option<bool>,
    #[serde(default)]
    pub(crate) supports_streaming: Option<bool>,
    #[serde(default)]
    pub(crate) supports_extended_thinking: Option<bool>,
    #[serde(default)]
    pub(crate) supports_image_generation: Option<bool>,
    #[serde(default)]
    pub(crate) is_active: Option<bool>,
    #[serde(default)]
    pub(crate) is_available: Option<bool>,
    #[serde(default)]
    pub(crate) config: Option<serde_json::Value>,
}

pub(crate) type AdminProviderModelUpdatePatch =
    AdminTypedObjectPatch<AdminProviderModelUpdateRequest>;

#[derive(Debug, Deserialize)]
pub(crate) struct AdminBatchAssignGlobalModelsRequest {
    pub(crate) global_model_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminImportProviderModelsRequest {
    pub(crate) model_ids: Vec<String>,
    #[serde(default)]
    pub(crate) tiered_pricing: Option<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_f64_from_number_or_string"
    )]
    pub(crate) price_per_request: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::{
        AdminCodexResetCreditConsumeRequest, AdminProviderKeyCreateRequest,
        AdminProviderKeyUpdateRequest,
    };

    #[test]
    fn provider_key_request_debug_output_redacts_authorization_material() {
        let create = serde_json::from_value::<AdminProviderKeyCreateRequest>(serde_json::json!({
            "name": "key",
            "api_key": "create-api-key-canary",
            "auth_config": {"refresh_token": "create-refresh-token-canary"},
            "fingerprint": {"device_id": "create-device-canary"}
        }))
        .expect("create request should deserialize");
        let update = serde_json::from_value::<AdminProviderKeyUpdateRequest>(serde_json::json!({
            "api_key": "update-api-key-canary",
            "auth_config": {"refresh_token": "update-refresh-token-canary"},
            "proxy": {"password": "update-proxy-canary"},
            "fingerprint": {"device_id": "update-device-canary"}
        }))
        .expect("update request should deserialize");

        let create_debug = format!("{create:?}");
        let update_debug = format!("{update:?}");
        assert!(create_debug.contains("[REDACTED]"));
        assert!(update_debug.contains("[REDACTED]"));
        for secret in [
            "create-api-key-canary",
            "create-refresh-token-canary",
            "create-device-canary",
        ] {
            assert!(
                !create_debug.contains(secret),
                "create debug leaked {secret}"
            );
        }
        for secret in [
            "update-api-key-canary",
            "update-refresh-token-canary",
            "update-proxy-canary",
            "update-device-canary",
        ] {
            assert!(
                !update_debug.contains(secret),
                "update debug leaked {secret}"
            );
        }
    }

    #[test]
    fn codex_reset_credit_consume_requires_an_explicit_credential_generation() {
        assert!(
            serde_json::from_value::<AdminCodexResetCreditConsumeRequest>(
                serde_json::json!({"idempotency_key":"reset-old-client"}),
            )
            .is_err()
        );

        let legacy_account =
            serde_json::from_value::<AdminCodexResetCreditConsumeRequest>(serde_json::json!({
                "idempotency_key":"reset-legacy-account",
                "expected_credential_generation":null,
            }))
            .expect("explicit null should fence an account without a generation");
        assert!(legacy_account.expected_credential_generation.is_null());

        let generated_account =
            serde_json::from_value::<AdminCodexResetCreditConsumeRequest>(serde_json::json!({
                "idempotency_key":"reset-generated-account",
                "expected_credential_generation":"credential-v2",
            }))
            .expect("string generation should deserialize");
        assert_eq!(
            generated_account.expected_credential_generation,
            serde_json::json!("credential-v2")
        );
    }
}
