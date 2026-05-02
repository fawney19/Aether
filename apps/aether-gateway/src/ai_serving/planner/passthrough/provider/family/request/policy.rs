use crate::ai_serving::planner::spec_metadata::LocalExecutionSurfaceSpecMetadata;
use crate::ai_serving::transport::{
    classify_same_format_provider_request_behavior as classify_same_format_provider_request_behavior_impl,
    resolve_same_format_provider_direct_auth as resolve_same_format_provider_direct_auth_impl,
    same_format_provider_transport_supported as same_format_provider_transport_supported_impl,
    same_format_provider_transport_unsupported_reason as same_format_provider_transport_unsupported_reason_impl,
    should_try_same_format_provider_oauth_auth as should_try_same_format_provider_oauth_auth_impl,
    GatewayProviderTransportSnapshot, SameFormatProviderFamily, SameFormatProviderRequestBehavior,
    SameFormatProviderRequestBehaviorParams,
};

use super::super::LocalSameFormatProviderFamily;

pub(super) fn classify_same_format_provider_request_behavior(
    transport: &GatewayProviderTransportSnapshot,
    spec_metadata: LocalExecutionSurfaceSpecMetadata,
) -> SameFormatProviderRequestBehavior {
    classify_same_format_provider_request_behavior_impl(
        transport,
        SameFormatProviderRequestBehaviorParams {
            require_streaming: spec_metadata.require_streaming,
            report_kind: spec_metadata
                .report_kind
                .expect("same-format provider specs should declare report kind"),
        },
    )
}

pub(super) fn same_format_provider_transport_supported(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: LocalSameFormatProviderFamily,
    api_format: &str,
) -> bool {
    same_format_provider_transport_supported_impl(
        behavior,
        transport,
        same_format_provider_family(family),
        api_format,
    )
}

pub(super) fn same_format_provider_transport_unsupported_reason(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: LocalSameFormatProviderFamily,
    api_format: &str,
) -> Option<&'static str> {
    same_format_provider_transport_unsupported_reason_impl(
        behavior,
        transport,
        same_format_provider_family(family),
        api_format,
    )
}

pub(super) fn should_try_same_format_provider_oauth_auth(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: LocalSameFormatProviderFamily,
) -> bool {
    should_try_same_format_provider_oauth_auth_impl(
        behavior,
        transport,
        same_format_provider_family(family),
    )
}

pub(super) fn resolve_same_format_provider_direct_auth(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: LocalSameFormatProviderFamily,
) -> Option<(String, String)> {
    resolve_same_format_provider_direct_auth_impl(
        behavior,
        transport,
        same_format_provider_family(family),
    )
}

fn same_format_provider_family(family: LocalSameFormatProviderFamily) -> SameFormatProviderFamily {
    match family {
        LocalSameFormatProviderFamily::Standard => SameFormatProviderFamily::Standard,
        LocalSameFormatProviderFamily::Gemini => SameFormatProviderFamily::Gemini,
    }
}
