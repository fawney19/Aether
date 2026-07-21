use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::backend::PostgresBackend;
use crate::{
    error::postgres_error, DataLayerError, StatsHourlyAggregationInput,
    StatsHourlyAggregationSummary,
};

mod sql;

use self::sql::*;

impl PostgresBackend {
    pub async fn aggregate_stats_hourly(
        &self,
        input: &StatsHourlyAggregationInput,
    ) -> Result<Option<StatsHourlyAggregationSummary>, DataLayerError> {
        let Some(hour_utc) = next_stats_hourly_bucket(self.pool(), input.target_hour_utc)
            .await
            .map_err(postgres_error)?
        else {
            return Ok(None);
        };

        perform_stats_hourly_aggregation_for_hour(self.pool(), hour_utc, input.aggregated_at)
            .await
            .map(Some)
            .map_err(postgres_error)
    }
}

async fn next_stats_hourly_bucket(
    pool: &crate::driver::postgres::PostgresPool,
    target_hour_utc: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    let latest_row = sqlx::query(SELECT_LATEST_STATS_HOURLY_HOUR_SQL)
        .fetch_one(pool)
        .await?;
    let latest_hour = latest_row.try_get::<Option<DateTime<Utc>>, _>("latest_hour")?;
    let search_from = latest_hour
        .map(|value| value + chrono::Duration::hours(1))
        .unwrap_or_else(|| {
            DateTime::<Utc>::from_timestamp(0, 0).expect("unix epoch should be valid")
        });
    let search_until = target_hour_utc + chrono::Duration::hours(1);
    if search_from >= search_until {
        return Ok(None);
    }

    let next_row = sqlx::query(SELECT_NEXT_STATS_HOURLY_BUCKET_SQL)
        .bind(search_from)
        .bind(search_until)
        .fetch_one(pool)
        .await?;
    let next_bucket = next_row.try_get::<Option<DateTime<Utc>>, _>("next_bucket")?;
    Ok(next_bucket.filter(|value| *value <= target_hour_utc))
}

async fn perform_stats_hourly_aggregation_for_hour(
    pool: &crate::driver::postgres::PostgresPool,
    hour_utc: DateTime<Utc>,
    aggregated_at: DateTime<Utc>,
) -> Result<StatsHourlyAggregationSummary, sqlx::Error> {
    let hour_end = hour_utc + chrono::Duration::hours(1);
    let mut tx = pool.begin().await?;

    let row = sqlx::query(SELECT_STATS_HOURLY_AGGREGATE_SQL)
        .bind(hour_utc)
        .bind(hour_end)
        .fetch_one(&mut *tx)
        .await?;
    let total_requests = row.try_get::<i64, _>("total_requests")?;
    let error_requests = row.try_get::<i64, _>("error_requests")?;
    let success_requests = total_requests.saturating_sub(error_requests);
    sqlx::query(UPSERT_STATS_HOURLY_SQL)
        .bind(Uuid::new_v4().to_string())
        .bind(hour_utc)
        .bind(total_requests)
        .bind(row.try_get::<i64, _>("cache_hit_total_requests")?)
        .bind(row.try_get::<i64, _>("cache_hit_requests")?)
        .bind(row.try_get::<i64, _>("completed_total_requests")?)
        .bind(row.try_get::<i64, _>("completed_cache_hit_requests")?)
        .bind(row.try_get::<i64, _>("completed_input_tokens")?)
        .bind(row.try_get::<i64, _>("completed_cache_creation_tokens")?)
        .bind(row.try_get::<i64, _>("completed_cache_read_tokens")?)
        .bind(row.try_get::<i64, _>("completed_total_input_context")?)
        .bind(row.try_get::<f64, _>("completed_cache_creation_cost")?)
        .bind(row.try_get::<f64, _>("completed_cache_read_cost")?)
        .bind(row.try_get::<f64, _>("settled_total_cost")?)
        .bind(row.try_get::<i64, _>("settled_total_requests")?)
        .bind(row.try_get::<i64, _>("settled_input_tokens")?)
        .bind(row.try_get::<i64, _>("settled_output_tokens")?)
        .bind(row.try_get::<i64, _>("settled_cache_creation_tokens")?)
        .bind(row.try_get::<i64, _>("settled_cache_read_tokens")?)
        .bind(row.try_get::<Option<i64>, _>("settled_first_finalized_at_unix_secs")?)
        .bind(row.try_get::<Option<i64>, _>("settled_last_finalized_at_unix_secs")?)
        .bind(success_requests)
        .bind(error_requests)
        .bind(row.try_get::<i64, _>("input_tokens")?)
        .bind(row.try_get::<i64, _>("output_tokens")?)
        .bind(row.try_get::<i64, _>("cache_creation_tokens")?)
        .bind(row.try_get::<i64, _>("cache_read_tokens")?)
        .bind(row.try_get::<f64, _>("total_cost")?)
        .bind(row.try_get::<f64, _>("actual_total_cost")?)
        .bind(row.try_get::<f64, _>("response_time_sum_ms")?)
        .bind(row.try_get::<i64, _>("response_time_samples")?)
        .bind(row.try_get::<f64, _>("avg_response_time_ms")?)
        .bind(true)
        .bind(aggregated_at)
        .bind(aggregated_at)
        .bind(aggregated_at)
        .execute(&mut *tx)
        .await?;

    let user_rows =
        upsert_stats_hourly_user_rows(&mut tx, hour_utc, hour_end, aggregated_at).await?;
    let user_model_rows =
        upsert_stats_hourly_user_model_rows(&mut tx, hour_utc, hour_end, aggregated_at).await?;
    let model_rows =
        upsert_stats_hourly_model_rows(&mut tx, hour_utc, hour_end, aggregated_at).await?;
    let provider_rows =
        upsert_stats_hourly_provider_rows(&mut tx, hour_utc, hour_end, aggregated_at).await?;
    tx.commit().await?;

    Ok(StatsHourlyAggregationSummary {
        hour_utc,
        total_requests,
        user_rows,
        user_model_rows,
        model_rows,
        provider_rows,
    })
}

async fn upsert_stats_hourly_user_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    hour_utc: DateTime<Utc>,
    hour_end: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_HOURLY_USER_SQL)
        .bind(hour_utc)
        .bind(hour_end)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_hourly_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    hour_utc: DateTime<Utc>,
    hour_end: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_HOURLY_MODEL_SQL)
        .bind(hour_utc)
        .bind(hour_end)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_hourly_user_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    hour_utc: DateTime<Utc>,
    hour_end: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_HOURLY_USER_MODEL_SQL)
        .bind(hour_utc)
        .bind(hour_end)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_hourly_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    hour_utc: DateTime<Utc>,
    hour_end: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_HOURLY_PROVIDER_SQL)
        .bind(hour_utc)
        .bind(hour_end)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}
