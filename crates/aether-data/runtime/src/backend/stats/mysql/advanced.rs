use sqlx::MySql;

use crate::backend::stats_common::stats_id;
use crate::error::SqlResultExt;
use crate::DataLayerError;

const CACHE_5M: &str = r#"GREATEST(
    COALESCE(usage.cache_creation_input_tokens_5m, 0),
    COALESCE(usage.cache_creation_ephemeral_5m_input_tokens, 0)
)"#;
const CACHE_1H: &str = r#"GREATEST(
    COALESCE(usage.cache_creation_input_tokens_1h, 0),
    COALESCE(usage.cache_creation_ephemeral_1h_input_tokens, 0)
)"#;
const CACHE_CREATION: &str = r#"CASE
    WHEN COALESCE(usage.cache_creation_input_tokens, 0) = 0
      AND ({cache_5m} + {cache_1h}) > 0
    THEN {cache_5m} + {cache_1h}
    ELSE GREATEST(COALESCE(usage.cache_creation_input_tokens, 0), 0)
END"#;
const EFFECTIVE_INPUT: &str = r#"CASE
    WHEN SUBSTRING_INDEX(
        LOWER(COALESCE(usage.endpoint_api_format, usage.api_format, '')), ':', 1
    ) IN ('openai', 'gemini', 'google')
      AND COALESCE(usage.input_tokens, 0) > 0
      AND COALESCE(usage.cache_read_input_tokens, 0) > 0
    THEN GREATEST(COALESCE(usage.input_tokens, 0) - COALESCE(usage.cache_read_input_tokens, 0), 0)
    ELSE GREATEST(COALESCE(usage.input_tokens, 0), 0)
END"#;
const SUCCESS: &str = r#"CASE
    WHEN usage.outcome_class = 'success' THEN 1 ELSE 0
END"#;
const SUCCESS_PERCENTILE_PREDICATE: &str = r#"(
    usage.outcome_class = 'success'
    OR (
        usage.outcome_class = 'in_flight'
        AND LOWER(TRIM(COALESCE(usage.status, ''))) = 'completed'
        AND (usage.status_code IS NULL OR usage.status_code < 400)
        AND TRIM(COALESCE(usage.error_message, '')) = ''
    )
)"#;
const AGGREGATABLE: &str = r#"usage.status NOT IN ('pending', 'streaming')
    AND usage.provider_name NOT IN ('unknown', 'pending')"#;
const SETTLED: &str = r#"COALESCE(settlement.billing_status, usage.billing_status) = 'settled'
    AND COALESCE(settlement.billing_total_cost_usd, usage.total_cost_usd, 0) > 0"#;

fn cache_creation_expr() -> String {
    CACHE_CREATION
        .replace("{cache_5m}", CACHE_5M)
        .replace("{cache_1h}", CACHE_1H)
}

fn total_input_context_expr() -> String {
    format!(
        "({EFFECTIVE_INPUT}) + ({}) + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)",
        cache_creation_expr()
    )
}

fn total_tokens_expr() -> String {
    format!(
        r#"COALESCE(
    NULLIF(GREATEST(COALESCE(usage.total_tokens, 0), 0), 0),
    ({EFFECTIVE_INPUT})
      + GREATEST(COALESCE(usage.output_tokens, 0), 0)
      + ({})
      + GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0),
    0
)"#,
        cache_creation_expr()
    )
}

fn percentile_cont(sorted: &[i64], percentile: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let position = percentile * (sorted.len().saturating_sub(1) as f64);
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let fraction = position - lower as f64;
    let value = sorted[lower] as f64 + (sorted[upper] - sorted[lower]) as f64 * fraction;
    Some(value.round() as i64)
}

async fn load_percentiles(
    tx: &mut sqlx::Transaction<'_, MySql>,
    column: &str,
    start_unix_secs: i64,
    end_unix_secs: i64,
) -> Result<(Option<i64>, Option<i64>, Option<i64>), DataLayerError> {
    let sql = format!(
        r#"
SELECT {column}
FROM `usage`
WHERE created_at_unix_ms >= ? AND created_at_unix_ms < ?
  AND {SUCCESS_PERCENTILE_PREDICATE}
  AND provider_name NOT IN ('unknown', 'pending')
  AND {column} IS NOT NULL
ORDER BY {column}
"#
    );
    let values: Vec<i64> = sqlx::query_scalar(&sql)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .fetch_all(&mut **tx)
        .await
        .map_sql_err()?;
    if values.len() < 10 {
        return Ok((None, None, None));
    }
    Ok((
        percentile_cont(&values, 0.50),
        percentile_cont(&values, 0.90),
        percentile_cont(&values, 0.99),
    ))
}

pub(super) async fn refresh_hourly(
    tx: &mut sqlx::Transaction<'_, MySql>,
    hour_utc: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
) -> Result<(), DataLayerError> {
    let cache_creation = cache_creation_expr();
    let total_context = total_input_context_expr();
    let sql = format!(
        r#"
UPDATE stats_hourly AS target
JOIN (
  SELECT
    COUNT(*) AS cache_hit_total_requests,
    COALESCE(SUM(CASE WHEN COALESCE(usage.cache_read_input_tokens, 0) > 0 THEN 1 ELSE 0 END), 0) AS cache_hit_requests,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN 1 ELSE 0 END), 0) AS completed_total_requests,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' AND COALESCE(usage.cache_read_input_tokens, 0) > 0 THEN 1 ELSE 0 END), 0) AS completed_cache_hit_requests,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN GREATEST(COALESCE(usage.input_tokens, 0), 0) ELSE 0 END), 0) AS completed_input_tokens,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN ({cache_creation}) ELSE 0 END), 0) AS completed_cache_creation_tokens,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) ELSE 0 END), 0) AS completed_cache_read_tokens,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN ({total_context}) ELSE 0 END), 0) AS completed_total_input_context,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN COALESCE(usage.cache_creation_cost_usd, 0) ELSE 0 END), 0) AS completed_cache_creation_cost,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN COALESCE(usage.cache_read_cost_usd, 0) ELSE 0 END), 0) AS completed_cache_read_cost,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN COALESCE(settlement.billing_total_cost_usd, usage.total_cost_usd, 0) ELSE 0 END), 0) AS settled_total_cost,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN 1 ELSE 0 END), 0) AS settled_total_requests,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.input_tokens, 0), 0) ELSE 0 END), 0) AS settled_input_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.output_tokens, 0), 0) ELSE 0 END), 0) AS settled_output_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN ({cache_creation}) ELSE 0 END), 0) AS settled_cache_creation_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) ELSE 0 END), 0) AS settled_cache_read_tokens,
    MIN(CASE WHEN {SETTLED} THEN COALESCE(settlement.finalized_at, usage.finalized_at) END) AS settled_first_finalized_at_unix_secs,
    MAX(CASE WHEN {SETTLED} THEN COALESCE(settlement.finalized_at, usage.finalized_at) END) AS settled_last_finalized_at_unix_secs,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL AND {AGGREGATABLE} THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0) AS response_time_sum_ms,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL AND {AGGREGATABLE} THEN 1 ELSE 0 END), 0) AS response_time_samples
    FROM `usage` AS `usage`
  LEFT JOIN usage_settlement_snapshots AS settlement ON settlement.request_id = usage.request_id
  WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
) AS aggregated
SET
  target.cache_hit_total_requests = aggregated.cache_hit_total_requests,
  target.cache_hit_requests = aggregated.cache_hit_requests,
  target.completed_total_requests = aggregated.completed_total_requests,
  target.completed_cache_hit_requests = aggregated.completed_cache_hit_requests,
  target.completed_input_tokens = aggregated.completed_input_tokens,
  target.completed_cache_creation_tokens = aggregated.completed_cache_creation_tokens,
  target.completed_cache_read_tokens = aggregated.completed_cache_read_tokens,
  target.completed_total_input_context = aggregated.completed_total_input_context,
  target.completed_cache_creation_cost = aggregated.completed_cache_creation_cost,
  target.completed_cache_read_cost = aggregated.completed_cache_read_cost,
  target.settled_total_cost = aggregated.settled_total_cost,
  target.settled_total_requests = aggregated.settled_total_requests,
  target.settled_input_tokens = aggregated.settled_input_tokens,
  target.settled_output_tokens = aggregated.settled_output_tokens,
  target.settled_cache_creation_tokens = aggregated.settled_cache_creation_tokens,
  target.settled_cache_read_tokens = aggregated.settled_cache_read_tokens,
  target.settled_first_finalized_at_unix_secs = aggregated.settled_first_finalized_at_unix_secs,
  target.settled_last_finalized_at_unix_secs = aggregated.settled_last_finalized_at_unix_secs,
  target.response_time_sum_ms = aggregated.response_time_sum_ms,
  target.response_time_samples = aggregated.response_time_samples
WHERE target.hour_utc = ?
"#
    );
    sqlx::query(&sql)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .bind(hour_utc)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    refresh_hourly_user(tx, hour_utc, start_unix_secs, end_unix_secs).await?;
    refresh_hourly_response_dimensions(tx, hour_utc, start_unix_secs, end_unix_secs).await
}

async fn refresh_hourly_user(
    tx: &mut sqlx::Transaction<'_, MySql>,
    hour_utc: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
) -> Result<(), DataLayerError> {
    let cache_creation = cache_creation_expr();
    let sql = format!(
        r#"
UPDATE stats_hourly_user AS target
JOIN (
  SELECT usage.user_id,
    COALESCE(SUM({cache_creation}), 0) AS cache_creation_tokens,
    COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
    COALESCE(SUM(COALESCE(settlement.billing_actual_total_cost_usd, usage.actual_total_cost_usd, 0)), 0) AS actual_total_cost,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0) AS response_time_sum_ms,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0) AS response_time_samples,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN COALESCE(settlement.billing_total_cost_usd, usage.total_cost_usd, 0) ELSE 0 END), 0) AS settled_total_cost,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN 1 ELSE 0 END), 0) AS settled_total_requests,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.input_tokens, 0), 0) ELSE 0 END), 0) AS settled_input_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.output_tokens, 0), 0) ELSE 0 END), 0) AS settled_output_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN ({cache_creation}) ELSE 0 END), 0) AS settled_cache_creation_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) ELSE 0 END), 0) AS settled_cache_read_tokens,
    MIN(CASE WHEN {SETTLED} THEN COALESCE(settlement.finalized_at, usage.finalized_at) END) AS settled_first_finalized_at_unix_secs,
    MAX(CASE WHEN {SETTLED} THEN COALESCE(settlement.finalized_at, usage.finalized_at) END) AS settled_last_finalized_at_unix_secs
    FROM `usage` AS `usage`
  LEFT JOIN usage_settlement_snapshots AS settlement ON settlement.request_id = usage.request_id
  WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
    AND usage.user_id IS NOT NULL AND usage.user_id <> '' AND {AGGREGATABLE}
  GROUP BY usage.user_id
) AS aggregated ON target.user_id = aggregated.user_id
SET target.cache_creation_tokens = aggregated.cache_creation_tokens,
  target.cache_read_tokens = aggregated.cache_read_tokens,
  target.actual_total_cost = aggregated.actual_total_cost,
  target.response_time_sum_ms = aggregated.response_time_sum_ms,
  target.response_time_samples = aggregated.response_time_samples,
  target.settled_total_cost = aggregated.settled_total_cost,
  target.settled_total_requests = aggregated.settled_total_requests,
  target.settled_input_tokens = aggregated.settled_input_tokens,
  target.settled_output_tokens = aggregated.settled_output_tokens,
  target.settled_cache_creation_tokens = aggregated.settled_cache_creation_tokens,
  target.settled_cache_read_tokens = aggregated.settled_cache_read_tokens,
  target.settled_first_finalized_at_unix_secs = aggregated.settled_first_finalized_at_unix_secs,
  target.settled_last_finalized_at_unix_secs = aggregated.settled_last_finalized_at_unix_secs
WHERE target.hour_utc = ?
"#
    );
    sqlx::query(&sql)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .bind(hour_utc)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    Ok(())
}

async fn refresh_hourly_response_dimensions(
    tx: &mut sqlx::Transaction<'_, MySql>,
    hour_utc: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
) -> Result<(), DataLayerError> {
    for (table, select_dimensions, group_by, join) in [
        (
            "stats_hourly_model",
            "usage.model AS model",
            "usage.model",
            "target.model = aggregated.model",
        ),
        (
            "stats_hourly_user_model",
            "usage.user_id AS user_id, usage.model AS model",
            "usage.user_id, usage.model",
            "target.user_id = aggregated.user_id AND target.model = aggregated.model",
        ),
    ] {
        let sql = format!(
            r#"
UPDATE {table} AS target
JOIN (
  SELECT {select_dimensions},
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0) AS response_time_sum_ms,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0) AS response_time_samples
    FROM `usage` AS `usage`
  WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ? AND {AGGREGATABLE}
  GROUP BY {group_by}
) AS aggregated ON {join}
SET target.response_time_sum_ms = aggregated.response_time_sum_ms,
    target.response_time_samples = aggregated.response_time_samples
WHERE target.hour_utc = ?
"#
        );
        sqlx::query(&sql)
            .bind(start_unix_secs)
            .bind(end_unix_secs)
            .bind(hour_utc)
            .execute(&mut **tx)
            .await
            .map_sql_err()?;
    }
    Ok(())
}

pub(super) async fn refresh_daily(
    tx: &mut sqlx::Transaction<'_, MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<(), DataLayerError> {
    let response = load_percentiles(tx, "response_time_ms", start_unix_secs, end_unix_secs).await?;
    let first_byte =
        load_percentiles(tx, "first_byte_time_ms", start_unix_secs, end_unix_secs).await?;
    refresh_daily_root(
        tx,
        day_start,
        start_unix_secs,
        end_unix_secs,
        response,
        first_byte,
    )
    .await?;
    refresh_daily_existing_dimensions(tx, day_start, start_unix_secs, end_unix_secs).await?;
    upsert_user_dimension(
        tx,
        "stats_user_daily_model",
        "model",
        "usage.model",
        "usage.model IS NOT NULL AND usage.model <> ''",
        day_start,
        start_unix_secs,
        end_unix_secs,
        now_unix_secs,
    )
    .await?;
    upsert_user_dimension(
        tx,
        "stats_user_daily_provider",
        "provider_name",
        "usage.provider_name",
        "usage.provider_name IS NOT NULL AND usage.provider_name <> ''",
        day_start,
        start_unix_secs,
        end_unix_secs,
        now_unix_secs,
    )
    .await?;
    upsert_user_dimension(
        tx,
        "stats_user_daily_api_format",
        "api_format",
        "LOWER(COALESCE(usage.endpoint_api_format, usage.api_format, ''))",
        "COALESCE(usage.endpoint_api_format, usage.api_format, '') <> ''",
        day_start,
        start_unix_secs,
        end_unix_secs,
        now_unix_secs,
    )
    .await?;
    upsert_model_provider_rows(tx, day_start, start_unix_secs, end_unix_secs, now_unix_secs)
        .await?;
    upsert_cost_savings_rows(tx, day_start, start_unix_secs, end_unix_secs, now_unix_secs).await?;
    refresh_user_summary(tx, end_unix_secs, now_unix_secs).await
}

async fn refresh_daily_root(
    tx: &mut sqlx::Transaction<'_, MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    response: (Option<i64>, Option<i64>, Option<i64>),
    first_byte: (Option<i64>, Option<i64>, Option<i64>),
) -> Result<(), DataLayerError> {
    let cache_creation = cache_creation_expr();
    let total_context = total_input_context_expr();
    let sql = format!(
        r#"
UPDATE stats_daily AS target
JOIN (
  SELECT
    COALESCE(SUM(CASE WHEN {AGGREGATABLE} THEN ({EFFECTIVE_INPUT}) ELSE 0 END), 0) AS effective_input_tokens,
    COALESCE(SUM(CASE WHEN {AGGREGATABLE} THEN ({total_context}) ELSE 0 END), 0) AS total_input_context,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL AND {AGGREGATABLE} THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0) AS response_time_sum_ms,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL AND {AGGREGATABLE} THEN 1 ELSE 0 END), 0) AS response_time_samples,
    COALESCE(SUM(CASE WHEN {AGGREGATABLE} THEN {CACHE_5M} ELSE 0 END), 0) AS cache_creation_ephemeral_5m_tokens,
    COALESCE(SUM(CASE WHEN {AGGREGATABLE} THEN {CACHE_1H} ELSE 0 END), 0) AS cache_creation_ephemeral_1h_tokens,
    COALESCE(SUM(CASE WHEN {AGGREGATABLE} THEN COALESCE(usage.input_cost_usd, 0) ELSE 0 END), 0) AS input_cost,
    COALESCE(SUM(CASE WHEN {AGGREGATABLE} THEN COALESCE(usage.output_cost_usd, 0) ELSE 0 END), 0) AS output_cost,
    COALESCE(SUM(CASE WHEN {AGGREGATABLE} THEN COALESCE(usage.cache_creation_cost_usd, 0) ELSE 0 END), 0) AS cache_creation_cost,
    COALESCE(SUM(CASE WHEN {AGGREGATABLE} THEN COALESCE(usage.cache_read_cost_usd, 0) ELSE 0 END), 0) AS cache_read_cost,
    COUNT(*) AS cache_hit_total_requests,
    COALESCE(SUM(CASE WHEN COALESCE(usage.cache_read_input_tokens, 0) > 0 THEN 1 ELSE 0 END), 0) AS cache_hit_requests,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN 1 ELSE 0 END), 0) AS completed_total_requests,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' AND COALESCE(usage.cache_read_input_tokens, 0) > 0 THEN 1 ELSE 0 END), 0) AS completed_cache_hit_requests,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN GREATEST(COALESCE(usage.input_tokens, 0), 0) ELSE 0 END), 0) AS completed_input_tokens,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN ({cache_creation}) ELSE 0 END), 0) AS completed_cache_creation_tokens,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) ELSE 0 END), 0) AS completed_cache_read_tokens,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN ({total_context}) ELSE 0 END), 0) AS completed_total_input_context,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN COALESCE(usage.cache_creation_cost_usd, 0) ELSE 0 END), 0) AS completed_cache_creation_cost,
    COALESCE(SUM(CASE WHEN usage.status = 'completed' THEN COALESCE(usage.cache_read_cost_usd, 0) ELSE 0 END), 0) AS completed_cache_read_cost,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN COALESCE(settlement.billing_total_cost_usd, usage.total_cost_usd, 0) ELSE 0 END), 0) AS settled_total_cost,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN 1 ELSE 0 END), 0) AS settled_total_requests,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.input_tokens, 0), 0) ELSE 0 END), 0) AS settled_input_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.output_tokens, 0), 0) ELSE 0 END), 0) AS settled_output_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN ({cache_creation}) ELSE 0 END), 0) AS settled_cache_creation_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) ELSE 0 END), 0) AS settled_cache_read_tokens,
    MIN(CASE WHEN {SETTLED} THEN COALESCE(settlement.finalized_at, usage.finalized_at) END) AS settled_first_finalized_at_unix_secs,
    MAX(CASE WHEN {SETTLED} THEN COALESCE(settlement.finalized_at, usage.finalized_at) END) AS settled_last_finalized_at_unix_secs
    FROM `usage` AS `usage`
  LEFT JOIN usage_settlement_snapshots AS settlement ON settlement.request_id = usage.request_id
  WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
) AS aggregated
SET target.effective_input_tokens = aggregated.effective_input_tokens,
  target.total_input_context = aggregated.total_input_context,
  target.response_time_sum_ms = aggregated.response_time_sum_ms,
  target.response_time_samples = aggregated.response_time_samples,
  target.cache_creation_ephemeral_5m_tokens = aggregated.cache_creation_ephemeral_5m_tokens,
  target.cache_creation_ephemeral_1h_tokens = aggregated.cache_creation_ephemeral_1h_tokens,
  target.input_cost = aggregated.input_cost,
  target.output_cost = aggregated.output_cost,
  target.cache_creation_cost = aggregated.cache_creation_cost,
  target.cache_read_cost = aggregated.cache_read_cost,
  target.cache_hit_total_requests = aggregated.cache_hit_total_requests,
  target.cache_hit_requests = aggregated.cache_hit_requests,
  target.completed_total_requests = aggregated.completed_total_requests,
  target.completed_cache_hit_requests = aggregated.completed_cache_hit_requests,
  target.completed_input_tokens = aggregated.completed_input_tokens,
  target.completed_cache_creation_tokens = aggregated.completed_cache_creation_tokens,
  target.completed_cache_read_tokens = aggregated.completed_cache_read_tokens,
  target.completed_total_input_context = aggregated.completed_total_input_context,
  target.completed_cache_creation_cost = aggregated.completed_cache_creation_cost,
  target.completed_cache_read_cost = aggregated.completed_cache_read_cost,
  target.settled_total_cost = aggregated.settled_total_cost,
  target.settled_total_requests = aggregated.settled_total_requests,
  target.settled_input_tokens = aggregated.settled_input_tokens,
  target.settled_output_tokens = aggregated.settled_output_tokens,
  target.settled_cache_creation_tokens = aggregated.settled_cache_creation_tokens,
  target.settled_cache_read_tokens = aggregated.settled_cache_read_tokens,
  target.settled_first_finalized_at_unix_secs = aggregated.settled_first_finalized_at_unix_secs,
  target.settled_last_finalized_at_unix_secs = aggregated.settled_last_finalized_at_unix_secs,
  target.p50_response_time_ms = ?, target.p90_response_time_ms = ?, target.p99_response_time_ms = ?,
  target.p50_first_byte_time_ms = ?, target.p90_first_byte_time_ms = ?, target.p99_first_byte_time_ms = ?
WHERE target.`date` = ?
"#
    );
    sqlx::query(&sql)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .bind(response.0)
        .bind(response.1)
        .bind(response.2)
        .bind(first_byte.0)
        .bind(first_byte.1)
        .bind(first_byte.2)
        .bind(day_start)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    Ok(())
}

async fn refresh_daily_existing_dimensions(
    tx: &mut sqlx::Transaction<'_, MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
) -> Result<(), DataLayerError> {
    let cache_creation = cache_creation_expr();
    let total_context = total_input_context_expr();
    let model_sql = format!(
        r#"
UPDATE stats_daily_model AS target
JOIN (
  SELECT usage.model,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0) AS response_time_sum_ms,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0) AS response_time_samples,
    COALESCE(SUM({CACHE_5M}), 0) AS cache_creation_ephemeral_5m_tokens,
    COALESCE(SUM({CACHE_1H}), 0) AS cache_creation_ephemeral_1h_tokens
    FROM `usage` AS `usage`
  WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
    AND {AGGREGATABLE} AND usage.model IS NOT NULL AND usage.model <> ''
  GROUP BY usage.model
) AS aggregated ON target.model = aggregated.model
SET target.response_time_sum_ms = aggregated.response_time_sum_ms,
  target.response_time_samples = aggregated.response_time_samples,
  target.cache_creation_ephemeral_5m_tokens = aggregated.cache_creation_ephemeral_5m_tokens,
  target.cache_creation_ephemeral_1h_tokens = aggregated.cache_creation_ephemeral_1h_tokens
WHERE target.`date` = ?
"#
    );
    sqlx::query(&model_sql)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .bind(day_start)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;

    let user_sql = format!(
        r#"
UPDATE stats_user_daily AS target
JOIN (
  SELECT usage.user_id,
    COALESCE(SUM({EFFECTIVE_INPUT}), 0) AS effective_input_tokens,
    COALESCE(SUM({total_context}), 0) AS total_input_context,
    COALESCE(SUM(COALESCE(usage.cache_creation_cost_usd, 0)), 0) AS cache_creation_cost,
    COALESCE(SUM(COALESCE(usage.cache_read_cost_usd, 0)), 0) AS cache_read_cost,
    COALESCE(SUM(COALESCE(settlement.billing_actual_total_cost_usd, usage.actual_total_cost_usd, 0)), 0) AS actual_total_cost,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0) AS response_time_sum_ms,
    COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0) AS response_time_samples,
    COALESCE(SUM({CACHE_5M}), 0) AS cache_creation_ephemeral_5m_tokens,
    COALESCE(SUM({CACHE_1H}), 0) AS cache_creation_ephemeral_1h_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN COALESCE(settlement.billing_total_cost_usd, usage.total_cost_usd, 0) ELSE 0 END), 0) AS settled_total_cost,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN 1 ELSE 0 END), 0) AS settled_total_requests,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.input_tokens, 0), 0) ELSE 0 END), 0) AS settled_input_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.output_tokens, 0), 0) ELSE 0 END), 0) AS settled_output_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN ({cache_creation}) ELSE 0 END), 0) AS settled_cache_creation_tokens,
    COALESCE(SUM(CASE WHEN {SETTLED} THEN GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) ELSE 0 END), 0) AS settled_cache_read_tokens,
    MIN(CASE WHEN {SETTLED} THEN COALESCE(settlement.finalized_at, usage.finalized_at) END) AS settled_first_finalized_at_unix_secs,
    MAX(CASE WHEN {SETTLED} THEN COALESCE(settlement.finalized_at, usage.finalized_at) END) AS settled_last_finalized_at_unix_secs
    FROM `usage` AS `usage`
  LEFT JOIN usage_settlement_snapshots AS settlement ON settlement.request_id = usage.request_id
  WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
    AND usage.user_id IS NOT NULL AND usage.user_id <> '' AND {AGGREGATABLE}
  GROUP BY usage.user_id
) AS aggregated ON target.user_id = aggregated.user_id
SET target.effective_input_tokens = aggregated.effective_input_tokens,
  target.total_input_context = aggregated.total_input_context,
  target.cache_creation_cost = aggregated.cache_creation_cost,
  target.cache_read_cost = aggregated.cache_read_cost,
  target.actual_total_cost = aggregated.actual_total_cost,
  target.response_time_sum_ms = aggregated.response_time_sum_ms,
  target.response_time_samples = aggregated.response_time_samples,
  target.cache_creation_ephemeral_5m_tokens = aggregated.cache_creation_ephemeral_5m_tokens,
  target.cache_creation_ephemeral_1h_tokens = aggregated.cache_creation_ephemeral_1h_tokens,
  target.settled_total_cost = aggregated.settled_total_cost,
  target.settled_total_requests = aggregated.settled_total_requests,
  target.settled_input_tokens = aggregated.settled_input_tokens,
  target.settled_output_tokens = aggregated.settled_output_tokens,
  target.settled_cache_creation_tokens = aggregated.settled_cache_creation_tokens,
  target.settled_cache_read_tokens = aggregated.settled_cache_read_tokens,
  target.settled_first_finalized_at_unix_secs = aggregated.settled_first_finalized_at_unix_secs,
  target.settled_last_finalized_at_unix_secs = aggregated.settled_last_finalized_at_unix_secs
WHERE target.`date` = ?
"#
    );
    sqlx::query(&user_sql)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .bind(day_start)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn upsert_user_dimension(
    tx: &mut sqlx::Transaction<'_, MySql>,
    table: &str,
    dimension_column: &str,
    dimension_expr: &str,
    dimension_filter: &str,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<(), DataLayerError> {
    let cache_creation = cache_creation_expr();
    let total_context = total_input_context_expr();
    let total_tokens = total_tokens_expr();
    let sql = format!(
        r#"
INSERT INTO {table} (
  id, user_id, username, `date`, {dimension_column}, total_requests, success_requests,
  sla_eligible_requests, user_error_requests,
  input_tokens, effective_input_tokens, output_tokens, total_tokens, total_input_context,
  cache_creation_tokens, cache_creation_ephemeral_5m_tokens,
  cache_creation_ephemeral_1h_tokens, cache_read_tokens, total_cost, actual_total_cost,
  response_time_sum_ms, response_time_samples, successful_response_time_sum_ms,
  successful_response_time_samples, created_at, updated_at
)
SELECT SHA2(UUID(), 256), usage.user_id,
  MAX(COALESCE(usage.username, users.username)), ?, {dimension_expr}, COUNT(*),
  COALESCE(SUM({SUCCESS}), 0),
  COALESCE(SUM(CASE WHEN usage.sla_eligible <> 0 THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.outcome_class = 'user_error' THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(GREATEST(COALESCE(usage.input_tokens, 0), 0)), 0),
  COALESCE(SUM({EFFECTIVE_INPUT}), 0),
  COALESCE(SUM(GREATEST(COALESCE(usage.output_tokens, 0), 0)), 0),
  COALESCE(SUM({total_tokens}), 0), COALESCE(SUM({total_context}), 0),
  COALESCE(SUM({cache_creation}), 0), COALESCE(SUM({CACHE_5M}), 0),
  COALESCE(SUM({CACHE_1H}), 0),
  COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0),
  COALESCE(SUM(COALESCE(settlement.billing_total_cost_usd, usage.total_cost_usd, 0)), 0),
  COALESCE(SUM(COALESCE(settlement.billing_actual_total_cost_usd, usage.actual_total_cost_usd, 0)), 0),
  COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN ({SUCCESS}) = 1 AND usage.response_time_ms IS NOT NULL THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN ({SUCCESS}) = 1 AND usage.response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0),
  ?, ?
 FROM `usage` AS `usage`
LEFT JOIN users ON users.id = usage.user_id
LEFT JOIN usage_settlement_snapshots AS settlement ON settlement.request_id = usage.request_id
WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
  AND usage.user_id IS NOT NULL AND usage.user_id <> ''
  AND {dimension_filter} AND {AGGREGATABLE}
GROUP BY usage.user_id, {dimension_expr}
ON DUPLICATE KEY UPDATE
  username = COALESCE(VALUES(username), {table}.username),
  total_requests = VALUES(total_requests), success_requests = VALUES(success_requests),
  sla_eligible_requests = VALUES(sla_eligible_requests),
  user_error_requests = VALUES(user_error_requests),
  input_tokens = VALUES(input_tokens), effective_input_tokens = VALUES(effective_input_tokens),
  output_tokens = VALUES(output_tokens), total_tokens = VALUES(total_tokens),
  total_input_context = VALUES(total_input_context),
  cache_creation_tokens = VALUES(cache_creation_tokens),
  cache_creation_ephemeral_5m_tokens = VALUES(cache_creation_ephemeral_5m_tokens),
  cache_creation_ephemeral_1h_tokens = VALUES(cache_creation_ephemeral_1h_tokens),
  cache_read_tokens = VALUES(cache_read_tokens), total_cost = VALUES(total_cost),
  actual_total_cost = VALUES(actual_total_cost),
  response_time_sum_ms = VALUES(response_time_sum_ms),
  response_time_samples = VALUES(response_time_samples),
  successful_response_time_sum_ms = VALUES(successful_response_time_sum_ms),
  successful_response_time_samples = VALUES(successful_response_time_samples),
  updated_at = VALUES(updated_at)
"#
    );
    sqlx::query(&sql)
        .bind(day_start)
        .bind(now_unix_secs)
        .bind(now_unix_secs)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    Ok(())
}

async fn upsert_model_provider_rows(
    tx: &mut sqlx::Transaction<'_, MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<(), DataLayerError> {
    let total_tokens = total_tokens_expr();
    let model_provider_sql = format!(
        r#"
INSERT INTO stats_daily_model_provider (
  id, `date`, model, provider_name, total_requests, total_tokens, total_cost,
  response_time_sum_ms, response_time_samples, created_at, updated_at
)
SELECT SHA2(UUID(), 256), ?, usage.model, usage.provider_name, COUNT(*),
  COALESCE(SUM({total_tokens}), 0),
  COALESCE(SUM(COALESCE(settlement.billing_total_cost_usd, usage.total_cost_usd, 0)), 0),
  COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0), ?, ?
 FROM `usage` AS `usage`
LEFT JOIN usage_settlement_snapshots AS settlement ON settlement.request_id = usage.request_id
WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
  AND usage.model IS NOT NULL AND usage.model <> '' AND {AGGREGATABLE}
GROUP BY usage.model, usage.provider_name
ON DUPLICATE KEY UPDATE
  total_requests = VALUES(total_requests), total_tokens = VALUES(total_tokens),
  total_cost = VALUES(total_cost), response_time_sum_ms = VALUES(response_time_sum_ms),
  response_time_samples = VALUES(response_time_samples), updated_at = VALUES(updated_at)
"#
    );
    sqlx::query(&model_provider_sql)
        .bind(day_start)
        .bind(now_unix_secs)
        .bind(now_unix_secs)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;

    let user_model_provider_sql = format!(
        r#"
INSERT INTO stats_user_daily_model_provider (
  id, user_id, username, `date`, model, provider_name, total_requests, total_tokens,
  total_cost, response_time_sum_ms, response_time_samples, created_at, updated_at
)
SELECT SHA2(UUID(), 256), usage.user_id, MAX(COALESCE(usage.username, users.username)),
  ?, usage.model, usage.provider_name, COUNT(*), COALESCE(SUM({total_tokens}), 0),
  COALESCE(SUM(COALESCE(settlement.billing_total_cost_usd, usage.total_cost_usd, 0)), 0),
  COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN GREATEST(usage.response_time_ms, 0) ELSE 0 END), 0),
  COALESCE(SUM(CASE WHEN usage.response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0), ?, ?
 FROM `usage` AS `usage`
LEFT JOIN users ON users.id = usage.user_id
LEFT JOIN usage_settlement_snapshots AS settlement ON settlement.request_id = usage.request_id
WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ?
  AND usage.user_id IS NOT NULL AND usage.user_id <> ''
  AND usage.model IS NOT NULL AND usage.model <> '' AND {AGGREGATABLE}
GROUP BY usage.user_id, usage.model, usage.provider_name
ON DUPLICATE KEY UPDATE
  username = COALESCE(VALUES(username), stats_user_daily_model_provider.username),
  total_requests = VALUES(total_requests), total_tokens = VALUES(total_tokens),
  total_cost = VALUES(total_cost), response_time_sum_ms = VALUES(response_time_sum_ms),
  response_time_samples = VALUES(response_time_samples), updated_at = VALUES(updated_at)
"#
    );
    sqlx::query(&user_model_provider_sql)
        .bind(day_start)
        .bind(now_unix_secs)
        .bind(now_unix_secs)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    Ok(())
}

async fn upsert_cost_savings_rows(
    tx: &mut sqlx::Transaction<'_, MySql>,
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<(), DataLayerError> {
    for (table, dimensions) in [
        ("stats_daily_cost_savings", Vec::new()),
        (
            "stats_daily_cost_savings_provider",
            vec![("provider_name", "COALESCE(usage.provider_name, '')")],
        ),
        (
            "stats_daily_cost_savings_model",
            vec![("model", "COALESCE(usage.model, '')")],
        ),
        (
            "stats_daily_cost_savings_model_provider",
            vec![
                ("model", "COALESCE(usage.model, '')"),
                ("provider_name", "COALESCE(usage.provider_name, '')"),
            ],
        ),
    ] {
        upsert_cost_savings_dimension(
            tx,
            table,
            false,
            &dimensions,
            day_start,
            start_unix_secs,
            end_unix_secs,
            now_unix_secs,
        )
        .await?;
    }
    for (table, dimensions) in [
        ("stats_user_daily_cost_savings", Vec::new()),
        (
            "stats_user_daily_cost_savings_provider",
            vec![("provider_name", "COALESCE(usage.provider_name, '')")],
        ),
        (
            "stats_user_daily_cost_savings_model",
            vec![("model", "COALESCE(usage.model, '')")],
        ),
        (
            "stats_user_daily_cost_savings_model_provider",
            vec![
                ("model", "COALESCE(usage.model, '')"),
                ("provider_name", "COALESCE(usage.provider_name, '')"),
            ],
        ),
    ] {
        upsert_cost_savings_dimension(
            tx,
            table,
            true,
            &dimensions,
            day_start,
            start_unix_secs,
            end_unix_secs,
            now_unix_secs,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn upsert_cost_savings_dimension(
    tx: &mut sqlx::Transaction<'_, MySql>,
    table: &str,
    per_user: bool,
    dimensions: &[(&str, &str)],
    day_start: i64,
    start_unix_secs: i64,
    end_unix_secs: i64,
    now_unix_secs: i64,
) -> Result<(), DataLayerError> {
    let dimension_columns = dimensions
        .iter()
        .map(|(column, _)| *column)
        .collect::<Vec<_>>();
    let dimension_exprs = dimensions
        .iter()
        .map(|(_, expression)| *expression)
        .collect::<Vec<_>>();
    let user_columns = if per_user { "user_id, username, " } else { "" };
    let user_select = if per_user {
        "usage.user_id, MAX(COALESCE(usage.username, users.username)), "
    } else {
        ""
    };
    let user_join = if per_user {
        "LEFT JOIN users ON users.id = usage.user_id"
    } else {
        ""
    };
    let user_filter = if per_user {
        "AND usage.user_id IS NOT NULL AND usage.user_id <> ''"
    } else {
        ""
    };
    let mut group_by = Vec::new();
    if per_user {
        group_by.push("usage.user_id");
    }
    group_by.extend(dimension_exprs.iter().copied());
    let dimension_columns_sql = if dimension_columns.is_empty() {
        String::new()
    } else {
        format!("{}, ", dimension_columns.join(", "))
    };
    let dimension_select_sql = if dimension_exprs.is_empty() {
        String::new()
    } else {
        format!("{}, ", dimension_exprs.join(", "))
    };
    let group_by_sql = if group_by.is_empty() {
        String::new()
    } else {
        format!("GROUP BY {}", group_by.join(", "))
    };
    let sql = format!(
        r#"
INSERT INTO {table} (
  id, {user_columns}`date`, {dimension_columns_sql}cache_read_tokens,
  cache_read_cost, cache_creation_cost, estimated_full_cost, created_at, updated_at
)
SELECT SHA2(UUID(), 256), {user_select}?, {dimension_select_sql}
  COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0),
  COALESCE(SUM(COALESCE(usage.cache_read_cost_usd, 0)), 0),
  COALESCE(SUM(COALESCE(usage.cache_creation_cost_usd, 0)), 0),
  COALESCE(SUM(
    COALESCE(settlement.input_price_per_1m, usage.input_price_per_1m, 0)
      * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0) / 1000000.0
  ), 0), ?, ?
 FROM `usage` AS `usage`
LEFT JOIN usage_settlement_snapshots AS settlement ON settlement.request_id = usage.request_id
{user_join}
WHERE usage.created_at_unix_ms >= ? AND usage.created_at_unix_ms < ? {user_filter}
{group_by_sql}
ON DUPLICATE KEY UPDATE
  {}cache_read_tokens = VALUES(cache_read_tokens),
  cache_read_cost = VALUES(cache_read_cost),
  cache_creation_cost = VALUES(cache_creation_cost),
  estimated_full_cost = VALUES(estimated_full_cost), updated_at = VALUES(updated_at)
"#,
        if per_user {
            format!("username = COALESCE(VALUES(username), {table}.username), ")
        } else {
            String::new()
        }
    );
    sqlx::query(&sql)
        .bind(day_start)
        .bind(now_unix_secs)
        .bind(now_unix_secs)
        .bind(start_unix_secs)
        .bind(end_unix_secs)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    Ok(())
}

async fn refresh_user_summary(
    tx: &mut sqlx::Transaction<'_, MySql>,
    cutoff_date: i64,
    now_unix_secs: i64,
) -> Result<(), DataLayerError> {
    sqlx::query(
        r#"
INSERT INTO stats_user_summary (
  id, user_id, username, cutoff_date, all_time_requests, all_time_success_requests,
  all_time_error_requests, all_time_sla_eligible_requests, all_time_user_error_requests,
  all_time_input_tokens, all_time_output_tokens,
  all_time_cache_creation_tokens, all_time_cache_read_tokens, all_time_cost,
  all_time_actual_cost, active_days, first_active_date, last_active_date,
  created_at, updated_at
)
SELECT SHA2(UUID(), 256), user_id, MAX(username), ?,
  COALESCE(SUM(total_requests), 0), COALESCE(SUM(success_requests), 0),
  COALESCE(SUM(error_requests), 0), COALESCE(SUM(sla_eligible_requests), 0),
  COALESCE(SUM(user_error_requests), 0), COALESCE(SUM(input_tokens), 0),
  COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cache_creation_tokens), 0),
  COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(total_cost), 0),
  COALESCE(SUM(actual_total_cost), 0),
  COALESCE(SUM(CASE WHEN total_requests > 0 THEN 1 ELSE 0 END), 0),
  MIN(CASE WHEN total_requests > 0 THEN `date` END),
  MAX(CASE WHEN total_requests > 0 THEN `date` END), ?, ?
FROM stats_user_daily
WHERE `date` < ?
GROUP BY user_id
ON DUPLICATE KEY UPDATE
  username = COALESCE(VALUES(username), stats_user_summary.username),
  cutoff_date = VALUES(cutoff_date), all_time_requests = VALUES(all_time_requests),
  all_time_success_requests = VALUES(all_time_success_requests),
  all_time_error_requests = VALUES(all_time_error_requests),
  all_time_sla_eligible_requests = VALUES(all_time_sla_eligible_requests),
  all_time_user_error_requests = VALUES(all_time_user_error_requests),
  all_time_input_tokens = VALUES(all_time_input_tokens),
  all_time_output_tokens = VALUES(all_time_output_tokens),
  all_time_cache_creation_tokens = VALUES(all_time_cache_creation_tokens),
  all_time_cache_read_tokens = VALUES(all_time_cache_read_tokens),
  all_time_cost = VALUES(all_time_cost), all_time_actual_cost = VALUES(all_time_actual_cost),
  active_days = VALUES(active_days), first_active_date = VALUES(first_active_date),
  last_active_date = VALUES(last_active_date), updated_at = VALUES(updated_at)
"#,
    )
    .bind(cutoff_date)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(cutoff_date)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    refresh_global_summary(tx, cutoff_date, now_unix_secs).await?;
    Ok(())
}

async fn refresh_global_summary(
    tx: &mut sqlx::Transaction<'_, MySql>,
    cutoff_date: i64,
    now_unix_secs: i64,
) -> Result<(), DataLayerError> {
    let existing_id: Option<String> =
        sqlx::query_scalar("SELECT id FROM stats_summary ORDER BY created_at, id LIMIT 1")
            .fetch_optional(&mut **tx)
            .await
            .map_sql_err()?;
    let summary_id = existing_id.unwrap_or_else(|| stats_id("stats-summary"));
    sqlx::query(
        r#"
INSERT INTO stats_summary (
  id, cutoff_date, all_time_requests, all_time_success_requests,
  all_time_error_requests, all_time_sla_eligible_requests, all_time_user_error_requests,
  all_time_input_tokens, all_time_output_tokens,
  all_time_cache_creation_tokens, all_time_cache_read_tokens, all_time_cost,
  all_time_actual_cost, total_users, active_users, total_api_keys,
  active_api_keys, created_at, updated_at
)
SELECT ?, ?, COALESCE(SUM(total_requests), 0), COALESCE(SUM(success_requests), 0),
  COALESCE(SUM(error_requests), 0), COALESCE(SUM(sla_eligible_requests), 0),
  COALESCE(SUM(user_error_requests), 0), COALESCE(SUM(input_tokens), 0),
  COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cache_creation_tokens), 0),
  COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(total_cost), 0),
  COALESCE(SUM(actual_total_cost), 0),
  (SELECT COUNT(*) FROM users),
  (SELECT COUNT(*) FROM users WHERE is_active <> 0),
  (SELECT COUNT(*) FROM api_keys),
  (SELECT COUNT(*) FROM api_keys WHERE is_active <> 0), ?, ?
FROM stats_daily
WHERE `date` < ?
ON DUPLICATE KEY UPDATE
  cutoff_date = VALUES(cutoff_date),
  all_time_requests = VALUES(all_time_requests),
  all_time_success_requests = VALUES(all_time_success_requests),
  all_time_error_requests = VALUES(all_time_error_requests),
  all_time_sla_eligible_requests = VALUES(all_time_sla_eligible_requests),
  all_time_user_error_requests = VALUES(all_time_user_error_requests),
  all_time_input_tokens = VALUES(all_time_input_tokens),
  all_time_output_tokens = VALUES(all_time_output_tokens),
  all_time_cache_creation_tokens = VALUES(all_time_cache_creation_tokens),
  all_time_cache_read_tokens = VALUES(all_time_cache_read_tokens),
  all_time_cost = VALUES(all_time_cost),
  all_time_actual_cost = VALUES(all_time_actual_cost),
  total_users = VALUES(total_users), active_users = VALUES(active_users),
  total_api_keys = VALUES(total_api_keys), active_api_keys = VALUES(active_api_keys),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(summary_id)
    .bind(cutoff_date)
    .bind(now_unix_secs)
    .bind(now_unix_secs)
    .bind(cutoff_date)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}
