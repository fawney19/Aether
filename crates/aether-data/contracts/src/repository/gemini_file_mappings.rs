use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GeminiFileMappingListQuery {
    pub include_expired: bool,
    pub search: Option<String>,
    pub offset: usize,
    pub limit: usize,
    pub now_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGeminiFileMappingListPage {
    pub items: Vec<StoredGeminiFileMapping>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiFileMappingMimeTypeCount {
    pub mime_type: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiFileMappingStats {
    pub total_mappings: usize,
    pub active_mappings: usize,
    pub expired_mappings: usize,
    pub by_mime_type: Vec<GeminiFileMappingMimeTypeCount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGeminiFileMapping {
    pub id: String,
    pub file_name: String,
    pub key_id: String,
    pub user_id: Option<String>,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub source_hash: Option<String>,
    pub created_at_unix_ms: u64,
    pub expires_at_unix_secs: u64,
}

impl StoredGeminiFileMapping {
    pub fn new(
        id: String,
        file_name: String,
        key_id: String,
        created_at_unix_ms: i64,
        expires_at_unix_secs: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if file_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "gemini_file_mappings.file_name is empty".to_string(),
            ));
        }
        if key_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "gemini_file_mappings.key_id is empty".to_string(),
            ));
        }
        let created_at_unix_ms = u64::try_from(created_at_unix_ms).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid gemini_file_mappings.created_at: {created_at_unix_ms}"
            ))
        })?;
        let expires_at_unix_secs = u64::try_from(expires_at_unix_secs).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid gemini_file_mappings.expires_at: {expires_at_unix_secs}"
            ))
        })?;
        Ok(Self {
            id,
            file_name,
            key_id,
            user_id: None,
            display_name: None,
            mime_type: None,
            source_hash: None,
            created_at_unix_ms,
            expires_at_unix_secs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertGeminiFileMappingRecord {
    pub id: String,
    pub file_name: String,
    pub key_id: String,
    pub user_id: Option<String>,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
    pub source_hash: Option<String>,
    pub expires_at_unix_secs: u64,
}

impl UpsertGeminiFileMappingRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.file_name.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "gemini_file_mappings.file_name is empty".to_string(),
            ));
        }
        if self.key_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "gemini_file_mappings.key_id is empty".to_string(),
            ));
        }
        if self.expires_at_unix_secs == 0 {
            return Err(crate::DataLayerError::InvalidInput(
                "gemini_file_mappings.expires_at is empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
pub trait GeminiFileMappingReadRepository: Send + Sync {
    async fn find_by_file_name(
        &self,
        file_name: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, crate::DataLayerError>;

    async fn list_mappings(
        &self,
        query: &GeminiFileMappingListQuery,
    ) -> Result<StoredGeminiFileMappingListPage, crate::DataLayerError>;

    async fn summarize_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<GeminiFileMappingStats, crate::DataLayerError>;
}

#[async_trait]
pub trait GeminiFileMappingWriteRepository: Send + Sync {
    async fn upsert(
        &self,
        record: UpsertGeminiFileMappingRecord,
    ) -> Result<StoredGeminiFileMapping, crate::DataLayerError>;
    async fn delete_by_file_name(&self, file_name: &str) -> Result<bool, crate::DataLayerError>;

    async fn delete_by_id(
        &self,
        mapping_id: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, crate::DataLayerError>;

    async fn delete_expired_before(
        &self,
        now_unix_secs: u64,
    ) -> Result<usize, crate::DataLayerError>;
}

pub trait GeminiFileMappingRepository:
    GeminiFileMappingReadRepository + GeminiFileMappingWriteRepository
{
}

impl<T> GeminiFileMappingRepository for T where
    T: GeminiFileMappingReadRepository + GeminiFileMappingWriteRepository
{
}
