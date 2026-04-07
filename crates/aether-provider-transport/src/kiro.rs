mod auth;
mod converter;
mod credentials;
mod headers;
mod policy;
mod refresh;
mod request;
mod url;

pub use auth::{
    build_kiro_request_auth_from_config, resolve_local_kiro_bearer_auth,
    resolve_local_kiro_request_auth, supports_local_kiro_auth_prerequisites,
    supports_local_kiro_request_auth_resolution, KiroBearerAuth, KiroRequestAuth, KIRO_AUTH_HEADER,
    PROVIDER_TYPE,
};
pub use converter::convert_claude_messages_to_conversation_state;
pub use credentials::{generate_machine_id, normalize_machine_id, KiroAuthConfig};
pub use headers::{build_generate_assistant_headers, AWS_EVENTSTREAM_CONTENT_TYPE};
pub use policy::{
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
    build_kiro_generate_assistant_response_url, resolve_kiro_base_url,
    GENERATE_ASSISTANT_RESPONSE_PATH, KIRO_ENVELOPE_NAME,
};
