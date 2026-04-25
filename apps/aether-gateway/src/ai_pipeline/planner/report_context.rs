use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
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
    pub(crate) upstream_url: Option<&'a str>,
    pub(crate) header_rules: Option<&'a Value>,
    pub(crate) body_rules: Option<&'a Value>,
    pub(crate) provider_request_method: Option<Value>,
    pub(crate) provider_request_headers: Option<&'a BTreeMap<String, String>>,
    pub(crate) original_headers: &'a http::HeaderMap,
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
        "codex" => Some("openai:cli"),
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
    use super::provider_stream_event_api_format_for_provider_type;

    #[test]
    fn codex_provider_uses_openai_cli_stream_event_format() {
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("codex"),
            Some("openai:cli")
        );
        assert_eq!(
            provider_stream_event_api_format_for_provider_type("CODEX"),
            Some("openai:cli")
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
}
