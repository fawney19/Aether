use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredAnnouncement {
    pub id: String,
    pub title: String,
    pub content: String,
    pub kind: String,
    pub priority: i32,
    pub is_active: bool,
    pub is_pinned: bool,
    pub requires_ack: bool,
    pub author_id: Option<String>,
    pub author_username: Option<String>,
    pub start_time_unix_secs: Option<u64>,
    pub end_time_unix_secs: Option<u64>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_secs: u64,
}

impl StoredAnnouncement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        title: String,
        content: String,
        kind: String,
        priority: i32,
        is_active: bool,
        is_pinned: bool,
        requires_ack: bool,
        author_id: Option<String>,
        author_username: Option<String>,
        start_time_unix_secs: Option<i64>,
        end_time_unix_secs: Option<i64>,
        created_at_unix_ms: i64,
        updated_at_unix_secs: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "announcements.id is empty".to_string(),
            ));
        }
        if title.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "announcements.title is empty".to_string(),
            ));
        }
        if content.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "announcements.content is empty".to_string(),
            ));
        }
        if kind.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "announcements.type is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            title,
            content,
            kind,
            priority,
            is_active,
            is_pinned,
            requires_ack,
            author_id,
            author_username,
            start_time_unix_secs: start_time_unix_secs
                .map(|value| parse_timestamp(value, "announcements.start_time"))
                .transpose()?,
            end_time_unix_secs: end_time_unix_secs
                .map(|value| parse_timestamp(value, "announcements.end_time"))
                .transpose()?,
            created_at_unix_ms: parse_timestamp(created_at_unix_ms, "announcements.created_at")?,
            updated_at_unix_secs: parse_timestamp(
                updated_at_unix_secs,
                "announcements.updated_at",
            )?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AnnouncementListQuery {
    pub active_only: bool,
    pub offset: usize,
    pub limit: usize,
    pub now_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAnnouncementPage {
    pub items: Vec<StoredAnnouncement>,
    pub total: u64,
}

fn parse_timestamp(value: i64, field: &str) -> Result<u64, crate::DataLayerError> {
    u64::try_from(value).map_err(|_| {
        crate::DataLayerError::UnexpectedValue(format!("{field} is negative: {value}"))
    })
}

#[async_trait]
pub trait AnnouncementReadRepository: Send + Sync {
    async fn find_by_id(
        &self,
        announcement_id: &str,
    ) -> Result<Option<StoredAnnouncement>, crate::DataLayerError>;

    async fn list_announcements(
        &self,
        query: &AnnouncementListQuery,
    ) -> Result<StoredAnnouncementPage, crate::DataLayerError>;

    async fn count_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<u64, crate::DataLayerError>;

    async fn list_required_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredAnnouncement>, crate::DataLayerError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreateAnnouncementRecord {
    pub title: String,
    pub content: String,
    pub kind: String,
    pub priority: i32,
    pub is_pinned: bool,
    pub requires_ack: bool,
    pub author_id: String,
    pub start_time_unix_secs: Option<u64>,
    pub end_time_unix_secs: Option<u64>,
}

impl CreateAnnouncementRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.title.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "announcement title cannot be empty".to_string(),
            ));
        }
        if self.content.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "announcement content cannot be empty".to_string(),
            ));
        }
        if self.kind.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "announcement type cannot be empty".to_string(),
            ));
        }
        if self.author_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "announcement author_id cannot be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdateAnnouncementRecord {
    pub announcement_id: String,
    pub title: Option<String>,
    pub content: Option<String>,
    pub kind: Option<String>,
    pub priority: Option<i32>,
    pub is_active: Option<bool>,
    pub is_pinned: Option<bool>,
    pub requires_ack: Option<bool>,
    pub start_time_unix_secs: Option<u64>,
    pub end_time_unix_secs: Option<u64>,
}

impl UpdateAnnouncementRecord {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.announcement_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "announcement_id cannot be empty".to_string(),
            ));
        }
        if self
            .title
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(crate::DataLayerError::InvalidInput(
                "announcement title cannot be empty".to_string(),
            ));
        }
        if self
            .content
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(crate::DataLayerError::InvalidInput(
                "announcement content cannot be empty".to_string(),
            ));
        }
        if self
            .kind
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(crate::DataLayerError::InvalidInput(
                "announcement type cannot be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
pub trait AnnouncementWriteRepository: Send + Sync {
    async fn create_announcement(
        &self,
        record: CreateAnnouncementRecord,
    ) -> Result<StoredAnnouncement, crate::DataLayerError>;

    async fn update_announcement(
        &self,
        record: UpdateAnnouncementRecord,
    ) -> Result<Option<StoredAnnouncement>, crate::DataLayerError>;

    async fn delete_announcement(
        &self,
        announcement_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn mark_announcement_as_read(
        &self,
        user_id: &str,
        announcement_id: &str,
        read_at_unix_secs: u64,
    ) -> Result<bool, crate::DataLayerError>;
}
