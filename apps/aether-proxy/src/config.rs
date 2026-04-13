use std::fmt;
use std::path::Path;

use aether_runtime::{FileLoggingConfig, LogDestination, LogRotation, ServiceRuntimeConfig};
use clap::Parser;
use serde::{Deserialize, Serialize};

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

/// Default bytes buffered before a tunnel request becomes non-replayable for
/// 307/308 redirects. Kept aligned with the current admin-side request size
/// default, but exposed as an independent proxy transport budget.
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 30;
#[allow(dead_code)]
pub const DEFAULT_REDIRECT_REPLAY_BUDGET_BYTES: usize = 5_242_880;
pub const DEFAULT_REDIRECT_REPLAY_BUDGET_HUMAN: &str = "5M";
pub const DEFAULT_LOG_RETENTION_DAYS: u64 = 7;
pub const DEFAULT_LOG_MAX_FILES: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ByteSizeValue {
    Text(String),
    Integer(u64),
}

impl<'de> Deserialize<'de> for ByteSizeValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ByteSizeValueVisitor;

        impl serde::de::Visitor<'_> for ByteSizeValueVisitor {
            type Value = ByteSizeValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a byte-size string like 5M or an integer byte count")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ByteSizeValue::Integer(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value < 0 {
                    return Err(E::custom("byte size must be >= 0"));
                }
                Ok(ByteSizeValue::Integer(value as u64))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ByteSizeValue::Text(value.to_string()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ByteSizeValue::Text(value))
            }
        }

        deserializer.deserialize_any(ByteSizeValueVisitor)
    }
}

fn deserialize_optional_byte_size<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<ByteSizeValue>::deserialize(deserializer)?;
    value
        .map(|value| match value {
            ByteSizeValue::Text(text) => {
                normalize_byte_size_text(&text).map_err(serde::de::Error::custom)
            }
            ByteSizeValue::Integer(value) => usize::try_from(value)
                .map(format_byte_size_human)
                .map_err(|_| serde::de::Error::custom("byte size exceeds usize")),
        })
        .transpose()
}

pub fn parse_byte_size(input: &str) -> Result<usize, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("byte size must not be empty".to_string());
    }

    let digits_end = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    if digits_end == 0 {
        return Err(format!("invalid byte size `{trimmed}`"));
    }

    let number = trimmed[..digits_end]
        .parse::<u64>()
        .map_err(|_| format!("invalid byte size `{trimmed}`"))?;
    let suffix = trimmed[digits_end..].trim().to_ascii_lowercase();
    let multiplier = match suffix.as_str() {
        "" | "b" => 1u64,
        "k" | "kb" | "kib" => 1024u64,
        "m" | "mb" | "mib" => 1024u64.pow(2),
        "g" | "gb" | "gib" => 1024u64.pow(3),
        _ => {
            return Err(format!(
                "invalid byte size suffix `{}`; use B, K, M, or G",
                &trimmed[digits_end..].trim()
            ))
        }
    };

    let total = number
        .checked_mul(multiplier)
        .ok_or_else(|| format!("byte size `{trimmed}` is too large"))?;
    usize::try_from(total).map_err(|_| format!("byte size `{trimmed}` exceeds usize"))
}

fn normalize_byte_size_text(input: &str) -> Result<String, String> {
    parse_byte_size(input).map(format_byte_size_human)
}

pub fn format_byte_size_human(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * 1024;
    const GIB: usize = 1024 * 1024 * 1024;

    if bytes == 0 {
        return "0".to_string();
    }
    if bytes.is_multiple_of(GIB) {
        return format!("{}G", bytes / GIB);
    }
    if bytes.is_multiple_of(MIB) {
        return format!("{}M", bytes / MIB);
    }
    if bytes.is_multiple_of(KIB) {
        return format!("{}K", bytes / KIB);
    }
    bytes.to_string()
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProxyLogDestinationArg {
    Stdout,
    File,
    Both,
}

impl From<ProxyLogDestinationArg> for LogDestination {
    fn from(value: ProxyLogDestinationArg) -> Self {
        match value {
            ProxyLogDestinationArg::Stdout => LogDestination::Stdout,
            ProxyLogDestinationArg::File => LogDestination::File,
            ProxyLogDestinationArg::Both => LogDestination::Both,
        }
    }
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProxyLogRotationArg {
    Hourly,
    Daily,
}

impl From<ProxyLogRotationArg> for LogRotation {
    fn from(value: ProxyLogRotationArg) -> Self {
        match value {
            ProxyLogRotationArg::Hourly => LogRotation::Hourly,
            ProxyLogRotationArg::Daily => LogRotation::Daily,
        }
    }
}

/// Aether tunnel proxy.
///
/// Deployed on overseas VPS to relay API traffic for Aether instances
/// behind the GFW. Connects to Aether via WebSocket tunnel, registers
/// with Aether, and relays upstream requests.
#[derive(Parser, Debug, Clone)]
#[command(version, about)]
pub struct Config {
    /// Aether server URL (e.g. https://aether.example.com)
    #[arg(long, env = "AETHER_PROXY_AETHER_URL")]
    pub aether_url: String,

    /// Management Token for Aether admin API (ae_xxx)
    #[arg(long, env = "AETHER_PROXY_MANAGEMENT_TOKEN")]
    pub management_token: String,

    /// Public IP address of this node (auto-detected if omitted)
    #[arg(long, env = "AETHER_PROXY_PUBLIC_IP")]
    pub public_ip: Option<String>,

    /// Human-readable node name
    #[arg(long, env = "AETHER_PROXY_NODE_NAME")]
    pub node_name: String,

    /// Region label (e.g. ap-northeast-1)
    #[arg(long, env = "AETHER_PROXY_NODE_REGION")]
    pub node_region: Option<String>,

    /// Heartbeat interval in seconds
    #[arg(
        long,
        env = "AETHER_PROXY_HEARTBEAT_INTERVAL",
        default_value_t = DEFAULT_HEARTBEAT_INTERVAL_SECS
    )]
    pub heartbeat_interval: u64,

    /// Allowed destination ports (default: 80,443,8080,8443)
    #[arg(
        long,
        env = "AETHER_PROXY_ALLOWED_PORTS",
        value_delimiter = ',',
        default_values_t = vec![80, 443, 8080, 8443]
    )]
    pub allowed_ports: Vec<u16>,

    /// Aether API request timeout in seconds
    #[arg(
        long,
        env = "AETHER_PROXY_AETHER_REQUEST_TIMEOUT",
        default_value_t = 10
    )]
    pub aether_request_timeout_secs: u64,

    /// Aether API connect timeout in seconds
    #[arg(
        long,
        env = "AETHER_PROXY_AETHER_CONNECT_TIMEOUT",
        default_value_t = 10
    )]
    pub aether_connect_timeout_secs: u64,

    /// Aether API max idle connections per host
    #[arg(
        long,
        env = "AETHER_PROXY_AETHER_POOL_MAX_IDLE_PER_HOST",
        default_value_t = 8
    )]
    pub aether_pool_max_idle_per_host: usize,

    /// Aether API idle timeout in seconds
    #[arg(
        long,
        env = "AETHER_PROXY_AETHER_POOL_IDLE_TIMEOUT",
        default_value_t = 90
    )]
    pub aether_pool_idle_timeout_secs: u64,

    /// Aether API TCP keepalive in seconds (0 disables)
    #[arg(long, env = "AETHER_PROXY_AETHER_TCP_KEEPALIVE", default_value_t = 60)]
    pub aether_tcp_keepalive_secs: u64,

    /// Aether API TCP_NODELAY
    #[arg(long, env = "AETHER_PROXY_AETHER_TCP_NODELAY", default_value_t = true)]
    pub aether_tcp_nodelay: bool,

    /// Enable HTTP/2 when talking to Aether API
    #[arg(long, env = "AETHER_PROXY_AETHER_HTTP2", default_value_t = true)]
    pub aether_http2: bool,

    /// Aether API retry attempts (including initial)
    #[arg(
        long,
        env = "AETHER_PROXY_AETHER_RETRY_MAX_ATTEMPTS",
        default_value_t = 3
    )]
    pub aether_retry_max_attempts: u32,

    /// Aether API retry base delay in milliseconds
    #[arg(
        long,
        env = "AETHER_PROXY_AETHER_RETRY_BASE_DELAY_MS",
        default_value_t = 200
    )]
    pub aether_retry_base_delay_ms: u64,

    /// Aether API retry max delay in milliseconds
    #[arg(
        long,
        env = "AETHER_PROXY_AETHER_RETRY_MAX_DELAY_MS",
        default_value_t = 2000
    )]
    pub aether_retry_max_delay_ms: u64,

    /// Maximum concurrent TCP connections (defaults to hardware estimate)
    #[arg(long, env = "AETHER_PROXY_MAX_CONCURRENT_CONNECTIONS")]
    pub max_concurrent_connections: Option<u64>,

    /// Maximum in-flight tunneled streams accepted by this proxy instance.
    #[arg(long, env = "AETHER_PROXY_MAX_IN_FLIGHT_STREAMS")]
    pub max_in_flight_streams: Option<usize>,

    /// Maximum in-flight tunneled streams admitted across all proxy instances.
    #[arg(long, env = "AETHER_PROXY_DISTRIBUTED_STREAM_LIMIT")]
    pub distributed_stream_limit: Option<usize>,

    /// Redis URL used for cross-instance stream admission.
    #[arg(long, env = "AETHER_PROXY_DISTRIBUTED_STREAM_REDIS_URL")]
    pub distributed_stream_redis_url: Option<String>,

    /// Optional key prefix for cross-instance stream admission state.
    #[arg(long, env = "AETHER_PROXY_DISTRIBUTED_STREAM_REDIS_KEY_PREFIX")]
    pub distributed_stream_redis_key_prefix: Option<String>,

    /// Lease TTL in milliseconds for distributed stream admission permits.
    #[arg(
        long,
        env = "AETHER_PROXY_DISTRIBUTED_STREAM_LEASE_TTL_MS",
        default_value_t = 30_000
    )]
    pub distributed_stream_lease_ttl_ms: u64,

    /// Renew interval in milliseconds for distributed stream admission permits.
    #[arg(
        long,
        env = "AETHER_PROXY_DISTRIBUTED_STREAM_RENEW_INTERVAL_MS",
        default_value_t = 10_000
    )]
    pub distributed_stream_renew_interval_ms: u64,

    /// Command timeout in milliseconds for distributed stream admission Redis calls.
    #[arg(
        long,
        env = "AETHER_PROXY_DISTRIBUTED_STREAM_COMMAND_TIMEOUT_MS",
        default_value_t = 1_000
    )]
    pub distributed_stream_command_timeout_ms: u64,

    /// DNS cache TTL in seconds
    #[arg(long, env = "AETHER_PROXY_DNS_CACHE_TTL", default_value_t = 60)]
    pub dns_cache_ttl_secs: u64,

    /// DNS cache capacity (entries)
    #[arg(long, env = "AETHER_PROXY_DNS_CACHE_CAPACITY", default_value_t = 1024)]
    pub dns_cache_capacity: usize,

    /// Upstream HTTP client connect timeout in seconds
    #[arg(
        long,
        env = "AETHER_PROXY_UPSTREAM_CONNECT_TIMEOUT",
        default_value_t = 30
    )]
    pub upstream_connect_timeout_secs: u64,

    /// Upstream HTTP client max idle connections per host
    #[arg(
        long,
        env = "AETHER_PROXY_UPSTREAM_POOL_MAX_IDLE_PER_HOST",
        default_value_t = 64
    )]
    pub upstream_pool_max_idle_per_host: usize,

    /// Upstream HTTP client idle timeout in seconds
    #[arg(
        long,
        env = "AETHER_PROXY_UPSTREAM_POOL_IDLE_TIMEOUT",
        default_value_t = 300
    )]
    pub upstream_pool_idle_timeout_secs: u64,

    /// Upstream TCP keepalive in seconds (0 disables)
    #[arg(
        long,
        env = "AETHER_PROXY_UPSTREAM_TCP_KEEPALIVE",
        default_value_t = 60
    )]
    pub upstream_tcp_keepalive_secs: u64,

    /// Upstream TCP_NODELAY
    #[arg(
        long,
        env = "AETHER_PROXY_UPSTREAM_TCP_NODELAY",
        default_value_t = true
    )]
    pub upstream_tcp_nodelay: bool,

    /// Maximum request body bytes buffered to support 307/308 redirect replay.
    /// Accepts values like 5M / 512K / 1G. Set to 0 to disable request-body replay buffering.
    #[arg(
        long,
        env = "AETHER_PROXY_REDIRECT_REPLAY_BUDGET_BYTES",
        value_parser = parse_byte_size,
        default_value = DEFAULT_REDIRECT_REPLAY_BUDGET_HUMAN
    )]
    pub redirect_replay_budget_bytes: usize,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, env = "AETHER_PROXY_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// Log destination (stdout, file, both)
    #[arg(
        long,
        env = "AETHER_PROXY_LOG_DESTINATION",
        value_enum,
        default_value = "stdout"
    )]
    pub log_destination: ProxyLogDestinationArg,

    /// Log directory when file logging is enabled
    #[arg(long, env = "AETHER_PROXY_LOG_DIR")]
    pub log_dir: Option<String>,

    /// Log rotation schedule for file logging
    #[arg(
        long,
        env = "AETHER_PROXY_LOG_ROTATION",
        value_enum,
        default_value = "daily"
    )]
    pub log_rotation: ProxyLogRotationArg,

    /// Log file retention days for file logging
    #[arg(
        long,
        env = "AETHER_PROXY_LOG_RETENTION_DAYS",
        default_value_t = DEFAULT_LOG_RETENTION_DAYS
    )]
    pub log_retention_days: u64,

    /// Maximum number of retained rolled log files
    #[arg(
        long,
        env = "AETHER_PROXY_LOG_MAX_FILES",
        default_value_t = DEFAULT_LOG_MAX_FILES
    )]
    pub log_max_files: usize,

    /// Tunnel reconnect base delay in milliseconds (used by exponential backoff)
    #[arg(
        long,
        env = "AETHER_PROXY_TUNNEL_RECONNECT_BASE_MS",
        default_value_t = 500
    )]
    pub tunnel_reconnect_base_ms: u64,

    /// Tunnel reconnect max delay in milliseconds (cap for exponential backoff)
    #[arg(
        long,
        env = "AETHER_PROXY_TUNNEL_RECONNECT_MAX_MS",
        default_value_t = 30000
    )]
    pub tunnel_reconnect_max_ms: u64,

    /// WebSocket tunnel ping interval in seconds
    #[arg(long, env = "AETHER_PROXY_TUNNEL_PING_INTERVAL", default_value_t = 15)]
    pub tunnel_ping_interval_secs: u64,

    /// Maximum concurrent streams over tunnel (auto-detected from hardware if omitted)
    #[arg(long, env = "AETHER_PROXY_TUNNEL_MAX_STREAMS")]
    pub tunnel_max_streams: Option<u32>,

    /// WebSocket tunnel TCP connect timeout in seconds
    #[arg(
        long,
        env = "AETHER_PROXY_TUNNEL_CONNECT_TIMEOUT",
        default_value_t = 15
    )]
    pub tunnel_connect_timeout_secs: u64,

    /// WebSocket tunnel TCP keepalive in seconds (0 disables)
    #[arg(long, env = "AETHER_PROXY_TUNNEL_TCP_KEEPALIVE", default_value_t = 30)]
    pub tunnel_tcp_keepalive_secs: u64,

    /// WebSocket tunnel TCP_NODELAY
    #[arg(long, env = "AETHER_PROXY_TUNNEL_TCP_NODELAY", default_value_t = true)]
    pub tunnel_tcp_nodelay: bool,

    /// Tunnel connection staleness timeout in seconds (triggers reconnect if no data received)
    #[arg(long, env = "AETHER_PROXY_TUNNEL_STALE_TIMEOUT", default_value_t = 45)]
    pub tunnel_stale_timeout_secs: u64,

    /// Number of parallel WebSocket tunnel connections per server (connection pool)
    #[arg(long, env = "AETHER_PROXY_TUNNEL_CONNECTIONS", default_value_t = 3)]
    pub tunnel_connections: u32,
}

impl Config {
    /// Validate configuration values are within sane ranges.
    /// Called after parsing to catch misconfigurations early.
    pub fn validate(&self) -> anyhow::Result<()> {
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
        for &port in &self.allowed_ports {
            if port == 0 {
                anyhow::bail!("allowed_ports: port 0 is not valid");
            }
        }
        if self.tunnel_connect_timeout_secs == 0 {
            anyhow::bail!("tunnel_connect_timeout_secs must be > 0");
        }
        if self.tunnel_ping_interval_secs == 0 {
            anyhow::bail!("tunnel_ping_interval_secs must be > 0");
        }
        if self.tunnel_stale_timeout_secs <= self.tunnel_ping_interval_secs {
            anyhow::bail!(
                "tunnel_stale_timeout_secs ({}) must be > tunnel_ping_interval_secs ({})",
                self.tunnel_stale_timeout_secs,
                self.tunnel_ping_interval_secs
            );
        }
        if self.tunnel_connections == 0 {
            anyhow::bail!("tunnel_connections must be > 0");
        }
        if self.aether_retry_max_attempts == 0 {
            anyhow::bail!("aether_retry_max_attempts must be >= 1");
        }
        if self.upstream_connect_timeout_secs == 0 {
            anyhow::bail!("upstream_connect_timeout_secs must be > 0");
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
            ProxyLogDestinationArg::File | ProxyLogDestinationArg::Both
        ) && self
            .log_dir
            .as_deref()
            .map(str::trim)
            .is_none_or(|value| value.is_empty())
        {
            anyhow::bail!("log_dir must be set when AETHER_PROXY_LOG_DESTINATION is file or both");
        }
        Ok(())
    }

    pub fn service_runtime_config(&self) -> anyhow::Result<ServiceRuntimeConfig> {
        let mut config = ServiceRuntimeConfig::new("aether-proxy", "aether_proxy=info")
            .with_log_format(aether_runtime::LogFormat::Pretty)
            .with_log_destination(self.log_destination.into())
            .with_node_role("proxy")
            .with_instance_id(self.node_name.trim().to_string());
        if matches!(
            self.log_destination,
            ProxyLogDestinationArg::File | ProxyLogDestinationArg::Both
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub aether_url: String,
    pub management_token: String,
    /// Per-server node name override. Falls back to the global `node_name`.
    pub node_name: Option<String>,
}

// ---------------------------------------------------------------------------
// TOML config file support
// ---------------------------------------------------------------------------

/// Serializable config for TOML file persistence.
/// All fields are optional -- only populated values are written.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub management_token: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_retry_max_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_retry_base_delay_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aether_retry_max_delay_ms: Option<u64>,
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
    pub upstream_tcp_keepalive_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_tcp_nodelay: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_byte_size"
    )]
    pub redirect_replay_budget_bytes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_destination: Option<ProxyLogDestinationArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_rotation: Option<ProxyLogRotationArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_retention_days: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_max_files: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_reconnect_base_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_reconnect_max_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_ping_interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_max_streams: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_connect_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_tcp_keepalive_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_tcp_nodelay: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_stale_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_connections: Option<u32>,

    /// Multi-server config: each entry connects to a separate Aether instance.
    /// When present, top-level aether_url/management_token are ignored for
    /// tunnel connections (but still injected as env for clap compatibility).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerEntry>,
}

impl ConfigFile {
    /// Load from a TOML file.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }

    /// Save to a TOML file.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Detect and migrate a 0.1.x config file to 0.2.0 format in-place.
    ///
    /// Returns `true` if migration was performed, `false` if already current.
    /// The original file is backed up as `<name>.v1.bak` before rewriting.
    pub fn migrate_legacy(path: &Path) -> anyhow::Result<bool> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };
        let mut table: toml::map::Map<String, toml::Value> = toml::from_str(&content)?;

        // Detect legacy format: presence of any 0.1.x-only key.
        let is_legacy = LEGACY_ONLY_KEYS.iter().any(|k| table.contains_key(*k))
            || DELEGATE_TO_UPSTREAM
                .iter()
                .any(|(old, _)| table.contains_key(*old));

        if !is_legacy {
            return Ok(false);
        }

        // 1. Rename delegate_* -> upstream_* (carry over user-customized values)
        for &(old, new) in DELEGATE_TO_UPSTREAM {
            if let Some(val) = table.remove(old) {
                table.entry(new.to_string()).or_insert(val);
            }
        }

        // 2. Build [[servers]] from top-level aether_url + management_token + node_name
        if !table.contains_key("servers") {
            let aether_url = table.get("aether_url").and_then(|v| v.as_str());
            let management_token = table.get("management_token").and_then(|v| v.as_str());
            if let (Some(url), Some(token)) = (aether_url, management_token) {
                let mut entry = toml::map::Map::new();
                entry.insert("aether_url".into(), toml::Value::String(url.to_string()));
                entry.insert(
                    "management_token".into(),
                    toml::Value::String(token.to_string()),
                );
                if let Some(name) = table.get("node_name").and_then(|v| v.as_str()) {
                    entry.insert("node_name".into(), toml::Value::String(name.to_string()));
                }
                table.insert(
                    "servers".into(),
                    toml::Value::Array(vec![toml::Value::Table(entry)]),
                );
            }
        }

        // 3. Remove top-level fields that are now in [[servers]] or obsolete
        table.remove("aether_url");
        table.remove("management_token");
        table.remove("node_name");
        for &key in LEGACY_ONLY_KEYS {
            table.remove(key);
        }

        // 4. Backup original file (abort migration if backup fails)
        let backup_path = path.with_extension("v1.bak");
        std::fs::copy(path, &backup_path).map_err(|e| {
            anyhow::anyhow!(
                "failed to backup config before migration: {} -> {}: {}",
                path.display(),
                backup_path.display(),
                e
            )
        })?;

        // 5. Write migrated config
        let new_content = toml::to_string_pretty(&table)?;
        std::fs::write(path, &new_content)?;

        eprintln!("  Config migrated from 0.1.x to 0.2.0 format.");
        eprintln!("  Backup saved: {}", backup_path.display());

        Ok(true)
    }

    /// Resolve the effective server list.
    ///
    /// If `[[servers]]` is present, use it. Otherwise fall back to the
    /// top-level `aether_url` + `management_token` as a single server.
    pub fn effective_servers(&self) -> Vec<ServerEntry> {
        if !self.servers.is_empty() {
            return self.servers.clone();
        }
        match (&self.aether_url, &self.management_token) {
            (Some(url), Some(token)) => vec![ServerEntry {
                aether_url: url.clone(),
                management_token: token.clone(),
                node_name: None,
            }],
            _ => vec![],
        }
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

        // When top-level fields are absent, fall back to the first [[servers]]
        // entry so that clap's required `aether_url` / `management_token` are
        // satisfied even with the new config format.
        let first_server = self.servers.first();
        let aether_url = self
            .aether_url
            .as_deref()
            .or(first_server.map(|s| s.aether_url.as_str()));
        let management_token = self
            .management_token
            .as_deref()
            .or(first_server.map(|s| s.management_token.as_str()));
        let node_name = self
            .node_name
            .as_deref()
            .or(first_server.and_then(|s| s.node_name.as_deref()));

        set!("AETHER_PROXY_AETHER_URL", aether_url);
        set!("AETHER_PROXY_MANAGEMENT_TOKEN", management_token);
        set!("AETHER_PROXY_PUBLIC_IP", self.public_ip);
        set!("AETHER_PROXY_NODE_NAME", node_name);
        set!("AETHER_PROXY_NODE_REGION", self.node_region);
        set!("AETHER_PROXY_HEARTBEAT_INTERVAL", self.heartbeat_interval);
        set!(
            "AETHER_PROXY_AETHER_REQUEST_TIMEOUT",
            self.aether_request_timeout_secs
        );
        set!(
            "AETHER_PROXY_AETHER_CONNECT_TIMEOUT",
            self.aether_connect_timeout_secs
        );
        set!(
            "AETHER_PROXY_AETHER_POOL_MAX_IDLE_PER_HOST",
            self.aether_pool_max_idle_per_host
        );
        set!(
            "AETHER_PROXY_AETHER_POOL_IDLE_TIMEOUT",
            self.aether_pool_idle_timeout_secs
        );
        set!(
            "AETHER_PROXY_AETHER_TCP_KEEPALIVE",
            self.aether_tcp_keepalive_secs
        );
        set!("AETHER_PROXY_AETHER_TCP_NODELAY", self.aether_tcp_nodelay);
        set!("AETHER_PROXY_AETHER_HTTP2", self.aether_http2);
        set!(
            "AETHER_PROXY_AETHER_RETRY_MAX_ATTEMPTS",
            self.aether_retry_max_attempts
        );
        set!(
            "AETHER_PROXY_AETHER_RETRY_BASE_DELAY_MS",
            self.aether_retry_base_delay_ms
        );
        set!(
            "AETHER_PROXY_AETHER_RETRY_MAX_DELAY_MS",
            self.aether_retry_max_delay_ms
        );
        set!(
            "AETHER_PROXY_MAX_CONCURRENT_CONNECTIONS",
            self.max_concurrent_connections
        );
        set!("AETHER_PROXY_DNS_CACHE_TTL", self.dns_cache_ttl_secs);
        set!("AETHER_PROXY_DNS_CACHE_CAPACITY", self.dns_cache_capacity);
        set!(
            "AETHER_PROXY_UPSTREAM_CONNECT_TIMEOUT",
            self.upstream_connect_timeout_secs
        );
        set!(
            "AETHER_PROXY_UPSTREAM_POOL_MAX_IDLE_PER_HOST",
            self.upstream_pool_max_idle_per_host
        );
        set!(
            "AETHER_PROXY_UPSTREAM_POOL_IDLE_TIMEOUT",
            self.upstream_pool_idle_timeout_secs
        );
        set!(
            "AETHER_PROXY_UPSTREAM_TCP_KEEPALIVE",
            self.upstream_tcp_keepalive_secs
        );
        set!(
            "AETHER_PROXY_UPSTREAM_TCP_NODELAY",
            self.upstream_tcp_nodelay
        );
        set!(
            "AETHER_PROXY_REDIRECT_REPLAY_BUDGET_BYTES",
            self.redirect_replay_budget_bytes
        );
        set!("AETHER_PROXY_LOG_LEVEL", self.log_level);
        set!(
            "AETHER_PROXY_LOG_DESTINATION",
            self.log_destination.map(|v| match v {
                ProxyLogDestinationArg::Stdout => "stdout",
                ProxyLogDestinationArg::File => "file",
                ProxyLogDestinationArg::Both => "both",
            })
        );
        set!("AETHER_PROXY_LOG_DIR", self.log_dir.as_deref());
        set!(
            "AETHER_PROXY_LOG_ROTATION",
            self.log_rotation.map(|v| match v {
                ProxyLogRotationArg::Hourly => "hourly",
                ProxyLogRotationArg::Daily => "daily",
            })
        );
        set!("AETHER_PROXY_LOG_RETENTION_DAYS", self.log_retention_days);
        set!("AETHER_PROXY_LOG_MAX_FILES", self.log_max_files);
        set!(
            "AETHER_PROXY_TUNNEL_RECONNECT_BASE_MS",
            self.tunnel_reconnect_base_ms
        );
        set!(
            "AETHER_PROXY_TUNNEL_RECONNECT_MAX_MS",
            self.tunnel_reconnect_max_ms
        );
        set!(
            "AETHER_PROXY_TUNNEL_PING_INTERVAL",
            self.tunnel_ping_interval_secs
        );
        set!("AETHER_PROXY_TUNNEL_MAX_STREAMS", self.tunnel_max_streams);
        set!(
            "AETHER_PROXY_TUNNEL_CONNECT_TIMEOUT",
            self.tunnel_connect_timeout_secs
        );
        set!(
            "AETHER_PROXY_TUNNEL_TCP_KEEPALIVE",
            self.tunnel_tcp_keepalive_secs
        );
        set!("AETHER_PROXY_TUNNEL_TCP_NODELAY", self.tunnel_tcp_nodelay);
        set!(
            "AETHER_PROXY_TUNNEL_STALE_TIMEOUT",
            self.tunnel_stale_timeout_secs
        );
        set!("AETHER_PROXY_TUNNEL_CONNECTIONS", self.tunnel_connections);

        // allowed_ports needs special handling (comma-separated)
        if let Some(ref ports) = self.allowed_ports {
            if force || std::env::var("AETHER_PROXY_ALLOWED_PORTS").is_err() {
                let s: String = ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                std::env::set_var("AETHER_PROXY_ALLOWED_PORTS", s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn parse_byte_size_supports_human_units() {
        assert_eq!(
            parse_byte_size("5M").expect("5M should parse"),
            5 * 1024 * 1024
        );
        assert_eq!(
            parse_byte_size("512K").expect("512K should parse"),
            512 * 1024
        );
        assert_eq!(
            parse_byte_size("1G").expect("1G should parse"),
            1024 * 1024 * 1024
        );
        assert_eq!(parse_byte_size("0").expect("0 should parse"), 0);
    }

    #[test]
    fn config_file_deserializes_budget_from_integer_and_string() {
        let numeric: ConfigFile =
            toml::from_str("redirect_replay_budget_bytes = 5242880").expect("numeric toml");
        assert_eq!(numeric.redirect_replay_budget_bytes.as_deref(), Some("5M"));

        let stringy: ConfigFile =
            toml::from_str("redirect_replay_budget_bytes = \"6m\"").expect("string toml");
        assert_eq!(stringy.redirect_replay_budget_bytes.as_deref(), Some("6M"));
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
}
