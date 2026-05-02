mod auth;
mod converter;
mod credentials;
mod headers;
mod policy;
mod refresh;
mod request;
mod url;

pub use auth::{
    build_kiro_request_auth_from_config, is_kiro_claude_messages_transport,
    is_kiro_provider_transport, resolve_local_kiro_bearer_auth, resolve_local_kiro_request_auth,
    supports_local_kiro_auth_prerequisites, supports_local_kiro_request_auth_resolution,
    KiroBearerAuth, KiroRequestAuth, KIRO_AUTH_HEADER, PROVIDER_TYPE,
};
pub use converter::convert_claude_messages_to_conversation_state;
pub use credentials::{generate_machine_id, normalize_machine_id, KiroAuthConfig};
pub use headers::{
    build_generate_assistant_headers, build_mcp_headers, AWS_EVENTSTREAM_CONTENT_TYPE,
    KIRO_EXTERNAL_IDP_TOKEN_TYPE, KIRO_PROFILE_ARN_HEADER, KIRO_TOKEN_TYPE_HEADER,
};
pub use policy::{
    local_kiro_request_transport_unsupported_reason_with_network,
    supports_local_kiro_request_transport, supports_local_kiro_request_transport_with_network,
};
pub use refresh::KiroOAuthRefreshAdapter;
pub use request::{
    apply_local_body_rules, apply_local_header_rules, body_rules_are_locally_supported,
    build_kiro_provider_headers, build_kiro_provider_request_body,
    header_rules_are_locally_supported, supports_local_kiro_request_shape,
    KiroProviderHeadersInput,
};
pub use url::{
    build_kiro_generate_assistant_response_url, build_kiro_mcp_url,
    build_kiro_mcp_url_from_resolved_url, resolve_kiro_base_url, GENERATE_ASSISTANT_RESPONSE_PATH,
    KIRO_ENVELOPE_NAME, MCP_PATH, MCP_STREAM_PATH,
};
