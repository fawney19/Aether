use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;

use crate::capability::ProviderPoolCapabilities;
use crate::provider::{
    provider_pool_endpoint_format_matches, provider_pool_matching_endpoint, ProviderPoolAdapter,
};
use crate::quota_refresh::ProviderPoolQuotaRequestSpec;

pub const GLM_CODING_PLAN_DEFAULT_ZAI_ANTHROPIC_BASE_URL: &str = "https://api.z.ai/api/anthropic";
pub const GLM_CODING_PLAN_DEFAULT_ZHIPU_ANTHROPIC_BASE_URL: &str =
    "https://open.bigmodel.cn/api/anthropic";
pub const GLM_CODING_PLAN_DEFAULT_ZAI_OPENAI_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";
pub const GLM_CODING_PLAN_DEFAULT_ZHIPU_OPENAI_BASE_URL: &str =
    "https://open.bigmodel.cn/api/coding/paas/v4";
pub const GLM_CODING_PLAN_MODEL_USAGE_PATH: &str = "/api/monitor/usage/model-usage";
pub const GLM_CODING_PLAN_QUOTA_LIMIT_PATH: &str = "/api/monitor/usage/quota/limit";

#[derive(Debug, Clone, Default)]
pub struct GlmCodingPlanProviderPoolAdapter;

impl ProviderPoolAdapter for GlmCodingPlanProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "glm_coding_plan"
    }

    fn capabilities(&self) -> ProviderPoolCapabilities {
        ProviderPoolCapabilities {
            plan_tier: false,
            quota_reset: false,
            quota_refresh: true,
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
        .or_else(|| {
            provider_pool_matching_endpoint(endpoints, include_inactive, |endpoint| {
                provider_pool_endpoint_format_matches(endpoint, "openai:chat")
            })
        })
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效的 claude:messages 或 openai:chat 端点".to_string()
    }
}

pub fn build_glm_coding_plan_pool_quota_request_with_base_url(
    key_id: &str,
    base_url: &str,
    api_key: &str,
    quota_kind: &str,
    query: Option<&str>,
) -> ProviderPoolQuotaRequestSpec {
    let path = match quota_kind {
        "model_usage" => GLM_CODING_PLAN_MODEL_USAGE_PATH,
        "quota_limit" => GLM_CODING_PLAN_QUOTA_LIMIT_PATH,
        _ => GLM_CODING_PLAN_QUOTA_LIMIT_PATH,
    };
    let url = glm_coding_plan_monitor_url(base_url, path, query);
    let mut headers = BTreeMap::new();
    headers.insert("authorization".to_string(), api_key.to_string());
    headers.insert("accept-language".to_string(), "en-US,en".to_string());
    headers.insert("content-type".to_string(), "application/json".to_string());

    ProviderPoolQuotaRequestSpec {
        request_id: format!("glm-coding-plan-{quota_kind}:{key_id}"),
        provider_name: "glm_coding_plan".to_string(),
        quota_kind: format!("glm_coding_plan:{quota_kind}"),
        method: "GET".to_string(),
        url,
        headers,
        content_type: None,
        json_body: None,
        client_api_format: "claude:messages".to_string(),
        provider_api_format: "glm_coding_plan:usage".to_string(),
        model_name: Some(format!("glm-coding-plan-{quota_kind}")),
        accept_invalid_certs: false,
    }
}

fn glm_coding_plan_monitor_url(base_url: &str, path: &str, query: Option<&str>) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let base = if base.is_empty() {
        GLM_CODING_PLAN_DEFAULT_ZHIPU_ANTHROPIC_BASE_URL
    } else {
        base
    };
    let monitor_base = if let Ok(url) = url::Url::parse(base) {
        let host = url.host_str().unwrap_or_default();
        match url.port() {
            Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
            None => format!("{}://{}", url.scheme(), host),
        }
    } else {
        base.to_string()
    };
    let mut url = format!("{}{}", monitor_base.trim_end_matches('/'), path);
    if let Some(query) = query.map(str::trim).filter(|value| !value.is_empty()) {
        url.push('?');
        url.push_str(query.trim_start_matches('?'));
    }
    url
}

#[cfg(test)]
mod tests {
    use super::{
        build_glm_coding_plan_pool_quota_request_with_base_url,
        GLM_CODING_PLAN_DEFAULT_ZAI_ANTHROPIC_BASE_URL,
        GLM_CODING_PLAN_DEFAULT_ZHIPU_ANTHROPIC_BASE_URL,
        GLM_CODING_PLAN_DEFAULT_ZHIPU_OPENAI_BASE_URL,
    };

    #[test]
    fn builds_zai_model_usage_request_from_anthropic_base_url() {
        let spec = build_glm_coding_plan_pool_quota_request_with_base_url(
            "key-1",
            GLM_CODING_PLAN_DEFAULT_ZAI_ANTHROPIC_BASE_URL,
            "token-1",
            "model_usage",
            Some("startTime=2026-06-22%2012%3A00%3A00&endTime=2026-06-23%2012%3A59%3A59"),
        );

        assert_eq!(
            spec.url,
            "https://api.z.ai/api/monitor/usage/model-usage?startTime=2026-06-22%2012%3A00%3A00&endTime=2026-06-23%2012%3A59%3A59"
        );
        assert_eq!(spec.method, "GET");
        assert_eq!(
            spec.headers.get("authorization").map(String::as_str),
            Some("token-1")
        );
    }

    #[test]
    fn builds_zhipu_quota_limit_request_from_anthropic_base_url() {
        let spec = build_glm_coding_plan_pool_quota_request_with_base_url(
            "key-1",
            GLM_CODING_PLAN_DEFAULT_ZHIPU_ANTHROPIC_BASE_URL,
            "token-1",
            "quota_limit",
            None,
        );

        assert_eq!(
            spec.url,
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        );
        assert_eq!(spec.provider_api_format, "glm_coding_plan:usage");
    }

    #[test]
    fn builds_zhipu_quota_limit_request_from_openai_coding_base_url() {
        let spec = build_glm_coding_plan_pool_quota_request_with_base_url(
            "key-1",
            GLM_CODING_PLAN_DEFAULT_ZHIPU_OPENAI_BASE_URL,
            "token-1",
            "quota_limit",
            None,
        );

        assert_eq!(
            spec.url,
            "https://open.bigmodel.cn/api/monitor/usage/quota/limit"
        );
    }

    #[test]
    fn preserves_non_default_port_when_building_monitor_url() {
        let spec = build_glm_coding_plan_pool_quota_request_with_base_url(
            "key-1",
            "http://127.0.0.1:3000/api/anthropic",
            "token-1",
            "quota_limit",
            None,
        );

        assert_eq!(
            spec.url,
            "http://127.0.0.1:3000/api/monitor/usage/quota/limit"
        );
    }
}
