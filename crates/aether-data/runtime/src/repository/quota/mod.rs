mod memory;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::quota::{
    ApplyRemoteProviderQuotaOutcome, ApplyRemoteProviderQuotaPatch, ProviderQuotaReadRepository,
    ProviderQuotaRepository, ProviderQuotaUsageObservation, ProviderQuotaWriteRepository,
    StoredProviderQuotaSnapshot,
};
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlProviderQuotaRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxProviderQuotaRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteProviderQuotaRepository;
pub use memory::InMemoryProviderQuotaRepository;
