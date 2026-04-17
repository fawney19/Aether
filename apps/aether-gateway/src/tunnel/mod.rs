mod embedded;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aether_contracts::tunnel::{
    TUNNEL_RELAY_FORWARDED_BY_HEADER, TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
};
use aether_data::repository::proxy_nodes::{
    ProxyNodeHeartbeatMutation, ProxyNodeTunnelStatusMutation, StoredProxyNode,
};
use aether_runtime::MetricSample;
use async_stream::stream;
use axum::body::{Body, Bytes};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::warn;

use self::embedded::{AppState as TunnelAppState, ConnConfig, ControlPlaneClient};
use super::api::response::{build_client_response, build_local_http_error_response};
use super::constants::TRACE_ID_HEADER;
use super::data::GatewayDataState;
use super::error::GatewayError;
use super::headers::{extract_or_generate_trace_id, should_skip_request_header};
use super::AppState;

pub(crate) use embedded::DirectRelayResponse;
pub(crate) use embedded::ProxyConn as TunnelProxyConn;
pub use embedded::{
    build_router_with_state as build_tunnel_runtime_router_with_state, protocol as tunnel_protocol,
    AppState as TunnelRuntimeState, ConnConfig as TunnelConnConfig,
    ControlPlaneClient as TunnelControlPlaneClient,
};

pub(crate) const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
pub(crate) const TUNNEL_HEARTBEAT_PATH: &str = "/api/internal/tunnel/heartbeat";
pub(crate) const TUNNEL_NODE_STATUS_PATH: &str = "/api/internal/tunnel/node-status";
pub(crate) const TUNNEL_RELAY_PATH_PATTERN: &str = "/api/internal/tunnel/relay/{node_id}";
pub(crate) const TUNNEL_ROUTE_FAMILY: &str = "tunnel_manage";

const DEFAULT_PROXY_IDLE_TIMEOUT_MS: u64 = 0;
const DEFAULT_PING_INTERVAL_MS: u64 = 15_000;
const DEFAULT_MAX_STREAMS: usize = 2048;
const DEFAULT_OUTBOUND_QUEUE_CAPACITY: usize = 512;
const DEFAULT_ATTACHMENT_TTL_SECS: u64 = 90;
const DEFAULT_OWNER_RELAY_BODY_LIMIT_BYTES: usize = 5_242_880;
const DEFAULT_TUNNEL_PROBE_BODY_LIMIT_BYTES: usize = 64 * 1024;
const TUNNEL_ATTACHMENT_KEY_PREFIX: &str = "tunnel.attachments.";
const TUNNEL_ATTACHMENT_REDIS_KEY_PREFIX: &str = "tunnel:attachments:";
const TUNNEL_INSTANCE_ID_ENV: &str = "AETHER_GATEWAY_INSTANCE_ID";
const TUNNEL_RELAY_BASE_URL_ENV: &str = "AETHER_TUNNEL_RELAY_BASE_URL";
const TUNNEL_ATTACHMENT_TTL_ENV: &str = "AETHER_TUNNEL_ATTACHMENT_TTL_SECS";

#[derive(Debug, Deserialize)]
struct InternalTunnelHeartbeatRequest {
    node_id: String,
    heartbeat_id: u64,
    #[serde(default)]
    heartbeat_interval: Option<i32>,
    #[serde(default)]
    active_connections: Option<i32>,
    #[serde(default)]
    total_requests: Option<i64>,
    #[serde(default)]
    avg_latency_ms: Option<f64>,
    #[serde(default)]
    failed_requests: Option<i64>,
    #[serde(default)]
    dns_failures: Option<i64>,
    #[serde(default)]
    stream_errors: Option<i64>,
    #[serde(default)]
    proxy_metadata: Option<serde_json::Value>,
    #[serde(default)]
    proxy_version: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TunnelInstanceIdentity {
    instance_id: String,
    relay_base_url: Option<String>,
    attachment_ttl_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TunnelAttachmentRecord {
    pub(crate) gateway_instance_id: String,
    pub(crate) relay_base_url: String,
    pub(crate) conn_count: usize,
    pub(crate) observed_at_unix_secs: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct TunnelAttachmentDirectory {
    identity: Arc<TunnelInstanceIdentity>,
}

impl TunnelAttachmentDirectory {
    fn from_environment() -> Self {
        Self {
            identity: Arc::new(TunnelInstanceIdentity {
                instance_id: resolve_tunnel_instance_id(),
                relay_base_url: std::env::var(TUNNEL_RELAY_BASE_URL_ENV)
                    .ok()
                    .and_then(|value| normalize_relay_base_url(&value)),
                attachment_ttl_secs: std::env::var(TUNNEL_ATTACHMENT_TTL_ENV)
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(|value| value.clamp(15, 3600))
                    .unwrap_or(DEFAULT_ATTACHMENT_TTL_SECS),
            }),
        }
    }

    pub(crate) fn from_parts(
        instance_id: impl Into<String>,
        relay_base_url: Option<impl Into<String>>,
        attachment_ttl_secs: u64,
    ) -> Self {
        Self {
            identity: Arc::new(TunnelInstanceIdentity {
                instance_id: instance_id.into(),
                relay_base_url: relay_base_url.map(Into::into),
                attachment_ttl_secs,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        instance_id: &str,
        relay_base_url: Option<&str>,
        attachment_ttl_secs: u64,
    ) -> Self {
        Self::from_parts(instance_id, relay_base_url, attachment_ttl_secs)
    }

    fn local_instance_id(&self) -> &str {
        &self.identity.instance_id
    }

    async fn refresh_from_heartbeat(
        &self,
        data: &GatewayDataState,
        request_body: &[u8],
    ) -> Result<(), String> {
        let payload = parse_embedded_tunnel_heartbeat_request(request_body)?;
        let node_id = payload.node_id.trim();
        let Some(node) = data
            .find_proxy_node(node_id)
            .await
            .map_err(|err| format!("attachment owner lookup failed: {err}"))?
        else {
            return Ok(());
        };
        if !node.tunnel_connected {
            return Ok(());
        }

        let Some(relay_base_url) = self.identity.relay_base_url.as_ref() else {
            return Ok(());
        };
        let conn_count = self
            .read_attachment_record(data, node_id)
            .await?
            .map(|record| record.conn_count)
            .unwrap_or(1);
        self.write_attachment_record(
            data,
            node_id,
            &TunnelAttachmentRecord {
                gateway_instance_id: self.identity.instance_id.clone(),
                relay_base_url: relay_base_url.clone(),
                conn_count,
                observed_at_unix_secs: current_unix_secs(),
            },
        )
        .await
    }

    async fn sync_node_status(
        &self,
        data: &GatewayDataState,
        node_id: &str,
        connected: bool,
        conn_count: usize,
        observed_at_unix_secs: u64,
    ) -> Result<(), String> {
        let node_id = node_id.trim();
        if node_id.is_empty() {
            return Ok(());
        }
        if !connected || conn_count == 0 {
            self.delete_attachment_record(data, node_id).await?;
            return Ok(());
        }

        let Some(relay_base_url) = self.identity.relay_base_url.as_ref() else {
            return Ok(());
        };
        self.write_attachment_record(
            data,
            node_id,
            &TunnelAttachmentRecord {
                gateway_instance_id: self.identity.instance_id.clone(),
                relay_base_url: relay_base_url.clone(),
                conn_count,
                observed_at_unix_secs,
            },
        )
        .await
    }

    async fn lookup_owner(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<Option<TunnelAttachmentRecord>, String> {
        let Some(record) = self.read_attachment_record(data, node_id).await? else {
            return Ok(None);
        };
        let is_expired = record
            .observed_at_unix_secs
            .saturating_add(self.identity.attachment_ttl_secs)
            < current_unix_secs();
        if is_expired || record.relay_base_url.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(record))
    }

    async fn clear_local_attachment_if_stale(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<(), String> {
        let Some(record) = self.read_attachment_record(data, node_id).await? else {
            return Ok(());
        };
        if record.gateway_instance_id == self.identity.instance_id {
            self.delete_attachment_record(data, node_id).await?;
        }
        Ok(())
    }

    async fn read_attachment_record(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<Option<TunnelAttachmentRecord>, String> {
        match self.read_attachment_record_from_redis(data, node_id).await {
            Ok(Some(record)) => return Ok(Some(record)),
            Ok(None) => {}
            Err(error) => {
                warn!(
                    error = %error,
                    node_id = %node_id,
                    "failed to read tunnel attachment from redis; falling back to system_config"
                );
            }
        }
        self.read_attachment_record_from_system_config(data, node_id)
            .await
    }

    async fn read_attachment_record_from_redis(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<Option<TunnelAttachmentRecord>, String> {
        let Some(runner) = data.kv_runner() else {
            return Ok(None);
        };
        let mut connection = runner
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| format!("attachment redis connect failed: {err}"))?;
        let namespaced_key = runner.keyspace().key(&tunnel_attachment_redis_key(node_id));
        let raw = redis::cmd("GET")
            .arg(&namespaced_key)
            .query_async::<Option<String>>(&mut connection)
            .await
            .map_err(|err| format!("attachment redis read failed: {err}"))?;
        raw.map(|value| {
            serde_json::from_str::<TunnelAttachmentRecord>(&value)
                .map_err(|err| format!("invalid redis tunnel attachment record: {err}"))
        })
        .transpose()
    }

    async fn read_attachment_record_from_system_config(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<Option<TunnelAttachmentRecord>, String> {
        let Some(value) = data
            .find_system_config_value(&tunnel_attachment_key(node_id))
            .await
            .map_err(|err| format!("attachment read failed: {err}"))?
        else {
            return Ok(None);
        };
        serde_json::from_value(value)
            .map(Some)
            .map_err(|err| format!("invalid tunnel attachment record: {err}"))
    }

    async fn write_attachment_record(
        &self,
        data: &GatewayDataState,
        node_id: &str,
        record: &TunnelAttachmentRecord,
    ) -> Result<(), String> {
        let serialized = serde_json::to_string(record)
            .map_err(|err| format!("attachment serialization failed: {err}"))?;
        if let Some(runner) = data.kv_runner() {
            if let Err(error) = runner
                .setex(
                    &tunnel_attachment_redis_key(node_id),
                    &serialized,
                    Some(self.identity.attachment_ttl_secs),
                )
                .await
            {
                warn!(
                    error = %error,
                    node_id = %node_id,
                    "failed to write tunnel attachment to redis; keeping system_config shadow only"
                );
            }
        }
        let value = serde_json::to_value(record)
            .map_err(|err| format!("attachment serialization failed: {err}"))?;
        data.upsert_system_config_value(&tunnel_attachment_key(node_id), &value, None)
            .await
            .map(|_| ())
            .map_err(|err| format!("attachment write failed: {err}"))
    }

    async fn delete_attachment_record(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<(), String> {
        if let Some(runner) = data.kv_runner() {
            if let Err(error) = runner.del(&tunnel_attachment_redis_key(node_id)).await {
                warn!(
                    error = %error,
                    node_id = %node_id,
                    "failed to delete tunnel attachment from redis; clearing system_config shadow anyway"
                );
            }
        }
        data.delete_system_config_value(&tunnel_attachment_key(node_id))
            .await
            .map(|_| ())
            .map_err(|err| format!("attachment delete failed: {err}"))
    }
}

#[derive(Clone)]
pub(crate) struct EmbeddedTunnelState {
    inner: TunnelAppState,
    attachment_directory: TunnelAttachmentDirectory,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct TunnelStatsSnapshot {
    pub(crate) proxy_connections: usize,
    pub(crate) nodes: usize,
    pub(crate) active_streams: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct TunnelProbeResponse {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl EmbeddedTunnelState {
    pub(crate) fn new() -> Self {
        Self::with_data(Arc::new(GatewayDataState::disabled()))
    }

    pub(crate) fn with_data(data: Arc<GatewayDataState>) -> Self {
        Self::with_data_and_directory(data, TunnelAttachmentDirectory::from_environment())
    }

    pub(crate) fn with_data_and_identity(
        data: Arc<GatewayDataState>,
        instance_id: impl Into<String>,
        relay_base_url: Option<impl Into<String>>,
        attachment_ttl_secs: u64,
    ) -> Self {
        Self::with_data_and_directory(
            data,
            TunnelAttachmentDirectory::from_parts(instance_id, relay_base_url, attachment_ttl_secs),
        )
    }

    pub(crate) fn with_data_and_directory(
        data: Arc<GatewayDataState>,
        attachment_directory: TunnelAttachmentDirectory,
    ) -> Self {
        Self {
            inner: TunnelAppState::new(
                build_embedded_control_plane(Arc::clone(&data), attachment_directory.clone()),
                ConnConfig {
                    ping_interval: Duration::from_millis(DEFAULT_PING_INTERVAL_MS),
                    idle_timeout: Duration::from_millis(DEFAULT_PROXY_IDLE_TIMEOUT_MS),
                    outbound_queue_capacity: DEFAULT_OUTBOUND_QUEUE_CAPACITY,
                },
                DEFAULT_MAX_STREAMS,
            )
            .with_data(data),
            attachment_directory,
        }
    }

    pub(crate) fn app_state(&self) -> TunnelAppState {
        self.inner.clone()
    }

    pub(crate) fn has_local_proxy(&self, node_id: &str) -> bool {
        self.inner.hub.has_local_proxy(node_id)
    }

    pub(crate) async fn open_direct_relay_stream(
        &self,
        node_id: &str,
        meta: tunnel_protocol::RequestMeta,
        body: Bytes,
    ) -> Result<DirectRelayResponse, String> {
        embedded::open_direct_relay_stream(&self.inner, node_id, meta, body).await
    }

    pub(crate) fn request_close_all_proxies(&self) -> usize {
        self.inner.hub.request_close_all_proxies()
    }

    pub(crate) fn stats(&self) -> TunnelStatsSnapshot {
        let stats = self.inner.hub.stats();
        TunnelStatsSnapshot {
            proxy_connections: stats.proxy_connections,
            nodes: stats.nodes,
            active_streams: stats.active_streams,
        }
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        self.inner.hub.stats().to_metric_samples()
    }

    pub(crate) async fn probe_node_url(
        &self,
        node_id: &str,
        url: &str,
        timeout_secs: u64,
    ) -> Result<u16, String> {
        Ok(self
            .probe_node_url_with_response(node_id, url, timeout_secs)
            .await?
            .status)
    }

    pub(crate) async fn probe_node_url_with_response(
        &self,
        node_id: &str,
        url: &str,
        timeout_secs: u64,
    ) -> Result<TunnelProbeResponse, String> {
        let timeout_secs = timeout_secs.clamp(5, 60);
        let meta = tunnel_protocol::RequestMeta {
            method: "GET".to_string(),
            url: url.trim().to_string(),
            headers: HashMap::new(),
            timeout: timeout_secs,
            follow_redirects: Some(false),
            http1_only: false,
        };
        let stream = self.inner.hub.open_local_stream(node_id, &meta)?;
        let stream_id = stream.id;
        let result = async {
            self.inner
                .hub
                .push_local_request_body(stream_id, Bytes::new(), true)?;
            let response = stream
                .wait_headers(Duration::from_secs(timeout_secs))
                .await?;
            let Some(mut body_rx) = stream.take_body_receiver() else {
                return Err("missing tunnel probe response body receiver".to_string());
            };
            let body = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
                let mut body_bytes = Vec::new();
                while let Some(event) = body_rx.recv().await {
                    match event {
                        embedded::LocalBodyEvent::Chunk(chunk) => {
                            let next_len = body_bytes.len().saturating_add(chunk.len());
                            if next_len > DEFAULT_TUNNEL_PROBE_BODY_LIMIT_BYTES {
                                return Err(format!(
                                    "tunnel probe body exceeds {} bytes",
                                    DEFAULT_TUNNEL_PROBE_BODY_LIMIT_BYTES
                                ));
                            }
                            body_bytes.extend_from_slice(&chunk);
                        }
                        embedded::LocalBodyEvent::End => break,
                        embedded::LocalBodyEvent::Error(error) => return Err(error),
                    }
                }
                Ok::<String, String>(String::from_utf8_lossy(&body_bytes).to_string())
            })
            .await
            .map_err(|_| "timed out waiting for tunnel probe response body".to_string())??;
            Ok(TunnelProbeResponse {
                status: response.status,
                body,
            })
        }
        .await;
        self.inner
            .hub
            .cancel_local_stream(stream_id, "tunnel health probe completed");
        result
    }

    pub(crate) fn local_instance_id(&self) -> &str {
        self.attachment_directory.local_instance_id()
    }

    pub(crate) async fn lookup_attachment_owner(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<Option<TunnelAttachmentRecord>, String> {
        self.attachment_directory.lookup_owner(data, node_id).await
    }

    pub(crate) async fn clear_local_attachment_if_stale(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<(), String> {
        self.attachment_directory
            .clear_local_attachment_if_stale(data, node_id)
            .await
    }
}

impl Default for EmbeddedTunnelState {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for EmbeddedTunnelState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedTunnelState")
            .field("proxy_idle_timeout_ms", &DEFAULT_PROXY_IDLE_TIMEOUT_MS)
            .field("ping_interval_ms", &DEFAULT_PING_INTERVAL_MS)
            .field("max_streams", &DEFAULT_MAX_STREAMS)
            .field("outbound_queue_capacity", &DEFAULT_OUTBOUND_QUEUE_CAPACITY)
            .field(
                "instance_id",
                &self.attachment_directory.local_instance_id(),
            )
            .finish()
    }
}

pub(crate) async fn proxy_tunnel(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    embedded::ws_proxy(ws, State(state.tunnel.app_state()), headers).await
}

pub(crate) async fn relay_request(
    path: Path<String>,
    State(state): State<AppState>,
    connect_info: ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> Result<axum::http::Response<Body>, GatewayError> {
    let node_id = path.0;
    if state.tunnel.has_local_proxy(&node_id) {
        return Ok(embedded::relay_request(
            Path(node_id),
            State(state.tunnel.app_state()),
            connect_info,
            request,
        )
        .await
        .into_response());
    }

    let trace_id = extract_or_generate_trace_id(request.headers());
    let already_forwarded = request
        .headers()
        .get(TUNNEL_RELAY_FORWARDED_BY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());

    if already_forwarded {
        return build_local_http_error_response(
            &trace_id,
            None,
            StatusCode::SERVICE_UNAVAILABLE,
            "tunnel owner unavailable",
        );
    }

    if let Some(owner) = state
        .tunnel
        .lookup_attachment_owner(state.data.as_ref(), &node_id)
        .await
        .map_err(GatewayError::Internal)?
    {
        if owner.gateway_instance_id != state.tunnel.local_instance_id() {
            return forward_relay_request_to_owner(&state, &node_id, request, &trace_id, &owner)
                .await;
        }
        state
            .tunnel
            .clear_local_attachment_if_stale(state.data.as_ref(), &node_id)
            .await
            .map_err(GatewayError::Internal)?;
    }

    Ok(embedded::relay_request(
        Path(node_id),
        State(state.tunnel.app_state()),
        connect_info,
        request,
    )
    .await
    .into_response())
}

pub(crate) fn is_tunnel_heartbeat_path(path: &str) -> bool {
    path == TUNNEL_HEARTBEAT_PATH
}

pub(crate) fn is_tunnel_node_status_path(path: &str) -> bool {
    path == TUNNEL_NODE_STATUS_PATH
}

fn build_embedded_control_plane(
    data: Arc<GatewayDataState>,
    attachment_directory: TunnelAttachmentDirectory,
) -> ControlPlaneClient {
    let heartbeat_data = Arc::clone(&data);
    let heartbeat_directory = attachment_directory.clone();
    let node_status_data = Arc::clone(&data);
    let node_status_directory = attachment_directory;
    ControlPlaneClient::local(
        move |payload| {
            let data = Arc::clone(&heartbeat_data);
            let directory = heartbeat_directory.clone();
            Box::pin(async move {
                let ack = apply_embedded_tunnel_heartbeat(data.as_ref(), &payload).await?;
                if let Err(error) = directory
                    .refresh_from_heartbeat(data.as_ref(), &payload)
                    .await
                {
                    warn!(error = %error, "failed to refresh tunnel attachment from heartbeat");
                }
                Ok(ack)
            })
        },
        move |node_id, connected, conn_count, observed_at_unix_secs| {
            let data = Arc::clone(&node_status_data);
            let directory = node_status_directory.clone();
            Box::pin(async move {
                apply_embedded_tunnel_node_status(
                    data.as_ref(),
                    &node_id,
                    connected,
                    conn_count,
                    Some(observed_at_unix_secs),
                )
                .await?;
                if let Err(error) = directory
                    .sync_node_status(
                        data.as_ref(),
                        &node_id,
                        connected,
                        conn_count,
                        observed_at_unix_secs,
                    )
                    .await
                {
                    warn!(error = %error, node_id = %node_id, "failed to sync tunnel attachment");
                }
                Ok(())
            })
        },
    )
}

async fn forward_relay_request_to_owner(
    state: &AppState,
    node_id: &str,
    request: Request,
    trace_id: &str,
    owner: &TunnelAttachmentRecord,
) -> Result<axum::http::Response<Body>, GatewayError> {
    let owner_url = build_owner_relay_url(&owner.relay_base_url, node_id)?;
    let (parts, body) = request.into_parts();
    let body_limit = owner_relay_body_limit_bytes(state.data.as_ref()).await;
    if request_content_length_exceeds_limit(&parts.headers, body_limit) {
        return build_local_http_error_response(
            trace_id,
            None,
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!("tunnel relay body exceeds {body_limit} bytes"),
        );
    }
    let limit_exceeded = Arc::new(AtomicBool::new(false));

    let mut upstream_request = state.client.post(owner_url);
    for (name, value) in &parts.headers {
        if should_skip_request_header(name.as_str()) || name == http::header::HOST {
            continue;
        }
        upstream_request = upstream_request.header(name, value);
    }
    upstream_request = upstream_request
        .header(
            TUNNEL_RELAY_FORWARDED_BY_HEADER,
            state.tunnel.local_instance_id(),
        )
        .header(
            TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
            owner.gateway_instance_id.as_str(),
        );
    if !parts.headers.contains_key(TRACE_ID_HEADER) {
        upstream_request = upstream_request.header(TRACE_ID_HEADER, trace_id);
    }

    let upstream_response = match upstream_request
        .body(build_owner_relay_request_body(
            body,
            body_limit,
            Arc::clone(&limit_exceeded),
        ))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) if limit_exceeded.load(Ordering::SeqCst) => {
            return build_local_http_error_response(
                trace_id,
                None,
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("tunnel relay body exceeds {body_limit} bytes"),
            );
        }
        Err(err) => {
            return Err(GatewayError::Internal(format!(
                "owner tunnel relay failed: {err}"
            )));
        }
    };

    build_client_response(upstream_response, trace_id, None)
}

fn build_owner_relay_url(relay_base_url: &str, node_id: &str) -> Result<String, GatewayError> {
    let mut url = url::Url::parse(relay_base_url)
        .map_err(|err| GatewayError::Internal(format!("invalid owner relay base url: {err}")))?;
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            GatewayError::Internal("owner relay base url cannot be a base-less URL".to_string())
        })?;
        segments.pop_if_empty();
        segments.push("api");
        segments.push("internal");
        segments.push("tunnel");
        segments.push("relay");
        segments.push(node_id.trim());
    }
    Ok(url.to_string())
}

async fn owner_relay_body_limit_bytes(data: &GatewayDataState) -> usize {
    data.find_system_config_value("max_request_body_size")
        .await
        .ok()
        .flatten()
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_OWNER_RELAY_BODY_LIMIT_BYTES)
}

fn request_content_length_exceeds_limit(headers: &HeaderMap, body_limit: usize) -> bool {
    headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|value| usize::try_from(value).ok())
        .is_some_and(|value| value > body_limit)
}

fn build_owner_relay_request_body(
    body: Body,
    body_limit: usize,
    limit_exceeded: Arc<AtomicBool>,
) -> reqwest::Body {
    let mut body_stream = body.into_data_stream();
    reqwest::Body::wrap_stream(stream! {
        let mut forwarded = 0usize;
        while let Some(next_chunk) = body_stream.next().await {
            match next_chunk {
                Ok(chunk) => {
                    forwarded = forwarded.saturating_add(chunk.len());
                    if forwarded > body_limit {
                        limit_exceeded.store(true, Ordering::SeqCst);
                        yield Err::<Bytes, io::Error>(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("tunnel relay body exceeds {body_limit} bytes"),
                        ));
                        break;
                    }
                    yield Ok::<Bytes, io::Error>(chunk);
                }
                Err(err) => {
                    yield Err::<Bytes, io::Error>(io::Error::other(err));
                    break;
                }
            }
        }
    })
}

fn tunnel_attachment_key(node_id: &str) -> String {
    format!("{TUNNEL_ATTACHMENT_KEY_PREFIX}{}", node_id.trim())
}

fn tunnel_attachment_redis_key(node_id: &str) -> String {
    format!("{TUNNEL_ATTACHMENT_REDIS_KEY_PREFIX}{}", node_id.trim())
}

fn resolve_tunnel_instance_id() -> String {
    std::env::var(TUNNEL_INSTANCE_ID_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| format!("gateway-{}", std::process::id()))
}

fn normalize_relay_base_url(value: &str) -> Option<String> {
    let normalized = value.trim().trim_end_matches('/').to_string();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn apply_embedded_tunnel_heartbeat(
    data: &GatewayDataState,
    request_body: &[u8],
) -> Result<Vec<u8>, String> {
    let payload = parse_embedded_tunnel_heartbeat_request(request_body)?;
    let node_id = payload.node_id.trim().to_string();
    let mutation = ProxyNodeHeartbeatMutation {
        node_id: node_id.clone(),
        heartbeat_interval: payload.heartbeat_interval,
        active_connections: payload.active_connections,
        total_requests_delta: payload.total_requests,
        avg_latency_ms: payload.avg_latency_ms,
        failed_requests_delta: payload.failed_requests,
        dns_failures_delta: payload.dns_failures,
        stream_errors_delta: payload.stream_errors,
        proxy_metadata: payload.proxy_metadata,
        proxy_version: payload.proxy_version,
    };

    let node = data
        .apply_proxy_node_heartbeat(&mutation)
        .await
        .map_err(|err| format!("heartbeat sync failed: {err}"))?
        .ok_or_else(|| format!("heartbeat sync failed: ProxyNode {node_id} 不存在"))?;

    Ok(build_embedded_tunnel_heartbeat_ack(
        &node,
        payload.heartbeat_id,
    ))
}

async fn apply_embedded_tunnel_node_status(
    data: &GatewayDataState,
    node_id: &str,
    connected: bool,
    conn_count: usize,
    observed_at_unix_secs: Option<u64>,
) -> Result<(), String> {
    let mutation = ProxyNodeTunnelStatusMutation {
        node_id: node_id.trim().to_string(),
        connected,
        conn_count: conn_count.min(i32::MAX as usize) as i32,
        detail: None,
        observed_at_unix_secs,
    };

    data.update_proxy_node_tunnel_status(&mutation)
        .await
        .map(|_| ())
        .map_err(|err| format!("node status sync failed: {err}"))
}

fn build_embedded_tunnel_heartbeat_ack(node: &StoredProxyNode, heartbeat_id: u64) -> Vec<u8> {
    let mut payload = serde_json::Map::new();
    payload.insert("heartbeat_id".to_string(), json!(heartbeat_id));
    if let Some(remote_config) = node.remote_config.as_ref() {
        payload.insert("remote_config".to_string(), remote_config.clone());
        payload.insert("config_version".to_string(), json!(node.config_version));
        if let Some(upgrade_to) = remote_config
            .as_object()
            .and_then(|value| value.get("upgrade_to"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            payload.insert("upgrade_to".to_string(), json!(upgrade_to));
        }
    }

    serde_json::to_vec(&serde_json::Value::Object(payload)).unwrap_or_else(|_| b"{}".to_vec())
}

fn parse_embedded_tunnel_heartbeat_request(
    request_body: &[u8],
) -> Result<InternalTunnelHeartbeatRequest, String> {
    let payload = serde_json::from_slice::<InternalTunnelHeartbeatRequest>(request_body)
        .map_err(|_| "invalid heartbeat payload".to_string())?;

    let node_id = payload.node_id.trim();
    if node_id.is_empty() || node_id.len() > 36 || payload.heartbeat_id == 0 {
        return Err("invalid heartbeat payload".to_string());
    }
    if payload
        .heartbeat_interval
        .is_some_and(|value| !(5..=600).contains(&value))
        || payload.active_connections.is_some_and(|value| value < 0)
        || payload.total_requests.is_some_and(|value| value < 0)
        || payload.avg_latency_ms.is_some_and(|value| value < 0.0)
        || payload.failed_requests.is_some_and(|value| value < 0)
        || payload.dns_failures.is_some_and(|value| value < 0)
        || payload.stream_errors.is_some_and(|value| value < 0)
        || payload
            .proxy_version
            .as_deref()
            .is_some_and(|value| value.chars().count() > 20)
        || payload
            .proxy_metadata
            .as_ref()
            .is_some_and(|value| !value.is_object())
    {
        return Err("invalid heartbeat payload".to_string());
    }

    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_embedded_tunnel_heartbeat, apply_embedded_tunnel_node_status, current_unix_secs,
        tunnel_attachment_key, GatewayDataState, TunnelAttachmentDirectory, TunnelAttachmentRecord,
    };
    use aether_data::repository::proxy_nodes::{
        InMemoryProxyNodeRepository, ProxyNodeReadRepository, StoredProxyNode,
    };
    use serde_json::json;
    use std::sync::Arc;

    fn sample_proxy_node(node_id: &str) -> StoredProxyNode {
        StoredProxyNode::new(
            node_id.to_string(),
            format!("proxy-{node_id}"),
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
            7,
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
            Some(json!({
                "allowed_ports": [443],
                "upgrade_to": "1.2.3",
            })),
            Some(1_700_000_000),
            Some(1_700_000_001),
        )
    }

    #[tokio::test]
    async fn embedded_tunnel_heartbeat_updates_proxy_node_repository() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-123",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));

        let ack = apply_embedded_tunnel_heartbeat(
            &data,
            br#"{
                "node_id": "node-123",
                "heartbeat_id": 42,
                "heartbeat_interval": 45,
                "active_connections": 5,
                "total_requests": 9,
                "avg_latency_ms": 12.5,
                "failed_requests": 1,
                "dns_failures": 2,
                "stream_errors": 3,
                "proxy_metadata": {"arch": "arm64"},
                "proxy_version": "2.0.0"
            }"#,
        )
        .await
        .expect("heartbeat should succeed");

        let payload: serde_json::Value =
            serde_json::from_slice(&ack).expect("ack payload should parse");
        assert_eq!(payload["heartbeat_id"], 42);
        assert_eq!(payload["config_version"], 7);
        assert_eq!(payload["upgrade_to"], "1.2.3");
        assert_eq!(payload["remote_config"]["allowed_ports"][0], 443);

        let node = repository
            .find_proxy_node("node-123")
            .await
            .expect("lookup should succeed")
            .expect("node should exist");
        assert_eq!(node.status, "online");
        assert_eq!(node.tunnel_connected, true);
        assert_eq!(node.heartbeat_interval, 45);
        assert_eq!(node.active_connections, 5);
        assert_eq!(node.total_requests, 9);
        assert_eq!(node.failed_requests, 1);
        assert_eq!(node.dns_failures, 2);
        assert_eq!(node.stream_errors, 3);
        assert_eq!(
            node.proxy_metadata
                .as_ref()
                .and_then(|value| value.get("version"))
                .and_then(serde_json::Value::as_str),
            Some("2.0.0")
        );
    }

    #[tokio::test]
    async fn embedded_tunnel_heartbeat_rejects_missing_heartbeat_id() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-123",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));

        let error = apply_embedded_tunnel_heartbeat(
            &data,
            br#"{
                "node_id": "node-123",
                "heartbeat_interval": 45,
                "active_connections": 5
            }"#,
        )
        .await
        .expect_err("heartbeat without heartbeat_id should fail");

        assert_eq!(error, "invalid heartbeat payload");
    }

    #[tokio::test]
    async fn embedded_tunnel_node_status_updates_proxy_node_repository() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-123",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));

        apply_embedded_tunnel_node_status(&data, "node-123", true, 4, Some(1_800_000_123))
            .await
            .expect("node status should succeed");

        let node = repository
            .find_proxy_node("node-123")
            .await
            .expect("lookup should succeed")
            .expect("node should exist");
        assert_eq!(node.status, "online");
        assert_eq!(node.tunnel_connected, true);
        assert_eq!(node.tunnel_connected_at_unix_secs, Some(1_800_000_123));
    }

    #[tokio::test]
    async fn tunnel_attachment_directory_syncs_and_clears_attachment_records() {
        let data = GatewayDataState::disabled().with_system_config_values_for_tests(vec![]);
        let directory = TunnelAttachmentDirectory::for_tests(
            "gateway-a",
            Some("http://gateway-a.internal"),
            90,
        );

        directory
            .sync_node_status(&data, "node-123", true, 2, 1_800_000_010)
            .await
            .expect("attachment should sync");
        let record = directory
            .lookup_owner(&data, "node-123")
            .await
            .expect("lookup should succeed")
            .expect("attachment should exist");
        assert_eq!(record.gateway_instance_id, "gateway-a");
        assert_eq!(record.relay_base_url, "http://gateway-a.internal");
        assert_eq!(record.conn_count, 2);
        assert_eq!(record.observed_at_unix_secs, 1_800_000_010);

        directory
            .sync_node_status(&data, "node-123", false, 0, 1_800_000_011)
            .await
            .expect("attachment should clear");
        assert!(directory
            .lookup_owner(&data, "node-123")
            .await
            .expect("lookup should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn tunnel_attachment_directory_ignores_expired_attachment_records() {
        let stale = TunnelAttachmentRecord {
            gateway_instance_id: "gateway-b".to_string(),
            relay_base_url: "http://gateway-b.internal".to_string(),
            conn_count: 1,
            observed_at_unix_secs: current_unix_secs().saturating_sub(120),
        };
        let data = GatewayDataState::disabled().with_system_config_values_for_tests(vec![(
            tunnel_attachment_key("node-123"),
            serde_json::to_value(&stale).expect("record should serialize"),
        )]);
        let directory = TunnelAttachmentDirectory::for_tests(
            "gateway-a",
            Some("http://gateway-a.internal"),
            30,
        );

        assert!(directory
            .lookup_owner(&data, "node-123")
            .await
            .expect("lookup should succeed")
            .is_none());
    }
}
