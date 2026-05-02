use std::collections::BTreeMap;

use serde::Serialize;

use crate::contracts::ExecutionRuntimeAuthContext;

#[derive(Debug, Serialize)]
pub struct AiControlPlanRequest {
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

#[allow(clippy::too_many_arguments)]
pub fn build_ai_control_plan_request(
    trace_id: &str,
    method: &str,
    path: &str,
    query_string: Option<&str>,
    headers: BTreeMap<String, String>,
    body_json: serde_json::Value,
    body_base64: Option<String>,
    auth_context: Option<ExecutionRuntimeAuthContext>,
) -> AiControlPlanRequest {
    AiControlPlanRequest {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{build_ai_control_plan_request, ExecutionRuntimeAuthContext};

    #[test]
    fn build_ai_control_plan_request_preserves_request_shape() {
        let payload = build_ai_control_plan_request(
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
                api_key_is_standalone: false,
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
