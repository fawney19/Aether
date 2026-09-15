use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use aether_provider_transport::xai::{
    insert_cli_identity_headers, XAI_CHAT_PROXY_BASE_URL, XAI_PROVIDER_TYPE,
};
use serde_json::{Map, Value};

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
    ProviderPoolMemberInput,
};
use crate::quota::{
    provider_pool_current_unix_secs, provider_pool_json_bool, provider_pool_json_f64,
    provider_pool_metadata_bucket, provider_pool_model_quota_exhausted,
    provider_pool_quota_snapshot_exhausted_decision, provider_pool_reset_deadline_elapsed,
    provider_pool_timestamp_unix_secs,
};
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const XAI_USER_PATH: &str = "/user";
pub const XAI_BILLING_PATH: &str = "/billing?format=credits";

#[derive(Debug, Clone, Default)]
pub struct XaiProviderPoolAdapter;

impl ProviderPoolAdapter for XaiProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        XAI_PROVIDER_TYPE
    }

    fn capabilities(&self) -> ProviderPoolCapabilities {
        ProviderPoolCapabilities {
            plan_tier: true,
            quota_reset: true,
            quota_refresh: true,
        }
    }

    fn quota_exhausted(&self, input: &ProviderPoolMemberInput<'_>) -> bool {
        if let Some(exhausted) = input.provider_model_name.and_then(|model| {
            provider_pool_model_quota_exhausted(input.key, input.provider_type, model)
        }) {
            return exhausted;
        }
        if let Some(exhausted) =
            provider_pool_quota_snapshot_exhausted_decision(input.key, input.provider_type)
        {
            return exhausted;
        }
        provider_pool_metadata_bucket(input.key.upstream_metadata.as_ref(), input.provider_type)
            .is_some_and(quota_exhausted_from_bucket)
    }

    fn quota_refresh_endpoint(
        &self,
        endpoints: &[StoredProviderCatalogEndpoint],
        include_inactive: bool,
    ) -> Option<StoredProviderCatalogEndpoint> {
        provider_pool_matching_endpoint(endpoints, include_inactive, |endpoint| {
            provider_pool_endpoint_format_matches(endpoint, "openai:responses")
        })
        .or_else(|| provider_pool_matching_endpoint(endpoints, include_inactive, |_| true))
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效的 openai:responses 端点".to_string()
    }
}

pub fn build_xai_pool_user_request(
    key_id: &str,
    authorization: (String, String),
) -> ProviderPoolQuotaRequestSpec {
    build_xai_pool_request(
        format!("xai-user:{key_id}"),
        "xai:user",
        "user",
        XAI_USER_PATH,
        authorization,
        None,
    )
}

pub fn build_xai_pool_billing_request(
    key_id: &str,
    authorization: (String, String),
    user_id: Option<&str>,
) -> ProviderPoolQuotaRequestSpec {
    build_xai_pool_request(
        format!("xai-billing:{key_id}"),
        "xai:billing",
        "billing",
        XAI_BILLING_PATH,
        authorization,
        user_id,
    )
}

fn build_xai_pool_request(
    request_id: String,
    provider_api_format: &str,
    model_name: &str,
    path: &str,
    authorization: (String, String),
    user_id: Option<&str>,
) -> ProviderPoolQuotaRequestSpec {
    let mut headers = BTreeMap::from([
        (authorization.0, authorization.1),
        ("accept".to_string(), "application/json".to_string()),
    ]);
    insert_cli_identity_headers(&mut headers);
    if let Some(user_id) = user_id.map(str::trim).filter(|value| !value.is_empty()) {
        headers.insert("x-userid".to_string(), user_id.to_string());
    }

    ProviderPoolQuotaRequestSpec {
        request_id,
        provider_name: XAI_PROVIDER_TYPE.to_string(),
        quota_kind: XAI_PROVIDER_TYPE.to_string(),
        method: "GET".to_string(),
        url: format!("{}{path}", XAI_CHAT_PROXY_BASE_URL.trim_end_matches('/')),
        headers,
        content_type: None,
        json_body: None,
        client_api_format: "openai:responses".to_string(),
        provider_api_format: provider_api_format.to_string(),
        model_name: Some(model_name.to_string()),
    }
}

pub(crate) fn quota_exhausted_from_bucket(bucket: &Map<String, Value>) -> bool {
    if provider_pool_current_unix_secs().is_some_and(|now| {
        provider_pool_reset_deadline_elapsed(
            bucket,
            provider_pool_timestamp_unix_secs(bucket.get("updated_at")),
            now,
        )
    }) {
        return false;
    }

    let usage_exhausted = provider_pool_json_f64(bucket.get("remaining"))
        .is_some_and(|value| value <= 0.0)
        || provider_pool_json_f64(bucket.get("usage_percentage"))
            .is_some_and(|value| value >= 100.0 - 1e-6)
        || match (
            provider_pool_json_f64(bucket.get("usage_limit")),
            provider_pool_json_f64(bucket.get("current_usage")),
        ) {
            (Some(limit), Some(current)) if limit > 0.0 => current >= limit,
            _ => false,
        };
    if !usage_exhausted {
        return false;
    }

    let prepaid_available =
        provider_pool_json_f64(bucket.get("prepaid_balance")).is_some_and(|value| value > 0.0);
    if prepaid_available {
        return false;
    }

    let on_demand_enabled = provider_pool_json_bool(bucket.get("on_demand_enabled")) != Some(false);
    let on_demand_cap = provider_pool_json_f64(bucket.get("on_demand_cap")).unwrap_or(0.0);
    let on_demand_used = provider_pool_json_f64(bucket.get("on_demand_used")).unwrap_or(0.0);
    if on_demand_enabled && on_demand_cap > 0.0 && on_demand_used < on_demand_cap {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::{
        build_xai_pool_billing_request, build_xai_pool_user_request, quota_exhausted_from_bucket,
    };
    use aether_provider_transport::xai::{
        XAI_CHAT_PROXY_BASE_URL, XAI_CLIENT_IDENTIFIER_VALUE, XAI_TOKEN_AUTH_VALUE,
    };
    use serde_json::{json, Map};

    fn bucket(value: serde_json::Value) -> Map<String, serde_json::Value> {
        value.as_object().cloned().expect("bucket should be object")
    }

    #[test]
    fn user_and_billing_requests_pin_cli_chat_proxy_and_identity_headers() {
        let authorization = ("authorization".to_string(), "Bearer xai-access".to_string());
        let user = build_xai_pool_user_request("key-1", authorization.clone());
        let billing = build_xai_pool_billing_request("key-1", authorization, Some("user-42"));

        assert_eq!(
            user.url,
            format!("{}/user", XAI_CHAT_PROXY_BASE_URL.trim_end_matches('/'))
        );
        assert_eq!(
            billing.url,
            format!(
                "{}/billing?format=credits",
                XAI_CHAT_PROXY_BASE_URL.trim_end_matches('/')
            )
        );
        assert_eq!(
            user.headers.get("x-xai-token-auth").map(String::as_str),
            Some(XAI_TOKEN_AUTH_VALUE)
        );
        assert_eq!(
            user.headers
                .get("x-grok-client-identifier")
                .map(String::as_str),
            Some(XAI_CLIENT_IDENTIFIER_VALUE)
        );
        assert!(!user.headers.contains_key("x-userid"));
        assert_eq!(
            billing.headers.get("x-userid").map(String::as_str),
            Some("user-42")
        );
        assert_eq!(
            billing.headers.get("authorization").map(String::as_str),
            Some("Bearer xai-access")
        );
    }

    #[test]
    fn percent_exhausted_without_prepaid_or_on_demand_is_exhausted() {
        assert!(quota_exhausted_from_bucket(&bucket(json!({
            "usage_percentage": 100.0,
            "prepaid_balance": 0.0,
            "on_demand_cap": 0.0,
            "on_demand_used": 0.0
        }))));
    }

    #[test]
    fn unified_billing_zero_on_demand_cap_is_not_exhausted_when_percent_remains() {
        assert!(!quota_exhausted_from_bucket(&bucket(json!({
            "usage_percentage": 46.0,
            "prepaid_balance": 0.0,
            "on_demand_cap": 0.0,
            "on_demand_used": 0.0
        }))));
    }

    #[test]
    fn prepaid_balance_keeps_account_available_after_weekly_pool_hits_100() {
        assert!(!quota_exhausted_from_bucket(&bucket(json!({
            "usage_percentage": 100.0,
            "prepaid_balance": 12.5,
            "on_demand_cap": 0.0
        }))));
    }
}
