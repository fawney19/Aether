use async_trait::async_trait;
use sqlx::{PgPool, Postgres, Row};

use aether_data_contracts::repository::quota::{
    ApplyRemoteProviderQuotaOutcome, ApplyRemoteProviderQuotaPatch, ProviderQuotaReadRepository,
    ProviderQuotaWriteRepository, StoredProviderQuotaSnapshot,
};
use aether_data_query::{DialectSql, SelectColumn, SelectQuery, SqlDialect};

use crate::{error::SqlxResultExt, DataLayerError};

fn quota_snapshot_select() -> SelectQuery<'static> {
    SelectQuery::new("providers").select_columns([
        SelectColumn::expr("id").alias("provider_id"),
        SelectColumn::expr(
            DialectSql::common("billing_type").with_postgres("CAST(billing_type AS TEXT)"),
        )
        .alias("billing_type"),
        SelectColumn::expr(DialectSql::dialect(
            "CAST(monthly_quota_usd AS DOUBLE PRECISION)",
            "CAST(monthly_quota_usd AS REAL)",
        ))
        .alias("monthly_quota_usd"),
        SelectColumn::expr(DialectSql::dialect(
            "CAST(COALESCE(monthly_used_usd, 0) AS DOUBLE PRECISION)",
            "CAST(COALESCE(monthly_used_usd, 0) AS REAL)",
        ))
        .alias("monthly_used_usd"),
        SelectColumn::expr("quota_reset_day"),
        SelectColumn::expr(DialectSql::dialect(
            "CAST(EXTRACT(EPOCH FROM quota_last_reset_at) AS BIGINT)",
            "quota_last_reset_at",
        ))
        .alias("quota_last_reset_at_unix_secs"),
        SelectColumn::expr(DialectSql::dialect(
            "CAST(EXTRACT(EPOCH FROM quota_expires_at) AS BIGINT)",
            "quota_expires_at",
        ))
        .alias("quota_expires_at_unix_secs"),
        SelectColumn::expr("is_active"),
    ])
}

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
        let mut statement = quota_snapshot_select().statement::<Postgres>(SqlDialect::Postgres);
        statement.where_eq("id", provider_id.to_string()).limit(1);
        let row = statement
            .finish()
            .build()
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

        let mut statement = quota_snapshot_select().statement::<Postgres>(SqlDialect::Postgres);
        statement
            .where_in("id", provider_ids)
            .order_by_sql("id ASC");
        statement
            .finish()
            .build()
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

    async fn apply_remote_provider_quota(
        &self,
        patch: &ApplyRemoteProviderQuotaPatch,
    ) -> Result<ApplyRemoteProviderQuotaOutcome, DataLayerError> {
        patch.validate()?;
        let window_start = i64::try_from(patch.remote_window_start_unix_secs).map_err(|_| {
            DataLayerError::InvalidInput("remote quota window is too large".to_string())
        })?;
        let expires_at = patch
            .quota_expires_at_unix_secs
            .map(i64::try_from)
            .transpose()
            .map_err(|_| {
                DataLayerError::InvalidInput("remote quota expiry is too large".to_string())
            })?;
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let row = sqlx::query(
            r#"
SELECT id AS provider_id,
       CAST(billing_type AS TEXT) AS billing_type,
       CAST(monthly_quota_usd AS DOUBLE PRECISION) AS monthly_quota_usd,
       CAST(COALESCE(monthly_used_usd, 0) AS DOUBLE PRECISION) AS monthly_used_usd,
       quota_reset_day,
       CAST(EXTRACT(EPOCH FROM quota_last_reset_at) AS BIGINT) AS quota_last_reset_at_unix_secs,
       CAST(EXTRACT(EPOCH FROM quota_expires_at) AS BIGINT) AS quota_expires_at_unix_secs,
       is_active
FROM providers
WHERE id = $1
FOR UPDATE
            "#,
        )
        .bind(patch.provider_id.trim())
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        let Some(row) = row else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(ApplyRemoteProviderQuotaOutcome::ProviderNotFound);
        };
        let mut stored = map_row(&row)?;
        if stored
            .quota_last_reset_at_unix_secs
            .is_some_and(|start| start >= patch.remote_window_end_unix_secs)
        {
            tx.rollback().await.map_postgres_err()?;
            return Ok(ApplyRemoteProviderQuotaOutcome::StaleWindow(stored));
        }
        if patch.was_applied_after_observation(&stored) {
            tx.rollback().await.map_postgres_err()?;
            return Ok(ApplyRemoteProviderQuotaOutcome::Applied(stored));
        }
        if patch.usage_changed_after_observation(&stored) {
            tx.rollback().await.map_postgres_err()?;
            return Ok(ApplyRemoteProviderQuotaOutcome::ConcurrentModification(
                stored,
            ));
        }
        patch.apply_to_snapshot(&mut stored);
        sqlx::query(
            r#"
UPDATE providers
SET billing_type = CAST($2 AS providerbillingtype),
    monthly_quota_usd = $3,
    monthly_used_usd = $4,
    quota_reset_day = $5,
    quota_last_reset_at = TO_TIMESTAMP($6::double precision),
    quota_expires_at = CASE
        WHEN $7::bigint IS NULL THEN NULL
        ELSE TO_TIMESTAMP($7::double precision)
    END,
    updated_at = NOW()
WHERE id = $1
            "#,
        )
        .bind(patch.provider_id.trim())
        .bind(&stored.billing_type)
        .bind(stored.monthly_quota_usd)
        .bind(stored.monthly_used_usd)
        .bind(stored.quota_reset_day.map(|days| days as i32))
        .bind(window_start)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        tx.commit().await.map_postgres_err()?;
        Ok(ApplyRemoteProviderQuotaOutcome::Applied(stored))
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
    use crate::{PostgresPoolConfig, PostgresPoolFactory};

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
