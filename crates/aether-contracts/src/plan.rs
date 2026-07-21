use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER: &str = "x-aether-execution-follow-redirects";
pub const EXECUTION_REQUEST_HTTP1_ONLY_HEADER: &str = "x-aether-execution-http1-only";
pub const EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER: &str =
    "x-aether-execution-accept-invalid-certs";
pub const MAX_EXECUTION_REQUEST_TIMEOUT_SECS: u64 = 1_200;
pub const MAX_EXECUTION_REQUEST_TIMEOUT_MS: u64 = MAX_EXECUTION_REQUEST_TIMEOUT_SECS * 1_000;
pub const MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_SECS: u64 = 300;
pub const MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_MS: u64 =
    MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_SECS * 1_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ExecutionTimeouts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_byte_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_bytes_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<String>,
}

impl RequestBody {
    pub fn from_json(json_body: Value) -> Self {
        Self {
            json_body: Some(json_body),
            body_bytes_b64: None,
            body_ref: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProxySnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, alias = "proxy_url", skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

pub const TRANSPORT_BACKEND_REQWEST_RUSTLS: &str = "reqwest_rustls";
pub const TRANSPORT_BACKEND_HYPER_RUSTLS: &str = "hyper_rustls";
pub const TRANSPORT_BACKEND_BROWSER_WREQ: &str = "browser_wreq";
pub const TRANSPORT_HTTP_MODE_AUTO: &str = "auto";
pub const TRANSPORT_HTTP_MODE_HTTP1_ONLY: &str = "http1_only";
pub const TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE: &str = "h2c_prior_knowledge";
pub const TRANSPORT_POOL_SCOPE_KEY: &str = "key";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ResolvedTransportProfile {
    pub profile_id: String,
    pub backend: String,
    pub http_mode: String,
    pub pool_scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_fingerprint: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

impl Default for ResolvedTransportProfile {
    fn default() -> Self {
        Self {
            profile_id: String::new(),
            backend: TRANSPORT_BACKEND_REQWEST_RUSTLS.to_string(),
            http_mode: TRANSPORT_HTTP_MODE_AUTO.to_string(),
            pool_scope: TRANSPORT_POOL_SCOPE_KEY.to_string(),
            header_fingerprint: None,
            extra: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionPlan {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
    pub method: String,
    #[serde(alias = "upstream_url")]
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
    pub body: RequestBody,
    /// Whether the upstream API uses a streaming response protocol.
    #[serde(default)]
    pub stream: bool,
    pub client_api_format: String,
    pub provider_api_format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_profile: Option<ResolvedTransportProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeouts: Option<ExecutionTimeouts>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_plan_with_json_body() {
        let plan = ExecutionPlan {
            request_id: "req_123".into(),
            candidate_id: Some("cand_123".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov_123".into(),
            endpoint_id: "ep_123".into(),
            key_id: "key_123".into(),
            method: "POST".into(),
            url: "https://example.com/v1/chat/completions".into(),
            headers: BTreeMap::from([("authorization".into(), "Bearer test".into())]),
            content_type: Some("application/json".into()),
            content_encoding: Some("gzip".into()),
            body: RequestBody::from_json(serde_json::json!({"model":"gpt-test"})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-test".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(30_000),
                read_ms: Some(3_600_000),
                first_byte_ms: Some(30_000),
                ..ExecutionTimeouts::default()
            }),
        };

        let raw = serde_json::to_value(&plan).expect("plan should serialize");
        assert_eq!(raw["body"]["json_body"]["model"], "gpt-test");
        assert_eq!(raw["content_encoding"], "gzip");
        assert_eq!(raw["stream"], true);
    }

    #[test]
    fn deserializes_python_control_plane_plan_shape() {
        let raw = serde_json::json!({
            "request_id": "req-1",
            "candidate_id": null,
            "provider_name": "openai",
            "provider_id": "prov-1",
            "endpoint_id": "ep-1",
            "key_id": "key-1",
            "method": "POST",
            "url": "https://example.com/v1/chat/completions",
            "headers": {"content-type": "application/json"},
            "content_encoding": "gzip",
            "body": {"json_body": {"model": "gpt-4.1"}},
            "stream": false,
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:chat",
            "model_name": "gpt-4.1",
            "proxy": {
                "enabled": true,
                "mode": "direct",
                "label": "no-proxy",
                "url": "http://proxy.internal"
            },
            "timeouts": {
                "connect_ms": 10000,
                "read_ms": 30000,
                "write_ms": 30000,
                "pool_ms": 10000,
                "total_ms": 300000
            }
        });

        let plan: ExecutionPlan =
            serde_json::from_value(raw).expect("python payload should deserialize");
        assert_eq!(plan.url, "https://example.com/v1/chat/completions");
        assert_eq!(plan.candidate_id, None);
        assert_eq!(plan.provider_name.as_deref(), Some("openai"));
        assert_eq!(plan.model_name.as_deref(), Some("gpt-4.1"));
        assert_eq!(plan.content_encoding.as_deref(), Some("gzip"));
        assert_eq!(
            plan.proxy.as_ref().and_then(|proxy| proxy.url.as_deref()),
            Some("http://proxy.internal")
        );
        assert_eq!(
            plan.timeouts
                .as_ref()
                .and_then(|timeouts| timeouts.total_ms),
            Some(300_000)
        );
    }
}
