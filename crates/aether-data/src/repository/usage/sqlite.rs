use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_ai_formats::UPSTREAM_IS_STREAM_KEY;
use aether_data_contracts::repository::usage::{parse_usage_body_ref, UsageBodyField};
use async_trait::async_trait;
use flate2::read::GzDecoder;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use super::{
    strip_deprecated_usage_display_fields, usage_can_recover_terminal_failure,
    usage_request_metadata_client_family, PendingUsageCleanupSummary,
    ProviderApiKeyWindowUsageRequest, StoredProviderApiKeyUsageSummary,
    StoredProviderApiKeyWindowUsageSummary, StoredProviderUsageSummary, StoredRequestUsageAudit,
    StoredUsageAuditAggregation, StoredUsageAuditSummary, StoredUsageBreakdownSummaryRow,
    StoredUsageCacheAffinityHitSummary, StoredUsageCacheAffinityIntervalRow,
    StoredUsageCacheHitSummary, StoredUsageCostSavingsSummary, StoredUsageDailySummary,
    StoredUsageDashboardDailyBreakdownRow, StoredUsageDashboardProviderCount,
    StoredUsageDashboardSummary, StoredUsageErrorDistributionRow, StoredUsageLeaderboardSummary,
    StoredUsagePerformancePercentilesRow, StoredUsageProviderPerformance,
    StoredUsageProviderPerformanceProviderRow, StoredUsageProviderPerformanceSummary,
    StoredUsageProviderPerformanceTimelineRow, StoredUsageSettledCostSummary,
    StoredUsageTimeSeriesBucket, StoredUsageUserTotals, UpsertUsageRecord,
    UsageAuditAggregationGroupBy, UsageAuditAggregationQuery, UsageAuditKeywordSearchQuery,
    UsageAuditListQuery, UsageAuditSummaryQuery, UsageBreakdownGroupBy, UsageBreakdownSummaryQuery,
    UsageCacheAffinityHitSummaryQuery, UsageCacheAffinityIntervalGroupBy,
    UsageCacheAffinityIntervalQuery, UsageCacheHitSummaryQuery, UsageCostSavingsSummaryQuery,
    UsageDailyHeatmapQuery, UsageDashboardDailyBreakdownQuery, UsageDashboardProviderCountsQuery,
    UsageDashboardSummaryQuery, UsageErrorDistributionQuery, UsageLeaderboardGroupBy,
    UsageLeaderboardQuery, UsageMonitoringErrorCountQuery, UsageMonitoringErrorListQuery,
    UsagePerformancePercentilesQuery, UsageProviderPerformanceQuery, UsageReadRepository,
    UsageSettledCostSummaryQuery, UsageTimeSeriesGranularity, UsageTimeSeriesQuery,
    UsageWriteRepository,
};
use crate::driver::sqlite::{sqlite_optional_real, sqlite_real, SqlitePool};
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
  CAST(cache_creation_cost_usd AS REAL) AS cache_creation_cost_usd,
  CAST(cache_read_cost_usd AS REAL) AS cache_read_cost_usd,
  CAST(output_price_per_1m AS REAL) AS output_price_per_1m,
  CAST(total_cost_usd AS REAL) AS total_cost_usd,
  CAST(actual_total_cost_usd AS REAL) AS actual_total_cost_usd,
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
  NULL AS username,
  NULL AS api_key_name,
  key_name,
  planner_kind,
  route_family,
  route_kind,
  execution_path,
  local_execution_runtime_miss_reason,
  finalized_at AS finalized_at_unix_secs,
  created_at_unix_ms,
  updated_at_unix_secs
FROM "usage"
"#;

const UPSERT_USAGE_SQL: &str = r#"
INSERT INTO "usage" (
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
ON CONFLICT (request_id) DO UPDATE SET
  user_id = excluded.user_id,
  api_key_id = excluded.api_key_id,
  provider_name = excluded.provider_name,
  model = excluded.model,
  target_model = excluded.target_model,
  provider_id = excluded.provider_id,
  provider_endpoint_id = excluded.provider_endpoint_id,
  provider_api_key_id = excluded.provider_api_key_id,
  request_type = excluded.request_type,
  api_format = excluded.api_format,
  api_family = excluded.api_family,
  endpoint_kind = excluded.endpoint_kind,
  endpoint_api_format = excluded.endpoint_api_format,
  provider_api_family = excluded.provider_api_family,
  provider_endpoint_kind = excluded.provider_endpoint_kind,
  has_format_conversion = excluded.has_format_conversion,
  is_stream = excluded.is_stream,
  upstream_is_stream = excluded.upstream_is_stream,
  input_tokens = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".input_tokens
        ELSE excluded.input_tokens
    END,
  output_tokens = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".output_tokens
        ELSE excluded.output_tokens
    END,
  total_tokens = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".total_tokens
        ELSE excluded.total_tokens
    END,
  cache_creation_input_tokens = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".cache_creation_input_tokens
        ELSE excluded.cache_creation_input_tokens
    END,
  cache_creation_ephemeral_5m_input_tokens = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".cache_creation_ephemeral_5m_input_tokens
        ELSE excluded.cache_creation_ephemeral_5m_input_tokens
    END,
  cache_creation_ephemeral_1h_input_tokens = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".cache_creation_ephemeral_1h_input_tokens
        ELSE excluded.cache_creation_ephemeral_1h_input_tokens
    END,
  cache_read_input_tokens = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".cache_read_input_tokens
        ELSE excluded.cache_read_input_tokens
    END,
  cache_creation_cost_usd = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".cache_creation_cost_usd
        ELSE excluded.cache_creation_cost_usd
    END,
  cache_read_cost_usd = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".cache_read_cost_usd
        ELSE excluded.cache_read_cost_usd
    END,
  output_price_per_1m = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".output_price_per_1m
        ELSE excluded.output_price_per_1m
    END,
  total_cost_usd = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".total_cost_usd
        ELSE excluded.total_cost_usd
    END,
  actual_total_cost_usd = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".actual_total_cost_usd
        ELSE excluded.actual_total_cost_usd
    END,
    status_code = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".status_code
        WHEN "usage".status = 'streaming' AND excluded.status = 'pending' THEN "usage".status_code
        WHEN "usage".status = 'streaming' AND excluded.status = 'streaming' AND excluded.status_code IS NULL THEN "usage".status_code
        ELSE excluded.status_code
    END,
    error_message = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".error_message
        WHEN "usage".status = 'streaming' AND excluded.status = 'pending' THEN "usage".error_message
        ELSE excluded.error_message
    END,
    error_category = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".error_category
        WHEN "usage".status = 'streaming' AND excluded.status = 'pending' THEN "usage".error_category
        ELSE excluded.error_category
    END,
    response_time_ms = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".response_time_ms
        WHEN excluded.response_time_ms IS NULL OR excluded.response_time_ms = 0 THEN COALESCE("usage".response_time_ms, excluded.response_time_ms)
        ELSE excluded.response_time_ms
    END,
    first_byte_time_ms = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".first_byte_time_ms
        WHEN excluded.first_byte_time_ms IS NULL OR excluded.first_byte_time_ms = 0 THEN COALESCE("usage".first_byte_time_ms, excluded.first_byte_time_ms)
        ELSE excluded.first_byte_time_ms
    END,
    status = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".status
        WHEN "usage".status = 'streaming' AND excluded.status = 'pending' THEN "usage".status
        ELSE excluded.status
    END,
  billing_status = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".billing_status
        ELSE excluded.billing_status
    END,
  request_metadata = excluded.request_metadata,
    candidate_id = COALESCE(excluded.candidate_id, "usage".candidate_id),
    candidate_index = COALESCE(excluded.candidate_index, "usage".candidate_index),
    key_name = COALESCE(excluded.key_name, "usage".key_name),
  planner_kind = excluded.planner_kind,
  route_family = excluded.route_family,
  route_kind = excluded.route_kind,
  execution_path = excluded.execution_path,
  local_execution_runtime_miss_reason = excluded.local_execution_runtime_miss_reason,
  finalized_at = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".finalized_at
        ELSE excluded.finalized_at
    END,
  updated_at_unix_secs = CASE
        WHEN "usage".status IN ('completed', 'failed', 'cancelled') AND excluded.status IN ('pending', 'streaming') THEN "usage".updated_at_unix_secs
        ELSE excluded.updated_at_unix_secs
    END
"#;

const SELECT_STALE_PENDING_USAGE_BATCH_SQL: &str = r#"
SELECT
  "usage".request_id,
  "usage".status,
  COALESCE(usage_settlement_snapshots.billing_status, "usage".billing_status) AS billing_status
FROM "usage"
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = "usage".request_id
WHERE "usage".status IN ('pending', 'streaming')
  AND "usage".created_at_unix_ms < ?
ORDER BY "usage".created_at_unix_ms ASC, "usage".request_id ASC
LIMIT ?
"#;

const SELECT_COMPLETED_REQUEST_CANDIDATES_SQL: &str = r#"
SELECT status, extra_data
FROM request_candidates
WHERE request_id = ?
  AND status IN ('streaming', 'success')
"#;

const SQLITE_PROVIDER_IDENTITY_IS_NOT_RESERVED: &str = r#"
(
  (
    provider_id IS NOT NULL
    AND TRIM(provider_id) <> ''
    AND LOWER(TRIM(provider_id)) NOT IN ('unknown', 'unknow', 'pending')
  )
  OR (
    provider_name IS NOT NULL
    AND TRIM(provider_name) <> ''
    AND LOWER(TRIM(provider_name)) NOT IN ('unknown', 'unknow', 'pending')
  )
)
"#;

const SQLITE_USAGE_CACHE_CREATION_TOKENS_EXPR: &str = r#"
CASE
  WHEN COALESCE(cache_creation_input_tokens, 0) = 0
       AND (
         COALESCE(cache_creation_ephemeral_5m_input_tokens, 0)
         + COALESCE(cache_creation_ephemeral_1h_input_tokens, 0)
       ) > 0
  THEN COALESCE(cache_creation_ephemeral_5m_input_tokens, 0)
     + COALESCE(cache_creation_ephemeral_1h_input_tokens, 0)
  ELSE MAX(COALESCE(cache_creation_input_tokens, 0), 0)
END
"#;

const SQLITE_USAGE_EFFECTIVE_INPUT_TOKENS_EXPR: &str = r#"
CASE
  WHEN (
    LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'openai'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'openai:%'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'gemini'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'gemini:%'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'google'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'google:%'
  )
  AND COALESCE(input_tokens, 0) > 0
  AND COALESCE(cache_read_input_tokens, 0) > 0
  THEN MAX(COALESCE(input_tokens, 0) - COALESCE(cache_read_input_tokens, 0), 0)
  ELSE MAX(COALESCE(input_tokens, 0), 0)
END
"#;

const SQLITE_USAGE_TOTAL_INPUT_CONTEXT_EXPR: &str = r#"
CASE
  WHEN (
    LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'openai'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'openai:%'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'gemini'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'gemini:%'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'google'
    OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'google:%'
  )
  THEN (
    CASE
      WHEN COALESCE(input_tokens, 0) > 0 AND COALESCE(cache_read_input_tokens, 0) > 0
      THEN MAX(COALESCE(input_tokens, 0) - COALESCE(cache_read_input_tokens, 0), 0)
      ELSE MAX(COALESCE(input_tokens, 0), 0)
    END
  ) + MAX(COALESCE(cache_read_input_tokens, 0), 0)
  ELSE MAX(COALESCE(input_tokens, 0), 0)
     + (
       CASE
         WHEN COALESCE(cache_creation_input_tokens, 0) = 0
              AND (
                COALESCE(cache_creation_ephemeral_5m_input_tokens, 0)
                + COALESCE(cache_creation_ephemeral_1h_input_tokens, 0)
              ) > 0
         THEN COALESCE(cache_creation_ephemeral_5m_input_tokens, 0)
            + COALESCE(cache_creation_ephemeral_1h_input_tokens, 0)
         ELSE MAX(COALESCE(cache_creation_input_tokens, 0), 0)
       END
     )
     + MAX(COALESCE(cache_read_input_tokens, 0), 0)
END
"#;

const SQLITE_USAGE_CANONICAL_TOTAL_TOKENS_EXPR: &str = r#"
(
  CASE
    WHEN (
      LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'openai'
      OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'openai:%'
      OR LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'gemini'
      OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'gemini:%'
      OR LOWER(COALESCE(endpoint_api_format, api_format, '')) = 'google'
      OR LOWER(COALESCE(endpoint_api_format, api_format, '')) LIKE 'google:%'
    )
    AND COALESCE(input_tokens, 0) > 0
    AND COALESCE(cache_read_input_tokens, 0) > 0
    THEN MAX(COALESCE(input_tokens, 0) - COALESCE(cache_read_input_tokens, 0), 0)
    ELSE MAX(COALESCE(input_tokens, 0), 0)
  END
  + MAX(COALESCE(output_tokens, 0), 0)
  + (
    CASE
      WHEN COALESCE(cache_creation_input_tokens, 0) = 0
           AND (
             COALESCE(cache_creation_ephemeral_5m_input_tokens, 0)
             + COALESCE(cache_creation_ephemeral_1h_input_tokens, 0)
           ) > 0
      THEN COALESCE(cache_creation_ephemeral_5m_input_tokens, 0)
         + COALESCE(cache_creation_ephemeral_1h_input_tokens, 0)
      ELSE MAX(COALESCE(cache_creation_input_tokens, 0), 0)
    END
  )
  + MAX(COALESCE(cache_read_input_tokens, 0), 0)
)
"#;

const SQLITE_USAGE_SUCCESS_FLAG_EXPR: &str = r#"
CASE
  WHEN status <> 'failed'
       AND (status_code IS NULL OR status_code < 400)
       AND error_message IS NULL
  THEN 1
  ELSE 0
END
"#;

const SQLITE_PROVIDER_KEY_SUCCESS_FLAG_EXPR: &str = r#"
CASE
  WHEN status IN ('completed', 'success', 'ok', 'billed', 'settled')
       AND (status_code IS NULL OR status_code < 400)
       AND (error_message IS NULL OR TRIM(error_message) = '')
  THEN 1
  ELSE 0
END
"#;

const SQLITE_PROVIDER_KEY_ERROR_FLAG_EXPR: &str = r#"
CASE
  WHEN status NOT IN ('pending', 'streaming')
       AND NOT (
         status IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND (error_message IS NULL OR TRIM(error_message) = '')
       )
  THEN 1
  ELSE 0
END
"#;

const SQLITE_MONITORING_ERROR_PREDICATE: &str = r#"
(
  LOWER(TRIM(COALESCE(status, ''))) IN ('failed', 'error')
  OR (error_category IS NOT NULL AND TRIM(error_category) <> '')
  OR (
    TRIM(COALESCE(status, '')) = ''
    AND (
      COALESCE(status_code, 0) >= 400
      OR (error_message IS NOT NULL AND TRIM(error_message) <> '')
    )
  )
)
"#;

const SQLITE_FINALIZED_USAGE_PREDICATE: &str = r#"
status NOT IN ('pending', 'streaming')
AND provider_name NOT IN ('unknown', 'pending')
"#;

fn push_sqlite_usage_where(builder: &mut QueryBuilder<'_, Sqlite>, has_where: &mut bool) {
    builder.push(if *has_where { " AND " } else { " WHERE " });
    *has_where = true;
}

fn push_sqlite_usage_list_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    query: &UsageAuditListQuery,
    has_where: &mut bool,
) {
    if let Some(created_from_unix_secs) = query.created_from_unix_secs {
        push_sqlite_usage_where(builder, has_where);
        builder
            .push("created_at_unix_ms >= ")
            .push_bind(created_from_unix_secs as i64);
    }
    if let Some(created_until_unix_secs) = query.created_until_unix_secs {
        push_sqlite_usage_where(builder, has_where);
        builder
            .push("created_at_unix_ms < ")
            .push_bind(created_until_unix_secs as i64);
    }
    if let Some(user_id) = query.user_id.as_deref() {
        push_sqlite_usage_where(builder, has_where);
        builder.push("user_id = ").push_bind(user_id.to_string());
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        push_sqlite_usage_where(builder, has_where);
        builder
            .push("provider_name = ")
            .push_bind(provider_name.to_string());
    }
    if let Some(model) = query.model.as_deref() {
        push_sqlite_usage_where(builder, has_where);
        builder.push("model = ").push_bind(model.to_string());
    }
    if let Some(api_format) = query.api_format.as_deref() {
        push_sqlite_usage_where(builder, has_where);
        builder
            .push("api_format = ")
            .push_bind(api_format.to_string());
    }
    if let Some(client_family) = query.client_family.as_deref().map(str::trim) {
        if !client_family.is_empty() {
            push_sqlite_usage_where(builder, has_where);
            builder
                .push("LOWER(COALESCE(NULLIF(TRIM(CAST(json_extract(request_metadata, '$.client_session_affinity.client_family') AS TEXT)), ''), NULLIF(TRIM(CAST(json_extract(request_metadata, '$.client_family') AS TEXT)), ''))) = ")
                .push_bind(client_family.to_ascii_lowercase());
        }
    }
    if query.exclude_unknown_model_or_provider {
        push_sqlite_usage_where(builder, has_where);
        builder.push(
            "(LOWER(TRIM(COALESCE(model, ''))) NOT IN ('unknown', 'unknow') \
AND LOWER(TRIM(COALESCE(provider_name, ''))) NOT IN ('unknown', 'unknow'))",
        );
    }
    if let Some(statuses) = query.statuses.as_deref() {
        if !statuses.is_empty() {
            push_sqlite_usage_where(builder, has_where);
            builder.push("status IN (");
            let mut separated = builder.separated(", ");
            for status in statuses {
                separated.push_bind(status.to_string());
            }
            separated.push_unseparated(")");
        }
    }
    push_sqlite_usage_excluded_status_codes(builder, has_where, &query.exclude_status_codes);
    if let Some(is_stream) = query.is_stream {
        push_sqlite_usage_where(builder, has_where);
        builder
            .push("is_stream = ")
            .push_bind(if is_stream { 1_i64 } else { 0_i64 });
    }
    if query.error_only {
        push_sqlite_usage_where(builder, has_where);
        builder.push(
            "(status = 'failed' \
OR COALESCE(status_code, 0) >= 400 \
OR (error_message IS NOT NULL AND TRIM(error_message) <> ''))",
        );
    }
}

fn push_sqlite_usage_excluded_status_codes(
    builder: &mut QueryBuilder<'_, Sqlite>,
    has_where: &mut bool,
    status_codes: &[u16],
) {
    if status_codes.is_empty() {
        return;
    }
    push_sqlite_usage_where(builder, has_where);
    builder.push("(status_code IS NULL OR status_code NOT IN (");
    let mut separated = builder.separated(", ");
    for status_code in status_codes {
        separated.push_bind(i64::from(*status_code));
    }
    separated.push_unseparated("))");
}

fn push_sqlite_usage_keyword_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    query: &UsageAuditKeywordSearchQuery,
    has_where: &mut bool,
) {
    push_sqlite_usage_list_filters(
        builder,
        &UsageAuditListQuery {
            created_from_unix_secs: query.created_from_unix_secs,
            created_until_unix_secs: query.created_until_unix_secs,
            user_id: query.user_id.clone(),
            provider_name: query.provider_name.clone(),
            model: query.model.clone(),
            api_format: query.api_format.clone(),
            client_family: query.client_family.clone(),
            exclude_unknown_model_or_provider: query.exclude_unknown_model_or_provider,
            statuses: query.statuses.clone(),
            exclude_status_codes: query.exclude_status_codes.clone(),
            is_stream: query.is_stream,
            error_only: query.error_only,
            limit: None,
            offset: None,
            newest_first: query.newest_first,
        },
        has_where,
    );

    for (index, keyword) in query.keywords.iter().enumerate() {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            continue;
        }
        let pattern = format!("%{}%", keyword.to_ascii_lowercase());
        push_sqlite_usage_where(builder, has_where);
        builder.push("(");
        builder
            .push("LOWER(COALESCE(model, '')) LIKE ")
            .push_bind(pattern.clone());
        builder
            .push(" OR LOWER(COALESCE(provider_name, '')) LIKE ")
            .push_bind(pattern.clone());
        if query.auth_user_reader_available {
            let matched_user_ids = query
                .matched_user_ids_by_keyword
                .get(index)
                .cloned()
                .unwrap_or_default();
            if !matched_user_ids.is_empty() {
                builder.push(" OR user_id IN (");
                let mut separated = builder.separated(", ");
                for user_id in matched_user_ids {
                    separated.push_bind(user_id);
                }
                separated.push_unseparated(")");
            }
        } else {
            builder
                .push(" OR user_id IN (SELECT id FROM users WHERE LOWER(COALESCE(username, '')) LIKE ")
                .push_bind(pattern.clone());
            builder.push(")");
        }
        if query.auth_api_key_reader_available {
            let matched_api_key_ids = query
                .matched_api_key_ids_by_keyword
                .get(index)
                .cloned()
                .unwrap_or_default();
            if !matched_api_key_ids.is_empty() {
                builder.push(" OR api_key_id IN (");
                let mut separated = builder.separated(", ");
                for api_key_id in matched_api_key_ids {
                    separated.push_bind(api_key_id);
                }
                separated.push_unseparated(")");
            }
        } else {
            builder
                .push(" OR api_key_id IN (SELECT id FROM api_keys WHERE LOWER(COALESCE(name, '')) LIKE ")
                .push_bind(pattern);
            builder.push(")");
        }
        builder.push(")");
    }

    if let Some(username_keyword) = query
        .username_keyword
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        push_sqlite_usage_where(builder, has_where);
        if query.auth_user_reader_available {
            if query.matched_user_ids_for_username.is_empty() {
                builder.push("0 = 1");
            } else {
                builder.push("user_id IN (");
                let mut separated = builder.separated(", ");
                for user_id in &query.matched_user_ids_for_username {
                    separated.push_bind(user_id.clone());
                }
                separated.push_unseparated(")");
            }
        } else {
            builder
                .push("user_id IN (SELECT id FROM users WHERE LOWER(COALESCE(username, '')) LIKE ")
                .push_bind(format!("%{}%", username_keyword.to_ascii_lowercase()));
            builder.push(")");
        }
    }
}

fn push_sqlite_usage_summary_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    query: &UsageAuditSummaryQuery,
    has_where: &mut bool,
) {
    push_sqlite_usage_where(builder, has_where);
    builder
        .push("created_at_unix_ms >= ")
        .push_bind(query.created_from_unix_secs as i64);
    push_sqlite_usage_where(builder, has_where);
    builder
        .push("created_at_unix_ms < ")
        .push_bind(query.created_until_unix_secs as i64);
    if let Some(user_id) = query.user_id.as_deref() {
        push_sqlite_usage_where(builder, has_where);
        builder.push("user_id = ").push_bind(user_id.to_string());
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        push_sqlite_usage_where(builder, has_where);
        builder
            .push("provider_name = ")
            .push_bind(provider_name.to_string());
    }
    if let Some(model) = query.model.as_deref() {
        push_sqlite_usage_where(builder, has_where);
        builder.push("model = ").push_bind(model.to_string());
    }
}

fn push_sqlite_usage_order_limit_offset(
    builder: &mut QueryBuilder<'_, Sqlite>,
    newest_first: bool,
    limit: Option<usize>,
    offset: Option<usize>,
) {
    if newest_first {
        builder.push(" ORDER BY created_at_unix_ms DESC, id ASC");
    } else {
        builder.push(" ORDER BY created_at_unix_ms ASC, request_id ASC");
    }
    if let Some(limit) = limit {
        builder.push(" LIMIT ").push_bind(limit as i64);
    }
    if let Some(offset) = offset {
        builder.push(" OFFSET ").push_bind(offset as i64);
    }
}

fn sqlite_usage_aggregation_group_expr(group_by: UsageAuditAggregationGroupBy) -> &'static str {
    match group_by {
        UsageAuditAggregationGroupBy::Model => "COALESCE(NULLIF(model, ''), 'unknown')",
        UsageAuditAggregationGroupBy::Provider => {
            "CASE WHEN provider_id IS NOT NULL \
AND TRIM(provider_id) <> '' \
AND LOWER(TRIM(provider_id)) NOT IN ('unknown', 'unknow', 'pending') \
THEN TRIM(provider_id) ELSE TRIM(provider_name) END"
        }
        UsageAuditAggregationGroupBy::ApiFormat => "COALESCE(NULLIF(api_format, ''), 'unknown')",
        UsageAuditAggregationGroupBy::User => "user_id",
    }
}

fn sqlite_usage_aggregation_secondary_expr(group_by: UsageAuditAggregationGroupBy) -> &'static str {
    match group_by {
        UsageAuditAggregationGroupBy::Provider => {
            "CASE WHEN SUM(CASE WHEN provider_id IS NOT NULL \
AND TRIM(provider_id) <> '' \
AND LOWER(TRIM(provider_id)) NOT IN ('unknown', 'unknow', 'pending') \
THEN 1 ELSE 0 END) > 0 THEN 'provider_id' \
WHEN SUM(CASE WHEN provider_name IS NOT NULL \
AND TRIM(provider_name) <> '' \
AND LOWER(TRIM(provider_name)) NOT IN ('unknown', 'unknow', 'pending') \
THEN 1 ELSE 0 END) > 0 THEN 'legacy_name' \
ELSE NULL END"
        }
        _ => "NULL",
    }
}

fn sqlite_aggregate_u64(row: &SqliteRow, field: &str) -> Result<u64, DataLayerError> {
    Ok(row.try_get::<i64, _>(field).map_sql_err()?.max(0) as u64)
}

fn sqlite_optional_u64(row: &SqliteRow, field: &str) -> Result<Option<u64>, DataLayerError> {
    Ok(row
        .try_get::<Option<i64>, _>(field)
        .map_sql_err()?
        .map(|value| value.max(0) as u64))
}

fn push_sqlite_usage_provider_performance_base_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    query: &UsageProviderPerformanceQuery,
    has_where: &mut bool,
) {
    push_sqlite_usage_range(
        builder,
        has_where,
        query.created_from_unix_secs,
        query.created_until_unix_secs,
    );
    push_sqlite_usage_where(builder, has_where);
    builder.push(
        "COALESCE(status, '') NOT IN ('pending', 'streaming') \
AND provider_id IS NOT NULL AND TRIM(provider_id) <> '' \
AND LOWER(TRIM(provider_id)) NOT IN ('unknown', 'pending') \
AND LOWER(TRIM(COALESCE(provider_name, ''))) NOT IN ('unknown', 'pending')",
    );
    push_sqlite_usage_provider_performance_filters(builder, query, has_where);
}

fn decode_sqlite_usage_audit_summary_row(
    row: &SqliteRow,
) -> Result<StoredUsageAuditSummary, DataLayerError> {
    Ok(StoredUsageAuditSummary {
        total_requests: sqlite_aggregate_u64(row, "total_requests")?,
        input_tokens: sqlite_aggregate_u64(row, "input_tokens")?,
        output_tokens: sqlite_aggregate_u64(row, "output_tokens")?,
        recorded_total_tokens: sqlite_aggregate_u64(row, "recorded_total_tokens")?,
        cache_creation_tokens: sqlite_aggregate_u64(row, "cache_creation_tokens")?,
        cache_creation_ephemeral_5m_tokens: sqlite_aggregate_u64(
            row,
            "cache_creation_ephemeral_5m_tokens",
        )?,
        cache_creation_ephemeral_1h_tokens: sqlite_aggregate_u64(
            row,
            "cache_creation_ephemeral_1h_tokens",
        )?,
        cache_read_tokens: sqlite_aggregate_u64(row, "cache_read_tokens")?,
        total_cost_usd: sqlite_real(row, "total_cost_usd")?,
        actual_total_cost_usd: sqlite_real(row, "actual_total_cost_usd")?,
        cache_creation_cost_usd: sqlite_real(row, "cache_creation_cost_usd")?,
        cache_read_cost_usd: sqlite_real(row, "cache_read_cost_usd")?,
        total_response_time_ms: sqlite_real(row, "total_response_time_ms")?,
        error_requests: sqlite_aggregate_u64(row, "error_requests")?,
    })
}

fn decode_sqlite_usage_aggregation_row(
    row: &SqliteRow,
) -> Result<StoredUsageAuditAggregation, DataLayerError> {
    Ok(StoredUsageAuditAggregation {
        group_key: row.try_get::<String, _>("group_key").map_sql_err()?,
        display_name: row.try_get("display_name").map_sql_err()?,
        secondary_name: row.try_get("secondary_name").map_sql_err()?,
        request_count: sqlite_aggregate_u64(row, "request_count")?,
        total_tokens: sqlite_aggregate_u64(row, "total_tokens")?,
        output_tokens: sqlite_aggregate_u64(row, "output_tokens")?,
        effective_input_tokens: sqlite_aggregate_u64(row, "effective_input_tokens")?,
        total_input_context: sqlite_aggregate_u64(row, "total_input_context")?,
        cache_creation_tokens: sqlite_aggregate_u64(row, "cache_creation_tokens")?,
        cache_creation_ephemeral_5m_tokens: sqlite_aggregate_u64(
            row,
            "cache_creation_ephemeral_5m_tokens",
        )?,
        cache_creation_ephemeral_1h_tokens: sqlite_aggregate_u64(
            row,
            "cache_creation_ephemeral_1h_tokens",
        )?,
        cache_read_tokens: sqlite_aggregate_u64(row, "cache_read_tokens")?,
        total_cost_usd: sqlite_real(row, "total_cost_usd")?,
        actual_total_cost_usd: sqlite_real(row, "actual_total_cost_usd")?,
        avg_response_time_ms: sqlite_optional_real(row, "avg_response_time_ms")?,
        success_count: row
            .try_get::<Option<i64>, _>("success_count")
            .map_sql_err()?
            .map(|value| value.max(0) as u64),
    })
}

fn push_sqlite_usage_range(
    builder: &mut QueryBuilder<'_, Sqlite>,
    has_where: &mut bool,
    created_from_unix_secs: u64,
    created_until_unix_secs: u64,
) {
    push_sqlite_usage_where(builder, has_where);
    builder
        .push("created_at_unix_ms >= ")
        .push_bind(created_from_unix_secs as i64);
    push_sqlite_usage_where(builder, has_where);
    builder
        .push("created_at_unix_ms < ")
        .push_bind(created_until_unix_secs as i64);
}

fn push_sqlite_usage_optional_text_filter(
    builder: &mut QueryBuilder<'_, Sqlite>,
    has_where: &mut bool,
    column: &str,
    value: Option<&str>,
) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        push_sqlite_usage_where(builder, has_where);
        builder
            .push(column)
            .push(" = ")
            .push_bind(value.to_string());
    }
}

fn push_sqlite_usage_bool_filter(
    builder: &mut QueryBuilder<'_, Sqlite>,
    has_where: &mut bool,
    column: &str,
    value: Option<bool>,
) {
    if let Some(value) = value {
        push_sqlite_usage_where(builder, has_where);
        builder
            .push(column)
            .push(" = ")
            .push_bind(if value { 1_i64 } else { 0_i64 });
    }
}

fn push_sqlite_usage_finalized_filter(
    builder: &mut QueryBuilder<'_, Sqlite>,
    has_where: &mut bool,
) {
    push_sqlite_usage_where(builder, has_where);
    builder.push(SQLITE_FINALIZED_USAGE_PREDICATE);
}

fn push_sqlite_usage_provider_performance_filters(
    builder: &mut QueryBuilder<'_, Sqlite>,
    query: &UsageProviderPerformanceQuery,
    has_where: &mut bool,
) {
    push_sqlite_usage_optional_text_filter(
        builder,
        has_where,
        "provider_id",
        query.provider_id.as_deref(),
    );
    push_sqlite_usage_optional_text_filter(builder, has_where, "model", query.model.as_deref());
    push_sqlite_usage_optional_text_filter(
        builder,
        has_where,
        "api_format",
        query.api_format.as_deref(),
    );
    push_sqlite_usage_optional_text_filter(
        builder,
        has_where,
        "endpoint_kind",
        query.endpoint_kind.as_deref(),
    );
    push_sqlite_usage_bool_filter(builder, has_where, "is_stream", query.is_stream);
    push_sqlite_usage_bool_filter(
        builder,
        has_where,
        "has_format_conversion",
        query.has_format_conversion,
    );
}

fn sqlite_usage_metadata_input_price_expr() -> &'static str {
    r#"
COALESCE(
  CAST(json_extract(request_metadata, '$.input_price_per_1m') AS REAL),
  CAST(json_extract(request_metadata, '$.settlement_snapshot.input_price_per_1m') AS REAL),
  CAST(json_extract(request_metadata, '$.billing_snapshot.input_price_per_1m') AS REAL),
  0
)
"#
}

fn sqlite_usage_bucket_expr(
    granularity: UsageTimeSeriesGranularity,
    tz_offset_minutes: i32,
) -> String {
    let offset = i64::from(tz_offset_minutes) * 60;
    match granularity {
        UsageTimeSeriesGranularity::Hour => {
            format!(
                "strftime('%Y-%m-%dT%H:00:00+00:00', created_at_unix_ms + ({offset}), 'unixepoch')"
            )
        }
        UsageTimeSeriesGranularity::Day => {
            format!("date(created_at_unix_ms + ({offset}), 'unixepoch')")
        }
    }
}

fn sqlite_usage_local_date_expr(tz_offset_minutes: i32) -> String {
    let offset = i64::from(tz_offset_minutes) * 60;
    format!("date(created_at_unix_ms + ({offset}), 'unixepoch')")
}

fn sqlite_usage_breakdown_group_expr(group_by: UsageBreakdownGroupBy) -> &'static str {
    match group_by {
        UsageBreakdownGroupBy::Model => "COALESCE(NULLIF(model, ''), 'unknown')",
        UsageBreakdownGroupBy::Provider => "COALESCE(NULLIF(provider_name, ''), 'unknown')",
        UsageBreakdownGroupBy::ApiFormat => "COALESCE(NULLIF(api_format, ''), 'unknown')",
    }
}

fn sqlite_usage_leaderboard_group_expr(
    group_by: UsageLeaderboardGroupBy,
) -> (&'static str, &'static str, &'static str) {
    match group_by {
        UsageLeaderboardGroupBy::Model => {
            ("model", "NULL", "model IS NOT NULL AND TRIM(model) <> ''")
        }
        UsageLeaderboardGroupBy::User => (
            "user_id",
            "(SELECT users.username FROM users WHERE users.id = user_id)",
            "user_id IS NOT NULL AND TRIM(user_id) <> ''",
        ),
        UsageLeaderboardGroupBy::ApiKey => (
            "api_key_id",
            "(SELECT api_keys.name FROM api_keys WHERE api_keys.id = api_key_id)",
            "api_key_id IS NOT NULL AND TRIM(api_key_id) <> ''",
        ),
    }
}

fn sqlite_usage_body_sql_columns(field: UsageBodyField) -> (&'static str, &'static str) {
    match field {
        UsageBodyField::RequestBody => ("request_body", "request_body_compressed"),
        UsageBodyField::ProviderRequestBody => {
            ("provider_request_body", "provider_request_body_compressed")
        }
        UsageBodyField::ResponseBody => ("response_body", "response_body_compressed"),
        UsageBodyField::ClientResponseBody => {
            ("client_response_body", "client_response_body_compressed")
        }
    }
}

fn inflate_usage_json_value(bytes: &[u8]) -> Result<serde_json::Value, DataLayerError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut json_bytes = Vec::new();
    decoder.read_to_end(&mut json_bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to decompress usage json: {err}"))
    })?;
    serde_json::from_slice(&json_bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to parse decompressed usage json: {err}"))
    })
}

fn parse_usage_json_text(raw: &str) -> Result<serde_json::Value, DataLayerError> {
    serde_json::from_str(raw).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to parse usage json: {err}"))
    })
}

#[derive(Debug, Clone)]
pub struct SqliteUsageWriteRepository {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
pub struct SqliteUsageReadRepository {
    pool: SqlitePool,
}

#[derive(Debug, Clone, Copy, Default)]
struct SqliteProviderPerformancePercentiles {
    p90_response_time_ms: Option<u64>,
    p99_response_time_ms: Option<u64>,
    p90_first_byte_time_ms: Option<u64>,
    p99_first_byte_time_ms: Option<u64>,
}

impl SqliteUsageReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn summarize_usage_daily_heatmap_raw_from_range(
        &self,
        created_from_unix_secs: u64,
        created_until_unix_secs: u64,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  date(created_at_unix_ms, 'unixepoch') AS date,
  COUNT(*) AS requests,
  COALESCE(SUM(
    MAX(COALESCE(input_tokens, 0), 0)
    + MAX(COALESCE(output_tokens, 0), 0)
    + {cache_creation_expr}
    + MAX(COALESCE(cache_read_input_tokens, 0), 0)
  ), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST(actual_total_cost_usd AS REAL), 0)), 0)
    AS actual_total_cost_usd
FROM "usage"
"#,
            cache_creation_expr = SQLITE_USAGE_CACHE_CREATION_TOKENS_EXPR
        ));
        let mut has_where = false;
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder
            .push("created_at_unix_ms >= ")
            .push_bind(created_from_unix_secs as i64);
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder
            .push("created_at_unix_ms < ")
            .push_bind(created_until_unix_secs as i64);
        push_sqlite_usage_finalized_filter(&mut builder, &mut has_where);
        push_sqlite_usage_optional_text_filter(&mut builder, &mut has_where, "user_id", user_id);
        builder.push(" GROUP BY date ORDER BY date ASC");

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_sqlite_usage_daily_summary).collect()
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
  date("date", 'unixepoch') AS date,
  total_requests AS requests,
  input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens AS total_tokens,
  total_cost AS total_cost_usd,
  total_cost AS actual_total_cost_usd
FROM stats_user_daily
WHERE user_id = ?
  AND "date" >= ?
  AND "date" < ?
  AND total_requests > 0
ORDER BY "date" ASC
"#,
            )
            .bind(user_id)
            .bind(created_from_unix_secs as i64)
            .bind(created_until_unix_secs as i64)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  date("date", 'unixepoch') AS date,
  total_requests AS requests,
  input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens AS total_tokens,
  total_cost AS total_cost_usd,
  actual_total_cost AS actual_total_cost_usd
FROM stats_daily
WHERE "date" >= ?
  AND "date" < ?
  AND total_requests > 0
ORDER BY "date" ASC
"#,
            )
            .bind(created_from_unix_secs as i64)
            .bind(created_until_unix_secs as i64)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?
        };

        rows.iter().map(map_sqlite_usage_daily_summary).collect()
    }

    async fn summarize_provider_performance_percentiles(
        &self,
        query: &UsageProviderPerformanceQuery,
    ) -> Result<SqliteProviderPerformancePercentiles, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
WITH filtered_usage AS (
  SELECT
    MAX(COALESCE(response_time_ms, 0), 0) AS response_time_ms,
    MAX(COALESCE(first_byte_time_ms, 0), 0) AS first_byte_time_ms,
    response_time_ms IS NOT NULL AS has_response_time,
    first_byte_time_ms IS NOT NULL AS has_first_byte_time,
    CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
      THEN 1
      ELSE 0
    END AS success_flag
  FROM "usage"
"#,
        );
        let mut has_where = false;
        push_sqlite_usage_provider_performance_base_filters(&mut builder, query, &mut has_where);
        builder.push(
            r#"
),
response_ranked AS (
  SELECT
    response_time_ms AS value,
    ROW_NUMBER() OVER (ORDER BY response_time_ms) AS rn,
    COUNT(response_time_ms) OVER () AS n
  FROM filtered_usage
  WHERE success_flag = 1 AND has_response_time
),
first_byte_ranked AS (
  SELECT
    first_byte_time_ms AS value,
    ROW_NUMBER() OVER (ORDER BY first_byte_time_ms) AS rn,
    COUNT(first_byte_time_ms) OVER () AS n
  FROM filtered_usage
  WHERE success_flag = 1 AND has_first_byte_time
),
response_positions AS (
  SELECT
    n,
    0.9 * (n - 1) AS p90_pos,
    0.99 * (n - 1) AS p99_pos
  FROM response_ranked
  GROUP BY n
),
first_byte_positions AS (
  SELECT
    n,
    0.9 * (n - 1) AS p90_pos,
    0.99 * (n - 1) AS p99_pos
  FROM first_byte_ranked
  GROUP BY n
),
response_percentiles AS (
  SELECT
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p90_pos = CAST(positions.p90_pos AS INTEGER)
          THEN CAST(positions.p90_pos AS INTEGER) + 1
          ELSE CAST(positions.p90_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p90_pos - CAST(positions.p90_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p90_response_time_ms,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p99_pos = CAST(positions.p99_pos AS INTEGER)
          THEN CAST(positions.p99_pos AS INTEGER) + 1
          ELSE CAST(positions.p99_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p99_pos - CAST(positions.p99_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p99_response_time_ms
  FROM response_positions AS positions
  JOIN response_ranked AS ranked ON ranked.n = positions.n
  GROUP BY positions.n, positions.p90_pos, positions.p99_pos
),
first_byte_percentiles AS (
  SELECT
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p90_pos = CAST(positions.p90_pos AS INTEGER)
          THEN CAST(positions.p90_pos AS INTEGER) + 1
          ELSE CAST(positions.p90_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p90_pos - CAST(positions.p90_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p90_first_byte_time_ms,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p99_pos = CAST(positions.p99_pos AS INTEGER)
          THEN CAST(positions.p99_pos AS INTEGER) + 1
          ELSE CAST(positions.p99_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p99_pos - CAST(positions.p99_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p99_first_byte_time_ms
  FROM first_byte_positions AS positions
  JOIN first_byte_ranked AS ranked ON ranked.n = positions.n
  GROUP BY positions.n, positions.p90_pos, positions.p99_pos
)
SELECT
  (SELECT p90_response_time_ms FROM response_percentiles) AS p90_response_time_ms,
  (SELECT p99_response_time_ms FROM response_percentiles) AS p99_response_time_ms,
  (SELECT p90_first_byte_time_ms FROM first_byte_percentiles) AS p90_first_byte_time_ms,
  (SELECT p99_first_byte_time_ms FROM first_byte_percentiles) AS p99_first_byte_time_ms
"#,
        );

        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(SqliteProviderPerformancePercentiles {
            p90_response_time_ms: sqlite_optional_u64(&row, "p90_response_time_ms")?,
            p99_response_time_ms: sqlite_optional_u64(&row, "p99_response_time_ms")?,
            p90_first_byte_time_ms: sqlite_optional_u64(&row, "p90_first_byte_time_ms")?,
            p99_first_byte_time_ms: sqlite_optional_u64(&row, "p99_first_byte_time_ms")?,
        })
    }

    async fn summarize_provider_performance_provider_percentiles(
        &self,
        query: &UsageProviderPerformanceQuery,
        provider_ids: &[String],
    ) -> Result<BTreeMap<String, SqliteProviderPerformancePercentiles>, DataLayerError> {
        if provider_ids.is_empty() {
            return Ok(BTreeMap::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
WITH filtered_usage AS (
  SELECT
    provider_id,
    MAX(COALESCE(response_time_ms, 0), 0) AS response_time_ms,
    MAX(COALESCE(first_byte_time_ms, 0), 0) AS first_byte_time_ms,
    response_time_ms IS NOT NULL AS has_response_time,
    first_byte_time_ms IS NOT NULL AS has_first_byte_time,
    CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
      THEN 1
      ELSE 0
    END AS success_flag
  FROM "usage"
"#,
        );
        let mut has_where = false;
        push_sqlite_usage_provider_performance_base_filters(&mut builder, query, &mut has_where);
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push("provider_id IN (");
        {
            let mut separated = builder.separated(", ");
            for provider_id in provider_ids {
                separated.push_bind(provider_id.clone());
            }
        }
        builder.push(
            r#")
),
response_ranked AS (
  SELECT
    provider_id,
    response_time_ms AS value,
    ROW_NUMBER() OVER (PARTITION BY provider_id ORDER BY response_time_ms) AS rn,
    COUNT(response_time_ms) OVER (PARTITION BY provider_id) AS n
  FROM filtered_usage
  WHERE success_flag = 1 AND has_response_time
),
first_byte_ranked AS (
  SELECT
    provider_id,
    first_byte_time_ms AS value,
    ROW_NUMBER() OVER (PARTITION BY provider_id ORDER BY first_byte_time_ms) AS rn,
    COUNT(first_byte_time_ms) OVER (PARTITION BY provider_id) AS n
  FROM filtered_usage
  WHERE success_flag = 1 AND has_first_byte_time
),
response_positions AS (
  SELECT
    provider_id,
    n,
    0.9 * (n - 1) AS p90_pos,
    0.99 * (n - 1) AS p99_pos
  FROM response_ranked
  GROUP BY provider_id, n
),
first_byte_positions AS (
  SELECT
    provider_id,
    n,
    0.9 * (n - 1) AS p90_pos,
    0.99 * (n - 1) AS p99_pos
  FROM first_byte_ranked
  GROUP BY provider_id, n
),
response_percentiles AS (
  SELECT
    positions.provider_id,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p90_pos = CAST(positions.p90_pos AS INTEGER)
          THEN CAST(positions.p90_pos AS INTEGER) + 1
          ELSE CAST(positions.p90_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p90_pos - CAST(positions.p90_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p90_response_time_ms,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p99_pos = CAST(positions.p99_pos AS INTEGER)
          THEN CAST(positions.p99_pos AS INTEGER) + 1
          ELSE CAST(positions.p99_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p99_pos - CAST(positions.p99_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p99_response_time_ms
  FROM response_positions AS positions
  JOIN response_ranked AS ranked
    ON ranked.provider_id = positions.provider_id AND ranked.n = positions.n
  GROUP BY positions.provider_id, positions.n, positions.p90_pos, positions.p99_pos
),
first_byte_percentiles AS (
  SELECT
    positions.provider_id,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p90_pos = CAST(positions.p90_pos AS INTEGER)
          THEN CAST(positions.p90_pos AS INTEGER) + 1
          ELSE CAST(positions.p90_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p90_pos - CAST(positions.p90_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p90_first_byte_time_ms,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p99_pos = CAST(positions.p99_pos AS INTEGER)
          THEN CAST(positions.p99_pos AS INTEGER) + 1
          ELSE CAST(positions.p99_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p99_pos - CAST(positions.p99_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p99_first_byte_time_ms
  FROM first_byte_positions AS positions
  JOIN first_byte_ranked AS ranked
    ON ranked.provider_id = positions.provider_id AND ranked.n = positions.n
  GROUP BY positions.provider_id, positions.n, positions.p90_pos, positions.p99_pos
),
provider_ids AS (
  SELECT DISTINCT provider_id FROM filtered_usage
)
SELECT
  provider_ids.provider_id,
  response_percentiles.p90_response_time_ms,
  response_percentiles.p99_response_time_ms,
  first_byte_percentiles.p90_first_byte_time_ms,
  first_byte_percentiles.p99_first_byte_time_ms
FROM provider_ids
LEFT JOIN response_percentiles ON response_percentiles.provider_id = provider_ids.provider_id
LEFT JOIN first_byte_percentiles ON first_byte_percentiles.provider_id = provider_ids.provider_id
"#,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let mut values = BTreeMap::new();
        for row in rows {
            values.insert(
                row.try_get::<String, _>("provider_id").map_sql_err()?,
                SqliteProviderPerformancePercentiles {
                    p90_response_time_ms: sqlite_optional_u64(&row, "p90_response_time_ms")?,
                    p99_response_time_ms: sqlite_optional_u64(&row, "p99_response_time_ms")?,
                    p90_first_byte_time_ms: sqlite_optional_u64(&row, "p90_first_byte_time_ms")?,
                    p99_first_byte_time_ms: sqlite_optional_u64(&row, "p99_first_byte_time_ms")?,
                },
            );
        }
        Ok(values)
    }

    async fn fetch_usage_items(
        &self,
        mut builder: QueryBuilder<'_, Sqlite>,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_usage_row).collect()
    }

    pub async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(USAGE_COLUMNS);
        let mut has_where = false;
        push_sqlite_usage_list_filters(&mut builder, query, &mut has_where);
        push_sqlite_usage_order_limit_offset(
            &mut builder,
            query.newest_first,
            query.limit,
            query.offset,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_usage_row).collect()
    }

    pub async fn count_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<u64, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(r#"SELECT COUNT(*) AS total FROM "usage""#);
        let mut has_where = false;
        push_sqlite_usage_list_filters(&mut builder, query, &mut has_where);

        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(row.try_get::<i64, _>("total").map_sql_err()?.max(0) as u64)
    }

    pub async fn list_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(USAGE_COLUMNS);
        let mut has_where = false;
        push_sqlite_usage_keyword_filters(&mut builder, query, &mut has_where);
        push_sqlite_usage_order_limit_offset(
            &mut builder,
            query.newest_first,
            query.limit,
            query.offset,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_usage_row).collect()
    }

    pub async fn count_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<u64, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(r#"SELECT COUNT(*) AS total FROM "usage""#);
        let mut has_where = false;
        push_sqlite_usage_keyword_filters(&mut builder, query, &mut has_where);

        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(row.try_get::<i64, _>("total").map_sql_err()?.max(0) as u64)
    }

    pub async fn summarize_usage_audits(
        &self,
        query: &UsageAuditSummaryQuery,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(StoredUsageAuditSummary::default());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  COUNT(*) AS total_requests,
  COALESCE(SUM(MAX(COALESCE(input_tokens, 0), 0)), 0) AS input_tokens,
  COALESCE(SUM(MAX(COALESCE(output_tokens, 0), 0)), 0) AS output_tokens,
  COALESCE(SUM(MAX(COALESCE(total_tokens, 0), 0)), 0) AS recorded_total_tokens,
  COALESCE(SUM({cache_creation_expr}), 0) AS cache_creation_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_creation_ephemeral_5m_input_tokens, 0), 0)), 0)
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_creation_ephemeral_1h_input_tokens, 0), 0)), 0)
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST(actual_total_cost_usd AS REAL), 0)), 0)
    AS actual_total_cost_usd,
  COALESCE(SUM(COALESCE(CAST(cache_creation_cost_usd AS REAL), 0)), 0)
    AS cache_creation_cost_usd,
  COALESCE(SUM(COALESCE(CAST(cache_read_cost_usd AS REAL), 0)), 0)
    AS cache_read_cost_usd,
  COALESCE(SUM(MAX(COALESCE(response_time_ms, 0), 0)), 0) AS total_response_time_ms,
  COALESCE(SUM(
    CASE
      WHEN COALESCE(status_code, 0) >= 400
        OR (error_message IS NOT NULL AND TRIM(error_message) <> '')
      THEN 1 ELSE 0
    END
  ), 0) AS error_requests
FROM "usage"
"#,
            cache_creation_expr = SQLITE_USAGE_CACHE_CREATION_TOKENS_EXPR
        ));
        let mut has_where = false;
        push_sqlite_usage_summary_filters(&mut builder, query, &mut has_where);

        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        decode_sqlite_usage_audit_summary_row(&row)
    }

    pub async fn aggregate_usage_audits(
        &self,
        query: &UsageAuditAggregationQuery,
    ) -> Result<Vec<StoredUsageAuditAggregation>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs || query.limit == 0 {
            return Ok(Vec::new());
        }

        let group_expr = sqlite_usage_aggregation_group_expr(query.group_by);
        let secondary_expr = sqlite_usage_aggregation_secondary_expr(query.group_by);
        let display_expr = if matches!(query.group_by, UsageAuditAggregationGroupBy::Provider) {
            "CASE WHEN provider_name IS NOT NULL AND TRIM(provider_name) <> '' AND LOWER(TRIM(provider_name)) NOT IN ('unknown', 'unknow', 'pending') THEN TRIM(provider_name) ELSE NULL END"
        } else {
            "NULL"
        };
        let avg_response_expr = if matches!(
            query.group_by,
            UsageAuditAggregationGroupBy::Provider | UsageAuditAggregationGroupBy::ApiFormat
        ) {
            "CASE WHEN COUNT(*) = 0 THEN 0 ELSE COALESCE(SUM(MAX(COALESCE(response_time_ms, 0), 0)), 0) * 1.0 / COUNT(*) END"
        } else {
            "NULL"
        };
        let success_count_expr = if matches!(query.group_by, UsageAuditAggregationGroupBy::Provider)
        {
            "COALESCE(SUM(CASE WHEN status IN ('completed', 'success', 'ok', 'billed', 'settled') AND (status_code IS NULL OR status_code < 400) THEN 1 ELSE 0 END), 0)"
        } else {
            "NULL"
        };

        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  {group_expr} AS group_key,
  {display_expr} AS display_name,
  {secondary_expr} AS secondary_name,
  COUNT(*) AS request_count,
  COALESCE(SUM(MAX(COALESCE(total_tokens, 0), 0)), 0) AS total_tokens,
  COALESCE(SUM(MAX(COALESCE(output_tokens, 0), 0)), 0) AS output_tokens,
  COALESCE(SUM({effective_input_expr}), 0) AS effective_input_tokens,
  COALESCE(SUM({total_input_context_expr}), 0) AS total_input_context,
  COALESCE(SUM({cache_creation_expr}), 0) AS cache_creation_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_creation_ephemeral_5m_input_tokens, 0), 0)), 0)
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_creation_ephemeral_1h_input_tokens, 0), 0)), 0)
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST(actual_total_cost_usd AS REAL), 0)), 0)
    AS actual_total_cost_usd,
  {avg_response_expr} AS avg_response_time_ms,
  {success_count_expr} AS success_count
FROM "usage"
"#,
            effective_input_expr = SQLITE_USAGE_EFFECTIVE_INPUT_TOKENS_EXPR,
            total_input_context_expr = SQLITE_USAGE_TOTAL_INPUT_CONTEXT_EXPR,
            secondary_expr = secondary_expr,
            cache_creation_expr = SQLITE_USAGE_CACHE_CREATION_TOKENS_EXPR
        ));
        let mut has_where = false;
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder
            .push("created_at_unix_ms >= ")
            .push_bind(query.created_from_unix_secs as i64);
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder
            .push("created_at_unix_ms < ")
            .push_bind(query.created_until_unix_secs as i64);
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push("status NOT IN ('pending', 'streaming')");
        if query.exclude_reserved_provider_labels {
            push_sqlite_usage_where(&mut builder, &mut has_where);
            builder.push(SQLITE_PROVIDER_IDENTITY_IS_NOT_RESERVED);
        }
        if matches!(query.group_by, UsageAuditAggregationGroupBy::User) {
            push_sqlite_usage_where(&mut builder, &mut has_where);
            builder.push("user_id IS NOT NULL AND TRIM(user_id) <> ''");
        }
        builder
            .push(" GROUP BY group_key")
            .push(" ORDER BY request_count DESC, group_key ASC")
            .push(" LIMIT ")
            .push_bind(query.limit as i64);

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(decode_sqlite_usage_aggregation_row)
            .collect()
    }

    pub async fn summarize_usage_totals_by_user_ids(
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

        let mut aggregate_builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  user_id,
  COALESCE(SUM(total_requests), 0) AS request_count,
  COALESCE(
    SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens),
    0
  ) AS total_tokens,
  MAX("date") AS latest_date
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

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  "usage".user_id,
  COUNT(*) AS request_count,
  COALESCE(SUM(MAX(COALESCE("usage".total_tokens, 0), 0)), 0) AS total_tokens
FROM "usage"
JOIN (
"#,
        );
        for (index, user_id) in unique_user_ids.iter().enumerate() {
            if index > 0 {
                builder.push(" UNION ALL ");
            }
            let cutoff = aggregate_cutoffs.get(user_id).copied().unwrap_or_default();
            builder
                .push("SELECT ")
                .push_bind(user_id.clone())
                .push(" AS user_id, ")
                .push_bind(to_i64(cutoff, "usage aggregate cutoff")?)
                .push(" AS cutoff_unix_secs");
        }
        builder.push(
            r#"
) AS requested ON requested.user_id = "usage".user_id
WHERE "usage".created_at_unix_ms >= requested.cutoff_unix_secs
  AND "usage".status NOT IN ('pending', 'streaming')
  AND "usage".provider_name NOT IN ('unknown', 'pending')
GROUP BY "usage".user_id
ORDER BY "usage".user_id ASC
"#,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        for row in rows {
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
  COALESCE(SUM(COALESCE(CAST(total_cost AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST(total_cost AS REAL), 0)), 0) AS actual_total_cost_usd,
  COALESCE(SUM(error_requests), 0) AS error_requests,
  0.0 AS response_time_sum_ms,
  0 AS response_time_samples
FROM stats_user_daily
WHERE user_id = ?
  AND "date" >= ?
  AND "date" < ?
"#,
            )
            .bind(user_id)
            .bind(query.created_from_unix_secs as i64)
            .bind(query.created_until_unix_secs as i64)
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
  0.0 AS cache_creation_cost_usd,
  0.0 AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(CAST(total_cost AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST(actual_total_cost AS REAL), 0)), 0) AS actual_total_cost_usd,
  COALESCE(SUM(error_requests), 0) AS error_requests,
  0.0 AS response_time_sum_ms,
  0 AS response_time_samples
FROM stats_daily
WHERE "date" >= ?
  AND "date" < ?
"#,
            )
            .bind(query.created_from_unix_secs as i64)
            .bind(query.created_until_unix_secs as i64)
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?
        };

        let total_requests = sqlite_aggregate_u64(&row, "total_requests")?;
        if total_requests == 0 {
            return Ok(None);
        }

        Ok(Some(StoredUsageDashboardSummary {
            total_requests,
            input_tokens: sqlite_aggregate_u64(&row, "input_tokens")?,
            effective_input_tokens: sqlite_aggregate_u64(&row, "effective_input_tokens")?,
            output_tokens: sqlite_aggregate_u64(&row, "output_tokens")?,
            total_tokens: sqlite_aggregate_u64(&row, "total_tokens")?,
            cache_creation_tokens: sqlite_aggregate_u64(&row, "cache_creation_tokens")?,
            cache_read_tokens: sqlite_aggregate_u64(&row, "cache_read_tokens")?,
            total_input_context: sqlite_aggregate_u64(&row, "total_input_context")?,
            cache_creation_cost_usd: sqlite_real(&row, "cache_creation_cost_usd")?,
            cache_read_cost_usd: sqlite_real(&row, "cache_read_cost_usd")?,
            total_cost_usd: sqlite_real(&row, "total_cost_usd")?,
            actual_total_cost_usd: sqlite_real(&row, "actual_total_cost_usd")?,
            error_requests: sqlite_aggregate_u64(&row, "error_requests")?,
            response_time_sum_ms: sqlite_real(&row, "response_time_sum_ms")?,
            response_time_samples: sqlite_aggregate_u64(&row, "response_time_samples")?,
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
  date("date", 'unixepoch') AS date,
  'aggregate' AS model,
  'aggregate' AS provider,
  COALESCE(SUM(total_requests), 0) AS requests,
  COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost AS REAL), 0)), 0) AS total_cost_usd,
  0.0 AS response_time_sum_ms,
  0 AS response_time_samples
FROM stats_user_daily
WHERE user_id = ?
  AND "date" >= ?
  AND "date" < ?
  AND total_requests > 0
GROUP BY "date"
ORDER BY "date" ASC
"#,
            )
            .bind(user_id)
            .bind(query.created_from_unix_secs as i64)
            .bind(query.created_until_unix_secs as i64)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  date("date", 'unixepoch') AS date,
  'aggregate' AS model,
  'aggregate' AS provider,
  COALESCE(SUM(total_requests), 0) AS requests,
  COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost AS REAL), 0)), 0) AS total_cost_usd,
  0.0 AS response_time_sum_ms,
  0 AS response_time_samples
FROM stats_daily
WHERE "date" >= ?
  AND "date" < ?
  AND total_requests > 0
GROUP BY "date"
ORDER BY "date" ASC
"#,
            )
            .bind(query.created_from_unix_secs as i64)
            .bind(query.created_until_unix_secs as i64)
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
                    requests: sqlite_aggregate_u64(row, "requests")?,
                    total_tokens: sqlite_aggregate_u64(row, "total_tokens")?,
                    total_cost_usd: sqlite_real(row, "total_cost_usd")?,
                    response_time_sum_ms: sqlite_real(row, "response_time_sum_ms")?,
                    response_time_samples: sqlite_aggregate_u64(row, "response_time_samples")?,
                })
            })
            .collect()
    }
}

#[async_trait]
impl UsageReadRepository for SqliteUsageReadRepository {
    async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(&format!("{USAGE_COLUMNS} WHERE id = ? LIMIT 1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_usage_row).transpose()
    }

    async fn list_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(USAGE_COLUMNS);
        builder.push(" WHERE id IN (");
        {
            let mut separated = builder.separated(", ");
            for id in ids {
                separated.push_bind(id);
            }
        }
        builder.push(") ORDER BY created_at_unix_ms DESC, id ASC");
        self.fetch_usage_items(builder).await
    }

    async fn find_by_request_id(
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

    async fn resolve_body_ref(
        &self,
        body_ref: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let has_blob_table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'usage_body_blobs'",
        )
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        if has_blob_table > 0 {
            let blob_row =
                sqlx::query("SELECT payload_gzip FROM usage_body_blobs WHERE body_ref = ? LIMIT 1")
                    .bind(body_ref)
                    .fetch_optional(&self.pool)
                    .await
                    .map_sql_err()?;
            if let Some(row) = blob_row.as_ref() {
                let payload_gzip = row.try_get::<Vec<u8>, _>("payload_gzip").map_sql_err()?;
                return inflate_usage_json_value(&payload_gzip).map(Some);
            }
        }

        let Some((request_id, field)) = parse_usage_body_ref(body_ref) else {
            return Ok(None);
        };
        let (inline_column, compressed_column) = sqlite_usage_body_sql_columns(field);
        let row = sqlx::query(&format!(
            "SELECT {inline_column} AS inline_body, {compressed_column} AS compressed_body FROM \"usage\" WHERE request_id = ? LIMIT 1"
        ))
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        let Some(row) = row.as_ref() else {
            return Ok(None);
        };
        if let Some(raw) = row
            .try_get::<Option<String>, _>("inline_body")
            .map_sql_err()?
        {
            return parse_usage_json_text(&raw).map(Some);
        }
        row.try_get::<Option<Vec<u8>>, _>("compressed_body")
            .map_sql_err()?
            .map(|bytes| inflate_usage_json_value(&bytes))
            .transpose()
    }

    async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_usage_audits(self, query).await
    }

    async fn count_usage_audits(&self, query: &UsageAuditListQuery) -> Result<u64, DataLayerError> {
        Self::count_usage_audits(self, query).await
    }

    async fn list_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_usage_audits_by_keyword_search(self, query).await
    }

    async fn count_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<u64, DataLayerError> {
        Self::count_usage_audits_by_keyword_search(self, query).await
    }

    async fn aggregate_usage_audits(
        &self,
        query: &UsageAuditAggregationQuery,
    ) -> Result<Vec<StoredUsageAuditAggregation>, DataLayerError> {
        Self::aggregate_usage_audits(self, query).await
    }

    async fn summarize_usage_audits(
        &self,
        query: &UsageAuditSummaryQuery,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        Self::summarize_usage_audits(self, query).await
    }

    async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUsageUserTotals>, DataLayerError> {
        Self::summarize_usage_totals_by_user_ids(self, user_ids).await
    }

    async fn summarize_usage_cache_hit_summary(
        &self,
        query: &UsageCacheHitSummaryQuery,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(StoredUsageCacheHitSummary::default());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  COUNT(*) AS total_requests,
  COALESCE(SUM(CASE WHEN COALESCE(cache_read_input_tokens, 0) > 0 THEN 1 ELSE 0 END), 0)
    AS cache_hit_requests
FROM "usage"
"#,
        );
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(StoredUsageCacheHitSummary {
            total_requests: sqlite_aggregate_u64(&row, "total_requests")?,
            cache_hit_requests: sqlite_aggregate_u64(&row, "cache_hit_requests")?,
        })
    }

    async fn summarize_usage_settled_cost(
        &self,
        query: &UsageSettledCostSummaryQuery,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(StoredUsageSettledCostSummary::default());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  COALESCE(SUM(CAST(total_cost_usd AS REAL)), 0) AS total_cost_usd,
  COUNT(*) AS total_requests,
  COALESCE(SUM(MAX(COALESCE(input_tokens, 0), 0)), 0) AS input_tokens,
  COALESCE(SUM(MAX(COALESCE(output_tokens, 0), 0)), 0) AS output_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_creation_input_tokens, 0), 0)), 0) AS cache_creation_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
  MIN(finalized_at) AS first_finalized_at_unix_secs,
  MAX(finalized_at) AS last_finalized_at_unix_secs
FROM "usage"
"#,
        );
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "api_key_id",
            query.api_key_id.as_deref(),
        );
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push("billing_status = 'settled' AND COALESCE(total_cost_usd, 0) > 0");
        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(StoredUsageSettledCostSummary {
            total_cost_usd: sqlite_real(&row, "total_cost_usd")?,
            total_requests: sqlite_aggregate_u64(&row, "total_requests")?,
            input_tokens: sqlite_aggregate_u64(&row, "input_tokens")?,
            output_tokens: sqlite_aggregate_u64(&row, "output_tokens")?,
            cache_creation_tokens: sqlite_aggregate_u64(&row, "cache_creation_tokens")?,
            cache_read_tokens: sqlite_aggregate_u64(&row, "cache_read_tokens")?,
            first_finalized_at_unix_secs: row
                .try_get::<Option<i64>, _>("first_finalized_at_unix_secs")
                .map_sql_err()?
                .map(|value| value.max(0) as u64),
            last_finalized_at_unix_secs: row
                .try_get::<Option<i64>, _>("last_finalized_at_unix_secs")
                .map_sql_err()?
                .map(|value| value.max(0) as u64),
        })
    }

    async fn summarize_usage_cache_affinity_hit_summary(
        &self,
        query: &UsageCacheAffinityHitSummaryQuery,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(StoredUsageCacheAffinityHitSummary::default());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  COUNT(*) AS total_requests,
  COALESCE(SUM(CASE WHEN COALESCE(cache_read_input_tokens, 0) > 0 THEN 1 ELSE 0 END), 0)
    AS requests_with_cache_hit,
  COALESCE(SUM(MAX(COALESCE(input_tokens, 0), 0)), 0) AS input_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
  COALESCE(SUM({cache_creation_expr}), 0) AS cache_creation_tokens,
  COALESCE(SUM({total_input_context_expr}), 0) AS total_input_context,
  COALESCE(SUM(COALESCE(CAST(cache_read_cost_usd AS REAL), 0)), 0) AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(CAST(cache_creation_cost_usd AS REAL), 0)), 0)
    AS cache_creation_cost_usd
FROM "usage"
"#,
            cache_creation_expr = SQLITE_USAGE_CACHE_CREATION_TOKENS_EXPR,
            total_input_context_expr = SQLITE_USAGE_TOTAL_INPUT_CONTEXT_EXPR
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push("status = 'completed'");
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "api_key_id",
            query.api_key_id.as_deref(),
        );

        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(StoredUsageCacheAffinityHitSummary {
            total_requests: sqlite_aggregate_u64(&row, "total_requests")?,
            requests_with_cache_hit: sqlite_aggregate_u64(&row, "requests_with_cache_hit")?,
            input_tokens: sqlite_aggregate_u64(&row, "input_tokens")?,
            cache_read_tokens: sqlite_aggregate_u64(&row, "cache_read_tokens")?,
            cache_creation_tokens: sqlite_aggregate_u64(&row, "cache_creation_tokens")?,
            total_input_context: sqlite_aggregate_u64(&row, "total_input_context")?,
            cache_read_cost_usd: sqlite_real(&row, "cache_read_cost_usd")?,
            cache_creation_cost_usd: sqlite_real(&row, "cache_creation_cost_usd")?,
        })
    }

    async fn list_usage_cache_affinity_intervals(
        &self,
        query: &UsageCacheAffinityIntervalQuery,
    ) -> Result<Vec<StoredUsageCacheAffinityIntervalRow>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }

        let group_expr = match query.group_by {
            UsageCacheAffinityIntervalGroupBy::User => "user_id",
            UsageCacheAffinityIntervalGroupBy::ApiKey => "api_key_id",
        };
        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
WITH filtered_usage AS (
  SELECT
    {group_expr} AS group_id,
    NULL AS username,
    model,
    created_at_unix_ms,
    id,
    LAG(created_at_unix_ms) OVER (
      PARTITION BY {group_expr}
      ORDER BY created_at_unix_ms ASC, id ASC
    ) AS previous_created_at_unix_secs
  FROM "usage"
"#
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "api_key_id",
            query.api_key_id.as_deref(),
        );
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push("status = 'completed'");
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder
            .push(group_expr)
            .push(" IS NOT NULL AND TRIM(")
            .push(group_expr)
            .push(") <> ''");
        builder.push(
            r#"
)
SELECT
  group_id,
  username,
  model,
  created_at_unix_ms AS created_at_unix_secs,
  (created_at_unix_ms - previous_created_at_unix_secs) * 1.0 / 60.0 AS interval_minutes
FROM filtered_usage
WHERE previous_created_at_unix_secs IS NOT NULL
ORDER BY created_at_unix_ms ASC, id ASC
"#,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredUsageCacheAffinityIntervalRow {
                    group_id: row.try_get("group_id").map_sql_err()?,
                    username: row.try_get("username").map_sql_err()?,
                    model: row.try_get("model").map_sql_err()?,
                    created_at_unix_secs: sqlite_aggregate_u64(row, "created_at_unix_secs")?,
                    interval_minutes: sqlite_real(row, "interval_minutes")?,
                })
            })
            .collect()
    }

    async fn summarize_dashboard_usage(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(StoredUsageDashboardSummary::default());
        }

        if let Some(summary) = self
            .summarize_dashboard_usage_from_daily_aggregates(query)
            .await?
        {
            return Ok(summary);
        }

        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  COUNT(*) AS total_requests,
  COALESCE(SUM(MAX(COALESCE(input_tokens, 0), 0)), 0) AS input_tokens,
  COALESCE(SUM({effective_input_expr}), 0) AS effective_input_tokens,
  COALESCE(SUM(MAX(COALESCE(output_tokens, 0), 0)), 0) AS output_tokens,
  COALESCE(SUM({total_tokens_expr}), 0) AS total_tokens,
  COALESCE(SUM({cache_creation_expr}), 0) AS cache_creation_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
  COALESCE(SUM({total_input_context_expr}), 0) AS total_input_context,
  COALESCE(SUM(COALESCE(CAST(cache_creation_cost_usd AS REAL), 0)), 0)
    AS cache_creation_cost_usd,
  COALESCE(SUM(COALESCE(CAST(cache_read_cost_usd AS REAL), 0)), 0)
    AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST(actual_total_cost_usd AS REAL), 0)), 0)
    AS actual_total_cost_usd,
  COALESCE(SUM(CASE WHEN COALESCE(status_code, 0) >= 400 OR status = 'failed' THEN 1 ELSE 0 END), 0)
    AS error_requests,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL THEN MAX(COALESCE(response_time_ms, 0), 0) ELSE 0 END), 0)
    AS response_time_sum_ms,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0)
    AS response_time_samples
FROM "usage"
"#,
            effective_input_expr = SQLITE_USAGE_EFFECTIVE_INPUT_TOKENS_EXPR,
            total_tokens_expr = SQLITE_USAGE_CANONICAL_TOTAL_TOKENS_EXPR,
            cache_creation_expr = SQLITE_USAGE_CACHE_CREATION_TOKENS_EXPR,
            total_input_context_expr = SQLITE_USAGE_TOTAL_INPUT_CONTEXT_EXPR
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_finalized_filter(&mut builder, &mut has_where);
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );

        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(StoredUsageDashboardSummary {
            total_requests: sqlite_aggregate_u64(&row, "total_requests")?,
            input_tokens: sqlite_aggregate_u64(&row, "input_tokens")?,
            effective_input_tokens: sqlite_aggregate_u64(&row, "effective_input_tokens")?,
            output_tokens: sqlite_aggregate_u64(&row, "output_tokens")?,
            total_tokens: sqlite_aggregate_u64(&row, "total_tokens")?,
            cache_creation_tokens: sqlite_aggregate_u64(&row, "cache_creation_tokens")?,
            cache_read_tokens: sqlite_aggregate_u64(&row, "cache_read_tokens")?,
            total_input_context: sqlite_aggregate_u64(&row, "total_input_context")?,
            cache_creation_cost_usd: sqlite_real(&row, "cache_creation_cost_usd")?,
            cache_read_cost_usd: sqlite_real(&row, "cache_read_cost_usd")?,
            total_cost_usd: sqlite_real(&row, "total_cost_usd")?,
            actual_total_cost_usd: sqlite_real(&row, "actual_total_cost_usd")?,
            error_requests: sqlite_aggregate_u64(&row, "error_requests")?,
            response_time_sum_ms: sqlite_real(&row, "response_time_sum_ms")?,
            response_time_samples: sqlite_aggregate_u64(&row, "response_time_samples")?,
        })
    }

    async fn list_dashboard_daily_breakdown(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }

        let aggregate_rows = self
            .list_dashboard_daily_breakdown_from_daily_aggregates(query)
            .await?;
        if !aggregate_rows.is_empty() {
            return Ok(aggregate_rows);
        }

        let date_expr = sqlite_usage_local_date_expr(query.tz_offset_minutes);
        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  {date_expr} AS date,
  model,
  provider_name AS provider,
  COUNT(*) AS requests,
  COALESCE(SUM(MAX(COALESCE(total_tokens, 0), 0)), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL THEN MAX(COALESCE(response_time_ms, 0), 0) ELSE 0 END), 0)
    AS response_time_sum_ms,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0)
    AS response_time_samples
FROM "usage"
"#
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_finalized_filter(&mut builder, &mut has_where);
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        builder.push(
            r#"
GROUP BY date, model, provider
ORDER BY date ASC, total_cost_usd DESC, model ASC, provider ASC
"#,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredUsageDashboardDailyBreakdownRow {
                    date: row.try_get("date").map_sql_err()?,
                    model: row.try_get("model").map_sql_err()?,
                    provider: row.try_get("provider").map_sql_err()?,
                    requests: sqlite_aggregate_u64(row, "requests")?,
                    total_tokens: sqlite_aggregate_u64(row, "total_tokens")?,
                    total_cost_usd: sqlite_real(row, "total_cost_usd")?,
                    response_time_sum_ms: sqlite_real(row, "response_time_sum_ms")?,
                    response_time_samples: sqlite_aggregate_u64(row, "response_time_samples")?,
                })
            })
            .collect()
    }

    async fn summarize_dashboard_provider_counts(
        &self,
        query: &UsageDashboardProviderCountsQuery,
    ) -> Result<Vec<StoredUsageDashboardProviderCount>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  provider_name,
  COUNT(*) AS request_count
FROM "usage"
"#,
        );
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_finalized_filter(&mut builder, &mut has_where);
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        builder.push(
            r#"
GROUP BY provider_name
ORDER BY request_count DESC, provider_name ASC
"#,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredUsageDashboardProviderCount {
                    provider_name: row.try_get("provider_name").map_sql_err()?,
                    request_count: sqlite_aggregate_u64(row, "request_count")?,
                })
            })
            .collect()
    }

    async fn summarize_usage_breakdown(
        &self,
        query: &UsageBreakdownSummaryQuery,
    ) -> Result<Vec<StoredUsageBreakdownSummaryRow>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }

        let group_expr = sqlite_usage_breakdown_group_expr(query.group_by);
        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  {group_expr} AS group_key,
  COUNT(*) AS request_count,
  COALESCE(SUM(MAX(COALESCE(input_tokens, 0), 0)), 0) AS input_tokens,
  COALESCE(SUM(MAX(COALESCE(total_tokens, 0), 0)), 0) AS total_tokens,
  COALESCE(SUM(MAX(COALESCE(output_tokens, 0), 0)), 0) AS output_tokens,
  COALESCE(SUM({effective_input_expr}), 0) AS effective_input_tokens,
  COALESCE(SUM({total_input_context_expr}), 0) AS total_input_context,
  COALESCE(SUM({cache_creation_expr}), 0) AS cache_creation_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_creation_ephemeral_5m_input_tokens, 0), 0)), 0)
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_creation_ephemeral_1h_input_tokens, 0), 0)), 0)
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST(actual_total_cost_usd AS REAL), 0)), 0)
    AS actual_total_cost_usd,
  COALESCE(SUM({success_flag_expr}), 0) AS success_count,
  COALESCE(SUM(CASE WHEN {success_flag_expr} = 1 AND response_time_ms IS NOT NULL THEN MAX(COALESCE(response_time_ms, 0), 0) ELSE 0 END), 0)
    AS response_time_sum_ms,
  COALESCE(SUM(CASE WHEN {success_flag_expr} = 1 AND response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0)
    AS response_time_samples,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL THEN MAX(COALESCE(response_time_ms, 0), 0) ELSE 0 END), 0)
    AS overall_response_time_sum_ms,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL THEN 1 ELSE 0 END), 0)
    AS overall_response_time_samples
FROM "usage"
"#,
            effective_input_expr = SQLITE_USAGE_EFFECTIVE_INPUT_TOKENS_EXPR,
            total_input_context_expr = SQLITE_USAGE_TOTAL_INPUT_CONTEXT_EXPR,
            cache_creation_expr = SQLITE_USAGE_CACHE_CREATION_TOKENS_EXPR,
            success_flag_expr = SQLITE_USAGE_SUCCESS_FLAG_EXPR
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_finalized_filter(&mut builder, &mut has_where);
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "provider_name",
            query.provider_name.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "model",
            query.model.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "api_format",
            query.api_format.as_deref(),
        );
        push_sqlite_usage_excluded_status_codes(
            &mut builder,
            &mut has_where,
            &query.exclude_status_codes,
        );
        if matches!(query.group_by, UsageBreakdownGroupBy::ApiFormat) {
            push_sqlite_usage_where(&mut builder, &mut has_where);
            builder.push("api_format IS NOT NULL");
        }
        builder.push(" GROUP BY group_key ORDER BY request_count DESC, group_key ASC");

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredUsageBreakdownSummaryRow {
                    group_key: row.try_get("group_key").map_sql_err()?,
                    request_count: sqlite_aggregate_u64(row, "request_count")?,
                    input_tokens: sqlite_aggregate_u64(row, "input_tokens")?,
                    total_tokens: sqlite_aggregate_u64(row, "total_tokens")?,
                    output_tokens: sqlite_aggregate_u64(row, "output_tokens")?,
                    effective_input_tokens: sqlite_aggregate_u64(row, "effective_input_tokens")?,
                    total_input_context: sqlite_aggregate_u64(row, "total_input_context")?,
                    cache_creation_tokens: sqlite_aggregate_u64(row, "cache_creation_tokens")?,
                    cache_creation_ephemeral_5m_tokens: sqlite_aggregate_u64(
                        row,
                        "cache_creation_ephemeral_5m_tokens",
                    )?,
                    cache_creation_ephemeral_1h_tokens: sqlite_aggregate_u64(
                        row,
                        "cache_creation_ephemeral_1h_tokens",
                    )?,
                    cache_read_tokens: sqlite_aggregate_u64(row, "cache_read_tokens")?,
                    total_cost_usd: sqlite_real(row, "total_cost_usd")?,
                    actual_total_cost_usd: sqlite_real(row, "actual_total_cost_usd")?,
                    success_count: sqlite_aggregate_u64(row, "success_count")?,
                    response_time_sum_ms: sqlite_real(row, "response_time_sum_ms")?,
                    response_time_samples: sqlite_aggregate_u64(row, "response_time_samples")?,
                    overall_response_time_sum_ms: sqlite_real(row, "overall_response_time_sum_ms")?,
                    overall_response_time_samples: sqlite_aggregate_u64(
                        row,
                        "overall_response_time_samples",
                    )?,
                })
            })
            .collect()
    }

    async fn count_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorCountQuery,
    ) -> Result<u64, DataLayerError> {
        let row = sqlx::query(&format!(
            r#"
SELECT COUNT(*) AS total
FROM "usage"
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND {error_predicate}
"#,
            error_predicate = SQLITE_MONITORING_ERROR_PREDICATE
        ))
        .bind(query.created_from_unix_secs as i64)
        .bind(query.created_until_unix_secs as i64)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        sqlite_aggregate_u64(&row, "total")
    }

    async fn list_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(USAGE_COLUMNS);
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push(SQLITE_MONITORING_ERROR_PREDICATE);
        builder.push(" ORDER BY created_at_unix_ms DESC, id ASC");
        if let Some(limit) = query.limit {
            builder.push(" LIMIT ").push_bind(limit as i64);
        }
        self.fetch_usage_items(builder).await
    }

    async fn summarize_usage_error_distribution(
        &self,
        query: &UsageErrorDistributionQuery,
    ) -> Result<Vec<StoredUsageErrorDistributionRow>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }

        let date_expr = sqlite_usage_local_date_expr(query.tz_offset_minutes);
        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  {date_expr} AS date,
  error_category,
  COUNT(*) AS count
FROM "usage"
"#
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push("error_category IS NOT NULL AND TRIM(error_category) <> ''");
        builder.push(
            r#"
GROUP BY date, error_category
ORDER BY date ASC, count DESC, error_category ASC
"#,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredUsageErrorDistributionRow {
                    date: row.try_get("date").map_sql_err()?,
                    error_category: row.try_get("error_category").map_sql_err()?,
                    count: sqlite_aggregate_u64(row, "count")?,
                })
            })
            .collect()
    }

    async fn summarize_usage_performance_percentiles(
        &self,
        query: &UsagePerformancePercentilesQuery,
    ) -> Result<Vec<StoredUsagePerformancePercentilesRow>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }

        let date_expr = sqlite_usage_local_date_expr(query.tz_offset_minutes);
        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
WITH filtered_usage AS (
  SELECT
    {date_expr} AS date,
    response_time_ms,
    first_byte_time_ms
  FROM "usage"
"#
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push("status = 'completed'");
        builder.push(
            r#"
),
response_ranked AS (
  SELECT
    date,
    response_time_ms AS value,
    ROW_NUMBER() OVER (PARTITION BY date ORDER BY response_time_ms) AS rn,
    COUNT(response_time_ms) OVER (PARTITION BY date) AS n
  FROM filtered_usage
  WHERE response_time_ms IS NOT NULL
),
first_byte_ranked AS (
  SELECT
    date,
    first_byte_time_ms AS value,
    ROW_NUMBER() OVER (PARTITION BY date ORDER BY first_byte_time_ms) AS rn,
    COUNT(first_byte_time_ms) OVER (PARTITION BY date) AS n
  FROM filtered_usage
  WHERE first_byte_time_ms IS NOT NULL
),
response_positions AS (
  SELECT
    date,
    n,
    0.5 * (n - 1) AS p50_pos,
    0.9 * (n - 1) AS p90_pos,
    0.99 * (n - 1) AS p99_pos
  FROM response_ranked
  GROUP BY date, n
),
first_byte_positions AS (
  SELECT
    date,
    n,
    0.5 * (n - 1) AS p50_pos,
    0.9 * (n - 1) AS p90_pos,
    0.99 * (n - 1) AS p99_pos
  FROM first_byte_ranked
  GROUP BY date, n
),
response_percentiles AS (
  SELECT
    positions.date,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p50_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p50_pos = CAST(positions.p50_pos AS INTEGER)
          THEN CAST(positions.p50_pos AS INTEGER) + 1
          ELSE CAST(positions.p50_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p50_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p50_pos - CAST(positions.p50_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p50_response_time_ms,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p90_pos = CAST(positions.p90_pos AS INTEGER)
          THEN CAST(positions.p90_pos AS INTEGER) + 1
          ELSE CAST(positions.p90_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p90_pos - CAST(positions.p90_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p90_response_time_ms,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p99_pos = CAST(positions.p99_pos AS INTEGER)
          THEN CAST(positions.p99_pos AS INTEGER) + 1
          ELSE CAST(positions.p99_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p99_pos - CAST(positions.p99_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p99_response_time_ms
  FROM response_positions AS positions
  JOIN response_ranked AS ranked ON ranked.date = positions.date
  GROUP BY positions.date, positions.n, positions.p50_pos, positions.p90_pos, positions.p99_pos
),
first_byte_percentiles AS (
  SELECT
    positions.date,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p50_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p50_pos = CAST(positions.p50_pos AS INTEGER)
          THEN CAST(positions.p50_pos AS INTEGER) + 1
          ELSE CAST(positions.p50_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p50_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p50_pos - CAST(positions.p50_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p50_first_byte_time_ms,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p90_pos = CAST(positions.p90_pos AS INTEGER)
          THEN CAST(positions.p90_pos AS INTEGER) + 1
          ELSE CAST(positions.p90_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p90_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p90_pos - CAST(positions.p90_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p90_first_byte_time_ms,
    CASE WHEN positions.n >= 10 THEN CAST((
      MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      + (
        MAX(CASE WHEN ranked.rn = CASE WHEN positions.p99_pos = CAST(positions.p99_pos AS INTEGER)
          THEN CAST(positions.p99_pos AS INTEGER) + 1
          ELSE CAST(positions.p99_pos AS INTEGER) + 2
        END THEN ranked.value END)
        - MAX(CASE WHEN ranked.rn = CAST(positions.p99_pos AS INTEGER) + 1 THEN ranked.value END)
      ) * (positions.p99_pos - CAST(positions.p99_pos AS INTEGER))
    ) AS INTEGER) ELSE NULL END AS p99_first_byte_time_ms
  FROM first_byte_positions AS positions
  JOIN first_byte_ranked AS ranked ON ranked.date = positions.date
  GROUP BY positions.date, positions.n, positions.p50_pos, positions.p90_pos, positions.p99_pos
),
dates AS (
  SELECT date FROM response_percentiles
  UNION
  SELECT date FROM first_byte_percentiles
)
SELECT
  dates.date,
  response_percentiles.p50_response_time_ms,
  response_percentiles.p90_response_time_ms,
  response_percentiles.p99_response_time_ms,
  first_byte_percentiles.p50_first_byte_time_ms,
  first_byte_percentiles.p90_first_byte_time_ms,
  first_byte_percentiles.p99_first_byte_time_ms
FROM dates
LEFT JOIN response_percentiles ON response_percentiles.date = dates.date
LEFT JOIN first_byte_percentiles ON first_byte_percentiles.date = dates.date
ORDER BY dates.date ASC
"#,
        );

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredUsagePerformancePercentilesRow {
                    date: row.try_get("date").map_sql_err()?,
                    p50_response_time_ms: row
                        .try_get::<Option<i64>, _>("p50_response_time_ms")
                        .map_sql_err()?
                        .map(|value| value.max(0) as u64),
                    p90_response_time_ms: row
                        .try_get::<Option<i64>, _>("p90_response_time_ms")
                        .map_sql_err()?
                        .map(|value| value.max(0) as u64),
                    p99_response_time_ms: row
                        .try_get::<Option<i64>, _>("p99_response_time_ms")
                        .map_sql_err()?
                        .map(|value| value.max(0) as u64),
                    p50_first_byte_time_ms: row
                        .try_get::<Option<i64>, _>("p50_first_byte_time_ms")
                        .map_sql_err()?
                        .map(|value| value.max(0) as u64),
                    p90_first_byte_time_ms: row
                        .try_get::<Option<i64>, _>("p90_first_byte_time_ms")
                        .map_sql_err()?
                        .map(|value| value.max(0) as u64),
                    p99_first_byte_time_ms: row
                        .try_get::<Option<i64>, _>("p99_first_byte_time_ms")
                        .map_sql_err()?
                        .map(|value| value.max(0) as u64),
                })
            })
            .collect()
    }

    async fn summarize_usage_provider_performance(
        &self,
        query: &UsageProviderPerformanceQuery,
    ) -> Result<StoredUsageProviderPerformance, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(StoredUsageProviderPerformance::default());
        }

        let summary_sql = format!(
            r#"
SELECT
  COUNT(*) AS request_count,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
    THEN 1 ELSE 0 END), 0) AS success_count,
  CASE
    WHEN COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(response_time_ms, 0), 0)
      ELSE 0
    END), 0) > 0
    THEN COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(output_tokens, 0), 0)
      ELSE 0
    END), 0) * 1000.0 / COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(response_time_ms, 0), 0)
      ELSE 0
    END), 0)
    ELSE NULL
  END AS avg_output_tps,
  AVG(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND first_byte_time_ms IS NOT NULL
    THEN MAX(COALESCE(first_byte_time_ms, 0), 0)
    ELSE NULL
  END) AS avg_first_byte_time_ms,
  AVG(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND response_time_ms IS NOT NULL
    THEN MAX(COALESCE(response_time_ms, 0), 0)
    ELSE NULL
  END) AS avg_response_time_ms,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND COALESCE(response_time_ms, 0) > 0
         AND COALESCE(output_tokens, 0) > 0
    THEN 1 ELSE 0 END), 0) AS tps_sample_count,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND response_time_ms IS NOT NULL
    THEN 1 ELSE 0 END), 0) AS response_time_sample_count,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND first_byte_time_ms IS NOT NULL
    THEN 1 ELSE 0 END), 0) AS first_byte_sample_count,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL AND response_time_ms >= {slow_threshold} THEN 1 ELSE 0 END), 0)
    AS slow_request_count
FROM "usage"
"#,
            slow_threshold = query.slow_threshold_ms
        );
        let mut summary_builder = QueryBuilder::<Sqlite>::new(summary_sql);
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut summary_builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_where(&mut summary_builder, &mut has_where);
        summary_builder.push(
            "COALESCE(status, '') NOT IN ('pending', 'streaming') \
AND provider_id IS NOT NULL AND TRIM(provider_id) <> '' \
AND LOWER(TRIM(provider_id)) NOT IN ('unknown', 'pending') \
AND LOWER(TRIM(COALESCE(provider_name, ''))) NOT IN ('unknown', 'pending')",
        );
        push_sqlite_usage_provider_performance_filters(&mut summary_builder, query, &mut has_where);
        let summary_row = summary_builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?;
        let summary_percentiles = self
            .summarize_provider_performance_percentiles(query)
            .await?;
        let summary = StoredUsageProviderPerformanceSummary {
            request_count: sqlite_aggregate_u64(&summary_row, "request_count")?,
            success_count: sqlite_aggregate_u64(&summary_row, "success_count")?,
            avg_output_tps: sqlite_optional_real(&summary_row, "avg_output_tps")?,
            avg_first_byte_time_ms: sqlite_optional_real(&summary_row, "avg_first_byte_time_ms")?,
            avg_response_time_ms: sqlite_optional_real(&summary_row, "avg_response_time_ms")?,
            p90_response_time_ms: summary_percentiles.p90_response_time_ms,
            p99_response_time_ms: summary_percentiles.p99_response_time_ms,
            p90_first_byte_time_ms: summary_percentiles.p90_first_byte_time_ms,
            p99_first_byte_time_ms: summary_percentiles.p99_first_byte_time_ms,
            tps_sample_count: sqlite_aggregate_u64(&summary_row, "tps_sample_count")?,
            response_time_sample_count: sqlite_aggregate_u64(
                &summary_row,
                "response_time_sample_count",
            )?,
            first_byte_sample_count: sqlite_aggregate_u64(&summary_row, "first_byte_sample_count")?,
            slow_request_count: sqlite_aggregate_u64(&summary_row, "slow_request_count")?,
        };

        let provider_sql = format!(
            r#"
SELECT
  provider_id,
  COALESCE(MAX(NULLIF(TRIM(provider_name), '')), provider_id) AS provider,
  COUNT(*) AS request_count,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
    THEN 1 ELSE 0 END), 0) AS success_count,
  COALESCE(SUM(MAX(COALESCE(output_tokens, 0), 0)), 0) AS output_tokens,
  CASE
    WHEN COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(response_time_ms, 0), 0)
      ELSE 0
    END), 0) > 0
    THEN COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(output_tokens, 0), 0)
      ELSE 0
    END), 0) * 1000.0 / COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(response_time_ms, 0), 0)
      ELSE 0
    END), 0)
    ELSE NULL
  END AS avg_output_tps,
  AVG(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND first_byte_time_ms IS NOT NULL
    THEN MAX(COALESCE(first_byte_time_ms, 0), 0)
    ELSE NULL
  END) AS avg_first_byte_time_ms,
  AVG(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND response_time_ms IS NOT NULL
    THEN MAX(COALESCE(response_time_ms, 0), 0)
    ELSE NULL
  END) AS avg_response_time_ms,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND COALESCE(response_time_ms, 0) > 0
         AND COALESCE(output_tokens, 0) > 0
    THEN 1 ELSE 0 END), 0) AS tps_sample_count,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND response_time_ms IS NOT NULL
    THEN 1 ELSE 0 END), 0) AS response_time_sample_count,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND first_byte_time_ms IS NOT NULL
    THEN 1 ELSE 0 END), 0) AS first_byte_sample_count,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL AND response_time_ms >= {slow_threshold} THEN 1 ELSE 0 END), 0)
    AS slow_request_count
FROM "usage"
"#,
            slow_threshold = query.slow_threshold_ms
        );
        let mut provider_builder = QueryBuilder::<Sqlite>::new(provider_sql);
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut provider_builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_where(&mut provider_builder, &mut has_where);
        provider_builder.push(
            "COALESCE(status, '') NOT IN ('pending', 'streaming') \
AND provider_id IS NOT NULL AND TRIM(provider_id) <> '' \
AND LOWER(TRIM(provider_id)) NOT IN ('unknown', 'pending') \
AND LOWER(TRIM(COALESCE(provider_name, ''))) NOT IN ('unknown', 'pending')",
        );
        push_sqlite_usage_provider_performance_filters(
            &mut provider_builder,
            query,
            &mut has_where,
        );
        provider_builder
            .push(" GROUP BY provider_id ORDER BY request_count DESC, provider_id ASC LIMIT ");
        provider_builder.push_bind(query.limit.max(1) as i64);
        let provider_rows = provider_builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        let provider_ids = provider_rows
            .iter()
            .map(|row| row.try_get::<String, _>("provider_id").map_sql_err())
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        let provider_percentiles = self
            .summarize_provider_performance_provider_percentiles(query, &provider_ids)
            .await?;
        let providers = provider_rows
            .iter()
            .map(|row| {
                let provider_id = row.try_get::<String, _>("provider_id").map_sql_err()?;
                let percentiles = provider_percentiles
                    .get(&provider_id)
                    .copied()
                    .unwrap_or_default();
                Ok(StoredUsageProviderPerformanceProviderRow {
                    provider_id,
                    provider: row.try_get("provider").map_sql_err()?,
                    request_count: sqlite_aggregate_u64(row, "request_count")?,
                    success_count: sqlite_aggregate_u64(row, "success_count")?,
                    output_tokens: sqlite_aggregate_u64(row, "output_tokens")?,
                    avg_output_tps: sqlite_optional_real(row, "avg_output_tps")?,
                    avg_first_byte_time_ms: sqlite_optional_real(row, "avg_first_byte_time_ms")?,
                    avg_response_time_ms: sqlite_optional_real(row, "avg_response_time_ms")?,
                    p90_response_time_ms: percentiles.p90_response_time_ms,
                    p99_response_time_ms: percentiles.p99_response_time_ms,
                    p90_first_byte_time_ms: percentiles.p90_first_byte_time_ms,
                    p99_first_byte_time_ms: percentiles.p99_first_byte_time_ms,
                    tps_sample_count: sqlite_aggregate_u64(row, "tps_sample_count")?,
                    response_time_sample_count: sqlite_aggregate_u64(
                        row,
                        "response_time_sample_count",
                    )?,
                    first_byte_sample_count: sqlite_aggregate_u64(row, "first_byte_sample_count")?,
                    slow_request_count: sqlite_aggregate_u64(row, "slow_request_count")?,
                })
            })
            .collect::<Result<Vec<_>, DataLayerError>>()?;
        let timeline = if provider_ids.is_empty() {
            Vec::new()
        } else {
            let bucket_expr = sqlite_usage_bucket_expr(query.granularity, query.tz_offset_minutes);
            let mut timeline_builder = QueryBuilder::<Sqlite>::new(format!(
                r#"
SELECT
  {bucket_expr} AS date,
  provider_id,
  COALESCE(MAX(NULLIF(TRIM(provider_name), '')), provider_id) AS provider,
  COUNT(*) AS request_count,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
    THEN 1 ELSE 0 END), 0) AS success_count,
  COALESCE(SUM(MAX(COALESCE(output_tokens, 0), 0)), 0) AS output_tokens,
  CASE
    WHEN COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(response_time_ms, 0), 0)
      ELSE 0
    END), 0) > 0
    THEN COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(output_tokens, 0), 0)
      ELSE 0
    END), 0) * 1000.0 / COALESCE(SUM(CASE
      WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
           AND COALESCE(response_time_ms, 0) > 0
           AND COALESCE(output_tokens, 0) > 0
      THEN MAX(COALESCE(response_time_ms, 0), 0)
      ELSE 0
    END), 0)
    ELSE NULL
  END AS avg_output_tps,
  AVG(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND first_byte_time_ms IS NOT NULL
    THEN MAX(COALESCE(first_byte_time_ms, 0), 0)
    ELSE NULL
  END) AS avg_first_byte_time_ms,
  AVG(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
         AND response_time_ms IS NOT NULL
    THEN MAX(COALESCE(response_time_ms, 0), 0)
    ELSE NULL
  END) AS avg_response_time_ms,
  COALESCE(SUM(CASE WHEN response_time_ms IS NOT NULL AND response_time_ms >= {slow_threshold} THEN 1 ELSE 0 END), 0)
    AS slow_request_count
FROM "usage"
"#,
                slow_threshold = query.slow_threshold_ms
            ));
            let mut has_where = false;
            push_sqlite_usage_range(
                &mut timeline_builder,
                &mut has_where,
                query.created_from_unix_secs,
                query.created_until_unix_secs,
            );
            push_sqlite_usage_where(&mut timeline_builder, &mut has_where);
            timeline_builder.push(
                "COALESCE(status, '') NOT IN ('pending', 'streaming') \
AND provider_id IS NOT NULL AND TRIM(provider_id) <> '' \
AND LOWER(TRIM(provider_id)) NOT IN ('unknown', 'pending') \
AND LOWER(TRIM(COALESCE(provider_name, ''))) NOT IN ('unknown', 'pending')",
            );
            push_sqlite_usage_provider_performance_filters(
                &mut timeline_builder,
                query,
                &mut has_where,
            );
            push_sqlite_usage_where(&mut timeline_builder, &mut has_where);
            timeline_builder.push("provider_id IN (");
            {
                let mut separated = timeline_builder.separated(", ");
                for provider_id in &provider_ids {
                    separated.push_bind(provider_id.clone());
                }
            }
            timeline_builder
                .push(") GROUP BY date, provider_id ORDER BY date ASC, provider_id ASC");
            let rows = timeline_builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_sql_err()?;
            rows.iter()
                .map(|row| {
                    Ok(StoredUsageProviderPerformanceTimelineRow {
                        date: row.try_get("date").map_sql_err()?,
                        provider_id: row.try_get("provider_id").map_sql_err()?,
                        provider: row.try_get("provider").map_sql_err()?,
                        request_count: sqlite_aggregate_u64(row, "request_count")?,
                        success_count: sqlite_aggregate_u64(row, "success_count")?,
                        output_tokens: sqlite_aggregate_u64(row, "output_tokens")?,
                        avg_output_tps: sqlite_optional_real(row, "avg_output_tps")?,
                        avg_first_byte_time_ms: sqlite_optional_real(
                            row,
                            "avg_first_byte_time_ms",
                        )?,
                        avg_response_time_ms: sqlite_optional_real(row, "avg_response_time_ms")?,
                        slow_request_count: sqlite_aggregate_u64(row, "slow_request_count")?,
                    })
                })
                .collect::<Result<Vec<_>, DataLayerError>>()?
        };

        Ok(StoredUsageProviderPerformance {
            summary,
            providers,
            timeline,
        })
    }

    async fn summarize_usage_cost_savings(
        &self,
        query: &UsageCostSavingsSummaryQuery,
    ) -> Result<StoredUsageCostSavingsSummary, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(StoredUsageCostSavingsSummary::default());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  COALESCE(SUM(MAX(COALESCE(cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST(cache_read_cost_usd AS REAL), 0)), 0) AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(CAST(cache_creation_cost_usd AS REAL), 0)), 0)
    AS cache_creation_cost_usd,
  COALESCE(SUM(
    {input_price_expr}
    * MAX(COALESCE(cache_read_input_tokens, 0), 0) / 1000000.0
  ), 0) AS estimated_full_cost_usd
FROM "usage"
"#,
            input_price_expr = sqlite_usage_metadata_input_price_expr()
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "provider_name",
            query.provider_name.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "model",
            query.model.as_deref(),
        );

        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(StoredUsageCostSavingsSummary {
            cache_read_tokens: sqlite_aggregate_u64(&row, "cache_read_tokens")?,
            cache_read_cost_usd: sqlite_real(&row, "cache_read_cost_usd")?,
            cache_creation_cost_usd: sqlite_real(&row, "cache_creation_cost_usd")?,
            estimated_full_cost_usd: sqlite_real(&row, "estimated_full_cost_usd")?,
        })
    }

    async fn summarize_usage_time_series(
        &self,
        query: &UsageTimeSeriesQuery,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }

        let bucket_expr = sqlite_usage_bucket_expr(query.granularity, query.tz_offset_minutes);
        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  {bucket_expr} AS bucket_key,
  COUNT(*) AS total_requests,
  COALESCE(SUM(MAX(COALESCE(input_tokens, 0), 0)), 0) AS input_tokens,
  COALESCE(SUM(MAX(COALESCE(output_tokens, 0), 0)), 0) AS output_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_creation_input_tokens, 0), 0)), 0)
    AS cache_creation_tokens,
  COALESCE(SUM(MAX(COALESCE(cache_read_input_tokens, 0), 0)), 0) AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd,
  COALESCE(SUM(MAX(COALESCE(response_time_ms, 0), 0)), 0) AS total_response_time_ms
FROM "usage"
"#
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "provider_name",
            query.provider_name.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "model",
            query.model.as_deref(),
        );
        builder.push(" GROUP BY bucket_key ORDER BY bucket_key ASC");

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredUsageTimeSeriesBucket {
                    bucket_key: row.try_get("bucket_key").map_sql_err()?,
                    total_requests: sqlite_aggregate_u64(row, "total_requests")?,
                    input_tokens: sqlite_aggregate_u64(row, "input_tokens")?,
                    output_tokens: sqlite_aggregate_u64(row, "output_tokens")?,
                    cache_creation_tokens: sqlite_aggregate_u64(row, "cache_creation_tokens")?,
                    cache_read_tokens: sqlite_aggregate_u64(row, "cache_read_tokens")?,
                    total_cost_usd: sqlite_real(row, "total_cost_usd")?,
                    total_response_time_ms: sqlite_real(row, "total_response_time_ms")?,
                })
            })
            .collect()
    }

    async fn summarize_usage_leaderboard(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }

        let (group_key_expr, legacy_name_expr, extra_filter) =
            sqlite_usage_leaderboard_group_expr(query.group_by);
        let mut builder = QueryBuilder::<Sqlite>::new(format!(
            r#"
SELECT
  {group_key_expr} AS group_key,
  MAX({legacy_name_expr}) AS legacy_name,
  COUNT(*) AS request_count,
  COALESCE(SUM({total_tokens_expr}), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd
FROM "usage"
"#,
            total_tokens_expr = SQLITE_USAGE_CANONICAL_TOTAL_TOKENS_EXPR
        ));
        let mut has_where = false;
        push_sqlite_usage_range(
            &mut builder,
            &mut has_where,
            query.created_from_unix_secs,
            query.created_until_unix_secs,
        );
        push_sqlite_usage_finalized_filter(&mut builder, &mut has_where);
        push_sqlite_usage_where(&mut builder, &mut has_where);
        builder.push(extra_filter);
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "user_id",
            query.user_id.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "provider_name",
            query.provider_name.as_deref(),
        );
        push_sqlite_usage_optional_text_filter(
            &mut builder,
            &mut has_where,
            "model",
            query.model.as_deref(),
        );
        builder.push(" GROUP BY group_key ORDER BY group_key ASC");

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter()
            .map(|row| {
                Ok(StoredUsageLeaderboardSummary {
                    group_key: row.try_get("group_key").map_sql_err()?,
                    legacy_name: row.try_get("legacy_name").map_sql_err()?,
                    request_count: sqlite_aggregate_u64(row, "request_count")?,
                    total_tokens: sqlite_aggregate_u64(row, "total_tokens")?,
                    total_cost_usd: sqlite_real(row, "total_cost_usd")?,
                })
            })
            .collect()
    }

    async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(USAGE_COLUMNS);
        let mut has_where = false;
        push_sqlite_usage_optional_text_filter(&mut builder, &mut has_where, "user_id", user_id);
        builder.push(" ORDER BY created_at_unix_ms DESC, id ASC LIMIT ");
        builder.push_bind(limit as i64);
        self.fetch_usage_items(builder).await
    }

    async fn summarize_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<BTreeMap<String, u64>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(BTreeMap::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  api_key_id,
  COALESCE(SUM(MAX(COALESCE(total_tokens, 0), 0)), 0) AS total_tokens
FROM "usage"
WHERE api_key_id IN (
"#,
        );
        {
            let mut separated = builder.separated(", ");
            for api_key_id in api_key_ids {
                separated.push_bind(api_key_id.clone());
            }
        }
        builder.push(") GROUP BY api_key_id ORDER BY api_key_id ASC");

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let mut totals = BTreeMap::new();
        for row in rows {
            totals.insert(
                row.try_get("api_key_id").map_sql_err()?,
                sqlite_aggregate_u64(&row, "total_tokens")?,
            );
        }
        Ok(totals)
    }

    async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<BTreeMap<String, StoredProviderApiKeyUsageSummary>, DataLayerError> {
        if provider_api_key_ids.is_empty() {
            return Ok(BTreeMap::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  provider_api_key_id,
  COUNT(*) AS request_count,
  COALESCE(SUM(MAX(COALESCE(total_tokens, 0), 0)), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd,
  MAX(created_at_unix_ms) AS last_used_at_unix_secs
FROM "usage"
WHERE provider_api_key_id IN (
"#,
        );
        {
            let mut separated = builder.separated(", ");
            for provider_api_key_id in provider_api_key_ids {
                separated.push_bind(provider_api_key_id.clone());
            }
        }
        builder.push(") GROUP BY provider_api_key_id ORDER BY provider_api_key_id ASC");

        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        let mut summaries = BTreeMap::new();
        for row in rows {
            let provider_api_key_id: String = row.try_get("provider_api_key_id").map_sql_err()?;
            summaries.insert(
                provider_api_key_id.clone(),
                StoredProviderApiKeyUsageSummary {
                    provider_api_key_id,
                    request_count: sqlite_aggregate_u64(&row, "request_count")?,
                    total_tokens: sqlite_aggregate_u64(&row, "total_tokens")?,
                    total_cost_usd: sqlite_real(&row, "total_cost_usd")?,
                    last_used_at_unix_secs: row
                        .try_get::<Option<i64>, _>("last_used_at_unix_secs")
                        .map_sql_err()?
                        .map(|value| value.max(0) as u64),
                },
            );
        }
        Ok(summaries)
    }

    async fn summarize_usage_by_provider_api_key_windows(
        &self,
        requests: &[ProviderApiKeyWindowUsageRequest],
    ) -> Result<Vec<StoredProviderApiKeyWindowUsageSummary>, DataLayerError> {
        let mut summaries = Vec::with_capacity(requests.len());
        for request in requests {
            let provider_api_key_id = request.provider_api_key_id.trim();
            if provider_api_key_id.is_empty() {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage provider_api_key_id cannot be empty".to_string(),
                ));
            }
            let window_code = request.window_code.trim();
            if window_code.is_empty() {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage window_code cannot be empty".to_string(),
                ));
            }
            if request.start_unix_secs >= request.end_unix_secs {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage range must be non-empty".to_string(),
                ));
            }

            let row = sqlx::query(
                r#"
SELECT
  COUNT(*) AS request_count,
  COALESCE(SUM(MAX(COALESCE(total_tokens, 0), 0)), 0) AS total_tokens,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd
FROM "usage"
WHERE provider_api_key_id = ?
  AND created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
"#,
            )
            .bind(provider_api_key_id)
            .bind(request.start_unix_secs as i64)
            .bind(request.end_unix_secs as i64)
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?;

            summaries.push(StoredProviderApiKeyWindowUsageSummary {
                provider_api_key_id: provider_api_key_id.to_string(),
                window_code: window_code.to_string(),
                request_count: sqlite_aggregate_u64(&row, "request_count")?,
                total_tokens: sqlite_aggregate_u64(&row, "total_tokens")?,
                total_cost_usd: sqlite_real(&row, "total_cost_usd")?,
            });
        }
        Ok(summaries)
    }

    async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  COUNT(*) AS total_requests,
  COALESCE(SUM(CASE
    WHEN LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
         AND (status_code IS NULL OR status_code < 400)
    THEN 1 ELSE 0 END), 0) AS successful_requests,
  COALESCE(SUM(CASE
    WHEN status NOT IN ('pending', 'streaming')
         AND NOT (
           LOWER(COALESCE(status, '')) IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND (status_code IS NULL OR status_code < 400)
         )
    THEN 1 ELSE 0 END), 0) AS failed_requests,
  COALESCE(AVG(CASE WHEN response_time_ms IS NOT NULL THEN MAX(COALESCE(response_time_ms, 0), 0) ELSE NULL END), 0)
    AS avg_response_time_ms,
  COALESCE(SUM(COALESCE(CAST(total_cost_usd AS REAL), 0)), 0) AS total_cost_usd
FROM "usage"
WHERE provider_id = ?
  AND created_at_unix_ms >= ?
"#,
        )
        .bind(provider_id)
        .bind(since_unix_secs as i64)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;

        Ok(StoredProviderUsageSummary {
            total_requests: sqlite_aggregate_u64(&row, "total_requests")?,
            successful_requests: sqlite_aggregate_u64(&row, "successful_requests")?,
            failed_requests: sqlite_aggregate_u64(&row, "failed_requests")?,
            avg_response_time_ms: sqlite_real(&row, "avg_response_time_ms")?,
            total_cost_usd: sqlite_real(&row, "total_cost_usd")?,
        })
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
}

fn map_sqlite_usage_daily_summary(
    row: &SqliteRow,
) -> Result<StoredUsageDailySummary, DataLayerError> {
    Ok(StoredUsageDailySummary {
        date: row.try_get("date").map_sql_err()?,
        requests: sqlite_aggregate_u64(row, "requests")?,
        total_tokens: sqlite_aggregate_u64(row, "total_tokens")?,
        total_cost_usd: sqlite_real(row, "total_cost_usd")?,
        actual_total_cost_usd: sqlite_real(row, "actual_total_cost_usd")?,
    })
}

fn usage_current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

impl SqliteUsageWriteRepository {
    pub fn new(pool: SqlitePool) -> Self {
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
impl UsageWriteRepository for SqliteUsageWriteRepository {
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
    total_cost_usd = 0.0,
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
  COALESCE(SUM(total_tokens), 0) AS total_tokens,
  CAST(COALESCE(SUM(total_cost_usd), 0) AS REAL) AS total_cost_usd,
  MAX(updated_at_unix_secs) AS last_used_at
FROM "usage"
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
            .bind(sqlite_real(row, "total_cost_usd")?)
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
    total_cost_usd = 0.0,
    total_response_time_ms = 0,
    last_used_at = NULL
"#,
        )
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        let rows = sqlx::query(&format!(
            r#"
SELECT
  provider_api_key_id,
  COUNT(*) AS request_count,
  COALESCE(SUM({success_flag_expr}), 0) AS success_count,
  COALESCE(SUM({error_flag_expr}), 0) AS error_count,
  COALESCE(SUM(CASE
    WHEN status IN ('pending', 'streaming') THEN 0
    ELSE MAX(COALESCE(total_tokens, 0), 0)
  END), 0) AS total_tokens,
  COALESCE(SUM(CASE
    WHEN status IN ('pending', 'streaming') THEN 0
    ELSE COALESCE(CAST(total_cost_usd AS REAL), 0)
  END), 0) AS total_cost_usd,
  COALESCE(SUM(CASE
    WHEN {success_flag_expr} = 1 AND response_time_ms IS NOT NULL
    THEN MAX(COALESCE(response_time_ms, 0), 0)
    ELSE 0
  END), 0) AS total_response_time_ms,
  MAX(created_at_unix_ms) AS last_used_at
FROM "usage"
WHERE provider_api_key_id IS NOT NULL
  AND TRIM(provider_api_key_id) <> ''
GROUP BY provider_api_key_id
"#,
            success_flag_expr = SQLITE_PROVIDER_KEY_SUCCESS_FLAG_EXPR,
            error_flag_expr = SQLITE_PROVIDER_KEY_ERROR_FLAG_EXPR
        ))
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;

        for row in &rows {
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
            .bind(row.try_get::<i64, _>("request_count").map_sql_err()?)
            .bind(row.try_get::<i64, _>("success_count").map_sql_err()?)
            .bind(row.try_get::<i64, _>("error_count").map_sql_err()?)
            .bind(row.try_get::<i64, _>("total_tokens").map_sql_err()?)
            .bind(sqlite_real(row, "total_cost_usd")?)
            .bind(
                row.try_get::<i64, _>("total_response_time_ms")
                    .map_sql_err()?,
            )
            .bind(
                row.try_get::<Option<i64>, _>("last_used_at")
                    .map_sql_err()?,
            )
            .bind(
                row.try_get::<String, _>("provider_api_key_id")
                    .map_sql_err()?,
            )
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        }

        Ok(rows.len() as u64)
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
                completed_request_ids_sqlite(&mut tx, stale_rows.iter().map(|row| &row.request_id))
                    .await?;

            for row in stale_rows {
                if completed_request_ids.contains(&row.request_id) {
                    sqlx::query(
                        r#"
UPDATE "usage"
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
                    latest_failed_candidate_sqlite(&mut tx, &row.request_id).await?;
                let (status_code, error_message) = resolve_stale_pending_failure(
                    candidate_info.as_ref(),
                    &row.status,
                    timeout_minutes,
                );
                let status_code_i64 = i64::from(status_code);
                if row.billing_status == "pending" {
                    sqlx::query(
                        r#"
UPDATE "usage"
SET status = 'failed',
    status_code = ?,
    error_message = ?,
    billing_status = 'void',
    finalized_at = ?,
    total_cost_usd = 0.0,
    actual_total_cost_usd = 0.0
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
                    upsert_void_usage_settlement_snapshot_sqlite(
                        &mut tx,
                        &row.request_id,
                        now_unix_secs,
                    )
                    .await?;
                } else {
                    sqlx::query(
                        r#"
UPDATE "usage"
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

async fn completed_request_ids_sqlite<'a>(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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

fn candidate_row_is_completed(row: &SqliteRow) -> Result<bool, DataLayerError> {
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

async fn upsert_void_usage_settlement_snapshot_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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
ON CONFLICT (request_id)
DO UPDATE SET
  billing_status = excluded.billing_status,
  finalized_at = COALESCE(usage_settlement_snapshots.finalized_at, excluded.finalized_at),
  updated_at = excluded.updated_at
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

async fn latest_failed_candidate_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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
    mut query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    usage: &'q UpsertUsageRecord,
) -> Result<sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>, DataLayerError>
{
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
        .bind(i64::from(usage.has_format_conversion.unwrap_or(false)))
        .bind(i64::from(usage.is_stream.unwrap_or(false)))
        .bind(i64::from(usage_upstream_is_stream(usage)))
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

fn map_usage_row(row: &SqliteRow) -> Result<StoredRequestUsageAudit, DataLayerError> {
    let mut audit = StoredRequestUsageAudit::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("request_id").map_sql_err()?,
        row.try_get("user_id").map_sql_err()?,
        row.try_get("api_key_id").map_sql_err()?,
        row.try_get("username").map_sql_err()?,
        row.try_get("api_key_name").map_sql_err()?,
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
        row.try_get::<i64, _>("has_format_conversion")
            .map_sql_err()?
            != 0,
        row.try_get::<i64, _>("is_stream").map_sql_err()? != 0,
        row_i32(row, "input_tokens")?,
        row_i32(row, "output_tokens")?,
        row_i32(row, "total_tokens")?,
        sqlite_real(row, "total_cost_usd")?,
        sqlite_real(row, "actual_total_cost_usd")?,
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
    audit.cache_creation_cost_usd =
        sqlite_optional_real(row, "cache_creation_cost_usd")?.unwrap_or(0.0);
    audit.cache_read_cost_usd = sqlite_optional_real(row, "cache_read_cost_usd")?.unwrap_or(0.0);
    audit.output_price_per_1m = sqlite_optional_real(row, "output_price_per_1m")?;
    audit.request_metadata = row
        .try_get::<Option<String>, _>("request_metadata")
        .map_sql_err()?
        .map(|raw| serde_json::from_str(&raw))
        .transpose()
        .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
    audit.client_family = usage_request_metadata_client_family(audit.request_metadata.as_ref())
        .map(ToOwned::to_owned);
    let upstream_is_stream = row
        .try_get::<Option<i64>, _>("upstream_is_stream")
        .map_sql_err()?
        .map(|value| value != 0);
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

fn row_i32(row: &SqliteRow, field: &str) -> Result<i32, DataLayerError> {
    let value: i64 = row.try_get(field).map_sql_err()?;
    i32::try_from(value).map_err(|_| DataLayerError::UnexpectedValue(format!("{field} overflow")))
}

fn row_optional_i32(row: &SqliteRow, field: &str) -> Result<Option<i32>, DataLayerError> {
    row.try_get::<Option<i64>, _>(field)
        .map_sql_err()?
        .map(|value| {
            i32::try_from(value)
                .map_err(|_| DataLayerError::UnexpectedValue(format!("{field} overflow")))
        })
        .transpose()
}

fn row_u64(row: &SqliteRow, field: &str) -> Result<u64, DataLayerError> {
    let value: i64 = row.try_get(field).map_sql_err()?;
    u64::try_from(value).map_err(|_| DataLayerError::UnexpectedValue(format!("{field} negative")))
}

#[cfg(test)]
mod tests {
    use super::{SqliteUsageReadRepository, SqliteUsageWriteRepository};
    use crate::lifecycle::migrate::run_sqlite_migrations;
    use crate::repository::usage::{
        UpsertUsageRecord, UsageAuditListQuery, UsageDailyHeatmapQuery,
        UsageDashboardDailyBreakdownQuery, UsageDashboardSummaryQuery, UsageReadRepository,
        UsageWriteRepository,
    };

    #[tokio::test]
    async fn sqlite_usage_write_repository_upserts_and_rebuilds_stats() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_stats_targets(&pool).await;

        let repository = SqliteUsageWriteRepository::new(pool.clone());
        let record = repository
            .upsert(sample_usage("request-1", "completed", "pending", 1_000))
            .await
            .expect("usage should upsert");

        assert_eq!(record.request_id, "request-1");
        assert_eq!(record.api_key_id.as_deref(), Some("api-key-1"));
        assert_eq!(record.total_tokens, 7);
        assert_eq!(record.cache_read_input_tokens, 2);
        assert_eq!(
            record.request_metadata.as_ref().unwrap()["trace_id"],
            "trace-1"
        );
        assert_eq!(
            record.request_metadata.as_ref().unwrap()["upstream_is_stream"],
            true
        );
        let upstream_is_stream: Option<i64> =
            sqlx::query_scalar("SELECT upstream_is_stream FROM \"usage\" WHERE request_id = ?")
                .bind("request-1")
                .fetch_one(&pool)
                .await
                .expect("usage stream mode should load");
        assert_eq!(upstream_is_stream, Some(1));

        let loaded = repository
            .find_by_request_id("request-1")
            .await
            .expect("usage should load")
            .expect("usage should exist");
        assert_eq!(
            loaded.provider_api_key_id.as_deref(),
            Some("provider-key-1")
        );

        let stats = sqlx::query_as::<_, (i64, i64, f64, Option<i64>)>(
            "SELECT total_requests, total_tokens, total_cost_usd, last_used_at FROM api_keys WHERE id = 'api-key-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("api key stats should load");
        assert_eq!(stats, (1, 7, 0.5, Some(1_000)));

        let provider_stats = sqlx::query_as::<_, (i64, i64, i64, i64, f64, i64, Option<i64>)>(
            "SELECT request_count, success_count, error_count, total_tokens, total_cost_usd, total_response_time_ms, last_used_at FROM provider_api_keys WHERE id = 'provider-key-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("provider key stats should load");
        assert_eq!(provider_stats, (1, 1, 0, 7, 0.5, 42, Some(1_000)));
    }

    #[tokio::test]
    async fn sqlite_usage_write_repository_does_not_regress_void_usage() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_stats_targets(&pool).await;

        let repository = SqliteUsageWriteRepository::new(pool);
        repository
            .upsert(sample_usage("request-1", "failed", "void", 1_000))
            .await
            .expect("void usage should upsert");
        let existing = repository
            .upsert(sample_usage("request-1", "pending", "pending", 1_001))
            .await
            .expect("stale usage should be ignored");

        assert_eq!(existing.status, "failed");
        assert_eq!(existing.billing_status, "void");
        assert_eq!(existing.updated_at_unix_secs, 1_000);
    }

    #[tokio::test]
    async fn sqlite_usage_write_repository_does_not_regress_terminal_usage_from_late_streaming() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_stats_targets(&pool).await;

        let repository = SqliteUsageWriteRepository::new(pool);
        repository
            .upsert(sample_usage("request-1", "completed", "pending", 1_000))
            .await
            .expect("terminal usage should upsert");

        let mut late_streaming = sample_usage("request-1", "streaming", "pending", 1_001);
        late_streaming.input_tokens = Some(0);
        late_streaming.output_tokens = Some(0);
        late_streaming.total_tokens = Some(0);
        late_streaming.cache_read_input_tokens = Some(0);
        late_streaming.cache_read_cost_usd = Some(0.0);
        late_streaming.total_cost_usd = Some(0.0);
        late_streaming.actual_total_cost_usd = Some(0.0);
        late_streaming.response_time_ms = Some(9_999);
        late_streaming.first_byte_time_ms = Some(9_999);
        late_streaming.finalized_at_unix_secs = None;

        let current = repository
            .upsert(late_streaming)
            .await
            .expect("late streaming usage should not regress terminal usage");

        assert_eq!(current.status, "completed");
        assert_eq!(current.billing_status, "pending");
        assert_eq!(current.total_tokens, 7);
        assert_eq!(current.cache_read_input_tokens, 2);
        assert_eq!(current.total_cost_usd, 0.5);
        assert_eq!(current.actual_total_cost_usd, 0.4);
        assert_eq!(current.response_time_ms, Some(42));
        assert_eq!(current.first_byte_time_ms, Some(12));
        assert_eq!(current.finalized_at_unix_secs, Some(1_000));
        assert_eq!(current.updated_at_unix_secs, 1_000);
    }

    #[tokio::test]
    async fn sqlite_usage_write_repository_preserves_streaming_response_start_from_late_active() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_stats_targets(&pool).await;

        let repository = SqliteUsageWriteRepository::new(pool);
        repository
            .upsert(sample_usage(
                "request-late-active",
                "streaming",
                "pending",
                1_000,
            ))
            .await
            .expect("response-start usage should upsert");

        let mut late_active = sample_usage("request-late-active", "streaming", "pending", 1_001);
        late_active.status_code = None;
        late_active.response_time_ms = None;
        late_active.first_byte_time_ms = None;

        let current = repository
            .upsert(late_active)
            .await
            .expect("late active usage should not clear response-start fields");

        assert_eq!(current.status, "streaming");
        assert_eq!(current.status_code, Some(200));
        assert_eq!(current.response_time_ms, Some(42));
        assert_eq!(current.first_byte_time_ms, Some(12));
    }

    #[tokio::test]
    async fn sqlite_usage_write_repository_cleans_stale_pending_requests() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_stats_targets(&pool).await;

        let repository = SqliteUsageWriteRepository::new(pool.clone());
        repository
            .upsert(sample_usage("request-recovered", "streaming", "pending", 1))
            .await
            .expect("streaming usage should upsert");
        repository
            .upsert(sample_usage("request-failed", "pending", "pending", 1))
            .await
            .expect("pending usage should upsert");

        sqlx::query(
            r#"
INSERT INTO request_candidates (
  id,
  request_id,
  candidate_index,
  retry_index,
  status,
  is_cached,
  created_at
) VALUES
  ('candidate-recovered', 'request-recovered', 0, 0, 'streaming', 0, 1),
  ('candidate-failed', 'request-failed', 0, 0, 'pending', 0, 1)
"#,
        )
        .execute(&pool)
        .await
        .expect("request candidates should seed");

        let summary = repository
            .cleanup_stale_pending_requests(2, 10, 5, 1)
            .await
            .expect("cleanup should run");
        assert_eq!(summary.recovered, 1);
        assert_eq!(summary.failed, 1);

        let recovered = repository
            .find_by_request_id("request-recovered")
            .await
            .expect("recovered usage should load")
            .expect("recovered usage should exist");
        assert_eq!(recovered.status, "completed");
        assert_eq!(recovered.status_code, Some(200));

        let failed = repository
            .find_by_request_id("request-failed")
            .await
            .expect("failed usage should load")
            .expect("failed usage should exist");
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.status_code, Some(504));
        assert_eq!(failed.billing_status, "void");
        assert_eq!(failed.total_cost_usd, 0.0);
        assert_eq!(failed.finalized_at_unix_secs, Some(10));

        let candidate_statuses = sqlx::query_as::<_, (String, String, Option<i64>)>(
            r#"
SELECT request_id, status, finished_at
FROM request_candidates
ORDER BY request_id
"#,
        )
        .fetch_all(&pool)
        .await
        .expect("candidate statuses should load");
        assert_eq!(
            candidate_statuses,
            vec![
                (
                    "request-failed".to_string(),
                    "failed".to_string(),
                    Some(10_000)
                ),
                (
                    "request-recovered".to_string(),
                    "success".to_string(),
                    Some(10_000)
                ),
            ]
        );

        let snapshot = sqlx::query_as::<_, (String, Option<i64>)>(
            "SELECT billing_status, finalized_at FROM usage_settlement_snapshots WHERE request_id = 'request-failed'",
        )
        .fetch_one(&pool)
        .await
        .expect("void settlement snapshot should load");
        assert_eq!(snapshot, ("void".to_string(), Some(10)));
    }

    #[tokio::test]
    async fn sqlite_usage_write_repository_cleanup_uses_failed_candidate_status_when_present() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_stats_targets(&pool).await;

        let repository = SqliteUsageWriteRepository::new(pool.clone());
        repository
            .upsert(sample_usage(
                "request-upstream-reset",
                "pending",
                "pending",
                1,
            ))
            .await
            .expect("pending usage should upsert");
        repository
            .upsert(sample_usage("request-stuck", "pending", "pending", 1))
            .await
            .expect("pending usage should upsert");

        // request-upstream-reset has a failed candidate carrying a concrete 502 status
        // and a connection-reset message — cleanup should use them instead of 504.
        // request-stuck has only a still-pending candidate, so cleanup should fall back to 504.
        sqlx::query(
            r#"
INSERT INTO request_candidates (
  id,
  request_id,
  candidate_index,
  retry_index,
  status,
  status_code,
  error_message,
  is_cached,
  created_at,
  started_at,
  finished_at
) VALUES
  ('candidate-reset', 'request-upstream-reset', 0, 0, 'failed', 502, 'upstream connection reset by peer', 0, 1, 2, 3),
  ('candidate-stuck', 'request-stuck', 0, 0, 'pending', NULL, NULL, 0, 1, NULL, NULL)
"#,
        )
        .execute(&pool)
        .await
        .expect("request candidates should seed");

        let summary = repository
            .cleanup_stale_pending_requests(2, 10, 5, 5)
            .await
            .expect("cleanup should run");
        assert_eq!(summary.recovered, 0);
        assert_eq!(summary.failed, 2);

        let reset = repository
            .find_by_request_id("request-upstream-reset")
            .await
            .expect("upstream-reset usage should load")
            .expect("upstream-reset usage should exist");
        assert_eq!(reset.status, "failed");
        assert_eq!(reset.status_code, Some(502));
        assert_eq!(
            reset.error_message.as_deref(),
            Some("upstream connection reset by peer")
        );

        let stuck = repository
            .find_by_request_id("request-stuck")
            .await
            .expect("stuck usage should load")
            .expect("stuck usage should exist");
        assert_eq!(stuck.status, "failed");
        assert_eq!(stuck.status_code, Some(504));
        assert!(stuck
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("超过 5 分钟未完成")));
    }

    #[tokio::test]
    async fn sqlite_usage_read_repository_reads_usage_contract_views() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_stats_targets(&pool).await;

        let writer = SqliteUsageWriteRepository::new(pool.clone());
        writer
            .upsert(sample_usage("request-1", "completed", "settled", 1_000))
            .await
            .expect("usage should upsert");
        writer
            .upsert(sample_usage("request-2", "failed", "void", 1_010))
            .await
            .expect("usage should upsert");

        let reader = SqliteUsageReadRepository::new(pool);
        let loaded = reader
            .find_by_request_id("request-1")
            .await
            .expect("usage should load")
            .expect("usage should exist");
        assert_eq!(loaded.total_tokens, 7);
        assert_eq!(loaded.billing_status, "settled");

        let listed = reader
            .list_usage_audits(&UsageAuditListQuery {
                provider_name: Some("Provider One".to_string()),
                newest_first: true,
                ..UsageAuditListQuery::default()
            })
            .await
            .expect("usage list should load");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].request_id, "request-2");

        let summary = reader
            .summarize_dashboard_usage(&UsageDashboardSummaryQuery {
                created_from_unix_secs: 999,
                created_until_unix_secs: 1_020,
                user_id: Some("user-1".to_string()),
            })
            .await
            .expect("dashboard summary should load");
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.error_requests, 1);
        assert_eq!(summary.total_tokens, 10);
    }

    #[tokio::test]
    async fn sqlite_usage_daily_heatmap_reads_imported_daily_aggregates() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            r#"
INSERT INTO stats_daily (
    id, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, actual_total_cost, is_complete, created_at, updated_at
) VALUES (
    'daily-1', 86400, 9, 8, 1, 10, 20, 3, 4, 1.25, 1.0, 1, 1, 1
);
INSERT INTO stats_user_daily (
    id, user_id, username, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, created_at, updated_at
) VALUES (
    'user-daily-1', 'user-1', 'user one', 86400, 5, 5, 0, 7, 8, 2, 1, 0.75, 1, 1
);
"#,
        )
        .execute(&pool)
        .await
        .expect("daily aggregates should seed");

        let reader = SqliteUsageReadRepository::new(pool);
        let admin = reader
            .summarize_usage_daily_heatmap(&UsageDailyHeatmapQuery {
                created_from_unix_secs: 0,
                user_id: None,
                admin_mode: true,
            })
            .await
            .expect("admin heatmap should load");
        assert_eq!(admin.len(), 1);
        assert_eq!(admin[0].date, "1970-01-02");
        assert_eq!(admin[0].requests, 9);
        assert_eq!(admin[0].total_tokens, 37);
        assert_eq!(admin[0].actual_total_cost_usd, 1.0);

        let user = reader
            .summarize_usage_daily_heatmap(&UsageDailyHeatmapQuery {
                created_from_unix_secs: 0,
                user_id: Some("user-1".to_string()),
                admin_mode: false,
            })
            .await
            .expect("user heatmap should load");
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].date, "1970-01-02");
        assert_eq!(user[0].requests, 5);
        assert_eq!(user[0].total_tokens, 18);
        assert_eq!(user[0].actual_total_cost_usd, 0.75);
    }

    #[tokio::test]
    async fn sqlite_usage_totals_by_user_ids_reads_imported_user_daily_aggregates() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            r#"
INSERT INTO stats_user_daily (
    id, user_id, username, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, created_at, updated_at
) VALUES (
    'user-daily-1', 'user-1', 'user one', 86400, 5, 5, 0, 7, 8, 2, 1, 0.75, 1, 1
);
INSERT INTO "usage" (
    request_id, id, user_id, api_key_id, provider_name, model, total_tokens,
    status, billing_status, created_at_unix_ms, updated_at_unix_secs
) VALUES
    ('raw-before-cutoff', 'usage-1', 'user-1', 'api-key-1', 'Provider One', 'model-1', 99,
     'completed', 'settled', 90000, 90000),
    ('raw-after-cutoff', 'usage-2', 'user-1', 'api-key-1', 'Provider One', 'model-1', 7,
     'completed', 'settled', 172800, 172800);
"#,
        )
        .execute(&pool)
        .await
        .expect("usage totals fixtures should seed");

        let reader = SqliteUsageReadRepository::new(pool);
        let totals = reader
            .summarize_usage_totals_by_user_ids(&["user-1".to_string()])
            .await
            .expect("user totals should load");

        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].user_id, "user-1");
        assert_eq!(totals[0].request_count, 6);
        assert_eq!(totals[0].total_tokens, 25);
    }

    #[tokio::test]
    async fn sqlite_dashboard_daily_stats_reads_imported_daily_aggregates() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        sqlx::query(
            r#"
INSERT INTO stats_daily (
    id, "date", total_requests, success_requests, error_requests,
    input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
    total_cost, actual_total_cost, is_complete, created_at, updated_at
) VALUES (
    'daily-1', 86400, 9, 8, 1, 10, 20, 3, 4, 1.25, 1.0, 1, 1, 1
);
"#,
        )
        .execute(&pool)
        .await
        .expect("daily aggregates should seed");

        let reader = SqliteUsageReadRepository::new(pool);
        let summary = reader
            .summarize_dashboard_usage(&UsageDashboardSummaryQuery {
                created_from_unix_secs: 0,
                created_until_unix_secs: 172800,
                user_id: None,
            })
            .await
            .expect("dashboard summary should load");
        assert_eq!(summary.total_requests, 9);
        assert_eq!(summary.total_tokens, 37);

        let rows = reader
            .list_dashboard_daily_breakdown(&UsageDashboardDailyBreakdownQuery {
                created_from_unix_secs: 0,
                created_until_unix_secs: 172800,
                tz_offset_minutes: 480,
                user_id: None,
            })
            .await
            .expect("dashboard daily breakdown should load");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, "1970-01-02");
        assert_eq!(rows[0].model, "aggregate");
        assert_eq!(rows[0].requests, 9);
        assert_eq!(rows[0].total_tokens, 37);
    }

    async fn seed_stats_targets(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO users (id, auth_source, created_at, updated_at)
VALUES ('user-1', 'local', 1, 1);
INSERT INTO api_keys (id, user_id, key_hash, created_at, updated_at)
VALUES ('api-key-1', 'user-1', 'hash-1', 1, 1);
INSERT INTO providers (id, name, provider_type, created_at, updated_at)
VALUES ('provider-1', 'Provider One', 'openai', 1, 1);
INSERT INTO provider_api_keys (id, provider_id, name, created_at, updated_at)
VALUES ('provider-key-1', 'provider-1', 'Provider Key One', 1, 1);
"#,
        )
        .execute(pool)
        .await
        .expect("stats targets should seed");
    }

    fn sample_usage(
        request_id: &str,
        status: &str,
        billing_status: &str,
        updated_at: u64,
    ) -> UpsertUsageRecord {
        UpsertUsageRecord {
            request_id: request_id.to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: Some("legacy-user".to_string()),
            api_key_name: Some("legacy-key".to_string()),
            provider_name: "Provider One".to_string(),
            model: "model-1".to_string(),
            target_model: Some("target-model".to_string()),
            provider_id: Some("provider-1".to_string()),
            provider_endpoint_id: Some("endpoint-1".to_string()),
            provider_api_key_id: Some("provider-key-1".to_string()),
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
}
