use std::collections::BTreeMap;
use std::fmt;

use aether_contracts::redact_url_for_debug;
use serde_json::Value;

#[derive(Clone, PartialEq)]
pub struct ProviderPoolQuotaRequestSpec {
    pub request_id: String,
    pub provider_name: String,
    pub quota_kind: String,
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub content_type: Option<String>,
    pub json_body: Option<Value>,
    pub client_api_format: String,
    pub provider_api_format: String,
    pub model_name: Option<String>,
}

impl fmt::Debug for ProviderPoolQuotaRequestSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderPoolQuotaRequestSpec")
            .field("request_id", &self.request_id)
            .field("provider_name", &self.provider_name)
            .field("quota_kind", &self.quota_kind)
            .field("method", &self.method)
            .field("url", &redact_url_for_debug(&self.url))
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("content_type", &self.content_type)
            .field("has_json_body", &self.json_body.is_some())
            .field(
                "json_body_bytes",
                &self
                    .json_body
                    .as_ref()
                    .and_then(|body| serde_json::to_vec(body).ok().map(|bytes| bytes.len())),
            )
            .field("client_api_format", &self.client_api_format)
            .field("provider_api_format", &self.provider_api_format)
            .field("model_name", &self.model_name)
            .finish()
    }
}
