use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::backend::stats_common::{stats_id, unix_ms, unix_secs, utc_from_unix_secs};
use crate::backend::SqliteBackend;
use crate::driver::sqlite::{sqlite_real, SqlitePool};
use crate::error::SqlResultExt;
use crate::{
    DataLayerError, StatsDailyAggregationInput, StatsDailyAggregationSummary,
    StatsHourlyAggregationInput, StatsHourlyAggregationSummary,
};

impl SqliteBackend {
    pub async fn aggregate_stats_hourly(
        &self,
        input: &StatsHourlyAggregationInput,
    ) -> Result<Option<StatsHourlyAggregationSummary>, DataLayerError> {
        let Some(hour_utc_unix_secs) =
            next_sqlite_stats_hourly_bucket(self.pool(), input.target_hour_utc).await?
        else {
            return Ok(None);
        };
        perform_sqlite_stats_hourly_aggregation(
            self.pool(),
            hour_utc_unix_secs,
            input.aggregated_at,
        )
        .await
        .map(Some)
    }

    pub async fn aggregate_stats_daily(
        &self,
        input: &StatsDailyAggregationInput,
    ) -> Result<Option<StatsDailyAggregationSummary>, DataLayerError> {
        let Some(day_start_unix_secs) =
            next_sqlite_stats_daily_bucket(self.pool(), input.target_day_utc).await?
        else {
            return Ok(None);
        };
        perform_sqlite_stats_daily_aggregation(
            self.pool(),
            day_start_unix_secs,
            input.aggregated_at,
        )
        .await
        .map(Some)
    }
}

async fn next_sqlite_stats_hourly_bucket(
    pool: &SqlitePool,
    target_hour_utc: DateTime<Utc>,
) -> Result<Option<i64>, DataLayerError> {
    let latest_hour: Option<i64> =
        sqlx::query_scalar("SELECT MAX(hour_utc) FROM stats_hourly WHERE is_complete <> 0")
            .fetch_one(pool)
            .await
            .map_sql_err()?;
    let search_from = latest_hour.map(|value| value + 3600).unwrap_or(0);
    let search_until = unix_secs(target_hour_utc) + 3600;
    if search_from >= search_until {
        return Ok(None);
    }
    let next_bucket: Option<i64> = sqlx::query_scalar(
        r#"
SELECT MIN(CAST(created_at_unix_ms / 3600000 AS INTEGER) * 3600)
FROM "usage"
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#,
    )
    .bind(unix_ms(search_from)?)
    .bind(unix_ms(search_until)?)
    .fetch_one(pool)
    .await
    .map_sql_err()?;
    Ok(next_bucket.filter(|value| *value <= unix_secs(target_hour_utc)))
}

async fn next_sqlite_stats_daily_bucket(
    pool: &SqlitePool,
    target_day_utc: DateTime<Utc>,
) -> Result<Option<i64>, DataLayerError> {
    let latest_day: Option<i64> =
        sqlx::query_scalar(r#"SELECT MAX("date") FROM stats_daily WHERE is_complete <> 0"#)
            .fetch_one(pool)
            .await
            .map_sql_err()?;
    let search_from = latest_day.map(|value| value + 86_400).unwrap_or(0);
    let search_until = unix_secs(target_day_utc) + 86_400;
    if search_from >= search_until {
        return Ok(None);
    }
    let next_bucket: Option<i64> = sqlx::query_scalar(
        r#"
SELECT MIN(CAST(created_at_unix_ms / 86400000 AS INTEGER) * 86400)
FROM "usage"
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#,
    )
    .bind(unix_ms(search_from)?)
    .bind(unix_ms(search_until)?)
    .fetch_one(pool)
    .await
    .map_sql_err()?;
    Ok(next_bucket.filter(|value| *value <= unix_secs(target_day_utc)))
}

const SQLITE_STATS_AGGREGATE_SQL: &str = r#"
SELECT
  COUNT(*) AS total_requests,
  COALESCE(SUM(CASE
    WHEN status = 'failed'
      OR status_code >= 400
      OR (error_category IS NOT NULL AND error_category <> '')
    THEN 1 ELSE 0 END), 0) AS error_requests,
  COALESCE(SUM(input_tokens), 0) AS input_tokens,
  COALESCE(SUM(output_tokens), 0) AS output_tokens,
  COALESCE(SUM(cache_creation_input_tokens), 0) AS cache_creation_tokens,
  COALESCE(SUM(cache_read_input_tokens), 0) AS cache_read_tokens,
  CAST(COALESCE(SUM(total_cost_usd), 0) AS REAL) AS total_cost,
  CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS REAL) AS actual_total_cost,
  CAST(COALESCE(AVG(response_time_ms), 0) AS REAL) AS avg_response_time_ms
FROM "usage"
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#;

async fn perform_sqlite_stats_hourly_aggregation(
    pool: &SqlitePool,
    hour_utc_unix_secs: i64,
    aggregated_at: DateTime<Utc>,
) -> Result<StatsHourlyAggregationSummary, DataLayerError> {
    let start_ms = unix_ms(hour_utc_unix_secs)?;
    let end_ms = unix_ms(hour_utc_unix_secs + 3600)?;
    let aggregated_at_unix_secs = unix_secs(aggregated_at);
    let mut tx = pool.begin().await.map_sql_err()?;
    let row = sqlx::query(SQLITE_STATS_AGGREGATE_SQL)
        .bind(start_ms)
        .bind(end_ms)
        .fetch_one(&mut *tx)
        .await
        .map_sql_err()?;
    let total_requests: i64 = row.try_get("total_requests").map_sql_err()?;
    let error_requests: i64 = row.try_get("error_requests").map_sql_err()?;

    sqlx::query(
        r#"
INSERT INTO stats_hourly (
  id, hour_utc, total_requests, success_requests, error_requests,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_cost, actual_total_cost, avg_response_time_ms, is_complete,
  aggregated_at, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)
ON CONFLICT (hour_utc) DO UPDATE SET
  total_requests = excluded.total_requests,
  success_requests = excluded.success_requests,
  error_requests = excluded.error_requests,
  input_tokens = excluded.input_tokens,
  output_tokens = excluded.output_tokens,
  cache_creation_tokens = excluded.cache_creation_tokens,
  cache_read_tokens = excluded.cache_read_tokens,
  total_cost = excluded.total_cost,
  actual_total_cost = excluded.actual_total_cost,
  avg_response_time_ms = excluded.avg_response_time_ms,
  is_complete = excluded.is_complete,
  aggregated_at = excluded.aggregated_at,
  updated_at = excluded.updated_at
"#,
    )
    .bind(stats_id(&format!("stats-hourly:{hour_utc_unix_secs}")))
    .bind(hour_utc_unix_secs)
    .bind(total_requests)
    .bind(total_requests.saturating_sub(error_requests))
    .bind(error_requests)
    .bind(row.try_get::<i64, _>("input_tokens").map_sql_err()?)
    .bind(row.try_get::<i64, _>("output_tokens").map_sql_err()?)
    .bind(
        row.try_get::<i64, _>("cache_creation_tokens")
            .map_sql_err()?,
    )
    .bind(row.try_get::<i64, _>("cache_read_tokens").map_sql_err()?)
    .bind(sqlite_real(&row, "total_cost")?)
    .bind(sqlite_real(&row, "actual_total_cost")?)
    .bind(sqlite_real(&row, "avg_response_time_ms")?)
    .bind(aggregated_at_unix_secs)
    .bind(aggregated_at_unix_secs)
    .bind(aggregated_at_unix_secs)
    .execute(&mut *tx)
    .await
    .map_sql_err()?;

    let user_rows = sqlite_group_count(&mut tx, "user_id", start_ms, end_ms).await?;
    let user_model_rows = sqlite_group_count(&mut tx, "user_id, model", start_ms, end_ms).await?;
    let model_rows = sqlite_group_count(&mut tx, "model", start_ms, end_ms).await?;
    let provider_rows = sqlite_group_count(&mut tx, "provider_name", start_ms, end_ms).await?;
    tx.commit().await.map_sql_err()?;

    Ok(StatsHourlyAggregationSummary {
        hour_utc: utc_from_unix_secs(hour_utc_unix_secs, "stats_hourly.hour_utc")?,
        total_requests,
        user_rows,
        user_model_rows,
        model_rows,
        provider_rows,
    })
}

async fn perform_sqlite_stats_daily_aggregation(
    pool: &SqlitePool,
    day_start_unix_secs: i64,
    aggregated_at: DateTime<Utc>,
) -> Result<StatsDailyAggregationSummary, DataLayerError> {
    let start_ms = unix_ms(day_start_unix_secs)?;
    let end_ms = unix_ms(day_start_unix_secs + 86_400)?;
    let aggregated_at_unix_secs = unix_secs(aggregated_at);
    let mut tx = pool.begin().await.map_sql_err()?;
    let row = sqlx::query(SQLITE_STATS_AGGREGATE_SQL)
        .bind(start_ms)
        .bind(end_ms)
        .fetch_one(&mut *tx)
        .await
        .map_sql_err()?;
    let total_requests: i64 = row.try_get("total_requests").map_sql_err()?;
    let error_requests: i64 = row.try_get("error_requests").map_sql_err()?;
    let unique_models = sqlite_group_count(&mut tx, "model", start_ms, end_ms).await? as i64;
    let unique_providers =
        sqlite_group_count(&mut tx, "provider_name", start_ms, end_ms).await? as i64;

    sqlx::query(
        r#"
INSERT INTO stats_daily (
  id, "date", total_requests, success_requests, error_requests,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_cost, actual_total_cost, avg_response_time_ms, fallback_count,
  unique_models, unique_providers, is_complete, aggregated_at, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, 1, ?, ?, ?)
ON CONFLICT ("date") DO UPDATE SET
  total_requests = excluded.total_requests,
  success_requests = excluded.success_requests,
  error_requests = excluded.error_requests,
  input_tokens = excluded.input_tokens,
  output_tokens = excluded.output_tokens,
  cache_creation_tokens = excluded.cache_creation_tokens,
  cache_read_tokens = excluded.cache_read_tokens,
  total_cost = excluded.total_cost,
  actual_total_cost = excluded.actual_total_cost,
  avg_response_time_ms = excluded.avg_response_time_ms,
  fallback_count = excluded.fallback_count,
  unique_models = excluded.unique_models,
  unique_providers = excluded.unique_providers,
  is_complete = excluded.is_complete,
  aggregated_at = excluded.aggregated_at,
  updated_at = excluded.updated_at
"#,
    )
    .bind(stats_id(&format!("stats-daily:{day_start_unix_secs}")))
    .bind(day_start_unix_secs)
    .bind(total_requests)
    .bind(total_requests.saturating_sub(error_requests))
    .bind(error_requests)
    .bind(row.try_get::<i64, _>("input_tokens").map_sql_err()?)
    .bind(row.try_get::<i64, _>("output_tokens").map_sql_err()?)
    .bind(
        row.try_get::<i64, _>("cache_creation_tokens")
            .map_sql_err()?,
    )
    .bind(row.try_get::<i64, _>("cache_read_tokens").map_sql_err()?)
    .bind(sqlite_real(&row, "total_cost")?)
    .bind(sqlite_real(&row, "actual_total_cost")?)
    .bind(sqlite_real(&row, "avg_response_time_ms")?)
    .bind(unique_models)
    .bind(unique_providers)
    .bind(aggregated_at_unix_secs)
    .bind(aggregated_at_unix_secs)
    .bind(aggregated_at_unix_secs)
    .execute(&mut *tx)
    .await
    .map_sql_err()?;

    let model_rows = usize::try_from(unique_models).unwrap_or(usize::MAX);
    let provider_rows = usize::try_from(unique_providers).unwrap_or(usize::MAX);
    let api_key_rows = sqlite_group_count(&mut tx, "api_key_id", start_ms, end_ms).await?;
    let error_rows = sqlite_error_group_count(&mut tx, start_ms, end_ms).await?;
    let user_rows = sqlite_group_count(&mut tx, "user_id", start_ms, end_ms).await?;
    tx.commit().await.map_sql_err()?;

    Ok(StatsDailyAggregationSummary {
        day_start_utc: utc_from_unix_secs(day_start_unix_secs, "stats_daily.date")?,
        total_requests,
        model_rows,
        provider_rows,
        api_key_rows,
        error_rows,
        user_rows,
    })
}

async fn sqlite_group_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    group_columns: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<usize, DataLayerError> {
    let not_empty = group_columns
        .split(',')
        .map(str::trim)
        .map(|column| format!("{column} IS NOT NULL AND {column} <> ''"))
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = format!(
        r#"
SELECT COUNT(*)
FROM (
  SELECT 1
  FROM "usage"
  WHERE created_at_unix_ms >= ?
    AND created_at_unix_ms < ?
    AND status NOT IN ('pending', 'streaming')
    AND provider_name NOT IN ('unknown', 'pending')
    AND {not_empty}
  GROUP BY {group_columns}
)
"#
    );
    let count: i64 = sqlx::query_scalar(&sql)
        .bind(start_ms)
        .bind(end_ms)
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()?;
    Ok(usize::try_from(count.max(0)).unwrap_or(usize::MAX))
}

async fn sqlite_error_group_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    start_ms: i64,
    end_ms: i64,
) -> Result<usize, DataLayerError> {
    let count: i64 = sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM (
  SELECT 1
  FROM "usage"
  WHERE created_at_unix_ms >= ?
    AND created_at_unix_ms < ?
    AND status NOT IN ('pending', 'streaming')
    AND provider_name NOT IN ('unknown', 'pending')
    AND (
      status = 'failed'
      OR status_code >= 400
      OR (error_category IS NOT NULL AND error_category <> '')
    )
  GROUP BY COALESCE(NULLIF(error_category, ''), 'unknown_error'), provider_name, model
)
"#,
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_one(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(count.max(0)).unwrap_or(usize::MAX))
}
