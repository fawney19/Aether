use async_trait::async_trait;
use serde_json::Value;

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
pub struct ProxyNodeTrafficMutation {
    pub node_id: String,
    pub total_requests_delta: i64,
    pub failed_requests_delta: i64,
    pub dns_failures_delta: i64,
    pub stream_errors_delta: i64,
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
    pub event_metadata: Option<serde_json::Value>,
    pub created_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProxyNodeEventQuery {
    pub limit: usize,
    pub from_unix_secs: Option<u64>,
    pub to_unix_secs: Option<u64>,
    pub event_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProxyNodeMetricsStep {
    OneMinute,
    OneHour,
}

impl ProxyNodeMetricsStep {
    pub fn bucket_size_secs(self) -> u64 {
        match self {
            Self::OneMinute => 60,
            Self::OneHour => 3_600,
        }
    }

    pub fn as_api_value(self) -> &'static str {
        match self {
            Self::OneMinute => "1m",
            Self::OneHour => "1h",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProxyNodeMetricsBucket {
    pub node_id: String,
    pub bucket_start_unix_secs: u64,
    pub samples: i64,
    pub uptime_samples: i64,
    pub active_connections_sum: i64,
    pub active_connections_max: i64,
    pub heartbeat_rtt_ms_sum: i64,
    pub heartbeat_rtt_ms_max: i64,
    pub connect_errors_delta: i64,
    pub disconnects_delta: i64,
    pub error_events_delta: i64,
    pub ws_in_bytes_delta: i64,
    pub ws_out_bytes_delta: i64,
    pub ws_in_frames_delta: i64,
    pub ws_out_frames_delta: i64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProxyFleetMetricsBucket {
    pub bucket_start_unix_secs: u64,
    pub samples: i64,
    pub uptime_samples: i64,
    pub active_connections_sum: i64,
    pub active_connections_max: i64,
    pub heartbeat_rtt_ms_sum: i64,
    pub heartbeat_rtt_ms_max: i64,
    pub connect_errors_delta: i64,
    pub disconnects_delta: i64,
    pub error_events_delta: i64,
    pub ws_in_bytes_delta: i64,
    pub ws_out_bytes_delta: i64,
    pub ws_in_frames_delta: i64,
    pub ws_out_frames_delta: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ProxyNodeMetricsCleanupSummary {
    pub deleted_1m_rows: usize,
    pub deleted_1h_rows: usize,
}

pub const PROXY_NODE_EVENT_TYPE_TUNNEL_ERROR: &str = "tunnel_err";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TunnelErrorEventRecord {
    pub timestamp_unix_secs: u64,
    pub timestamp_unix_ms: Option<u64>,
    pub category: String,
    pub message: String,
    pub severity: Option<String>,
    pub component: Option<String>,
    pub summary: Option<String>,
    pub operator_action: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TunnelMetricsCounters {
    pub connect_errors: u64,
    pub disconnects: u64,
    pub error_events_total: u64,
    pub ws_in_bytes: u64,
    pub ws_out_bytes: u64,
    pub ws_in_frames: u64,
    pub ws_out_frames: u64,
    pub heartbeat_rtt_last_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelMetricsSample {
    pub samples: i64,
    pub uptime_samples: i64,
    pub active_connections_sum: i64,
    pub active_connections_max: i64,
    pub heartbeat_rtt_ms_sum: i64,
    pub heartbeat_rtt_ms_max: i64,
    pub connect_errors_delta: i64,
    pub disconnects_delta: i64,
    pub error_events_delta: i64,
    pub ws_in_bytes_delta: i64,
    pub ws_out_bytes_delta: i64,
    pub ws_in_frames_delta: i64,
    pub ws_out_frames_delta: i64,
    pub recent_error_events: Vec<TunnelErrorEventRecord>,
}

pub fn bucket_start_unix_secs(timestamp_unix_secs: u64, step: ProxyNodeMetricsStep) -> u64 {
    let size = step.bucket_size_secs();
    timestamp_unix_secs / size * size
}

pub fn build_tunnel_metrics_sample(
    previous_proxy_metadata: Option<&Value>,
    current_proxy_metadata: Option<&Value>,
    active_connections: i32,
    tunnel_connected: bool,
) -> Option<TunnelMetricsSample> {
    let current = extract_tunnel_metrics_counters(current_proxy_metadata)?;
    let previous = extract_tunnel_metrics_counters(previous_proxy_metadata);
    let current_recent_errors = extract_recent_tunnel_errors(current_proxy_metadata);

    let connect_errors_delta =
        counter_delta_u64(previous.map(|v| v.connect_errors), current.connect_errors);
    let disconnects_delta = counter_delta_u64(previous.map(|v| v.disconnects), current.disconnects);
    let error_events_delta = counter_delta_u64(
        previous.map(|v| v.error_events_total),
        current.error_events_total,
    );
    let ws_in_bytes_delta = counter_delta_u64(previous.map(|v| v.ws_in_bytes), current.ws_in_bytes);
    let ws_out_bytes_delta =
        counter_delta_u64(previous.map(|v| v.ws_out_bytes), current.ws_out_bytes);
    let ws_in_frames_delta =
        counter_delta_u64(previous.map(|v| v.ws_in_frames), current.ws_in_frames);
    let ws_out_frames_delta =
        counter_delta_u64(previous.map(|v| v.ws_out_frames), current.ws_out_frames);

    let take_recent = usize::try_from(error_events_delta).unwrap_or(usize::MAX);
    let recent_error_events = if take_recent == 0 {
        Vec::new()
    } else {
        let capture = take_recent.min(current_recent_errors.len());
        let from = current_recent_errors.len().saturating_sub(capture);
        current_recent_errors[from..].to_vec()
    };

    let active_connections = i64::from(active_connections.max(0));
    let heartbeat_rtt_last_ms = i64::try_from(current.heartbeat_rtt_last_ms).unwrap_or(i64::MAX);

    Some(TunnelMetricsSample {
        samples: 1,
        uptime_samples: if tunnel_connected { 1 } else { 0 },
        active_connections_sum: active_connections,
        active_connections_max: active_connections,
        heartbeat_rtt_ms_sum: heartbeat_rtt_last_ms,
        heartbeat_rtt_ms_max: heartbeat_rtt_last_ms,
        connect_errors_delta: i64::try_from(connect_errors_delta).unwrap_or(i64::MAX),
        disconnects_delta: i64::try_from(disconnects_delta).unwrap_or(i64::MAX),
        error_events_delta: i64::try_from(error_events_delta).unwrap_or(i64::MAX),
        ws_in_bytes_delta: i64::try_from(ws_in_bytes_delta).unwrap_or(i64::MAX),
        ws_out_bytes_delta: i64::try_from(ws_out_bytes_delta).unwrap_or(i64::MAX),
        ws_in_frames_delta: i64::try_from(ws_in_frames_delta).unwrap_or(i64::MAX),
        ws_out_frames_delta: i64::try_from(ws_out_frames_delta).unwrap_or(i64::MAX),
        recent_error_events,
    })
}

pub fn build_tunnel_error_event_detail(event: &TunnelErrorEventRecord) -> String {
    format!(
        "[{}] {}",
        event.category,
        event.summary.as_deref().unwrap_or(event.message.as_str())
    )
}

pub fn log_reported_tunnel_error_event(
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

pub fn preserve_proxy_metadata_tunnel_security(
    previous_proxy_metadata: Option<&Value>,
    next_proxy_metadata: Option<Value>,
) -> Option<Value> {
    let Some(tunnel_security) = previous_proxy_metadata
        .and_then(|value| value.get("tunnel_security"))
        .filter(|value| value.is_object())
        .cloned()
    else {
        return next_proxy_metadata;
    };

    match next_proxy_metadata {
        Some(Value::Object(mut metadata)) => {
            metadata
                .entry("tunnel_security".to_string())
                .or_insert(tunnel_security);
            Some(Value::Object(metadata))
        }
        Some(value) => Some(value),
        None => {
            let mut metadata = serde_json::Map::new();
            metadata.insert("tunnel_security".to_string(), tunnel_security);
            Some(Value::Object(metadata))
        }
    }
}

fn extract_tunnel_metrics_counters(
    proxy_metadata: Option<&Value>,
) -> Option<TunnelMetricsCounters> {
    let tunnel_metrics = proxy_metadata
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("tunnel_metrics"))
        .and_then(Value::as_object)?;

    Some(TunnelMetricsCounters {
        connect_errors: json_u64(tunnel_metrics.get("connect_errors")).unwrap_or(0),
        disconnects: json_u64(tunnel_metrics.get("disconnects")).unwrap_or(0),
        error_events_total: json_u64(tunnel_metrics.get("error_events_total")).unwrap_or(0),
        ws_in_bytes: json_u64(tunnel_metrics.get("ws_in_bytes")).unwrap_or(0),
        ws_out_bytes: json_u64(tunnel_metrics.get("ws_out_bytes")).unwrap_or(0),
        ws_in_frames: json_u64(tunnel_metrics.get("ws_in_frames")).unwrap_or(0),
        ws_out_frames: json_u64(tunnel_metrics.get("ws_out_frames")).unwrap_or(0),
        heartbeat_rtt_last_ms: json_u64(tunnel_metrics.get("heartbeat_rtt_last_ms")).unwrap_or(0),
    })
}

fn extract_recent_tunnel_errors(proxy_metadata: Option<&Value>) -> Vec<TunnelErrorEventRecord> {
    proxy_metadata
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("recent_tunnel_errors"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let item = item.as_object()?;
                    Some(TunnelErrorEventRecord {
                        timestamp_unix_secs: json_u64(item.get("timestamp_unix_secs"))
                            .unwrap_or_default(),
                        timestamp_unix_ms: json_u64(item.get("timestamp_unix_ms")),
                        category: item
                            .get("category")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        message: item
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("n/a")
                            .to_string(),
                        severity: json_string(item.get("severity")),
                        component: json_string(item.get("component")),
                        summary: json_string(item.get("summary")),
                        operator_action: json_string(item.get("operator_action")),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn json_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|n| (n >= 0).then_some(n as u64)))
    })
}

fn json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn counter_delta_u64(previous: Option<u64>, current: u64) -> u64 {
    match previous {
        Some(previous) if current >= previous => current - previous,
        Some(_) => current,
        None => 0,
    }
}

fn normalize_proxy_version_label(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(
        trimmed
            .strip_prefix("tunnel-v")
            .or_else(|| trimmed.strip_prefix("proxy-v"))
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

    async fn list_proxy_node_events_filtered(
        &self,
        node_id: &str,
        query: &ProxyNodeEventQuery,
    ) -> Result<Vec<StoredProxyNodeEvent>, crate::DataLayerError> {
        let mut items = self.list_proxy_node_events(node_id, query.limit).await?;
        if let Some(from_unix_secs) = query.from_unix_secs {
            items.retain(|item| item.created_at_unix_ms.unwrap_or(0) >= from_unix_secs);
        }
        if let Some(to_unix_secs) = query.to_unix_secs {
            items.retain(|item| item.created_at_unix_ms.unwrap_or(u64::MAX) <= to_unix_secs);
        }
        if let Some(event_type) = query.event_type.as_deref() {
            items.retain(|item| item.event_type.eq_ignore_ascii_case(event_type));
        }
        items.truncate(query.limit);
        Ok(items)
    }

    async fn list_proxy_node_metrics(
        &self,
        node_id: &str,
        step: ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeMetricsBucket>, crate::DataLayerError>;

    async fn list_proxy_fleet_metrics(
        &self,
        step: ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyFleetMetricsBucket>, crate::DataLayerError>;
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

    async fn record_traffic(
        &self,
        mutation: &ProxyNodeTrafficMutation,
    ) -> Result<bool, crate::DataLayerError>;

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

    async fn cleanup_proxy_node_metrics(
        &self,
        retain_1m_from_unix_secs: u64,
        retain_1h_from_unix_secs: u64,
        delete_limit: usize,
    ) -> Result<ProxyNodeMetricsCleanupSummary, crate::DataLayerError>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        bucket_start_unix_secs, build_tunnel_error_event_detail, build_tunnel_metrics_sample,
        normalize_proxy_node_scheduling_state, preserve_proxy_metadata_tunnel_security,
        proxy_node_accepts_new_tunnels, proxy_reported_version,
        reconcile_remote_config_after_heartbeat, remote_config_scheduling_state,
        remote_config_upgrade_target, ProxyNodeMetricsStep, StoredProxyNode,
    };

    #[test]
    fn normalizes_reported_versions_and_clears_completed_upgrade_targets() {
        let remote_config = json!({
            "node_name": "edge-1",
            "upgrade_to": "tunnel-v2.0.0",
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
            reconcile_remote_config_after_heartbeat(Some(&remote_config), Some("tunnel-v2.0.0"))
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

    #[test]
    fn builds_tunnel_metrics_sample_with_reset_safe_counter_deltas() {
        let previous = json!({
            "tunnel_metrics": {
                "connect_errors": 10,
                "disconnects": 5,
                "error_events_total": 7,
                "ws_in_bytes": 1_000,
                "ws_out_bytes": 2_000,
                "ws_in_frames": 10,
                "ws_out_frames": 20,
                "heartbeat_rtt_last_ms": 30
            }
        });
        let current = json!({
            "tunnel_metrics": {
                "connect_errors": 12,
                "disconnects": 2,
                "error_events_total": 9,
                "ws_in_bytes": 1_500,
                "ws_out_bytes": 100,
                "ws_in_frames": 11,
                "ws_out_frames": 3,
                "heartbeat_rtt_last_ms": 44
            },
            "recent_tunnel_errors": [
                {"timestamp_unix_secs": 100, "category": "older", "message": "old"},
                {
                    "timestamp_unix_secs": 101,
                    "timestamp_unix_ms": 101_999,
                    "category": "newer",
                    "message": "new",
                    "severity": "error",
                    "component": "tunnel_write",
                    "summary": "WebSocket write failed because the peer closed or reset the connection",
                    "operator_action": "Check gateway restarts and network resets."
                }
            ]
        });

        let sample = build_tunnel_metrics_sample(Some(&previous), Some(&current), 4, true)
            .expect("sample should build");
        assert_eq!(sample.samples, 1);
        assert_eq!(sample.uptime_samples, 1);
        assert_eq!(sample.active_connections_sum, 4);
        assert_eq!(sample.heartbeat_rtt_ms_sum, 44);
        assert_eq!(sample.connect_errors_delta, 2);
        assert_eq!(sample.disconnects_delta, 2);
        assert_eq!(sample.error_events_delta, 2);
        assert_eq!(sample.ws_in_bytes_delta, 500);
        assert_eq!(sample.ws_out_bytes_delta, 100);
        assert_eq!(sample.ws_out_frames_delta, 3);
        assert_eq!(sample.recent_error_events.len(), 2);
        assert_eq!(sample.recent_error_events[0].category, "older");
        assert_eq!(sample.recent_error_events[0].summary, None);
        assert_eq!(
            sample.recent_error_events[1].severity.as_deref(),
            Some("error")
        );
        assert_eq!(
            sample.recent_error_events[1].component.as_deref(),
            Some("tunnel_write")
        );
        assert_eq!(
            sample.recent_error_events[1].timestamp_unix_ms,
            Some(101_999)
        );
        assert_eq!(
            build_tunnel_error_event_detail(&sample.recent_error_events[1]),
            "[newer] WebSocket write failed because the peer closed or reset the connection"
        );
    }

    #[test]
    fn builds_tunnel_metrics_sample_uses_first_counter_report_as_baseline() {
        let current = json!({
            "tunnel_metrics": {
                "connect_errors": 12,
                "disconnects": 5,
                "error_events_total": 7,
                "ws_in_bytes": 1_500,
                "ws_out_bytes": 2_500,
                "ws_in_frames": 15,
                "ws_out_frames": 25,
                "heartbeat_rtt_last_ms": 44
            },
            "recent_tunnel_errors": [
                {"timestamp_unix_secs": 101, "category": "newer", "message": "new"}
            ]
        });

        let sample = build_tunnel_metrics_sample(None, Some(&current), 4, true)
            .expect("sample should build");

        assert_eq!(sample.samples, 1);
        assert_eq!(sample.heartbeat_rtt_ms_sum, 44);
        assert_eq!(sample.connect_errors_delta, 0);
        assert_eq!(sample.disconnects_delta, 0);
        assert_eq!(sample.error_events_delta, 0);
        assert_eq!(sample.ws_in_bytes_delta, 0);
        assert_eq!(sample.ws_out_bytes_delta, 0);
        assert_eq!(sample.ws_in_frames_delta, 0);
        assert_eq!(sample.ws_out_frames_delta, 0);
        assert!(sample.recent_error_events.is_empty());
    }

    #[test]
    fn preserves_secure_tunnel_metadata_across_heartbeat_metadata_refresh() {
        let previous = json!({
            "version": "1.0.0",
            "tunnel_security": {
                "mode": "non_tls_required",
                "encryption_key": "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
            }
        });
        let next = json!({
            "version": "1.0.1",
            "tunnel_metrics": {"connect_successes": 1}
        });

        let merged = preserve_proxy_metadata_tunnel_security(Some(&previous), Some(next))
            .expect("metadata should remain present");
        assert_eq!(
            merged
                .pointer("/tunnel_security/mode")
                .and_then(|v| v.as_str()),
            Some("non_tls_required")
        );
        assert_eq!(
            merged
                .pointer("/tunnel_security/encryption_key")
                .and_then(|v| v.as_str()),
            Some("BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=")
        );
        assert_eq!(
            merged.pointer("/tunnel_metrics/connect_successes"),
            Some(&json!(1))
        );
    }

    #[test]
    fn maps_timestamps_to_metric_buckets() {
        assert_eq!(
            bucket_start_unix_secs(1_710_000_119, ProxyNodeMetricsStep::OneMinute),
            1_710_000_060
        );
        assert_eq!(
            bucket_start_unix_secs(1_710_003_999, ProxyNodeMetricsStep::OneHour),
            1_710_003_600
        );
    }
}
