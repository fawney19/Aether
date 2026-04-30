use std::collections::BTreeMap;

use aether_scheduler_core::SchedulerRankingOutcome;
use serde_json::{Map, Value};

use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::planner::candidate_metadata::append_ranking_metadata_to_object;
use crate::headers::RequestOrigin;
use crate::orchestration::ExecutionAttemptIdentity;

pub(crate) struct LocalExecutionReportContextParts<'a> {
    pub(crate) auth_context: &'a ExecutionRuntimeAuthContext,
    pub(crate) request_id: &'a str,
    pub(crate) candidate_id: &'a str,
    pub(crate) attempt_identity: ExecutionAttemptIdentity,
    pub(crate) model: &'a str,
    pub(crate) provider_name: &'a str,
    pub(crate) provider_id: &'a str,
    pub(crate) endpoint_id: &'a str,
    pub(crate) key_id: &'a str,
    pub(crate) key_name: Option<&'a str>,
    pub(crate) model_id: Option<&'a str>,
    pub(crate) global_model_id: Option<&'a str>,
    pub(crate) global_model_name: Option<&'a str>,
    pub(crate) provider_api_format: &'a str,
    pub(crate) client_api_format: &'a str,
    pub(crate) mapped_model: Option<&'a str>,
    pub(crate) candidate_group_id: Option<&'a str>,
    pub(crate) ranking: Option<&'a SchedulerRankingOutcome>,
    pub(crate) upstream_url: Option<&'a str>,
    pub(crate) header_rules: Option<&'a Value>,
    pub(crate) body_rules: Option<&'a Value>,
    pub(crate) provider_request_method: Option<Value>,
    pub(crate) provider_request_headers: Option<&'a BTreeMap<String, String>>,
    pub(crate) original_headers: &'a http::HeaderMap,
    pub(crate) request_origin: Option<RequestOrigin>,
    pub(crate) original_request_body_json: Option<&'a Value>,
    pub(crate) original_request_body_base64: Option<&'a str>,
    pub(crate) client_requested_stream: bool,
    pub(crate) upstream_is_stream: bool,
    pub(crate) has_envelope: bool,
    pub(crate) needs_conversion: bool,
    pub(crate) extra_fields: Map<String, Value>,
}

pub(crate) fn build_local_execution_report_context(
    parts: LocalExecutionReportContextParts<'_>,
) -> Value {
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
        Value::Number(parts.attempt_identity.candidate_index.into()),
    );
    object.insert(
        "retry_index".to_string(),
        Value::Number(parts.attempt_identity.retry_index.into()),
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
        serde_json::to_value(crate::ai_pipeline::collect_control_headers(
            parts.original_headers,
        ))
        .expect("control headers should serialize"),
    );
    object.insert(
        "original_request_body".to_string(),
        crate::ai_pipeline::build_report_context_original_request_echo(
            parts.original_request_body_json,
            parts.original_request_body_base64,
        )
        .unwrap_or(Value::Null),
    );
    let RequestOrigin {
        client_ip,
        user_agent,
    } = parts
        .request_origin
        .unwrap_or_else(|| crate::headers::request_origin_from_headers(parts.original_headers));
    if let Some(client_ip) = client_ip {
        object.insert("client_ip".to_string(), Value::String(client_ip));
    }
    if let Some(user_agent) = user_agent {
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
        append_ranking_metadata_to_object(&mut object, ranking);
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
    if let Some(pool_key_index) = parts.attempt_identity.pool_key_index {
        object.insert(
            "pool_key_index".to_string(),
            Value::Number(pool_key_index.into()),
        );
    }

    object.extend(parts.extra_fields);
    Value::Object(object)
}

pub(crate) fn provider_stream_event_api_format_for_provider_type(
    provider_type: &str,
) -> Option<&'static str> {
    match provider_type.trim().to_ascii_lowercase().as_str() {
        "codex" => Some("openai:responses"),
        _ => None,
    }
}

pub(crate) fn insert_provider_stream_event_api_format(
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Map, Value};

    use super::{
        build_local_execution_report_context, provider_stream_event_api_format_for_provider_type,
        LocalExecutionReportContextParts,
    };
    use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
    use crate::headers::RequestOrigin;
    use crate::orchestration::ExecutionAttemptIdentity;

    #[test]
    fn codex_provider_uses_openai_responses_stream_event_format() {
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("codex"),
            Some("openai:responses")
        );
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("CODEX"),
            Some("openai:responses")
        );
    }

    #[test]
    fn ordinary_providers_do_not_override_stream_event_format() {
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("openai"),
            None
        );
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("anthropic"),
            None
        );
    }

    #[test]
    fn local_execution_report_context_records_request_origin() {
        let auth_context = ExecutionRuntimeAuthContext {
            user_id: "user-1".to_string(),
            api_key_id: "api-key-1".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: None,
            access_allowed: true,
            api_key_is_standalone: false,
        };
        let original_headers = http::HeaderMap::new();
        let provider_request_headers = BTreeMap::new();

        let report_context =
            build_local_execution_report_context(LocalExecutionReportContextParts {
                auth_context: &auth_context,
                request_id: "trace-1",
                candidate_id: "candidate-1",
                attempt_identity: ExecutionAttemptIdentity::new(0, 0),
                model: "gpt-5",
                provider_name: "OpenAI",
                provider_id: "provider-1",
                endpoint_id: "endpoint-1",
                key_id: "key-1",
                key_name: None,
                model_id: None,
                global_model_id: None,
                global_model_name: None,
                provider_api_format: "openai:chat",
                client_api_format: "openai:chat",
                mapped_model: None,
                candidate_group_id: None,
                ranking: None,
                upstream_url: None,
                header_rules: None,
                body_rules: None,
                provider_request_method: None,
                provider_request_headers: Some(&provider_request_headers),
                original_headers: &original_headers,
                request_origin: Some(RequestOrigin {
                    client_ip: Some("203.0.113.8".to_string()),
                    user_agent: Some("Claude-Code/1.0".to_string()),
                }),
                original_request_body_json: Some(&json!({"model": "gpt-5"})),
                original_request_body_base64: None,
                client_requested_stream: false,
                upstream_is_stream: false,
                has_envelope: false,
                needs_conversion: false,
                extra_fields: Map::new(),
            });

        assert_eq!(
            report_context["client_ip"],
            Value::String("203.0.113.8".to_string())
        );
        assert_eq!(
            report_context["user_agent"],
            Value::String("Claude-Code/1.0".to_string())
        );
    }
}
