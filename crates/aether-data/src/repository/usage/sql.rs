use aether_data_contracts::repository::usage::{
    parse_usage_body_ref, usage_body_ref, StoredUsageAuditAggregation, StoredUsageAuditSummary,
    StoredUsageLeaderboardSummary, StoredUsageTimeSeriesBucket, UsageAuditAggregationGroupBy,
    UsageAuditAggregationQuery, UsageAuditSummaryQuery, UsageBodyField, UsageLeaderboardGroupBy,
    UsageLeaderboardQuery, UsageTimeSeriesGranularity, UsageTimeSeriesQuery,
};
use async_trait::async_trait;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use futures_util::future::BoxFuture;
use futures_util::TryStreamExt;
use serde_json::Map;
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use std::io::{Read, Write};
use uuid::Uuid;

use super::{
    incoming_usage_can_recover_terminal_failure, strip_deprecated_usage_display_fields,
    StoredProviderApiKeyUsageSummary, StoredProviderUsageSummary, StoredRequestUsageAudit,
    StoredUsageDailySummary, UpsertUsageRecord, UsageAuditListQuery, UsageDailyHeatmapQuery,
    UsageReadRepository, UsageWriteRepository,
};
use crate::postgres::PostgresTransactionRunner;
use crate::{error::SqlxResultExt, DataLayerError};

// Legacy inline body columns on public.usage are deprecated. Keep the threshold at zero so
// newly captured bodies always spill to usage_body_blobs and resolve through usage_http_audits.
const MAX_INLINE_USAGE_BODY_BYTES: usize = 0;
const FIND_USAGE_BODY_BLOB_BY_REF_SQL: &str =
    r#"SELECT payload_gzip FROM usage_body_blobs WHERE body_ref = $1 LIMIT 1"#;
const UPSERT_USAGE_BODY_BLOB_SQL: &str = r#"
INSERT INTO usage_body_blobs (
  body_ref,
  request_id,
  body_field,
  payload_gzip
) VALUES (
  $1,
  $2,
  $3,
  $4
)
ON CONFLICT (body_ref)
DO UPDATE SET
  payload_gzip = EXCLUDED.payload_gzip,
  updated_at = NOW()
"#;
const DELETE_USAGE_BODY_BLOB_SQL: &str = r#"
DELETE FROM usage_body_blobs
WHERE body_ref = $1
"#;
const RESET_STALE_VOID_USAGE_SQL: &str = r#"
UPDATE "usage"
SET
  billing_status = 'pending',
  finalized_at = NULL
WHERE request_id = $1
  AND billing_status = 'void'
  AND status IN ('failed', 'cancelled')
"#;
const RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL: &str = r#"
UPDATE usage_settlement_snapshots
SET
  billing_status = 'pending',
  finalized_at = NULL,
  updated_at = NOW()
WHERE request_id = $1
  AND billing_status = 'void'
"#;
const UPSERT_USAGE_HTTP_AUDIT_SQL: &str = r#"
INSERT INTO usage_http_audits (
  request_id,
  request_headers,
  provider_request_headers,
  response_headers,
  client_response_headers,
  request_body_ref,
  provider_request_body_ref,
  response_body_ref,
  client_response_body_ref,
  body_capture_mode
) VALUES (
  $1,
  $2::json,
  $3::json,
  $4::json,
  $5::json,
  $6,
  $7,
  $8,
  $9,
  $10
)
ON CONFLICT (request_id)
DO UPDATE SET
  request_headers = COALESCE(EXCLUDED.request_headers, usage_http_audits.request_headers),
  provider_request_headers = COALESCE(
    EXCLUDED.provider_request_headers,
    usage_http_audits.provider_request_headers
  ),
  response_headers = COALESCE(EXCLUDED.response_headers, usage_http_audits.response_headers),
  client_response_headers = COALESCE(
    EXCLUDED.client_response_headers,
    usage_http_audits.client_response_headers
  ),
  request_body_ref = COALESCE(EXCLUDED.request_body_ref, usage_http_audits.request_body_ref),
  provider_request_body_ref = COALESCE(
    EXCLUDED.provider_request_body_ref,
    usage_http_audits.provider_request_body_ref
  ),
  response_body_ref = COALESCE(EXCLUDED.response_body_ref, usage_http_audits.response_body_ref),
  client_response_body_ref = COALESCE(
    EXCLUDED.client_response_body_ref,
    usage_http_audits.client_response_body_ref
  ),
  body_capture_mode = COALESCE(
    NULLIF(EXCLUDED.body_capture_mode, 'none'),
    usage_http_audits.body_capture_mode,
    'none'
  ),
  updated_at = NOW()
"#;
const UPSERT_USAGE_ROUTING_SNAPSHOT_SQL: &str = r#"
INSERT INTO usage_routing_snapshots (
  request_id,
  candidate_id,
  candidate_index,
  key_name,
  planner_kind,
  route_family,
  route_kind,
  execution_path,
  local_execution_runtime_miss_reason,
  selected_provider_id,
  selected_endpoint_id,
  selected_provider_api_key_id,
  has_format_conversion
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13
)
ON CONFLICT (request_id)
DO UPDATE SET
  candidate_id = COALESCE(EXCLUDED.candidate_id, usage_routing_snapshots.candidate_id),
  candidate_index = COALESCE(
    EXCLUDED.candidate_index,
    usage_routing_snapshots.candidate_index
  ),
  key_name = COALESCE(EXCLUDED.key_name, usage_routing_snapshots.key_name),
  planner_kind = COALESCE(EXCLUDED.planner_kind, usage_routing_snapshots.planner_kind),
  route_family = COALESCE(EXCLUDED.route_family, usage_routing_snapshots.route_family),
  route_kind = COALESCE(EXCLUDED.route_kind, usage_routing_snapshots.route_kind),
  execution_path = COALESCE(EXCLUDED.execution_path, usage_routing_snapshots.execution_path),
  local_execution_runtime_miss_reason = COALESCE(
    EXCLUDED.local_execution_runtime_miss_reason,
    usage_routing_snapshots.local_execution_runtime_miss_reason
  ),
  selected_provider_id = COALESCE(
    EXCLUDED.selected_provider_id,
    usage_routing_snapshots.selected_provider_id
  ),
  selected_endpoint_id = COALESCE(
    EXCLUDED.selected_endpoint_id,
    usage_routing_snapshots.selected_endpoint_id
  ),
  selected_provider_api_key_id = COALESCE(
    EXCLUDED.selected_provider_api_key_id,
    usage_routing_snapshots.selected_provider_api_key_id
  ),
  has_format_conversion = COALESCE(
    EXCLUDED.has_format_conversion,
    usage_routing_snapshots.has_format_conversion
  ),
  updated_at = NOW()
"#;
const UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL: &str = r#"
INSERT INTO usage_settlement_snapshots (
  request_id,
  billing_status,
  billing_snapshot_schema_version,
  billing_snapshot_status,
  rate_multiplier,
  is_free_tier,
  input_price_per_1m,
  output_price_per_1m,
  cache_creation_price_per_1m,
  cache_read_price_per_1m,
  price_per_request
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11
)
ON CONFLICT (request_id)
DO UPDATE SET
  billing_snapshot_schema_version = COALESCE(
    EXCLUDED.billing_snapshot_schema_version,
    usage_settlement_snapshots.billing_snapshot_schema_version
  ),
  billing_snapshot_status = COALESCE(
    EXCLUDED.billing_snapshot_status,
    usage_settlement_snapshots.billing_snapshot_status
  ),
  rate_multiplier = COALESCE(
    EXCLUDED.rate_multiplier,
    usage_settlement_snapshots.rate_multiplier
  ),
  is_free_tier = COALESCE(
    EXCLUDED.is_free_tier,
    usage_settlement_snapshots.is_free_tier
  ),
  input_price_per_1m = COALESCE(
    EXCLUDED.input_price_per_1m,
    usage_settlement_snapshots.input_price_per_1m
  ),
  output_price_per_1m = COALESCE(
    EXCLUDED.output_price_per_1m,
    usage_settlement_snapshots.output_price_per_1m
  ),
  cache_creation_price_per_1m = COALESCE(
    EXCLUDED.cache_creation_price_per_1m,
    usage_settlement_snapshots.cache_creation_price_per_1m
  ),
  cache_read_price_per_1m = COALESCE(
    EXCLUDED.cache_read_price_per_1m,
    usage_settlement_snapshots.cache_read_price_per_1m
  ),
  price_per_request = COALESCE(
    EXCLUDED.price_per_request,
    usage_settlement_snapshots.price_per_request
  ),
  updated_at = NOW()
"#;

const FIND_BY_REQUEST_ID_SQL: &str = r#"
SELECT
  "usage".id,
  "usage".request_id,
  "usage".user_id,
  "usage".api_key_id,
  "usage".username,
  "usage".api_key_name,
  "usage".provider_name,
  "usage".model,
  "usage".target_model,
  "usage".provider_id,
  "usage".provider_endpoint_id,
  "usage".provider_api_key_id,
  "usage".request_type,
  "usage".api_format,
  "usage".api_family,
  "usage".endpoint_kind,
  "usage".endpoint_api_format,
  "usage".provider_api_family,
  "usage".provider_endpoint_kind,
  COALESCE("usage".has_format_conversion, FALSE) AS has_format_conversion,
  COALESCE("usage".is_stream, FALSE) AS is_stream,
  "usage".input_tokens,
  "usage".output_tokens,
  "usage".total_tokens,
  COALESCE("usage".cache_creation_input_tokens, 0) AS cache_creation_input_tokens,
  COALESCE("usage".cache_creation_input_tokens_5m, 0) AS cache_creation_ephemeral_5m_input_tokens,
  COALESCE("usage".cache_creation_input_tokens_1h, 0) AS cache_creation_ephemeral_1h_input_tokens,
  COALESCE("usage".cache_read_input_tokens, 0) AS cache_read_input_tokens,
  COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0) AS cache_creation_cost_usd,
  COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0) AS cache_read_cost_usd,
  COALESCE(
    CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
    CAST("usage".output_price_per_1m AS DOUBLE PRECISION)
  ) AS output_price_per_1m,
  COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0) AS actual_total_cost_usd,
  "usage".status_code,
  "usage".error_message,
  "usage".error_category,
  "usage".response_time_ms,
  "usage".first_byte_time_ms,
  "usage".status,
  COALESCE(usage_settlement_snapshots.billing_status, "usage".billing_status) AS billing_status,
  COALESCE(usage_http_audits.request_headers, "usage".request_headers) AS request_headers,
  "usage".request_body,
  "usage".request_body_compressed,
  COALESCE(
    usage_http_audits.provider_request_headers,
    "usage".provider_request_headers
  ) AS provider_request_headers,
  "usage".provider_request_body,
  "usage".provider_request_body_compressed,
  COALESCE(usage_http_audits.response_headers, "usage".response_headers) AS response_headers,
  "usage".response_body,
  "usage".response_body_compressed,
  COALESCE(
    usage_http_audits.client_response_headers,
    "usage".client_response_headers
  ) AS client_response_headers,
  "usage".client_response_body,
  "usage".client_response_body_compressed,
  "usage".request_metadata,
  usage_http_audits.request_body_ref AS http_request_body_ref,
  usage_http_audits.provider_request_body_ref AS http_provider_request_body_ref,
  usage_http_audits.response_body_ref AS http_response_body_ref,
  usage_http_audits.client_response_body_ref AS http_client_response_body_ref,
  usage_routing_snapshots.candidate_id AS routing_candidate_id,
  usage_routing_snapshots.candidate_index AS routing_candidate_index,
  usage_routing_snapshots.key_name AS routing_key_name,
  usage_routing_snapshots.planner_kind AS routing_planner_kind,
  usage_routing_snapshots.route_family AS routing_route_family,
  usage_routing_snapshots.route_kind AS routing_route_kind,
  usage_routing_snapshots.execution_path AS routing_execution_path,
  usage_routing_snapshots.local_execution_runtime_miss_reason AS routing_local_execution_runtime_miss_reason,
  usage_settlement_snapshots.billing_snapshot_schema_version AS settlement_billing_snapshot_schema_version,
  usage_settlement_snapshots.billing_snapshot_status AS settlement_billing_snapshot_status,
  CAST(usage_settlement_snapshots.rate_multiplier AS DOUBLE PRECISION) AS settlement_rate_multiplier,
  usage_settlement_snapshots.is_free_tier AS settlement_is_free_tier,
  CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION) AS settlement_input_price_per_1m,
  CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION) AS settlement_output_price_per_1m,
  CAST(usage_settlement_snapshots.cache_creation_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_creation_price_per_1m,
  CAST(usage_settlement_snapshots.cache_read_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_read_price_per_1m,
  CAST(usage_settlement_snapshots.price_per_request AS DOUBLE PRECISION) AS settlement_price_per_request,
  CAST(EXTRACT(EPOCH FROM "usage".created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(
        usage_settlement_snapshots.finalized_at,
        "usage".finalized_at,
        "usage".created_at
      )
    ) AS BIGINT
  ) AS updated_at_unix_secs,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at)
    ) AS BIGINT
  ) AS finalized_at_unix_secs
FROM "usage"
LEFT JOIN usage_http_audits
  ON usage_http_audits.request_id = "usage".request_id
LEFT JOIN usage_routing_snapshots
  ON usage_routing_snapshots.request_id = "usage".request_id
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = "usage".request_id
WHERE "usage".request_id = $1
LIMIT 1
"#;

const FIND_BY_ID_SQL: &str = r#"
SELECT
  "usage".id,
  "usage".request_id,
  "usage".user_id,
  "usage".api_key_id,
  "usage".username,
  "usage".api_key_name,
  "usage".provider_name,
  "usage".model,
  "usage".target_model,
  "usage".provider_id,
  "usage".provider_endpoint_id,
  "usage".provider_api_key_id,
  "usage".request_type,
  "usage".api_format,
  "usage".api_family,
  "usage".endpoint_kind,
  "usage".endpoint_api_format,
  "usage".provider_api_family,
  "usage".provider_endpoint_kind,
  COALESCE("usage".has_format_conversion, FALSE) AS has_format_conversion,
  COALESCE("usage".is_stream, FALSE) AS is_stream,
  "usage".input_tokens,
  "usage".output_tokens,
  "usage".total_tokens,
  COALESCE("usage".cache_creation_input_tokens, 0) AS cache_creation_input_tokens,
  COALESCE("usage".cache_creation_input_tokens_5m, 0) AS cache_creation_ephemeral_5m_input_tokens,
  COALESCE("usage".cache_creation_input_tokens_1h, 0) AS cache_creation_ephemeral_1h_input_tokens,
  COALESCE("usage".cache_read_input_tokens, 0) AS cache_read_input_tokens,
  COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0) AS cache_creation_cost_usd,
  COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0) AS cache_read_cost_usd,
  COALESCE(
    CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
    CAST("usage".output_price_per_1m AS DOUBLE PRECISION)
  ) AS output_price_per_1m,
  COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0) AS actual_total_cost_usd,
  "usage".status_code,
  "usage".error_message,
  "usage".error_category,
  "usage".response_time_ms,
  "usage".first_byte_time_ms,
  "usage".status,
  COALESCE(usage_settlement_snapshots.billing_status, "usage".billing_status) AS billing_status,
  COALESCE(usage_http_audits.request_headers, "usage".request_headers) AS request_headers,
  "usage".request_body,
  "usage".request_body_compressed,
  COALESCE(
    usage_http_audits.provider_request_headers,
    "usage".provider_request_headers
  ) AS provider_request_headers,
  "usage".provider_request_body,
  "usage".provider_request_body_compressed,
  COALESCE(usage_http_audits.response_headers, "usage".response_headers) AS response_headers,
  "usage".response_body,
  "usage".response_body_compressed,
  COALESCE(
    usage_http_audits.client_response_headers,
    "usage".client_response_headers
  ) AS client_response_headers,
  "usage".client_response_body,
  "usage".client_response_body_compressed,
  "usage".request_metadata,
  usage_http_audits.request_body_ref AS http_request_body_ref,
  usage_http_audits.provider_request_body_ref AS http_provider_request_body_ref,
  usage_http_audits.response_body_ref AS http_response_body_ref,
  usage_http_audits.client_response_body_ref AS http_client_response_body_ref,
  usage_routing_snapshots.candidate_id AS routing_candidate_id,
  usage_routing_snapshots.candidate_index AS routing_candidate_index,
  usage_routing_snapshots.key_name AS routing_key_name,
  usage_routing_snapshots.planner_kind AS routing_planner_kind,
  usage_routing_snapshots.route_family AS routing_route_family,
  usage_routing_snapshots.route_kind AS routing_route_kind,
  usage_routing_snapshots.execution_path AS routing_execution_path,
  usage_routing_snapshots.local_execution_runtime_miss_reason AS routing_local_execution_runtime_miss_reason,
  usage_settlement_snapshots.billing_snapshot_schema_version AS settlement_billing_snapshot_schema_version,
  usage_settlement_snapshots.billing_snapshot_status AS settlement_billing_snapshot_status,
  CAST(usage_settlement_snapshots.rate_multiplier AS DOUBLE PRECISION) AS settlement_rate_multiplier,
  usage_settlement_snapshots.is_free_tier AS settlement_is_free_tier,
  CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION) AS settlement_input_price_per_1m,
  CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION) AS settlement_output_price_per_1m,
  CAST(usage_settlement_snapshots.cache_creation_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_creation_price_per_1m,
  CAST(usage_settlement_snapshots.cache_read_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_read_price_per_1m,
  CAST(usage_settlement_snapshots.price_per_request AS DOUBLE PRECISION) AS settlement_price_per_request,
  CAST(EXTRACT(EPOCH FROM "usage".created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(
        usage_settlement_snapshots.finalized_at,
        "usage".finalized_at,
        "usage".created_at
      )
    ) AS BIGINT
  ) AS updated_at_unix_secs,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at)
    ) AS BIGINT
  ) AS finalized_at_unix_secs
FROM "usage"
LEFT JOIN usage_http_audits
  ON usage_http_audits.request_id = "usage".request_id
LEFT JOIN usage_routing_snapshots
  ON usage_routing_snapshots.request_id = "usage".request_id
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = "usage".request_id
WHERE "usage".id = $1
LIMIT 1
"#;

const SUMMARIZE_PROVIDER_USAGE_SINCE_SQL: &str = r#"
SELECT
  COALESCE(SUM(total_requests), 0) AS total_requests,
  COALESCE(SUM(successful_requests), 0) AS successful_requests,
  COALESCE(SUM(failed_requests), 0) AS failed_requests,
  COALESCE(AVG(avg_response_time_ms), 0) AS avg_response_time_ms,
  COALESCE(SUM(total_cost_usd), 0) AS total_cost_usd
FROM provider_usage_tracking
WHERE provider_id = $1
  AND window_start >= TO_TIMESTAMP($2::double precision)
"#;

const SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL: &str = r#"
SELECT
  api_key_id,
  COALESCE(
    SUM(
      COALESCE(
        total_tokens,
        COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
      )
    ),
    0
  ) AS total_tokens
FROM "usage"
WHERE api_key_id = ANY($1::TEXT[])
GROUP BY api_key_id
ORDER BY api_key_id ASC
"#;

const SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL: &str = r#"
SELECT
  provider_api_key_id,
  COUNT(*)::BIGINT AS request_count,
  COALESCE(
    SUM(
      COALESCE(
        total_tokens,
        COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
      )
    ),
    0
  ) AS total_tokens,
  COALESCE(CAST(SUM(total_cost_usd) AS DOUBLE PRECISION), 0) AS total_cost_usd,
  CAST(EXTRACT(EPOCH FROM MAX(created_at)) AS BIGINT) AS last_used_at_unix_secs
FROM "usage"
WHERE provider_api_key_id = ANY($1::TEXT[])
GROUP BY provider_api_key_id
ORDER BY provider_api_key_id ASC
"#;

const LIST_USAGE_AUDITS_PREFIX: &str = r#"
SELECT
  "usage".id,
  "usage".request_id,
  "usage".user_id,
  "usage".api_key_id,
  "usage".username,
  "usage".api_key_name,
  "usage".provider_name,
  "usage".model,
  "usage".target_model,
  "usage".provider_id,
  "usage".provider_endpoint_id,
  "usage".provider_api_key_id,
  "usage".request_type,
  "usage".api_format,
  "usage".api_family,
  "usage".endpoint_kind,
  "usage".endpoint_api_format,
  "usage".provider_api_family,
  "usage".provider_endpoint_kind,
  COALESCE("usage".has_format_conversion, FALSE) AS has_format_conversion,
  COALESCE("usage".is_stream, FALSE) AS is_stream,
  "usage".input_tokens,
  "usage".output_tokens,
  "usage".total_tokens,
  COALESCE("usage".cache_creation_input_tokens, 0) AS cache_creation_input_tokens,
  COALESCE("usage".cache_creation_input_tokens_5m, 0) AS cache_creation_ephemeral_5m_input_tokens,
  COALESCE("usage".cache_creation_input_tokens_1h, 0) AS cache_creation_ephemeral_1h_input_tokens,
  COALESCE("usage".cache_read_input_tokens, 0) AS cache_read_input_tokens,
  COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0) AS cache_creation_cost_usd,
  COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0) AS cache_read_cost_usd,
  COALESCE(
    CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
    CAST("usage".output_price_per_1m AS DOUBLE PRECISION)
  ) AS output_price_per_1m,
  COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0) AS actual_total_cost_usd,
  "usage".status_code,
  "usage".error_message,
  "usage".error_category,
  "usage".response_time_ms,
  "usage".first_byte_time_ms,
  "usage".status,
  COALESCE(usage_settlement_snapshots.billing_status, "usage".billing_status) AS billing_status,
  NULL::json AS request_headers,
  NULL::json AS request_body,
  NULL::bytea AS request_body_compressed,
  NULL::json AS provider_request_headers,
  NULL::json AS provider_request_body,
  NULL::bytea AS provider_request_body_compressed,
  NULL::json AS response_headers,
  NULL::json AS response_body,
  NULL::bytea AS response_body_compressed,
  NULL::json AS client_response_headers,
  NULL::json AS client_response_body,
  NULL::bytea AS client_response_body_compressed,
  NULL::json AS request_metadata,
  NULL::varchar AS http_request_body_ref,
  NULL::varchar AS http_provider_request_body_ref,
  NULL::varchar AS http_response_body_ref,
  NULL::varchar AS http_client_response_body_ref,
  NULL::varchar AS routing_candidate_id,
  NULL::integer AS routing_candidate_index,
  NULL::varchar AS routing_key_name,
  NULL::varchar AS routing_planner_kind,
  NULL::varchar AS routing_route_family,
  NULL::varchar AS routing_route_kind,
  NULL::varchar AS routing_execution_path,
  NULL::varchar AS routing_local_execution_runtime_miss_reason,
  usage_settlement_snapshots.billing_snapshot_schema_version AS settlement_billing_snapshot_schema_version,
  usage_settlement_snapshots.billing_snapshot_status AS settlement_billing_snapshot_status,
  CAST(usage_settlement_snapshots.rate_multiplier AS DOUBLE PRECISION) AS settlement_rate_multiplier,
  usage_settlement_snapshots.is_free_tier AS settlement_is_free_tier,
  CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION) AS settlement_input_price_per_1m,
  CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION) AS settlement_output_price_per_1m,
  CAST(usage_settlement_snapshots.cache_creation_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_creation_price_per_1m,
  CAST(usage_settlement_snapshots.cache_read_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_read_price_per_1m,
  CAST(usage_settlement_snapshots.price_per_request AS DOUBLE PRECISION) AS settlement_price_per_request,
  CAST(EXTRACT(EPOCH FROM "usage".created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(
        usage_settlement_snapshots.finalized_at,
        "usage".finalized_at,
        "usage".created_at
      )
    ) AS BIGINT
  ) AS updated_at_unix_secs,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at)
    ) AS BIGINT
  ) AS finalized_at_unix_secs
FROM "usage"
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = "usage".request_id
"#;

struct UsageAuditAggregationSqlFragments {
    filtered_extra_where: &'static str,
    group_key_expr: &'static str,
    display_name_expr: &'static str,
    secondary_name_expr: &'static str,
    aggregate_display_name_expr: &'static str,
    aggregate_secondary_name_expr: &'static str,
    avg_response_time_expr: &'static str,
    success_count_expr: &'static str,
}

fn usage_audit_aggregation_sql_fragments(
    group_by: UsageAuditAggregationGroupBy,
) -> UsageAuditAggregationSqlFragments {
    match group_by {
        UsageAuditAggregationGroupBy::Model => UsageAuditAggregationSqlFragments {
            filtered_extra_where: "",
            group_key_expr: "model",
            display_name_expr: "NULL::varchar",
            secondary_name_expr: "NULL::varchar",
            aggregate_display_name_expr: "NULL::varchar",
            aggregate_secondary_name_expr: "NULL::varchar",
            avg_response_time_expr: "NULL::DOUBLE PRECISION",
            success_count_expr: "NULL::BIGINT",
        },
        UsageAuditAggregationGroupBy::Provider => UsageAuditAggregationSqlFragments {
            filtered_extra_where: "",
            group_key_expr: "provider_group_key",
            display_name_expr: "provider_display_name",
            secondary_name_expr: "NULL::varchar",
            aggregate_display_name_expr:
                "COALESCE(MAX(NULLIF(display_name, 'Unknown')), 'Unknown')",
            aggregate_secondary_name_expr: "NULL::varchar",
            avg_response_time_expr: "AVG(response_time_ms::DOUBLE PRECISION)",
            success_count_expr: "COALESCE(SUM(success_flag), 0)::BIGINT",
        },
        UsageAuditAggregationGroupBy::ApiFormat => UsageAuditAggregationSqlFragments {
            filtered_extra_where: "",
            group_key_expr: "api_format_group_key",
            display_name_expr: "NULL::varchar",
            secondary_name_expr: "NULL::varchar",
            aggregate_display_name_expr: "NULL::varchar",
            aggregate_secondary_name_expr: "NULL::varchar",
            avg_response_time_expr: "AVG(response_time_ms::DOUBLE PRECISION)",
            success_count_expr: "NULL::BIGINT",
        },
        UsageAuditAggregationGroupBy::User => UsageAuditAggregationSqlFragments {
            filtered_extra_where: " AND \"usage\".user_id IS NOT NULL",
            group_key_expr: "user_id",
            display_name_expr: "NULL::varchar",
            secondary_name_expr: "NULL::varchar",
            aggregate_display_name_expr: "NULL::varchar",
            aggregate_secondary_name_expr: "NULL::varchar",
            avg_response_time_expr: "NULL::DOUBLE PRECISION",
            success_count_expr: "NULL::BIGINT",
        },
    }
}

struct UsageLeaderboardSqlFragments {
    filtered_extra_where: &'static str,
    group_key_expr: &'static str,
    legacy_name_expr: &'static str,
}

fn usage_leaderboard_sql_fragments(
    group_by: UsageLeaderboardGroupBy,
) -> UsageLeaderboardSqlFragments {
    match group_by {
        UsageLeaderboardGroupBy::Model => UsageLeaderboardSqlFragments {
            filtered_extra_where: "",
            group_key_expr: "\"usage\".model",
            legacy_name_expr: "NULL::varchar",
        },
        UsageLeaderboardGroupBy::User => UsageLeaderboardSqlFragments {
            filtered_extra_where: " AND \"usage\".user_id IS NOT NULL",
            group_key_expr: "\"usage\".user_id",
            legacy_name_expr: "NULLIF(BTRIM(\"usage\".username), '')",
        },
        UsageLeaderboardGroupBy::ApiKey => UsageLeaderboardSqlFragments {
            filtered_extra_where: " AND \"usage\".api_key_id IS NOT NULL",
            group_key_expr: "\"usage\".api_key_id",
            legacy_name_expr: "NULLIF(BTRIM(\"usage\".api_key_name), '')",
        },
    }
}

const LIST_RECENT_USAGE_AUDITS_PREFIX: &str = r#"
SELECT
  "usage".id,
  "usage".request_id,
  "usage".user_id,
  "usage".api_key_id,
  "usage".username,
  "usage".api_key_name,
  "usage".provider_name,
  "usage".model,
  "usage".target_model,
  "usage".provider_id,
  "usage".provider_endpoint_id,
  "usage".provider_api_key_id,
  "usage".request_type,
  "usage".api_format,
  "usage".api_family,
  "usage".endpoint_kind,
  "usage".endpoint_api_format,
  "usage".provider_api_family,
  "usage".provider_endpoint_kind,
  COALESCE("usage".has_format_conversion, FALSE) AS has_format_conversion,
  COALESCE("usage".is_stream, FALSE) AS is_stream,
  "usage".input_tokens,
  "usage".output_tokens,
  "usage".total_tokens,
  COALESCE("usage".cache_creation_input_tokens, 0) AS cache_creation_input_tokens,
  COALESCE("usage".cache_creation_input_tokens_5m, 0) AS cache_creation_ephemeral_5m_input_tokens,
  COALESCE("usage".cache_creation_input_tokens_1h, 0) AS cache_creation_ephemeral_1h_input_tokens,
  COALESCE("usage".cache_read_input_tokens, 0) AS cache_read_input_tokens,
  COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0) AS cache_creation_cost_usd,
  COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0) AS cache_read_cost_usd,
  COALESCE(
    CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
    CAST("usage".output_price_per_1m AS DOUBLE PRECISION)
  ) AS output_price_per_1m,
  COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0) AS actual_total_cost_usd,
  "usage".status_code,
  "usage".error_message,
  "usage".error_category,
  "usage".response_time_ms,
  "usage".first_byte_time_ms,
  "usage".status,
  COALESCE(usage_settlement_snapshots.billing_status, "usage".billing_status) AS billing_status,
  NULL::json AS request_headers,
  NULL::json AS request_body,
  NULL::bytea AS request_body_compressed,
  NULL::json AS provider_request_headers,
  NULL::json AS provider_request_body,
  NULL::bytea AS provider_request_body_compressed,
  NULL::json AS response_headers,
  NULL::json AS response_body,
  NULL::bytea AS response_body_compressed,
  NULL::json AS client_response_headers,
  NULL::json AS client_response_body,
  NULL::bytea AS client_response_body_compressed,
  NULL::json AS request_metadata,
  NULL::varchar AS http_request_body_ref,
  NULL::varchar AS http_provider_request_body_ref,
  NULL::varchar AS http_response_body_ref,
  NULL::varchar AS http_client_response_body_ref,
  NULL::varchar AS routing_candidate_id,
  NULL::integer AS routing_candidate_index,
  NULL::varchar AS routing_key_name,
  NULL::varchar AS routing_planner_kind,
  NULL::varchar AS routing_route_family,
  NULL::varchar AS routing_route_kind,
  NULL::varchar AS routing_execution_path,
  NULL::varchar AS routing_local_execution_runtime_miss_reason,
  usage_settlement_snapshots.billing_snapshot_schema_version AS settlement_billing_snapshot_schema_version,
  usage_settlement_snapshots.billing_snapshot_status AS settlement_billing_snapshot_status,
  CAST(usage_settlement_snapshots.rate_multiplier AS DOUBLE PRECISION) AS settlement_rate_multiplier,
  usage_settlement_snapshots.is_free_tier AS settlement_is_free_tier,
  CAST(usage_settlement_snapshots.input_price_per_1m AS DOUBLE PRECISION) AS settlement_input_price_per_1m,
  CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION) AS settlement_output_price_per_1m,
  CAST(usage_settlement_snapshots.cache_creation_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_creation_price_per_1m,
  CAST(usage_settlement_snapshots.cache_read_price_per_1m AS DOUBLE PRECISION) AS settlement_cache_read_price_per_1m,
  CAST(usage_settlement_snapshots.price_per_request AS DOUBLE PRECISION) AS settlement_price_per_request,
  CAST(EXTRACT(EPOCH FROM "usage".created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(
        usage_settlement_snapshots.finalized_at,
        "usage".finalized_at,
        "usage".created_at
      )
    ) AS BIGINT
  ) AS updated_at_unix_secs,
  CAST(
    EXTRACT(
      EPOCH FROM COALESCE(usage_settlement_snapshots.finalized_at, "usage".finalized_at)
    ) AS BIGINT
  ) AS finalized_at_unix_secs
FROM "usage"
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = "usage".request_id
"#;

const UPSERT_SQL: &str = r#"
INSERT INTO "usage" (
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
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
  input_tokens,
  output_tokens,
  total_tokens,
  cache_creation_input_tokens,
  cache_creation_input_tokens_5m,
  cache_creation_input_tokens_1h,
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
  request_headers,
  request_body,
  request_body_compressed,
  provider_request_headers,
  provider_request_body,
  provider_request_body_compressed,
  response_headers,
  response_body,
  response_body_compressed,
  client_response_headers,
  client_response_body,
  client_response_body_compressed,
  request_metadata,
  finalized_at,
  created_at
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  $8,
  $9,
  $10,
  $11,
  $12,
  $13,
  $14,
  $15,
  $16,
  $17,
  $18,
  $19,
  COALESCE($20, FALSE),
  COALESCE($21, FALSE),
  COALESCE($22, 0),
  COALESCE($23, 0),
  COALESCE($24, COALESCE($22, 0) + COALESCE($23, 0)),
  COALESCE($25, 0),
  COALESCE($26, 0),
  COALESCE($27, 0),
  COALESCE($28, 0),
  COALESCE($29, 0),
  $30,
  COALESCE($31, 0),
  COALESCE($32, 0),
  $33,
  $34,
  $35,
  $36,
  $37,
  $38,
  $39,
  $40,
  $41::json,
  $42::json,
  $43,
  $44::json,
  $45::json,
  $46,
  $47::json,
  $48::json,
  $49,
  $50::json,
  $51::json,
  $52,
  $53::json,
  CASE
    WHEN $54 IS NULL THEN NULL
    ELSE TO_TIMESTAMP($54::double precision)
  END,
  COALESCE(TO_TIMESTAMP($55::double precision), NOW())
)
ON CONFLICT (request_id)
DO UPDATE SET
  user_id = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.user_id, "usage".user_id) ELSE "usage".user_id END,
  api_key_id = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.api_key_id, "usage".api_key_id) ELSE "usage".api_key_id END,
  username = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.username, "usage".username) ELSE "usage".username END,
  api_key_name = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.api_key_name, "usage".api_key_name) ELSE "usage".api_key_name END,
  provider_name = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.provider_name, "usage".provider_name) ELSE "usage".provider_name END,
  model = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.model, "usage".model) ELSE "usage".model END,
  target_model = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.target_model, "usage".target_model) ELSE "usage".target_model END,
  provider_id = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.provider_id, "usage".provider_id) ELSE "usage".provider_id END,
  provider_endpoint_id = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.provider_endpoint_id, "usage".provider_endpoint_id) ELSE "usage".provider_endpoint_id END,
  provider_api_key_id = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.provider_api_key_id, "usage".provider_api_key_id) ELSE "usage".provider_api_key_id END,
  request_type = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.request_type, "usage".request_type) ELSE "usage".request_type END,
  api_format = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.api_format, "usage".api_format) ELSE "usage".api_format END,
  api_family = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.api_family, "usage".api_family) ELSE "usage".api_family END,
  endpoint_kind = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.endpoint_kind, "usage".endpoint_kind) ELSE "usage".endpoint_kind END,
  endpoint_api_format = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.endpoint_api_format, "usage".endpoint_api_format) ELSE "usage".endpoint_api_format END,
  provider_api_family = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.provider_api_family, "usage".provider_api_family) ELSE "usage".provider_api_family END,
  provider_endpoint_kind = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.provider_endpoint_kind, "usage".provider_endpoint_kind) ELSE "usage".provider_endpoint_kind END,
  has_format_conversion = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.has_format_conversion, "usage".has_format_conversion) ELSE "usage".has_format_conversion END,
  is_stream = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.is_stream, "usage".is_stream) ELSE "usage".is_stream END,
  input_tokens = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.input_tokens, "usage".input_tokens) ELSE "usage".input_tokens END,
  output_tokens = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.output_tokens, "usage".output_tokens) ELSE "usage".output_tokens END,
  total_tokens = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.total_tokens, "usage".total_tokens) ELSE "usage".total_tokens END,
  cache_creation_input_tokens = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.cache_creation_input_tokens, "usage".cache_creation_input_tokens) ELSE "usage".cache_creation_input_tokens END,
  cache_creation_input_tokens_5m = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.cache_creation_input_tokens_5m, "usage".cache_creation_input_tokens_5m) ELSE "usage".cache_creation_input_tokens_5m END,
  cache_creation_input_tokens_1h = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.cache_creation_input_tokens_1h, "usage".cache_creation_input_tokens_1h) ELSE "usage".cache_creation_input_tokens_1h END,
  cache_read_input_tokens = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.cache_read_input_tokens, "usage".cache_read_input_tokens) ELSE "usage".cache_read_input_tokens END,
  cache_creation_cost_usd = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.cache_creation_cost_usd, "usage".cache_creation_cost_usd) ELSE "usage".cache_creation_cost_usd END,
  cache_read_cost_usd = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.cache_read_cost_usd, "usage".cache_read_cost_usd) ELSE "usage".cache_read_cost_usd END,
  output_price_per_1m = NULL,
  total_cost_usd = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.total_cost_usd, "usage".total_cost_usd) ELSE "usage".total_cost_usd END,
  actual_total_cost_usd = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.actual_total_cost_usd, "usage".actual_total_cost_usd) ELSE "usage".actual_total_cost_usd END,
  status_code = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN "usage".status = 'streaming' AND EXCLUDED.status = 'pending' THEN "usage".status_code
    WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') AND EXCLUDED.status_code IS NULL THEN NULL
    ELSE COALESCE(EXCLUDED.status_code, "usage".status_code)
  END ELSE "usage".status_code END,
  error_message = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN "usage".status = 'streaming' AND EXCLUDED.status = 'pending' THEN "usage".error_message
    WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') THEN EXCLUDED.error_message
    ELSE COALESCE(EXCLUDED.error_message, "usage".error_message)
  END ELSE "usage".error_message END,
  error_category = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN "usage".status = 'streaming' AND EXCLUDED.status = 'pending' THEN "usage".error_category
    WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') THEN EXCLUDED.error_category
    ELSE COALESCE(EXCLUDED.error_category, "usage".error_category)
  END ELSE "usage".error_category END,
  response_time_ms = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.response_time_ms, "usage".response_time_ms) ELSE "usage".response_time_ms END,
  first_byte_time_ms = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.first_byte_time_ms, "usage".first_byte_time_ms) ELSE "usage".first_byte_time_ms END,
  status = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN "usage".status = 'streaming' AND EXCLUDED.status = 'pending' THEN "usage".status
    ELSE EXCLUDED.status
  END ELSE "usage".status END,
  billing_status = CASE WHEN "usage".billing_status = 'pending' THEN EXCLUDED.billing_status ELSE "usage".billing_status END,
  request_headers = NULL,
  request_body = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.request_body_compressed IS NOT NULL OR $56 THEN NULL
    ELSE COALESCE(EXCLUDED.request_body, "usage".request_body)
  END ELSE "usage".request_body END,
  request_body_compressed = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.request_body IS NOT NULL OR $56 THEN NULL
    ELSE COALESCE(EXCLUDED.request_body_compressed, "usage".request_body_compressed)
  END ELSE "usage".request_body_compressed END,
  provider_request_headers = NULL,
  provider_request_body = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.provider_request_body_compressed IS NOT NULL OR $57 THEN NULL
    ELSE COALESCE(EXCLUDED.provider_request_body, "usage".provider_request_body)
  END ELSE "usage".provider_request_body END,
  provider_request_body_compressed = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.provider_request_body IS NOT NULL OR $57 THEN NULL
    ELSE COALESCE(EXCLUDED.provider_request_body_compressed, "usage".provider_request_body_compressed)
  END ELSE "usage".provider_request_body_compressed END,
  response_headers = NULL,
  response_body = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.response_body_compressed IS NOT NULL OR $58 THEN NULL
    ELSE COALESCE(EXCLUDED.response_body, "usage".response_body)
  END ELSE "usage".response_body END,
  response_body_compressed = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.response_body IS NOT NULL OR $58 THEN NULL
    ELSE COALESCE(EXCLUDED.response_body_compressed, "usage".response_body_compressed)
  END ELSE "usage".response_body_compressed END,
  client_response_headers = NULL,
  client_response_body = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.client_response_body_compressed IS NOT NULL OR $59 THEN NULL
    ELSE COALESCE(EXCLUDED.client_response_body, "usage".client_response_body)
  END ELSE "usage".client_response_body END,
  client_response_body_compressed = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.client_response_body IS NOT NULL OR $59 THEN NULL
    ELSE COALESCE(EXCLUDED.client_response_body_compressed, "usage".client_response_body_compressed)
  END ELSE "usage".client_response_body_compressed END,
  request_metadata = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.request_metadata, "usage".request_metadata) ELSE "usage".request_metadata END,
  finalized_at = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.finalized_at, "usage".finalized_at) ELSE "usage".finalized_at END
RETURNING
  id,
  request_id,
  user_id,
  api_key_id,
  username,
  api_key_name,
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
  COALESCE(has_format_conversion, FALSE) AS has_format_conversion,
  COALESCE(is_stream, FALSE) AS is_stream,
  input_tokens,
  output_tokens,
  total_tokens,
  COALESCE(cache_creation_input_tokens, 0) AS cache_creation_input_tokens,
  COALESCE(cache_creation_input_tokens_5m, 0) AS cache_creation_ephemeral_5m_input_tokens,
  COALESCE(cache_creation_input_tokens_1h, 0) AS cache_creation_ephemeral_1h_input_tokens,
  COALESCE(cache_read_input_tokens, 0) AS cache_read_input_tokens,
  COALESCE(CAST(cache_creation_cost_usd AS DOUBLE PRECISION), 0) AS cache_creation_cost_usd,
  COALESCE(CAST(cache_read_cost_usd AS DOUBLE PRECISION), 0) AS cache_read_cost_usd,
  CAST(output_price_per_1m AS DOUBLE PRECISION) AS output_price_per_1m,
  COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
  COALESCE(CAST(actual_total_cost_usd AS DOUBLE PRECISION), 0) AS actual_total_cost_usd,
  status_code,
  error_message,
  error_category,
  response_time_ms,
  first_byte_time_ms,
  status,
  billing_status,
  request_headers,
  request_body,
  request_body_compressed,
  provider_request_headers,
  provider_request_body,
  provider_request_body_compressed,
  response_headers,
  response_body,
  response_body_compressed,
  client_response_headers,
  client_response_body,
  client_response_body_compressed,
  request_metadata,
  NULL::varchar AS http_request_body_ref,
  NULL::varchar AS http_provider_request_body_ref,
  NULL::varchar AS http_response_body_ref,
  NULL::varchar AS http_client_response_body_ref,
  NULL::varchar AS routing_candidate_id,
  NULL::integer AS routing_candidate_index,
  NULL::varchar AS routing_key_name,
  NULL::varchar AS routing_planner_kind,
  NULL::varchar AS routing_route_family,
  NULL::varchar AS routing_route_kind,
  NULL::varchar AS routing_execution_path,
  NULL::varchar AS routing_local_execution_runtime_miss_reason,
  NULL::varchar AS settlement_billing_snapshot_schema_version,
  NULL::varchar AS settlement_billing_snapshot_status,
  NULL::double precision AS settlement_rate_multiplier,
  NULL::boolean AS settlement_is_free_tier,
  NULL::double precision AS settlement_input_price_per_1m,
  NULL::double precision AS settlement_output_price_per_1m,
  NULL::double precision AS settlement_cache_creation_price_per_1m,
  NULL::double precision AS settlement_cache_read_price_per_1m,
  NULL::double precision AS settlement_price_per_request,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM COALESCE(finalized_at, created_at)) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT) AS finalized_at_unix_secs
"#;

#[derive(Debug, Clone)]
pub struct SqlxUsageReadRepository {
    pool: PgPool,
    tx_runner: PostgresTransactionRunner,
}

impl SqlxUsageReadRepository {
    pub fn new(pool: PgPool) -> Self {
        let tx_runner = PostgresTransactionRunner::new(pool.clone());
        Self { pool, tx_runner }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn transaction_runner(&self) -> &PostgresTransactionRunner {
        &self.tx_runner
    }

    pub async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(FIND_BY_REQUEST_ID_SQL)
            .bind(request_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        let usage = row
            .as_ref()
            .map(|row| map_usage_row(row, true))
            .transpose()?;
        match usage {
            Some(usage) => self.hydrate_usage_body_refs(usage).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(FIND_BY_ID_SQL)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref()
            .map(|row| map_usage_row(row, false))
            .transpose()
    }

    pub async fn resolve_body_ref(&self, body_ref: &str) -> Result<Option<Value>, DataLayerError> {
        let blob_row = sqlx::query(FIND_USAGE_BODY_BLOB_BY_REF_SQL)
            .bind(body_ref)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        if let Some(row) = blob_row.as_ref() {
            let payload_gzip = row
                .try_get::<Vec<u8>, _>("payload_gzip")
                .map_postgres_err()?;
            return inflate_usage_json_value(&payload_gzip).map(Some);
        }
        let Some((request_id, field)) = parse_usage_body_ref(body_ref) else {
            return Ok(None);
        };
        let (inline_column, compressed_column) = usage_body_sql_columns(field);
        let row = sqlx::query(&format!(
            "SELECT {inline_column} AS inline_body, {compressed_column} AS compressed_body FROM \"usage\" WHERE request_id = $1 LIMIT 1"
        ))
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref()
            .map(|row| usage_json_column(row, "inline_body", "compressed_body", true))
            .transpose()
            .map(|value| value.and_then(|column| column.value))
    }

    async fn hydrate_usage_body_refs(
        &self,
        mut usage: StoredRequestUsageAudit,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        if usage.request_body.is_none() {
            usage.request_body = self
                .resolve_usage_body_ref(&usage, UsageBodyField::RequestBody)
                .await?;
        }
        if usage.provider_request_body.is_none() {
            usage.provider_request_body = self
                .resolve_usage_body_ref(&usage, UsageBodyField::ProviderRequestBody)
                .await?;
        }
        if usage.response_body.is_none() {
            usage.response_body = self
                .resolve_usage_body_ref(&usage, UsageBodyField::ResponseBody)
                .await?;
        }
        if usage.client_response_body.is_none() {
            usage.client_response_body = self
                .resolve_usage_body_ref(&usage, UsageBodyField::ClientResponseBody)
                .await?;
        }
        Ok(usage)
    }

    async fn resolve_usage_body_ref(
        &self,
        usage: &StoredRequestUsageAudit,
        field: UsageBodyField,
    ) -> Result<Option<Value>, DataLayerError> {
        let body_ref = usage.body_ref(field);
        match body_ref {
            Some(body_ref) => self.resolve_body_ref(body_ref).await,
            None => Ok(None),
        }
    }

    pub async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, DataLayerError> {
        let row = sqlx::query(SUMMARIZE_PROVIDER_USAGE_SINCE_SQL)
            .bind(provider_id)
            .bind(since_unix_secs as f64)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;

        Ok(StoredProviderUsageSummary {
            total_requests: row
                .try_get::<i64, _>("total_requests")
                .map_postgres_err()?
                .max(0) as u64,
            successful_requests: row
                .try_get::<i64, _>("successful_requests")
                .map_postgres_err()?
                .max(0) as u64,
            failed_requests: row
                .try_get::<i64, _>("failed_requests")
                .map_postgres_err()?
                .max(0) as u64,
            avg_response_time_ms: row
                .try_get::<f64, _>("avg_response_time_ms")
                .map_postgres_err()?,
            total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
        })
    }

    pub async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_USAGE_AUDITS_PREFIX);
        let mut has_where = false;

        if let Some(created_from_unix_secs) = query.created_from_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at >= TO_TIMESTAMP(")
                .push_bind(created_from_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(created_until_unix_secs) = query.created_until_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at < TO_TIMESTAMP(")
                .push_bind(created_until_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }
        if let Some(api_format) = query.api_format.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".api_format = ")
                .push_bind(api_format.to_string());
        }
        if let Some(statuses) = query.statuses.as_deref() {
            if !statuses.is_empty() {
                builder.push(if has_where { " AND " } else { " WHERE " });
                has_where = true;
                builder.push("\"usage\".status IN (");
                let mut separated = builder.separated(", ");
                for status in statuses {
                    separated.push_bind(status.to_string());
                }
                separated.push_unseparated(")");
            }
        }
        if let Some(is_stream) = query.is_stream {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("\"usage\".is_stream = ").push_bind(is_stream);
        }
        if query.error_only {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder.push(
                "(\"usage\".status = 'failed' \
OR COALESCE(\"usage\".status_code, 0) >= 400 \
OR (\"usage\".error_message IS NOT NULL AND BTRIM(\"usage\".error_message) <> ''))",
            );
        }

        if query.newest_first {
            builder.push(" ORDER BY \"usage\".created_at DESC, \"usage\".id ASC");
        } else {
            builder.push(" ORDER BY \"usage\".created_at ASC, \"usage\".request_id ASC");
        }
        if let Some(limit) = query.limit {
            builder.push(" LIMIT ").push_bind(limit as i64);
        }
        if let Some(offset) = query.offset {
            builder.push(" OFFSET ").push_bind(offset as i64);
        }
        let query = builder.build();
        let mut rows = query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row, false)?);
        }
        Ok(items)
    }

    pub async fn count_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<Postgres>::new(r#"SELECT COUNT(*)::BIGINT AS total FROM "usage""#);
        let mut has_where = false;

        if let Some(created_from_unix_secs) = query.created_from_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at >= TO_TIMESTAMP(")
                .push_bind(created_from_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(created_until_unix_secs) = query.created_until_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at < TO_TIMESTAMP(")
                .push_bind(created_until_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }
        if let Some(api_format) = query.api_format.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".api_format = ")
                .push_bind(api_format.to_string());
        }
        if let Some(statuses) = query.statuses.as_deref() {
            if !statuses.is_empty() {
                builder.push(if has_where { " AND " } else { " WHERE " });
                has_where = true;
                builder.push("\"usage\".status IN (");
                let mut separated = builder.separated(", ");
                for status in statuses {
                    separated.push_bind(status.to_string());
                }
                separated.push_unseparated(")");
            }
        }
        if let Some(is_stream) = query.is_stream {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("\"usage\".is_stream = ").push_bind(is_stream);
        }
        if query.error_only {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder.push(
                "(\"usage\".status = 'failed' \
OR COALESCE(\"usage\".status_code, 0) >= 400 \
OR (\"usage\".error_message IS NOT NULL AND BTRIM(\"usage\".error_message) <> ''))",
            );
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64)
    }

    pub async fn summarize_usage_audits(
        &self,
        query: &UsageAuditSummaryQuery,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  COUNT(*)::BIGINT AS total_requests,
  COALESCE(SUM(GREATEST(COALESCE("usage".input_tokens, 0), 0)), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".output_tokens, 0), 0)), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".total_tokens, 0), 0)), 0)::BIGINT AS recorded_total_tokens,
  COALESCE(SUM(
    CASE
      WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
           AND (
             COALESCE("usage".cache_creation_input_tokens_5m, 0)
             + COALESCE("usage".cache_creation_input_tokens_1h, 0)
           ) > 0
      THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
         + COALESCE("usage".cache_creation_input_tokens_1h, 0)
      ELSE COALESCE("usage".cache_creation_input_tokens, 0)
    END
  ), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_creation_input_tokens_5m, 0), 0)), 0)::BIGINT
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_creation_input_tokens_1h, 0), 0)), 0)::BIGINT
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS actual_total_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_creation_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_read_cost_usd,
  COALESCE(SUM(GREATEST(COALESCE("usage".response_time_ms, 0), 0)::DOUBLE PRECISION), 0)
    AS total_response_time_ms,
  COALESCE(SUM(
    CASE
      WHEN COALESCE("usage".status_code, 0) >= 400 OR "usage".error_message IS NOT NULL THEN 1
      ELSE 0
    END
  ), 0)::BIGINT AS error_requests
FROM "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(StoredUsageAuditSummary {
            total_requests: row
                .try_get::<i64, _>("total_requests")
                .map_postgres_err()?
                .max(0) as u64,
            input_tokens: row
                .try_get::<i64, _>("input_tokens")
                .map_postgres_err()?
                .max(0) as u64,
            output_tokens: row
                .try_get::<i64, _>("output_tokens")
                .map_postgres_err()?
                .max(0) as u64,
            recorded_total_tokens: row
                .try_get::<i64, _>("recorded_total_tokens")
                .map_postgres_err()?
                .max(0) as u64,
            cache_creation_tokens: row
                .try_get::<i64, _>("cache_creation_tokens")
                .map_postgres_err()?
                .max(0) as u64,
            cache_creation_ephemeral_5m_tokens: row
                .try_get::<i64, _>("cache_creation_ephemeral_5m_tokens")
                .map_postgres_err()?
                .max(0) as u64,
            cache_creation_ephemeral_1h_tokens: row
                .try_get::<i64, _>("cache_creation_ephemeral_1h_tokens")
                .map_postgres_err()?
                .max(0) as u64,
            cache_read_tokens: row
                .try_get::<i64, _>("cache_read_tokens")
                .map_postgres_err()?
                .max(0) as u64,
            total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
            actual_total_cost_usd: row
                .try_get::<f64, _>("actual_total_cost_usd")
                .map_postgres_err()?,
            cache_creation_cost_usd: row
                .try_get::<f64, _>("cache_creation_cost_usd")
                .map_postgres_err()?,
            cache_read_cost_usd: row
                .try_get::<f64, _>("cache_read_cost_usd")
                .map_postgres_err()?,
            total_response_time_ms: row
                .try_get::<f64, _>("total_response_time_ms")
                .map_postgres_err()?,
            error_requests: row
                .try_get::<i64, _>("error_requests")
                .map_postgres_err()?
                .max(0) as u64,
        })
    }

    pub async fn summarize_usage_time_series(
        &self,
        query: &UsageTimeSeriesQuery,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        match query.granularity {
            UsageTimeSeriesGranularity::Day => {
                builder
                    .push("TO_CHAR(date_trunc('day', \"usage\".created_at + (")
                    .push_bind(query.tz_offset_minutes)
                    .push("::integer * INTERVAL '1 minute')), 'YYYY-MM-DD') AS bucket_key");
            }
            UsageTimeSeriesGranularity::Hour => {
                builder
                    .push("TO_CHAR(date_trunc('hour', \"usage\".created_at + (")
                    .push_bind(query.tz_offset_minutes)
                    .push("::integer * INTERVAL '1 minute')), 'YYYY-MM-DD\"T\"HH24:00:00+00:00') AS bucket_key");
            }
        }
        builder.push(
            r#",
  COUNT(*)::BIGINT AS total_requests,
  COALESCE(SUM(GREATEST(COALESCE("usage".input_tokens, 0), 0)), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".output_tokens, 0), 0)), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_creation_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_creation_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd,
  COALESCE(SUM(GREATEST(COALESCE("usage".response_time_ms, 0), 0)::DOUBLE PRECISION), 0)
    AS total_response_time_ms
FROM "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }
        builder.push(" GROUP BY bucket_key ORDER BY bucket_key ASC");

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(StoredUsageTimeSeriesBucket {
                bucket_key: row.try_get::<String, _>("bucket_key").map_postgres_err()?,
                total_requests: row
                    .try_get::<i64, _>("total_requests")
                    .map_postgres_err()?
                    .max(0) as u64,
                input_tokens: row
                    .try_get::<i64, _>("input_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                output_tokens: row
                    .try_get::<i64, _>("output_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_creation_tokens: row
                    .try_get::<i64, _>("cache_creation_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_read_tokens: row
                    .try_get::<i64, _>("cache_read_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
                total_response_time_ms: row
                    .try_get::<f64, _>("total_response_time_ms")
                    .map_postgres_err()?,
            });
        }
        Ok(items)
    }

    pub async fn summarize_usage_leaderboard(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
        let fragments = usage_leaderboard_sql_fragments(query.group_by);
        let sql = format!(
            r#"
SELECT
  {group_key_expr} AS group_key,
  MAX({legacy_name_expr}) AS legacy_name,
  COUNT(*)::BIGINT AS request_count,
  COALESCE(SUM(
    GREATEST(COALESCE("usage".input_tokens, 0), 0)
    + GREATEST(COALESCE("usage".output_tokens, 0), 0)
    + GREATEST(COALESCE("usage".cache_creation_input_tokens, 0), 0)
    + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
  ), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd
FROM "usage"
WHERE "usage".created_at >= TO_TIMESTAMP($1::double precision)
  AND "usage".created_at < TO_TIMESTAMP($2::double precision)
  AND "usage".status NOT IN ('pending', 'streaming')
  AND "usage".provider_name NOT IN ('unknown', 'pending')
  {filtered_extra_where}
  AND ($3::varchar IS NULL OR "usage".user_id = $3)
  AND ($4::varchar IS NULL OR "usage".provider_name = $4)
  AND ($5::varchar IS NULL OR "usage".model = $5)
GROUP BY group_key
ORDER BY group_key ASC
"#,
            group_key_expr = fragments.group_key_expr,
            legacy_name_expr = fragments.legacy_name_expr,
            filtered_extra_where = fragments.filtered_extra_where,
        );
        let mut rows = sqlx::query(&sql)
            .bind(query.created_from_unix_secs as f64)
            .bind(query.created_until_unix_secs as f64)
            .bind(query.user_id.as_deref())
            .bind(query.provider_name.as_deref())
            .bind(query.model.as_deref())
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(StoredUsageLeaderboardSummary {
                group_key: row.try_get::<String, _>("group_key").map_postgres_err()?,
                legacy_name: row
                    .try_get::<Option<String>, _>("legacy_name")
                    .map_postgres_err()?,
                request_count: row
                    .try_get::<i64, _>("request_count")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_tokens: row
                    .try_get::<i64, _>("total_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
            });
        }
        Ok(items)
    }

    pub async fn aggregate_usage_audits(
        &self,
        query: &UsageAuditAggregationQuery,
    ) -> Result<Vec<StoredUsageAuditAggregation>, DataLayerError> {
        let fragments = usage_audit_aggregation_sql_fragments(query.group_by);
        let sql = format!(
            r#"
WITH filtered_usage AS (
  SELECT
    "usage".model AS model,
    "usage".user_id AS user_id,
    COALESCE("usage".provider_id, 'unknown') AS provider_group_key,
    CASE
      WHEN BTRIM(COALESCE("usage".provider_name, '')) = ''
        OR "usage".provider_name IN ('unknown', 'pending')
      THEN 'Unknown'
      ELSE "usage".provider_name
    END AS provider_display_name,
    COALESCE("usage".api_format, 'unknown') AS api_format_group_key,
    GREATEST(COALESCE("usage".input_tokens, 0), 0) AS input_tokens,
    GREATEST(COALESCE("usage".output_tokens, 0), 0) AS output_tokens,
    GREATEST(COALESCE("usage".total_tokens, 0), 0) AS total_tokens,
    CASE
      WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
           AND (
             COALESCE("usage".cache_creation_input_tokens_5m, 0)
             + COALESCE("usage".cache_creation_input_tokens_1h, 0)
           ) > 0
      THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
         + COALESCE("usage".cache_creation_input_tokens_1h, 0)
      ELSE COALESCE("usage".cache_creation_input_tokens, 0)
    END AS cache_creation_tokens,
    GREATEST(COALESCE("usage".cache_creation_input_tokens_5m, 0), 0)
      AS cache_creation_ephemeral_5m_tokens,
    GREATEST(COALESCE("usage".cache_creation_input_tokens_1h, 0), 0)
      AS cache_creation_ephemeral_1h_tokens,
    GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0) AS cache_read_tokens,
    COALESCE("usage".endpoint_api_format, "usage".api_format) AS normalized_api_format,
    COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
    COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0) AS actual_total_cost_usd,
    GREATEST(COALESCE("usage".response_time_ms, 0), 0) AS response_time_ms,
    CASE
      WHEN "usage".status IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND ("usage".status_code IS NULL OR "usage".status_code < 400)
      THEN 1
      ELSE 0
    END AS success_flag
  FROM "usage"
  WHERE "usage".created_at >= TO_TIMESTAMP($1::double precision)
    AND "usage".created_at < TO_TIMESTAMP($2::double precision)
    AND "usage".status NOT IN ('pending', 'streaming')
    {filtered_extra_where}
),
normalized_usage AS (
  SELECT
    {group_key_expr} AS group_key,
    {display_name_expr} AS display_name,
    {secondary_name_expr} AS secondary_name,
    total_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost_usd,
    actual_total_cost_usd,
    response_time_ms,
    success_flag,
    CASE
      WHEN input_tokens <= 0 THEN 0
      WHEN cache_read_tokens <= 0 THEN input_tokens
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN GREATEST(input_tokens - cache_read_tokens, 0)
      ELSE input_tokens
    END AS effective_input_tokens,
    CASE
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('claude', 'anthropic')
      THEN input_tokens + cache_creation_tokens + cache_read_tokens
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN (
        CASE
          WHEN input_tokens <= 0 THEN 0
          WHEN cache_read_tokens <= 0 THEN input_tokens
          WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
               IN ('openai', 'gemini', 'google')
          THEN GREATEST(input_tokens - cache_read_tokens, 0)
          ELSE input_tokens
        END
      ) + cache_read_tokens
      ELSE CASE
        WHEN cache_creation_tokens > 0
        THEN input_tokens + cache_creation_tokens + cache_read_tokens
        ELSE input_tokens + cache_read_tokens
      END
    END AS total_input_context
  FROM filtered_usage
),
aggregated_usage AS (
  SELECT
    group_key,
    {aggregate_display_name_expr} AS display_name,
    {aggregate_secondary_name_expr} AS secondary_name,
    COUNT(*)::BIGINT AS request_count,
    COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
    COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
    COALESCE(SUM(effective_input_tokens), 0)::BIGINT AS effective_input_tokens,
    COALESCE(SUM(total_input_context), 0)::BIGINT AS total_input_context,
    COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
    COALESCE(SUM(cache_creation_ephemeral_5m_tokens), 0)::BIGINT
      AS cache_creation_ephemeral_5m_tokens,
    COALESCE(SUM(cache_creation_ephemeral_1h_tokens), 0)::BIGINT
      AS cache_creation_ephemeral_1h_tokens,
    COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
    COALESCE(SUM(total_cost_usd), 0) AS total_cost_usd,
    COALESCE(SUM(actual_total_cost_usd), 0) AS actual_total_cost_usd,
    {avg_response_time_expr} AS avg_response_time_ms,
    {success_count_expr} AS success_count
  FROM normalized_usage
  GROUP BY group_key
)
SELECT
  group_key,
  display_name,
  secondary_name,
  request_count,
  total_tokens,
  output_tokens,
  effective_input_tokens,
  total_input_context,
  cache_creation_tokens,
  cache_creation_ephemeral_5m_tokens,
  cache_creation_ephemeral_1h_tokens,
  cache_read_tokens,
  total_cost_usd,
  actual_total_cost_usd,
  avg_response_time_ms,
  success_count
FROM aggregated_usage
ORDER BY request_count DESC, group_key ASC
LIMIT $3
"#,
            filtered_extra_where = fragments.filtered_extra_where,
            group_key_expr = fragments.group_key_expr,
            display_name_expr = fragments.display_name_expr,
            secondary_name_expr = fragments.secondary_name_expr,
            aggregate_display_name_expr = fragments.aggregate_display_name_expr,
            aggregate_secondary_name_expr = fragments.aggregate_secondary_name_expr,
            avg_response_time_expr = fragments.avg_response_time_expr,
            success_count_expr = fragments.success_count_expr,
        );

        let mut rows = sqlx::query(&sql)
            .bind(query.created_from_unix_secs as f64)
            .bind(query.created_until_unix_secs as f64)
            .bind(i64::try_from(query.limit).map_err(|_| {
                DataLayerError::InvalidInput(format!(
                    "invalid usage aggregation limit: {}",
                    query.limit
                ))
            })?)
            .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(StoredUsageAuditAggregation {
                group_key: row.try_get::<String, _>("group_key").map_postgres_err()?,
                display_name: row
                    .try_get::<Option<String>, _>("display_name")
                    .map_postgres_err()?,
                secondary_name: row
                    .try_get::<Option<String>, _>("secondary_name")
                    .map_postgres_err()?,
                request_count: row
                    .try_get::<i64, _>("request_count")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_tokens: row
                    .try_get::<i64, _>("total_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                output_tokens: row
                    .try_get::<i64, _>("output_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                effective_input_tokens: row
                    .try_get::<i64, _>("effective_input_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_input_context: row
                    .try_get::<i64, _>("total_input_context")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_creation_tokens: row
                    .try_get::<i64, _>("cache_creation_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_creation_ephemeral_5m_tokens: row
                    .try_get::<i64, _>("cache_creation_ephemeral_5m_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_creation_ephemeral_1h_tokens: row
                    .try_get::<i64, _>("cache_creation_ephemeral_1h_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_read_tokens: row
                    .try_get::<i64, _>("cache_read_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
                actual_total_cost_usd: row
                    .try_get::<f64, _>("actual_total_cost_usd")
                    .map_postgres_err()?,
                avg_response_time_ms: row
                    .try_get::<Option<f64>, _>("avg_response_time_ms")
                    .map_postgres_err()?,
                success_count: row
                    .try_get::<Option<i64>, _>("success_count")
                    .map_postgres_err()?
                    .map(|value| value.max(0) as u64),
            });
        }
        Ok(items)
    }

    pub async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        let mut sql = String::from(
            r#"SELECT
  DATE("usage".created_at) AS day,
  COUNT(*)::BIGINT AS requests,
  COALESCE(SUM("usage".input_tokens + "usage".output_tokens
    + CASE
        WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
             AND (COALESCE("usage".cache_creation_input_tokens_5m, 0) + COALESCE("usage".cache_creation_input_tokens_1h, 0)) > 0
        THEN COALESCE("usage".cache_creation_input_tokens_5m, 0) + COALESCE("usage".cache_creation_input_tokens_1h, 0)
        ELSE COALESCE("usage".cache_creation_input_tokens, 0)
      END
    + COALESCE("usage".cache_read_input_tokens, 0)), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(CAST("usage".total_cost_usd AS DOUBLE PRECISION)), 0) AS total_cost_usd,
  COALESCE(SUM(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION)), 0) AS actual_total_cost_usd
FROM "usage"
WHERE "usage".created_at >= TO_TIMESTAMP($1::double precision)"#,
        );
        if query.admin_mode {
            sql.push_str(" AND \"usage\".status NOT IN ('pending', 'streaming')");
        } else {
            sql.push_str(
                " AND \"usage\".billing_status = 'settled' AND CAST(\"usage\".total_cost_usd AS DOUBLE PRECISION) > 0",
            );
        }
        let mut bind_index = 2;
        if query.user_id.is_some() {
            sql.push_str(&format!(" AND \"usage\".user_id = ${bind_index}"));
            bind_index += 1;
        }
        let _ = bind_index;
        sql.push_str(" GROUP BY day ORDER BY day ASC");

        let mut q = sqlx::query(&sql).bind(query.created_from_unix_secs as f64);
        if let Some(user_id) = &query.user_id {
            q = q.bind(user_id.clone());
        }

        let mut rows = q.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let day: chrono::NaiveDate = row.try_get("day").map_postgres_err()?;
            let requests: i64 = row.try_get("requests").map_postgres_err()?;
            let total_tokens: i64 = row.try_get("total_tokens").map_postgres_err()?;
            let total_cost_usd: f64 = row.try_get("total_cost_usd").map_postgres_err()?;
            let actual_total_cost_usd: f64 =
                row.try_get("actual_total_cost_usd").map_postgres_err()?;
            items.push(StoredUsageDailySummary {
                date: day.to_string(),
                requests: requests as u64,
                total_tokens: total_tokens as u64,
                total_cost_usd,
                actual_total_cost_usd,
            });
        }
        Ok(items)
    }

    pub async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_RECENT_USAGE_AUDITS_PREFIX);
        if let Some(user_id) = user_id {
            builder
                .push(" WHERE \"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        builder
            .push(" ORDER BY \"usage\".created_at DESC, \"usage\".id ASC LIMIT ")
            .push_bind(i64::try_from(limit).map_err(|_| {
                DataLayerError::InvalidInput(format!("invalid recent usage limit: {limit}"))
            })?);
        let query = builder.build();
        let mut rows = query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row, false)?);
        }
        Ok(items)
    }

    pub async fn summarize_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }

        let mut rows = sqlx::query(SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL)
            .bind(api_key_ids)
            .fetch(&self.pool);

        let mut totals = std::collections::BTreeMap::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let api_key_id: String = row.try_get("api_key_id").map_postgres_err()?;
            let total_tokens = row
                .try_get::<i64, _>("total_tokens")
                .map_postgres_err()?
                .max(0) as u64;
            totals.insert(api_key_id, total_tokens);
        }
        Ok(totals)
    }

    pub async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, StoredProviderApiKeyUsageSummary>, DataLayerError>
    {
        if provider_api_key_ids.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }

        let mut rows = sqlx::query(SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL)
            .bind(provider_api_key_ids)
            .fetch(&self.pool);

        let mut summaries = std::collections::BTreeMap::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let provider_api_key_id: String =
                row.try_get("provider_api_key_id").map_postgres_err()?;
            let request_count = row
                .try_get::<i64, _>("request_count")
                .map_postgres_err()?
                .try_into()
                .map_err(|_| {
                    DataLayerError::UnexpectedValue(
                        "usage.request_count aggregate is negative".to_string(),
                    )
                })?;
            let total_tokens = row
                .try_get::<i64, _>("total_tokens")
                .map_postgres_err()?
                .try_into()
                .map_err(|_| {
                    DataLayerError::UnexpectedValue(
                        "usage.total_tokens aggregate is negative".to_string(),
                    )
                })?;
            let total_cost_usd: f64 = row.try_get("total_cost_usd").map_postgres_err()?;
            if !total_cost_usd.is_finite() {
                return Err(DataLayerError::UnexpectedValue(
                    "usage.total_cost_usd aggregate is not finite".to_string(),
                ));
            }
            let last_used_at_unix_secs = row
                .try_get::<Option<i64>, _>("last_used_at_unix_secs")
                .map_postgres_err()?
                .map(|value| {
                    value.try_into().map_err(|_| {
                        DataLayerError::UnexpectedValue(
                            "usage.last_used_at_unix_secs aggregate is negative".to_string(),
                        )
                    })
                })
                .transpose()?;

            summaries.insert(
                provider_api_key_id.clone(),
                StoredProviderApiKeyUsageSummary {
                    provider_api_key_id,
                    request_count,
                    total_tokens,
                    total_cost_usd,
                    last_used_at_unix_secs,
                },
            );
        }

        Ok(summaries)
    }

    pub async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        usage.validate()?;
        let usage = strip_deprecated_usage_display_fields(usage);
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    if incoming_usage_can_recover_terminal_failure(
                        usage.status.as_str(),
                        usage.billing_status.as_str(),
                    ) {
                        sqlx::query(RESET_STALE_VOID_USAGE_SQL)
                            .bind(&usage.request_id)
                            .execute(&mut **tx)
                            .await
                            .map_postgres_err()?;
                        sqlx::query(RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL)
                            .bind(&usage.request_id)
                            .execute(&mut **tx)
                            .await
                            .map_postgres_err()?;
                    }

                    let request_headers_json = json_bind_text(usage.request_headers.as_ref())?;
                    let request_body_storage =
                        prepare_usage_body_storage(usage.request_body.as_ref())?;
                    let provider_request_headers_json =
                        json_bind_text(usage.provider_request_headers.as_ref())?;
                    let provider_request_body_storage =
                        prepare_usage_body_storage(usage.provider_request_body.as_ref())?;
                    let response_headers_json = json_bind_text(usage.response_headers.as_ref())?;
                    let response_body_storage =
                        prepare_usage_body_storage(usage.response_body.as_ref())?;
                    let client_response_headers_json =
                        json_bind_text(usage.client_response_headers.as_ref())?;
                    let client_response_body_storage =
                        prepare_usage_body_storage(usage.client_response_body.as_ref())?;
                    let http_audit_refs = UsageHttpAuditRefs {
                        request_body_ref: resolved_write_usage_body_ref(
                            usage.request_body_ref.as_deref(),
                            &usage.request_id,
                            UsageBodyField::RequestBody,
                            request_body_storage.has_detached_blob(),
                            None,
                        ),
                        provider_request_body_ref: resolved_write_usage_body_ref(
                            usage.provider_request_body_ref.as_deref(),
                            &usage.request_id,
                            UsageBodyField::ProviderRequestBody,
                            provider_request_body_storage.has_detached_blob(),
                            None,
                        ),
                        response_body_ref: resolved_write_usage_body_ref(
                            usage.response_body_ref.as_deref(),
                            &usage.request_id,
                            UsageBodyField::ResponseBody,
                            response_body_storage.has_detached_blob(),
                            None,
                        ),
                        client_response_body_ref: resolved_write_usage_body_ref(
                            usage.client_response_body_ref.as_deref(),
                            &usage.request_id,
                            UsageBodyField::ClientResponseBody,
                            client_response_body_storage.has_detached_blob(),
                            None,
                        ),
                    };
                    let request_metadata_value = prepare_request_metadata_for_body_storage(
                        usage.request_metadata.clone(),
                        [
                            (
                                UsageBodyField::RequestBody,
                                &request_body_storage,
                                usage.request_body.as_ref(),
                                usage.request_body_ref.as_deref(),
                            ),
                            (
                                UsageBodyField::ProviderRequestBody,
                                &provider_request_body_storage,
                                usage.provider_request_body.as_ref(),
                                usage.provider_request_body_ref.as_deref(),
                            ),
                            (
                                UsageBodyField::ResponseBody,
                                &response_body_storage,
                                usage.response_body.as_ref(),
                                usage.response_body_ref.as_deref(),
                            ),
                            (
                                UsageBodyField::ClientResponseBody,
                                &client_response_body_storage,
                                usage.client_response_body.as_ref(),
                                usage.client_response_body_ref.as_deref(),
                            ),
                        ],
                    );
                    let http_audit_capture_mode = usage_http_audit_capture_mode(
                        &http_audit_refs,
                        [
                            usage.request_body.as_ref(),
                            usage.provider_request_body.as_ref(),
                            usage.response_body.as_ref(),
                            usage.client_response_body.as_ref(),
                        ],
                    );
                    let routing_snapshot =
                        usage_routing_snapshot_from_usage(&usage, request_metadata_value.as_ref());
                    let settlement_pricing_snapshot = usage_settlement_pricing_snapshot_from_usage(
                        &usage,
                        request_metadata_value.as_ref(),
                    );
                    let request_metadata_json = json_bind_text(request_metadata_value.as_ref())?;
                    let row = sqlx::query(UPSERT_SQL)
                        .bind(Uuid::new_v4().to_string())
                        .bind(&usage.request_id)
                        .bind(&usage.user_id)
                        .bind(&usage.api_key_id)
                        .bind(&usage.username)
                        .bind(&usage.api_key_name)
                        .bind(&usage.provider_name)
                        .bind(&usage.model)
                        .bind(&usage.target_model)
                        .bind(&usage.provider_id)
                        .bind(&usage.provider_endpoint_id)
                        .bind(&usage.provider_api_key_id)
                        .bind(&usage.request_type)
                        .bind(&usage.api_format)
                        .bind(&usage.api_family)
                        .bind(&usage.endpoint_kind)
                        .bind(&usage.endpoint_api_format)
                        .bind(&usage.provider_api_family)
                        .bind(&usage.provider_endpoint_kind)
                        .bind(usage.has_format_conversion)
                        .bind(usage.is_stream)
                        .bind(usage.input_tokens.map(to_i32).transpose()?)
                        .bind(usage.output_tokens.map(to_i32).transpose()?)
                        .bind(
                            usage
                                .total_tokens
                                .or_else(|| {
                                    Some(
                                        usage.input_tokens.unwrap_or_default()
                                            + usage.output_tokens.unwrap_or_default(),
                                    )
                                })
                                .map(to_i32)
                                .transpose()?,
                        )
                        .bind(usage.cache_creation_input_tokens.map(to_i32).transpose()?)
                        .bind(
                            usage
                                .cache_creation_ephemeral_5m_input_tokens
                                .map(to_i32)
                                .transpose()?,
                        )
                        .bind(
                            usage
                                .cache_creation_ephemeral_1h_input_tokens
                                .map(to_i32)
                                .transpose()?,
                        )
                        .bind(usage.cache_read_input_tokens.map(to_i32).transpose()?)
                        .bind(usage.cache_creation_cost_usd)
                        .bind(usage.cache_read_cost_usd)
                        .bind(None::<f64>)
                        .bind(usage.total_cost_usd)
                        .bind(usage.actual_total_cost_usd)
                        .bind(usage.status_code.map(i32::from))
                        .bind(&usage.error_message)
                        .bind(&usage.error_category)
                        .bind(usage.response_time_ms.map(to_i32).transpose()?)
                        .bind(usage.first_byte_time_ms.map(to_i32).transpose()?)
                        .bind(&usage.status)
                        .bind(&usage.billing_status)
                        .bind(None::<String>)
                        .bind(&request_body_storage.inline_json)
                        .bind(None::<Vec<u8>>)
                        .bind(None::<String>)
                        .bind(&provider_request_body_storage.inline_json)
                        .bind(None::<Vec<u8>>)
                        .bind(None::<String>)
                        .bind(&response_body_storage.inline_json)
                        .bind(None::<Vec<u8>>)
                        .bind(None::<String>)
                        .bind(&client_response_body_storage.inline_json)
                        .bind(None::<Vec<u8>>)
                        .bind(&request_metadata_json)
                        .bind(usage.finalized_at_unix_secs.map(|value| value as f64))
                        .bind(usage.created_at_unix_ms.map(|value| value as f64))
                        .bind(request_body_storage.has_detached_blob())
                        .bind(provider_request_body_storage.has_detached_blob())
                        .bind(response_body_storage.has_detached_blob())
                        .bind(client_response_body_storage.has_detached_blob())
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    sync_usage_body_blob_storage(
                        &mut **tx,
                        &usage.request_id,
                        UsageBodyField::RequestBody,
                        usage.request_body.as_ref(),
                        &request_body_storage,
                    )
                    .await?;
                    sync_usage_body_blob_storage(
                        &mut **tx,
                        &usage.request_id,
                        UsageBodyField::ProviderRequestBody,
                        usage.provider_request_body.as_ref(),
                        &provider_request_body_storage,
                    )
                    .await?;
                    sync_usage_body_blob_storage(
                        &mut **tx,
                        &usage.request_id,
                        UsageBodyField::ResponseBody,
                        usage.response_body.as_ref(),
                        &response_body_storage,
                    )
                    .await?;
                    sync_usage_body_blob_storage(
                        &mut **tx,
                        &usage.request_id,
                        UsageBodyField::ClientResponseBody,
                        usage.client_response_body.as_ref(),
                        &client_response_body_storage,
                    )
                    .await?;
                    let http_audit_headers = UsageHttpAuditHeaders {
                        request_headers_json: request_headers_json.as_deref(),
                        provider_request_headers_json: provider_request_headers_json.as_deref(),
                        response_headers_json: response_headers_json.as_deref(),
                        client_response_headers_json: client_response_headers_json.as_deref(),
                    };
                    sync_usage_http_audit_storage(
                        &mut **tx,
                        &usage.request_id,
                        &http_audit_headers,
                        &http_audit_refs,
                        http_audit_capture_mode,
                    )
                    .await?;
                    sync_usage_routing_snapshot_storage(
                        &mut **tx,
                        &usage.request_id,
                        &routing_snapshot,
                    )
                    .await?;
                    sync_usage_settlement_pricing_snapshot_storage(
                        &mut **tx,
                        &usage.request_id,
                        &settlement_pricing_snapshot,
                    )
                    .await?;

                    let mut stored = map_usage_row(&row, true)?;
                    if request_body_storage.has_detached_blob() {
                        stored.request_body = usage.request_body.clone();
                    }
                    stored.request_headers = usage.request_headers.clone();
                    stored.provider_request_headers = usage.provider_request_headers.clone();
                    if provider_request_body_storage.has_detached_blob() {
                        stored.provider_request_body = usage.provider_request_body.clone();
                    }
                    stored.response_headers = usage.response_headers.clone();
                    if response_body_storage.has_detached_blob() {
                        stored.response_body = usage.response_body.clone();
                    }
                    stored.client_response_headers = usage.client_response_headers.clone();
                    if client_response_body_storage.has_detached_blob() {
                        stored.client_response_body = usage.client_response_body.clone();
                    }
                    stored.request_body_ref = resolved_write_usage_body_ref(
                        usage.request_body_ref.as_deref(),
                        &usage.request_id,
                        UsageBodyField::RequestBody,
                        request_body_storage.has_detached_blob(),
                        http_audit_refs.request_body_ref.as_deref(),
                    );
                    stored.provider_request_body_ref = resolved_write_usage_body_ref(
                        usage.provider_request_body_ref.as_deref(),
                        &usage.request_id,
                        UsageBodyField::ProviderRequestBody,
                        provider_request_body_storage.has_detached_blob(),
                        http_audit_refs.provider_request_body_ref.as_deref(),
                    );
                    stored.response_body_ref = resolved_write_usage_body_ref(
                        usage.response_body_ref.as_deref(),
                        &usage.request_id,
                        UsageBodyField::ResponseBody,
                        response_body_storage.has_detached_blob(),
                        http_audit_refs.response_body_ref.as_deref(),
                    );
                    stored.client_response_body_ref = resolved_write_usage_body_ref(
                        usage.client_response_body_ref.as_deref(),
                        &usage.request_id,
                        UsageBodyField::ClientResponseBody,
                        client_response_body_storage.has_detached_blob(),
                        http_audit_refs.client_response_body_ref.as_deref(),
                    );
                    stored.candidate_id = routing_snapshot.candidate_id.clone();
                    stored.candidate_index = routing_snapshot.candidate_index;
                    stored.key_name = routing_snapshot.key_name.clone();
                    stored.planner_kind = routing_snapshot.planner_kind.clone();
                    stored.route_family = routing_snapshot.route_family.clone();
                    stored.route_kind = routing_snapshot.route_kind.clone();
                    stored.execution_path = routing_snapshot.execution_path.clone();
                    stored.local_execution_runtime_miss_reason =
                        routing_snapshot.local_execution_runtime_miss_reason.clone();
                    stored.output_price_per_1m = settlement_pricing_snapshot.output_price_per_1m;
                    stored.request_metadata = request_metadata_value;
                    Ok(stored)
                }) as BoxFuture<'_, Result<StoredRequestUsageAudit, DataLayerError>>
            })
            .await
    }
}

#[async_trait]
impl UsageReadRepository for SqlxUsageReadRepository {
    async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        Self::find_by_id(self, id).await
    }

    async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        Self::find_by_request_id(self, request_id).await
    }

    async fn resolve_body_ref(&self, body_ref: &str) -> Result<Option<Value>, DataLayerError> {
        Self::resolve_body_ref(self, body_ref).await
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

    async fn summarize_usage_time_series(
        &self,
        query: &UsageTimeSeriesQuery,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        Self::summarize_usage_time_series(self, query).await
    }

    async fn summarize_usage_leaderboard(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
        Self::summarize_usage_leaderboard(self, query).await
    }

    async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_recent_usage_audits(self, user_id, limit).await
    }

    async fn summarize_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        Self::summarize_total_tokens_by_api_key_ids(self, api_key_ids).await
    }

    async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, StoredProviderApiKeyUsageSummary>, DataLayerError>
    {
        Self::summarize_usage_by_provider_api_key_ids(self, provider_api_key_ids).await
    }

    async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, DataLayerError> {
        Self::summarize_provider_usage_since(self, provider_id, since_unix_secs).await
    }

    async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        Self::summarize_usage_daily_heatmap(self, query).await
    }
}

#[async_trait]
impl UsageWriteRepository for SqlxUsageReadRepository {
    async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        Self::upsert(self, usage).await
    }
}

fn map_usage_row(
    row: &sqlx::postgres::PgRow,
    resolve_compressed_bodies: bool,
) -> Result<StoredRequestUsageAudit, DataLayerError> {
    let mut usage = StoredRequestUsageAudit::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("request_id").map_postgres_err()?,
        row.try_get("user_id").map_postgres_err()?,
        row.try_get("api_key_id").map_postgres_err()?,
        row.try_get("username").map_postgres_err()?,
        row.try_get("api_key_name").map_postgres_err()?,
        row.try_get("provider_name").map_postgres_err()?,
        row.try_get("model").map_postgres_err()?,
        row.try_get("target_model").map_postgres_err()?,
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("provider_endpoint_id").map_postgres_err()?,
        row.try_get("provider_api_key_id").map_postgres_err()?,
        row.try_get("request_type").map_postgres_err()?,
        row.try_get("api_format").map_postgres_err()?,
        row.try_get("api_family").map_postgres_err()?,
        row.try_get("endpoint_kind").map_postgres_err()?,
        row.try_get("endpoint_api_format").map_postgres_err()?,
        row.try_get("provider_api_family").map_postgres_err()?,
        row.try_get("provider_endpoint_kind").map_postgres_err()?,
        row.try_get("has_format_conversion").map_postgres_err()?,
        row.try_get("is_stream").map_postgres_err()?,
        row.try_get("input_tokens").map_postgres_err()?,
        row.try_get("output_tokens").map_postgres_err()?,
        row.try_get("total_tokens").map_postgres_err()?,
        row.try_get("total_cost_usd").map_postgres_err()?,
        row.try_get("actual_total_cost_usd").map_postgres_err()?,
        row.try_get("status_code").map_postgres_err()?,
        row.try_get("error_message").map_postgres_err()?,
        row.try_get("error_category").map_postgres_err()?,
        row.try_get("response_time_ms").map_postgres_err()?,
        row.try_get("first_byte_time_ms").map_postgres_err()?,
        row.try_get("status").map_postgres_err()?,
        row.try_get("billing_status").map_postgres_err()?,
        row.try_get("created_at_unix_ms").map_postgres_err()?,
        row.try_get("updated_at_unix_secs").map_postgres_err()?,
        row.try_get("finalized_at_unix_secs").map_postgres_err()?,
    )?;
    usage.cache_creation_input_tokens = row
        .try_get::<Option<i32>, _>("cache_creation_input_tokens")
        .map_postgres_err()?
        .map(|value| to_u64(value, "usage.cache_creation_input_tokens"))
        .transpose()?
        .unwrap_or_default();
    usage.cache_creation_ephemeral_5m_input_tokens = row
        .try_get::<Option<i32>, _>("cache_creation_ephemeral_5m_input_tokens")
        .map_postgres_err()?
        .map(|value| to_u64(value, "usage.cache_creation_ephemeral_5m_input_tokens"))
        .transpose()?
        .unwrap_or_default();
    usage.cache_creation_ephemeral_1h_input_tokens = row
        .try_get::<Option<i32>, _>("cache_creation_ephemeral_1h_input_tokens")
        .map_postgres_err()?
        .map(|value| to_u64(value, "usage.cache_creation_ephemeral_1h_input_tokens"))
        .transpose()?
        .unwrap_or_default();
    usage.cache_read_input_tokens = row
        .try_get::<Option<i32>, _>("cache_read_input_tokens")
        .map_postgres_err()?
        .map(|value| to_u64(value, "usage.cache_read_input_tokens"))
        .transpose()?
        .unwrap_or_default();
    usage.cache_creation_cost_usd = row
        .try_get::<f64, _>("cache_creation_cost_usd")
        .map_postgres_err()?;
    usage.cache_read_cost_usd = row
        .try_get::<f64, _>("cache_read_cost_usd")
        .map_postgres_err()?;
    usage.output_price_per_1m = row.try_get("output_price_per_1m").map_postgres_err()?;
    usage.request_headers = row.try_get("request_headers").map_postgres_err()?;
    let request_body = usage_json_column(
        row,
        "request_body",
        "request_body_compressed",
        resolve_compressed_bodies,
    )?;
    usage.provider_request_headers = row.try_get("provider_request_headers").map_postgres_err()?;
    let provider_request_body = usage_json_column(
        row,
        "provider_request_body",
        "provider_request_body_compressed",
        resolve_compressed_bodies,
    )?;
    usage.response_headers = row.try_get("response_headers").map_postgres_err()?;
    let response_body = usage_json_column(
        row,
        "response_body",
        "response_body_compressed",
        resolve_compressed_bodies,
    )?;
    usage.client_response_headers = row.try_get("client_response_headers").map_postgres_err()?;
    let client_response_body = usage_json_column(
        row,
        "client_response_body",
        "client_response_body_compressed",
        resolve_compressed_bodies,
    )?;
    let request_metadata: Option<Value> = row.try_get("request_metadata").map_postgres_err()?;
    let http_audit_refs = UsageHttpAuditRefs {
        request_body_ref: row.try_get("http_request_body_ref").map_postgres_err()?,
        provider_request_body_ref: row
            .try_get("http_provider_request_body_ref")
            .map_postgres_err()?,
        response_body_ref: row.try_get("http_response_body_ref").map_postgres_err()?,
        client_response_body_ref: row
            .try_get("http_client_response_body_ref")
            .map_postgres_err()?,
    };
    let routing_snapshot = usage_routing_snapshot_from_row(row)?;
    let settlement_pricing_snapshot = usage_settlement_pricing_snapshot_from_row(row)?;
    usage.request_body = request_body.value;
    usage.provider_request_body = provider_request_body.value;
    usage.response_body = response_body.value;
    usage.client_response_body = client_response_body.value;
    let request_metadata_object = request_metadata.as_ref().and_then(Value::as_object);
    usage.request_body_ref = resolved_read_usage_body_ref(
        None,
        request_metadata_object,
        &usage.request_id,
        UsageBodyField::RequestBody,
        request_body.has_compressed_storage,
        http_audit_refs.request_body_ref.as_deref(),
    );
    usage.provider_request_body_ref = resolved_read_usage_body_ref(
        None,
        request_metadata_object,
        &usage.request_id,
        UsageBodyField::ProviderRequestBody,
        provider_request_body.has_compressed_storage,
        http_audit_refs.provider_request_body_ref.as_deref(),
    );
    usage.response_body_ref = resolved_read_usage_body_ref(
        None,
        request_metadata_object,
        &usage.request_id,
        UsageBodyField::ResponseBody,
        response_body.has_compressed_storage,
        http_audit_refs.response_body_ref.as_deref(),
    );
    usage.client_response_body_ref = resolved_read_usage_body_ref(
        None,
        request_metadata_object,
        &usage.request_id,
        UsageBodyField::ClientResponseBody,
        client_response_body.has_compressed_storage,
        http_audit_refs.client_response_body_ref.as_deref(),
    );
    usage.candidate_id = routing_snapshot.candidate_id.clone();
    usage.candidate_index = routing_snapshot.candidate_index;
    usage.key_name = routing_snapshot.key_name.clone();
    usage.planner_kind = routing_snapshot.planner_kind.clone();
    usage.route_family = routing_snapshot.route_family.clone();
    usage.route_kind = routing_snapshot.route_kind.clone();
    usage.execution_path = routing_snapshot.execution_path.clone();
    usage.local_execution_runtime_miss_reason =
        routing_snapshot.local_execution_runtime_miss_reason.clone();
    usage.request_metadata = attach_usage_settlement_pricing_snapshot_metadata(
        request_metadata,
        &settlement_pricing_snapshot,
    );
    Ok(usage)
}

fn to_i32(value: u64) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("invalid usage integer value: {value}"))
    })
}

fn to_u64(value: i32, field_name: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageBodyStorage {
    inline_json: Option<String>,
    detached_blob_bytes: Option<Vec<u8>>,
}

impl UsageBodyStorage {
    fn has_detached_blob(&self) -> bool {
        self.detached_blob_bytes.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct UsageBodyColumn {
    value: Option<Value>,
    has_compressed_storage: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageHttpAuditRefs {
    request_body_ref: Option<String>,
    provider_request_body_ref: Option<String>,
    response_body_ref: Option<String>,
    client_response_body_ref: Option<String>,
}

impl UsageHttpAuditRefs {
    fn any_present(&self) -> bool {
        self.request_body_ref.is_some()
            || self.provider_request_body_ref.is_some()
            || self.response_body_ref.is_some()
            || self.client_response_body_ref.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageHttpAuditHeaders<'a> {
    request_headers_json: Option<&'a str>,
    provider_request_headers_json: Option<&'a str>,
    response_headers_json: Option<&'a str>,
    client_response_headers_json: Option<&'a str>,
}

impl UsageHttpAuditHeaders<'_> {
    fn any_present(&self) -> bool {
        self.request_headers_json.is_some()
            || self.provider_request_headers_json.is_some()
            || self.response_headers_json.is_some()
            || self.client_response_headers_json.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageRoutingSnapshot {
    candidate_id: Option<String>,
    candidate_index: Option<u64>,
    key_name: Option<String>,
    planner_kind: Option<String>,
    route_family: Option<String>,
    route_kind: Option<String>,
    execution_path: Option<String>,
    local_execution_runtime_miss_reason: Option<String>,
    selected_provider_id: Option<String>,
    selected_endpoint_id: Option<String>,
    selected_provider_api_key_id: Option<String>,
    has_format_conversion: Option<bool>,
}

impl UsageRoutingSnapshot {
    fn has_metadata_fields(&self) -> bool {
        self.candidate_id.is_some()
            || self.candidate_index.is_some()
            || self.key_name.is_some()
            || self.planner_kind.is_some()
            || self.route_family.is_some()
            || self.route_kind.is_some()
            || self.execution_path.is_some()
            || self.local_execution_runtime_miss_reason.is_some()
    }

    fn any_present(&self) -> bool {
        self.has_metadata_fields()
            || self.selected_provider_id.is_some()
            || self.selected_endpoint_id.is_some()
            || self.selected_provider_api_key_id.is_some()
            || self.has_format_conversion.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct UsageSettlementPricingSnapshot {
    billing_status: Option<String>,
    billing_snapshot_schema_version: Option<String>,
    billing_snapshot_status: Option<String>,
    rate_multiplier: Option<f64>,
    is_free_tier: Option<bool>,
    input_price_per_1m: Option<f64>,
    output_price_per_1m: Option<f64>,
    cache_creation_price_per_1m: Option<f64>,
    cache_read_price_per_1m: Option<f64>,
    price_per_request: Option<f64>,
}

impl UsageSettlementPricingSnapshot {
    fn any_present(&self) -> bool {
        self.billing_status.is_some()
            || self.billing_snapshot_schema_version.is_some()
            || self.billing_snapshot_status.is_some()
            || self.rate_multiplier.is_some()
            || self.is_free_tier.is_some()
            || self.input_price_per_1m.is_some()
            || self.output_price_per_1m.is_some()
            || self.cache_creation_price_per_1m.is_some()
            || self.cache_read_price_per_1m.is_some()
            || self.price_per_request.is_some()
    }
}

fn prepare_usage_body_storage(value: Option<&Value>) -> Result<UsageBodyStorage, DataLayerError> {
    let Some(value) = value else {
        return Ok(UsageBodyStorage {
            inline_json: None,
            detached_blob_bytes: None,
        });
    };
    let bytes = serde_json::to_vec(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to serialize usage json: {err}"))
    })?;
    if bytes.len() == MAX_INLINE_USAGE_BODY_BYTES {
        return Ok(UsageBodyStorage {
            inline_json: Some(String::from_utf8(bytes).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "failed to encode inline usage body as utf-8: {err}"
                ))
            })?),
            detached_blob_bytes: None,
        });
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(&bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to compress usage json: {err}"))
    })?;
    let detached_blob_bytes = encoder.finish().map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to finish usage json compression: {err}"))
    })?;
    Ok(UsageBodyStorage {
        inline_json: None,
        detached_blob_bytes: Some(detached_blob_bytes),
    })
}

fn json_bind_text(value: Option<&Value>) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!("failed to serialize usage json: {err}"))
            })
        })
        .transpose()
}

#[cfg(test)]
fn usage_http_audit_body_refs(metadata: Option<&Value>) -> UsageHttpAuditRefs {
    let object = metadata.and_then(Value::as_object);
    UsageHttpAuditRefs {
        request_body_ref: metadata_ref_value(object, "request_body_ref"),
        provider_request_body_ref: metadata_ref_value(object, "provider_request_body_ref"),
        response_body_ref: metadata_ref_value(object, "response_body_ref"),
        client_response_body_ref: metadata_ref_value(object, "client_response_body_ref"),
    }
}

fn resolved_read_usage_body_ref(
    explicit_ref: Option<&str>,
    metadata: Option<&serde_json::Map<String, Value>>,
    request_id: &str,
    field: UsageBodyField,
    has_compressed_storage: bool,
    http_audit_ref: Option<&str>,
) -> Option<String> {
    explicit_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            http_audit_ref
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| has_compressed_storage.then(|| usage_body_ref(request_id, field)))
        .or_else(|| metadata_usage_body_ref_value(metadata, request_id, field))
}

fn resolved_write_usage_body_ref(
    explicit_ref: Option<&str>,
    request_id: &str,
    field: UsageBodyField,
    has_compressed_storage: bool,
    http_audit_ref: Option<&str>,
) -> Option<String> {
    explicit_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| has_compressed_storage.then(|| usage_body_ref(request_id, field)))
        .or_else(|| {
            http_audit_ref
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn metadata_ref_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<String> {
    metadata
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn metadata_usage_body_ref_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    request_id: &str,
    field: UsageBodyField,
) -> Option<String> {
    metadata_ref_value(metadata, field.as_ref_key())
        .and_then(|value| parse_usage_body_ref(&value))
        .filter(|(parsed_request_id, parsed_field)| {
            parsed_request_id == request_id && *parsed_field == field
        })
        .map(|(parsed_request_id, parsed_field)| usage_body_ref(&parsed_request_id, parsed_field))
}

fn metadata_number_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<f64> {
    metadata
        .and_then(|object| object.get(key))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn metadata_u64_value(metadata: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<u64> {
    metadata.and_then(|object| {
        object.get(key).and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        })
    })
}

fn metadata_bool_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<bool> {
    metadata
        .and_then(|object| object.get(key))
        .and_then(Value::as_bool)
}

fn billing_snapshot_object(
    metadata: Option<&serde_json::Map<String, Value>>,
) -> Option<&serde_json::Map<String, Value>> {
    metadata
        .and_then(|object| object.get("billing_snapshot"))
        .and_then(Value::as_object)
}

fn billing_snapshot_string_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<String> {
    billing_snapshot_object(metadata)
        .and_then(|snapshot| snapshot.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn billing_snapshot_resolved_number(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<f64> {
    billing_snapshot_object(metadata)
        .and_then(|snapshot| snapshot.get("resolved_variables"))
        .and_then(Value::as_object)
        .and_then(|variables| variables.get(key))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn usage_http_audit_capture_mode(
    refs: &UsageHttpAuditRefs,
    body_values: [Option<&Value>; 4],
) -> &'static str {
    if refs.any_present() {
        return "ref_backed";
    }
    if body_values.iter().any(Option::is_some) {
        return "inline_legacy";
    }
    "none"
}

fn usage_routing_snapshot_from_usage(
    usage: &UpsertUsageRecord,
    metadata: Option<&Value>,
) -> UsageRoutingSnapshot {
    let object = metadata.and_then(Value::as_object);
    let mut snapshot = UsageRoutingSnapshot {
        candidate_id: usage
            .candidate_id
            .clone()
            .or_else(|| metadata_ref_value(object, "candidate_id")),
        candidate_index: usage
            .candidate_index
            .or_else(|| metadata_u64_value(object, "candidate_index")),
        key_name: usage
            .key_name
            .clone()
            .or_else(|| metadata_ref_value(object, "key_name")),
        planner_kind: usage
            .planner_kind
            .clone()
            .or_else(|| metadata_ref_value(object, "planner_kind")),
        route_family: usage
            .route_family
            .clone()
            .or_else(|| metadata_ref_value(object, "route_family")),
        route_kind: usage
            .route_kind
            .clone()
            .or_else(|| metadata_ref_value(object, "route_kind")),
        execution_path: usage
            .execution_path
            .clone()
            .or_else(|| metadata_ref_value(object, "execution_path")),
        local_execution_runtime_miss_reason: usage
            .local_execution_runtime_miss_reason
            .clone()
            .or_else(|| metadata_ref_value(object, "local_execution_runtime_miss_reason")),
        selected_provider_id: None,
        selected_endpoint_id: None,
        selected_provider_api_key_id: None,
        has_format_conversion: None,
    };
    if !snapshot.has_metadata_fields() {
        return snapshot;
    }

    snapshot.selected_provider_id = usage.provider_id.clone();
    snapshot.selected_endpoint_id = usage.provider_endpoint_id.clone();
    snapshot.selected_provider_api_key_id = usage.provider_api_key_id.clone();
    snapshot.has_format_conversion = usage.has_format_conversion;
    snapshot
}

fn usage_settlement_pricing_snapshot_from_usage(
    usage: &UpsertUsageRecord,
    metadata: Option<&Value>,
) -> UsageSettlementPricingSnapshot {
    let object = metadata.and_then(Value::as_object);
    let snapshot = UsageSettlementPricingSnapshot {
        billing_status: Some(usage.billing_status.clone()),
        billing_snapshot_schema_version: metadata_ref_value(
            object,
            "billing_snapshot_schema_version",
        )
        .or_else(|| billing_snapshot_string_value(object, "schema_version")),
        billing_snapshot_status: metadata_ref_value(object, "billing_snapshot_status")
            .or_else(|| billing_snapshot_string_value(object, "status")),
        rate_multiplier: metadata_number_value(object, "rate_multiplier"),
        is_free_tier: metadata_bool_value(object, "is_free_tier"),
        input_price_per_1m: metadata_number_value(object, "input_price_per_1m")
            .or_else(|| billing_snapshot_resolved_number(object, "input_price_per_1m")),
        output_price_per_1m: metadata_number_value(object, "output_price_per_1m")
            .or_else(|| billing_snapshot_resolved_number(object, "output_price_per_1m"))
            .or(usage.output_price_per_1m),
        cache_creation_price_per_1m: metadata_number_value(object, "cache_creation_price_per_1m")
            .or_else(|| billing_snapshot_resolved_number(object, "cache_creation_price_per_1m")),
        cache_read_price_per_1m: metadata_number_value(object, "cache_read_price_per_1m")
            .or_else(|| billing_snapshot_resolved_number(object, "cache_read_price_per_1m")),
        price_per_request: metadata_number_value(object, "price_per_request")
            .or_else(|| billing_snapshot_resolved_number(object, "price_per_request")),
    };
    if snapshot.any_present() {
        snapshot
    } else {
        UsageSettlementPricingSnapshot::default()
    }
}

fn usage_json_column(
    row: &sqlx::postgres::PgRow,
    inline_column: &str,
    compressed_column: &str,
    resolve_compressed: bool,
) -> Result<UsageBodyColumn, DataLayerError> {
    let inline = row
        .try_get::<Option<Value>, _>(inline_column)
        .map_postgres_err()?;
    if inline.is_some() {
        return Ok(UsageBodyColumn {
            value: inline,
            has_compressed_storage: false,
        });
    }
    let compressed = row
        .try_get::<Option<Vec<u8>>, _>(compressed_column)
        .map_postgres_err()?;
    let has_compressed_storage = compressed.is_some();
    let value = if resolve_compressed {
        compressed
            .map(|bytes| inflate_usage_json_value(&bytes))
            .transpose()?
    } else {
        None
    };
    Ok(UsageBodyColumn {
        value,
        has_compressed_storage,
    })
}

fn inflate_usage_json_value(bytes: &[u8]) -> Result<Value, DataLayerError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut json_bytes = Vec::new();
    decoder.read_to_end(&mut json_bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to decompress usage json: {err}"))
    })?;
    serde_json::from_slice(&json_bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to parse decompressed usage json: {err}"))
    })
}

#[cfg(test)]
fn attach_compressed_body_refs(
    request_id: &str,
    metadata: Option<Value>,
    has_request_body_compressed: bool,
    has_provider_request_body_compressed: bool,
    has_response_body_compressed: bool,
    has_client_response_body_compressed: bool,
) -> Option<Value> {
    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => return Some(value),
        None => Map::new(),
    };
    maybe_insert_usage_body_ref(
        &mut metadata,
        "request_body_ref",
        request_id,
        "request_body",
        has_request_body_compressed,
    );
    maybe_insert_usage_body_ref(
        &mut metadata,
        "provider_request_body_ref",
        request_id,
        "provider_request_body",
        has_provider_request_body_compressed,
    );
    maybe_insert_usage_body_ref(
        &mut metadata,
        "response_body_ref",
        request_id,
        "response_body",
        has_response_body_compressed,
    );
    maybe_insert_usage_body_ref(
        &mut metadata,
        "client_response_body_ref",
        request_id,
        "client_response_body",
        has_client_response_body_compressed,
    );
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

#[cfg(test)]
fn attach_usage_http_audit_body_refs(
    metadata: Option<Value>,
    refs: &UsageHttpAuditRefs,
) -> Option<Value> {
    if !refs.any_present() {
        return metadata;
    }

    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => return Some(value),
        None => Map::new(),
    };
    maybe_insert_string_value(
        &mut metadata,
        "request_body_ref",
        refs.request_body_ref.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "provider_request_body_ref",
        refs.provider_request_body_ref.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "response_body_ref",
        refs.response_body_ref.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "client_response_body_ref",
        refs.client_response_body_ref.as_deref(),
    );
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

#[cfg(test)]
fn attach_usage_routing_snapshot_metadata(
    metadata: Option<Value>,
    snapshot: &UsageRoutingSnapshot,
) -> Option<Value> {
    if !snapshot.has_metadata_fields() {
        return metadata;
    }

    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => return Some(value),
        None => Map::new(),
    };
    maybe_insert_string_value(
        &mut metadata,
        "candidate_id",
        snapshot.candidate_id.as_deref(),
    );
    maybe_insert_string_value(&mut metadata, "key_name", snapshot.key_name.as_deref());
    maybe_insert_string_value(
        &mut metadata,
        "planner_kind",
        snapshot.planner_kind.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "route_family",
        snapshot.route_family.as_deref(),
    );
    maybe_insert_string_value(&mut metadata, "route_kind", snapshot.route_kind.as_deref());
    maybe_insert_string_value(
        &mut metadata,
        "execution_path",
        snapshot.execution_path.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "local_execution_runtime_miss_reason",
        snapshot.local_execution_runtime_miss_reason.as_deref(),
    );
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

fn prepare_request_metadata_for_body_storage<const N: usize>(
    metadata: Option<Value>,
    body_fields: [(
        UsageBodyField,
        &UsageBodyStorage,
        Option<&Value>,
        Option<&str>,
    ); N],
) -> Option<Value> {
    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => {
            let mut object = Map::new();
            object.insert("request_metadata".to_string(), value);
            object
        }
        None => Map::new(),
    };
    let should_replace = !metadata.is_empty()
        || body_fields.iter().any(|(_, storage, value, explicit_ref)| {
            storage.has_detached_blob() || value.is_some() || explicit_ref.is_some()
        });
    if !should_replace {
        return None;
    }

    for (field, storage, value, explicit_ref) in body_fields {
        if storage.has_detached_blob() || value.is_some() || explicit_ref.is_some() {
            let ref_key = field.as_ref_key();
            metadata.remove(ref_key);
        }
    }

    Some(Value::Object(metadata))
}

async fn sync_usage_body_blob_storage<'e, E>(
    executor: E,
    request_id: &str,
    field: UsageBodyField,
    value: Option<&Value>,
    storage: &UsageBodyStorage,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let body_ref = usage_body_ref(request_id, field);
    if let Some(payload_gzip) = storage.detached_blob_bytes.as_ref() {
        sqlx::query(UPSERT_USAGE_BODY_BLOB_SQL)
            .bind(&body_ref)
            .bind(request_id)
            .bind(field.as_storage_field())
            .bind(payload_gzip)
            .execute(executor)
            .await
            .map_postgres_err()?;
        return Ok(());
    }

    if value.is_some() {
        sqlx::query(DELETE_USAGE_BODY_BLOB_SQL)
            .bind(&body_ref)
            .execute(executor)
            .await
            .map_postgres_err()?;
    }

    Ok(())
}

async fn sync_usage_http_audit_storage<'e, E>(
    executor: E,
    request_id: &str,
    headers: &UsageHttpAuditHeaders<'_>,
    refs: &UsageHttpAuditRefs,
    body_capture_mode: &str,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    if !headers.any_present() && !refs.any_present() && body_capture_mode == "none" {
        return Ok(());
    }

    sqlx::query(UPSERT_USAGE_HTTP_AUDIT_SQL)
        .bind(request_id)
        .bind(headers.request_headers_json)
        .bind(headers.provider_request_headers_json)
        .bind(headers.response_headers_json)
        .bind(headers.client_response_headers_json)
        .bind(refs.request_body_ref.as_deref())
        .bind(refs.provider_request_body_ref.as_deref())
        .bind(refs.response_body_ref.as_deref())
        .bind(refs.client_response_body_ref.as_deref())
        .bind(body_capture_mode)
        .execute(executor)
        .await
        .map_postgres_err()?;

    Ok(())
}

async fn sync_usage_routing_snapshot_storage<'e, E>(
    executor: E,
    request_id: &str,
    snapshot: &UsageRoutingSnapshot,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    if !snapshot.any_present() {
        return Ok(());
    }

    sqlx::query(UPSERT_USAGE_ROUTING_SNAPSHOT_SQL)
        .bind(request_id)
        .bind(snapshot.candidate_id.as_deref())
        .bind(snapshot.candidate_index.map(to_i32).transpose()?)
        .bind(snapshot.key_name.as_deref())
        .bind(snapshot.planner_kind.as_deref())
        .bind(snapshot.route_family.as_deref())
        .bind(snapshot.route_kind.as_deref())
        .bind(snapshot.execution_path.as_deref())
        .bind(snapshot.local_execution_runtime_miss_reason.as_deref())
        .bind(snapshot.selected_provider_id.as_deref())
        .bind(snapshot.selected_endpoint_id.as_deref())
        .bind(snapshot.selected_provider_api_key_id.as_deref())
        .bind(snapshot.has_format_conversion)
        .execute(executor)
        .await
        .map_postgres_err()?;

    Ok(())
}

async fn sync_usage_settlement_pricing_snapshot_storage<'e, E>(
    executor: E,
    request_id: &str,
    snapshot: &UsageSettlementPricingSnapshot,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    if !snapshot.any_present() {
        return Ok(());
    }

    sqlx::query(UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL)
        .bind(request_id)
        .bind(snapshot.billing_status.as_deref().unwrap_or("pending"))
        .bind(snapshot.billing_snapshot_schema_version.as_deref())
        .bind(snapshot.billing_snapshot_status.as_deref())
        .bind(snapshot.rate_multiplier)
        .bind(snapshot.is_free_tier)
        .bind(snapshot.input_price_per_1m)
        .bind(snapshot.output_price_per_1m)
        .bind(snapshot.cache_creation_price_per_1m)
        .bind(snapshot.cache_read_price_per_1m)
        .bind(snapshot.price_per_request)
        .execute(executor)
        .await
        .map_postgres_err()?;

    Ok(())
}

#[cfg(test)]
fn maybe_insert_usage_body_ref(
    metadata: &mut Map<String, Value>,
    key: &str,
    request_id: &str,
    field: &str,
    should_insert: bool,
) {
    if !should_insert || metadata.contains_key(key) {
        return;
    }
    metadata.insert(
        key.to_string(),
        Value::String(usage_body_ref(
            request_id,
            UsageBodyField::from_storage_field(field).expect("known usage body field"),
        )),
    );
}

fn maybe_insert_string_value(metadata: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if metadata.contains_key(key) {
        return;
    }
    metadata.insert(key.to_string(), Value::String(value.to_string()));
}

fn maybe_insert_number_value(metadata: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return;
    };
    if metadata.contains_key(key) {
        return;
    }
    let Some(number) = serde_json::Number::from_f64(value) else {
        return;
    };
    metadata.insert(key.to_string(), Value::Number(number));
}

fn maybe_insert_bool_value(metadata: &mut Map<String, Value>, key: &str, value: Option<bool>) {
    let Some(value) = value else {
        return;
    };
    if metadata.contains_key(key) {
        return;
    }
    metadata.insert(key.to_string(), Value::Bool(value));
}

fn usage_routing_snapshot_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<UsageRoutingSnapshot, DataLayerError> {
    Ok(UsageRoutingSnapshot {
        candidate_id: row.try_get("routing_candidate_id").map_postgres_err()?,
        candidate_index: row
            .try_get::<Option<i32>, _>("routing_candidate_index")
            .map_postgres_err()?
            .map(|value| to_u64(value, "usage_routing_snapshots.candidate_index"))
            .transpose()?,
        key_name: row.try_get("routing_key_name").map_postgres_err()?,
        planner_kind: row.try_get("routing_planner_kind").map_postgres_err()?,
        route_family: row.try_get("routing_route_family").map_postgres_err()?,
        route_kind: row.try_get("routing_route_kind").map_postgres_err()?,
        execution_path: row.try_get("routing_execution_path").map_postgres_err()?,
        local_execution_runtime_miss_reason: row
            .try_get("routing_local_execution_runtime_miss_reason")
            .map_postgres_err()?,
        selected_provider_id: None,
        selected_endpoint_id: None,
        selected_provider_api_key_id: None,
        has_format_conversion: None,
    })
}

fn usage_settlement_pricing_snapshot_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<UsageSettlementPricingSnapshot, DataLayerError> {
    Ok(UsageSettlementPricingSnapshot {
        billing_status: None,
        billing_snapshot_schema_version: row
            .try_get("settlement_billing_snapshot_schema_version")
            .map_postgres_err()?,
        billing_snapshot_status: row
            .try_get("settlement_billing_snapshot_status")
            .map_postgres_err()?,
        rate_multiplier: row
            .try_get("settlement_rate_multiplier")
            .map_postgres_err()?,
        is_free_tier: row.try_get("settlement_is_free_tier").map_postgres_err()?,
        input_price_per_1m: row
            .try_get("settlement_input_price_per_1m")
            .map_postgres_err()?,
        output_price_per_1m: row
            .try_get("settlement_output_price_per_1m")
            .map_postgres_err()?,
        cache_creation_price_per_1m: row
            .try_get("settlement_cache_creation_price_per_1m")
            .map_postgres_err()?,
        cache_read_price_per_1m: row
            .try_get("settlement_cache_read_price_per_1m")
            .map_postgres_err()?,
        price_per_request: row
            .try_get("settlement_price_per_request")
            .map_postgres_err()?,
    })
}

fn attach_usage_settlement_pricing_snapshot_metadata(
    metadata: Option<Value>,
    snapshot: &UsageSettlementPricingSnapshot,
) -> Option<Value> {
    if !snapshot.any_present() {
        return metadata;
    }

    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => return Some(value),
        None => Map::new(),
    };
    maybe_insert_string_value(
        &mut metadata,
        "billing_snapshot_schema_version",
        snapshot.billing_snapshot_schema_version.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "billing_snapshot_status",
        snapshot.billing_snapshot_status.as_deref(),
    );
    maybe_insert_number_value(&mut metadata, "rate_multiplier", snapshot.rate_multiplier);
    maybe_insert_bool_value(&mut metadata, "is_free_tier", snapshot.is_free_tier);
    maybe_insert_number_value(
        &mut metadata,
        "input_price_per_1m",
        snapshot.input_price_per_1m,
    );
    maybe_insert_number_value(
        &mut metadata,
        "output_price_per_1m",
        snapshot.output_price_per_1m,
    );
    maybe_insert_number_value(
        &mut metadata,
        "cache_creation_price_per_1m",
        snapshot.cache_creation_price_per_1m,
    );
    maybe_insert_number_value(
        &mut metadata,
        "cache_read_price_per_1m",
        snapshot.cache_read_price_per_1m,
    );
    maybe_insert_number_value(
        &mut metadata,
        "price_per_request",
        snapshot.price_per_request,
    );
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

fn usage_body_sql_columns(field: UsageBodyField) -> (&'static str, &'static str) {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        attach_compressed_body_refs, attach_usage_http_audit_body_refs,
        attach_usage_routing_snapshot_metadata, attach_usage_settlement_pricing_snapshot_metadata,
        inflate_usage_json_value, prepare_request_metadata_for_body_storage,
        prepare_usage_body_storage, resolved_read_usage_body_ref, resolved_write_usage_body_ref,
        usage_body_ref, usage_http_audit_body_refs, usage_http_audit_capture_mode,
        usage_routing_snapshot_from_usage, usage_settlement_pricing_snapshot_from_usage,
        SqlxUsageReadRepository, UsageHttpAuditRefs, UsageRoutingSnapshot,
        UsageSettlementPricingSnapshot, MAX_INLINE_USAGE_BODY_BYTES,
    };
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};
    use crate::repository::usage::UpsertUsageRecord;
    use aether_data_contracts::repository::usage::UsageBodyField;

    #[tokio::test]
    async fn repository_constructs_from_lazy_pool() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");

        let pool = factory.connect_lazy().expect("pool should build");
        let repository = SqlxUsageReadRepository::new(pool);
        let _ = repository.pool();
        let _ = repository.transaction_runner();
    }

    #[tokio::test]
    async fn validates_upsert_before_hitting_database() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");

        let pool = factory.connect_lazy().expect("pool should build");
        let repository = SqlxUsageReadRepository::new(pool);
        let result = repository
            .upsert(UpsertUsageRecord {
                request_id: "".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: None,
                provider_endpoint_id: None,
                provider_api_key_id: None,
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(false),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30),
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: Some(200),
                error_message: None,
                error_category: None,
                response_time_ms: Some(100),
                first_byte_time_ms: None,
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                request_headers: None,
                request_body: None,
                request_body_ref: None,
                provider_request_headers: None,
                provider_request_body: None,
                provider_request_body_ref: None,
                response_headers: None,
                response_body: None,
                response_body_ref: None,
                client_response_headers: None,
                client_response_body: None,
                client_response_body_ref: None,
                candidate_id: None,
                candidate_index: None,
                key_name: None,
                planner_kind: None,
                route_family: None,
                route_kind: None,
                execution_path: None,
                local_execution_runtime_miss_reason: None,
                request_metadata: None,
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 101,
            })
            .await;

        assert!(result.is_err());
    }

    #[test]
    fn usage_sql_does_not_require_updated_at_column() {
        assert!(!super::FIND_BY_REQUEST_ID_SQL.contains("COALESCE(updated_at, created_at)"));
        assert!(!super::LIST_USAGE_AUDITS_PREFIX.contains("COALESCE(updated_at, created_at)"));
        assert!(!super::UPSERT_SQL.contains("\n  updated_at\n"));
        assert!(!super::UPSERT_SQL.contains("updated_at = CASE"));
    }

    #[test]
    fn usage_sql_summarizes_tokens_by_api_key_ids_in_database() {
        assert!(super::SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL.contains("GROUP BY api_key_id"));
        assert!(super::SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL.contains("ANY($1::TEXT[])"));
    }

    #[test]
    fn usage_sql_summarizes_usage_by_provider_api_key_ids_in_database() {
        assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL
            .contains("GROUP BY provider_api_key_id"));
        assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL.contains("MAX(created_at)"));
        assert!(super::SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL.contains("ANY($1::TEXT[])"));
    }

    #[test]
    fn usage_sql_supports_recent_usage_audits_query() {
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("FROM \"usage\""));
    }

    #[test]
    fn usage_sql_reads_http_audits_for_single_record_fetches() {
        assert!(super::FIND_BY_REQUEST_ID_SQL.contains("LEFT JOIN usage_http_audits"));
        assert!(super::FIND_BY_ID_SQL.contains("LEFT JOIN usage_http_audits"));
        assert!(super::FIND_BY_REQUEST_ID_SQL.contains("http_request_body_ref"));
        assert!(super::FIND_BY_ID_SQL.contains("http_client_response_body_ref"));
    }

    #[test]
    fn usage_sql_reads_routing_snapshots_for_single_record_fetches() {
        assert!(super::FIND_BY_REQUEST_ID_SQL.contains("LEFT JOIN usage_routing_snapshots"));
        assert!(super::FIND_BY_ID_SQL.contains("LEFT JOIN usage_routing_snapshots"));
        assert!(super::FIND_BY_REQUEST_ID_SQL.contains("routing_candidate_id"));
        assert!(super::FIND_BY_REQUEST_ID_SQL.contains("routing_candidate_index"));
        assert!(super::FIND_BY_ID_SQL.contains("routing_local_execution_runtime_miss_reason"));
    }

    #[test]
    fn usage_sql_reads_settlement_snapshots_for_single_record_fetches() {
        assert!(super::FIND_BY_REQUEST_ID_SQL.contains("LEFT JOIN usage_settlement_snapshots"));
        assert!(super::FIND_BY_ID_SQL.contains("LEFT JOIN usage_settlement_snapshots"));
        assert!(
            super::FIND_BY_REQUEST_ID_SQL.contains("settlement_billing_snapshot_schema_version")
        );
        assert!(super::FIND_BY_ID_SQL.contains("settlement_price_per_request"));
    }

    #[test]
    fn usage_sql_qualifies_shared_usage_columns_for_single_record_fetches() {
        for sql in [super::FIND_BY_REQUEST_ID_SQL, super::FIND_BY_ID_SQL] {
            assert!(sql.contains("\"usage\".request_id"));
            assert!(
                sql.contains(
                    "COALESCE(usage_settlement_snapshots.billing_status, \"usage\".billing_status) AS billing_status"
                )
            );
            assert!(sql.contains(
                "CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION)"
            ));
            assert!(sql.contains("EXTRACT(EPOCH FROM \"usage\".created_at)"));
            assert!(sql.contains("usage_settlement_snapshots.finalized_at"));
            assert!(!sql.contains("CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT)"));
            assert!(!sql.contains("CAST(output_price_per_1m AS DOUBLE PRECISION)"));
        }

        assert!(super::FIND_BY_REQUEST_ID_SQL.contains("WHERE \"usage\".request_id = $1"));
        assert!(super::FIND_BY_ID_SQL.contains("WHERE \"usage\".id = $1"));
    }

    #[test]
    fn usage_sql_uses_json_null_placeholders_for_usage_payload_columns() {
        assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::json AS request_headers"));
        assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::json AS provider_request_body"));
        assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::bytea AS request_body_compressed"));
        assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::varchar AS http_request_body_ref"));
        assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::varchar AS routing_candidate_id"));
        assert!(
            super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::integer AS routing_candidate_index")
        );
        assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("LEFT JOIN usage_settlement_snapshots"));
        assert!(
            super::LIST_USAGE_AUDITS_PREFIX.contains("settlement_billing_snapshot_schema_version")
        );
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::json AS request_headers"));
        assert!(
            super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::json AS provider_request_body")
        );
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
            .contains("NULL::bytea AS client_response_body_compressed"));
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
            .contains("NULL::varchar AS http_client_response_body_ref"));
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
            .contains("NULL::integer AS routing_candidate_index"));
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
            .contains("NULL::varchar AS routing_execution_path"));
        assert!(
            super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("LEFT JOIN usage_settlement_snapshots")
        );
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("settlement_price_per_request"));
        assert!(!super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::jsonb"));
        assert!(!super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::jsonb"));
    }

    #[test]
    fn usage_sql_reads_list_output_price_from_settlement_snapshots_before_legacy_usage_column() {
        assert!(super::LIST_USAGE_AUDITS_PREFIX
            .contains("CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION)"));
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX
            .contains("CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION)"));
    }

    #[test]
    fn usage_sql_casts_json_payload_bind_parameters_explicitly() {
        for placeholder in [41, 42, 44, 45, 47, 48, 50, 51, 53] {
            assert!(
                super::UPSERT_SQL.contains(format!("${placeholder}::json").as_str()),
                "missing ::json cast for placeholder ${placeholder}"
            );
        }
    }

    #[test]
    fn usage_sql_insert_values_aligns_request_metadata_and_timestamps() {
        assert!(super::UPSERT_SQL.contains("\n  $51::json,\n  $52,\n  $53::json,\n  CASE"));
        assert!(super::UPSERT_SQL.contains("WHEN $54 IS NULL THEN NULL"));
        assert!(super::UPSERT_SQL.contains("TO_TIMESTAMP($55::double precision)"));
    }

    #[test]
    fn usage_sql_upsert_returning_includes_routing_placeholders() {
        assert!(super::UPSERT_SQL.contains("NULL::varchar AS routing_candidate_id"));
        assert!(super::UPSERT_SQL.contains("NULL::varchar AS routing_planner_kind"));
        assert!(super::UPSERT_SQL.contains("NULL::varchar AS routing_execution_path"));
        assert!(super::UPSERT_SQL
            .contains("NULL::varchar AS settlement_billing_snapshot_schema_version"));
        assert!(
            super::UPSERT_SQL.contains("NULL::double precision AS settlement_input_price_per_1m")
        );
    }

    #[test]
    fn usage_sql_dual_writes_usage_settlement_pricing_snapshots() {
        assert!(super::UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL
            .contains("INSERT INTO usage_settlement_snapshots"));
        assert!(super::UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL
            .contains("billing_snapshot_schema_version"));
        assert!(super::UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL.contains("price_per_request"));
    }

    #[test]
    fn usage_sql_clears_legacy_output_price_column_on_upsert() {
        assert!(super::UPSERT_SQL.contains("output_price_per_1m = NULL"));
        assert!(include_str!("sql.rs").contains(".bind(None::<f64>)"));
    }

    #[test]
    fn usage_sql_clears_legacy_header_columns_on_upsert() {
        assert!(super::UPSERT_SQL.contains("request_headers = NULL"));
        assert!(super::UPSERT_SQL.contains("provider_request_headers = NULL"));
        assert!(super::UPSERT_SQL.contains("response_headers = NULL"));
        assert!(super::UPSERT_SQL.contains("client_response_headers = NULL"));
    }

    #[test]
    fn usage_sql_detached_body_flags_clear_inline_and_compressed_columns() {
        assert!(super::UPSERT_SQL
            .contains("WHEN EXCLUDED.request_body_compressed IS NOT NULL OR $56 THEN NULL"));
        assert!(super::UPSERT_SQL.contains(
            "WHEN EXCLUDED.provider_request_body_compressed IS NOT NULL OR $57 THEN NULL"
        ));
        assert!(super::UPSERT_SQL
            .contains("WHEN EXCLUDED.response_body_compressed IS NOT NULL OR $58 THEN NULL"));
        assert!(super::UPSERT_SQL.contains(
            "WHEN EXCLUDED.client_response_body_compressed IS NOT NULL OR $59 THEN NULL"
        ));
    }

    #[test]
    fn usage_sql_clears_stale_failure_fields_for_non_failed_status_updates() {
        assert!(super::UPSERT_SQL.contains(
            "WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') AND EXCLUDED.status_code IS NULL THEN NULL"
        ));
        assert!(super::UPSERT_SQL.contains(
            "WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') THEN EXCLUDED.error_message"
        ));
        assert!(super::UPSERT_SQL.contains(
            "WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') THEN EXCLUDED.error_category"
        ));
    }

    #[test]
    fn usage_sql_does_not_allow_streaming_to_regress_back_to_pending() {
        assert!(super::UPSERT_SQL.contains(
            "WHEN \"usage\".status = 'streaming' AND EXCLUDED.status = 'pending' THEN \"usage\".status_code"
        ));
        assert!(super::UPSERT_SQL.contains(
            "WHEN \"usage\".status = 'streaming' AND EXCLUDED.status = 'pending' THEN \"usage\".error_message"
        ));
        assert!(super::UPSERT_SQL.contains(
            "WHEN \"usage\".status = 'streaming' AND EXCLUDED.status = 'pending' THEN \"usage\".status"
        ));
    }

    #[test]
    fn usage_sql_recovers_void_failures_before_upsert_and_settlement() {
        assert!(super::RESET_STALE_VOID_USAGE_SQL.contains("UPDATE \"usage\""));
        assert!(super::RESET_STALE_VOID_USAGE_SQL.contains("billing_status = 'pending'"));
        assert!(super::RESET_STALE_VOID_USAGE_SQL.contains("finalized_at = NULL"));
        assert!(super::RESET_STALE_VOID_USAGE_SQL.contains("status IN ('failed', 'cancelled')"));
        assert!(super::RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL
            .contains("UPDATE usage_settlement_snapshots"));
        assert!(super::RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL
            .contains("billing_status = 'pending'"));
        assert!(
            super::RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL.contains("finalized_at = NULL")
        );
    }

    #[test]
    fn prepare_usage_body_storage_detaches_small_payloads_into_blob_storage() {
        let payload = json!({"message": "hello"});
        let storage = prepare_usage_body_storage(Some(&payload)).expect("storage should serialize");

        assert!(storage.inline_json.is_none());
        let compressed = storage
            .detached_blob_bytes
            .as_deref()
            .expect("small payload should now be ref-backed");
        assert_eq!(
            inflate_usage_json_value(compressed).expect("payload should inflate"),
            payload
        );
    }

    #[test]
    fn prepare_usage_body_storage_compresses_large_payloads() {
        let payload = json!({
            "content": "x".repeat(MAX_INLINE_USAGE_BODY_BYTES + 128)
        });
        let storage = prepare_usage_body_storage(Some(&payload)).expect("storage should serialize");

        assert!(storage.inline_json.is_none());
        let compressed = storage
            .detached_blob_bytes
            .as_deref()
            .expect("large payload should be compressed");
        assert_eq!(
            inflate_usage_json_value(compressed).expect("payload should inflate"),
            payload
        );
    }

    #[test]
    fn prepare_request_metadata_for_body_storage_strips_body_ref_compatibility_keys() {
        let detached = prepare_usage_body_storage(Some(&json!({
            "content": "x".repeat(MAX_INLINE_USAGE_BODY_BYTES + 32)
        })))
        .expect("detached storage should build");
        let inline =
            prepare_usage_body_storage(Some(&json!({"message": "inline"}))).expect("inline body");

        let metadata = prepare_request_metadata_for_body_storage(
            Some(json!({
                "trace_id": "trace-1",
                "request_body_ref": "blob://old-request",
                "provider_request_body_ref": "blob://old-provider"
            })),
            [
                (
                    UsageBodyField::RequestBody,
                    &detached,
                    Some(&json!({"request": true})),
                    Some("usage://request/req-123/request_body"),
                ),
                (
                    UsageBodyField::ProviderRequestBody,
                    &inline,
                    Some(&json!({"provider": true})),
                    None,
                ),
            ],
        )
        .expect("metadata should be present");

        assert_eq!(
            metadata,
            json!({
                "trace_id": "trace-1"
            })
        );
    }

    #[test]
    fn attach_compressed_body_refs_adds_missing_ref_metadata() {
        let metadata = attach_compressed_body_refs(
            "req-123",
            Some(json!({
                "candidate_id": "cand-1",
                "provider_request_body_ref": "blob://existing"
            })),
            true,
            true,
            true,
            false,
        )
        .expect("metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "candidate_id": "cand-1",
                "request_body_ref": usage_body_ref("req-123", UsageBodyField::RequestBody),
                "provider_request_body_ref": "blob://existing",
                "response_body_ref": usage_body_ref("req-123", UsageBodyField::ResponseBody)
            })
        );
    }

    #[test]
    fn usage_http_audit_body_refs_extracts_only_non_empty_values() {
        let refs = usage_http_audit_body_refs(Some(&json!({
            "request_body_ref": "usage://request/req-123/request_body",
            "provider_request_body_ref": "  ",
            "response_body_ref": "usage://request/req-123/response_body"
        })));

        assert_eq!(
            refs,
            UsageHttpAuditRefs {
                request_body_ref: Some("usage://request/req-123/request_body".to_string()),
                provider_request_body_ref: None,
                response_body_ref: Some("usage://request/req-123/response_body".to_string()),
                client_response_body_ref: None,
            }
        );
    }

    #[test]
    fn resolved_read_usage_body_ref_prefers_typed_then_http_audit_then_compressed_then_metadata() {
        let metadata = json!({
            "request_body_ref": "usage://request/req-123/request_body"
        });
        let invalid_metadata = json!({
            "request_body_ref": "blob://metadata-request"
        });
        let mismatched_metadata = json!({
            "request_body_ref": "usage://request/req-other/request_body"
        });

        assert_eq!(
            resolved_read_usage_body_ref(
                Some("usage://request/req-123/request_body"),
                metadata.as_object(),
                "req-123",
                UsageBodyField::RequestBody,
                true,
                Some("usage://request/req-123/request_body"),
            ),
            Some("usage://request/req-123/request_body".to_string())
        );
        assert_eq!(
            resolved_read_usage_body_ref(
                None,
                metadata.as_object(),
                "req-123",
                UsageBodyField::RequestBody,
                false,
                Some("usage://request/req-123/request_body"),
            ),
            Some("usage://request/req-123/request_body".to_string())
        );
        assert_eq!(
            resolved_read_usage_body_ref(
                None,
                metadata.as_object(),
                "req-123",
                UsageBodyField::RequestBody,
                true,
                None,
            ),
            Some(usage_body_ref("req-123", UsageBodyField::RequestBody))
        );
        assert_eq!(
            resolved_read_usage_body_ref(
                None,
                invalid_metadata.as_object(),
                "req-123",
                UsageBodyField::RequestBody,
                false,
                None,
            ),
            None
        );
        assert_eq!(
            resolved_read_usage_body_ref(
                None,
                mismatched_metadata.as_object(),
                "req-123",
                UsageBodyField::RequestBody,
                false,
                None,
            ),
            None
        );
        assert_eq!(
            resolved_read_usage_body_ref(
                None,
                None,
                "req-123",
                UsageBodyField::ResponseBody,
                true,
                Some("usage://request/req-123/response_body"),
            ),
            Some(usage_body_ref("req-123", UsageBodyField::ResponseBody))
        );
        assert_eq!(
            resolved_read_usage_body_ref(
                None,
                None,
                "req-123",
                UsageBodyField::ClientResponseBody,
                false,
                Some("usage://request/req-123/client_response_body"),
            ),
            Some("usage://request/req-123/client_response_body".to_string())
        );
    }

    #[test]
    fn resolved_write_usage_body_ref_ignores_metadata_compatibility_keys() {
        assert_eq!(
            resolved_write_usage_body_ref(
                None,
                "req-123",
                UsageBodyField::RequestBody,
                false,
                None,
            ),
            None
        );
        assert_eq!(
            resolved_write_usage_body_ref(
                Some("usage://request/req-123/request_body"),
                "req-123",
                UsageBodyField::RequestBody,
                true,
                Some("usage://request/req-123/request_body"),
            ),
            Some("usage://request/req-123/request_body".to_string())
        );
        assert_eq!(
            resolved_write_usage_body_ref(
                None,
                "req-123",
                UsageBodyField::ResponseBody,
                true,
                Some("usage://request/req-123/response_body"),
            ),
            Some(usage_body_ref("req-123", UsageBodyField::ResponseBody))
        );
        assert_eq!(
            resolved_write_usage_body_ref(
                None,
                "req-123",
                UsageBodyField::ClientResponseBody,
                false,
                Some("usage://request/req-123/client_response_body"),
            ),
            Some("usage://request/req-123/client_response_body".to_string())
        );
    }

    #[test]
    fn usage_http_audit_capture_mode_prefers_refs_over_inline_legacy() {
        let refs = UsageHttpAuditRefs {
            request_body_ref: Some("usage://request/req-123/request_body".to_string()),
            ..UsageHttpAuditRefs::default()
        };
        assert_eq!(
            usage_http_audit_capture_mode(
                &refs,
                [Some(&json!({"request": true})), None, None, None]
            ),
            "ref_backed"
        );
        assert_eq!(
            usage_http_audit_capture_mode(
                &UsageHttpAuditRefs::default(),
                [Some(&json!({"request": true})), None, None, None]
            ),
            "inline_legacy"
        );
        assert_eq!(
            usage_http_audit_capture_mode(&UsageHttpAuditRefs::default(), [None, None, None, None]),
            "none"
        );
    }

    #[test]
    fn attach_usage_http_audit_body_refs_adds_missing_metadata_without_overwriting_existing_keys() {
        let metadata = attach_usage_http_audit_body_refs(
            Some(json!({
                "candidate_id": "cand-1",
                "request_body_ref": "blob://existing"
            })),
            &UsageHttpAuditRefs {
                request_body_ref: Some("usage://request/req-123/request_body".to_string()),
                provider_request_body_ref: Some(
                    "usage://request/req-123/provider_request_body".to_string(),
                ),
                response_body_ref: None,
                client_response_body_ref: Some(
                    "usage://request/req-123/client_response_body".to_string(),
                ),
            },
        )
        .expect("metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "candidate_id": "cand-1",
                "request_body_ref": "blob://existing",
                "provider_request_body_ref": "usage://request/req-123/provider_request_body",
                "client_response_body_ref": "usage://request/req-123/client_response_body"
            })
        );
    }

    #[test]
    fn usage_routing_snapshot_from_usage_only_activates_for_routing_metadata() {
        let snapshot = usage_routing_snapshot_from_usage(
            &UpsertUsageRecord {
                request_id: "req-123".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(false),
                input_tokens: Some(1),
                output_tokens: Some(2),
                total_tokens: Some(3),
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: Some(200),
                error_message: None,
                error_category: None,
                response_time_ms: Some(100),
                first_byte_time_ms: None,
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                request_headers: None,
                request_body: None,
                request_body_ref: None,
                provider_request_headers: None,
                provider_request_body: None,
                provider_request_body_ref: None,
                response_headers: None,
                response_body: None,
                response_body_ref: None,
                client_response_headers: None,
                client_response_body: None,
                client_response_body_ref: None,
                candidate_id: None,
                candidate_index: None,
                key_name: None,
                planner_kind: None,
                route_family: None,
                route_kind: None,
                execution_path: None,
                local_execution_runtime_miss_reason: None,
                request_metadata: None,
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 100,
            },
            Some(&json!({
                "candidate_id": "cand-1",
                "key_name": "primary",
                "planner_kind": "claude_cli_sync",
                "route_family": "claude",
                "route_kind": "cli",
                "execution_path": "local_execution_runtime_miss",
                "local_execution_runtime_miss_reason": "all_candidates_skipped"
            })),
        );

        assert_eq!(
            snapshot,
            UsageRoutingSnapshot {
                candidate_id: Some("cand-1".to_string()),
                candidate_index: None,
                key_name: Some("primary".to_string()),
                planner_kind: Some("claude_cli_sync".to_string()),
                route_family: Some("claude".to_string()),
                route_kind: Some("cli".to_string()),
                execution_path: Some("local_execution_runtime_miss".to_string()),
                local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
                selected_provider_id: Some("provider-1".to_string()),
                selected_endpoint_id: Some("endpoint-1".to_string()),
                selected_provider_api_key_id: Some("provider-key-1".to_string()),
                has_format_conversion: Some(false),
            }
        );

        let empty_snapshot = usage_routing_snapshot_from_usage(
            &UpsertUsageRecord {
                request_id: "req-124".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-2".to_string()),
                provider_endpoint_id: Some("endpoint-2".to_string()),
                provider_api_key_id: Some("provider-key-2".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(true),
                is_stream: Some(false),
                input_tokens: Some(1),
                output_tokens: Some(2),
                total_tokens: Some(3),
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: Some(200),
                error_message: None,
                error_category: None,
                response_time_ms: Some(100),
                first_byte_time_ms: None,
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                request_headers: None,
                request_body: None,
                request_body_ref: None,
                provider_request_headers: None,
                provider_request_body: None,
                provider_request_body_ref: None,
                response_headers: None,
                response_body: None,
                response_body_ref: None,
                client_response_headers: None,
                client_response_body: None,
                client_response_body_ref: None,
                candidate_id: None,
                candidate_index: None,
                key_name: None,
                planner_kind: None,
                route_family: None,
                route_kind: None,
                execution_path: None,
                local_execution_runtime_miss_reason: None,
                request_metadata: None,
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 100,
            },
            Some(&json!({"trace_id": "trace-1"})),
        );

        assert_eq!(empty_snapshot, UsageRoutingSnapshot::default());
    }

    #[test]
    fn usage_routing_snapshot_from_usage_prefers_typed_routing_fields_without_metadata() {
        let snapshot = usage_routing_snapshot_from_usage(
            &UpsertUsageRecord {
                request_id: "req-typed-routing-1".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(true),
                is_stream: Some(false),
                input_tokens: Some(1),
                output_tokens: Some(2),
                total_tokens: Some(3),
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: Some(200),
                error_message: None,
                error_category: None,
                response_time_ms: Some(100),
                first_byte_time_ms: None,
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                request_headers: None,
                request_body: None,
                request_body_ref: None,
                provider_request_headers: None,
                provider_request_body: None,
                provider_request_body_ref: None,
                response_headers: None,
                response_body: None,
                response_body_ref: None,
                client_response_headers: None,
                client_response_body: None,
                client_response_body_ref: None,
                candidate_id: Some("cand-typed".to_string()),
                candidate_index: Some(2),
                key_name: Some("primary".to_string()),
                planner_kind: Some("claude_cli_sync".to_string()),
                route_family: Some("claude".to_string()),
                route_kind: Some("cli".to_string()),
                execution_path: Some("local_execution_runtime_miss".to_string()),
                local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
                request_metadata: Some(json!({
                    "trace_id": "trace-1"
                })),
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 100,
            },
            None,
        );

        assert_eq!(
            snapshot,
            UsageRoutingSnapshot {
                candidate_id: Some("cand-typed".to_string()),
                candidate_index: Some(2),
                key_name: Some("primary".to_string()),
                planner_kind: Some("claude_cli_sync".to_string()),
                route_family: Some("claude".to_string()),
                route_kind: Some("cli".to_string()),
                execution_path: Some("local_execution_runtime_miss".to_string()),
                local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
                selected_provider_id: Some("provider-1".to_string()),
                selected_endpoint_id: Some("endpoint-1".to_string()),
                selected_provider_api_key_id: Some("provider-key-1".to_string()),
                has_format_conversion: Some(true),
            }
        );
    }

    #[test]
    fn attach_usage_routing_snapshot_metadata_adds_missing_keys_without_overwriting_existing_values(
    ) {
        let metadata = attach_usage_routing_snapshot_metadata(
            Some(json!({
                "candidate_id": "cand-existing",
                "route_kind": "cli"
            })),
            &UsageRoutingSnapshot {
                candidate_id: Some("cand-1".to_string()),
                candidate_index: Some(2),
                key_name: Some("primary".to_string()),
                planner_kind: Some("claude_cli_sync".to_string()),
                route_family: Some("claude".to_string()),
                route_kind: Some("chat".to_string()),
                execution_path: Some("local_execution_runtime_miss".to_string()),
                local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
                selected_provider_id: None,
                selected_endpoint_id: None,
                selected_provider_api_key_id: None,
                has_format_conversion: None,
            },
        )
        .expect("metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "candidate_id": "cand-existing",
                "key_name": "primary",
                "planner_kind": "claude_cli_sync",
                "route_family": "claude",
                "route_kind": "cli",
                "execution_path": "local_execution_runtime_miss",
                "local_execution_runtime_miss_reason": "all_candidates_skipped"
            })
        );
    }

    #[test]
    fn usage_settlement_pricing_snapshot_from_usage_extracts_typed_billing_fields() {
        let snapshot = usage_settlement_pricing_snapshot_from_usage(
            &UpsertUsageRecord {
                request_id: "req-125".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(false),
                input_tokens: Some(1),
                output_tokens: Some(2),
                total_tokens: Some(3),
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: Some(15.0),
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: Some(200),
                error_message: None,
                error_category: None,
                response_time_ms: Some(100),
                first_byte_time_ms: None,
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                request_headers: None,
                request_body: None,
                request_body_ref: None,
                provider_request_headers: None,
                provider_request_body: None,
                provider_request_body_ref: None,
                response_headers: None,
                response_body: None,
                response_body_ref: None,
                client_response_headers: None,
                client_response_body: None,
                client_response_body_ref: None,
                candidate_id: None,
                candidate_index: None,
                key_name: None,
                planner_kind: None,
                route_family: None,
                route_kind: None,
                execution_path: None,
                local_execution_runtime_miss_reason: None,
                request_metadata: None,
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 100,
            },
            Some(&json!({
                "rate_multiplier": 0.5,
                "is_free_tier": false,
                "billing_snapshot": {
                    "schema_version": "2.0",
                    "status": "complete",
                    "resolved_variables": {
                        "input_price_per_1m": 3.0,
                        "output_price_per_1m": 15.0,
                        "cache_creation_price_per_1m": 3.75,
                        "cache_read_price_per_1m": 0.30,
                        "price_per_request": 0.02
                    }
                }
            })),
        );

        assert_eq!(
            snapshot,
            UsageSettlementPricingSnapshot {
                billing_status: Some("pending".to_string()),
                billing_snapshot_schema_version: Some("2.0".to_string()),
                billing_snapshot_status: Some("complete".to_string()),
                rate_multiplier: Some(0.5),
                is_free_tier: Some(false),
                input_price_per_1m: Some(3.0),
                output_price_per_1m: Some(15.0),
                cache_creation_price_per_1m: Some(3.75),
                cache_read_price_per_1m: Some(0.30),
                price_per_request: Some(0.02),
            }
        );
    }

    #[test]
    fn usage_settlement_pricing_snapshot_with_billing_status_only_is_still_persisted() {
        let snapshot = UsageSettlementPricingSnapshot {
            billing_status: Some("pending".to_string()),
            ..UsageSettlementPricingSnapshot::default()
        };

        assert!(snapshot.any_present());
    }

    #[test]
    fn attach_usage_settlement_pricing_snapshot_metadata_adds_missing_values_without_overwriting() {
        let metadata = attach_usage_settlement_pricing_snapshot_metadata(
            Some(json!({
                "rate_multiplier": 1.0,
                "billing_snapshot_status": "complete"
            })),
            &UsageSettlementPricingSnapshot {
                billing_status: None,
                billing_snapshot_schema_version: Some("2.0".to_string()),
                billing_snapshot_status: Some("incomplete".to_string()),
                rate_multiplier: Some(0.5),
                is_free_tier: Some(false),
                input_price_per_1m: Some(3.0),
                output_price_per_1m: Some(15.0),
                cache_creation_price_per_1m: Some(3.75),
                cache_read_price_per_1m: Some(0.30),
                price_per_request: Some(0.02),
            },
        )
        .expect("metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "rate_multiplier": 1.0,
                "billing_snapshot_status": "complete",
                "billing_snapshot_schema_version": "2.0",
                "is_free_tier": false,
                "input_price_per_1m": 3.0,
                "output_price_per_1m": 15.0,
                "cache_creation_price_per_1m": 3.75,
                "cache_read_price_per_1m": 0.30,
                "price_per_request": 0.02
            })
        );
    }
}
