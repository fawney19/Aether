mod memory;

pub use aether_data_contracts::repository::auth_modules::{
    AuthModuleReadRepository, AuthModuleWriteRepository, StoredLdapModuleConfig,
    StoredOAuthProviderModuleConfig,
};
#[cfg(feature = "mysql")]
pub use aether_data_mysql::{MysqlAuthModuleReadRepository, MysqlAuthModuleRepository};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::{SqlxAuthModuleReadRepository, SqlxAuthModuleRepository};
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::{SqliteAuthModuleReadRepository, SqliteAuthModuleRepository};
pub use memory::InMemoryAuthModuleReadRepository;
