mod embedded;

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use aether_contracts::tunnel::{
    resolve_tunnel_request_timeouts, sign_tunnel_relay_request,
    try_decode_tunnel_relay_request_meta, tunnel_relay_payload_digest,
    tunnel_relay_payload_digest_from_hashes, RequestMeta, TunnelRelayPayloadDigest,
    MAX_TUNNEL_RELAY_META_LEN, TUNNEL_RELAY_AUTH_NONCE_HEADER, TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
    TUNNEL_RELAY_AUTH_SENDER_HEADER, TUNNEL_RELAY_AUTH_SIGNATURE_HEADER,
    TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER, TUNNEL_RELAY_FORWARDED_BY_HEADER,
    TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
};
use aether_data::repository::proxy_nodes::{
    ProxyNodeHeartbeatMutation, ProxyNodeTunnelStatusMutation, StoredProxyNode,
};
use aether_gateway_tunnel::EmbeddedTunnelDefaults;
use aether_runtime::MetricSample;
use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeLockLease, RuntimeState};
use async_stream::stream;
use axum::body::{Body, Bytes};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use tracing::warn;

use self::embedded::{AppState as TunnelAppState, ConnConfig, ControlPlaneClient};
pub(crate) use self::embedded::{ControlPlaneAuthError, RelayAuthError};
use super::api::response::{build_client_response, build_local_http_error_response};
use super::constants::TRACE_ID_HEADER;
use super::data::GatewayDataState;
use super::error::GatewayError;
use super::headers::{extract_or_generate_trace_id, should_skip_request_header};
use super::AppState;

pub(crate) use aether_gateway_tunnel::{
    is_tunnel_heartbeat_path, is_tunnel_node_status_path, TunnelAttachmentRecord,
    DEFAULT_TUNNEL_PROBE_BODY_LIMIT_BYTES, PROXY_TUNNEL_PATH, TUNNEL_HEARTBEAT_PATH,
    TUNNEL_NODE_STATUS_PATH, TUNNEL_RELAY_PATH_PATTERN, TUNNEL_ROUTE_FAMILY,
};
pub(crate) use embedded::DirectRelayResponse;
pub(crate) use embedded::ProxyConn as TunnelProxyConn;
pub use embedded::{
    build_router_with_state as build_tunnel_runtime_router_with_state, protocol as tunnel_protocol,
    AppState as TunnelRuntimeState, ConnConfig as TunnelConnConfig,
    ControlPlaneClient as TunnelControlPlaneClient,
};

#[cfg(feature = "testkit")]
pub fn build_tunnel_pressure_router(
    state: TunnelRuntimeState,
    instance_id: &str,
    secret: &[u8],
) -> Result<axum::Router, String> {
    validate_tunnel_relay_auth_secret(secret)?;
    let directory = TunnelAttachmentDirectory::from_parts(instance_id, None::<String>, 90);
    let state = state.with_relay_auth(
        instance_id,
        Some(secret.to_vec()),
        Arc::clone(&directory.runtime_state),
    );
    let runtime_router = build_tunnel_runtime_router_with_state(state.clone());
    let mut gateway = AppState::new().map_err(|error| error.to_string())?;
    gateway.tunnel = EmbeddedTunnelState {
        inner: state,
        attachment_directory: directory,
        relay_auth_secret: Ok(Arc::from(secret)),
    };
    // HTTP relay requests must pass the same authentication and verified spool
    // preparation as the gateway before reaching the embedded local dispatcher.
    Ok(axum::Router::new()
        .route(
            TUNNEL_RELAY_PATH_PATTERN,
            axum::routing::post(relay_request),
        )
        .with_state(gateway)
        .fallback_service(runtime_router))
}

const DEFAULT_ATTACHMENT_TTL_SECS: u64 = 90;
const TUNNEL_ATTACHMENT_KEY_PREFIX: &str = "tunnel.attachments.";
const TUNNEL_ATTACHMENT_REDIS_KEY_PREFIX: &str = "tunnel:attachments:";
const TUNNEL_INSTANCE_ID_ENV: &str = "AETHER_GATEWAY_INSTANCE_ID";
const TUNNEL_RELAY_BASE_URL_ENV: &str = "AETHER_TUNNEL_RELAY_BASE_URL";
// Owner-forward relay URLs are deployment metadata, but a corrupted or
// stale attachment record must not turn the gateway into a generic internal
// network client.  Private HTTPS relay targets therefore require an explicit
// operator opt-in, just like the tunnel's private upstream target escape
// hatch.  Loopback HTTP remains supported for the local single-process path.
const TUNNEL_RELAY_ALLOW_PRIVATE_TARGETS_ENV: &str = "AETHER_TUNNEL_RELAY_ALLOW_PRIVATE_TARGETS";
// Prefer this narrow host allowlist for private owner relay deployments. The
// legacy *_ALLOW_PRIVATE_TARGETS switch remains available for operators that
// intentionally trust every private address in their deployment, but an exact
// hostname list limits the blast radius of a stale or corrupted attachment.
const TUNNEL_RELAY_PRIVATE_HOST_ALLOWLIST_ENV: &str = "AETHER_TUNNEL_RELAY_PRIVATE_HOST_ALLOWLIST";
const TUNNEL_ATTACHMENT_TTL_ENV: &str = "AETHER_TUNNEL_ATTACHMENT_TTL_SECS";
const TUNNEL_RELAY_AUTH_SECRET_ENV: &str = "AETHER_TUNNEL_RELAY_AUTH_SECRET";
const TUNNEL_HEARTBEAT_STATE_KEY_PREFIX: &str = "tunnel:heartbeat:session:";
const TUNNEL_HEARTBEAT_STATE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const TUNNEL_HEARTBEAT_LOCK_TTL: Duration = Duration::from_secs(60);
// Attachment updates touch both the shared runtime key and the durable
// system-config shadow. Keep the lease comfortably longer than the bounded
// operation so a slow database call cannot outlive the lock and race a new
// owner. RuntimeState also supports lock renewal, but these short operations
// are deliberately cancelled before renewal is needed.
const TUNNEL_ATTACHMENT_LOCK_TTL: Duration = Duration::from_secs(60);
const TUNNEL_ATTACHMENT_LOCK_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const TUNNEL_ATTACHMENT_OPERATION_TIMEOUT: Duration = Duration::from_secs(45);
const TUNNEL_ATTACHMENT_LOCK_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
const TUNNEL_RELAY_BODY_READ_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_TUNNEL_RELAY_MAX_BODY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TUNNEL_RELAY_BODY_BYTES: u64 = 1024 * 1024 * 1024;
const TUNNEL_RELAY_MAX_BODY_MB_ENV: &str = "AETHER_TUNNEL_RELAY_MAX_BODY_MB";
const DEFAULT_TUNNEL_RELAY_SPOOL_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_TUNNEL_RELAY_SPOOL_BUDGET_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const TUNNEL_RELAY_SPOOL_BUDGET_MB_ENV: &str = "AETHER_TUNNEL_RELAY_SPOOL_BUDGET_MB";
static TUNNEL_RELAY_SPOOL_BYTES_IN_USE: AtomicU64 = AtomicU64::new(0);
pub(crate) const TUNNEL_RELAY_AUTH_SECRET_MIN_BYTES: usize = 32;
pub(crate) const TUNNEL_RELAY_ROLLOUT_PROBE_HEADER: &str = "x-aether-tunnel-rollout-probe";
pub(crate) const TUNNEL_RELAY_ROLLOUT_PROBE_VALUE: &str = "1";
const TUNNEL_AFFINITY_AUTH_CONTEXT: &str = "aether.tunnel.affinity-auth.v1";
const TUNNEL_AFFINITY_GATEWAY_MARKER: &str = "rust-phase3b-affinity";
const MAX_TUNNEL_AFFINITY_ID_LEN: usize = 200;
const OWNER_FORWARD_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OWNER_FORWARD_DNS_ADDRESSES: usize = 32;
// This bounds cached DNS answer/client combinations, not concurrent requests or HTTP/2 streams.
const MAX_OWNER_FORWARD_PINNED_CLIENT_CACHE_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OwnerForwardDnsPinKey {
    scheme: String,
    host: String,
    port: u16,
    addresses: Vec<SocketAddr>,
}

struct OwnerForwardPinnedClientCacheEntry {
    client: reqwest::Client,
    last_used: u64,
}

fn owner_forward_pinned_client_cache(
) -> &'static Mutex<HashMap<OwnerForwardDnsPinKey, OwnerForwardPinnedClientCacheEntry>> {
    static CACHE: OnceLock<
        Mutex<HashMap<OwnerForwardDnsPinKey, OwnerForwardPinnedClientCacheEntry>>,
    > = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owner_forward_pinned_client_cache_clock() -> &'static AtomicU64 {
    static CLOCK: OnceLock<AtomicU64> = OnceLock::new();
    CLOCK.get_or_init(|| AtomicU64::new(0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TunnelAffinityAuthContext {
    pub(crate) client_ip: std::net::IpAddr,
}

pub(crate) struct PendingTunnelAffinityAuth {
    context: TunnelAffinityAuthContext,
    relay_auth: embedded::PendingRelayAuth,
}

pub(crate) async fn send_owner_forward_request(
    request: reqwest::RequestBuilder,
    first_byte_timeout: Option<Duration>,
) -> Result<reqwest::Response, String> {
    match first_byte_timeout {
        Some(timeout) => match tokio::time::timeout(timeout, request.send()).await {
            Ok(result) => result.map_err(|error| owner_forward_request_error(&error)),
            Err(_) => Err(format!(
                "owner gateway first byte timeout after {} ms",
                timeout.as_millis()
            )),
        },
        None => request
            .send()
            .await
            .map_err(|error| owner_forward_request_error(&error)),
    }
}

/// Project reqwest failures at the relay boundary.  `reqwest::Error::to_string`
/// can include the complete request URL (including credentials, query tokens,
/// or an internal host), so it must never be copied into a client-facing
/// `GatewayError` or admin probe response.
pub(crate) fn owner_forward_request_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "owner gateway request timed out".to_string();
    }
    if error.is_connect() {
        return "owner gateway connection failed".to_string();
    }
    if error.is_redirect() {
        return "owner gateway redirect was rejected".to_string();
    }
    if error.is_body() {
        return "owner gateway request body failed".to_string();
    }
    if error.is_decode() {
        return "owner gateway response decode failed".to_string();
    }
    "owner gateway request failed".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedOwnerForwardTarget {
    scheme: String,
    host: String,
    port: u16,
    addresses: Vec<SocketAddr>,
    literal_host: bool,
}

/// Resolve an owner URL once and return the exact addresses that must be used
/// for the ensuing request.  A normal reqwest client performs another DNS
/// lookup when it opens a connection; that lookup can observe a different
/// answer after a validation pass (DNS rebinding).  Callers use this result to
/// install a `resolve_to_addrs` override on the client that sends the request.
async fn resolve_owner_forward_target(
    owner_url: &str,
) -> Result<ResolvedOwnerForwardTarget, String> {
    let url = url::Url::parse(owner_url.trim())
        .map_err(|error| format!("invalid owner gateway URL: {error}"))?;
    validate_tunnel_relay_transport_url(&url)?;
    if url.fragment().is_some() {
        return Err("owner gateway URL must not include a fragment".to_string());
    }
    let host = match url
        .host()
        .ok_or_else(|| "owner gateway URL must include a host".to_string())?
    {
        url::Host::Domain(host) => host.trim().to_string(),
        url::Host::Ipv4(address) => address.to_string(),
        url::Host::Ipv6(address) => address.to_string(),
    };
    if host.is_empty() {
        return Err("owner gateway URL must include a host".to_string());
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "owner gateway URL must include a port".to_string())?;
    let addresses = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        let resolved = tokio::time::timeout(
            OWNER_FORWARD_DNS_LOOKUP_TIMEOUT,
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| "owner gateway DNS resolution timed out".to_string())?
        .map_err(|_| "owner gateway DNS resolution failed".to_string())?;
        // Keep the resolver iterator bounded before collecting it.  A hostile
        // or misconfigured resolver must not be able to force an unbounded
        // address vector before the target policy gets a chance to inspect it.
        resolved
            .take(MAX_OWNER_FORWARD_DNS_ADDRESSES.saturating_add(1))
            .collect()
    };
    build_owner_forward_target(url.scheme(), &host, port, addresses)
}

fn build_owner_forward_target(
    scheme: &str,
    host: &str,
    port: u16,
    addresses: Vec<SocketAddr>,
) -> Result<ResolvedOwnerForwardTarget, String> {
    build_owner_forward_target_with_policy(
        scheme,
        host,
        port,
        addresses,
        tunnel_relay_allows_private_targets() || tunnel_relay_private_host_is_allowlisted(host),
    )
}

fn build_owner_forward_target_with_policy(
    scheme: &str,
    host: &str,
    port: u16,
    mut addresses: Vec<SocketAddr>,
    allow_private_targets: bool,
) -> Result<ResolvedOwnerForwardTarget, String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("owner gateway URL must include a host".to_string());
    }
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err("owner gateway URL must use HTTP or HTTPS".to_string());
    }
    if addresses.len() > MAX_OWNER_FORWARD_DNS_ADDRESSES {
        return Err(format!(
            "owner gateway DNS resolution returned too many addresses (maximum {})",
            MAX_OWNER_FORWARD_DNS_ADDRESSES
        ));
    }
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err("owner gateway DNS resolution returned no addresses".to_string());
    }
    if addresses.iter().any(|address| address.port() != port) {
        return Err(
            "owner gateway DNS resolution returned an address with the wrong port".to_string(),
        );
    }
    if scheme.eq_ignore_ascii_case("http")
        && addresses.iter().any(|address| !address.ip().is_loopback())
    {
        return Err("loopback HTTP owner gateway resolved to a non-loopback address".to_string());
    }
    if !allow_private_targets
        && addresses.iter().any(|address| {
            aether_http::is_private_or_reserved_ip(address.ip())
                // The existing local-development exception is deliberately
                // narrow: HTTPS never gets an implicit loopback/private
                // exemption, while literal/localhost HTTP is checked above.
                && !(scheme.eq_ignore_ascii_case("http") && address.ip().is_loopback())
        })
    {
        return Err(format!(
            "owner gateway DNS resolution returned a private or reserved address; set {TUNNEL_RELAY_ALLOW_PRIVATE_TARGETS_ENV}=true only for a trusted internal relay"
        ));
    }

    Ok(ResolvedOwnerForwardTarget {
        scheme: scheme.to_ascii_lowercase(),
        host: host.to_string(),
        port,
        addresses,
        literal_host: host.parse::<std::net::IpAddr>().is_ok(),
    })
}

fn tunnel_relay_allows_private_targets() -> bool {
    std::env::var(TUNNEL_RELAY_ALLOW_PRIVATE_TARGETS_ENV)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

/// Return whether a private owner relay hostname was explicitly allowlisted.
///
/// Entries are comma-separated exact DNS names (case-insensitive); a trailing
/// dot is ignored for DNS canonicalisation.  We intentionally do not support
/// arbitrary suffix or wildcard matching: allowing `.internal` would let a
/// compromised attachment redirect traffic to any sibling service in that
/// namespace.  The broad `*_ALLOW_PRIVATE_TARGETS=true` switch above remains
/// the explicit escape hatch for deployments that need that behaviour.
fn tunnel_relay_private_host_is_allowlisted(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    std::env::var(TUNNEL_RELAY_PRIVATE_HOST_ALLOWLIST_ENV)
        .ok()
        .is_some_and(|value| tunnel_relay_private_host_matches_allowlist(&host, &value))
}

fn tunnel_relay_private_host_matches_allowlist(host: &str, allowlist: &str) -> bool {
    let host = host.trim().trim_end_matches('.');
    !host.is_empty()
        && allowlist.split(',').any(|entry| {
            let entry = entry.trim().trim_end_matches('.');
            !entry.is_empty() && entry.eq_ignore_ascii_case(host)
        })
}

fn build_owner_forward_pinned_client(
    target: &ResolvedOwnerForwardTarget,
) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(OWNER_FORWARD_DNS_LOOKUP_TIMEOUT)
        .http2_adaptive_window(true)
        .resolve_to_addrs(&target.host, &target.addresses)
        .build()
        .map_err(|_| "failed to build pinned owner gateway client".to_string())
}

/// Return a client whose DNS answer is fixed to the addresses resolved for
/// `owner_url`.  Literal IP hosts are already immune to DNS rebinding and keep
/// using the configured shared client so test/deployment-specific client
/// settings (for example a deliberately short timeout) remain intact.
pub(crate) async fn owner_forward_client_for_url(
    base_client: &reqwest::Client,
    owner_url: &str,
) -> Result<reqwest::Client, String> {
    let target = resolve_owner_forward_target(owner_url).await?;
    if target.literal_host {
        return Ok(base_client.clone());
    }

    let key = OwnerForwardDnsPinKey {
        scheme: target.scheme,
        host: target.host,
        port: target.port,
        addresses: target.addresses,
    };
    let clock = owner_forward_pinned_client_cache_clock().fetch_add(1, Ordering::Relaxed);
    if let Ok(mut cache) = owner_forward_pinned_client_cache().lock() {
        if let Some(entry) = cache.get_mut(&key) {
            entry.last_used = clock;
            return Ok(entry.client.clone());
        }
    }

    let client = build_owner_forward_pinned_client(&ResolvedOwnerForwardTarget {
        scheme: key.scheme.clone(),
        host: key.host.clone(),
        port: key.port,
        addresses: key.addresses.clone(),
        literal_host: false,
    })?;
    let mut cache = owner_forward_pinned_client_cache()
        .lock()
        .map_err(|_| "owner gateway DNS pin cache is unavailable".to_string())?;
    if let Some(entry) = cache.get_mut(&key) {
        entry.last_used = clock;
        return Ok(entry.client.clone());
    }
    if cache.len() >= MAX_OWNER_FORWARD_PINNED_CLIENT_CACHE_ENTRIES {
        if let Some(oldest_key) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        {
            cache.remove(&oldest_key);
        }
    }
    cache.insert(
        key,
        OwnerForwardPinnedClientCacheEntry {
            client: client.clone(),
            last_used: clock,
        },
    );
    Ok(client)
}

#[derive(Debug, Deserialize)]
struct InternalTunnelHeartbeatRequest {
    node_id: String,
    heartbeat_session_id: String,
    heartbeat_id: u64,
    #[serde(default)]
    heartbeat_interval: Option<i32>,
    #[serde(default)]
    active_connections: Option<i32>,
    #[serde(default)]
    total_requests: Option<i64>,
    #[serde(default)]
    window_total_requests: Option<i64>,
    #[serde(default)]
    avg_latency_ms: Option<f64>,
    #[serde(default)]
    failed_requests: Option<i64>,
    #[serde(default)]
    window_failed_requests: Option<i64>,
    #[serde(default)]
    dns_failures: Option<i64>,
    #[serde(default)]
    window_dns_failures: Option<i64>,
    #[serde(default)]
    stream_errors: Option<i64>,
    #[serde(default)]
    window_stream_errors: Option<i64>,
    #[serde(default)]
    proxy_metadata: Option<serde_json::Value>,
    #[serde(default)]
    proxy_version: Option<String>,
}

pub(crate) struct TunnelHeartbeatClaim {
    lease: RuntimeLockLease,
}

#[derive(Debug, Clone)]
pub(crate) struct TunnelInstanceIdentity {
    instance_id: String,
    relay_base_url: Option<String>,
    attachment_ttl_secs: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct TunnelAttachmentDirectory {
    identity: Arc<TunnelInstanceIdentity>,
    runtime_state: Arc<RuntimeState>,
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
            runtime_state: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
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
            runtime_state: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        }
    }

    fn with_runtime_state(mut self, runtime_state: Arc<RuntimeState>) -> Self {
        self.runtime_state = runtime_state;
        self
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

    async fn refresh_from_authenticated_heartbeat(
        &self,
        data: &GatewayDataState,
        authenticated_node_id: &str,
        authenticated_generation: &str,
        request_body: &[u8],
    ) -> Result<(), String> {
        let payload = parse_embedded_tunnel_heartbeat_request(request_body)?;
        let node_id = payload.node_id.trim();
        if node_id != authenticated_node_id {
            return Err("heartbeat node_id does not match authenticated tunnel node".to_string());
        }
        let Some(node) = data
            .find_proxy_node(node_id)
            .await
            .map_err(|err| format!("attachment owner lookup failed: {err}"))?
        else {
            return Ok(());
        };
        if node.tunnel_generation != authenticated_generation || !node.tunnel_connected {
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
                tunnel_generation: authenticated_generation.to_string(),
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
        node_generation: &str,
        connected: bool,
        conn_count: usize,
        observed_at_unix_secs: u64,
    ) -> Result<(), String> {
        let node_id = node_id.trim();
        if node_id.is_empty() {
            return Ok(());
        }
        if !connected || conn_count == 0 {
            self.delete_attachment_record_if_owned(data, node_id, Some(node_generation))
                .await?;
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
                tunnel_generation: node_generation.to_string(),
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
        let current_generation = data
            .find_proxy_node(node_id)
            .await
            .map_err(|err| format!("attachment node lookup failed: {err}"))?
            .map(|node| node.tunnel_generation);
        if current_generation.as_deref() != Some(record.tunnel_generation.as_str()) {
            return Ok(None);
        }
        if !record.is_routable(current_unix_secs(), self.identity.attachment_ttl_secs) {
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
        if record.is_owned_by(&self.identity.instance_id) {
            self.delete_attachment_record_if_owned(data, node_id, None)
                .await?;
        }
        Ok(())
    }

    async fn read_attachment_record(
        &self,
        data: &GatewayDataState,
        node_id: &str,
    ) -> Result<Option<TunnelAttachmentRecord>, String> {
        match self.read_attachment_record_from_runtime(node_id).await {
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

    async fn read_attachment_record_from_runtime(
        &self,
        node_id: &str,
    ) -> Result<Option<TunnelAttachmentRecord>, String> {
        let raw = self
            .runtime_state
            .kv_get(&tunnel_attachment_redis_key(node_id))
            .await
            .map_err(|err| format!("attachment runtime read failed: {err}"))?;
        raw.map(|value| {
            serde_json::from_str::<TunnelAttachmentRecord>(&value)
                .map_err(|err| format!("invalid runtime tunnel attachment record: {err}"))
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
        let lease = self.acquire_attachment_lock(node_id).await?;
        let result = match tokio::time::timeout(
            TUNNEL_ATTACHMENT_OPERATION_TIMEOUT,
            self.write_attachment_record_unlocked(data, node_id, record),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(format!(
                "attachment write timed out after {} seconds",
                TUNNEL_ATTACHMENT_OPERATION_TIMEOUT.as_secs()
            )),
        };
        self.release_attachment_lock(lease).await;
        result
    }

    async fn write_attachment_record_unlocked(
        &self,
        data: &GatewayDataState,
        node_id: &str,
        record: &TunnelAttachmentRecord,
    ) -> Result<(), String> {
        let serialized = serde_json::to_string(record)
            .map_err(|err| format!("attachment serialization failed: {err}"))?;
        if let Err(error) = self
            .runtime_state
            .kv_set(
                &tunnel_attachment_redis_key(node_id),
                serialized.clone(),
                Some(Duration::from_secs(self.identity.attachment_ttl_secs)),
            )
            .await
        {
            warn!(
                error = %error,
                node_id = %node_id,
                "failed to write tunnel attachment to runtime state; keeping system_config shadow only"
            );
        }
        let value = serde_json::to_value(record)
            .map_err(|err| format!("attachment serialization failed: {err}"))?;
        data.upsert_system_config_value(&tunnel_attachment_key(node_id), &value, None)
            .await
            .map(|_| ())
            .map_err(|err| format!("attachment write failed: {err}"))
    }

    async fn acquire_attachment_lock(&self, node_id: &str) -> Result<RuntimeLockLease, String> {
        let key = format!("tunnel:attachments:lock:{}", node_id.trim());
        let owner = format!("tunnel-attachment:{}", self.identity.instance_id);
        let acquired = tokio::time::timeout(
            TUNNEL_ATTACHMENT_LOCK_ACQUIRE_TIMEOUT,
            self.runtime_state
                .lock_try_acquire(&key, &owner, TUNNEL_ATTACHMENT_LOCK_TTL),
        )
        .await
        .map_err(|_| {
            format!(
                "attachment lock acquisition timed out after {} seconds",
                TUNNEL_ATTACHMENT_LOCK_ACQUIRE_TIMEOUT.as_secs()
            )
        })?
        .map_err(|err| format!("attachment lock failed: {err}"))?;
        acquired.ok_or_else(|| "attachment update is busy; retry later".to_string())
    }

    async fn release_attachment_lock(&self, lease: RuntimeLockLease) {
        match tokio::time::timeout(
            TUNNEL_ATTACHMENT_LOCK_RELEASE_TIMEOUT,
            self.runtime_state.lock_release(&lease),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                warn!(error = %error, key = %lease.key, "failed to release tunnel attachment lock");
            }
            Err(_) => {
                warn!(
                    key = %lease.key,
                    timeout_secs = TUNNEL_ATTACHMENT_LOCK_RELEASE_TIMEOUT.as_secs(),
                    "timed out releasing tunnel attachment lock"
                );
            }
        }
    }

    async fn delete_attachment_record_if_owned(
        &self,
        data: &GatewayDataState,
        node_id: &str,
        expected_tunnel_generation: Option<&str>,
    ) -> Result<(), String> {
        let lease = self.acquire_attachment_lock(node_id).await?;

        let result = match tokio::time::timeout(TUNNEL_ATTACHMENT_OPERATION_TIMEOUT, async {
            // Read both shadows while holding the same distributed lock used
            // by writers. If either store already belongs to another gateway,
            // leave both records untouched; this prevents an old disconnect
            // event from deleting a replacement owner.
            let runtime_record = self.read_attachment_record_from_runtime(node_id).await?;
            let config_record = self
                .read_attachment_record_from_system_config(data, node_id)
                .await?;
            let expected_owner = self.identity.instance_id.as_str();
            if runtime_record.as_ref().is_some_and(|record| {
                !record.is_owned_by(expected_owner)
                    || expected_tunnel_generation
                        .is_some_and(|expected| record.tunnel_generation != expected)
            }) || config_record.as_ref().is_some_and(|record| {
                !record.is_owned_by(expected_owner)
                    || expected_tunnel_generation
                        .is_some_and(|expected| record.tunnel_generation != expected)
            }) {
                return Ok(());
            }

            if runtime_record.is_some()
                && !self
                    .runtime_state
                    .kv_delete(&tunnel_attachment_redis_key(node_id))
                    .await
                    .map_err(|error| format!("attachment runtime delete failed: {error}"))?
            {
                return Err("attachment runtime record disappeared before delete".to_string());
            }
            if config_record.is_some()
                && !data
                    .delete_system_config_value(&tunnel_attachment_key(node_id))
                    .await
                    .map_err(|err| format!("attachment delete failed: {err}"))?
            {
                return Err("attachment system record disappeared before delete".to_string());
            }
            Ok(())
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(format!(
                "attachment delete timed out after {} seconds",
                TUNNEL_ATTACHMENT_OPERATION_TIMEOUT.as_secs()
            )),
        };
        self.release_attachment_lock(lease).await;
        result
    }
}

#[derive(Clone)]
pub(crate) struct EmbeddedTunnelState {
    inner: TunnelAppState,
    attachment_directory: TunnelAttachmentDirectory,
    relay_auth_secret: Result<Arc<[u8]>, Arc<str>>,
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

pub(crate) struct RelayAuthHeaders {
    sender_instance_id: String,
    owner_instance_id: String,
    timestamp_unix_secs: u64,
    nonce: String,
    payload_digest: String,
    signature: String,
}

#[derive(Clone)]
pub(crate) struct VerifiedRelaySpool {
    inner: Arc<RelaySpoolInner>,
}

struct RelaySpoolInner {
    path: PathBuf,
    meta: RequestMeta,
    metadata_envelope: Bytes,
    body_offset: u64,
    body_len: u64,
    body_sha256: [u8; 32],
    reserved_bytes: u64,
}

impl Drop for RelaySpoolInner {
    fn drop(&mut self) {
        TUNNEL_RELAY_SPOOL_BYTES_IN_USE.fetch_sub(self.reserved_bytes, Ordering::AcqRel);
        let _ = std::fs::remove_file(&self.path);
    }
}

impl VerifiedRelaySpool {
    pub(crate) fn meta(&self) -> &RequestMeta {
        &self.inner.meta
    }

    async fn open_from_start(&self) -> Result<tokio::fs::File, String> {
        tokio::fs::File::open(&self.inner.path)
            .await
            .map_err(|error| format!("failed to reopen tunnel relay spool: {error}"))
    }

    async fn open_body(&self) -> Result<tokio::fs::File, String> {
        let mut file = self.open_from_start().await?;
        file.seek(std::io::SeekFrom::Start(self.inner.body_offset))
            .await
            .map_err(|error| format!("failed to seek tunnel relay spool: {error}"))?;
        Ok(file)
    }

    fn reqwest_body(&self, file: tokio::fs::File) -> reqwest::Body {
        let spool = self.clone();
        reqwest::Body::wrap_stream(async_stream::stream! {
            let _spool = spool;
            let mut stream = ReaderStream::new(file);
            while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                yield chunk;
            }
        })
    }

    pub(crate) async fn body_stream(
        &self,
    ) -> Result<impl futures_util::Stream<Item = Result<Bytes, io::Error>> + Send + 'static, String>
    {
        let file = self.open_body().await?;
        let spool = self.clone();
        Ok(async_stream::stream! {
            let _spool = spool;
            let mut stream = ReaderStream::new(file);
            while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                yield chunk;
            }
        })
    }

    fn payload_digest(&self) -> TunnelRelayPayloadDigest {
        tunnel_relay_payload_digest_from_hashes(
            &self.inner.metadata_envelope,
            self.inner.body_len,
            self.inner.body_sha256,
        )
    }
}

impl RelayAuthHeaders {
    pub(crate) fn apply(self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header(TUNNEL_RELAY_AUTH_SENDER_HEADER, self.sender_instance_id)
            .header(TUNNEL_RELAY_OWNER_INSTANCE_HEADER, self.owner_instance_id)
            .header(TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER, self.timestamp_unix_secs)
            .header(TUNNEL_RELAY_AUTH_NONCE_HEADER, self.nonce)
            .header(TUNNEL_RELAY_AUTH_PAYLOAD_HEADER, self.payload_digest)
            .header(TUNNEL_RELAY_AUTH_SIGNATURE_HEADER, self.signature)
    }

    pub(crate) fn apply_to_headers(self, headers: &mut HeaderMap) -> Result<(), String> {
        insert_relay_auth_header(
            headers,
            TUNNEL_RELAY_AUTH_SENDER_HEADER,
            &self.sender_instance_id,
        )?;
        insert_relay_auth_header(
            headers,
            TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
            &self.owner_instance_id,
        )?;
        insert_relay_auth_header(
            headers,
            TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
            &self.timestamp_unix_secs.to_string(),
        )?;
        insert_relay_auth_header(headers, TUNNEL_RELAY_AUTH_NONCE_HEADER, &self.nonce)?;
        insert_relay_auth_header(
            headers,
            TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
            &self.payload_digest,
        )?;
        insert_relay_auth_header(headers, TUNNEL_RELAY_AUTH_SIGNATURE_HEADER, &self.signature)
    }
}

fn insert_relay_auth_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), String> {
    let value = HeaderValue::from_str(value)
        .map_err(|error| format!("invalid tunnel relay authentication header: {error}"))?;
    headers.insert(http::header::HeaderName::from_static(name), value);
    Ok(())
}

pub(crate) fn build_relay_auth_headers_from_environment(
    owner_instance_id: &str,
    node_id: &str,
    metadata_envelope: &[u8],
    body: &[u8],
) -> Result<RelayAuthHeaders, String> {
    let secret = resolve_tunnel_relay_auth_secret_from_environment()?;
    build_relay_auth_headers(
        secret.as_bytes(),
        &resolve_tunnel_instance_id(),
        owner_instance_id,
        node_id,
        false,
        false,
        metadata_envelope,
        body,
    )
}

fn build_relay_auth_headers(
    secret: &[u8],
    sender_instance_id: &str,
    owner_instance_id: &str,
    node_id: &str,
    forwarded_by: bool,
    rollout_probe: bool,
    metadata_envelope: &[u8],
    body: &[u8],
) -> Result<RelayAuthHeaders, String> {
    build_relay_auth_headers_for_digest(
        secret,
        sender_instance_id,
        owner_instance_id,
        node_id,
        forwarded_by,
        rollout_probe,
        tunnel_relay_payload_digest(metadata_envelope, body),
    )
}

fn build_relay_auth_headers_for_digest(
    secret: &[u8],
    sender_instance_id: &str,
    owner_instance_id: &str,
    node_id: &str,
    forwarded_by: bool,
    rollout_probe: bool,
    payload_digest: TunnelRelayPayloadDigest,
) -> Result<RelayAuthHeaders, String> {
    validate_tunnel_relay_auth_secret(secret)?;
    let timestamp_unix_secs = current_unix_secs();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let forwarded_by_value = if forwarded_by { sender_instance_id } else { "" };
    let signature = sign_tunnel_relay_request(
        secret,
        sender_instance_id,
        owner_instance_id,
        node_id,
        forwarded_by_value,
        rollout_probe,
        timestamp_unix_secs,
        &nonce,
        &payload_digest,
    );
    Ok(RelayAuthHeaders {
        sender_instance_id: sender_instance_id.to_string(),
        owner_instance_id: owner_instance_id.to_string(),
        timestamp_unix_secs,
        nonce,
        payload_digest: payload_digest.encode_header_value(),
        signature,
    })
}

#[derive(Serialize)]
struct TunnelAffinityAuthMetadata {
    context: &'static str,
    method: String,
    path_and_query: String,
    gateway_marker: Option<String>,
    affinity_forwarded_by: Option<String>,
    affinity_owner_instance_id: Option<String>,
    affinity_node_id: Option<String>,
    forwarded_host: Option<String>,
    forwarded_for: Option<String>,
    forwarded_proto: Option<String>,
    trusted_user_id: Option<String>,
    trusted_api_key_id: Option<String>,
    trusted_access_allowed: Option<String>,
    trusted_balance_remaining: Option<String>,
}

pub(crate) fn build_tunnel_affinity_auth_metadata(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<Vec<u8>, String> {
    let metadata = TunnelAffinityAuthMetadata {
        context: TUNNEL_AFFINITY_AUTH_CONTEXT,
        method: method.as_str().to_string(),
        path_and_query: uri
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/")
            .to_string(),
        gateway_marker: canonical_header_value(headers, crate::constants::GATEWAY_HEADER)?,
        affinity_forwarded_by: canonical_header_value(
            headers,
            crate::constants::TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
        )?,
        affinity_owner_instance_id: canonical_header_value(
            headers,
            crate::constants::TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
        )?,
        affinity_node_id: canonical_header_value(
            headers,
            crate::constants::TUNNEL_AFFINITY_NODE_ID_HEADER,
        )?,
        forwarded_host: canonical_header_value(headers, crate::constants::FORWARDED_HOST_HEADER)?,
        forwarded_for: canonical_header_value(headers, crate::constants::FORWARDED_FOR_HEADER)?,
        forwarded_proto: canonical_header_value(headers, crate::constants::FORWARDED_PROTO_HEADER)?,
        trusted_user_id: canonical_header_value(
            headers,
            crate::constants::TRUSTED_AUTH_USER_ID_HEADER,
        )?,
        trusted_api_key_id: canonical_header_value(
            headers,
            crate::constants::TRUSTED_AUTH_API_KEY_ID_HEADER,
        )?,
        trusted_access_allowed: canonical_header_value(
            headers,
            crate::constants::TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
        )?,
        trusted_balance_remaining: canonical_header_value(
            headers,
            crate::constants::TRUSTED_AUTH_BALANCE_HEADER,
        )?,
    };
    serde_json::to_vec(&metadata)
        .map_err(|error| format!("encode tunnel affinity authentication metadata failed: {error}"))
}

fn canonical_header_value(
    headers: &HeaderMap,
    name: &'static str,
) -> Result<Option<String>, String> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(format!(
            "duplicate tunnel affinity authentication header: {name}"
        ));
    }
    value
        .to_str()
        .map(|value| Some(value.to_string()))
        .map_err(|_| format!("invalid tunnel affinity authentication header: {name}"))
}

impl EmbeddedTunnelState {
    pub(crate) fn new() -> Self {
        Self::with_data(Arc::new(GatewayDataState::disabled()))
    }

    pub(crate) fn with_data(data: Arc<GatewayDataState>) -> Self {
        Self::with_data_and_directory(data, TunnelAttachmentDirectory::from_environment())
    }

    pub(crate) fn with_data_and_runtime_state(
        data: Arc<GatewayDataState>,
        runtime_state: Arc<RuntimeState>,
    ) -> Self {
        Self::with_data_and_directory(
            data,
            TunnelAttachmentDirectory::from_environment().with_runtime_state(runtime_state),
        )
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

    pub(crate) fn with_data_identity_and_runtime_state(
        data: Arc<GatewayDataState>,
        instance_id: impl Into<String>,
        relay_base_url: Option<impl Into<String>>,
        attachment_ttl_secs: u64,
        runtime_state: Arc<RuntimeState>,
    ) -> Self {
        Self::with_data_and_directory(
            data,
            TunnelAttachmentDirectory::from_parts(instance_id, relay_base_url, attachment_ttl_secs)
                .with_runtime_state(runtime_state),
        )
    }

    pub(crate) fn with_data_and_directory(
        data: Arc<GatewayDataState>,
        attachment_directory: TunnelAttachmentDirectory,
    ) -> Self {
        let defaults = EmbeddedTunnelDefaults::default();
        let relay_auth_secret = resolve_tunnel_relay_auth_secret()
            .map(Arc::<[u8]>::from)
            .map_err(Arc::from);
        Self {
            inner: TunnelAppState::new(
                build_embedded_control_plane(Arc::clone(&data), attachment_directory.clone()),
                ConnConfig {
                    ping_interval: defaults.ping_interval,
                    idle_timeout: defaults.proxy_idle_timeout,
                    outbound_queue_capacity: defaults.outbound_queue_capacity,
                },
                defaults.max_streams,
            )
            .with_relay_auth(
                attachment_directory.local_instance_id().to_string(),
                relay_auth_secret
                    .as_ref()
                    .ok()
                    .map(|value| value.as_ref().to_vec()),
                Arc::clone(&attachment_directory.runtime_state),
            )
            .with_data(data),
            attachment_directory,
            relay_auth_secret,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_data_and_directory_for_tests(
        data: Arc<GatewayDataState>,
        attachment_directory: TunnelAttachmentDirectory,
        relay_auth_secret: &str,
    ) -> Self {
        let defaults = EmbeddedTunnelDefaults::default();
        let relay_auth_secret = validate_tunnel_relay_auth_secret(relay_auth_secret.as_bytes())
            .map(|()| Arc::<[u8]>::from(relay_auth_secret.as_bytes()))
            .map_err(Arc::from);
        Self {
            inner: TunnelAppState::new(
                build_embedded_control_plane(Arc::clone(&data), attachment_directory.clone()),
                ConnConfig {
                    ping_interval: defaults.ping_interval,
                    idle_timeout: defaults.proxy_idle_timeout,
                    outbound_queue_capacity: defaults.outbound_queue_capacity,
                },
                defaults.max_streams,
            )
            .with_relay_auth(
                attachment_directory.local_instance_id().to_string(),
                relay_auth_secret
                    .as_ref()
                    .ok()
                    .map(|value| value.as_ref().to_vec()),
                Arc::clone(&attachment_directory.runtime_state),
            )
            .with_data(data),
            attachment_directory,
            relay_auth_secret,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_data_identity_runtime_state_and_relay_secret_for_tests(
        data: Arc<GatewayDataState>,
        instance_id: &str,
        relay_base_url: Option<&str>,
        runtime_state: Arc<RuntimeState>,
        relay_auth_secret: &str,
    ) -> Self {
        Self::with_data_and_directory_for_tests(
            data,
            TunnelAttachmentDirectory::for_tests(instance_id, relay_base_url, 90)
                .with_runtime_state(runtime_state),
            relay_auth_secret,
        )
    }

    pub(crate) fn app_state(&self) -> TunnelAppState {
        self.inner.clone()
    }

    pub(crate) async fn authenticate_relay_request(
        &self,
        headers: &HeaderMap,
        node_id: &str,
        metadata_envelope: &[u8],
        body: &[u8],
        require_local_owner: bool,
    ) -> Result<(), embedded::RelayAuthError> {
        self.inner
            .authenticate_relay_request(
                headers,
                node_id,
                &tunnel_relay_payload_digest(metadata_envelope, body),
                require_local_owner,
            )
            .await
    }

    pub(crate) async fn prepare_tunnel_affinity_auth_request(
        &self,
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
    ) -> Result<Option<PendingTunnelAffinityAuth>, RelayAuthError> {
        if !has_tunnel_affinity_auth_headers(headers) {
            return Ok(None);
        }
        if headers.contains_key(TUNNEL_RELAY_ROLLOUT_PROBE_HEADER) {
            return Err(RelayAuthError::Invalid);
        }
        if headers.contains_key("x-real-ip") {
            return Err(RelayAuthError::Invalid);
        }

        let gateway_marker = required_affinity_header(
            headers,
            crate::constants::GATEWAY_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        if gateway_marker != TUNNEL_AFFINITY_GATEWAY_MARKER {
            return Err(RelayAuthError::Invalid);
        }

        let relay_sender = required_affinity_header(
            headers,
            TUNNEL_RELAY_AUTH_SENDER_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        let relay_owner = required_affinity_header(
            headers,
            TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        let relay_forwarded_by = required_affinity_header(
            headers,
            TUNNEL_RELAY_FORWARDED_BY_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        let affinity_forwarded_by = required_affinity_header(
            headers,
            crate::constants::TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        let affinity_owner = required_affinity_header(
            headers,
            crate::constants::TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        let affinity_node_id = required_affinity_header(
            headers,
            crate::constants::TUNNEL_AFFINITY_NODE_ID_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        if relay_sender != relay_forwarded_by
            || relay_sender != affinity_forwarded_by
            || relay_owner != affinity_owner
            || affinity_owner != self.local_instance_id()
        {
            return Err(RelayAuthError::Invalid);
        }

        required_affinity_header(
            headers,
            crate::constants::TRUSTED_AUTH_USER_ID_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        required_affinity_header(
            headers,
            crate::constants::TRUSTED_AUTH_API_KEY_ID_HEADER,
            MAX_TUNNEL_AFFINITY_ID_LEN,
        )?;
        let access_allowed = required_affinity_header(
            headers,
            crate::constants::TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
            5,
        )?;
        if !matches!(access_allowed, "true" | "false") {
            return Err(RelayAuthError::Invalid);
        }
        if let Some(balance) =
            optional_affinity_header(headers, crate::constants::TRUSTED_AUTH_BALANCE_HEADER, 64)?
        {
            if !balance
                .parse::<f64>()
                .ok()
                .is_some_and(|value| value.is_finite())
            {
                return Err(RelayAuthError::Invalid);
            }
        }
        let client_ip =
            required_affinity_header(headers, crate::constants::FORWARDED_FOR_HEADER, 45)?
                .parse::<std::net::IpAddr>()
                .map_err(|_| RelayAuthError::Invalid)?;

        let metadata = build_tunnel_affinity_auth_metadata(method, uri, headers)
            .map_err(|_| RelayAuthError::Invalid)?;
        let relay_auth = self
            .inner
            .authenticate_relay_request_headers(headers, affinity_node_id, true)
            .await?;
        if !relay_auth.payload_digest.matches_metadata(&metadata) {
            return Err(RelayAuthError::Invalid);
        }
        Ok(Some(PendingTunnelAffinityAuth {
            context: TunnelAffinityAuthContext { client_ip },
            relay_auth,
        }))
    }

    pub(crate) async fn commit_tunnel_affinity_auth_request(
        &self,
        pending: PendingTunnelAffinityAuth,
        body: &[u8],
    ) -> Result<TunnelAffinityAuthContext, RelayAuthError> {
        if !pending.relay_auth.payload_digest.matches_body(body) {
            return Err(RelayAuthError::Invalid);
        }
        self.inner.commit_relay_auth(&pending.relay_auth).await?;
        Ok(pending.context)
    }

    pub(crate) fn build_relay_auth_headers(
        &self,
        owner_instance_id: &str,
        node_id: &str,
        forwarded_by: bool,
        rollout_probe: bool,
        metadata_envelope: &[u8],
        body: &[u8],
    ) -> Result<RelayAuthHeaders, String> {
        let secret = self
            .relay_auth_secret
            .as_deref()
            .map_err(|error| error.to_string())?;
        let sender_instance_id = self.local_instance_id();
        build_relay_auth_headers(
            secret,
            sender_instance_id,
            owner_instance_id,
            node_id,
            forwarded_by,
            rollout_probe,
            metadata_envelope,
            body,
        )
    }

    pub(crate) fn build_relay_auth_headers_for_digest(
        &self,
        owner_instance_id: &str,
        node_id: &str,
        forwarded_by: bool,
        rollout_probe: bool,
        payload_digest: TunnelRelayPayloadDigest,
    ) -> Result<RelayAuthHeaders, String> {
        let secret = self
            .relay_auth_secret
            .as_deref()
            .map_err(|error| error.to_string())?;
        build_relay_auth_headers_for_digest(
            secret,
            self.local_instance_id(),
            owner_instance_id,
            node_id,
            forwarded_by,
            rollout_probe,
            payload_digest,
        )
    }

    pub(crate) fn register_secure_tunnel_key(
        &self,
        node_id: impl Into<String>,
        key: impl Into<String>,
    ) {
        self.inner.register_secure_tunnel_key(node_id, key);
    }

    pub(crate) async fn authenticate_control_plane_request(
        &self,
        headers: &HeaderMap,
        method: &str,
        path: &str,
        payload_node_id: &str,
        body: &[u8],
    ) -> Result<String, ControlPlaneAuthError> {
        self.inner
            .authenticate_control_plane_request(headers, method, path, payload_node_id, body)
            .await
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

    pub(crate) fn request_close_proxies_for_node(&self, node_id: &str) -> usize {
        self.inner.hub.request_close_proxies_for_node(node_id)
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

    pub(crate) async fn probe_node_url_routed(
        &self,
        state: &AppState,
        node_id: &str,
        url: &str,
        timeout_secs: u64,
    ) -> Result<u16, String> {
        if self.has_local_proxy(node_id) {
            return self.probe_node_url(node_id, url, timeout_secs).await;
        }

        let Some(owner) = self
            .lookup_attachment_owner(state.data.as_ref(), node_id)
            .await?
        else {
            return self.probe_node_url(node_id, url, timeout_secs).await;
        };
        if owner.gateway_instance_id == self.local_instance_id() {
            self.clear_local_attachment_if_stale(state.data.as_ref(), node_id)
                .await?;
            return self.probe_node_url(node_id, url, timeout_secs).await;
        }

        let timeout_secs = timeout_secs.clamp(5, 60);
        let owner_url = build_tunnel_owner_relay_url(&owner.relay_base_url, node_id)
            .map_err(|error| format!("invalid owner tunnel probe URL: {error}"))?;
        let payload = encode_tunnel_relay_envelope(&build_tunnel_probe_meta(url, timeout_secs))?;
        let relay_auth = self.build_relay_auth_headers(
            &owner.gateway_instance_id,
            node_id,
            true,
            true,
            &payload,
            &[],
        )?;
        let owner_client =
            owner_forward_client_for_url(&state.owner_forward_client, &owner_url).await?;
        let request = owner_client
            .post(owner_url)
            .header(TUNNEL_RELAY_FORWARDED_BY_HEADER, self.local_instance_id())
            .header(
                TUNNEL_RELAY_ROLLOUT_PROBE_HEADER,
                TUNNEL_RELAY_ROLLOUT_PROBE_VALUE,
            )
            .timeout(Duration::from_secs(timeout_secs))
            .body(payload);
        let response = relay_auth
            .apply(request)
            .send()
            .await
            .map_err(|error| owner_forward_request_error(&error))?;
        Ok(response.status().as_u16())
    }

    pub(crate) async fn probe_node_url_with_response(
        &self,
        node_id: &str,
        url: &str,
        timeout_secs: u64,
    ) -> Result<TunnelProbeResponse, String> {
        let timeout_secs = timeout_secs.clamp(5, 60);
        let meta = build_tunnel_probe_meta(url, timeout_secs);
        let stream = self
            .inner
            .open_authorized_local_stream(node_id, &meta)
            .await?;
        let stream_id = stream.id;
        let result = async {
            self.inner
                .hub
                .push_local_request_body(stream_id, Bytes::new(), true)
                .await?;
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

fn has_tunnel_affinity_auth_headers(headers: &HeaderMap) -> bool {
    [
        TUNNEL_RELAY_AUTH_SENDER_HEADER,
        TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
        TUNNEL_RELAY_AUTH_NONCE_HEADER,
        TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
        TUNNEL_RELAY_AUTH_SIGNATURE_HEADER,
        TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
        TUNNEL_RELAY_FORWARDED_BY_HEADER,
        TUNNEL_RELAY_ROLLOUT_PROBE_HEADER,
        crate::constants::TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
        crate::constants::TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
        crate::constants::TUNNEL_AFFINITY_NODE_ID_HEADER,
        crate::constants::TRUSTED_AUTH_USER_ID_HEADER,
        crate::constants::TRUSTED_AUTH_API_KEY_ID_HEADER,
        crate::constants::TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
        crate::constants::TRUSTED_AUTH_BALANCE_HEADER,
    ]
    .into_iter()
    .any(|name| headers.contains_key(name))
}

fn required_affinity_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
    max_len: usize,
) -> Result<&'a str, RelayAuthError> {
    optional_affinity_header(headers, name, max_len)?.ok_or(RelayAuthError::Invalid)
}

fn optional_affinity_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
    max_len: usize,
) -> Result<Option<&'a str>, RelayAuthError> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(RelayAuthError::Invalid);
    }
    let value = value.to_str().map_err(|_| RelayAuthError::Invalid)?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max_len || trimmed != value {
        return Err(RelayAuthError::Invalid);
    }
    Ok(Some(trimmed))
}

fn build_tunnel_probe_meta(url: &str, timeout_secs: u64) -> tunnel_protocol::RequestMeta {
    tunnel_protocol::RequestMeta {
        provider_id: None,
        endpoint_id: None,
        key_id: None,
        method: "GET".to_string(),
        url: url.trim().to_string(),
        headers: HashMap::new(),
        stream: false,
        request_timeout_ms: None,
        stream_first_byte_timeout_ms: None,
        timeout: timeout_secs,
        follow_redirects: Some(false),
        http1_only: false,
        transport_profile: None,
    }
}

fn encode_tunnel_relay_envelope(meta: &tunnel_protocol::RequestMeta) -> Result<Vec<u8>, String> {
    let meta = serde_json::to_vec(meta)
        .map_err(|error| format!("failed to encode tunnel probe metadata: {error}"))?;
    let meta_len = u32::try_from(meta.len())
        .map_err(|_| "tunnel probe metadata exceeds relay envelope limit".to_string())?;
    let mut payload = Vec::with_capacity(4usize.saturating_add(meta.len()));
    payload.extend_from_slice(&meta_len.to_be_bytes());
    payload.extend_from_slice(&meta);
    Ok(payload)
}

impl Default for EmbeddedTunnelState {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for EmbeddedTunnelState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let defaults = EmbeddedTunnelDefaults::default();
        f.debug_struct("EmbeddedTunnelState")
            .field(
                "proxy_idle_timeout_ms",
                &defaults.proxy_idle_timeout.as_millis(),
            )
            .field("ping_interval_ms", &defaults.ping_interval.as_millis())
            .field("max_streams", &defaults.max_streams)
            .field("outbound_queue_capacity", &defaults.outbound_queue_capacity)
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
    connect_info: ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> impl IntoResponse {
    embedded::ws_proxy(ws, State(state.tunnel.app_state()), connect_info, headers).await
}

pub(crate) async fn relay_request(
    path: Path<String>,
    State(state): State<AppState>,
    connect_info: ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> Result<axum::http::Response<Body>, GatewayError> {
    let node_id = path.0;
    let trace_id = extract_or_generate_trace_id(request.headers());
    let (mut parts, body) = request.into_parts();
    let relay_body_limit = tunnel_relay_body_limit_bytes();
    match declared_relay_body_len(&parts.headers) {
        Ok(Some(length)) if length > relay_body_limit => {
            return build_local_http_error_response(
                &trace_id,
                None,
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("tunnel relay body exceeds {relay_body_limit} bytes"),
            );
        }
        Err(error) => {
            return build_local_http_error_response(
                &trace_id,
                None,
                StatusCode::BAD_REQUEST,
                &error,
            );
        }
        _ => {}
    }
    let pending_auth = match state
        .tunnel
        .app_state()
        .authenticate_relay_request_headers(&parts.headers, &node_id, true)
        .await
    {
        Ok(pending) => pending,
        Err(embedded::RelayAuthError::Unavailable) => {
            return build_local_http_error_response(
                &trace_id,
                None,
                StatusCode::SERVICE_UNAVAILABLE,
                "tunnel relay authentication is not configured",
            );
        }
        Err(embedded::RelayAuthError::Invalid) => {
            return build_local_http_error_response(
                &trace_id,
                None,
                StatusCode::FORBIDDEN,
                "invalid tunnel relay authentication",
            );
        }
    };
    let authenticated_body = match prepare_owner_relay_request_body_with_limits(
        body,
        relay_body_limit,
        TUNNEL_RELAY_BODY_READ_TIMEOUT,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            return build_local_http_error_response(
                &trace_id,
                None,
                if error.starts_with("tunnel relay body exceeds") {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                },
                &error,
            );
        }
    };
    if pending_auth.payload_digest != authenticated_body.payload_digest() {
        return build_local_http_error_response(
            &trace_id,
            None,
            StatusCode::FORBIDDEN,
            "invalid tunnel relay payload integrity",
        );
    }
    match state
        .tunnel
        .app_state()
        .commit_relay_auth(&pending_auth)
        .await
    {
        Ok(()) => {}
        Err(embedded::RelayAuthError::Unavailable) => {
            return build_local_http_error_response(
                &trace_id,
                None,
                StatusCode::SERVICE_UNAVAILABLE,
                "tunnel relay authentication is not configured",
            );
        }
        Err(embedded::RelayAuthError::Invalid) => {
            return build_local_http_error_response(
                &trace_id,
                None,
                StatusCode::FORBIDDEN,
                "invalid tunnel relay authentication",
            );
        }
    }
    parts.extensions.insert(embedded::RelayRequestAuthenticated);

    if state.tunnel.has_local_proxy(&node_id) {
        return relay_spool_to_local_proxy(
            state.tunnel.app_state(),
            connect_info,
            node_id,
            parts,
            authenticated_body,
        )
        .await;
    }

    let already_forwarded = parts
        .headers
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
            return forward_relay_request_to_owner(
                &state,
                &node_id,
                parts,
                authenticated_body,
                &trace_id,
                &owner,
            )
            .await;
        }
        state
            .tunnel
            .clear_local_attachment_if_stale(state.data.as_ref(), &node_id)
            .await
            .map_err(GatewayError::Internal)?;
    }

    relay_spool_to_local_proxy(
        state.tunnel.app_state(),
        connect_info,
        node_id,
        parts,
        authenticated_body,
    )
    .await
}

async fn relay_spool_to_local_proxy(
    state: TunnelAppState,
    connect_info: ConnectInfo<std::net::SocketAddr>,
    node_id: String,
    parts: http::request::Parts,
    spool: VerifiedRelaySpool,
) -> Result<axum::http::Response<Body>, GatewayError> {
    let mut request = Request::from_parts(parts, Body::empty());
    request.extensions_mut().insert(spool);
    Ok(
        embedded::relay_request(Path(node_id), State(state), connect_info, request)
            .await
            .into_response(),
    )
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
        move |connection, payload| {
            let data = Arc::clone(&heartbeat_data);
            let directory = heartbeat_directory.clone();
            Box::pin(async move {
                crate::tunnel::embedded::validate_proxy_connection_credential(
                    data.as_ref(),
                    &connection,
                )
                .await
                .map_err(str::to_string)?;
                let authenticated_node_id = connection.node_id.clone();
                let authenticated_generation = connection.node_generation.clone();
                let ack = apply_embedded_tunnel_heartbeat(
                    data.as_ref(),
                    directory.runtime_state.as_ref(),
                    &authenticated_node_id,
                    &authenticated_generation,
                    &payload,
                )
                .await?;
                if let Err(error) = directory
                    .refresh_from_authenticated_heartbeat(
                        data.as_ref(),
                        &authenticated_node_id,
                        &authenticated_generation,
                        &payload,
                    )
                    .await
                {
                    warn!(error = %error, "failed to refresh tunnel attachment from heartbeat");
                }
                Ok(ack)
            })
        },
        move |connection, connected, conn_count, observed_at_unix_secs| {
            let data = Arc::clone(&node_status_data);
            let directory = node_status_directory.clone();
            Box::pin(async move {
                crate::tunnel::embedded::validate_proxy_connection_credential(
                    data.as_ref(),
                    &connection,
                )
                .await
                .map_err(str::to_string)?;
                let node_id = connection.node_id.clone();
                let node_generation = connection.node_generation.clone();
                apply_embedded_tunnel_node_status(
                    data.as_ref(),
                    &node_id,
                    &node_generation,
                    connected,
                    conn_count,
                    Some(observed_at_unix_secs),
                )
                .await?;
                if let Err(error) = directory
                    .sync_node_status(
                        data.as_ref(),
                        &node_id,
                        &node_generation,
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
    parts: http::request::Parts,
    prepared_body: VerifiedRelaySpool,
    trace_id: &str,
    owner: &TunnelAttachmentRecord,
) -> Result<axum::http::Response<Body>, GatewayError> {
    let owner_url = build_tunnel_owner_relay_url(&owner.relay_base_url, node_id)
        .map_err(GatewayError::Internal)?;
    let payload_digest = prepared_body.payload_digest();
    let meta = prepared_body.meta().clone();
    let file = prepared_body
        .open_from_start()
        .await
        .map_err(GatewayError::Internal)?;
    let request_body = prepared_body.reqwest_body(file);
    let relay_auth = state
        .tunnel
        .build_relay_auth_headers_for_digest(
            &owner.gateway_instance_id,
            node_id,
            true,
            false,
            payload_digest,
        )
        .map_err(GatewayError::Internal)?;

    let connection_declared = aether_http::connection_declared_header_names(
        parts
            .headers
            .get_all(http::header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok()),
    );
    let owner_client = owner_forward_client_for_url(&state.owner_forward_client, &owner_url)
        .await
        .map_err(GatewayError::Internal)?;
    let mut upstream_request = owner_client.post(owner_url);
    for (name, value) in &parts.headers {
        if should_skip_request_header(name.as_str())
            || name == http::header::HOST
            || is_tunnel_relay_auth_header(name.as_str())
            || connection_declared.contains(&name.as_str().to_ascii_lowercase())
        {
            continue;
        }
        upstream_request = upstream_request.header(name, value);
    }
    upstream_request = relay_auth.apply(upstream_request).header(
        TUNNEL_RELAY_FORWARDED_BY_HEADER,
        state.tunnel.local_instance_id(),
    );
    let resolved_timeouts = resolve_tunnel_request_timeouts(&meta);
    if let Some(timeout_ms) = resolved_timeouts.response_body_ms {
        upstream_request = upstream_request.timeout(Duration::from_millis(timeout_ms));
    }
    if !parts.headers.contains_key(TRACE_ID_HEADER) {
        upstream_request = upstream_request.header(TRACE_ID_HEADER, trace_id);
    }

    let first_byte_timeout = meta
        .stream
        .then_some(Duration::from_millis(resolved_timeouts.first_byte_ms));
    let upstream_response =
        match send_owner_forward_request(upstream_request.body(request_body), first_byte_timeout)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                return Err(GatewayError::Internal(format!(
                    "owner tunnel relay failed: {err}"
                )));
            }
        };

    build_client_response(upstream_response, trace_id, None)
}

pub(crate) fn is_tunnel_relay_auth_header(name: &str) -> bool {
    name.eq_ignore_ascii_case(TUNNEL_RELAY_AUTH_SENDER_HEADER)
        || name.eq_ignore_ascii_case(TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER)
        || name.eq_ignore_ascii_case(TUNNEL_RELAY_AUTH_NONCE_HEADER)
        || name.eq_ignore_ascii_case(TUNNEL_RELAY_AUTH_PAYLOAD_HEADER)
        || name.eq_ignore_ascii_case(TUNNEL_RELAY_AUTH_SIGNATURE_HEADER)
        || name.eq_ignore_ascii_case(TUNNEL_RELAY_OWNER_INSTANCE_HEADER)
        || name.eq_ignore_ascii_case(TUNNEL_RELAY_FORWARDED_BY_HEADER)
        || name.eq_ignore_ascii_case(TUNNEL_RELAY_ROLLOUT_PROBE_HEADER)
}

pub(crate) fn build_tunnel_owner_relay_url(
    relay_base_url: &str,
    node_id: &str,
) -> Result<String, String> {
    let node_id = node_id.trim();
    let node_id_lower = node_id.to_ascii_lowercase();
    if node_id.is_empty()
        || matches!(
            node_id_lower.as_str(),
            "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e."
        )
    {
        return Err("tunnel relay node ID is invalid".to_string());
    }
    let mut url = parse_tunnel_relay_base_url(relay_base_url)?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "tunnel relay base URL cannot be a base-less URL".to_string())?;
        segments.pop_if_empty();
        segments.push("api");
        segments.push("internal");
        segments.push("tunnel");
        segments.push("relay");
        segments.push(node_id);
    }
    Ok(url.to_string())
}

pub(crate) fn build_tunnel_affinity_forward_url(
    relay_base_url: &str,
    request_uri: &Uri,
) -> Result<String, String> {
    let mut url = parse_tunnel_relay_base_url(relay_base_url)?;
    url.set_path(request_uri.path());
    url.set_query(request_uri.query());
    url.set_fragment(None);
    validate_tunnel_relay_transport_url(&url)?;
    Ok(url.to_string())
}

fn parse_tunnel_relay_base_url(value: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(value.trim())
        .map_err(|error| format!("invalid tunnel relay base URL: {error}"))?;
    validate_tunnel_relay_transport_url(&url)?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err("tunnel relay base URL must not include a query or fragment".to_string());
    }
    Ok(url)
}

pub(crate) fn validate_tunnel_relay_transport_url(url: &url::Url) -> Result<(), String> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err("tunnel relay URL must not include credentials".to_string());
    }
    let host = url
        .host()
        .ok_or_else(|| "tunnel relay URL must include a host".to_string())?;
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let loopback = match host {
                url::Host::Domain(host) => {
                    host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
                }
                url::Host::Ipv4(address) => address.is_loopback(),
                url::Host::Ipv6(address) => address.is_loopback(),
            };
            if loopback {
                Ok(())
            } else {
                Err("tunnel relay URL must use HTTPS unless the host is loopback".to_string())
            }
        }
        _ => Err("tunnel relay URL must use HTTPS or loopback HTTP".to_string()),
    }
}

fn tunnel_relay_body_limit_bytes() -> u64 {
    std::env::var(TUNNEL_RELAY_MAX_BODY_MB_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(|value| {
            value
                .saturating_mul(1024 * 1024)
                .min(MAX_TUNNEL_RELAY_BODY_BYTES)
        })
        .unwrap_or(DEFAULT_TUNNEL_RELAY_MAX_BODY_BYTES)
}

fn tunnel_relay_spool_budget_bytes() -> u64 {
    std::env::var(TUNNEL_RELAY_SPOOL_BUDGET_MB_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(|value| {
            value
                .saturating_mul(1024 * 1024)
                .min(MAX_TUNNEL_RELAY_SPOOL_BUDGET_BYTES)
        })
        .unwrap_or(DEFAULT_TUNNEL_RELAY_SPOOL_BUDGET_BYTES)
}

fn reserve_tunnel_relay_spool_bytes(bytes: u64) -> Result<(), String> {
    let budget = tunnel_relay_spool_budget_bytes();
    loop {
        let current = TUNNEL_RELAY_SPOOL_BYTES_IN_USE.load(Ordering::Acquire);
        let next = current
            .checked_add(bytes)
            .ok_or_else(|| "tunnel relay spool budget overflow".to_string())?;
        if next > budget {
            return Err(format!(
                "tunnel relay spool budget exhausted at {budget} bytes"
            ));
        }
        if TUNNEL_RELAY_SPOOL_BYTES_IN_USE
            .compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Ok(());
        }
    }
}

fn declared_relay_body_len(headers: &HeaderMap) -> Result<Option<u64>, String> {
    let mut values = headers.get_all(http::header::CONTENT_LENGTH).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err("duplicate content-length header on tunnel relay request".to_string());
    }
    let value = value
        .to_str()
        .map_err(|_| "invalid content-length header on tunnel relay request".to_string())?;
    value
        .trim()
        .parse::<u64>()
        .map(Some)
        .map_err(|_| "invalid content-length header on tunnel relay request".to_string())
}

async fn prepare_owner_relay_request_body(body: Body) -> Result<VerifiedRelaySpool, String> {
    prepare_owner_relay_request_body_with_limits(
        body,
        tunnel_relay_body_limit_bytes(),
        TUNNEL_RELAY_BODY_READ_TIMEOUT,
    )
    .await
}

async fn prepare_owner_relay_request_body_with_limits(
    body: Body,
    max_envelope_bytes: u64,
    read_timeout: Duration,
) -> Result<VerifiedRelaySpool, String> {
    let mut reserved_bytes = 0_u64;
    let path = std::env::temp_dir().join(format!(
        "aether-tunnel-relay-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .await
        .map_err(|error| format!("failed to create tunnel relay spool: {error}"))?;
    let mut cleanup = Some(path.clone());
    let result = async {
        let mut body_stream = body.into_data_stream();
        let mut metadata_prefix = bytes::BytesMut::new();
        let mut decoded = None;
        let mut body_hasher = Sha256::new();
        let mut body_len = 0_u64;
        let mut envelope_len = 0_u64;

        loop {
            let next_chunk = tokio::time::timeout(
                read_timeout,
                futures_util::StreamExt::next(&mut body_stream),
            )
            .await
            .map_err(|_| {
                format!(
                    "tunnel relay body read timed out after {} seconds",
                    read_timeout.as_secs()
                )
            })?;
            let Some(chunk) = next_chunk else {
                break;
            };
            let chunk = chunk.map_err(|error| format!("tunnel relay body read failed: {error}"))?;
            let chunk_start = envelope_len;
            envelope_len = envelope_len
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| "tunnel relay body length overflow".to_string())?;
            if envelope_len > max_envelope_bytes {
                return Err(format!(
                    "tunnel relay body exceeds {max_envelope_bytes} bytes"
                ));
            }
            reserve_tunnel_relay_spool_bytes(chunk.len() as u64)?;
            reserved_bytes = reserved_bytes.saturating_add(chunk.len() as u64);
            tokio::time::timeout(read_timeout, file.write_all(&chunk))
                .await
                .map_err(|_| {
                    format!(
                        "tunnel relay spool write timed out after {} seconds",
                        read_timeout.as_secs()
                    )
                })?
                .map_err(|error| format!("failed to write tunnel relay spool: {error}"))?;

            if decoded.is_some() {
                body_hasher.update(&chunk);
                body_len = body_len.saturating_add(chunk.len() as u64);
                continue;
            }

            let prefix_limit = 4usize.saturating_add(MAX_TUNNEL_RELAY_META_LEN);
            let remaining = prefix_limit.saturating_sub(metadata_prefix.len());
            metadata_prefix.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            match try_decode_tunnel_relay_request_meta(&metadata_prefix)? {
                Some((meta, body_offset)) => {
                    let body_offset_u64 = body_offset as u64;
                    if envelope_len > body_offset_u64 {
                        let body_in_chunk = body_offset_u64
                            .saturating_sub(chunk_start)
                            .min(chunk.len() as u64)
                            as usize;
                        let first_body = &chunk[body_in_chunk..];
                        body_hasher.update(first_body);
                        body_len = first_body.len() as u64;
                    }
                    decoded = Some((meta, body_offset));
                }
                None if metadata_prefix.len() < prefix_limit => {}
                None => return Err("incomplete tunnel relay metadata".to_string()),
            }
        }
        tokio::time::timeout(read_timeout, file.flush())
            .await
            .map_err(|_| {
                format!(
                    "tunnel relay spool flush timed out after {} seconds",
                    read_timeout.as_secs()
                )
            })?
            .map_err(|error| format!("failed to flush tunnel relay spool: {error}"))?;
        let (meta, body_offset) =
            decoded.ok_or_else(|| "incomplete tunnel relay metadata".to_string())?;
        let metadata_envelope = metadata_prefix.freeze().slice(..body_offset);
        Ok(VerifiedRelaySpool {
            inner: Arc::new(RelaySpoolInner {
                path,
                meta,
                metadata_envelope,
                body_offset: body_offset as u64,
                body_len,
                body_sha256: body_hasher.finalize().into(),
                reserved_bytes,
            }),
        })
    }
    .await;
    if result.is_ok() {
        cleanup = None;
    } else if reserved_bytes > 0 {
        TUNNEL_RELAY_SPOOL_BYTES_IN_USE.fetch_sub(reserved_bytes, Ordering::AcqRel);
    }
    if let Some(path) = cleanup {
        let _ = tokio::fs::remove_file(path).await;
    }
    result
}

fn tunnel_attachment_key(node_id: &str) -> String {
    format!("{TUNNEL_ATTACHMENT_KEY_PREFIX}{}", node_id.trim())
}

fn tunnel_attachment_redis_key(node_id: &str) -> String {
    format!("{TUNNEL_ATTACHMENT_REDIS_KEY_PREFIX}{}", node_id.trim())
}

pub(crate) fn resolve_tunnel_instance_id() -> String {
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

fn resolve_tunnel_relay_auth_secret() -> Result<Vec<u8>, String> {
    resolve_tunnel_relay_auth_secret_from_environment().map(String::into_bytes)
}

fn resolve_tunnel_relay_auth_secret_from_environment() -> Result<String, String> {
    let value = std::env::var(TUNNEL_RELAY_AUTH_SECRET_ENV).map_err(|error| match error {
        std::env::VarError::NotPresent => missing_tunnel_relay_auth_secret_error(),
        std::env::VarError::NotUnicode(_) => {
            format!("{TUNNEL_RELAY_AUTH_SECRET_ENV} must be valid UTF-8")
        }
    })?;
    resolve_tunnel_relay_auth_secret_value(Some(&value))
}

fn resolve_tunnel_relay_auth_secret_value(value: Option<&str>) -> Result<String, String> {
    let value = value.ok_or_else(missing_tunnel_relay_auth_secret_error)?;
    let value = value.trim();
    validate_tunnel_relay_auth_secret(value.as_bytes())?;
    Ok(value.to_string())
}

fn missing_tunnel_relay_auth_secret_error() -> String {
    format!("{TUNNEL_RELAY_AUTH_SECRET_ENV} is required for tunnel relay authentication")
}

fn validate_tunnel_relay_auth_secret(secret: &[u8]) -> Result<(), String> {
    if secret.len() < TUNNEL_RELAY_AUTH_SECRET_MIN_BYTES {
        return Err(format!(
            "{TUNNEL_RELAY_AUTH_SECRET_ENV} must contain at least {TUNNEL_RELAY_AUTH_SECRET_MIN_BYTES} bytes"
        ));
    }
    Ok(())
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn apply_embedded_tunnel_heartbeat(
    data: &GatewayDataState,
    runtime_state: &RuntimeState,
    authenticated_node_id: &str,
    authenticated_generation: &str,
    request_body: &[u8],
) -> Result<Vec<u8>, String> {
    let payload = parse_embedded_tunnel_heartbeat_request(request_body)?;
    let node_id = payload.node_id.trim().to_string();
    if node_id != authenticated_node_id {
        return Err("heartbeat node_id does not match authenticated tunnel node".to_string());
    }
    let claim = claim_tunnel_heartbeat(
        runtime_state,
        &node_id,
        &payload.heartbeat_session_id,
        payload.heartbeat_id,
    )
    .await?;
    if claim.is_none() {
        let node = data
            .find_proxy_node(&node_id)
            .await
            .map_err(|err| format!("heartbeat duplicate lookup failed: {err}"))?
            .filter(|node| node.tunnel_generation == authenticated_generation)
            .ok_or_else(|| "proxy tunnel credential was revoked".to_string())?;
        return Ok(build_embedded_tunnel_heartbeat_ack(
            &node,
            payload.heartbeat_id,
        ));
    }
    let claim = claim.expect("fresh heartbeat claim should be present");
    let mutation = ProxyNodeHeartbeatMutation {
        node_id: node_id.clone(),
        expected_tunnel_generation: Some(authenticated_generation.to_string()),
        heartbeat_interval: payload.heartbeat_interval,
        active_connections: payload.active_connections,
        total_requests_delta: payload.window_total_requests.or(payload.total_requests),
        avg_latency_ms: payload.avg_latency_ms,
        failed_requests_delta: payload.window_failed_requests.or(payload.failed_requests),
        dns_failures_delta: payload.window_dns_failures.or(payload.dns_failures),
        stream_errors_delta: payload.window_stream_errors.or(payload.stream_errors),
        proxy_metadata: payload.proxy_metadata,
        proxy_version: payload.proxy_version,
    };

    crate::state::decrypt_or_migrate_proxy_tunnel_psk(data, &node_id)
        .await
        .map_err(|err| format!("heartbeat security migration failed: {err}"))?;
    let node_result = data
        .apply_proxy_node_heartbeat(&mutation)
        .await
        .map_err(|err| format!("heartbeat sync failed: {err}"))
        .and_then(|node| {
            node.ok_or_else(|| format!("heartbeat sync failed: ProxyNode {node_id} 不存在"))
        });
    let node = match node_result {
        Ok(node) => {
            finish_tunnel_heartbeat_claim(runtime_state, claim).await;
            node
        }
        Err(error) => {
            finish_tunnel_heartbeat_claim(runtime_state, claim).await;
            return Err(error);
        }
    };

    Ok(build_embedded_tunnel_heartbeat_ack(
        &node,
        payload.heartbeat_id,
    ))
}

pub(crate) async fn claim_tunnel_heartbeat(
    runtime_state: &RuntimeState,
    node_id: &str,
    heartbeat_session_id: &str,
    heartbeat_id: u64,
) -> Result<Option<TunnelHeartbeatClaim>, String> {
    let state_key = tunnel_heartbeat_state_key(node_id, heartbeat_session_id)?;
    let lock_key = format!("{state_key}:lock");
    let owner = format!("tunnel-heartbeat:{node_id}");
    let Some(lease) = runtime_state
        .lock_try_acquire(&lock_key, &owner, TUNNEL_HEARTBEAT_LOCK_TTL)
        .await
        .map_err(|error| format!("heartbeat replay lock failed: {error}"))?
    else {
        return Err("heartbeat with this session is already being processed".to_string());
    };

    let previous_heartbeat_id = match runtime_state.kv_get(&state_key).await {
        Ok(Some(value)) => match value.parse::<u64>() {
            Ok(value) => Some(value),
            Err(_) => {
                let _ = runtime_state.lock_release(&lease).await;
                return Err("invalid heartbeat replay state".to_string());
            }
        },
        Ok(None) => None,
        Err(error) => {
            let _ = runtime_state.lock_release(&lease).await;
            return Err(format!("heartbeat replay state read failed: {error}"));
        }
    };

    if previous_heartbeat_id.is_some_and(|previous| heartbeat_id <= previous) {
        let _ = runtime_state.lock_release(&lease).await;
        return Ok(None);
    }

    if let Err(error) = runtime_state
        .kv_set(
            &state_key,
            heartbeat_id.to_string(),
            Some(TUNNEL_HEARTBEAT_STATE_TTL),
        )
        .await
    {
        let _ = runtime_state.lock_release(&lease).await;
        return Err(format!("heartbeat replay state write failed: {error}"));
    }

    Ok(Some(TunnelHeartbeatClaim { lease }))
}

pub(crate) async fn finish_tunnel_heartbeat_claim(
    runtime_state: &RuntimeState,
    claim: TunnelHeartbeatClaim,
) {
    if let Err(error) = runtime_state.lock_release(&claim.lease).await {
        warn!(error = %error, "failed to release heartbeat replay lock after commit");
    }
}

fn tunnel_heartbeat_state_key(node_id: &str, heartbeat_session_id: &str) -> Result<String, String> {
    validate_tunnel_heartbeat_session_id(heartbeat_session_id)?;
    let heartbeat_session_id = heartbeat_session_id.trim();

    let mut digest = Sha256::new();
    digest.update(node_id.as_bytes());
    digest.update([0]);
    digest.update(heartbeat_session_id.as_bytes());
    Ok(format!(
        "{TUNNEL_HEARTBEAT_STATE_KEY_PREFIX}{:x}",
        digest.finalize()
    ))
}

pub(crate) fn validate_tunnel_heartbeat_session_id(
    heartbeat_session_id: &str,
) -> Result<(), String> {
    let heartbeat_session_id = heartbeat_session_id.trim();
    if heartbeat_session_id.is_empty()
        || heartbeat_session_id.len() > 128
        || !heartbeat_session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err("invalid heartbeat payload".to_string());
    }
    Ok(())
}

async fn apply_embedded_tunnel_node_status(
    data: &GatewayDataState,
    node_id: &str,
    node_generation: &str,
    connected: bool,
    conn_count: usize,
    observed_at_unix_secs: Option<u64>,
) -> Result<(), String> {
    let mutation = ProxyNodeTunnelStatusMutation {
        node_id: node_id.trim().to_string(),
        expected_tunnel_generation: Some(node_generation.to_string()),
        connected,
        conn_count: conn_count.min(i32::MAX as usize) as i32,
        detail: None,
        observed_at_unix_secs,
    };

    data.update_proxy_node_tunnel_status(&mutation)
        .await
        .map_err(|err| format!("node status sync failed: {err}"))?
        .ok_or_else(|| "proxy tunnel credential was revoked".to_string())?;
    Ok(())
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
    if node_id.is_empty()
        || node_id.len() > 36
        || payload.heartbeat_id == 0
        || tunnel_heartbeat_state_key(node_id, &payload.heartbeat_session_id).is_err()
    {
        return Err("invalid heartbeat payload".to_string());
    }
    if payload
        .heartbeat_interval
        .is_some_and(|value| !(5..=600).contains(&value))
        || payload.active_connections.is_some_and(|value| value < 0)
        || payload.total_requests.is_some_and(|value| value < 0)
        || payload.window_total_requests.is_some_and(|value| value < 0)
        || payload.avg_latency_ms.is_some_and(|value| value < 0.0)
        || payload.failed_requests.is_some_and(|value| value < 0)
        || payload
            .window_failed_requests
            .is_some_and(|value| value < 0)
        || payload.dns_failures.is_some_and(|value| value < 0)
        || payload.window_dns_failures.is_some_and(|value| value < 0)
        || payload.stream_errors.is_some_and(|value| value < 0)
        || payload.window_stream_errors.is_some_and(|value| value < 0)
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
        apply_embedded_tunnel_heartbeat, apply_embedded_tunnel_node_status,
        build_owner_forward_target, build_owner_forward_target_with_policy,
        build_tunnel_affinity_auth_metadata, build_tunnel_owner_relay_url, build_tunnel_probe_meta,
        current_unix_secs, encode_tunnel_relay_envelope, owner_forward_client_for_url,
        prepare_owner_relay_request_body, prepare_owner_relay_request_body_with_limits,
        resolve_owner_forward_target, resolve_tunnel_relay_auth_secret_value,
        tunnel_attachment_key, tunnel_relay_private_host_matches_allowlist,
        validate_tunnel_relay_transport_url, AppState, GatewayDataState, TunnelAttachmentDirectory,
        TunnelAttachmentRecord, MAX_OWNER_FORWARD_DNS_ADDRESSES,
    };
    use aether_contracts::tunnel::{
        try_decode_tunnel_relay_request_meta, TUNNEL_RELAY_FORWARDED_BY_HEADER,
        TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
    };
    use aether_data::repository::proxy_nodes::{
        InMemoryProxyNodeRepository, ProxyNodeReadRepository, StoredProxyNode,
    };
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use axum::body::{Body, Bytes};
    use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
    use axum::routing::{any, post};
    use axum::Router;
    use serde_json::json;
    use std::io;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const RELAY_TEST_SECRET: &str = "relay-test-secret-at-least-32-bytes";
    const TEST_TUNNEL_GENERATION: &str = "test-generation-1";

    #[test]
    fn tunnel_relay_secret_requires_an_independent_32_byte_value() {
        let missing = resolve_tunnel_relay_auth_secret_value(None)
            .expect_err("missing relay secret should fail");
        assert!(missing.contains("AETHER_TUNNEL_RELAY_AUTH_SECRET"));

        let short = resolve_tunnel_relay_auth_secret_value(Some(&"x".repeat(31)))
            .expect_err("31-byte relay secret should fail");
        assert!(short.contains("at least 32 bytes"));

        assert_eq!(
            resolve_tunnel_relay_auth_secret_value(Some(&"x".repeat(32)))
                .expect("32-byte relay secret should pass"),
            "x".repeat(32)
        );
    }

    #[test]
    fn tunnel_relay_url_policy_requires_https_except_for_loopback_http() {
        for accepted in [
            "https://gateway.example.com/base",
            "http://localhost:8084",
            "http://localhost.:8084",
            "http://127.0.0.1:8084",
            "http://[::1]:8084",
        ] {
            let url = url::Url::parse(accepted).expect("accepted URL should parse");
            validate_tunnel_relay_transport_url(&url)
                .unwrap_or_else(|error| panic!("{accepted} should be accepted: {error}"));
        }

        for rejected in [
            "http://gateway.example.com",
            "http://10.0.0.2:8084",
            "ftp://gateway.example.com",
            "https://user:password@gateway.example.com",
            "file:///tmp/relay",
        ] {
            let url = url::Url::parse(rejected).expect("rejected URL should still parse");
            assert!(
                validate_tunnel_relay_transport_url(&url).is_err(),
                "{rejected} should be rejected"
            );
        }
    }

    #[test]
    fn tunnel_owner_relay_url_uses_the_validated_base_and_escapes_node_id() {
        assert_eq!(
            build_tunnel_owner_relay_url("https://gateway.example.com/base", "node/a")
                .expect("HTTPS relay URL should build"),
            "https://gateway.example.com/base/api/internal/tunnel/relay/node%2Fa"
        );
        assert!(build_tunnel_owner_relay_url("http://gateway.example.com", "node-1").is_err());
        for invalid_node_id in [
            "", "   ", ".", "..", "%2e", "%2E", "%2e%2e", "%2E%2e", ".%2E", "%2e.",
        ] {
            assert!(
                build_tunnel_owner_relay_url("https://gateway.example.com/base", invalid_node_id)
                    .is_err(),
                "reserved or empty node ID {invalid_node_id:?} must not collapse the relay path"
            );
        }
    }

    #[test]
    fn owner_forward_target_deduplicates_addresses_and_preserves_ports() {
        let target = build_owner_forward_target(
            "https",
            "gateway.example.com",
            8443,
            vec![
                "[2001:4860:4860::8888]:8443".parse().unwrap(),
                "8.8.8.8:8443".parse().unwrap(),
                "8.8.8.8:8443".parse().unwrap(),
            ],
        )
        .expect("valid DNS answers should produce a target");
        assert_eq!(target.host, "gateway.example.com");
        assert_eq!(target.port, 8443);
        assert_eq!(target.addresses.len(), 2);
        assert!(!target.literal_host);
    }

    #[test]
    fn owner_forward_target_rejects_empty_or_mismatched_dns_answers() {
        assert!(build_owner_forward_target("https", "gateway.example.com", 443, vec![]).is_err());
        assert!(build_owner_forward_target(
            "https",
            "gateway.example.com",
            443,
            vec!["192.0.2.10:8443".parse().unwrap()],
        )
        .is_err());
    }

    #[test]
    fn owner_forward_target_rejects_an_oversized_dns_answer_set() {
        let addresses = (1..=MAX_OWNER_FORWARD_DNS_ADDRESSES + 1)
            .map(|octet| SocketAddr::from(([192, 0, 2, octet as u8], 443)))
            .collect();
        assert!(
            build_owner_forward_target("https", "gateway.example.com", 443, addresses).is_err()
        );
    }

    #[test]
    fn owner_forward_target_rejects_non_loopback_http_dns_answers() {
        assert!(build_owner_forward_target(
            "http",
            "localhost",
            8084,
            vec!["192.0.2.10:8084".parse().unwrap()],
        )
        .is_err());
        assert!(build_owner_forward_target(
            "http",
            "localhost",
            8084,
            vec![
                "127.0.0.1:8084".parse().unwrap(),
                "[::1]:8084".parse().unwrap(),
            ],
        )
        .is_ok());

        // Private HTTPS owner deployments require an explicit deployment
        // opt-in; the default path must not be an SSRF primitive.
        assert!(build_owner_forward_target(
            "https",
            "gateway.internal",
            8443,
            vec!["10.0.0.8:8443".parse().unwrap()],
        )
        .is_err());
        assert!(build_owner_forward_target_with_policy(
            "https",
            "gateway.internal",
            8443,
            vec!["10.0.0.8:8443".parse().unwrap()],
            true,
        )
        .is_ok());
    }

    #[test]
    fn private_relay_host_allowlist_matches_exact_dns_names_only() {
        assert!(tunnel_relay_private_host_matches_allowlist(
            "gateway-a.internal.",
            "gateway-a.internal, gateway-b.internal"
        ));
        assert!(tunnel_relay_private_host_matches_allowlist(
            "GATEWAY-B.INTERNAL",
            "gateway-a.internal, gateway-b.internal."
        ));
        assert!(!tunnel_relay_private_host_matches_allowlist(
            "api.gateway-a.internal",
            "gateway-a.internal"
        ));
        assert!(!tunnel_relay_private_host_matches_allowlist(
            "gateway-a.internal.evil.example",
            ".internal"
        ));
    }

    #[test]
    fn owner_forward_target_marks_literal_ipv4_and_ipv6_hosts() {
        let ipv4 = build_owner_forward_target(
            "http",
            "127.0.0.1",
            8084,
            vec!["127.0.0.1:8084".parse().unwrap()],
        )
        .expect("IPv4 literal should be valid");
        let ipv6 =
            build_owner_forward_target("http", "::1", 8084, vec!["[::1]:8084".parse().unwrap()])
                .expect("IPv6 literal should be valid");
        assert!(ipv4.literal_host);
        assert!(ipv6.literal_host);
    }

    #[tokio::test]
    async fn owner_forward_resolution_recognizes_bracketed_ipv6_literal_url() {
        let target = resolve_owner_forward_target("http://[::1]:8084/api/internal/tunnel/relay/n")
            .await
            .expect("loopback IPv6 owner URL should resolve without DNS");
        assert_eq!(target.host, "::1");
        assert_eq!(target.addresses, vec!["[::1]:8084".parse().unwrap()]);
        assert!(target.literal_host);
    }

    #[tokio::test]
    async fn owner_forward_domain_client_uses_pinned_transport_instead_of_shared_proxy() {
        let owner_app = Router::new().route("/relay", post(|| async { StatusCode::NO_CONTENT }));
        let owner_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owner listener should bind");
        let owner_port = owner_listener
            .local_addr()
            .expect("owner address should be available")
            .port();
        let owner_server = tokio::spawn(async move {
            axum::serve(owner_listener, owner_app)
                .await
                .expect("owner server should run");
        });

        let proxy_hits = Arc::new(AtomicUsize::new(0));
        let proxy_hits_for_route = Arc::clone(&proxy_hits);
        let proxy_app = Router::new().fallback(any(move || {
            let proxy_hits = Arc::clone(&proxy_hits_for_route);
            async move {
                proxy_hits.fetch_add(1, Ordering::Relaxed);
                StatusCode::BAD_GATEWAY
            }
        }));
        let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("proxy listener should bind");
        let proxy_url = format!(
            "http://{}",
            proxy_listener
                .local_addr()
                .expect("proxy address should be available")
        );
        let proxy_server = tokio::spawn(async move {
            axum::serve(proxy_listener, proxy_app)
                .await
                .expect("proxy server should run");
        });

        let shared_client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(&proxy_url).expect("proxy URL should build"))
            .build()
            .expect("shared client should build");
        let owner_url = format!("http://localhost:{owner_port}/relay");
        let pinned_client = owner_forward_client_for_url(&shared_client, &owner_url)
            .await
            .expect("localhost owner URL should resolve and build a pinned client");
        let response = pinned_client
            .post(&owner_url)
            .send()
            .await
            .expect("pinned owner request should bypass the shared proxy");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(proxy_hits.load(Ordering::Relaxed), 0);

        owner_server.abort();
        proxy_server.abort();
        let _ = owner_server.await;
        let _ = proxy_server.await;
    }

    #[test]
    fn tunnel_affinity_auth_metadata_is_deterministic_and_binds_identity_and_path() {
        let method = Method::POST;
        let uri: Uri = "/v1/chat/completions?stream=false"
            .parse()
            .expect("URI should parse");
        let mut headers = HeaderMap::new();
        headers.insert(
            crate::constants::GATEWAY_HEADER,
            HeaderValue::from_static("rust-phase3b-affinity"),
        );
        headers.insert(
            crate::constants::TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
            HeaderValue::from_static("gateway-a"),
        );
        headers.insert(
            crate::constants::TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
            HeaderValue::from_static("gateway-b"),
        );
        headers.insert(
            crate::constants::TUNNEL_AFFINITY_NODE_ID_HEADER,
            HeaderValue::from_static("node-1"),
        );
        headers.insert(
            crate::constants::TRUSTED_AUTH_USER_ID_HEADER,
            HeaderValue::from_static("user-1"),
        );

        let first = build_tunnel_affinity_auth_metadata(&method, &uri, &headers)
            .expect("metadata should build");
        let second = build_tunnel_affinity_auth_metadata(&method, &uri, &headers)
            .expect("metadata should be deterministic");
        assert_eq!(first, second);

        let other_uri: Uri = "/v1/responses".parse().expect("URI should parse");
        assert_ne!(
            first,
            build_tunnel_affinity_auth_metadata(&method, &other_uri, &headers)
                .expect("metadata should build")
        );
        headers.insert(
            crate::constants::TRUSTED_AUTH_USER_ID_HEADER,
            HeaderValue::from_static("user-2"),
        );
        assert_ne!(
            first,
            build_tunnel_affinity_auth_metadata(&method, &uri, &headers)
                .expect("metadata should build")
        );
    }

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
        .with_tunnel_generation(TEST_TUNNEL_GENERATION.to_string())
    }

    #[test]
    fn routed_tunnel_probe_builds_a_valid_owner_relay_envelope() {
        let meta = build_tunnel_probe_meta("https://probe.example/health", 7);
        let envelope = encode_tunnel_relay_envelope(&meta).expect("probe should encode");
        let (decoded, body_offset) = try_decode_tunnel_relay_request_meta(&envelope)
            .expect("probe envelope should decode")
            .expect("probe envelope should contain complete metadata");

        assert_eq!(decoded.method, "GET");
        assert_eq!(decoded.url, "https://probe.example/health");
        assert_eq!(decoded.timeout, 7);
        assert_eq!(body_offset, envelope.len());
    }

    #[tokio::test]
    async fn routed_tunnel_probe_forwards_to_the_attachment_owner() {
        let captured = Arc::new(Mutex::new(None::<(HeaderMap, Bytes)>));
        let captured_for_route = Arc::clone(&captured);
        let app = Router::new().route(
            "/api/internal/tunnel/relay/{node_id}",
            post(move |headers: HeaderMap, body: Bytes| {
                let captured = Arc::clone(&captured_for_route);
                async move {
                    *captured.lock().expect("capture lock") = Some((headers, body));
                    StatusCode::NO_CONTENT
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owner listener should bind");
        let owner_base_url = format!("http://{}", listener.local_addr().expect("owner address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("owner server should run");
        });

        let owner = TunnelAttachmentRecord {
            gateway_instance_id: "gateway-b".to_string(),
            relay_base_url: owner_base_url,
            tunnel_generation: TEST_TUNNEL_GENERATION.to_string(),
            conn_count: 1,
            observed_at_unix_secs: current_unix_secs(),
        };
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-remote",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(repository)
            .with_system_config_values_for_tests(vec![(
                tunnel_attachment_key("node-remote"),
                serde_json::to_value(owner).expect("owner record should serialize"),
            )]);
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(data)
            .with_tunnel_identity_and_relay_secret_for_tests(
                "gateway-a",
                Some("https://gateway-a.internal"),
                RELAY_TEST_SECRET,
            );

        let status = state
            .tunnel
            .probe_node_url_routed(&state, "node-remote", "https://probe.example/health", 5)
            .await
            .expect("remote owner probe should succeed");
        assert_eq!(status, StatusCode::NO_CONTENT.as_u16());

        let (headers, body) = captured
            .lock()
            .expect("capture lock")
            .take()
            .expect("owner should receive the probe");
        assert_eq!(
            headers
                .get(TUNNEL_RELAY_FORWARDED_BY_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("gateway-a")
        );
        assert_eq!(
            headers
                .get(TUNNEL_RELAY_OWNER_INSTANCE_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("gateway-b")
        );
        assert_eq!(
            headers
                .get(super::TUNNEL_RELAY_ROLLOUT_PROBE_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(super::TUNNEL_RELAY_ROLLOUT_PROBE_VALUE)
        );
        let (meta, body_offset) = try_decode_tunnel_relay_request_meta(&body)
            .expect("owner probe envelope should decode")
            .expect("owner probe metadata should be complete");
        assert_eq!(meta.url, "https://probe.example/health");
        assert_eq!(body_offset, body.len());

        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn owner_relay_body_preparation_rejects_invalid_metadata() {
        let mut envelope = Vec::new();
        envelope.extend_from_slice(&1u32.to_be_bytes());
        envelope.push(b'{');

        let error = prepare_owner_relay_request_body(Body::from(envelope))
            .await
            .err()
            .expect("invalid metadata should fail");

        assert!(error.contains("invalid relay metadata"));
    }

    #[tokio::test]
    async fn owner_relay_body_preparation_hashes_a_large_body_in_one_chunk() {
        let meta = build_tunnel_probe_meta("https://probe.example/health", 7);
        let body = vec![b'x'; aether_contracts::tunnel::MAX_TUNNEL_RELAY_META_LEN + 1024];
        let envelope = {
            let metadata = serde_json::to_vec(&meta).expect("metadata should encode");
            let mut envelope = Vec::with_capacity(4 + metadata.len() + body.len());
            envelope.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
            envelope.extend_from_slice(&metadata);
            envelope.extend_from_slice(&body);
            envelope
        };

        let spool = prepare_owner_relay_request_body(Body::from(envelope))
            .await
            .expect("single-chunk relay body should prepare");
        assert_eq!(spool.payload_digest().body_len(), body.len() as u64);
        assert!(spool.payload_digest().matches_body(&body));
    }

    #[tokio::test]
    async fn owner_relay_body_preparation_rejects_envelope_above_hard_limit() {
        let meta = build_tunnel_probe_meta("https://probe.example/health", 7);
        let envelope = encode_tunnel_relay_envelope(&meta).expect("metadata should encode");
        let limit = envelope.len() as u64;
        let oversized = envelope
            .into_iter()
            .chain(std::iter::once(b'x'))
            .collect::<Vec<_>>();
        let error = prepare_owner_relay_request_body_with_limits(
            Body::from(oversized),
            limit,
            Duration::from_secs(1),
        )
        .await
        .err()
        .expect("oversized relay envelope should fail");
        assert!(error.contains("tunnel relay body exceeds"));
    }

    #[tokio::test]
    async fn owner_relay_body_preparation_times_out_slow_body_streams() {
        let meta = build_tunnel_probe_meta("https://probe.example/health", 7);
        let envelope = encode_tunnel_relay_envelope(&meta).expect("metadata should encode");
        let body = Body::from_stream(async_stream::stream! {
            tokio::time::sleep(Duration::from_millis(50)).await;
            yield Ok::<Bytes, io::Error>(Bytes::from(envelope));
        });
        let error = prepare_owner_relay_request_body_with_limits(
            body,
            1024 * 1024,
            Duration::from_millis(10),
        )
        .await
        .err()
        .expect("slow relay body should time out");
        assert!(error.contains("tunnel relay body read timed out"));
    }

    #[tokio::test]
    async fn embedded_tunnel_heartbeat_updates_proxy_node_repository() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-123",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));

        let ack = apply_embedded_tunnel_heartbeat(
            &data,
            &RuntimeState::memory(MemoryRuntimeStateConfig::default()),
            "node-123",
            TEST_TUNNEL_GENERATION,
            br#"{
                "node_id": "node-123",
                "heartbeat_session_id": "session-1",
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
    async fn embedded_tunnel_heartbeat_replay_does_not_repeat_counter_deltas() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-replay",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));
        let runtime_state = RuntimeState::memory(MemoryRuntimeStateConfig::default());
        let heartbeat = br#"{
            "node_id": "node-replay",
            "heartbeat_session_id": "session-replay",
            "heartbeat_id": 7,
            "total_requests": 9,
            "failed_requests": 1,
            "dns_failures": 2,
            "stream_errors": 3
        }"#;

        for _ in 0..2 {
            let ack = apply_embedded_tunnel_heartbeat(
                &data,
                &runtime_state,
                "node-replay",
                TEST_TUNNEL_GENERATION,
                heartbeat,
            )
            .await
            .expect("original and replayed heartbeat should both receive an ACK");
            let ack: serde_json::Value =
                serde_json::from_slice(&ack).expect("ack payload should parse");
            assert_eq!(ack["heartbeat_id"], 7);
        }

        let node = repository
            .find_proxy_node("node-replay")
            .await
            .expect("lookup should succeed")
            .expect("node should exist");
        assert_eq!(node.total_requests, 9);
        assert_eq!(node.failed_requests, 1);
        assert_eq!(node.dns_failures, 2);
        assert_eq!(node.stream_errors, 3);
    }

    #[tokio::test]
    async fn embedded_tunnel_heartbeat_rejects_missing_heartbeat_id() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-123",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));

        let error = apply_embedded_tunnel_heartbeat(
            &data,
            &RuntimeState::memory(MemoryRuntimeStateConfig::default()),
            "node-123",
            TEST_TUNNEL_GENERATION,
            br#"{
                "node_id": "node-123",
                "heartbeat_session_id": "session-1",
                "heartbeat_interval": 45,
                "active_connections": 5
            }"#,
        )
        .await
        .expect_err("heartbeat without heartbeat_id should fail");

        assert_eq!(error, "invalid heartbeat payload");
    }

    #[tokio::test]
    async fn embedded_tunnel_heartbeat_rejects_a_different_authenticated_node() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "victim-node",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));

        let error = apply_embedded_tunnel_heartbeat(
            &data,
            &RuntimeState::memory(MemoryRuntimeStateConfig::default()),
            "attacker-node",
            TEST_TUNNEL_GENERATION,
            br#"{
                "node_id": "victim-node",
                "heartbeat_session_id": "session-1",
                "heartbeat_id": 43,
                "active_connections": 99,
                "total_requests": 500,
                "proxy_metadata": {
                    "tunnel_security": {
                        "mode": "disabled",
                        "encryption_key": "attacker-controlled"
                    }
                }
            }"#,
        )
        .await
        .expect_err("cross-node heartbeat must fail");

        assert_eq!(
            error,
            "heartbeat node_id does not match authenticated tunnel node"
        );
        let node = repository
            .find_proxy_node("victim-node")
            .await
            .expect("lookup should succeed")
            .expect("victim should still exist");
        assert_eq!(node.active_connections, 0);
        assert_eq!(node.total_requests, 0);
    }

    #[tokio::test]
    async fn embedded_tunnel_node_status_updates_proxy_node_repository() {
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
            "node-123",
        )]));
        let data = GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository));

        apply_embedded_tunnel_node_status(
            &data,
            "node-123",
            TEST_TUNNEL_GENERATION,
            true,
            4,
            Some(1_800_000_123),
        )
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
        tokio::time::timeout(Duration::from_secs(5), async {
            let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
                "node-123",
            )]));
            let data = GatewayDataState::with_proxy_node_repository_for_tests(repository)
                .with_system_config_values_for_tests(vec![]);
            let directory = TunnelAttachmentDirectory::for_tests(
                "gateway-a",
                Some("http://gateway-a.internal"),
                90,
            );
            let observed_at_unix_secs = current_unix_secs();

            directory
                .sync_node_status(
                    &data,
                    "node-123",
                    TEST_TUNNEL_GENERATION,
                    true,
                    2,
                    observed_at_unix_secs,
                )
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
            assert_eq!(record.observed_at_unix_secs, observed_at_unix_secs);

            directory
                .sync_node_status(
                    &data,
                    "node-123",
                    TEST_TUNNEL_GENERATION,
                    false,
                    0,
                    observed_at_unix_secs.saturating_add(1),
                )
                .await
                .expect("attachment should clear");
            assert!(directory
                .lookup_owner(&data, "node-123")
                .await
                .expect("lookup should succeed")
                .is_none());
        })
        .await
        .expect("attachment directory scenario should complete before timeout");
    }

    #[tokio::test]
    async fn stale_gateway_disconnect_cannot_delete_new_attachment_owner() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
                "node-123",
            )]));
            let data = Arc::new(
                GatewayDataState::with_proxy_node_repository_for_tests(repository)
                    .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new()),
            );
            let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
            let gateway_a = TunnelAttachmentDirectory::for_tests(
                "gateway-a",
                Some("http://gateway-a.internal"),
                90,
            )
            .with_runtime_state(Arc::clone(&runtime));
            let gateway_b = TunnelAttachmentDirectory::for_tests(
                "gateway-b",
                Some("http://gateway-b.internal"),
                90,
            )
            .with_runtime_state(runtime);
            let observed_at_unix_secs = current_unix_secs();

            gateway_a
                .sync_node_status(
                    data.as_ref(),
                    "node-123",
                    TEST_TUNNEL_GENERATION,
                    true,
                    1,
                    observed_at_unix_secs,
                )
                .await
                .expect("gateway A should publish attachment");
            gateway_b
                .sync_node_status(
                    data.as_ref(),
                    "node-123",
                    TEST_TUNNEL_GENERATION,
                    true,
                    1,
                    observed_at_unix_secs.saturating_add(1),
                )
                .await
                .expect("gateway B should publish replacement attachment");
            gateway_a
                .sync_node_status(
                    data.as_ref(),
                    "node-123",
                    TEST_TUNNEL_GENERATION,
                    false,
                    0,
                    observed_at_unix_secs.saturating_add(2),
                )
                .await
                .expect("stale gateway disconnect should be harmless");

            let record = gateway_b
                .lookup_owner(data.as_ref(), "node-123")
                .await
                .expect("attachment lookup should succeed")
                .expect("new owner attachment should remain");
            assert_eq!(record.gateway_instance_id, "gateway-b");
        })
        .await
        .expect("stale disconnect scenario should complete before timeout");
    }

    #[tokio::test]
    async fn tunnel_attachment_directory_ignores_expired_attachment_records() {
        let stale = TunnelAttachmentRecord {
            gateway_instance_id: "gateway-b".to_string(),
            relay_base_url: "http://gateway-b.internal".to_string(),
            tunnel_generation: "test-generation-stale".to_string(),
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
