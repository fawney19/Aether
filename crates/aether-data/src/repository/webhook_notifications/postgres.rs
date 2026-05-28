use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use super::types::{
    event_filter_matches, ClaimWebhookDeliveriesRequest, CreateWebhookDeliveryRecord,
    RecordWebhookDeliveryAttemptRecord, RescheduleWebhookDeliveryRecord, StoredWebhookDelivery,
    StoredWebhookDeliveryAttempt, StoredWebhookDeliveryPage, StoredWebhookEndpoint,
    UpdateWebhookEndpointTestResultRecord, UpsertWebhookEndpointRecord,
    WebhookDeliveryAttemptListQuery, WebhookDeliveryListQuery, WebhookEndpointListQuery,
    WebhookNotificationReadRepository, WebhookNotificationWriteRepository,
};
use crate::error::SqlxResultExt;
use crate::DataLayerError;
use aether_data_query::{push_eq, push_limit_offset, WhereClause};

const ENDPOINT_SELECT: &str = r#"
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

const DELIVERY_SELECT: &str = r#"
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

const ATTEMPT_SELECT: &str = r#"
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
pub struct SqlxWebhookNotificationRepository {
    pool: PgPool,
}

impl SqlxWebhookNotificationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn apply_delivery_filter(
        builder: &mut QueryBuilder<'_, Postgres>,
        query: &WebhookDeliveryListQuery,
    ) {
        let mut where_clause = WhereClause::new();
        if let Some(endpoint_id) = query.endpoint_id.as_deref() {
            push_eq(
                builder,
                &mut where_clause,
                "endpoint_id",
                endpoint_id.to_string(),
            );
        }
        if let Some(status) = query.status.as_deref() {
            push_eq(builder, &mut where_clause, "status", status.to_string());
        }
        if let Some(event_type) = query.event_type.as_deref() {
            push_eq(
                builder,
                &mut where_clause,
                "event_type",
                event_type.to_string(),
            );
        }
        if let Some(is_test) = query.is_test {
            push_eq(builder, &mut where_clause, "is_test", is_test);
        }
    }

    async fn find_delivery_by_endpoint_event(
        &self,
        endpoint_id: &str,
        event_id: &str,
    ) -> Result<Option<StoredWebhookDelivery>, DataLayerError> {
        let row = sqlx::query(&format!(
            "{DELIVERY_SELECT} WHERE endpoint_id = $1 AND event_id = $2 LIMIT 1"
        ))
        .bind(endpoint_id)
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref().map(map_delivery_row).transpose()
    }
}

#[async_trait]
impl WebhookNotificationReadRepository for SqlxWebhookNotificationRepository {
    async fn list_webhook_endpoints(
        &self,
        query: &WebhookEndpointListQuery,
    ) -> Result<Vec<StoredWebhookEndpoint>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(ENDPOINT_SELECT);
        let mut where_clause = WhereClause::new();
        if let Some(enabled) = query.enabled {
            push_eq(&mut builder, &mut where_clause, "enabled", enabled);
        }
        builder.push(" ORDER BY updated_at_unix_secs DESC, name ASC");
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        let mut endpoints = rows
            .iter()
            .map(map_endpoint_row)
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(event_type) = query.event_type.as_deref() {
            endpoints.retain(|endpoint| {
                event_filter_matches(endpoint.event_filter_json.as_ref(), event_type)
            });
        }
        Ok(endpoints)
    }

    async fn find_webhook_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<Option<StoredWebhookEndpoint>, DataLayerError> {
        let row = sqlx::query(&format!("{ENDPOINT_SELECT} WHERE id = $1 LIMIT 1"))
            .bind(endpoint_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_endpoint_row).transpose()
    }

    async fn find_webhook_delivery(
        &self,
        delivery_id: &str,
    ) -> Result<Option<StoredWebhookDelivery>, DataLayerError> {
        let row = sqlx::query(&format!("{DELIVERY_SELECT} WHERE id = $1 LIMIT 1"))
            .bind(delivery_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_delivery_row).transpose()
    }

    async fn list_webhook_deliveries(
        &self,
        query: &WebhookDeliveryListQuery,
    ) -> Result<StoredWebhookDeliveryPage, DataLayerError> {
        let limit = query.limit.max(1);
        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(id) AS total FROM webhook_deliveries");
        Self::apply_delivery_filter(&mut count_builder, query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;

        let mut builder = QueryBuilder::<Postgres>::new(DELIVERY_SELECT);
        Self::apply_delivery_filter(&mut builder, query);
        builder.push(" ORDER BY created_at_unix_secs DESC, id ASC");
        push_limit_offset(
            &mut builder,
            i64_from_usize(limit, "webhook delivery limit")?,
            i64_from_usize(query.offset, "webhook delivery offset")?,
        );
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        let items = rows
            .iter()
            .map(map_delivery_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredWebhookDeliveryPage {
            items,
            total: u64::try_from(total).unwrap_or_default(),
        })
    }

    async fn list_webhook_delivery_attempts(
        &self,
        query: &WebhookDeliveryAttemptListQuery,
    ) -> Result<Vec<StoredWebhookDeliveryAttempt>, DataLayerError> {
        let limit = query.limit.max(1);
        let rows = sqlx::query(&format!(
            "{ATTEMPT_SELECT} WHERE delivery_id = $1 ORDER BY attempt_index ASC LIMIT $2 OFFSET $3"
        ))
        .bind(&query.delivery_id)
        .bind(i64_from_usize(limit, "webhook attempt limit")?)
        .bind(i64_from_usize(query.offset, "webhook attempt offset")?)
        .fetch_all(&self.pool)
        .await
        .map_postgres_err()?;
        rows.iter().map(map_attempt_row).collect()
    }
}

#[async_trait]
impl WebhookNotificationWriteRepository for SqlxWebhookNotificationRepository {
    async fn upsert_webhook_endpoint(
        &self,
        record: &UpsertWebhookEndpointRecord,
    ) -> Result<StoredWebhookEndpoint, DataLayerError> {
        record.validate()?;
        let id = record.effective_id();
        let now = current_unix_secs();
        sqlx::query(
            r#"
INSERT INTO webhook_endpoints (
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
  created_by,
  created_at_unix_secs,
  updated_at_unix_secs
) VALUES (
  $1,
  $2,
  $3,
  CASE $4 WHEN 'set' THEN $5 ELSE NULL END,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13,
  $14
)
ON CONFLICT(id) DO UPDATE SET
  name = EXCLUDED.name,
  url = EXCLUDED.url,
  secret_encrypted = CASE $4
    WHEN 'set' THEN $5
    WHEN 'clear' THEN NULL
    ELSE webhook_endpoints.secret_encrypted
  END,
  enabled = EXCLUDED.enabled,
  event_filter_json = EXCLUDED.event_filter_json,
  timeout_ms = EXCLUDED.timeout_ms,
  max_attempts = EXCLUDED.max_attempts,
  initial_retry_delay_seconds = EXCLUDED.initial_retry_delay_seconds,
  max_retry_delay_seconds = EXCLUDED.max_retry_delay_seconds,
  created_by = COALESCE(webhook_endpoints.created_by, EXCLUDED.created_by),
  updated_at_unix_secs = EXCLUDED.updated_at_unix_secs
"#,
        )
        .bind(&id)
        .bind(&record.name)
        .bind(&record.url)
        .bind(record.secret_encrypted.mode_name())
        .bind(record.secret_encrypted.value())
        .bind(record.enabled)
        .bind(record.event_filter_json.clone())
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
        .bind(u64_to_i64(now, "created_at_unix_secs")?)
        .bind(u64_to_i64(now, "updated_at_unix_secs")?)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;

        self.find_webhook_endpoint(&id).await?.ok_or_else(|| {
            DataLayerError::UnexpectedValue("webhook endpoint missing after upsert".to_string())
        })
    }

    async fn delete_webhook_endpoint(&self, endpoint_id: &str) -> Result<bool, DataLayerError> {
        let affected = sqlx::query("DELETE FROM webhook_endpoints WHERE id = $1")
            .bind(endpoint_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?
            .rows_affected();
        Ok(affected > 0)
    }

    async fn create_webhook_delivery(
        &self,
        record: &CreateWebhookDeliveryRecord,
    ) -> Result<StoredWebhookDelivery, DataLayerError> {
        record.validate()?;
        let id = record.effective_id();
        let now = current_unix_secs();
        sqlx::query(
            r#"
INSERT INTO webhook_deliveries (
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
  created_at_unix_secs,
  updated_at_unix_secs
) VALUES ($1, $2, $3, $4, $5, $6, 'pending', 0, $7, $8, $9, $10)
ON CONFLICT(endpoint_id, event_id) DO NOTHING
"#,
        )
        .bind(&id)
        .bind(&record.endpoint_id)
        .bind(&record.event_id)
        .bind(&record.event_type)
        .bind(record.is_test)
        .bind(record.payload_json.clone())
        .bind(u32_to_i32(record.max_attempts, "max_attempts")?)
        .bind(u64_to_i64(
            record.next_attempt_at_unix_secs,
            "next_attempt_at_unix_secs",
        )?)
        .bind(u64_to_i64(now, "created_at_unix_secs")?)
        .bind(u64_to_i64(now, "updated_at_unix_secs")?)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;

        self.find_delivery_by_endpoint_event(&record.endpoint_id, &record.event_id)
            .await?
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue("webhook delivery missing after create".to_string())
            })
    }

    async fn claim_due_webhook_deliveries(
        &self,
        request: &ClaimWebhookDeliveriesRequest,
    ) -> Result<Vec<StoredWebhookDelivery>, DataLayerError> {
        request.validate()?;
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let rows = sqlx::query(
            r#"
SELECT id
FROM webhook_deliveries
WHERE status IN ('pending', 'retrying')
  AND next_attempt_at_unix_secs <= $1
  AND (lease_owner IS NULL OR lease_expires_at_unix_secs IS NULL OR lease_expires_at_unix_secs <= $2)
ORDER BY next_attempt_at_unix_secs ASC, created_at_unix_secs ASC, id ASC
LIMIT $3
FOR UPDATE SKIP LOCKED
"#,
        )
        .bind(u64_to_i64(request.now_unix_secs, "now_unix_secs")?)
        .bind(u64_to_i64(request.now_unix_secs, "now_unix_secs")?)
        .bind(i64_from_usize(
            request.limit,
            "webhook delivery claim limit",
        )?)
        .fetch_all(&mut *tx)
        .await
        .map_postgres_err()?;

        let mut claimed = Vec::new();
        for row in rows {
            let id: String = row.try_get("id").map_postgres_err()?;
            let affected = sqlx::query(
                r#"
UPDATE webhook_deliveries
SET status = 'delivering',
    lease_owner = $1,
    lease_expires_at_unix_secs = $2,
    updated_at_unix_secs = $3
WHERE id = $4
  AND status IN ('pending', 'retrying')
  AND next_attempt_at_unix_secs <= $5
  AND (lease_owner IS NULL OR lease_expires_at_unix_secs IS NULL OR lease_expires_at_unix_secs <= $5)
"#,
            )
            .bind(&request.owner)
            .bind(u64_to_i64(
                request.lease_expires_at_unix_secs(),
                "lease_expires_at_unix_secs",
            )?)
            .bind(u64_to_i64(request.now_unix_secs, "updated_at_unix_secs")?)
            .bind(&id)
            .bind(u64_to_i64(request.now_unix_secs, "now_unix_secs")?)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?
            .rows_affected();

            if affected > 0 {
                let row = sqlx::query(&format!("{DELIVERY_SELECT} WHERE id = $1 LIMIT 1"))
                    .bind(&id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_postgres_err()?;
                claimed.push(map_delivery_row(&row)?);
            }
        }
        tx.commit().await.map_postgres_err()?;
        Ok(claimed)
    }

    async fn record_webhook_delivery_attempt(
        &self,
        record: &RecordWebhookDeliveryAttemptRecord,
    ) -> Result<StoredWebhookDeliveryAttempt, DataLayerError> {
        record.validate()?;
        let id = record.effective_id();
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        sqlx::query(
            r#"
INSERT INTO webhook_delivery_attempts (
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
) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
ON CONFLICT(delivery_id, attempt_index) DO UPDATE SET
  status = EXCLUDED.status,
  http_status = EXCLUDED.http_status,
  error_message = EXCLUDED.error_message,
  response_excerpt = EXCLUDED.response_excerpt,
  started_at_unix_secs = EXCLUDED.started_at_unix_secs,
  completed_at_unix_secs = EXCLUDED.completed_at_unix_secs,
  duration_ms = EXCLUDED.duration_ms
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
        .bind(
            record
                .duration_ms
                .map(|value| u32_to_i32(value, "duration_ms"))
                .transpose()?,
        )
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;

        sqlx::query(
            r#"
UPDATE webhook_deliveries
SET status = $1,
    attempt_count = GREATEST(attempt_count, $2),
    last_attempt_at_unix_secs = $3,
    delivered_at_unix_secs = COALESCE($4, delivered_at_unix_secs),
    failed_at_unix_secs = COALESCE($5, failed_at_unix_secs),
    last_http_status = $6,
    last_error = $7,
    response_excerpt = $8,
    next_attempt_at_unix_secs = $9,
    lease_owner = NULL,
    lease_expires_at_unix_secs = NULL,
    updated_at_unix_secs = $10
WHERE id = $11
"#,
        )
        .bind(&record.delivery_status)
        .bind(u32_to_i32(record.attempt_index, "attempt_index")?)
        .bind(u64_to_i64(
            record.completed_or_started_at(),
            "last_attempt_at_unix_secs",
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
        .bind(
            record
                .next_attempt_at_unix_secs
                .map(|value| u64_to_i64(value, "next_attempt_at_unix_secs"))
                .transpose()?,
        )
        .bind(u64_to_i64(
            record.completed_or_started_at(),
            "updated_at_unix_secs",
        )?)
        .bind(&record.delivery_id)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;

        let row = sqlx::query(&format!(
            "{ATTEMPT_SELECT} WHERE delivery_id = $1 AND attempt_index = $2 LIMIT 1"
        ))
        .bind(&record.delivery_id)
        .bind(u32_to_i32(record.attempt_index, "attempt_index")?)
        .fetch_one(&mut *tx)
        .await
        .map_postgres_err()?;
        let attempt = map_attempt_row(&row)?;
        tx.commit().await.map_postgres_err()?;
        Ok(attempt)
    }

    async fn reschedule_webhook_delivery(
        &self,
        record: &RescheduleWebhookDeliveryRecord,
    ) -> Result<bool, DataLayerError> {
        record.validate()?;
        let affected = sqlx::query(
            r#"
UPDATE webhook_deliveries
SET status = $1,
    next_attempt_at_unix_secs = $2,
    lease_owner = NULL,
    lease_expires_at_unix_secs = NULL,
    updated_at_unix_secs = $3
WHERE id = $4
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
        .map_postgres_err()?
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
SET last_test_delivery_id = $1,
    last_test_status = $2,
    last_test_error = $3,
    last_test_at_unix_secs = $4,
    updated_at_unix_secs = $5
WHERE id = $6
"#,
        )
        .bind(record.delivery_id.as_deref())
        .bind(&record.status)
        .bind(record.error_message.as_deref())
        .bind(u64_to_i64(
            record.tested_at_unix_secs,
            "last_test_at_unix_secs",
        )?)
        .bind(u64_to_i64(
            record.tested_at_unix_secs,
            "updated_at_unix_secs",
        )?)
        .bind(&record.endpoint_id)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();
        Ok(affected > 0)
    }
}

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn map_endpoint_row(row: &PgRow) -> Result<StoredWebhookEndpoint, DataLayerError> {
    Ok(StoredWebhookEndpoint {
        id: row.try_get("id").map_postgres_err()?,
        name: row.try_get("name").map_postgres_err()?,
        url: row.try_get("url").map_postgres_err()?,
        secret_encrypted: row.try_get("secret_encrypted").map_postgres_err()?,
        enabled: row.try_get("enabled").map_postgres_err()?,
        event_filter_json: row.try_get("event_filter_json").map_postgres_err()?,
        timeout_ms: i32_to_u32(row.try_get("timeout_ms").map_postgres_err()?),
        max_attempts: i32_to_u32(row.try_get("max_attempts").map_postgres_err()?),
        initial_retry_delay_seconds: i32_to_u32(
            row.try_get("initial_retry_delay_seconds")
                .map_postgres_err()?,
        ),
        max_retry_delay_seconds: i32_to_u32(
            row.try_get("max_retry_delay_seconds").map_postgres_err()?,
        ),
        last_test_delivery_id: row.try_get("last_test_delivery_id").map_postgres_err()?,
        last_test_status: row.try_get("last_test_status").map_postgres_err()?,
        last_test_error: row.try_get("last_test_error").map_postgres_err()?,
        last_test_at_unix_secs: optional_i64_to_u64(
            row.try_get("last_test_at_unix_secs").map_postgres_err()?,
        ),
        created_by: row.try_get("created_by").map_postgres_err()?,
        created_at_unix_secs: i64_to_u64(row.try_get("created_at_unix_secs").map_postgres_err()?),
        updated_at_unix_secs: i64_to_u64(row.try_get("updated_at_unix_secs").map_postgres_err()?),
    })
}

fn map_delivery_row(row: &PgRow) -> Result<StoredWebhookDelivery, DataLayerError> {
    Ok(StoredWebhookDelivery {
        id: row.try_get("id").map_postgres_err()?,
        endpoint_id: row.try_get("endpoint_id").map_postgres_err()?,
        event_id: row.try_get("event_id").map_postgres_err()?,
        event_type: row.try_get("event_type").map_postgres_err()?,
        is_test: row.try_get("is_test").map_postgres_err()?,
        payload_json: row.try_get("payload_json").map_postgres_err()?,
        status: row.try_get("status").map_postgres_err()?,
        attempt_count: i32_to_u32(row.try_get("attempt_count").map_postgres_err()?),
        max_attempts: i32_to_u32(row.try_get("max_attempts").map_postgres_err()?),
        next_attempt_at_unix_secs: i64_to_u64(
            row.try_get("next_attempt_at_unix_secs")
                .map_postgres_err()?,
        ),
        last_attempt_at_unix_secs: optional_i64_to_u64(
            row.try_get("last_attempt_at_unix_secs")
                .map_postgres_err()?,
        ),
        delivered_at_unix_secs: optional_i64_to_u64(
            row.try_get("delivered_at_unix_secs").map_postgres_err()?,
        ),
        failed_at_unix_secs: optional_i64_to_u64(
            row.try_get("failed_at_unix_secs").map_postgres_err()?,
        ),
        last_http_status: row.try_get("last_http_status").map_postgres_err()?,
        last_error: row.try_get("last_error").map_postgres_err()?,
        response_excerpt: row.try_get("response_excerpt").map_postgres_err()?,
        lease_owner: row.try_get("lease_owner").map_postgres_err()?,
        lease_expires_at_unix_secs: optional_i64_to_u64(
            row.try_get("lease_expires_at_unix_secs")
                .map_postgres_err()?,
        ),
        created_at_unix_secs: i64_to_u64(row.try_get("created_at_unix_secs").map_postgres_err()?),
        updated_at_unix_secs: i64_to_u64(row.try_get("updated_at_unix_secs").map_postgres_err()?),
    })
}

fn map_attempt_row(row: &PgRow) -> Result<StoredWebhookDeliveryAttempt, DataLayerError> {
    Ok(StoredWebhookDeliveryAttempt {
        id: row.try_get("id").map_postgres_err()?,
        delivery_id: row.try_get("delivery_id").map_postgres_err()?,
        attempt_index: i32_to_u32(row.try_get("attempt_index").map_postgres_err()?),
        status: row.try_get("status").map_postgres_err()?,
        http_status: row.try_get("http_status").map_postgres_err()?,
        error_message: row.try_get("error_message").map_postgres_err()?,
        response_excerpt: row.try_get("response_excerpt").map_postgres_err()?,
        started_at_unix_secs: i64_to_u64(row.try_get("started_at_unix_secs").map_postgres_err()?),
        completed_at_unix_secs: optional_i64_to_u64(
            row.try_get("completed_at_unix_secs").map_postgres_err()?,
        ),
        duration_ms: optional_i32_to_u32(row.try_get("duration_ms").map_postgres_err()?),
    })
}

fn i64_from_usize(value: usize, label: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{label} overflow: {value}")))
}

fn u64_to_i64(value: u64, label: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{label} overflow: {value}")))
}

fn u32_to_i32(value: u32, label: &str) -> Result<i32, DataLayerError> {
    i32::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("{label} overflow: {value}")))
}

fn i64_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn optional_i64_to_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn i32_to_u32(value: i32) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn optional_i32_to_u32(value: Option<i32>) -> Option<u32> {
    value.and_then(|value| u32::try_from(value).ok())
}
