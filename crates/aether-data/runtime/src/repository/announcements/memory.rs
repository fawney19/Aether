use std::collections::BTreeSet;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use uuid::Uuid;

use crate::DataLayerError;
use aether_data_contracts::repository::announcements::{
    AnnouncementListQuery, AnnouncementReadRepository, AnnouncementWriteRepository,
    CreateAnnouncementRecord, StoredAnnouncement, StoredAnnouncementPage, UpdateAnnouncementRecord,
};

#[derive(Debug, Default)]
pub struct InMemoryAnnouncementReadRepository {
    announcements: RwLock<Vec<StoredAnnouncement>>,
    announcement_reads: RwLock<BTreeSet<(String, String)>>,
}

impl InMemoryAnnouncementReadRepository {
    pub fn seed<I>(announcements: I) -> Self
    where
        I: IntoIterator<Item = StoredAnnouncement>,
    {
        Self::seed_with_reads(announcements, std::iter::empty::<(String, String)>())
    }

    pub fn seed_with_reads<I, J>(announcements: I, reads: J) -> Self
    where
        I: IntoIterator<Item = StoredAnnouncement>,
        J: IntoIterator<Item = (String, String)>,
    {
        Self {
            announcements: RwLock::new(announcements.into_iter().collect()),
            announcement_reads: RwLock::new(reads.into_iter().collect()),
        }
    }

    fn now_unix_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

#[async_trait]
impl AnnouncementReadRepository for InMemoryAnnouncementReadRepository {
    async fn find_by_id(
        &self,
        announcement_id: &str,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        Ok(self
            .announcements
            .read()
            .expect("announcement repository lock")
            .iter()
            .find(|announcement| announcement.id == announcement_id)
            .cloned())
    }

    async fn list_announcements(
        &self,
        query: &AnnouncementListQuery,
    ) -> Result<StoredAnnouncementPage, DataLayerError> {
        let now_unix_secs = query.now_unix_secs.unwrap_or_else(Self::now_unix_secs);
        let announcements = self
            .announcements
            .read()
            .expect("announcement repository lock");

        let mut items: Vec<_> = announcements
            .iter()
            .filter(|announcement| {
                if !query.active_only {
                    return true;
                }
                announcement.is_active
                    && announcement
                        .start_time_unix_secs
                        .is_none_or(|value| value <= now_unix_secs)
                    && announcement
                        .end_time_unix_secs
                        .is_none_or(|value| value >= now_unix_secs)
            })
            .cloned()
            .collect();

        items.sort_by(|left, right| {
            right
                .is_pinned
                .cmp(&left.is_pinned)
                .then_with(|| right.priority.cmp(&left.priority))
                .then_with(|| right.created_at_unix_ms.cmp(&left.created_at_unix_ms))
                .then_with(|| left.id.cmp(&right.id))
        });

        let total = items.len() as u64;
        let items = items
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();

        Ok(StoredAnnouncementPage { items, total })
    }

    async fn count_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        let announcements = self
            .announcements
            .read()
            .expect("announcement repository lock");
        let reads = self
            .announcement_reads
            .read()
            .expect("announcement reads repository lock");

        let total = announcements
            .iter()
            .filter(|announcement| {
                announcement.is_active
                    && announcement
                        .start_time_unix_secs
                        .is_none_or(|value| value <= now_unix_secs)
                    && announcement
                        .end_time_unix_secs
                        .is_none_or(|value| value >= now_unix_secs)
                    && !reads.contains(&(user_id.to_string(), announcement.id.clone()))
            })
            .count() as u64;

        Ok(total)
    }

    async fn list_required_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredAnnouncement>, DataLayerError> {
        let announcements = self
            .announcements
            .read()
            .expect("announcement repository lock");
        let reads = self
            .announcement_reads
            .read()
            .expect("announcement reads repository lock");

        let mut items = announcements
            .iter()
            .filter(|announcement| {
                announcement.requires_ack
                    && announcement.is_active
                    && announcement
                        .start_time_unix_secs
                        .is_none_or(|value| value <= now_unix_secs)
                    && announcement
                        .end_time_unix_secs
                        .is_none_or(|value| value >= now_unix_secs)
                    && !reads.contains(&(user_id.to_string(), announcement.id.clone()))
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .is_pinned
                .cmp(&left.is_pinned)
                .then_with(|| right.priority.cmp(&left.priority))
                .then_with(|| right.created_at_unix_ms.cmp(&left.created_at_unix_ms))
                .then_with(|| left.id.cmp(&right.id))
        });
        items.truncate(limit);
        Ok(items)
    }
}

#[async_trait]
impl AnnouncementWriteRepository for InMemoryAnnouncementReadRepository {
    async fn create_announcement(
        &self,
        record: CreateAnnouncementRecord,
    ) -> Result<StoredAnnouncement, DataLayerError> {
        record.validate()?;
        let now_unix_secs = Self::now_unix_secs();
        let announcement = StoredAnnouncement::new(
            Uuid::new_v4().to_string(),
            record.title,
            record.content,
            record.kind,
            record.priority,
            true,
            record.is_pinned,
            record.requires_ack,
            Some(record.author_id),
            None,
            record.start_time_unix_secs.map(|value| value as i64),
            record.end_time_unix_secs.map(|value| value as i64),
            now_unix_secs as i64,
            now_unix_secs as i64,
        )?;
        self.announcements
            .write()
            .expect("announcement repository lock")
            .push(announcement.clone());
        Ok(announcement)
    }

    async fn update_announcement(
        &self,
        record: UpdateAnnouncementRecord,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        record.validate()?;
        let mut announcements = self
            .announcements
            .write()
            .expect("announcement repository lock");
        let Some(announcement) = announcements
            .iter_mut()
            .find(|announcement| announcement.id == record.announcement_id)
        else {
            return Ok(None);
        };

        if let Some(title) = record.title {
            announcement.title = title;
        }
        if let Some(content) = record.content {
            announcement.content = content;
        }
        if let Some(kind) = record.kind {
            announcement.kind = kind;
        }
        if let Some(priority) = record.priority {
            announcement.priority = priority;
        }
        if let Some(is_active) = record.is_active {
            announcement.is_active = is_active;
        }
        if let Some(is_pinned) = record.is_pinned {
            announcement.is_pinned = is_pinned;
        }
        if let Some(requires_ack) = record.requires_ack {
            announcement.requires_ack = requires_ack;
        }
        if let Some(start_time_unix_secs) = record.start_time_unix_secs {
            announcement.start_time_unix_secs = Some(start_time_unix_secs);
        }
        if let Some(end_time_unix_secs) = record.end_time_unix_secs {
            announcement.end_time_unix_secs = Some(end_time_unix_secs);
        }
        announcement.updated_at_unix_secs = Self::now_unix_secs();
        Ok(Some(announcement.clone()))
    }

    async fn delete_announcement(&self, announcement_id: &str) -> Result<bool, DataLayerError> {
        let mut announcements = self
            .announcements
            .write()
            .expect("announcement repository lock");
        let original_len = announcements.len();
        announcements.retain(|announcement| announcement.id != announcement_id);
        let deleted = announcements.len() != original_len;
        if deleted {
            self.announcement_reads
                .write()
                .expect("announcement reads repository lock")
                .retain(|(_, read_announcement_id)| read_announcement_id != announcement_id);
        }
        Ok(deleted)
    }

    async fn mark_announcement_as_read(
        &self,
        user_id: &str,
        announcement_id: &str,
        _read_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let inserted = self
            .announcement_reads
            .write()
            .expect("announcement reads repository lock")
            .insert((user_id.to_string(), announcement_id.to_string()));
        Ok(inserted)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryAnnouncementReadRepository;
    use crate::repository::announcements::{
        AnnouncementReadRepository, AnnouncementWriteRepository, CreateAnnouncementRecord,
        StoredAnnouncement, UpdateAnnouncementRecord,
    };

    #[tokio::test]
    async fn reads_seeded_announcements() {
        let repository = InMemoryAnnouncementReadRepository::seed(vec![StoredAnnouncement::new(
            "announcement-1".to_string(),
            "系统维护".to_string(),
            "今天晚些时候维护".to_string(),
            "maintenance".to_string(),
            10,
            true,
            true,
            false,
            Some("admin-1".to_string()),
            Some("admin".to_string()),
            None,
            None,
            1_711_000_000,
            1_711_000_100,
        )
        .expect("announcement should build")]);

        let announcement = repository
            .find_by_id("announcement-1")
            .await
            .expect("announcement should load")
            .expect("announcement should exist");

        assert_eq!(announcement.title, "系统维护");
        assert_eq!(announcement.author_username.as_deref(), Some("admin"));
    }

    #[tokio::test]
    async fn mutates_seeded_announcements() {
        let repository = InMemoryAnnouncementReadRepository::seed(vec![]);

        let created = repository
            .create_announcement(CreateAnnouncementRecord {
                title: "系统维护".to_string(),
                content: "今天晚些时候维护".to_string(),
                kind: "maintenance".to_string(),
                priority: 10,
                is_pinned: true,
                requires_ack: false,
                author_id: "admin-1".to_string(),
                start_time_unix_secs: None,
                end_time_unix_secs: None,
            })
            .await
            .expect("create should succeed");
        assert_eq!(created.kind, "maintenance");

        let updated = repository
            .update_announcement(UpdateAnnouncementRecord {
                announcement_id: created.id.clone(),
                title: Some("系统升级".to_string()),
                content: None,
                kind: Some("important".to_string()),
                priority: Some(99),
                is_active: Some(false),
                is_pinned: Some(false),
                requires_ack: Some(true),
                start_time_unix_secs: None,
                end_time_unix_secs: None,
            })
            .await
            .expect("update should succeed")
            .expect("announcement should exist");
        assert_eq!(updated.title, "系统升级");
        assert_eq!(updated.kind, "important");
        assert!(!updated.is_active);

        let deleted = repository
            .delete_announcement(&created.id)
            .await
            .expect("delete should succeed");
        assert!(deleted);
    }

    #[tokio::test]
    async fn tracks_user_announcement_read_state() {
        let repository = InMemoryAnnouncementReadRepository::seed_with_reads(
            vec![
                StoredAnnouncement::new(
                    "announcement-1".to_string(),
                    "系统维护".to_string(),
                    "今天晚些时候维护".to_string(),
                    "maintenance".to_string(),
                    10,
                    true,
                    true,
                    false,
                    Some("admin-1".to_string()),
                    Some("admin".to_string()),
                    None,
                    None,
                    1_711_000_000,
                    1_711_000_100,
                )
                .expect("announcement should build"),
                StoredAnnouncement::new(
                    "announcement-2".to_string(),
                    "系统升级".to_string(),
                    "升级说明".to_string(),
                    "info".to_string(),
                    5,
                    true,
                    false,
                    false,
                    Some("admin-1".to_string()),
                    Some("admin".to_string()),
                    None,
                    None,
                    1_711_000_000,
                    1_711_000_100,
                )
                .expect("announcement should build"),
            ],
            [("user-1".to_string(), "announcement-1".to_string())],
        );

        let unread = repository
            .count_unread_active_announcements("user-1", 1_711_000_200)
            .await
            .expect("count should succeed");
        assert_eq!(unread, 1);

        let inserted = repository
            .mark_announcement_as_read("user-1", "announcement-2", 1_711_000_300)
            .await
            .expect("mark read should succeed");
        assert!(inserted);

        let unread = repository
            .count_unread_active_announcements("user-1", 1_711_000_200)
            .await
            .expect("count should succeed");
        assert_eq!(unread, 0);
    }
}
