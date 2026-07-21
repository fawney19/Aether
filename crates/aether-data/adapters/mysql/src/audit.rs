use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, Row};

use aether_data_contracts::repository::audit::*;
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::MysqlPool;

#[derive(Debug, Clone)]
pub struct MysqlAuditLogReadRepository {
    pool: MysqlPool,
}

impl MysqlAuditLogReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditLogReadRepository for MysqlAuditLogReadRepository {
    async fn list_admin_audit_logs(
        &self,
        query: &AuditLogListQuery,
    ) -> Result<StoredAdminAuditLogPage, DataLayerError> {
        let total = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(*)
FROM audit_logs AS a
LEFT JOIN users AS u ON a.user_id = u.id
WHERE a.created_at >= ?
  AND (? IS NULL OR LOWER(u.username) LIKE LOWER(?) ESCAPE '\\')
  AND (? IS NULL OR a.event_type = ?)
"#,
        )
        .bind(query.cutoff_unix_secs as i64)
        .bind(query.username_pattern.as_deref())
        .bind(query.username_pattern.as_deref())
        .bind(query.event_type.as_deref())
        .bind(query.event_type.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;

        let rows = sqlx::query(
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
WHERE a.created_at >= ?
  AND (? IS NULL OR LOWER(u.username) LIKE LOWER(?) ESCAPE '\\')
  AND (? IS NULL OR a.event_type = ?)
ORDER BY a.created_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(query.cutoff_unix_secs as i64)
        .bind(query.username_pattern.as_deref())
        .bind(query.username_pattern.as_deref())
        .bind(query.event_type.as_deref())
        .bind(query.event_type.as_deref())
        .bind(query.limit as i64)
        .bind(query.offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        let items = rows
            .iter()
            .map(map_mysql_admin_audit_log_row)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(StoredAdminAuditLogPage {
            items,
            total: total.max(0) as u64,
        })
    }

    async fn list_admin_suspicious_activities(
        &self,
        cutoff_unix_secs: u64,
    ) -> Result<Vec<StoredSuspiciousActivity>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT id, event_type, user_id, description, ip_address, event_metadata AS metadata, created_at
FROM audit_logs
WHERE created_at >= ?
  AND event_type IN (?, ?, ?, ?)
ORDER BY created_at DESC
LIMIT 100
"#,
        )
        .bind(cutoff_unix_secs as i64)
        .bind(SUSPICIOUS_EVENT_TYPES[0])
        .bind(SUSPICIOUS_EVENT_TYPES[1])
        .bind(SUSPICIOUS_EVENT_TYPES[2])
        .bind(SUSPICIOUS_EVENT_TYPES[3])
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        rows.iter().map(map_mysql_suspicious_activity_row).collect()
    }

    async fn read_admin_user_behavior_event_counts(
        &self,
        user_id: &str,
        cutoff_unix_secs: u64,
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT event_type, COUNT(*) AS count
FROM audit_logs
WHERE user_id = ?
  AND created_at >= ?
GROUP BY event_type
"#,
        )
        .bind(user_id)
        .bind(cutoff_unix_secs as i64)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        Ok(rows
            .iter()
            .filter_map(|row| event_count_from_mysql_row(row).ok())
            .collect())
    }

    async fn list_user_audit_logs(
        &self,
        user_id: &str,
        query: &AuditLogListQuery,
    ) -> Result<StoredUserAuditLogPage, DataLayerError> {
        let total = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(*)
FROM audit_logs
WHERE user_id = ?
  AND created_at >= ?
  AND (? IS NULL OR event_type = ?)
"#,
        )
        .bind(user_id)
        .bind(query.cutoff_unix_secs as i64)
        .bind(query.event_type.as_deref())
        .bind(query.event_type.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;

        let rows = sqlx::query(
            r#"
SELECT id, event_type, description, ip_address, status_code, created_at
FROM audit_logs
WHERE user_id = ?
  AND created_at >= ?
  AND (? IS NULL OR event_type = ?)
ORDER BY created_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(user_id)
        .bind(query.cutoff_unix_secs as i64)
        .bind(query.event_type.as_deref())
        .bind(query.event_type.as_deref())
        .bind(query.limit as i64)
        .bind(query.offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        let items = rows
            .iter()
            .map(map_mysql_user_audit_log_row)
            .collect::<Result<Vec<_>, _>>()?;

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
DELETE FROM audit_logs
WHERE id IN (
    SELECT id
    FROM (
        SELECT id
        FROM audit_logs
        WHERE created_at < ?
        ORDER BY created_at ASC, id ASC
        LIMIT ?
    ) AS doomed
)
"#,
        )
        .bind(cutoff_unix_secs.min(i64::MAX as u64) as i64)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
    }
}

fn mysql_created_at_unix_secs(row: &MySqlRow) -> Result<u64, DataLayerError> {
    let value = row.try_get::<i64, _>("created_at").map_sql_err()?;
    Ok(value.max(0) as u64)
}

fn map_mysql_admin_audit_log_row(row: &MySqlRow) -> Result<StoredAdminAuditLog, DataLayerError> {
    Ok(StoredAdminAuditLog {
        id: row.try_get("id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        user_id: row.try_get("user_id").map_sql_err()?,
        user_email: row.try_get("user_email").map_sql_err()?,
        user_username: row.try_get("user_username").map_sql_err()?,
        description: row.try_get("description").map_sql_err()?,
        ip_address: row.try_get("ip_address").map_sql_err()?,
        status_code: row.try_get("status_code").map_sql_err()?,
        error_message: row.try_get("error_message").map_sql_err()?,
        metadata: optional_json_from_text(row.try_get("metadata").map_sql_err()?)?,
        created_at_unix_secs: mysql_created_at_unix_secs(row)?,
    })
}

fn map_mysql_suspicious_activity_row(
    row: &MySqlRow,
) -> Result<StoredSuspiciousActivity, DataLayerError> {
    Ok(StoredSuspiciousActivity {
        id: row.try_get("id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        user_id: row.try_get("user_id").map_sql_err()?,
        description: row.try_get("description").map_sql_err()?,
        ip_address: row.try_get("ip_address").map_sql_err()?,
        metadata: optional_json_from_text(row.try_get("metadata").map_sql_err()?)?,
        created_at_unix_secs: mysql_created_at_unix_secs(row)?,
    })
}

fn map_mysql_user_audit_log_row(row: &MySqlRow) -> Result<StoredUserAuditLog, DataLayerError> {
    Ok(StoredUserAuditLog {
        id: row.try_get("id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        description: row.try_get("description").map_sql_err()?,
        ip_address: row.try_get("ip_address").map_sql_err()?,
        status_code: row.try_get("status_code").map_sql_err()?,
        created_at_unix_secs: mysql_created_at_unix_secs(row)?,
    })
}

fn event_count_from_mysql_row(row: &MySqlRow) -> Result<(String, u64), DataLayerError> {
    let event_type = row.try_get("event_type").map_sql_err()?;
    let count = row.try_get::<i64, _>("count").map_sql_err()?.max(0) as u64;
    Ok((event_type, count))
}
