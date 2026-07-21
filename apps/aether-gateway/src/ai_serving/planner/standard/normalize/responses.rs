use serde_json::Value;

use crate::ai_serving::transport::apply_standard_provider_request_body_rules_with_request_headers;
use crate::ai_serving::{
    apply_openai_responses_compact_special_body_edits,
    build_cross_format_openai_responses_request_body_with_model_directives as surface_build_cross_format_openai_responses_request_body,
    build_local_openai_responses_request_body_with_model_directives as surface_build_local_openai_responses_request_body,
    GatewayProviderTransportSnapshot,
};

use super::{
    enforce_provider_body_stream_policy, request_requires_body_stream_field,
    validate_final_openai_provider_request,
};

pub(crate) fn build_local_openai_responses_request_body(
    body_json: &Value,
    mapped_model: &str,
    require_streaming: bool,
    force_body_stream_field: bool,
    provider_type: &str,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    _user_api_key_id: Option<&str>,
    request_headers: &http::HeaderMap,
    enable_model_directives: bool,
) -> Option<Value> {
    build_local_openai_responses_request_body_with_codex_model_capabilities(
        body_json,
        mapped_model,
        require_streaming,
        force_body_stream_field,
        provider_type,
        provider_api_format,
        body_rules,
        request_headers,
        None,
        enable_model_directives,
    )
}

pub(crate) fn build_local_openai_responses_request_body_with_codex_model_capabilities(
    body_json: &Value,
    mapped_model: &str,
    require_streaming: bool,
    force_body_stream_field: bool,
    provider_type: &str,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    request_headers: &http::HeaderMap,
    model_capabilities: Option<&crate::ai_serving::CodexResponsesModelCapabilities>,
    enable_model_directives: bool,
) -> Option<Value> {
    let provider_request_body = surface_build_local_openai_responses_request_body(
        body_json,
        mapped_model,
        require_streaming,
        enable_model_directives,
    )?;
    let mut provider_request_body =
        apply_standard_provider_request_body_rules_with_request_headers(
            provider_request_body,
            body_rules,
            body_json,
            request_headers,
        )?;
    let source_model = body_json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(mapped_model);
    crate::ai_serving::apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
        &mut provider_request_body,
        provider_type,
        provider_api_format,
        mapped_model,
        source_model,
        model_capabilities,
        body_rules,
    );
    apply_openai_responses_compact_special_body_edits(
        &mut provider_request_body,
        provider_api_format,
    );
    enforce_provider_body_stream_policy(
        &mut provider_request_body,
        provider_api_format,
        require_streaming,
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

pub(crate) fn build_cross_format_openai_responses_request_body(
    body_json: &Value,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    force_body_stream_field: bool,
    provider_type: &str,
    body_rules: Option<&Value>,
    _user_api_key_id: Option<&str>,
    request_headers: &http::HeaderMap,
    enable_model_directives: bool,
) -> Option<Value> {
    build_cross_format_openai_responses_request_body_with_codex_model_capabilities(
        body_json,
        mapped_model,
        client_api_format,
        provider_api_format,
        upstream_is_stream,
        force_body_stream_field,
        provider_type,
        body_rules,
        request_headers,
        None,
        enable_model_directives,
    )
}

pub(crate) fn build_cross_format_openai_responses_request_body_with_codex_model_capabilities(
    body_json: &Value,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    force_body_stream_field: bool,
    provider_type: &str,
    body_rules: Option<&Value>,
    request_headers: &http::HeaderMap,
    model_capabilities: Option<&crate::ai_serving::CodexResponsesModelCapabilities>,
    enable_model_directives: bool,
) -> Option<Value> {
    let provider_request_body = surface_build_cross_format_openai_responses_request_body(
        body_json,
        mapped_model,
        client_api_format,
        provider_api_format,
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
    let source_model = body_json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(mapped_model);
    crate::ai_serving::apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
        &mut provider_request_body,
        provider_type,
        provider_api_format,
        mapped_model,
        source_model,
        model_capabilities,
        body_rules,
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

pub(crate) fn build_local_openai_responses_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    compact: bool,
) -> Option<String> {
    crate::ai_serving::transport::build_local_openai_responses_upstream_url(
        transport,
        compact,
        parts.uri.query(),
    )
}

pub(crate) fn build_cross_format_openai_responses_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<String> {
    crate::ai_serving::transport::build_cross_format_openai_responses_upstream_url(
        transport,
        mapped_model,
        client_api_format,
        provider_api_format,
        upstream_is_stream,
        parts.uri.query(),
    )
}
