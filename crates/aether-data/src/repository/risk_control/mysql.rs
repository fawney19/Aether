use aether_data_query::{push_eq, WhereClause};
use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use super::types::{
    InsertRiskControlLogRecord, InsertRiskControlNotificationOutboxRecord,
    RiskControlHashListQuery, RiskControlLogListQuery, RiskControlLogRetentionSummary,
    RiskControlNotificationOutboxSummary, RiskControlReadRepository, RiskControlWriteRepository,
    StoredRiskControlFlaggedHash, StoredRiskControlHashPage, StoredRiskControlLog,
    StoredRiskControlLogPage, StoredRiskControlNotificationOutbox,
    UpsertRiskControlFlaggedHashRecord,
};
use crate::driver::mysql::MysqlPool;
use crate::error::SqlResultExt;
use crate::DataLayerError;

#[derive(Debug, Clone)]
pub struct MysqlRiskControlRepository {
    pool: MysqlPool,
}

impl MysqlRiskControlRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn find_log_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRiskControlLog>, DataLayerError> {
        let row = sqlx::query(LOG_SELECT_SQL)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_log_row).transpose()
    }

    async fn find_hash_by_value(
        &self,
        input_hash: &str,
    ) -> Result<Option<StoredRiskControlFlaggedHash>, DataLayerError> {
        let row = sqlx::query(HASH_SELECT_SQL)
            .bind(input_hash)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_hash_row).transpose()
    }

    async fn find_notification_outbox_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRiskControlNotificationOutbox>, DataLayerError> {
        let row = sqlx::query(OUTBOX_SELECT_SQL)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_outbox_row).transpose()
    }

    async fn list_notification_outbox_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(OUTBOX_SELECT_BASE);
        builder.push(" WHERE id IN (");
        let mut separated = builder.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(")");
        builder.push(" ORDER BY next_attempt_at ASC, created_at ASC, id ASC");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_outbox_row).collect()
    }
}

const LOG_SELECT_BASE: &str = r#"
SELECT
  id, trace_id, request_id, user_id, username, user_email, api_key_id, api_key_name,
  route_family, route_kind, api_format, endpoint, model, mode, action, decision_source,
  flagged, highest_category, highest_score, category_scores, thresholds, matched_keywords,
  input_hash, excerpt, latency_ms, queue_delay_ms, violation_count, auto_action,
  notification_sent, notification_attempts, notification_last_error, notification_last_attempt_at,
  error_message, created_at
FROM risk_control_logs
"#;

const LOG_SELECT_SQL: &str = r#"
SELECT
  id, trace_id, request_id, user_id, username, user_email, api_key_id, api_key_name,
  route_family, route_kind, api_format, endpoint, model, mode, action, decision_source,
  flagged, highest_category, highest_score, category_scores, thresholds, matched_keywords,
  input_hash, excerpt, latency_ms, queue_delay_ms, violation_count, auto_action,
  notification_sent, notification_attempts, notification_last_error, notification_last_attempt_at,
  error_message, created_at
FROM risk_control_logs
WHERE id = ?
LIMIT 1
"#;

const HASH_SELECT_BASE: &str = r#"
SELECT
  input_hash, source_log_id, reason, highest_category, highest_score, excerpt,
  first_seen_at, last_seen_at, hit_count
FROM risk_control_flagged_hashes
"#;

const HASH_SELECT_SQL: &str = r#"
SELECT
  input_hash, source_log_id, reason, highest_category, highest_score, excerpt,
  first_seen_at, last_seen_at, hit_count
FROM risk_control_flagged_hashes
WHERE input_hash = ?
LIMIT 1
"#;

const OUTBOX_SELECT_BASE: &str = r#"
SELECT
  id, log_id, item_key, title, markdown_body, text_body, variables_json, status,
  attempt_count, max_attempts, next_attempt_at, lease_until, last_error,
  created_at, updated_at, sent_at
FROM risk_control_notification_outbox
"#;

const OUTBOX_SELECT_SQL: &str = r#"
SELECT
  id, log_id, item_key, title, markdown_body, text_body, variables_json, status,
  attempt_count, max_attempts, next_attempt_at, lease_until, last_error,
  created_at, updated_at, sent_at
FROM risk_control_notification_outbox
WHERE id = ?
LIMIT 1
"#;

#[async_trait]
impl RiskControlReadRepository for MysqlRiskControlRepository {
    async fn list_logs(
        &self,
        query: &RiskControlLogListQuery,
    ) -> Result<StoredRiskControlLogPage, DataLayerError> {
        let mut count_builder =
            QueryBuilder::<MySql>::new("SELECT COUNT(id) AS total FROM risk_control_logs");
        apply_log_filters(&mut count_builder, query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?
            .max(0) as u64;

        let mut list_builder = QueryBuilder::<MySql>::new(LOG_SELECT_BASE);
        apply_log_filters(&mut list_builder, query);
        list_builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
        list_builder.push_bind(query.limit.clamp(1, 500) as i64);
        list_builder.push(" OFFSET ");
        list_builder.push_bind(query.offset as i64);
        let rows = list_builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_log_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredRiskControlLogPage { items, total })
    }

    async fn count_flagged_logs_since(
        &self,
        user_id: &str,
        since_unix_secs: u64,
    ) -> Result<u64, DataLayerError> {
        let total = sqlx::query_scalar::<_, i64>(
            r#"
SELECT COUNT(id)
FROM risk_control_logs
WHERE flagged = 1 AND user_id = ? AND created_at >= ?
"#,
        )
        .bind(user_id)
        .bind(since_unix_secs as i64)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        Ok(total.max(0) as u64)
    }

    async fn get_flagged_hash(
        &self,
        input_hash: &str,
    ) -> Result<Option<StoredRiskControlFlaggedHash>, DataLayerError> {
        self.find_hash_by_value(input_hash).await
    }

    async fn list_flagged_hashes(
        &self,
        query: &RiskControlHashListQuery,
    ) -> Result<StoredRiskControlHashPage, DataLayerError> {
        let total = self.count_flagged_hashes().await?;
        let mut builder = QueryBuilder::<MySql>::new(HASH_SELECT_BASE);
        builder.push(" ORDER BY last_seen_at DESC, hit_count DESC, input_hash ASC LIMIT ");
        builder.push_bind(query.limit.clamp(1, 500) as i64);
        builder.push(" OFFSET ");
        builder.push_bind(query.offset as i64);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let items = rows
            .iter()
            .map(map_hash_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredRiskControlHashPage { items, total })
    }

    async fn count_flagged_hashes(&self) -> Result<u64, DataLayerError> {
        let total = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(input_hash) FROM risk_control_flagged_hashes",
        )
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        Ok(total.max(0) as u64)
    }

    async fn list_notification_outbox_by_log_ids(
        &self,
        log_ids: &[String],
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        if log_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(OUTBOX_SELECT_BASE);
        builder.push(" WHERE log_id IN (");
        let mut separated = builder.separated(", ");
        for log_id in log_ids {
            separated.push_bind(log_id);
        }
        separated.push_unseparated(")");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_outbox_row).collect()
    }

    async fn notification_outbox_summary(
        &self,
    ) -> Result<RiskControlNotificationOutboxSummary, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  COUNT(CASE WHEN status = 'pending' THEN 1 END) AS pending,
  COUNT(CASE WHEN status = 'processing' THEN 1 END) AS processing,
  COUNT(CASE WHEN status = 'sent' THEN 1 END) AS sent,
  COUNT(CASE WHEN status = 'dead' THEN 1 END) AS dead,
  MIN(CASE WHEN status = 'pending' THEN created_at END) AS oldest_pending_at,
  MIN(CASE WHEN status = 'pending' THEN next_attempt_at END) AS next_attempt_at
FROM risk_control_notification_outbox
"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        let last_error = sqlx::query_scalar::<_, Option<String>>(
            r#"
SELECT last_error
FROM risk_control_notification_outbox
WHERE status <> 'sent' AND last_error IS NOT NULL
ORDER BY updated_at DESC, id DESC
LIMIT 1
"#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?
        .flatten();
        Ok(RiskControlNotificationOutboxSummary {
            pending: u64_from_i64(row.try_get("pending").map_sql_err()?),
            processing: u64_from_i64(row.try_get("processing").map_sql_err()?),
            sent: u64_from_i64(row.try_get("sent").map_sql_err()?),
            dead: u64_from_i64(row.try_get("dead").map_sql_err()?),
            oldest_pending_at_unix_secs: optional_u64_from_i64(
                row.try_get("oldest_pending_at").map_sql_err()?,
            ),
            next_attempt_at_unix_secs: optional_u64_from_i64(
                row.try_get("next_attempt_at").map_sql_err()?,
            ),
            last_error,
        })
    }
}

#[async_trait]
impl RiskControlWriteRepository for MysqlRiskControlRepository {
    async fn insert_log(
        &self,
        record: InsertRiskControlLogRecord,
    ) -> Result<StoredRiskControlLog, DataLayerError> {
        record.validate()?;
        let id = record
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = record
            .created_at_unix_secs
            .unwrap_or_else(current_unix_secs);
        sqlx::query(
            r#"
INSERT INTO risk_control_logs (
  id, trace_id, request_id, user_id, username, user_email, api_key_id, api_key_name,
  route_family, route_kind, api_format, endpoint, model, mode, action, decision_source,
  flagged, highest_category, highest_score, category_scores, thresholds, matched_keywords,
  input_hash, excerpt, latency_ms, queue_delay_ms, violation_count, auto_action,
  notification_sent, notification_attempts, notification_last_error, notification_last_attempt_at,
  error_message, created_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&id)
        .bind(record.trace_id)
        .bind(record.request_id)
        .bind(record.user_id)
        .bind(record.username)
        .bind(record.user_email)
        .bind(record.api_key_id)
        .bind(record.api_key_name)
        .bind(record.route_family)
        .bind(record.route_kind)
        .bind(record.api_format)
        .bind(record.endpoint)
        .bind(record.model)
        .bind(record.mode)
        .bind(record.action)
        .bind(record.decision_source)
        .bind(record.flagged)
        .bind(record.highest_category)
        .bind(record.highest_score)
        .bind(serialize_optional_json(record.category_scores)?)
        .bind(serialize_optional_json(record.thresholds)?)
        .bind(serialize_optional_json(record.matched_keywords)?)
        .bind(record.input_hash)
        .bind(record.excerpt)
        .bind(record.latency_ms.map(|value| value as i64))
        .bind(record.queue_delay_ms.map(|value| value as i64))
        .bind(record.violation_count as i64)
        .bind(record.auto_action)
        .bind(record.notification_sent)
        .bind(record.notification_attempts as i64)
        .bind(record.notification_last_error)
        .bind(record.notification_last_attempt_at_unix_secs.map(|value| value as i64))
        .bind(record.error_message)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.find_log_by_id(&id).await?.ok_or_else(|| {
            DataLayerError::UnexpectedValue("created risk control log missing".into())
        })
    }

    async fn mark_log_notification_sent(&self, id: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            "UPDATE risk_control_logs SET notification_sent = 1, notification_last_error = NULL WHERE id = ?",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn record_log_notification_result(
        &self,
        id: &str,
        sent: bool,
        error_message: Option<String>,
        attempted_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE risk_control_logs
SET
  notification_sent = CASE WHEN ? THEN 1 ELSE notification_sent END,
  notification_attempts = notification_attempts + 1,
  notification_last_error = ?,
  notification_last_attempt_at = ?
WHERE id = ?
"#,
        )
        .bind(sent)
        .bind(error_message)
        .bind(attempted_at_unix_secs as i64)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn upsert_flagged_hash(
        &self,
        record: UpsertRiskControlFlaggedHashRecord,
    ) -> Result<StoredRiskControlFlaggedHash, DataLayerError> {
        record.validate()?;
        let now = record.now_unix_secs.unwrap_or_else(current_unix_secs);
        sqlx::query(
            r#"
INSERT INTO risk_control_flagged_hashes (
  input_hash, source_log_id, reason, highest_category, highest_score, excerpt,
  first_seen_at, last_seen_at, hit_count
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1)
ON DUPLICATE KEY UPDATE
  source_log_id = COALESCE(VALUES(source_log_id), source_log_id),
  reason = COALESCE(VALUES(reason), reason),
  highest_category = COALESCE(VALUES(highest_category), highest_category),
  highest_score = GREATEST(VALUES(highest_score), highest_score),
  excerpt = COALESCE(VALUES(excerpt), excerpt),
  last_seen_at = VALUES(last_seen_at),
  hit_count = hit_count + 1
"#,
        )
        .bind(&record.input_hash)
        .bind(record.source_log_id)
        .bind(record.reason)
        .bind(record.highest_category)
        .bind(record.highest_score)
        .bind(record.excerpt)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.find_hash_by_value(&record.input_hash)
            .await?
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue("upserted risk control hash missing".into())
            })
    }

    async fn delete_flagged_hash(&self, input_hash: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query("DELETE FROM risk_control_flagged_hashes WHERE input_hash = ?")
            .bind(input_hash)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn clear_flagged_hashes(&self) -> Result<u64, DataLayerError> {
        let result = sqlx::query("DELETE FROM risk_control_flagged_hashes")
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        Ok(result.rows_affected())
    }

    async fn delete_expired_logs(
        &self,
        hit_before_unix_secs: Option<u64>,
        non_hit_before_unix_secs: Option<u64>,
    ) -> Result<RiskControlLogRetentionSummary, DataLayerError> {
        let hit_deleted = match hit_before_unix_secs {
            Some(cutoff) => {
                sqlx::query("DELETE FROM risk_control_logs WHERE flagged = 1 AND created_at < ?")
                    .bind(cutoff as i64)
                    .execute(&self.pool)
                    .await
                    .map_sql_err()?
                    .rows_affected()
            }
            None => 0,
        };
        let non_hit_deleted = match non_hit_before_unix_secs {
            Some(cutoff) => {
                sqlx::query("DELETE FROM risk_control_logs WHERE flagged = 0 AND created_at < ?")
                    .bind(cutoff as i64)
                    .execute(&self.pool)
                    .await
                    .map_sql_err()?
                    .rows_affected()
            }
            None => 0,
        };
        Ok(RiskControlLogRetentionSummary {
            hit_deleted,
            non_hit_deleted,
        })
    }

    async fn insert_notification_outbox(
        &self,
        record: InsertRiskControlNotificationOutboxRecord,
    ) -> Result<StoredRiskControlNotificationOutbox, DataLayerError> {
        record.validate()?;
        let id = record
            .id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = record
            .created_at_unix_secs
            .unwrap_or_else(current_unix_secs);
        let next_attempt_at = record.next_attempt_at_unix_secs.unwrap_or(now);
        sqlx::query(
            r#"
INSERT INTO risk_control_notification_outbox (
  id, log_id, item_key, title, markdown_body, text_body, variables_json, status,
  attempt_count, max_attempts, next_attempt_at, lease_until, last_error, created_at, updated_at, sent_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', 0, ?, ?, NULL, NULL, ?, ?, NULL)
ON DUPLICATE KEY UPDATE
  id = VALUES(id),
  item_key = VALUES(item_key),
  title = VALUES(title),
  markdown_body = VALUES(markdown_body),
  text_body = VALUES(text_body),
  variables_json = VALUES(variables_json),
  status = 'pending',
  attempt_count = 0,
  max_attempts = VALUES(max_attempts),
  next_attempt_at = VALUES(next_attempt_at),
  lease_until = NULL,
  last_error = NULL,
  updated_at = VALUES(updated_at),
  sent_at = NULL
"#,
        )
        .bind(&id)
        .bind(record.log_id)
        .bind(record.item_key)
        .bind(record.title)
        .bind(record.markdown_body)
        .bind(record.text_body)
        .bind(serialize_json(record.variables_json)?)
        .bind(record.max_attempts as i64)
        .bind(next_attempt_at as i64)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.find_notification_outbox_by_id(&id)
            .await?
            .ok_or_else(|| DataLayerError::UnexpectedValue("created outbox row missing".into()))
    }

    async fn claim_due_notification_outbox(
        &self,
        now_unix_secs: u64,
        lease_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        let limit = limit.clamp(1, 500) as i64;
        let now = now_unix_secs as i64;
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let due_ids = sqlx::query_scalar::<_, String>(
            r#"
SELECT id
FROM risk_control_notification_outbox
WHERE (status = 'pending' AND next_attempt_at <= ?)
   OR (status = 'processing' AND lease_until <= ?)
ORDER BY COALESCE(next_attempt_at, created_at) ASC, created_at ASC, id ASC
LIMIT ?
FOR UPDATE SKIP LOCKED
"#,
        )
        .bind(now)
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_sql_err()?;
        let mut claimed_ids = Vec::new();
        for id in due_ids {
            let result = sqlx::query(
                r#"
UPDATE risk_control_notification_outbox
SET status = 'processing', lease_until = ?, updated_at = ?
WHERE id = ?
  AND ((status = 'pending' AND next_attempt_at <= ?)
    OR (status = 'processing' AND lease_until <= ?))
"#,
            )
            .bind(lease_until_unix_secs as i64)
            .bind(now)
            .bind(&id)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            if result.rows_affected() > 0 {
                claimed_ids.push(id);
            }
        }
        tx.commit().await.map_sql_err()?;
        self.list_notification_outbox_by_ids(&claimed_ids).await
    }

    async fn record_notification_outbox_attempt(
        &self,
        id: &str,
        sent: bool,
        error_message: Option<String>,
        attempted_at_unix_secs: u64,
        next_attempt_at_unix_secs: Option<u64>,
    ) -> Result<bool, DataLayerError> {
        let (status, next_attempt_at, last_error, sent_at) = if sent {
            ("sent", None, None, Some(attempted_at_unix_secs as i64))
        } else if let Some(next) = next_attempt_at_unix_secs {
            ("pending", Some(next as i64), error_message, None)
        } else {
            ("dead", None, error_message, None)
        };
        let result = sqlx::query(
            r#"
UPDATE risk_control_notification_outbox
SET
  status = ?,
  attempt_count = attempt_count + 1,
  next_attempt_at = ?,
  lease_until = NULL,
  last_error = ?,
  updated_at = ?,
  sent_at = ?
WHERE id = ?
"#,
        )
        .bind(status)
        .bind(next_attempt_at)
        .bind(last_error)
        .bind(attempted_at_unix_secs as i64)
        .bind(sent_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn defer_notification_outbox_attempt(
        &self,
        id: &str,
        error_message: Option<String>,
        deferred_at_unix_secs: u64,
        next_attempt_at_unix_secs: u64,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE risk_control_notification_outbox
SET
  status = 'pending',
  next_attempt_at = ?,
  lease_until = NULL,
  last_error = ?,
  updated_at = ?,
  sent_at = NULL
WHERE id = ?
"#,
        )
        .bind(next_attempt_at_unix_secs as i64)
        .bind(error_message)
        .bind(deferred_at_unix_secs as i64)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn reset_notification_outbox_for_retry(
        &self,
        log_id: &str,
        next_attempt_at_unix_secs: u64,
    ) -> Result<Vec<StoredRiskControlNotificationOutbox>, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE risk_control_notification_outbox
SET
  status = 'pending',
  attempt_count = 0,
  next_attempt_at = ?,
  lease_until = NULL,
  last_error = NULL,
  updated_at = ?,
  sent_at = NULL
WHERE log_id = ? AND status <> 'sent'
"#,
        )
        .bind(next_attempt_at_unix_secs as i64)
        .bind(next_attempt_at_unix_secs as i64)
        .bind(log_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(Vec::new());
        }
        self.list_notification_outbox_by_log_ids(&[log_id.to_string()])
            .await
    }
}

fn apply_log_filters(builder: &mut QueryBuilder<'_, MySql>, query: &RiskControlLogListQuery) {
    let mut where_clause = WhereClause::new();

    if let Some(value) = query.user_id.as_deref() {
        push_eq(builder, &mut where_clause, "user_id", value.to_string());
    }
    if let Some(value) = query.api_key_id.as_deref() {
        push_eq(builder, &mut where_clause, "api_key_id", value.to_string());
    }
    if let Some(value) = query.flagged {
        push_eq(builder, &mut where_clause, "flagged", value);
    }
    if let Some(value) = query.action.as_deref() {
        push_eq(builder, &mut where_clause, "action", value.to_string());
    }
    if let Some(value) = query.decision_source.as_deref() {
        push_eq(
            builder,
            &mut where_clause,
            "decision_source",
            value.to_string(),
        );
    }
    if let Some(value) = query.endpoint.as_deref() {
        push_eq(builder, &mut where_clause, "endpoint", value.to_string());
    }
    if let Some(value) = query.model.as_deref() {
        push_eq(builder, &mut where_clause, "model", value.to_string());
    }
    if let Some(value) = query.q.as_deref() {
        push_log_search_filter(builder, &mut where_clause, value);
    }
    if let Some(value) = query.from_unix_secs {
        where_clause.push_next(builder);
        builder.push("created_at >= ");
        builder.push_bind(value as i64);
    }
    if let Some(value) = query.to_unix_secs {
        where_clause.push_next(builder);
        builder.push("created_at <= ");
        builder.push_bind(value as i64);
    }
}

const LOG_SEARCH_COLUMNS: &[&str] = &[
    "username",
    "user_email",
    "user_id",
    "api_key_name",
    "api_key_id",
    "route_family",
    "route_kind",
    "api_format",
    "endpoint",
    "model",
    "action",
    "decision_source",
    "highest_category",
    "input_hash",
    "excerpt",
    "error_message",
    "notification_last_error",
];

fn push_log_search_filter(
    builder: &mut QueryBuilder<'_, MySql>,
    where_clause: &mut WhereClause,
    raw: &str,
) {
    let needle = raw.trim().to_lowercase();
    if needle.is_empty() {
        return;
    }

    let pattern = format!("%{needle}%");
    where_clause.push_next(builder);
    builder.push("(");
    for (index, column) in LOG_SEARCH_COLUMNS.iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder.push("LOWER(COALESCE(");
        builder.push(*column);
        builder.push(", '')) LIKE ");
        builder.push_bind(pattern.clone());
    }
    builder.push(")");
}

fn map_log_row(row: &MySqlRow) -> Result<StoredRiskControlLog, DataLayerError> {
    Ok(StoredRiskControlLog {
        id: row.try_get("id").map_sql_err()?,
        trace_id: row.try_get("trace_id").map_sql_err()?,
        request_id: row.try_get("request_id").map_sql_err()?,
        user_id: row.try_get("user_id").map_sql_err()?,
        username: row.try_get("username").map_sql_err()?,
        user_email: row.try_get("user_email").map_sql_err()?,
        api_key_id: row.try_get("api_key_id").map_sql_err()?,
        api_key_name: row.try_get("api_key_name").map_sql_err()?,
        route_family: row.try_get("route_family").map_sql_err()?,
        route_kind: row.try_get("route_kind").map_sql_err()?,
        api_format: row.try_get("api_format").map_sql_err()?,
        endpoint: row.try_get("endpoint").map_sql_err()?,
        model: row.try_get("model").map_sql_err()?,
        mode: row.try_get("mode").map_sql_err()?,
        action: row.try_get("action").map_sql_err()?,
        decision_source: row.try_get("decision_source").map_sql_err()?,
        flagged: row.try_get("flagged").map_sql_err()?,
        highest_category: row.try_get("highest_category").map_sql_err()?,
        highest_score: row.try_get("highest_score").map_sql_err()?,
        category_scores: parse_optional_json(row.try_get("category_scores").map_sql_err()?)?,
        thresholds: parse_optional_json(row.try_get("thresholds").map_sql_err()?)?,
        matched_keywords: parse_optional_json(row.try_get("matched_keywords").map_sql_err()?)?,
        input_hash: row.try_get("input_hash").map_sql_err()?,
        excerpt: row.try_get("excerpt").map_sql_err()?,
        latency_ms: optional_u64_from_i64(row.try_get("latency_ms").map_sql_err()?),
        queue_delay_ms: optional_u64_from_i64(row.try_get("queue_delay_ms").map_sql_err()?),
        violation_count: u64_from_i64(row.try_get("violation_count").map_sql_err()?),
        auto_action: row.try_get("auto_action").map_sql_err()?,
        notification_sent: row.try_get("notification_sent").map_sql_err()?,
        notification_attempts: u64_from_i64(row.try_get("notification_attempts").map_sql_err()?),
        notification_last_error: row.try_get("notification_last_error").map_sql_err()?,
        notification_last_attempt_at_unix_secs: optional_u64_from_i64(
            row.try_get("notification_last_attempt_at").map_sql_err()?,
        ),
        error_message: row.try_get("error_message").map_sql_err()?,
        created_at_unix_secs: u64_from_i64(row.try_get("created_at").map_sql_err()?),
    })
}

fn map_hash_row(row: &MySqlRow) -> Result<StoredRiskControlFlaggedHash, DataLayerError> {
    Ok(StoredRiskControlFlaggedHash {
        input_hash: row.try_get("input_hash").map_sql_err()?,
        source_log_id: row.try_get("source_log_id").map_sql_err()?,
        reason: row.try_get("reason").map_sql_err()?,
        highest_category: row.try_get("highest_category").map_sql_err()?,
        highest_score: row.try_get("highest_score").map_sql_err()?,
        excerpt: row.try_get("excerpt").map_sql_err()?,
        first_seen_at_unix_secs: u64_from_i64(row.try_get("first_seen_at").map_sql_err()?),
        last_seen_at_unix_secs: u64_from_i64(row.try_get("last_seen_at").map_sql_err()?),
        hit_count: u64_from_i64(row.try_get("hit_count").map_sql_err()?),
    })
}

fn map_outbox_row(row: &MySqlRow) -> Result<StoredRiskControlNotificationOutbox, DataLayerError> {
    Ok(StoredRiskControlNotificationOutbox {
        id: row.try_get("id").map_sql_err()?,
        log_id: row.try_get("log_id").map_sql_err()?,
        item_key: row.try_get("item_key").map_sql_err()?,
        title: row.try_get("title").map_sql_err()?,
        markdown_body: row.try_get("markdown_body").map_sql_err()?,
        text_body: row.try_get("text_body").map_sql_err()?,
        variables_json: parse_json(row.try_get("variables_json").map_sql_err()?)?,
        status: row.try_get("status").map_sql_err()?,
        attempt_count: u64_from_i64(row.try_get("attempt_count").map_sql_err()?),
        max_attempts: u64_from_i64(row.try_get("max_attempts").map_sql_err()?),
        next_attempt_at_unix_secs: optional_u64_from_i64(
            row.try_get("next_attempt_at").map_sql_err()?,
        ),
        lease_until_unix_secs: optional_u64_from_i64(row.try_get("lease_until").map_sql_err()?),
        last_error: row.try_get("last_error").map_sql_err()?,
        created_at_unix_secs: u64_from_i64(row.try_get("created_at").map_sql_err()?),
        updated_at_unix_secs: u64_from_i64(row.try_get("updated_at").map_sql_err()?),
        sent_at_unix_secs: optional_u64_from_i64(row.try_get("sent_at").map_sql_err()?),
    })
}

fn parse_optional_json(value: Option<String>) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .map(|raw| {
            serde_json::from_str(&raw)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))
        })
        .transpose()
}

fn parse_json(value: String) -> Result<serde_json::Value, DataLayerError> {
    serde_json::from_str(&value).map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))
}

fn serialize_json(value: serde_json::Value) -> Result<String, DataLayerError> {
    serde_json::to_string(&value).map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))
}

fn serialize_optional_json(
    value: Option<serde_json::Value>,
) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| {
            serde_json::to_string(&value)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))
        })
        .transpose()
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn optional_u64_from_i64(value: Option<i64>) -> Option<u64> {
    value.map(u64_from_i64)
}

fn u64_from_i64(value: i64) -> u64 {
    value.max(0) as u64
}
