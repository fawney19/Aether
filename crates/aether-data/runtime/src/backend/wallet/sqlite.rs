use sqlx::Row;

use crate::backend::SqliteBackend;
use crate::driver::sqlite::sqlite_real;
use crate::error::SqlResultExt;
use crate::{DataLayerError, WalletDailyUsageAggregationInput, WalletDailyUsageAggregationResult};

use super::{u64_to_i64, wallet_daily_usage_id};

const SELECT_WALLET_DAILY_USAGE_AGGREGATES_SQL: &str = r#"
SELECT
  usage_settlement_snapshots.wallet_id AS wallet_id,
  COUNT(*) AS total_requests,
  CAST(COALESCE(SUM("usage".total_cost_usd), 0) AS REAL) AS total_cost_usd,
  COALESCE(SUM("usage".input_tokens), 0) AS input_tokens,
  COALESCE(SUM("usage".output_tokens), 0) AS output_tokens,
  COALESCE(SUM("usage".cache_creation_input_tokens), 0) AS cache_creation_tokens,
  COALESCE(SUM("usage".cache_read_input_tokens), 0) AS cache_read_tokens,
  MIN(COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at)) AS first_finalized_at,
  MAX(COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at)) AS last_finalized_at
FROM "usage"
JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = "usage".request_id
WHERE usage_settlement_snapshots.wallet_id IS NOT NULL
  AND usage_settlement_snapshots.wallet_id <> ''
  AND COALESCE(usage_settlement_snapshots.billing_status, "usage".billing_status) = 'settled'
  AND "usage".total_cost_usd > 0
  AND COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at) >= ?
  AND COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at) < ?
GROUP BY usage_settlement_snapshots.wallet_id
"#;

impl SqliteBackend {
    pub async fn aggregate_wallet_daily_usage(
        &self,
        input: &WalletDailyUsageAggregationInput,
    ) -> Result<WalletDailyUsageAggregationResult, DataLayerError> {
        let window_start = u64_to_i64(input.window_start_unix_secs, "window_start")?;
        let window_end = u64_to_i64(input.window_end_unix_secs, "window_end")?;
        let aggregated_at = u64_to_i64(input.aggregated_at_unix_secs, "aggregated_at")?;
        let mut tx = self.pool().begin().await.map_sql_err()?;

        let rows = sqlx::query(SELECT_WALLET_DAILY_USAGE_AGGREGATES_SQL)
            .bind(window_start)
            .bind(window_end)
            .fetch_all(&mut *tx)
            .await
            .map_sql_err()?;

        let mut aggregated_wallets = 0usize;
        for row in rows {
            let wallet_id: String = row.try_get("wallet_id").map_sql_err()?;
            sqlx::query(
                r#"
DELETE FROM wallet_daily_usage_ledgers
WHERE wallet_id = ?
  AND billing_date = ?
  AND billing_timezone = ?
"#,
            )
            .bind(&wallet_id)
            .bind(&input.billing_date)
            .bind(&input.billing_timezone)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;

            sqlx::query(
                r#"
INSERT INTO wallet_daily_usage_ledgers (
  id,
  wallet_id,
  billing_date,
  billing_timezone,
  total_cost_usd,
  total_requests,
  input_tokens,
  output_tokens,
  cache_creation_tokens,
  cache_read_tokens,
  first_finalized_at,
  last_finalized_at,
  aggregated_at,
  created_at,
  updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
            )
            .bind(wallet_daily_usage_id(
                &wallet_id,
                &input.billing_date,
                &input.billing_timezone,
            ))
            .bind(&wallet_id)
            .bind(&input.billing_date)
            .bind(&input.billing_timezone)
            .bind(sqlite_real(&row, "total_cost_usd")?)
            .bind(row.try_get::<i64, _>("total_requests").map_sql_err()?)
            .bind(row.try_get::<i64, _>("input_tokens").map_sql_err()?)
            .bind(row.try_get::<i64, _>("output_tokens").map_sql_err()?)
            .bind(
                row.try_get::<i64, _>("cache_creation_tokens")
                    .map_sql_err()?,
            )
            .bind(row.try_get::<i64, _>("cache_read_tokens").map_sql_err()?)
            .bind(
                row.try_get::<Option<i64>, _>("first_finalized_at")
                    .map_sql_err()?,
            )
            .bind(
                row.try_get::<Option<i64>, _>("last_finalized_at")
                    .map_sql_err()?,
            )
            .bind(aggregated_at)
            .bind(aggregated_at)
            .bind(aggregated_at)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            aggregated_wallets += 1;
        }

        let deleted_stale_ledgers = sqlx::query(
            r#"
DELETE FROM wallet_daily_usage_ledgers
WHERE billing_date = ?
  AND billing_timezone = ?
  AND NOT EXISTS (
    SELECT 1
    FROM "usage"
    JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = "usage".request_id
    WHERE usage_settlement_snapshots.wallet_id = wallet_daily_usage_ledgers.wallet_id
      AND COALESCE(usage_settlement_snapshots.billing_status, "usage".billing_status) = 'settled'
      AND "usage".total_cost_usd > 0
      AND COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at) >= ?
      AND COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at) < ?
  )
"#,
        )
        .bind(&input.billing_date)
        .bind(&input.billing_timezone)
        .bind(window_start)
        .bind(window_end)
        .execute(&mut *tx)
        .await
        .map_sql_err()?
        .rows_affected();

        tx.commit().await.map_sql_err()?;
        Ok(WalletDailyUsageAggregationResult {
            aggregated_wallets,
            deleted_stale_ledgers: usize::try_from(deleted_stale_ledgers).unwrap_or(usize::MAX),
        })
    }
}
