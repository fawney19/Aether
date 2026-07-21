use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
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
    pub accept_invalid_certs: bool,
}
