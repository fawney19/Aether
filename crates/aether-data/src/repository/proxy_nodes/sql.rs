use async_trait::async_trait;
use futures_util::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgRow, PgPool, Row};

use super::types::{
    normalize_proxy_metadata, reconcile_remote_config_after_heartbeat, ProxyNodeHeartbeatMutation,
    ProxyNodeManualCreateMutation, ProxyNodeManualUpdateMutation, ProxyNodeReadRepository,
    ProxyNodeRegistrationMutation, ProxyNodeRemoteConfigMutation, ProxyNodeTunnelStatusMutation,
    ProxyNodeWriteRepository, StoredProxyNode, StoredProxyNodeEvent,
};
use crate::{
    error::{postgres_error, SqlxResultExt},
    DataLayerError,
};

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

const LIST_PROXY_NODES_SQL: &str = r#"
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
ORDER BY name ASC, id ASC
"#;

const LIST_PROXY_NODE_EVENTS_SQL: &str = r#"
SELECT
  id,
  node_id,
  CAST(event_type AS TEXT) AS event_type,
  detail,
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
            created_at_unix_ms: Self::optional_unix_secs(
                row.try_get("created_at_unix_ms").map_postgres_err()?,
            ),
        })
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
        let mut rows = sqlx::query(LIST_PROXY_NODES_SQL).fetch(&self.pool);
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
        let row = sqlx::query(FIND_PROXY_NODE_SQL)
            .bind(node_id)
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
        let mut rows = sqlx::query(LIST_PROXY_NODE_EVENTS_SQL)
            .bind(node_id)
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(Self::row_to_event(&row)?);
        }
        Ok(items)
    }
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
                "non-tunnel mode is no longer supported, please upgrade aether-proxy to use tunnel mode"
                    .to_string(),
            ));
        }

        let normalized_proxy_metadata = normalize_proxy_metadata(
            mutation.proxy_metadata.as_ref(),
            mutation.proxy_version.as_deref(),
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
            sqlx::query(
                r#"
INSERT INTO proxy_node_events (node_id, event_type, detail, created_at)
VALUES (
  $1,
  $2,
  $3,
  NOW()
)
"#,
            )
            .bind(&mutation.node_id)
            .bind(event_type)
            .bind(format!("[stale_ignored] {event_detail}"))
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

        sqlx::query(
            r#"
INSERT INTO proxy_node_events (node_id, event_type, detail, created_at)
VALUES (
  $1,
  $2,
  $3,
  CASE
    WHEN $4::double precision IS NULL THEN NOW()
    ELSE TO_TIMESTAMP($4::double precision)
  END
)
"#,
        )
        .bind(&mutation.node_id)
        .bind(event_type)
        .bind(event_detail)
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
