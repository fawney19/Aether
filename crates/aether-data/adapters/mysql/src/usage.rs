use std::collections::{BTreeMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_ai_formats::UPSTREAM_IS_STREAM_KEY;
use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use aether_data_contracts::repository::usage::{
    strip_deprecated_usage_display_fields, usage_can_recover_terminal_failure,
    usage_request_metadata_client_family, PendingUsageCleanupSummary, StoredRequestUsageAudit,
    StoredUsageDailySummary, StoredUsageDashboardDailyBreakdownRow, StoredUsageDashboardSummary,
    StoredUsageUserTotals, UpsertUsageRecord, UsageCleanupExecutionMode, UsageCleanupPreviewCounts,
    UsageCleanupSummary, UsageCleanupTargets, UsageCleanupWindow, UsageDailyHeatmapQuery,
    UsageDashboardDailyBreakdownQuery, UsageDashboardSummaryQuery, UsageWriteRepository,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::MysqlPool;

mod cleanup;
mod counters;
mod http_capture;
mod read;
mod snapshots;

pub use read::MysqlUsageReadFilter;

const USAGE_COLUMNS: &str = r#"
SELECT
  id,
  `usage`.request_id,
  user_id,
  api_key_id,
  `usage`.username,
  `usage`.api_key_name,
  provider_name,
  model,
  target_model,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.selected_provider_id
    ELSE `usage`.provider_id
  END AS provider_id,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.selected_endpoint_id
    ELSE `usage`.provider_endpoint_id
  END AS provider_endpoint_id,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.selected_provider_api_key_id
    ELSE `usage`.provider_api_key_id
  END AS provider_api_key_id,
  request_type,
  api_format,
  api_family,
  endpoint_kind,
  endpoint_api_format,
  provider_api_family,
  provider_endpoint_kind,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN COALESCE(usage_routing_snapshots.has_format_conversion, FALSE)
    ELSE COALESCE(`usage`.has_format_conversion, FALSE)
  END AS has_format_conversion,
  is_stream,
  upstream_is_stream,
  input_tokens,
  COALESCE(usage_settlement_snapshots.billing_output_tokens, `usage`.output_tokens, 0)
    AS output_tokens,
  total_tokens,
  COALESCE(
    usage_settlement_snapshots.billing_cache_creation_tokens,
    CASE
      WHEN usage_settlement_snapshots.billing_cache_creation_5m_tokens IS NOT NULL
        OR usage_settlement_snapshots.billing_cache_creation_1h_tokens IS NOT NULL
      THEN COALESCE(usage_settlement_snapshots.billing_cache_creation_5m_tokens, 0)
        + COALESCE(usage_settlement_snapshots.billing_cache_creation_1h_tokens, 0)
    END,
    `usage`.cache_creation_input_tokens,
    0
  ) AS cache_creation_input_tokens,
  COALESCE(
    usage_settlement_snapshots.billing_cache_creation_5m_tokens,
    `usage`.cache_creation_ephemeral_5m_input_tokens,
    0
  ) AS cache_creation_ephemeral_5m_input_tokens,
  COALESCE(
    usage_settlement_snapshots.billing_cache_creation_1h_tokens,
    `usage`.cache_creation_ephemeral_1h_input_tokens,
    0
  ) AS cache_creation_ephemeral_1h_input_tokens,
  COALESCE(
    usage_settlement_snapshots.billing_cache_read_tokens,
    `usage`.cache_read_input_tokens,
    0
  ) AS cache_read_input_tokens,
  CAST(COALESCE(
    usage_settlement_snapshots.billing_cache_creation_cost_usd,
    `usage`.cache_creation_cost_usd,
    0
  ) AS DOUBLE) AS cache_creation_cost_usd,
  CAST(COALESCE(
    usage_settlement_snapshots.billing_cache_read_cost_usd,
    `usage`.cache_read_cost_usd,
    0
  ) AS DOUBLE) AS cache_read_cost_usd,
  CAST(COALESCE(
    usage_settlement_snapshots.output_price_per_1m,
    `usage`.output_price_per_1m
  ) AS DOUBLE) AS output_price_per_1m,
  CAST(COALESCE(
    usage_settlement_snapshots.billing_total_cost_usd,
    `usage`.total_cost_usd,
    0
  ) AS DOUBLE) AS total_cost_usd,
  CAST(COALESCE(
    usage_settlement_snapshots.billing_actual_total_cost_usd,
    `usage`.actual_total_cost_usd,
    0
  ) AS DOUBLE) AS actual_total_cost_usd,
  status_code,
  error_message,
  error_category,
  response_time_ms,
  first_byte_time_ms,
  status,
  COALESCE(usage_settlement_snapshots.billing_status, `usage`.billing_status)
    AS billing_status,
  CAST(COALESCE(usage_http_audits.request_headers, `usage`.request_headers) AS CHAR) AS request_headers,
  CAST(`usage`.request_body AS CHAR) AS request_body,
  `usage`.request_body_compressed,
  CAST(COALESCE(
    usage_http_audits.provider_request_headers,
    `usage`.provider_request_headers
  ) AS CHAR) AS provider_request_headers,
  CAST(`usage`.provider_request_body AS CHAR) AS provider_request_body,
  `usage`.provider_request_body_compressed,
  CAST(COALESCE(usage_http_audits.response_headers, `usage`.response_headers) AS CHAR) AS response_headers,
  CAST(`usage`.response_body AS CHAR) AS response_body,
  `usage`.response_body_compressed,
  CAST(COALESCE(
    usage_http_audits.client_response_headers,
    `usage`.client_response_headers
  ) AS CHAR) AS client_response_headers,
  CAST(`usage`.client_response_body AS CHAR) AS client_response_body,
  `usage`.client_response_body_compressed,
  usage_http_audits.request_body_ref AS http_request_body_ref,
  usage_http_audits.provider_request_body_ref AS http_provider_request_body_ref,
  usage_http_audits.response_body_ref AS http_response_body_ref,
  usage_http_audits.client_response_body_ref AS http_client_response_body_ref,
  usage_http_audits.request_body_state AS http_request_body_state,
  usage_http_audits.provider_request_body_state AS http_provider_request_body_state,
  usage_http_audits.response_body_state AS http_response_body_state,
  usage_http_audits.client_response_body_state AS http_client_response_body_state,
  request_metadata,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.candidate_id
    ELSE `usage`.candidate_id
  END AS routing_candidate_id,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.candidate_index
    ELSE `usage`.candidate_index
  END AS routing_candidate_index,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.key_name
    ELSE `usage`.key_name
  END AS routing_key_name,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.planner_kind
    ELSE `usage`.planner_kind
  END AS routing_planner_kind,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.route_family
    ELSE `usage`.route_family
  END AS routing_route_family,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.route_kind
    ELSE `usage`.route_kind
  END AS routing_route_kind,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.execution_path
    ELSE `usage`.execution_path
  END AS routing_execution_path,
  CASE
    WHEN usage_routing_snapshots.request_id IS NOT NULL
    THEN usage_routing_snapshots.local_execution_runtime_miss_reason
    ELSE `usage`.local_execution_runtime_miss_reason
  END AS routing_local_execution_runtime_miss_reason,
  usage_settlement_snapshots.billing_snapshot_schema_version
    AS settlement_billing_snapshot_schema_version,
  usage_settlement_snapshots.billing_snapshot_status AS settlement_billing_snapshot_status,
  CAST(usage_settlement_snapshots.rate_multiplier AS DOUBLE) AS settlement_rate_multiplier,
  usage_settlement_snapshots.is_free_tier AS settlement_is_free_tier,
  CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE)
    AS settlement_input_price_per_1m,
  CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE)
    AS settlement_output_price_per_1m,
  CAST(usage_settlement_snapshots.cache_creation_price_per_1m AS DOUBLE)
    AS settlement_cache_creation_price_per_1m,
  CAST(usage_settlement_snapshots.cache_read_price_per_1m AS DOUBLE)
    AS settlement_cache_read_price_per_1m,
  CAST(usage_settlement_snapshots.price_per_request AS DOUBLE)
    AS settlement_price_per_request,
  usage_settlement_snapshots.settlement_snapshot_schema_version
    AS settlement_snapshot_schema_version,
  CAST(usage_settlement_snapshots.settlement_snapshot AS CHAR) AS settlement_snapshot,
  CAST(usage_settlement_snapshots.billing_dimensions AS CHAR)
    AS settlement_billing_dimensions,
  usage_settlement_snapshots.billing_input_tokens AS settlement_billing_input_tokens,
  usage_settlement_snapshots.billing_effective_input_tokens
    AS settlement_billing_effective_input_tokens,
  usage_settlement_snapshots.billing_output_tokens AS settlement_billing_output_tokens,
  usage_settlement_snapshots.billing_cache_creation_tokens
    AS settlement_billing_cache_creation_tokens,
  usage_settlement_snapshots.billing_cache_creation_5m_tokens
    AS settlement_billing_cache_creation_5m_tokens,
  usage_settlement_snapshots.billing_cache_creation_1h_tokens
    AS settlement_billing_cache_creation_1h_tokens,
  usage_settlement_snapshots.billing_cache_read_tokens
    AS settlement_billing_cache_read_tokens,
  usage_settlement_snapshots.billing_total_input_context
    AS settlement_billing_total_input_context,
  CAST(usage_settlement_snapshots.billing_cache_creation_cost_usd AS DOUBLE)
    AS settlement_billing_cache_creation_cost_usd,
  CAST(usage_settlement_snapshots.billing_cache_read_cost_usd AS DOUBLE)
    AS settlement_billing_cache_read_cost_usd,
  CAST(usage_settlement_snapshots.billing_total_cost_usd AS DOUBLE)
    AS settlement_billing_total_cost_usd,
  CAST(usage_settlement_snapshots.billing_actual_total_cost_usd AS DOUBLE)
    AS settlement_billing_actual_total_cost_usd,
  usage_settlement_snapshots.billing_pricing_source AS settlement_billing_pricing_source,
  usage_settlement_snapshots.billing_rule_id AS settlement_billing_rule_id,
  usage_settlement_snapshots.billing_rule_version AS settlement_billing_rule_version,
  COALESCE(usage_settlement_snapshots.finalized_at, `usage`.finalized_at)
    AS finalized_at_unix_secs,
  created_at_unix_ms,
  updated_at_unix_secs
FROM `usage`
LEFT JOIN usage_http_audits
  ON usage_http_audits.request_id = `usage`.request_id
LEFT JOIN usage_routing_snapshots
  ON usage_routing_snapshots.request_id = `usage`.request_id
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = `usage`.request_id
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
  outcome_class,
  sla_eligible,
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
  ?, ?, ?
)
ON DUPLICATE KEY UPDATE
  user_id = VALUES(user_id),
  api_key_id = VALUES(api_key_id),
  provider_name = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN provider_name ELSE VALUES(provider_name) END,
  model = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN model ELSE VALUES(model) END,
  target_model = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN target_model ELSE VALUES(target_model) END,
  provider_id = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN provider_id ELSE VALUES(provider_id) END,
  provider_endpoint_id = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN provider_endpoint_id ELSE VALUES(provider_endpoint_id) END,
  provider_api_key_id = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN provider_api_key_id ELSE VALUES(provider_api_key_id) END,
  request_type = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN request_type ELSE VALUES(request_type) END,
  api_format = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN api_format ELSE VALUES(api_format) END,
  api_family = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN api_family ELSE VALUES(api_family) END,
  endpoint_kind = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN endpoint_kind ELSE VALUES(endpoint_kind) END,
  endpoint_api_format = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN endpoint_api_format ELSE VALUES(endpoint_api_format) END,
  provider_api_family = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN provider_api_family ELSE VALUES(provider_api_family) END,
  provider_endpoint_kind = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN provider_endpoint_kind ELSE VALUES(provider_endpoint_kind) END,
  has_format_conversion = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN has_format_conversion ELSE VALUES(has_format_conversion) END,
  is_stream = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN is_stream ELSE VALUES(is_stream) END,
  upstream_is_stream = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN upstream_is_stream ELSE VALUES(upstream_is_stream) END,
  input_tokens = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN input_tokens
    ELSE VALUES(input_tokens)
  END,
  output_tokens = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN output_tokens
    ELSE VALUES(output_tokens)
  END,
  total_tokens = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN total_tokens
    ELSE VALUES(total_tokens)
  END,
  cache_creation_input_tokens = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN cache_creation_input_tokens
    ELSE VALUES(cache_creation_input_tokens)
  END,
  cache_creation_ephemeral_5m_input_tokens = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN cache_creation_ephemeral_5m_input_tokens
    ELSE VALUES(cache_creation_ephemeral_5m_input_tokens)
  END,
  cache_creation_ephemeral_1h_input_tokens = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN cache_creation_ephemeral_1h_input_tokens
    ELSE VALUES(cache_creation_ephemeral_1h_input_tokens)
  END,
  cache_read_input_tokens = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN cache_read_input_tokens
    ELSE VALUES(cache_read_input_tokens)
  END,
  cache_creation_cost_usd = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN cache_creation_cost_usd
    ELSE VALUES(cache_creation_cost_usd)
  END,
  cache_read_cost_usd = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN cache_read_cost_usd
    ELSE VALUES(cache_read_cost_usd)
  END,
  output_price_per_1m = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN output_price_per_1m
    ELSE VALUES(output_price_per_1m)
  END,
  total_cost_usd = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN total_cost_usd
    ELSE VALUES(total_cost_usd)
  END,
  actual_total_cost_usd = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN actual_total_cost_usd
    ELSE VALUES(actual_total_cost_usd)
  END,
  status_code = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN status_code
    WHEN status = 'streaming' AND VALUES(status) = 'pending' THEN status_code
    WHEN status = 'streaming' AND VALUES(status) = 'streaming' AND VALUES(status_code) IS NULL THEN status_code
    ELSE VALUES(status_code)
  END,
  outcome_class = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN outcome_class
    WHEN status = 'streaming' AND VALUES(status) = 'pending' THEN outcome_class
    ELSE VALUES(outcome_class)
  END,
  sla_eligible = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN sla_eligible
    WHEN status = 'streaming' AND VALUES(status) = 'pending' THEN sla_eligible
    ELSE VALUES(sla_eligible)
  END,
  error_message = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN error_message
    WHEN status = 'streaming' AND VALUES(status) = 'pending' THEN error_message
    ELSE VALUES(error_message)
  END,
  error_category = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN error_category
    WHEN status = 'streaming' AND VALUES(status) = 'pending' THEN error_category
    ELSE VALUES(error_category)
  END,
  response_time_ms = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN response_time_ms
    WHEN VALUES(response_time_ms) IS NULL OR VALUES(response_time_ms) = 0
    THEN COALESCE(response_time_ms, VALUES(response_time_ms))
    ELSE VALUES(response_time_ms)
  END,
  first_byte_time_ms = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN first_byte_time_ms
    WHEN VALUES(first_byte_time_ms) IS NULL OR VALUES(first_byte_time_ms) = 0
    THEN COALESCE(first_byte_time_ms, VALUES(first_byte_time_ms))
    ELSE VALUES(first_byte_time_ms)
  END,
  billing_status = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN billing_status
    ELSE VALUES(billing_status)
  END,
  request_metadata = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN request_metadata ELSE VALUES(request_metadata) END,
  candidate_id = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN candidate_id ELSE VALUES(candidate_id) END,
  candidate_index = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN candidate_index ELSE VALUES(candidate_index) END,
  key_name = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN key_name ELSE VALUES(key_name) END,
  planner_kind = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN planner_kind ELSE VALUES(planner_kind) END,
  route_family = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN route_family ELSE VALUES(route_family) END,
  route_kind = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN route_kind ELSE VALUES(route_kind) END,
  execution_path = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN execution_path ELSE VALUES(execution_path) END,
  local_execution_runtime_miss_reason = CASE WHEN (status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming')) OR (status = 'streaming' AND VALUES(status) = 'pending') THEN local_execution_runtime_miss_reason ELSE VALUES(local_execution_runtime_miss_reason) END,
  finalized_at = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN finalized_at
    ELSE VALUES(finalized_at)
  END,
  updated_at_unix_secs = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN updated_at_unix_secs
    ELSE VALUES(updated_at_unix_secs)
  END,
  status = CASE
    WHEN status IN ('completed', 'failed', 'cancelled') AND VALUES(status) IN ('pending', 'streaming') THEN status
    WHEN status = 'streaming' AND VALUES(status) = 'pending' THEN status
    ELSE VALUES(status)
  END
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

const MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR: &str = r#"
GREATEST(
  COALESCE(
    CASE
      WHEN settlement.billing_effective_input_tokens IS NOT NULL THEN
        GREATEST(settlement.billing_effective_input_tokens, 0)
        + GREATEST(COALESCE(settlement.billing_output_tokens, `usage`.output_tokens, 0), 0)
        + GREATEST(
            COALESCE(
              settlement.billing_cache_creation_tokens,
              CASE
                WHEN settlement.billing_cache_creation_5m_tokens IS NOT NULL
                  OR settlement.billing_cache_creation_1h_tokens IS NOT NULL
                THEN COALESCE(settlement.billing_cache_creation_5m_tokens, 0)
                   + COALESCE(settlement.billing_cache_creation_1h_tokens, 0)
              END,
              CASE
                WHEN COALESCE(`usage`.cache_creation_input_tokens, 0) = 0
                     AND (
                       COALESCE(`usage`.cache_creation_ephemeral_5m_input_tokens, 0)
                       + COALESCE(`usage`.cache_creation_ephemeral_1h_input_tokens, 0)
                     ) > 0
                THEN COALESCE(`usage`.cache_creation_ephemeral_5m_input_tokens, 0)
                   + COALESCE(`usage`.cache_creation_ephemeral_1h_input_tokens, 0)
                ELSE COALESCE(`usage`.cache_creation_input_tokens, 0)
              END,
              0
            ),
            0
          )
        + GREATEST(
            COALESCE(
              settlement.billing_cache_read_tokens,
              `usage`.cache_read_input_tokens,
              0
            ),
            0
          )
      WHEN settlement.billing_total_input_context IS NOT NULL THEN
        GREATEST(settlement.billing_total_input_context, 0)
        + GREATEST(COALESCE(settlement.billing_output_tokens, `usage`.output_tokens, 0), 0)
    END,
    NULLIF(GREATEST(COALESCE(`usage`.total_tokens, 0), 0), 0),
    (
      CASE
        WHEN (
          LOWER(COALESCE(`usage`.endpoint_api_format, `usage`.api_format, '')) = 'openai'
          OR LOWER(COALESCE(`usage`.endpoint_api_format, `usage`.api_format, '')) LIKE 'openai:%'
          OR LOWER(COALESCE(`usage`.endpoint_api_format, `usage`.api_format, '')) = 'gemini'
          OR LOWER(COALESCE(`usage`.endpoint_api_format, `usage`.api_format, '')) LIKE 'gemini:%'
          OR LOWER(COALESCE(`usage`.endpoint_api_format, `usage`.api_format, '')) = 'google'
          OR LOWER(COALESCE(`usage`.endpoint_api_format, `usage`.api_format, '')) LIKE 'google:%'
        )
        AND COALESCE(`usage`.input_tokens, 0) > 0
        AND COALESCE(`usage`.cache_read_input_tokens, 0) > 0
        THEN GREATEST(
          COALESCE(`usage`.input_tokens, 0) - COALESCE(`usage`.cache_read_input_tokens, 0),
          0
        )
        ELSE GREATEST(COALESCE(`usage`.input_tokens, 0), 0)
      END
      + GREATEST(COALESCE(`usage`.output_tokens, 0), 0)
      + (
        CASE
          WHEN COALESCE(`usage`.cache_creation_input_tokens, 0) = 0
               AND (
                 COALESCE(`usage`.cache_creation_ephemeral_5m_input_tokens, 0)
                 + COALESCE(`usage`.cache_creation_ephemeral_1h_input_tokens, 0)
               ) > 0
          THEN COALESCE(`usage`.cache_creation_ephemeral_5m_input_tokens, 0)
             + COALESCE(`usage`.cache_creation_ephemeral_1h_input_tokens, 0)
          ELSE GREATEST(COALESCE(`usage`.cache_creation_input_tokens, 0), 0)
        END
      )
      + GREATEST(COALESCE(`usage`.cache_read_input_tokens, 0), 0)
    ),
    0
  ),
  0
)
"#;

const MYSQL_PROVIDER_KEY_SUCCESS_FLAG_EXPR: &str = r#"
CASE
  WHEN outcome_class = 'success' THEN 1
  ELSE 0
END
"#;

const MYSQL_PROVIDER_KEY_ERROR_FLAG_EXPR: &str = r#"
CASE
  WHEN outcome_class = 'service_error' THEN 1
  ELSE 0
END
"#;

#[derive(Debug, Clone)]
pub struct MysqlUsageWriteRepository {
    pool: MysqlPool,
}

#[derive(Debug, Clone)]
pub struct MysqlUsageStorage {
    pool: MysqlPool,
}

impl MysqlUsageStorage {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    pub async fn read_usage_counter_health(
        &self,
    ) -> Result<aether_data_contracts::repository::usage::UsageCounterHealthSnapshot, DataLayerError>
    {
        counters::read_health(&self.pool).await
    }

    pub async fn read_usage_counter_pending_health(
        &self,
    ) -> Result<
        aether_data_contracts::repository::usage::UsageCounterPendingHealthSnapshot,
        DataLayerError,
    > {
        counters::read_pending_health(&self.pool).await
    }

    async fn summarize_usage_daily_heatmap_raw_from_range(
        &self,
        created_from_unix_secs: u64,
        created_until_unix_secs: u64,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        let mut sql = format!(
            r#"
SELECT
  DATE_FORMAT(FROM_UNIXTIME(created_at_unix_ms), '%Y-%m-%d') AS date,
  CAST(COUNT(*) AS SIGNED) AS requests,
  CAST(COALESCE(SUM({canonical_total_tokens_expr}), 0) AS SIGNED) AS total_tokens,
  CAST(COALESCE(SUM(COALESCE(total_cost_usd, 0)), 0) AS DOUBLE) AS total_cost_usd,
  CAST(COALESCE(SUM(COALESCE(actual_total_cost_usd, 0)), 0) AS DOUBLE) AS actual_total_cost_usd
FROM `usage`
LEFT JOIN usage_settlement_snapshots AS settlement
  ON settlement.request_id = `usage`.request_id
WHERE created_at_unix_ms >= ?
  AND created_at_unix_ms < ?
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#,
            canonical_total_tokens_expr = MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR,
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

    pub async fn summarize_usage_daily_heatmap(
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

    pub async fn summarize_dashboard_usage_from_daily_aggregates(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<Option<StoredUsageDashboardSummary>, DataLayerError> {
        let row = if let Some(user_id) = query.user_id.as_deref() {
            sqlx::query(
                r#"
SELECT
  CAST(COALESCE(SUM(total_requests), 0) AS SIGNED) AS total_requests,
  CAST(COALESCE(SUM(sla_eligible_requests), 0) AS SIGNED) AS sla_eligible_requests,
  CAST(COALESCE(SUM(input_tokens), 0) AS SIGNED) AS input_tokens,
  CAST(COALESCE(SUM(input_tokens), 0) AS SIGNED) AS effective_input_tokens,
  CAST(COALESCE(SUM(output_tokens), 0) AS SIGNED) AS output_tokens,
  CAST(COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS SIGNED) AS total_tokens,
  CAST(COALESCE(SUM(cache_creation_tokens), 0) AS SIGNED) AS cache_creation_tokens,
  CAST(COALESCE(SUM(cache_read_tokens), 0) AS SIGNED) AS cache_read_tokens,
  CAST(COALESCE(SUM(input_tokens + cache_creation_tokens + cache_read_tokens), 0) AS SIGNED) AS total_input_context,
  CAST(0.0 AS DOUBLE) AS cache_creation_cost_usd,
  CAST(0.0 AS DOUBLE) AS cache_read_cost_usd,
  CAST(COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS DOUBLE) AS total_cost_usd,
  CAST(COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS DOUBLE) AS actual_total_cost_usd,
  CAST(COALESCE(SUM(error_requests), 0) AS SIGNED) AS error_requests,
  CAST(COALESCE(SUM(user_error_requests), 0) AS SIGNED) AS user_error_requests,
  CAST(0.0 AS DOUBLE) AS response_time_sum_ms,
  CAST(0 AS SIGNED) AS response_time_samples
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
  CAST(COALESCE(SUM(total_requests), 0) AS SIGNED) AS total_requests,
  CAST(COALESCE(SUM(sla_eligible_requests), 0) AS SIGNED) AS sla_eligible_requests,
  CAST(COALESCE(SUM(input_tokens), 0) AS SIGNED) AS input_tokens,
  CAST(COALESCE(SUM(input_tokens), 0) AS SIGNED) AS effective_input_tokens,
  CAST(COALESCE(SUM(output_tokens), 0) AS SIGNED) AS output_tokens,
  CAST(COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS SIGNED) AS total_tokens,
  CAST(COALESCE(SUM(cache_creation_tokens), 0) AS SIGNED) AS cache_creation_tokens,
  CAST(COALESCE(SUM(cache_read_tokens), 0) AS SIGNED) AS cache_read_tokens,
  CAST(COALESCE(SUM(input_tokens + cache_creation_tokens + cache_read_tokens), 0) AS SIGNED) AS total_input_context,
  CAST(COALESCE(SUM(COALESCE(cache_creation_cost, 0)), 0) AS DOUBLE) AS cache_creation_cost_usd,
  CAST(COALESCE(SUM(COALESCE(cache_read_cost, 0)), 0) AS DOUBLE) AS cache_read_cost_usd,
  CAST(COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS DOUBLE) AS total_cost_usd,
  CAST(COALESCE(SUM(COALESCE(actual_total_cost, 0)), 0) AS DOUBLE) AS actual_total_cost_usd,
  CAST(COALESCE(SUM(error_requests), 0) AS SIGNED) AS error_requests,
  CAST(COALESCE(SUM(user_error_requests), 0) AS SIGNED) AS user_error_requests,
  CAST(0.0 AS DOUBLE) AS response_time_sum_ms,
  CAST(0 AS SIGNED) AS response_time_samples
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
            sla_eligible_requests: row_u64(&row, "sla_eligible_requests")?,
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
            user_error_requests: row_u64(&row, "user_error_requests")?,
            response_time_sum_ms: row.try_get("response_time_sum_ms").map_sql_err()?,
            response_time_samples: row_u64(&row, "response_time_samples")?,
        }))
    }

    pub async fn list_dashboard_daily_breakdown_from_daily_aggregates(
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
  CAST(COALESCE(SUM(total_requests), 0) AS SIGNED) AS requests,
  CAST(COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS SIGNED) AS total_tokens,
  CAST(COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS DOUBLE) AS total_cost_usd,
  CAST(0.0 AS DOUBLE) AS response_time_sum_ms,
  CAST(0 AS SIGNED) AS response_time_samples
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
  CAST(COALESCE(SUM(total_requests), 0) AS SIGNED) AS requests,
  CAST(COALESCE(SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens), 0) AS SIGNED) AS total_tokens,
  CAST(COALESCE(SUM(COALESCE(total_cost, 0)), 0) AS DOUBLE) AS total_cost_usd,
  CAST(0.0 AS DOUBLE) AS response_time_sum_ms,
  CAST(0 AS SIGNED) AS response_time_samples
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

        let mut aggregate_builder = QueryBuilder::<MySql>::new(
            r#"
SELECT
  user_id,
  CAST(COALESCE(SUM(total_requests), 0) AS SIGNED) AS request_count,
  CAST(COALESCE(
    SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens),
    0
  ) AS SIGNED) AS total_tokens,
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

        let mut raw_builder = QueryBuilder::<MySql>::new(format!(
            r#"
SELECT
  `usage`.user_id,
  CAST(COUNT(*) AS SIGNED) AS request_count,
  CAST(COALESCE(SUM({canonical_total_tokens_expr}), 0) AS SIGNED) AS total_tokens
FROM `usage`
JOIN (
"#,
            canonical_total_tokens_expr = MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR,
        ));
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
LEFT JOIN usage_settlement_snapshots AS settlement
  ON settlement.request_id = `usage`.request_id
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

impl MysqlUsageWriteRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    pub async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(&format!(
            "{USAGE_COLUMNS} WHERE `usage`.request_id = ? LIMIT 1"
        ))
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        let usage = row
            .as_ref()
            .map(|row| map_usage_row(row, true))
            .transpose()?;
        match usage {
            Some(usage) => http_capture::hydrate_usage_body_refs(&self.pool, usage)
                .await
                .map(Some),
            None => Ok(None),
        }
    }
}

#[async_trait]
impl UsageWriteRepository for MysqlUsageWriteRepository {
    async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        let mut usage = strip_deprecated_usage_display_fields(usage);
        usage.validate()?;
        let prepared_capture = http_capture::prepare_usage_http_capture(&mut usage)?;
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let existing = counters::lock_and_load_usage(&mut tx, &usage.request_id).await?;
        let recovers_terminal_failure = existing.as_ref().is_some_and(|existing| {
            usage_can_recover_terminal_failure(
                &existing.status,
                &existing.billing_status,
                &usage.status,
                &usage.billing_status,
            )
        });
        if let Some(existing) = existing.as_ref() {
            if (existing.billing_status == "settled" || existing.billing_status == "void")
                && !recovers_terminal_failure
            {
                let existing = existing.clone();
                tx.rollback().await.map_sql_err()?;
                return http_capture::hydrate_usage_body_refs(&self.pool, existing).await;
            }
        }

        let capture_update_allowed = recovers_terminal_failure
            || http_capture::capture_update_allowed(existing.as_ref(), &usage.status);
        if capture_update_allowed {
            http_capture::apply_previous_metadata_tombstones(&mut usage, existing.as_ref());
        }
        let prepared_snapshots = capture_update_allowed
            .then(|| snapshots::from_usage(&usage))
            .transpose()?;
        bind_upsert(sqlx::query(UPSERT_USAGE_SQL), &usage)?
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        if capture_update_allowed {
            http_capture::sync_usage_http_capture(&mut tx, &usage.request_id, &prepared_capture)
                .await?;
            let (routing_snapshot, settlement_snapshot) = prepared_snapshots
                .as_ref()
                .expect("capture-allowed usage has prepared snapshots");
            snapshots::sync(
                &mut tx,
                &usage.request_id,
                routing_snapshot,
                settlement_snapshot,
                matches!(usage.status.as_str(), "completed" | "failed" | "cancelled"),
            )
            .await?;
        }
        counters::enqueue_usage_transition_for_request(
            &mut tx,
            &usage.request_id,
            existing.as_ref(),
        )
        .await?;
        tx.commit().await.map_sql_err()?;
        self.find_by_request_id(&usage.request_id)
            .await?
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue("usage upsert returned no row".to_string())
            })
    }

    async fn rebuild_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        sqlx::query(
            r#"
UPDATE api_keys
SET total_requests = 0,
    total_tokens = 0,
    total_cost_usd = 0,
    last_used_at = NULL
"#,
        )
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let rows_affected = sqlx::query(&format!(
            r#"
UPDATE api_keys
JOIN (
  SELECT
    api_key_id,
    COUNT(*) AS total_requests,
    COALESCE(SUM({canonical_total_tokens_expr}), 0) AS total_tokens,
    COALESCE(SUM(COALESCE(total_cost_usd, 0)), 0) AS total_cost_usd,
    MAX(created_at_unix_ms) AS last_used_at
  FROM `usage`
  LEFT JOIN usage_settlement_snapshots AS settlement
    ON settlement.request_id = `usage`.request_id
  WHERE api_key_id IS NOT NULL
    AND TRIM(api_key_id) <> ''
    AND status NOT IN ('pending', 'streaming')
  GROUP BY api_key_id
) AS aggregated ON aggregated.api_key_id = api_keys.id
SET api_keys.total_requests = aggregated.total_requests,
    api_keys.total_tokens = aggregated.total_tokens,
    api_keys.total_cost_usd = aggregated.total_cost_usd,
    api_keys.last_used_at = aggregated.last_used_at
"#,
            canonical_total_tokens_expr = MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR,
        ))
        .execute(&mut *tx)
        .await
        .map_sql_err()?
        .rows_affected();
        tx.commit().await.map_sql_err()?;
        Ok(rows_affected)
    }

    async fn rebuild_provider_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        sqlx::query(
            r#"
UPDATE provider_api_keys
SET request_count = 0,
    success_count = 0,
    sla_eligible_count = 0,
    error_count = 0,
    user_error_count = 0,
    total_tokens = 0,
    total_cost_usd = 0,
    total_response_time_ms = 0,
    last_used_at = NULL
"#,
        )
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let rows_affected = sqlx::query(&format!(
            r#"
UPDATE provider_api_keys
JOIN (
  SELECT
    provider_api_key_id,
    COUNT(*) AS request_count,
    COALESCE(SUM({success_flag_expr}), 0) AS success_count,
    COALESCE(SUM(CASE WHEN sla_eligible THEN 1 ELSE 0 END), 0) AS sla_eligible_count,
    COALESCE(SUM({error_flag_expr}), 0) AS error_count,
    COALESCE(SUM(CASE WHEN outcome_class = 'user_error' THEN 1 ELSE 0 END), 0) AS user_error_count,
    COALESCE(SUM(CASE
      WHEN status IN ('pending', 'streaming') THEN 0
      ELSE {canonical_total_tokens_expr}
    END), 0) AS total_tokens,
    COALESCE(SUM(CASE
      WHEN status IN ('pending', 'streaming') THEN 0
      ELSE COALESCE(total_cost_usd, 0)
    END), 0) AS total_cost_usd,
    COALESCE(SUM(CASE
      WHEN {success_flag_expr} = 1 AND response_time_ms IS NOT NULL
      THEN GREATEST(COALESCE(response_time_ms, 0), 0)
      ELSE 0
    END), 0) AS total_response_time_ms,
    MAX(created_at_unix_ms) AS last_used_at
  FROM `usage`
  LEFT JOIN usage_settlement_snapshots AS settlement
    ON settlement.request_id = `usage`.request_id
  WHERE provider_api_key_id IS NOT NULL
    AND TRIM(provider_api_key_id) <> ''
  GROUP BY provider_api_key_id
) AS aggregated ON aggregated.provider_api_key_id = provider_api_keys.id
SET provider_api_keys.request_count = aggregated.request_count,
    provider_api_keys.success_count = aggregated.success_count,
    provider_api_keys.sla_eligible_count = aggregated.sla_eligible_count,
    provider_api_keys.error_count = aggregated.error_count,
    provider_api_keys.user_error_count = aggregated.user_error_count,
    provider_api_keys.total_tokens = aggregated.total_tokens,
    provider_api_keys.total_cost_usd = aggregated.total_cost_usd,
    provider_api_keys.total_response_time_ms = aggregated.total_response_time_ms,
    provider_api_keys.last_used_at = aggregated.last_used_at
"#,
            success_flag_expr = MYSQL_PROVIDER_KEY_SUCCESS_FLAG_EXPR,
            error_flag_expr = MYSQL_PROVIDER_KEY_ERROR_FLAG_EXPR,
            canonical_total_tokens_expr = MYSQL_USAGE_CANONICAL_TOTAL_TOKENS_EXPR,
        ))
        .execute(&mut *tx)
        .await
        .map_sql_err()?
        .rows_affected();
        tx.commit().await.map_sql_err()?;
        Ok(rows_affected)
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
                .bind(to_i64(cutoff_unix_secs, "stale pending usage cutoff")?)
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
    error_message = NULL,
    outcome_class = 'success',
    sla_eligible = TRUE
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
    outcome_class = ?,
    sla_eligible = ?,
    billing_status = 'void',
    finalized_at = ?,
    total_cost_usd = 0,
    actual_total_cost_usd = 0
WHERE request_id = ?
"#,
                    )
                    .bind(status_code_i64)
                    .bind(&error_message)
                    .bind(if status_code == 400 {
                        "user_error"
                    } else {
                        "service_error"
                    })
                    .bind(status_code != 400)
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
    error_message = ?,
    outcome_class = ?,
    sla_eligible = ?
WHERE request_id = ?
"#,
                    )
                    .bind(status_code_i64)
                    .bind(&error_message)
                    .bind(if status_code == 400 {
                        "user_error"
                    } else {
                        "service_error"
                    })
                    .bind(status_code != 400)
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

    async fn flush_usage_counter_deltas(
        &self,
        batch_size: usize,
    ) -> Result<aether_data_contracts::repository::usage::UsageCounterFlushSummary, DataLayerError>
    {
        counters::flush(&self.pool, batch_size).await
    }

    async fn enqueue_proxy_node_counter_delta(
        &self,
        delta: aether_data_contracts::repository::usage::ProxyNodeCounterDelta,
    ) -> Result<bool, DataLayerError> {
        counters::enqueue_proxy_node(&self.pool, delta).await
    }

    async fn enqueue_management_token_counter_delta(
        &self,
        delta: aether_data_contracts::repository::usage::ManagementTokenCounterDelta,
    ) -> Result<bool, DataLayerError> {
        counters::enqueue_management_token(&self.pool, delta).await
    }

    async fn enqueue_api_key_last_used_delta(
        &self,
        delta: aether_data_contracts::repository::usage::ApiKeyLastUsedDelta,
    ) -> Result<bool, DataLayerError> {
        counters::enqueue_api_key_last_used(&self.pool, delta).await
    }

    async fn cleanup_processed_usage_counter_deltas(
        &self,
        cutoff_unix_secs: u64,
        batch_size: usize,
    ) -> Result<usize, DataLayerError> {
        counters::cleanup_processed(&self.pool, cutoff_unix_secs, batch_size).await
    }

    async fn cleanup_usage(
        &self,
        window: &UsageCleanupWindow,
        batch_size: usize,
        auto_delete_expired_keys: bool,
        targets: UsageCleanupTargets,
        mode: UsageCleanupExecutionMode,
    ) -> Result<UsageCleanupSummary, DataLayerError> {
        cleanup::cleanup_usage(
            &self.pool,
            window,
            batch_size,
            auto_delete_expired_keys,
            targets,
            mode,
        )
        .await
    }

    async fn preview_usage_cleanup(
        &self,
        window: &UsageCleanupWindow,
        targets: UsageCleanupTargets,
        mode: UsageCleanupExecutionMode,
    ) -> Result<UsageCleanupPreviewCounts, DataLayerError> {
        cleanup::preview_usage_cleanup(&self.pool, window, targets, mode).await
    }
}

struct StalePendingUsageRow {
    request_id: String,
    status: String,
    billing_status: String,
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
  billing_snapshot_schema_version = NULL,
  billing_snapshot_status = NULL,
  settlement_snapshot_schema_version = NULL,
  settlement_snapshot = NULL,
  billing_dimensions = NULL,
  billing_input_tokens = NULL,
  billing_effective_input_tokens = NULL,
  billing_output_tokens = NULL,
  billing_cache_creation_tokens = NULL,
  billing_cache_creation_5m_tokens = NULL,
  billing_cache_creation_1h_tokens = NULL,
  billing_cache_read_tokens = NULL,
  billing_total_input_context = NULL,
  billing_cache_creation_cost_usd = NULL,
  billing_cache_read_cost_usd = NULL,
  billing_total_cost_usd = NULL,
  billing_actual_total_cost_usd = NULL,
  billing_pricing_source = NULL,
  billing_rule_id = NULL,
  billing_rule_version = NULL,
  rate_multiplier = NULL,
  is_free_tier = NULL,
  input_price_per_1m = NULL,
  output_price_per_1m = NULL,
  cache_creation_price_per_1m = NULL,
  cache_read_price_per_1m = NULL,
  price_per_request = NULL,
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
        .unwrap_or(usage.updated_at_unix_secs);
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
        .bind(usage.outcome_class().as_str())
        .bind(usage.sla_eligible())
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

fn map_usage_row(
    row: &MySqlRow,
    resolve_legacy_compressed: bool,
) -> Result<StoredRequestUsageAudit, DataLayerError> {
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
    http_capture::hydrate_usage_row(row, &mut audit, resolve_legacy_compressed)?;
    let upstream_is_stream = row
        .try_get::<Option<bool>, _>("upstream_is_stream")
        .map_sql_err()?;
    merge_usage_stream_metadata(&mut audit.request_metadata, upstream_is_stream);
    snapshots::hydrate_row(row, &mut audit)?;
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
mod tests;
