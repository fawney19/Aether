mod memory;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::background_tasks::{
    BackgroundTaskKind, BackgroundTaskListQuery, BackgroundTaskReadRepository,
    BackgroundTaskRepository, BackgroundTaskStatus, BackgroundTaskSummary,
    BackgroundTaskWriteRepository, StoredBackgroundTaskEvent, StoredBackgroundTaskRun,
    StoredBackgroundTaskRunPage, UpsertBackgroundTaskEvent, UpsertBackgroundTaskRun,
};

#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlBackgroundTaskRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxBackgroundTaskRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteBackgroundTaskRepository;
pub use memory::InMemoryBackgroundTaskRepository;
