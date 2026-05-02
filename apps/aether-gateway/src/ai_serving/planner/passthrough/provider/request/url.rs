use crate::ai_serving::transport::{
    build_same_format_provider_upstream_url as build_same_format_provider_upstream_url_impl,
    SameFormatProviderUpstreamUrlParams,
};
use crate::ai_serving::GatewayProviderTransportSnapshot;

use super::super::LocalSameFormatProviderSpec;

pub(crate) fn build_same_format_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    spec: LocalSameFormatProviderSpec,
    upstream_is_stream: bool,
    kiro_auth: Option<&crate::ai_serving::transport::kiro::KiroRequestAuth>,
) -> Option<String> {
    build_same_format_provider_upstream_url_impl(
        transport,
        SameFormatProviderUpstreamUrlParams {
            provider_api_format: spec.api_format,
            mapped_model,
            upstream_is_stream,
            request_query: parts.uri.query(),
            kiro_api_region: kiro_auth.map(|auth| auth.auth_config.effective_api_region()),
        },
    )
}
