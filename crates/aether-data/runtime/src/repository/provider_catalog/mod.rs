mod memory;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::provider_catalog::{
    patch_provider_catalog_runtime_credentials, provider_catalog_runtime_credentials_cas_matches,
    ProviderCatalogKeyAdaptiveState, ProviderCatalogKeyAdaptiveStateUpdate,
    ProviderCatalogKeyAdminCasUpdate, ProviderCatalogKeyHealthStateUpdate,
    ProviderCatalogKeyListOrder, ProviderCatalogKeyListQuery,
    ProviderCatalogKeyOAuthCredentialCasDelete, ProviderCatalogKeyOAuthCredentialFence,
    ProviderCatalogKeyOAuthRuntimeStateCasUpdate, ProviderCatalogKeyRuntimeMetadataUpdate,
    ProviderCatalogKeyStatusSnapshotUpdate, ProviderCatalogProviderConfigCasUpdate,
    ProviderCatalogReadRepository, ProviderCatalogRuntimeCredentialsCas, ProviderCatalogSnapshot,
    ProviderCatalogUpstreamMetadataNamespaceExpectation,
    ProviderCatalogUpstreamMetadataNamespaceUpdate, ProviderCatalogWriteRepository,
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogKeyMaintenanceSummary, StoredProviderCatalogKeyPage,
    StoredProviderCatalogKeyStats, StoredProviderCatalogProvider,
};
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlProviderCatalogReadRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxProviderCatalogReadRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteProviderCatalogReadRepository;
pub use memory::InMemoryProviderCatalogReadRepository;
