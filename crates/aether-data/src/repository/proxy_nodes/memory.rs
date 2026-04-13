use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::types::{
    normalize_proxy_metadata, reconcile_remote_config_after_heartbeat, ProxyNodeHeartbeatMutation,
    ProxyNodeReadRepository, ProxyNodeRegistrationMutation, ProxyNodeRemoteConfigMutation,
    ProxyNodeTunnelStatusMutation, ProxyNodeWriteRepository, StoredProxyNode, StoredProxyNodeEvent,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryProxyNodeRepository {
    nodes: RwLock<BTreeMap<String, StoredProxyNode>>,
    events: RwLock<Vec<StoredProxyNodeEvent>>,
}

impl InMemoryProxyNodeRepository {
    pub fn seed<I>(nodes: I) -> Self
    where
        I: IntoIterator<Item = StoredProxyNode>,
    {
        Self {
            nodes: RwLock::new(
                nodes
                    .into_iter()
                    .map(|node| (node.id.clone(), node))
                    .collect(),
            ),
            events: RwLock::new(Vec::new()),
        }
    }

    pub fn seed_with_events<I, J>(nodes: I, events: J) -> Self
    where
        I: IntoIterator<Item = StoredProxyNode>,
        J: IntoIterator<Item = StoredProxyNodeEvent>,
    {
        Self {
            nodes: RwLock::new(
                nodes
                    .into_iter()
                    .map(|node| (node.id.clone(), node))
                    .collect(),
            ),
            events: RwLock::new(events.into_iter().collect()),
        }
    }

    fn now_unix_secs() -> Option<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
    }

    fn next_event_id(events: &[StoredProxyNodeEvent]) -> i64 {
        events.iter().map(|event| event.id).max().unwrap_or(0) + 1
    }

    fn normalize_remote_config(
        mutation: &ProxyNodeRemoteConfigMutation,
        existing: Option<&Value>,
    ) -> Option<Value> {
        let mut config = match existing {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };

        if let Some(node_name) = mutation.node_name.as_ref() {
            config.insert("node_name".to_string(), Value::String(node_name.clone()));
        }
        if let Some(allowed_ports) = mutation.allowed_ports.as_ref() {
            config.insert("allowed_ports".to_string(), json!(allowed_ports));
        }
        if let Some(log_level) = mutation.log_level.as_ref() {
            config.insert("log_level".to_string(), Value::String(log_level.clone()));
        }
        if let Some(heartbeat_interval) = mutation.heartbeat_interval {
            config.insert("heartbeat_interval".to_string(), json!(heartbeat_interval));
        }
        if let Some(scheduling_state) = mutation.scheduling_state.as_ref() {
            match scheduling_state {
                Some(state) => {
                    config.insert("scheduling_state".to_string(), Value::String(state.clone()));
                }
                None => {
                    config.remove("scheduling_state");
                }
            }
        }
        if let Some(upgrade_to) = mutation.upgrade_to.as_ref() {
            match upgrade_to {
                Some(version) => {
                    config.insert("upgrade_to".to_string(), Value::String(version.clone()));
                }
                None => {
                    config.remove("upgrade_to");
                }
            }
        }

        (!config.is_empty()).then_some(Value::Object(config))
    }
}

#[async_trait]
impl ProxyNodeReadRepository for InMemoryProxyNodeRepository {
    async fn list_proxy_nodes(&self) -> Result<Vec<StoredProxyNode>, DataLayerError> {
        let nodes = self.nodes.read().expect("proxy node repository lock");
        let mut items = nodes.values().cloned().collect::<Vec<_>>();
        items.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(items)
    }

    async fn find_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let nodes = self.nodes.read().expect("proxy node repository lock");
        Ok(nodes.get(node_id).cloned())
    }

    async fn list_proxy_node_events(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeEvent>, DataLayerError> {
        let events = self.events.read().expect("proxy node repository lock");
        let mut items = events
            .iter()
            .filter(|event| event.node_id == node_id)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .created_at_unix_ms
                .unwrap_or(0)
                .cmp(&left.created_at_unix_ms.unwrap_or(0))
                .then(right.id.cmp(&left.id))
        });
        items.truncate(limit);
        Ok(items)
    }
}

#[async_trait]
impl ProxyNodeWriteRepository for InMemoryProxyNodeRepository {
    async fn reset_stale_tunnel_statuses(&self) -> Result<usize, DataLayerError> {
        let mut nodes = self.nodes.write().expect("proxy node repository lock");
        let now = Self::now_unix_secs();
        let mut updated = 0usize;

        for node in nodes.values_mut() {
            if node.is_manual || !node.tunnel_connected {
                continue;
            }
            node.tunnel_connected = false;
            node.status = "offline".to_string();
            node.active_connections = 0;
            node.tunnel_connected_at_unix_secs = now;
            node.updated_at_unix_secs = now;
            updated = updated.saturating_add(1);
        }

        Ok(updated)
    }

    async fn register_node(
        &self,
        mutation: &ProxyNodeRegistrationMutation,
    ) -> Result<StoredProxyNode, DataLayerError> {
        let mut nodes = self.nodes.write().expect("proxy node repository lock");
        let now = Self::now_unix_secs();
        let normalized_proxy_metadata = normalize_proxy_metadata(
            mutation.proxy_metadata.as_ref(),
            mutation.proxy_version.as_deref(),
        );

        if let Some(existing_id) = nodes
            .iter()
            .find(|(_, node)| {
                !node.is_manual && node.ip == mutation.ip && node.port == mutation.port
            })
            .map(|(node_id, _)| node_id.clone())
        {
            let node = nodes
                .get_mut(&existing_id)
                .expect("existing proxy node should be present");
            node.name = mutation.name.clone();
            node.ip = mutation.ip.clone();
            node.port = mutation.port;
            node.region = mutation.region.clone();
            node.last_heartbeat_at_unix_secs = now;
            node.heartbeat_interval = mutation.heartbeat_interval;
            node.tunnel_mode = mutation.tunnel_mode;
            node.registered_by = mutation.registered_by.clone();

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
            return Ok(node.clone());
        }

        let mut node = StoredProxyNode::new(
            Uuid::new_v4().to_string(),
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
            normalized_proxy_metadata,
            mutation.hardware_info.clone(),
            mutation.estimated_max_concurrency,
            None,
            None,
            now,
            now,
        );
        node.avg_latency_ms = mutation.avg_latency_ms;

        nodes.insert(node.id.clone(), node.clone());
        Ok(node)
    }

    async fn apply_heartbeat(
        &self,
        mutation: &ProxyNodeHeartbeatMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let mut nodes = self.nodes.write().expect("proxy node repository lock");
        let Some(node) = nodes.get_mut(&mutation.node_id) else {
            return Ok(None);
        };
        if !node.tunnel_mode {
            return Err(DataLayerError::InvalidInput(
                "non-tunnel mode is no longer supported, please upgrade aether-proxy to use tunnel mode"
                    .to_string(),
            ));
        }

        let now = Self::now_unix_secs();
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

        Ok(Some(node.clone()))
    }

    async fn update_tunnel_status(
        &self,
        mutation: &ProxyNodeTunnelStatusMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let mut nodes = self.nodes.write().expect("proxy node repository lock");
        let Some(node) = nodes.get_mut(&mutation.node_id) else {
            return Ok(None);
        };

        let event_time = mutation
            .observed_at_unix_secs
            .or_else(Self::now_unix_secs)
            .unwrap_or(0);
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
        let mut events = self.events.write().expect("proxy node repository lock");
        if let Some(last_transition) = node.tunnel_connected_at_unix_secs {
            if event_time < last_transition {
                let event_id = Self::next_event_id(&events);
                events.push(StoredProxyNodeEvent {
                    id: event_id,
                    node_id: mutation.node_id.clone(),
                    event_type: event_type.to_string(),
                    detail: Some(format!("[stale_ignored] {event_detail}")),
                    created_at_unix_ms: Self::now_unix_secs(),
                });
                return Ok(Some(node.clone()));
            }
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
        let event_id = Self::next_event_id(&events);
        events.push(StoredProxyNodeEvent {
            id: event_id,
            node_id: mutation.node_id.clone(),
            event_type: event_type.to_string(),
            detail: Some(event_detail),
            created_at_unix_ms: Some(event_time),
        });
        Ok(Some(node.clone()))
    }

    async fn unregister_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let mut nodes = self.nodes.write().expect("proxy node repository lock");
        let Some(node) = nodes.get_mut(node_id) else {
            return Ok(None);
        };
        let now = Self::now_unix_secs();
        node.status = "offline".to_string();
        node.tunnel_connected = false;
        node.active_connections = 0;
        node.tunnel_connected_at_unix_secs = now;
        node.updated_at_unix_secs = now;
        Ok(Some(node.clone()))
    }

    async fn update_remote_config(
        &self,
        mutation: &ProxyNodeRemoteConfigMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        let mut nodes = self.nodes.write().expect("proxy node repository lock");
        let Some(node) = nodes.get_mut(&mutation.node_id) else {
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
        node.updated_at_unix_secs = Self::now_unix_secs();
        Ok(Some(node.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryProxyNodeRepository;
    use crate::repository::proxy_nodes::{
        ProxyNodeHeartbeatMutation, ProxyNodeReadRepository, ProxyNodeRegistrationMutation,
        ProxyNodeRemoteConfigMutation, ProxyNodeTunnelStatusMutation, ProxyNodeWriteRepository,
        StoredProxyNode, StoredProxyNodeEvent,
    };
    use serde_json::json;

    fn sample_node() -> StoredProxyNode {
        StoredProxyNode::new(
            "node-1".to_string(),
            "proxy-1".to_string(),
            "127.0.0.1".to_string(),
            0,
            false,
            "offline".to_string(),
            30,
            0,
            0,
            0,
            0,
            0,
            true,
            false,
            2,
        )
        .expect("node should build")
        .with_runtime_fields(
            Some("test".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(json!({"allowed_ports": [443]})),
            Some(1_700_000_000),
            Some(1_700_000_001),
        )
    }

    #[tokio::test]
    async fn applies_heartbeat_and_tunnel_status_mutations() {
        let repository = InMemoryProxyNodeRepository::seed(vec![sample_node()]);

        let heartbeat = repository
            .apply_heartbeat(&ProxyNodeHeartbeatMutation {
                node_id: "node-1".to_string(),
                heartbeat_interval: Some(45),
                active_connections: Some(5),
                total_requests_delta: Some(8),
                avg_latency_ms: Some(12.5),
                failed_requests_delta: Some(2),
                dns_failures_delta: Some(1),
                stream_errors_delta: Some(3),
                proxy_metadata: Some(json!({"arch": "arm64"})),
                proxy_version: Some("1.2.3".to_string()),
            })
            .await
            .expect("heartbeat should succeed")
            .expect("node should exist");

        assert_eq!(heartbeat.status, "online");
        assert_eq!(heartbeat.heartbeat_interval, 45);
        assert_eq!(heartbeat.active_connections, 5);
        assert_eq!(heartbeat.total_requests, 8);
        assert_eq!(heartbeat.failed_requests, 2);
        assert_eq!(heartbeat.dns_failures, 1);
        assert_eq!(heartbeat.stream_errors, 3);
        assert_eq!(
            heartbeat
                .proxy_metadata
                .as_ref()
                .and_then(|value| value.get("version"))
                .and_then(|value| value.as_str()),
            Some("1.2.3")
        );

        let stale = repository
            .update_tunnel_status(&ProxyNodeTunnelStatusMutation {
                node_id: "node-1".to_string(),
                connected: false,
                conn_count: 0,
                detail: None,
                observed_at_unix_secs: Some(1),
            })
            .await
            .expect("status update should succeed")
            .expect("node should exist");
        assert_eq!(stale.status, "online");

        let stale_events = repository
            .list_proxy_node_events("node-1", 10)
            .await
            .expect("list events should succeed");
        assert_eq!(stale_events.len(), 1);
        assert_eq!(stale_events[0].event_type, "disconnected");
        assert_eq!(
            stale_events[0].detail.as_deref(),
            Some("[stale_ignored] [tunnel_node_status] conn_count=0")
        );

        let updated = repository
            .update_tunnel_status(&ProxyNodeTunnelStatusMutation {
                node_id: "node-1".to_string(),
                connected: false,
                conn_count: 0,
                detail: None,
                observed_at_unix_secs: Some(1_800_000_000),
            })
            .await
            .expect("status update should succeed")
            .expect("node should exist");
        assert_eq!(updated.status, "offline");
        assert!(!updated.tunnel_connected);
        assert_eq!(updated.active_connections, 0);

        let events = repository
            .list_proxy_node_events("node-1", 10)
            .await
            .expect("list events should succeed");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "disconnected");
        assert_eq!(events[0].created_at_unix_ms, Some(1_800_000_000));
        assert_eq!(
            events[0].detail.as_deref(),
            Some("[tunnel_node_status] conn_count=0")
        );

        let found = repository
            .find_proxy_node("node-1")
            .await
            .expect("find should succeed")
            .expect("node should exist");
        assert_eq!(found.status, "offline");
    }

    #[tokio::test]
    async fn lists_seeded_proxy_node_events_in_descending_order() {
        let repository = InMemoryProxyNodeRepository::seed_with_events(
            vec![sample_node()],
            vec![
                StoredProxyNodeEvent {
                    id: 1,
                    node_id: "node-1".to_string(),
                    event_type: "connected".to_string(),
                    detail: Some("older".to_string()),
                    created_at_unix_ms: Some(1_710_000_000),
                },
                StoredProxyNodeEvent {
                    id: 2,
                    node_id: "node-1".to_string(),
                    event_type: "disconnected".to_string(),
                    detail: Some("newer".to_string()),
                    created_at_unix_ms: Some(1_710_000_100),
                },
            ],
        );

        let events = repository
            .list_proxy_node_events("node-1", 1)
            .await
            .expect("list events should succeed");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, 2);
        assert_eq!(events[0].detail.as_deref(), Some("newer"));
    }

    #[tokio::test]
    async fn resets_stale_tunnel_statuses_without_touching_manual_nodes() {
        let mut stale_tunnel = sample_node();
        stale_tunnel.tunnel_connected = true;
        stale_tunnel.status = "online".to_string();
        stale_tunnel.active_connections = 6;

        let mut manual_node = sample_node();
        manual_node.id = "manual-node".to_string();
        manual_node.is_manual = true;
        manual_node.tunnel_connected = true;
        manual_node.status = "online".to_string();
        manual_node.active_connections = 4;

        let repository = InMemoryProxyNodeRepository::seed(vec![stale_tunnel, manual_node]);

        let updated = repository
            .reset_stale_tunnel_statuses()
            .await
            .expect("reset should succeed");
        assert_eq!(updated, 1);

        let stale = repository
            .find_proxy_node("node-1")
            .await
            .expect("lookup should succeed")
            .expect("stale node should exist");
        assert_eq!(stale.status, "offline");
        assert!(!stale.tunnel_connected);
        assert_eq!(stale.active_connections, 0);

        let manual = repository
            .find_proxy_node("manual-node")
            .await
            .expect("lookup should succeed")
            .expect("manual node should exist");
        assert_eq!(manual.status, "online");
        assert!(manual.tunnel_connected);
        assert_eq!(manual.active_connections, 4);
    }

    #[tokio::test]
    async fn registers_updates_config_and_unregisters_nodes() {
        let repository = InMemoryProxyNodeRepository::default();

        let registered = repository
            .register_node(&ProxyNodeRegistrationMutation {
                name: "proxy-01".to_string(),
                ip: "127.0.0.1".to_string(),
                port: 0,
                region: Some("test".to_string()),
                heartbeat_interval: 30,
                active_connections: Some(1),
                total_requests: Some(2),
                avg_latency_ms: Some(3.5),
                hardware_info: Some(json!({"cpu": "arm64"})),
                estimated_max_concurrency: Some(128),
                proxy_metadata: Some(json!({"arch": "arm64"})),
                proxy_version: Some("2.1.0".to_string()),
                registered_by: Some("admin-1".to_string()),
                tunnel_mode: true,
            })
            .await
            .expect("register should succeed");
        assert_eq!(registered.status, "offline");
        assert_eq!(registered.total_requests, 2);
        assert_eq!(registered.config_version, 0);

        let updated = repository
            .update_remote_config(&ProxyNodeRemoteConfigMutation {
                node_id: registered.id.clone(),
                node_name: Some("proxy-02".to_string()),
                allowed_ports: Some(vec![443, 8443]),
                log_level: Some("info".to_string()),
                heartbeat_interval: Some(45),
                scheduling_state: Some(Some("draining".to_string())),
                upgrade_to: Some(Some("2.2.0".to_string())),
            })
            .await
            .expect("config update should succeed")
            .expect("node should exist");
        assert_eq!(updated.name, "proxy-02");
        assert_eq!(updated.config_version, 1);
        assert_eq!(
            updated
                .remote_config
                .as_ref()
                .and_then(|value| value.get("scheduling_state")),
            Some(&json!("draining"))
        );
        assert_eq!(
            updated
                .remote_config
                .as_ref()
                .and_then(|value| value.get("upgrade_to")),
            Some(&json!("2.2.0"))
        );
        let after_upgrade = repository
            .apply_heartbeat(&ProxyNodeHeartbeatMutation {
                node_id: registered.id.clone(),
                heartbeat_interval: None,
                active_connections: Some(2),
                total_requests_delta: Some(1),
                avg_latency_ms: Some(2.0),
                failed_requests_delta: Some(0),
                dns_failures_delta: Some(0),
                stream_errors_delta: Some(0),
                proxy_metadata: Some(json!({"arch": "arm64"})),
                proxy_version: Some("proxy-v2.2.0".to_string()),
            })
            .await
            .expect("heartbeat should succeed")
            .expect("node should exist");
        assert_eq!(after_upgrade.config_version, 2);
        assert!(after_upgrade
            .remote_config
            .as_ref()
            .and_then(|value| value.get("upgrade_to"))
            .is_none());

        let unregistered = repository
            .unregister_node(&registered.id)
            .await
            .expect("unregister should succeed")
            .expect("node should exist");
        assert_eq!(unregistered.status, "offline");
        assert!(!unregistered.tunnel_connected);
    }
}
