pub mod memory;
#[cfg(feature = "postgres")]
pub mod postgres {
    pub use aether_data_postgres::SqlxGeminiFileMappingRepository;
}
pub mod types {
    pub use aether_data_contracts::repository::gemini_file_mappings::*;
}

pub use aether_data_contracts::repository::gemini_file_mappings::{
    GeminiFileMappingListQuery, GeminiFileMappingMimeTypeCount, GeminiFileMappingReadRepository,
    GeminiFileMappingRepository, GeminiFileMappingStats, GeminiFileMappingWriteRepository,
    StoredGeminiFileMapping, StoredGeminiFileMappingListPage, UpsertGeminiFileMappingRecord,
};
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxGeminiFileMappingRepository;
pub use memory::InMemoryGeminiFileMappingRepository;
