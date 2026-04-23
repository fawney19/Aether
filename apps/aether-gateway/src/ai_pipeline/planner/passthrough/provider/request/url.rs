use crate::ai_pipeline::GatewayProviderTransportSnapshot;

use super::super::LocalSameFormatProviderSpec;

pub(crate) fn build_same_format_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    spec: LocalSameFormatProviderSpec,
    upstream_is_stream: bool,
    kiro_auth: Option<&crate::ai_pipeline::transport::kiro::KiroRequestAuth>,
) -> Option<String> {
    maybe_add_gemini_stream_alt_sse(crate::ai_pipeline::build_provider_transport_request_url(
        transport,
        spec.api_format,
        Some(mapped_model),
        upstream_is_stream,
        parts.uri.query(),
        kiro_auth.map(|auth| auth.auth_config.effective_api_region()),
    ))
}

fn maybe_add_gemini_stream_alt_sse(url: Option<String>) -> Option<String> {
    url
}
