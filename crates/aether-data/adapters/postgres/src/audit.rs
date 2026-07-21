use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use sqlx::{postgres::PgRow, Row};

use aether_data_contracts::repository::audit::*;
use aether_data_contracts::DataLayerError;

use crate::error::SqlxResultExt;
use crate::PostgresPool;

#[derive(Debug, Clone)]
pub struct PostgresAuditLogReadRepository {
    pool: PostgresPool,
}

impl PostgresAuditLogReadRepository {
    pub fn new(pool: PostgresPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditLogReadRepository for PostgresAuditLogReadRepository {
    async fn list_admin_audit_logs(
        &self,
        query: &AuditLogListQuery,
    ) -> Result<StoredAdminAuditLogPage, DataLayerError> {
        let cutoff_time = postgres_cutoff_time(query.cutoff_unix_secs);
        let total = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(*)
FROM audit_logs AS a
LEFT JOIN users AS u ON a.user_id = u.id
WHERE a.created_at >= $1
  AND ($2::text IS NULL OR u.username ILIKE $2 ESCAPE '\')
  AND ($3::text IS NULL OR a.event_type = $3)
"#,
        )
        .bind(cutoff_time)
        .bind(query.username_pattern.as_deref())
        .bind(query.event_type.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;

        let mut rows = sqlx::query(
            r#"
SELECT
  a.id,
  a.event_type,
  a.user_id,
  u.email AS user_email,
  u.username AS user_username,
  a.description,
  a.ip_address,
  a.status_code,
  a.error_message,
  a.event_metadata AS metadata,
  a.created_at
FROM audit_logs AS a
LEFT JOIN users AS u ON a.user_id = u.id
WHERE a.created_at >= $1
  AND ($2::text IS NULL OR u.username ILIKE $2 ESCAPE '\')
  AND ($3::text IS NULL OR a.event_type = $3)
ORDER BY a.created_at DESC
LIMIT $4 OFFSET $5
"#,
        )
        .bind(cutoff_time)
        .bind(query.username_pattern.as_deref())
        .bind(query.event_type.as_deref())
        .bind(i64::try_from(query.limit).unwrap_or(i64::MAX))
        .bind(i64::try_from(query.offset).unwrap_or(i64::MAX))
        .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_postgres_admin_audit_log_row(&row)?);
        }

        Ok(StoredAdminAuditLogPage {
            items,
            total: total.max(0) as u64,
        })
    }

    async fn list_admin_suspicious_activities(
        &self,
        cutoff_unix_secs: u64,
    ) -> Result<Vec<StoredSuspiciousActivity>, DataLayerError> {
        let cutoff_time = postgres_cutoff_time(cutoff_unix_secs);
        let mut rows = sqlx::query(
            r#"
SELECT
  id,
  event_type,
  user_id,
  description,
  ip_address,
  event_metadata AS metadata,
  created_at
FROM audit_logs
WHERE created_at >= $1
  AND event_type = ANY($2)
ORDER BY created_at DESC
LIMIT 100
"#,
        )
        .bind(cutoff_time)
        .bind(SUSPICIOUS_EVENT_TYPES.to_vec())
        .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_postgres_suspicious_activity_row(&row)?);
        }
        Ok(items)
    }

    async fn read_admin_user_behavior_event_counts(
        &self,
        user_id: &str,
        cutoff_unix_secs: u64,
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        let cutoff_time = postgres_cutoff_time(cutoff_unix_secs);
        let mut rows = sqlx::query(
            r#"
SELECT event_type, COUNT(*)::bigint AS count
FROM audit_logs
WHERE user_id = $1
  AND created_at >= $2
GROUP BY event_type
"#,
        )
        .bind(user_id)
        .bind(cutoff_time)
        .fetch(&self.pool);

        let mut counts = std::collections::BTreeMap::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            if let Ok((event_type, count)) = event_count_from_postgres_row(&row) {
                counts.insert(event_type, count);
            }
        }
        Ok(counts)
    }

    async fn list_user_audit_logs(
        &self,
        user_id: &str,
        query: &AuditLogListQuery,
    ) -> Result<StoredUserAuditLogPage, DataLayerError> {
        let cutoff_time = postgres_cutoff_time(query.cutoff_unix_secs);
        let total = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(*)
FROM audit_logs
WHERE user_id = $1
  AND created_at >= $2
  AND ($3::text IS NULL OR event_type = $3)
"#,
        )
        .bind(user_id)
        .bind(cutoff_time)
        .bind(query.event_type.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;

        let mut rows = sqlx::query(
            r#"
SELECT id, event_type, description, ip_address, status_code, created_at
FROM audit_logs
WHERE user_id = $1
  AND created_at >= $2
  AND ($3::text IS NULL OR event_type = $3)
ORDER BY created_at DESC
LIMIT $4 OFFSET $5
"#,
        )
        .bind(user_id)
        .bind(cutoff_time)
        .bind(query.event_type.as_deref())
        .bind(i64::try_from(query.limit).unwrap_or(i64::MAX))
        .bind(i64::try_from(query.offset).unwrap_or(i64::MAX))
        .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_postgres_user_audit_log_row(&row)?);
        }

        Ok(StoredUserAuditLogPage {
            items,
            total: total.max(0) as u64,
        })
    }

    async fn delete_audit_logs_before(
        &self,
        cutoff_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        let deleted = sqlx::query(
            r#"
WITH doomed AS (
    SELECT id
    FROM audit_logs
    WHERE created_at < $1
    ORDER BY created_at ASC, id ASC
    LIMIT $2
)
DELETE FROM audit_logs AS audit
USING doomed
WHERE audit.id = doomed.id
"#,
        )
        .bind(postgres_cutoff_time(cutoff_unix_secs))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();
        Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
    }
}

fn postgres_cutoff_time(cutoff_unix_secs: u64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(cutoff_unix_secs.min(i64::MAX as u64) as i64, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("unix epoch is valid"))
}

fn postgres_created_at_unix_secs(row: &PgRow) -> Result<u64, DataLayerError> {
    let value = row
        .try_get::<DateTime<Utc>, _>("created_at")
        .map_postgres_err()?;
    Ok(value.timestamp().max(0) as u64)
}

fn map_postgres_admin_audit_log_row(row: &PgRow) -> Result<StoredAdminAuditLog, DataLayerError> {
    Ok(StoredAdminAuditLog {
        id: row.try_get("id").map_postgres_err()?,
        event_type: row.try_get("event_type").map_postgres_err()?,
        user_id: row.try_get("user_id").map_postgres_err()?,
        user_email: row.try_get("user_email").map_postgres_err()?,
        user_username: row.try_get("user_username").map_postgres_err()?,
        description: row.try_get("description").map_postgres_err()?,
        ip_address: row.try_get("ip_address").map_postgres_err()?,
        status_code: row.try_get("status_code").map_postgres_err()?,
        error_message: row.try_get("error_message").map_postgres_err()?,
        metadata: row.try_get("metadata").map_postgres_err()?,
        created_at_unix_secs: postgres_created_at_unix_secs(row)?,
    })
}

fn map_postgres_suspicious_activity_row(
    row: &PgRow,
) -> Result<StoredSuspiciousActivity, DataLayerError> {
    Ok(StoredSuspiciousActivity {
        id: row.try_get("id").map_postgres_err()?,
        event_type: row.try_get("event_type").map_postgres_err()?,
        user_id: row.try_get("user_id").map_postgres_err()?,
        description: row.try_get("description").map_postgres_err()?,
        ip_address: row.try_get("ip_address").map_postgres_err()?,
        metadata: row.try_get("metadata").map_postgres_err()?,
        created_at_unix_secs: postgres_created_at_unix_secs(row)?,
    })
}

fn map_postgres_user_audit_log_row(row: &PgRow) -> Result<StoredUserAuditLog, DataLayerError> {
    Ok(StoredUserAuditLog {
        id: row.try_get("id").map_postgres_err()?,
        event_type: row.try_get("event_type").map_postgres_err()?,
        description: row.try_get("description").map_postgres_err()?,
        ip_address: row.try_get("ip_address").map_postgres_err()?,
        status_code: row.try_get("status_code").map_postgres_err()?,
        created_at_unix_secs: postgres_created_at_unix_secs(row)?,
    })
}

fn event_count_from_postgres_row(row: &PgRow) -> Result<(String, u64), DataLayerError> {
    let event_type = row.try_get("event_type").map_postgres_err()?;
    let count = row.try_get::<i64, _>("count").map_postgres_err()?.max(0) as u64;
    Ok((event_type, count))
}
