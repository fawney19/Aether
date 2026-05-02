use serde_json::Value;

use super::super::LocalSameFormatProviderSpec;
use crate::ai_serving::transport::{
    build_same_format_provider_request_body as build_same_format_provider_request_body_impl,
    SameFormatProviderFamily, SameFormatProviderRequestBodyInput,
};

pub(crate) fn build_same_format_provider_request_body(
    body_json: &Value,
    mapped_model: &str,
    spec: LocalSameFormatProviderSpec,
    body_rules: Option<&Value>,
    upstream_is_stream: bool,
    kiro_auth: Option<&crate::ai_serving::transport::kiro::KiroRequestAuth>,
    is_claude_code: bool,
) -> Option<Value> {
    build_same_format_provider_request_body_impl(SameFormatProviderRequestBodyInput {
        body_json,
        mapped_model,
        family: same_format_provider_family(spec.family),
        body_rules,
        upstream_is_stream,
        kiro_auth_config: kiro_auth.map(|auth| &auth.auth_config),
        is_claude_code,
    })
}

fn same_format_provider_family(
    family: super::super::LocalSameFormatProviderFamily,
) -> SameFormatProviderFamily {
    match family {
        super::super::LocalSameFormatProviderFamily::Standard => SameFormatProviderFamily::Standard,
        super::super::LocalSameFormatProviderFamily::Gemini => SameFormatProviderFamily::Gemini,
    }
}
