use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub(crate) struct InternalTunnelHeartbeatRequest {
    pub(crate) node_id: String,
    #[serde(default)]
    pub(crate) heartbeat_id: Option<u64>,
    #[serde(default)]
    pub(crate) heartbeat_interval: Option<i32>,
    #[serde(default)]
    pub(crate) active_connections: Option<i32>,
    #[serde(default)]
    pub(crate) total_requests: Option<i64>,
    #[serde(default)]
    pub(crate) avg_latency_ms: Option<f64>,
    #[serde(default)]
    pub(crate) failed_requests: Option<i64>,
    #[serde(default)]
    pub(crate) dns_failures: Option<i64>,
    #[serde(default)]
    pub(crate) stream_errors: Option<i64>,
    #[serde(default)]
    pub(crate) proxy_metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) proxy_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct InternalTunnelNodeStatusRequest {
    pub(crate) node_id: String,
    pub(crate) connected: bool,
    #[serde(default)]
    pub(crate) conn_count: i32,
    #[serde(default)]
    pub(crate) observed_at_unix_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct InternalGatewayResolveRequest {
    #[serde(default)]
    pub(crate) trace_id: Option<String>,
    pub(crate) method: String,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) query_string: Option<String>,
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct InternalGatewayAuthContextRequest {
    #[serde(default)]
    pub(crate) trace_id: Option<String>,
    #[serde(default)]
    pub(crate) query_string: Option<String>,
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) auth_endpoint_signature: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct InternalGatewayExecuteRequest {
    #[serde(default)]
    pub(crate) trace_id: Option<String>,
    pub(crate) method: String,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) query_string: Option<String>,
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) body_json: serde_json::Value,
    #[serde(default)]
    pub(crate) body_base64: Option<String>,
    #[serde(default)]
    pub(crate) auth_context: Option<crate::control::GatewayControlAuthContext>,
}
