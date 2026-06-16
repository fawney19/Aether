use serde_json::Value;

use super::super::LocalSameFormatProviderSpec;
use crate::ai_serving::transport::{
    build_same_format_provider_request_body as build_same_format_provider_request_body_impl,
    build_same_format_provider_request_body_with_compatibility_report as build_same_format_provider_request_body_with_compatibility_report_impl,
    SameFormatProviderFamily, SameFormatProviderRequestBodyInput,
    SameFormatProviderRequestBodyOutput,
};

pub(crate) fn build_same_format_provider_request_body(
    body_json: &Value,
    provider_api_format: &str,
    mapped_model: &str,
    spec: LocalSameFormatProviderSpec,
    body_rules: Option<&Value>,
    request_headers: Option<&http::HeaderMap>,
    upstream_is_stream: bool,
    force_body_stream_field: bool,
    kiro_auth: Option<&crate::ai_serving::transport::kiro::KiroRequestAuth>,
    is_claude_code: bool,
    enable_model_directives: bool,
) -> Option<Value> {
    build_same_format_provider_request_body_impl(SameFormatProviderRequestBodyInput {
        body_json,
        mapped_model,
        client_api_format: spec.api_format,
        provider_api_format,
        source_model: body_json.get("model").and_then(Value::as_str),
        family: same_format_provider_family(spec.family),
        body_rules,
        request_headers,
        upstream_is_stream,
        force_body_stream_field,
        kiro_auth_config: kiro_auth.map(|auth| &auth.auth_config),
        is_claude_code,
        enable_model_directives,
    })
}

pub(crate) fn build_same_format_provider_request_body_with_compatibility_report(
    body_json: &Value,
    provider_api_format: &str,
    mapped_model: &str,
    spec: LocalSameFormatProviderSpec,
    body_rules: Option<&Value>,
    request_headers: Option<&http::HeaderMap>,
    upstream_is_stream: bool,
    force_body_stream_field: bool,
    kiro_auth: Option<&crate::ai_serving::transport::kiro::KiroRequestAuth>,
    is_claude_code: bool,
    enable_model_directives: bool,
) -> Option<SameFormatProviderRequestBodyOutput> {
    build_same_format_provider_request_body_with_compatibility_report_impl(
        SameFormatProviderRequestBodyInput {
            body_json,
            mapped_model,
            client_api_format: spec.api_format,
            provider_api_format,
            source_model: body_json.get("model").and_then(Value::as_str),
            family: same_format_provider_family(spec.family),
            body_rules,
            request_headers,
            upstream_is_stream,
            force_body_stream_field,
            kiro_auth_config: kiro_auth.map(|auth| &auth.auth_config),
            is_claude_code,
            enable_model_directives,
        },
    )
}

fn same_format_provider_family(
    family: super::super::LocalSameFormatProviderFamily,
) -> SameFormatProviderFamily {
    match family {
        super::super::LocalSameFormatProviderFamily::Standard => SameFormatProviderFamily::Standard,
        super::super::LocalSameFormatProviderFamily::Gemini => SameFormatProviderFamily::Gemini,
    }
}
