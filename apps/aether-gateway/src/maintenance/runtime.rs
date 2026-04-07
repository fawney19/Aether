use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use flate2::{write::GzEncoder, Compression};
use futures_util::stream::{self, StreamExt};
use serde_json::Value;
use sqlx::Row;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::data::GatewayDataState;
use crate::handlers::admin::provider::ops::providers::actions::admin_provider_ops_local_action_response;
use crate::{AppState, GatewayError};

#[path = "runtime/audit_cleanup.rs"]
mod audit_cleanup;
#[path = "runtime/config.rs"]
mod config;
#[path = "runtime/db_maintenance.rs"]
mod db_maintenance;
#[path = "runtime/pending_cleanup.rs"]
mod pending_cleanup;
#[path = "runtime/provider_checkin.rs"]
mod provider_checkin;
#[path = "runtime/request_candidate_cleanup.rs"]
mod request_candidate_cleanup;
#[path = "runtime/runners.rs"]
mod runners;
#[path = "runtime/schedule.rs"]
mod schedule;
#[path = "runtime/stats_daily.rs"]
mod stats_daily;
#[path = "runtime/stats_hourly.rs"]
mod stats_hourly;
#[cfg(test)]
#[path = "runtime/tests.rs"]
mod tests;
#[path = "runtime/usage_cleanup.rs"]
mod usage_cleanup;
#[path = "runtime/wallet_daily_usage.rs"]
mod wallet_daily_usage;
#[path = "runtime/workers.rs"]
mod workers;
use audit_cleanup::*;
use config::*;
use db_maintenance::*;
use pending_cleanup::*;
pub(crate) use provider_checkin::{perform_provider_checkin_once, ProviderCheckinRunSummary};
use request_candidate_cleanup::*;
use runners::*;
use schedule::*;
use stats_daily::*;
use stats_hourly::*;
use usage_cleanup::*;
use wallet_daily_usage::*;
pub(crate) use workers::*;

pub(super) fn postgres_error(
    error: impl std::fmt::Display,
) -> aether_data_contracts::DataLayerError {
    aether_data_contracts::DataLayerError::postgres(error)
}

const AUDIT_LOG_CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const GEMINI_FILE_MAPPING_CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const PENDING_CLEANUP_INTERVAL: Duration = Duration::from_secs(5 * 60);
const POOL_MONITOR_INTERVAL: Duration = Duration::from_secs(5 * 60);
const PROVIDER_CHECKIN_CONCURRENCY: usize = 3;
const PROVIDER_CHECKIN_DEFAULT_TIME: &str = "01:05";
const REQUEST_CANDIDATE_CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const STATS_DAILY_AGGREGATION_HOUR: u32 = 0;
const STATS_DAILY_AGGREGATION_MINUTE: u32 = 5;
const STATS_HOURLY_AGGREGATION_MINUTE: u32 = 5;
const EXPIRED_API_KEY_PRE_CLEAN_BATCH_SIZE: usize = 2_000;
const USAGE_CLEANUP_HOUR: u32 = 3;
const USAGE_CLEANUP_MINUTE: u32 = 0;
const WALLET_DAILY_USAGE_AGGREGATION_HOUR: u32 = 0;
const WALLET_DAILY_USAGE_AGGREGATION_MINUTE: u32 = 10;
const DB_MAINTENANCE_WEEKLY_INTERVAL: chrono::Duration = chrono::Duration::days(7);
const DB_MAINTENANCE_WEEKDAY: Weekday = Weekday::Sun;
const DB_MAINTENANCE_HOUR: u32 = 5;
const DB_MAINTENANCE_MINUTE: u32 = 0;
const MAINTENANCE_DEFAULT_TIMEZONE: &str = "Asia/Shanghai";
const DB_MAINTENANCE_TABLES: &[&str] = &["usage", "request_candidates", "audit_logs"];
const SELECT_WALLET_DAILY_USAGE_AGGREGATION_ROWS_SQL: &str = r#"
SELECT
    wallet_id,
    COUNT(id) AS total_requests,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost_usd,
    COALESCE(SUM(input_tokens), 0) AS input_tokens,
    COALESCE(SUM(output_tokens), 0) AS output_tokens,
    COALESCE(SUM(cache_creation_input_tokens), 0) AS cache_creation_tokens,
    COALESCE(SUM(cache_read_input_tokens), 0) AS cache_read_tokens,
    MIN(finalized_at) AS first_finalized_at,
    MAX(finalized_at) AS last_finalized_at
FROM usage
WHERE wallet_id IS NOT NULL
  AND billing_status = 'settled'
  AND total_cost_usd > 0
  AND finalized_at >= $1
  AND finalized_at < $2
GROUP BY wallet_id
"#;
const UPSERT_WALLET_DAILY_USAGE_LEDGER_SQL: &str = r#"
INSERT INTO wallet_daily_usage_ledgers (
    id,
    wallet_id,
    billing_date,
    billing_timezone,
    total_cost_usd,
    total_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    first_finalized_at,
    last_finalized_at,
    aggregated_at,
    created_at,
    updated_at
)
VALUES (
    $1, $2, $3, $4, $5,
    $6, $7, $8, $9, $10,
    $11, $12, $13, $14, $15
)
ON CONFLICT (wallet_id, billing_date, billing_timezone)
DO UPDATE SET
    total_cost_usd = EXCLUDED.total_cost_usd,
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    first_finalized_at = EXCLUDED.first_finalized_at,
    last_finalized_at = EXCLUDED.last_finalized_at,
    aggregated_at = EXCLUDED.aggregated_at,
    updated_at = EXCLUDED.updated_at
"#;
const DELETE_STALE_WALLET_DAILY_USAGE_LEDGERS_SQL: &str = r#"
DELETE FROM wallet_daily_usage_ledgers AS ledgers
WHERE ledgers.billing_date = $1
  AND ledgers.billing_timezone = $2
  AND NOT EXISTS (
      SELECT 1
      FROM usage
      WHERE usage.wallet_id = ledgers.wallet_id
        AND usage.billing_status = 'settled'
        AND usage.total_cost_usd > 0
        AND usage.finalized_at >= $3
        AND usage.finalized_at < $4
  )
"#;
const SELECT_STALE_PENDING_USAGE_BATCH_SQL: &str = r#"
SELECT
  id,
  request_id,
  status,
  billing_status
FROM usage
WHERE status = ANY($1)
  AND created_at < $2
ORDER BY created_at ASC, id ASC
LIMIT $3
FOR UPDATE SKIP LOCKED
"#;
const SELECT_COMPLETED_PENDING_REQUEST_IDS_SQL: &str = r#"
SELECT DISTINCT request_id
FROM request_candidates
WHERE request_id = ANY($1)
  AND (
    status = 'streaming'
    OR (
      status = 'success'
      AND COALESCE(extra_data->>'stream_completed', 'false') = 'true'
    )
  )
"#;
const UPDATE_RECOVERED_STALE_USAGE_SQL: &str = r#"
UPDATE usage
SET status = 'completed',
    status_code = 200,
    error_message = NULL
WHERE id = $1
"#;
const UPDATE_FAILED_STALE_USAGE_SQL: &str = r#"
UPDATE usage
SET status = 'failed',
    status_code = 504,
    error_message = $2
WHERE id = $1
"#;
const UPDATE_FAILED_VOID_STALE_USAGE_SQL: &str = r#"
UPDATE usage
SET status = 'failed',
    status_code = 504,
    error_message = $2,
    billing_status = 'void',
    finalized_at = $3,
    total_cost_usd = 0,
    request_cost_usd = 0,
    actual_total_cost_usd = 0,
    actual_request_cost_usd = 0
WHERE id = $1
"#;
const UPDATE_RECOVERED_STREAMING_CANDIDATES_SQL: &str = r#"
UPDATE request_candidates
SET status = 'success',
    finished_at = $2
WHERE request_id = ANY($1)
  AND status = 'streaming'
"#;
const UPDATE_FAILED_PENDING_CANDIDATES_SQL: &str = r#"
UPDATE request_candidates
SET status = 'failed',
    finished_at = $2,
    error_message = '请求超时（服务器可能已重启）'
WHERE request_id = ANY($1)
  AND status = ANY($3)
"#;
const DELETE_OLD_USAGE_RECORDS_SQL: &str = r#"
WITH doomed AS (
    SELECT id
    FROM usage
    WHERE created_at < $1
    ORDER BY created_at ASC, id ASC
    LIMIT $2
)
DELETE FROM usage AS usage_rows
USING doomed
WHERE usage_rows.id = doomed.id
"#;
const SELECT_USAGE_HEADER_BATCH_SQL: &str = r#"
SELECT id
FROM usage
WHERE created_at < $1
  AND ($2::timestamptz IS NULL OR created_at >= $2)
  AND (
    request_headers IS NOT NULL
    OR response_headers IS NOT NULL
    OR provider_request_headers IS NOT NULL
    OR client_response_headers IS NOT NULL
  )
ORDER BY created_at ASC, id ASC
LIMIT $3
"#;
const CLEAR_USAGE_HEADER_FIELDS_SQL: &str = r#"
UPDATE usage
SET request_headers = NULL,
    response_headers = NULL,
    provider_request_headers = NULL,
    client_response_headers = NULL
WHERE id = ANY($1)
"#;
const SELECT_USAGE_STALE_BODY_BATCH_SQL: &str = r#"
SELECT id
FROM usage
WHERE created_at < $1
  AND ($2::timestamptz IS NULL OR created_at >= $2)
  AND (
    request_body IS NOT NULL
    OR response_body IS NOT NULL
    OR provider_request_body IS NOT NULL
    OR client_response_body IS NOT NULL
    OR request_body_compressed IS NOT NULL
    OR response_body_compressed IS NOT NULL
    OR provider_request_body_compressed IS NOT NULL
    OR client_response_body_compressed IS NOT NULL
  )
ORDER BY created_at ASC, id ASC
LIMIT $3
"#;
const CLEAR_USAGE_BODY_FIELDS_SQL: &str = r#"
UPDATE usage
SET request_body = NULL,
    response_body = NULL,
    provider_request_body = NULL,
    client_response_body = NULL,
    request_body_compressed = NULL,
    response_body_compressed = NULL,
    provider_request_body_compressed = NULL,
    client_response_body_compressed = NULL
WHERE id = ANY($1)
"#;
const SELECT_USAGE_BODY_COMPRESSION_BATCH_SQL: &str = r#"
SELECT
    id,
    request_body,
    response_body,
    provider_request_body,
    client_response_body
FROM usage
WHERE created_at < $1
  AND ($2::timestamptz IS NULL OR created_at >= $2)
  AND (
    request_body IS NOT NULL
    OR response_body IS NOT NULL
    OR provider_request_body IS NOT NULL
    OR client_response_body IS NOT NULL
  )
ORDER BY created_at ASC, id ASC
LIMIT $3
"#;
const UPDATE_USAGE_BODY_COMPRESSION_SQL: &str = r#"
UPDATE usage
SET request_body = NULL,
    response_body = NULL,
    provider_request_body = NULL,
    client_response_body = NULL,
    request_body_compressed = $2,
    response_body_compressed = $3,
    provider_request_body_compressed = $4,
    client_response_body_compressed = $5
WHERE id = $1
"#;
const SELECT_EXPIRED_ACTIVE_API_KEYS_SQL: &str = r#"
SELECT id, auto_delete_on_expiry
FROM api_keys
WHERE expires_at <= NOW()
  AND is_active IS TRUE
ORDER BY expires_at ASC NULLS FIRST, id ASC
"#;
const NULLIFY_USAGE_API_KEY_BATCH_SQL: &str = r#"
WITH doomed AS (
    SELECT id
    FROM usage
    WHERE api_key_id = $1
    ORDER BY created_at ASC, id ASC
    LIMIT $2
)
UPDATE usage AS usage_rows
SET api_key_id = NULL
FROM doomed
WHERE usage_rows.id = doomed.id
"#;
const NULLIFY_REQUEST_CANDIDATE_API_KEY_BATCH_SQL: &str = r#"
WITH doomed AS (
    SELECT id
    FROM request_candidates
    WHERE api_key_id = $1
    ORDER BY created_at ASC, id ASC
    LIMIT $2
)
UPDATE request_candidates AS candidate_rows
SET api_key_id = NULL
FROM doomed
WHERE candidate_rows.id = doomed.id
"#;
const DELETE_EXPIRED_API_KEY_SQL: &str = r#"
DELETE FROM api_keys
WHERE id = $1
"#;
const DISABLE_EXPIRED_API_KEY_SQL: &str = r#"
UPDATE api_keys
SET is_active = FALSE,
    updated_at = $2
WHERE id = $1
  AND is_active IS TRUE
"#;
const DELETE_AUDIT_LOGS_BEFORE_SQL: &str = r#"
WITH doomed AS (
    SELECT id
    FROM audit_logs
    WHERE created_at < $1
    ORDER BY created_at ASC, id ASC
    LIMIT $2
)
DELETE FROM audit_logs AS audit
USING doomed
WHERE audit.id = doomed.id
"#;
const SELECT_STATS_DAILY_AGGREGATE_SQL: &str = r#"
SELECT
    CAST(COUNT(id) AS BIGINT) AS total_requests,
    CAST(COALESCE(SUM(CASE WHEN status_code >= 400 OR error_message IS NOT NULL THEN 1 ELSE 0 END), 0) AS BIGINT) AS error_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
    CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
    CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
    CAST(COALESCE(SUM(input_cost_usd), 0) AS DOUBLE PRECISION) AS input_cost,
    CAST(COALESCE(SUM(output_cost_usd), 0) AS DOUBLE PRECISION) AS output_cost,
    CAST(COALESCE(SUM(cache_creation_cost_usd), 0) AS DOUBLE PRECISION) AS cache_creation_cost,
    CAST(COALESCE(SUM(cache_read_cost_usd), 0) AS DOUBLE PRECISION) AS cache_read_cost,
    CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms,
    CAST(COUNT(DISTINCT model) AS BIGINT) AS unique_models,
    CAST(COUNT(DISTINCT provider_name) AS BIGINT) AS unique_providers
FROM usage
WHERE created_at >= $1
  AND created_at < $2
"#;
const SELECT_STATS_DAILY_FALLBACK_COUNT_SQL: &str = r#"
SELECT CAST(COUNT(*) AS BIGINT) AS fallback_count
FROM (
    SELECT request_id
    FROM request_candidates
    WHERE created_at >= $1
      AND created_at < $2
      AND status = ANY($3)
    GROUP BY request_id
    HAVING COUNT(id) > 1
) AS fallback_requests
"#;
const SELECT_STATS_DAILY_RESPONSE_TIME_PERCENTILES_SQL: &str = r#"
SELECT
    CAST(COUNT(*) AS BIGINT) AS sample_count,
    CAST(percentile_cont(0.5) WITHIN GROUP (ORDER BY response_time_ms) AS DOUBLE PRECISION) AS p50,
    CAST(percentile_cont(0.9) WITHIN GROUP (ORDER BY response_time_ms) AS DOUBLE PRECISION) AS p90,
    CAST(percentile_cont(0.99) WITHIN GROUP (ORDER BY response_time_ms) AS DOUBLE PRECISION) AS p99
FROM usage
WHERE created_at >= $1
  AND created_at < $2
  AND status = 'completed'
  AND response_time_ms IS NOT NULL
"#;
const SELECT_STATS_DAILY_FIRST_BYTE_PERCENTILES_SQL: &str = r#"
SELECT
    CAST(COUNT(*) AS BIGINT) AS sample_count,
    CAST(percentile_cont(0.5) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p50,
    CAST(percentile_cont(0.9) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p90,
    CAST(percentile_cont(0.99) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p99
FROM usage
WHERE created_at >= $1
  AND created_at < $2
  AND status = 'completed'
  AND first_byte_time_ms IS NOT NULL
"#;
const UPSERT_STATS_DAILY_SQL: &str = r#"
INSERT INTO stats_daily (
    id,
    date,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    input_cost,
    output_cost,
    cache_creation_cost,
    cache_read_cost,
    avg_response_time_ms,
    p50_response_time_ms,
    p90_response_time_ms,
    p99_response_time_ms,
    p50_first_byte_time_ms,
    p90_first_byte_time_ms,
    p99_first_byte_time_ms,
    fallback_count,
    unique_models,
    unique_providers,
    is_complete,
    aggregated_at,
    created_at,
    updated_at
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8,
    $9, $10, $11, $12, $13, $14, $15, $16,
    $17, $18, $19, $20, $21, $22, $23, $24,
    $25, $26, $27, $28, $29
)
ON CONFLICT (date)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    input_cost = EXCLUDED.input_cost,
    output_cost = EXCLUDED.output_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    cache_read_cost = EXCLUDED.cache_read_cost,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    p50_response_time_ms = EXCLUDED.p50_response_time_ms,
    p90_response_time_ms = EXCLUDED.p90_response_time_ms,
    p99_response_time_ms = EXCLUDED.p99_response_time_ms,
    p50_first_byte_time_ms = EXCLUDED.p50_first_byte_time_ms,
    p90_first_byte_time_ms = EXCLUDED.p90_first_byte_time_ms,
    p99_first_byte_time_ms = EXCLUDED.p99_first_byte_time_ms,
    fallback_count = EXCLUDED.fallback_count,
    unique_models = EXCLUDED.unique_models,
    unique_providers = EXCLUDED.unique_providers,
    is_complete = EXCLUDED.is_complete,
    aggregated_at = EXCLUDED.aggregated_at,
    updated_at = EXCLUDED.updated_at
"#;
const SELECT_STATS_DAILY_MODEL_AGGREGATES_SQL: &str = r#"
SELECT
    model,
    CAST(COUNT(id) AS BIGINT) AS total_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
    CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
    CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms
FROM usage
WHERE created_at >= $1
  AND created_at < $2
  AND model IS NOT NULL
  AND model <> ''
GROUP BY model
"#;
const UPSERT_STATS_DAILY_MODEL_SQL: &str = r#"
INSERT INTO stats_daily_model (
    id,
    date,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    avg_response_time_ms,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
ON CONFLICT (date, model)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    updated_at = EXCLUDED.updated_at
"#;
const SELECT_STATS_DAILY_PROVIDER_AGGREGATES_SQL: &str = r#"
SELECT
    COALESCE(provider_name, 'Unknown') AS provider_name,
    CAST(COUNT(id) AS BIGINT) AS total_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
    CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
FROM usage
WHERE created_at >= $1
  AND created_at < $2
GROUP BY COALESCE(provider_name, 'Unknown')
"#;
const UPSERT_STATS_DAILY_PROVIDER_SQL: &str = r#"
INSERT INTO stats_daily_provider (
    id,
    date,
    provider_name,
    total_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
ON CONFLICT (date, provider_name)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = EXCLUDED.updated_at
"#;
const SELECT_STATS_DAILY_API_KEY_AGGREGATES_SQL: &str = r#"
SELECT
    api_key_id,
    MAX(api_key_name) AS api_key_name,
    CAST(COUNT(id) AS BIGINT) AS total_requests,
    CAST(COALESCE(SUM(CASE WHEN status_code >= 400 OR error_message IS NOT NULL THEN 1 ELSE 0 END), 0) AS BIGINT) AS error_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
    CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
FROM usage
WHERE created_at >= $1
  AND created_at < $2
  AND api_key_id IS NOT NULL
GROUP BY api_key_id
"#;
const UPSERT_STATS_DAILY_API_KEY_SQL: &str = r#"
INSERT INTO stats_daily_api_key (
    id,
    api_key_id,
    api_key_name,
    date,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
ON CONFLICT (api_key_id, date)
DO UPDATE SET
    api_key_name = COALESCE(EXCLUDED.api_key_name, stats_daily_api_key.api_key_name),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = EXCLUDED.updated_at
"#;
const DELETE_STATS_DAILY_ERRORS_FOR_DATE_SQL: &str = r#"
DELETE FROM stats_daily_error
WHERE date = $1
"#;
const SELECT_STATS_DAILY_ERROR_AGGREGATES_SQL: &str = r#"
SELECT
    error_category,
    provider_name,
    model,
    CAST(COUNT(id) AS BIGINT) AS total_count
FROM usage
WHERE created_at >= $1
  AND created_at < $2
  AND error_category IS NOT NULL
GROUP BY error_category, provider_name, model
"#;
const INSERT_STATS_DAILY_ERROR_SQL: &str = r#"
INSERT INTO stats_daily_error (
    id,
    date,
    error_category,
    provider_name,
    model,
    count,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
"#;
const SELECT_ACTIVE_USER_IDS_SQL: &str = r#"
SELECT id
FROM users
WHERE is_active IS TRUE
ORDER BY id ASC
"#;
const SELECT_STATS_USER_DAILY_AGGREGATES_SQL: &str = r#"
SELECT
    user_id,
    MAX(username) AS username,
    CAST(COUNT(id) AS BIGINT) AS total_requests,
    CAST(COALESCE(SUM(CASE WHEN status_code >= 400 OR error_message IS NOT NULL THEN 1 ELSE 0 END), 0) AS BIGINT) AS error_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
    CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
FROM usage
WHERE created_at >= $1
  AND created_at < $2
  AND user_id IS NOT NULL
GROUP BY user_id
"#;
const UPSERT_STATS_USER_DAILY_SQL: &str = r#"
INSERT INTO stats_user_daily (
    id,
    user_id,
    username,
    date,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
ON CONFLICT (user_id, date)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = EXCLUDED.updated_at
"#;
const SELECT_EXISTING_STATS_SUMMARY_ID_SQL: &str = r#"
SELECT id
FROM stats_summary
ORDER BY created_at ASC, id ASC
LIMIT 1
"#;
const SELECT_STATS_SUMMARY_TOTALS_SQL: &str = r#"
SELECT
    CAST(COALESCE(SUM(total_requests), 0) AS BIGINT) AS all_time_requests,
    CAST(COALESCE(SUM(success_requests), 0) AS BIGINT) AS all_time_success_requests,
    CAST(COALESCE(SUM(error_requests), 0) AS BIGINT) AS all_time_error_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS all_time_input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS all_time_output_tokens,
    CAST(COALESCE(SUM(cache_creation_tokens), 0) AS BIGINT) AS all_time_cache_creation_tokens,
    CAST(COALESCE(SUM(cache_read_tokens), 0) AS BIGINT) AS all_time_cache_read_tokens,
    CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS all_time_cost,
    CAST(COALESCE(SUM(actual_total_cost), 0) AS DOUBLE PRECISION) AS all_time_actual_cost
FROM stats_daily
WHERE date < $1
"#;
const SELECT_STATS_SUMMARY_ENTITY_COUNTS_SQL: &str = r#"
SELECT
    CAST((SELECT COUNT(id) FROM users) AS BIGINT) AS total_users,
    CAST((SELECT COUNT(id) FROM users WHERE is_active IS TRUE) AS BIGINT) AS active_users,
    CAST((SELECT COUNT(id) FROM api_keys) AS BIGINT) AS total_api_keys,
    CAST((SELECT COUNT(id) FROM api_keys WHERE is_active IS TRUE) AS BIGINT) AS active_api_keys
"#;
const INSERT_STATS_SUMMARY_SQL: &str = r#"
INSERT INTO stats_summary (
    id,
    cutoff_date,
    all_time_requests,
    all_time_success_requests,
    all_time_error_requests,
    all_time_input_tokens,
    all_time_output_tokens,
    all_time_cache_creation_tokens,
    all_time_cache_read_tokens,
    all_time_cost,
    all_time_actual_cost,
    total_users,
    active_users,
    total_api_keys,
    active_api_keys,
    created_at,
    updated_at
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8,
    $9, $10, $11, $12, $13, $14, $15, $16, $17
)
"#;
const UPDATE_STATS_SUMMARY_SQL: &str = r#"
UPDATE stats_summary
SET cutoff_date = $2,
    all_time_requests = $3,
    all_time_success_requests = $4,
    all_time_error_requests = $5,
    all_time_input_tokens = $6,
    all_time_output_tokens = $7,
    all_time_cache_creation_tokens = $8,
    all_time_cache_read_tokens = $9,
    all_time_cost = $10,
    all_time_actual_cost = $11,
    total_users = $12,
    active_users = $13,
    total_api_keys = $14,
    active_api_keys = $15,
    updated_at = $16
WHERE id = $1
"#;
const SELECT_STATS_HOURLY_AGGREGATE_SQL: &str = r#"
SELECT
    COUNT(id) AS total_requests,
    COALESCE(SUM(CASE WHEN status_code >= 400 OR error_message IS NOT NULL THEN 1 ELSE 0 END), 0) AS error_requests,
    COALESCE(SUM(input_tokens), 0) AS input_tokens,
    COALESCE(SUM(output_tokens), 0) AS output_tokens,
    COALESCE(SUM(cache_creation_input_tokens), 0) AS cache_creation_tokens,
    COALESCE(SUM(cache_read_input_tokens), 0) AS cache_read_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
    CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
    CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms
FROM usage
WHERE created_at >= $1
  AND created_at < $2
"#;
const UPSERT_STATS_HOURLY_SQL: &str = r#"
INSERT INTO stats_hourly (
    id,
    hour_utc,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    avg_response_time_ms,
    is_complete,
    aggregated_at,
    created_at,
    updated_at
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8,
    $9, $10, $11, $12, $13, $14, $15, $16
)
ON CONFLICT (hour_utc)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    is_complete = EXCLUDED.is_complete,
    aggregated_at = EXCLUDED.aggregated_at,
    updated_at = EXCLUDED.updated_at
"#;
const SELECT_STATS_HOURLY_USER_AGGREGATES_SQL: &str = r#"
SELECT
    user_id,
    COUNT(id) AS total_requests,
    COALESCE(SUM(CASE WHEN status_code >= 400 OR error_message IS NOT NULL THEN 1 ELSE 0 END), 0) AS error_requests,
    COALESCE(SUM(input_tokens), 0) AS input_tokens,
    COALESCE(SUM(output_tokens), 0) AS output_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
FROM usage
WHERE created_at >= $1
  AND created_at < $2
  AND user_id IS NOT NULL
GROUP BY user_id
"#;
const UPSERT_STATS_HOURLY_USER_SQL: &str = r#"
INSERT INTO stats_hourly_user (
    id,
    hour_utc,
    user_id,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    total_cost,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
ON CONFLICT (hour_utc, user_id)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = EXCLUDED.updated_at
"#;
const SELECT_STATS_HOURLY_MODEL_AGGREGATES_SQL: &str = r#"
SELECT
    model,
    COUNT(id) AS total_requests,
    COALESCE(SUM(input_tokens), 0) AS input_tokens,
    COALESCE(SUM(output_tokens), 0) AS output_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
    CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms
FROM usage
WHERE created_at >= $1
  AND created_at < $2
GROUP BY model
"#;
const UPSERT_STATS_HOURLY_MODEL_SQL: &str = r#"
INSERT INTO stats_hourly_model (
    id,
    hour_utc,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    total_cost,
    avg_response_time_ms,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
ON CONFLICT (hour_utc, model)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_cost = EXCLUDED.total_cost,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    updated_at = EXCLUDED.updated_at
"#;
const SELECT_STATS_HOURLY_PROVIDER_AGGREGATES_SQL: &str = r#"
SELECT
    provider_name,
    COUNT(id) AS total_requests,
    COALESCE(SUM(input_tokens), 0) AS input_tokens,
    COALESCE(SUM(output_tokens), 0) AS output_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
FROM usage
WHERE created_at >= $1
  AND created_at < $2
GROUP BY provider_name
"#;
const UPSERT_STATS_HOURLY_PROVIDER_SQL: &str = r#"
INSERT INTO stats_hourly_provider (
    id,
    hour_utc,
    provider_name,
    total_requests,
    input_tokens,
    output_tokens,
    total_cost,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
ON CONFLICT (hour_utc, provider_name)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_cost = EXCLUDED.total_cost,
    updated_at = EXCLUDED.updated_at
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatsAggregationSummary {
    day_start_utc: DateTime<Utc>,
    total_requests: i64,
    model_rows: usize,
    provider_rows: usize,
    api_key_rows: usize,
    error_rows: usize,
    user_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PercentileSummary {
    p50: Option<i64>,
    p90: Option<i64>,
    p99: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct UsageCleanupSummary {
    body_compressed: usize,
    body_cleaned: usize,
    header_cleaned: usize,
    keys_cleaned: usize,
    records_deleted: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsageCleanupSettings {
    detail_retention_days: u64,
    compressed_retention_days: u64,
    header_retention_days: u64,
    log_retention_days: u64,
    batch_size: usize,
    auto_delete_expired_keys: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UsageCleanupWindow {
    detail_cutoff: DateTime<Utc>,
    compressed_cutoff: DateTime<Utc>,
    header_cutoff: DateTime<Utc>,
    log_cutoff: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
struct UsageBodyCompressionRow {
    id: String,
    request_body: Option<Value>,
    response_body: Option<Value>,
    provider_request_body: Option<Value>,
    client_response_body: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PoolMonitorSummary {
    checked_out: usize,
    pool_size: usize,
    idle: usize,
    max_connections: u32,
    usage_rate: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExpiredApiKeyRow<'a> {
    id: &'a str,
    auto_delete_on_expiry: Option<bool>,
}

pub(crate) async fn cleanup_expired_gemini_file_mappings_once(
    data: &GatewayDataState,
) -> Result<usize, aether_data::DataLayerError> {
    data.delete_expired_gemini_file_mappings(now_unix_secs())
        .await
}

fn summarize_postgres_pool(data: &GatewayDataState) -> Option<PoolMonitorSummary> {
    let pool = data.postgres_pool()?;
    let max_connections = data.postgres_max_connections()?.max(1);
    let pool_size = usize::try_from(pool.size()).unwrap_or(usize::MAX);
    let idle = pool.num_idle();
    let checked_out = pool_size.saturating_sub(idle);
    let usage_rate = checked_out as f64 / f64::from(max_connections) * 100.0;

    Some(PoolMonitorSummary {
        checked_out,
        pool_size,
        idle,
        max_connections,
        usage_rate,
    })
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
