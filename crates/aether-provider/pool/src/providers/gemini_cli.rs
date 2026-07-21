use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use serde_json::json;

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
};
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH: &str = "/v1internal:retrieveUserQuota";
pub const GEMINI_CLI_USER_AGENT: &str = "GeminiCLI/0.1.5 (Windows; AMD64)";

#[derive(Debug, Clone, Default)]
pub struct GeminiCliProviderPoolAdapter;

impl ProviderPoolAdapter for GeminiCliProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "gemini_cli"
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
            provider_pool_endpoint_format_matches(endpoint, "gemini:generate_content")
        })
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效的 gemini:generate_content 端点".to_string()
    }
}

pub fn build_gemini_cli_pool_quota_request(
    key_id: &str,
    endpoint_base_url: &str,
    authorization: (String, String),
    project_id: &str,
) -> ProviderPoolQuotaRequestSpec {
    let headers = BTreeMap::from([
        ("authorization".to_string(), authorization.1),
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json".to_string()),
        ("user-agent".to_string(), GEMINI_CLI_USER_AGENT.to_string()),
    ]);

    ProviderPoolQuotaRequestSpec {
        request_id: format!("gemini-cli-quota:{key_id}"),
        provider_name: "gemini_cli".to_string(),
        quota_kind: "gemini_cli".to_string(),
        method: "POST".to_string(),
        url: format!(
            "{}{}",
            endpoint_base_url.trim_end_matches('/'),
            GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH
        ),
        headers,
        content_type: Some("application/json".to_string()),
        json_body: Some(json!({
            "project": project_id,
        })),
        client_api_format: "gemini:generate_content".to_string(),
        provider_api_format: "gemini_cli:retrieve_user_quota".to_string(),
        model_name: Some("retrieveUserQuota".to_string()),
        accept_invalid_certs: false,
    }
}
