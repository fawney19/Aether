use serde_json::Value;

use crate::ai_serving::transport::apply_standard_provider_request_body_rules_with_request_headers;
use crate::ai_serving::{
    apply_codex_openai_responses_chat_body_edits,
    apply_openai_responses_compact_special_body_edits,
    build_cross_format_openai_chat_request_body_with_provider_context as surface_build_cross_format_openai_chat_request_body,
    build_local_openai_chat_request_body_with_model_directives as surface_build_local_openai_chat_request_body,
    GatewayProviderTransportSnapshot,
};

use super::{
    enforce_provider_body_stream_policy, request_requires_body_stream_field,
    validate_final_openai_provider_request,
};

pub(crate) fn build_local_openai_chat_request_body(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
    force_body_stream_field: bool,
    body_rules: Option<&Value>,
    request_headers: &http::HeaderMap,
    enable_model_directives: bool,
) -> Option<Value> {
    let provider_request_body = surface_build_local_openai_chat_request_body(
        body_json,
        mapped_model,
        upstream_is_stream,
        enable_model_directives,
    )?;
    let mut provider_request_body =
        apply_standard_provider_request_body_rules_with_request_headers(
            provider_request_body,
            body_rules,
            body_json,
            request_headers,
        )?;
    enforce_provider_body_stream_policy(
        &mut provider_request_body,
        "openai:chat",
        upstream_is_stream,
        request_requires_body_stream_field(body_json, force_body_stream_field),
    );
    validate_final_openai_provider_request(
        "openai:chat",
        mapped_model,
        body_json,
        &provider_request_body,
    )?;
    Some(provider_request_body)
}

pub(crate) fn build_local_openai_chat_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    crate::ai_serving::transport::build_local_openai_chat_upstream_url(transport, parts.uri.query())
}

pub(crate) fn build_cross_format_openai_chat_request_body(
    body_json: &Value,
    mapped_model: &str,
    provider_type: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    force_body_stream_field: bool,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
    request_headers: &http::HeaderMap,
    enable_model_directives: bool,
    strict_format_conversion: bool,
) -> Option<Value> {
    let provider_request_body = surface_build_cross_format_openai_chat_request_body(
        body_json,
        mapped_model,
        provider_type,
        provider_api_format,
        upstream_is_stream,
        enable_model_directives,
        user_api_key_id,
        strict_format_conversion,
    )?;
    let mut provider_request_body =
        apply_standard_provider_request_body_rules_with_request_headers(
            provider_request_body,
            body_rules,
            body_json,
            request_headers,
        )?;
    apply_codex_openai_responses_chat_body_edits(
        &mut provider_request_body,
        provider_type,
        provider_api_format,
        body_rules,
        user_api_key_id,
    );
    apply_openai_responses_compact_special_body_edits(
        &mut provider_request_body,
        provider_api_format,
    );
    enforce_provider_body_stream_policy(
        &mut provider_request_body,
        provider_api_format,
        upstream_is_stream,
        request_requires_body_stream_field(body_json, force_body_stream_field),
    );
    validate_final_openai_provider_request(
        provider_api_format,
        mapped_model,
        body_json,
        &provider_request_body,
    )?;
    Some(provider_request_body)
}

pub(crate) fn build_cross_format_openai_chat_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<String> {
    crate::ai_serving::transport::build_cross_format_openai_chat_upstream_url(
        transport,
        mapped_model,
        provider_api_format,
        upstream_is_stream,
        parts.uri.query(),
    )
}

#[cfg(test)]
mod antigravity_schema_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn antigravity_chat_route_preserves_tool_schema_and_alternate_responses_shape() {
        let schema = json!({"type": "object", "properties": {"mode": {"const": "fast"}}});
        let body = json!({"model": "client", "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "probe", "parameters": schema}}]});
        let responses_body = json!({"model": "client", "input": "hi",
            "tools": [{"type": "function", "name": "probe", "parameters": schema}]});
        for input in [body, responses_body] {
            for provider in ["antigravity", "gemini"] {
                let output = build_cross_format_openai_chat_request_body(
                    &input,
                    "claude-test",
                    provider,
                    "gemini:generate_content",
                    true,
                    false,
                    None,
                    None,
                    &http::HeaderMap::new(),
                    false,
                    false,
                )
                .unwrap();
                let parameters = &output["tools"][0]["functionDeclarations"][0]["parameters"];
                assert_eq!(parameters == &schema, provider == "antigravity");
                assert!(output.get("stream").is_none());
                assert_eq!(output["contents"][0]["parts"][0]["text"], "hi");
            }
        }
    }
}
