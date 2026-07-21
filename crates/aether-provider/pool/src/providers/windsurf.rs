use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_pool_core::PoolSchedulingPreset;
use serde_json::{json, Map, Value};

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
    ProviderPoolMemberInput,
};
use crate::quota::{
    provider_pool_json_bool, provider_pool_json_f64, provider_pool_member_quota_snapshot,
    provider_pool_metadata_bucket, provider_pool_quota_snapshot_exhausted_decision,
};
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const WINDSURF_DEFAULT_BASE_URL: &str = "https://server.codeium.com";
pub const WINDSURF_USER_STATUS_PATH: &str =
    "/exa.seat_management_pb.SeatManagementService/GetUserStatus";
pub const WINDSURF_MODEL_CONFIGS_PATH: &str =
    "/exa.api_server_pb.ApiServerService/GetCascadeModelConfigs";
pub const WINDSURF_RATE_LIMIT_PATH: &str =
    "/exa.api_server_pb.ApiServerService/CheckUserMessageRateLimit";

#[derive(Debug, Clone, Default)]
pub struct WindsurfProviderPoolAdapter;

impl ProviderPoolAdapter for WindsurfProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "windsurf"
    }

    fn capabilities(&self) -> ProviderPoolCapabilities {
        ProviderPoolCapabilities {
            plan_tier: true,
            quota_reset: true,
            quota_refresh: true,
        }
    }

    fn default_scheduling_presets(&self) -> Vec<PoolSchedulingPreset> {
        vec![PoolSchedulingPreset {
            preset: "recent_refresh".to_string(),
            enabled: true,
            mode: None,
        }]
    }

    fn quota_exhausted(&self, input: &ProviderPoolMemberInput<'_>) -> bool {
        if windsurf_quota_snapshot_hard_exhausted(input.key, input.provider_type) {
            return true;
        }
        if let Some(exhausted) =
            provider_pool_quota_snapshot_exhausted_decision(input.key, input.provider_type)
        {
            return exhausted;
        }
        provider_pool_metadata_bucket(input.key.upstream_metadata.as_ref(), input.provider_type)
            .is_some_and(windsurf_quota_exhausted_from_bucket)
    }

    fn quota_refresh_endpoint(
        &self,
        endpoints: &[StoredProviderCatalogEndpoint],
        include_inactive: bool,
    ) -> Option<StoredProviderCatalogEndpoint> {
        provider_pool_matching_endpoint(endpoints, include_inactive, |endpoint| {
            provider_pool_endpoint_format_matches(endpoint, "openai:chat")
        })
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效的 openai:chat 端点".to_string()
    }
}

fn windsurf_quota_snapshot_hard_exhausted(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> bool {
    provider_pool_member_quota_snapshot(key, provider_type)
        .and_then(|quota| quota.get("code"))
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .is_some_and(|code| matches!(code.as_str(), "banned" | "forbidden" | "quarantined"))
}

pub fn build_windsurf_pool_quota_request(
    key_id: &str,
    api_key: &str,
) -> ProviderPoolQuotaRequestSpec {
    build_windsurf_pool_quota_request_with_base_url(key_id, WINDSURF_DEFAULT_BASE_URL, api_key)
}

pub fn build_windsurf_pool_quota_request_with_base_url(
    key_id: &str,
    base_url: &str,
    api_key: &str,
) -> ProviderPoolQuotaRequestSpec {
    build_windsurf_connect_rpc_request(
        format!("windsurf-quota:{key_id}"),
        "windsurf:user_status",
        "windsurf-user-status",
        base_url,
        WINDSURF_USER_STATUS_PATH,
        api_key,
    )
}

pub fn build_windsurf_pool_model_configs_request(
    key_id: &str,
    api_key: &str,
) -> ProviderPoolQuotaRequestSpec {
    build_windsurf_pool_model_configs_request_with_base_url(
        key_id,
        WINDSURF_DEFAULT_BASE_URL,
        api_key,
    )
}

pub fn build_windsurf_pool_model_configs_request_with_base_url(
    key_id: &str,
    base_url: &str,
    api_key: &str,
) -> ProviderPoolQuotaRequestSpec {
    build_windsurf_connect_rpc_request(
        format!("windsurf-models:{key_id}"),
        "windsurf:model_configs",
        "windsurf-model-configs",
        base_url,
        WINDSURF_MODEL_CONFIGS_PATH,
        api_key,
    )
}

pub fn build_windsurf_pool_rate_limit_request(
    key_id: &str,
    api_key: &str,
) -> ProviderPoolQuotaRequestSpec {
    build_windsurf_pool_rate_limit_request_with_base_url(key_id, WINDSURF_DEFAULT_BASE_URL, api_key)
}

pub fn build_windsurf_pool_rate_limit_request_with_base_url(
    key_id: &str,
    base_url: &str,
    api_key: &str,
) -> ProviderPoolQuotaRequestSpec {
    build_windsurf_connect_rpc_request(
        format!("windsurf-rate-limit:{key_id}"),
        "windsurf:rate_limit",
        "windsurf-rate-limit",
        base_url,
        WINDSURF_RATE_LIMIT_PATH,
        api_key,
    )
}

fn build_windsurf_connect_rpc_request(
    request_id: String,
    provider_api_format: &str,
    model_name: &str,
    base_url: &str,
    path: &str,
    api_key: &str,
) -> ProviderPoolQuotaRequestSpec {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "application/json".to_string());
    headers.insert("connect-protocol-version".to_string(), "1".to_string());
    headers.insert("user-agent".to_string(), "windsurf/1.9600.41".to_string());

    ProviderPoolQuotaRequestSpec {
        request_id,
        provider_name: "windsurf".to_string(),
        quota_kind: "windsurf".to_string(),
        method: "POST".to_string(),
        url: format!("{}{}", base_url.trim_end_matches('/'), path),
        headers,
        content_type: Some("application/json".to_string()),
        json_body: Some(json!({
            "metadata": windsurf_metadata(api_key),
        })),
        client_api_format: "openai:chat".to_string(),
        provider_api_format: provider_api_format.to_string(),
        model_name: Some(model_name.to_string()),
        accept_invalid_certs: false,
    }
}

fn windsurf_metadata(api_key: &str) -> Value {
    json!({
        "apiKey": api_key,
        "ideName": "windsurf",
        "ideVersion": "1.9600.41",
        "extensionName": "windsurf",
        "extensionVersion": "1.9600.41",
        "locale": "en",
    })
}

pub(crate) fn windsurf_quota_exhausted_from_bucket(bucket: &Map<String, Value>) -> bool {
    if provider_pool_json_bool(bucket.get("banned"))
        .or_else(|| provider_pool_json_bool(bucket.get("quarantined")))
        .unwrap_or(false)
    {
        return true;
    }
    let daily_remaining = provider_pool_json_f64(bucket.get("daily_remaining_percent"));
    let weekly_remaining = provider_pool_json_f64(bucket.get("weekly_remaining_percent"));
    daily_remaining.is_some_and(|value| value <= 0.0)
        || weekly_remaining.is_some_and(|value| value <= 0.0)
}

#[cfg(test)]
mod tests {
    use super::{windsurf_quota_exhausted_from_bucket, windsurf_quota_snapshot_hard_exhausted};
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use serde_json::json;

    fn sample_key_with_quota(code: &str, exhausted: bool) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-windsurf".to_string(),
            "provider-windsurf".to_string(),
            "windsurf@example.com".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("sample key should build");
        key.status_snapshot = Some(json!({
            "quota": {
                "provider_type": "windsurf",
                "code": code,
                "exhausted": exhausted,
                "windows": [{
                    "code": "daily",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0
                }]
            }
        }));
        key
    }

    #[test]
    fn windsurf_rate_limit_bucket_does_not_mark_quota_exhausted() {
        let bucket = json!({
            "rate_limit": {
                "limited": true,
                "retry_after_ms": 60_000u64
            },
            "daily_remaining_percent": 50.0,
            "weekly_remaining_percent": 50.0
        });
        let bucket = bucket.as_object().expect("bucket should be object");

        assert!(!windsurf_quota_exhausted_from_bucket(bucket));
    }

    #[test]
    fn windsurf_banned_and_quarantined_snapshot_codes_are_hard_exhausted() {
        for code in ["banned", "forbidden", "quarantined"] {
            let key = sample_key_with_quota(code, false);

            assert!(
                windsurf_quota_snapshot_hard_exhausted(&key, "windsurf"),
                "{code} should be hard exhausted"
            );
        }

        let cooldown_key = sample_key_with_quota("cooldown", false);
        assert!(!windsurf_quota_snapshot_hard_exhausted(
            &cooldown_key,
            "windsurf"
        ));
        let rate_limited_key = sample_key_with_quota("rate_limited", false);
        assert!(!windsurf_quota_snapshot_hard_exhausted(
            &rate_limited_key,
            "windsurf"
        ));
    }
}
