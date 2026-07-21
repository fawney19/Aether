use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::backend::PostgresBackend;
use crate::{
    error::postgres_error, DataLayerError, StatsDailyAggregationInput, StatsDailyAggregationSummary,
};

mod percentiles;
mod sql;

use self::percentiles::{percentile_ms_to_i64, PercentileSummary};
use self::sql::*;

impl PostgresBackend {
    pub async fn aggregate_stats_daily(
        &self,
        input: &StatsDailyAggregationInput,
    ) -> Result<Option<StatsDailyAggregationSummary>, DataLayerError> {
        let Some(day_start_utc) = next_stats_aggregation_day(self.pool(), input.target_day_utc)
            .await
            .map_err(postgres_error)?
        else {
            return Ok(None);
        };

        perform_stats_aggregation_for_day(self.pool(), day_start_utc, input.aggregated_at)
            .await
            .map(Some)
            .map_err(postgres_error)
    }
}

async fn next_stats_aggregation_day(
    pool: &crate::driver::postgres::PostgresPool,
    target_day_utc: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    let latest_row = sqlx::query(SELECT_LATEST_STATS_DAILY_DATE_SQL)
        .fetch_one(pool)
        .await?;
    let latest_day = latest_row.try_get::<Option<DateTime<Utc>>, _>("latest_date")?;
    let search_from = latest_day
        .map(|value| value + chrono::Duration::days(1))
        .unwrap_or_else(|| {
            DateTime::<Utc>::from_timestamp(0, 0).expect("unix epoch should be valid")
        });
    let search_until = target_day_utc + chrono::Duration::days(1);
    if search_from >= search_until {
        return Ok(None);
    }

    let next_row = sqlx::query(SELECT_NEXT_STATS_DAILY_BUCKET_SQL)
        .bind(search_from)
        .bind(search_until)
        .fetch_one(pool)
        .await?;
    let next_bucket = next_row.try_get::<Option<DateTime<Utc>>, _>("next_bucket")?;
    Ok(next_bucket.filter(|value| *value <= target_day_utc))
}

async fn perform_stats_aggregation_for_day(
    pool: &crate::driver::postgres::PostgresPool,
    day_start_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<StatsDailyAggregationSummary, sqlx::Error> {
    let day_end_utc = day_start_utc + chrono::Duration::days(1);
    let mut tx = pool.begin().await?;
    let aggregate_row = sqlx::query(SELECT_STATS_DAILY_AGGREGATE_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .fetch_one(&mut *tx)
        .await?;
    let total_requests = aggregate_row.try_get::<i64, _>("total_requests")?;
    let error_requests = aggregate_row.try_get::<i64, _>("error_requests")?;
    let success_requests = total_requests.saturating_sub(error_requests);
    let fallback_count = sqlx::query(SELECT_STATS_DAILY_FALLBACK_COUNT_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(vec!["success", "failed"])
        .fetch_one(&mut *tx)
        .await?
        .try_get::<i64, _>("fallback_count")?;
    let response_percentiles = fetch_stats_daily_percentiles(
        &mut tx,
        SELECT_STATS_DAILY_RESPONSE_TIME_PERCENTILES_SQL,
        day_start_utc,
        day_end_utc,
    )
    .await?;
    let first_byte_percentiles = fetch_stats_daily_percentiles(
        &mut tx,
        SELECT_STATS_DAILY_FIRST_BYTE_PERCENTILES_SQL,
        day_start_utc,
        day_end_utc,
    )
    .await?;

    sqlx::query(UPSERT_STATS_DAILY_SQL)
        .bind(Uuid::new_v4().to_string())
        .bind(day_start_utc)
        .bind(total_requests)
        .bind(aggregate_row.try_get::<i64, _>("cache_hit_total_requests")?)
        .bind(aggregate_row.try_get::<i64, _>("cache_hit_requests")?)
        .bind(aggregate_row.try_get::<i64, _>("completed_total_requests")?)
        .bind(aggregate_row.try_get::<i64, _>("completed_cache_hit_requests")?)
        .bind(aggregate_row.try_get::<i64, _>("completed_input_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("completed_cache_creation_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("completed_cache_read_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("completed_total_input_context")?)
        .bind(aggregate_row.try_get::<f64, _>("completed_cache_creation_cost")?)
        .bind(aggregate_row.try_get::<f64, _>("completed_cache_read_cost")?)
        .bind(aggregate_row.try_get::<f64, _>("settled_total_cost")?)
        .bind(aggregate_row.try_get::<i64, _>("settled_total_requests")?)
        .bind(aggregate_row.try_get::<i64, _>("settled_input_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("settled_output_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("settled_cache_creation_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("settled_cache_read_tokens")?)
        .bind(aggregate_row.try_get::<Option<i64>, _>("settled_first_finalized_at_unix_secs")?)
        .bind(aggregate_row.try_get::<Option<i64>, _>("settled_last_finalized_at_unix_secs")?)
        .bind(success_requests)
        .bind(error_requests)
        .bind(aggregate_row.try_get::<i64, _>("input_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("effective_input_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("output_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("cache_creation_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("cache_creation_ephemeral_5m_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("cache_creation_ephemeral_1h_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("cache_read_tokens")?)
        .bind(aggregate_row.try_get::<i64, _>("total_input_context")?)
        .bind(aggregate_row.try_get::<f64, _>("total_cost")?)
        .bind(aggregate_row.try_get::<f64, _>("actual_total_cost")?)
        .bind(aggregate_row.try_get::<f64, _>("input_cost")?)
        .bind(aggregate_row.try_get::<f64, _>("output_cost")?)
        .bind(aggregate_row.try_get::<f64, _>("cache_creation_cost")?)
        .bind(aggregate_row.try_get::<f64, _>("cache_read_cost")?)
        .bind(aggregate_row.try_get::<f64, _>("response_time_sum_ms")?)
        .bind(aggregate_row.try_get::<i64, _>("response_time_samples")?)
        .bind(aggregate_row.try_get::<f64, _>("avg_response_time_ms")?)
        .bind(response_percentiles.p50)
        .bind(response_percentiles.p90)
        .bind(response_percentiles.p99)
        .bind(first_byte_percentiles.p50)
        .bind(first_byte_percentiles.p90)
        .bind(first_byte_percentiles.p99)
        .bind(fallback_count)
        .bind(aggregate_row.try_get::<i64, _>("unique_models")?)
        .bind(aggregate_row.try_get::<i64, _>("unique_providers")?)
        .bind(true)
        .bind(now_utc)
        .bind(now_utc)
        .bind(now_utc)
        .execute(&mut *tx)
        .await?;

    let model_rows =
        upsert_stats_daily_model_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    let provider_rows =
        upsert_stats_daily_provider_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    upsert_stats_daily_model_provider_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    upsert_stats_daily_cost_savings_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    upsert_stats_daily_cost_savings_provider_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
        .await?;
    upsert_stats_daily_cost_savings_model_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
        .await?;
    upsert_stats_daily_cost_savings_model_provider_rows(
        &mut tx,
        day_start_utc,
        day_end_utc,
        now_utc,
    )
    .await?;
    let api_key_rows =
        upsert_stats_daily_api_key_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    let error_rows =
        refresh_stats_daily_error_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    let user_rows =
        upsert_stats_user_daily_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    upsert_stats_user_daily_model_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    upsert_stats_user_daily_model_provider_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
        .await?;
    upsert_stats_user_daily_provider_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    upsert_stats_user_daily_cost_savings_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    upsert_stats_user_daily_cost_savings_provider_rows(
        &mut tx,
        day_start_utc,
        day_end_utc,
        now_utc,
    )
    .await?;
    upsert_stats_user_daily_cost_savings_model_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
        .await?;
    upsert_stats_user_daily_cost_savings_model_provider_rows(
        &mut tx,
        day_start_utc,
        day_end_utc,
        now_utc,
    )
    .await?;
    upsert_stats_user_daily_api_format_rows(&mut tx, day_start_utc, day_end_utc, now_utc).await?;
    refresh_stats_summary_row(&mut tx, day_end_utc, now_utc).await?;
    refresh_stats_user_summary_rows(&mut tx, day_end_utc, now_utc).await?;
    tx.commit().await?;

    Ok(StatsDailyAggregationSummary {
        day_start_utc,
        total_requests,
        model_rows,
        provider_rows,
        api_key_rows,
        error_rows,
        user_rows,
    })
}

async fn fetch_stats_daily_percentiles(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sql: &str,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
) -> Result<PercentileSummary, sqlx::Error> {
    let row = sqlx::query(sql)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .fetch_one(&mut **tx)
        .await?;
    let sample_count = row.try_get::<i64, _>("sample_count")?;
    if sample_count < 10 {
        return Ok(PercentileSummary::default());
    }

    Ok(PercentileSummary {
        p50: percentile_ms_to_i64(row.try_get::<Option<f64>, _>("p50")?),
        p90: percentile_ms_to_i64(row.try_get::<Option<f64>, _>("p90")?),
        p99: percentile_ms_to_i64(row.try_get::<Option<f64>, _>("p99")?),
    })
}

async fn upsert_stats_daily_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_DAILY_MODEL_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_daily_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_DAILY_PROVIDER_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_daily_model_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_DAILY_MODEL_PROVIDER_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_daily_cost_savings_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_DAILY_COST_SAVINGS_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_daily_cost_savings_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_DAILY_COST_SAVINGS_PROVIDER_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_daily_cost_savings_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_DAILY_COST_SAVINGS_MODEL_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_daily_cost_savings_model_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_DAILY_COST_SAVINGS_MODEL_PROVIDER_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_daily_api_key_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_DAILY_API_KEY_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn refresh_stats_daily_error_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    sqlx::query(DELETE_STATS_DAILY_ERRORS_FOR_DATE_SQL)
        .bind(day_start_utc)
        .execute(&mut **tx)
        .await?;
    let rows_affected = sqlx::query(INSERT_STATS_DAILY_ERROR_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_MODEL_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_model_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_MODEL_PROVIDER_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_PROVIDER_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_cost_savings_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_COST_SAVINGS_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_cost_savings_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_COST_SAVINGS_PROVIDER_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_cost_savings_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_COST_SAVINGS_MODEL_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_cost_savings_model_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_COST_SAVINGS_MODEL_PROVIDER_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn upsert_stats_user_daily_api_format_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    day_start_utc: DateTime<Utc>,
    day_end_utc: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<usize, sqlx::Error> {
    let rows_affected = sqlx::query(UPSERT_STATS_USER_DAILY_API_FORMAT_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    Ok(usize::try_from(rows_affected).unwrap_or(usize::MAX))
}

async fn refresh_stats_summary_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cutoff_date: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let totals_row = sqlx::query(SELECT_STATS_SUMMARY_TOTALS_SQL)
        .bind(cutoff_date)
        .fetch_one(&mut **tx)
        .await?;
    let entity_counts_row = sqlx::query(SELECT_STATS_SUMMARY_ENTITY_COUNTS_SQL)
        .fetch_one(&mut **tx)
        .await?;
    let existing_summary_id = sqlx::query_scalar::<_, String>(SELECT_EXISTING_STATS_SUMMARY_ID_SQL)
        .fetch_optional(&mut **tx)
        .await?;

    let all_time_requests = totals_row.try_get::<i64, _>("all_time_requests")?;
    let all_time_success_requests = totals_row.try_get::<i64, _>("all_time_success_requests")?;
    let all_time_error_requests = totals_row.try_get::<i64, _>("all_time_error_requests")?;
    let all_time_input_tokens = totals_row.try_get::<i64, _>("all_time_input_tokens")?;
    let all_time_output_tokens = totals_row.try_get::<i64, _>("all_time_output_tokens")?;
    let all_time_cache_creation_tokens =
        totals_row.try_get::<i64, _>("all_time_cache_creation_tokens")?;
    let all_time_cache_read_tokens = totals_row.try_get::<i64, _>("all_time_cache_read_tokens")?;
    let all_time_cost = totals_row.try_get::<f64, _>("all_time_cost")?;
    let all_time_actual_cost = totals_row.try_get::<f64, _>("all_time_actual_cost")?;
    let total_users = entity_counts_row.try_get::<i64, _>("total_users")?;
    let active_users = entity_counts_row.try_get::<i64, _>("active_users")?;
    let total_api_keys = entity_counts_row.try_get::<i64, _>("total_api_keys")?;
    let active_api_keys = entity_counts_row.try_get::<i64, _>("active_api_keys")?;

    if let Some(summary_id) = existing_summary_id {
        sqlx::query(UPDATE_STATS_SUMMARY_SQL)
            .bind(summary_id)
            .bind(cutoff_date)
            .bind(all_time_requests)
            .bind(all_time_success_requests)
            .bind(all_time_error_requests)
            .bind(all_time_input_tokens)
            .bind(all_time_output_tokens)
            .bind(all_time_cache_creation_tokens)
            .bind(all_time_cache_read_tokens)
            .bind(all_time_cost)
            .bind(all_time_actual_cost)
            .bind(total_users)
            .bind(active_users)
            .bind(total_api_keys)
            .bind(active_api_keys)
            .bind(now_utc)
            .execute(&mut **tx)
            .await?;
    } else {
        sqlx::query(INSERT_STATS_SUMMARY_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(cutoff_date)
            .bind(all_time_requests)
            .bind(all_time_success_requests)
            .bind(all_time_error_requests)
            .bind(all_time_input_tokens)
            .bind(all_time_output_tokens)
            .bind(all_time_cache_creation_tokens)
            .bind(all_time_cache_read_tokens)
            .bind(all_time_cost)
            .bind(all_time_actual_cost)
            .bind(total_users)
            .bind(active_users)
            .bind(total_api_keys)
            .bind(active_api_keys)
            .bind(now_utc)
            .bind(now_utc)
            .execute(&mut **tx)
            .await?;
    }

    Ok(())
}

async fn refresh_stats_user_summary_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cutoff_date: DateTime<Utc>,
    now_utc: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(UPSERT_STATS_USER_SUMMARY_SQL)
        .bind(cutoff_date)
        .bind(now_utc)
        .execute(&mut **tx)
        .await?;

    Ok(())
}
