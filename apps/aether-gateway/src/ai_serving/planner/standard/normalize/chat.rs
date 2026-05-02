use serde_json::Value;

use crate::ai_serving::transport::apply_standard_provider_request_body_rules;
use crate::ai_serving::{
    apply_codex_openai_responses_special_body_edits,
    apply_openai_responses_compact_special_body_edits,
    build_cross_format_openai_chat_request_body as surface_build_cross_format_openai_chat_request_body,
    build_local_openai_chat_request_body as surface_build_local_openai_chat_request_body,
    GatewayProviderTransportSnapshot,
};

pub(crate) fn build_local_openai_chat_request_body(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
    body_rules: Option<&Value>,
) -> Option<Value> {
    let provider_request_body =
        surface_build_local_openai_chat_request_body(body_json, mapped_model, upstream_is_stream)?;
    apply_standard_provider_request_body_rules(provider_request_body, body_rules, body_json)
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
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) -> Option<Value> {
    let provider_request_body = surface_build_cross_format_openai_chat_request_body(
        body_json,
        mapped_model,
        provider_api_format,
        upstream_is_stream,
    )?;
    let mut provider_request_body =
        apply_standard_provider_request_body_rules(provider_request_body, body_rules, body_json)?;
    apply_codex_openai_responses_special_body_edits(
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
