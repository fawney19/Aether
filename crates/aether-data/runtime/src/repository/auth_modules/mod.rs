mod memory;

pub use aether_data_contracts::repository::auth_modules::{
    AuthModuleReadRepository, AuthModuleWriteRepository, CompareAndSwapLdapConfigResult,
    LdapBindPasswordUpdate, StoredLdapModuleConfig, StoredOAuthProviderModuleConfig,
};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::{SqlxAuthModuleReadRepository, SqlxAuthModuleRepository};
pub use memory::InMemoryAuthModuleReadRepository;
