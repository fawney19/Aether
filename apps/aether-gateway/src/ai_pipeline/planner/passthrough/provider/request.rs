use std::collections::BTreeMap;

use serde_json::Value;
use url::form_urlencoded;

use crate::ai_pipeline::GatewayProviderTransportSnapshot;

use super::{
    apply_local_body_rules, build_antigravity_v1internal_url, build_claude_code_messages_url,
    build_claude_messages_url, build_gemini_content_url,
    build_kiro_generate_assistant_response_url, build_kiro_provider_request_body,
    build_passthrough_path_url, build_vertex_api_key_gemini_content_url,
    resolve_local_vertex_api_key_query_auth, sanitize_claude_code_request_body,
    AntigravityRequestUrlAction, LocalSameFormatProviderFamily, LocalSameFormatProviderSpec,
};

pub(super) fn build_same_format_provider_request_body(
    body_json: &Value,
    mapped_model: &str,
    spec: LocalSameFormatProviderSpec,
    body_rules: Option<&Value>,
    upstream_is_stream: bool,
    kiro_auth: Option<&crate::ai_pipeline::transport::kiro::KiroRequestAuth>,
    is_claude_code: bool,
) -> Option<Value> {
    if let Some(kiro_auth) = kiro_auth {
        return build_kiro_provider_request_body(
            body_json,
            mapped_model,
            &kiro_auth.auth_config,
            body_rules,
        );
    }

    let request_body_object = body_json.as_object()?;
    let mut provider_request_body = serde_json::Map::from_iter(
        request_body_object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    match spec.family {
        LocalSameFormatProviderFamily::Standard => {
            provider_request_body
                .insert("model".to_string(), Value::String(mapped_model.to_string()));
            if upstream_is_stream {
                provider_request_body.insert("stream".to_string(), Value::Bool(true));
            }
        }
        LocalSameFormatProviderFamily::Gemini => {
            provider_request_body.remove("model");
        }
    }
    let mut provider_request_body = Value::Object(provider_request_body);
    if is_claude_code {
        sanitize_claude_code_request_body(&mut provider_request_body);
    }
    if !apply_local_body_rules(&mut provider_request_body, body_rules, Some(body_json)) {
        return None;
    }
    Some(provider_request_body)
}

pub(super) fn build_same_format_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    spec: LocalSameFormatProviderSpec,
    upstream_is_stream: bool,
    kiro_auth: Option<&crate::ai_pipeline::transport::kiro::KiroRequestAuth>,
) -> Option<String> {
    if let Some(kiro_auth) = kiro_auth {
        return build_kiro_generate_assistant_response_url(
            &transport.endpoint.base_url,
            parts.uri.query(),
            Some(kiro_auth.auth_config.effective_api_region()),
        );
    }
    if transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("claude_code")
    {
        return Some(build_claude_code_messages_url(
            &transport.endpoint.base_url,
            parts.uri.query(),
        ));
    }
    if transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("vertex_ai")
    {
        let auth = resolve_local_vertex_api_key_query_auth(transport)?;
        return build_vertex_api_key_gemini_content_url(
            mapped_model,
            upstream_is_stream,
            &auth.value,
            parts.uri.query(),
        );
    }
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

    if let Some(path) = custom_path {
        let blocked_keys = match spec.family {
            LocalSameFormatProviderFamily::Standard => &[][..],
            LocalSameFormatProviderFamily::Gemini => &["key"][..],
        };
        let url = build_passthrough_path_url(
            &transport.endpoint.base_url,
            path,
            parts.uri.query(),
            blocked_keys,
        )?;
        return Some(maybe_add_gemini_stream_alt_sse(url, spec));
    }

    let url = match spec.family {
        LocalSameFormatProviderFamily::Standard => Some(build_claude_messages_url(
            &transport.endpoint.base_url,
            parts.uri.query(),
        )),
        LocalSameFormatProviderFamily::Gemini => build_gemini_content_url(
            &transport.endpoint.base_url,
            mapped_model,
            spec.require_streaming,
            parts.uri.query(),
        ),
    }?;

    Some(maybe_add_gemini_stream_alt_sse(url, spec))
}

pub(super) fn extract_gemini_model_from_path(path: &str) -> Option<String> {
    let (_, suffix) = path.split_once("/models/")?;
    let model = suffix
        .split_once(':')
        .map(|(value, _)| value)
        .unwrap_or(suffix);
    let model = model.trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

fn maybe_add_gemini_stream_alt_sse(
    upstream_url: String,
    spec: LocalSameFormatProviderSpec,
) -> String {
    if spec.family != LocalSameFormatProviderFamily::Gemini || !spec.require_streaming {
        return upstream_url;
    }

    let has_alt = upstream_url
        .split_once('?')
        .map(|(_, query)| {
            form_urlencoded::parse(query.as_bytes())
                .any(|(key, _)| key.as_ref().eq_ignore_ascii_case("alt"))
        })
        .unwrap_or(false);
    if has_alt {
        return upstream_url;
    }

    if upstream_url.contains('?') {
        format!("{upstream_url}&alt=sse")
    } else {
        format!("{upstream_url}?alt=sse")
    }
}
