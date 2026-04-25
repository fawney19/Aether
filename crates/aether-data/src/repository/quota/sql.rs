use async_trait::async_trait;
use sqlx::{PgPool, Row};

use super::{
    ProviderQuotaReadRepository, ProviderQuotaWriteRepository, StoredProviderQuotaSnapshot,
};
use crate::{error::SqlxResultExt, DataLayerError};

const FIND_BY_PROVIDER_ID_SQL: &str = r#"
SELECT
  id AS provider_id,
  CAST(billing_type AS TEXT) AS billing_type,
  CAST(monthly_quota_usd AS DOUBLE PRECISION) AS monthly_quota_usd,
  CAST(COALESCE(monthly_used_usd, 0) AS DOUBLE PRECISION) AS monthly_used_usd,
  quota_reset_day,
  CAST(EXTRACT(EPOCH FROM quota_last_reset_at) AS BIGINT) AS quota_last_reset_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM quota_expires_at) AS BIGINT) AS quota_expires_at_unix_secs,
  is_active
FROM providers
WHERE id = $1
LIMIT 1
"#;

const FIND_BY_PROVIDER_IDS_SQL: &str = r#"
SELECT
  id AS provider_id,
  CAST(billing_type AS TEXT) AS billing_type,
  CAST(monthly_quota_usd AS DOUBLE PRECISION) AS monthly_quota_usd,
  CAST(COALESCE(monthly_used_usd, 0) AS DOUBLE PRECISION) AS monthly_used_usd,
  quota_reset_day,
  CAST(EXTRACT(EPOCH FROM quota_last_reset_at) AS BIGINT) AS quota_last_reset_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM quota_expires_at) AS BIGINT) AS quota_expires_at_unix_secs,
  is_active
FROM providers
WHERE id = ANY($1::TEXT[])
ORDER BY id ASC
"#;

const RESET_DUE_SQL: &str = r#"
UPDATE providers
SET
  monthly_used_usd = 0,
  quota_last_reset_at = TO_TIMESTAMP($1::double precision),
  updated_at = NOW()
WHERE
  billing_type = 'monthly_quota'
  AND is_active = TRUE
  AND (
    quota_last_reset_at IS NULL
    OR (EXTRACT(EPOCH FROM TO_TIMESTAMP($1::double precision)) - EXTRACT(EPOCH FROM quota_last_reset_at)) >= (quota_reset_day * 86400)
  )
"#;

#[derive(Debug, Clone)]
pub struct SqlxProviderQuotaRepository {
    pool: PgPool,
}

impl SqlxProviderQuotaRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProviderQuotaReadRepository for SqlxProviderQuotaRepository {
    async fn find_by_provider_id(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredProviderQuotaSnapshot>, DataLayerError> {
        let row = sqlx::query(FIND_BY_PROVIDER_ID_SQL)
            .bind(provider_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_row).transpose()
    }

    async fn find_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderQuotaSnapshot>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        sqlx::query(FIND_BY_PROVIDER_IDS_SQL)
            .bind(provider_ids)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?
            .iter()
            .map(map_row)
            .collect()
    }
}

#[async_trait]
impl ProviderQuotaWriteRepository for SqlxProviderQuotaRepository {
    async fn reset_due(&self, now_unix_secs: u64) -> Result<usize, DataLayerError> {
        let result = sqlx::query(RESET_DUE_SQL)
            .bind(i64::try_from(now_unix_secs).map_err(|_| {
                DataLayerError::InvalidInput("provider quota reset timestamp overflow".to_string())
            })?)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() as usize)
    }
}

fn map_row(row: &sqlx::postgres::PgRow) -> Result<StoredProviderQuotaSnapshot, DataLayerError> {
    StoredProviderQuotaSnapshot::new(
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("billing_type").map_postgres_err()?,
        row.try_get("monthly_quota_usd").map_postgres_err()?,
        row.try_get("monthly_used_usd").map_postgres_err()?,
        row.try_get("quota_reset_day").map_postgres_err()?,
        row.try_get("quota_last_reset_at_unix_secs")
            .map_postgres_err()?,
        row.try_get("quota_expires_at_unix_secs")
            .map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::SqlxProviderQuotaRepository;
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
        let _repository = SqlxProviderQuotaRepository::new(pool);
    }
}
