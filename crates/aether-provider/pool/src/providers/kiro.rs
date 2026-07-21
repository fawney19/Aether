use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use serde_json::{Map, Value};
use url::form_urlencoded;
use uuid::Uuid;

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
    ProviderPoolMemberInput,
};
use crate::quota::{
    provider_pool_current_unix_secs, provider_pool_json_f64, provider_pool_metadata_bucket,
    provider_pool_quota_snapshot_exhausted_decision, provider_pool_reset_deadline_elapsed,
    provider_pool_timestamp_unix_secs,
};
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const KIRO_USAGE_LIMITS_PATH: &str = "/getUsageLimits";
pub const KIRO_USAGE_SDK_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, PartialEq)]
pub struct KiroPoolQuotaAuthInput {
    pub authorization_value: String,
    pub api_region: String,
    pub kiro_version: String,
    pub machine_id: String,
    pub profile_arn: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct KiroProviderPoolAdapter;

impl ProviderPoolAdapter for KiroProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "kiro"
    }

    fn capabilities(&self) -> ProviderPoolCapabilities {
        ProviderPoolCapabilities {
            plan_tier: true,
            quota_reset: true,
            quota_refresh: true,
        }
    }

    fn quota_exhausted(&self, input: &ProviderPoolMemberInput<'_>) -> bool {
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
            provider_pool_endpoint_format_matches(endpoint, "claude:messages")
        })
        .or_else(|| provider_pool_matching_endpoint(endpoints, include_inactive, |_| true))
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效的 Kiro 端点".to_string()
    }
}

pub fn build_kiro_pool_quota_request(
    key_id: &str,
    auth: &KiroPoolQuotaAuthInput,
) -> ProviderPoolQuotaRequestSpec {
    let host = format!("q.{}.amazonaws.com", normalize_region(&auth.api_region));
    let machine_id = auth.machine_id.trim();
    let ide_tag = if machine_id.is_empty() {
        format!("KiroIDE-{}", normalize_kiro_version(&auth.kiro_version))
    } else {
        format!(
            "KiroIDE-{}-{machine_id}",
            normalize_kiro_version(&auth.kiro_version)
        )
    };
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("origin", "AI_EDITOR");
    serializer.append_pair("resourceType", "AGENTIC_REQUEST");
    serializer.append_pair("isEmailRequired", "true");
    if let Some(profile_arn) = auth
        .profile_arn
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        serializer.append_pair("profileArn", profile_arn);
    }

    ProviderPoolQuotaRequestSpec {
        request_id: format!("kiro-quota:{key_id}"),
        provider_name: "kiro".to_string(),
        quota_kind: "kiro".to_string(),
        method: "GET".to_string(),
        url: format!("https://{host}{KIRO_USAGE_LIMITS_PATH}?{}", serializer.finish()),
        headers: BTreeMap::from([
            (
                "x-amz-user-agent".to_string(),
                format!("aws-sdk-js/{KIRO_USAGE_SDK_VERSION} {ide_tag}"),
            ),
            (
                "user-agent".to_string(),
                format!(
                    "aws-sdk-js/{KIRO_USAGE_SDK_VERSION} ua/2.1 os/other#unknown lang/js md/nodejs#22.21.1 api/codewhispererruntime#1.0.0 m/N,E {ide_tag}"
                ),
            ),
            ("host".to_string(), host),
            ("amz-sdk-invocation-id".to_string(), Uuid::new_v4().to_string()),
            ("amz-sdk-request".to_string(), "attempt=1; max=1".to_string()),
            (
                "authorization".to_string(),
                auth.authorization_value.clone(),
            ),
            ("connection".to_string(), "close".to_string()),
        ]),
        content_type: None,
        json_body: None,
        client_api_format: "claude:messages".to_string(),
        provider_api_format: "kiro:usage".to_string(),
        model_name: Some("kiro-usage-limits".to_string()),
        accept_invalid_certs: false,
    }
}

fn normalize_region(value: &str) -> &str {
    let value = value.trim();
    if value.is_empty() {
        "us-east-1"
    } else {
        value
    }
}

fn normalize_kiro_version(value: &str) -> &str {
    let value = value.trim();
    if value.is_empty() {
        "0.3.210"
    } else {
        value
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
    if provider_pool_json_f64(bucket.get("remaining")).is_some_and(|value| value <= 0.0) {
        return true;
    }
    if provider_pool_json_f64(bucket.get("usage_percentage")).is_some_and(|value| value >= 100.0) {
        return true;
    }
    match (
        provider_pool_json_f64(bucket.get("usage_limit")),
        provider_pool_json_f64(bucket.get("current_usage")),
    ) {
        (Some(limit), Some(current)) if limit > 0.0 => current >= limit,
        _ => false,
    }
}
