#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use clap::{Args as ClapArgs, Parser, ValueEnum};
use tracing::{debug, info, warn};

use aether_crypto::warm_python_fernet_secret;
use aether_data::postgres::PostgresPoolConfig;
use aether_data::redis::RedisClientConfig;
use aether_gateway::{
    attach_static_frontend, build_router_with_state, AppState, FrontdoorCorsConfig,
    FrontdoorUserRpmConfig, GatewayDataConfig, UsageRuntimeConfig, VideoTaskTruthSourceMode,
};
use aether_runtime::{
    init_service_runtime, DistributedConcurrencyGate, FileLoggingConfig, LogDestination, LogFormat,
    LogRotation, RedisDistributedConcurrencyConfig, ServiceRuntimeConfig,
};

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum VideoTaskTruthSourceArg {
    PythonSyncReport,
    RustAuthoritative,
}

impl From<VideoTaskTruthSourceArg> for VideoTaskTruthSourceMode {
    fn from(value: VideoTaskTruthSourceArg) -> Self {
        match value {
            VideoTaskTruthSourceArg::PythonSyncReport => VideoTaskTruthSourceMode::PythonSyncReport,
            VideoTaskTruthSourceArg::RustAuthoritative => {
                VideoTaskTruthSourceMode::RustAuthoritative
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum DeploymentTopologyArg {
    SingleNode,
    MultiNode,
}

impl DeploymentTopologyArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SingleNode => "single-node",
            Self::MultiNode => "multi-node",
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum NodeRoleArg {
    All,
    Frontdoor,
    Background,
}

impl NodeRoleArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Frontdoor => "frontdoor",
            Self::Background => "background",
        }
    }

    const fn spawns_background_tasks(self) -> bool {
        matches!(self, Self::All | Self::Background)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum GatewayLogFormatArg {
    Pretty,
    Json,
}

impl From<GatewayLogFormatArg> for LogFormat {
    fn from(value: GatewayLogFormatArg) -> Self {
        match value {
            GatewayLogFormatArg::Pretty => LogFormat::Pretty,
            GatewayLogFormatArg::Json => LogFormat::Json,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum GatewayLogDestinationArg {
    Stdout,
    File,
    Both,
}

impl GatewayLogDestinationArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::File => "file",
            Self::Both => "both",
        }
    }
}

impl From<GatewayLogDestinationArg> for LogDestination {
    fn from(value: GatewayLogDestinationArg) -> Self {
        match value {
            GatewayLogDestinationArg::Stdout => LogDestination::Stdout,
            GatewayLogDestinationArg::File => LogDestination::File,
            GatewayLogDestinationArg::Both => LogDestination::Both,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum GatewayLogRotationArg {
    Hourly,
    Daily,
}

impl GatewayLogRotationArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Hourly => "hourly",
            Self::Daily => "daily",
        }
    }
}

impl From<GatewayLogRotationArg> for LogRotation {
    fn from(value: GatewayLogRotationArg) -> Self {
        match value {
            GatewayLogRotationArg::Hourly => LogRotation::Hourly,
            GatewayLogRotationArg::Daily => LogRotation::Daily,
        }
    }
}

fn env_var_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(ClapArgs, Debug, Clone)]
struct GatewayDataArgs {
    #[arg(long, env = "AETHER_GATEWAY_DATA_POSTGRES_URL")]
    postgres_url: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_ENCRYPTION_KEY")]
    encryption_key: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_REDIS_URL")]
    redis_url: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_REDIS_KEY_PREFIX")]
    redis_key_prefix: Option<String>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS",
        default_value_t = 1
    )]
    postgres_min_connections: u32,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS",
        default_value_t = 30
    )]
    postgres_max_connections: u32,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_ACQUIRE_TIMEOUT_MS",
        default_value_t = 3_000
    )]
    postgres_acquire_timeout_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_IDLE_TIMEOUT_MS",
        default_value_t = 60_000
    )]
    postgres_idle_timeout_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_LIFETIME_MS",
        default_value_t = 1_800_000
    )]
    postgres_max_lifetime_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_STATEMENT_CACHE_CAPACITY",
        default_value_t = 100
    )]
    postgres_statement_cache_capacity: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_REQUIRE_SSL",
        default_value_t = false
    )]
    postgres_require_ssl: bool,
}

impl GatewayDataArgs {
    fn effective_postgres_url(&self) -> Option<String> {
        self.postgres_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                std::env::var("DATABASE_URL")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
    }

    fn effective_redis_url(&self) -> Option<String> {
        self.redis_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                std::env::var("REDIS_URL")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
    }

    fn effective_encryption_key(&self) -> Option<String> {
        self.encryption_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                std::env::var("ENCRYPTION_KEY")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
    }

    fn configured_encryption_key_mismatch(&self) -> bool {
        let gateway_value = std::env::var("AETHER_GATEWAY_DATA_ENCRYPTION_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let default_value = std::env::var("ENCRYPTION_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        matches!(
            (gateway_value, default_value),
            (Some(gateway_value), Some(default_value)) if gateway_value != default_value
        )
    }

    fn to_config(&self) -> GatewayDataConfig {
        let database_url = self.effective_postgres_url();
        let redis_url = self.effective_redis_url();

        let mut config = match database_url.as_deref() {
            Some(database_url) => GatewayDataConfig::from_postgres_config(PostgresPoolConfig {
                database_url: database_url.to_string(),
                min_connections: self.postgres_min_connections,
                max_connections: self.postgres_max_connections,
                acquire_timeout_ms: self.postgres_acquire_timeout_ms,
                idle_timeout_ms: self.postgres_idle_timeout_ms,
                max_lifetime_ms: self.postgres_max_lifetime_ms,
                statement_cache_capacity: self.postgres_statement_cache_capacity,
                require_ssl: self.postgres_require_ssl,
            }),
            None => GatewayDataConfig::disabled(),
        };

        if let Some(redis_url) = redis_url.as_deref() {
            config = config.with_redis_config(RedisClientConfig {
                url: redis_url.to_string(),
                key_prefix: self
                    .redis_key_prefix
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
            });
        }

        match self.effective_encryption_key() {
            Some(value) => {
                warm_python_fernet_secret(&value);
                config.with_encryption_key(value)
            }
            None => config,
        }
    }
}

#[derive(ClapArgs, Debug, Clone)]
struct GatewayUsageArgs {
    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_STREAM_KEY",
        default_value = "usage:events"
    )]
    queue_stream_key: String,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_GROUP",
        default_value = "usage_consumers"
    )]
    queue_group: String,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_DLQ_STREAM_KEY",
        default_value = "usage:events:dlq"
    )]
    queue_dlq_stream_key: String,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_STREAM_MAXLEN",
        default_value_t = 2_000
    )]
    queue_stream_maxlen: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_BATCH_SIZE",
        default_value_t = 200
    )]
    queue_batch_size: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_BLOCK_MS",
        default_value_t = 500
    )]
    queue_block_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_IDLE_MS",
        default_value_t = 30_000
    )]
    queue_reclaim_idle_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_COUNT",
        default_value_t = 200
    )]
    queue_reclaim_count: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_INTERVAL_MS",
        default_value_t = 5_000
    )]
    queue_reclaim_interval_ms: u64,
}

impl GatewayUsageArgs {
    fn to_config(&self) -> UsageRuntimeConfig {
        UsageRuntimeConfig {
            enabled: true,
            stream_key: self.queue_stream_key.trim().to_string(),
            consumer_group: self.queue_group.trim().to_string(),
            dlq_stream_key: self.queue_dlq_stream_key.trim().to_string(),
            stream_maxlen: self.queue_stream_maxlen.max(1),
            consumer_batch_size: self.queue_batch_size.max(1),
            consumer_block_ms: self.queue_block_ms.max(1),
            reclaim_idle_ms: self.queue_reclaim_idle_ms.max(1),
            reclaim_count: self.queue_reclaim_count.max(1),
            reclaim_interval_ms: self.queue_reclaim_interval_ms.max(1),
        }
    }
}

#[derive(ClapArgs, Debug, Clone)]
struct GatewayFrontdoorArgs {
    #[arg(long, env = "ENVIRONMENT", default_value = "development")]
    environment: String,

    #[arg(long, env = "CORS_ORIGINS")]
    cors_origins: Option<String>,

    #[arg(long, env = "CORS_ALLOW_CREDENTIALS", default_value_t = true)]
    cors_allow_credentials: bool,
}

impl GatewayFrontdoorArgs {
    fn cors_config(&self) -> Option<FrontdoorCorsConfig> {
        FrontdoorCorsConfig::from_environment(
            self.environment.trim(),
            self.cors_origins.as_deref(),
            self.cors_allow_credentials,
        )
    }
}

#[derive(ClapArgs, Debug, Clone)]
struct GatewayRateLimitArgs {
    #[arg(long, env = "RPM_BUCKET_SECONDS", default_value_t = 60)]
    bucket_seconds: u64,

    #[arg(long, env = "RPM_KEY_TTL_SECONDS", default_value_t = 120)]
    key_ttl_seconds: u64,

    #[arg(long, env = "RATE_LIMIT_FAIL_OPEN", default_value_t = true)]
    fail_open: bool,
}

impl GatewayRateLimitArgs {
    fn config(&self) -> FrontdoorUserRpmConfig {
        FrontdoorUserRpmConfig::new(self.bucket_seconds, self.key_ttl_seconds, self.fail_open)
    }
}

#[derive(ClapArgs, Debug, Clone)]
struct GatewayLoggingArgs {
    #[arg(long, env = "AETHER_LOG_FORMAT", value_enum, default_value = "pretty")]
    log_format: GatewayLogFormatArg,

    #[arg(
        long,
        env = "AETHER_LOG_DESTINATION",
        value_enum,
        default_value = "stdout"
    )]
    log_destination: GatewayLogDestinationArg,

    #[arg(long, env = "AETHER_LOG_DIR")]
    log_dir: Option<String>,

    #[arg(long, env = "AETHER_LOG_ROTATION", value_enum, default_value = "daily")]
    log_rotation: GatewayLogRotationArg,

    #[arg(long, env = "AETHER_LOG_RETENTION_DAYS", default_value_t = 7)]
    log_retention_days: u64,

    #[arg(long, env = "AETHER_LOG_MAX_FILES", default_value_t = 30)]
    log_max_files: usize,
}

impl GatewayLoggingArgs {
    fn apply_to_runtime_config(
        &self,
        mut config: ServiceRuntimeConfig,
    ) -> Result<ServiceRuntimeConfig, std::io::Error> {
        config = config
            .with_log_format(self.log_format.into())
            .with_log_destination(self.log_destination.into());
        if matches!(
            self.log_destination,
            GatewayLogDestinationArg::File | GatewayLogDestinationArg::Both
        ) {
            let log_dir = self
                .log_dir
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "AETHER_LOG_DIR is required when AETHER_LOG_DESTINATION=file|both",
                    )
                })?;
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

#[derive(Parser, Debug)]
#[command(
    name = "aether-gateway",
    about = "Phase 3a Rust ingress gateway for Aether"
)]
struct Args {
    #[arg(long, env = "AETHER_GATEWAY_BIND", default_value = "0.0.0.0:8084")]
    bind: String,

    /// 容器内健康检查入口：根据当前 bind 端口探测本地 /health。
    #[arg(long, hide = true, default_value_t = false)]
    healthcheck: bool,

    #[arg(
        long,
        hide = true,
        env = "AETHER_GATEWAY_HEALTHCHECK_TIMEOUT_MS",
        default_value_t = 3_000
    )]
    healthcheck_timeout_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY",
        value_enum,
        default_value = "single-node"
    )]
    deployment_topology: DeploymentTopologyArg,

    #[arg(
        long,
        env = "AETHER_GATEWAY_NODE_ROLE",
        value_enum,
        default_value = "all"
    )]
    node_role: NodeRoleArg,

    /// Path to frontend static files directory (SPA). When set, the gateway
    /// serves the frontend directly without nginx.
    #[arg(long, env = "AETHER_GATEWAY_STATIC_DIR")]
    static_dir: Option<String>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE",
        value_enum,
        default_value = "python-sync-report"
    )]
    video_task_truth_source_mode: VideoTaskTruthSourceArg,

    #[arg(
        long,
        env = "AETHER_GATEWAY_VIDEO_TASK_POLLER_INTERVAL_MS",
        default_value_t = 5000
    )]
    video_task_poller_interval_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_VIDEO_TASK_POLLER_BATCH_SIZE",
        default_value_t = 32
    )]
    video_task_poller_batch_size: usize,

    #[arg(long, env = "AETHER_GATEWAY_VIDEO_TASK_STORE_PATH")]
    video_task_store_path: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_MAX_IN_FLIGHT_REQUESTS")]
    max_in_flight_requests: Option<usize>,

    #[arg(long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_LIMIT")]
    distributed_request_limit: Option<usize>,

    #[arg(long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_URL")]
    distributed_request_redis_url: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_KEY_PREFIX")]
    distributed_request_redis_key_prefix: Option<String>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_LEASE_TTL_MS",
        default_value_t = 30_000
    )]
    distributed_request_lease_ttl_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_RENEW_INTERVAL_MS",
        default_value_t = 10_000
    )]
    distributed_request_renew_interval_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_COMMAND_TIMEOUT_MS",
        default_value_t = 1_000
    )]
    distributed_request_command_timeout_ms: u64,

    #[command(flatten)]
    data: GatewayDataArgs,

    #[command(flatten)]
    usage: GatewayUsageArgs,

    #[command(flatten)]
    frontdoor: GatewayFrontdoorArgs,

    #[command(flatten)]
    rate_limit: GatewayRateLimitArgs,

    #[command(flatten)]
    logging: GatewayLoggingArgs,
}

impl Args {
    fn runtime_config(&self) -> Result<ServiceRuntimeConfig, std::io::Error> {
        let config = self
            .logging
            .apply_to_runtime_config(ServiceRuntimeConfig::new(
                "aether-gateway",
                "aether_gateway=info",
            ))?;
        Ok(config
            .with_node_role(self.node_role.as_str())
            .with_instance_id(resolve_gateway_log_instance_id()))
    }
}

fn resolve_gateway_log_instance_id() -> String {
    env_var_trimmed("AETHER_GATEWAY_INSTANCE_ID")
        .or_else(|| env_var_trimmed("HOSTNAME"))
        .unwrap_or_else(|| "local".to_string())
}

fn resolve_bind_http_base_url(bind: &str) -> Result<String, std::io::Error> {
    let trimmed = bind.trim();
    if trimmed.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_GATEWAY_BIND cannot be empty",
        ));
    }

    if let Ok(socket_addr) = trimmed.parse::<std::net::SocketAddr>() {
        let host = match socket_addr.ip() {
            std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
            std::net::IpAddr::V4(ip) => ip.to_string(),
            std::net::IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        };
        return Ok(format!("http://{host}:{}", socket_addr.port()));
    }

    let (host, port) = trimmed.rsplit_once(':').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("AETHER_GATEWAY_BIND must include a port: {trimmed}"),
        )
    })?;
    let port = port.parse::<u16>().map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid bind port in AETHER_GATEWAY_BIND={trimmed}: {error}"),
        )
    })?;

    let host = host.trim();
    if host.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid host in AETHER_GATEWAY_BIND={trimmed}"),
        ));
    }
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    Ok(format!("http://{host}:{port}"))
}

fn resolve_healthcheck_url(bind: &str) -> Result<String, std::io::Error> {
    Ok(format!("{}/health", resolve_bind_http_base_url(bind)?))
}

async fn run_healthcheck(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let url = resolve_healthcheck_url(&args.bind)?;
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(
            args.healthcheck_timeout_ms.max(1),
        ))
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

fn validate_deployment_topology(
    args: &Args,
    data_postgres_url: Option<&str>,
    data_redis_url: Option<&str>,
) -> Result<(), std::io::Error> {
    if matches!(args.deployment_topology, DeploymentTopologyArg::SingleNode) {
        if data_postgres_url.is_none() && data_redis_url.is_none() {
            warn!(
                "single-node deployment is starting without Postgres or Redis; local-only mode is allowed, but admin/auth/billing persistence will be limited"
            );
        }
        return Ok(());
    }

    if matches!(args.node_role, NodeRoleArg::All) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_GATEWAY_NODE_ROLE=all is only valid for single-node deployment; use frontdoor or background when AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY=multi-node",
        ));
    }

    let mut missing = Vec::new();
    if data_postgres_url.is_none() {
        missing.push("DATABASE_URL or AETHER_GATEWAY_DATA_POSTGRES_URL");
    }
    if data_redis_url.is_none() {
        missing.push("REDIS_URL or AETHER_GATEWAY_DATA_REDIS_URL");
    }

    if !missing.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "multi-node deployment requires shared data backends; missing {}",
                missing.join(", ")
            ),
        ));
    }

    if args
        .video_task_store_path
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_GATEWAY_VIDEO_TASK_STORE_PATH must be unset when AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY=multi-node; use shared Postgres-backed state instead",
        ));
    }

    if env_var_trimmed("AETHER_GATEWAY_INSTANCE_ID").is_none() {
        warn!(
            "multi-node deployment started without AETHER_GATEWAY_INSTANCE_ID; this is acceptable for stateless frontdoor replicas, but tunnel owner routing should set an explicit per-node instance id"
        );
    }
    if env_var_trimmed("AETHER_TUNNEL_RELAY_BASE_URL").is_none() {
        warn!(
            "multi-node deployment started without AETHER_TUNNEL_RELAY_BASE_URL; frontdoor replicas are fine, but proxy tunnel owner relay cannot forward across nodes until a per-node reachable base URL is configured"
        );
    }
    if !matches!(
        args.video_task_truth_source_mode,
        VideoTaskTruthSourceArg::RustAuthoritative
    ) {
        warn!(
            "multi-node deployment is still using python-sync-report video task truth source; keep rust-authoritative as the long-term cluster baseline"
        );
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.healthcheck {
        return run_healthcheck(&args).await;
    }
    init_service_runtime(args.runtime_config()?)?;
    let data_postgres_url = args.data.effective_postgres_url();
    let data_redis_url = args.data.effective_redis_url();
    validate_deployment_topology(
        &args,
        data_postgres_url.as_deref(),
        data_redis_url.as_deref(),
    )?;
    let data_config = args.data.to_config();
    let rate_limit_config = if matches!(args.deployment_topology, DeploymentTopologyArg::MultiNode)
    {
        args.rate_limit.config().with_local_fallback(false)
    } else {
        args.rate_limit.config()
    };
    if args.data.configured_encryption_key_mismatch() {
        warn!(
            "AETHER_GATEWAY_DATA_ENCRYPTION_KEY differs from ENCRYPTION_KEY; aether-gateway will prefer the gateway-specific value"
        );
    }
    info!(
        event_name = "gateway_starting",
        log_type = "ops",
        bind = %args.bind,
        environment = %args.frontdoor.environment,
        deployment_topology = args.deployment_topology.as_str(),
        node_role = args.node_role.as_str(),
        frontdoor_mode = "compatibility_frontdoor",
        log_format = ?args.logging.log_format,
        log_destination = args.logging.log_destination.as_str(),
        video_task_truth_source_mode = ?args.video_task_truth_source_mode,
        "aether-gateway starting"
    );
    debug!(
        event_name = "gateway_startup_config",
        log_type = "ops",
        log_dir = args.logging.log_dir.as_deref().unwrap_or("-"),
        log_rotation = args.logging.log_rotation.as_str(),
        log_retention_days = args.logging.log_retention_days,
        log_max_files = args.logging.log_max_files,
        static_dir = args.static_dir.as_deref().unwrap_or("-"),
        cors_origins = args.frontdoor.cors_origins.as_deref().unwrap_or("-"),
        cors_allow_credentials = args.frontdoor.cors_allow_credentials,
        frontdoor_rpm_bucket_seconds = args.rate_limit.bucket_seconds,
        frontdoor_rpm_key_ttl_seconds = args.rate_limit.key_ttl_seconds,
        frontdoor_rpm_fail_open = args.rate_limit.fail_open,
        frontdoor_rpm_allow_local_fallback = rate_limit_config.allow_local_fallback(),
        video_task_poller_interval_ms = args.video_task_poller_interval_ms,
        video_task_poller_batch_size = args.video_task_poller_batch_size,
        video_task_store_path = args.video_task_store_path.as_deref().unwrap_or("-"),
        max_in_flight_requests = args.max_in_flight_requests.unwrap_or_default(),
        distributed_request_limit = args.distributed_request_limit.unwrap_or_default(),
        distributed_request_redis_configured = args
            .distributed_request_redis_url
            .as_deref()
            .or(data_redis_url.as_deref())
            .is_some(),
        data_postgres_configured = data_postgres_url.is_some(),
        data_redis_configured = data_redis_url.is_some(),
        data_has_encryption_key = data_config.encryption_key().is_some(),
        data_postgres_require_ssl = args.data.postgres_require_ssl,
        "aether-gateway startup configuration"
    );

    let mut state = AppState::new()?
        .with_data_config(data_config)?
        .with_usage_runtime_config(args.usage.to_config())?
        .with_video_task_truth_source_mode(args.video_task_truth_source_mode.into());
    if let Some(cors_config) = args.frontdoor.cors_config() {
        state = state.with_frontdoor_cors_config(cors_config);
    }
    state = state.with_frontdoor_user_rpm_config(rate_limit_config);
    if matches!(
        args.video_task_truth_source_mode,
        VideoTaskTruthSourceArg::RustAuthoritative
    ) {
        state = state.with_video_task_poller_config(
            std::time::Duration::from_millis(args.video_task_poller_interval_ms.max(1)),
            args.video_task_poller_batch_size.max(1),
        );
    }
    if let Some(path) = args
        .video_task_store_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        state = state.with_video_task_store_path(path)?;
    }
    if let Some(limit) = args.max_in_flight_requests.filter(|limit| *limit > 0) {
        state = state.with_request_concurrency_limit(limit);
    }
    if let Some(limit) = args.distributed_request_limit.filter(|limit| *limit > 0) {
        let redis_url = args
            .distributed_request_redis_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(data_redis_url.as_deref())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "AETHER_GATEWAY_DISTRIBUTED_REQUEST_REDIS_URL or REDIS_URL/AETHER_GATEWAY_DATA_REDIS_URL is required when distributed request limit is enabled",
                )
            })?;
        state =
            state.with_distributed_request_concurrency_gate(DistributedConcurrencyGate::new_redis(
                "gateway_requests_distributed",
                limit,
                RedisDistributedConcurrencyConfig {
                    url: redis_url.to_string(),
                    key_prefix: args
                        .distributed_request_redis_key_prefix
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                    lease_ttl_ms: args.distributed_request_lease_ttl_ms.max(1),
                    renew_interval_ms: args.distributed_request_renew_interval_ms.max(1),
                    command_timeout_ms: Some(args.distributed_request_command_timeout_ms.max(1)),
                },
            )?);
    }
    if matches!(args.deployment_topology, DeploymentTopologyArg::MultiNode)
        && !state.has_usage_data_writer()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage persistence requires a configured Postgres data backend; set AETHER_GATEWAY_DATA_POSTGRES_URL before starting aether-gateway",
        )
        .into());
    }
    if matches!(args.deployment_topology, DeploymentTopologyArg::SingleNode)
        && !state.has_usage_data_writer()
    {
        warn!(
            "usage persistence backend is not configured; single-node local-only mode will run without durable usage records"
        );
    }
    info!(
        has_data_backends = state.has_data_backends(),
        has_video_task_data_reader = state.has_video_task_data_reader(),
        has_usage_data_writer = state.has_usage_data_writer(),
        has_usage_worker_backend = state.has_usage_worker_backend(),
        control_api_configured = true,
        execution_runtime_configured = state.execution_runtime_configured(),
        "aether-gateway data layer configured"
    );
    // Run pending database migrations before serving traffic
    info!("running database migrations...");
    if state.run_postgres_migrations().await? {
        info!("database migrations complete");
    }

    let background_tasks = if args.node_role.spawns_background_tasks() {
        state.spawn_background_tasks()
    } else {
        info!(
            node_role = args.node_role.as_str(),
            "background workers disabled for this node role"
        );
        Vec::new()
    };
    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    let public_base_url = resolve_bind_http_base_url(&args.bind)
        .unwrap_or_else(|_| format!("http://{}", args.bind.trim()));
    let frontdoor_health_url = format!("{public_base_url}/_gateway/health");
    let api_router = build_router_with_state(state);

    // Compose the final router: API routes + optional static file serving + CF header stripping
    let router = if let Some(ref static_dir) = args.static_dir {
        use tower_http::compression::CompressionLayer;
        info!(static_dir = %static_dir, "serving frontend static files");

        attach_static_frontend(api_router, static_dir)
            .layer(CompressionLayer::new())
            .layer(axum::middleware::from_fn(
                aether_gateway::strip_cf_headers_middleware,
            ))
    } else {
        api_router.layer(axum::middleware::from_fn(
            aether_gateway::strip_cf_headers_middleware,
        ))
    };

    info!(
        event_name = "gateway_ready",
        log_type = "ops",
        bind = %args.bind,
        public_url = %public_base_url,
        healthcheck_url = %frontdoor_health_url,
        legacy_route_policy = "fail_closed",
        "aether-gateway ready"
    );

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    for handle in background_tasks {
        handle.abort();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::resolve_healthcheck_url;

    #[test]
    fn resolves_ipv4_healthcheck_url() {
        assert_eq!(
            resolve_healthcheck_url("0.0.0.0:80").unwrap(),
            "http://127.0.0.1:80/health"
        );
    }

    #[test]
    fn resolves_ipv6_healthcheck_url() {
        assert_eq!(
            resolve_healthcheck_url("[::]:8080").unwrap(),
            "http://[::1]:8080/health"
        );
    }

    #[test]
    fn preserves_explicit_ipv4_bind_for_healthcheck_url() {
        assert_eq!(
            resolve_healthcheck_url("172.18.0.2:9000").unwrap(),
            "http://172.18.0.2:9000/health"
        );
    }

    #[test]
    fn preserves_explicit_ipv6_bind_for_healthcheck_url() {
        assert_eq!(
            resolve_healthcheck_url("[2001:db8::2]:9000").unwrap(),
            "http://[2001:db8::2]:9000/health"
        );
    }

    #[test]
    fn preserves_hostname_healthcheck_url() {
        assert_eq!(
            resolve_healthcheck_url("gateway.internal:9000").unwrap(),
            "http://gateway.internal:9000/health"
        );
    }

    #[test]
    fn rejects_bind_without_port() {
        let error = resolve_healthcheck_url("not-a-socket").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
