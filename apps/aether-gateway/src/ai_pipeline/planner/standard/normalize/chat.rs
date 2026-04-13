use serde_json::Value;

use crate::ai_pipeline::conversion::{request_conversion_kind, RequestConversionKind};
use crate::ai_pipeline::transport::apply_local_body_rules;
use crate::ai_pipeline::transport::url::{
    build_claude_messages_url, build_gemini_content_url, build_openai_chat_url,
    build_openai_cli_url, build_passthrough_path_url,
};
use crate::ai_pipeline::{
    apply_codex_openai_cli_special_body_edits, apply_openai_compact_special_body_edits,
    build_cross_format_openai_chat_request_body as pipeline_build_cross_format_openai_chat_request_body,
    build_local_openai_chat_request_body as pipeline_build_local_openai_chat_request_body,
    GatewayProviderTransportSnapshot,
};

pub(crate) fn build_local_openai_chat_request_body(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
    body_rules: Option<&Value>,
) -> Option<Value> {
    let mut provider_request_body =
        pipeline_build_local_openai_chat_request_body(body_json, mapped_model, upstream_is_stream)?;
    if !apply_local_body_rules(&mut provider_request_body, body_rules, Some(body_json)) {
        return None;
    }
    Some(provider_request_body)
}

pub(crate) fn build_local_openai_chat_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
) -> Option<String> {
    let custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match custom_path {
        Some(path) => {
            build_passthrough_path_url(&transport.endpoint.base_url, path, parts.uri.query(), &[])
        }
        None => Some(build_openai_chat_url(
            &transport.endpoint.base_url,
            parts.uri.query(),
        )),
    }
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
    let mut provider_request_body = pipeline_build_cross_format_openai_chat_request_body(
        body_json,
        mapped_model,
        provider_api_format,
        upstream_is_stream,
    )?;
    if !apply_local_body_rules(&mut provider_request_body, body_rules, Some(body_json)) {
        return None;
    }
    apply_codex_openai_cli_special_body_edits(
        &mut provider_request_body,
        provider_type,
        provider_api_format,
        body_rules,
        user_api_key_id,
    );
    apply_openai_compact_special_body_edits(&mut provider_request_body, provider_api_format);
    Some(provider_request_body)
}

pub(crate) fn build_cross_format_openai_chat_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<String> {
    let conversion_kind = request_conversion_kind("openai:chat", provider_api_format)?;
    let custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match custom_path {
        Some(path) => {
            build_passthrough_path_url(&transport.endpoint.base_url, path, parts.uri.query(), &[])
        }
        None => match conversion_kind {
            RequestConversionKind::ToClaudeStandard => Some(build_claude_messages_url(
                &transport.endpoint.base_url,
                parts.uri.query(),
            )),
            RequestConversionKind::ToGeminiStandard => build_gemini_content_url(
                &transport.endpoint.base_url,
                mapped_model,
                upstream_is_stream,
                parts.uri.query(),
            ),
            RequestConversionKind::ToOpenAIFamilyCli => Some(build_openai_cli_url(
                &transport.endpoint.base_url,
                parts.uri.query(),
                false,
            )),
            _ => None,
        },
    }
}
