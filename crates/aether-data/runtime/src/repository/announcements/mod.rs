mod memory;

pub use aether_data_contracts::repository::announcements::{
    AnnouncementListQuery, AnnouncementReadRepository, AnnouncementWriteRepository,
    CreateAnnouncementRecord, StoredAnnouncement, StoredAnnouncementPage, UpdateAnnouncementRecord,
};
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlAnnouncementRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxAnnouncementReadRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteAnnouncementRepository;
pub use memory::InMemoryAnnouncementReadRepository;
