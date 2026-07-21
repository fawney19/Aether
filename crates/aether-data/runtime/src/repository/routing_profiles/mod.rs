mod memory;

pub(crate) use aether_data_contracts::repository::routing_profiles::{
    CreateRoutingGroupBindingRecord, CreateRoutingGroupRecord, CreateRoutingGroupVersionRecord,
    RoutingGroupBindingQuery, RoutingGroupLookupKey, RoutingGroupReadRepository,
    RoutingGroupWriteRepository, StoredRoutingGroup, StoredRoutingGroupBinding,
    StoredRoutingGroupVersion, UpdateRoutingGroupBindingRecord, UpdateRoutingGroupRecord,
};
pub use memory::InMemoryRoutingGroupRepository;

#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlRoutingGroupRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::PostgresRoutingGroupRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteRoutingGroupRepository;
