use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

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

use crate::error::SqlResultExt;
use crate::SqlitePool;

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

#[derive(Debug, Clone)]
pub struct SqliteProxyNodeReadRepository {
    pool: SqlitePool,
}

impl SqliteProxyNodeReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn upsert_node(&self, node: &StoredProxyNode) -> Result<(), DataLayerError> {
        let now = current_unix_secs();
        sqlx::query(
            r#"
INSERT INTO proxy_nodes (
  id, name, ip, port, region, status, registered_by, last_heartbeat_at,
  heartbeat_interval, active_connections, total_requests, avg_latency_ms,
  is_manual, proxy_url, proxy_username, proxy_password, created_at,
  updated_at, remote_config, config_version, hardware_info,
  estimated_max_concurrency, tunnel_mode, tunnel_connected, tunnel_connected_at,
  failed_requests, dns_failures, stream_errors, proxy_metadata
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET
  name = excluded.name,
  ip = excluded.ip,
  port = excluded.port,
  region = excluded.region,
  status = excluded.status,
  registered_by = excluded.registered_by,
  last_heartbeat_at = excluded.last_heartbeat_at,
  heartbeat_interval = excluded.heartbeat_interval,
  active_connections = excluded.active_connections,
  total_requests = excluded.total_requests,
  avg_latency_ms = excluded.avg_latency_ms,
  is_manual = excluded.is_manual,
  proxy_url = excluded.proxy_url,
  proxy_username = excluded.proxy_username,
  proxy_password = excluded.proxy_password,
  updated_at = excluded.updated_at,
  remote_config = excluded.remote_config,
  config_version = excluded.config_version,
  hardware_info = excluded.hardware_info,
  estimated_max_concurrency = excluded.estimated_max_concurrency,
  tunnel_mode = excluded.tunnel_mode,
  tunnel_connected = excluded.tunnel_connected,
  tunnel_connected_at = excluded.tunnel_connected_at,
  failed_requests = excluded.failed_requests,
  dns_failures = excluded.dns_failures,
  stream_errors = excluded.stream_errors,
  proxy_metadata = excluded.proxy_metadata
"#,
        )
        .bind(&node.id)
        .bind(&node.name)
        .bind(&node.ip)
        .bind(node.port)
        .bind(&node.region)
        .bind(&node.status)
        .bind(&node.registered_by)
        .bind(optional_i64_from_u64(
            node.last_heartbeat_at_unix_secs,
            "proxy_nodes.last_heartbeat_at",
        )?)
        .bind(node.heartbeat_interval)
        .bind(node.active_connections)
        .bind(node.total_requests)
        .bind(node.avg_latency_ms)
        .bind(node.is_manual)
        .bind(&node.proxy_url)
        .bind(&node.proxy_username)
        .bind(&node.proxy_password)
        .bind(node.created_at_unix_ms.unwrap_or(now) as i64)
        .bind(node.updated_at_unix_secs.unwrap_or(now) as i64)
        .bind(optional_json_to_string(
            &node.remote_config,
            "proxy_nodes.remote_config",
        )?)
        .bind(node.config_version)
        .bind(optional_json_to_string(
            &node.hardware_info,
            "proxy_nodes.hardware_info",
        )?)
        .bind(node.estimated_max_concurrency)
        .bind(node.tunnel_mode)
        .bind(node.tunnel_connected)
        .bind(optional_i64_from_u64(
            node.tunnel_connected_at_unix_secs,
            "proxy_nodes.tunnel_connected_at",
        )?)
        .bind(node.failed_requests)
        .bind(node.dns_failures)
        .bind(node.stream_errors)
        .bind(optional_json_to_string(
            &node.proxy_metadata,
            "proxy_nodes.proxy_metadata",
        )?)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(())
    }

    async fn find_duplicate_proxy_node(
        &self,
        ip: &str,
        port: i32,
        excluding_node_id: Option<&str>,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let row = if let Some(excluding_node_id) = excluding_node_id {
            sqlx::query(&format!(
                "{PROXY_NODE_COLUMNS} WHERE ip = ? AND port = ? AND id <> ? LIMIT 1"
            ))
            .bind(ip)
            .bind(port)
            .bind(excluding_node_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?
        } else {
            sqlx::query(&format!(
                "{PROXY_NODE_COLUMNS} WHERE ip = ? AND port = ? LIMIT 1"
            ))
            .bind(ip)
            .bind(port)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?
        };
        row.as_ref().map(map_proxy_node_row).transpose()
    }

    async fn insert_event(
        &self,
        node_id: &str,
        event_type: &str,
        detail: Option<&str>,
        event_metadata: Option<&serde_json::Value>,
        created_at_unix_secs: Option<u64>,
    ) -> Result<(), DataLayerError> {
        sqlx::query(
            r#"
INSERT INTO proxy_node_events (node_id, event_type, detail, event_metadata, created_at)
VALUES (?, ?, ?, ?, ?)
"#,
        )
        .bind(node_id)
        .bind(event_type)
        .bind(detail)
        .bind(optional_json_to_string(
            &event_metadata.cloned(),
            "proxy_node_events.event_metadata",
        )?)
        .bind(created_at_unix_secs.unwrap_or_else(current_unix_secs) as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(())
    }

    async fn upsert_metrics_bucket(
        &self,
        table: &str,
        node_id: &str,
        bucket_start: u64,
        sample: &TunnelMetricsSample,
    ) -> Result<(), DataLayerError> {
        sqlx::query(&format!(
            r#"
INSERT INTO {table} (
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
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(node_id, bucket_start_unix_secs) DO UPDATE SET
  samples = {table}.samples + excluded.samples,
  uptime_samples = {table}.uptime_samples + excluded.uptime_samples,
  active_connections_sum = {table}.active_connections_sum + excluded.active_connections_sum,
  active_connections_max = MAX({table}.active_connections_max, excluded.active_connections_max),
  heartbeat_rtt_ms_sum = {table}.heartbeat_rtt_ms_sum + excluded.heartbeat_rtt_ms_sum,
  heartbeat_rtt_ms_max = MAX({table}.heartbeat_rtt_ms_max, excluded.heartbeat_rtt_ms_max),
  connect_errors_delta = {table}.connect_errors_delta + excluded.connect_errors_delta,
  disconnects_delta = {table}.disconnects_delta + excluded.disconnects_delta,
  error_events_delta = {table}.error_events_delta + excluded.error_events_delta,
  ws_in_bytes_delta = {table}.ws_in_bytes_delta + excluded.ws_in_bytes_delta,
  ws_out_bytes_delta = {table}.ws_out_bytes_delta + excluded.ws_out_bytes_delta,
  ws_in_frames_delta = {table}.ws_in_frames_delta + excluded.ws_in_frames_delta,
  ws_out_frames_delta = {table}.ws_out_frames_delta + excluded.ws_out_frames_delta
"#
        ))
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
        .map_sql_err()?;
        Ok(())
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
}

const PROXY_NODE_COLUMNS: &str = r#"
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
  status,
  registered_by,
  last_heartbeat_at AS last_heartbeat_at_unix_secs,
  heartbeat_interval,
  active_connections,
  total_requests,
  avg_latency_ms,
  failed_requests,
  dns_failures,
  stream_errors,
  proxy_metadata,
  hardware_info,
  estimated_max_concurrency,
  tunnel_mode,
  tunnel_connected,
  tunnel_connected_at AS tunnel_connected_at_unix_secs,
  remote_config,
  config_version,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
FROM proxy_nodes
"#;

const PROXY_NODE_EVENT_COLUMNS: &str = r#"
SELECT
  id,
  node_id,
  event_type,
  detail,
  event_metadata,
  created_at AS created_at_unix_ms
FROM proxy_node_events
"#;

#[async_trait]
impl ProxyNodeReadRepository for SqliteProxyNodeReadRepository {
    async fn list_proxy_nodes(&self) -> Result<Vec<StoredProxyNode>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(PROXY_NODE_COLUMNS);
        builder.push(" ORDER BY name ASC, id ASC");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_proxy_node_row).collect()
    }

    async fn find_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(PROXY_NODE_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(&mut builder, &mut where_clause, "id", node_id.to_string());
        push_limit(&mut builder, 1);
        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_proxy_node_row).transpose()
    }

    async fn list_proxy_node_events(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeEvent>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(PROXY_NODE_EVENT_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "node_id",
            node_id.to_string(),
        );
        builder.push(" ORDER BY created_at DESC, id DESC");
        push_limit(&mut builder, i64::try_from(limit).unwrap_or(i64::MAX));
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_proxy_node_event_row).collect()
    }

    async fn list_proxy_node_events_filtered(
        &self,
        node_id: &str,
        query: &ProxyNodeEventQuery,
    ) -> Result<Vec<StoredProxyNodeEvent>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(PROXY_NODE_EVENT_COLUMNS);
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
                .push("created_at >= ")
                .push_bind(i64::try_from(from_unix_secs).unwrap_or(i64::MAX));
        }
        if let Some(to_unix_secs) = query.to_unix_secs {
            where_clause.push_next(&mut builder);
            builder
                .push("created_at <= ")
                .push_bind(i64::try_from(to_unix_secs).unwrap_or(i64::MAX));
        }
        if let Some(event_type) = query.event_type.as_deref() {
            where_clause.push_next(&mut builder);
            builder
                .push("LOWER(event_type) = LOWER(")
                .push_bind(event_type.to_string())
                .push(")");
        }
        builder.push(" ORDER BY created_at DESC, id DESC");
        push_limit(&mut builder, i64::try_from(query.limit).unwrap_or(i64::MAX));
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_proxy_node_event_row).collect()
    }

    async fn list_proxy_node_metrics(
        &self,
        node_id: &str,
        step: ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeMetricsBucket>, DataLayerError> {
        let table = match step {
            ProxyNodeMetricsStep::OneMinute => "proxy_node_metrics_1m",
            ProxyNodeMetricsStep::OneHour => "proxy_node_metrics_1h",
        };
        let rows = sqlx::query(&format!(
            r#"
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
FROM {table}
WHERE node_id = ?
  AND bucket_start_unix_secs >= ?
  AND bucket_start_unix_secs <= ?
ORDER BY bucket_start_unix_secs ASC
LIMIT ?
"#
        ))
        .bind(node_id)
        .bind(i64::try_from(from_unix_secs).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_unix_secs).unwrap_or(i64::MAX))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_proxy_node_metric_row).collect()
    }

    async fn list_proxy_fleet_metrics(
        &self,
        step: ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyFleetMetricsBucket>, DataLayerError> {
        let table = match step {
            ProxyNodeMetricsStep::OneMinute => "proxy_node_metrics_1m",
            ProxyNodeMetricsStep::OneHour => "proxy_node_metrics_1h",
        };
        let rows = sqlx::query(&format!(
            r#"
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
FROM {table}
WHERE bucket_start_unix_secs >= ?
  AND bucket_start_unix_secs <= ?
GROUP BY bucket_start_unix_secs
ORDER BY bucket_start_unix_secs ASC
LIMIT ?
"#
        ))
        .bind(i64::try_from(from_unix_secs).unwrap_or(i64::MAX))
        .bind(i64::try_from(to_unix_secs).unwrap_or(i64::MAX))
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_proxy_fleet_metric_row).collect()
    }
}

#[async_trait]
impl ProxyNodeWriteRepository for SqliteProxyNodeReadRepository {
    async fn reset_stale_tunnel_statuses(&self) -> Result<usize, DataLayerError> {
        let now = current_unix_secs() as i64;
        let result = sqlx::query(
            r#"
UPDATE proxy_nodes
SET tunnel_connected = 0,
    status = 'offline',
    active_connections = 0,
    tunnel_connected_at = ?,
    updated_at = ?
WHERE is_manual = 0
  AND tunnel_connected = 1
"#,
        )
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() as usize)
    }

    async fn create_manual_node(
        &self,
        mutation: &ProxyNodeManualCreateMutation,
    ) -> Result<StoredProxyNode, DataLayerError> {
        if let Some(existing) = self
            .find_duplicate_proxy_node(&mutation.ip, mutation.port, None)
            .await?
        {
            return Err(duplicate_proxy_node_error(&existing));
        }

        let now = Some(current_unix_secs());
        let node = StoredProxyNode::new(
            uuid::Uuid::new_v4().to_string(),
            mutation.name.clone(),
            mutation.ip.clone(),
            mutation.port,
            true,
            "online".to_string(),
            0,
            0,
            0,
            0,
            0,
            0,
            false,
            false,
            0,
        )?
        .with_manual_proxy_fields(
            Some(mutation.proxy_url.clone()),
            mutation.proxy_username.clone(),
            mutation.proxy_password.clone(),
        )
        .with_runtime_fields(
            mutation.region.clone(),
            mutation.registered_by.clone(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            now,
            now,
        );

        self.upsert_node(&node).await?;
        Ok(node)
    }

    async fn update_manual_node(
        &self,
        mutation: &ProxyNodeManualUpdateMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let Some(mut node) = self.find_proxy_node(&mutation.node_id).await? else {
            return Ok(None);
        };
        if !node.is_manual {
            return Err(DataLayerError::InvalidInput(
                "只能编辑手动添加的代理节点".to_string(),
            ));
        }

        let next_ip = mutation.ip.as_deref().unwrap_or(node.ip.as_str());
        let next_port = mutation.port.unwrap_or(node.port);
        if let Some(existing) = self
            .find_duplicate_proxy_node(next_ip, next_port, Some(&mutation.node_id))
            .await?
        {
            return Err(duplicate_proxy_node_error(&existing));
        }

        if let Some(name) = mutation.name.as_ref() {
            node.name = name.clone();
        }
        if let Some(ip) = mutation.ip.as_ref() {
            node.ip = ip.clone();
        }
        if let Some(port) = mutation.port {
            node.port = port;
        }
        if let Some(region) = mutation.region.as_ref() {
            node.region = Some(region.clone());
        }
        if let Some(proxy_url) = mutation.proxy_url.as_ref() {
            node.proxy_url = Some(proxy_url.clone());
        }
        if let Some(proxy_username) = mutation.proxy_username.as_ref() {
            node.proxy_username = Some(proxy_username.clone());
        }
        if let Some(proxy_password) = mutation.proxy_password.as_ref() {
            node.proxy_password = Some(proxy_password.clone());
        }
        node.updated_at_unix_secs = Some(current_unix_secs());
        self.upsert_node(&node).await?;
        Ok(Some(node))
    }

    async fn register_node(
        &self,
        mutation: &ProxyNodeRegistrationMutation,
    ) -> Result<StoredProxyNode, DataLayerError> {
        let now = Some(current_unix_secs());
        let normalized_proxy_metadata = normalize_proxy_metadata(
            mutation.proxy_metadata.as_ref(),
            mutation.proxy_version.as_deref(),
        );

        let existing = sqlx::query(&format!(
            "{PROXY_NODE_COLUMNS} WHERE ip = ? AND port = ? AND is_manual = 0 ORDER BY created_at ASC, id ASC LIMIT 1"
        ))
        .bind(&mutation.ip)
        .bind(mutation.port)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;

        let mut node = if let Some(row) = existing.as_ref() {
            map_proxy_node_row(row)?
        } else {
            StoredProxyNode::new(
                uuid::Uuid::new_v4().to_string(),
                mutation.name.clone(),
                mutation.ip.clone(),
                mutation.port,
                false,
                "offline".to_string(),
                mutation.heartbeat_interval,
                mutation.active_connections.unwrap_or(0),
                mutation.total_requests.unwrap_or(0),
                0,
                0,
                0,
                mutation.tunnel_mode,
                false,
                0,
            )?
            .with_runtime_fields(
                mutation.region.clone(),
                mutation.registered_by.clone(),
                now,
                mutation.avg_latency_ms,
                normalized_proxy_metadata.clone(),
                mutation.hardware_info.clone(),
                mutation.estimated_max_concurrency,
                None,
                None,
                now,
                now,
            )
        };

        node.name = mutation.name.clone();
        node.ip = mutation.ip.clone();
        node.port = mutation.port;
        node.region = mutation.region.clone();
        node.registered_by = mutation.registered_by.clone();
        node.last_heartbeat_at_unix_secs = now;
        node.heartbeat_interval = mutation.heartbeat_interval;
        node.tunnel_mode = mutation.tunnel_mode;
        if let Some(active_connections) = mutation.active_connections {
            node.active_connections = active_connections;
        }
        if let Some(total_requests) = mutation.total_requests {
            node.total_requests = total_requests;
        }
        if let Some(avg_latency_ms) = mutation.avg_latency_ms {
            node.avg_latency_ms = Some(avg_latency_ms);
        }
        if let Some(hardware_info) = mutation.hardware_info.as_ref() {
            node.hardware_info = Some(hardware_info.clone());
        }
        if let Some(estimated_max_concurrency) = mutation.estimated_max_concurrency {
            node.estimated_max_concurrency = Some(estimated_max_concurrency);
        }
        if let Some(proxy_metadata) = normalized_proxy_metadata {
            node.proxy_metadata = Some(proxy_metadata);
        }
        if node.created_at_unix_ms.is_none() {
            node.created_at_unix_ms = now;
        }
        node.updated_at_unix_secs = now;
        self.upsert_node(&node).await?;
        Ok(node)
    }

    async fn apply_heartbeat(
        &self,
        mutation: &ProxyNodeHeartbeatMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let Some(mut node) = self.find_proxy_node(&mutation.node_id).await? else {
            return Ok(None);
        };
        if !node.tunnel_mode {
            return Err(DataLayerError::InvalidInput(
                "non-tunnel mode is no longer supported, please upgrade aether-tunnel to use tunnel mode"
                    .to_string(),
            ));
        }

        let previous_proxy_metadata = node.proxy_metadata.clone();
        let now_unix_secs = current_unix_secs();
        let now = Some(now_unix_secs);
        node.last_heartbeat_at_unix_secs = now;
        if node.status != "online" || !node.tunnel_connected {
            node.status = "online".to_string();
            node.tunnel_connected = true;
            node.tunnel_connected_at_unix_secs = now;
            node.updated_at_unix_secs = now;
        }
        if let Some(value) = mutation.heartbeat_interval {
            node.heartbeat_interval = value;
        }
        if let Some(value) = mutation.active_connections {
            node.active_connections = value;
        }
        if let Some(value) = mutation.avg_latency_ms {
            node.avg_latency_ms = Some(value);
        }
        let normalized_proxy_metadata = normalize_proxy_metadata(
            mutation.proxy_metadata.as_ref(),
            mutation.proxy_version.as_deref(),
        );
        let normalized_proxy_metadata = preserve_proxy_metadata_tunnel_security(
            previous_proxy_metadata.as_ref(),
            normalized_proxy_metadata,
        );
        if let Some(value) = normalized_proxy_metadata {
            node.proxy_metadata = Some(value);
        }
        if let Some(value) = mutation.total_requests_delta.filter(|value| *value > 0) {
            node.total_requests += value;
        }
        if let Some(value) = mutation.failed_requests_delta.filter(|value| *value > 0) {
            node.failed_requests += value;
        }
        if let Some(value) = mutation.dns_failures_delta.filter(|value| *value > 0) {
            node.dns_failures += value;
        }
        if let Some(value) = mutation.stream_errors_delta.filter(|value| *value > 0) {
            node.stream_errors += value;
        }
        let reconciled_remote_config = reconcile_remote_config_after_heartbeat(
            node.remote_config.as_ref(),
            mutation.proxy_version.as_deref(),
        );
        if reconciled_remote_config != node.remote_config {
            node.remote_config = reconciled_remote_config;
            node.config_version = node.config_version.saturating_add(1);
            node.updated_at_unix_secs = now;
        }

        let tunnel_metrics_sample = build_tunnel_metrics_sample(
            previous_proxy_metadata.as_ref(),
            node.proxy_metadata.as_ref(),
            node.active_connections,
            node.tunnel_connected,
        );

        self.upsert_node(&node).await?;

        if let Some(sample) = tunnel_metrics_sample.as_ref() {
            self.upsert_metrics_bucket(
                "proxy_node_metrics_1m",
                &node.id,
                bucket_start_unix_secs(now_unix_secs, ProxyNodeMetricsStep::OneMinute),
                sample,
            )
            .await?;
            self.upsert_metrics_bucket(
                "proxy_node_metrics_1h",
                &node.id,
                bucket_start_unix_secs(now_unix_secs, ProxyNodeMetricsStep::OneHour),
                sample,
            )
            .await?;

            for error in &sample.recent_error_events {
                log_reported_tunnel_error_event(&node.id, error, now_unix_secs);
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
                    &node.id,
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

        Ok(Some(node))
    }

    async fn record_traffic(
        &self,
        mutation: &ProxyNodeTrafficMutation,
    ) -> Result<bool, DataLayerError> {
        let Some(mut node) = self.find_proxy_node(&mutation.node_id).await? else {
            return Ok(false);
        };
        if !node.is_manual {
            return Ok(false);
        }
        node.total_requests += mutation.total_requests_delta.max(0);
        node.failed_requests += mutation.failed_requests_delta.max(0);
        node.dns_failures += mutation.dns_failures_delta.max(0);
        node.stream_errors += mutation.stream_errors_delta.max(0);
        node.updated_at_unix_secs = Some(current_unix_secs());
        self.upsert_node(&node).await?;
        Ok(true)
    }

    async fn update_tunnel_status(
        &self,
        mutation: &ProxyNodeTunnelStatusMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let Some(mut node) = self.find_proxy_node(&mutation.node_id).await? else {
            return Ok(None);
        };

        let event_time = mutation
            .observed_at_unix_secs
            .unwrap_or_else(current_unix_secs);
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

        if node
            .tunnel_connected_at_unix_secs
            .is_some_and(|last_transition| event_time < last_transition)
        {
            self.insert_event(
                &mutation.node_id,
                event_type,
                Some(&format!("[stale_ignored] {event_detail}")),
                None,
                Some(current_unix_secs()),
            )
            .await?;
            return Ok(Some(node));
        }

        node.tunnel_connected = mutation.connected;
        node.tunnel_connected_at_unix_secs = Some(event_time);
        node.status = if mutation.connected {
            "online".to_string()
        } else {
            "offline".to_string()
        };
        if !mutation.connected {
            node.active_connections = 0;
        }
        node.updated_at_unix_secs = Some(event_time);
        self.upsert_node(&node).await?;
        self.insert_event(
            &mutation.node_id,
            event_type,
            Some(&event_detail),
            None,
            Some(event_time),
        )
        .await?;
        Ok(Some(node))
    }

    async fn unregister_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let Some(mut node) = self.find_proxy_node(node_id).await? else {
            return Ok(None);
        };
        let now = Some(current_unix_secs());
        node.status = "offline".to_string();
        node.tunnel_connected = false;
        node.active_connections = 0;
        node.tunnel_connected_at_unix_secs = now;
        node.updated_at_unix_secs = now;
        self.upsert_node(&node).await?;
        Ok(Some(node))
    }

    async fn delete_node(&self, node_id: &str) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let existing = self.find_proxy_node(node_id).await?;
        if existing.is_some() {
            sqlx::query("DELETE FROM proxy_node_events WHERE node_id = ?")
                .bind(node_id)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
            sqlx::query("DELETE FROM proxy_node_metrics_1m WHERE node_id = ?")
                .bind(node_id)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
            sqlx::query("DELETE FROM proxy_node_metrics_1h WHERE node_id = ?")
                .bind(node_id)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
            sqlx::query("DELETE FROM proxy_nodes WHERE id = ?")
                .bind(node_id)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
        }
        Ok(existing)
    }

    async fn update_remote_config(
        &self,
        mutation: &ProxyNodeRemoteConfigMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let Some(mut node) = self.find_proxy_node(&mutation.node_id).await? else {
            return Ok(None);
        };
        if node.is_manual {
            return Err(DataLayerError::InvalidInput(
                "手动节点不支持远程配置下发".to_string(),
            ));
        }
        if let Some(node_name) = mutation.node_name.as_ref() {
            node.name = node_name.clone();
        }
        node.remote_config = Self::normalize_remote_config(mutation, node.remote_config.as_ref());
        node.config_version = node.config_version.saturating_add(1);
        node.updated_at_unix_secs = Some(current_unix_secs());
        self.upsert_node(&node).await?;
        Ok(Some(node))
    }

    async fn increment_manual_node_requests(
        &self,
        node_id: &str,
        total_delta: i64,
        failed_delta: i64,
        latency_ms: Option<i64>,
    ) -> Result<(), DataLayerError> {
        let Some(mut node) = self.find_proxy_node(node_id).await? else {
            return Ok(());
        };
        if !node.is_manual {
            return Ok(());
        }
        if total_delta > 0 {
            node.total_requests += total_delta;
        }
        if failed_delta > 0 {
            node.failed_requests += failed_delta;
        }
        if let Some(ms) = latency_ms {
            node.avg_latency_ms = Some(ms as f64);
        }
        node.updated_at_unix_secs = Some(current_unix_secs());
        self.upsert_node(&node).await
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
DELETE FROM proxy_node_metrics_1m
WHERE (node_id, bucket_start_unix_secs) IN (
  SELECT node_id, bucket_start_unix_secs
  FROM proxy_node_metrics_1m
  WHERE bucket_start_unix_secs < ?
  ORDER BY bucket_start_unix_secs ASC
  LIMIT ?
)
"#,
        )
        .bind(i64::try_from(retain_1m_from_unix_secs).unwrap_or(i64::MAX))
        .bind(delete_limit_i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected() as usize;

        let deleted_1h = sqlx::query(
            r#"
DELETE FROM proxy_node_metrics_1h
WHERE (node_id, bucket_start_unix_secs) IN (
  SELECT node_id, bucket_start_unix_secs
  FROM proxy_node_metrics_1h
  WHERE bucket_start_unix_secs < ?
  ORDER BY bucket_start_unix_secs ASC
  LIMIT ?
)
"#,
        )
        .bind(i64::try_from(retain_1h_from_unix_secs).unwrap_or(i64::MAX))
        .bind(delete_limit_i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected() as usize;

        Ok(ProxyNodeMetricsCleanupSummary {
            deleted_1m_rows: deleted_1m,
            deleted_1h_rows: deleted_1h,
        })
    }
}

fn optional_unix_secs(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn optional_i64_from_u64(
    value: Option<u64>,
    field_name: &str,
) -> Result<Option<i64>, DataLayerError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                DataLayerError::InvalidInput(format!("{field_name} exceeds i64: {value}"))
            })
        })
        .transpose()
}

fn optional_json_to_string(
    value: &Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    value
        .as_ref()
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains unserializable JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn duplicate_proxy_node_error(node: &StoredProxyNode) -> DataLayerError {
    DataLayerError::InvalidInput(format!(
        "已存在相同地址的代理节点: {} ({}:{})",
        node.name, node.ip, node.port
    ))
}

fn optional_json_from_string(
    value: Option<String>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains invalid JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn map_proxy_node_row(row: &SqliteRow) -> Result<StoredProxyNode, DataLayerError> {
    Ok(StoredProxyNode::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        row.try_get("ip").map_sql_err()?,
        row.try_get("port").map_sql_err()?,
        row.try_get("is_manual").map_sql_err()?,
        row.try_get("status").map_sql_err()?,
        row.try_get("heartbeat_interval").map_sql_err()?,
        row.try_get("active_connections").map_sql_err()?,
        row.try_get("total_requests").map_sql_err()?,
        row.try_get("failed_requests").map_sql_err()?,
        row.try_get("dns_failures").map_sql_err()?,
        row.try_get("stream_errors").map_sql_err()?,
        row.try_get("tunnel_mode").map_sql_err()?,
        row.try_get("tunnel_connected").map_sql_err()?,
        row.try_get("config_version").map_sql_err()?,
    )?
    .with_manual_proxy_fields(
        row.try_get("proxy_url").map_sql_err()?,
        row.try_get("proxy_username").map_sql_err()?,
        row.try_get("proxy_password").map_sql_err()?,
    )
    .with_runtime_fields(
        row.try_get("region").map_sql_err()?,
        row.try_get("registered_by").map_sql_err()?,
        optional_unix_secs(row.try_get("last_heartbeat_at_unix_secs").map_sql_err()?),
        row.try_get("avg_latency_ms").map_sql_err()?,
        optional_json_from_string(
            row.try_get("proxy_metadata").map_sql_err()?,
            "proxy_nodes.proxy_metadata",
        )?,
        optional_json_from_string(
            row.try_get("hardware_info").map_sql_err()?,
            "proxy_nodes.hardware_info",
        )?,
        row.try_get("estimated_max_concurrency").map_sql_err()?,
        optional_unix_secs(row.try_get("tunnel_connected_at_unix_secs").map_sql_err()?),
        optional_json_from_string(
            row.try_get("remote_config").map_sql_err()?,
            "proxy_nodes.remote_config",
        )?,
        optional_unix_secs(row.try_get("created_at_unix_ms").map_sql_err()?),
        optional_unix_secs(row.try_get("updated_at_unix_secs").map_sql_err()?),
    ))
}

fn map_proxy_node_event_row(row: &SqliteRow) -> Result<StoredProxyNodeEvent, DataLayerError> {
    Ok(StoredProxyNodeEvent {
        id: row.try_get("id").map_sql_err()?,
        node_id: row.try_get("node_id").map_sql_err()?,
        event_type: row.try_get("event_type").map_sql_err()?,
        detail: row.try_get("detail").map_sql_err()?,
        event_metadata: optional_json_from_string(
            row.try_get("event_metadata").map_sql_err()?,
            "proxy_node_events.event_metadata",
        )?,
        created_at_unix_ms: optional_unix_secs(row.try_get("created_at_unix_ms").map_sql_err()?),
    })
}

fn map_proxy_node_metric_row(
    row: &SqliteRow,
) -> Result<StoredProxyNodeMetricsBucket, DataLayerError> {
    Ok(StoredProxyNodeMetricsBucket {
        node_id: row.try_get("node_id").map_sql_err()?,
        bucket_start_unix_secs: optional_unix_secs(
            row.try_get("bucket_start_unix_secs").map_sql_err()?,
        )
        .unwrap_or_default(),
        samples: row.try_get("samples").map_sql_err()?,
        uptime_samples: row.try_get("uptime_samples").map_sql_err()?,
        active_connections_sum: row.try_get("active_connections_sum").map_sql_err()?,
        active_connections_max: row.try_get("active_connections_max").map_sql_err()?,
        heartbeat_rtt_ms_sum: row.try_get("heartbeat_rtt_ms_sum").map_sql_err()?,
        heartbeat_rtt_ms_max: row.try_get("heartbeat_rtt_ms_max").map_sql_err()?,
        connect_errors_delta: row.try_get("connect_errors_delta").map_sql_err()?,
        disconnects_delta: row.try_get("disconnects_delta").map_sql_err()?,
        error_events_delta: row.try_get("error_events_delta").map_sql_err()?,
        ws_in_bytes_delta: row.try_get("ws_in_bytes_delta").map_sql_err()?,
        ws_out_bytes_delta: row.try_get("ws_out_bytes_delta").map_sql_err()?,
        ws_in_frames_delta: row.try_get("ws_in_frames_delta").map_sql_err()?,
        ws_out_frames_delta: row.try_get("ws_out_frames_delta").map_sql_err()?,
    })
}

fn map_proxy_fleet_metric_row(
    row: &SqliteRow,
) -> Result<StoredProxyFleetMetricsBucket, DataLayerError> {
    Ok(StoredProxyFleetMetricsBucket {
        bucket_start_unix_secs: optional_unix_secs(
            row.try_get("bucket_start_unix_secs").map_sql_err()?,
        )
        .unwrap_or_default(),
        samples: row.try_get("samples").map_sql_err()?,
        uptime_samples: row.try_get("uptime_samples").map_sql_err()?,
        active_connections_sum: row.try_get("active_connections_sum").map_sql_err()?,
        active_connections_max: row.try_get("active_connections_max").map_sql_err()?,
        heartbeat_rtt_ms_sum: row.try_get("heartbeat_rtt_ms_sum").map_sql_err()?,
        heartbeat_rtt_ms_max: row.try_get("heartbeat_rtt_ms_max").map_sql_err()?,
        connect_errors_delta: row.try_get("connect_errors_delta").map_sql_err()?,
        disconnects_delta: row.try_get("disconnects_delta").map_sql_err()?,
        error_events_delta: row.try_get("error_events_delta").map_sql_err()?,
        ws_in_bytes_delta: row.try_get("ws_in_bytes_delta").map_sql_err()?,
        ws_out_bytes_delta: row.try_get("ws_out_bytes_delta").map_sql_err()?,
        ws_in_frames_delta: row.try_get("ws_in_frames_delta").map_sql_err()?,
        ws_out_frames_delta: row.try_get("ws_out_frames_delta").map_sql_err()?,
    })
}

#[cfg(test)]
mod tests {
    use super::SqliteProxyNodeReadRepository;
    use crate::run_migrations;
    use aether_data_contracts::repository::proxy_nodes::{
        ProxyNodeEventQuery, ProxyNodeHeartbeatMutation, ProxyNodeManualCreateMutation,
        ProxyNodeManualUpdateMutation, ProxyNodeMetricsStep, ProxyNodeReadRepository,
        ProxyNodeRegistrationMutation, ProxyNodeRemoteConfigMutation, ProxyNodeTrafficMutation,
        ProxyNodeTunnelStatusMutation, ProxyNodeWriteRepository,
        PROXY_NODE_EVENT_TYPE_TUNNEL_ERROR,
    };
    use serde_json::json;

    #[tokio::test]
    async fn sqlite_repository_reads_proxy_nodes_and_events() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        sqlx::query(
            r#"
INSERT INTO proxy_nodes (
  id, name, ip, port, status, heartbeat_interval, active_connections,
  total_requests, failed_requests, dns_failures, stream_errors,
  tunnel_mode, tunnel_connected, config_version, proxy_metadata,
  hardware_info, remote_config, created_at, updated_at
) VALUES (
  'node-1', 'Node 1', '127.0.0.1', 8080, 'online', 30, 1,
  10, 2, 1, 0, 1, 1, 3, '{"version":"1.0.0"}',
  '{"cpu":"m1"}', '{"log_level":"debug"}', 1, 2
)
"#,
        )
        .execute(&pool)
        .await
        .expect("proxy node should seed");
        sqlx::query(
            r#"
INSERT INTO proxy_node_events (node_id, event_type, detail, created_at)
VALUES ('node-1', 'registered', 'ok', 3)
"#,
        )
        .execute(&pool)
        .await
        .expect("proxy node event should seed");

        let repository = SqliteProxyNodeReadRepository::new(pool);
        let nodes = repository
            .list_proxy_nodes()
            .await
            .expect("proxy nodes should list");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "node-1");
        assert_eq!(
            nodes[0].proxy_metadata,
            Some(serde_json::json!({"version": "1.0.0"}))
        );

        let node = repository
            .find_proxy_node("node-1")
            .await
            .expect("proxy node should load")
            .expect("proxy node should exist");
        assert_eq!(node.config_version, 3);

        let events = repository
            .list_proxy_node_events("node-1", 10)
            .await
            .expect("events should list");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "registered");
    }

    #[tokio::test]
    async fn sqlite_repository_writes_proxy_node_contract_views() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        let repository = SqliteProxyNodeReadRepository::new(pool);
        let manual = repository
            .create_manual_node(&ProxyNodeManualCreateMutation {
                name: "manual-1".to_string(),
                ip: "127.0.0.2".to_string(),
                port: 8081,
                region: Some("local".to_string()),
                proxy_url: "http://127.0.0.2:8081".to_string(),
                proxy_username: Some("user".to_string()),
                proxy_password: Some("pass".to_string()),
                registered_by: Some("admin".to_string()),
            })
            .await
            .expect("manual node should create");
        assert!(manual.is_manual);
        assert_eq!(manual.status, "online");

        let manual = repository
            .update_manual_node(&ProxyNodeManualUpdateMutation {
                node_id: manual.id.clone(),
                name: Some("manual-updated".to_string()),
                ip: None,
                port: None,
                region: Some("edge".to_string()),
                proxy_url: None,
                proxy_username: None,
                proxy_password: None,
            })
            .await
            .expect("manual node should update")
            .expect("manual node should exist");
        assert_eq!(manual.name, "manual-updated");
        assert_eq!(manual.region, Some("edge".to_string()));

        assert!(repository
            .record_traffic(&ProxyNodeTrafficMutation {
                node_id: manual.id.clone(),
                total_requests_delta: 5,
                failed_requests_delta: 1,
                dns_failures_delta: 1,
                stream_errors_delta: 0,
            })
            .await
            .expect("manual traffic should record"));
        repository
            .increment_manual_node_requests(&manual.id, 3, 1, Some(42))
            .await
            .expect("manual request counters should increment");
        let manual = repository
            .find_proxy_node(&manual.id)
            .await
            .expect("manual node should reload")
            .expect("manual node should exist");
        assert_eq!(manual.total_requests, 8);
        assert_eq!(manual.failed_requests, 2);
        assert_eq!(manual.avg_latency_ms, Some(42.0));

        let registered = repository
            .register_node(&ProxyNodeRegistrationMutation {
                name: "tunnel-1".to_string(),
                ip: "10.0.0.1".to_string(),
                port: 7000,
                region: Some("us".to_string()),
                heartbeat_interval: 30,
                active_connections: Some(2),
                total_requests: Some(10),
                avg_latency_ms: Some(12.5),
                hardware_info: Some(json!({"cpu":"m1"})),
                estimated_max_concurrency: Some(100),
                proxy_metadata: Some(json!({"arch":"arm64"})),
                proxy_version: Some("1.0.0".to_string()),
                registered_by: Some("proxy".to_string()),
                tunnel_mode: true,
            })
            .await
            .expect("tunnel node should register");
        assert!(!registered.is_manual);
        assert!(registered.tunnel_mode);
        assert_eq!(
            registered
                .proxy_metadata
                .as_ref()
                .and_then(|value| value.get("version"))
                .and_then(serde_json::Value::as_str),
            Some("1.0.0")
        );

        let configured = repository
            .update_remote_config(&ProxyNodeRemoteConfigMutation {
                node_id: registered.id.clone(),
                node_name: Some("tunnel-renamed".to_string()),
                allowed_ports: Some(vec![443, 8443]),
                log_level: Some("debug".to_string()),
                heartbeat_interval: Some(45),
                scheduling_state: Some(Some("draining".to_string())),
                upgrade_to: Some(Some("tunnel-v2.0.0".to_string())),
            })
            .await
            .expect("remote config should update")
            .expect("tunnel node should exist");
        assert_eq!(configured.name, "tunnel-renamed");
        assert_eq!(configured.config_version, 1);

        let heartbeat = repository
            .apply_heartbeat(&ProxyNodeHeartbeatMutation {
                node_id: registered.id.clone(),
                heartbeat_interval: Some(45),
                active_connections: Some(4),
                total_requests_delta: Some(6),
                avg_latency_ms: Some(10.0),
                failed_requests_delta: Some(1),
                dns_failures_delta: Some(0),
                stream_errors_delta: Some(2),
                proxy_metadata: Some(json!({"arch":"arm64"})),
                proxy_version: Some("2.0.0".to_string()),
            })
            .await
            .expect("heartbeat should apply")
            .expect("tunnel node should exist");
        assert_eq!(heartbeat.status, "online");
        assert!(heartbeat.tunnel_connected);
        assert_eq!(heartbeat.active_connections, 4);
        assert_eq!(heartbeat.total_requests, 16);
        assert_eq!(heartbeat.config_version, 2);
        assert!(heartbeat
            .remote_config
            .as_ref()
            .and_then(|value| value.get("upgrade_to"))
            .is_none());

        let stale = repository
            .update_tunnel_status(&ProxyNodeTunnelStatusMutation {
                node_id: registered.id.clone(),
                connected: false,
                conn_count: 0,
                detail: None,
                observed_at_unix_secs: Some(1),
            })
            .await
            .expect("stale tunnel status should be recorded")
            .expect("tunnel node should exist");
        assert_eq!(stale.status, "online");

        let disconnected = repository
            .update_tunnel_status(&ProxyNodeTunnelStatusMutation {
                node_id: registered.id.clone(),
                connected: false,
                conn_count: 0,
                detail: Some("closed".to_string()),
                observed_at_unix_secs: Some(1_800_000_000),
            })
            .await
            .expect("tunnel status should update")
            .expect("tunnel node should exist");
        assert_eq!(disconnected.status, "offline");
        assert!(!disconnected.tunnel_connected);

        let events = repository
            .list_proxy_node_events(&registered.id, 10)
            .await
            .expect("events should list");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].detail.as_deref(), Some("closed"));
        assert_eq!(
            events[1].detail.as_deref(),
            Some("[stale_ignored] [tunnel_node_status] conn_count=0")
        );

        assert_eq!(
            repository
                .reset_stale_tunnel_statuses()
                .await
                .expect("stale tunnels should reset"),
            0
        );
        assert!(repository
            .unregister_node(&registered.id)
            .await
            .expect("node should unregister")
            .is_some());
        assert!(repository
            .delete_node(&registered.id)
            .await
            .expect("node should delete")
            .is_some());
        assert!(repository
            .delete_node(&manual.id)
            .await
            .expect("manual node should delete")
            .is_some());
    }

    #[tokio::test]
    async fn sqlite_repository_aggregates_proxy_node_metrics_and_filters_events() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        let repository = SqliteProxyNodeReadRepository::new(pool);
        let registered = repository
            .register_node(&ProxyNodeRegistrationMutation {
                name: "tunnel-1".to_string(),
                ip: "10.0.0.1".to_string(),
                port: 7000,
                region: None,
                heartbeat_interval: 30,
                active_connections: Some(0),
                total_requests: Some(0),
                avg_latency_ms: None,
                hardware_info: None,
                estimated_max_concurrency: None,
                proxy_metadata: None,
                proxy_version: Some("1.0.0".to_string()),
                registered_by: None,
                tunnel_mode: true,
            })
            .await
            .expect("node should register");
        let now = super::current_unix_secs();
        repository
            .apply_heartbeat(&ProxyNodeHeartbeatMutation {
                node_id: registered.id.clone(),
                heartbeat_interval: Some(30),
                active_connections: Some(0),
                total_requests_delta: None,
                avg_latency_ms: None,
                failed_requests_delta: None,
                dns_failures_delta: None,
                stream_errors_delta: None,
                proxy_metadata: Some(json!({
                    "tunnel_metrics": {
                        "connect_errors": 0,
                        "disconnects": 0,
                        "error_events_total": 0,
                        "ws_in_bytes": 0,
                        "ws_out_bytes": 0,
                        "ws_in_frames": 0,
                        "ws_out_frames": 0,
                        "heartbeat_rtt_last_ms": 0
                    }
                })),
                proxy_version: Some("1.0.0".to_string()),
            })
            .await
            .expect("baseline heartbeat should apply")
            .expect("node should exist");
        repository
            .apply_heartbeat(&ProxyNodeHeartbeatMutation {
                node_id: registered.id.clone(),
                heartbeat_interval: Some(30),
                active_connections: Some(5),
                total_requests_delta: None,
                avg_latency_ms: None,
                failed_requests_delta: None,
                dns_failures_delta: None,
                stream_errors_delta: None,
                proxy_metadata: Some(json!({
                    "tunnel_metrics": {
                        "connect_errors": 4,
                        "disconnects": 1,
                        "error_events_total": 1,
                        "ws_in_bytes": 100,
                        "ws_out_bytes": 200,
                        "ws_in_frames": 3,
                        "ws_out_frames": 6,
                        "heartbeat_rtt_last_ms": 33
                    },
                    "recent_tunnel_errors": [{
                        "timestamp_unix_secs": now,
                        "category": "tcp_connect_timeout",
                        "message": "timeout"
                    }]
                })),
                proxy_version: Some("1.0.0".to_string()),
            })
            .await
            .expect("heartbeat should apply")
            .expect("node should exist");

        let metrics = repository
            .list_proxy_node_metrics(
                &registered.id,
                ProxyNodeMetricsStep::OneMinute,
                now.saturating_sub(120),
                now.saturating_add(120),
                10,
            )
            .await
            .expect("metrics should list");
        assert!(!metrics.is_empty());
        assert_eq!(metrics.iter().map(|bucket| bucket.samples).sum::<i64>(), 2);
        assert_eq!(
            metrics
                .iter()
                .map(|bucket| bucket.uptime_samples)
                .sum::<i64>(),
            2
        );
        assert_eq!(
            metrics
                .iter()
                .map(|bucket| bucket.active_connections_max)
                .max()
                .unwrap_or_default(),
            5
        );
        assert_eq!(
            metrics
                .iter()
                .map(|bucket| bucket.heartbeat_rtt_ms_sum)
                .sum::<i64>(),
            33
        );
        assert_eq!(
            metrics
                .iter()
                .map(|bucket| bucket.connect_errors_delta)
                .sum::<i64>(),
            4
        );
        assert_eq!(
            metrics
                .iter()
                .map(|bucket| bucket.ws_out_frames_delta)
                .sum::<i64>(),
            6
        );

        let fleet = repository
            .list_proxy_fleet_metrics(
                ProxyNodeMetricsStep::OneMinute,
                now.saturating_sub(120),
                now.saturating_add(120),
                10,
            )
            .await
            .expect("fleet metrics should list");
        assert!(!fleet.is_empty());
        assert_eq!(fleet.iter().map(|bucket| bucket.samples).sum::<i64>(), 2);
        assert_eq!(
            fleet
                .iter()
                .map(|bucket| bucket.error_events_delta)
                .sum::<i64>(),
            1
        );

        let events = repository
            .list_proxy_node_events_filtered(
                &registered.id,
                &ProxyNodeEventQuery {
                    limit: 10,
                    from_unix_secs: Some(now.saturating_sub(120)),
                    to_unix_secs: Some(now.saturating_add(120)),
                    event_type: Some(PROXY_NODE_EVENT_TYPE_TUNNEL_ERROR.to_string()),
                },
            )
            .await
            .expect("events should list");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, PROXY_NODE_EVENT_TYPE_TUNNEL_ERROR);
        assert_eq!(
            events[0]
                .event_metadata
                .as_ref()
                .and_then(|value| value.get("category"))
                .and_then(serde_json::Value::as_str),
            Some("tcp_connect_timeout")
        );

        let cleanup = repository
            .cleanup_proxy_node_metrics(now.saturating_add(1), now.saturating_add(1), 10)
            .await
            .expect("cleanup should run");
        assert_eq!(cleanup.deleted_1m_rows, 1);
        assert_eq!(cleanup.deleted_1h_rows, 1);
    }
}
