pub mod antigravity;
pub mod auth;
mod auth_config;
mod cache;
pub mod claude_code;
pub mod conversion;
mod diagnostics;
pub mod gemini_cli;
mod gemini_files;
mod generic_oauth;
pub mod grok;
mod headers;
pub mod kiro;
mod network;
pub mod oauth_refresh;
mod openai_image;
pub mod policy;
pub mod provider_types;
mod request_body;
mod request_url;
pub mod rules;
pub mod same_format_provider;
pub mod snapshot;
mod standard;
pub mod url;
pub mod vertex;
mod video;
pub mod windsurf;

pub use aether_oauth as oauth;
pub use auth::{build_passthrough_headers, ensure_upstream_auth_header};
pub use cache::{provider_transport_snapshot_looks_refreshed, ProviderTransportSnapshotCacheKey};
pub use conversion::{
    candidate_common_transport_skip_reason, candidate_transport_pair_skip_reason,
    request_conversion_direct_auth, request_conversion_enabled_for_transport,
    request_conversion_transport_supported, request_conversion_transport_unsupported_reason,
    request_pair_allowed_for_transport, request_pair_direct_auth,
    request_pair_transport_unsupported_reason, CandidateTransportPolicyFacts,
};
pub use diagnostics::{
    append_transport_diagnostics_to_value, build_request_trace_proxy_value,
    build_transport_diagnostics,
};
pub use gemini_cli::{
    build_gemini_cli_v1internal_request, build_gemini_cli_v1internal_url,
    classify_gemini_cli_v1internal_request_body, gemini_cli_v1internal_requires_upstream_streaming,
    is_gemini_cli_provider_transport, resolve_gemini_cli_project_id,
    resolve_local_gemini_cli_request_auth, GeminiCliRequestAuth, GeminiCliRequestAuthSupport,
    GeminiCliRequestAuthUnsupportedReason, GeminiCliRequestEnvelopeSupport,
    GeminiCliRequestEnvelopeUnsupportedReason, GeminiCliRequestUrlAction, GEMINI_CLI_PROVIDER_TYPE,
    GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH, GEMINI_CLI_USER_AGENT,
    GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME, GEMINI_CLI_V1INTERNAL_PATH_TEMPLATE,
};
pub use gemini_files::{
    build_gemini_files_headers, build_gemini_files_request_body, build_gemini_files_upstream_url,
    gemini_files_transport_unsupported_reason, resolve_gemini_files_auth, GeminiFilesHeadersInput,
    GeminiFilesRequestBodyError, GeminiFilesRequestBodyParts,
};
pub use generic_oauth::{
    supports_local_generic_oauth_request_auth_resolution, GenericOAuthRefreshAdapter,
};
pub use grok::{
    build_grok_app_chat_body, build_grok_browser_headers, build_grok_upstream_url, grok_base_url,
    grok_browser_profile_id_from_user_agent,
    grok_browser_profile_metadata_from_resolved_transport_profile,
    grok_browser_resolved_transport_profile,
    grok_browser_resolved_transport_profile_from_auth_config,
    grok_browser_transport_fingerprint_from_auth_config, is_grok_provider_transport,
    resolve_grok_session_auth, GrokBrowserProfileMetadata, GrokHeaderInput, GROK_CHAT_PATH,
    GROK_DEFAULT_BASE_URL, GROK_DEFAULT_BROWSER_PROFILE, GROK_DEFAULT_USER_AGENT,
    GROK_INTERNAL_HEADER, GROK_RATE_LIMITS_PATH,
};
pub use headers::{should_skip_request_header, should_skip_upstream_passthrough_header};
pub use network::{
    resolve_transport_execution_timeouts, resolve_transport_profile, resolve_transport_profile_id,
    resolve_transport_proxy_snapshot, resolve_transport_proxy_snapshot_with_tunnel_affinity,
    transport_profile_is_configured, transport_proxy_is_locally_supported,
    TransportTunnelAffinityLookup, TransportTunnelAttachmentOwner,
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
pub use request_body::{
    apply_transport_request_body_semantics, TransportRequestBodySemanticsError,
};
pub use request_url::{
    build_cross_format_openai_chat_upstream_url, build_cross_format_openai_responses_upstream_url,
    build_kiro_cross_format_upstream_url, build_local_openai_chat_upstream_url,
    build_local_openai_responses_upstream_url, build_transport_request_url,
    build_transport_request_url_for_request_body, gemini_embedding_request_body_uses_batch,
    TransportRequestUrlParams,
};
pub use rules::{
    apply_local_body_rules, apply_local_body_rules_with_request_headers, apply_local_header_rules,
    apply_local_header_rules_with_request_headers, body_rules_are_locally_supported,
    body_rules_handle_path, body_rules_have_enabled_rules, header_rules_are_locally_supported,
    header_rules_have_enabled_rules,
};
pub use same_format_provider::{
    build_same_format_provider_headers, build_same_format_provider_request_body,
    build_same_format_provider_request_body_with_compatibility_report,
    build_same_format_provider_upstream_url, classify_same_format_provider_request_behavior,
    resolve_same_format_provider_direct_auth, same_format_provider_transport_supported,
    same_format_provider_transport_unsupported_reason,
    same_format_provider_transport_unsupported_reason_for_trace,
    should_try_same_format_provider_oauth_auth, SameFormatProviderCompatibilityEdit,
    SameFormatProviderCompatibilityEditAction, SameFormatProviderFamily,
    SameFormatProviderHeadersInput, SameFormatProviderRequestBehavior,
    SameFormatProviderRequestBehaviorParams, SameFormatProviderRequestBodyInput,
    SameFormatProviderRequestBodyOutput, SameFormatProviderUpstreamUrlParams,
};
pub use snapshot::{
    read_provider_transport_snapshot, GatewayProviderTransportSnapshot,
    ProviderTransportSnapshotSource,
};
pub use standard::{
    apply_standard_provider_request_body_rules,
    apply_standard_provider_request_body_rules_with_request_headers,
    build_standard_plan_fallback_headers, build_standard_plan_fallback_openai_chat_url,
    build_standard_plan_fallback_openai_responses_url, build_standard_provider_request_headers,
    StandardPlanFallbackAcceptPolicy, StandardPlanFallbackHeadersInput,
    StandardProviderRequestHeaders, StandardProviderRequestHeadersInput,
};
pub use vertex::{
    is_vertex_api_key_transport_context, is_vertex_service_account_transport_context,
    is_vertex_transport_context, uses_vertex_api_key_query_auth,
};
pub use video::{
    build_video_create_headers, build_video_create_request_body, build_video_create_upstream_url,
    reconstruct_local_video_task_snapshot, resolve_local_video_task_transport,
    resolve_video_create_auth, video_create_transport_unsupported_reason,
    ProviderVideoCreateFamily, ProviderVideoCreateHeadersInput, VideoTaskTransportSnapshotLookup,
};
pub use windsurf::{
    build_windsurf_cascade_headers, build_windsurf_cascade_request_body,
    build_windsurf_cascade_upstream_url, is_windsurf_provider_transport,
    local_windsurf_request_transport_unsupported_reason_with_network, GET_CHAT_MESSAGE_PATH,
    WINDSURF_ENVELOPE_NAME,
};
