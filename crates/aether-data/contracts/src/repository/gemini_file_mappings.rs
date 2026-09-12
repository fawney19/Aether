use async_trait::async_trait;

pub const GEMINI_FILE_MAPPING_MAX_FILE_NAME_CHARS: usize = 512;
pub const GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS: usize = 512;
pub const GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GeminiFileMappingListQuery {
    pub user_id: Option<String>,
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
        validate_text_length(
            &self.file_name,
            "file_name",
            GEMINI_FILE_MAPPING_MAX_FILE_NAME_CHARS,
        )?;
        if let Some(display_name) = self.display_name.as_deref() {
            validate_text_length(
                display_name,
                "display_name",
                GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS,
            )?;
        }
        if let Some(mime_type) = self.mime_type.as_deref() {
            validate_text_length(
                mime_type,
                "mime_type",
                GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS,
            )?;
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

fn validate_text_length(
    value: &str,
    field: &str,
    max_chars: usize,
) -> Result<(), crate::DataLayerError> {
    if value.chars().nth(max_chars).is_some() {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "gemini_file_mappings.{field} exceeds maximum length {max_chars}"
        )));
    }
    Ok(())
}

#[async_trait]
pub trait GeminiFileMappingReadRepository: Send + Sync {
    async fn find_by_file_name(
        &self,
        file_name: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, crate::DataLayerError>;

    /// Return an unexpired mapping only when it belongs to `user_id`.
    ///
    /// Repository implementations must apply the file name, user and expiry
    /// predicates in the same read operation. Public callers must not emulate
    /// this with an unrestricted lookup followed by an ownership check.
    async fn find_active_by_file_name_for_user(
        &self,
        file_name: &str,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<StoredGeminiFileMapping>, crate::DataLayerError>;

    /// Return an unexpired mapping only when both user and provider key match.
    /// This is the routing lookup used before forwarding file object requests
    /// to an upstream provider credential.
    async fn find_active_by_file_name_for_owner(
        &self,
        file_name: &str,
        key_id: &str,
        user_id: &str,
        now_unix_secs: u64,
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

    /// Insert a new mapping or refresh it only when the persisted owner is the
    /// same provider key and user. The ownership check and write must be one
    /// atomic repository operation so callers cannot be bypassed with a
    /// check-then-write race.
    async fn upsert_if_owner_matches(
        &self,
        record: UpsertGeminiFileMappingRecord,
    ) -> Result<Option<StoredGeminiFileMapping>, crate::DataLayerError>;

    async fn delete_by_file_name(&self, file_name: &str) -> Result<bool, crate::DataLayerError>;

    async fn delete_by_file_name_for_user(
        &self,
        file_name: &str,
        user_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    /// Delete only when both persisted ownership dimensions still match.
    async fn delete_by_file_name_for_owner(
        &self,
        file_name: &str,
        key_id: &str,
        user_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

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

#[cfg(test)]
mod tests {
    use super::{
        UpsertGeminiFileMappingRecord, GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS,
        GEMINI_FILE_MAPPING_MAX_FILE_NAME_CHARS, GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS,
    };
    use crate::DataLayerError;

    fn record() -> UpsertGeminiFileMappingRecord {
        UpsertGeminiFileMappingRecord {
            id: "mapping-1".to_string(),
            file_name: "files/example".to_string(),
            key_id: "key-1".to_string(),
            user_id: Some("user-1".to_string()),
            display_name: Some("example".to_string()),
            mime_type: Some("application/octet-stream".to_string()),
            source_hash: None,
            expires_at_unix_secs: 1,
        }
    }

    #[test]
    fn mapping_metadata_accepts_schema_limits() {
        let mut record = record();
        record.file_name = "f".repeat(GEMINI_FILE_MAPPING_MAX_FILE_NAME_CHARS);
        record.display_name = Some("d".repeat(GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS));
        record.mime_type = Some("m".repeat(GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS));

        record.validate().expect("schema limits should validate");
    }

    #[test]
    fn mapping_metadata_rejects_values_beyond_schema_limits() {
        for (field, record) in [
            ("file_name", {
                let mut record = record();
                record.file_name = "f".repeat(GEMINI_FILE_MAPPING_MAX_FILE_NAME_CHARS + 1);
                record
            }),
            ("display_name", {
                let mut record = record();
                record.display_name =
                    Some("d".repeat(GEMINI_FILE_MAPPING_MAX_DISPLAY_NAME_CHARS + 1));
                record
            }),
            ("mime_type", {
                let mut record = record();
                record.mime_type = Some("m".repeat(GEMINI_FILE_MAPPING_MAX_MIME_TYPE_CHARS + 1));
                record
            }),
        ] {
            let error = record
                .validate()
                .expect_err("oversized mapping metadata should fail");
            assert!(
                matches!(error, DataLayerError::InvalidInput(message) if message.contains(field))
            );
        }
    }
}
