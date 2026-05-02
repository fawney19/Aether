pub mod antigravity;
pub mod auth;
mod auth_config;
mod cache;
pub mod claude_code;
pub mod conversion;
mod diagnostics;
mod gemini_files;
mod generic_oauth;
mod headers;
pub mod kiro;
mod network;
pub mod oauth_refresh;
mod openai_image;
pub mod policy;
pub mod provider_types;
mod request_url;
pub mod rules;
pub mod same_format_provider;
pub mod snapshot;
mod standard;
pub mod url;
pub mod vertex;
mod video;

pub use aether_oauth as oauth;
pub use auth::{build_passthrough_headers, ensure_upstream_auth_header};
pub use cache::{provider_transport_snapshot_looks_refreshed, ProviderTransportSnapshotCacheKey};
pub use conversion::{
    candidate_common_transport_skip_reason, candidate_transport_pair_skip_reason,
    request_conversion_direct_auth, request_conversion_enabled_for_transport,
    request_conversion_transport_supported, request_conversion_transport_unsupported_reason,
    request_pair_allowed_for_transport, CandidateTransportPolicyFacts,
};
pub use diagnostics::{
    append_transport_diagnostics_to_value, build_request_trace_proxy_value,
    build_transport_diagnostics,
};
pub use gemini_files::{
    build_gemini_files_headers, build_gemini_files_request_body, build_gemini_files_upstream_url,
    gemini_files_transport_unsupported_reason, resolve_gemini_files_auth, GeminiFilesHeadersInput,
    GeminiFilesRequestBodyError, GeminiFilesRequestBodyParts,
};
pub use generic_oauth::{
    supports_local_generic_oauth_request_auth_resolution, GenericOAuthRefreshAdapter,
};
pub use headers::{should_skip_request_header, should_skip_upstream_passthrough_header};
pub use network::{
    resolve_transport_execution_timeouts, resolve_transport_proxy_snapshot,
    resolve_transport_proxy_snapshot_with_tunnel_affinity, resolve_transport_tls_profile,
    transport_proxy_is_locally_supported, TransportTunnelAffinityLookup,
    TransportTunnelAttachmentOwner,
};
pub use oauth_refresh::{
    supports_local_oauth_request_auth_resolution, CachedOAuthEntry, LocalOAuthHttpExecutor,
    LocalOAuthHttpRequest, LocalOAuthHttpResponse, LocalOAuthRefreshCoordinator,
    LocalOAuthRefreshError, LocalResolvedOAuthRequestAuth, ReqwestLocalOAuthHttpExecutor,
};
pub use openai_image::{
    build_openai_image_headers, build_openai_image_upstream_url,
    openai_image_transport_unsupported_reason, resolve_openai_image_auth,
    ProviderOpenAiImageHeadersInput,
};
pub use policy::{
    local_gemini_transport_unsupported_reason,
    local_gemini_transport_unsupported_reason_with_network,
    local_openai_chat_transport_unsupported_reason, local_standard_transport_unsupported_reason,
    local_standard_transport_unsupported_reason_with_network, supports_local_gemini_transport,
    supports_local_gemini_transport_with_network, supports_local_standard_transport,
};
pub use request_url::{
    build_cross_format_openai_chat_upstream_url, build_cross_format_openai_responses_upstream_url,
    build_kiro_cross_format_upstream_url, build_local_openai_chat_upstream_url,
    build_local_openai_responses_upstream_url, build_transport_request_url,
    TransportRequestUrlParams,
};
pub use rules::{
    apply_local_body_rules, apply_local_header_rules, body_rules_are_locally_supported,
    body_rules_handle_path, header_rules_are_locally_supported,
};
pub use same_format_provider::{
    build_same_format_provider_headers, build_same_format_provider_request_body,
    build_same_format_provider_upstream_url, classify_same_format_provider_request_behavior,
    resolve_same_format_provider_direct_auth, same_format_provider_transport_supported,
    same_format_provider_transport_unsupported_reason,
    same_format_provider_transport_unsupported_reason_for_trace,
    should_try_same_format_provider_oauth_auth, SameFormatProviderFamily,
    SameFormatProviderHeadersInput, SameFormatProviderRequestBehavior,
    SameFormatProviderRequestBehaviorParams, SameFormatProviderRequestBodyInput,
    SameFormatProviderUpstreamUrlParams,
};
pub use snapshot::{
    read_provider_transport_snapshot, GatewayProviderTransportSnapshot,
    ProviderTransportSnapshotSource,
};
pub use standard::{
    apply_standard_provider_request_body_rules, build_standard_plan_fallback_headers,
    build_standard_plan_fallback_openai_chat_url,
    build_standard_plan_fallback_openai_responses_url, build_standard_provider_request_headers,
    StandardPlanFallbackAcceptPolicy, StandardPlanFallbackHeadersInput,
    StandardProviderRequestHeaders, StandardProviderRequestHeadersInput,
};
pub use vertex::{is_vertex_api_key_transport_context, uses_vertex_api_key_query_auth};
pub use video::{
    build_video_create_headers, build_video_create_request_body, build_video_create_upstream_url,
    reconstruct_local_video_task_snapshot, resolve_local_video_task_transport,
    resolve_video_create_auth, video_create_transport_unsupported_reason,
    ProviderVideoCreateFamily, ProviderVideoCreateHeadersInput, VideoTaskTransportSnapshotLookup,
};
