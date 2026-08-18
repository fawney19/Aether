use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::backend::stats_common::{stats_id, unix_secs, utc_from_unix_secs};
use crate::backend::MysqlBackend;
use crate::driver::mysql::MysqlPool;
use crate::error::SqlResultExt;
use crate::{
    DataLayerError, StatsDailyAggregationInput, StatsDailyAggregationSummary,
    StatsHourlyAggregationInput, StatsHourlyAggregationSummary,
};

mod advanced;

impl MysqlBackend {
    pub async fn aggregate_stats_hourly(
        &self,
        input: &StatsHourlyAggregationInput,
    ) -> Result<Option<StatsHourlyAggregationSummary>, DataLayerError> {
        let Some(hour_utc_unix_secs) =
            next_mysql_stats_hourly_bucket(self.pool(), input.target_hour_utc).await?
        else {
            return Ok(None);
        };
        perform_mysql_stats_hourly_aggregation(self.pool(), hour_utc_unix_secs, input.aggregated_at)
            .await
            .map(Some)
    }

    pub async fn aggregate_stats_daily(
        &self,
        input: &StatsDailyAggregationInput,
    ) -> Result<Option<StatsDailyAggregationSummary>, DataLayerError> {
        let Some(day_start_unix_secs) =
            next_mysql_stats_daily_bucket(self.pool(), input.target_day_utc).await?
        else {
            return Ok(None);
        };
        perform_mysql_stats_daily_aggregation(self.pool(), day_start_unix_secs, input.aggregated_at)
            .await
            .map(Some)
    }
}

async fn next_mysql_stats_hourly_bucket(
    pool: &MysqlPool,
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
SELECT CAST(MIN(FLOOR(created_at_unix_ms / 3600) * 3600) AS SIGNED)
FROM `usage`
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#,
    )
    .bind(search_from)
    .bind(search_until)
    .fetch_one(pool)
    .await
    .map_sql_err()?;
    Ok(next_bucket.filter(|value| *value <= unix_secs(target_hour_utc)))
}

async fn next_mysql_stats_daily_bucket(
    pool: &MysqlPool,
    target_day_utc: DateTime<Utc>,
) -> Result<Option<i64>, DataLayerError> {
    let latest_day: Option<i64> =
        sqlx::query_scalar("SELECT MAX(`date`) FROM stats_daily WHERE is_complete <> 0")
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
SELECT CAST(MIN(FLOOR(created_at_unix_ms / 86400) * 86400) AS SIGNED)
FROM `usage`
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#,
    )
    .bind(search_from)
    .bind(search_until)
    .fetch_one(pool)
    .await
    .map_sql_err()?;
    Ok(next_bucket.filter(|value| *value <= unix_secs(target_day_utc)))
}

const MYSQL_STATS_AGGREGATE_SQL: &str = r#"
SELECT
  CAST(COUNT(*) AS SIGNED) AS total_requests,
  CAST(COALESCE(SUM(CASE WHEN outcome_class = 'success' THEN 1 ELSE 0 END), 0) AS SIGNED) AS success_requests,
  CAST(COALESCE(SUM(CASE WHEN sla_eligible <> 0 THEN 1 ELSE 0 END), 0) AS SIGNED) AS sla_eligible_requests,
  CAST(COALESCE(SUM(CASE WHEN outcome_class = 'service_error' THEN 1 ELSE 0 END), 0) AS SIGNED) AS error_requests,
  CAST(COALESCE(SUM(CASE WHEN outcome_class = 'user_error' THEN 1 ELSE 0 END), 0) AS SIGNED) AS user_error_requests,
  CAST(COALESCE(SUM(input_tokens), 0) AS SIGNED) AS input_tokens,
  CAST(COALESCE(SUM(output_tokens), 0) AS SIGNED) AS output_tokens,
  CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS SIGNED) AS cache_creation_tokens,
  CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS SIGNED) AS cache_read_tokens,
  CAST(COALESCE(SUM(total_cost_usd), 0.0) AS DOUBLE) AS total_cost,
  CAST(COALESCE(SUM(actual_total_cost_usd), 0.0) AS DOUBLE) AS actual_total_cost,
  CAST(COALESCE(AVG(response_time_ms), 0.0) AS DOUBLE) AS avg_response_time_ms
FROM `usage`
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#;

async fn perform_mysql_stats_hourly_aggregation(
    pool: &MysqlPool,
    hour_utc_unix_secs: i64,
    aggregated_at: DateTime<Utc>,
) -> Result<StatsHourlyAggregationSummary, DataLayerError> {
    let start_unix_secs = hour_utc_unix_secs;
    let end_unix_secs = hour_utc_unix_secs + 3600;
    let aggregated_at_unix_secs = unix_secs(aggregated_at);
    let mut tx = pool.begin().await.map_sql_err()?;
    let row = sqlx::query(MYSQL_STATS_AGGREGATE_SQL)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .fetch_one(&mut *tx)
        .await
        .map_sql_err()?;
    let total_requests: i64 = row.try_get("total_requests").map_sql_err()?;
    let success_requests: i64 = row.try_get("success_requests").map_sql_err()?;
    let sla_eligible_requests: i64 = row.try_get("sla_eligible_requests").map_sql_err()?;
    let error_requests: i64 = row.try_get("error_requests").map_sql_err()?;
    let user_error_requests: i64 = row.try_get("user_error_requests").map_sql_err()?;

    sqlx::query(
        r#"
INSERT INTO stats_hourly (
  id, hour_utc, total_requests, success_requests, error_requests,
  sla_eligible_requests, user_error_requests,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_cost, actual_total_cost, avg_response_time_ms, is_complete,
  aggregated_at, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, TRUE, ?, ?, ?)
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests),
  success_requests = VALUES(success_requests),
  error_requests = VALUES(error_requests),
  sla_eligible_requests = VALUES(sla_eligible_requests),
  user_error_requests = VALUES(user_error_requests),
  input_tokens = VALUES(input_tokens),
  output_tokens = VALUES(output_tokens),
  cache_creation_tokens = VALUES(cache_creation_tokens),
  cache_read_tokens = VALUES(cache_read_tokens),
  total_cost = VALUES(total_cost),
  actual_total_cost = VALUES(actual_total_cost),
  avg_response_time_ms = VALUES(avg_response_time_ms),
  is_complete = VALUES(is_complete),
  aggregated_at = VALUES(aggregated_at),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(stats_id(&format!("stats-hourly:{hour_utc_unix_secs}")))
    .bind(hour_utc_unix_secs)
    .bind(total_requests)
    .bind(success_requests)
    .bind(error_requests)
    .bind(sla_eligible_requests)
    .bind(user_error_requests)
    .bind(row.try_get::<i64, _>("input_tokens").map_sql_err()?)
    .bind(row.try_get::<i64, _>("output_tokens").map_sql_err()?)
    .bind(
        row.try_get::<i64, _>("cache_creation_tokens")
            .map_sql_err()?,
    )
    .bind(row.try_get::<i64, _>("cache_read_tokens").map_sql_err()?)
    .bind(row.try_get::<f64, _>("total_cost").map_sql_err()?)
    .bind(row.try_get::<f64, _>("actual_total_cost").map_sql_err()?)
    .bind(
        row.try_get::<f64, _>("avg_response_time_ms")
            .map_sql_err()?,
    )
    .bind(aggregated_at_unix_secs)
    .bind(aggregated_at_unix_secs)
    .bind(aggregated_at_unix_secs)
    .execute(&mut *tx)
    .await
    .map_sql_err()?;

    let user_rows = upsert_mysql_stats_hourly_user_rows(
        &mut tx,
        hour_utc_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    let user_model_rows = upsert_mysql_stats_hourly_user_model_rows(
        &mut tx,
        hour_utc_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    let model_rows = upsert_mysql_stats_hourly_model_rows(
        &mut tx,
        hour_utc_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    let provider_rows = upsert_mysql_stats_hourly_provider_rows(
        &mut tx,
        hour_utc_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    advanced::refresh_hourly(&mut tx, hour_utc_unix_secs, start_unix_secs, end_unix_secs).await?;
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

async fn perform_mysql_stats_daily_aggregation(
    pool: &MysqlPool,
    day_start_unix_secs: i64,
    aggregated_at: DateTime<Utc>,
) -> Result<StatsDailyAggregationSummary, DataLayerError> {
    let start_unix_secs = day_start_unix_secs;
    let end_unix_secs = day_start_unix_secs + 86_400;
    let aggregated_at_unix_secs = unix_secs(aggregated_at);
    let mut tx = pool.begin().await.map_sql_err()?;
    let row = sqlx::query(MYSQL_STATS_AGGREGATE_SQL)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .fetch_one(&mut *tx)
        .await
        .map_sql_err()?;
    let total_requests: i64 = row.try_get("total_requests").map_sql_err()?;
    let success_requests: i64 = row.try_get("success_requests").map_sql_err()?;
    let sla_eligible_requests: i64 = row.try_get("sla_eligible_requests").map_sql_err()?;
    let error_requests: i64 = row.try_get("error_requests").map_sql_err()?;
    let user_error_requests: i64 = row.try_get("user_error_requests").map_sql_err()?;
    let unique_models =
        mysql_group_count(&mut tx, "model", start_unix_secs, end_unix_secs).await? as i64;
    let unique_providers =
        mysql_group_count(&mut tx, "provider_name", start_unix_secs, end_unix_secs).await? as i64;
    let fallback_count =
        mysql_daily_fallback_count(&mut tx, start_unix_secs, end_unix_secs).await?;

    sqlx::query(
        r#"
INSERT INTO stats_daily (
  id, `date`, total_requests, success_requests, error_requests,
  sla_eligible_requests, user_error_requests,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_cost, actual_total_cost, avg_response_time_ms, fallback_count,
  unique_models, unique_providers, is_complete, aggregated_at, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, TRUE, ?, ?, ?)
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests),
  success_requests = VALUES(success_requests),
  error_requests = VALUES(error_requests),
  sla_eligible_requests = VALUES(sla_eligible_requests),
  user_error_requests = VALUES(user_error_requests),
  input_tokens = VALUES(input_tokens),
  output_tokens = VALUES(output_tokens),
  cache_creation_tokens = VALUES(cache_creation_tokens),
  cache_read_tokens = VALUES(cache_read_tokens),
  total_cost = VALUES(total_cost),
  actual_total_cost = VALUES(actual_total_cost),
  avg_response_time_ms = VALUES(avg_response_time_ms),
  fallback_count = VALUES(fallback_count),
  unique_models = VALUES(unique_models),
  unique_providers = VALUES(unique_providers),
  is_complete = VALUES(is_complete),
  aggregated_at = VALUES(aggregated_at),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(stats_id(&format!("stats-daily:{day_start_unix_secs}")))
    .bind(day_start_unix_secs)
    .bind(total_requests)
    .bind(success_requests)
    .bind(error_requests)
    .bind(sla_eligible_requests)
    .bind(user_error_requests)
    .bind(row.try_get::<i64, _>("input_tokens").map_sql_err()?)
    .bind(row.try_get::<i64, _>("output_tokens").map_sql_err()?)
    .bind(
        row.try_get::<i64, _>("cache_creation_tokens")
            .map_sql_err()?,
    )
    .bind(row.try_get::<i64, _>("cache_read_tokens").map_sql_err()?)
    .bind(row.try_get::<f64, _>("total_cost").map_sql_err()?)
    .bind(row.try_get::<f64, _>("actual_total_cost").map_sql_err()?)
    .bind(
        row.try_get::<f64, _>("avg_response_time_ms")
            .map_sql_err()?,
    )
    .bind(fallback_count)
    .bind(unique_models)
    .bind(unique_providers)
    .bind(aggregated_at_unix_secs)
    .bind(aggregated_at_unix_secs)
    .bind(aggregated_at_unix_secs)
    .execute(&mut *tx)
    .await
    .map_sql_err()?;

    let model_rows = upsert_mysql_stats_daily_model_rows(
        &mut tx,
        day_start_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    let provider_rows = upsert_mysql_stats_daily_provider_rows(
        &mut tx,
        day_start_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    let api_key_rows = upsert_mysql_stats_daily_api_key_rows(
        &mut tx,
        day_start_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    let error_rows = refresh_mysql_stats_daily_error_rows(
        &mut tx,
        day_start_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    let user_rows = upsert_mysql_stats_user_daily_rows(
        &mut tx,
        day_start_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
    advanced::refresh_daily(
        &mut tx,
        day_start_unix_secs,
        start_unix_secs,
        end_unix_secs,
        aggregated_at_unix_secs,
    )
    .await?;
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

async fn upsert_mysql_stats_hourly_user_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    hour_utc: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    let result = sqlx::query(
        r#"
INSERT INTO stats_hourly_user (
  id, hour_utc, user_id, total_requests, success_requests, error_requests,
  sla_eligible_requests, user_error_requests,
  input_tokens, output_tokens, total_cost, created_at, updated_at
)
SELECT
  SHA2(UUID(), 256), ?, user_id, COUNT(*),
  COALESCE(SUM(CASE WHEN outcome_class = 'success' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN outcome_class = 'service_error' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN sla_eligible <> 0 THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN outcome_class = 'user_error' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
  COALESCE(SUM(total_cost_usd), 0), ?, ?
FROM `usage`
WHERE created_at_unix_ms >= ? AND created_at_unix_ms < ?
  AND user_id IS NOT NULL AND user_id <> ''
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
GROUP BY user_id
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), success_requests = VALUES(success_requests),
  error_requests = VALUES(error_requests), input_tokens = VALUES(input_tokens),
  sla_eligible_requests = VALUES(sla_eligible_requests),
  user_error_requests = VALUES(user_error_requests),
  output_tokens = VALUES(output_tokens), total_cost = VALUES(total_cost),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(hour_utc)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn upsert_mysql_stats_hourly_user_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    hour_utc: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    let result = sqlx::query(
        r#"
INSERT INTO stats_hourly_user_model (
  id, hour_utc, user_id, model, total_requests, input_tokens, output_tokens,
  total_cost, created_at, updated_at
)
SELECT SHA2(UUID(), 256), ?, user_id, model, COUNT(*),
  COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
  COALESCE(SUM(total_cost_usd), 0), ?, ?
FROM `usage`
WHERE created_at_unix_ms >= ? AND created_at_unix_ms < ?
  AND user_id IS NOT NULL AND user_id <> '' AND model IS NOT NULL AND model <> ''
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
GROUP BY user_id, model
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), input_tokens = VALUES(input_tokens),
  output_tokens = VALUES(output_tokens), total_cost = VALUES(total_cost),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(hour_utc)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn upsert_mysql_stats_hourly_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    hour_utc: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    let result = sqlx::query(
        r#"
INSERT INTO stats_hourly_model (
  id, hour_utc, model, total_requests, input_tokens, output_tokens, total_cost,
  avg_response_time_ms, created_at, updated_at
)
SELECT SHA2(UUID(), 256), ?, model, COUNT(*),
  COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
  COALESCE(SUM(total_cost_usd), 0), COALESCE(AVG(response_time_ms), 0), ?, ?
FROM `usage`
WHERE created_at_unix_ms >= ? AND created_at_unix_ms < ?
  AND model IS NOT NULL AND model <> ''
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
GROUP BY model
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), input_tokens = VALUES(input_tokens),
  output_tokens = VALUES(output_tokens), total_cost = VALUES(total_cost),
  avg_response_time_ms = VALUES(avg_response_time_ms), updated_at = VALUES(updated_at)
"#,
    )
    .bind(hour_utc)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn upsert_mysql_stats_hourly_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    hour_utc: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    let result = sqlx::query(
        r#"
INSERT INTO stats_hourly_provider (
  id, hour_utc, provider_name, total_requests, input_tokens, output_tokens,
  total_cost, created_at, updated_at
)
SELECT SHA2(UUID(), 256), ?, provider_name, COUNT(*),
  COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
  COALESCE(SUM(total_cost_usd), 0), ?, ?
FROM `usage`
WHERE created_at_unix_ms >= ? AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
GROUP BY provider_name
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), input_tokens = VALUES(input_tokens),
  output_tokens = VALUES(output_tokens), total_cost = VALUES(total_cost),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(hour_utc)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn upsert_mysql_stats_daily_model_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    let result = sqlx::query(
        r#"
INSERT INTO stats_daily_model (
  id, `date`, model, total_requests, input_tokens, output_tokens,
  cache_creation_tokens, cache_read_tokens, total_cost, avg_response_time_ms,
  created_at, updated_at
)
SELECT SHA2(UUID(), 256), ?, model, COUNT(*),
  COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
  COALESCE(SUM(cache_creation_input_tokens), 0),
  COALESCE(SUM(cache_read_input_tokens), 0), COALESCE(SUM(total_cost_usd), 0),
  COALESCE(AVG(response_time_ms), 0), ?, ?
FROM `usage`
WHERE created_at_unix_ms >= ? AND created_at_unix_ms < ?
  AND model IS NOT NULL AND model <> ''
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
GROUP BY model
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), input_tokens = VALUES(input_tokens),
  output_tokens = VALUES(output_tokens), cache_creation_tokens = VALUES(cache_creation_tokens),
  cache_read_tokens = VALUES(cache_read_tokens), total_cost = VALUES(total_cost),
  avg_response_time_ms = VALUES(avg_response_time_ms), updated_at = VALUES(updated_at)
"#,
    )
    .bind(day_start)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn upsert_mysql_stats_daily_provider_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    let result = sqlx::query(
        r#"
INSERT INTO stats_daily_provider (
  id, `date`, provider_name, total_requests, input_tokens, output_tokens,
  cache_creation_tokens, cache_read_tokens, total_cost, created_at, updated_at
)
SELECT SHA2(UUID(), 256), ?, provider_name, COUNT(*),
  COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
  COALESCE(SUM(cache_creation_input_tokens), 0),
  COALESCE(SUM(cache_read_input_tokens), 0), COALESCE(SUM(total_cost_usd), 0), ?, ?
FROM `usage`
WHERE created_at_unix_ms >= ? AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
GROUP BY provider_name
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), input_tokens = VALUES(input_tokens),
  output_tokens = VALUES(output_tokens), cache_creation_tokens = VALUES(cache_creation_tokens),
  cache_read_tokens = VALUES(cache_read_tokens), total_cost = VALUES(total_cost),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(day_start)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn upsert_mysql_stats_daily_api_key_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    let result = sqlx::query(
        r#"
INSERT INTO stats_daily_api_key (
  id, api_key_id, `date`, total_requests, success_requests, error_requests,
  sla_eligible_requests, user_error_requests,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_cost, api_key_name, created_at, updated_at
)
SELECT SHA2(UUID(), 256), usage.api_key_id, ?, COUNT(*),
  COALESCE(SUM(CASE WHEN usage.outcome_class = 'success' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.outcome_class = 'service_error' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.sla_eligible <> 0 THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.outcome_class = 'user_error' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(usage.input_tokens), 0), COALESCE(SUM(usage.output_tokens), 0),
  COALESCE(SUM(usage.cache_creation_input_tokens), 0),
  COALESCE(SUM(usage.cache_read_input_tokens), 0),
  COALESCE(SUM(usage.total_cost_usd), 0), MAX(api_keys.name), ?, ?
FROM `usage` AS `usage`
LEFT JOIN api_keys ON api_keys.id = usage.api_key_id
WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
  AND usage.api_key_id IS NOT NULL AND usage.api_key_id <> ''
GROUP BY usage.api_key_id
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), success_requests = VALUES(success_requests),
  error_requests = VALUES(error_requests), input_tokens = VALUES(input_tokens),
  sla_eligible_requests = VALUES(sla_eligible_requests),
  user_error_requests = VALUES(user_error_requests),
  output_tokens = VALUES(output_tokens), cache_creation_tokens = VALUES(cache_creation_tokens),
  cache_read_tokens = VALUES(cache_read_tokens), total_cost = VALUES(total_cost),
  api_key_name = COALESCE(VALUES(api_key_name), stats_daily_api_key.api_key_name),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(day_start)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn refresh_mysql_stats_daily_error_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    sqlx::query("DELETE FROM stats_daily_error WHERE `date` = ?")
        .bind(day_start)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    let result = sqlx::query(
        r#"
INSERT INTO stats_daily_error (
  id, `date`, error_category, provider_name, model, count, created_at, updated_at
)
SELECT SHA2(UUID(), 256), ?, error_category, provider_name, model, COUNT(*), ?, ?
FROM `usage`
WHERE created_at_unix_ms >= ? AND created_at_unix_ms < ?
  AND outcome_class = 'service_error'
  AND error_category IS NOT NULL AND error_category <> ''
GROUP BY error_category, provider_name, model
"#,
    )
    .bind(day_start)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn upsert_mysql_stats_user_daily_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<usize, DataLayerError> {
    let result = sqlx::query(
        r#"
INSERT INTO stats_user_daily (
  id, user_id, `date`, total_requests, success_requests, error_requests,
  sla_eligible_requests, user_error_requests,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  total_cost, username, created_at, updated_at
)
SELECT SHA2(UUID(), 256), usage.user_id, ?, COUNT(*),
  COALESCE(SUM(CASE WHEN usage.outcome_class = 'success' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.outcome_class = 'service_error' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.sla_eligible <> 0 THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.outcome_class = 'user_error' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(usage.input_tokens), 0), COALESCE(SUM(usage.output_tokens), 0),
  COALESCE(SUM(usage.cache_creation_input_tokens), 0),
  COALESCE(SUM(usage.cache_read_input_tokens), 0),
  COALESCE(SUM(usage.total_cost_usd), 0), MAX(users.username), ?, ?
FROM `usage` AS `usage`
LEFT JOIN users ON users.id = usage.user_id
WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
  AND usage.user_id IS NOT NULL AND usage.user_id <> ''
  AND usage.status NOT IN ('pending', 'streaming')
  AND usage.provider_name NOT IN ('unknown', 'pending')
GROUP BY usage.user_id
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), success_requests = VALUES(success_requests),
  error_requests = VALUES(error_requests), input_tokens = VALUES(input_tokens),
  sla_eligible_requests = VALUES(sla_eligible_requests),
  user_error_requests = VALUES(user_error_requests),
  output_tokens = VALUES(output_tokens), cache_creation_tokens = VALUES(cache_creation_tokens),
  cache_read_tokens = VALUES(cache_read_tokens), total_cost = VALUES(total_cost),
  username = COALESCE(VALUES(username), stats_user_daily.username),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(day_start)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(start_unix_secs)
    .bind(end_unix_secs)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(usize::try_from(result.rows_affected()).unwrap_or(usize::MAX))
}

async fn mysql_daily_fallback_count(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    start_unix_secs: i64,
    end_unix_secs: i64,
) -> Result<i64, DataLayerError> {
    let start_unix_ms = start_unix_secs.checked_mul(1000).ok_or_else(|| {
        DataLayerError::InvalidInput("stats fallback window start overflows milliseconds".into())
    })?;
    let end_unix_ms = end_unix_secs.checked_mul(1000).ok_or_else(|| {
        DataLayerError::InvalidInput("stats fallback window end overflows milliseconds".into())
    })?;
    sqlx::query_scalar(
        r#"
SELECT COUNT(*)
FROM (
  SELECT request_id
  FROM request_candidates
  WHERE created_at >= ? AND created_at < ?
    AND status IN ('success', 'failed')
  GROUP BY request_id
  HAVING COUNT(id) > 1
) AS fallback_requests
"#,
    )
    .bind(start_unix_ms)
    .bind(end_unix_ms)
    .fetch_one(&mut **tx)
    .await
    .map_sql_err()
}

async fn mysql_group_count(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    group_columns: &str,
    start_unix_secs: i64,
    end_unix_secs: i64,
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
  FROM `usage`
  WHERE created_at_unix_ms >= ?
    AND created_at_unix_ms < ?
    AND status NOT IN ('pending', 'streaming')
    AND provider_name NOT IN ('unknown', 'pending')
    AND {not_empty}
  GROUP BY {group_columns}
) AS grouped
"#
    );
    let count: i64 = sqlx::query_scalar(&sql)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()?;
    Ok(usize::try_from(count.max(0)).unwrap_or(usize::MAX))
}
