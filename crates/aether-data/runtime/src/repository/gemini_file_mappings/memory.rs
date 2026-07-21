use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::DataLayerError;
use aether_data_contracts::repository::gemini_file_mappings::{
    GeminiFileMappingListQuery, GeminiFileMappingMimeTypeCount, GeminiFileMappingReadRepository,
    GeminiFileMappingStats, GeminiFileMappingWriteRepository, StoredGeminiFileMapping,
    StoredGeminiFileMappingListPage, UpsertGeminiFileMappingRecord,
};

#[derive(Default)]
pub struct InMemoryGeminiFileMappingRepository {
    by_file: RwLock<BTreeMap<String, StoredGeminiFileMapping>>,
}

impl InMemoryGeminiFileMappingRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredGeminiFileMapping>,
    {
        let mut by_file = BTreeMap::new();
        for item in items {
            by_file.insert(item.file_name.clone(), item);
        }
        Self {
            by_file: RwLock::new(by_file),
        }
    }
}

#[async_trait]
impl GeminiFileMappingReadRepository for InMemoryGeminiFileMappingRepository {
    async fn find_by_file_name(
        &self,
        file_name: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, DataLayerError> {
        let guard = self.by_file.read().expect("gemini mapping repository lock");
        Ok(guard.get(file_name).cloned())
    }

    async fn list_mappings(
        &self,
        query: &GeminiFileMappingListQuery,
    ) -> Result<StoredGeminiFileMappingListPage, DataLayerError> {
        let guard = self.by_file.read().expect("gemini mapping repository lock");
        let search = query
            .search
            .as_deref()
            .map(|value| value.to_ascii_lowercase());
        let mut items = guard
            .values()
            .filter(|item| query.include_expired || item.expires_at_unix_secs > query.now_unix_secs)
            .filter(|item| {
                search.as_deref().is_none_or(|needle| {
                    item.file_name.to_ascii_lowercase().contains(needle)
                        || item
                            .display_name
                            .as_deref()
                            .map(|value| value.to_ascii_lowercase().contains(needle))
                            .unwrap_or(false)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .created_at_unix_ms
                .cmp(&left.created_at_unix_ms)
                .then_with(|| left.file_name.cmp(&right.file_name))
        });
        let total = items.len();
        let page_items = items
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect::<Vec<_>>();
        Ok(StoredGeminiFileMappingListPage {
            items: page_items,
            total,
        })
    }

    async fn summarize_mappings(
        &self,
        now_unix_secs: u64,
    ) -> Result<GeminiFileMappingStats, DataLayerError> {
        let guard = self.by_file.read().expect("gemini mapping repository lock");
        let total_mappings = guard.len();
        let active_items = guard
            .values()
            .filter(|item| item.expires_at_unix_secs > now_unix_secs)
            .collect::<Vec<_>>();
        let active_mappings = active_items.len();
        let expired_mappings = total_mappings.saturating_sub(active_mappings);
        let mut by_mime_type = BTreeMap::<String, usize>::new();
        for item in active_items {
            let mime_type = item
                .mime_type
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            *by_mime_type.entry(mime_type).or_default() += 1;
        }
        Ok(GeminiFileMappingStats {
            total_mappings,
            active_mappings,
            expired_mappings,
            by_mime_type: by_mime_type
                .into_iter()
                .map(|(mime_type, count)| GeminiFileMappingMimeTypeCount { mime_type, count })
                .collect(),
        })
    }
}

#[async_trait]
impl GeminiFileMappingWriteRepository for InMemoryGeminiFileMappingRepository {
    async fn upsert(
        &self,
        record: UpsertGeminiFileMappingRecord,
    ) -> Result<StoredGeminiFileMapping, DataLayerError> {
        record.validate()?;
        let mut guard = self
            .by_file
            .write()
            .expect("gemini mapping repository lock");
        let created_at_unix_ms = guard
            .get(&record.file_name)
            .map(|existing| existing.created_at_unix_ms)
            .unwrap_or_else(current_unix_secs);
        let mapping = StoredGeminiFileMapping {
            id: record.id.clone(),
            file_name: record.file_name.clone(),
            key_id: record.key_id.clone(),
            user_id: record.user_id.clone(),
            display_name: record.display_name.clone(),
            mime_type: record.mime_type.clone(),
            source_hash: record.source_hash.clone(),
            created_at_unix_ms,
            expires_at_unix_secs: record.expires_at_unix_secs,
        };
        guard.insert(record.file_name.clone(), mapping.clone());
        Ok(mapping)
    }

    async fn delete_by_file_name(&self, file_name: &str) -> Result<bool, DataLayerError> {
        let mut guard = self
            .by_file
            .write()
            .expect("gemini mapping repository lock");
        Ok(guard.remove(file_name).is_some())
    }

    async fn delete_by_id(
        &self,
        mapping_id: &str,
    ) -> Result<Option<StoredGeminiFileMapping>, DataLayerError> {
        let mut guard = self
            .by_file
            .write()
            .expect("gemini mapping repository lock");
        let Some(file_name) = guard
            .iter()
            .find_map(|(file_name, item)| (item.id == mapping_id).then(|| file_name.clone()))
        else {
            return Ok(None);
        };
        Ok(guard.remove(&file_name))
    }

    async fn delete_expired_before(&self, now_unix_secs: u64) -> Result<usize, DataLayerError> {
        let mut guard = self
            .by_file
            .write()
            .expect("gemini mapping repository lock");
        let before = guard.len();
        guard.retain(|_, item| item.expires_at_unix_secs > now_unix_secs);
        Ok(before.saturating_sub(guard.len()))
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use crate::repository::gemini_file_mappings::{
        GeminiFileMappingListQuery, GeminiFileMappingReadRepository,
        GeminiFileMappingWriteRepository,
    };

    use super::{InMemoryGeminiFileMappingRepository, UpsertGeminiFileMappingRecord};
    use crate::DataLayerError;

    fn sample_record(id: &str, file_name: &str) -> UpsertGeminiFileMappingRecord {
        UpsertGeminiFileMappingRecord {
            id: id.to_string(),
            file_name: file_name.to_string(),
            key_id: "key-1".to_string(),
            user_id: Some("user-1".to_string()),
            display_name: Some("display".to_string()),
            mime_type: Some("image/png".to_string()),
            source_hash: Some("hash-1".to_string()),
            expires_at_unix_secs: 4_102_444_800,
        }
    }

    #[tokio::test]
    async fn upsert_and_find() -> Result<(), DataLayerError> {
        let repo = InMemoryGeminiFileMappingRepository::default();
        let record = sample_record("id-1", "files/abc");
        let stored = repo.upsert(record.clone()).await?;
        assert_eq!(stored.file_name, "files/abc");

        let fetched = repo.find_by_file_name("files/abc").await?;
        assert_eq!(fetched.unwrap().key_id, "key-1");
        Ok(())
    }

    #[tokio::test]
    async fn delete_removes_entry() -> Result<(), DataLayerError> {
        let repo = InMemoryGeminiFileMappingRepository::default();
        let record = sample_record("id-2", "files/def");
        let _stored = repo.upsert(record).await?;
        assert!(repo.find_by_file_name("files/def").await?.is_some());
        assert!(repo.delete_by_file_name("files/def").await?);
        assert!(repo.find_by_file_name("files/def").await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn upsert_preserves_created_at_when_replacing_existing_file_name(
    ) -> Result<(), DataLayerError> {
        let repo = InMemoryGeminiFileMappingRepository::default();
        let first = repo.upsert(sample_record("id-1", "files/same")).await?;
        let replaced = repo.upsert(sample_record("id-2", "files/same")).await?;

        assert_eq!(replaced.created_at_unix_ms, first.created_at_unix_ms);
        assert_eq!(replaced.id, "id-2");
        Ok(())
    }

    #[tokio::test]
    async fn list_and_summarize_mappings() -> Result<(), DataLayerError> {
        let repo = InMemoryGeminiFileMappingRepository::seed(vec![
            repo_item("id-1", "files/alpha", "image/png", 10, 200),
            repo_item("id-2", "files/beta", "video/mp4", 20, 50),
            repo_item("id-3", "files/gamma", "", 30, 220),
        ]);

        let page = repo
            .list_mappings(&GeminiFileMappingListQuery {
                include_expired: false,
                search: Some("ga".to_string()),
                offset: 0,
                limit: 10,
                now_unix_secs: 100,
            })
            .await?;
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].id, "id-3");

        let stats = repo.summarize_mappings(100).await?;
        assert_eq!(stats.total_mappings, 3);
        assert_eq!(stats.active_mappings, 2);
        assert_eq!(stats.expired_mappings, 1);
        assert_eq!(stats.by_mime_type.len(), 2);
        assert_eq!(stats.by_mime_type[0].mime_type, "image/png");
        assert_eq!(stats.by_mime_type[0].count, 1);
        assert_eq!(stats.by_mime_type[1].mime_type, "unknown");
        assert_eq!(stats.by_mime_type[1].count, 1);
        Ok(())
    }

    #[tokio::test]
    async fn delete_by_id_and_cleanup_expired() -> Result<(), DataLayerError> {
        let repo = InMemoryGeminiFileMappingRepository::seed(vec![
            repo_item("id-1", "files/alpha", "image/png", 10, 200),
            repo_item("id-2", "files/beta", "video/mp4", 20, 50),
        ]);

        let deleted = repo.delete_by_id("id-1").await?;
        assert_eq!(
            deleted.as_ref().map(|item| item.file_name.as_str()),
            Some("files/alpha")
        );
        assert!(repo.find_by_file_name("files/alpha").await?.is_none());

        let deleted_count = repo.delete_expired_before(100).await?;
        assert_eq!(deleted_count, 1);
        assert!(repo.find_by_file_name("files/beta").await?.is_none());
        Ok(())
    }

    fn repo_item(
        id: &str,
        file_name: &str,
        mime_type: &str,
        created_at_unix_ms: u64,
        expires_at_unix_secs: u64,
    ) -> crate::repository::gemini_file_mappings::StoredGeminiFileMapping {
        crate::repository::gemini_file_mappings::StoredGeminiFileMapping {
            id: id.to_string(),
            file_name: file_name.to_string(),
            key_id: "key-1".to_string(),
            user_id: Some("user-1".to_string()),
            display_name: Some(format!("display-{id}")),
            mime_type: (!mime_type.is_empty()).then(|| mime_type.to_string()),
            source_hash: Some(format!("hash-{id}")),
            created_at_unix_ms,
            expires_at_unix_secs,
        }
    }
}
