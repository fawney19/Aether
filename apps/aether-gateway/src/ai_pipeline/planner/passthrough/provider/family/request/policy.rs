use crate::ai_pipeline::planner::spec_metadata::LocalExecutionSurfaceSpecMetadata;
use crate::ai_pipeline::transport::auth::{resolve_local_gemini_auth, resolve_local_standard_auth};
use crate::ai_pipeline::transport::claude_code::local_claude_code_transport_unsupported_reason_with_network;
use crate::ai_pipeline::transport::kiro::local_kiro_request_transport_unsupported_reason_with_network;
use crate::ai_pipeline::transport::policy::{
    local_gemini_transport_unsupported_reason_with_network,
    local_standard_transport_unsupported_reason_with_network,
};
use crate::ai_pipeline::transport::vertex::{
    is_vertex_api_key_transport_context,
    local_vertex_api_key_gemini_transport_unsupported_reason_with_network,
};
use crate::ai_pipeline::GatewayProviderTransportSnapshot;

use super::super::LocalSameFormatProviderFamily;

pub(super) struct SameFormatProviderRequestBehavior {
    pub(super) is_antigravity: bool,
    pub(super) is_claude_code: bool,
    pub(super) is_vertex: bool,
    pub(super) is_kiro: bool,
    pub(super) upstream_is_stream: bool,
    pub(super) report_kind: &'static str,
}

pub(super) fn classify_same_format_provider_request_behavior(
    transport: &GatewayProviderTransportSnapshot,
    spec_metadata: LocalExecutionSurfaceSpecMetadata,
) -> SameFormatProviderRequestBehavior {
    let is_antigravity = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("antigravity");
    let is_claude_code = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("claude_code");
    let is_vertex = is_vertex_api_key_transport_context(transport);
    let is_kiro = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("kiro");
    let default_report_kind = spec_metadata
        .report_kind
        .expect("same-format provider specs should declare report kind");
    let upstream_is_stream = is_kiro || is_antigravity || spec_metadata.require_streaming;
    let report_kind = if is_kiro && !spec_metadata.require_streaming {
        "claude_cli_sync_finalize"
    } else if is_antigravity && !spec_metadata.require_streaming {
        match spec_metadata.api_format {
            "gemini:chat" => "gemini_chat_sync_finalize",
            "gemini:cli" => "gemini_cli_sync_finalize",
            _ => default_report_kind,
        }
    } else {
        default_report_kind
    };

    SameFormatProviderRequestBehavior {
        is_antigravity,
        is_claude_code,
        is_vertex,
        is_kiro,
        upstream_is_stream,
        report_kind,
    }
}

pub(super) fn same_format_provider_transport_supported(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: LocalSameFormatProviderFamily,
    api_format: &str,
) -> bool {
    same_format_provider_transport_unsupported_reason(behavior, transport, family, api_format)
        .is_none()
}

pub(super) fn same_format_provider_transport_unsupported_reason(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: LocalSameFormatProviderFamily,
    api_format: &str,
) -> Option<&'static str> {
    if behavior.is_kiro {
        local_kiro_request_transport_unsupported_reason_with_network(transport)
    } else if behavior.is_antigravity {
        None
    } else if behavior.is_claude_code {
        local_claude_code_transport_unsupported_reason_with_network(transport, api_format)
    } else if behavior.is_vertex {
        local_vertex_api_key_gemini_transport_unsupported_reason_with_network(transport)
    } else {
        match family {
            LocalSameFormatProviderFamily::Standard => {
                local_standard_transport_unsupported_reason_with_network(transport, api_format)
            }
            LocalSameFormatProviderFamily::Gemini => {
                local_gemini_transport_unsupported_reason_with_network(transport, api_format)
            }
        }
    }
}

pub(super) fn should_try_same_format_provider_oauth_auth(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: LocalSameFormatProviderFamily,
) -> bool {
    behavior.is_kiro
        || matches!(family, LocalSameFormatProviderFamily::Standard)
            && resolve_local_standard_auth(transport).is_none()
        || matches!(family, LocalSameFormatProviderFamily::Gemini)
            && !behavior.is_vertex
            && resolve_local_gemini_auth(transport).is_none()
}

pub(super) fn resolve_same_format_provider_direct_auth(
    behavior: &SameFormatProviderRequestBehavior,
    transport: &GatewayProviderTransportSnapshot,
    family: LocalSameFormatProviderFamily,
) -> Option<(String, String)> {
    if behavior.is_vertex {
        None
    } else {
        match family {
            LocalSameFormatProviderFamily::Standard => resolve_local_standard_auth(transport),
            LocalSameFormatProviderFamily::Gemini => resolve_local_gemini_auth(transport),
        }
    }
}
