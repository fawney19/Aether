use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::data::GatewayDataState;
use aether_data_contracts::DataLayerError;

use super::{
    postgres_error, stats_aggregation_target_day, system_config_bool, PercentileSummary,
    StatsAggregationSummary, DELETE_STATS_DAILY_ERRORS_FOR_DATE_SQL, INSERT_STATS_DAILY_ERROR_SQL,
    INSERT_STATS_SUMMARY_SQL, SELECT_EXISTING_STATS_SUMMARY_ID_SQL,
    SELECT_STATS_DAILY_AGGREGATE_SQL, SELECT_STATS_DAILY_FALLBACK_COUNT_SQL,
    SELECT_STATS_DAILY_FIRST_BYTE_PERCENTILES_SQL,
    SELECT_STATS_DAILY_RESPONSE_TIME_PERCENTILES_SQL, SELECT_STATS_SUMMARY_ENTITY_COUNTS_SQL,
    SELECT_STATS_SUMMARY_TOTALS_SQL, UPDATE_STATS_SUMMARY_SQL, UPSERT_STATS_DAILY_API_KEY_SQL,
    UPSERT_STATS_DAILY_MODEL_SQL, UPSERT_STATS_DAILY_PROVIDER_SQL, UPSERT_STATS_DAILY_SQL,
    UPSERT_STATS_USER_DAILY_SQL,
};

pub(super) async fn perform_stats_aggregation_once(
    data: &GatewayDataState,
) -> Result<Option<StatsAggregationSummary>, DataLayerError> {
    let Some(pool) = data.postgres_pool() else {
        return Ok(None);
    };
    if !system_config_bool(data, "enable_stats_aggregation", true).await? {
        return Ok(None);
    }

    let now_utc = Utc::now();
    let day_start_utc = stats_aggregation_target_day(now_utc);
    let day_end_utc = day_start_utc + chrono::Duration::days(1);
    let mut tx = pool.begin().await.map_err(postgres_error)?;
    let aggregate_row = sqlx::query(SELECT_STATS_DAILY_AGGREGATE_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .fetch_one(&mut *tx)
        .await
        .map_err(postgres_error)?;
    let total_requests = aggregate_row
        .try_get::<i64, _>("total_requests")
        .map_err(postgres_error)?;
    let error_requests = aggregate_row
        .try_get::<i64, _>("error_requests")
        .map_err(postgres_error)?;
    let success_requests = total_requests.saturating_sub(error_requests);
    let fallback_count = sqlx::query(SELECT_STATS_DAILY_FALLBACK_COUNT_SQL)
        .bind(day_start_utc)
        .bind(day_end_utc)
        .bind(vec!["success", "failed"])
        .fetch_one(&mut *tx)
        .await
        .map_err(postgres_error)?
        .try_get::<i64, _>("fallback_count")
        .map_err(postgres_error)?;
    let response_percentiles = fetch_stats_daily_percentiles(
        &mut tx,
        SELECT_STATS_DAILY_RESPONSE_TIME_PERCENTILES_SQL,
        day_start_utc,
        day_end_utc,
    )
    .await
    .map_err(postgres_error)?;
    let first_byte_percentiles = fetch_stats_daily_percentiles(
        &mut tx,
        SELECT_STATS_DAILY_FIRST_BYTE_PERCENTILES_SQL,
        day_start_utc,
        day_end_utc,
    )
    .await
    .map_err(postgres_error)?;

    sqlx::query(UPSERT_STATS_DAILY_SQL)
        .bind(Uuid::new_v4().to_string())
        .bind(day_start_utc)
        .bind(total_requests)
        .bind(success_requests)
        .bind(error_requests)
        .bind(
            aggregate_row
                .try_get::<i64, _>("input_tokens")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<i64, _>("output_tokens")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<i64, _>("cache_creation_tokens")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<i64, _>("cache_read_tokens")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<f64, _>("total_cost")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<f64, _>("actual_total_cost")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<f64, _>("input_cost")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<f64, _>("output_cost")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<f64, _>("cache_creation_cost")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<f64, _>("cache_read_cost")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<f64, _>("avg_response_time_ms")
                .map_err(postgres_error)?,
        )
        .bind(response_percentiles.p50)
        .bind(response_percentiles.p90)
        .bind(response_percentiles.p99)
        .bind(first_byte_percentiles.p50)
        .bind(first_byte_percentiles.p90)
        .bind(first_byte_percentiles.p99)
        .bind(fallback_count)
        .bind(
            aggregate_row
                .try_get::<i64, _>("unique_models")
                .map_err(postgres_error)?,
        )
        .bind(
            aggregate_row
                .try_get::<i64, _>("unique_providers")
                .map_err(postgres_error)?,
        )
        .bind(true)
        .bind(now_utc)
        .bind(now_utc)
        .bind(now_utc)
        .execute(&mut *tx)
        .await
        .map_err(postgres_error)?;

    let model_rows = upsert_stats_daily_model_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
        .await
        .map_err(postgres_error)?;
    let provider_rows =
        upsert_stats_daily_provider_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
            .await
            .map_err(postgres_error)?;
    let api_key_rows =
        upsert_stats_daily_api_key_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
            .await
            .map_err(postgres_error)?;
    let error_rows = refresh_stats_daily_error_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
        .await
        .map_err(postgres_error)?;
    let user_rows = upsert_stats_user_daily_rows(&mut tx, day_start_utc, day_end_utc, now_utc)
        .await
        .map_err(postgres_error)?;
    refresh_stats_summary_row(&mut tx, day_end_utc, now_utc)
        .await
        .map_err(postgres_error)?;
    tx.commit().await.map_err(postgres_error)?;

    Ok(Some(StatsAggregationSummary {
        day_start_utc,
        total_requests,
        model_rows,
        provider_rows,
        api_key_rows,
        error_rows,
        user_rows,
    }))
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

fn percentile_ms_to_i64(value: Option<f64>) -> Option<i64> {
    value.and_then(|raw| raw.is_finite().then_some(raw.floor() as i64))
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
