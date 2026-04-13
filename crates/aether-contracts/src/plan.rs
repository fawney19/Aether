use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER: &str = "x-aether-execution-follow-redirects";
pub const EXECUTION_REQUEST_HTTP1_ONLY_HEADER: &str = "x-aether-execution-http1-only";

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
    #[serde(default)]
    pub stream: bool,
    pub client_api_format: String,
    pub provider_api_format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_profile: Option<String>,
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
            tls_profile: Some("chrome".into()),
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
