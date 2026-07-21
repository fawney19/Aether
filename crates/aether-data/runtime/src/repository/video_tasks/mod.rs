mod memory;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskReadRepository, VideoTaskRepository, VideoTaskStatus,
    VideoTaskStatusCount, VideoTaskWriteRepository,
};
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlVideoTaskRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::{SqlxVideoTaskReadRepository, SqlxVideoTaskRepository};
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteVideoTaskRepository;
pub use memory::InMemoryVideoTaskRepository;
