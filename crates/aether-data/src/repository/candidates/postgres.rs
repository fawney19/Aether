use async_trait::async_trait;
use futures_util::{future::BoxFuture, stream::TryStream, TryStreamExt};
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::{
    PublicHealthStatusCount, PublicHealthTimelineBucket, RequestCandidateReadRepository,
    RequestCandidateStatus, RequestCandidateWriteRepository, StoredRequestCandidate,
    UpsertRequestCandidateRecord,
};
use crate::driver::postgres::PostgresTransaction;
use crate::driver::postgres::PostgresTransactionRunner;
use crate::{error::SqlxResultExt, DataLayerError};
use aether_data_query::{push_eq, push_in, push_limit, WhereClause};

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
  status = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.status
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.status
    ELSE EXCLUDED.status
  END,
  skip_reason = COALESCE(EXCLUDED.skip_reason, request_candidates.skip_reason),
  is_cached = COALESCE($14, request_candidates.is_cached),
  status_code = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.status_code
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.status_code
    ELSE COALESCE(EXCLUDED.status_code, request_candidates.status_code)
  END,
  error_type = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.error_type
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.error_type
    ELSE COALESCE(EXCLUDED.error_type, request_candidates.error_type)
  END,
  error_message = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.error_message
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.error_message
    ELSE COALESCE(EXCLUDED.error_message, request_candidates.error_message)
  END,
  latency_ms = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.latency_ms
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.latency_ms
    ELSE COALESCE(EXCLUDED.latency_ms, request_candidates.latency_ms)
  END,
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
  finished_at = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.finished_at
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.finished_at
    ELSE COALESCE(EXCLUDED.finished_at, request_candidates.finished_at)
  END
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

const UPSERT_CONFLICT_SQL: &str = r#"
ON CONFLICT (request_id, candidate_index, retry_index)
DO UPDATE SET
  user_id = COALESCE(EXCLUDED.user_id, request_candidates.user_id),
  api_key_id = COALESCE(EXCLUDED.api_key_id, request_candidates.api_key_id),
  username = COALESCE(EXCLUDED.username, request_candidates.username),
  api_key_name = COALESCE(EXCLUDED.api_key_name, request_candidates.api_key_name),
  provider_id = COALESCE(EXCLUDED.provider_id, request_candidates.provider_id),
  endpoint_id = COALESCE(EXCLUDED.endpoint_id, request_candidates.endpoint_id),
  key_id = COALESCE(EXCLUDED.key_id, request_candidates.key_id),
  status = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.status
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.status
    ELSE EXCLUDED.status
  END,
  skip_reason = COALESCE(EXCLUDED.skip_reason, request_candidates.skip_reason),
  is_cached = COALESCE(EXCLUDED.is_cached, request_candidates.is_cached),
  status_code = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.status_code
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.status_code
    ELSE COALESCE(EXCLUDED.status_code, request_candidates.status_code)
  END,
  error_type = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.error_type
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.error_type
    ELSE COALESCE(EXCLUDED.error_type, request_candidates.error_type)
  END,
  error_message = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.error_message
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.error_message
    ELSE COALESCE(EXCLUDED.error_message, request_candidates.error_message)
  END,
  latency_ms = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.latency_ms
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.latency_ms
    ELSE COALESCE(EXCLUDED.latency_ms, request_candidates.latency_ms)
  END,
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
  finished_at = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.finished_at
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.finished_at
    ELSE COALESCE(EXCLUDED.finished_at, request_candidates.finished_at)
  END
"#;

const UPSERT_CONFLICT_INHERIT_IS_CACHED_SQL: &str = r#"
ON CONFLICT (request_id, candidate_index, retry_index)
DO UPDATE SET
  user_id = COALESCE(EXCLUDED.user_id, request_candidates.user_id),
  api_key_id = COALESCE(EXCLUDED.api_key_id, request_candidates.api_key_id),
  username = COALESCE(EXCLUDED.username, request_candidates.username),
  api_key_name = COALESCE(EXCLUDED.api_key_name, request_candidates.api_key_name),
  provider_id = COALESCE(EXCLUDED.provider_id, request_candidates.provider_id),
  endpoint_id = COALESCE(EXCLUDED.endpoint_id, request_candidates.endpoint_id),
  key_id = COALESCE(EXCLUDED.key_id, request_candidates.key_id),
  status = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.status
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.status
    ELSE EXCLUDED.status
  END,
  skip_reason = COALESCE(EXCLUDED.skip_reason, request_candidates.skip_reason),
  is_cached = request_candidates.is_cached,
  status_code = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.status_code
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.status_code
    ELSE COALESCE(EXCLUDED.status_code, request_candidates.status_code)
  END,
  error_type = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.error_type
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.error_type
    ELSE COALESCE(EXCLUDED.error_type, request_candidates.error_type)
  END,
  error_message = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.error_message
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.error_message
    ELSE COALESCE(EXCLUDED.error_message, request_candidates.error_message)
  END,
  latency_ms = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.latency_ms
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.latency_ms
    ELSE COALESCE(EXCLUDED.latency_ms, request_candidates.latency_ms)
  END,
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
  finished_at = CASE
    WHEN request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')
      AND EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')
      THEN request_candidates.finished_at
    WHEN request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'
      THEN request_candidates.finished_at
    ELSE COALESCE(EXCLUDED.finished_at, request_candidates.finished_at)
  END
"#;

const UPSERT_MANY_PREFIX_SQL: &str = r#"
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
"#;

const MAX_POSTGRES_REQUEST_CANDIDATE_UPSERT_ROWS: usize = 1_000;

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
        let mut builder = QueryBuilder::<Postgres>::new(candidate_columns());
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "request_id",
            request_id.to_string(),
        );
        builder.push(" ORDER BY candidate_index ASC, retry_index ASC, created_at ASC");
        collect_query_rows(builder.build().fetch(&self.pool), map_request_candidate_row).await
    }

    pub async fn list_attempted_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(candidate_columns());
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "request_id",
            request_id.to_string(),
        );
        builder.push(
            " AND (status IN ('streaming', 'success', 'failed', 'cancelled') \
             OR (status = 'pending' AND started_at IS NOT NULL)) \
             ORDER BY candidate_index ASC, retry_index ASC, created_at ASC",
        );
        collect_query_rows(builder.build().fetch(&self.pool), map_request_candidate_row).await
    }

    pub async fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new(candidate_columns());
        builder.push(" ORDER BY created_at DESC");
        push_limit(
            &mut builder,
            i64::try_from(limit).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid recent request candidate limit: {limit}"
                ))
            })?,
        );
        collect_query_rows(builder.build().fetch(&self.pool), map_request_candidate_row).await
    }

    pub async fn list_by_provider_id(
        &self,
        provider_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new(candidate_columns());
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "provider_id",
            provider_id.to_string(),
        );
        builder.push(" ORDER BY created_at DESC");
        push_limit(
            &mut builder,
            i64::try_from(limit).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid provider request candidate limit: {limit}"
                ))
            })?,
        );
        collect_query_rows(builder.build().fetch(&self.pool), map_request_candidate_row).await
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

        let mut builder = QueryBuilder::<Postgres>::new(candidate_columns());
        let mut where_clause = WhereClause::new();
        push_in(&mut builder, &mut where_clause, "endpoint_id", endpoint_ids);
        builder
            .push(" AND created_at >= TO_TIMESTAMP(")
            .push_bind(since_unix_secs as f64)
            .push(") AND status IN ('success', 'failed', 'skipped') ORDER BY created_at DESC");
        push_limit(
            &mut builder,
            i64::try_from(limit).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid finalized request candidate limit: {limit}"
                ))
            })?,
        );
        collect_query_rows(builder.build().fetch(&self.pool), map_request_candidate_row).await
    }

    pub async fn count_finalized_statuses_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
    ) -> Result<Vec<PublicHealthStatusCount>, DataLayerError> {
        if endpoint_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT endpoint_id, status, COUNT(id) AS count FROM request_candidates",
        );
        let mut where_clause = WhereClause::new();
        push_in(&mut builder, &mut where_clause, "endpoint_id", endpoint_ids);
        builder
            .push(" AND created_at >= TO_TIMESTAMP(")
            .push_bind(since_unix_secs as f64)
            .push(") AND status IN ('success', 'failed', 'skipped') GROUP BY endpoint_id, status");
        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        rows.iter()
            .map(|row| {
                Ok(PublicHealthStatusCount {
                    endpoint_id: row_get(row, "endpoint_id")?,
                    status: RequestCandidateStatus::from_database(
                        row_get::<String>(row, "status")?.as_str(),
                    )?,
                    count: u64::try_from(row_get::<i64>(row, "count")?).map_err(|_| {
                        DataLayerError::UnexpectedValue(
                            "public health status count out of range".to_string(),
                        )
                    })?,
                })
            })
            .collect()
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

    pub async fn upsert_many(
        &self,
        candidates: Vec<UpsertRequestCandidateRecord>,
    ) -> Result<usize, DataLayerError> {
        if candidates.is_empty() {
            return Ok(0);
        }
        let rows = candidates
            .into_iter()
            .map(BatchUpsertRequestCandidateRow::try_from)
            .collect::<Result<Vec<_>, _>>()?;

        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let mut persisted = 0usize;
                    for ordered_batch in split_request_candidate_upsert_batches(rows) {
                        let (explicit_is_cached, inherited_is_cached): (Vec<_>, Vec<_>) =
                            ordered_batch
                                .into_iter()
                                .partition(|row| row.is_cached.is_some());

                        persisted = persisted.saturating_add(
                            execute_partitioned_upsert_many_batch(tx, &explicit_is_cached, true)
                                .await?,
                        );
                        persisted = persisted.saturating_add(
                            execute_partitioned_upsert_many_batch(tx, &inherited_is_cached, false)
                                .await?,
                        );
                    }

                    Ok(persisted)
                }) as BoxFuture<'_, Result<usize, DataLayerError>>
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

async fn execute_partitioned_upsert_many_batch(
    tx: &mut PostgresTransaction,
    rows: &[BatchUpsertRequestCandidateRow],
    overwrite_is_cached: bool,
) -> Result<usize, DataLayerError> {
    let mut persisted = 0usize;
    for chunk in rows.chunks(MAX_POSTGRES_REQUEST_CANDIDATE_UPSERT_ROWS) {
        persisted = persisted
            .saturating_add(execute_upsert_many_batch(tx, chunk, overwrite_is_cached).await?);
    }
    Ok(persisted)
}

#[derive(Debug)]
struct BatchUpsertRequestCandidateRow {
    id: String,
    request_id: String,
    user_id: Option<String>,
    api_key_id: Option<String>,
    username: Option<String>,
    api_key_name: Option<String>,
    candidate_index: i32,
    retry_index: i32,
    provider_id: Option<String>,
    endpoint_id: Option<String>,
    key_id: Option<String>,
    status: &'static str,
    skip_reason: Option<String>,
    is_cached: Option<bool>,
    status_code: Option<i32>,
    error_type: Option<String>,
    error_message: Option<String>,
    latency_ms: Option<i32>,
    concurrent_requests: Option<i32>,
    extra_data: Option<serde_json::Value>,
    required_capabilities: Option<serde_json::Value>,
    created_at_unix_ms: Option<f64>,
    started_at_unix_ms: Option<f64>,
    finished_at_unix_ms: Option<f64>,
}

impl TryFrom<UpsertRequestCandidateRecord> for BatchUpsertRequestCandidateRow {
    type Error = DataLayerError;

    fn try_from(candidate: UpsertRequestCandidateRecord) -> Result<Self, Self::Error> {
        candidate.validate()?;
        Ok(Self {
            id: if candidate.id.trim().is_empty() {
                Uuid::new_v4().to_string()
            } else {
                candidate.id
            },
            request_id: candidate.request_id,
            user_id: candidate.user_id,
            api_key_id: candidate.api_key_id,
            username: candidate.username,
            api_key_name: candidate.api_key_name,
            candidate_index: to_i32(candidate.candidate_index)?,
            retry_index: to_i32(candidate.retry_index)?,
            provider_id: candidate.provider_id,
            endpoint_id: candidate.endpoint_id,
            key_id: candidate.key_id,
            status: status_to_database(candidate.status),
            skip_reason: candidate.skip_reason,
            is_cached: candidate.is_cached,
            status_code: candidate.status_code.map(i32::from),
            error_type: candidate.error_type,
            error_message: candidate.error_message,
            latency_ms: candidate.latency_ms.map(to_i32_u64).transpose()?,
            concurrent_requests: candidate.concurrent_requests.map(to_i32).transpose()?,
            extra_data: candidate.extra_data,
            required_capabilities: candidate.required_capabilities,
            created_at_unix_ms: candidate.created_at_unix_ms.map(|value| value as f64),
            started_at_unix_ms: candidate.started_at_unix_ms.map(|value| value as f64),
            finished_at_unix_ms: candidate.finished_at_unix_ms.map(|value| value as f64),
        })
    }
}

async fn execute_upsert_many_batch(
    tx: &mut PostgresTransaction,
    rows: &[BatchUpsertRequestCandidateRow],
    overwrite_is_cached: bool,
) -> Result<usize, DataLayerError> {
    if rows.is_empty() {
        return Ok(0);
    }

    let mut builder = QueryBuilder::<Postgres>::new(UPSERT_MANY_PREFIX_SQL);
    builder.push_values(rows, |mut values, row| {
        values
            .push_bind(row.id.clone())
            .push_bind(row.request_id.clone())
            .push_bind(row.user_id.clone())
            .push_bind(row.api_key_id.clone())
            .push_bind(row.username.clone())
            .push_bind(row.api_key_name.clone())
            .push_bind(row.candidate_index)
            .push_bind(row.retry_index)
            .push_bind(row.provider_id.clone())
            .push_bind(row.endpoint_id.clone())
            .push_bind(row.key_id.clone())
            .push_bind(row.status)
            .push_bind(row.skip_reason.clone())
            .push_bind(row.is_cached.unwrap_or(false))
            .push_bind(row.status_code)
            .push_bind(row.error_type.clone())
            .push_bind(row.error_message.clone())
            .push_bind(row.latency_ms)
            .push_bind(row.concurrent_requests)
            .push_bind(row.extra_data.clone())
            .push_bind(row.required_capabilities.clone())
            .push("COALESCE(CASE WHEN ")
            .push_bind_unseparated(row.created_at_unix_ms)
            .push_unseparated(" IS NOT NULL AND ")
            .push_bind_unseparated(row.created_at_unix_ms)
            .push_unseparated(" > 1000.0 THEN TO_TIMESTAMP(")
            .push_bind_unseparated(row.created_at_unix_ms)
            .push_unseparated(" / 1000.0) END, TO_TIMESTAMP(")
            .push_bind_unseparated(row.started_at_unix_ms)
            .push_unseparated(" / 1000.0), TO_TIMESTAMP(")
            .push_bind_unseparated(row.finished_at_unix_ms)
            .push_unseparated(" / 1000.0), NOW())")
            .push("TO_TIMESTAMP(")
            .push_bind_unseparated(row.started_at_unix_ms)
            .push_unseparated(" / 1000.0)")
            .push("TO_TIMESTAMP(")
            .push_bind_unseparated(row.finished_at_unix_ms)
            .push_unseparated(" / 1000.0)");
    });
    builder.push(upsert_many_conflict_sql(overwrite_is_cached));
    let result = builder
        .build()
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(rows.len()))
}

fn split_request_candidate_upsert_batches(
    rows: Vec<BatchUpsertRequestCandidateRow>,
) -> Vec<Vec<BatchUpsertRequestCandidateRow>> {
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut seen = std::collections::HashSet::<(String, i32, i32)>::new();

    for row in rows {
        let key = (row.request_id.clone(), row.candidate_index, row.retry_index);
        if seen.contains(&key) && !current.is_empty() {
            batches.push(current);
            current = Vec::new();
            seen.clear();
        }
        seen.insert(key);
        current.push(row);
    }

    if !current.is_empty() {
        batches.push(current);
    }

    batches
}

fn upsert_many_conflict_sql(overwrite_is_cached: bool) -> &'static str {
    if overwrite_is_cached {
        UPSERT_CONFLICT_SQL
    } else {
        UPSERT_CONFLICT_INHERIT_IS_CACHED_SQL
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

    async fn list_attempted_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<StoredRequestCandidate>, DataLayerError> {
        Self::list_attempted_by_request_id(self, request_id).await
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

    async fn upsert_many(
        &self,
        candidates: Vec<UpsertRequestCandidateRecord>,
    ) -> Result<usize, DataLayerError> {
        Self::upsert_many(self, candidates).await
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

fn candidate_columns() -> &'static str {
    LIST_BY_REQUEST_ID_SQL
        .split_once("WHERE request_id = $1")
        .map(|(prefix, _)| prefix)
        .unwrap_or(LIST_BY_REQUEST_ID_SQL)
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
    use super::{
        SqlxRequestCandidateReadRepository, UPSERT_CONFLICT_INHERIT_IS_CACHED_SQL,
        UPSERT_CONFLICT_SQL, UPSERT_SQL,
    };
    use crate::driver::postgres::{PostgresPoolConfig, PostgresPoolFactory};

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

    #[test]
    fn upsert_sql_keeps_terminal_candidate_state_when_lifecycle_events_arrive_late() {
        for sql in [
            UPSERT_SQL,
            UPSERT_CONFLICT_SQL,
            UPSERT_CONFLICT_INHERIT_IS_CACHED_SQL,
        ] {
            assert!(sql.contains(
                "request_candidates.status IN ('success', 'failed', 'cancelled', 'skipped')"
            ));
            assert!(
                sql.contains("EXCLUDED.status IN ('available', 'unused', 'pending', 'streaming')")
            );
            assert!(sql.contains(
                "request_candidates.status = 'streaming' AND EXCLUDED.status = 'pending'"
            ));
            assert!(sql.contains("THEN request_candidates.status"));
            assert!(sql.contains("THEN request_candidates.latency_ms"));
        }
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
