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

use crate::admin_api::admin_provider_ops_local_action_response;
use crate::data::GatewayDataState;
use crate::{AppState, GatewayError};

#[path = "runtime/audit_cleanup.rs"]
mod audit_cleanup;
#[path = "runtime/config.rs"]
mod config;
#[path = "runtime/db_maintenance.rs"]
mod db_maintenance;
#[path = "runtime/pending_cleanup.rs"]
mod pending_cleanup;
#[path = "runtime/pool_quota_probe.rs"]
mod pool_quota_probe;
#[path = "runtime/provider_checkin.rs"]
mod provider_checkin;
#[path = "runtime/proxy_node_staleness.rs"]
mod proxy_node_staleness;
#[path = "runtime/proxy_upgrade_rollout.rs"]
mod proxy_upgrade_rollout;
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
pub(crate) use pool_quota_probe::{
    perform_pool_quota_probe_once, perform_pool_quota_probe_once_with_config,
    select_pool_quota_probe_key_ids, spawn_pool_quota_probe_worker, PoolQuotaProbeRunSummary,
    PoolQuotaProbeWorkerConfig,
};
pub(crate) use provider_checkin::{perform_provider_checkin_once, ProviderCheckinRunSummary};
use proxy_node_staleness::*;
use proxy_upgrade_rollout::*;
pub(crate) use proxy_upgrade_rollout::{
    cancel_proxy_upgrade_rollout, clear_proxy_upgrade_rollout_conflicts,
    collect_proxy_upgrade_rollout_probes, inspect_proxy_upgrade_rollout,
    record_proxy_upgrade_traffic_success, restore_proxy_upgrade_rollout_skipped_nodes,
    retry_proxy_upgrade_rollout_node, skip_proxy_upgrade_rollout_node, start_proxy_upgrade_rollout,
    ProxyUpgradeRolloutCancelSummary, ProxyUpgradeRolloutConflictClearSummary,
    ProxyUpgradeRolloutNodeActionSummary, ProxyUpgradeRolloutPendingProbe,
    ProxyUpgradeRolloutProbeConfig, ProxyUpgradeRolloutSkippedRestoreSummary,
    ProxyUpgradeRolloutStatus, ProxyUpgradeRolloutSummary, ProxyUpgradeRolloutTrackedNodeState,
};
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
const PROXY_NODE_STALE_SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const PROXY_UPGRADE_ROLLOUT_INTERVAL: Duration = Duration::from_secs(15);
const PROXY_NODE_STALE_MIN_GRACE_SECS: u64 = 15;
const PROXY_NODE_STALE_MISSED_HEARTBEATS: u64 = 3;
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
const UPSERT_WALLET_DAILY_USAGE_LEDGER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage_settlement_snapshots.wallet_id,
        COUNT(usage.id) AS total_requests,
        CAST(COALESCE(SUM(usage.total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost_usd,
        COALESCE(SUM(usage.input_tokens), 0) AS input_tokens,
        COALESCE(SUM(usage.output_tokens), 0) AS output_tokens,
        COALESCE(SUM(usage.cache_creation_input_tokens), 0) AS cache_creation_tokens,
        COALESCE(SUM(usage.cache_read_input_tokens), 0) AS cache_read_tokens,
        MIN(COALESCE(usage_settlement_snapshots.finalized_at, usage.finalized_at)) AS first_finalized_at,
        MAX(COALESCE(usage_settlement_snapshots.finalized_at, usage.finalized_at)) AS last_finalized_at
    FROM usage_billing_facts AS usage
    JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage_settlement_snapshots.wallet_id IS NOT NULL
      AND COALESCE(usage_settlement_snapshots.billing_status, usage.billing_status) = 'settled'
      AND usage.total_cost_usd > 0
      AND COALESCE(usage_settlement_snapshots.finalized_at, usage.finalized_at) >= $1
      AND COALESCE(usage_settlement_snapshots.finalized_at, usage.finalized_at) < $2
    GROUP BY usage_settlement_snapshots.wallet_id
)
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
SELECT
    md5(CONCAT('wallet-daily-usage:', aggregated.wallet_id, ':', CAST($3 AS TEXT), ':', $4)),
    aggregated.wallet_id,
    $3,
    $4,
    aggregated.total_cost_usd,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_read_tokens,
    aggregated.first_finalized_at,
    aggregated.last_finalized_at,
    $5,
    $5,
    $5
FROM aggregated
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
      FROM usage_billing_facts AS usage
      JOIN usage_settlement_snapshots
        ON usage_settlement_snapshots.request_id = usage.request_id
      WHERE usage_settlement_snapshots.wallet_id = ledgers.wallet_id
        AND COALESCE(usage_settlement_snapshots.billing_status, usage.billing_status) = 'settled'
        AND usage.total_cost_usd > 0
        AND COALESCE(usage_settlement_snapshots.finalized_at, usage.finalized_at) >= $3
        AND COALESCE(usage_settlement_snapshots.finalized_at, usage.finalized_at) < $4
  )
"#;
const SELECT_STALE_PENDING_USAGE_BATCH_SQL: &str = r#"
SELECT
  usage.id,
  usage.request_id,
  usage.status,
  COALESCE(usage_settlement_snapshots.billing_status, usage.billing_status) AS billing_status
FROM usage
LEFT JOIN usage_settlement_snapshots
  ON usage_settlement_snapshots.request_id = usage.request_id
WHERE usage.status = ANY($1)
  AND usage.created_at < $2
ORDER BY usage.created_at ASC, usage.id ASC
LIMIT $3
FOR UPDATE OF usage SKIP LOCKED
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
WITH updated_usage AS (
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
    RETURNING request_id
)
INSERT INTO usage_settlement_snapshots (
    request_id,
    billing_status,
    finalized_at
)
SELECT request_id, 'void', $3
FROM updated_usage
ON CONFLICT (request_id)
DO UPDATE SET
    billing_status = EXCLUDED.billing_status,
    finalized_at = COALESCE(
        usage_settlement_snapshots.finalized_at,
        EXCLUDED.finalized_at
    ),
    updated_at = NOW()
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
SELECT id, request_id
FROM usage
WHERE created_at < $1
  AND ($2::timestamptz IS NULL OR created_at >= $2)
  AND (
    request_headers IS NOT NULL
    OR response_headers IS NOT NULL
    OR provider_request_headers IS NOT NULL
    OR client_response_headers IS NOT NULL
    OR EXISTS (
      SELECT 1
      FROM usage_http_audits
      WHERE usage_http_audits.request_id = usage.request_id
        AND (
          usage_http_audits.request_headers IS NOT NULL
          OR usage_http_audits.response_headers IS NOT NULL
          OR usage_http_audits.provider_request_headers IS NOT NULL
          OR usage_http_audits.client_response_headers IS NOT NULL
        )
    )
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
const CLEAR_USAGE_HTTP_AUDIT_HEADERS_SQL: &str = r#"
UPDATE usage_http_audits
SET request_headers = NULL,
    response_headers = NULL,
    provider_request_headers = NULL,
    client_response_headers = NULL,
    updated_at = NOW()
WHERE request_id = ANY($1)
"#;
const SELECT_USAGE_STALE_BODY_BATCH_SQL: &str = r#"
SELECT id, request_id
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
    OR EXISTS (
      SELECT 1
      FROM usage_body_blobs
      WHERE usage_body_blobs.request_id = usage.request_id
    )
    OR EXISTS (
      SELECT 1
      FROM usage_http_audits
      WHERE usage_http_audits.request_id = usage.request_id
        AND (
          usage_http_audits.request_body_ref IS NOT NULL
          OR usage_http_audits.provider_request_body_ref IS NOT NULL
          OR usage_http_audits.response_body_ref IS NOT NULL
          OR usage_http_audits.client_response_body_ref IS NOT NULL
        )
    )
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
const DELETE_USAGE_BODY_BLOBS_SQL: &str = r#"
DELETE FROM usage_body_blobs
WHERE request_id = ANY($1)
"#;
const CLEAR_USAGE_HTTP_AUDIT_BODY_REFS_SQL: &str = r#"
UPDATE usage_http_audits
SET request_body_ref = NULL,
    provider_request_body_ref = NULL,
    response_body_ref = NULL,
    client_response_body_ref = NULL,
    body_capture_mode = 'none',
    updated_at = NOW()
WHERE request_id = ANY($1)
"#;
const DELETE_EMPTY_USAGE_HTTP_AUDITS_SQL: &str = r#"
DELETE FROM usage_http_audits
WHERE request_id = ANY($1)
  AND request_headers IS NULL
  AND response_headers IS NULL
  AND provider_request_headers IS NULL
  AND client_response_headers IS NULL
  AND request_body_ref IS NULL
  AND provider_request_body_ref IS NULL
  AND response_body_ref IS NULL
  AND client_response_body_ref IS NULL
"#;
const SELECT_USAGE_BODY_COMPRESSION_BATCH_SQL: &str = r#"
SELECT
    id
FROM usage
WHERE created_at < $1
  AND ($2::timestamptz IS NULL OR created_at >= $2)
  AND (
    request_body IS NOT NULL
    OR request_body_compressed IS NOT NULL
    OR response_body IS NOT NULL
    OR response_body_compressed IS NOT NULL
    OR provider_request_body IS NOT NULL
    OR provider_request_body_compressed IS NOT NULL
    OR client_response_body IS NOT NULL
    OR client_response_body_compressed IS NOT NULL
  )
ORDER BY created_at ASC, id ASC
LIMIT $3
"#;
const SELECT_USAGE_LEGACY_BODY_REF_METADATA_BATCH_SQL: &str = r#"
SELECT
    id,
    request_id,
    request_metadata
FROM usage
WHERE created_at < $1
  AND ($2::timestamptz IS NULL OR created_at >= $2)
  AND request_metadata IS NOT NULL
  AND (
    request_metadata::jsonb ? 'request_body_ref'
    OR request_metadata::jsonb ? 'provider_request_body_ref'
    OR request_metadata::jsonb ? 'response_body_ref'
    OR request_metadata::jsonb ? 'client_response_body_ref'
  )
ORDER BY created_at ASC, id ASC
LIMIT $3
"#;
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
const UPDATE_USAGE_REQUEST_METADATA_SQL: &str = r#"
UPDATE usage
SET request_metadata = $2::json,
    updated_at = NOW()
WHERE id = $1
"#;
const UPSERT_USAGE_HTTP_AUDIT_BODY_REFS_SQL: &str = r#"
INSERT INTO usage_http_audits (
  request_id,
  request_body_ref,
  provider_request_body_ref,
  response_body_ref,
  client_response_body_ref,
  body_capture_mode
) VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6
)
ON CONFLICT (request_id)
DO UPDATE SET
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
  body_capture_mode = CASE
    WHEN EXCLUDED.request_body_ref IS NOT NULL
      OR EXCLUDED.provider_request_body_ref IS NOT NULL
      OR EXCLUDED.response_body_ref IS NOT NULL
      OR EXCLUDED.client_response_body_ref IS NOT NULL
    THEN EXCLUDED.body_capture_mode
    ELSE usage_http_audits.body_capture_mode
  END,
  updated_at = NOW()
"#;
const SELECT_USAGE_BODY_COMPRESSION_ROW_SQL: &str = r#"
SELECT
    id,
    request_id,
    request_body,
    request_body_compressed,
    response_body,
    response_body_compressed,
    provider_request_body,
    provider_request_body_compressed,
    client_response_body,
    client_response_body_compressed
FROM usage
WHERE id = $1
LIMIT 1
"#;
const UPDATE_USAGE_BODY_COMPRESSION_SQL: &str = r#"
UPDATE usage
SET request_body = NULL,
    response_body = NULL,
    provider_request_body = NULL,
    client_response_body = NULL,
    request_body_compressed = NULL,
    response_body_compressed = NULL,
    provider_request_body_compressed = NULL,
    client_response_body_compressed = NULL
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
const DISABLE_EXPIRED_API_KEY_WALLET_SQL: &str = r#"
UPDATE wallets
SET status = 'disabled',
    updated_at = NOW()
WHERE api_key_id = $1
  AND status <> 'disabled'
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
    (
        SELECT CAST(COUNT(cache_hit_usage.id) AS BIGINT)
        FROM usage_billing_facts AS cache_hit_usage
        WHERE cache_hit_usage.created_at >= $1
          AND cache_hit_usage.created_at < $2
    ) AS cache_hit_total_requests,
    (
        SELECT CAST(
            COUNT(cache_hit_usage.id) FILTER (
                WHERE GREATEST(COALESCE(cache_hit_usage.cache_read_input_tokens, 0), 0) > 0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS cache_hit_usage
        WHERE cache_hit_usage.created_at >= $1
          AND cache_hit_usage.created_at < $2
    ) AS cache_hit_requests,
    (
        SELECT CAST(COUNT(completed_usage.id) AS BIGINT)
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_total_requests,
    (
        SELECT CAST(
            COUNT(completed_usage.id) FILTER (
                WHERE GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0) > 0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_hit_requests,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_input_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(completed_usage.cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                           + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(completed_usage.cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_creation_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_read_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(
                            lower(
                                COALESCE(
                                    COALESCE(
                                        completed_usage.endpoint_api_format,
                                        completed_usage.api_format
                                    ),
                                    ''
                                )
                            ),
                            ':',
                            1
                        ) IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(completed_usage.cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                        + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                    + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(completed_usage.cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        WHEN split_part(
                            lower(
                                COALESCE(
                                    COALESCE(
                                        completed_usage.endpoint_api_format,
                                        completed_usage.api_format
                                    ),
                                    ''
                                )
                            ),
                            ':',
                            1
                        ) IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0) <= 0
                                THEN 0
                                WHEN GREATEST(
                                    COALESCE(completed_usage.cache_read_input_tokens, 0),
                                    0
                                ) <= 0
                                THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                        - GREATEST(
                                            COALESCE(completed_usage.cache_read_input_tokens, 0),
                                            0
                                        ),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(
                                        completed_usage.cache_creation_input_tokens,
                                        0
                                    ) = 0
                                         AND (
                                            COALESCE(
                                                completed_usage.cache_creation_input_tokens_5m,
                                                0
                                            )
                                            + COALESCE(
                                                completed_usage.cache_creation_input_tokens_1h,
                                                0
                                            )
                                         ) > 0
                                    THEN COALESCE(
                                        completed_usage.cache_creation_input_tokens_5m,
                                        0
                                    )
                                        + COALESCE(
                                            completed_usage.cache_creation_input_tokens_1h,
                                            0
                                        )
                                    ELSE COALESCE(
                                        completed_usage.cache_creation_input_tokens,
                                        0
                                    )
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(
                                            completed_usage.cache_creation_input_tokens,
                                            0
                                        ) = 0
                                             AND (
                                                COALESCE(
                                                    completed_usage.cache_creation_input_tokens_5m,
                                                    0
                                                )
                                                + COALESCE(
                                                    completed_usage.cache_creation_input_tokens_1h,
                                                    0
                                                )
                                             ) > 0
                                        THEN COALESCE(
                                            completed_usage.cache_creation_input_tokens_5m,
                                            0
                                        )
                                            + COALESCE(
                                                completed_usage.cache_creation_input_tokens_1h,
                                                0
                                            )
                                        ELSE COALESCE(
                                            completed_usage.cache_creation_input_tokens,
                                            0
                                        )
                                    END
                                )
                                + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_total_input_context,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(completed_usage.cache_creation_cost_usd AS DOUBLE PRECISION),
                        0
                    )
                ),
                0
            ) AS DOUBLE PRECISION
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_creation_cost,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    COALESCE(CAST(completed_usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)
                ),
                0
            ) AS DOUBLE PRECISION
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_read_cost,
    (
        SELECT CAST(
            COALESCE(SUM(COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0)), 0)
                AS DOUBLE PRECISION
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_total_cost,
    (
        SELECT CAST(COUNT(settled_usage.id) AS BIGINT)
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_total_requests,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(settled_usage.input_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_input_tokens,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(settled_usage.output_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_output_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(settled_usage.cache_creation_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_cache_creation_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(settled_usage.cache_read_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_cache_read_tokens,
    (
        SELECT MIN(CAST(EXTRACT(EPOCH FROM settled_usage.finalized_at) AS BIGINT))
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_first_finalized_at_unix_secs,
    (
        SELECT MAX(CAST(EXTRACT(EPOCH FROM settled_usage.finalized_at) AS BIGINT))
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_last_finalized_at_unix_secs,
    CAST(COUNT(id) AS BIGINT) AS total_requests,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN status_code >= 400
                         OR lower(COALESCE(status, '')) = 'failed'
                         OR error_message IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT
    ) AS error_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                    WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                    THEN GREATEST(COALESCE(input_tokens, 0), 0)
                    WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                         IN ('openai', 'gemini', 'google')
                    THEN GREATEST(
                        GREATEST(COALESCE(input_tokens, 0), 0)
                            - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                        0
                    )
                    ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                END
            ),
            0
        ) AS BIGINT
    ) AS effective_input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                         AND (
                            COALESCE(cache_creation_input_tokens_5m, 0)
                            + COALESCE(cache_creation_input_tokens_1h, 0)
                         ) > 0
                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                       + COALESCE(cache_creation_input_tokens_1h, 0)
                    ELSE COALESCE(cache_creation_input_tokens, 0)
                END
            ),
            0
        ) AS BIGINT
    ) AS cache_creation_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
        AS cache_creation_ephemeral_5m_tokens,
    CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
        AS cache_creation_ephemeral_1h_tokens,
    CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                         IN ('claude', 'anthropic')
                    THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        + CASE
                            WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                 AND (
                                    COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                 ) > 0
                            THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                            ELSE COALESCE(cache_creation_input_tokens, 0)
                          END
                        + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                    WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                         IN ('openai', 'gemini', 'google')
                    THEN (
                        CASE
                            WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                            WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            ELSE GREATEST(
                                GREATEST(COALESCE(input_tokens, 0), 0)
                                    - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                0
                            )
                        END
                    ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                    ELSE CASE
                        WHEN (
                            CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                            END
                        ) > 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                              )
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                    END
                END
            ),
            0
        ) AS BIGINT
    ) AS total_input_context,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
    CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
    CAST(COALESCE(SUM(input_cost_usd), 0) AS DOUBLE PRECISION) AS input_cost,
    CAST(COALESCE(SUM(output_cost_usd), 0) AS DOUBLE PRECISION) AS output_cost,
    CAST(COALESCE(SUM(cache_creation_cost_usd), 0) AS DOUBLE PRECISION) AS cache_creation_cost,
    CAST(COALESCE(SUM(cache_read_cost_usd), 0) AS DOUBLE PRECISION) AS cache_read_cost,
    COALESCE(
        SUM(
            CASE
                WHEN response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT
    ) AS response_time_samples,
    CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms,
    CAST(COUNT(DISTINCT model) AS BIGINT) AS unique_models,
    CAST(COUNT(DISTINCT provider_name) AS BIGINT) AS unique_providers
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
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
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status = 'completed'
  AND provider_name NOT IN ('unknown', 'pending')
  AND response_time_ms IS NOT NULL
"#;
const SELECT_STATS_DAILY_FIRST_BYTE_PERCENTILES_SQL: &str = r#"
SELECT
    CAST(COUNT(*) AS BIGINT) AS sample_count,
    CAST(percentile_cont(0.5) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p50,
    CAST(percentile_cont(0.9) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p90,
    CAST(percentile_cont(0.99) WITHIN GROUP (ORDER BY first_byte_time_ms) AS DOUBLE PRECISION) AS p99
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status = 'completed'
  AND provider_name NOT IN ('unknown', 'pending')
  AND first_byte_time_ms IS NOT NULL
"#;
const UPSERT_STATS_DAILY_SQL: &str = r#"
INSERT INTO stats_daily (
    id,
    date,
    total_requests,
    cache_hit_total_requests,
    cache_hit_requests,
    completed_total_requests,
    completed_cache_hit_requests,
    completed_input_tokens,
    completed_cache_creation_tokens,
    completed_cache_read_tokens,
    completed_total_input_context,
    completed_cache_creation_cost,
    completed_cache_read_cost,
    settled_total_cost,
    settled_total_requests,
    settled_input_tokens,
    settled_output_tokens,
    settled_cache_creation_tokens,
    settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs,
    success_requests,
    error_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_input_context,
    total_cost,
    actual_total_cost,
    input_cost,
    output_cost,
    cache_creation_cost,
    cache_read_cost,
    response_time_sum_ms,
    response_time_samples,
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
    $25, $26, $27, $28, $29, $30, $31, $32,
    $33, $34, $35, $36, $37, $38, $39, $40,
    $41, $42, $43, $44, $45, $46, $47, $48,
    $49, $50, $51, $52, $53
)
ON CONFLICT (date)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    cache_hit_total_requests = EXCLUDED.cache_hit_total_requests,
    cache_hit_requests = EXCLUDED.cache_hit_requests,
    completed_total_requests = EXCLUDED.completed_total_requests,
    completed_cache_hit_requests = EXCLUDED.completed_cache_hit_requests,
    completed_input_tokens = EXCLUDED.completed_input_tokens,
    completed_cache_creation_tokens = EXCLUDED.completed_cache_creation_tokens,
    completed_cache_read_tokens = EXCLUDED.completed_cache_read_tokens,
    completed_total_input_context = EXCLUDED.completed_total_input_context,
    completed_cache_creation_cost = EXCLUDED.completed_cache_creation_cost,
    completed_cache_read_cost = EXCLUDED.completed_cache_read_cost,
    settled_total_cost = EXCLUDED.settled_total_cost,
    settled_total_requests = EXCLUDED.settled_total_requests,
    settled_input_tokens = EXCLUDED.settled_input_tokens,
    settled_output_tokens = EXCLUDED.settled_output_tokens,
    settled_cache_creation_tokens = EXCLUDED.settled_cache_creation_tokens,
    settled_cache_read_tokens = EXCLUDED.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = EXCLUDED.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = EXCLUDED.settled_last_finalized_at_unix_secs,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_input_context = EXCLUDED.total_input_context,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    input_cost = EXCLUDED.input_cost,
    output_cost = EXCLUDED.output_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    cache_read_cost = EXCLUDED.cache_read_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
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
const UPSERT_STATS_DAILY_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        model,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples,
        CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND model IS NOT NULL
      AND model <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY model
)
INSERT INTO stats_daily_model (
    id,
    date,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    avg_response_time_ms,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-daily-model:', aggregated.model, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.model,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.avg_response_time_ms,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, model)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_DAILY_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        COALESCE(provider_name, 'Unknown') AS provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY COALESCE(provider_name, 'Unknown')
)
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
SELECT
    md5(CONCAT('stats-daily-provider:', aggregated.provider_name, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    $3,
    $3
FROM aggregated
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
const UPSERT_STATS_DAILY_MODEL_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        model,
        provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND model IS NOT NULL
      AND model <> ''
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY model, provider_name
)
INSERT INTO stats_daily_model_provider (
    id,
    date,
    model,
    provider_name,
    total_requests,
    total_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-model-provider:',
            CAST($1 AS TEXT),
            ':',
            aggregated.model,
            ':',
            aggregated.provider_name
        )
    ),
    $1,
    aggregated.model,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.total_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, model, provider_name)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    total_tokens = EXCLUDED.total_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_DAILY_COST_SAVINGS_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
)
INSERT INTO stats_daily_cost_savings (
    id,
    date,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-daily-cost-savings:', CAST($1 AS TEXT))),
    $1,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date)
DO UPDATE SET
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_DAILY_COST_SAVINGS_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        COALESCE(usage.provider_name, '') AS provider_name,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
    GROUP BY COALESCE(usage.provider_name, '')
)
INSERT INTO stats_daily_cost_savings_provider (
    id,
    date,
    provider_name,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-cost-savings-provider:',
            CAST($1 AS TEXT),
            ':',
            aggregated.provider_name
        )
    ),
    $1,
    aggregated.provider_name,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, provider_name)
DO UPDATE SET
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_DAILY_COST_SAVINGS_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        COALESCE(usage.model, '') AS model,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
    GROUP BY COALESCE(usage.model, '')
)
INSERT INTO stats_daily_cost_savings_model (
    id,
    date,
    model,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-cost-savings-model:',
            CAST($1 AS TEXT),
            ':',
            aggregated.model
        )
    ),
    $1,
    aggregated.model,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, model)
DO UPDATE SET
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_DAILY_COST_SAVINGS_MODEL_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        COALESCE(usage.model, '') AS model,
        COALESCE(usage.provider_name, '') AS provider_name,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
    GROUP BY COALESCE(usage.model, ''), COALESCE(usage.provider_name, '')
)
INSERT INTO stats_daily_cost_savings_model_provider (
    id,
    date,
    model,
    provider_name,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-daily-cost-savings-model-provider:',
            CAST($1 AS TEXT),
            ':',
            aggregated.model,
            ':',
            aggregated.provider_name
        )
    ),
    $1,
    aggregated.model,
    aggregated.provider_name,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (date, model, provider_name)
DO UPDATE SET
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_DAILY_API_KEY_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        api_key_id,
        MAX(api_key_name) AS api_key_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status_code >= 400 OR error_message IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS error_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens), 0) AS BIGINT) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND api_key_id IS NOT NULL
    GROUP BY api_key_id
)
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
SELECT
    md5(CONCAT('stats-daily-api-key:', aggregated.api_key_id, ':', CAST($1 AS TEXT))),
    aggregated.api_key_id,
    aggregated.api_key_name,
    $1,
    aggregated.total_requests,
    GREATEST(aggregated.total_requests - aggregated.error_requests, 0),
    aggregated.error_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    $3,
    $3
FROM aggregated
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
const INSERT_STATS_DAILY_ERROR_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        error_category,
        provider_name,
        model,
        CAST(COUNT(id) AS BIGINT) AS total_count
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND error_category IS NOT NULL
    GROUP BY error_category, provider_name, model
)
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
SELECT
    md5(
        CONCAT(
            'stats-daily-error:',
            CAST($1 AS TEXT),
            ':',
            aggregated.error_category,
            ':',
            COALESCE(aggregated.provider_name, ''),
            ':',
            COALESCE(aggregated.model, '')
        )
    ),
    $1,
    aggregated.error_category,
    aggregated.provider_name,
    aggregated.model,
    aggregated.total_count,
    $3,
    $3
FROM aggregated
"#;
const UPSERT_STATS_USER_DAILY_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status_code >= 400
                             OR lower(COALESCE(status, '')) = 'failed'
                             OR error_message IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS error_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                                WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                                THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(input_tokens, 0), 0)
                                        - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                             AND (
                                                COALESCE(cache_creation_input_tokens_5m, 0)
                                                + COALESCE(cache_creation_input_tokens_1h, 0)
                                             ) > 0
                                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                        ELSE COALESCE(cache_creation_input_tokens, 0)
                                    END
                                  )
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        ) AS total_input_context,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(cache_creation_cost_usd), 0) AS DOUBLE PRECISION) AS cache_creation_cost,
        CAST(COALESCE(SUM(cache_read_cost_usd), 0) AS DOUBLE PRECISION) AS cache_read_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS settled_total_cost,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN GREATEST(COALESCE(output_tokens, 0), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_output_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN GREATEST(COALESCE(cache_creation_input_tokens, 0), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_cache_creation_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS settled_cache_read_tokens,
        MIN(
            CASE
                WHEN billing_status = 'settled'
                     AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                     AND finalized_at IS NOT NULL
                THEN CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT)
                ELSE NULL
            END
        ) AS settled_first_finalized_at_unix_secs,
        MAX(
            CASE
                WHEN billing_status = 'settled'
                     AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                     AND finalized_at IS NOT NULL
                THEN CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT)
                ELSE NULL
            END
        ) AS settled_last_finalized_at_unix_secs,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id
)
INSERT INTO stats_user_daily (
    id,
    user_id,
    username,
    date,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_input_context,
    total_cost,
    cache_creation_cost,
    cache_read_cost,
    actual_total_cost,
    settled_total_cost,
    settled_total_requests,
    settled_input_tokens,
    settled_output_tokens,
    settled_cache_creation_tokens,
    settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-daily:', aggregated.user_id, ':', CAST($1 AS TEXT))),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.total_requests,
    GREATEST(aggregated.total_requests - aggregated.error_requests, 0),
    aggregated.error_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_input_context,
    aggregated.total_cost,
    aggregated.cache_creation_cost,
    aggregated.cache_read_cost,
    aggregated.actual_total_cost,
    aggregated.settled_total_cost,
    aggregated.settled_total_requests,
    aggregated.settled_input_tokens,
    aggregated.settled_output_tokens,
    aggregated.settled_cache_creation_tokens,
    aggregated.settled_cache_read_tokens,
    aggregated.settled_first_finalized_at_unix_secs,
    aggregated.settled_last_finalized_at_unix_secs,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_input_context = EXCLUDED.total_input_context,
    total_cost = EXCLUDED.total_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    cache_read_cost = EXCLUDED.cache_read_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    settled_total_cost = EXCLUDED.settled_total_cost,
    settled_total_requests = EXCLUDED.settled_total_requests,
    settled_input_tokens = EXCLUDED.settled_input_tokens,
    settled_output_tokens = EXCLUDED.settled_output_tokens,
    settled_cache_creation_tokens = EXCLUDED.settled_cache_creation_tokens,
    settled_cache_read_tokens = EXCLUDED.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = EXCLUDED.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = EXCLUDED.settled_last_finalized_at_unix_secs,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_USER_DAILY_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        model,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS success_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                                WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                                THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(input_tokens, 0), 0)
                                        - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                             AND (
                                                COALESCE(cache_creation_input_tokens_5m, 0)
                                                + COALESCE(cache_creation_input_tokens_1h, 0)
                                             ) > 0
                                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                        ELSE COALESCE(cache_creation_input_tokens, 0)
                                    END
                                  )
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        ) AS total_input_context,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples
        ,
        COALESCE(
            SUM(
                CASE
                    WHEN status <> 'failed'
                         AND (status_code IS NULL OR status_code < 400)
                         AND error_message IS NULL
                         AND response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS successful_response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                             AND response_time_ms IS NOT NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS successful_response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND model IS NOT NULL
      AND model <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, model
)
INSERT INTO stats_user_daily_model (
    id,
    user_id,
    username,
    date,
    model,
    total_requests,
    success_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    total_tokens,
    total_input_context,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    successful_response_time_sum_ms,
    successful_response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-daily-model:', aggregated.user_id, ':', CAST($1 AS TEXT), ':', aggregated.model)),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.model,
    aggregated.total_requests,
    aggregated.success_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.total_tokens,
    aggregated.total_input_context,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.successful_response_time_sum_ms,
    aggregated.successful_response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, model)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_model.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_tokens = EXCLUDED.total_tokens,
    total_input_context = EXCLUDED.total_input_context,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    successful_response_time_sum_ms = EXCLUDED.successful_response_time_sum_ms,
    successful_response_time_samples = EXCLUDED.successful_response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_USER_DAILY_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS success_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                                WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                                THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(input_tokens, 0), 0)
                                        - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                             AND (
                                                COALESCE(cache_creation_input_tokens_5m, 0)
                                                + COALESCE(cache_creation_input_tokens_1h, 0)
                                             ) > 0
                                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                        ELSE COALESCE(cache_creation_input_tokens, 0)
                                    END
                                  )
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        ) AS total_input_context,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples,
        COALESCE(
            SUM(
                CASE
                    WHEN status <> 'failed'
                         AND (status_code IS NULL OR status_code < 400)
                         AND error_message IS NULL
                         AND response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS successful_response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                             AND response_time_ms IS NOT NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS successful_response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, provider_name
)
INSERT INTO stats_user_daily_provider (
    id,
    user_id,
    username,
    date,
    provider_name,
    total_requests,
    success_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    total_tokens,
    total_input_context,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    successful_response_time_sum_ms,
    successful_response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-daily-provider:', aggregated.user_id, ':', CAST($1 AS TEXT), ':', aggregated.provider_name)),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.success_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.total_tokens,
    aggregated.total_input_context,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.successful_response_time_sum_ms,
    aggregated.successful_response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, provider_name)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_provider.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_tokens = EXCLUDED.total_tokens,
    total_input_context = EXCLUDED.total_input_context,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    successful_response_time_sum_ms = EXCLUDED.successful_response_time_sum_ms,
    successful_response_time_samples = EXCLUDED.successful_response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_USER_DAILY_API_FORMAT_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        api_format,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS success_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                        WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN GREATEST(
                            GREATEST(COALESCE(input_tokens, 0), 0)
                                - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                            0
                        )
                        ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS effective_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(cache_creation_input_tokens_5m, 0)
                                + COALESCE(cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                           + COALESCE(cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        ) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_5m), 0) AS BIGINT)
            AS cache_creation_ephemeral_5m_tokens,
        CAST(COALESCE(SUM(cache_creation_input_tokens_1h), 0) AS BIGINT)
            AS cache_creation_ephemeral_1h_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                    + COALESCE(cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        WHEN split_part(lower(COALESCE(COALESCE(endpoint_api_format, api_format), '')), ':', 1)
                             IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(input_tokens, 0), 0) <= 0 THEN 0
                                WHEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0) <= 0
                                THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(input_tokens, 0), 0)
                                        - GREATEST(COALESCE(cache_read_input_tokens, 0), 0),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                         AND (
                                            COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                         ) > 0
                                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                        + COALESCE(cache_creation_input_tokens_1h, 0)
                                    ELSE COALESCE(cache_creation_input_tokens, 0)
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                                             AND (
                                                COALESCE(cache_creation_input_tokens_5m, 0)
                                                + COALESCE(cache_creation_input_tokens_1h, 0)
                                             ) > 0
                                        THEN COALESCE(cache_creation_input_tokens_5m, 0)
                                            + COALESCE(cache_creation_input_tokens_1h, 0)
                                        ELSE COALESCE(cache_creation_input_tokens, 0)
                                    END
                                  )
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(input_tokens, 0), 0)
                                + GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        ) AS total_input_context,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples,
        COALESCE(
            SUM(
                CASE
                    WHEN status <> 'failed'
                         AND (status_code IS NULL OR status_code < 400)
                         AND error_message IS NULL
                         AND response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS successful_response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN status <> 'failed'
                             AND (status_code IS NULL OR status_code < 400)
                             AND error_message IS NULL
                             AND response_time_ms IS NOT NULL
                        THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS successful_response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND api_format IS NOT NULL
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, api_format
)
INSERT INTO stats_user_daily_api_format (
    id,
    user_id,
    username,
    date,
    api_format,
    total_requests,
    success_requests,
    input_tokens,
    effective_input_tokens,
    output_tokens,
    total_tokens,
    total_input_context,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    successful_response_time_sum_ms,
    successful_response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-daily-api-format:', aggregated.user_id, ':', CAST($1 AS TEXT), ':', aggregated.api_format)),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.api_format,
    aggregated.total_requests,
    aggregated.success_requests,
    aggregated.input_tokens,
    aggregated.effective_input_tokens,
    aggregated.output_tokens,
    aggregated.total_tokens,
    aggregated.total_input_context,
    aggregated.cache_creation_tokens,
    aggregated.cache_creation_ephemeral_5m_tokens,
    aggregated.cache_creation_ephemeral_1h_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.successful_response_time_sum_ms,
    aggregated.successful_response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, api_format)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_api_format.username),
    total_requests = EXCLUDED.total_requests,
    success_requests = EXCLUDED.success_requests,
    input_tokens = EXCLUDED.input_tokens,
    effective_input_tokens = EXCLUDED.effective_input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_tokens = EXCLUDED.total_tokens,
    total_input_context = EXCLUDED.total_input_context,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens = EXCLUDED.cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens = EXCLUDED.cache_creation_ephemeral_1h_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    successful_response_time_sum_ms = EXCLUDED.successful_response_time_sum_ms,
    successful_response_time_samples = EXCLUDED.successful_response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_USER_DAILY_MODEL_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        model,
        provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(total_tokens), 0) AS BIGINT) AS total_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN response_time_ms IS NOT NULL THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND model IS NOT NULL
      AND model <> ''
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, model, provider_name
)
INSERT INTO stats_user_daily_model_provider (
    id,
    user_id,
    username,
    date,
    model,
    provider_name,
    total_requests,
    total_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-model-provider:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT),
            ':',
            aggregated.model,
            ':',
            aggregated.provider_name
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.model,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.total_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, model, provider_name)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_model_provider.username),
    total_requests = EXCLUDED.total_requests,
    total_tokens = EXCLUDED.total_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_USER_DAILY_COST_SAVINGS_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage.user_id,
        MAX(usage.username) AS username,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
      AND usage.user_id IS NOT NULL
    GROUP BY usage.user_id
)
INSERT INTO stats_user_daily_cost_savings (
    id,
    user_id,
    username,
    date,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-cost-savings:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT)
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_cost_savings.username),
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_USER_DAILY_COST_SAVINGS_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage.user_id,
        MAX(usage.username) AS username,
        COALESCE(usage.provider_name, '') AS provider_name,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
      AND usage.user_id IS NOT NULL
    GROUP BY usage.user_id, COALESCE(usage.provider_name, '')
)
INSERT INTO stats_user_daily_cost_savings_provider (
    id,
    user_id,
    username,
    date,
    provider_name,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-cost-savings-provider:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT),
            ':',
            aggregated.provider_name
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.provider_name,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, provider_name)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_cost_savings_provider.username),
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_USER_DAILY_COST_SAVINGS_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage.user_id,
        MAX(usage.username) AS username,
        COALESCE(usage.model, '') AS model,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
      AND usage.user_id IS NOT NULL
    GROUP BY usage.user_id, COALESCE(usage.model, '')
)
INSERT INTO stats_user_daily_cost_savings_model (
    id,
    user_id,
    username,
    date,
    model,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-cost-savings-model:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT),
            ':',
            aggregated.model
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.model,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, model)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_daily_cost_savings_model.username),
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_USER_DAILY_COST_SAVINGS_MODEL_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        usage.user_id,
        MAX(usage.username) AS username,
        COALESCE(usage.model, '') AS model,
        COALESCE(usage.provider_name, '') AS provider_name,
        CAST(
            COALESCE(SUM(GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)), 0) AS BIGINT
        ) AS cache_read_tokens,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_read_cost,
        CAST(
            COALESCE(
                SUM(COALESCE(CAST(usage.cache_creation_cost_usd AS DOUBLE PRECISION), 0)),
                0
            ) AS DOUBLE PRECISION
        ) AS cache_creation_cost,
        CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(usage_settlement_snapshots.output_price_per_1m AS DOUBLE PRECISION),
                        CAST(usage.output_price_per_1m AS DOUBLE PRECISION),
                        0
                    ) * GREATEST(COALESCE(usage.cache_read_input_tokens, 0), 0)::DOUBLE PRECISION
                        / 1000000.0
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS estimated_full_cost
    FROM usage_billing_facts AS usage
    LEFT JOIN usage_settlement_snapshots
      ON usage_settlement_snapshots.request_id = usage.request_id
    WHERE usage.created_at >= $1
      AND usage.created_at < $2
      AND usage.user_id IS NOT NULL
    GROUP BY usage.user_id, COALESCE(usage.model, ''), COALESCE(usage.provider_name, '')
)
INSERT INTO stats_user_daily_cost_savings_model_provider (
    id,
    user_id,
    username,
    date,
    model,
    provider_name,
    cache_read_tokens,
    cache_read_cost,
    cache_creation_cost,
    estimated_full_cost,
    created_at,
    updated_at
)
SELECT
    md5(
        CONCAT(
            'stats-user-daily-cost-savings-model-provider:',
            aggregated.user_id,
            ':',
            CAST($1 AS TEXT),
            ':',
            aggregated.model,
            ':',
            aggregated.provider_name
        )
    ),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.model,
    aggregated.provider_name,
    aggregated.cache_read_tokens,
    aggregated.cache_read_cost,
    aggregated.cache_creation_cost,
    aggregated.estimated_full_cost,
    $3,
    $3
FROM aggregated
ON CONFLICT (user_id, date, model, provider_name)
DO UPDATE SET
    username = COALESCE(
        EXCLUDED.username,
        stats_user_daily_cost_savings_model_provider.username
    ),
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    cache_read_cost = EXCLUDED.cache_read_cost,
    cache_creation_cost = EXCLUDED.cache_creation_cost,
    estimated_full_cost = EXCLUDED.estimated_full_cost,
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
const UPSERT_STATS_USER_SUMMARY_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        MAX(username) AS username,
        CAST(COALESCE(SUM(total_requests), 0) AS BIGINT) AS all_time_requests,
        CAST(COALESCE(SUM(success_requests), 0) AS BIGINT) AS all_time_success_requests,
        CAST(COALESCE(SUM(error_requests), 0) AS BIGINT) AS all_time_error_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS all_time_input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS all_time_output_tokens,
        CAST(COALESCE(SUM(cache_creation_tokens), 0) AS BIGINT) AS all_time_cache_creation_tokens,
        CAST(COALESCE(SUM(cache_read_tokens), 0) AS BIGINT) AS all_time_cache_read_tokens,
        CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS all_time_cost,
        CAST(COALESCE(SUM(actual_total_cost), 0) AS DOUBLE PRECISION) AS all_time_actual_cost,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN total_requests > 0 THEN 1
                        ELSE 0
                    END
                ),
                0
            ) AS BIGINT
        ) AS active_days,
        MIN(CASE WHEN total_requests > 0 THEN date ELSE NULL END) AS first_active_date,
        MAX(CASE WHEN total_requests > 0 THEN date ELSE NULL END) AS last_active_date
    FROM stats_user_daily
    WHERE user_id IS NOT NULL
      AND date < $1
    GROUP BY user_id
)
INSERT INTO stats_user_summary (
    id,
    user_id,
    username,
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
    active_days,
    first_active_date,
    last_active_date,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-user-summary:', aggregated.user_id)),
    aggregated.user_id,
    aggregated.username,
    $1,
    aggregated.all_time_requests,
    aggregated.all_time_success_requests,
    aggregated.all_time_error_requests,
    aggregated.all_time_input_tokens,
    aggregated.all_time_output_tokens,
    aggregated.all_time_cache_creation_tokens,
    aggregated.all_time_cache_read_tokens,
    aggregated.all_time_cost,
    aggregated.all_time_actual_cost,
    aggregated.active_days,
    aggregated.first_active_date,
    aggregated.last_active_date,
    $2,
    $2
FROM aggregated
ON CONFLICT (user_id)
DO UPDATE SET
    username = COALESCE(EXCLUDED.username, stats_user_summary.username),
    cutoff_date = EXCLUDED.cutoff_date,
    all_time_requests = EXCLUDED.all_time_requests,
    all_time_success_requests = EXCLUDED.all_time_success_requests,
    all_time_error_requests = EXCLUDED.all_time_error_requests,
    all_time_input_tokens = EXCLUDED.all_time_input_tokens,
    all_time_output_tokens = EXCLUDED.all_time_output_tokens,
    all_time_cache_creation_tokens = EXCLUDED.all_time_cache_creation_tokens,
    all_time_cache_read_tokens = EXCLUDED.all_time_cache_read_tokens,
    all_time_cost = EXCLUDED.all_time_cost,
    all_time_actual_cost = EXCLUDED.all_time_actual_cost,
    active_days = EXCLUDED.active_days,
    first_active_date = EXCLUDED.first_active_date,
    last_active_date = EXCLUDED.last_active_date,
    updated_at = EXCLUDED.updated_at
"#;
const SELECT_LATEST_STATS_DAILY_DATE_SQL: &str = r#"
SELECT MAX(date) AS latest_date
FROM stats_daily
WHERE is_complete IS TRUE
"#;
const SELECT_NEXT_STATS_DAILY_BUCKET_SQL: &str = r#"
SELECT date_trunc('day', MIN(created_at)) AS next_bucket
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#;
const SELECT_LATEST_STATS_HOURLY_HOUR_SQL: &str = r#"
SELECT MAX(hour_utc) AS latest_hour
FROM stats_hourly
WHERE is_complete IS TRUE
"#;
const SELECT_NEXT_STATS_HOURLY_BUCKET_SQL: &str = r#"
SELECT date_trunc('hour', MIN(created_at)) AS next_bucket
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#;
const SELECT_STATS_HOURLY_AGGREGATE_SQL: &str = r#"
SELECT
    (
        SELECT CAST(COUNT(cache_hit_usage.id) AS BIGINT)
        FROM usage_billing_facts AS cache_hit_usage
        WHERE cache_hit_usage.created_at >= $1
          AND cache_hit_usage.created_at < $2
    ) AS cache_hit_total_requests,
    (
        SELECT CAST(
            COUNT(cache_hit_usage.id) FILTER (
                WHERE GREATEST(COALESCE(cache_hit_usage.cache_read_input_tokens, 0), 0) > 0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS cache_hit_usage
        WHERE cache_hit_usage.created_at >= $1
          AND cache_hit_usage.created_at < $2
    ) AS cache_hit_requests,
    (
        SELECT CAST(COUNT(completed_usage.id) AS BIGINT)
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_total_requests,
    (
        SELECT CAST(
            COUNT(completed_usage.id) FILTER (
                WHERE GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0) > 0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_hit_requests,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_input_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN COALESCE(completed_usage.cache_creation_input_tokens, 0) = 0
                             AND (
                                COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                             ) > 0
                        THEN COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                           + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                        ELSE COALESCE(completed_usage.cache_creation_input_tokens, 0)
                    END
                ),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_creation_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_read_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN split_part(
                            lower(
                                COALESCE(
                                    COALESCE(
                                        completed_usage.endpoint_api_format,
                                        completed_usage.api_format
                                    ),
                                    ''
                                )
                            ),
                            ':',
                            1
                        ) IN ('claude', 'anthropic')
                        THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                            + CASE
                                WHEN COALESCE(completed_usage.cache_creation_input_tokens, 0) = 0
                                     AND (
                                        COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                        + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                                     ) > 0
                                THEN COALESCE(completed_usage.cache_creation_input_tokens_5m, 0)
                                    + COALESCE(completed_usage.cache_creation_input_tokens_1h, 0)
                                ELSE COALESCE(completed_usage.cache_creation_input_tokens, 0)
                              END
                            + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        WHEN split_part(
                            lower(
                                COALESCE(
                                    COALESCE(
                                        completed_usage.endpoint_api_format,
                                        completed_usage.api_format
                                    ),
                                    ''
                                )
                            ),
                            ':',
                            1
                        ) IN ('openai', 'gemini', 'google')
                        THEN (
                            CASE
                                WHEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0) <= 0
                                THEN 0
                                WHEN GREATEST(
                                    COALESCE(completed_usage.cache_read_input_tokens, 0),
                                    0
                                ) <= 0
                                THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                ELSE GREATEST(
                                    GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                        - GREATEST(
                                            COALESCE(completed_usage.cache_read_input_tokens, 0),
                                            0
                                        ),
                                    0
                                )
                            END
                        ) + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        ELSE CASE
                            WHEN (
                                CASE
                                    WHEN COALESCE(
                                        completed_usage.cache_creation_input_tokens,
                                        0
                                    ) = 0
                                         AND (
                                            COALESCE(
                                                completed_usage.cache_creation_input_tokens_5m,
                                                0
                                            )
                                            + COALESCE(
                                                completed_usage.cache_creation_input_tokens_1h,
                                                0
                                            )
                                         ) > 0
                                    THEN COALESCE(
                                        completed_usage.cache_creation_input_tokens_5m,
                                        0
                                    )
                                        + COALESCE(
                                            completed_usage.cache_creation_input_tokens_1h,
                                            0
                                        )
                                    ELSE COALESCE(
                                        completed_usage.cache_creation_input_tokens,
                                        0
                                    )
                                END
                            ) > 0
                            THEN GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                + (
                                    CASE
                                        WHEN COALESCE(
                                            completed_usage.cache_creation_input_tokens,
                                            0
                                        ) = 0
                                             AND (
                                                COALESCE(
                                                    completed_usage.cache_creation_input_tokens_5m,
                                                    0
                                                )
                                                + COALESCE(
                                                    completed_usage.cache_creation_input_tokens_1h,
                                                    0
                                                )
                                             ) > 0
                                        THEN COALESCE(
                                            completed_usage.cache_creation_input_tokens_5m,
                                            0
                                        )
                                            + COALESCE(
                                                completed_usage.cache_creation_input_tokens_1h,
                                                0
                                            )
                                        ELSE COALESCE(
                                            completed_usage.cache_creation_input_tokens,
                                            0
                                        )
                                    END
                                )
                                + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                            ELSE GREATEST(COALESCE(completed_usage.input_tokens, 0), 0)
                                + GREATEST(COALESCE(completed_usage.cache_read_input_tokens, 0), 0)
                        END
                    END
                ),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_total_input_context,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    COALESCE(
                        CAST(completed_usage.cache_creation_cost_usd AS DOUBLE PRECISION),
                        0
                    )
                ),
                0
            ) AS DOUBLE PRECISION
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_creation_cost,
    (
        SELECT CAST(
            COALESCE(
                SUM(
                    COALESCE(CAST(completed_usage.cache_read_cost_usd AS DOUBLE PRECISION), 0)
                ),
                0
            ) AS DOUBLE PRECISION
        )
        FROM usage_billing_facts AS completed_usage
        WHERE completed_usage.created_at >= $1
          AND completed_usage.created_at < $2
          AND completed_usage.status = 'completed'
    ) AS completed_cache_read_cost,
    (
        SELECT CAST(
            COALESCE(SUM(COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0)), 0)
                AS DOUBLE PRECISION
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_total_cost,
    (
        SELECT CAST(COUNT(settled_usage.id) AS BIGINT)
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_total_requests,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(settled_usage.input_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_input_tokens,
    (
        SELECT CAST(
            COALESCE(SUM(GREATEST(COALESCE(settled_usage.output_tokens, 0), 0)), 0) AS BIGINT
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_output_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(settled_usage.cache_creation_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_cache_creation_tokens,
    (
        SELECT CAST(
            COALESCE(
                SUM(GREATEST(COALESCE(settled_usage.cache_read_input_tokens, 0), 0)),
                0
            ) AS BIGINT
        )
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_cache_read_tokens,
    (
        SELECT MIN(CAST(EXTRACT(EPOCH FROM settled_usage.finalized_at) AS BIGINT))
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_first_finalized_at_unix_secs,
    (
        SELECT MAX(CAST(EXTRACT(EPOCH FROM settled_usage.finalized_at) AS BIGINT))
        FROM usage_billing_facts AS settled_usage
        WHERE settled_usage.created_at >= $1
          AND settled_usage.created_at < $2
          AND settled_usage.billing_status = 'settled'
          AND COALESCE(CAST(settled_usage.total_cost_usd AS DOUBLE PRECISION), 0) > 0
    ) AS settled_last_finalized_at_unix_secs,
    CAST(COUNT(id) AS BIGINT) AS total_requests,
    CAST(COALESCE(
        SUM(
            CASE
                WHEN status_code >= 400
                     OR lower(COALESCE(status, '')) = 'failed'
                     OR error_message IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    ) AS BIGINT) AS error_requests,
    CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
    CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
    CAST(COALESCE(
        SUM(
            CASE
                WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                     AND (
                        COALESCE(cache_creation_input_tokens_5m, 0)
                        + COALESCE(cache_creation_input_tokens_1h, 0)
                     ) > 0
                THEN COALESCE(cache_creation_input_tokens_5m, 0)
                   + COALESCE(cache_creation_input_tokens_1h, 0)
                ELSE COALESCE(cache_creation_input_tokens, 0)
            END
        ),
        0
    ) AS BIGINT) AS cache_creation_tokens,
    CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
    CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
    CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
    COALESCE(
        SUM(
            CASE
                WHEN response_time_ms IS NOT NULL
                THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                ELSE 0
            END
        ),
        0
    ) AS response_time_sum_ms,
    CAST(COALESCE(
        SUM(
            CASE
                WHEN response_time_ms IS NOT NULL THEN 1
                ELSE 0
            END
        ),
        0
    ) AS BIGINT) AS response_time_samples,
    CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms
FROM usage_billing_facts AS usage
WHERE created_at >= $1
  AND created_at < $2
  AND status NOT IN ('pending', 'streaming')
  AND provider_name NOT IN ('unknown', 'pending')
"#;
const UPSERT_STATS_HOURLY_SQL: &str = r#"
INSERT INTO stats_hourly (
    id,
    hour_utc,
    total_requests,
    cache_hit_total_requests,
    cache_hit_requests,
    completed_total_requests,
    completed_cache_hit_requests,
    completed_input_tokens,
    completed_cache_creation_tokens,
    completed_cache_read_tokens,
    completed_total_input_context,
    completed_cache_creation_cost,
    completed_cache_read_cost,
    settled_total_cost,
    settled_total_requests,
    settled_input_tokens,
    settled_output_tokens,
    settled_cache_creation_tokens,
    settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    response_time_sum_ms,
    response_time_samples,
    avg_response_time_ms,
    is_complete,
    aggregated_at,
    created_at,
    updated_at
)
VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8,
    $9, $10, $11, $12, $13, $14, $15, $16,
    $17, $18, $19, $20, $21, $22, $23, $24,
    $25, $26, $27, $28, $29, $30, $31, $32,
    $33, $34, $35, $36
)
ON CONFLICT (hour_utc)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    cache_hit_total_requests = EXCLUDED.cache_hit_total_requests,
    cache_hit_requests = EXCLUDED.cache_hit_requests,
    completed_total_requests = EXCLUDED.completed_total_requests,
    completed_cache_hit_requests = EXCLUDED.completed_cache_hit_requests,
    completed_input_tokens = EXCLUDED.completed_input_tokens,
    completed_cache_creation_tokens = EXCLUDED.completed_cache_creation_tokens,
    completed_cache_read_tokens = EXCLUDED.completed_cache_read_tokens,
    completed_total_input_context = EXCLUDED.completed_total_input_context,
    completed_cache_creation_cost = EXCLUDED.completed_cache_creation_cost,
    completed_cache_read_cost = EXCLUDED.completed_cache_read_cost,
    settled_total_cost = EXCLUDED.settled_total_cost,
    settled_total_requests = EXCLUDED.settled_total_requests,
    settled_input_tokens = EXCLUDED.settled_input_tokens,
    settled_output_tokens = EXCLUDED.settled_output_tokens,
    settled_cache_creation_tokens = EXCLUDED.settled_cache_creation_tokens,
    settled_cache_read_tokens = EXCLUDED.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = EXCLUDED.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = EXCLUDED.settled_last_finalized_at_unix_secs,
    success_requests = EXCLUDED.success_requests,
    error_requests = EXCLUDED.error_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    cache_creation_tokens = EXCLUDED.cache_creation_tokens,
    cache_read_tokens = EXCLUDED.cache_read_tokens,
    total_cost = EXCLUDED.total_cost,
    actual_total_cost = EXCLUDED.actual_total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    is_complete = EXCLUDED.is_complete,
    aggregated_at = EXCLUDED.aggregated_at,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_HOURLY_USER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN status_code >= 400
                         OR lower(COALESCE(status, '')) = 'failed'
                         OR error_message IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS error_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN COALESCE(cache_creation_input_tokens, 0) = 0
                         AND (
                            COALESCE(cache_creation_input_tokens_5m, 0)
                            + COALESCE(cache_creation_input_tokens_1h, 0)
                         ) > 0
                    THEN COALESCE(cache_creation_input_tokens_5m, 0)
                       + COALESCE(cache_creation_input_tokens_1h, 0)
                    ELSE COALESCE(cache_creation_input_tokens, 0)
                END
            ),
            0
        ) AS BIGINT) AS cache_creation_tokens,
        CAST(COALESCE(SUM(cache_read_input_tokens), 0) AS BIGINT) AS cache_read_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        CAST(COALESCE(SUM(actual_total_cost_usd), 0) AS DOUBLE PRECISION) AS actual_total_cost,
        CAST(
            COALESCE(
                SUM(
                    CASE
                        WHEN billing_status = 'settled'
                             AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                        THEN COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0)
                        ELSE 0
                    END
                ),
                0
            ) AS DOUBLE PRECISION
        ) AS settled_total_cost,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_total_requests,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN GREATEST(COALESCE(input_tokens, 0), 0)
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_input_tokens,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN GREATEST(COALESCE(output_tokens, 0), 0)
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_output_tokens,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN GREATEST(COALESCE(cache_creation_input_tokens, 0), 0)
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_cache_creation_tokens,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN billing_status = 'settled'
                         AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                    THEN GREATEST(COALESCE(cache_read_input_tokens, 0), 0)
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS settled_cache_read_tokens,
        MIN(
            CASE
                WHEN billing_status = 'settled'
                     AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                     AND finalized_at IS NOT NULL
                THEN CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT)
                ELSE NULL
            END
        ) AS settled_first_finalized_at_unix_secs,
        MAX(
            CASE
                WHEN billing_status = 'settled'
                     AND COALESCE(CAST(total_cost_usd AS DOUBLE PRECISION), 0) > 0
                     AND finalized_at IS NOT NULL
                THEN CAST(EXTRACT(EPOCH FROM finalized_at) AS BIGINT)
                ELSE NULL
            END
        ) AS settled_last_finalized_at_unix_secs,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id
)
INSERT INTO stats_hourly_user (
    id,
    hour_utc,
    user_id,
    total_requests,
    success_requests,
    error_requests,
    input_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_read_tokens,
    total_cost,
    actual_total_cost,
    settled_total_cost,
    settled_total_requests,
    settled_input_tokens,
    settled_output_tokens,
    settled_cache_creation_tokens,
    settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-hourly-user:', aggregated.user_id, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.user_id,
    aggregated.total_requests,
    GREATEST(aggregated.total_requests - aggregated.error_requests, 0),
    aggregated.error_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.cache_creation_tokens,
    aggregated.cache_read_tokens,
    aggregated.total_cost,
    aggregated.actual_total_cost,
    aggregated.settled_total_cost,
    aggregated.settled_total_requests,
    aggregated.settled_input_tokens,
    aggregated.settled_output_tokens,
    aggregated.settled_cache_creation_tokens,
    aggregated.settled_cache_read_tokens,
    aggregated.settled_first_finalized_at_unix_secs,
    aggregated.settled_last_finalized_at_unix_secs,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (hour_utc, user_id)
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
    settled_total_cost = EXCLUDED.settled_total_cost,
    settled_total_requests = EXCLUDED.settled_total_requests,
    settled_input_tokens = EXCLUDED.settled_input_tokens,
    settled_output_tokens = EXCLUDED.settled_output_tokens,
    settled_cache_creation_tokens = EXCLUDED.settled_cache_creation_tokens,
    settled_cache_read_tokens = EXCLUDED.settled_cache_read_tokens,
    settled_first_finalized_at_unix_secs = EXCLUDED.settled_first_finalized_at_unix_secs,
    settled_last_finalized_at_unix_secs = EXCLUDED.settled_last_finalized_at_unix_secs,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_HOURLY_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        model,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS response_time_samples,
        CAST(COALESCE(AVG(response_time_ms), 0) AS DOUBLE PRECISION) AS avg_response_time_ms
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND model IS NOT NULL
      AND model <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY model
)
INSERT INTO stats_hourly_model (
    id,
    hour_utc,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    avg_response_time_ms,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-hourly-model:', aggregated.model, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.model,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    aggregated.avg_response_time_ms,
    $3,
    $3
FROM aggregated
ON CONFLICT (hour_utc, model)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    avg_response_time_ms = EXCLUDED.avg_response_time_ms,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_HOURLY_USER_MODEL_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        user_id,
        model,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost,
        COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL
                    THEN GREATEST(COALESCE(response_time_ms, 0), 0)::DOUBLE PRECISION
                    ELSE 0
                END
            ),
            0
        ) AS response_time_sum_ms,
        CAST(COALESCE(
            SUM(
                CASE
                    WHEN response_time_ms IS NOT NULL THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS BIGINT) AS response_time_samples
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND user_id IS NOT NULL
      AND model IS NOT NULL
      AND model <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY user_id, model
)
INSERT INTO stats_hourly_user_model (
    id,
    hour_utc,
    user_id,
    model,
    total_requests,
    input_tokens,
    output_tokens,
    total_cost,
    response_time_sum_ms,
    response_time_samples,
    created_at,
    updated_at
)
SELECT
    md5(CONCAT('stats-hourly-user-model:', aggregated.user_id, ':', aggregated.model, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.user_id,
    aggregated.model,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.total_cost,
    aggregated.response_time_sum_ms,
    aggregated.response_time_samples,
    $3,
    $3
FROM aggregated
ON CONFLICT (hour_utc, user_id, model)
DO UPDATE SET
    total_requests = EXCLUDED.total_requests,
    input_tokens = EXCLUDED.input_tokens,
    output_tokens = EXCLUDED.output_tokens,
    total_cost = EXCLUDED.total_cost,
    response_time_sum_ms = EXCLUDED.response_time_sum_ms,
    response_time_samples = EXCLUDED.response_time_samples,
    updated_at = EXCLUDED.updated_at
"#;
const UPSERT_STATS_HOURLY_PROVIDER_SQL: &str = r#"
WITH aggregated AS (
    SELECT
        provider_name,
        CAST(COUNT(id) AS BIGINT) AS total_requests,
        CAST(COALESCE(SUM(input_tokens), 0) AS BIGINT) AS input_tokens,
        CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
        CAST(COALESCE(SUM(total_cost_usd), 0) AS DOUBLE PRECISION) AS total_cost
    FROM usage_billing_facts AS usage
    WHERE created_at >= $1
      AND created_at < $2
      AND provider_name IS NOT NULL
      AND provider_name <> ''
      AND status NOT IN ('pending', 'streaming')
      AND provider_name NOT IN ('unknown', 'pending')
    GROUP BY provider_name
)
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
SELECT
    md5(CONCAT('stats-hourly-provider:', aggregated.provider_name, ':', CAST($1 AS TEXT))),
    $1,
    aggregated.provider_name,
    aggregated.total_requests,
    aggregated.input_tokens,
    aggregated.output_tokens,
    aggregated.total_cost,
    $3,
    $3
FROM aggregated
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
    body_externalized: usize,
    legacy_body_refs_migrated: usize,
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
    request_id: String,
    request_body: Option<Value>,
    request_body_compressed: Option<Vec<u8>>,
    response_body: Option<Value>,
    response_body_compressed: Option<Vec<u8>>,
    provider_request_body: Option<Value>,
    provider_request_body_compressed: Option<Vec<u8>>,
    client_response_body: Option<Value>,
    client_response_body_compressed: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageBodyCleanupRow {
    id: String,
    request_id: String,
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
