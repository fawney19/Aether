use std::collections::BTreeMap;

use aether_contracts::{ExecutionPlan, ExecutionTimeouts, ProxySnapshot};
use serde::{Deserialize, Serialize};

use crate::contracts::ExecutionRuntimeAuthContext;

#[derive(Debug, Serialize)]
pub struct GatewayControlPlanRequest {
    pub trace_id: String,
    pub method: String,
    pub path: String,
    pub query_string: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub body_json: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    pub auth_context: Option<ExecutionRuntimeAuthContext>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GatewayControlPlanResponse {
    pub action: String,
    #[serde(default)]
    pub plan_kind: Option<String>,
    #[serde(default)]
    pub plan: Option<ExecutionPlan>,
    #[serde(default)]
    pub report_kind: Option<String>,
    #[serde(default)]
    pub report_context: Option<serde_json::Value>,
    #[serde(default)]
    pub auth_context: Option<ExecutionRuntimeAuthContext>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GatewayControlSyncDecisionResponse {
    pub action: String,
    #[serde(default)]
    pub decision_kind: Option<String>,
    #[serde(default)]
    pub execution_strategy: Option<String>,
    #[serde(default)]
    pub conversion_mode: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub candidate_id: Option<String>,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub upstream_base_url: Option<String>,
    #[serde(default)]
    pub upstream_url: Option<String>,
    #[serde(default)]
    pub provider_request_method: Option<String>,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub auth_value: Option<String>,
    #[serde(default)]
    pub provider_api_format: Option<String>,
    #[serde(default)]
    pub client_api_format: Option<String>,
    #[serde(default)]
    pub provider_contract: Option<String>,
    #[serde(default)]
    pub client_contract: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub mapped_model: Option<String>,
    #[serde(default)]
    pub prompt_cache_key: Option<String>,
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub provider_request_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub provider_request_body: Option<serde_json::Value>,
    #[serde(default)]
    pub provider_request_body_base64: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub proxy: Option<ProxySnapshot>,
    #[serde(default)]
    pub tls_profile: Option<String>,
    #[serde(default)]
    pub timeouts: Option<ExecutionTimeouts>,
    #[serde(default)]
    pub upstream_is_stream: bool,
    #[serde(default)]
    pub report_kind: Option<String>,
    #[serde(default)]
    pub report_context: Option<serde_json::Value>,
    #[serde(default)]
    pub auth_context: Option<ExecutionRuntimeAuthContext>,
}

#[derive(Debug)]
pub struct LocalSyncPlanAndReport {
    pub plan: ExecutionPlan,
    pub report_kind: Option<String>,
    pub report_context: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct LocalStreamPlanAndReport {
    pub plan: ExecutionPlan,
    pub report_kind: Option<String>,
    pub report_context: Option<serde_json::Value>,
}

#[allow(clippy::too_many_arguments)]
pub fn build_gateway_control_plan_request(
    trace_id: &str,
    method: &str,
    path: &str,
    query_string: Option<&str>,
    headers: BTreeMap<String, String>,
    body_json: serde_json::Value,
    body_base64: Option<String>,
    auth_context: Option<ExecutionRuntimeAuthContext>,
) -> GatewayControlPlanRequest {
    GatewayControlPlanRequest {
        trace_id: trace_id.to_string(),
        method: method.to_string(),
        path: path.to_string(),
        query_string: query_string.map(ToOwned::to_owned),
        headers,
        body_json,
        body_base64,
        auth_context,
    }
}

pub fn augment_sync_report_context(
    report_context: Option<serde_json::Value>,
    provider_request_headers: &BTreeMap<String, String>,
    _provider_request_body: &serde_json::Value,
) -> serde_json::Result<Option<serde_json::Value>> {
    let mut report_context = match report_context {
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => serde_json::Map::new(),
        None => serde_json::Map::new(),
    };

    report_context.insert(
        "provider_request_headers".to_string(),
        serde_json::to_value(provider_request_headers)?,
    );

    Ok(Some(serde_json::Value::Object(report_context)))
}

fn decision_has_exact_provider_request(payload: &GatewayControlSyncDecisionResponse) -> bool {
    !payload.provider_request_headers.is_empty()
        && (payload.provider_request_body.is_some()
            || payload
                .provider_request_body_base64
                .as_ref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false))
}

pub fn generic_decision_missing_exact_provider_request(
    payload: &GatewayControlSyncDecisionResponse,
) -> bool {
    !decision_has_exact_provider_request(payload)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        augment_sync_report_context, build_gateway_control_plan_request,
        generic_decision_missing_exact_provider_request, ExecutionRuntimeAuthContext,
        GatewayControlSyncDecisionResponse,
    };

    #[test]
    fn generic_decision_detects_missing_exact_provider_request() {
        let payload = GatewayControlSyncDecisionResponse {
            action: "local".to_string(),
            decision_kind: Some("sync".to_string()),
            execution_strategy: None,
            conversion_mode: None,
            request_id: None,
            candidate_id: None,
            provider_name: None,
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            upstream_base_url: None,
            upstream_url: None,
            provider_request_method: None,
            auth_header: None,
            auth_value: None,
            provider_api_format: None,
            client_api_format: None,
            provider_contract: None,
            client_contract: None,
            model_name: None,
            mapped_model: None,
            prompt_cache_key: None,
            extra_headers: Default::default(),
            provider_request_headers: Default::default(),
            provider_request_body: None,
            provider_request_body_base64: None,
            content_type: None,
            proxy: None,
            tls_profile: None,
            timeouts: None,
            upstream_is_stream: false,
            report_kind: None,
            report_context: None,
            auth_context: None,
        };

        assert!(generic_decision_missing_exact_provider_request(&payload));
    }

    #[test]
    fn augment_sync_report_context_attaches_provider_request_headers_only() {
        let report_context = augment_sync_report_context(
            Some(serde_json::json!({"trace_id": "abc"})),
            &BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
            &serde_json::json!({"model": "gpt-5"}),
        )
        .expect("context should serialize")
        .expect("context should exist");

        assert_eq!(
            report_context.get("trace_id"),
            Some(&serde_json::json!("abc"))
        );
        assert_eq!(
            report_context
                .get("provider_request_headers")
                .and_then(|value| value.get("content-type")),
            Some(&serde_json::json!("application/json"))
        );
        assert!(report_context.get("provider_request_body").is_none());
    }

    #[test]
    fn build_gateway_control_plan_request_preserves_request_shape() {
        let payload = build_gateway_control_plan_request(
            "trace-123",
            "POST",
            "/v1/chat/completions",
            Some("stream=true"),
            BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
            serde_json::json!({"model": "gpt-5"}),
            Some("eyJmb28iOiJiYXIifQ==".to_string()),
            Some(ExecutionRuntimeAuthContext {
                user_id: "user-1".to_string(),
                api_key_id: "key-1".to_string(),
                username: None,
                api_key_name: None,
                balance_remaining: Some(12.5),
                access_allowed: true,
            }),
        );

        assert_eq!(payload.trace_id, "trace-123");
        assert_eq!(payload.method, "POST");
        assert_eq!(payload.path, "/v1/chat/completions");
        assert_eq!(payload.query_string.as_deref(), Some("stream=true"));
        assert_eq!(
            payload.headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(payload.body_json, serde_json::json!({"model": "gpt-5"}));
        assert_eq!(payload.body_base64.as_deref(), Some("eyJmb28iOiJiYXIifQ=="));
        assert_eq!(
            payload
                .auth_context
                .as_ref()
                .map(|ctx| ctx.user_id.as_str()),
            Some("user-1")
        );
    }
}
