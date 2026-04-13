use std::collections::BTreeMap;

use serde_json::Value;
use url::form_urlencoded;

use crate::ai_pipeline::conversion::{request_conversion_kind, RequestConversionKind};
use crate::ai_pipeline::transport::antigravity::{
    build_antigravity_v1internal_url, AntigravityRequestUrlAction,
};
use crate::ai_pipeline::transport::apply_local_body_rules;
use crate::ai_pipeline::transport::url::{
    build_claude_messages_url, build_gemini_content_url, build_openai_chat_url,
    build_openai_cli_url, build_passthrough_path_url,
};
use crate::ai_pipeline::{
    apply_codex_openai_cli_special_body_edits, apply_openai_compact_special_body_edits,
    build_cross_format_openai_cli_request_body as pipeline_build_cross_format_openai_cli_request_body,
    build_local_openai_cli_request_body as pipeline_build_local_openai_cli_request_body,
    GatewayProviderTransportSnapshot,
};

pub(crate) fn build_local_openai_cli_request_body(
    body_json: &Value,
    mapped_model: &str,
    require_streaming: bool,
    provider_type: &str,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) -> Option<Value> {
    let mut provider_request_body =
        pipeline_build_local_openai_cli_request_body(body_json, mapped_model, require_streaming)?;
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

pub(crate) fn build_cross_format_openai_cli_request_body(
    body_json: &Value,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    provider_type: &str,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) -> Option<Value> {
    let mut provider_request_body = pipeline_build_cross_format_openai_cli_request_body(
        body_json,
        mapped_model,
        client_api_format,
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

pub(crate) fn build_local_openai_cli_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    compact: bool,
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
        None => Some(build_openai_cli_url(
            &transport.endpoint.base_url,
            parts.uri.query(),
            compact,
        )),
    }
}

pub(crate) fn build_cross_format_openai_cli_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<String> {
    let conversion_kind = request_conversion_kind(client_api_format, provider_api_format)?;
    if transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("antigravity")
    {
        let query = parts.uri.query().map(|query| {
            form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect::<BTreeMap<String, String>>()
        });
        return build_antigravity_v1internal_url(
            &transport.endpoint.base_url,
            if upstream_is_stream {
                AntigravityRequestUrlAction::StreamGenerateContent
            } else {
                AntigravityRequestUrlAction::GenerateContent
            },
            query.as_ref(),
        );
    }

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
            RequestConversionKind::ToOpenAIChat => Some(build_openai_chat_url(
                &transport.endpoint.base_url,
                parts.uri.query(),
            )),
            RequestConversionKind::ToOpenAIFamilyCli => Some(build_openai_cli_url(
                &transport.endpoint.base_url,
                parts.uri.query(),
                false,
            )),
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
        },
    }
}
