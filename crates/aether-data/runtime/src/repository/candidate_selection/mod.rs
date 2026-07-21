mod memory;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::candidate_selection::{
    MinimalCandidateSelectionReadRepository, MinimalCandidateSelectionRepository,
    StoredMinimalCandidateSelectionRow, StoredPoolKeyCandidateOrder,
    StoredPoolKeyCandidateRowsByKeyIdsQuery, StoredPoolKeyCandidateRowsQuery,
    StoredProviderModelMapping, StoredRequestedModelCandidateRowsQuery,
};
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlMinimalCandidateSelectionReadRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxMinimalCandidateSelectionReadRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteMinimalCandidateSelectionReadRepository;
pub use memory::InMemoryMinimalCandidateSelectionReadRepository;
