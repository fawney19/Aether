mod antigravity;
mod codex;
mod generic;
mod kiro;

pub use antigravity::AntigravityProviderOAuthAdapter;
pub use codex::CodexProviderOAuthAdapter;
pub use generic::{
    GenericProviderOAuthAdapter, GenericProviderOAuthTemplate, GENERIC_PROVIDER_OAUTH_TEMPLATES,
};
pub use kiro::{
    generate_kiro_machine_id, KiroAuthConfig, KiroProviderOAuthAdapter, KIRO_PROVIDER_TYPE,
};
