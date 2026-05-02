use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProxyNode {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub port: i32,
    pub region: Option<String>,
    pub is_manual: bool,
    pub proxy_url: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub status: String,
    pub registered_by: Option<String>,
    pub last_heartbeat_at_unix_secs: Option<u64>,
    pub heartbeat_interval: i32,
    pub active_connections: i32,
    pub total_requests: i64,
    pub avg_latency_ms: Option<f64>,
    pub failed_requests: i64,
    pub dns_failures: i64,
    pub stream_errors: i64,
    pub proxy_metadata: Option<serde_json::Value>,
    pub hardware_info: Option<serde_json::Value>,
    pub estimated_max_concurrency: Option<i32>,
    pub tunnel_mode: bool,
    pub tunnel_connected: bool,
    pub tunnel_connected_at_unix_secs: Option<u64>,
    pub remote_config: Option<serde_json::Value>,
    pub config_version: i32,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl StoredProxyNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        ip: String,
        port: i32,
        is_manual: bool,
        status: String,
        heartbeat_interval: i32,
        active_connections: i32,
        total_requests: i64,
        failed_requests: i64,
        dns_failures: i64,
        stream_errors: i64,
        tunnel_mode: bool,
        tunnel_connected: bool,
        config_version: i32,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "proxy_nodes.id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "proxy_nodes.name is empty".to_string(),
            ));
        }
        if ip.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "proxy_nodes.ip is empty".to_string(),
            ));
        }
        if status.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "proxy_nodes.status is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            name,
            ip,
            port,
            region: None,
            is_manual,
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            status,
            registered_by: None,
            last_heartbeat_at_unix_secs: None,
            heartbeat_interval,
            active_connections,
            total_requests,
            avg_latency_ms: None,
            failed_requests,
            dns_failures,
            stream_errors,
            proxy_metadata: None,
            hardware_info: None,
            estimated_max_concurrency: None,
            tunnel_mode,
            tunnel_connected,
            tunnel_connected_at_unix_secs: None,
            remote_config: None,
            config_version,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_runtime_fields(
        mut self,
        region: Option<String>,
        registered_by: Option<String>,
        last_heartbeat_at_unix_secs: Option<u64>,
        avg_latency_ms: Option<f64>,
        proxy_metadata: Option<serde_json::Value>,
        hardware_info: Option<serde_json::Value>,
        estimated_max_concurrency: Option<i32>,
        tunnel_connected_at_unix_secs: Option<u64>,
        remote_config: Option<serde_json::Value>,
        created_at_unix_ms: Option<u64>,
        updated_at_unix_secs: Option<u64>,
    ) -> Self {
        self.region = region;
        self.registered_by = registered_by;
        self.last_heartbeat_at_unix_secs = last_heartbeat_at_unix_secs;
        self.avg_latency_ms = avg_latency_ms;
        self.proxy_metadata = proxy_metadata;
        self.hardware_info = hardware_info;
        self.estimated_max_concurrency = estimated_max_concurrency;
        self.tunnel_connected_at_unix_secs = tunnel_connected_at_unix_secs;
        self.remote_config = remote_config;
        self.created_at_unix_ms = created_at_unix_ms;
        self.updated_at_unix_secs = updated_at_unix_secs;
        self
    }

    pub fn with_manual_proxy_fields(
        mut self,
        proxy_url: Option<String>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
    ) -> Self {
        self.proxy_url = proxy_url;
        self.proxy_username = proxy_username;
        self.proxy_password = proxy_password;
        self
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProxyNodeHeartbeatMutation {
    pub node_id: String,
    pub heartbeat_interval: Option<i32>,
    pub active_connections: Option<i32>,
    pub total_requests_delta: Option<i64>,
    pub avg_latency_ms: Option<f64>,
    pub failed_requests_delta: Option<i64>,
    pub dns_failures_delta: Option<i64>,
    pub stream_errors_delta: Option<i64>,
    pub proxy_metadata: Option<serde_json::Value>,
    pub proxy_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProxyNodeRegistrationMutation {
    pub name: String,
    pub ip: String,
    pub port: i32,
    pub region: Option<String>,
    pub heartbeat_interval: i32,
    pub active_connections: Option<i32>,
    pub total_requests: Option<i64>,
    pub avg_latency_ms: Option<f64>,
    pub hardware_info: Option<serde_json::Value>,
    pub estimated_max_concurrency: Option<i32>,
    pub proxy_metadata: Option<serde_json::Value>,
    pub proxy_version: Option<String>,
    pub registered_by: Option<String>,
    pub tunnel_mode: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProxyNodeManualCreateMutation {
    pub name: String,
    pub ip: String,
    pub port: i32,
    pub region: Option<String>,
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub registered_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProxyNodeManualUpdateMutation {
    pub node_id: String,
    pub name: Option<String>,
    pub ip: Option<String>,
    pub port: Option<i32>,
    pub region: Option<String>,
    pub proxy_url: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProxyNodeTunnelStatusMutation {
    pub node_id: String,
    pub connected: bool,
    pub conn_count: i32,
    pub detail: Option<String>,
    pub observed_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProxyNodeRemoteConfigMutation {
    pub node_id: String,
    pub node_name: Option<String>,
    pub allowed_ports: Option<Vec<u16>>,
    pub log_level: Option<String>,
    pub heartbeat_interval: Option<i32>,
    pub scheduling_state: Option<Option<String>>,
    pub upgrade_to: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProxyNodeEvent {
    pub id: i64,
    pub node_id: String,
    pub event_type: String,
    pub detail: Option<String>,
    pub created_at_unix_ms: Option<u64>,
}

pub fn normalize_proxy_metadata(
    proxy_metadata: Option<&serde_json::Value>,
    proxy_version: Option<&str>,
) -> Option<serde_json::Value> {
    let mut normalized = match proxy_metadata {
        Some(serde_json::Value::Object(map)) => map.clone(),
        Some(_) | None => serde_json::Map::new(),
    };

    let raw_version = normalized
        .remove("version")
        .and_then(|value| value.as_str().map(str::to_string));
    let version = proxy_version
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(20).collect::<String>())
        .or_else(|| {
            raw_version
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(20).collect::<String>())
        });
    if let Some(version) = version {
        normalized.insert("version".to_string(), serde_json::Value::String(version));
    }

    if normalized.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(normalized))
    }
}

fn normalize_proxy_version_label(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed
            .strip_prefix("proxy-v")
            .unwrap_or(trimmed)
            .to_ascii_lowercase(),
    )
}

pub const PROXY_NODE_SCHEDULING_STATE_DRAINING: &str = "draining";
pub const PROXY_NODE_SCHEDULING_STATE_CORDONED: &str = "cordoned";

pub fn normalize_proxy_node_scheduling_state(value: &str) -> Option<&'static str> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case(PROXY_NODE_SCHEDULING_STATE_DRAINING) {
        return Some(PROXY_NODE_SCHEDULING_STATE_DRAINING);
    }
    if trimmed.eq_ignore_ascii_case(PROXY_NODE_SCHEDULING_STATE_CORDONED) {
        return Some(PROXY_NODE_SCHEDULING_STATE_CORDONED);
    }
    None
}

pub fn remote_config_scheduling_state(
    remote_config: Option<&serde_json::Value>,
) -> Option<&'static str> {
    remote_config
        .and_then(serde_json::Value::as_object)
        .and_then(|value| value.get("scheduling_state"))
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_proxy_node_scheduling_state)
}

pub fn proxy_node_accepts_new_tunnels(node: &StoredProxyNode) -> bool {
    remote_config_scheduling_state(node.remote_config.as_ref()).is_none()
}

pub fn proxy_reported_version(proxy_metadata: Option<&serde_json::Value>) -> Option<String> {
    proxy_metadata
        .and_then(serde_json::Value::as_object)
        .and_then(|value| value.get("version"))
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_proxy_version_label)
}

pub fn remote_config_upgrade_target(remote_config: Option<&serde_json::Value>) -> Option<String> {
    remote_config
        .and_then(serde_json::Value::as_object)
        .and_then(|value| value.get("upgrade_to"))
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_proxy_version_label)
}

pub fn reconcile_remote_config_after_heartbeat(
    remote_config: Option<&serde_json::Value>,
    proxy_version: Option<&str>,
) -> Option<serde_json::Value> {
    let Some(mut config) = remote_config
        .and_then(serde_json::Value::as_object)
        .cloned()
    else {
        return remote_config.cloned();
    };
    let Some(target_version) = config
        .get("upgrade_to")
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_proxy_version_label)
    else {
        return Some(serde_json::Value::Object(config));
    };
    let Some(reported_version) = proxy_version.and_then(normalize_proxy_version_label) else {
        return Some(serde_json::Value::Object(config));
    };

    if reported_version == target_version {
        config.remove("upgrade_to");
    }

    (!config.is_empty()).then_some(serde_json::Value::Object(config))
}

#[async_trait]
pub trait ProxyNodeReadRepository: Send + Sync {
    async fn list_proxy_nodes(&self) -> Result<Vec<StoredProxyNode>, crate::DataLayerError>;

    async fn find_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, crate::DataLayerError>;

    async fn list_proxy_node_events(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeEvent>, crate::DataLayerError>;
}

#[async_trait]
pub trait ProxyNodeWriteRepository: Send + Sync {
    async fn reset_stale_tunnel_statuses(&self) -> Result<usize, crate::DataLayerError>;

    async fn create_manual_node(
        &self,
        mutation: &ProxyNodeManualCreateMutation,
    ) -> Result<StoredProxyNode, crate::DataLayerError>;

    async fn update_manual_node(
        &self,
        mutation: &ProxyNodeManualUpdateMutation,
    ) -> Result<Option<StoredProxyNode>, crate::DataLayerError>;

    async fn register_node(
        &self,
        mutation: &ProxyNodeRegistrationMutation,
    ) -> Result<StoredProxyNode, crate::DataLayerError>;

    async fn apply_heartbeat(
        &self,
        mutation: &ProxyNodeHeartbeatMutation,
    ) -> Result<Option<StoredProxyNode>, crate::DataLayerError>;

    async fn update_tunnel_status(
        &self,
        mutation: &ProxyNodeTunnelStatusMutation,
    ) -> Result<Option<StoredProxyNode>, crate::DataLayerError>;

    async fn unregister_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, crate::DataLayerError>;

    async fn delete_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, crate::DataLayerError>;

    async fn update_remote_config(
        &self,
        mutation: &ProxyNodeRemoteConfigMutation,
    ) -> Result<Option<StoredProxyNode>, crate::DataLayerError>;

    async fn increment_manual_node_requests(
        &self,
        node_id: &str,
        total_delta: i64,
        failed_delta: i64,
        latency_ms: Option<i64>,
    ) -> Result<(), crate::DataLayerError>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        normalize_proxy_node_scheduling_state, proxy_node_accepts_new_tunnels,
        proxy_reported_version, reconcile_remote_config_after_heartbeat,
        remote_config_scheduling_state, remote_config_upgrade_target, StoredProxyNode,
    };

    #[test]
    fn normalizes_reported_versions_and_clears_completed_upgrade_targets() {
        let remote_config = json!({
            "node_name": "edge-1",
            "upgrade_to": "proxy-v2.0.0",
        });
        let proxy_metadata = json!({
            "version": "2.0.0",
            "arch": "arm64",
        });

        assert_eq!(
            proxy_reported_version(Some(&proxy_metadata)).as_deref(),
            Some("2.0.0")
        );
        assert_eq!(
            remote_config_upgrade_target(Some(&remote_config)).as_deref(),
            Some("2.0.0")
        );

        let reconciled =
            reconcile_remote_config_after_heartbeat(Some(&remote_config), Some("proxy-v2.0.0"))
                .expect("reconciled config should remain an object");
        assert_eq!(reconciled.get("upgrade_to"), None);
        assert_eq!(reconciled.get("node_name"), Some(&json!("edge-1")));
    }

    #[test]
    fn normalizes_proxy_node_scheduling_state_and_detects_unschedulable_nodes() {
        assert_eq!(
            normalize_proxy_node_scheduling_state("draining"),
            Some("draining")
        );
        assert_eq!(
            normalize_proxy_node_scheduling_state(" CORDONED "),
            Some("cordoned")
        );
        assert_eq!(normalize_proxy_node_scheduling_state("active"), None);

        let remote_config = json!({
            "node_name": "edge-1",
            "scheduling_state": "draining",
        });
        assert_eq!(
            remote_config_scheduling_state(Some(&remote_config)),
            Some("draining")
        );

        let node = StoredProxyNode::new(
            "node-1".to_string(),
            "edge-1".to_string(),
            "127.0.0.1".to_string(),
            0,
            false,
            "online".to_string(),
            30,
            0,
            0,
            0,
            0,
            0,
            true,
            true,
            0,
        )
        .expect("node should build")
        .with_runtime_fields(
            None,
            None,
            Some(1_800_000_000),
            None,
            None,
            None,
            None,
            Some(1_800_000_001),
            Some(remote_config),
            Some(1_800_000_000),
            Some(1_800_000_001),
        );

        assert!(!proxy_node_accepts_new_tunnels(&node));
    }
}
