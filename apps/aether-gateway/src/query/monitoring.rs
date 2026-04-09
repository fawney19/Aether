use std::collections::BTreeMap;

use aether_data::postgres::PostgresPool;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::Row;

use crate::GatewayError;

fn internal(err: impl ToString) -> GatewayError {
    GatewayError::Internal(err.to_string())
}

pub(crate) async fn list_admin_audit_logs(
    pool: &PostgresPool,
    cutoff_time: DateTime<Utc>,
    username_pattern: Option<&str>,
    event_type: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<(Vec<Value>, usize), GatewayError> {
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
    .bind(username_pattern)
    .bind(event_type)
    .fetch_one(pool)
    .await
    .map_err(|err| GatewayError::Internal(format!("admin audit logs count failed: {err}")))?;

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
WHERE a.created_at >= $1
  AND ($2::text IS NULL OR u.username ILIKE $2 ESCAPE '\')
  AND ($3::text IS NULL OR a.event_type = $3)
ORDER BY a.created_at DESC
LIMIT $4 OFFSET $5
"#,
    )
    .bind(cutoff_time)
    .bind(username_pattern)
    .bind(event_type)
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .bind(i64::try_from(offset).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await
    .map_err(|err| GatewayError::Internal(format!("admin audit logs read failed: {err}")))?;

    Ok((
        rows.into_iter().map(admin_audit_log_row_to_json).collect(),
        usize::try_from(total.max(0)).unwrap_or(usize::MAX),
    ))
}

pub(crate) async fn list_admin_suspicious_activities(
    pool: &PostgresPool,
    cutoff_time: DateTime<Utc>,
) -> Result<Vec<Value>, GatewayError> {
    let rows = sqlx::query(
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
    .bind(vec![
        "suspicious_activity",
        "unauthorized_access",
        "login_failed",
        "request_rate_limited",
    ])
    .fetch_all(pool)
    .await
    .map_err(|err| {
        GatewayError::Internal(format!("admin suspicious activities read failed: {err}"))
    })?;

    Ok(rows.into_iter().map(admin_suspicious_row_to_json).collect())
}

pub(crate) async fn read_admin_user_behavior_event_counts(
    pool: &PostgresPool,
    user_id: &str,
    cutoff_time: DateTime<Utc>,
) -> Result<BTreeMap<String, u64>, GatewayError> {
    let rows = sqlx::query(
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
    .fetch_all(pool)
    .await
    .map_err(|err| GatewayError::Internal(format!("admin user behavior read failed: {err}")))?;

    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let event_type = row.try_get::<String, _>("event_type").ok()?;
            let count = row
                .try_get::<i64, _>("count")
                .ok()
                .and_then(|value| u64::try_from(value.max(0)).ok())
                .unwrap_or(0);
            Some((event_type, count))
        })
        .collect())
}

pub(crate) async fn list_user_audit_logs(
    pool: &PostgresPool,
    user_id: &str,
    cutoff_time: DateTime<Utc>,
    event_type: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<(Vec<Value>, usize), GatewayError> {
    let total = match sqlx::query_scalar::<_, i64>(
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
    .bind(event_type)
    .fetch_one(pool)
    .await
    {
        Ok(value) => usize::try_from(value.max(0)).unwrap_or(usize::MAX),
        Err(err) => {
            return Err(GatewayError::Internal(format!(
                "user audit logs count failed: {err}"
            )))
        }
    };

    let rows = match sqlx::query(
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
    .bind(event_type)
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .bind(i64::try_from(offset).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await
    {
        Ok(value) => value,
        Err(err) => {
            return Err(GatewayError::Internal(format!(
                "user audit logs read failed: {err}"
            )))
        }
    };

    Ok((
        rows.into_iter().map(user_audit_log_row_to_json).collect(),
        total,
    ))
}

fn admin_audit_log_row_to_json(row: sqlx::postgres::PgRow) -> Value {
    let created_at = row
        .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
        .ok()
        .map(|value| value.to_rfc3339());
    json!({
        "id": row.try_get::<String, _>("id").ok(),
        "event_type": row.try_get::<String, _>("event_type").ok(),
        "user_id": row.try_get::<Option<String>, _>("user_id").ok().flatten(),
        "user_email": row.try_get::<Option<String>, _>("user_email").ok().flatten(),
        "user_username": row.try_get::<Option<String>, _>("user_username").ok().flatten(),
        "description": row.try_get::<Option<String>, _>("description").ok().flatten(),
        "ip_address": row.try_get::<Option<String>, _>("ip_address").ok().flatten(),
        "status_code": row.try_get::<Option<i32>, _>("status_code").ok().flatten(),
        "error_message": row.try_get::<Option<String>, _>("error_message").ok().flatten(),
        "metadata": row.try_get::<Option<serde_json::Value>, _>("metadata").ok().flatten(),
        "created_at": created_at,
    })
}

fn admin_suspicious_row_to_json(row: sqlx::postgres::PgRow) -> Value {
    let created_at = row
        .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
        .ok()
        .map(|value| value.to_rfc3339());
    json!({
        "id": row.try_get::<String, _>("id").ok(),
        "event_type": row.try_get::<String, _>("event_type").ok(),
        "user_id": row.try_get::<Option<String>, _>("user_id").ok().flatten(),
        "description": row.try_get::<Option<String>, _>("description").ok().flatten(),
        "ip_address": row.try_get::<Option<String>, _>("ip_address").ok().flatten(),
        "metadata": row.try_get::<Option<serde_json::Value>, _>("metadata").ok().flatten(),
        "created_at": created_at,
    })
}

fn user_audit_log_row_to_json(row: sqlx::postgres::PgRow) -> Value {
    let created_at = row
        .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
        .ok()
        .map(|value| value.to_rfc3339());
    json!({
        "id": row.try_get::<String, _>("id").ok(),
        "event_type": row.try_get::<String, _>("event_type").ok(),
        "description": row.try_get::<String, _>("description").ok(),
        "ip_address": row.try_get::<Option<String>, _>("ip_address").ok().flatten(),
        "status_code": row.try_get::<Option<i32>, _>("status_code").ok().flatten(),
        "created_at": created_at,
    })
}
