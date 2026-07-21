mod memory;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::global_models::{
    metadata_supports_embedding, AdminGlobalModelListQuery, AdminProviderModelListQuery,
    CreateAdminGlobalModelRecord, GlobalModelReadRepository, GlobalModelSnapshot,
    GlobalModelWriteRepository, PublicCatalogModelListQuery, PublicCatalogModelSearchQuery,
    PublicGlobalModelQuery, StoredAdminGlobalModel, StoredAdminGlobalModelPage,
    StoredAdminProviderModel, StoredProviderActiveGlobalModel, StoredProviderModelStats,
    StoredPublicCatalogModel, StoredPublicGlobalModel, StoredPublicGlobalModelPage,
    UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlGlobalModelReadRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxGlobalModelReadRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteGlobalModelReadRepository;
pub use memory::InMemoryGlobalModelReadRepository;
