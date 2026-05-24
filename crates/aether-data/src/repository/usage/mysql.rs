use std::collections::{BTreeMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_ai_formats::UPSTREAM_IS_STREAM_KEY;
use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use super::{
    provider_api_key_usage_is_error, provider_api_key_usage_is_success,
    strip_deprecated_usage_display_fields, usage_can_recover_terminal_failure,
    usage_request_metadata_client_family, InMemoryUsageReadRepository, PendingUsageCleanupSummary,
    StoredRequestUsageAudit, StoredUsageDailySummary, StoredUsageDashboardDailyBreakdownRow,
    StoredUsageDashboardSummary, StoredUsageUserTotals, UpsertUsageRecord, UsageDailyHeatmapQuery,
    UsageDashboardDailyBreakdownQuery, UsageDashboardSummaryQuery, UsageReadRepository,
    UsageWriteRepository,
};
use crate::driver::mysql::MysqlPool;
use crate::error::SqlResultExt;
use crate::DataLayerError;

const USAGE_COLUMNS: &str = r#"
SELECT
  id,
  request_id,
  user_id,
  api_key_id,
  provider_name,
  model,
  target_model,
  provider_id,
  provider_endpoint_id,
  provider_api_key_id,
  request_type,
  api_format,
  api_family,
  endpoint_kind,
  endpoint_api_format,
  provider_api_family,
  provider_endpoint_kind,
  has_format_conversion,
  is_stream,
  upstream_is_stream,
  input_tokens,
  output_tokens,
  total_tokens,
  cache_creation_input_tokens,
  cache_creation_ephemeral_5m_input_tokens,
  cache_creation_ephemeral_1h_input_tokens,
  cache_read_input_tokens,
  cache_creation_cost_usd,
  cache_read_cost_usd,
  output_price_per_1m,
  total_cost_usd,
  actual_total_cost_usd,
  status_code,
  error_message,
  error_category,
  response_time_ms,
  first_byte_time_ms,
  status,
  billing_status,
  request_metadata,
  candidate_id,
  candidate_index,
  key_name,
  planner_kind,
  route_family,
  route_kind,
  execution_path,
  local_execution_runtime_miss_reason,
  finalized_at AS finalized_at_unix_secs,
  created_at_unix_ms,
  updated_at_unix_secs
FROM `usage`
"#;

const UPSERT_USAGE_SQL: &str = r#"
INSERT INTO `usage` (
  request_id,
  id,
  user_id,
  api_key_id,
  provider_name,
  model,
  target_model,
  provider_id,
  provider_endpoint_id,
  provider_api_key_id,
  request_type,
  api_format,
  api_family,
  endpoint_kind,
  endpoint_api_format,
  provider_api_family,
  provider_endpoint_kind,
  has_format_conversion,
  is_stream,
  upstream_is_stream,
  input_tokens,
  output_tokens,
  total_tokens,
  cache_creation_input_tokens,
  cache_creation_ephemeral_5m_input_tokens,
  cache_creation_ephemeral_1h_input_tokens,
  cache_read_input_tokens,
  cache_creation_cost_usd,
  cache_read_cost_usd,
  output_price_per_1m,
  total_cost_usd,
  actual_total_cost_usd,
  status_code,
  error_message,
  error_category,
  response_time_ms,
  first_byte_time_ms,
  status,
  billing_status,
  request_metadata,
  candidate_id,
  candidate_index,
  key_name,
  planner_kind,
  route_family,
  route_kind,
  execution_path,
  local_execution_runtime_miss_reason,
  finalized_at,
  created_at_unix_ms,
  updated_at_unix_secs
) VALUES (
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
  ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
  ?
)
ON DUPLICATE KEY UPDATE
  user_id = VALUES(user_id),
  api_key_id = VALUES(api_key_id),
  provider_name = VALUES(provider_name),
  model = VALUES(model),
  target_model = VALUES(target_model),
  provider_id = VALUES(provider_id),
  provider_endpoint_id = VALUES(provider_endpoint_id),
  provider_api_key_id = VALUES(provider_api_key_id),
  request_type = VALUES(request_type),
  api_format = VALUES(api_format),
  api_family = VALUES(api_family),
  endpoint_kind = VALUES(endpoint_kind),
  endpoint_api_format = VALUES(endpoint_api_format),
  provider_api_family = VALUES(provider_api_family),
  provider_endpoint_kind = VALUES(provider_endpoint_kind),
  has_format_conversion = VALUES(has_format_conversion),
  is_stream = VALUES(is_stream),
  upstream_is_stream = VALUES(upstream_is_stream),
  input_tokens = VALUES(input_tokens),
  output_tokens = VALUES(output_tokens),
  total_tokens = VALUES(total_tokens),
  cache_creation_input_tokens = VALUES(cache_creation_input_tokens),
  cache_creation_ephemeral_5m_input_tokens = VALUES(cache_creation_ephemeral_5m_input_tokens),
  cache_creation_ephemeral_1h_input_tokens = VALUES(cache_creation_ephemeral_1h_input_tokens),
  cache_read_input_tokens = VALUES(cache_read_input_tokens),
  cache_creation_cost_usd = VALUES(cache_creation_cost_usd),
  cache_read_cost_usd = VALUES(cache_read_cost_usd),
  output_price_per_1m = VALUES(output_price_per_1m),
  total_cost_usd = VALUES(total_cost_usd),
  actual_total_cost_usd = VALUES(actual_total_cost_usd),
  status_code = VALUES(status_code),
  error_message = VALUES(error_message),
  error_category = VALUES(error_category),
  response_time_ms = VALUES(response_time_ms),
  first_byte_time_ms = VALUES(first_byte_time_ms),
  status = VALUES(status),
  billing_status = VALUES(billing_status),
  request_metadata = VALUES(request_metadata),
  candidate_id = VALUES(candidate_id),
  candidate_index = VALUES(candidate_index),
  key_name = VALUES(key_name),
  planner_kind = VALUES(planner_kind),
  route_family = VALUES(route_family),
  route_kind = VALUES(route_kind),
  execution_path = VALUES(execution_path),
  local_execution_runtime_miss_reason = VALUES(local_execution_runtime_miss_reason),
  finalized_at = VALUES(finalized_at),
  updated_at_unix_secs = VALUES(updated_at_unix_secs)
"#;

const SELECT_STALE_PENDING_USAGE_BATCH_SQL: &str = r#"
SELECT
  `usage`.request_id,
  `usage`.status,
  COALESCE(usage_settlement_snapshots.billing_status, `usage`.billing_status) AS billing_status
FROM `usage`
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = `usage`.request_id
WHERE `usage`.status IN ('pending', 'streaming')
  AND `usage`.created_at_unix_ms < ?
ORDER BY `usage`.created_at_unix_ms ASC, `usage`.request_id ASC
LIMIT ?
"#;

const SELECT_COMPLETED_REQUEST_CANDIDATES_SQL: &str = r#"
SELECT status, extra_data
FROM request_candidates
WHERE request_id = ?
  AND status IN ('streaming', 'success')
"#;

#[derive(Debug, Clone)]
pub struct MysqlUsageWriteRepository {
    pool: MysqlPool,
}

#[derive(Debug, Clone)]
pub struct MysqlUsageReadRepository {
    pool: MysqlPool,
}

impl MysqlUsageReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn materialize_read_model(&self) -> Result<InMemoryUsageReadRepository, DataLayerError> {
        let rows = sqlx::query(&format!(
            "{USAGE_COLUMNS} ORDER BY created_at_unix_ms ASC, request_id ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        let items = rows
            .iter()
            .map(map_usage_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(InMemoryUsageReadRepository::seed(items))
    }

    async fn summarize_usage_daily_heatmap_raw_from_range(
        &self,
        created_from_unix_secs: u64,
        created_until_unix_secs: u64,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        let mut sql = String::from(
            r#"
SELECT
  DATE_FORMAT(FROM_UNIXTIME(created_at_unix_ms), '%Y-%m-%d') AS date,
  COUNT(*) AS requests,
  COALESCE(SUM(
    GREATEST(COALESCE(input_tokens, 0), 0)
    + GREATEST(COALESCE(output_tokens, 0), 0)
    + CASE
        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
             AND (COALESCE(cache_creation_ephemeral_5m_input_tokens, 0) + COALESCE(cache_creation_ephemeral_1h_input_tokens, 0)) > 0
        THEN COALESCE(cache_creation_ephemeral_5m_input_tokens, 0) + COALESCE(cache_creation_ephemeral_1h_input_tokens, 0)
        ELSE GREATEST(COALESCE(cache_creation_input_tokens, 0), 0)
      END
    + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
  ), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(total_cost_usd, 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(actual_total_cost_usd, 0)), 0) AS actual_total_cost_usd
FROM `usage`
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#,
        );
        if user_id.is_some() {
            sql.push_str("  AND user_id = ?\n");
        }
        sql.push_str("GROUP BY date ORDER BY date ASC");

        let mut query = sqlx::query(&sql)
            .bind(to_i64(created_from_unix_secs, "usage.created_at_unix_ms")?)
            .bind(to_i64(created_until_unix_secs, "usage.created_at_unix_ms")?);
        if let Some(user_id) = user_id {
            query = query.bind(user_id.to_string());
        }
        let rows = query.fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_mysql_usage_daily_summary).collect()
    }

    async fn summarize_usage_daily_heatmap_from_daily_aggregates(
        &self,
        created_from_unix_secs: u64,
        created_until_unix_secs: u64,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        let rows = if let Some(user_id) = user_id {
            sqlx::query(
                r#"
SELECT
  DATE_FORMAT(FROM_UNIXTIME(`date`), '%Y-%m-%d') AS date,
  total_requests AS requests,
  input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens AS total_tokens,
  total_cost AS total_cost_usd,
  total_cost AS actual_total_cost_usd
FROM stats_user_daily
WHERE user_id = ?
  AND `date` >= ?
  AND `date` < ?
  AND total_requests > 0
ORDER BY `date` ASC
"#,
            )
            .bind(user_id)
            .bind(to_i64(created_from_unix_secs, "stats_user_daily.date")?)
            .bind(to_i64(created_until_unix_secs, "stats_user_daily.date")?)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  DATE_FORMAT(FROM_UNIXTIME(`date`), '%Y-%m-%d') AS date,
  total_requests AS requests,
  input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens AS total_tokens,
  total_cost AS total_cost_usd,
  actual_total_cost AS actual_total_cost_usd
FROM stats_daily
WHERE `date` >= ?
  AND `date` < ?
  AND total_requests > 0
ORDER BY `date` ASC
"#,
            )
            .bind(to_i64(created_from_unix_secs, "stats_daily.date")?)
            .bind(to_i64(created_until_unix_secs, "stats_daily.date")?)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?
        };

        rows.iter().map(map_mysql_usage_daily_summary).collect()
    }

    async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        let created_until_unix_secs = usage_current_unix_secs().saturating_add(1);
        let user_id = query.user_id.as_deref();
        let mut summaries = BTreeMap::<String, StoredUsageDailySummary>::new();

        for item in self
            .summarize_usage_daily_heatmap_from_daily_aggregates(
                query.created_from_unix_secs,
                created_until_unix_secs,
                user_id,
            )
            .await?
        {
            summaries.insert(item.date.clone(), item);
        }
        for item in self
            .summarize_usage_daily_heatmap_raw_from_range(
                query.created_from_unix_secs,
                created_until_unix_secs,
                user_id,
            )
            .await?
        {
            summaries.entry(item.date.clone()).or_insert(item);
        }

        Ok(summaries.into_values().collect())
    }

    async fn summarize_dashboard_usage_from_daily_aggregates(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<Option<StoredUsageDashboardSummary>, DataLayerError> {
        let row = if let Some(user_id) = query.user_id.as_deref() {
            sqlx::query(
                r#"
SELECT
  COALESCE(SUM(total_requests), 0) AS total_requests,
  COALESCE(SUM(input_tokens), 0) AS input_tokens,
  COALESCE(SUM(input_tokens), 0) AS effective_input_tokens,
  COALESCE(SUM(output_tokens), 0) AS output_tokens,
  COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS total_tokens,
  COALESCE(SUM(cache_creation_tokens), 0) AS cache_creation_tokens,
  COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens,
  COALESCE(SUM(input_tokens + cache_creation_tokens + cache_read_tokens), 0) AS total_input_context,
  0.0 AS cache_creation_cost_usd,
  0.0 AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS actual_total_cost_usd,
  COALESCE(SUM(error_requests), 0) AS error_requests,
  0.0 AS response_time_sum_ms,
  0 AS response_time_samples
FROM stats_user_daily
WHERE user_id = ?
  AND `date` >= ?
  AND `date` < ?
"#,
            )
            .bind(user_id)
            .bind(to_i64(
                query.created_from_unix_secs,
                "stats_user_daily.date",
            )?)
            .bind(to_i64(
                query.created_until_unix_secs,
                "stats_user_daily.date",
            )?)
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  COALESCE(SUM(total_requests), 0) AS total_requests,
  COALESCE(SUM(input_tokens), 0) AS input_tokens,
  COALESCE(SUM(input_tokens), 0) AS effective_input_tokens,
  COALESCE(SUM(output_tokens), 0) AS output_tokens,
  COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS total_tokens,
  COALESCE(SUM(cache_creation_tokens), 0) AS cache_creation_tokens,
  COALESCE(SUM(cache_read_tokens), 0) AS cache_read_tokens,
  COALESCE(SUM(input_tokens + cache_creation_tokens + cache_read_tokens), 0) AS total_input_context,
  COALESCE(SUM(COALESCE(cache_creation_cost, 0)), 0) AS cache_creation_cost_usd,
  COALESCE(SUM(COALESCE(cache_read_cost, 0)), 0) AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(actual_total_cost, 0)), 0) AS actual_total_cost_usd,
  COALESCE(SUM(error_requests), 0) AS error_requests,
  0.0 AS response_time_sum_ms,
  0 AS response_time_samples
FROM stats_daily
WHERE `date` >= ?
  AND `date` < ?
"#,
            )
            .bind(to_i64(query.created_from_unix_secs, "stats_daily.date")?)
            .bind(to_i64(query.created_until_unix_secs, "stats_daily.date")?)
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?
        };

        let total_requests = row_u64(&row, "total_requests")?;
        if total_requests == 0 {
            return Ok(None);
        }

        Ok(Some(StoredUsageDashboardSummary {
            total_requests,
            input_tokens: row_u64(&row, "input_tokens")?,
            effective_input_tokens: row_u64(&row, "effective_input_tokens")?,
            output_tokens: row_u64(&row, "output_tokens")?,
            total_tokens: row_u64(&row, "total_tokens")?,
            cache_creation_tokens: row_u64(&row, "cache_creation_tokens")?,
            cache_read_tokens: row_u64(&row, "cache_read_tokens")?,
            total_input_context: row_u64(&row, "total_input_context")?,
            cache_creation_cost_usd: row.try_get("cache_creation_cost_usd").map_sql_err()?,
            cache_read_cost_usd: row.try_get("cache_read_cost_usd").map_sql_err()?,
            total_cost_usd: row.try_get("total_cost_usd").map_sql_err()?,
            actual_total_cost_usd: row.try_get("actual_total_cost_usd").map_sql_err()?,
            error_requests: row_u64(&row, "error_requests")?,
            response_time_sum_ms: row.try_get("response_time_sum_ms").map_sql_err()?,
            response_time_samples: row_u64(&row, "response_time_samples")?,
        }))
    }

    async fn list_dashboard_daily_breakdown_from_daily_aggregates(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        let rows = if let Some(user_id) = query.user_id.as_deref() {
            sqlx::query(
                r#"
SELECT
  DATE_FORMAT(FROM_UNIXTIME(`date`), '%Y-%m-%d') AS date,
  'aggregate' AS model,
  'aggregate' AS provider,
  COALESCE(SUM(total_requests), 0) AS requests,
  COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS total_cost_usd,
  0.0 AS response_time_sum_ms,
  0 AS response_time_samples
FROM stats_user_daily
WHERE user_id = ?
  AND `date` >= ?
  AND `date` < ?
  AND total_requests > 0
GROUP BY `date`
ORDER BY `date` ASC
"#,
            )
            .bind(user_id)
            .bind(to_i64(
                query.created_from_unix_secs,
                "stats_user_daily.date",
            )?)
            .bind(to_i64(
                query.created_until_unix_secs,
                "stats_user_daily.date",
            )?)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  DATE_FORMAT(FROM_UNIXTIME(`date`), '%Y-%m-%d') AS date,
  'aggregate' AS model,
  'aggregate' AS provider,
  COALESCE(SUM(total_requests), 0) AS requests,
  COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS total_cost_usd,
  0.0 AS response_time_sum_ms,
  0 AS response_time_samples
FROM stats_daily
WHERE `date` >= ?
  AND `date` < ?
  AND total_requests > 0
GROUP BY `date`
ORDER BY `date` ASC
"#,
            )
            .bind(to_i64(query.created_from_unix_secs, "stats_daily.date")?)
            .bind(to_i64(query.created_until_unix_secs, "stats_daily.date")?)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?
        };

        rows.iter()
            .map(|row| {
                Ok(StoredUsageDashboardDailyBreakdownRow {
                    date: row.try_get("date").map_sql_err()?,
                    model: row.try_get("model").map_sql_err()?,
                    provider: row.try_get("provider").map_sql_err()?,
                    requests: row_u64(row, "requests")?,
                    total_tokens: row_u64(row, "total_tokens")?,
                    total_cost_usd: row.try_get("total_cost_usd").map_sql_err()?,
                    response_time_sum_ms: row.try_get("response_time_sum_ms").map_sql_err()?,
                    response_time_samples: row_u64(row, "response_time_samples")?,
                })
            })
            .collect()
    }

    async fn summarize_dashboard_usage(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        if let Some(summary) = self
            .summarize_dashboard_usage_from_daily_aggregates(query)
            .await?
        {
            return Ok(summary);
        }

        let repository = self.materialize_read_model().await?;
        <InMemoryUsageReadRepository as UsageReadRepository>::summarize_dashboard_usage(
            &repository,
            query,
        )
        .await
    }

    async fn list_dashboard_daily_breakdown(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        let aggregate_rows = self
            .list_dashboard_daily_breakdown_from_daily_aggregates(query)
            .await?;
        if !aggregate_rows.is_empty() {
            return Ok(aggregate_rows);
        }

        let repository = self.materialize_read_model().await?;
        <InMemoryUsageReadRepository as UsageReadRepository>::list_dashboard_daily_breakdown(
            &repository,
            query,
        )
        .await
    }

    async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUsageUserTotals>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        let unique_user_ids = user_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let mut totals = BTreeMap::<String, StoredUsageUserTotals>::new();
        let mut aggregate_cutoffs = BTreeMap::<String, u64>::new();

        let mut aggregate_builder = QueryBuilder::<MySql>::new(
            r#"
SELECT
  user_id,
  COALESCE(SUM(total_requests), 0) AS request_count,
  COALESCE(
    SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens),
    0
  ) AS total_tokens,
  MAX(`date`) AS latest_date
FROM stats_user_daily
WHERE user_id IN (
"#,
        );
        {
            let mut separated = aggregate_builder.separated(", ");
            for user_id in &unique_user_ids {
                separated.push_bind(user_id.clone());
            }
        }
        aggregate_builder.push(") GROUP BY user_id ORDER BY user_id ASC");

        let aggregate_rows = aggregate_builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        for row in aggregate_rows {
            let user_id: String = row.try_get("user_id").map_sql_err()?;
            let latest_date = row.try_get::<i64, _>("latest_date").map_sql_err()?.max(0) as u64;
            aggregate_cutoffs.insert(user_id.clone(), latest_date.saturating_add(86_400));
            totals.insert(
                user_id.clone(),
                StoredUsageUserTotals {
                    user_id,
                    request_count: row_u64(&row, "request_count")?,
                    total_tokens: row_u64(&row, "total_tokens")?,
                },
            );
        }

        let mut raw_builder = QueryBuilder::<MySql>::new(
            r#"
SELECT
  `usage`.user_id,
  COUNT(*) AS request_count,
  COALESCE(SUM(GREATEST(COALESCE(`usage`.total_tokens, 0), 0)), 0) AS total_tokens
FROM `usage`
JOIN (
"#,
        );
        for (index, user_id) in unique_user_ids.iter().enumerate() {
            if index > 0 {
                raw_builder.push(" UNION ALL ");
            }
            let cutoff = aggregate_cutoffs.get(user_id).copied().unwrap_or_default();
            raw_builder
                .push("SELECT ")
                .push_bind(user_id.clone())
                .push(" AS user_id, ")
                .push_bind(to_i64(cutoff, "usage aggregate cutoff")?)
                .push(" AS cutoff_unix_secs");
        }
        raw_builder.push(
            r#"
) AS requested ON requested.user_id = `usage`.user_id
WHERE `usage`.created_at_unix_ms >= requested.cutoff_unix_secs
  AND `usage`.status NOT IN ('pending', 'streaming')
  AND `usage`.provider_name NOT IN ('unknown', 'pending')
GROUP BY `usage`.user_id
ORDER BY `usage`.user_id ASC
"#,
        );

        let raw_rows = raw_builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        for row in raw_rows {
            let user_id: String = row.try_get("user_id").map_sql_err()?;
            let entry = totals
                .entry(user_id.clone())
                .or_insert_with(|| StoredUsageUserTotals {
                    user_id,
                    request_count: 0,
                    total_tokens: 0,
                });
            entry.request_count = entry
                .request_count
                .saturating_add(row_u64(&row, "request_count")?);
            entry.total_tokens = entry
                .total_tokens
                .saturating_add(row_u64(&row, "total_tokens")?);
        }

        Ok(totals.into_values().collect())
    }
}

impl_materialized_usage_read_repository!(MysqlUsageReadRepository);

impl MysqlUsageWriteRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    pub async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(&format!("{USAGE_COLUMNS} WHERE request_id = ? LIMIT 1"))
            .bind(request_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_usage_row).transpose()
    }
}

#[async_trait]
impl UsageWriteRepository for MysqlUsageWriteRepository {
    async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        let usage = strip_deprecated_usage_display_fields(usage);
        usage.validate()?;

        if let Some(existing) = self.find_by_request_id(&usage.request_id).await? {
            if (existing.billing_status == "settled" || existing.billing_status == "void")
                && !usage_can_recover_terminal_failure(
                    &existing.status,
                    &existing.billing_status,
                    &usage.status,
                    &usage.billing_status,
                )
            {
                return Ok(existing);
            }
        }

        bind_upsert(sqlx::query(UPSERT_USAGE_SQL), &usage)?
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        self.rebuild_api_key_usage_stats().await?;
        self.rebuild_provider_api_key_usage_stats().await?;
        self.find_by_request_id(&usage.request_id)
            .await?
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue("usage upsert returned no row".to_string())
            })
    }

    async fn rebuild_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        sqlx::query(
            r#"
UPDATE api_keys
SET total_requests = 0,
    total_tokens = 0,
    total_cost_usd = 0,
    last_used_at = NULL
"#,
        )
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        let rows = sqlx::query(
            r#"
SELECT
  api_key_id,
  COUNT(*) AS total_requests,
  CAST(COALESCE(SUM(total_tokens), 0) AS SIGNED) AS total_tokens,
  CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE) AS total_cost_usd,
  MAX(updated_at_unix_secs) AS last_used_at
FROM `usage`
WHERE api_key_id IS NOT NULL AND api_key_id <> ''
GROUP BY api_key_id
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        for row in &rows {
            sqlx::query(
                r#"
UPDATE api_keys
SET total_requests = ?,
    total_tokens = ?,
    total_cost_usd = ?,
    last_used_at = ?
WHERE id = ?
"#,
            )
            .bind(row.try_get::<i64, _>("total_requests").map_sql_err()?)
            .bind(row.try_get::<i64, _>("total_tokens").map_sql_err()?)
            .bind(row.try_get::<f64, _>("total_cost_usd").map_sql_err()?)
            .bind(
                row.try_get::<Option<i64>, _>("last_used_at")
                    .map_sql_err()?,
            )
            .bind(row.try_get::<String, _>("api_key_id").map_sql_err()?)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        }

        Ok(rows.len() as u64)
    }

    async fn rebuild_provider_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        sqlx::query(
            r#"
UPDATE provider_api_keys
SET request_count = 0,
    success_count = 0,
    error_count = 0,
    total_tokens = 0,
    total_cost_usd = 0,
    total_response_time_ms = 0,
    last_used_at = NULL
"#,
        )
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        let rows = sqlx::query(
            r#"
SELECT
  provider_api_key_id,
  status,
  status_code,
  error_message,
  total_tokens,
  total_cost_usd,
  response_time_ms,
  updated_at_unix_secs
FROM `usage`
WHERE provider_api_key_id IS NOT NULL AND provider_api_key_id <> ''
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        let mut stats = BTreeMap::<String, ProviderKeyStats>::new();
        for row in rows {
            let key_id: String = row.try_get("provider_api_key_id").map_sql_err()?;
            let status: String = row.try_get("status").map_sql_err()?;
            let status_code = row.try_get::<Option<i64>, _>("status_code").map_sql_err()?;
            let status_code_u16 = status_code.and_then(|value| u16::try_from(value).ok());
            let error_message: Option<String> = row.try_get("error_message").map_sql_err()?;
            let entry = stats.entry(key_id).or_default();
            entry.request_count += 1;
            let is_success = provider_api_key_usage_is_success(
                &status,
                status_code_u16,
                error_message.as_deref(),
            );
            let is_in_flight = matches!(status.as_str(), "pending" | "streaming");
            if is_success {
                entry.success_count += 1;
            }
            if provider_api_key_usage_is_error(&status, status_code_u16, error_message.as_deref()) {
                entry.error_count += 1;
            }
            if !is_in_flight {
                entry.total_tokens += row.try_get::<i64, _>("total_tokens").map_sql_err()?;
                entry.total_cost_usd += row.try_get::<f64, _>("total_cost_usd").map_sql_err()?;
            }
            if is_success {
                entry.total_response_time_ms += row
                    .try_get::<Option<i64>, _>("response_time_ms")
                    .map_sql_err()?
                    .unwrap_or_default();
            }
            entry.last_used_at = entry.last_used_at.max(
                row.try_get::<Option<i64>, _>("updated_at_unix_secs")
                    .map_sql_err()?,
            );
        }

        for (key_id, stat) in &stats {
            sqlx::query(
                r#"
UPDATE provider_api_keys
SET request_count = ?,
    success_count = ?,
    error_count = ?,
    total_tokens = ?,
    total_cost_usd = ?,
    total_response_time_ms = ?,
    last_used_at = ?
WHERE id = ?
"#,
            )
            .bind(stat.request_count)
            .bind(stat.success_count)
            .bind(stat.error_count)
            .bind(stat.total_tokens)
            .bind(stat.total_cost_usd)
            .bind(stat.total_response_time_ms)
            .bind(stat.last_used_at)
            .bind(key_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        }

        Ok(stats.len() as u64)
    }

    async fn cleanup_stale_pending_requests(
        &self,
        cutoff_unix_secs: u64,
        now_unix_secs: u64,
        timeout_minutes: u64,
        batch_size: usize,
    ) -> Result<PendingUsageCleanupSummary, DataLayerError> {
        if batch_size == 0 {
            return Ok(PendingUsageCleanupSummary::default());
        }

        let cutoff_unix_ms = cutoff_unix_secs.saturating_mul(1000);
        let now_unix_ms = now_unix_secs.saturating_mul(1000);
        let mut summary = PendingUsageCleanupSummary::default();
        let batch_size_u64 = u64::try_from(batch_size).map_err(|_| {
            DataLayerError::InvalidInput(format!(
                "invalid stale pending usage batch size: {batch_size}"
            ))
        })?;

        loop {
            let mut tx = self.pool.begin().await.map_sql_err()?;
            let stale_rows = sqlx::query(SELECT_STALE_PENDING_USAGE_BATCH_SQL)
                .bind(to_i64(cutoff_unix_ms, "stale pending usage cutoff")?)
                .bind(to_i64(batch_size_u64, "stale pending usage batch size")?)
                .fetch_all(&mut *tx)
                .await
                .map_sql_err()?;

            if stale_rows.is_empty() {
                tx.rollback().await.map_sql_err()?;
                break;
            }

            let stale_rows = stale_rows
                .iter()
                .map(|row| {
                    Ok(StalePendingUsageRow {
                        request_id: row.try_get("request_id").map_sql_err()?,
                        status: row.try_get("status").map_sql_err()?,
                        billing_status: row.try_get("billing_status").map_sql_err()?,
                    })
                })
                .collect::<Result<Vec<_>, DataLayerError>>()?;
            let completed_request_ids =
                completed_request_ids_mysql(&mut tx, stale_rows.iter().map(|row| &row.request_id))
                    .await?;

            for row in stale_rows {
                if completed_request_ids.contains(&row.request_id) {
                    sqlx::query(
                        r#"
UPDATE `usage`
SET status = 'completed',
    status_code = 200,
    error_message = NULL
WHERE request_id = ?
"#,
                    )
                    .bind(&row.request_id)
                    .execute(&mut *tx)
                    .await
                    .map_sql_err()?;
                    sqlx::query(
                        r#"
UPDATE request_candidates
SET status = 'success',
    finished_at = ?
WHERE request_id = ?
  AND status = 'streaming'
"#,
                    )
                    .bind(to_i64(now_unix_ms, "request candidate finished_at")?)
                    .bind(&row.request_id)
                    .execute(&mut *tx)
                    .await
                    .map_sql_err()?;
                    summary.recovered += 1;
                    continue;
                }

                let candidate_info =
                    latest_failed_candidate_mysql(&mut tx, &row.request_id).await?;
                let (status_code, error_message) = resolve_stale_pending_failure(
                    candidate_info.as_ref(),
                    &row.status,
                    timeout_minutes,
                );
                let status_code_i64 = i64::from(status_code);
                if row.billing_status == "pending" {
                    sqlx::query(
                        r#"
UPDATE `usage`
SET status = 'failed',
    status_code = ?,
    error_message = ?,
    billing_status = 'void',
    finalized_at = ?,
    total_cost_usd = 0,
    actual_total_cost_usd = 0
WHERE request_id = ?
"#,
                    )
                    .bind(status_code_i64)
                    .bind(&error_message)
                    .bind(to_i64(now_unix_secs, "usage finalized_at")?)
                    .bind(&row.request_id)
                    .execute(&mut *tx)
                    .await
                    .map_sql_err()?;
                    upsert_void_usage_settlement_snapshot_mysql(
                        &mut tx,
                        &row.request_id,
                        now_unix_secs,
                    )
                    .await?;
                } else {
                    sqlx::query(
                        r#"
UPDATE `usage`
SET status = 'failed',
    status_code = ?,
    error_message = ?
WHERE request_id = ?
"#,
                    )
                    .bind(status_code_i64)
                    .bind(&error_message)
                    .bind(&row.request_id)
                    .execute(&mut *tx)
                    .await
                    .map_sql_err()?;
                }

                sqlx::query(
                    r#"
UPDATE request_candidates
SET status = 'failed',
    finished_at = ?,
    error_message = '请求超时（服务器可能已重启）'
WHERE request_id = ?
  AND status IN ('pending', 'streaming')
"#,
                )
                .bind(to_i64(now_unix_ms, "request candidate finished_at")?)
                .bind(&row.request_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
                summary.failed += 1;
            }

            tx.commit().await.map_sql_err()?;
        }

        Ok(summary)
    }
}

struct StalePendingUsageRow {
    request_id: String,
    status: String,
    billing_status: String,
}

#[derive(Default)]
struct ProviderKeyStats {
    request_count: i64,
    success_count: i64,
    error_count: i64,
    total_tokens: i64,
    total_cost_usd: f64,
    total_response_time_ms: i64,
    last_used_at: Option<i64>,
}

async fn completed_request_ids_mysql<'a>(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    request_ids: impl Iterator<Item = &'a String>,
) -> Result<HashSet<String>, DataLayerError> {
    let mut completed = HashSet::new();
    for request_id in request_ids {
        let rows = sqlx::query(SELECT_COMPLETED_REQUEST_CANDIDATES_SQL)
            .bind(request_id)
            .fetch_all(&mut **tx)
            .await
            .map_sql_err()?;
        let mut is_completed = false;
        for row in &rows {
            if candidate_row_is_completed(row)? {
                is_completed = true;
                break;
            }
        }
        if is_completed {
            completed.insert(request_id.clone());
        }
    }
    Ok(completed)
}

fn candidate_row_is_completed(row: &MySqlRow) -> Result<bool, DataLayerError> {
    let status: String = row.try_get("status").map_sql_err()?;
    if status == "streaming" {
        return Ok(true);
    }
    if status != "success" {
        return Ok(false);
    }
    let Some(extra_data) = row
        .try_get::<Option<String>, _>("extra_data")
        .map_sql_err()?
    else {
        return Ok(false);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&extra_data) else {
        return Ok(false);
    };
    Ok(value
        .get("stream_completed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

async fn upsert_void_usage_settlement_snapshot_mysql(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    request_id: &str,
    now_unix_secs: u64,
) -> Result<(), DataLayerError> {
    let now = to_i64(now_unix_secs, "usage settlement snapshot timestamp")?;
    sqlx::query(
        r#"
INSERT INTO usage_settlement_snapshots (
  request_id,
  billing_status,
  finalized_at,
  created_at,
  updated_at
) VALUES (?, 'void', ?, ?, ?)
ON DUPLICATE KEY UPDATE
  billing_status = VALUES(billing_status),
  finalized_at = COALESCE(usage_settlement_snapshots.finalized_at, VALUES(finalized_at)),
  updated_at = VALUES(updated_at)
"#,
    )
    .bind(request_id)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

fn stale_pending_error_message(status: &str, timeout_minutes: u64) -> String {
    format!("请求超时: 状态 '{status}' 超过 {timeout_minutes} 分钟未完成")
}

struct FailedCandidateCleanupInfo {
    status_code: Option<u16>,
    error_message: Option<String>,
}

fn resolve_stale_pending_failure(
    candidate: Option<&FailedCandidateCleanupInfo>,
    status: &str,
    timeout_minutes: u64,
) -> (u16, String) {
    match candidate {
        Some(info) => (
            info.status_code.unwrap_or(502),
            info.error_message
                .clone()
                .unwrap_or_else(|| stale_pending_error_message(status, timeout_minutes)),
        ),
        None => (504, stale_pending_error_message(status, timeout_minutes)),
    }
}

async fn latest_failed_candidate_mysql(
    tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
    request_id: &str,
) -> Result<Option<FailedCandidateCleanupInfo>, DataLayerError> {
    let row = sqlx::query(
        r#"
SELECT status_code, error_message
FROM request_candidates
WHERE request_id = ?
  AND status IN ('failed', 'cancelled')
ORDER BY
  COALESCE(finished_at, started_at, created_at) DESC,
  retry_index DESC,
  candidate_index DESC
LIMIT 1
"#,
    )
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await
    .map_sql_err()?;

    let Some(row) = row else {
        return Ok(None);
    };
    let status_code = row
        .try_get::<Option<i64>, _>("status_code")
        .map_sql_err()?
        .and_then(|value| u16::try_from(value).ok());
    let error_message = row
        .try_get::<Option<String>, _>("error_message")
        .map_sql_err()?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok(Some(FailedCandidateCleanupInfo {
        status_code,
        error_message,
    }))
}

fn bind_upsert<'q>(
    mut query: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    usage: &'q UpsertUsageRecord,
) -> Result<sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>, DataLayerError> {
    let input_tokens = usage.input_tokens.unwrap_or_default();
    let output_tokens = usage.output_tokens.unwrap_or_default();
    let cache_creation_tokens = usage
        .cache_creation_input_tokens
        .or_else(|| {
            Some(
                usage
                    .cache_creation_ephemeral_5m_input_tokens
                    .unwrap_or_default()
                    + usage
                        .cache_creation_ephemeral_1h_input_tokens
                        .unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    let cache_read_tokens = usage.cache_read_input_tokens.unwrap_or_default();
    let total_tokens = usage
        .total_tokens
        .unwrap_or(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens);
    let created_at = usage
        .created_at_unix_ms
        .unwrap_or(usage.updated_at_unix_secs.saturating_mul(1000));
    let request_metadata = usage
        .request_metadata
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| DataLayerError::InvalidInput(err.to_string()))?;

    query = query
        .bind(&usage.request_id)
        .bind(&usage.request_id)
        .bind(usage.user_id.as_deref())
        .bind(usage.api_key_id.as_deref())
        .bind(&usage.provider_name)
        .bind(&usage.model)
        .bind(usage.target_model.as_deref())
        .bind(usage.provider_id.as_deref())
        .bind(usage.provider_endpoint_id.as_deref())
        .bind(usage.provider_api_key_id.as_deref())
        .bind(usage.request_type.as_deref())
        .bind(usage.api_format.as_deref())
        .bind(usage.api_family.as_deref())
        .bind(usage.endpoint_kind.as_deref())
        .bind(usage.endpoint_api_format.as_deref())
        .bind(usage.provider_api_family.as_deref())
        .bind(usage.provider_endpoint_kind.as_deref())
        .bind(usage.has_format_conversion.unwrap_or(false))
        .bind(usage.is_stream.unwrap_or(false))
        .bind(usage_upstream_is_stream(usage))
        .bind(to_i64(input_tokens, "input_tokens")?)
        .bind(to_i64(output_tokens, "output_tokens")?)
        .bind(to_i64(total_tokens, "total_tokens")?)
        .bind(to_i64(
            cache_creation_tokens,
            "cache_creation_input_tokens",
        )?)
        .bind(to_i64(
            usage
                .cache_creation_ephemeral_5m_input_tokens
                .unwrap_or_default(),
            "cache_creation_ephemeral_5m_input_tokens",
        )?)
        .bind(to_i64(
            usage
                .cache_creation_ephemeral_1h_input_tokens
                .unwrap_or_default(),
            "cache_creation_ephemeral_1h_input_tokens",
        )?)
        .bind(to_i64(cache_read_tokens, "cache_read_input_tokens")?)
        .bind(usage.cache_creation_cost_usd.unwrap_or_default())
        .bind(usage.cache_read_cost_usd.unwrap_or_default())
        .bind(usage.output_price_per_1m)
        .bind(usage.total_cost_usd.unwrap_or_default())
        .bind(usage.actual_total_cost_usd.unwrap_or_default())
        .bind(usage.status_code.map(i64::from))
        .bind(usage.error_message.as_deref())
        .bind(usage.error_category.as_deref())
        .bind(usage.response_time_ms.map(|value| value as i64))
        .bind(usage.first_byte_time_ms.map(|value| value as i64))
        .bind(&usage.status)
        .bind(&usage.billing_status)
        .bind(request_metadata)
        .bind(usage.candidate_id.as_deref())
        .bind(usage.candidate_index.map(|value| value as i64))
        .bind(usage.key_name.as_deref())
        .bind(usage.planner_kind.as_deref())
        .bind(usage.route_family.as_deref())
        .bind(usage.route_kind.as_deref())
        .bind(usage.execution_path.as_deref())
        .bind(usage.local_execution_runtime_miss_reason.as_deref())
        .bind(usage.finalized_at_unix_secs.map(|value| value as i64))
        .bind(to_i64(created_at, "created_at_unix_ms")?)
        .bind(to_i64(usage.updated_at_unix_secs, "updated_at_unix_secs")?);
    Ok(query)
}

fn map_usage_row(row: &MySqlRow) -> Result<StoredRequestUsageAudit, DataLayerError> {
    let id = row
        .try_get::<Option<String>, _>("id")
        .map_sql_err()?
        .unwrap_or_else(|| {
            row.try_get::<String, _>("request_id")
                .unwrap_or_else(|_| "unknown".to_string())
        });
    let mut audit = StoredRequestUsageAudit::new(
        id,
        row.try_get("request_id").map_sql_err()?,
        row.try_get("user_id").map_sql_err()?,
        row.try_get("api_key_id").map_sql_err()?,
        None,
        None,
        row.try_get("provider_name").map_sql_err()?,
        row.try_get("model").map_sql_err()?,
        row.try_get("target_model").map_sql_err()?,
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("provider_endpoint_id").map_sql_err()?,
        row.try_get("provider_api_key_id").map_sql_err()?,
        row.try_get("request_type").map_sql_err()?,
        row.try_get("api_format").map_sql_err()?,
        row.try_get("api_family").map_sql_err()?,
        row.try_get("endpoint_kind").map_sql_err()?,
        row.try_get("endpoint_api_format").map_sql_err()?,
        row.try_get("provider_api_family").map_sql_err()?,
        row.try_get("provider_endpoint_kind").map_sql_err()?,
        row.try_get::<bool, _>("has_format_conversion")
            .map_sql_err()?,
        row.try_get::<bool, _>("is_stream").map_sql_err()?,
        row_i32(row, "input_tokens")?,
        row_i32(row, "output_tokens")?,
        row_i32(row, "total_tokens")?,
        row.try_get("total_cost_usd").map_sql_err()?,
        row.try_get("actual_total_cost_usd").map_sql_err()?,
        row_optional_i32(row, "status_code")?,
        row.try_get("error_message").map_sql_err()?,
        row.try_get("error_category").map_sql_err()?,
        row_optional_i32(row, "response_time_ms")?,
        row_optional_i32(row, "first_byte_time_ms")?,
        row.try_get("status").map_sql_err()?,
        row.try_get("billing_status").map_sql_err()?,
        row.try_get("created_at_unix_ms").map_sql_err()?,
        row.try_get("updated_at_unix_secs").map_sql_err()?,
        row.try_get("finalized_at_unix_secs").map_sql_err()?,
    )?;
    audit.cache_creation_input_tokens = row_u64(row, "cache_creation_input_tokens")?;
    audit.cache_creation_ephemeral_5m_input_tokens =
        row_u64(row, "cache_creation_ephemeral_5m_input_tokens")?;
    audit.cache_creation_ephemeral_1h_input_tokens =
        row_u64(row, "cache_creation_ephemeral_1h_input_tokens")?;
    audit.cache_read_input_tokens = row_u64(row, "cache_read_input_tokens")?;
    audit.cache_creation_cost_usd = row.try_get("cache_creation_cost_usd").map_sql_err()?;
    audit.cache_read_cost_usd = row.try_get("cache_read_cost_usd").map_sql_err()?;
    audit.output_price_per_1m = row.try_get("output_price_per_1m").map_sql_err()?;
    audit.request_metadata = row
        .try_get::<Option<String>, _>("request_metadata")
        .map_sql_err()?
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
    audit.client_family = usage_request_metadata_client_family(audit.request_metadata.as_ref())
        .map(ToOwned::to_owned);
    let upstream_is_stream = row
        .try_get::<Option<bool>, _>("upstream_is_stream")
        .map_sql_err()?;
    merge_usage_stream_metadata(&mut audit.request_metadata, upstream_is_stream);
    audit.candidate_id = row.try_get("candidate_id").map_sql_err()?;
    audit.candidate_index = row
        .try_get::<Option<i64>, _>("candidate_index")
        .map_sql_err()?
        .map(|value| value as u64);
    audit.key_name = row.try_get("key_name").map_sql_err()?;
    audit.planner_kind = row.try_get("planner_kind").map_sql_err()?;
    audit.route_family = row.try_get("route_family").map_sql_err()?;
    audit.route_kind = row.try_get("route_kind").map_sql_err()?;
    audit.execution_path = row.try_get("execution_path").map_sql_err()?;
    audit.local_execution_runtime_miss_reason = row
        .try_get("local_execution_runtime_miss_reason")
        .map_sql_err()?;
    Ok(audit)
}

fn to_i64(value: u64, field: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| DataLayerError::InvalidInput(format!("{field} overflow")))
}

fn usage_upstream_is_stream(usage: &UpsertUsageRecord) -> bool {
    usage
        .request_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get(UPSTREAM_IS_STREAM_KEY))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| usage.is_stream.unwrap_or(false))
}

fn merge_usage_stream_metadata(metadata: &mut Option<serde_json::Value>, upstream: Option<bool>) {
    let Some(upstream) = upstream else {
        return;
    };
    let value = metadata.get_or_insert_with(|| serde_json::json!({}));
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object
        .entry(UPSTREAM_IS_STREAM_KEY)
        .or_insert(serde_json::Value::Bool(upstream));
}

fn row_i32(row: &MySqlRow, field: &str) -> Result<i32, DataLayerError> {
    let value: i64 = row.try_get(field).map_sql_err()?;
    i32::try_from(value).map_err(|_| DataLayerError::UnexpectedValue(format!("{field} overflow")))
}

fn row_optional_i32(row: &MySqlRow, field: &str) -> Result<Option<i32>, DataLayerError> {
    row.try_get::<Option<i64>, _>(field)
        .map_sql_err()?
        .map(|value| {
            i32::try_from(value)
                .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} overflow")))
        })
        .transpose()
}

fn row_u64(row: &MySqlRow, field: &str) -> Result<u64, DataLayerError> {
    let value: i64 = row.try_get(field).map_sql_err()?;
    u64::try_from(value).map_err(|_| DataLayerError::UnexpectedValue(format!("{field} negative")))
}

fn map_mysql_usage_daily_summary(
    row: &MySqlRow,
) -> Result<StoredUsageDailySummary, DataLayerError> {
    Ok(StoredUsageDailySummary {
        date: row.try_get("date").map_sql_err()?,
        requests: row_u64(row, "requests")?,
        total_tokens: row_u64(row, "total_tokens")?,
        total_cost_usd: row.try_get("total_cost_usd").map_sql_err()?,
        actual_total_cost_usd: row.try_get("actual_total_cost_usd").map_sql_err()?,
    })
}

fn usage_current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{MysqlUsageReadRepository, MysqlUsageWriteRepository};
    use crate::lifecycle::migrate::run_mysql_migrations;
    use crate::repository::usage::{
        UpsertUsageRecord, UsageAuditListQuery, UsageDashboardSummaryQuery, UsageReadRepository,
        UsageWriteRepository,
    };

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlUsageWriteRepository::new(pool);
    }

    #[test]
    fn mysql_usage_daily_heatmap_reads_imported_daily_aggregates() {
        let source = include_str!("mysql.rs");
        assert!(source.contains("summarize_usage_daily_heatmap_from_daily_aggregates"));
        assert!(source.contains("FROM stats_daily"));
        assert!(source.contains("FROM stats_user_daily"));
        assert!(source.contains("summaries.entry(item.date.clone()).or_insert(item)"));
    }

    #[test]
    fn mysql_usage_totals_by_user_ids_reads_imported_user_daily_aggregates() {
        let source = include_str!("mysql.rs");
        assert!(source.contains("async fn summarize_usage_totals_by_user_ids"));
        assert!(source.contains("FROM stats_user_daily"));
        assert!(source.contains("MAX(`date`) AS latest_date"));
        assert!(source.contains("requested.cutoff_unix_secs"));
    }

    #[test]
    fn mysql_dashboard_reads_imported_daily_aggregates() {
        let source = include_str!("mysql.rs");
        assert!(source.contains("summarize_dashboard_usage_from_daily_aggregates"));
        assert!(source.contains("list_dashboard_daily_breakdown_from_daily_aggregates"));
        assert!(source.contains("FROM stats_daily"));
        assert!(source.contains("FROM stats_user_daily"));
        assert!(source.contains("'aggregate' AS model"));
    }

    #[tokio::test]
    async fn mysql_usage_write_repository_upserts_when_url_is_set() {
        let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!(
                "skipping mysql usage write smoke test because AETHER_TEST_MYSQL_URL is unset"
            );
            return;
        };

        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("mysql test pool should connect");
        run_mysql_migrations(&pool)
            .await
            .expect("mysql migrations should run");

        let suffix = unique_suffix();
        let user_id = format!("user-{suffix}");
        let api_key_id = format!("api-key-{suffix}");
        let provider_id = format!("provider-{suffix}");
        let provider_key_id = format!("provider-key-{suffix}");
        seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;

        let repository = MysqlUsageWriteRepository::new(pool.clone());
        let record = repository
            .upsert(sample_usage(
                &format!("request-{suffix}"),
                &user_id,
                &api_key_id,
                &provider_id,
                &provider_key_id,
                "completed",
                "pending",
                1_000,
            ))
            .await
            .expect("usage should upsert");

        assert_eq!(record.api_key_id.as_deref(), Some(api_key_id.as_str()));
        assert_eq!(
            record.provider_api_key_id.as_deref(),
            Some(provider_key_id.as_str())
        );
        assert_eq!(record.total_tokens, 7);
        assert_eq!(
            record.request_metadata.as_ref().unwrap()["upstream_is_stream"],
            true
        );
        let upstream_is_stream: Option<bool> =
            sqlx::query_scalar("SELECT upstream_is_stream FROM `usage` WHERE request_id = ?")
                .bind(format!("request-{suffix}"))
                .fetch_one(&pool)
                .await
                .expect("usage stream mode should load");
        assert_eq!(upstream_is_stream, Some(true));

        let stats = sqlx::query_as::<_, (i64, i64, f64, Option<i64>)>(
            "SELECT total_requests, total_tokens, total_cost_usd, last_used_at FROM api_keys WHERE id = ?",
        )
        .bind(&api_key_id)
        .fetch_one(&pool)
        .await
        .expect("api key stats should load");
        assert_eq!(stats, (1, 7, 0.5, Some(1_000)));

        let provider_stats = sqlx::query_as::<_, (i64, i64, i64, i64, f64, i64, Option<i64>)>(
            "SELECT request_count, success_count, error_count, total_tokens, total_cost_usd, total_response_time_ms, last_used_at FROM provider_api_keys WHERE id = ?",
        )
        .bind(&provider_key_id)
        .fetch_one(&pool)
        .await
        .expect("provider key stats should load");
        assert_eq!(provider_stats, (1, 1, 0, 7, 0.5, 42, Some(1_000)));
    }

    #[tokio::test]
    async fn mysql_usage_read_repository_reads_usage_contract_views_when_url_is_set() {
        let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!(
                "skipping mysql usage read smoke test because AETHER_TEST_MYSQL_URL is unset"
            );
            return;
        };

        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("mysql test pool should connect");
        run_mysql_migrations(&pool)
            .await
            .expect("mysql migrations should run");

        let suffix = unique_suffix();
        let user_id = format!("user-read-{suffix}");
        let api_key_id = format!("api-key-read-{suffix}");
        let provider_id = format!("provider-read-{suffix}");
        let provider_key_id = format!("provider-key-read-{suffix}");
        seed_stats_targets(&pool, &user_id, &api_key_id, &provider_id, &provider_key_id).await;

        let writer = MysqlUsageWriteRepository::new(pool.clone());
        writer
            .upsert(sample_usage(
                &format!("request-read-1-{suffix}"),
                &user_id,
                &api_key_id,
                &provider_id,
                &provider_key_id,
                "completed",
                "settled",
                1_000,
            ))
            .await
            .expect("usage should upsert");
        writer
            .upsert(sample_usage(
                &format!("request-read-2-{suffix}"),
                &user_id,
                &api_key_id,
                &provider_id,
                &provider_key_id,
                "failed",
                "void",
                1_010,
            ))
            .await
            .expect("usage should upsert");

        let reader = MysqlUsageReadRepository::new(pool);
        let loaded = reader
            .find_by_request_id(&format!("request-read-1-{suffix}"))
            .await
            .expect("usage should load")
            .expect("usage should exist");
        assert_eq!(loaded.total_tokens, 7);
        assert_eq!(loaded.billing_status, "settled");

        let listed = reader
            .list_usage_audits(&UsageAuditListQuery {
                user_id: Some(user_id.clone()),
                provider_name: Some("Provider One".to_string()),
                newest_first: true,
                ..UsageAuditListQuery::default()
            })
            .await
            .expect("usage list should load");
        assert_eq!(listed.len(), 2);
        assert!(listed[0].request_id.starts_with("request-read-2-"));

        let summary = reader
            .summarize_dashboard_usage(&UsageDashboardSummaryQuery {
                created_from_unix_secs: 999,
                created_until_unix_secs: 1_020,
                user_id: Some(user_id),
            })
            .await
            .expect("dashboard summary should load");
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.error_requests, 1);
        assert_eq!(summary.total_tokens, 10);
    }

    async fn seed_stats_targets(
        pool: &sqlx::MySqlPool,
        user_id: &str,
        api_key_id: &str,
        provider_id: &str,
        provider_key_id: &str,
    ) {
        sqlx::query(
            r#"
INSERT INTO users (id, auth_source, created_at, updated_at)
VALUES (?, 'local', 1, 1)
"#,
        )
        .bind(user_id)
        .execute(pool)
        .await
        .expect("user should seed");

        sqlx::query(
            r#"
INSERT INTO api_keys (id, user_id, key_hash, created_at, updated_at)
VALUES (?, ?, ?, 1, 1)
"#,
        )
        .bind(api_key_id)
        .bind(user_id)
        .bind(format!("hash-{api_key_id}"))
        .execute(pool)
        .await
        .expect("api key should seed");

        sqlx::query(
            r#"
INSERT INTO providers (id, name, provider_type, created_at, updated_at)
VALUES (?, ?, 'openai', 1, 1)
"#,
        )
        .bind(provider_id)
        .bind(format!("Provider {provider_id}"))
        .execute(pool)
        .await
        .expect("provider should seed");

        sqlx::query(
            r#"
INSERT INTO provider_api_keys (id, provider_id, name, created_at, updated_at)
VALUES (?, ?, ?, 1, 1)
"#,
        )
        .bind(provider_key_id)
        .bind(provider_id)
        .bind(format!("Provider Key {provider_key_id}"))
        .execute(pool)
        .await
        .expect("provider key should seed");
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_usage(
        request_id: &str,
        user_id: &str,
        api_key_id: &str,
        provider_id: &str,
        provider_key_id: &str,
        status: &str,
        billing_status: &str,
        updated_at: u64,
    ) -> UpsertUsageRecord {
        UpsertUsageRecord {
            request_id: request_id.to_string(),
            user_id: Some(user_id.to_string()),
            api_key_id: Some(api_key_id.to_string()),
            username: Some("legacy-user".to_string()),
            api_key_name: Some("legacy-key".to_string()),
            provider_name: "Provider One".to_string(),
            model: "model-1".to_string(),
            target_model: Some("target-model".to_string()),
            provider_id: Some(provider_id.to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some(provider_key_id.to_string()),
            request_type: Some("chat".to_string()),
            api_format: Some("openai".to_string()),
            api_family: Some("chat".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai".to_string()),
            provider_api_family: Some("chat".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(true),
            is_stream: Some(false),
            input_tokens: Some(2),
            output_tokens: Some(3),
            total_tokens: None,
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: Some(0),
            cache_creation_ephemeral_1h_input_tokens: Some(0),
            cache_read_input_tokens: Some(2),
            cache_creation_cost_usd: Some(0.0),
            cache_read_cost_usd: Some(0.1),
            output_price_per_1m: Some(2.0),
            total_cost_usd: Some(0.5),
            actual_total_cost_usd: Some(0.4),
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(42),
            first_byte_time_ms: Some(12),
            status: status.to_string(),
            billing_status: billing_status.to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            request_body_state: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            provider_request_body_state: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            response_body_state: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            client_response_body_state: None,
            candidate_id: Some("candidate-1".to_string()),
            candidate_index: Some(1),
            key_name: Some("key-one".to_string()),
            planner_kind: Some("default".to_string()),
            route_family: Some("chat".to_string()),
            route_kind: Some("completion".to_string()),
            execution_path: Some("remote".to_string()),
            local_execution_runtime_miss_reason: None,
            request_metadata: Some(serde_json::json!({
                "trace_id": "trace-1",
                "upstream_is_stream": true,
            })),
            finalized_at_unix_secs: Some(updated_at),
            created_at_unix_ms: Some(updated_at),
            updated_at_unix_secs: updated_at,
        }
    }

    fn unique_suffix() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}-{nanos}", std::process::id())
    }
}
