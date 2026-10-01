mod auth;
mod fingerprint;
mod mimicry;
mod policy;
mod profile;
mod request;
mod url;

pub use auth::supports_local_claude_code_auth;
pub use fingerprint::{
    generate_fingerprint, generate_random_fingerprint, header_fingerprint_from_fingerprint,
    sanitize_fingerprint,
};
pub use mimicry::{
    apply_claude_code_body_mimicry, apply_claude_code_body_mimicry_for_transport,
    ClaudeCodeBodyMimicryContext,
};
pub use policy::{
    local_claude_code_transport_unsupported_reason_with_network,
    supports_local_claude_code_transport_with_network,
};
pub use profile::{
    claude_code_client_profile, claude_code_client_user_agent, claude_code_client_version,
    current_claude_code_transport_identity_profile, set_claude_code_cli_version,
    set_claude_code_client_profile, ClaudeCodeBodyCapabilityGate, ClaudeCodeClientProfile,
    ClaudeCodeTransportIdentityProfile, ClaudeCodeTransportIdentityProfileVersion,
    ClaudeCodeTransportIdentityTemplate, CLAUDE_CODE_BUILTIN_CLI_VERSION,
    CLAUDE_CODE_CONTEXT_MANAGEMENT_BETA, CLAUDE_CODE_TRANSPORT_IDENTITY_2026_04,
};
pub use request::{
    build_claude_code_passthrough_headers, sanitize_claude_code_request_body,
    sanitize_claude_code_request_body_for_beta_header,
};
pub use url::build_claude_code_messages_url;
