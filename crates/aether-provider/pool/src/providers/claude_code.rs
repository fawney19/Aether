use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
};
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const CLAUDE_CODE_OAUTH_USAGE_URL: &str =
    "https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1";
pub const CLAUDE_CODE_OAUTH_BETA: &str = "oauth-2025-04-20";

#[derive(Debug, Clone, Default)]
pub struct ClaudeCodeProviderPoolAdapter;

impl ProviderPoolAdapter for ClaudeCodeProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "claude_code"
    }

    fn capabilities(&self) -> ProviderPoolCapabilities {
        ProviderPoolCapabilities {
            quota_refresh: true,
            ..ProviderPoolCapabilities::default()
        }
    }

    fn quota_refresh_endpoint(
        &self,
        endpoints: &[StoredProviderCatalogEndpoint],
        include_inactive: bool,
    ) -> Option<StoredProviderCatalogEndpoint> {
        provider_pool_matching_endpoint(endpoints, include_inactive, |endpoint| {
            provider_pool_endpoint_format_matches(endpoint, "claude:messages")
        })
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效的 claude:messages 端点".to_string()
    }
}

/// Builds the `GET /api/oauth/usage` request that reports the account's 5h / 7d
/// utilization windows (and, via `cedar_ember=1`, its quota reset credits). The URL is fixed (the origin allowlist only accepts
/// `api.anthropic.com`), independent of the inference endpoint's base URL.
pub fn build_claude_code_pool_quota_request(
    key_id: &str,
    authorization: (String, String),
) -> ProviderPoolQuotaRequestSpec {
    let headers = BTreeMap::from([
        ("authorization".to_string(), authorization.1),
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        (
            "anthropic-beta".to_string(),
            CLAUDE_CODE_OAUTH_BETA.to_string(),
        ),
        // The reset-credit (`cedar_ember`) block is only returned to first-party CLI callers.
        ("x-app".to_string(), "cli".to_string()),
        (
            "user-agent".to_string(),
            aether_provider_transport::claude_code::claude_code_client_user_agent(),
        ),
    ]);

    ProviderPoolQuotaRequestSpec {
        request_id: format!("claude-code-quota:{key_id}"),
        provider_name: "claude_code".to_string(),
        quota_kind: "claude_code".to_string(),
        method: "GET".to_string(),
        url: CLAUDE_CODE_OAUTH_USAGE_URL.to_string(),
        headers,
        content_type: None,
        json_body: None,
        client_api_format: "claude:messages".to_string(),
        provider_api_format: "claude_code:oauth_usage".to_string(),
        model_name: Some("oauth_usage".to_string()),
    }
}
