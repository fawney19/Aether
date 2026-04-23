pub mod antigravity;
pub mod auth;
mod auth_config;
mod cache;
pub mod claude_code;
mod generic_oauth;
mod headers;
pub mod kiro;
mod network;
pub mod oauth_refresh;
pub mod policy;
pub mod provider_types;
mod request_url;
pub mod rules;
pub mod snapshot;
pub mod url;
pub mod vertex;
mod video;

pub use auth::{build_passthrough_headers, ensure_upstream_auth_header};
pub use cache::{provider_transport_snapshot_looks_refreshed, ProviderTransportSnapshotCacheKey};
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
pub use policy::{
    local_gemini_transport_unsupported_reason,
    local_gemini_transport_unsupported_reason_with_network,
    local_openai_chat_transport_unsupported_reason, local_standard_transport_unsupported_reason,
    local_standard_transport_unsupported_reason_with_network, supports_local_gemini_transport,
    supports_local_gemini_transport_with_network, supports_local_standard_transport,
};
pub use request_url::{build_transport_request_url, TransportRequestUrlParams};
pub use rules::{
    apply_local_body_rules, apply_local_header_rules, body_rules_are_locally_supported,
    body_rules_handle_path, header_rules_are_locally_supported,
};
pub use snapshot::{
    read_provider_transport_snapshot, GatewayProviderTransportSnapshot,
    ProviderTransportSnapshotSource,
};
pub use vertex::{is_vertex_api_key_transport_context, uses_vertex_api_key_query_auth};
pub use video::{
    reconstruct_local_video_task_snapshot, resolve_local_video_task_transport,
    VideoTaskTransportSnapshotLookup,
};
