use std::fmt;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use aether_runtime::{FileLoggingConfig, LogDestination, LogRotation, ServiceRuntimeConfig};
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::hardware::HardwareInfo;

/// Fields that existed in 0.1.x but were removed in 0.2.0.
const LEGACY_ONLY_KEYS: &[&str] = &[
    "hmac_key",
    "listen_port",
    "timestamp_tolerance",
    "connect_timeout_secs",
    "tls_handshake_timeout_secs",
    "enable_tls",
    "tls_cert",
    "tls_key",
];
const REMOVED_TUNNEL_SECONDS_KEYS: &[&str] = &[
    "tunnel_ping_interval_secs",
    "tunnel_connect_timeout_secs",
    "tunnel_stale_timeout_secs",
];
const REMOVED_SINGLE_SERVER_KEYS: &[&str] = &["aether_url", "management_token"];
/// Configuration keys that no longer affect runtime behavior but are ignored
/// while loading so existing installations can upgrade without editing TOML.
const IGNORED_CONFIG_KEYS: &[&str] = &["redirect_replay_budget_bytes"];

/// Fields renamed from 0.1.x `delegate_*` to 0.2.0 `upstream_*`.
const DELEGATE_TO_UPSTREAM: &[(&str, &str)] = &[
    (
        "delegate_connect_timeout_secs",
        "upstream_connect_timeout_secs",
    ),
    (
        "delegate_pool_max_idle_per_host",
        "upstream_pool_max_idle_per_host",
    ),
    (
        "delegate_pool_idle_timeout_secs",
        "upstream_pool_idle_timeout_secs",
    ),
    ("delegate_tcp_keepalive_secs", "upstream_tcp_keepalive_secs"),
    ("delegate_tcp_nodelay", "upstream_tcp_nodelay"),
];

pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;
pub const DEFAULT_LOG_RETENTION_DAYS: u64 = 7;
pub const DEFAULT_LOG_MAX_FILES: usize = 30;
pub const DEFAULT_LOG_DIR: &str = "logs";
pub const DEFAULT_TUNNEL_RECONNECT_BASE_MS: u64 = 50;
pub const DEFAULT_TUNNEL_RECONNECT_MAX_MS: u64 = 250;
pub const DEFAULT_TUNNEL_PING_INTERVAL_MS: u64 = 10_000;
pub const DEFAULT_TUNNEL_CONNECT_TIMEOUT_MS: u64 = 3_000;
pub const DEFAULT_TUNNEL_STALE_TIMEOUT_MS: u64 = 30_000;
pub const DEFAULT_TUNNEL_SCALE_CHECK_INTERVAL_MS: u64 = 1_000;
pub const DEFAULT_TUNNEL_SCALE_UP_THRESHOLD_PERCENT: u32 = 50;
pub const DEFAULT_TUNNEL_SCALE_DOWN_THRESHOLD_PERCENT: u32 = 35;
pub const DEFAULT_TUNNEL_SCALE_DOWN_GRACE_SECS: u64 = 15;
pub const DEFAULT_TUNNEL_STREAM_INITIAL_WINDOW_BYTES: u32 = 4 * 1024 * 1024;
pub const DEFAULT_TUNNEL_DRAIN_DEADLINE_MS: u64 = 30_000;
pub const DEFAULT_UPSTREAM_CLIENT_POOL_CAPACITY: usize = 256;
const AUTO_TUNNEL_CONNECTIONS_REDUNDANT_FLOOR: u64 = 2;
const AUTO_TUNNEL_CONNECTIONS_BASE_CAP: u64 = 4;
// Bias the automatic pool toward a per-device upper band without letting
// tiny nodes fan out into too many idle tunnels.
const AUTO_TUNNEL_CONNECTIONS_PER_CPU_CAP: u64 = 4;
const AUTO_TUNNEL_CONNECTIONS_MAX_CAP: u64 = 32;

const TUNNEL_PING_INTERVAL_MS_ENV: &str = "AETHER_TUNNEL_PING_INTERVAL_MS";
const TUNNEL_CONNECT_TIMEOUT_MS_ENV: &str = "AETHER_TUNNEL_CONNECT_TIMEOUT_MS";
const TUNNEL_STALE_TIMEOUT_MS_ENV: &str = "AETHER_TUNNEL_STALE_TIMEOUT_MS";
const TUNNEL_PROFILE_ENV: &str = "AETHER_TUNNEL_PROFILE";
const TUNNEL_STREAM_INITIAL_WINDOW_BYTES_ENV: &str = "AETHER_TUNNEL_STREAM_INITIAL_WINDOW_BYTES";
const TUNNEL_DRAIN_DEADLINE_MS_ENV: &str = "AETHER_TUNNEL_DRAIN_DEADLINE_MS";

// The configuration contains only scalar settings and a bounded list of
// server entries. Refuse an unexpectedly large local file before TOML parsing
// so a replaced or corrupted config cannot force an unbounded allocation at
// service startup.
const MAX_CONFIG_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunnelPoolSizing {
    pub initial_connections: u32,
    pub max_connections: u32,
}
#[derive(clap::ValueEnum, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelLogDestinationArg {
    Stdout,
    File,
    Both,
}

impl From<TunnelLogDestinationArg> for LogDestination {
    fn from(value: TunnelLogDestinationArg) -> Self {
        match value {
            TunnelLogDestinationArg::Stdout => LogDestination::Stdout,
            TunnelLogDestinationArg::File => LogDestination::File,
            TunnelLogDestinationArg::Both => LogDestination::Both,
        }
    }
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelLogRotationArg {
    Hourly,
    Daily,
}

impl From<TunnelLogRotationArg> for LogRotation {
    fn from(value: TunnelLogRotationArg) -> Self {
        match value {
            TunnelLogRotationArg::Hourly => LogRotation::Hourly,
            TunnelLogRotationArg::Daily => LogRotation::Daily,
        }
    }
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelProfileArg {
    Lite,
    Standard,
    Throughput,
}

impl fmt::Display for TunnelProfileArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TunnelProfileArg::Lite => "lite",
            TunnelProfileArg::Standard => "standard",
            TunnelProfileArg::Throughput => "throughput",
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelSecurity {
    Off,
    NonTlsRequired,
}

impl fmt::Display for TunnelSecurity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TunnelSecurity::Off => "off",
            TunnelSecurity::NonTlsRequired => "non_tls_required",
        })
    }
}

impl FromStr for TunnelSecurity {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "off" => Ok(Self::Off),
            "non_tls_required" | "non-tls-required" => Ok(Self::NonTlsRequired),
            other => Err(format!(
                "invalid tunnel_security {other:?}; expected off or non_tls_required"
            )),
        }
    }
}

pub fn validate_tunnel_encryption_key(value: &str) -> anyhow::Result<()> {
    aether_contracts::tunnel_security::decode_psk(value)
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!(err))
}

pub fn effective_tunnel_security(
    aether_url: &str,
    configured: Option<TunnelSecurity>,
    tunnel_encryption_key: Option<&str>,
) -> TunnelSecurity {
    match configured {
        Some(TunnelSecurity::NonTlsRequired) => return TunnelSecurity::NonTlsRequired,
        Some(TunnelSecurity::Off) => return TunnelSecurity::Off,
        None => {}
    }
    if aether_url.trim_start().starts_with("http://")
        && tunnel_encryption_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
    {
        return TunnelSecurity::NonTlsRequired;
    }
    TunnelSecurity::Off
}

pub(crate) fn validate_aether_url(value: &str) -> anyhow::Result<()> {
    let value = value.trim();
    let parsed = url::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("aether_url must be an absolute HTTP(S) URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        anyhow::bail!("aether_url must be an absolute HTTP(S) URL");
    }
    if !aether_http::is_https_or_loopback_http_url(&parsed) {
        anyhow::bail!(
            "aether_url must use HTTPS; HTTP is allowed only for a literal loopback host"
        );
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("aether_url must not contain embedded credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        anyhow::bail!("aether_url must not contain a query string or fragment");
    }
    Ok(())
}

pub(crate) fn aether_url_for_log(value: &str) -> String {
    let Ok(parsed) = url::Url::parse(value.trim()) else {
        return "<invalid-aether-url>".to_string();
    };
    if !matches!(parsed.scheme(), "http" | "https" | "ws" | "wss") || parsed.host_str().is_none() {
        return "<invalid-aether-url>".to_string();
    }
    parsed.origin().ascii_serialization()
}

/// Aether tunnel agent.
///
/// Deployed on overseas VPS to relay API traffic for Aether instances
/// behind the GFW. Connects to Aether via WebSocket tunnel, registers
/// with Aether, and relays upstream requests.
#[derive(Parser, Clone)]
#[command(version, about)]
pub struct Config {
    /// Aether server URL (e.g. https://aether.example.com)
    #[arg(long, env = "AETHER_TUNNEL_AETHER_URL")]
    pub aether_url: String,

    /// Management Token for Aether admin API (ae_xxx)
    #[arg(long, env = "AETHER_TUNNEL_MANAGEMENT_TOKEN")]
    pub management_token: String,

    /// Public IP address of this node (auto-detected if omitted)
    #[arg(long, env = "AETHER_TUNNEL_PUBLIC_IP")]
    pub public_ip: Option<String>,

    /// Human-readable node name
    #[arg(long, env = "AETHER_TUNNEL_NODE_NAME")]
    pub node_name: String,

    /// Application-layer tunnel security mode.
    #[arg(
        long,
        env = "AETHER_TUNNEL_SECURITY",
        default_value_t = TunnelSecurity::Off
    )]
    pub tunnel_security: TunnelSecurity,

    /// Base64-encoded 32-byte PSK used when tunnel_security=non_tls_required.
    #[arg(long, env = "AETHER_TUNNEL_ENCRYPTION_KEY")]
    pub tunnel_encryption_key: Option<String>,

    /// Region label (e.g. ap-northeast-1)
    #[arg(long, env = "AETHER_TUNNEL_NODE_REGION")]
    pub node_region: Option<String>,

    /// Heartbeat interval in seconds
    #[arg(
        long,
        env = "AETHER_TUNNEL_HEARTBEAT_INTERVAL",
        default_value_t = DEFAULT_HEARTBEAT_INTERVAL_SECS
    )]
    pub heartbeat_interval: u64,

    /// Allowed destination ports (default: 80,443,8080,8443)
    #[arg(
        long,
        env = "AETHER_TUNNEL_ALLOWED_PORTS",
        value_delimiter = ',',
        default_values_t = vec![80, 443, 8080, 8443]
    )]
    pub allowed_ports: Vec<u16>,

    /// Allow private/reserved upstream IP targets. Disabled by default; enable
    /// explicitly only for deployments that require access to private services.
    #[arg(
        long,
        env = "AETHER_TUNNEL_ALLOW_PRIVATE_TARGETS",
        default_value_t = false
    )]
    pub allow_private_targets: bool,

    /// Aether API request timeout in seconds
    #[arg(
        long,
        env = "AETHER_TUNNEL_AETHER_REQUEST_TIMEOUT",
        default_value_t = 10
    )]
    pub aether_request_timeout_secs: u64,

    /// Aether API connect timeout in seconds
    #[arg(
        long,
        env = "AETHER_TUNNEL_AETHER_CONNECT_TIMEOUT",
        default_value_t = 10
    )]
    pub aether_connect_timeout_secs: u64,

    /// Aether API max idle connections per host
    #[arg(
        long,
        env = "AETHER_TUNNEL_AETHER_POOL_MAX_IDLE_PER_HOST",
        default_value_t = 8
    )]
    pub aether_pool_max_idle_per_host: usize,

    /// Aether API idle timeout in seconds
    #[arg(
        long,
        env = "AETHER_TUNNEL_AETHER_POOL_IDLE_TIMEOUT",
        default_value_t = 90
    )]
    pub aether_pool_idle_timeout_secs: u64,

    /// Aether API TCP keepalive in seconds (0 disables)
    #[arg(long, env = "AETHER_TUNNEL_AETHER_TCP_KEEPALIVE", default_value_t = 60)]
    pub aether_tcp_keepalive_secs: u64,

    /// Aether API TCP_NODELAY
    #[arg(long, env = "AETHER_TUNNEL_AETHER_TCP_NODELAY", default_value_t = true)]
    pub aether_tcp_nodelay: bool,

    /// Enable HTTP/2 when talking to Aether API
    #[arg(long, env = "AETHER_TUNNEL_AETHER_HTTP2", default_value_t = true)]
    pub aether_http2: bool,

    /// Optional egress proxy used for Aether API registration and WebSocket tunnel reconnects.
    /// Supported schemes: http, socks5, socks5h.
    #[arg(long, env = "AETHER_TUNNEL_AETHER_OUTBOUND_PROXY_URL")]
    pub aether_outbound_proxy_url: Option<String>,

    /// Aether API retry attempts (including initial)
    #[arg(
        long,
        env = "AETHER_TUNNEL_AETHER_RETRY_MAX_ATTEMPTS",
        default_value_t = 3
    )]
    pub aether_retry_max_attempts: u32,

    /// Aether API retry base delay in milliseconds
    #[arg(
        long,
        env = "AETHER_TUNNEL_AETHER_RETRY_BASE_DELAY_MS",
        default_value_t = 200
    )]
    pub aether_retry_base_delay_ms: u64,

    /// Aether API retry max delay in milliseconds
    #[arg(
        long,
        env = "AETHER_TUNNEL_AETHER_RETRY_MAX_DELAY_MS",
        default_value_t = 2000
    )]
    pub aether_retry_max_delay_ms: u64,

    /// Optional local diagnostics listener for /health, /metrics, and /stats.
    /// Bind only to loopback addresses, for example 127.0.0.1:9311.
    #[arg(long, env = "AETHER_TUNNEL_DIAGNOSTICS_BIND")]
    pub diagnostics_bind: Option<SocketAddr>,

    /// Maximum concurrent TCP connections (defaults to hardware estimate)
    #[arg(long, env = "AETHER_TUNNEL_MAX_CONCURRENT_CONNECTIONS")]
    pub max_concurrent_connections: Option<u64>,

    /// Maximum in-flight tunneled streams accepted by this tunnel instance.
    #[arg(long, env = "AETHER_TUNNEL_MAX_IN_FLIGHT_STREAMS")]
    pub max_in_flight_streams: Option<usize>,

    /// Maximum in-flight tunneled streams admitted across all tunnel instances.
    #[arg(long, env = "AETHER_TUNNEL_DISTRIBUTED_STREAM_LIMIT")]
    pub distributed_stream_limit: Option<usize>,

    /// Redis URL used for cross-instance stream admission.
    #[arg(long, env = "AETHER_TUNNEL_DISTRIBUTED_STREAM_REDIS_URL")]
    pub distributed_stream_redis_url: Option<String>,

    /// Optional key prefix for cross-instance stream admission state.
    #[arg(long, env = "AETHER_TUNNEL_DISTRIBUTED_STREAM_REDIS_KEY_PREFIX")]
    pub distributed_stream_redis_key_prefix: Option<String>,

    /// Lease TTL in milliseconds for distributed stream admission permits.
    #[arg(
        long,
        env = "AETHER_TUNNEL_DISTRIBUTED_STREAM_LEASE_TTL_MS",
        default_value_t = 30_000
    )]
    pub distributed_stream_lease_ttl_ms: u64,

    /// Renew interval in milliseconds for distributed stream admission permits.
    #[arg(
        long,
        env = "AETHER_TUNNEL_DISTRIBUTED_STREAM_RENEW_INTERVAL_MS",
        default_value_t = 10_000
    )]
    pub distributed_stream_renew_interval_ms: u64,

    /// Command timeout in milliseconds for distributed stream admission Redis calls.
    #[arg(
        long,
        env = "AETHER_TUNNEL_DISTRIBUTED_STREAM_COMMAND_TIMEOUT_MS",
        default_value_t = 1_000
    )]
    pub distributed_stream_command_timeout_ms: u64,

    /// DNS cache TTL in seconds
    #[arg(long, env = "AETHER_TUNNEL_DNS_CACHE_TTL", default_value_t = 60)]
    pub dns_cache_ttl_secs: u64,

    /// DNS cache capacity (entries)
    #[arg(long, env = "AETHER_TUNNEL_DNS_CACHE_CAPACITY", default_value_t = 1024)]
    pub dns_cache_capacity: usize,

    /// Upstream HTTP client connect timeout in seconds
    #[arg(
        long,
        env = "AETHER_TUNNEL_UPSTREAM_CONNECT_TIMEOUT",
        default_value_t = 30
    )]
    pub upstream_connect_timeout_secs: u64,

    /// Upstream HTTP client max idle connections per host
    #[arg(
        long,
        env = "AETHER_TUNNEL_UPSTREAM_POOL_MAX_IDLE_PER_HOST",
        default_value_t = 64
    )]
    pub upstream_pool_max_idle_per_host: usize,

    /// Upstream HTTP client idle timeout in seconds
    #[arg(
        long,
        env = "AETHER_TUNNEL_UPSTREAM_POOL_IDLE_TIMEOUT",
        default_value_t = 300
    )]
    pub upstream_pool_idle_timeout_secs: u64,

    /// Maximum number of keyed upstream HTTP clients retained by the tunnel.
    #[arg(
        long,
        env = "AETHER_TUNNEL_UPSTREAM_CLIENT_POOL_CAPACITY",
        default_value_t = DEFAULT_UPSTREAM_CLIENT_POOL_CAPACITY
    )]
    pub upstream_client_pool_capacity: usize,

    /// Upstream TCP keepalive in seconds (0 disables)
    #[arg(
        long,
        env = "AETHER_TUNNEL_UPSTREAM_TCP_KEEPALIVE",
        default_value_t = 60
    )]
    pub upstream_tcp_keepalive_secs: u64,

    /// Upstream TCP_NODELAY
    #[arg(
        long,
        env = "AETHER_TUNNEL_UPSTREAM_TCP_NODELAY",
        default_value_t = true
    )]
    pub upstream_tcp_nodelay: bool,

    /// Optional egress proxy used only for provider upstream requests.
    /// Supported schemes: http, socks5, socks5h.
    #[arg(long, env = "AETHER_TUNNEL_UPSTREAM_PROXY_URL")]
    pub upstream_proxy_url: Option<String>,

    #[arg(
        long,
        env = "AETHER_TUNNEL_UPSTREAM_PROXY_REMOTE_DNS",
        default_value_t = false,
        help = "Trust an HTTP or SOCKS5h upstream proxy to resolve hostnames and enforce destination IP access controls"
    )]
    pub upstream_proxy_remote_dns: bool,

    /// Accepted only so older launch commands and environments keep working.
    /// Redirect request bodies are always replayed without a cumulative size limit.
    #[arg(
        long = "redirect-replay-budget-bytes",
        env = "AETHER_TUNNEL_REDIRECT_REPLAY_BUDGET_BYTES",
        hide = true
    )]
    pub legacy_redirect_replay_budget_bytes_ignored: Option<String>,

    /// Emit detailed x-proxy-timing headers on tunneled upstream responses.
    #[arg(
        long,
        env = "AETHER_TUNNEL_EMIT_PROXY_TIMING_HEADER",
        default_value_t = true
    )]
    pub emit_proxy_timing_header: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, env = "AETHER_TUNNEL_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// Log destination (stdout, file, both)
    #[arg(
        long,
        env = "AETHER_TUNNEL_LOG_DESTINATION",
        value_enum,
        default_value = "both"
    )]
    pub log_destination: TunnelLogDestinationArg,

    /// Log directory when file logging is enabled
    #[arg(long, env = "AETHER_TUNNEL_LOG_DIR", default_value = DEFAULT_LOG_DIR)]
    pub log_dir: Option<String>,

    /// Log rotation schedule for file logging
    #[arg(
        long,
        env = "AETHER_TUNNEL_LOG_ROTATION",
        value_enum,
        default_value = "daily"
    )]
    pub log_rotation: TunnelLogRotationArg,

    /// Log file retention days for file logging
    #[arg(
        long,
        env = "AETHER_TUNNEL_LOG_RETENTION_DAYS",
        default_value_t = DEFAULT_LOG_RETENTION_DAYS
    )]
    pub log_retention_days: u64,

    /// Maximum number of retained rolled log files
    #[arg(
        long,
        env = "AETHER_TUNNEL_LOG_MAX_FILES",
        default_value_t = DEFAULT_LOG_MAX_FILES
    )]
    pub log_max_files: usize,

    /// Tunnel reconnect base delay in milliseconds (used by exponential backoff)
    #[arg(
        long,
        env = "AETHER_TUNNEL_RECONNECT_BASE_MS",
        default_value_t = DEFAULT_TUNNEL_RECONNECT_BASE_MS
    )]
    pub tunnel_reconnect_base_ms: u64,

    /// Tunnel reconnect max delay in milliseconds (cap for exponential backoff)
    #[arg(
        long,
        env = "AETHER_TUNNEL_RECONNECT_MAX_MS",
        default_value_t = DEFAULT_TUNNEL_RECONNECT_MAX_MS
    )]
    pub tunnel_reconnect_max_ms: u64,

    /// WebSocket tunnel ping interval in milliseconds
    #[arg(
        long,
        env = TUNNEL_PING_INTERVAL_MS_ENV,
        default_value_t = DEFAULT_TUNNEL_PING_INTERVAL_MS
    )]
    pub tunnel_ping_interval_ms: u64,

    /// Maximum concurrent streams over tunnel (auto-detected from hardware if omitted)
    #[arg(long, env = "AETHER_TUNNEL_MAX_STREAMS")]
    pub tunnel_max_streams: Option<u32>,

    /// Tunnel connection pool profile used when connection counts are not explicit.
    #[arg(
        long,
        env = TUNNEL_PROFILE_ENV,
        value_enum,
        default_value_t = TunnelProfileArg::Standard
    )]
    pub tunnel_profile: TunnelProfileArg,

    /// Initial per-stream flow-control window advertised by this tunnel.
    #[arg(
        long,
        env = TUNNEL_STREAM_INITIAL_WINDOW_BYTES_ENV,
        default_value_t = DEFAULT_TUNNEL_STREAM_INITIAL_WINDOW_BYTES
    )]
    pub tunnel_stream_initial_window_bytes: u32,

    /// Deadline for graceful tunnel drain after GOAWAY.
    #[arg(
        long,
        env = TUNNEL_DRAIN_DEADLINE_MS_ENV,
        default_value_t = DEFAULT_TUNNEL_DRAIN_DEADLINE_MS
    )]
    pub tunnel_drain_deadline_ms: u64,

    /// WebSocket tunnel TCP connect timeout in milliseconds
    #[arg(
        long,
        env = TUNNEL_CONNECT_TIMEOUT_MS_ENV,
        default_value_t = DEFAULT_TUNNEL_CONNECT_TIMEOUT_MS
    )]
    pub tunnel_connect_timeout_ms: u64,

    /// Force direct WebSocket tunnel TCP connects, or Aether outbound proxy endpoint connects, to IPv4 addresses only.
    #[arg(
        long,
        env = "AETHER_TUNNEL_IPV4_ONLY",
        default_value_t = false,
        action = clap::ArgAction::Set,
        default_missing_value = "true",
        num_args = 0..=1,
        require_equals = true
    )]
    pub tunnel_ipv4_only: bool,

    /// Force direct WebSocket tunnel TCP connects, or Aether outbound proxy endpoint connects, to IPv6 addresses only.
    #[arg(
        long,
        env = "AETHER_TUNNEL_IPV6_ONLY",
        default_value_t = false,
        action = clap::ArgAction::Set,
        default_missing_value = "true",
        num_args = 0..=1,
        require_equals = true
    )]
    pub tunnel_ipv6_only: bool,

    /// WebSocket tunnel TCP keepalive in seconds (0 disables)
    #[arg(long, env = "AETHER_TUNNEL_TCP_KEEPALIVE", default_value_t = 30)]
    pub tunnel_tcp_keepalive_secs: u64,

    /// WebSocket tunnel TCP_NODELAY
    #[arg(long, env = "AETHER_TUNNEL_TCP_NODELAY", default_value_t = true)]
    pub tunnel_tcp_nodelay: bool,

    /// Tunnel connection staleness timeout in milliseconds
    #[arg(
        long,
        env = TUNNEL_STALE_TIMEOUT_MS_ENV,
        default_value_t = DEFAULT_TUNNEL_STALE_TIMEOUT_MS
    )]
    pub tunnel_stale_timeout_ms: u64,

    /// Minimum number of parallel WebSocket tunnel connections per server.
    /// If omitted, a device-aware redundant value is auto-detected at startup.
    #[arg(long, env = "AETHER_TUNNEL_CONNECTIONS")]
    pub tunnel_connections: Option<u32>,

    /// Maximum number of WebSocket tunnel connections per server.
    /// When larger than `tunnel_connections`, the tunnel may autoscale up to this limit.
    #[arg(long, env = "AETHER_TUNNEL_CONNECTIONS_MAX")]
    pub tunnel_connections_max: Option<u32>,

    /// Autoscale evaluation interval for the tunnel pool.
    #[arg(
        long,
        env = "AETHER_TUNNEL_SCALE_CHECK_INTERVAL_MS",
        default_value_t = DEFAULT_TUNNEL_SCALE_CHECK_INTERVAL_MS
    )]
    pub tunnel_scale_check_interval_ms: u64,

    /// Per-tunnel occupancy percentage that triggers scale-up.
    #[arg(
        long,
        env = "AETHER_TUNNEL_SCALE_UP_THRESHOLD_PERCENT",
        default_value_t = DEFAULT_TUNNEL_SCALE_UP_THRESHOLD_PERCENT
    )]
    pub tunnel_scale_up_threshold_percent: u32,

    /// Per-tunnel occupancy percentage that allows scale-down after the grace window.
    #[arg(
        long,
        env = "AETHER_TUNNEL_SCALE_DOWN_THRESHOLD_PERCENT",
        default_value_t = DEFAULT_TUNNEL_SCALE_DOWN_THRESHOLD_PERCENT
    )]
    pub tunnel_scale_down_threshold_percent: u32,

    /// Low-load grace window before a secondary tunnel is drained.
    #[arg(
        long,
        env = "AETHER_TUNNEL_SCALE_DOWN_GRACE_SECS",
        default_value_t = DEFAULT_TUNNEL_SCALE_DOWN_GRACE_SECS
    )]
    pub tunnel_scale_down_grace_secs: u64,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Config")
            .field("aether_url", &aether_url_for_log(&self.aether_url))
            .field("management_token", &"<redacted>")
            .field("node_name", &self.node_name)
            .field("node_region", &self.node_region)
            .field("tunnel_security", &self.tunnel_security)
            .field(
                "tunnel_encryption_key",
                &self.tunnel_encryption_key.as_ref().map(|_| "<redacted>"),
            )
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Validate configuration values are within sane ranges.
    /// Called after parsing to catch misconfigurations early.
    pub fn validate(&self) -> anyhow::Result<()> {
        validate_aether_url(&self.aether_url)?;
        if self.management_token.trim().is_empty() {
            anyhow::bail!("management_token must not be empty");
        }
        if self.heartbeat_interval == 0 {
            anyhow::bail!("heartbeat_interval must be > 0");
        }
        if self.heartbeat_interval > 3600 {
            anyhow::bail!("heartbeat_interval must be <= 3600");
        }
        if self.allowed_ports.is_empty() {
            anyhow::bail!("allowed_ports must not be empty");
        }
        if self.node_name.trim().is_empty() {
            anyhow::bail!("node_name must not be empty");
        }
        if self.tunnel_security == TunnelSecurity::NonTlsRequired {
            let Some(key) = normalized_proxy_url(&self.tunnel_encryption_key) else {
                anyhow::bail!(
                    "tunnel_encryption_key must be set when tunnel_security=non_tls_required"
                );
            };
            validate_tunnel_encryption_key(key)?;
        }
        for &port in &self.allowed_ports {
            if port == 0 {
                anyhow::bail!("allowed_ports: port 0 is not valid");
            }
        }
        let tunnel_connect_timeout = self.tunnel_connect_timeout()?;
        if tunnel_connect_timeout.is_zero() {
            anyhow::bail!("effective tunnel connect timeout must be > 0");
        }
        if self.tunnel_ipv4_only && self.tunnel_ipv6_only {
            anyhow::bail!("tunnel_ipv4_only and tunnel_ipv6_only cannot both be enabled");
        }
        let tunnel_ping_interval = self.tunnel_ping_interval()?;
        if tunnel_ping_interval.is_zero() {
            anyhow::bail!("effective tunnel ping interval must be > 0");
        }
        let tunnel_stale_timeout = self.tunnel_stale_timeout()?;
        if tunnel_stale_timeout <= tunnel_ping_interval {
            anyhow::bail!(
                "effective tunnel stale timeout ({:?}) must be > effective tunnel ping interval ({:?})",
                tunnel_stale_timeout,
                tunnel_ping_interval
            );
        }
        if matches!(self.tunnel_connections, Some(0)) {
            anyhow::bail!("tunnel_connections must be > 0");
        }
        if matches!(self.tunnel_connections_max, Some(0)) {
            anyhow::bail!("tunnel_connections_max must be > 0");
        }
        if matches!(self.tunnel_max_streams, Some(0)) {
            anyhow::bail!("tunnel_max_streams must be > 0");
        }
        if self.tunnel_stream_initial_window_bytes == 0 {
            anyhow::bail!("tunnel_stream_initial_window_bytes must be > 0");
        }
        if u64::from(self.tunnel_stream_initial_window_bytes)
            > aether_contracts::tunnel::MAX_TUNNEL_DECOMPRESSED_PAYLOAD_BYTES as u64
        {
            anyhow::bail!(
                "tunnel_stream_initial_window_bytes exceeds the maximum tunnel payload size"
            );
        }
        if self.tunnel_drain_deadline_ms == 0 {
            anyhow::bail!("tunnel_drain_deadline_ms must be > 0");
        }
        if let (Some(min_connections), Some(max_connections)) =
            (self.tunnel_connections, self.tunnel_connections_max)
        {
            if max_connections < min_connections {
                anyhow::bail!("tunnel_connections_max must be >= tunnel_connections");
            }
        }
        if self.tunnel_scale_check_interval_ms == 0 {
            anyhow::bail!("tunnel_scale_check_interval_ms must be > 0");
        }
        if self.tunnel_scale_down_grace_secs == 0 {
            anyhow::bail!("tunnel_scale_down_grace_secs must be > 0");
        }
        if !(1..=100).contains(&self.tunnel_scale_up_threshold_percent) {
            anyhow::bail!("tunnel_scale_up_threshold_percent must be within 1..=100");
        }
        if !(1..100).contains(&self.tunnel_scale_down_threshold_percent) {
            anyhow::bail!("tunnel_scale_down_threshold_percent must be within 1..100");
        }
        if self.tunnel_scale_down_threshold_percent >= self.tunnel_scale_up_threshold_percent {
            anyhow::bail!(
                "tunnel_scale_down_threshold_percent must be < tunnel_scale_up_threshold_percent"
            );
        }
        if self.aether_retry_max_attempts == 0 {
            anyhow::bail!("aether_retry_max_attempts must be >= 1");
        }
        if let Some(addr) = self.diagnostics_bind {
            if !addr.ip().is_loopback() {
                anyhow::bail!("diagnostics_bind must use a loopback address");
            }
        }
        if self.upstream_connect_timeout_secs == 0 {
            anyhow::bail!("upstream_connect_timeout_secs must be > 0");
        }
        if self.upstream_client_pool_capacity == 0 {
            anyhow::bail!("upstream_client_pool_capacity must be > 0");
        }
        if let Some(proxy_url) = normalized_proxy_url(&self.aether_outbound_proxy_url) {
            crate::egress_proxy::UpstreamProxyConfig::parse(proxy_url)
                .map_err(|err| anyhow::anyhow!("aether_outbound_proxy_url invalid: {err}"))?;
        }
        if let Some(proxy_url) = normalized_proxy_url(&self.upstream_proxy_url) {
            crate::egress_proxy::UpstreamProxyConfig::parse(proxy_url)
                .map_err(|err| anyhow::anyhow!("upstream_proxy_url invalid: {err}"))?;
        }
        if self.upstream_proxy_remote_dns {
            let proxy_url = normalized_proxy_url(&self.upstream_proxy_url).ok_or_else(|| {
                anyhow::anyhow!("upstream_proxy_remote_dns requires upstream_proxy_url")
            })?;
            let proxy = crate::egress_proxy::UpstreamProxyConfig::parse(proxy_url)
                .map_err(anyhow::Error::msg)?;
            if !proxy.supports_remote_target_dns() {
                anyhow::bail!("upstream_proxy_remote_dns requires an http:// or socks5h:// proxy");
            }
        }
        if matches!(self.max_in_flight_streams, Some(0)) {
            anyhow::bail!("max_in_flight_streams must be > 0");
        }
        if matches!(self.distributed_stream_limit, Some(0)) {
            anyhow::bail!("distributed_stream_limit must be > 0");
        }
        if self.distributed_stream_limit.is_some() && self.distributed_stream_redis_url.is_none() {
            anyhow::bail!(
                "distributed_stream_redis_url must be set when distributed_stream_limit is enabled"
            );
        }
        if self.distributed_stream_lease_ttl_ms == 0 {
            anyhow::bail!("distributed_stream_lease_ttl_ms must be > 0");
        }
        if self.distributed_stream_renew_interval_ms == 0 {
            anyhow::bail!("distributed_stream_renew_interval_ms must be > 0");
        }
        if self.distributed_stream_renew_interval_ms >= self.distributed_stream_lease_ttl_ms {
            anyhow::bail!(
                "distributed_stream_renew_interval_ms must be < distributed_stream_lease_ttl_ms"
            );
        }
        if self.distributed_stream_command_timeout_ms == 0 {
            anyhow::bail!("distributed_stream_command_timeout_ms must be > 0");
        }
        if matches!(
            self.log_destination,
            TunnelLogDestinationArg::File | TunnelLogDestinationArg::Both
        ) && self
            .log_dir
            .as_deref()
            .map(str::trim)
            .is_none_or(|value| value.is_empty())
        {
            anyhow::bail!("log_dir must be set when AETHER_TUNNEL_LOG_DESTINATION is file or both");
        }
        Ok(())
    }

    pub fn tunnel_ping_interval(&self) -> anyhow::Result<Duration> {
        Ok(Duration::from_millis(self.tunnel_ping_interval_ms))
    }

    pub fn tunnel_connect_timeout(&self) -> anyhow::Result<Duration> {
        Ok(Duration::from_millis(self.tunnel_connect_timeout_ms))
    }

    pub fn tunnel_ip_family(&self) -> crate::egress_proxy::IpFamily {
        if self.tunnel_ipv4_only {
            crate::egress_proxy::IpFamily::Ipv4Only
        } else if self.tunnel_ipv6_only {
            crate::egress_proxy::IpFamily::Ipv6Only
        } else {
            crate::egress_proxy::IpFamily::Any
        }
    }

    pub fn tunnel_stale_timeout(&self) -> anyhow::Result<Duration> {
        Ok(Duration::from_millis(self.tunnel_stale_timeout_ms))
    }

    pub fn effective_aether_outbound_proxy_url(&self) -> Option<&str> {
        normalized_proxy_url(&self.aether_outbound_proxy_url)
    }

    pub fn resolve_tunnel_pool_sizing(
        &self,
        hw_info: &HardwareInfo,
    ) -> anyhow::Result<TunnelPoolSizing> {
        let per_tunnel_capacity = u64::from(self.tunnel_max_streams.unwrap_or(128).max(1));
        let estimated = self
            .max_in_flight_streams
            .and_then(|limit| u64::try_from(limit).ok())
            .unwrap_or(hw_info.estimated_max_concurrency)
            .max(per_tunnel_capacity);
        let (profile_initial_floor, profile_initial_cap, profile_max_floor) =
            match self.tunnel_profile {
                TunnelProfileArg::Lite => (2, 2, 2),
                TunnelProfileArg::Standard => (4, 4, 4),
                TunnelProfileArg::Throughput => (8, 8, 8),
            };
        let cpu_soft_cap = u64::from(hw_info.cpu_cores.max(1))
            .saturating_mul(AUTO_TUNNEL_CONNECTIONS_PER_CPU_CAP)
            .clamp(profile_max_floor, AUTO_TUNNEL_CONNECTIONS_MAX_CAP);
        let auto_initial_floor = AUTO_TUNNEL_CONNECTIONS_REDUNDANT_FLOOR
            .max(profile_initial_floor)
            .min(cpu_soft_cap);
        let auto_initial_cap = AUTO_TUNNEL_CONNECTIONS_BASE_CAP
            .max(profile_initial_cap)
            .min(cpu_soft_cap)
            .max(auto_initial_floor);

        let auto_initial = div_ceil_u64(estimated, per_tunnel_capacity.saturating_mul(8))
            .clamp(auto_initial_floor, auto_initial_cap);
        let high_water_per_tunnel = div_ceil_u64(
            per_tunnel_capacity.saturating_mul(u64::from(self.tunnel_scale_up_threshold_percent)),
            100,
        )
        .max(1);
        let auto_max_floor = auto_initial.max(
            AUTO_TUNNEL_CONNECTIONS_BASE_CAP
                .max(profile_max_floor)
                .min(cpu_soft_cap),
        );
        let auto_max =
            div_ceil_u64(estimated, high_water_per_tunnel).clamp(auto_max_floor, cpu_soft_cap);

        let initial_connections = u64::from(self.tunnel_connections.unwrap_or(auto_initial as u32));
        let max_connections = match self.tunnel_connections_max {
            Some(explicit) => u64::from(explicit),
            None if self.tunnel_connections.is_some() => initial_connections,
            None => auto_max,
        };

        if max_connections < initial_connections {
            anyhow::bail!(
                "effective tunnel_connections_max ({max_connections}) must be >= tunnel_connections ({initial_connections})"
            );
        }

        Ok(TunnelPoolSizing {
            initial_connections: u32::try_from(initial_connections)
                .expect("effective tunnel initial connections should fit in u32"),
            max_connections: u32::try_from(max_connections)
                .expect("effective tunnel max connections should fit in u32"),
        })
    }

    pub fn service_runtime_config(&self) -> anyhow::Result<ServiceRuntimeConfig> {
        let mut config = ServiceRuntimeConfig::new("aether-tunnel", "aether_tunnel=info")
            .with_log_format(aether_runtime::LogFormat::Pretty)
            .with_log_destination(self.log_destination.into())
            .with_node_role("proxy")
            .with_instance_id(self.node_name.trim().to_string());
        if matches!(
            self.log_destination,
            TunnelLogDestinationArg::File | TunnelLogDestinationArg::Both
        ) {
            let log_dir = self
                .log_dir
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow::anyhow!("log_dir must be configured for file logging"))?;
            config = config.with_file_logging(FileLoggingConfig::new(
                log_dir,
                self.log_rotation.into(),
                self.log_retention_days,
                self.log_max_files,
            ));
        }
        Ok(config)
    }
}

/// Per-server connection config (used in multi-server TOML `[[servers]]`).
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerEntry {
    pub aether_url: String,
    pub management_token: String,
    /// Per-server node name override. Falls back to the global `node_name`.
    pub node_name: Option<String>,
    /// Per-server tunnel security mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_security: Option<TunnelSecurity>,
    /// Per-server PSK for secure non-TLS tunnel handshakes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_encryption_key: Option<String>,
}

impl std::fmt::Debug for ServerEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerEntry")
            .field("aether_url", &aether_url_for_log(&self.aether_url))
            .field("management_token", &"<redacted>")
            .field("node_name", &self.node_name)
            .field("tunnel_security", &self.tunnel_security)
            .field(
                "tunnel_encryption_key",
                &self.tunnel_encryption_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl ServerEntry {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        validate_aether_url(&self.aether_url)?;
        if self.management_token.trim().is_empty() {
            anyhow::bail!("management_token must not be empty");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TOML config file support
// ---------------------------------------------------------------------------

/// Serializable config for TOML file persistence.
/// All fields are optional -- only populated values are written.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_interval: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_ports: Option<Vec<u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_private_targets: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_request_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_connect_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_pool_max_idle_per_host: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_pool_idle_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_tcp_keepalive_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_tcp_nodelay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_http2: Option<bool>,
    #[serde(
        alias = "aether_proxy_url",
        alias = "aether_tunnel_url",
        skip_serializing_if = "Option::is_none"
    )]
    pub aether_outbound_proxy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_retry_max_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_retry_base_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_retry_max_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics_bind: Option<SocketAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_connections: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_cache_ttl_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_cache_capacity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_connect_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_pool_max_idle_per_host: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_pool_idle_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_client_pool_capacity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_tcp_keepalive_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_tcp_nodelay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_proxy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_proxy_remote_dns: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emit_proxy_timing_header: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_destination: Option<TunnelLogDestinationArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_rotation: Option<TunnelLogRotationArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_retention_days: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_max_files: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_reconnect_base_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_reconnect_max_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_ping_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_max_streams: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_profile: Option<TunnelProfileArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_stream_initial_window_bytes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_drain_deadline_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_connect_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_ipv4_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_ipv6_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_tcp_keepalive_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_tcp_nodelay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_stale_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_connections: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_connections_max: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_scale_check_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_scale_up_threshold_percent: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_scale_down_threshold_percent: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_scale_down_grace_secs: Option<u64>,

    /// Multi-server config: each entry connects to a separate Aether instance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerEntry>,
}

impl std::fmt::Debug for ConfigFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigFile")
            .field("node_name", &self.node_name)
            .field("node_region", &self.node_region)
            .field("server_count", &self.servers.len())
            .finish_non_exhaustive()
    }
}

impl ConfigFile {
    /// Load from a TOML file.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let advertised_len = file.metadata()?.len();
        if advertised_len > MAX_CONFIG_FILE_BYTES {
            anyhow::bail!(
                "tunnel config exceeds the {} byte limit",
                MAX_CONFIG_FILE_BYTES
            );
        }

        let mut content = String::with_capacity(advertised_len as usize);
        Read::by_ref(&mut file)
            .take(MAX_CONFIG_FILE_BYTES.saturating_add(1))
            .read_to_string(&mut content)?;
        if content.len() as u64 > MAX_CONFIG_FILE_BYTES {
            anyhow::bail!(
                "tunnel config exceeds the {} byte limit",
                MAX_CONFIG_FILE_BYTES
            );
        }
        parse_config_file_content(&content)
    }

    /// Save to a TOML file.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(self)?;
        write_private_config_atomically(path, content.as_bytes())?;
        Ok(())
    }

    /// Inject values as environment variables so clap picks them up.
    ///
    /// Only sets variables that are **not** already present in the
    /// environment, preserving the precedence: CLI > env > config file.
    pub fn inject_env(&self) {
        self.inject_env_inner(false);
    }

    /// Inject values as environment variables, **overriding** any existing
    /// values.  Used after setup to ensure the freshly-saved config takes
    /// effect before re-parsing.
    pub fn inject_env_override(&self) {
        self.inject_env_inner(true);
    }

    fn inject_env_inner(&self, force: bool) {
        macro_rules! set {
            ($env:expr, $val:expr) => {
                if let Some(ref v) = $val {
                    if force || std::env::var($env).is_err() {
                        std::env::set_var($env, v.to_string());
                    }
                }
            };
        }

        let first_server = self.servers.first();
        let aether_url = first_server.map(|s| s.aether_url.as_str());
        let management_token = first_server.map(|s| s.management_token.as_str());
        let tunnel_security =
            first_server.map(|s| s.tunnel_security.unwrap_or(TunnelSecurity::Off));
        let tunnel_encryption_key = first_server.and_then(|s| s.tunnel_encryption_key.as_deref());
        let node_name = self
            .node_name
            .as_deref()
            .or(first_server.and_then(|s| s.node_name.as_deref()));

        set!("AETHER_TUNNEL_AETHER_URL", aether_url);
        set!("AETHER_TUNNEL_MANAGEMENT_TOKEN", management_token);
        set!("AETHER_TUNNEL_SECURITY", tunnel_security);
        set!("AETHER_TUNNEL_ENCRYPTION_KEY", tunnel_encryption_key);
        set!("AETHER_TUNNEL_PUBLIC_IP", self.public_ip);
        set!("AETHER_TUNNEL_NODE_NAME", node_name);
        set!("AETHER_TUNNEL_NODE_REGION", self.node_region);
        set!("AETHER_TUNNEL_HEARTBEAT_INTERVAL", self.heartbeat_interval);
        set!(
            "AETHER_TUNNEL_ALLOW_PRIVATE_TARGETS",
            self.allow_private_targets
        );
        set!(
            "AETHER_TUNNEL_AETHER_REQUEST_TIMEOUT",
            self.aether_request_timeout_secs
        );
        set!(
            "AETHER_TUNNEL_AETHER_CONNECT_TIMEOUT",
            self.aether_connect_timeout_secs
        );
        set!(
            "AETHER_TUNNEL_AETHER_POOL_MAX_IDLE_PER_HOST",
            self.aether_pool_max_idle_per_host
        );
        set!(
            "AETHER_TUNNEL_AETHER_POOL_IDLE_TIMEOUT",
            self.aether_pool_idle_timeout_secs
        );
        set!(
            "AETHER_TUNNEL_AETHER_TCP_KEEPALIVE",
            self.aether_tcp_keepalive_secs
        );
        set!("AETHER_TUNNEL_AETHER_TCP_NODELAY", self.aether_tcp_nodelay);
        set!("AETHER_TUNNEL_AETHER_HTTP2", self.aether_http2);
        set!(
            "AETHER_TUNNEL_AETHER_OUTBOUND_PROXY_URL",
            self.aether_outbound_proxy_url
        );
        set!(
            "AETHER_TUNNEL_AETHER_RETRY_MAX_ATTEMPTS",
            self.aether_retry_max_attempts
        );
        set!(
            "AETHER_TUNNEL_AETHER_RETRY_BASE_DELAY_MS",
            self.aether_retry_base_delay_ms
        );
        set!(
            "AETHER_TUNNEL_AETHER_RETRY_MAX_DELAY_MS",
            self.aether_retry_max_delay_ms
        );
        set!("AETHER_TUNNEL_DIAGNOSTICS_BIND", self.diagnostics_bind);
        set!(
            "AETHER_TUNNEL_MAX_CONCURRENT_CONNECTIONS",
            self.max_concurrent_connections
        );
        set!("AETHER_TUNNEL_DNS_CACHE_TTL", self.dns_cache_ttl_secs);
        set!("AETHER_TUNNEL_DNS_CACHE_CAPACITY", self.dns_cache_capacity);
        set!(
            "AETHER_TUNNEL_UPSTREAM_CONNECT_TIMEOUT",
            self.upstream_connect_timeout_secs
        );
        set!(
            "AETHER_TUNNEL_UPSTREAM_POOL_MAX_IDLE_PER_HOST",
            self.upstream_pool_max_idle_per_host
        );
        set!(
            "AETHER_TUNNEL_UPSTREAM_POOL_IDLE_TIMEOUT",
            self.upstream_pool_idle_timeout_secs
        );
        set!(
            "AETHER_TUNNEL_UPSTREAM_CLIENT_POOL_CAPACITY",
            self.upstream_client_pool_capacity
        );
        set!(
            "AETHER_TUNNEL_UPSTREAM_TCP_KEEPALIVE",
            self.upstream_tcp_keepalive_secs
        );
        set!(
            "AETHER_TUNNEL_UPSTREAM_TCP_NODELAY",
            self.upstream_tcp_nodelay
        );
        set!("AETHER_TUNNEL_UPSTREAM_PROXY_URL", self.upstream_proxy_url);
        set!(
            "AETHER_TUNNEL_UPSTREAM_PROXY_REMOTE_DNS",
            self.upstream_proxy_remote_dns
        );
        set!(
            "AETHER_TUNNEL_EMIT_PROXY_TIMING_HEADER",
            self.emit_proxy_timing_header
        );
        set!("AETHER_TUNNEL_LOG_LEVEL", self.log_level);
        set!(
            "AETHER_TUNNEL_LOG_DESTINATION",
            self.log_destination.map(|v| match v {
                TunnelLogDestinationArg::Stdout => "stdout",
                TunnelLogDestinationArg::File => "file",
                TunnelLogDestinationArg::Both => "both",
            })
        );
        set!("AETHER_TUNNEL_LOG_DIR", self.log_dir.as_deref());
        set!(
            "AETHER_TUNNEL_LOG_ROTATION",
            self.log_rotation.map(|v| match v {
                TunnelLogRotationArg::Hourly => "hourly",
                TunnelLogRotationArg::Daily => "daily",
            })
        );
        set!("AETHER_TUNNEL_LOG_RETENTION_DAYS", self.log_retention_days);
        set!("AETHER_TUNNEL_LOG_MAX_FILES", self.log_max_files);
        set!(
            "AETHER_TUNNEL_RECONNECT_BASE_MS",
            self.tunnel_reconnect_base_ms
        );
        set!(
            "AETHER_TUNNEL_RECONNECT_MAX_MS",
            self.tunnel_reconnect_max_ms
        );
        set!(TUNNEL_PING_INTERVAL_MS_ENV, self.tunnel_ping_interval_ms);
        set!("AETHER_TUNNEL_MAX_STREAMS", self.tunnel_max_streams);
        set!(
            TUNNEL_PROFILE_ENV,
            self.tunnel_profile.map(|value| value.to_string())
        );
        set!(
            TUNNEL_STREAM_INITIAL_WINDOW_BYTES_ENV,
            self.tunnel_stream_initial_window_bytes
        );
        set!(TUNNEL_DRAIN_DEADLINE_MS_ENV, self.tunnel_drain_deadline_ms);
        set!(
            TUNNEL_CONNECT_TIMEOUT_MS_ENV,
            self.tunnel_connect_timeout_ms
        );
        set!("AETHER_TUNNEL_IPV4_ONLY", self.tunnel_ipv4_only);
        set!("AETHER_TUNNEL_IPV6_ONLY", self.tunnel_ipv6_only);
        set!(
            "AETHER_TUNNEL_TCP_KEEPALIVE",
            self.tunnel_tcp_keepalive_secs
        );
        set!("AETHER_TUNNEL_TCP_NODELAY", self.tunnel_tcp_nodelay);
        set!(TUNNEL_STALE_TIMEOUT_MS_ENV, self.tunnel_stale_timeout_ms);
        set!("AETHER_TUNNEL_CONNECTIONS", self.tunnel_connections);
        set!("AETHER_TUNNEL_CONNECTIONS_MAX", self.tunnel_connections_max);
        set!(
            "AETHER_TUNNEL_SCALE_CHECK_INTERVAL_MS",
            self.tunnel_scale_check_interval_ms
        );
        set!(
            "AETHER_TUNNEL_SCALE_UP_THRESHOLD_PERCENT",
            self.tunnel_scale_up_threshold_percent
        );
        set!(
            "AETHER_TUNNEL_SCALE_DOWN_THRESHOLD_PERCENT",
            self.tunnel_scale_down_threshold_percent
        );
        set!(
            "AETHER_TUNNEL_SCALE_DOWN_GRACE_SECS",
            self.tunnel_scale_down_grace_secs
        );

        // allowed_ports needs special handling (comma-separated)
        if let Some(ref ports) = self.allowed_ports {
            if force || std::env::var("AETHER_TUNNEL_ALLOWED_PORTS").is_err() {
                let s: String = ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                std::env::set_var("AETHER_TUNNEL_ALLOWED_PORTS", s);
            }
        }
    }
}

fn write_private_config_atomically(path: &Path, content: &[u8]) -> io::Result<()> {
    reject_config_symlink(path)?;

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration path must name a file",
        )
    })?;
    if !parent.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "configuration parent directory does not exist",
        ));
    }

    let mut temp_path = None;
    let mut temp_file = None;
    for _ in 0..16 {
        let candidate = parent.join(format!(
            ".{}.tmp-{}",
            file_name.to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                temp_path = Some(candidate);
                temp_file = Some(file);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let temp_path = temp_path.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique configuration temporary file",
        )
    })?;
    let mut temp_file = temp_file.expect("temporary path and file are created together");

    let replace_result = (|| -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            temp_file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        temp_file.write_all(content)?;
        temp_file.sync_all()?;
        drop(temp_file);

        // Do not silently replace a credential-bearing symlink. The second
        // check also covers a target created while the temporary file was written.
        reject_config_symlink(path)?;
        replace_config_file(&temp_path, path)?;

        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();

    if replace_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    replace_result
}

fn reject_config_symlink(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if config_metadata_is_link_like(&metadata) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing to save configuration through a symbolic link or reparse point",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn config_metadata_is_link_like(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

#[cfg(not(windows))]
fn replace_config_file(temp_path: &Path, path: &Path) -> io::Result<()> {
    std::fs::rename(temp_path, path)
}

#[cfg(windows)]
fn replace_config_file(temp_path: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let existing_file_name = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let new_file_name = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            existing_file_name.as_ptr(),
            new_file_name.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn parse_config_file_content(content: &str) -> anyhow::Result<ConfigFile> {
    reject_removed_config_keys(content)?;
    let mut value: toml::Value = toml::from_str(content)?;
    discard_ignored_config_keys(&mut value);
    promote_server_scoped_upstream_proxy_url(&mut value)?;
    Ok(value.try_into()?)
}

fn discard_ignored_config_keys(value: &mut toml::Value) {
    let Some(root) = value.as_table_mut() else {
        return;
    };
    for key in IGNORED_CONFIG_KEYS {
        root.remove(*key);
    }
}

fn normalized_proxy_url(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn promote_server_scoped_upstream_proxy_url(value: &mut toml::Value) -> anyhow::Result<()> {
    const KEY: &str = "upstream_proxy_url";

    let Some(root) = value.as_table_mut() else {
        return Ok(());
    };

    let mut promoted = root.get(KEY).cloned();
    let Some(servers) = root.get_mut("servers").and_then(toml::Value::as_array_mut) else {
        return Ok(());
    };

    for (index, server) in servers.iter_mut().enumerate() {
        let Some(table) = server.as_table_mut() else {
            continue;
        };
        let Some(server_value) = table.remove(KEY) else {
            continue;
        };

        match promoted.as_ref() {
            Some(existing) if existing != &server_value => {
                anyhow::bail!(
                    "conflicting upstream_proxy_url values: top-level value and [[servers]] entry {} differ; configure it once at the top level",
                    index + 1
                );
            }
            Some(_) => {}
            None => promoted = Some(server_value),
        }
    }

    if let Some(promoted) = promoted {
        root.insert(KEY.to_string(), promoted);
    }

    Ok(())
}

fn reject_removed_config_keys(content: &str) -> anyhow::Result<()> {
    let value: toml::Value = toml::from_str(content)?;
    let Some(table) = value.as_table() else {
        return Ok(());
    };

    let removed_seconds = REMOVED_TUNNEL_SECONDS_KEYS
        .iter()
        .copied()
        .filter(|key| table.contains_key(*key))
        .collect::<Vec<_>>();
    if !removed_seconds.is_empty() {
        anyhow::bail!(
            "removed tunnel config keys detected: {}. Use *_ms variants instead",
            removed_seconds.join(", ")
        );
    }

    let removed_single_server = REMOVED_SINGLE_SERVER_KEYS
        .iter()
        .copied()
        .filter(|key| table.contains_key(*key))
        .collect::<Vec<_>>();
    if !removed_single_server.is_empty() {
        anyhow::bail!(
            "single-server top-level config keys are no longer supported: {}. Use [[servers]] entries instead",
            removed_single_server.join(", ")
        );
    }

    let removed_legacy = LEGACY_ONLY_KEYS
        .iter()
        .copied()
        .filter(|key| table.contains_key(*key))
        .chain(
            DELEGATE_TO_UPSTREAM
                .iter()
                .map(|(old, _)| *old)
                .filter(|key| table.contains_key(*key)),
        )
        .collect::<Vec<_>>();
    if !removed_legacy.is_empty() {
        anyhow::bail!(
            "legacy config keys are no longer supported: {}",
            removed_legacy.join(", ")
        );
    }

    Ok(())
}

fn div_ceil_u64(value: u64, divisor: u64) -> u64 {
    if divisor == 0 {
        return value;
    }
    value.saturating_add(divisor.saturating_sub(1)) / divisor
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;
    use crate::hardware::HardwareInfo;

    #[test]
    fn proxy_remote_dns_requires_explicit_trust_and_a_remote_dns_proxy() {
        let mut config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
        ]);
        assert!(!config.upstream_proxy_remote_dns);
        let argument = Config::command()
            .get_arguments()
            .find(|argument| argument.get_id() == "upstream_proxy_remote_dns")
            .unwrap()
            .clone();
        assert_eq!(
            argument.get_env().unwrap(),
            "AETHER_TUNNEL_UPSTREAM_PROXY_REMOTE_DNS"
        );

        config.upstream_proxy_remote_dns = true;
        for proxy in [None, Some(" "), Some("socks5://127.0.0.1:1080")] {
            config.upstream_proxy_url = proxy.map(str::to_string);
            assert!(config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("upstream_proxy_remote_dns"));
        }
        for proxy in ["http://127.0.0.1:8080", "socks5h://127.0.0.1:1080"] {
            config.upstream_proxy_url = Some(proxy.to_string());
            config
                .validate()
                .expect("explicit remote DNS configuration should validate");
        }
    }

    #[test]
    fn config_file_round_trips_proxy_remote_dns() {
        let config = parse_config_file_content(
            "upstream_proxy_url = \"socks5h://127.0.0.1:1080\"\nupstream_proxy_remote_dns = true",
        )
        .unwrap();
        assert_eq!(config.upstream_proxy_remote_dns, Some(true));
        let round_trip: ConfigFile = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(round_trip.upstream_proxy_remote_dns, Some(true));
        assert_eq!(ConfigFile::default().upstream_proxy_remote_dns, None);
    }

    fn config_save_test_dir(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aether-tunnel-config-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).expect("config save test directory should be created");
        path
    }

    fn secret_bearing_config_file(secret: &str) -> ConfigFile {
        ConfigFile {
            node_name: Some("secure-save-test".to_string()),
            servers: vec![ServerEntry {
                aether_url: "https://example.com".to_string(),
                management_token: secret.to_string(),
                node_name: None,
                tunnel_security: None,
                tunnel_encryption_key: None,
            }],
            ..ConfigFile::default()
        }
    }

    #[test]
    fn config_file_save_replaces_an_existing_config() {
        let directory = config_save_test_dir("replace-existing");
        let path = directory.join("tunnel.toml");

        secret_bearing_config_file("first-management-secret")
            .save(&path)
            .expect("initial config save should succeed");
        secret_bearing_config_file("second-management-secret")
            .save(&path)
            .expect("replacement config save should succeed");

        let saved = std::fs::read_to_string(&path).expect("replacement config should be readable");
        assert!(saved.contains("second-management-secret"));
        assert!(!saved.contains("first-management-secret"));
        assert_eq!(
            std::fs::read_dir(&directory)
                .expect("test directory should be readable")
                .count(),
            1,
            "replacement save must not leave a temporary file"
        );

        std::fs::remove_dir_all(directory).expect("config save test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn config_file_save_atomically_replaces_with_owner_only_permissions() {
        use std::io::Read as _;
        use std::os::unix::fs::PermissionsExt as _;

        let directory = config_save_test_dir("atomic-private");
        let path = directory.join("tunnel.toml");
        std::fs::write(&path, "old configuration")
            .expect("existing config fixture should be written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("existing config fixture should be world-readable");
        let mut old_handle =
            std::fs::File::open(&path).expect("existing config handle should open");

        secret_bearing_config_file("management-secret")
            .save(&path)
            .expect("config save should succeed");

        let mode = std::fs::metadata(&path)
            .expect("saved config metadata should exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let saved = std::fs::read_to_string(&path).expect("saved config should be readable");
        assert!(saved.contains("management-secret"));

        let mut old = String::new();
        old_handle
            .read_to_string(&mut old)
            .expect("old inode should remain readable through its open handle");
        assert_eq!(old, "old configuration");
        assert_eq!(
            std::fs::read_dir(&directory)
                .expect("test directory should be readable")
                .count(),
            1,
            "successful save must not leave a temporary file"
        );

        std::fs::remove_dir_all(directory).expect("config save test directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn config_file_save_rejects_symbolic_link_targets() {
        use std::os::unix::fs::symlink;

        let directory = config_save_test_dir("symlink");
        let target = directory.join("target.toml");
        let path = directory.join("tunnel.toml");
        std::fs::write(&target, "target sentinel")
            .expect("symlink target fixture should be written");
        symlink(&target, &path).expect("config symlink fixture should be created");

        let error = secret_bearing_config_file("management-secret")
            .save(&path)
            .expect_err("saving through a symlink must fail");

        assert!(error.to_string().contains("symbolic link"));
        assert_eq!(
            std::fs::read_to_string(&target).expect("symlink target should remain readable"),
            "target sentinel"
        );
        assert!(std::fs::symlink_metadata(&path)
            .expect("config symlink should still exist")
            .file_type()
            .is_symlink());

        std::fs::remove_dir_all(directory).expect("config save test directory should be removed");
    }

    #[test]
    fn config_file_save_cleans_temporary_file_when_replace_fails() {
        let directory = config_save_test_dir("cleanup");
        let path = directory.join("destination-is-a-directory");
        std::fs::create_dir(&path).expect("destination directory fixture should be created");

        secret_bearing_config_file("management-secret")
            .save(&path)
            .expect_err("replacing a directory with a config file must fail");

        let entries = std::fs::read_dir(&directory)
            .expect("test directory should be readable")
            .map(|entry| {
                entry
                    .expect("test directory entry should be readable")
                    .path()
            })
            .collect::<Vec<_>>();
        assert_eq!(entries, vec![path]);

        std::fs::remove_dir_all(directory).expect("config save test directory should be removed");
    }

    #[test]
    fn config_file_load_ignores_removed_redirect_replay_budget() {
        let config = parse_config_file_content("redirect_replay_budget_bytes = \"1K\"")
            .expect("removed replay budget should not break existing config files");
        let serialized = toml::to_string(&config).expect("config should serialize");
        assert!(!serialized.contains("redirect_replay_budget_bytes"));
    }

    #[test]
    fn config_file_load_rejects_oversized_files_before_parsing() {
        let directory = config_save_test_dir("oversized-load");
        let path = directory.join("tunnel.toml");
        std::fs::write(&path, vec![b'a'; MAX_CONFIG_FILE_BYTES as usize + 1])
            .expect("oversized config fixture should be written");

        let error = ConfigFile::load(&path).expect_err("oversized config must be rejected");
        assert!(error.to_string().contains("exceeds"));

        std::fs::remove_dir_all(directory).expect("config test directory should be removed");
    }

    #[test]
    fn cli_accepts_but_hides_legacy_redirect_replay_budget() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--redirect-replay-budget-bytes",
            "1K",
        ]);

        assert_eq!(
            config
                .legacy_redirect_replay_budget_bytes_ignored
                .as_deref(),
            Some("1K")
        );
        let mut command = Config::command();
        let legacy_arg = command
            .get_arguments()
            .find(|arg| arg.get_id() == "legacy_redirect_replay_budget_bytes_ignored")
            .expect("legacy redirect replay argument");
        assert_eq!(
            legacy_arg.get_env(),
            Some(std::ffi::OsStr::new(
                "AETHER_TUNNEL_REDIRECT_REPLAY_BUDGET_BYTES"
            ))
        );
        let help = command.render_long_help().to_string();
        assert!(!help.contains("redirect-replay-budget-bytes"));
    }

    #[test]
    fn config_file_deserializes_allow_private_targets() {
        let cfg: ConfigFile = toml::from_str("allow_private_targets = true").expect("bool toml");
        assert_eq!(cfg.allow_private_targets, Some(true));
    }

    #[test]
    fn config_file_deserializes_tunnel_ip_family_flags() {
        let cfg: ConfigFile = toml::from_str(
            r#"
tunnel_ipv4_only = true
tunnel_ipv6_only = false
"#,
        )
        .expect("tunnel IP-family TOML");

        assert_eq!(cfg.tunnel_ipv4_only, Some(true));
        assert_eq!(cfg.tunnel_ipv6_only, Some(false));
    }

    #[test]
    fn config_file_deserializes_server_tunnel_security_fields() {
        let cfg: ConfigFile = toml::from_str(
            r#"
[[servers]]
aether_url = "http://127.0.0.1:8084"
management_token = "ae_test"
node_name = "jp-proxy-01"
tunnel_security = "non_tls_required"
tunnel_encryption_key = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
"#,
        )
        .expect("server tunnel security TOML");

        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(
            cfg.servers[0].tunnel_security,
            Some(TunnelSecurity::NonTlsRequired)
        );
        assert_eq!(
            cfg.servers[0].tunnel_encryption_key.as_deref(),
            Some("BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=")
        );
    }

    #[test]
    fn config_file_deserializes_upstream_proxy_url() {
        let cfg: ConfigFile = toml::from_str("upstream_proxy_url = \"http://proxy.example:8080\"")
            .expect("proxy URL toml");
        assert_eq!(
            cfg.upstream_proxy_url.as_deref(),
            Some("http://proxy.example:8080")
        );
    }

    #[test]
    fn config_file_deserializes_aether_outbound_proxy_url() {
        let cfg: ConfigFile =
            toml::from_str("aether_outbound_proxy_url = \"socks5h://127.0.0.1:1080\"")
                .expect("proxy URL toml");
        assert_eq!(
            cfg.aether_outbound_proxy_url.as_deref(),
            Some("socks5h://127.0.0.1:1080")
        );
    }

    #[test]
    fn config_file_deserializes_legacy_aether_proxy_url_alias() {
        let cfg: ConfigFile = toml::from_str("aether_proxy_url = \"socks5h://127.0.0.1:1080\"")
            .expect("legacy proxy URL toml");
        assert_eq!(
            cfg.aether_outbound_proxy_url.as_deref(),
            Some("socks5h://127.0.0.1:1080")
        );
    }

    #[test]
    fn aether_outbound_proxy_url_requires_explicit_opt_in() {
        let default_direct = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--upstream-proxy-url",
            "socks5h://127.0.0.1:1080",
        ]);
        assert_eq!(default_direct.effective_aether_outbound_proxy_url(), None);

        let explicit = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--upstream-proxy-url",
            "socks5h://127.0.0.1:1080",
            "--aether-outbound-proxy-url",
            "http://127.0.0.1:8080",
        ]);
        assert_eq!(
            explicit.effective_aether_outbound_proxy_url(),
            Some("http://127.0.0.1:8080")
        );
    }

    #[test]
    fn tunnel_logs_default_to_rotating_file_and_stdout() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
        ]);

        assert_eq!(config.log_destination, TunnelLogDestinationArg::Both);
        assert_eq!(config.log_dir.as_deref(), Some(DEFAULT_LOG_DIR));
        assert_eq!(config.log_rotation, TunnelLogRotationArg::Daily);
        assert_eq!(config.log_retention_days, DEFAULT_LOG_RETENTION_DAYS);

        let runtime = config
            .service_runtime_config()
            .expect("default file logging should be valid");
        assert_eq!(runtime.observability.log_destination, LogDestination::Both);
        let file_logging = runtime
            .observability
            .file_logging
            .expect("file logging should be enabled by default");
        assert_eq!(file_logging.dir, std::path::PathBuf::from(DEFAULT_LOG_DIR));
        assert_eq!(file_logging.rotation, LogRotation::Daily);
        assert_eq!(file_logging.retention_days, DEFAULT_LOG_RETENTION_DAYS);
    }

    #[test]
    fn config_file_load_accepts_server_scoped_upstream_proxy_url() {
        let cfg = parse_config_file_content(
            r#"
[[servers]]
aether_url = "https://aether.example.com"
upstream_proxy_url = "socks5://127.0.0.1:1080"
management_token = "ae_test"
node_name = "tunnel-test"
"#,
        )
        .expect("server-scoped proxy URL should be promoted");

        assert_eq!(
            cfg.upstream_proxy_url.as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(cfg.servers[0].aether_url, "https://aether.example.com");
    }

    #[test]
    fn config_file_load_rejects_conflicting_server_scoped_upstream_proxy_url() {
        let error = parse_config_file_content(
            r#"
upstream_proxy_url = "socks5://127.0.0.1:1080"

[[servers]]
aether_url = "https://aether.example.com"
upstream_proxy_url = "socks5://127.0.0.1:1081"
management_token = "ae_test"
node_name = "tunnel-test"
"#,
        )
        .expect_err("conflicting proxy URLs should be rejected");

        assert!(
            error.to_string().contains("conflicting upstream_proxy_url"),
            "error should mention the conflicting key"
        );
    }

    #[test]
    fn config_file_rejects_removed_tunnel_seconds_keys() {
        let error = reject_removed_config_keys("tunnel_ping_interval_secs = 5")
            .expect_err("removed tunnel seconds keys should be rejected");
        assert!(
            error.to_string().contains("tunnel_ping_interval_secs"),
            "error should mention removed key"
        );
    }

    #[test]
    fn config_file_rejects_top_level_single_server_keys() {
        let error = reject_removed_config_keys("aether_url = \"https://example.com\"")
            .expect_err("top-level single-server key should be rejected");
        assert!(
            error.to_string().contains("aether_url"),
            "error should mention removed single-server key"
        );
    }

    #[test]
    fn config_file_rejects_legacy_keys() {
        let error = reject_removed_config_keys("delegate_connect_timeout_secs = 10")
            .expect_err("legacy delegate key should be rejected");
        assert!(
            error.to_string().contains("delegate_connect_timeout_secs"),
            "error should mention removed legacy key"
        );
    }

    #[test]
    fn config_requires_node_name() {
        let command = Config::command();
        let node_name = command
            .get_arguments()
            .find(|arg| arg.get_id() == "node_name")
            .expect("node_name arg");

        assert!(node_name.is_required_set());
        assert!(node_name.get_default_values().is_empty());
    }

    #[test]
    fn cli_defaults_private_targets_to_disabled() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
        ]);
        assert!(!config.allow_private_targets);
    }

    #[test]
    fn cli_allows_explicit_private_targets() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--allow-private-targets",
        ]);
        assert!(config.allow_private_targets);
    }

    #[test]
    fn cli_defaults_tunnel_ip_family_to_any() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
        ]);

        assert!(!config.tunnel_ipv4_only);
        assert!(!config.tunnel_ipv6_only);
        assert_eq!(
            config.tunnel_ip_family(),
            crate::egress_proxy::IpFamily::Any
        );
    }

    #[test]
    fn cli_defaults_tunnel_security_to_off() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
        ]);

        assert_eq!(config.tunnel_security, TunnelSecurity::Off);
        assert!(config.tunnel_encryption_key.is_none());
    }

    #[test]
    fn aether_url_validation_rejects_embedded_secrets_and_non_http_schemes() {
        for value in [
            "https://alice:password@example.com",
            "https://example.com?token=secret",
            "https://example.com#secret-fragment",
            "http://example.com",
            "http://10.0.0.1:8084",
            "http://[::ffff:127.0.0.1]:8084",
            "file:///etc/passwd",
            "not-a-url",
        ] {
            assert!(
                validate_aether_url(value).is_err(),
                "URL should be rejected: {value}"
            );
        }
        validate_aether_url("https://example.com/base/path")
            .expect("ordinary HTTPS URL should validate");
        validate_aether_url("http://127.0.0.1:8084/base/path")
            .expect("literal loopback HTTP should validate");
        validate_aether_url("http://[::1]:8084/base/path")
            .expect("literal IPv6 loopback HTTP should validate");
    }

    #[test]
    fn aether_url_log_projection_removes_credentials_query_and_fragment() {
        let projected = aether_url_for_log(
            "https://alice:password@example.com/base?token=query-secret#secret-fragment",
        );

        assert_eq!(projected, "https://example.com");
        for secret in ["alice", "password", "query-secret", "secret-fragment"] {
            assert!(!projected.contains(secret));
        }
        assert_eq!(aether_url_for_log("not-a-url"), "<invalid-aether-url>");
    }

    #[test]
    fn config_and_server_entries_require_management_tokens() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            " ",
            "--node-name",
            "tunnel-test",
        ]);
        assert!(config.validate().is_err());

        let entry = ServerEntry {
            aether_url: "https://example.com".to_string(),
            management_token: " ".to_string(),
            node_name: None,
            tunnel_security: None,
            tunnel_encryption_key: None,
        };
        assert!(entry.validate().is_err());
    }

    #[test]
    fn secret_bearing_config_debug_output_is_redacted() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://alice:password@example.com/base?token=query-secret",
            "--management-token",
            "management-secret",
            "--node-name",
            "tunnel-test",
            "--tunnel-encryption-key",
            "psk-secret",
        ]);
        let config_debug = format!("{config:?}");
        for secret in [
            "alice",
            "password",
            "query-secret",
            "management-secret",
            "psk-secret",
        ] {
            assert!(!config_debug.contains(secret));
        }

        let entry = ServerEntry {
            aether_url:
                "https://alice:password@example.com/base?token=query-secret#fragment-secret"
                    .to_string(),
            management_token: "management-secret".to_string(),
            node_name: Some("edge-1".to_string()),
            tunnel_security: Some(TunnelSecurity::NonTlsRequired),
            tunnel_encryption_key: Some("psk-secret".to_string()),
        };
        let entry_debug = format!("{entry:?}");
        for secret in [
            "alice",
            "password",
            "query-secret",
            "fragment-secret",
            "management-secret",
            "psk-secret",
        ] {
            assert!(!entry_debug.contains(secret));
        }

        let file = ConfigFile {
            upstream_proxy_url: Some("http://proxy-user:proxy-secret@proxy.example".to_string()),
            servers: vec![entry],
            ..ConfigFile::default()
        };
        let file_debug = format!("{file:?}");
        for secret in [
            "proxy-user",
            "proxy-secret",
            "management-secret",
            "psk-secret",
        ] {
            assert!(!file_debug.contains(secret));
        }
    }

    #[test]
    fn validate_requires_encryption_key_for_non_tls_security() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "http://127.0.0.1:8084",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-security",
            "non_tls_required",
        ]);

        let error = config
            .validate()
            .expect_err("non_tls_required should require a PSK");
        assert!(error.to_string().contains("tunnel_encryption_key"));

        let with_key = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "http://127.0.0.1:8084",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-security",
            "non_tls_required",
            "--tunnel-encryption-key",
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        ]);
        with_key
            .validate()
            .expect("non_tls_required with a PSK should validate");
    }

    #[test]
    fn validate_infers_non_tls_security_for_http_url_with_key() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "http://127.0.0.1:8084",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-encryption-key",
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        ]);

        assert_eq!(config.tunnel_security, TunnelSecurity::Off);
        assert_eq!(
            effective_tunnel_security(
                &config.aether_url,
                None,
                config.tunnel_encryption_key.as_deref(),
            ),
            TunnelSecurity::NonTlsRequired
        );
        assert_eq!(
            effective_tunnel_security(
                &config.aether_url,
                Some(TunnelSecurity::Off),
                config.tunnel_encryption_key.as_deref(),
            ),
            TunnelSecurity::Off
        );
        config
            .validate()
            .expect("http URL with PSK should validate when tunnel_security is off");
    }

    #[test]
    fn validate_rejects_invalid_tunnel_encryption_key() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "http://127.0.0.1:8084",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-security",
            "non_tls_required",
            "--tunnel-encryption-key",
            "not-a-valid-32-byte-key",
        ]);

        let error = config
            .validate()
            .expect_err("invalid PSK should fail validation");
        assert!(error.to_string().contains("base64-encoded 32 bytes"));
    }

    #[test]
    fn cli_accepts_tunnel_ipv4_only() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-ipv4-only",
        ]);

        assert!(config.tunnel_ipv4_only);
        assert_eq!(
            config.tunnel_ip_family(),
            crate::egress_proxy::IpFamily::Ipv4Only
        );
    }

    #[test]
    fn cli_accepts_tunnel_ipv6_only() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-ipv6-only",
        ]);

        assert!(config.tunnel_ipv6_only);
        assert_eq!(
            config.tunnel_ip_family(),
            crate::egress_proxy::IpFamily::Ipv6Only
        );
    }

    #[test]
    fn cli_parses_conflicting_tunnel_ip_family_flags_before_validation() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-ipv4-only",
            "--tunnel-ipv6-only",
        ]);

        assert!(config.tunnel_ipv4_only);
        assert!(config.tunnel_ipv6_only);
        let error = config
            .validate()
            .expect_err("conflicting tunnel IP-family flags should fail validation");
        assert!(error.to_string().contains("tunnel_ipv4_only"));
    }

    #[test]
    fn cli_accepts_explicit_false_tunnel_ip_family_flags() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-ipv4-only=false",
            "--tunnel-ipv6-only=false",
        ]);

        assert!(!config.tunnel_ipv4_only);
        assert!(!config.tunnel_ipv6_only);
        config
            .validate()
            .expect("explicit false family flags should be valid");
    }

    #[test]
    fn validate_rejects_conflicting_toml_tunnel_ip_family_flags() {
        let config = Config {
            tunnel_ipv4_only: true,
            tunnel_ipv6_only: true,
            ..Config::parse_from([
                "aether-tunnel",
                "--aether-url",
                "https://example.com",
                "--management-token",
                "ae_test",
                "--node-name",
                "tunnel-test",
            ])
        };

        let error = config
            .validate()
            .expect_err("conflicting TOML-injected tunnel family flags should fail validation");
        assert!(error.to_string().contains("tunnel_ipv4_only"));
    }

    #[test]
    fn validate_rejects_zero_tunnel_max_streams() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-max-streams",
            "0",
        ]);

        let error = config
            .validate()
            .expect_err("zero tunnel stream capacity must be rejected");
        assert!(error.to_string().contains("tunnel_max_streams"));
    }

    #[test]
    fn tunnel_fast_recovery_defaults_use_millisecond_values() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
        ]);
        assert_eq!(
            config
                .tunnel_ping_interval()
                .expect("ping interval should resolve"),
            Duration::from_millis(DEFAULT_TUNNEL_PING_INTERVAL_MS)
        );
        assert_eq!(
            config
                .tunnel_connect_timeout()
                .expect("connect timeout should resolve"),
            Duration::from_millis(DEFAULT_TUNNEL_CONNECT_TIMEOUT_MS)
        );
        assert_eq!(
            config
                .tunnel_stale_timeout()
                .expect("stale timeout should resolve"),
            Duration::from_millis(DEFAULT_TUNNEL_STALE_TIMEOUT_MS)
        );
        assert_eq!(
            config.tunnel_reconnect_base_ms,
            DEFAULT_TUNNEL_RECONNECT_BASE_MS
        );
        assert_eq!(
            config.tunnel_reconnect_max_ms,
            DEFAULT_TUNNEL_RECONNECT_MAX_MS
        );
    }

    #[test]
    fn tunnel_millisecond_flags_take_effect_when_explicitly_set() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-ping-interval-ms",
            "100",
            "--tunnel-connect-timeout-ms",
            "200",
            "--tunnel-stale-timeout-ms",
            "300",
        ]);
        assert_eq!(
            config
                .tunnel_ping_interval()
                .expect("ping interval should resolve"),
            Duration::from_millis(100)
        );
        assert_eq!(
            config
                .tunnel_connect_timeout()
                .expect("connect timeout should resolve"),
            Duration::from_millis(200)
        );
        assert_eq!(
            config
                .tunnel_stale_timeout()
                .expect("stale timeout should resolve"),
            Duration::from_millis(300)
        );
    }

    #[test]
    fn auto_tunnel_pool_sizing_uses_hardware_capacity() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-max-streams",
            "1024",
        ]);
        let hw = HardwareInfo {
            cpu_cores: 12,
            total_memory_mb: 20_480,
            os_info: "test".to_string(),
            fd_limit: 1_048_576,
            estimated_max_concurrency: 24_000,
        };

        let sizing = config
            .resolve_tunnel_pool_sizing(&hw)
            .expect("sizing should resolve");
        assert_eq!(sizing.initial_connections, 4);
        assert_eq!(sizing.max_connections, 32);
    }

    #[test]
    fn auto_tunnel_pool_sizing_prefers_redundant_floor_when_hardware_allows() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-max-streams",
            "1024",
        ]);
        let hw = HardwareInfo {
            cpu_cores: 4,
            total_memory_mb: 4_096,
            os_info: "test".to_string(),
            fd_limit: 1_048_576,
            estimated_max_concurrency: 64,
        };

        let sizing = config
            .resolve_tunnel_pool_sizing(&hw)
            .expect("sizing should resolve");
        assert_eq!(sizing.initial_connections, 4);
        assert_eq!(sizing.max_connections, 4);
    }

    #[test]
    fn auto_tunnel_pool_sizing_keeps_single_core_nodes_redundant() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-max-streams",
            "200",
        ]);
        let hw = HardwareInfo {
            cpu_cores: 1,
            total_memory_mb: 183,
            os_info: "test".to_string(),
            fd_limit: 65_535,
            estimated_max_concurrency: 2_000,
        };

        let sizing = config
            .resolve_tunnel_pool_sizing(&hw)
            .expect("sizing should resolve");
        assert_eq!(sizing.initial_connections, 4);
        assert_eq!(sizing.max_connections, 4);
    }

    #[test]
    fn auto_tunnel_pool_sizing_respects_stream_admission_limit() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-max-streams",
            "45",
            "--max-in-flight-streams",
            "45",
        ]);
        let hw = HardwareInfo {
            cpu_cores: 1,
            total_memory_mb: 183,
            os_info: "test".to_string(),
            fd_limit: 65_535,
            estimated_max_concurrency: 2_000,
        };

        let sizing = config
            .resolve_tunnel_pool_sizing(&hw)
            .expect("sizing should resolve");
        assert_eq!(sizing.initial_connections, 4);
        assert_eq!(sizing.max_connections, 4);
    }

    #[test]
    fn explicit_tunnel_connections_keep_fixed_pool_without_max_override() {
        let config = Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
            "--tunnel-max-streams",
            "512",
            "--tunnel-connections",
            "2",
        ]);
        let hw = HardwareInfo {
            cpu_cores: 12,
            total_memory_mb: 20_480,
            os_info: "test".to_string(),
            fd_limit: 1_048_576,
            estimated_max_concurrency: 24_000,
        };

        let sizing = config
            .resolve_tunnel_pool_sizing(&hw)
            .expect("sizing should resolve");
        assert_eq!(sizing.initial_connections, 2);
        assert_eq!(sizing.max_connections, 2);
    }
}
