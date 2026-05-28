use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite, SqlitePool};

use super::types::{
    ClaimWebhookDeliveriesRequest, CreateWebhookDeliveryRecord, RecordWebhookDeliveryAttemptRecord,
    RescheduleWebhookDeliveryRecord, StoredWebhookDelivery, StoredWebhookDeliveryAttempt,
    StoredWebhookDeliveryPage, StoredWebhookEndpoint, UpdateWebhookEndpointTestResultRecord,
    UpsertWebhookEndpointRecord, WebhookDeliveryAttemptListQuery, WebhookDeliveryListQuery,
    WebhookEndpointListQuery, WebhookNotificationReadRepository,
    WebhookNotificationWriteRepository, WebhookSecretUpdate,
};
use crate::error::SqlResultExt;
use crate::DataLayerError;

const ENDPOINT_COLUMNS: &str = r#"
SELECT
  id,
  name,
  url,
  secret_encrypted,
  enabled,
  event_filter_json,
  timeout_ms,
  max_attempts,
  initial_retry_delay_seconds,
  max_retry_delay_seconds,
  last_test_delivery_id,
  last_test_status,
  last_test_error,
  last_test_at_unix_secs,
  created_by,
  created_at_unix_secs,
  updated_at_unix_secs
FROM webhook_endpoints
"#;

const DELIVERY_COLUMNS: &str = r#"
SELECT
  id,
  endpoint_id,
  event_id,
  event_type,
  is_test,
  payload_json,
  status,
  attempt_count,
  max_attempts,
  next_attempt_at_unix_secs,
  last_attempt_at_unix_secs,
  delivered_at_unix_secs,
  failed_at_unix_secs,
  last_http_status,
  last_error,
  response_excerpt,
  lease_owner,
  lease_expires_at_unix_secs,
  created_at_unix_secs,
  updated_at_unix_secs
FROM webhook_deliveries
"#;

const ATTEMPT_COLUMNS: &str = r#"
SELECT
  id,
  delivery_id,
  attempt_index,
  status,
  http_status,
  error_message,
  response_excerpt,
  started_at_unix_secs,
  completed_at_unix_secs,
  duration_ms
FROM webhook_delivery_attempts
"#;

#[derive(Debug, Clone)]
pub struct SqliteWebhookNotificationRepository {
    pool: SqlitePool,
}

impl SqliteWebhookNotificationRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl WebhookNotificationReadRepository for SqliteWebhookNotificationRepository {
    async fn list_webhook_endpoints(
        &self,
        query: &WebhookEndpointListQuery,
    ) -> Result<Vec<StoredWebhookEndpoint>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(ENDPOINT_COLUMNS);
        let mut has_where = false;
        if let Some(enabled) = query.enabled {
            push_where(&mut builder, &mut has_where);
            builder.push("enabled = ").push_bind(enabled);
        }
        builder.push(" ORDER BY updated_at_unix_secs DESC, created_at_unix_secs DESC");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let mut endpoints = rows
            .iter()
            .map(map_endpoint_row)
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(event_type) = query.event_type.as_deref() {
            endpoints.retain(|endpoint| endpoint.subscribes_to(event_type));
        }
        Ok(endpoints)
    }

    async fn find_webhook_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<Option<StoredWebhookEndpoint>, DataLayerError> {
        let row = sqlx::query(&format!("{ENDPOINT_COLUMNS} WHERE id = ? LIMIT 1"))
            .bind(endpoint_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_endpoint_row).transpose()
    }

    async fn find_webhook_delivery(
        &self,
        delivery_id: &str,
    ) -> Result<Option<StoredWebhookDelivery>, DataLayerError> {
        let row = sqlx::query(&format!("{DELIVERY_COLUMNS} WHERE id = ? LIMIT 1"))
            .bind(delivery_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_delivery_row).transpose()
    }

    async fn list_webhook_deliveries(
        &self,
        query: &WebhookDeliveryListQuery,
    ) -> Result<StoredWebhookDeliveryPage, DataLayerError> {
        let mut count_builder =
            QueryBuilder::<Sqlite>::new("SELECT COUNT(id) AS total FROM webhook_deliveries");
        apply_delivery_filter(&mut count_builder, query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?;

        let mut builder = QueryBuilder::<Sqlite>::new(DELIVERY_COLUMNS);
        apply_delivery_filter(&mut builder, query);
        builder.push(" ORDER BY created_at_unix_secs DESC, updated_at_unix_secs DESC");
        builder
            .push(" LIMIT ")
            .push_bind(i64_from_usize(
                query.limit.max(1),
                "webhook delivery limit",
            )?)
            .push(" OFFSET ")
            .push_bind(i64_from_usize(query.offset, "webhook delivery offset")?);
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        Ok(StoredWebhookDeliveryPage {
            items: rows
                .iter()
                .map(map_delivery_row)
                .collect::<Result<Vec<_>, _>>()?,
            total: u64::try_from(total).unwrap_or_default(),
        })
    }

    async fn list_webhook_delivery_attempts(
        &self,
        query: &WebhookDeliveryAttemptListQuery,
    ) -> Result<Vec<StoredWebhookDeliveryAttempt>, DataLayerError> {
        let rows = sqlx::query(&format!(
            "{ATTEMPT_COLUMNS} WHERE delivery_id = ? ORDER BY attempt_index ASC, started_at_unix_secs ASC LIMIT ? OFFSET ?"
        ))
        .bind(&query.delivery_id)
        .bind(i64_from_usize(
            query.limit.max(1),
            "webhook delivery attempt limit",
        )?)
        .bind(i64_from_usize(
            query.offset,
            "webhook delivery attempt offset",
        )?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_attempt_row).collect()
    }
}

#[async_trait]
impl WebhookNotificationWriteRepository for SqliteWebhookNotificationRepository {
    async fn upsert_webhook_endpoint(
        &self,
        record: &UpsertWebhookEndpointRecord,
    ) -> Result<StoredWebhookEndpoint, DataLayerError> {
        record.validate()?;
        let id = record.effective_id();
        let now = now_unix_secs_i64()?;
        let event_filter_json = json_to_string(record.event_filter_json.as_ref())?;
        match &record.secret_encrypted {
            WebhookSecretUpdate::Preserve => {
                sqlx::query(
                    r#"
INSERT INTO webhook_endpoints (
  id, name, url, enabled, event_filter_json, timeout_ms, max_attempts,
  initial_retry_delay_seconds, max_retry_delay_seconds, created_by,
  created_at_unix_secs, updated_at_unix_secs
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
  name = excluded.name,
  url = excluded.url,
  enabled = excluded.enabled,
  event_filter_json = excluded.event_filter_json,
  timeout_ms = excluded.timeout_ms,
  max_attempts = excluded.max_attempts,
  initial_retry_delay_seconds = excluded.initial_retry_delay_seconds,
  max_retry_delay_seconds = excluded.max_retry_delay_seconds,
  updated_at_unix_secs = excluded.updated_at_unix_secs
"#,
                )
                .bind(&id)
                .bind(&record.name)
                .bind(&record.url)
                .bind(record.enabled)
                .bind(event_filter_json)
                .bind(u32_to_i32(record.timeout_ms, "timeout_ms")?)
                .bind(u32_to_i32(record.max_attempts, "max_attempts")?)
                .bind(u32_to_i32(
                    record.initial_retry_delay_seconds,
                    "initial_retry_delay_seconds",
                )?)
                .bind(u32_to_i32(
                    record.max_retry_delay_seconds,
                    "max_retry_delay_seconds",
                )?)
                .bind(record.created_by.as_deref())
                .bind(now)
                .bind(now)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
            }
            WebhookSecretUpdate::Clear | WebhookSecretUpdate::Set(_) => {
                sqlx::query(
                    r#"
INSERT INTO webhook_endpoints (
  id, name, url, secret_encrypted, enabled, event_filter_json, timeout_ms, max_attempts,
  initial_retry_delay_seconds, max_retry_delay_seconds, created_by,
  created_at_unix_secs, updated_at_unix_secs
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
  name = excluded.name,
  url = excluded.url,
  secret_encrypted = excluded.secret_encrypted,
  enabled = excluded.enabled,
  event_filter_json = excluded.event_filter_json,
  timeout_ms = excluded.timeout_ms,
  max_attempts = excluded.max_attempts,
  initial_retry_delay_seconds = excluded.initial_retry_delay_seconds,
  max_retry_delay_seconds = excluded.max_retry_delay_seconds,
  updated_at_unix_secs = excluded.updated_at_unix_secs
"#,
                )
                .bind(&id)
                .bind(&record.name)
                .bind(&record.url)
                .bind(record.secret_encrypted.value())
                .bind(record.enabled)
                .bind(event_filter_json)
                .bind(u32_to_i32(record.timeout_ms, "timeout_ms")?)
                .bind(u32_to_i32(record.max_attempts, "max_attempts")?)
                .bind(u32_to_i32(
                    record.initial_retry_delay_seconds,
                    "initial_retry_delay_seconds",
                )?)
                .bind(u32_to_i32(
                    record.max_retry_delay_seconds,
                    "max_retry_delay_seconds",
                )?)
                .bind(record.created_by.as_deref())
                .bind(now)
                .bind(now)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
            }
        }
        self.find_webhook_endpoint(&id).await?.ok_or_else(|| {
            DataLayerError::UnexpectedValue("webhook endpoint missing after upsert".to_string())
        })
    }

    async fn delete_webhook_endpoint(&self, endpoint_id: &str) -> Result<bool, DataLayerError> {
        let affected = sqlx::query("DELETE FROM webhook_endpoints WHERE id = ?")
            .bind(endpoint_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(affected > 0)
    }

    async fn create_webhook_delivery(
        &self,
        record: &CreateWebhookDeliveryRecord,
    ) -> Result<StoredWebhookDelivery, DataLayerError> {
        record.validate()?;
        let id = record.effective_id();
        let now = now_unix_secs_i64()?;
        sqlx::query(
            r#"
INSERT INTO webhook_deliveries (
  id, endpoint_id, event_id, event_type, is_test, payload_json, status,
  attempt_count, max_attempts, next_attempt_at_unix_secs, created_at_unix_secs,
  updated_at_unix_secs
) VALUES (?, ?, ?, ?, ?, ?, 'pending', 0, ?, ?, ?, ?)
ON CONFLICT(endpoint_id, event_id) DO NOTHING
"#,
        )
        .bind(&id)
        .bind(&record.endpoint_id)
        .bind(&record.event_id)
        .bind(&record.event_type)
        .bind(record.is_test)
        .bind(required_json_to_string(
            &record.payload_json,
            "payload_json",
        )?)
        .bind(u32_to_i32(record.max_attempts, "max_attempts")?)
        .bind(u64_to_i64(
            record.next_attempt_at_unix_secs,
            "next_attempt_at_unix_secs",
        )?)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        let row = sqlx::query(&format!(
            "{DELIVERY_COLUMNS} WHERE endpoint_id = ? AND event_id = ? LIMIT 1"
        ))
        .bind(&record.endpoint_id)
        .bind(&record.event_id)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        map_delivery_row(&row)
    }

    async fn claim_due_webhook_deliveries(
        &self,
        request: &ClaimWebhookDeliveriesRequest,
    ) -> Result<Vec<StoredWebhookDelivery>, DataLayerError> {
        request.validate()?;
        let now = u64_to_i64(request.now_unix_secs, "now_unix_secs")?;
        let lease_expires_at = u64_to_i64(
            request.lease_expires_at_unix_secs(),
            "lease_expires_at_unix_secs",
        )?;
        let limit = i64_from_usize(request.limit, "webhook delivery claim limit")?;
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let rows = sqlx::query(
            r#"
SELECT id
FROM webhook_deliveries
WHERE status IN ('pending', 'retrying')
  AND next_attempt_at_unix_secs <= ?
  AND (lease_owner IS NULL OR lease_expires_at_unix_secs IS NULL OR lease_expires_at_unix_secs <= ?)
ORDER BY next_attempt_at_unix_secs ASC, created_at_unix_secs ASC
LIMIT ?
"#,
        )
        .bind(now)
        .bind(now)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_sql_err()?;

        let mut claimed = Vec::new();
        for row in rows {
            let id: String = row.try_get("id").map_sql_err()?;
            let affected = sqlx::query(
                r#"
UPDATE webhook_deliveries
SET status = 'delivering',
    lease_owner = ?,
    lease_expires_at_unix_secs = ?,
    updated_at_unix_secs = ?
WHERE id = ?
  AND status IN ('pending', 'retrying')
  AND next_attempt_at_unix_secs <= ?
  AND (lease_owner IS NULL OR lease_expires_at_unix_secs IS NULL OR lease_expires_at_unix_secs <= ?)
"#,
            )
            .bind(&request.owner)
            .bind(lease_expires_at)
            .bind(now)
            .bind(&id)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?
            .rows_affected();

            if affected > 0 {
                let row = sqlx::query(&format!("{DELIVERY_COLUMNS} WHERE id = ? LIMIT 1"))
                    .bind(&id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_sql_err()?;
                claimed.push(map_delivery_row(&row)?);
            }
        }
        tx.commit().await.map_sql_err()?;
        Ok(claimed)
    }

    async fn record_webhook_delivery_attempt(
        &self,
        record: &RecordWebhookDeliveryAttemptRecord,
    ) -> Result<StoredWebhookDeliveryAttempt, DataLayerError> {
        record.validate()?;
        let id = record.effective_id();
        let completed_or_started =
            u64_to_i64(record.completed_or_started_at(), "completed_at_unix_secs")?;
        let mut tx = self.pool.begin().await.map_sql_err()?;
        sqlx::query(
            r#"
INSERT INTO webhook_delivery_attempts (
  id, delivery_id, attempt_index, status, http_status, error_message,
  response_excerpt, started_at_unix_secs, completed_at_unix_secs, duration_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(delivery_id, attempt_index) DO UPDATE SET
  status = excluded.status,
  http_status = excluded.http_status,
  error_message = excluded.error_message,
  response_excerpt = excluded.response_excerpt,
  completed_at_unix_secs = excluded.completed_at_unix_secs,
  duration_ms = excluded.duration_ms
"#,
        )
        .bind(&id)
        .bind(&record.delivery_id)
        .bind(u32_to_i32(record.attempt_index, "attempt_index")?)
        .bind(&record.status)
        .bind(record.http_status)
        .bind(record.error_message.as_deref())
        .bind(record.response_excerpt.as_deref())
        .bind(u64_to_i64(
            record.started_at_unix_secs,
            "started_at_unix_secs",
        )?)
        .bind(
            record
                .completed_at_unix_secs
                .map(|value| u64_to_i64(value, "completed_at_unix_secs"))
                .transpose()?,
        )
        .bind(record.duration_ms.map(|value| value as i32))
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        sqlx::query(
            r#"
UPDATE webhook_deliveries
SET status = ?,
    attempt_count = MAX(attempt_count, ?),
    next_attempt_at_unix_secs = ?,
    last_attempt_at_unix_secs = ?,
    delivered_at_unix_secs = ?,
    failed_at_unix_secs = ?,
    last_http_status = ?,
    last_error = ?,
    response_excerpt = ?,
    lease_owner = NULL,
    lease_expires_at_unix_secs = NULL,
    updated_at_unix_secs = ?
WHERE id = ?
"#,
        )
        .bind(&record.delivery_status)
        .bind(u32_to_i32(record.attempt_index, "attempt_index")?)
        .bind(
            record
                .next_attempt_at_unix_secs
                .map(|value| u64_to_i64(value, "next_attempt_at_unix_secs"))
                .transpose()?,
        )
        .bind(u64_to_i64(
            record.started_at_unix_secs,
            "started_at_unix_secs",
        )?)
        .bind(
            record
                .delivered_at_unix_secs
                .map(|value| u64_to_i64(value, "delivered_at_unix_secs"))
                .transpose()?,
        )
        .bind(
            record
                .failed_at_unix_secs
                .map(|value| u64_to_i64(value, "failed_at_unix_secs"))
                .transpose()?,
        )
        .bind(record.http_status)
        .bind(record.error_message.as_deref())
        .bind(record.response_excerpt.as_deref())
        .bind(completed_or_started)
        .bind(&record.delivery_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        tx.commit().await.map_sql_err()?;

        let row = sqlx::query(&format!(
            "{ATTEMPT_COLUMNS} WHERE delivery_id = ? AND attempt_index = ? LIMIT 1"
        ))
        .bind(&record.delivery_id)
        .bind(u32_to_i32(record.attempt_index, "attempt_index")?)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        map_attempt_row(&row)
    }

    async fn reschedule_webhook_delivery(
        &self,
        record: &RescheduleWebhookDeliveryRecord,
    ) -> Result<bool, DataLayerError> {
        record.validate()?;
        let affected = sqlx::query(
            r#"
UPDATE webhook_deliveries
SET status = ?,
    next_attempt_at_unix_secs = ?,
    lease_owner = NULL,
    lease_expires_at_unix_secs = NULL,
    updated_at_unix_secs = ?
WHERE id = ?
  AND status <> 'delivering'
"#,
        )
        .bind(&record.status)
        .bind(u64_to_i64(
            record.next_attempt_at_unix_secs,
            "next_attempt_at_unix_secs",
        )?)
        .bind(u64_to_i64(
            record.updated_at_unix_secs,
            "updated_at_unix_secs",
        )?)
        .bind(&record.delivery_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(affected > 0)
    }

    async fn update_webhook_endpoint_test_result(
        &self,
        record: &UpdateWebhookEndpointTestResultRecord,
    ) -> Result<bool, DataLayerError> {
        record.validate()?;
        let affected = sqlx::query(
            r#"
UPDATE webhook_endpoints
SET last_test_delivery_id = ?,
    last_test_status = ?,
    last_test_error = ?,
    last_test_at_unix_secs = ?,
    updated_at_unix_secs = ?
WHERE id = ?
"#,
        )
        .bind(record.delivery_id.as_deref())
        .bind(&record.status)
        .bind(record.error_message.as_deref())
        .bind(u64_to_i64(
            record.tested_at_unix_secs,
            "tested_at_unix_secs",
        )?)
        .bind(u64_to_i64(
            record.tested_at_unix_secs,
            "tested_at_unix_secs",
        )?)
        .bind(&record.endpoint_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(affected > 0)
    }
}

fn apply_delivery_filter(builder: &mut QueryBuilder<'_, Sqlite>, query: &WebhookDeliveryListQuery) {
    let mut has_where = false;
    if let Some(endpoint_id) = query.endpoint_id.as_deref() {
        push_where(builder, &mut has_where);
        builder
            .push("endpoint_id = ")
            .push_bind(endpoint_id.to_string());
    }
    if let Some(status) = query.status.as_deref() {
        push_where(builder, &mut has_where);
        builder.push("status = ").push_bind(status.to_string());
    }
    if let Some(event_type) = query.event_type.as_deref() {
        push_where(builder, &mut has_where);
        builder
            .push("event_type = ")
            .push_bind(event_type.to_string());
    }
    if let Some(is_test) = query.is_test {
        push_where(builder, &mut has_where);
        builder.push("is_test = ").push_bind(is_test);
    }
}

fn push_where(builder: &mut QueryBuilder<'_, Sqlite>, has_where: &mut bool) {
    if *has_where {
        builder.push(" AND ");
    } else {
        builder.push(" WHERE ");
        *has_where = true;
    }
}

fn map_endpoint_row(row: &SqliteRow) -> Result<StoredWebhookEndpoint, DataLayerError> {
    Ok(StoredWebhookEndpoint {
        id: row.try_get("id").map_sql_err()?,
        name: row.try_get("name").map_sql_err()?,
        url: row.try_get("url").map_sql_err()?,
        secret_encrypted: row.try_get("secret_encrypted").map_sql_err()?,
        enabled: row.try_get("enabled").map_sql_err()?,
        event_filter_json: parse_optional_json(row.try_get("event_filter_json").map_sql_err()?)?,
        timeout_ms: u32_from_i32(row.try_get("timeout_ms").map_sql_err()?, "timeout_ms")?,
        max_attempts: u32_from_i32(row.try_get("max_attempts").map_sql_err()?, "max_attempts")?,
        initial_retry_delay_seconds: u32_from_i32(
            row.try_get("initial_retry_delay_seconds").map_sql_err()?,
            "initial_retry_delay_seconds",
        )?,
        max_retry_delay_seconds: u32_from_i32(
            row.try_get("max_retry_delay_seconds").map_sql_err()?,
            "max_retry_delay_seconds",
        )?,
        last_test_delivery_id: row.try_get("last_test_delivery_id").map_sql_err()?,
        last_test_status: row.try_get("last_test_status").map_sql_err()?,
        last_test_error: row.try_get("last_test_error").map_sql_err()?,
        last_test_at_unix_secs: optional_u64_from_i64(
            row.try_get("last_test_at_unix_secs").map_sql_err()?,
        ),
        created_by: row.try_get("created_by").map_sql_err()?,
        created_at_unix_secs: u64_from_i64(
            row.try_get("created_at_unix_secs").map_sql_err()?,
            "created_at_unix_secs",
        )?,
        updated_at_unix_secs: u64_from_i64(
            row.try_get("updated_at_unix_secs").map_sql_err()?,
            "updated_at_unix_secs",
        )?,
    })
}

fn map_delivery_row(row: &SqliteRow) -> Result<StoredWebhookDelivery, DataLayerError> {
    Ok(StoredWebhookDelivery {
        id: row.try_get("id").map_sql_err()?,
        endpoint_id: row.try_get("endpoint_id").map_sql_err()?,
        event_id: row.try_get("event_id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        is_test: row.try_get("is_test").map_sql_err()?,
        payload_json: parse_required_json(
            row.try_get("payload_json").map_sql_err()?,
            "payload_json",
        )?,
        status: row.try_get("status").map_sql_err()?,
        attempt_count: u32_from_i32(row.try_get("attempt_count").map_sql_err()?, "attempt_count")?,
        max_attempts: u32_from_i32(row.try_get("max_attempts").map_sql_err()?, "max_attempts")?,
        next_attempt_at_unix_secs: u64_from_i64(
            row.try_get("next_attempt_at_unix_secs").map_sql_err()?,
            "next_attempt_at_unix_secs",
        )?,
        last_attempt_at_unix_secs: optional_u64_from_i64(
            row.try_get("last_attempt_at_unix_secs").map_sql_err()?,
        ),
        delivered_at_unix_secs: optional_u64_from_i64(
            row.try_get("delivered_at_unix_secs").map_sql_err()?,
        ),
        failed_at_unix_secs: optional_u64_from_i64(
            row.try_get("failed_at_unix_secs").map_sql_err()?,
        ),
        last_http_status: row.try_get("last_http_status").map_sql_err()?,
        last_error: row.try_get("last_error").map_sql_err()?,
        response_excerpt: row.try_get("response_excerpt").map_sql_err()?,
        lease_owner: row.try_get("lease_owner").map_sql_err()?,
        lease_expires_at_unix_secs: optional_u64_from_i64(
            row.try_get("lease_expires_at_unix_secs").map_sql_err()?,
        ),
        created_at_unix_secs: u64_from_i64(
            row.try_get("created_at_unix_secs").map_sql_err()?,
            "created_at_unix_secs",
        )?,
        updated_at_unix_secs: u64_from_i64(
            row.try_get("updated_at_unix_secs").map_sql_err()?,
            "updated_at_unix_secs",
        )?,
    })
}

fn map_attempt_row(row: &SqliteRow) -> Result<StoredWebhookDeliveryAttempt, DataLayerError> {
    Ok(StoredWebhookDeliveryAttempt {
        id: row.try_get("id").map_sql_err()?,
        delivery_id: row.try_get("delivery_id").map_sql_err()?,
        attempt_index: u32_from_i32(row.try_get("attempt_index").map_sql_err()?, "attempt_index")?,
        status: row.try_get("status").map_sql_err()?,
        http_status: row.try_get("http_status").map_sql_err()?,
        error_message: row.try_get("error_message").map_sql_err()?,
        response_excerpt: row.try_get("response_excerpt").map_sql_err()?,
        started_at_unix_secs: u64_from_i64(
            row.try_get("started_at_unix_secs").map_sql_err()?,
            "started_at_unix_secs",
        )?,
        completed_at_unix_secs: optional_u64_from_i64(
            row.try_get("completed_at_unix_secs").map_sql_err()?,
        ),
        duration_ms: optional_u32_from_i32(row.try_get("duration_ms").map_sql_err()?),
    })
}

fn parse_optional_json(value: Option<String>) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .map(|raw| serde_json::from_str::<serde_json::Value>(&raw).map_err(DataLayerError::sql))
        .transpose()
}

fn parse_required_json(value: String, field: &str) -> Result<serde_json::Value, DataLayerError> {
    serde_json::from_str::<serde_json::Value>(&value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("webhook delivery {field} is invalid: {err}"))
    })
}

fn json_to_string(value: Option<&serde_json::Value>) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| serde_json::to_string(value).map_err(DataLayerError::sql))
        .transpose()
}

fn required_json_to_string(
    value: &serde_json::Value,
    field: &str,
) -> Result<String, DataLayerError> {
    serde_json::to_string(value)
        .map_err(|err| DataLayerError::InvalidInput(format!("{field} is not serializable: {err}")))
}

fn now_unix_secs_i64() -> Result<i64, DataLayerError> {
    u64_to_i64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        "now_unix_secs",
    )
}

fn i64_from_usize(value: usize, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| DataLayerError::InvalidInput(format!("{field} is too large")))
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| DataLayerError::InvalidInput(format!("{field} is too large")))
}

fn u32_to_i32(value: u32, field: &str) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| DataLayerError::InvalidInput(format!("{field} is too large")))
}

fn u64_from_i64(value: i64, field: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} cannot be negative")))
}

fn u32_from_i32(value: i32, field: &str) -> Result<u32, DataLayerError> {
    u32::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} cannot be negative")))
}

fn optional_u64_from_i64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn optional_u32_from_i32(value: Option<i32>) -> Option<u32> {
    value.and_then(|value| u32::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::SqliteBackend;
    use crate::lifecycle::migrate::run_sqlite_migrations;
    use crate::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};

    #[tokio::test]
    async fn webhook_delivery_claim_and_retry_round_trip() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite::memory:".to_string(),
            pool: SqlPoolConfig {
                max_connections: 1,
                ..SqlPoolConfig::default()
            },
        };
        let backend = SqliteBackend::from_config(config).expect("sqlite backend should build");
        run_sqlite_migrations(backend.pool())
            .await
            .expect("sqlite migrations should run");
        let repo = SqliteWebhookNotificationRepository::new(backend.pool_clone());

        let endpoint = repo
            .upsert_webhook_endpoint(&UpsertWebhookEndpointRecord {
                id: Some("endpoint-1".to_string()),
                name: "Risk hook".to_string(),
                url: "https://example.com/webhook".to_string(),
                secret_encrypted: WebhookSecretUpdate::Set("encrypted-secret".to_string()),
                enabled: true,
                event_filter_json: Some(serde_json::json!(["risk.hit"])),
                timeout_ms: 5_000,
                max_attempts: 3,
                initial_retry_delay_seconds: 5,
                max_retry_delay_seconds: 60,
                created_by: Some("admin-1".to_string()),
            })
            .await
            .expect("endpoint should upsert");
        assert!(endpoint.subscribes_to("risk.hit"));

        let delivery = repo
            .create_webhook_delivery(&CreateWebhookDeliveryRecord {
                id: Some("delivery-1".to_string()),
                endpoint_id: endpoint.id.clone(),
                event_id: "event-1".to_string(),
                event_type: "risk.hit".to_string(),
                is_test: false,
                payload_json: serde_json::json!({"score": 91}),
                max_attempts: 3,
                next_attempt_at_unix_secs: 100,
            })
            .await
            .expect("delivery should create");
        assert_eq!(delivery.status, "pending");

        let first_claim = repo
            .claim_due_webhook_deliveries(&ClaimWebhookDeliveriesRequest {
                owner: "worker-1".to_string(),
                now_unix_secs: 100,
                lease_seconds: 30,
                limit: 10,
            })
            .await
            .expect("delivery should claim");
        assert_eq!(first_claim.len(), 1);
        assert_eq!(first_claim[0].status, "delivering");

        let duplicate_claim = repo
            .claim_due_webhook_deliveries(&ClaimWebhookDeliveriesRequest {
                owner: "worker-1".to_string(),
                now_unix_secs: 100,
                lease_seconds: 30,
                limit: 10,
            })
            .await
            .expect("duplicate claim should run");
        assert!(duplicate_claim.is_empty());

        let attempt = repo
            .record_webhook_delivery_attempt(&RecordWebhookDeliveryAttemptRecord {
                id: Some("attempt-1".to_string()),
                delivery_id: delivery.id.clone(),
                attempt_index: 1,
                status: "failed".to_string(),
                http_status: Some(500),
                error_message: Some("upstream timeout".to_string()),
                response_excerpt: Some("timeout".to_string()),
                started_at_unix_secs: 105,
                completed_at_unix_secs: Some(106),
                duration_ms: Some(1_000),
                delivery_status: "retrying".to_string(),
                next_attempt_at_unix_secs: Some(120),
                delivered_at_unix_secs: None,
                failed_at_unix_secs: None,
            })
            .await
            .expect("attempt should record");
        assert_eq!(attempt.status, "failed");

        let early_retry_claim = repo
            .claim_due_webhook_deliveries(&ClaimWebhookDeliveriesRequest {
                owner: "worker-2".to_string(),
                now_unix_secs: 119,
                lease_seconds: 30,
                limit: 10,
            })
            .await
            .expect("early retry claim should run");
        assert!(early_retry_claim.is_empty());

        let retry_claim = repo
            .claim_due_webhook_deliveries(&ClaimWebhookDeliveriesRequest {
                owner: "worker-2".to_string(),
                now_unix_secs: 120,
                lease_seconds: 30,
                limit: 10,
            })
            .await
            .expect("retry delivery should claim");
        assert_eq!(retry_claim.len(), 1);
        assert_eq!(retry_claim[0].attempt_count, 1);
        assert_eq!(
            retry_claim[0].last_error.as_deref(),
            Some("upstream timeout")
        );
    }
}
