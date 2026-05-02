use std::collections::BTreeMap;

use aether_ai_formats::api::ExecutionRuntimeAuthContext;
use aether_scheduler_core::SchedulerRankingOutcome;
use serde_json::{Map, Value};

use crate::append_ai_ranking_metadata_to_object;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AiRequestOrigin {
    pub client_ip: Option<String>,
    pub user_agent: Option<String>,
}

pub struct AiExecutionReportContextParts<'a> {
    pub auth_context: &'a ExecutionRuntimeAuthContext,
    pub request_id: &'a str,
    pub candidate_id: &'a str,
    pub candidate_index: u32,
    pub retry_index: u32,
    pub pool_key_index: Option<u32>,
    pub model: &'a str,
    pub provider_name: &'a str,
    pub provider_id: &'a str,
    pub endpoint_id: &'a str,
    pub key_id: &'a str,
    pub key_name: Option<&'a str>,
    pub model_id: Option<&'a str>,
    pub global_model_id: Option<&'a str>,
    pub global_model_name: Option<&'a str>,
    pub provider_api_format: &'a str,
    pub client_api_format: &'a str,
    pub mapped_model: Option<&'a str>,
    pub candidate_group_id: Option<&'a str>,
    pub ranking: Option<&'a SchedulerRankingOutcome>,
    pub upstream_url: Option<&'a str>,
    pub header_rules: Option<&'a Value>,
    pub body_rules: Option<&'a Value>,
    pub provider_request_method: Option<Value>,
    pub provider_request_headers: Option<&'a BTreeMap<String, String>>,
    pub original_headers: &'a BTreeMap<String, String>,
    pub original_request_body: Option<Value>,
    pub request_origin: AiRequestOrigin,
    pub client_requested_stream: bool,
    pub upstream_is_stream: bool,
    pub has_envelope: bool,
    pub needs_conversion: bool,
    pub extra_fields: Map<String, Value>,
}

pub fn build_ai_execution_report_context(parts: AiExecutionReportContextParts<'_>) -> Value {
    let mut object = Map::new();
    object.insert(
        "user_id".to_string(),
        Value::String(parts.auth_context.user_id.clone()),
    );
    object.insert(
        "api_key_id".to_string(),
        Value::String(parts.auth_context.api_key_id.clone()),
    );
    object.insert(
        "api_key_is_standalone".to_string(),
        Value::Bool(parts.auth_context.api_key_is_standalone),
    );
    object.insert(
        "username".to_string(),
        parts
            .auth_context
            .username
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    object.insert(
        "api_key_name".to_string(),
        parts
            .auth_context
            .api_key_name
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    object.insert(
        "request_id".to_string(),
        Value::String(parts.request_id.to_string()),
    );
    object.insert(
        "candidate_id".to_string(),
        Value::String(parts.candidate_id.to_string()),
    );
    object.insert(
        "candidate_index".to_string(),
        Value::Number(parts.candidate_index.into()),
    );
    object.insert(
        "retry_index".to_string(),
        Value::Number(parts.retry_index.into()),
    );
    object.insert("model".to_string(), Value::String(parts.model.to_string()));
    object.insert(
        "provider_name".to_string(),
        Value::String(parts.provider_name.to_string()),
    );
    object.insert(
        "provider_id".to_string(),
        Value::String(parts.provider_id.to_string()),
    );
    object.insert(
        "endpoint_id".to_string(),
        Value::String(parts.endpoint_id.to_string()),
    );
    object.insert(
        "key_id".to_string(),
        Value::String(parts.key_id.to_string()),
    );
    object.insert(
        "provider_api_format".to_string(),
        Value::String(parts.provider_api_format.to_string()),
    );
    object.insert(
        "client_api_format".to_string(),
        Value::String(parts.client_api_format.to_string()),
    );
    object.insert(
        "original_headers".to_string(),
        serde_json::to_value(parts.original_headers).expect("control headers should serialize"),
    );
    object.insert(
        "original_request_body".to_string(),
        parts.original_request_body.unwrap_or(Value::Null),
    );
    if let Some(client_ip) = parts.request_origin.client_ip {
        object.insert("client_ip".to_string(), Value::String(client_ip));
    }
    if let Some(user_agent) = parts.request_origin.user_agent {
        object.insert("user_agent".to_string(), Value::String(user_agent));
    }
    object.insert(
        "client_requested_stream".to_string(),
        Value::Bool(parts.client_requested_stream),
    );
    object.insert(
        "upstream_is_stream".to_string(),
        Value::Bool(parts.upstream_is_stream),
    );
    object.insert("has_envelope".to_string(), Value::Bool(parts.has_envelope));
    object.insert(
        "needs_conversion".to_string(),
        Value::Bool(parts.needs_conversion),
    );

    if let Some(key_name) = parts.key_name {
        object.insert("key_name".to_string(), Value::String(key_name.to_string()));
    }
    if let Some(model_id) = parts.model_id {
        object.insert("model_id".to_string(), Value::String(model_id.to_string()));
    }
    if let Some(global_model_id) = parts.global_model_id {
        object.insert(
            "global_model_id".to_string(),
            Value::String(global_model_id.to_string()),
        );
    }
    if let Some(global_model_name) = parts.global_model_name {
        object.insert(
            "global_model_name".to_string(),
            Value::String(global_model_name.to_string()),
        );
    }
    if let Some(mapped_model) = parts.mapped_model {
        object.insert(
            "mapped_model".to_string(),
            Value::String(mapped_model.to_string()),
        );
    }
    if let Some(candidate_group_id) = parts.candidate_group_id {
        object.insert(
            "candidate_group_id".to_string(),
            Value::String(candidate_group_id.to_string()),
        );
    }
    if let Some(ranking) = parts.ranking {
        append_ai_ranking_metadata_to_object(&mut object, ranking);
    }
    if let Some(upstream_url) = parts.upstream_url {
        object.insert(
            "upstream_url".to_string(),
            Value::String(upstream_url.to_string()),
        );
    }
    if let Some(header_rules) = parts.header_rules {
        object.insert("header_rules".to_string(), header_rules.clone());
    }
    if let Some(body_rules) = parts.body_rules {
        object.insert("body_rules".to_string(), body_rules.clone());
    }
    if let Some(provider_request_method) = parts.provider_request_method {
        object.insert(
            "provider_request_method".to_string(),
            provider_request_method,
        );
    }
    if let Some(provider_request_headers) = parts.provider_request_headers {
        object.insert(
            "provider_request_headers".to_string(),
            serde_json::to_value(provider_request_headers)
                .expect("provider request headers should serialize"),
        );
    }
    if let Some(pool_key_index) = parts.pool_key_index {
        object.insert(
            "pool_key_index".to_string(),
            Value::Number(pool_key_index.into()),
        );
    }

    object.extend(parts.extra_fields);
    Value::Object(object)
}

pub fn provider_stream_event_api_format_for_provider_type(
    provider_type: &str,
) -> Option<&'static str> {
    match provider_type.trim().to_ascii_lowercase().as_str() {
        "codex" => Some("openai:responses"),
        _ => None,
    }
}

pub fn insert_provider_stream_event_api_format(
    extra_fields: &mut Map<String, Value>,
    provider_type: &str,
) {
    if let Some(api_format) = provider_stream_event_api_format_for_provider_type(provider_type) {
        extra_fields.insert(
            "provider_stream_event_api_format".to_string(),
            Value::String(api_format.to_string()),
        );
    }
}

pub fn build_ai_report_context_original_request_echo(
    body_json: Option<&Value>,
    body_bytes_b64: Option<&str>,
) -> Option<Value> {
    if let Some(body_bytes_b64) = body_bytes_b64
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(serde_json::json!({ "body_bytes_b64": body_bytes_b64 }));
    }

    body_json.filter(|body| !body.is_null()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_scheduler_core::{SchedulerPriorityMode, SchedulerRankingMode};
    use serde_json::json;

    fn sample_auth_context() -> ExecutionRuntimeAuthContext {
        ExecutionRuntimeAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "key-1".to_string(),
            username: Some("alice".to_string()),
            api_key_name: Some("primary".to_string()),
            balance_remaining: Some(42.0),
            access_allowed: true,
            api_key_is_standalone: false,
        }
    }

    #[test]
    fn report_context_builds_core_execution_fields() {
        let auth_context = sample_auth_context();
        let original_headers = BTreeMap::from([("x-trace-id".to_string(), "trace-a".to_string())]);
        let provider_headers =
            BTreeMap::from([("authorization".to_string(), "Bearer token".to_string())]);
        let mut extra_fields = Map::new();
        extra_fields.insert("extra".to_string(), json!("value"));
        let ranking = SchedulerRankingOutcome {
            original_index: 2,
            ranking_index: 1,
            priority_mode: SchedulerPriorityMode::Provider,
            ranking_mode: SchedulerRankingMode::CacheAffinity,
            priority_slot: 7,
            promoted_by: Some("cached_affinity"),
            demoted_by: None,
        };

        let report = build_ai_execution_report_context(AiExecutionReportContextParts {
            auth_context: &auth_context,
            request_id: "trace-a",
            candidate_id: "candidate-a",
            candidate_index: 3,
            retry_index: 1,
            pool_key_index: Some(0),
            model: "gpt-5",
            provider_name: "RightCode",
            provider_id: "provider-1",
            endpoint_id: "endpoint-1",
            key_id: "key-1",
            key_name: Some("primary"),
            model_id: Some("model-1"),
            global_model_id: Some("global-1"),
            global_model_name: Some("GPT-5"),
            provider_api_format: "openai:responses",
            client_api_format: "openai:chat",
            mapped_model: Some("gpt-5"),
            candidate_group_id: Some("group-1"),
            ranking: Some(&ranking),
            upstream_url: Some("https://example.com/v1/responses"),
            header_rules: Some(&json!({"set": []})),
            body_rules: None,
            provider_request_method: Some(json!("POST")),
            provider_request_headers: Some(&provider_headers),
            original_headers: &original_headers,
            original_request_body: Some(json!({"model": "gpt-5"})),
            request_origin: AiRequestOrigin {
                client_ip: Some("127.0.0.1".to_string()),
                user_agent: Some("test-agent".to_string()),
            },
            client_requested_stream: false,
            upstream_is_stream: true,
            has_envelope: false,
            needs_conversion: true,
            extra_fields,
        });

        assert_eq!(report["user_id"], "user-1");
        assert_eq!(report["candidate_index"], 3);
        assert_eq!(report["retry_index"], 1);
        assert_eq!(report["pool_key_index"], 0);
        assert_eq!(report["original_headers"]["x-trace-id"], "trace-a");
        assert_eq!(report["original_request_body"]["model"], "gpt-5");
        assert_eq!(report["ranking_index"], 1);
        assert_eq!(
            report["provider_request_headers"]["authorization"],
            "Bearer token"
        );
        assert_eq!(report["extra"], "value");
    }

    #[test]
    fn provider_stream_event_api_format_is_codex_only() {
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("codex"),
            Some("openai:responses")
        );
        assert_eq!(
            provider_stream_event_api_format_for_provider_type(" CODEX "),
            Some("openai:responses")
        );
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("openai"),
            None
        );
    }

    #[test]
    fn original_request_echo_preserves_full_request_body() {
        let body = json!({
            "messages": [{"role": "user", "content": "large payload should be omitted"}],
            "service_tier": "default",
            "instructions": "Be concise.",
            "thinking": {"type": "enabled", "budget_tokens": 512},
            "metadata": {"trace": "keep"},
            "body_bytes_b64": "aGVsbG8=",
        });

        let echo = build_ai_report_context_original_request_echo(Some(&body), None)
            .expect("echo should be produced");

        assert_eq!(echo, body);
    }

    #[test]
    fn original_request_echo_prefers_binary_body_bytes() {
        let echo = build_ai_report_context_original_request_echo(
            Some(&json!({"ignored": true})),
            Some("aGVsbG8="),
        )
        .expect("echo should be produced");

        assert_eq!(echo, json!({"body_bytes_b64": "aGVsbG8="}));
    }
}
