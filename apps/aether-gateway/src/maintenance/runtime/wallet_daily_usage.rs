use chrono::{DateTime, Utc};

use crate::data::GatewayDataState;
use aether_data_contracts::DataLayerError;

use super::{
    maintenance_timezone, wallet_daily_usage_aggregation_target,
    DELETE_STALE_WALLET_DAILY_USAGE_LEDGERS_SQL, UPSERT_WALLET_DAILY_USAGE_LEDGER_SQL,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalletDailyUsageAggregationSummary {
    pub(crate) billing_date: chrono::NaiveDate,
    pub(crate) billing_timezone: String,
    pub(crate) aggregated_wallets: usize,
    pub(crate) deleted_stale_ledgers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WalletDailyUsageAggregationTarget {
    pub(super) billing_date: chrono::NaiveDate,
    pub(super) billing_timezone: String,
    pub(super) window_start_utc: DateTime<Utc>,
    pub(super) window_end_utc: DateTime<Utc>,
}

pub(super) async fn perform_wallet_daily_usage_aggregation_once(
    data: &GatewayDataState,
) -> Result<WalletDailyUsageAggregationSummary, DataLayerError> {
    let timezone = maintenance_timezone();
    let now_utc = Utc::now();
    let target = wallet_daily_usage_aggregation_target(now_utc, timezone);
    let Some(pool) = data.postgres_pool() else {
        return Ok(WalletDailyUsageAggregationSummary {
            billing_date: target.billing_date,
            billing_timezone: target.billing_timezone,
            aggregated_wallets: 0,
            deleted_stale_ledgers: 0,
        });
    };

    let mut tx = pool.begin().await.map_err(postgres_error)?;
    let aggregated_wallets = sqlx::query(UPSERT_WALLET_DAILY_USAGE_LEDGER_SQL)
        .bind(target.window_start_utc)
        .bind(target.window_end_utc)
        .bind(target.billing_date)
        .bind(target.billing_timezone.as_str())
        .bind(now_utc)
        .execute(&mut *tx)
        .await
        .map_err(postgres_error)?
        .rows_affected();

    let deleted_stale_ledgers = sqlx::query(DELETE_STALE_WALLET_DAILY_USAGE_LEDGERS_SQL)
        .bind(target.billing_date)
        .bind(target.billing_timezone.as_str())
        .bind(target.window_start_utc)
        .bind(target.window_end_utc)
        .execute(&mut *tx)
        .await
        .map_err(postgres_error)?
        .rows_affected();
    tx.commit().await.map_err(postgres_error)?;

    Ok(WalletDailyUsageAggregationSummary {
        billing_date: target.billing_date,
        billing_timezone: target.billing_timezone,
        aggregated_wallets: usize::try_from(aggregated_wallets).unwrap_or(usize::MAX),
        deleted_stale_ledgers: usize::try_from(deleted_stale_ledgers).unwrap_or(usize::MAX),
    })
}

fn postgres_error(error: sqlx::Error) -> DataLayerError {
    DataLayerError::postgres(error)
}
