use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, Row};

use aether_data_contracts::repository::audit::*;
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::SqlitePool;

#[derive(Debug, Clone)]
pub struct SqliteAuditLogReadRepository {
    pool: SqlitePool,
}

impl SqliteAuditLogReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditLogReadRepository for SqliteAuditLogReadRepository {
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
  AND (? IS NULL OR LOWER(u.username) LIKE LOWER(?) ESCAPE '\')
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
  AND (? IS NULL OR LOWER(u.username) LIKE LOWER(?) ESCAPE '\')
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
            .map(map_sqlite_admin_audit_log_row)
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

        rows.iter()
            .map(map_sqlite_suspicious_activity_row)
            .collect()
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
            .filter_map(|row| event_count_from_sqlite_row(row).ok())
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
            .map(map_sqlite_user_audit_log_row)
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
    FROM audit_logs
    WHERE created_at < ?
    ORDER BY created_at ASC, id ASC
    LIMIT ?
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

fn sqlite_created_at_unix_secs(row: &SqliteRow) -> Result<u64, DataLayerError> {
    let value = row.try_get::<i64, _>("created_at").map_sql_err()?;
    Ok(value.max(0) as u64)
}

fn map_sqlite_admin_audit_log_row(row: &SqliteRow) -> Result<StoredAdminAuditLog, DataLayerError> {
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
        created_at_unix_secs: sqlite_created_at_unix_secs(row)?,
    })
}

fn map_sqlite_suspicious_activity_row(
    row: &SqliteRow,
) -> Result<StoredSuspiciousActivity, DataLayerError> {
    Ok(StoredSuspiciousActivity {
        id: row.try_get("id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        user_id: row.try_get("user_id").map_sql_err()?,
        description: row.try_get("description").map_sql_err()?,
        ip_address: row.try_get("ip_address").map_sql_err()?,
        metadata: optional_json_from_text(row.try_get("metadata").map_sql_err()?)?,
        created_at_unix_secs: sqlite_created_at_unix_secs(row)?,
    })
}

fn map_sqlite_user_audit_log_row(row: &SqliteRow) -> Result<StoredUserAuditLog, DataLayerError> {
    Ok(StoredUserAuditLog {
        id: row.try_get("id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        description: row.try_get("description").map_sql_err()?,
        ip_address: row.try_get("ip_address").map_sql_err()?,
        status_code: row.try_get("status_code").map_sql_err()?,
        created_at_unix_secs: sqlite_created_at_unix_secs(row)?,
    })
}

fn event_count_from_sqlite_row(row: &SqliteRow) -> Result<(String, u64), DataLayerError> {
    let event_type = row.try_get("event_type").map_sql_err()?;
    let count = row.try_get::<i64, _>("count").map_sql_err()?.max(0) as u64;
    Ok((event_type, count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_migrations;

    #[tokio::test]
    async fn sqlite_audit_log_repository_reads_monitoring_views() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_sqlite_audit_logs(&pool).await;

        let repository = SqliteAuditLogReadRepository::new(pool);
        let admin_page = repository
            .list_admin_audit_logs(&AuditLogListQuery {
                cutoff_unix_secs: 150,
                username_pattern: Some("%ali%".to_string()),
                event_type: Some("login_failed".to_string()),
                limit: 10,
                offset: 0,
            })
            .await
            .expect("admin audit logs should read");
        assert_eq!(admin_page.total, 1);
        assert_eq!(admin_page.items[0].id, "audit-2");
        assert_eq!(admin_page.items[0].user_username.as_deref(), Some("alice"));
        assert_eq!(
            admin_page.items[0]
                .metadata
                .as_ref()
                .and_then(|value| value.get("risk"))
                .and_then(|value| value.as_str()),
            Some("high")
        );

        let suspicious = repository
            .list_admin_suspicious_activities(150)
            .await
            .expect("suspicious activities should read");
        assert_eq!(suspicious.len(), 1);
        assert_eq!(suspicious[0].event_type, "login_failed");

        let counts = repository
            .read_admin_user_behavior_event_counts("user-1", 0)
            .await
            .expect("user behavior counts should read");
        assert_eq!(counts.get("login_failed"), Some(&1));
        assert_eq!(counts.get("request_success"), Some(&1));

        let user_page = repository
            .list_user_audit_logs(
                "user-1",
                &AuditLogListQuery {
                    cutoff_unix_secs: 0,
                    username_pattern: None,
                    event_type: Some("request_success".to_string()),
                    limit: 10,
                    offset: 0,
                },
            )
            .await
            .expect("user audit logs should read");
        assert_eq!(user_page.total, 1);
        assert_eq!(user_page.items[0].id, "audit-1");
        assert_eq!(user_page.items[0].status_code, Some(200));

        let deleted = repository
            .delete_audit_logs_before(250, 1)
            .await
            .expect("audit cleanup should delete one old row");
        assert_eq!(deleted, 1);
        let user_page = repository
            .list_user_audit_logs(
                "user-1",
                &AuditLogListQuery {
                    cutoff_unix_secs: 0,
                    username_pattern: None,
                    event_type: Some("request_success".to_string()),
                    limit: 10,
                    offset: 0,
                },
            )
            .await
            .expect("user audit logs should read after cleanup");
        assert_eq!(user_page.total, 0);
    }

    async fn seed_sqlite_audit_logs(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO users (id, email, username, role, auth_source, created_at, updated_at)
VALUES
  ('user-1', 'alice@example.com', 'alice', 'user', 'local', 1, 1),
  ('user-2', 'bob@example.com', 'bob', 'user', 'local', 1, 1)
"#,
        )
        .execute(pool)
        .await
        .expect("users should insert");

        sqlx::query(
            r#"
INSERT INTO audit_logs (
    id,
    event_type,
    user_id,
    description,
    ip_address,
    event_metadata,
    status_code,
    created_at
)
VALUES
  ('audit-1', 'request_success', 'user-1', 'completed request', '127.0.0.1', NULL, 200, 100),
  ('audit-2', 'login_failed', 'user-1', 'failed login', '127.0.0.2', '{"risk":"high"}', 401, 200),
  ('audit-3', 'password_changed', 'user-2', 'other user changed password', '127.0.0.3', NULL, 200, 300)
"#,
        )
        .execute(pool)
        .await
        .expect("audit logs should insert");
    }
}
