use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use futures_util::TryStreamExt;
use sqlx::{postgres::PgRow, PgPool, Row};

use super::types::{
    AnnouncementListQuery, AnnouncementReadRepository, AnnouncementWriteRepository,
    CreateAnnouncementRecord, StoredAnnouncement, StoredAnnouncementPage, UpdateAnnouncementRecord,
};
use crate::{error::SqlxResultExt, DataLayerError};

const FIND_ANNOUNCEMENT_BY_ID_SQL: &str = r#"
SELECT
  a.id,
  a.title,
  a.content,
  a.type,
  a.priority,
  a.is_active,
  a.is_pinned,
  a.author_id,
  u.username AS author_username,
  EXTRACT(EPOCH FROM a.start_time)::bigint AS start_time_unix_secs,
  EXTRACT(EPOCH FROM a.end_time)::bigint AS end_time_unix_secs,
  EXTRACT(EPOCH FROM a.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM a.updated_at)::bigint AS updated_at_unix_secs
FROM announcements a
LEFT JOIN users u ON u.id = a.author_id
WHERE a.id = $1
LIMIT 1
"#;

const LIST_ANNOUNCEMENTS_SQL: &str = r#"
SELECT
  a.id,
  a.title,
  a.content,
  a.type,
  a.priority,
  a.is_active,
  a.is_pinned,
  a.author_id,
  u.username AS author_username,
  EXTRACT(EPOCH FROM a.start_time)::bigint AS start_time_unix_secs,
  EXTRACT(EPOCH FROM a.end_time)::bigint AS end_time_unix_secs,
  EXTRACT(EPOCH FROM a.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM a.updated_at)::bigint AS updated_at_unix_secs
FROM announcements a
LEFT JOIN users u ON u.id = a.author_id
WHERE (
  NOT $1 OR (
    a.is_active = TRUE
    AND (a.start_time IS NULL OR a.start_time <= TO_TIMESTAMP($2::double precision))
    AND (a.end_time IS NULL OR a.end_time >= TO_TIMESTAMP($2::double precision))
  )
)
ORDER BY a.is_pinned DESC, a.priority DESC, a.created_at DESC, a.id ASC
OFFSET $3
LIMIT $4
"#;

const COUNT_ANNOUNCEMENTS_SQL: &str = r#"
SELECT COUNT(a.id) AS total
FROM announcements a
WHERE (
  NOT $1 OR (
    a.is_active = TRUE
    AND (a.start_time IS NULL OR a.start_time <= TO_TIMESTAMP($2::double precision))
    AND (a.end_time IS NULL OR a.end_time >= TO_TIMESTAMP($2::double precision))
  )
)
"#;

const COUNT_UNREAD_ACTIVE_ANNOUNCEMENTS_SQL: &str = r#"
SELECT COUNT(a.id) AS total
FROM announcements a
WHERE a.is_active = TRUE
  AND (a.start_time IS NULL OR a.start_time <= TO_TIMESTAMP($2::double precision))
  AND (a.end_time IS NULL OR a.end_time >= TO_TIMESTAMP($2::double precision))
  AND NOT EXISTS (
    SELECT 1
    FROM announcement_reads r
    WHERE r.user_id = $1
      AND r.announcement_id = a.id
  )
"#;

const CREATE_ANNOUNCEMENT_SQL: &str = r#"
INSERT INTO announcements (
  id,
  title,
  content,
  type,
  priority,
  author_id,
  is_active,
  is_pinned,
  start_time,
  end_time,
  created_at,
  updated_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  TRUE,
  $7,
  $8,
  $9,
  NOW(),
  NOW()
)
RETURNING
  id,
  title,
  content,
  type,
  priority,
  is_active,
  is_pinned,
  author_id,
  (SELECT username FROM users WHERE id = announcements.author_id) AS author_username,
  EXTRACT(EPOCH FROM start_time)::bigint AS start_time_unix_secs,
  EXTRACT(EPOCH FROM end_time)::bigint AS end_time_unix_secs,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const UPDATE_ANNOUNCEMENT_SQL: &str = r#"
UPDATE announcements
SET
  title = COALESCE($2, title),
  content = COALESCE($3, content),
  type = COALESCE($4, type),
  priority = COALESCE($5, priority),
  is_active = COALESCE($6, is_active),
  is_pinned = COALESCE($7, is_pinned),
  start_time = COALESCE($8, start_time),
  end_time = COALESCE($9, end_time),
  updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  title,
  content,
  type,
  priority,
  is_active,
  is_pinned,
  author_id,
  (SELECT username FROM users WHERE id = announcements.author_id) AS author_username,
  EXTRACT(EPOCH FROM start_time)::bigint AS start_time_unix_secs,
  EXTRACT(EPOCH FROM end_time)::bigint AS end_time_unix_secs,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const DELETE_ANNOUNCEMENT_SQL: &str = r#"
DELETE FROM announcements
WHERE id = $1
"#;

const MARK_ANNOUNCEMENT_AS_READ_SQL: &str = r#"
INSERT INTO announcement_reads (
  id,
  user_id,
  announcement_id,
  read_at
)
VALUES (
  $1,
  $2,
  $3,
  TO_TIMESTAMP($4::double precision)
)
ON CONFLICT (user_id, announcement_id) DO NOTHING
"#;

#[derive(Debug, Clone)]
pub struct SqlxAnnouncementReadRepository {
    pool: PgPool,
}

impl SqlxAnnouncementReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AnnouncementReadRepository for SqlxAnnouncementReadRepository {
    async fn find_by_id(
        &self,
        announcement_id: &str,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        let row = sqlx::query(FIND_ANNOUNCEMENT_BY_ID_SQL)
            .bind(announcement_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_announcement_row).transpose()
    }

    async fn list_announcements(
        &self,
        query: &AnnouncementListQuery,
    ) -> Result<StoredAnnouncementPage, DataLayerError> {
        let now_unix_secs = query.now_unix_secs.unwrap_or_else(current_unix_secs);
        let total_row = sqlx::query(COUNT_ANNOUNCEMENTS_SQL)
            .bind(query.active_only)
            .bind(now_unix_secs as f64)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let total = total_row
            .try_get::<i64, _>("total")
            .map_postgres_err()?
            .max(0) as u64;

        let mut rows = sqlx::query(LIST_ANNOUNCEMENTS_SQL)
            .bind(query.active_only)
            .bind(now_unix_secs as f64)
            .bind(query.offset as i64)
            .bind(query.limit as i64)
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_announcement_row(&row)?);
        }

        Ok(StoredAnnouncementPage { items, total })
    }

    async fn count_unread_active_announcements(
        &self,
        user_id: &str,
        now_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        let row = sqlx::query(COUNT_UNREAD_ACTIVE_ANNOUNCEMENTS_SQL)
            .bind(user_id)
            .bind(now_unix_secs as f64)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64)
    }
}

#[async_trait]
impl AnnouncementWriteRepository for SqlxAnnouncementReadRepository {
    async fn create_announcement(
        &self,
        record: CreateAnnouncementRecord,
    ) -> Result<StoredAnnouncement, DataLayerError> {
        record.validate()?;
        let row = sqlx::query(CREATE_ANNOUNCEMENT_SQL)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(record.title)
            .bind(record.content)
            .bind(record.kind)
            .bind(record.priority)
            .bind(record.author_id)
            .bind(record.is_pinned)
            .bind(optional_datetime(record.start_time_unix_secs))
            .bind(optional_datetime(record.end_time_unix_secs))
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        map_announcement_row(&row)
    }

    async fn update_announcement(
        &self,
        record: UpdateAnnouncementRecord,
    ) -> Result<Option<StoredAnnouncement>, DataLayerError> {
        record.validate()?;
        let row = sqlx::query(UPDATE_ANNOUNCEMENT_SQL)
            .bind(record.announcement_id)
            .bind(record.title)
            .bind(record.content)
            .bind(record.kind)
            .bind(record.priority)
            .bind(record.is_active)
            .bind(record.is_pinned)
            .bind(optional_datetime(record.start_time_unix_secs))
            .bind(optional_datetime(record.end_time_unix_secs))
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_announcement_row).transpose()
    }

    async fn delete_announcement(&self, announcement_id: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query(DELETE_ANNOUNCEMENT_SQL)
            .bind(announcement_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn mark_announcement_as_read(
        &self,
        user_id: &str,
        announcement_id: &str,
        read_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(MARK_ANNOUNCEMENT_AS_READ_SQL)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(user_id)
            .bind(announcement_id)
            .bind(read_at_unix_secs as f64)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }
}

fn optional_datetime(unix_secs: Option<u64>) -> Option<chrono::DateTime<Utc>> {
    unix_secs.and_then(|value| {
        i64::try_from(value)
            .ok()
            .and_then(|value| Utc.timestamp_opt(value, 0).single())
    })
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn map_announcement_row(row: &PgRow) -> Result<StoredAnnouncement, DataLayerError> {
    StoredAnnouncement::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("title").map_postgres_err()?,
        row.try_get("content").map_postgres_err()?,
        row.try_get("type").map_postgres_err()?,
        row.try_get("priority").map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
        row.try_get("is_pinned").map_postgres_err()?,
        row.try_get("author_id").map_postgres_err()?,
        row.try_get("author_username").map_postgres_err()?,
        row.try_get("start_time_unix_secs").map_postgres_err()?,
        row.try_get("end_time_unix_secs").map_postgres_err()?,
        row.try_get("created_at_unix_ms").map_postgres_err()?,
        row.try_get("updated_at_unix_secs").map_postgres_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::SqlxAnnouncementReadRepository;
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};

    #[tokio::test]
    async fn repository_constructs_from_lazy_pool() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");

        let pool = factory.connect_lazy().expect("pool should build");
        let _repository = SqlxAnnouncementReadRepository::new(pool);
    }
}
