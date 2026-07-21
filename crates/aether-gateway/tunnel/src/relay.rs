use serde::{Deserialize, Serialize};

pub const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
pub const TUNNEL_HEARTBEAT_PATH: &str = "/api/internal/tunnel/heartbeat";
pub const TUNNEL_NODE_STATUS_PATH: &str = "/api/internal/tunnel/node-status";
pub const TUNNEL_RELAY_PATH_PATTERN: &str = "/api/internal/tunnel/relay/{node_id}";
pub const TUNNEL_ROUTE_FAMILY: &str = "tunnel_manage";

pub const DEFAULT_OWNER_RELAY_BODY_LIMIT_BYTES: usize = 5_242_880;
pub const DEFAULT_TUNNEL_PROBE_BODY_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelAttachmentRecord {
    pub gateway_instance_id: String,
    pub relay_base_url: String,
    pub conn_count: usize,
    pub observed_at_unix_secs: u64,
}

impl TunnelAttachmentRecord {
    pub fn is_routable(&self, now_unix_secs: u64, ttl_secs: u64) -> bool {
        self.conn_count > 0
            && !self.relay_base_url.trim().is_empty()
            && self.observed_at_unix_secs.saturating_add(ttl_secs) >= now_unix_secs
    }

    pub fn is_owned_by(&self, gateway_instance_id: &str) -> bool {
        self.gateway_instance_id == gateway_instance_id
    }
}

pub fn is_tunnel_heartbeat_path(path: &str) -> bool {
    path == TUNNEL_HEARTBEAT_PATH
}

pub fn is_tunnel_node_status_path(path: &str) -> bool {
    path == TUNNEL_NODE_STATUS_PATH
}

#[cfg(test)]
mod tests {
    use super::TunnelAttachmentRecord;

    fn record() -> TunnelAttachmentRecord {
        TunnelAttachmentRecord {
            gateway_instance_id: "gateway-a".to_string(),
            relay_base_url: "http://gateway-a.internal".to_string(),
            conn_count: 1,
            observed_at_unix_secs: 100,
        }
    }

    #[test]
    fn attachment_is_routable_until_ttl_boundary() {
        let record = record();
        assert!(record.is_routable(190, 90));
        assert!(!record.is_routable(191, 90));
    }

    #[test]
    fn attachment_requires_connection_and_relay_url() {
        let mut record = record();
        record.conn_count = 0;
        assert!(!record.is_routable(100, 90));
        record.conn_count = 1;
        record.relay_base_url.clear();
        assert!(!record.is_routable(100, 90));
    }
}
