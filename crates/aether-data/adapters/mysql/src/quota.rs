use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, Row};

use aether_data_contracts::repository::quota::{
    ApplyRemoteProviderQuotaOutcome, ApplyRemoteProviderQuotaPatch, ProviderQuotaReadRepository,
    ProviderQuotaWriteRepository, StoredProviderQuotaSnapshot,
};
use aether_data_query::{DialectSql, SelectColumn, SelectQuery, SqlDialect};

use crate::error::SqlResultExt;
use crate::{DataLayerError, MysqlPool};

fn quota_snapshot_select() -> SelectQuery<'static> {
    SelectQuery::new("providers").select_columns([
        SelectColumn::expr("id").alias("provider_id"),
        SelectColumn::expr(
            DialectSql::common("billing_type").with_postgres("CAST(billing_type AS TEXT)"),
        )
        .alias("billing_type"),
        SelectColumn::expr(
            DialectSql::dialect(
                "CAST(monthly_quota_usd AS DOUBLE PRECISION)",
                "CAST(monthly_quota_usd AS REAL)",
            )
            .with_mysql("CAST(monthly_quota_usd AS DOUBLE)"),
        )
        .alias("monthly_quota_usd"),
        SelectColumn::expr(
            DialectSql::dialect(
                "CAST(COALESCE(monthly_used_usd, 0) AS DOUBLE PRECISION)",
                "CAST(COALESCE(monthly_used_usd, 0) AS REAL)",
            )
            .with_mysql("CAST(COALESCE(monthly_used_usd, 0) AS DOUBLE)"),
        )
        .alias("monthly_used_usd"),
        SelectColumn::expr("quota_reset_day"),
        SelectColumn::expr(
            DialectSql::dialect(
                "CAST(EXTRACT(EPOCH FROM quota_last_reset_at) AS BIGINT)",
                "quota_last_reset_at",
            )
            .with_mysql("quota_last_reset_at"),
        )
        .alias("quota_last_reset_at_unix_secs"),
        SelectColumn::expr(
            DialectSql::dialect(
                "CAST(EXTRACT(EPOCH FROM quota_expires_at) AS BIGINT)",
                "quota_expires_at",
            )
            .with_mysql("quota_expires_at"),
        )
        .alias("quota_expires_at_unix_secs"),
        SelectColumn::expr("is_active"),
    ])
}

#[derive(Debug, Clone)]
pub struct MysqlProviderQuotaRepository {
    pool: MysqlPool,
}

impl MysqlProviderQuotaRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProviderQuotaReadRepository for MysqlProviderQuotaRepository {
    async fn find_by_provider_id(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredProviderQuotaSnapshot>, DataLayerError> {
        let mut statement = quota_snapshot_select().statement::<MySql>(SqlDialect::MySql);
        statement.where_eq("id", provider_id.to_string()).limit(1);
        let row = statement
            .finish()
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_row).transpose()
    }

    async fn find_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderQuotaSnapshot>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut statement = quota_snapshot_select().statement::<MySql>(SqlDialect::MySql);
        statement
            .where_in("id", provider_ids)
            .order_by_sql("id ASC");
        let rows = statement
            .finish()
            .build()
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        rows.iter().map(map_row).collect()
    }
}

#[async_trait]
impl ProviderQuotaWriteRepository for MysqlProviderQuotaRepository {
    async fn reset_due(&self, now_unix_secs: u64) -> Result<usize, DataLayerError> {
        let now = i64::try_from(now_unix_secs).map_err(|_| {
            DataLayerError::InvalidInput("provider quota reset timestamp overflow".to_string())
        })?;
        let rows_affected = sqlx::query(
            r#"
UPDATE providers
SET monthly_used_usd = 0,
    quota_last_reset_at = ?,
    updated_at = ?
WHERE billing_type = 'monthly_quota'
  AND is_active = 1
  AND (
    quota_last_reset_at IS NULL
    OR (? - quota_last_reset_at) >= (quota_reset_day * 86400)
  )
"#,
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(usize::try_from(rows_affected).unwrap_or_default())
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
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let row = sqlx::query(
            r#"
SELECT id AS provider_id,
       billing_type,
       CAST(monthly_quota_usd AS DOUBLE) AS monthly_quota_usd,
       CAST(COALESCE(monthly_used_usd, 0) AS DOUBLE) AS monthly_used_usd,
       quota_reset_day,
       quota_last_reset_at AS quota_last_reset_at_unix_secs,
       quota_expires_at AS quota_expires_at_unix_secs,
       is_active
FROM providers
WHERE id = ?
FOR UPDATE
            "#,
        )
        .bind(patch.provider_id.trim())
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?;
        let Some(row) = row else {
            tx.rollback().await.map_sql_err()?;
            return Ok(ApplyRemoteProviderQuotaOutcome::ProviderNotFound);
        };
        let mut stored = map_row(&row)?;
        if stored
            .quota_last_reset_at_unix_secs
            .is_some_and(|start| start >= patch.remote_window_end_unix_secs)
        {
            tx.rollback().await.map_sql_err()?;
            return Ok(ApplyRemoteProviderQuotaOutcome::StaleWindow(stored));
        }
        if patch.was_applied_after_observation(&stored) {
            tx.rollback().await.map_sql_err()?;
            return Ok(ApplyRemoteProviderQuotaOutcome::Applied(stored));
        }
        if patch.usage_changed_after_observation(&stored) {
            tx.rollback().await.map_sql_err()?;
            return Ok(ApplyRemoteProviderQuotaOutcome::ConcurrentModification(
                stored,
            ));
        }
        patch.apply_to_snapshot(&mut stored);
        sqlx::query(
            r#"
UPDATE providers
SET billing_type = ?,
    monthly_quota_usd = ?,
    monthly_used_usd = ?,
    quota_reset_day = ?,
    quota_last_reset_at = ?,
    quota_expires_at = ?,
    updated_at = ?
WHERE id = ?
            "#,
        )
        .bind(&stored.billing_type)
        .bind(stored.monthly_quota_usd)
        .bind(stored.monthly_used_usd)
        .bind(stored.quota_reset_day.map(|days| days as i64))
        .bind(window_start)
        .bind(expires_at)
        .bind(chrono::Utc::now().timestamp().max(0))
        .bind(patch.provider_id.trim())
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        tx.commit().await.map_sql_err()?;
        Ok(ApplyRemoteProviderQuotaOutcome::Applied(stored))
    }
}

fn map_row(row: &MySqlRow) -> Result<StoredProviderQuotaSnapshot, DataLayerError> {
    StoredProviderQuotaSnapshot::new(
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("billing_type").map_sql_err()?,
        row.try_get("monthly_quota_usd").map_sql_err()?,
        row.try_get("monthly_used_usd").map_sql_err()?,
        row.try_get("quota_reset_day").map_sql_err()?,
        row.try_get("quota_last_reset_at_unix_secs").map_sql_err()?,
        row.try_get("quota_expires_at_unix_secs").map_sql_err()?,
        row.try_get("is_active").map_sql_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::{quota_snapshot_select, MysqlProviderQuotaRepository};
    use aether_data_query::SqlDialect;

    #[test]
    fn quota_projection_renders_for_mysql() {
        let sql = quota_snapshot_select().render(SqlDialect::MySql);

        assert!(sql.contains("id AS `provider_id`"));
        assert!(sql.contains("CAST(monthly_quota_usd AS DOUBLE) AS `monthly_quota_usd`"));
        assert!(sql.contains("quota_last_reset_at AS `quota_last_reset_at_unix_secs`"));
    }

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlProviderQuotaRepository::new(pool);
    }
}
