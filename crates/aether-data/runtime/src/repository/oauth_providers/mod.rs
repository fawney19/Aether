mod memory;

pub use aether_data_contracts::repository::oauth_providers::{
    validate_oauth_frontend_callback_url, validate_oauth_provider_endpoint_config,
    validate_oauth_redirect_uri, EncryptedSecretUpdate, OAuthProviderReadRepository,
    OAuthProviderRepository, OAuthProviderWriteRepository, StoredOAuthProviderConfig,
    UpsertOAuthProviderConfigOutcome, UpsertOAuthProviderConfigRecord,
};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxOAuthProviderRepository;
pub use memory::InMemoryOAuthProviderRepository;
