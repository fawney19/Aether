use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, Row};

use aether_data_contracts::repository::announcements::*;
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::MysqlPool;

const ANNOUNCEMENT_SELECT: &str = r#"
SELECT
  a.id,
  a.title,
  a.content,
  a.`type` AS type,
  a.priority,
  a.is_active,
  a.is_pinned,
  a.requires_ack,
  a.author_id,
  u.username AS author_username,
  a.start_time AS start_time_unix_secs,
  a.end_time AS end_time_unix_secs,
  a.created_at AS created_at_unix_ms,
  a.updated_at AS updated_at_unix_secs
FROM announcements a
LEFT JOIN users u ON u.id = a.author_id
"#;

#[derive(Debug, Clone)]
pub struct MysqlAnnouncementRepository {
    pool: MysqlPool,
}

impl MysqlAnnouncementRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn reload_by_id(
        &self,
        announcement_id: &str,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        self.find_by_id(announcement_id).await
    }
}

#[async_trait]
impl AnnouncementReadRepository for MysqlAnnouncementRepository {
    async fn find_by_id(
        &self,
        announcement_id: &str,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        let row = sqlx::query(&format!("{ANNOUNCEMENT_SELECT} WHERE a.id = ? LIMIT 1"))
            .bind(announcement_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_announcement_row).transpose()
    }

    async fn list_announcements(
        &self,
        query: &AnnouncementListQuery,
    ) -> Result<StoredAnnouncementPage, DataLayerError> {
        let now_unix_secs = query.now_unix_secs.unwrap_or_else(current_unix_secs);
        let total_row = sqlx::query(
            r#"
SELECT COUNT(a.id) AS total
FROM announcements a
WHERE (
  NOT ? OR (
    a.is_active = 1
    AND (a.start_time IS NULL OR a.start_time <= ?)
    AND (a.end_time IS NULL OR a.end_time >= ?)
  )
)
"#,
        )
        .bind(query.active_only)
        .bind(now_unix_secs as i64)
        .bind(now_unix_secs as i64)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        let total = total_row.try_get::<i64, _>("total").map_sql_err()?.max(0) as u64;

        let rows = sqlx::query(&format!(
            r#"
{ANNOUNCEMENT_SELECT}
WHERE (
  NOT ? OR (
    a.is_active = 1
    AND (a.start_time IS NULL OR a.start_time <= ?)
    AND (a.end_time IS NULL OR a.end_time >= ?)
  )
)
ORDER BY a.is_pinned DESC, a.priority DESC, a.created_at DESC, a.id ASC
LIMIT ? OFFSET ?
"#
        ))
        .bind(query.active_only)
        .bind(now_unix_secs as i64)
        .bind(now_unix_secs as i64)
        .bind(query.limit as i64)
        .bind(query.offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_announcement_row)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(StoredAnnouncementPage { items, total })
    }

    async fn count_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT COUNT(a.id) AS total
FROM announcements a
WHERE a.is_active = 1
  AND (a.start_time IS NULL OR a.start_time <= ?)
  AND (a.end_time IS NULL OR a.end_time >= ?)
  AND NOT EXISTS (
    SELECT 1
    FROM announcement_reads r
    WHERE r.user_id = ?
      AND r.announcement_id = a.id
  )
"#,
        )
        .bind(now_unix_secs as i64)
        .bind(now_unix_secs as i64)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        Ok(row.try_get::<i64, _>("total").map_sql_err()?.max(0) as u64)
    }

    async fn list_required_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredAnnouncement>, DataLayerError> {
        let rows = sqlx::query(&format!(
            r#"
{ANNOUNCEMENT_SELECT}
WHERE a.is_active = 1
  AND a.requires_ack = 1
  AND (a.start_time IS NULL OR a.start_time <= ?)
  AND (a.end_time IS NULL OR a.end_time >= ?)
  AND NOT EXISTS (
    SELECT 1
    FROM announcement_reads r
    WHERE r.user_id = ?
      AND r.announcement_id = a.id
  )
ORDER BY a.is_pinned DESC, a.priority DESC, a.created_at DESC, a.id ASC
LIMIT ?
"#
        ))
        .bind(now_unix_secs as i64)
        .bind(now_unix_secs as i64)
        .bind(user_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_announcement_row).collect()
    }
}

#[async_trait]
impl AnnouncementWriteRepository for MysqlAnnouncementRepository {
    async fn create_announcement(
        &self,
        record: CreateAnnouncementRecord,
    ) -> Result<StoredAnnouncement, DataLayerError> {
        record.validate()?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = current_unix_secs() as i64;
        sqlx::query(
            r#"
INSERT INTO announcements (
  id, title, content, `type`, priority, author_id, is_active, is_pinned,
  requires_ack, start_time, end_time, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&id)
        .bind(record.title)
        .bind(record.content)
        .bind(record.kind)
        .bind(record.priority)
        .bind(record.author_id)
        .bind(record.is_pinned)
        .bind(record.requires_ack)
        .bind(optional_i64_from_u64(
            record.start_time_unix_secs,
            "announcements.start_time",
        )?)
        .bind(optional_i64_from_u64(
            record.end_time_unix_secs,
            "announcements.end_time",
        )?)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_by_id(&id)
            .await?
            .ok_or_else(|| DataLayerError::UnexpectedValue("created announcement missing".into()))
    }

    async fn update_announcement(
        &self,
        record: UpdateAnnouncementRecord,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        record.validate()?;
        let id = record.announcement_id;
        sqlx::query(
            r#"
UPDATE announcements
SET title = COALESCE(?, title),
    content = COALESCE(?, content),
    `type` = COALESCE(?, `type`),
    priority = COALESCE(?, priority),
    is_active = COALESCE(?, is_active),
    is_pinned = COALESCE(?, is_pinned),
    requires_ack = COALESCE(?, requires_ack),
    start_time = COALESCE(?, start_time),
    end_time = COALESCE(?, end_time),
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(record.title)
        .bind(record.content)
        .bind(record.kind)
        .bind(record.priority)
        .bind(record.is_active)
        .bind(record.is_pinned)
        .bind(record.requires_ack)
        .bind(optional_i64_from_u64(
            record.start_time_unix_secs,
            "announcements.start_time",
        )?)
        .bind(optional_i64_from_u64(
            record.end_time_unix_secs,
            "announcements.end_time",
        )?)
        .bind(current_unix_secs() as i64)
        .bind(&id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_by_id(&id).await
    }

    async fn delete_announcement(&self, announcement_id: &str) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        sqlx::query("DELETE FROM announcement_reads WHERE announcement_id = ?")
            .bind(announcement_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        let rows_affected = sqlx::query("DELETE FROM announcements WHERE id = ?")
            .bind(announcement_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?
            .rows_affected();
        tx.commit().await.map_sql_err()?;
        Ok(rows_affected > 0)
    }

    async fn mark_announcement_as_read(
        &self,
        user_id: &str,
        announcement_id: &str,
        read_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let rows_affected = sqlx::query(
            r#"
INSERT IGNORE INTO announcement_reads (id, user_id, announcement_id, read_at)
VALUES (?, ?, ?, ?)
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(announcement_id)
        .bind(i64_from_u64(
            read_at_unix_secs,
            "announcement_reads.read_at",
        )?)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }
}

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn i64_from_u64(value: u64, field_name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field_name} exceeds i64: {value}")))
}

fn optional_i64_from_u64(
    value: Option<u64>,
    field_name: &str,
) -> Result<Option<i64>, DataLayerError> {
    value
        .map(|value| i64_from_u64(value, field_name))
        .transpose()
}

fn map_announcement_row(row: &MySqlRow) -> Result<StoredAnnouncement, DataLayerError> {
    StoredAnnouncement::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("title").map_sql_err()?,
        row.try_get("content").map_sql_err()?,
        row.try_get("type").map_sql_err()?,
        row.try_get("priority").map_sql_err()?,
        row.try_get("is_active").map_sql_err()?,
        row.try_get("is_pinned").map_sql_err()?,
        row.try_get("requires_ack").map_sql_err()?,
        row.try_get("author_id").map_sql_err()?,
        row.try_get("author_username").map_sql_err()?,
        row.try_get("start_time_unix_secs").map_sql_err()?,
        row.try_get("end_time_unix_secs").map_sql_err()?,
        row.try_get("created_at_unix_ms").map_sql_err()?,
        row.try_get("updated_at_unix_secs").map_sql_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::MysqlAnnouncementRepository;

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlAnnouncementRepository::new(pool);
    }
}
