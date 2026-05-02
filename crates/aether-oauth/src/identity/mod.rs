mod adapter;
pub mod providers;
mod service;

pub use adapter::{
    ExternalIdentity, IdentityClaims, IdentityOAuthExchangeContext, IdentityOAuthProvider,
    IdentityOAuthProviderConfig, IdentityOAuthStartContext,
};
pub use service::{
    bind_oauth_identity, login_with_oauth, start_identity_oauth, BoundOAuthIdentity,
    IdentityOAuthService, OAuthLoginOutcome,
};
