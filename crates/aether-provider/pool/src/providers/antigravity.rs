use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use serde_json::json;

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
};
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";

#[derive(Debug, Clone, Default)]
pub struct AntigravityProviderPoolAdapter;

impl ProviderPoolAdapter for AntigravityProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "antigravity"
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

pub fn build_antigravity_pool_quota_request(
    key_id: &str,
    endpoint_base_url: &str,
    authorization: (String, String),
    project_id: &str,
    mut identity_headers: BTreeMap<String, String>,
) -> ProviderPoolQuotaRequestSpec {
    let mut headers = std::mem::take(&mut identity_headers);
    headers.insert("authorization".to_string(), authorization.1);
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "application/json".to_string());
    headers
        .entry("user-agent".to_string())
        .or_insert_with(|| "antigravity".to_string());

    ProviderPoolQuotaRequestSpec {
        request_id: format!("antigravity-quota:{key_id}"),
        provider_name: "antigravity".to_string(),
        quota_kind: "antigravity".to_string(),
        method: "POST".to_string(),
        url: format!(
            "{}{}",
            endpoint_base_url.trim_end_matches('/'),
            ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH
        ),
        headers,
        content_type: Some("application/json".to_string()),
        json_body: Some(json!({ "project": project_id })),
        client_api_format: "gemini:generate_content".to_string(),
        provider_api_format: "antigravity:fetch_available_models".to_string(),
        model_name: Some("fetchAvailableModels".to_string()),
        accept_invalid_certs: false,
    }
}
