use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, Row, Sqlite};

use aether_data_contracts::repository::quota::{
    ApplyRemoteProviderQuotaOutcome, ApplyRemoteProviderQuotaPatch, ProviderQuotaReadRepository,
    ProviderQuotaWriteRepository, StoredProviderQuotaSnapshot,
};
use aether_data_query::{DialectSql, SelectColumn, SelectQuery, SqlDialect};

use crate::error::SqlResultExt;
use crate::{sqlite_optional_real, sqlite_real, DataLayerError, SqlitePool};

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

#[derive(Debug, Clone)]
pub struct SqliteProviderQuotaRepository {
    pool: SqlitePool,
}

impl SqliteProviderQuotaRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProviderQuotaReadRepository for SqliteProviderQuotaRepository {
    async fn find_by_provider_id(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredProviderQuotaSnapshot>, DataLayerError> {
        let mut statement = quota_snapshot_select().statement::<Sqlite>(SqlDialect::Sqlite);
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

        let mut statement = quota_snapshot_select().statement::<Sqlite>(SqlDialect::Sqlite);
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
impl ProviderQuotaWriteRepository for SqliteProviderQuotaRepository {
    async fn reset_due(&self, now_unix_secs: u64) -> Result<usize, DataLayerError> {
        let now = i64::try_from(now_unix_secs).map_err(|_| {
            DataLayerError::InvalidInput("provider quota reset timestamp overflow".to_string())
        })?;
        let rows_affected = sqlx::query(
            r#"
UPDATE providers
SET monthly_used_usd = 0.0,
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
       CAST(monthly_quota_usd AS REAL) AS monthly_quota_usd,
       CAST(COALESCE(monthly_used_usd, 0) AS REAL) AS monthly_used_usd,
       quota_reset_day,
       quota_last_reset_at AS quota_last_reset_at_unix_secs,
       quota_expires_at AS quota_expires_at_unix_secs,
       is_active
FROM providers
WHERE id = ?
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
        let expected_window_start = stored
            .quota_last_reset_at_unix_secs
            .map(i64::try_from)
            .transpose()
            .map_err(|_| {
                DataLayerError::InvalidInput("stored quota window is too large".to_string())
            })?;
        let expected_used_usd = stored.monthly_used_usd;
        patch.apply_to_snapshot(&mut stored);
        let rows_affected = sqlx::query(
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
  AND quota_last_reset_at IS ?
  AND COALESCE(monthly_used_usd, 0) = ?
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
        .bind(expected_window_start)
        .bind(expected_used_usd)
        .execute(&mut *tx)
        .await
        .map_sql_err()?
        .rows_affected();
        if rows_affected == 0 {
            tx.rollback().await.map_sql_err()?;
            let latest = self.find_by_provider_id(patch.provider_id.trim()).await?;
            return ApplyRemoteProviderQuotaOutcome::from_unapplied_row(latest, patch);
        }
        tx.commit().await.map_sql_err()?;
        Ok(ApplyRemoteProviderQuotaOutcome::Applied(stored))
    }
}

fn map_row(row: &SqliteRow) -> Result<StoredProviderQuotaSnapshot, DataLayerError> {
    StoredProviderQuotaSnapshot::new(
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("billing_type").map_sql_err()?,
        sqlite_optional_real(row, "monthly_quota_usd")?,
        sqlite_real(row, "monthly_used_usd")?,
        row.try_get("quota_reset_day").map_sql_err()?,
        row.try_get("quota_last_reset_at_unix_secs").map_sql_err()?,
        row.try_get("quota_expires_at_unix_secs").map_sql_err()?,
        row.try_get("is_active").map_sql_err()?,
    )
}

#[cfg(test)]
mod tests {
    use super::SqliteProviderQuotaRepository;
    use aether_data_contracts::repository::quota::{
        ApplyRemoteProviderQuotaOutcome, ApplyRemoteProviderQuotaPatch,
        ProviderQuotaReadRepository, ProviderQuotaUsageObservation, ProviderQuotaWriteRepository,
    };

    use crate::run_migrations;

    #[tokio::test]
    async fn sqlite_repository_reads_and_resets_provider_quotas() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_provider_quotas(&pool).await;

        let repository = SqliteProviderQuotaRepository::new(pool);
        let quota = repository
            .find_by_provider_id("provider-1")
            .await
            .expect("quota should load")
            .expect("quota should exist");
        assert_eq!(quota.monthly_used_usd, 5.0);

        let quota = repository
            .find_by_provider_id("provider-null-used")
            .await
            .expect("quota with null usage should load")
            .expect("quota with null usage should exist");
        assert_eq!(quota.monthly_used_usd, 0.0);

        let quotas = repository
            .find_by_provider_ids(&["provider-2".to_string(), "provider-1".to_string()])
            .await
            .expect("quotas should load");
        assert_eq!(
            quotas
                .iter()
                .map(|quota| quota.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider-1", "provider-2"]
        );

        let reset = repository
            .reset_due(1_000 + 7 * 24 * 60 * 60)
            .await
            .expect("quota reset should run");
        assert_eq!(reset, 1);
        let quota = repository
            .find_by_provider_id("provider-1")
            .await
            .expect("quota should reload")
            .expect("quota should exist");
        assert_eq!(quota.monthly_used_usd, 0.0);
        assert_eq!(quota.quota_last_reset_at_unix_secs, Some(605_800));

        let initial_remote = ApplyRemoteProviderQuotaPatch {
            provider_id: "provider-1".to_string(),
            billing_type: "monthly_quota".to_string(),
            monthly_quota_usd: Some(100.0),
            remote_monthly_used_usd: 3.0,
            remote_window_start_unix_secs: 700_000,
            remote_window_end_unix_secs: 800_000,
            quota_reset_day: Some(30),
            quota_expires_at_unix_secs: Some(900_000),
            local_usage_observation: Some(ProviderQuotaUsageObservation {
                monthly_used_usd: 0.0,
                quota_last_reset_at_unix_secs: Some(605_800),
            }),
            preserve_local_used_usd: false,
        };
        repository
            .apply_remote_provider_quota(&initial_remote)
            .await
            .expect("remote quota should apply");
        repository
            .apply_remote_provider_quota(&initial_remote)
            .await
            .expect("repeating the same observation must be idempotent");
        assert!(matches!(
            repository
                .apply_remote_provider_quota(&ApplyRemoteProviderQuotaPatch {
                    remote_monthly_used_usd: 4.0,
                    ..initial_remote.clone()
                })
                .await
                .expect("divergent concurrent snapshot should classify"),
            ApplyRemoteProviderQuotaOutcome::ConcurrentModification(_)
        ));
        let quota = repository
            .find_by_provider_id("provider-1")
            .await
            .expect("quota should reload")
            .expect("quota should exist");
        assert_eq!(quota.monthly_used_usd, 3.0);
        repository
            .apply_remote_provider_quota(&ApplyRemoteProviderQuotaPatch {
                remote_monthly_used_usd: 2.0,
                local_usage_observation: Some(ProviderQuotaUsageObservation {
                    monthly_used_usd: 2.0,
                    quota_last_reset_at_unix_secs: Some(700_000),
                }),
                ..initial_remote.clone()
            })
            .await
            .expect("fetch-time local increment should survive reconciliation");
        repository
            .apply_remote_provider_quota(&ApplyRemoteProviderQuotaPatch {
                remote_monthly_used_usd: 2.0,
                local_usage_observation: Some(ProviderQuotaUsageObservation {
                    monthly_used_usd: 3.0,
                    quota_last_reset_at_unix_secs: Some(700_000),
                }),
                ..initial_remote.clone()
            })
            .await
            .expect("next authoritative snapshot should remove the old local estimate");
        let quota = repository
            .find_by_provider_id("provider-1")
            .await
            .expect("quota should reload")
            .expect("quota should exist");
        assert_eq!(quota.monthly_quota_usd, Some(100.0));
        assert_eq!(quota.monthly_used_usd, 2.0);
        assert_eq!(quota.quota_reset_day, Some(30));

        repository
            .apply_remote_provider_quota(&ApplyRemoteProviderQuotaPatch {
                remote_monthly_used_usd: 1.0,
                remote_window_start_unix_secs: 800_000,
                remote_window_end_unix_secs: 900_000,
                local_usage_observation: Some(ProviderQuotaUsageObservation {
                    monthly_used_usd: 2.0,
                    quota_last_reset_at_unix_secs: Some(700_000),
                }),
                ..initial_remote.clone()
            })
            .await
            .expect("new remote window should apply");
        assert!(matches!(
            repository
                .apply_remote_provider_quota(&initial_remote)
                .await
                .expect("stale remote window should classify"),
            ApplyRemoteProviderQuotaOutcome::StaleWindow(_)
        ));

        let preserve_local_usage = ApplyRemoteProviderQuotaPatch {
            remote_monthly_used_usd: 0.0,
            remote_window_start_unix_secs: 900_000,
            remote_window_end_unix_secs: 1_000_000,
            local_usage_observation: None,
            preserve_local_used_usd: true,
            ..initial_remote.clone()
        };
        repository
            .apply_remote_provider_quota(&preserve_local_usage)
            .await
            .expect("state-only remote quota update should apply");
        let quota = repository
            .find_by_provider_id("provider-1")
            .await
            .expect("quota should reload")
            .expect("quota should exist");
        assert_eq!(quota.monthly_used_usd, 1.0);
    }

    async fn seed_provider_quotas(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO providers (
  id, name, provider_type, billing_type, monthly_quota_usd, monthly_used_usd,
  quota_reset_day, quota_last_reset_at, is_active, created_at, updated_at
)
VALUES
  ('provider-1', 'Provider One', 'openai', 'monthly_quota', 20.0, 5.0, 7, 1000, 1, 1, 1),
  ('provider-2', 'Provider Two', 'openai', 'payg', NULL, 1.5, NULL, NULL, 1, 1, 1),
  ('provider-null-used', 'Provider Null Used', 'openai', 'payg', NULL, NULL, NULL, NULL, 1, 1, 1)
"#,
        )
        .execute(pool)
        .await
        .expect("providers should seed");
    }
}
