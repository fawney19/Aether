use async_trait::async_trait;
use futures_util::future::BoxFuture;
use futures_util::TryStreamExt;
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::{
    StoredProviderApiKeyUsageSummary, StoredProviderUsageSummary, StoredRequestUsageAudit,
    UpsertUsageRecord, UsageAuditListQuery, UsageReadRepository, UsageWriteRepository,
};
use crate::postgres::PostgresTransactionRunner;
use crate::{error::SqlxResultExt, DataLayerError};

const FIND_BY_REQUEST_ID_SQL: &str = r#"
SELECT
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
  provider_request_headers,
  provider_request_body,
  response_headers,
  response_body,
  client_response_headers,
  client_response_body,
  request_metadata,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM COALESCE(finalized_at, created_at)) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT) AS finalized_at_unix_secs
FROM "usage"
WHERE request_id = $1
LIMIT 1
"#;

const FIND_BY_ID_SQL: &str = r#"
SELECT
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
  provider_request_headers,
  provider_request_body,
  response_headers,
  response_body,
  client_response_headers,
  client_response_body,
  request_metadata,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM COALESCE(finalized_at, created_at)) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT) AS finalized_at_unix_secs
FROM "usage"
WHERE id = $1
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
  NULL::json AS request_headers,
  NULL::json AS request_body,
  NULL::json AS provider_request_headers,
  NULL::json AS provider_request_body,
  NULL::json AS response_headers,
  NULL::json AS response_body,
  NULL::json AS client_response_headers,
  NULL::json AS client_response_body,
  NULL::json AS request_metadata,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM COALESCE(finalized_at, created_at)) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT) AS finalized_at_unix_secs
FROM "usage"
"#;

const LIST_RECENT_USAGE_AUDITS_PREFIX: &str = r#"
SELECT
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
  NULL::json AS request_headers,
  NULL::json AS request_body,
  NULL::json AS provider_request_headers,
  NULL::json AS provider_request_body,
  NULL::json AS response_headers,
  NULL::json AS response_body,
  NULL::json AS client_response_headers,
  NULL::json AS client_response_body,
  NULL::json AS request_metadata,
  CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_ms,
  CAST(EXTRACT(EPOCH FROM COALESCE(finalized_at, created_at)) AS BIGINT) AS updated_at_unix_secs,
  CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT) AS finalized_at_unix_secs
FROM "usage"
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
  provider_request_headers,
  provider_request_body,
  response_headers,
  response_body,
  client_response_headers,
  client_response_body,
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
  $43::json,
  $44::json,
  $45::json,
  $46::json,
  $47::json,
  $48::json,
  $49::json,
  CASE
    WHEN $50 IS NULL THEN NULL
    ELSE TO_TIMESTAMP($50::double precision)
  END,
  COALESCE(TO_TIMESTAMP($51::double precision), NOW())
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
  output_price_per_1m = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.output_price_per_1m, "usage".output_price_per_1m) ELSE "usage".output_price_per_1m END,
  total_cost_usd = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.total_cost_usd, "usage".total_cost_usd) ELSE "usage".total_cost_usd END,
  actual_total_cost_usd = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.actual_total_cost_usd, "usage".actual_total_cost_usd) ELSE "usage".actual_total_cost_usd END,
  status_code = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') AND EXCLUDED.status_code IS NULL THEN NULL
    ELSE COALESCE(EXCLUDED.status_code, "usage".status_code)
  END ELSE "usage".status_code END,
  error_message = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') THEN EXCLUDED.error_message
    ELSE COALESCE(EXCLUDED.error_message, "usage".error_message)
  END ELSE "usage".error_message END,
  error_category = CASE WHEN "usage".billing_status = 'pending' THEN CASE
    WHEN EXCLUDED.status IN ('pending', 'streaming', 'completed', 'cancelled') THEN EXCLUDED.error_category
    ELSE COALESCE(EXCLUDED.error_category, "usage".error_category)
  END ELSE "usage".error_category END,
  response_time_ms = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.response_time_ms, "usage".response_time_ms) ELSE "usage".response_time_ms END,
  first_byte_time_ms = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.first_byte_time_ms, "usage".first_byte_time_ms) ELSE "usage".first_byte_time_ms END,
  status = CASE WHEN "usage".billing_status = 'pending' THEN EXCLUDED.status ELSE "usage".status END,
  billing_status = CASE WHEN "usage".billing_status = 'pending' THEN EXCLUDED.billing_status ELSE "usage".billing_status END,
  request_headers = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.request_headers, "usage".request_headers) ELSE "usage".request_headers END,
  request_body = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.request_body, "usage".request_body) ELSE "usage".request_body END,
  provider_request_headers = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.provider_request_headers, "usage".provider_request_headers) ELSE "usage".provider_request_headers END,
  provider_request_body = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.provider_request_body, "usage".provider_request_body) ELSE "usage".provider_request_body END,
  response_headers = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.response_headers, "usage".response_headers) ELSE "usage".response_headers END,
  response_body = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.response_body, "usage".response_body) ELSE "usage".response_body END,
  client_response_headers = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.client_response_headers, "usage".client_response_headers) ELSE "usage".client_response_headers END,
  client_response_body = CASE WHEN "usage".billing_status = 'pending' THEN COALESCE(EXCLUDED.client_response_body, "usage".client_response_body) ELSE "usage".client_response_body END,
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
  provider_request_headers,
  provider_request_body,
  response_headers,
  response_body,
  client_response_headers,
  client_response_body,
  request_metadata,
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
        row.as_ref().map(map_usage_row).transpose()
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
        row.as_ref().map(map_usage_row).transpose()
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
                .push("created_at >= TO_TIMESTAMP(")
                .push_bind(created_from_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(created_until_unix_secs) = query.created_until_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("created_at < TO_TIMESTAMP(")
                .push_bind(created_until_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("user_id = ").push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder.push("model = ").push_bind(model.to_string());
        }

        builder.push(" ORDER BY created_at ASC, request_id ASC");
        let query = builder.build();
        let mut rows = query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row)?);
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
                .push(" WHERE user_id = ")
                .push_bind(user_id.to_string());
        }
        builder
            .push(" ORDER BY created_at DESC, id ASC LIMIT ")
            .push_bind(i64::try_from(limit).map_err(|_| {
                DataLayerError::InvalidInput(format!("invalid recent usage limit: {limit}"))
            })?);
        let query = builder.build();
        let mut rows = query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row)?);
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
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    let request_headers_json = json_bind_text(usage.request_headers.as_ref())?;
                    let request_body_json = json_bind_text(usage.request_body.as_ref())?;
                    let provider_request_headers_json =
                        json_bind_text(usage.provider_request_headers.as_ref())?;
                    let provider_request_body_json =
                        json_bind_text(usage.provider_request_body.as_ref())?;
                    let response_headers_json = json_bind_text(usage.response_headers.as_ref())?;
                    let response_body_json = json_bind_text(usage.response_body.as_ref())?;
                    let client_response_headers_json =
                        json_bind_text(usage.client_response_headers.as_ref())?;
                    let client_response_body_json =
                        json_bind_text(usage.client_response_body.as_ref())?;
                    let request_metadata_json = json_bind_text(usage.request_metadata.as_ref())?;
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
                        .bind(usage.output_price_per_1m)
                        .bind(usage.total_cost_usd)
                        .bind(usage.actual_total_cost_usd)
                        .bind(usage.status_code.map(i32::from))
                        .bind(&usage.error_message)
                        .bind(&usage.error_category)
                        .bind(usage.response_time_ms.map(to_i32).transpose()?)
                        .bind(usage.first_byte_time_ms.map(to_i32).transpose()?)
                        .bind(&usage.status)
                        .bind(&usage.billing_status)
                        .bind(&request_headers_json)
                        .bind(&request_body_json)
                        .bind(&provider_request_headers_json)
                        .bind(&provider_request_body_json)
                        .bind(&response_headers_json)
                        .bind(&response_body_json)
                        .bind(&client_response_headers_json)
                        .bind(&client_response_body_json)
                        .bind(&request_metadata_json)
                        .bind(usage.finalized_at_unix_secs.map(|value| value as f64))
                        .bind(usage.created_at_unix_ms.map(|value| value as f64))
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    map_usage_row(&row)
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

    async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_usage_audits(self, query).await
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

fn map_usage_row(row: &sqlx::postgres::PgRow) -> Result<StoredRequestUsageAudit, DataLayerError> {
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
    usage.request_body = row.try_get("request_body").map_postgres_err()?;
    usage.provider_request_headers = row.try_get("provider_request_headers").map_postgres_err()?;
    usage.provider_request_body = row.try_get("provider_request_body").map_postgres_err()?;
    usage.response_headers = row.try_get("response_headers").map_postgres_err()?;
    usage.response_body = row.try_get("response_body").map_postgres_err()?;
    usage.client_response_headers = row.try_get("client_response_headers").map_postgres_err()?;
    usage.client_response_body = row.try_get("client_response_body").map_postgres_err()?;
    usage.request_metadata = row.try_get("request_metadata").map_postgres_err()?;
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
mod tests {
    use super::SqlxUsageReadRepository;
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};
    use crate::repository::usage::UpsertUsageRecord;

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
                provider_request_headers: None,
                provider_request_body: None,
                response_headers: None,
                response_body: None,
                client_response_headers: None,
                client_response_body: None,
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
    fn usage_sql_uses_json_null_placeholders_for_usage_payload_columns() {
        assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::json AS request_headers"));
        assert!(super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::json AS provider_request_body"));
        assert!(super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::json AS request_headers"));
        assert!(
            super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::json AS provider_request_body")
        );
        assert!(!super::LIST_USAGE_AUDITS_PREFIX.contains("NULL::jsonb"));
        assert!(!super::LIST_RECENT_USAGE_AUDITS_PREFIX.contains("NULL::jsonb"));
    }

    #[test]
    fn usage_sql_casts_json_payload_bind_parameters_explicitly() {
        for placeholder in 41..=49 {
            assert!(
                super::UPSERT_SQL.contains(format!("${placeholder}::json").as_str()),
                "missing ::json cast for placeholder ${placeholder}"
            );
        }
    }

    #[test]
    fn usage_sql_insert_values_aligns_request_metadata_and_timestamps() {
        assert!(super::UPSERT_SQL.contains("\n  $48::json,\n  $49::json,\n  CASE"));
        assert!(super::UPSERT_SQL.contains("WHEN $50 IS NULL THEN NULL"));
        assert!(super::UPSERT_SQL.contains("TO_TIMESTAMP($51::double precision)"));
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
}
