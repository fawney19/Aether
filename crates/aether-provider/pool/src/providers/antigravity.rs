use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use serde_json::json;

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
    ProviderPoolMemberInput,
};
use crate::quota::provider_pool_model_quota_exhausted;
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";
pub const ANTIGRAVITY_RETRIEVE_USER_QUOTA_SUMMARY_PATH: &str =
    "/v1internal:retrieveUserQuotaSummary";

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

    fn quota_exhausted(&self, input: &ProviderPoolMemberInput<'_>) -> bool {
        input
            .provider_model_name
            .and_then(|model| {
                provider_pool_model_quota_exhausted(input.key, input.provider_type, model)
            })
            .unwrap_or_else(|| {
                crate::quota::provider_pool_quota_snapshot_exhausted_decision(
                    input.key,
                    input.provider_type,
                )
                .unwrap_or(false)
            })
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
    }
}

pub fn build_antigravity_pool_quota_summary_request(
    key_id: &str,
    endpoint_base_url: &str,
    authorization: (String, String),
    project_id: Option<&str>,
    mut identity_headers: BTreeMap<String, String>,
) -> ProviderPoolQuotaRequestSpec {
    let mut headers = std::mem::take(&mut identity_headers);
    headers.insert("authorization".to_string(), authorization.1);
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "application/json".to_string());
    headers
        .entry("user-agent".to_string())
        .or_insert_with(|| "antigravity".to_string());

    let json_body = project_id
        .map(str::trim)
        .filter(|project_id| !project_id.is_empty())
        .map_or_else(|| json!({}), |project_id| json!({ "project": project_id }));

    ProviderPoolQuotaRequestSpec {
        request_id: format!("antigravity-quota-summary:{key_id}"),
        provider_name: "antigravity".to_string(),
        quota_kind: "antigravity".to_string(),
        method: "POST".to_string(),
        url: format!(
            "{}{}",
            endpoint_base_url.trim_end_matches('/'),
            ANTIGRAVITY_RETRIEVE_USER_QUOTA_SUMMARY_PATH
        ),
        headers,
        content_type: Some("application/json".to_string()),
        json_body: Some(json_body),
        client_api_format: "gemini:generate_content".to_string(),
        provider_api_format: "antigravity:retrieve_user_quota_summary".to_string(),
        model_name: Some("retrieveUserQuotaSummary".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{
        build_antigravity_pool_quota_summary_request, ANTIGRAVITY_RETRIEVE_USER_QUOTA_SUMMARY_PATH,
    };

    #[test]
    fn grouped_quota_request_can_retry_without_project_on_the_same_endpoint() {
        let with_project = build_antigravity_pool_quota_summary_request(
            "key-1",
            "https://daily-cloudcode-pa.googleapis.com/",
            ("authorization".to_string(), "Bearer token".to_string()),
            Some("project-1"),
            BTreeMap::new(),
        );
        let without_project = build_antigravity_pool_quota_summary_request(
            "key-1",
            "https://daily-cloudcode-pa.googleapis.com/",
            ("authorization".to_string(), "Bearer token".to_string()),
            None,
            BTreeMap::new(),
        );

        assert_eq!(
            with_project.url,
            format!(
                "https://daily-cloudcode-pa.googleapis.com{ANTIGRAVITY_RETRIEVE_USER_QUOTA_SUMMARY_PATH}"
            )
        );
        assert_eq!(
            with_project.json_body,
            Some(json!({"project": "project-1"}))
        );
        assert_eq!(without_project.url, with_project.url);
        assert_eq!(without_project.json_body, Some(json!({})));
    }
}
