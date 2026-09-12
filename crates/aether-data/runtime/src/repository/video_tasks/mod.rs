mod memory;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskLookupKey, VideoTaskModelCount,
    VideoTaskQueryFilter, VideoTaskReadRepository, VideoTaskRepository, VideoTaskStatus,
    VideoTaskStatusCount, VideoTaskWriteRepository,
};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::{SqlxVideoTaskReadRepository, SqlxVideoTaskRepository};
pub use memory::InMemoryVideoTaskRepository;
