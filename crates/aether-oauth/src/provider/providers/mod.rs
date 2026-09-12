mod antigravity;
mod claude_code;
mod codex;
mod generic;
mod kiro;
mod windsurf;

pub use antigravity::{AntigravityProviderOAuthAdapter, ANTIGRAVITY_USER_INFO_URL};
pub use claude_code::{
    ClaudeCodeProviderOAuthAdapter, CLAUDE_CODE_AUTHORIZE_URL, CLAUDE_CODE_CLIENT_ID,
    CLAUDE_CODE_COOKIE_SCOPE, CLAUDE_CODE_OAUTH_SCOPES, CLAUDE_CODE_PROVIDER_TYPE,
    CLAUDE_CODE_REDIRECT_URI, CLAUDE_CODE_TOKEN_URL, CLAUDE_CODE_WEB_BASE_URL,
};
pub use codex::CodexProviderOAuthAdapter;
pub use generic::{
    derive_codex_identity_fingerprint, GenericProviderOAuthAdapter, GenericProviderOAuthTemplate,
    ANTIGRAVITY_OAUTH_CLIENT_ID_ENV, ANTIGRAVITY_OAUTH_CLIENT_SECRET_ENV,
    GEMINI_CLI_OAUTH_CLIENT_ID_ENV, GEMINI_CLI_OAUTH_CLIENT_SECRET_ENV,
    GENERIC_PROVIDER_OAUTH_TEMPLATES,
};
pub use kiro::{
    generate_kiro_machine_id, is_valid_kiro_region, normalize_kiro_machine_id,
    normalize_kiro_region, KiroAuthConfig, KiroProviderOAuthAdapter, DEFAULT_KIRO_VERSION,
    DEFAULT_NODE_VERSION, DEFAULT_REGION, DEFAULT_SYSTEM_VERSION, KIRO_PROVIDER_TYPE,
};
pub use windsurf::{
    WindsurfProviderOAuthAdapter, WINDSURF_CLIENT_ID, WINDSURF_PROVIDER_TYPE,
    WINDSURF_SHOW_AUTH_TOKEN_REDIRECT, WINDSURF_SIGNIN_URL,
};
