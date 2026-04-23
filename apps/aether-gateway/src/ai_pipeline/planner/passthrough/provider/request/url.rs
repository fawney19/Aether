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
    crate::provider_transport::build_transport_request_url(
        transport,
        crate::provider_transport::TransportRequestUrlParams {
            provider_api_format: spec.api_format,
            mapped_model: Some(mapped_model),
            upstream_is_stream,
            request_query: parts.uri.query(),
            kiro_api_region: kiro_auth.map(|auth| auth.auth_config.effective_api_region()),
        },
    )
}
