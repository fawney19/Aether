use async_trait::async_trait;
use futures_util::future::BoxFuture;
use futures_util::TryStreamExt;
use sqlx::{PgPool, Row};

use super::types::{
    ShadowResultLookupKey, ShadowResultMatchStatus, ShadowResultReadRepository,
    ShadowResultWriteRepository, StoredShadowResult, UpsertShadowResult,
};
use crate::postgres::PostgresTransactionRunner;
use crate::{error::SqlxResultExt, DataLayerError};

const FIND_BY_TRACE_FINGERPRINT_SQL: &str = r#"
SELECT
  trace_id,
  request_fingerprint,
  NULL::TEXT AS request_id,
  route_family,
  route_kind,
  candidate_id,
  rust_result_digest,
  python_result_digest,
  match_status,
  status_code,
  error_message,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM gateway_shadow_results
WHERE trace_id = $1 AND request_fingerprint = $2
LIMIT 1
"#;

const LIST_RECENT_SQL: &str = r#"
SELECT
  trace_id,
  request_fingerprint,
  NULL::TEXT AS request_id,
  route_family,
  route_kind,
  candidate_id,
  rust_result_digest,
  python_result_digest,
  match_status,
  status_code,
  error_message,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
FROM gateway_shadow_results
ORDER BY updated_at DESC
LIMIT $1
"#;

const UPSERT_SQL: &str = r#"
INSERT INTO gateway_shadow_results (
  trace_id,
  request_fingerprint,
  route_family,
  route_kind,
  candidate_id,
  rust_result_digest,
  python_result_digest,
  match_status,
  status_code,
  error_message,
  created_at,
  updated_at
) VALUES (
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
  TO_TIMESTAMP($11::double precision),
  TO_TIMESTAMP($12::double precision)
)
ON CONFLICT (trace_id, request_fingerprint)
DO UPDATE SET
  route_family = EXCLUDED.route_family,
  route_kind = EXCLUDED.route_kind,
  candidate_id = EXCLUDED.candidate_id,
  rust_result_digest = EXCLUDED.rust_result_digest,
  python_result_digest = EXCLUDED.python_result_digest,
  match_status = EXCLUDED.match_status,
  status_code = EXCLUDED.status_code,
  error_message = EXCLUDED.error_message,
  updated_at = EXCLUDED.updated_at
RETURNING
  trace_id,
  request_fingerprint,
  NULL::TEXT AS request_id,
  route_family,
  route_kind,
  candidate_id,
  rust_result_digest,
  python_result_digest,
  match_status,
  status_code,
  error_message,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
"#;

#[derive(Debug, Clone)]
pub struct SqlxShadowResultRepository {
    pool: PgPool,
    tx_runner: PostgresTransactionRunner,
}

impl SqlxShadowResultRepository {
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

    pub async fn find(
        &self,
        key: ShadowResultLookupKey<'_>,
    ) -> Result<Option<StoredShadowResult>, DataLayerError> {
        match key {
            ShadowResultLookupKey::TraceFingerprint {
                trace_id,
                request_fingerprint,
            } => {
                self.find_by_trace_fingerprint(trace_id, request_fingerprint)
                    .await
            }
        }
    }

    pub async fn find_by_trace_fingerprint(
        &self,
        trace_id: &str,
        request_fingerprint: &str,
    ) -> Result<Option<StoredShadowResult>, DataLayerError> {
        let row = sqlx::query(FIND_BY_TRACE_FINGERPRINT_SQL)
            .bind(trace_id)
            .bind(request_fingerprint)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_shadow_result_row).transpose()
    }

    pub async fn list_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredShadowResult>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut rows = sqlx::query(LIST_RECENT_SQL)
            .bind(i64::try_from(limit).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid recent shadow result limit: {limit}"
                ))
            })?)
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_shadow_result_row(&row)?);
        }
        Ok(items)
    }

    pub async fn upsert(
        &self,
        result: UpsertShadowResult,
    ) -> Result<StoredShadowResult, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let row = sqlx::query(UPSERT_SQL)
                        .bind(&result.trace_id)
                        .bind(&result.request_fingerprint)
                        .bind(&result.route_family)
                        .bind(&result.route_kind)
                        .bind(&result.candidate_id)
                        .bind(&result.rust_result_digest)
                        .bind(&result.python_result_digest)
                        .bind(match_status_to_database(result.match_status))
                        .bind(result.status_code.map(i32::from))
                        .bind(&result.error_message)
                        .bind(result.created_at_unix_ms as f64)
                        .bind(result.updated_at_unix_secs as f64)
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    map_shadow_result_row(&row)
                }) as BoxFuture<'_, Result<StoredShadowResult, DataLayerError>>
            })
            .await
    }
}

#[async_trait]
impl ShadowResultReadRepository for SqlxShadowResultRepository {
    async fn find(
        &self,
        key: ShadowResultLookupKey<'_>,
    ) -> Result<Option<StoredShadowResult>, DataLayerError> {
        Self::find(self, key).await
    }

    async fn list_recent(&self, limit: usize) -> Result<Vec<StoredShadowResult>, DataLayerError> {
        Self::list_recent(self, limit).await
    }
}

#[async_trait]
impl ShadowResultWriteRepository for SqlxShadowResultRepository {
    async fn upsert(
        &self,
        result: UpsertShadowResult,
    ) -> Result<StoredShadowResult, DataLayerError> {
        Self::upsert(self, result).await
    }
}

fn match_status_to_database(status: ShadowResultMatchStatus) -> &'static str {
    match status {
        ShadowResultMatchStatus::Pending => "pending",
        ShadowResultMatchStatus::Match => "match",
        ShadowResultMatchStatus::Mismatch => "mismatch",
        ShadowResultMatchStatus::Error => "error",
    }
}

fn map_shadow_result_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredShadowResult, DataLayerError> {
    let match_status = ShadowResultMatchStatus::from_database(
        row.try_get::<String, _>("match_status")
            .map_postgres_err()?
            .as_str(),
    )?;
    StoredShadowResult::new(
        row.try_get("trace_id").map_postgres_err()?,
        row.try_get("request_fingerprint").map_postgres_err()?,
        row.try_get("request_id").map_postgres_err()?,
        row.try_get("route_family").map_postgres_err()?,
        row.try_get("route_kind").map_postgres_err()?,
        row.try_get("candidate_id").map_postgres_err()?,
        row.try_get("rust_result_digest").map_postgres_err()?,
        row.try_get("python_result_digest").map_postgres_err()?,
        match_status,
        row.try_get("status_code").map_postgres_err()?,
        row.try_get("error_message").map_postgres_err()?,
        row.try_get("created_at_unix_ms").map_postgres_err()?,
        row.try_get("updated_at_unix_secs").map_postgres_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::SqlxShadowResultRepository;
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
        let repository = SqlxShadowResultRepository::new(pool);
        let _ = repository.pool();
        let _ = repository.transaction_runner();
    }
}
