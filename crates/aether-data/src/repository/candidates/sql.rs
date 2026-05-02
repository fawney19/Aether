use async_trait::async_trait;
use futures_util::{future::BoxFuture, stream::TryStream, TryStreamExt};
use sqlx::{postgres::PgRow, PgPool, Row};
use uuid::Uuid;

use super::{
    PublicHealthStatusCount, PublicHealthTimelineBucket, RequestCandidateReadRepository,
    RequestCandidateStatus, RequestCandidateWriteRepository, StoredRequestCandidate,
    UpsertRequestCandidateRecord,
};
use crate::postgres::PostgresTransactionRunner;
use crate::{error::SqlxResultExt, DataLayerError};

const LIST_BY_REQUEST_ID_SQL: &str = r#"
SELECT
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
  candidate_index,
  retry_index,
  provider_id,
  endpoint_id,
  key_id,
  status,
  skip_reason,
  is_cached,
  status_code,
  error_type,
  error_message,
  latency_ms,
  concurrent_requests,
  extra_data,
  required_capabilities,
  CAST(EXTRACT(EPOCH FROM created_at) * 1000 AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM started_at) * 1000 AS BIGINT) AS started_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM finished_at) * 1000 AS BIGINT) AS finished_at_unix_ms
FROM request_candidates
WHERE request_id = $1
ORDER BY candidate_index ASC, retry_index ASC, created_at ASC
"#;

const LIST_RECENT_SQL: &str = r#"
SELECT
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
  candidate_index,
  retry_index,
  provider_id,
  endpoint_id,
  key_id,
  status,
  skip_reason,
  is_cached,
  status_code,
  error_type,
  error_message,
  latency_ms,
  concurrent_requests,
  extra_data,
  required_capabilities,
  CAST(EXTRACT(EPOCH FROM created_at) * 1000 AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM started_at) * 1000 AS BIGINT) AS started_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM finished_at) * 1000 AS BIGINT) AS finished_at_unix_ms
FROM request_candidates
ORDER BY created_at DESC
LIMIT $1
"#;

const LIST_BY_PROVIDER_ID_SQL: &str = r#"
SELECT
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
  candidate_index,
  retry_index,
  provider_id,
  endpoint_id,
  key_id,
  status,
  skip_reason,
  is_cached,
  status_code,
  error_type,
  error_message,
  latency_ms,
  concurrent_requests,
  extra_data,
  required_capabilities,
  CAST(EXTRACT(EPOCH FROM created_at) * 1000 AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM started_at) * 1000 AS BIGINT) AS started_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM finished_at) * 1000 AS BIGINT) AS finished_at_unix_ms
FROM request_candidates
WHERE provider_id = $1
ORDER BY created_at DESC
LIMIT $2
"#;

const LIST_FINALIZED_BY_ENDPOINT_IDS_SINCE_SQL: &str = r#"
SELECT
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
  candidate_index,
  retry_index,
  provider_id,
  endpoint_id,
  key_id,
  status,
  skip_reason,
  is_cached,
  status_code,
  error_type,
  error_message,
  latency_ms,
  concurrent_requests,
  extra_data,
  required_capabilities,
  CAST(EXTRACT(EPOCH FROM created_at) * 1000 AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM started_at) * 1000 AS BIGINT) AS started_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM finished_at) * 1000 AS BIGINT) AS finished_at_unix_ms
FROM request_candidates
WHERE endpoint_id = ANY($1)
  AND created_at >= TO_TIMESTAMP($2)
  AND status IN ('success', 'failed', 'skipped')
ORDER BY created_at DESC
LIMIT $3
"#;

const COUNT_FINALIZED_STATUSES_BY_ENDPOINT_IDS_SINCE_SQL: &str = r#"
SELECT
  endpoint_id,
  status,
  COUNT(id) AS count
FROM request_candidates
WHERE endpoint_id = ANY($1)
  AND created_at >= TO_TIMESTAMP($2)
  AND status IN ('success', 'failed', 'skipped')
GROUP BY endpoint_id, status
"#;

const AGGREGATE_FINALIZED_TIMELINE_BY_ENDPOINT_IDS_SINCE_SQL: &str = r#"
SELECT
  endpoint_id,
  FLOOR(EXTRACT(EPOCH FROM (created_at - TO_TIMESTAMP($2))) / $4)::BIGINT AS segment_idx,
  COUNT(id) AS total_count,
  SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) AS success_count,
  SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS failed_count,
  CAST(EXTRACT(EPOCH FROM MIN(created_at)) * 1000 AS BIGINT) AS min_created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM MAX(created_at)) * 1000 AS BIGINT) AS max_created_at_unix_ms
FROM request_candidates
WHERE endpoint_id = ANY($1)
  AND created_at >= TO_TIMESTAMP($2)
  AND created_at <= TO_TIMESTAMP($3)
  AND status IN ('success', 'failed', 'skipped')
GROUP BY
  endpoint_id,
  FLOOR(EXTRACT(EPOCH FROM (created_at - TO_TIMESTAMP($2))) / $4)::BIGINT
"#;

const UPSERT_SQL: &str = r#"
INSERT INTO request_candidates (
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
  candidate_index,
  retry_index,
  provider_id,
  endpoint_id,
  key_id,
  status,
  skip_reason,
  is_cached,
  status_code,
  error_type,
  error_message,
  latency_ms,
  concurrent_requests,
  extra_data,
  required_capabilities,
  created_at,
  started_at,
  finished_at
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13,
  COALESCE($14, false),
  $15,
  $16,
  $17,
  $18,
  $19,
  $20,
  $21,
  COALESCE(
    CASE
      WHEN $22 IS NOT NULL AND $22 > 1000.0 THEN TO_TIMESTAMP($22 / 1000.0)
    END,
    TO_TIMESTAMP($23 / 1000.0),
    TO_TIMESTAMP($24 / 1000.0),
    NOW()
  ),
  TO_TIMESTAMP($23 / 1000.0),
  TO_TIMESTAMP($24 / 1000.0)
)
ON CONFLICT (request_id, candidate_index, retry_index)
DO UPDATE SET
  user_id = COALESCE(EXCLUDED.user_id, request_candidates.user_id),
  api_key_id = COALESCE(EXCLUDED.api_key_id, request_candidates.api_key_id),
  username = COALESCE(EXCLUDED.username, request_candidates.username),
  api_key_name = COALESCE(EXCLUDED.api_key_name, request_candidates.api_key_name),
  provider_id = COALESCE(EXCLUDED.provider_id, request_candidates.provider_id),
  endpoint_id = COALESCE(EXCLUDED.endpoint_id, request_candidates.endpoint_id),
  key_id = COALESCE(EXCLUDED.key_id, request_candidates.key_id),
  status = EXCLUDED.status,
  skip_reason = COALESCE(EXCLUDED.skip_reason, request_candidates.skip_reason),
  is_cached = COALESCE($14, request_candidates.is_cached),
  status_code = COALESCE(EXCLUDED.status_code, request_candidates.status_code),
  error_type = COALESCE(EXCLUDED.error_type, request_candidates.error_type),
  error_message = COALESCE(EXCLUDED.error_message, request_candidates.error_message),
  latency_ms = COALESCE(EXCLUDED.latency_ms, request_candidates.latency_ms),
  concurrent_requests = COALESCE(EXCLUDED.concurrent_requests, request_candidates.concurrent_requests),
  extra_data = CASE
    WHEN request_candidates.extra_data IS NULL THEN EXCLUDED.extra_data
    WHEN EXCLUDED.extra_data IS NULL THEN request_candidates.extra_data
    WHEN json_typeof(request_candidates.extra_data) = 'object'
      AND json_typeof(EXCLUDED.extra_data) = 'object'
      THEN (request_candidates.extra_data::jsonb || EXCLUDED.extra_data::jsonb)::json
    ELSE EXCLUDED.extra_data
  END,
  required_capabilities = COALESCE(EXCLUDED.required_capabilities, request_candidates.required_capabilities),
  created_at = CASE
    WHEN request_candidates.created_at <= TO_TIMESTAMP(1)
      THEN EXCLUDED.created_at
    ELSE request_candidates.created_at
  END,
  started_at = COALESCE(EXCLUDED.started_at, request_candidates.started_at),
  finished_at = COALESCE(EXCLUDED.finished_at, request_candidates.finished_at)
RETURNING
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
  candidate_index,
  retry_index,
  provider_id,
  endpoint_id,
  key_id,
  status,
  skip_reason,
  is_cached,
  status_code,
  error_type,
  error_message,
  latency_ms,
  concurrent_requests,
  extra_data,
  required_capabilities,
  CAST(EXTRACT(EPOCH FROM created_at) * 1000 AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM started_at) * 1000 AS BIGINT) AS started_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM finished_at) * 1000 AS BIGINT) AS finished_at_unix_ms
"#;

const DELETE_CREATED_BEFORE_SQL: &str = r#"
DELETE FROM request_candidates
WHERE id IN (
  SELECT id
  FROM request_candidates
  WHERE created_at < TO_TIMESTAMP($1)
  ORDER BY created_at ASC, id ASC
  LIMIT $2
)
"#;

#[derive(Debug, Clone)]
pub struct SqlxRequestCandidateReadRepository {
    pool: PgPool,
    tx_runner: PostgresTransactionRunner,
}

impl SqlxRequestCandidateReadRepository {
    pub fn new(pool: PgPool) -> Self {
        let tx_runner = PostgresTransactionRunner::new(pool.clone());
        Self { pool, tx_runner }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn transaction_runner(&self) -> &PostgresTransactionRunner {
        &self.tx_runner
    }

    pub async fn list_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        collect_query_rows(
            sqlx::query(LIST_BY_REQUEST_ID_SQL)
                .bind(request_id)
                .fetch(&self.pool),
            map_request_candidate_row,
        )
        .await
    }

    pub async fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        collect_query_rows(
            sqlx::query(LIST_RECENT_SQL)
                .bind(i64::try_from(limit).map_err(|_| {
                    DataLayerError::UnexpectedValue(format!(
                        "invalid recent request candidate limit: {limit}"
                    ))
                })?)
                .fetch(&self.pool),
            map_request_candidate_row,
        )
        .await
    }

    pub async fn list_by_provider_id(
        &self,
        provider_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let limit_value = i64::try_from(limit).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "invalid provider request candidate limit: {limit}"
            ))
        })?;

        collect_query_rows(
            sqlx::query(LIST_BY_PROVIDER_ID_SQL)
                .bind(provider_id)
                .bind(limit_value)
                .fetch(&self.pool),
            map_request_candidate_row,
        )
        .await
    }

    pub async fn list_finalized_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if endpoint_ids.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        collect_query_rows(
            sqlx::query(LIST_FINALIZED_BY_ENDPOINT_IDS_SINCE_SQL)
                .bind(endpoint_ids)
                .bind(since_unix_secs as f64)
                .bind(i64::try_from(limit).map_err(|_| {
                    DataLayerError::UnexpectedValue(format!(
                        "invalid finalized request candidate limit: {limit}"
                    ))
                })?)
                .fetch(&self.pool),
            map_request_candidate_row,
        )
        .await
    }

    pub async fn count_finalized_statuses_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
    ) -> Result<Vec<PublicHealthStatusCount>, DataLayerError> {
        if endpoint_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut rows = sqlx::query(COUNT_FINALIZED_STATUSES_BY_ENDPOINT_IDS_SINCE_SQL)
            .bind(endpoint_ids)
            .bind(since_unix_secs as f64)
            .fetch(&self.pool);
        let mut counts = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let entry = {
                let status = RequestCandidateStatus::from_database(
                    row_get::<String>(&row, "status")?.as_str(),
                )?;
                PublicHealthStatusCount {
                    endpoint_id: row_get(&row, "endpoint_id")?,
                    status,
                    count: u64::try_from(row_get::<i64>(&row, "count")?).map_err(|_| {
                        DataLayerError::UnexpectedValue(
                            "public health status count out of range".to_string(),
                        )
                    })?,
                }
            };
            counts.push(entry);
        }
        Ok(counts)
    }

    pub async fn aggregate_finalized_timeline_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        until_unix_secs: u64,
        segments: u32,
    ) -> Result<Vec<PublicHealthTimelineBucket>, DataLayerError> {
        if endpoint_ids.is_empty() || segments == 0 || until_unix_secs < since_unix_secs {
            return Ok(Vec::new());
        }

        let span_seconds = until_unix_secs.saturating_sub(since_unix_secs);
        let segment_seconds = if span_seconds == 0 {
            1.0
        } else {
            (span_seconds as f64) / (segments as f64)
        };

        let mut rows = sqlx::query(AGGREGATE_FINALIZED_TIMELINE_BY_ENDPOINT_IDS_SINCE_SQL)
            .bind(endpoint_ids)
            .bind(since_unix_secs as f64)
            .bind(until_unix_secs as f64)
            .bind(segment_seconds)
            .fetch(&self.pool);
        let mut buckets = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let bucket = {
                let raw_segment_idx = row_get::<i64>(&row, "segment_idx")?;
                let segment_idx = if raw_segment_idx < 0 {
                    0
                } else {
                    u32::try_from(raw_segment_idx).map_err(|_| {
                        DataLayerError::UnexpectedValue(format!(
                            "public health segment idx out of range: {raw_segment_idx}"
                        ))
                    })?
                }
                .min(segments.saturating_sub(1));

                PublicHealthTimelineBucket {
                    endpoint_id: row_get(&row, "endpoint_id")?,
                    segment_idx,
                    total_count: u64::try_from(row_get::<i64>(&row, "total_count")?).map_err(
                        |_| {
                            DataLayerError::UnexpectedValue(
                                "public health total_count out of range".to_string(),
                            )
                        },
                    )?,
                    success_count: u64::try_from(row_get::<i64>(&row, "success_count")?).map_err(
                        |_| {
                            DataLayerError::UnexpectedValue(
                                "public health success_count out of range".to_string(),
                            )
                        },
                    )?,
                    failed_count: u64::try_from(row_get::<i64>(&row, "failed_count")?).map_err(
                        |_| {
                            DataLayerError::UnexpectedValue(
                                "public health failed_count out of range".to_string(),
                            )
                        },
                    )?,
                    min_created_at_unix_ms: row_get::<Option<i64>>(&row, "min_created_at_unix_ms")?
                        .map(|value| {
                            u64::try_from(value).map_err(|_| {
                                DataLayerError::UnexpectedValue(format!(
                                    "public health min_created_at_unix_ms out of range: {value}"
                                ))
                            })
                        })
                        .transpose()?,
                    max_created_at_unix_ms: row_get::<Option<i64>>(&row, "max_created_at_unix_ms")?
                        .map(|value| {
                            u64::try_from(value).map_err(|_| {
                                DataLayerError::UnexpectedValue(format!(
                                    "public health max_created_at_unix_ms out of range: {value}"
                                ))
                            })
                        })
                        .transpose()?,
                }
            };
            buckets.push(bucket);
        }
        Ok(buckets)
    }

    pub async fn upsert(
        &self,
        candidate: UpsertRequestCandidateRecord,
    ) -> Result<StoredRequestCandidate, DataLayerError> {
        candidate.validate()?;
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let row = sqlx::query(UPSERT_SQL)
                        .bind(if candidate.id.trim().is_empty() {
                            Uuid::new_v4().to_string()
                        } else {
                            candidate.id.clone()
                        })
                        .bind(&candidate.request_id)
                        .bind(&candidate.user_id)
                        .bind(&candidate.api_key_id)
                        .bind(&candidate.username)
                        .bind(&candidate.api_key_name)
                        .bind(to_i32(candidate.candidate_index)?)
                        .bind(to_i32(candidate.retry_index)?)
                        .bind(&candidate.provider_id)
                        .bind(&candidate.endpoint_id)
                        .bind(&candidate.key_id)
                        .bind(status_to_database(candidate.status))
                        .bind(&candidate.skip_reason)
                        .bind(candidate.is_cached)
                        .bind(candidate.status_code.map(i32::from))
                        .bind(&candidate.error_type)
                        .bind(&candidate.error_message)
                        .bind(candidate.latency_ms.map(to_i32_u64).transpose()?)
                        .bind(candidate.concurrent_requests.map(to_i32).transpose()?)
                        .bind(&candidate.extra_data)
                        .bind(&candidate.required_capabilities)
                        .bind(candidate.created_at_unix_ms.map(|value| value as f64))
                        .bind(candidate.started_at_unix_ms.map(|value| value as f64))
                        .bind(candidate.finished_at_unix_ms.map(|value| value as f64))
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    map_request_candidate_row(&row)
                }) as BoxFuture<'_, Result<StoredRequestCandidate, DataLayerError>>
            })
            .await
    }

    pub async fn delete_created_before(
        &self,
        created_before_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        if limit == 0 {
            return Ok(0);
        }

        let result = sqlx::query(DELETE_CREATED_BEFORE_SQL)
            .bind(created_before_unix_secs as f64)
            .bind(i64::try_from(limit).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid request candidate delete limit: {limit}"
                ))
            })?)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() as usize)
    }
}

#[async_trait]
impl RequestCandidateReadRepository for SqlxRequestCandidateReadRepository {
    async fn list_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        Self::list_by_request_id(self, request_id).await
    }

    async fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        Self::list_recent(self, limit).await
    }

    async fn list_finalized_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        Self::list_finalized_by_endpoint_ids_since(self, endpoint_ids, since_unix_secs, limit).await
    }

    async fn list_by_provider_id(
        &self,
        provider_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        Self::list_by_provider_id(self, provider_id, limit).await
    }

    async fn count_finalized_statuses_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
    ) -> Result<Vec<PublicHealthStatusCount>, DataLayerError> {
        Self::count_finalized_statuses_by_endpoint_ids_since(self, endpoint_ids, since_unix_secs)
            .await
    }

    async fn aggregate_finalized_timeline_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        until_unix_secs: u64,
        segments: u32,
    ) -> Result<Vec<PublicHealthTimelineBucket>, DataLayerError> {
        Self::aggregate_finalized_timeline_by_endpoint_ids_since(
            self,
            endpoint_ids,
            since_unix_secs,
            until_unix_secs,
            segments,
        )
        .await
    }
}

#[async_trait]
impl RequestCandidateWriteRepository for SqlxRequestCandidateReadRepository {
    async fn upsert(
        &self,
        candidate: UpsertRequestCandidateRecord,
    ) -> Result<StoredRequestCandidate, DataLayerError> {
        Self::upsert(self, candidate).await
    }

    async fn delete_created_before(
        &self,
        created_before_unix_secs: u64,
        limit: usize,
    ) -> Result<usize, DataLayerError> {
        Self::delete_created_before(self, created_before_unix_secs, limit).await
    }
}

async fn collect_query_rows<T, S>(
    mut rows: S,
    map_row: fn(&PgRow) -> Result<T, DataLayerError>,
) -> Result<Vec<T>, DataLayerError>
where
    S: TryStream<Ok = PgRow, Error = sqlx::Error> + Unpin,
{
    let mut items = Vec::new();
    while let Some(row) = rows.try_next().await.map_postgres_err()? {
        items.push(map_row(&row)?);
    }
    Ok(items)
}

fn map_request_candidate_row(row: &PgRow) -> Result<StoredRequestCandidate, DataLayerError> {
    let status = RequestCandidateStatus::from_database(row_get::<String>(row, "status")?.as_str())?;
    StoredRequestCandidate::new(
        row_get(row, "id")?,
        row_get(row, "request_id")?,
        row_get(row, "user_id")?,
        row_get(row, "api_key_id")?,
        row_get(row, "username")?,
        row_get(row, "api_key_name")?,
        row_get(row, "candidate_index")?,
        row_get(row, "retry_index")?,
        row_get(row, "provider_id")?,
        row_get(row, "endpoint_id")?,
        row_get(row, "key_id")?,
        status,
        row_get(row, "skip_reason")?,
        row_get(row, "is_cached")?,
        row_get(row, "status_code")?,
        row_get(row, "error_type")?,
        row_get(row, "error_message")?,
        row_get(row, "latency_ms")?,
        row_get(row, "concurrent_requests")?,
        row_get(row, "extra_data")?,
        row_get(row, "required_capabilities")?,
        row_get(row, "created_at_unix_ms")?,
        row_get(row, "started_at_unix_ms")?,
        row_get(row, "finished_at_unix_ms")?,
    )
}

fn row_get<T>(row: &PgRow, column: &str) -> Result<T, DataLayerError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_postgres_err()
}

fn status_to_database(status: RequestCandidateStatus) -> &'static str {
    match status {
        RequestCandidateStatus::Available => "available",
        RequestCandidateStatus::Unused => "unused",
        RequestCandidateStatus::Pending => "pending",
        RequestCandidateStatus::Streaming => "streaming",
        RequestCandidateStatus::Success => "success",
        RequestCandidateStatus::Failed => "failed",
        RequestCandidateStatus::Cancelled => "cancelled",
        RequestCandidateStatus::Skipped => "skipped",
    }
}

fn to_i32(value: u32) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("request candidate value out of range: {value}"))
    })
}

fn to_i32_u64(value: u64) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("request candidate value out of range: {value}"))
    })
}

#[cfg(test)]
mod tests {
    use super::{SqlxRequestCandidateReadRepository, UPSERT_SQL};
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};

    #[test]
    fn upsert_sql_does_not_default_missing_or_epoch_created_at_to_epoch() {
        assert!(!UPSERT_SQL.contains("COALESCE($22, 0)"));
        assert!(UPSERT_SQL.contains("WHEN $22 IS NOT NULL AND $22 > 1000.0"));
        assert!(UPSERT_SQL.contains("TO_TIMESTAMP($22 / 1000.0)"));
        assert!(UPSERT_SQL.contains("TO_TIMESTAMP($23 / 1000.0)"));
        assert!(UPSERT_SQL.contains("TO_TIMESTAMP($24 / 1000.0)"));
        assert!(UPSERT_SQL.contains("NOW()"));
        assert!(UPSERT_SQL.contains("request_candidates.created_at <= TO_TIMESTAMP(1)"));
        assert!(UPSERT_SQL.contains("THEN EXCLUDED.created_at"));
    }

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
        let repository = SqlxRequestCandidateReadRepository::new(pool);
        let _ = repository.pool();
        let _ = repository.transaction_runner();
    }
}
