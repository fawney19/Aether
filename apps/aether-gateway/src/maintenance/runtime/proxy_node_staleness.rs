use std::time::{SystemTime, UNIX_EPOCH};

use aether_data::repository::proxy_nodes::ProxyNodeTunnelStatusMutation;
use aether_data_contracts::DataLayerError;

use crate::data::GatewayDataState;

use super::{PROXY_NODE_STALE_MIN_GRACE_SECS, PROXY_NODE_STALE_MISSED_HEARTBEATS};

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn stale_proxy_node_grace_secs(heartbeat_interval: i32) -> u64 {
    let interval = u64::try_from(heartbeat_interval.max(5)).unwrap_or(5);
    interval
        .saturating_mul(PROXY_NODE_STALE_MISSED_HEARTBEATS)
        .max(PROXY_NODE_STALE_MIN_GRACE_SECS)
}

pub(super) async fn cleanup_stale_proxy_nodes_once(
    data: &GatewayDataState,
) -> Result<usize, DataLayerError> {
    if !data.has_proxy_node_reader() || !data.has_proxy_node_writer() {
        return Ok(0);
    }

    let now_unix_secs = current_unix_secs();
    let nodes = data.list_proxy_nodes().await?;
    let mut updated = 0usize;

    for node in nodes {
        if node.is_manual || !node.tunnel_connected {
            continue;
        }

        let grace_secs = stale_proxy_node_grace_secs(node.heartbeat_interval);
        let is_stale = node
            .last_heartbeat_at_unix_secs
            .map(|last_seen| last_seen.saturating_add(grace_secs) < now_unix_secs)
            .unwrap_or(true);
        if !is_stale {
            continue;
        }

        let detail = format!(
            "[heartbeat_timeout] last_heartbeat_at={} grace_secs={}",
            node.last_heartbeat_at_unix_secs.unwrap_or(0),
            grace_secs
        );
        let mutation = ProxyNodeTunnelStatusMutation {
            node_id: node.id.clone(),
            connected: false,
            conn_count: 0,
            detail: Some(detail),
            observed_at_unix_secs: Some(now_unix_secs),
        };
        if data
            .update_proxy_node_tunnel_status(&mutation)
            .await?
            .is_some()
        {
            updated = updated.saturating_add(1);
        }
    }

    Ok(updated)
}
