mod auth;
mod policy;
mod request;
mod url;

pub use auth::{
    is_gemini_cli_provider_transport, resolve_gemini_cli_project_id,
    resolve_local_gemini_cli_request_auth, GeminiCliRequestAuth, GeminiCliRequestAuthSupport,
    GeminiCliRequestAuthUnsupportedReason, GEMINI_CLI_PROVIDER_TYPE,
};
pub use policy::gemini_cli_v1internal_requires_upstream_streaming;
pub use request::{
    build_gemini_cli_v1internal_request, classify_gemini_cli_v1internal_request_body,
    GeminiCliRequestEnvelopeSupport, GeminiCliRequestEnvelopeUnsupportedReason,
};
pub use url::{
    build_gemini_cli_v1internal_url, GeminiCliRequestUrlAction,
    GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH, GEMINI_CLI_USER_AGENT,
    GEMINI_CLI_V1INTERNAL_PATH_TEMPLATE,
};

pub const GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME: &str = "gemini_cli:v1internal";
