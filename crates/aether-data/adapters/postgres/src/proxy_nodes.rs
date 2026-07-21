use async_trait::async_trait;
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use aether_data_contracts::repository::proxy_nodes::{
    bucket_start_unix_secs, build_tunnel_error_event_detail, build_tunnel_metrics_sample,
    normalize_proxy_metadata, preserve_proxy_metadata_tunnel_security,
    reconcile_remote_config_after_heartbeat, ProxyNodeEventQuery, ProxyNodeHeartbeatMutation,
    ProxyNodeManualCreateMutation, ProxyNodeManualUpdateMutation, ProxyNodeMetricsCleanupSummary,
    ProxyNodeMetricsStep, ProxyNodeReadRepository, ProxyNodeRegistrationMutation,
    ProxyNodeRemoteConfigMutation, ProxyNodeTrafficMutation, ProxyNodeTunnelStatusMutation,
    ProxyNodeWriteRepository, StoredProxyFleetMetricsBucket, StoredProxyNode, StoredProxyNodeEvent,
    StoredProxyNodeMetricsBucket, TunnelErrorEventRecord, TunnelMetricsSample,
    PROXY_NODE_EVENT_TYPE_TUNNEL_ERROR,
};
use aether_data_contracts::DataLayerError;
use aether_data_query::{push_eq, push_limit, WhereClause};

use crate::error::{postgres_error, SqlxResultExt};

fn log_reported_tunnel_error_event(
    node_id: &str,
    event: &TunnelErrorEventRecord,
    received_at_unix_secs: u64,
) {
    tracing::warn!(
        event_name = "proxy_tunnel_error_reported",
        source = "heartbeat",
        node_id = %node_id,
        category = %event.category,
        message = %event.message,
        severity = ?event.severity,
        component = ?event.component,
        summary = ?event.summary,
        operator_action = ?event.operator_action,
        error_reported_at_unix_secs = event.timestamp_unix_secs,
        error_reported_at_unix_ms = ?event.timestamp_unix_ms,
        report_received_at_unix_secs = received_at_unix_secs,
        "proxy reported tunnel error via heartbeat"
    );
}

const FIND_PROXY_NODE_SQL: &str = r#"
SELECT
  id,
  name,
  ip,
  port,
  region,
  is_manual,
  proxy_url,
  proxy_username,
  proxy_password,
  CAST(status AS TEXT) AS status,
  registered_by,
  EXTRACT(EPOCH FROM last_heartbeat_at)::bigint AS last_heartbeat_at_unix_secs,
  heartbeat_interval,
  active_connections,
  total_requests,
  CAST(avg_latency_ms AS DOUBLE PRECISION) AS avg_latency_ms,
  failed_requests,
  dns_failures,
  stream_errors,
  proxy_metadata,
  hardware_info,
  estimated_max_concurrency,
  tunnel_mode,
  tunnel_connected,
  EXTRACT(EPOCH FROM tunnel_connected_at)::bigint AS tunnel_connected_at_unix_secs,
  remote_config,
  config_version,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM proxy_nodes
WHERE id = $1
LIMIT 1
"#;

const LIST_PROXY_NODE_EVENTS_SQL: &str = r#"
SELECT
  id,
  node_id,
  CAST(event_type AS TEXT) AS event_type,
  detail,
  event_metadata,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms
FROM proxy_node_events
WHERE node_id = $1
ORDER BY created_at DESC, id DESC
LIMIT $2
"#;

const APPLY_HEARTBEAT_SQL: &str = r#"
UPDATE proxy_nodes
SET
  last_heartbeat_at = NOW(),
  status = CASE
    WHEN status <> 'online'::proxynodestatus OR tunnel_connected = FALSE
      THEN 'online'::proxynodestatus
    ELSE status
  END,
  tunnel_connected = CASE
    WHEN status <> 'online'::proxynodestatus OR tunnel_connected = FALSE
      THEN TRUE
    ELSE tunnel_connected
  END,
  tunnel_connected_at = CASE
    WHEN status <> 'online'::proxynodestatus OR tunnel_connected = FALSE
      THEN NOW()
    ELSE tunnel_connected_at
  END,
  updated_at = CASE
    WHEN status <> 'online'::proxynodestatus OR tunnel_connected = FALSE
      THEN NOW()
    ELSE updated_at
  END,
  heartbeat_interval = COALESCE($2, heartbeat_interval),
  active_connections = COALESCE($3, active_connections),
  avg_latency_ms = COALESCE($4, avg_latency_ms),
  proxy_metadata = COALESCE($5::json, proxy_metadata),
  total_requests = total_requests + GREATEST(COALESCE($6, 0), 0),
  failed_requests = failed_requests + GREATEST(COALESCE($7, 0), 0),
  dns_failures = dns_failures + GREATEST(COALESCE($8, 0), 0),
  stream_errors = stream_errors + GREATEST(COALESCE($9, 0), 0)
WHERE id = $1
"#;

const FIND_EXISTING_TUNNEL_NODE_SQL: &str = r#"
SELECT
  id,
  name,
  ip,
  port,
  region,
  is_manual,
  proxy_url,
  proxy_username,
  proxy_password,
  CAST(status AS TEXT) AS status,
  registered_by,
  EXTRACT(EPOCH FROM last_heartbeat_at)::bigint AS last_heartbeat_at_unix_secs,
  heartbeat_interval,
  active_connections,
  total_requests,
  CAST(avg_latency_ms AS DOUBLE PRECISION) AS avg_latency_ms,
  failed_requests,
  dns_failures,
  stream_errors,
  proxy_metadata,
  hardware_info,
  estimated_max_concurrency,
  tunnel_mode,
  tunnel_connected,
  EXTRACT(EPOCH FROM tunnel_connected_at)::bigint AS tunnel_connected_at_unix_secs,
  remote_config,
  config_version,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
FROM proxy_nodes
WHERE ip = $1
  AND port = $2
  AND is_manual = FALSE
ORDER BY created_at ASC, id ASC
LIMIT 1
FOR UPDATE
"#;

const INSERT_PROXY_NODE_SQL: &str = r#"
INSERT INTO proxy_nodes (
  id,
  name,
  ip,
  port,
  region,
  status,
  registered_by,
  last_heartbeat_at,
  heartbeat_interval,
  active_connections,
  total_requests,
  avg_latency_ms,
  hardware_info,
  estimated_max_concurrency,
  tunnel_mode,
  tunnel_connected,
  proxy_metadata
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  'offline'::proxynodestatus,
  $6,
  NOW(),
  $7,
  COALESCE($8, 0),
  COALESCE($9, 0),
  $10,
  $11::json,
  $12,
  $13,
  FALSE,
  $14::json
)
"#;

const FIND_DUPLICATE_PROXY_NODE_SQL: &str = r#"
SELECT
  id,
  name,
  ip,
  port
FROM proxy_nodes
WHERE ip = $1
  AND port = $2
LIMIT 1
FOR UPDATE
"#;

const FIND_DUPLICATE_PROXY_NODE_EXCLUDING_ID_SQL: &str = r#"
SELECT
  id,
  name,
  ip,
  port
FROM proxy_nodes
WHERE ip = $1
  AND port = $2
  AND id <> $3
LIMIT 1
FOR UPDATE
"#;

const INSERT_MANUAL_PROXY_NODE_SQL: &str = r#"
INSERT INTO proxy_nodes (
  id,
  name,
  ip,
  port,
  region,
  is_manual,
  proxy_url,
  proxy_username,
  proxy_password,
  status,
  registered_by,
  last_heartbeat_at,
  heartbeat_interval,
  active_connections,
  total_requests,
  tunnel_mode,
  tunnel_connected,
  config_version
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  TRUE,
  $6,
  $7,
  $8,
  'online'::proxynodestatus,
  $9,
  NULL,
  0,
  0,
  0,
  FALSE,
  FALSE,
  0
)
"#;

const UPDATE_PROXY_NODE_REGISTRATION_SQL: &str = r#"
UPDATE proxy_nodes
SET
  name = $2,
  ip = $3,
  port = $4,
  region = $5,
  registered_by = $6,
  last_heartbeat_at = NOW(),
  heartbeat_interval = $7,
  active_connections = COALESCE($8, active_connections),
  total_requests = COALESCE($9, total_requests),
  avg_latency_ms = COALESCE($10, avg_latency_ms),
  hardware_info = COALESCE($11::json, hardware_info),
  estimated_max_concurrency = COALESCE($12, estimated_max_concurrency),
  tunnel_mode = $13,
  proxy_metadata = COALESCE($14::json, proxy_metadata),
  updated_at = NOW()
WHERE id = $1
"#;

const UPDATE_MANUAL_PROXY_NODE_SQL: &str = r#"
UPDATE proxy_nodes
SET
  name = COALESCE($2, name),
  ip = COALESCE($3, ip),
  port = COALESCE($4, port),
  region = COALESCE($5, region),
  proxy_url = COALESCE($6, proxy_url),
  proxy_username = COALESCE($7, proxy_username),
  proxy_password = COALESCE($8, proxy_password),
  updated_at = NOW()
WHERE id = $1
  AND is_manual = TRUE
"#;

const RECORD_PROXY_NODE_TRAFFIC_SQL: &str = r#"
UPDATE proxy_nodes
SET
  total_requests = total_requests + GREATEST($2, 0),
  failed_requests = failed_requests + GREATEST($3, 0),
  dns_failures = dns_failures + GREATEST($4, 0),
  stream_errors = stream_errors + GREATEST($5, 0),
  updated_at = NOW()
WHERE id = $1
  AND is_manual = TRUE
"#;

const UNREGISTER_PROXY_NODE_SQL: &str = r#"
UPDATE proxy_nodes
SET
  status = 'offline'::proxynodestatus,
  tunnel_connected = FALSE,
  tunnel_connected_at = NOW(),
  updated_at = NOW()
WHERE id = $1
"#;

const DELETE_PROXY_NODE_SQL: &str = r#"
DELETE FROM proxy_nodes
WHERE id = $1
"#;

const UPDATE_PROXY_NODE_REMOTE_CONFIG_SQL: &str = r#"
UPDATE proxy_nodes
SET
  name = COALESCE($2, name),
  remote_config = $3::json,
  config_version = config_version + 1,
  updated_at = NOW()
WHERE id = $1
"#;

const RESET_STALE_TUNNEL_STATUSES_SQL: &str = r#"
UPDATE proxy_nodes
SET
  tunnel_connected = FALSE,
  status = 'offline'::proxynodestatus,
  active_connections = 0,
  tunnel_connected_at = NOW(),
  updated_at = NOW()
WHERE is_manual = FALSE
  AND tunnel_connected = TRUE
"#;

const INCREMENT_MANUAL_PROXY_NODE_REQUESTS_SQL: &str = r#"
UPDATE proxy_nodes
SET
  total_requests = total_requests + GREATEST($1::bigint, 0),
  failed_requests = failed_requests + GREATEST($2::bigint, 0),
  avg_latency_ms = COALESCE($3, avg_latency_ms),
  last_heartbeat_at = NOW(),
  updated_at = NOW()
WHERE id = $4
  AND is_manual = TRUE
"#;

const INSERT_PROXY_NODE_EVENT_SQL: &str = r#"
INSERT INTO proxy_node_events (node_id, event_type, detail, event_metadata, created_at)
VALUES (
  $1,
  $2,
  $3,
  $4::json,
  CASE
    WHEN $5::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($5::double precision)
  END
)
"#;

const UPSERT_PROXY_NODE_METRICS_1M_SQL: &str = r#"
INSERT INTO proxy_node_metrics_1m (
  node_id,
  bucket_start_unix_secs,
  samples,
  uptime_samples,
  active_connections_sum,
  active_connections_max,
  heartbeat_rtt_ms_sum,
  heartbeat_rtt_ms_max,
  connect_errors_delta,
  disconnects_delta,
  error_events_delta,
  ws_in_bytes_delta,
  ws_out_bytes_delta,
  ws_in_frames_delta,
  ws_out_frames_delta
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
ON CONFLICT (node_id, bucket_start_unix_secs) DO UPDATE SET
  samples = proxy_node_metrics_1m.samples + EXCLUDED.samples,
  uptime_samples = proxy_node_metrics_1m.uptime_samples + EXCLUDED.uptime_samples,
  active_connections_sum = proxy_node_metrics_1m.active_connections_sum + EXCLUDED.active_connections_sum,
  active_connections_max = GREATEST(proxy_node_metrics_1m.active_connections_max, EXCLUDED.active_connections_max),
  heartbeat_rtt_ms_sum = proxy_node_metrics_1m.heartbeat_rtt_ms_sum + EXCLUDED.heartbeat_rtt_ms_sum,
  heartbeat_rtt_ms_max = GREATEST(proxy_node_metrics_1m.heartbeat_rtt_ms_max, EXCLUDED.heartbeat_rtt_ms_max),
  connect_errors_delta = proxy_node_metrics_1m.connect_errors_delta + EXCLUDED.connect_errors_delta,
  disconnects_delta = proxy_node_metrics_1m.disconnects_delta + EXCLUDED.disconnects_delta,
  error_events_delta = proxy_node_metrics_1m.error_events_delta + EXCLUDED.error_events_delta,
  ws_in_bytes_delta = proxy_node_metrics_1m.ws_in_bytes_delta + EXCLUDED.ws_in_bytes_delta,
  ws_out_bytes_delta = proxy_node_metrics_1m.ws_out_bytes_delta + EXCLUDED.ws_out_bytes_delta,
  ws_in_frames_delta = proxy_node_metrics_1m.ws_in_frames_delta + EXCLUDED.ws_in_frames_delta,
  ws_out_frames_delta = proxy_node_metrics_1m.ws_out_frames_delta + EXCLUDED.ws_out_frames_delta
"#;

const UPSERT_PROXY_NODE_METRICS_1H_SQL: &str = r#"
INSERT INTO proxy_node_metrics_1h (
  node_id,
  bucket_start_unix_secs,
  samples,
  uptime_samples,
  active_connections_sum,
  active_connections_max,
  heartbeat_rtt_ms_sum,
  heartbeat_rtt_ms_max,
  connect_errors_delta,
  disconnects_delta,
  error_events_delta,
  ws_in_bytes_delta,
  ws_out_bytes_delta,
  ws_in_frames_delta,
  ws_out_frames_delta
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
ON CONFLICT (node_id, bucket_start_unix_secs) DO UPDATE SET
  samples = proxy_node_metrics_1h.samples + EXCLUDED.samples,
  uptime_samples = proxy_node_metrics_1h.uptime_samples + EXCLUDED.uptime_samples,
  active_connections_sum = proxy_node_metrics_1h.active_connections_sum + EXCLUDED.active_connections_sum,
  active_connections_max = GREATEST(proxy_node_metrics_1h.active_connections_max, EXCLUDED.active_connections_max),
  heartbeat_rtt_ms_sum = proxy_node_metrics_1h.heartbeat_rtt_ms_sum + EXCLUDED.heartbeat_rtt_ms_sum,
  heartbeat_rtt_ms_max = GREATEST(proxy_node_metrics_1h.heartbeat_rtt_ms_max, EXCLUDED.heartbeat_rtt_ms_max),
  connect_errors_delta = proxy_node_metrics_1h.connect_errors_delta + EXCLUDED.connect_errors_delta,
  disconnects_delta = proxy_node_metrics_1h.disconnects_delta + EXCLUDED.disconnects_delta,
  error_events_delta = proxy_node_metrics_1h.error_events_delta + EXCLUDED.error_events_delta,
  ws_in_bytes_delta = proxy_node_metrics_1h.ws_in_bytes_delta + EXCLUDED.ws_in_bytes_delta,
  ws_out_bytes_delta = proxy_node_metrics_1h.ws_out_bytes_delta + EXCLUDED.ws_out_bytes_delta,
  ws_in_frames_delta = proxy_node_metrics_1h.ws_in_frames_delta + EXCLUDED.ws_in_frames_delta,
  ws_out_frames_delta = proxy_node_metrics_1h.ws_out_frames_delta + EXCLUDED.ws_out_frames_delta
"#;

const LIST_PROXY_NODE_METRICS_1M_SQL: &str = r#"
SELECT
  node_id,
  bucket_start_unix_secs,
  samples,
  uptime_samples,
  active_connections_sum,
  active_connections_max,
  heartbeat_rtt_ms_sum,
  heartbeat_rtt_ms_max,
  connect_errors_delta,
  disconnects_delta,
  error_events_delta,
  ws_in_bytes_delta,
  ws_out_bytes_delta,
  ws_in_frames_delta,
  ws_out_frames_delta
FROM proxy_node_metrics_1m
WHERE node_id = $1
  AND bucket_start_unix_secs >= $2
  AND bucket_start_unix_secs <= $3
ORDER BY bucket_start_unix_secs ASC
LIMIT $4
"#;

const LIST_PROXY_NODE_METRICS_1H_SQL: &str = r#"
SELECT
  node_id,
  bucket_start_unix_secs,
  samples,
  uptime_samples,
  active_connections_sum,
  active_connections_max,
  heartbeat_rtt_ms_sum,
  heartbeat_rtt_ms_max,
  connect_errors_delta,
  disconnects_delta,
  error_events_delta,
  ws_in_bytes_delta,
  ws_out_bytes_delta,
  ws_in_frames_delta,
  ws_out_frames_delta
FROM proxy_node_metrics_1h
WHERE node_id = $1
  AND bucket_start_unix_secs >= $2
  AND bucket_start_unix_secs <= $3
ORDER BY bucket_start_unix_secs ASC
LIMIT $4
"#;

const LIST_PROXY_FLEET_METRICS_1M_SQL: &str = r#"
SELECT
  bucket_start_unix_secs,
  SUM(samples) AS samples,
  SUM(uptime_samples) AS uptime_samples,
  SUM(active_connections_sum) AS active_connections_sum,
  MAX(active_connections_max) AS active_connections_max,
  SUM(heartbeat_rtt_ms_sum) AS heartbeat_rtt_ms_sum,
  MAX(heartbeat_rtt_ms_max) AS heartbeat_rtt_ms_max,
  SUM(connect_errors_delta) AS connect_errors_delta,
  SUM(disconnects_delta) AS disconnects_delta,
  SUM(error_events_delta) AS error_events_delta,
  SUM(ws_in_bytes_delta) AS ws_in_bytes_delta,
  SUM(ws_out_bytes_delta) AS ws_out_bytes_delta,
  SUM(ws_in_frames_delta) AS ws_in_frames_delta,
  SUM(ws_out_frames_delta) AS ws_out_frames_delta
FROM proxy_node_metrics_1m
WHERE bucket_start_unix_secs >= $1
  AND bucket_start_unix_secs <= $2
GROUP BY bucket_start_unix_secs
ORDER BY bucket_start_unix_secs ASC
LIMIT $3
"#;

const LIST_PROXY_FLEET_METRICS_1H_SQL: &str = r#"
SELECT
  bucket_start_unix_secs,
  SUM(samples) AS samples,
  SUM(uptime_samples) AS uptime_samples,
  SUM(active_connections_sum) AS active_connections_sum,
  MAX(active_connections_max) AS active_connections_max,
  SUM(heartbeat_rtt_ms_sum) AS heartbeat_rtt_ms_sum,
  MAX(heartbeat_rtt_ms_max) AS heartbeat_rtt_ms_max,
  SUM(connect_errors_delta) AS connect_errors_delta,
  SUM(disconnects_delta) AS disconnects_delta,
  SUM(error_events_delta) AS error_events_delta,
  SUM(ws_in_bytes_delta) AS ws_in_bytes_delta,
  SUM(ws_out_bytes_delta) AS ws_out_bytes_delta,
  SUM(ws_in_frames_delta) AS ws_in_frames_delta,
  SUM(ws_out_frames_delta) AS ws_out_frames_delta
FROM proxy_node_metrics_1h
WHERE bucket_start_unix_secs >= $1
  AND bucket_start_unix_secs <= $2
GROUP BY bucket_start_unix_secs
ORDER BY bucket_start_unix_secs ASC
LIMIT $3
"#;

#[derive(Debug, Clone)]
pub struct SqlxProxyNodeRepository {
    pool: PgPool,
}

impl SqlxProxyNodeRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn optional_unix_secs(value: Option<i64>) -> Option<u64> {
        value.and_then(|value| u64::try_from(value).ok())
    }

    fn row_to_stored(row: &PgRow) -> Result<StoredProxyNode, DataLayerError> {
        Ok(StoredProxyNode::new(
            row.try_get("id").map_postgres_err()?,
            row.try_get("name").map_postgres_err()?,
            row.try_get("ip").map_postgres_err()?,
            row.try_get("port").map_postgres_err()?,
            row.try_get("is_manual").map_postgres_err()?,
            row.try_get("status").map_postgres_err()?,
            row.try_get("heartbeat_interval").map_postgres_err()?,
            row.try_get("active_connections").map_postgres_err()?,
            row.try_get("total_requests").map_postgres_err()?,
            row.try_get("failed_requests").map_postgres_err()?,
            row.try_get("dns_failures").map_postgres_err()?,
            row.try_get("stream_errors").map_postgres_err()?,
            row.try_get("tunnel_mode").map_postgres_err()?,
            row.try_get("tunnel_connected").map_postgres_err()?,
            row.try_get("config_version").map_postgres_err()?,
        )?
        .with_manual_proxy_fields(
            row.try_get("proxy_url").map_postgres_err()?,
            row.try_get("proxy_username").map_postgres_err()?,
            row.try_get("proxy_password").map_postgres_err()?,
        )
        .with_runtime_fields(
            row.try_get("region").map_postgres_err()?,
            row.try_get("registered_by").map_postgres_err()?,
            Self::optional_unix_secs(
                row.try_get("last_heartbeat_at_unix_secs")
                    .map_postgres_err()?,
            ),
            row.try_get("avg_latency_ms").map_postgres_err()?,
            row.try_get("proxy_metadata").map_postgres_err()?,
            row.try_get("hardware_info").map_postgres_err()?,
            row.try_get("estimated_max_concurrency")
                .map_postgres_err()?,
            Self::optional_unix_secs(
                row.try_get("tunnel_connected_at_unix_secs")
                    .map_postgres_err()?,
            ),
            row.try_get("remote_config").map_postgres_err()?,
            Self::optional_unix_secs(row.try_get("created_at_unix_ms").map_postgres_err()?),
            Self::optional_unix_secs(row.try_get("updated_at_unix_secs").map_postgres_err()?),
        ))
    }

    fn row_to_event(row: &PgRow) -> Result<StoredProxyNodeEvent, DataLayerError> {
        Ok(StoredProxyNodeEvent {
            id: row.try_get("id").map_postgres_err()?,
            node_id: row.try_get("node_id").map_postgres_err()?,
            event_type: row.try_get("event_type").map_postgres_err()?,
            detail: row.try_get("detail").map_postgres_err()?,
            event_metadata: row.try_get("event_metadata").map_postgres_err()?,
            created_at_unix_ms: Self::optional_unix_secs(
                row.try_get("created_at_unix_ms").map_postgres_err()?,
            ),
        })
    }

    fn row_to_node_metric(row: &PgRow) -> Result<StoredProxyNodeMetricsBucket, DataLayerError> {
        Ok(StoredProxyNodeMetricsBucket {
            node_id: row.try_get("node_id").map_postgres_err()?,
            bucket_start_unix_secs: Self::optional_unix_secs(
                row.try_get("bucket_start_unix_secs").map_postgres_err()?,
            )
            .unwrap_or_default(),
            samples: row.try_get("samples").map_postgres_err()?,
            uptime_samples: row.try_get("uptime_samples").map_postgres_err()?,
            active_connections_sum: row.try_get("active_connections_sum").map_postgres_err()?,
            active_connections_max: row.try_get("active_connections_max").map_postgres_err()?,
            heartbeat_rtt_ms_sum: row.try_get("heartbeat_rtt_ms_sum").map_postgres_err()?,
            heartbeat_rtt_ms_max: row.try_get("heartbeat_rtt_ms_max").map_postgres_err()?,
            connect_errors_delta: row.try_get("connect_errors_delta").map_postgres_err()?,
            disconnects_delta: row.try_get("disconnects_delta").map_postgres_err()?,
            error_events_delta: row.try_get("error_events_delta").map_postgres_err()?,
            ws_in_bytes_delta: row.try_get("ws_in_bytes_delta").map_postgres_err()?,
            ws_out_bytes_delta: row.try_get("ws_out_bytes_delta").map_postgres_err()?,
            ws_in_frames_delta: row.try_get("ws_in_frames_delta").map_postgres_err()?,
            ws_out_frames_delta: row.try_get("ws_out_frames_delta").map_postgres_err()?,
        })
    }

    fn row_to_fleet_metric(row: &PgRow) -> Result<StoredProxyFleetMetricsBucket, DataLayerError> {
        Ok(StoredProxyFleetMetricsBucket {
            bucket_start_unix_secs: Self::optional_unix_secs(
                row.try_get("bucket_start_unix_secs").map_postgres_err()?,
            )
            .unwrap_or_default(),
            samples: row.try_get("samples").map_postgres_err()?,
            uptime_samples: row.try_get("uptime_samples").map_postgres_err()?,
            active_connections_sum: row.try_get("active_connections_sum").map_postgres_err()?,
            active_connections_max: row.try_get("active_connections_max").map_postgres_err()?,
            heartbeat_rtt_ms_sum: row.try_get("heartbeat_rtt_ms_sum").map_postgres_err()?,
            heartbeat_rtt_ms_max: row.try_get("heartbeat_rtt_ms_max").map_postgres_err()?,
            connect_errors_delta: row.try_get("connect_errors_delta").map_postgres_err()?,
            disconnects_delta: row.try_get("disconnects_delta").map_postgres_err()?,
            error_events_delta: row.try_get("error_events_delta").map_postgres_err()?,
            ws_in_bytes_delta: row.try_get("ws_in_bytes_delta").map_postgres_err()?,
            ws_out_bytes_delta: row.try_get("ws_out_bytes_delta").map_postgres_err()?,
            ws_in_frames_delta: row.try_get("ws_in_frames_delta").map_postgres_err()?,
            ws_out_frames_delta: row.try_get("ws_out_frames_delta").map_postgres_err()?,
        })
    }

    async fn insert_event(
        &self,
        node_id: &str,
        event_type: &str,
        detail: Option<&str>,
        event_metadata: Option<&serde_json::Value>,
        created_at_unix_secs: Option<u64>,
    ) -> Result<(), DataLayerError> {
        sqlx::query(INSERT_PROXY_NODE_EVENT_SQL)
            .bind(node_id)
            .bind(event_type)
            .bind(detail)
            .bind(event_metadata)
            .bind(created_at_unix_secs.map(|value| value as f64))
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(())
    }

    async fn upsert_metrics_bucket(
        &self,
        step: ProxyNodeMetricsStep,
        node_id: &str,
        bucket_start: u64,
        sample: &TunnelMetricsSample,
    ) -> Result<(), DataLayerError> {
        let sql = match step {
            ProxyNodeMetricsStep::OneMinute => UPSERT_PROXY_NODE_METRICS_1M_SQL,
            ProxyNodeMetricsStep::OneHour => UPSERT_PROXY_NODE_METRICS_1H_SQL,
        };
        sqlx::query(sql)
            .bind(node_id)
            .bind(i64::try_from(bucket_start).unwrap_or(i64::MAX))
            .bind(sample.samples)
            .bind(sample.uptime_samples)
            .bind(sample.active_connections_sum)
            .bind(sample.active_connections_max)
            .bind(sample.heartbeat_rtt_ms_sum)
            .bind(sample.heartbeat_rtt_ms_max)
            .bind(sample.connect_errors_delta)
            .bind(sample.disconnects_delta)
            .bind(sample.error_events_delta)
            .bind(sample.ws_in_bytes_delta)
            .bind(sample.ws_out_bytes_delta)
            .bind(sample.ws_in_frames_delta)
            .bind(sample.ws_out_frames_delta)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(())
    }

    fn registration_lock_key(ip: &str, port: i32) -> i64 {
        let mut hasher = Sha256::new();
        hasher.update(ip.as_bytes());
        hasher.update(b":");
        hasher.update(port.to_string().as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        i64::from_be_bytes(bytes)
    }

    fn normalize_remote_config(
        mutation: &ProxyNodeRemoteConfigMutation,
        existing: Option<&serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let mut config = match existing {
            Some(serde_json::Value::Object(map)) => map.clone(),
            _ => serde_json::Map::new(),
        };

        if let Some(node_name) = mutation.node_name.as_ref() {
            config.insert(
                "node_name".to_string(),
                serde_json::Value::String(node_name.clone()),
            );
        }
        if let Some(allowed_ports) = mutation.allowed_ports.as_ref() {
            config.insert(
                "allowed_ports".to_string(),
                serde_json::json!(allowed_ports),
            );
        }
        if let Some(log_level) = mutation.log_level.as_ref() {
            config.insert(
                "log_level".to_string(),
                serde_json::Value::String(log_level.clone()),
            );
        }
        if let Some(heartbeat_interval) = mutation.heartbeat_interval {
            config.insert(
                "heartbeat_interval".to_string(),
                serde_json::json!(heartbeat_interval),
            );
        }
        if let Some(scheduling_state) = mutation.scheduling_state.as_ref() {
            match scheduling_state {
                Some(state) => {
                    config.insert(
                        "scheduling_state".to_string(),
                        serde_json::Value::String(state.clone()),
                    );
                }
                None => {
                    config.remove("scheduling_state");
                }
            }
        }
        if let Some(upgrade_to) = mutation.upgrade_to.as_ref() {
            match upgrade_to {
                Some(version) => {
                    config.insert(
                        "upgrade_to".to_string(),
                        serde_json::Value::String(version.clone()),
                    );
                }
                None => {
                    config.remove("upgrade_to");
                }
            }
        }

        (!config.is_empty()).then_some(serde_json::Value::Object(config))
    }

    fn duplicate_proxy_node_detail(name: &str, ip: &str, port: i32) -> String {
        format!("已存在相同地址的代理节点: {name} ({ip}:{port})")
    }

    async fn find_duplicate_proxy_node_locked(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        ip: &str,
        port: i32,
        exclude_node_id: Option<&str>,
    ) -> Result<Option<(String, String, i32)>, DataLayerError> {
        let row = if let Some(exclude_node_id) = exclude_node_id {
            sqlx::query(FIND_DUPLICATE_PROXY_NODE_EXCLUDING_ID_SQL)
                .bind(ip)
                .bind(port)
                .bind(exclude_node_id)
                .fetch_optional(&mut **tx)
                .await
                .map_postgres_err()?
        } else {
            sqlx::query(FIND_DUPLICATE_PROXY_NODE_SQL)
                .bind(ip)
                .bind(port)
                .fetch_optional(&mut **tx)
                .await
                .map_postgres_err()?
        };

        row.map(|row| {
            Ok((
                row.try_get("name").map_postgres_err()?,
                row.try_get("ip").map_postgres_err()?,
                row.try_get("port").map_postgres_err()?,
            ))
        })
        .transpose()
    }
}

#[async_trait]
impl ProxyNodeReadRepository for SqlxProxyNodeRepository {
    async fn list_proxy_nodes(&self) -> Result<Vec<StoredProxyNode>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(proxy_node_columns());
        builder.push(" ORDER BY name ASC, id ASC");
        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(Self::row_to_stored(&row)?);
        }
        Ok(items)
    }

    async fn find_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(proxy_node_columns());
        let mut where_clause = WhereClause::new();
        push_eq(&mut builder, &mut where_clause, "id", node_id.to_string());
        push_limit(&mut builder, 1);
        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.map(|row| Self::row_to_stored(&row)).transpose()
    }

    async fn list_proxy_node_events(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeEvent>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(proxy_node_event_columns());
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "node_id",
            node_id.to_string(),
        );
        builder.push(" ORDER BY created_at DESC, id DESC");
        push_limit(&mut builder, i64::try_from(limit).unwrap_or(i64::MAX));
        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(Self::row_to_event(&row)?);
        }
        Ok(items)
    }

    async fn list_proxy_node_events_filtered(
        &self,
        node_id: &str,
        query: &ProxyNodeEventQuery,
    ) -> Result<Vec<StoredProxyNodeEvent>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(proxy_node_event_columns());
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "node_id",
            node_id.to_string(),
        );
        if let Some(from_unix_secs) = query.from_unix_secs {
            where_clause.push_next(&mut builder);
            builder
                .push("created_at >= TO_TIMESTAMP(")
                .push_bind(from_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(to_unix_secs) = query.to_unix_secs {
            where_clause.push_next(&mut builder);
            builder
                .push("created_at <= TO_TIMESTAMP(")
                .push_bind(to_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(event_type) = query.event_type.as_deref() {
            where_clause.push_next(&mut builder);
            builder
                .push("LOWER(CAST(event_type AS TEXT)) = LOWER(")
                .push_bind(event_type.to_string())
                .push("::text)");
        }
        builder.push(" ORDER BY created_at DESC, id DESC");
        push_limit(&mut builder, i64::try_from(query.limit).unwrap_or(i64::MAX));
        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(Self::row_to_event(&row)?);
        }
        Ok(items)
    }

    async fn list_proxy_node_metrics(
        &self,
        node_id: &str,
        step: ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeMetricsBucket>, DataLayerError> {
        let sql = match step {
            ProxyNodeMetricsStep::OneMinute => LIST_PROXY_NODE_METRICS_1M_SQL,
            ProxyNodeMetricsStep::OneHour => LIST_PROXY_NODE_METRICS_1H_SQL,
        };
        let mut rows = sqlx::query(sql)
            .bind(node_id)
            .bind(i64::try_from(from_unix_secs).unwrap_or(i64::MAX))
            .bind(i64::try_from(to_unix_secs).unwrap_or(i64::MAX))
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(Self::row_to_node_metric(&row)?);
        }
        Ok(items)
    }

    async fn list_proxy_fleet_metrics(
        &self,
        step: ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyFleetMetricsBucket>, DataLayerError> {
        let sql = match step {
            ProxyNodeMetricsStep::OneMinute => LIST_PROXY_FLEET_METRICS_1M_SQL,
            ProxyNodeMetricsStep::OneHour => LIST_PROXY_FLEET_METRICS_1H_SQL,
        };
        let mut rows = sqlx::query(sql)
            .bind(i64::try_from(from_unix_secs).unwrap_or(i64::MAX))
            .bind(i64::try_from(to_unix_secs).unwrap_or(i64::MAX))
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(Self::row_to_fleet_metric(&row)?);
        }
        Ok(items)
    }
}

fn proxy_node_columns() -> &'static str {
    FIND_PROXY_NODE_SQL
        .split_once("WHERE id = $1")
        .map(|(prefix, _)| prefix)
        .unwrap_or(FIND_PROXY_NODE_SQL)
}

fn proxy_node_event_columns() -> &'static str {
    LIST_PROXY_NODE_EVENTS_SQL
        .split_once("WHERE node_id = $1")
        .map(|(prefix, _)| prefix)
        .unwrap_or(LIST_PROXY_NODE_EVENTS_SQL)
}

#[async_trait]
impl ProxyNodeWriteRepository for SqlxProxyNodeRepository {
    async fn reset_stale_tunnel_statuses(&self) -> Result<usize, DataLayerError> {
        let result = sqlx::query(RESET_STALE_TUNNEL_STATUSES_SQL)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() as usize)
    }

    async fn create_manual_node(
        &self,
        mutation: &ProxyNodeManualCreateMutation,
    ) -> Result<StoredProxyNode, DataLayerError> {
        let lock_key = Self::registration_lock_key(&mutation.ip, mutation.port);
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(lock_key)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;

        if let Some((name, ip, port)) =
            Self::find_duplicate_proxy_node_locked(&mut tx, &mutation.ip, mutation.port, None)
                .await?
        {
            return Err(DataLayerError::InvalidInput(
                Self::duplicate_proxy_node_detail(&name, &ip, port),
            ));
        }

        let node_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(INSERT_MANUAL_PROXY_NODE_SQL)
            .bind(&node_id)
            .bind(&mutation.name)
            .bind(&mutation.ip)
            .bind(mutation.port)
            .bind(mutation.region.as_deref())
            .bind(&mutation.proxy_url)
            .bind(mutation.proxy_username.as_deref())
            .bind(mutation.proxy_password.as_deref())
            .bind(mutation.registered_by.as_deref())
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;

        tx.commit().await.map_err(postgres_error)?;
        self.find_proxy_node(&node_id).await?.ok_or_else(|| {
            DataLayerError::UnexpectedValue("created manual proxy node missing".to_string())
        })
    }

    async fn update_manual_node(
        &self,
        mutation: &ProxyNodeManualUpdateMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let existing = self.find_proxy_node(&mutation.node_id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };
        if !existing.is_manual {
            return Err(DataLayerError::InvalidInput(
                "只能编辑手动添加的代理节点".to_string(),
            ));
        }

        let next_ip = mutation.ip.as_deref().unwrap_or(existing.ip.as_str());
        let next_port = mutation.port.unwrap_or(existing.port);
        let lock_key = Self::registration_lock_key(next_ip, next_port);
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(lock_key)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;

        if let Some((name, ip, port)) = Self::find_duplicate_proxy_node_locked(
            &mut tx,
            next_ip,
            next_port,
            Some(&mutation.node_id),
        )
        .await?
        {
            return Err(DataLayerError::InvalidInput(
                Self::duplicate_proxy_node_detail(&name, &ip, port),
            ));
        }

        sqlx::query(UPDATE_MANUAL_PROXY_NODE_SQL)
            .bind(&mutation.node_id)
            .bind(mutation.name.as_deref())
            .bind(mutation.ip.as_deref())
            .bind(mutation.port)
            .bind(mutation.region.as_deref())
            .bind(mutation.proxy_url.as_deref())
            .bind(mutation.proxy_username.as_deref())
            .bind(mutation.proxy_password.as_deref())
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;

        tx.commit().await.map_err(postgres_error)?;
        self.find_proxy_node(&mutation.node_id).await
    }

    async fn register_node(
        &self,
        mutation: &ProxyNodeRegistrationMutation,
    ) -> Result<StoredProxyNode, DataLayerError> {
        let normalized_proxy_metadata = normalize_proxy_metadata(
            mutation.proxy_metadata.as_ref(),
            mutation.proxy_version.as_deref(),
        );
        let lock_key = Self::registration_lock_key(&mutation.ip, mutation.port);
        let mut tx = self.pool.begin().await.map_postgres_err()?;

        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(lock_key)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;

        let existing = sqlx::query(FIND_EXISTING_TUNNEL_NODE_SQL)
            .bind(&mutation.ip)
            .bind(mutation.port)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;

        let node_id = if let Some(row) = existing.as_ref() {
            let existing = Self::row_to_stored(row)?;
            sqlx::query(UPDATE_PROXY_NODE_REGISTRATION_SQL)
                .bind(&existing.id)
                .bind(&mutation.name)
                .bind(&mutation.ip)
                .bind(mutation.port)
                .bind(mutation.region.as_deref())
                .bind(mutation.registered_by.as_deref())
                .bind(mutation.heartbeat_interval)
                .bind(mutation.active_connections)
                .bind(mutation.total_requests)
                .bind(mutation.avg_latency_ms)
                .bind(mutation.hardware_info.as_ref())
                .bind(mutation.estimated_max_concurrency)
                .bind(mutation.tunnel_mode)
                .bind(normalized_proxy_metadata.as_ref())
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
            existing.id
        } else {
            let node_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(INSERT_PROXY_NODE_SQL)
                .bind(&node_id)
                .bind(&mutation.name)
                .bind(&mutation.ip)
                .bind(mutation.port)
                .bind(mutation.region.as_deref())
                .bind(mutation.registered_by.as_deref())
                .bind(mutation.heartbeat_interval)
                .bind(mutation.active_connections)
                .bind(mutation.total_requests)
                .bind(mutation.avg_latency_ms)
                .bind(mutation.hardware_info.as_ref())
                .bind(mutation.estimated_max_concurrency)
                .bind(mutation.tunnel_mode)
                .bind(normalized_proxy_metadata.as_ref())
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
            node_id
        };

        tx.commit().await.map_err(postgres_error)?;
        self.find_proxy_node(&node_id).await?.ok_or_else(|| {
            DataLayerError::UnexpectedValue("registered proxy node missing".to_string())
        })
    }

    async fn apply_heartbeat(
        &self,
        mutation: &ProxyNodeHeartbeatMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let existing = self.find_proxy_node(&mutation.node_id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };
        if !existing.tunnel_mode {
            return Err(DataLayerError::InvalidInput(
                "non-tunnel mode is no longer supported, please upgrade aether-tunnel to use tunnel mode"
                    .to_string(),
            ));
        }

        let normalized_proxy_metadata = normalize_proxy_metadata(
            mutation.proxy_metadata.as_ref(),
            mutation.proxy_version.as_deref(),
        );
        let normalized_proxy_metadata = preserve_proxy_metadata_tunnel_security(
            existing.proxy_metadata.as_ref(),
            normalized_proxy_metadata,
        );

        sqlx::query(APPLY_HEARTBEAT_SQL)
            .bind(&mutation.node_id)
            .bind(mutation.heartbeat_interval)
            .bind(mutation.active_connections)
            .bind(mutation.avg_latency_ms)
            .bind(normalized_proxy_metadata)
            .bind(mutation.total_requests_delta)
            .bind(mutation.failed_requests_delta)
            .bind(mutation.dns_failures_delta)
            .bind(mutation.stream_errors_delta)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;

        let updated = self.find_proxy_node(&mutation.node_id).await?;
        let Some(updated) = updated else {
            return Ok(None);
        };
        let now_unix_secs = updated
            .last_heartbeat_at_unix_secs
            .unwrap_or_else(|| chrono::Utc::now().timestamp().max(0) as u64);
        let tunnel_metrics_sample = build_tunnel_metrics_sample(
            existing.proxy_metadata.as_ref(),
            updated.proxy_metadata.as_ref(),
            updated.active_connections,
            updated.tunnel_connected,
        );

        if let Some(sample) = tunnel_metrics_sample.as_ref() {
            self.upsert_metrics_bucket(
                ProxyNodeMetricsStep::OneMinute,
                &updated.id,
                bucket_start_unix_secs(now_unix_secs, ProxyNodeMetricsStep::OneMinute),
                sample,
            )
            .await?;
            self.upsert_metrics_bucket(
                ProxyNodeMetricsStep::OneHour,
                &updated.id,
                bucket_start_unix_secs(now_unix_secs, ProxyNodeMetricsStep::OneHour),
                sample,
            )
            .await?;

            for error in &sample.recent_error_events {
                log_reported_tunnel_error_event(&updated.id, error, now_unix_secs);
                let detail = build_tunnel_error_event_detail(error);
                let event_metadata = serde_json::json!({
                    "source": "heartbeat",
                    "category": error.category,
                    "message": error.message,
                    "severity": error.severity.as_deref(),
                    "component": error.component.as_deref(),
                    "summary": error.summary.as_deref(),
                    "operator_action": error.operator_action.as_deref(),
                    "timestamp_unix_secs": error.timestamp_unix_secs,
                    "timestamp_unix_ms": error.timestamp_unix_ms,
                });
                self.insert_event(
                    &updated.id,
                    PROXY_NODE_EVENT_TYPE_TUNNEL_ERROR,
                    Some(detail.as_str()),
                    Some(&event_metadata),
                    Some(if error.timestamp_unix_secs == 0 {
                        now_unix_secs
                    } else {
                        error.timestamp_unix_secs
                    }),
                )
                .await?;
            }
        }

        if reconcile_remote_config_after_heartbeat(
            updated.remote_config.as_ref(),
            mutation.proxy_version.as_deref(),
        ) != updated.remote_config
        {
            return self
                .update_remote_config(&ProxyNodeRemoteConfigMutation {
                    node_id: mutation.node_id.clone(),
                    node_name: None,
                    allowed_ports: None,
                    log_level: None,
                    heartbeat_interval: None,
                    scheduling_state: None,
                    upgrade_to: Some(None),
                })
                .await;
        }

        Ok(Some(updated))
    }

    async fn record_traffic(
        &self,
        mutation: &ProxyNodeTrafficMutation,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(RECORD_PROXY_NODE_TRAFFIC_SQL)
            .bind(&mutation.node_id)
            .bind(mutation.total_requests_delta)
            .bind(mutation.failed_requests_delta)
            .bind(mutation.dns_failures_delta)
            .bind(mutation.stream_errors_delta)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;

        Ok(result.rows_affected() > 0)
    }

    async fn update_tunnel_status(
        &self,
        mutation: &ProxyNodeTunnelStatusMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let existing = self.find_proxy_node(&mutation.node_id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        let observed_at_unix_secs = mutation.observed_at_unix_secs;
        let event_type = if mutation.connected {
            "connected"
        } else {
            "disconnected"
        };
        let event_detail = mutation.detail.clone().unwrap_or_else(|| {
            format!(
                "[tunnel_node_status] conn_count={}",
                i32::max(mutation.conn_count, 0)
            )
        });

        let mut tx = self.pool.begin().await.map_postgres_err()?;

        if existing
            .tunnel_connected_at_unix_secs
            .zip(observed_at_unix_secs)
            .is_some_and(|(last_transition, observed_at)| observed_at < last_transition)
        {
            sqlx::query(INSERT_PROXY_NODE_EVENT_SQL)
                .bind(&mutation.node_id)
                .bind(event_type)
                .bind(format!("[stale_ignored] {event_detail}"))
                .bind(None::<serde_json::Value>)
                .bind(None::<f64>)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
            tx.commit().await.map_err(postgres_error)?;
            return self.find_proxy_node(&mutation.node_id).await;
        }

        sqlx::query(
            r#"
UPDATE proxy_nodes
SET
  tunnel_connected = $2,
  active_connections = CASE
    WHEN $2 THEN active_connections
    ELSE 0
  END,
  tunnel_connected_at = CASE
    WHEN $3::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($3::double precision)
  END,
  status = CASE
    WHEN $2 THEN 'online'::proxynodestatus
    ELSE 'offline'::proxynodestatus
  END,
  updated_at = CASE
    WHEN $3::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($3::double precision)
  END
WHERE id = $1
"#,
        )
        .bind(&mutation.node_id)
        .bind(mutation.connected)
        .bind(observed_at_unix_secs.map(|value| value as f64))
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;

        sqlx::query(INSERT_PROXY_NODE_EVENT_SQL)
            .bind(&mutation.node_id)
            .bind(event_type)
            .bind(event_detail)
            .bind(None::<serde_json::Value>)
            .bind(observed_at_unix_secs.map(|value| value as f64))
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;

        tx.commit().await.map_err(postgres_error)?;
        self.find_proxy_node(&mutation.node_id).await
    }

    async fn unregister_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let existing = self.find_proxy_node(node_id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        sqlx::query(UNREGISTER_PROXY_NODE_SQL)
            .bind(node_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;

        self.find_proxy_node(&existing.id).await
    }

    async fn delete_node(&self, node_id: &str) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let existing = self.find_proxy_node(node_id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };

        sqlx::query(DELETE_PROXY_NODE_SQL)
            .bind(node_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;

        Ok(Some(existing))
    }

    async fn update_remote_config(
        &self,
        mutation: &ProxyNodeRemoteConfigMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let existing = self.find_proxy_node(&mutation.node_id).await?;
        let Some(existing) = existing else {
            return Ok(None);
        };
        if existing.is_manual {
            return Err(DataLayerError::InvalidInput(
                "手动节点不支持远程配置下发".to_string(),
            ));
        }

        let remote_config =
            Self::normalize_remote_config(mutation, existing.remote_config.as_ref());
        sqlx::query(UPDATE_PROXY_NODE_REMOTE_CONFIG_SQL)
            .bind(&mutation.node_id)
            .bind(mutation.node_name.as_deref())
            .bind(remote_config.as_ref())
            .execute(&self.pool)
            .await
            .map_postgres_err()?;

        self.find_proxy_node(&mutation.node_id).await
    }

    async fn increment_manual_node_requests(
        &self,
        node_id: &str,
        total_delta: i64,
        failed_delta: i64,
        latency_ms: Option<i64>,
    ) -> Result<(), DataLayerError> {
        sqlx::query(INCREMENT_MANUAL_PROXY_NODE_REQUESTS_SQL)
            .bind(total_delta)
            .bind(failed_delta)
            .bind(latency_ms)
            .bind(node_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(())
    }

    async fn cleanup_proxy_node_metrics(
        &self,
        retain_1m_from_unix_secs: u64,
        retain_1h_from_unix_secs: u64,
        delete_limit: usize,
    ) -> Result<ProxyNodeMetricsCleanupSummary, DataLayerError> {
        let delete_limit_i64 = i64::try_from(delete_limit.max(1)).unwrap_or(i64::MAX);
        let deleted_1m = sqlx::query(
            r#"
WITH expired AS (
  SELECT node_id, bucket_start_unix_secs
  FROM proxy_node_metrics_1m
  WHERE bucket_start_unix_secs < $1
  ORDER BY bucket_start_unix_secs ASC
  LIMIT $2
)
DELETE FROM proxy_node_metrics_1m metrics
USING expired
WHERE metrics.node_id = expired.node_id
  AND metrics.bucket_start_unix_secs = expired.bucket_start_unix_secs
"#,
        )
        .bind(i64::try_from(retain_1m_from_unix_secs).unwrap_or(i64::MAX))
        .bind(delete_limit_i64)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected() as usize;

        let deleted_1h = sqlx::query(
            r#"
WITH expired AS (
  SELECT node_id, bucket_start_unix_secs
  FROM proxy_node_metrics_1h
  WHERE bucket_start_unix_secs < $1
  ORDER BY bucket_start_unix_secs ASC
  LIMIT $2
)
DELETE FROM proxy_node_metrics_1h metrics
USING expired
WHERE metrics.node_id = expired.node_id
  AND metrics.bucket_start_unix_secs = expired.bucket_start_unix_secs
"#,
        )
        .bind(i64::try_from(retain_1h_from_unix_secs).unwrap_or(i64::MAX))
        .bind(delete_limit_i64)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected() as usize;

        Ok(ProxyNodeMetricsCleanupSummary {
            deleted_1m_rows: deleted_1m,
            deleted_1h_rows: deleted_1h,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn proxy_node_sql_uses_json_casts_for_json_columns() {
        assert!(super::APPLY_HEARTBEAT_SQL
            .contains("proxy_metadata = COALESCE($5::json, proxy_metadata)"));
        assert!(super::INSERT_PROXY_NODE_SQL
            .contains("\n  $11::json,\n  $12,\n  $13,\n  FALSE,\n  $14::json\n"));
        assert!(super::UPDATE_PROXY_NODE_REGISTRATION_SQL
            .contains("hardware_info = COALESCE($11::json, hardware_info)"));
        assert!(super::UPDATE_PROXY_NODE_REGISTRATION_SQL
            .contains("proxy_metadata = COALESCE($14::json, proxy_metadata)"));
        assert!(super::UPDATE_PROXY_NODE_REMOTE_CONFIG_SQL.contains("remote_config = $3::json"));
    }

    #[test]
    fn proxy_node_sql_does_not_use_jsonb_casts() {
        assert!(!super::APPLY_HEARTBEAT_SQL.contains("::jsonb"));
        assert!(!super::INSERT_PROXY_NODE_SQL.contains("::jsonb"));
        assert!(!super::UPDATE_PROXY_NODE_REGISTRATION_SQL.contains("::jsonb"));
        assert!(!super::UPDATE_PROXY_NODE_REMOTE_CONFIG_SQL.contains("::jsonb"));
    }
}
