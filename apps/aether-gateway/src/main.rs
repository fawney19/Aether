#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use tracing::{debug, info, warn};

use aether_crypto::warm_python_fernet_secret;
use aether_data::lifecycle::export::{
    copy_database_records, export_database_jsonl, import_database_jsonl, DataCopyOptions,
    ExportDomain,
};
use aether_data::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig, DEFAULT_SQLITE_DATABASE_URL};
use aether_gateway::{
    attach_static_frontend, build_router_with_state, set_gateway_frontdoor_app_port, AppState,
    FrontdoorCorsConfig, FrontdoorUserRpmConfig, GatewayDataConfig, UsageRuntimeConfig,
    VideoTaskTruthSourceMode,
};
use aether_runtime::{
    init_service_runtime, FileLoggingConfig, LogDestination, LogFormat, LogRotation,
    ServiceRuntimeConfig,
};
use aether_runtime_state::{
    RedisClientConfig, RuntimeSemaphoreConfig, RuntimeState, RuntimeStateBackendMode,
    RuntimeStateConfig,
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
enum DatabaseDriverArg {
    Sqlite,
    Mysql,
    Postgres,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum ExportDomainArg {
    Users,
    ApiKeys,
    Providers,
    ProviderKeys,
    Endpoints,
    Models,
    GlobalModels,
    SystemConfigs,
    Wallets,
    Usage,
    Billing,
}

impl From<ExportDomainArg> for ExportDomain {
    fn from(value: ExportDomainArg) -> Self {
        match value {
            ExportDomainArg::Users => ExportDomain::Users,
            ExportDomainArg::ApiKeys => ExportDomain::ApiKeys,
            ExportDomainArg::Providers => ExportDomain::Providers,
            ExportDomainArg::ProviderKeys => ExportDomain::ProviderKeys,
            ExportDomainArg::Endpoints => ExportDomain::Endpoints,
            ExportDomainArg::Models => ExportDomain::Models,
            ExportDomainArg::GlobalModels => ExportDomain::GlobalModels,
            ExportDomainArg::SystemConfigs => ExportDomain::SystemConfigs,
            ExportDomainArg::Wallets => ExportDomain::Wallets,
            ExportDomainArg::Usage => ExportDomain::Usage,
            ExportDomainArg::Billing => ExportDomain::Billing,
        }
    }
}

impl From<DatabaseDriverArg> for DatabaseDriver {
    fn from(value: DatabaseDriverArg) -> Self {
        match value {
            DatabaseDriverArg::Sqlite => DatabaseDriver::Sqlite,
            DatabaseDriverArg::Mysql => DatabaseDriver::Mysql,
            DatabaseDriverArg::Postgres => DatabaseDriver::Postgres,
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
enum RuntimeBackendArg {
    Auto,
    Redis,
    Memory,
}

impl RuntimeBackendArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Redis => "redis",
            Self::Memory => "memory",
        }
    }

    const fn to_runtime_state_backend(self) -> RuntimeStateBackendMode {
        match self {
            Self::Auto => RuntimeStateBackendMode::Auto,
            Self::Redis => RuntimeStateBackendMode::Redis,
            Self::Memory => RuntimeStateBackendMode::Memory,
        }
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

const GATEWAY_TOKIO_WORKER_STACK_SIZE_BYTES: usize = 8 * 1024 * 1024;

fn env_var_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(ClapArgs, Debug, Clone)]
struct GatewayDataArgs {
    #[arg(long, env = "AETHER_DATABASE_DRIVER")]
    database_driver: Option<DatabaseDriverArg>,

    #[arg(long, env = "AETHER_DATABASE_URL")]
    database_url: Option<String>,

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
        default_value_t = 4
    )]
    postgres_min_connections: u32,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS",
        default_value_t = 20
    )]
    postgres_max_connections: u32,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_ACQUIRE_TIMEOUT_MS",
        default_value_t = 10_000
    )]
    postgres_acquire_timeout_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_IDLE_TIMEOUT_MS",
        default_value_t = 30_000
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
    fn effective_database_driver(&self) -> Option<DatabaseDriver> {
        self.database_driver.map(Into::into).or_else(|| {
            self.database_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(DatabaseDriver::from_database_url)
        })
    }

    fn effective_database_url(&self) -> Option<String> {
        let configured_url = self
            .database_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        match (self.effective_database_driver(), configured_url) {
            (Some(DatabaseDriver::Sqlite), None) => Some(DEFAULT_SQLITE_DATABASE_URL.to_string()),
            (_, Some(url)) => Some(url),
            (None, None) => self.effective_postgres_url(),
            (Some(DatabaseDriver::Postgres), None) => self.effective_postgres_url(),
            (Some(DatabaseDriver::Mysql), None) => None,
        }
    }

    fn effective_sql_database_config(&self) -> Option<SqlDatabaseConfig> {
        let url = self.effective_database_url()?;
        let driver = self
            .effective_database_driver()
            .or_else(|| DatabaseDriver::from_database_url(&url))
            .unwrap_or(DatabaseDriver::Postgres);

        Some(SqlDatabaseConfig {
            driver,
            url,
            pool: SqlPoolConfig {
                min_connections: self.postgres_min_connections,
                max_connections: self.postgres_max_connections,
                acquire_timeout_ms: self.postgres_acquire_timeout_ms,
                idle_timeout_ms: self.postgres_idle_timeout_ms,
                max_lifetime_ms: self.postgres_max_lifetime_ms,
                statement_cache_capacity: self.postgres_statement_cache_capacity,
                require_ssl: driver != DatabaseDriver::Sqlite && self.postgres_require_ssl,
            },
        })
    }

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
        let database = self.effective_sql_database_config();

        let config = match database {
            Some(database) => GatewayDataConfig::from_database_config(database),
            None => GatewayDataConfig::disabled(),
        };

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
            queue_terminal_events: true,
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

#[derive(Subcommand, Debug, Clone)]
enum DataCommand {
    /// Export persistent SQL data to database-neutral JSONL.
    Export(DataExportArgs),
    /// Import database-neutral JSONL into the selected SQL database.
    Import(DataImportArgs),
    /// Copy persistent SQL data directly between two databases without a JSONL file.
    Copy(DataCopyArgs),
}

#[derive(ClapArgs, Debug, Clone)]
struct DataExportArgs {
    #[command(flatten)]
    data: GatewayDataArgs,

    #[arg(long)]
    output: PathBuf,

    #[arg(long, value_enum, value_delimiter = ',')]
    domains: Vec<ExportDomainArg>,
}

#[derive(ClapArgs, Debug, Clone)]
struct DataImportArgs {
    #[command(flatten)]
    data: GatewayDataArgs,

    #[arg(long)]
    input: PathBuf,
}

#[derive(ClapArgs, Debug, Clone)]
struct DataCopyArgs {
    #[arg(long, value_enum)]
    source_driver: DatabaseDriverArg,

    #[arg(long)]
    source_url: String,

    #[arg(long, value_enum)]
    target_driver: DatabaseDriverArg,

    #[arg(long)]
    target_url: String,

    #[arg(long, value_enum, value_delimiter = ',')]
    domains: Vec<ExportDomainArg>,

    #[arg(long)]
    omit_request_body_details: bool,
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
    #[command(subcommand)]
    command: Option<DataCommand>,

    #[arg(long, env = "APP_PORT", default_value_t = 8084)]
    app_port: u16,

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

    #[arg(long, default_value_t = false)]
    migrate: bool,

    #[arg(long, default_value_t = false)]
    apply_backfills: bool,

    #[arg(
        long,
        env = "AETHER_GATEWAY_AUTO_PREPARE_DATABASE",
        default_value_t = false
    )]
    auto_prepare_database: bool,

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

    #[arg(long, env = "AETHER_RUNTIME_BACKEND", value_enum)]
    runtime_backend: Option<RuntimeBackendArg>,

    #[arg(long, env = "AETHER_RUNTIME_REDIS_URL")]
    runtime_redis_url: Option<String>,

    #[arg(long, env = "AETHER_RUNTIME_REDIS_KEY_PREFIX")]
    runtime_redis_key_prefix: Option<String>,

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
    fn effective_runtime_backend(
        &self,
        database: Option<&SqlDatabaseConfig>,
        data_redis_url: Option<&str>,
    ) -> RuntimeBackendArg {
        if let Some(runtime_backend) = self.runtime_backend {
            if !matches!(runtime_backend, RuntimeBackendArg::Auto) {
                return runtime_backend;
            }
        }
        if matches!(self.deployment_topology, DeploymentTopologyArg::MultiNode) {
            return RuntimeBackendArg::Redis;
        }
        if database.is_some_and(|database| database.driver == DatabaseDriver::Sqlite) {
            return RuntimeBackendArg::Memory;
        }
        if data_redis_url.is_some() {
            RuntimeBackendArg::Redis
        } else {
            RuntimeBackendArg::Memory
        }
    }

    fn effective_runtime_redis_url(&self, data_redis_url: Option<&str>) -> Option<String> {
        self.runtime_redis_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| data_redis_url.map(ToOwned::to_owned))
    }

    fn effective_runtime_redis_key_prefix(&self) -> Option<String> {
        self.runtime_redis_key_prefix
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.data
                    .redis_key_prefix
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            })
    }

    fn runtime_state_config(
        &self,
        runtime_backend: RuntimeBackendArg,
        data_redis_url: Option<&str>,
    ) -> RuntimeStateConfig {
        let redis = self
            .effective_runtime_redis_url(data_redis_url)
            .map(|url| RedisClientConfig {
                url,
                key_prefix: self.effective_runtime_redis_key_prefix(),
            });
        RuntimeStateConfig {
            backend: runtime_backend.to_runtime_state_backend(),
            redis,
            ..RuntimeStateConfig::default()
        }
    }

    fn runtime_config(&self) -> Result<ServiceRuntimeConfig, std::io::Error> {
        let default_log_filter = if self.command.is_some()
            || self.migrate
            || self.apply_backfills
            || self.auto_prepare_database
        {
            "aether_gateway=info,aether_data=info"
        } else {
            "aether_gateway=info"
        };
        let config = self
            .logging
            .apply_to_runtime_config(ServiceRuntimeConfig::new(
                "aether-gateway",
                default_log_filter,
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

fn validate_app_port(app_port: u16) -> Result<u16, std::io::Error> {
    if app_port == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "APP_PORT must be between 1 and 65535",
        ));
    }
    Ok(app_port)
}

fn gateway_bind_addr(app_port: u16) -> Result<std::net::SocketAddr, std::io::Error> {
    Ok(std::net::SocketAddr::from((
        [0, 0, 0, 0],
        validate_app_port(app_port)?,
    )))
}

fn resolve_local_http_base_url(app_port: u16) -> Result<String, std::io::Error> {
    Ok(format!("http://127.0.0.1:{}", validate_app_port(app_port)?))
}

fn resolve_healthcheck_url(app_port: u16) -> Result<String, std::io::Error> {
    Ok(format!("{}/health", resolve_local_http_base_url(app_port)?))
}

async fn run_healthcheck(
    app_port: u16,
    healthcheck_timeout_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = resolve_healthcheck_url(app_port)?;
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(
            healthcheck_timeout_ms.max(1),
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
    database: Option<&SqlDatabaseConfig>,
    data_redis_url: Option<&str>,
    runtime_backend: RuntimeBackendArg,
) -> Result<(), std::io::Error> {
    if matches!(args.deployment_topology, DeploymentTopologyArg::SingleNode) {
        if database.is_none() && data_redis_url.is_none() {
            warn!(
                "single-node deployment is starting without SQL database or Redis; local-only mode is allowed, but admin/auth/billing persistence will be limited"
            );
        }
        if matches!(runtime_backend, RuntimeBackendArg::Redis) && data_redis_url.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "AETHER_RUNTIME_BACKEND=redis requires REDIS_URL or AETHER_GATEWAY_DATA_REDIS_URL",
            ));
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
    if database.is_none() {
        missing.push("AETHER_DATABASE_URL, DATABASE_URL, or AETHER_GATEWAY_DATA_POSTGRES_URL");
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

    if matches!(runtime_backend, RuntimeBackendArg::Memory) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_RUNTIME_BACKEND=memory is only valid for single-node deployment",
        ));
    }

    if database.is_some_and(|database| database.driver == DatabaseDriver::Sqlite) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_DATABASE_DRIVER=sqlite is only valid for single-node deployment",
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(GATEWAY_TOKIO_WORKER_STACK_SIZE_BYTES)
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if let Some(command) = args.command.as_ref() {
        init_service_runtime(args.runtime_config()?)?;
        return run_data_command(command).await;
    }
    if args.migrate {
        init_service_runtime(args.runtime_config()?)?;
        return run_explicit_migrations(&args).await;
    }
    if args.apply_backfills {
        init_service_runtime(args.runtime_config()?)?;
        return run_explicit_backfills(&args).await;
    }
    let app_port = validate_app_port(args.app_port)?;
    let bind_addr = gateway_bind_addr(app_port)?;
    set_gateway_frontdoor_app_port(app_port);
    if args.healthcheck {
        return run_healthcheck(app_port, args.healthcheck_timeout_ms).await;
    }
    init_service_runtime(args.runtime_config()?)?;
    let sql_database_config = args.data.effective_sql_database_config();
    let data_postgres_url = args.data.effective_postgres_url();
    let data_redis_url = args.data.effective_redis_url();
    let runtime_backend =
        args.effective_runtime_backend(sql_database_config.as_ref(), data_redis_url.as_deref());
    let runtime_redis_url = args.effective_runtime_redis_url(data_redis_url.as_deref());
    validate_deployment_topology(
        &args,
        sql_database_config.as_ref(),
        runtime_redis_url.as_deref(),
        runtime_backend,
    )?;
    let runtime_state = Arc::new(
        RuntimeState::from_config(
            args.runtime_state_config(runtime_backend, data_redis_url.as_deref()),
        )
        .await
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string()))?,
    );
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
        bind = %bind_addr,
        app_port,
        environment = %args.frontdoor.environment,
        deployment_topology = args.deployment_topology.as_str(),
        node_role = args.node_role.as_str(),
        runtime_backend = runtime_backend.as_str(),
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
            .or(runtime_redis_url.as_deref())
            .is_some(),
        data_database_configured = sql_database_config.is_some(),
        data_database_driver = sql_database_config
            .as_ref()
            .map(|database| database.driver.as_str())
            .unwrap_or("-"),
        data_postgres_configured = data_postgres_url.is_some(),
        runtime_redis_configured = matches!(runtime_backend, RuntimeBackendArg::Redis),
        data_redis_url_supplied = data_redis_url.is_some(),
        data_has_encryption_key = data_config.encryption_key().is_some(),
        data_postgres_require_ssl = args.data.postgres_require_ssl,
        "aether-gateway startup configuration"
    );

    let mut state = AppState::new()?
        .with_runtime_state(runtime_state)
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
        let distributed_gate = state
            .runtime_state()
            .semaphore(
                "gateway_requests_distributed",
                limit,
                RuntimeSemaphoreConfig {
                    lease_ttl_ms: args.distributed_request_lease_ttl_ms.max(1),
                    renew_interval_ms: args.distributed_request_renew_interval_ms.max(1),
                    command_timeout_ms: Some(args.distributed_request_command_timeout_ms.max(1)),
                },
            )
            .map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, err.to_string())
            })?;
        state = state.with_distributed_request_concurrency_gate(distributed_gate);
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
    prepare_database_startup_requirements(&state, args.auto_prepare_database).await?;
    let reset_stale_proxy_nodes = state.reset_stale_proxy_node_tunnel_statuses().await?;
    if reset_stale_proxy_nodes > 0 {
        info!(
            reset_stale_proxy_nodes,
            "reset stale tunnel-connected proxy nodes on startup"
        );
    }
    state.bootstrap_admin_from_env().await?;

    let background_tasks = if args.node_role.spawns_background_tasks() {
        Some(state.spawn_background_tasks())
    } else {
        info!(
            node_role = args.node_role.as_str(),
            "background workers disabled for this node role"
        );
        None
    };
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let public_base_url = resolve_local_http_base_url(app_port)?;
    let frontdoor_health_url = format!("{public_base_url}/_gateway/health");
    let api_router = build_router_with_state(state);

    // Compose the final router: API routes + optional static file serving.
    let router = if let Some(ref static_dir) = args.static_dir {
        use tower_http::compression::CompressionLayer;
        info!(static_dir = %static_dir, "serving frontend static files");

        attach_static_frontend(api_router, static_dir).layer(CompressionLayer::new())
    } else {
        api_router
    };

    info!(
        event_name = "gateway_ready",
        log_type = "ops",
        bind = %bind_addr,
        app_port,
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
    if let Some(background_tasks) = background_tasks {
        background_tasks.shutdown().await;
    }
    Ok(())
}

async fn run_data_command(command: &DataCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        DataCommand::Export(args) => run_data_export(args).await,
        DataCommand::Import(args) => run_data_import(args).await,
        DataCommand::Copy(args) => run_data_copy(args).await,
    }
}

fn required_sql_database_config(
    data: &GatewayDataArgs,
) -> Result<SqlDatabaseConfig, Box<dyn std::error::Error>> {
    data.effective_sql_database_config().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_DATABASE_DRIVER/AETHER_DATABASE_URL, AETHER_GATEWAY_DATA_POSTGRES_URL, or DATABASE_URL is required",
        )
        .into()
    })
}

fn requested_export_domains(args: &DataExportArgs) -> Vec<ExportDomain> {
    requested_domains(&args.domains)
}

fn requested_domains(domains: &[ExportDomainArg]) -> Vec<ExportDomain> {
    domains.iter().copied().map(Into::into).collect::<Vec<_>>()
}

fn current_unix_secs() -> Result<u64, std::time::SystemTimeError> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}

async fn run_data_export(args: &DataExportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let database = required_sql_database_config(&args.data)?;
    let driver = database.driver;
    let domains = requested_export_domains(args);
    let created_at_unix_secs = current_unix_secs()?;
    let encoded = export_database_jsonl(database, domains, created_at_unix_secs).await?;

    tokio::fs::write(&args.output, encoded.as_bytes()).await?;
    info!(
        driver = %driver,
        output = %args.output.display(),
        bytes = encoded.len(),
        "database export complete"
    );
    println!(
        "exported {} bytes from {} to {}",
        encoded.len(),
        driver,
        args.output.display()
    );
    Ok(())
}

async fn run_data_import(args: &DataImportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let database = required_sql_database_config(&args.data)?;
    let driver = database.driver;
    let input = tokio::fs::read_to_string(&args.input).await?;
    let imported = import_database_jsonl(database, &input).await?;

    info!(
        driver = %driver,
        input = %args.input.display(),
        imported,
        "database import complete"
    );
    println!(
        "imported {} records into {} from {}",
        imported,
        driver,
        args.input.display()
    );
    Ok(())
}

fn copy_database_config(
    driver: DatabaseDriverArg,
    url: &str,
    label: &str,
) -> Result<SqlDatabaseConfig, Box<dyn std::error::Error>> {
    let url = url.trim();
    if url.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{label} database URL must not be empty"),
        )
        .into());
    }
    let driver = DatabaseDriver::from(driver);
    Ok(SqlDatabaseConfig::new(
        driver,
        url,
        SqlPoolConfig {
            require_ssl: false,
            ..SqlPoolConfig::default()
        },
    )?)
}

async fn run_data_copy(args: &DataCopyArgs) -> Result<(), Box<dyn std::error::Error>> {
    let source = copy_database_config(args.source_driver, &args.source_url, "source")?;
    let target = copy_database_config(args.target_driver, &args.target_url, "target")?;
    let source_driver = source.driver;
    let target_driver = target.driver;
    let domains = requested_domains(&args.domains);
    let created_at_unix_secs = current_unix_secs()?;
    let imported = copy_database_records(
        source,
        target,
        domains,
        created_at_unix_secs,
        DataCopyOptions {
            omit_request_body_details: args.omit_request_body_details,
        },
    )
    .await?;

    info!(
        source_driver = %source_driver,
        target_driver = %target_driver,
        imported,
        "database copy complete"
    );
    println!(
        "copied {} records from {} to {} without a JSONL file",
        imported, source_driver, target_driver
    );
    Ok(())
}

async fn run_explicit_migrations(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.data.effective_sql_database_config().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_DATABASE_DRIVER/AETHER_DATABASE_URL, AETHER_GATEWAY_DATA_POSTGRES_URL, or DATABASE_URL is required when running --migrate",
        )
        .into());
    }

    if args.data.configured_encryption_key_mismatch() {
        warn!(
            "AETHER_GATEWAY_DATA_ENCRYPTION_KEY differs from ENCRYPTION_KEY; aether-gateway will prefer the gateway-specific value"
        );
    }

    let state = AppState::new()?.with_data_config(args.data.to_config())?;
    let pending = state
        .pending_database_migrations()
        .await?
        .unwrap_or_default();
    if pending.is_empty() {
        info!(
            pending_migrations = 0,
            "database migrations already up to date"
        );
        return Ok(());
    }

    let next = pending
        .first()
        .expect("pending migrations should have a first element");
    info!(
        pending_migrations = pending.len(),
        next_version = next.version,
        next_description = %next.description,
        pending_versions = %format_pending_migrations(&pending),
        "running database migrations by explicit request..."
    );
    if state.run_database_migrations().await? {
        info!("database migrations complete");
    }
    Ok(())
}

async fn run_explicit_backfills(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let database = args.data.effective_sql_database_config().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_DATABASE_DRIVER/AETHER_DATABASE_URL, AETHER_GATEWAY_DATA_POSTGRES_URL, or DATABASE_URL is required when running --apply-backfills",
        )
    })?;
    let state = AppState::new()?.with_data_config(args.data.to_config())?;
    ensure_database_schema_is_current(&state).await?;

    let pending = state
        .pending_database_backfills()
        .await?
        .unwrap_or_default();
    if pending.is_empty() {
        info!(
            driver = %database.driver,
            pending_backfills = 0,
            "database backfills already up to date"
        );
        return Ok(());
    }

    let next = pending
        .first()
        .expect("pending backfills should have a first element");
    info!(
        pending_backfills = pending.len(),
        next_version = next.version,
        next_description = %next.description,
        pending_versions = %format_pending_backfills(&pending),
        "running database backfills by explicit request..."
    );
    if state.run_database_backfills().await? {
        info!("database backfills complete");
    }
    Ok(())
}

async fn prepare_database_startup_requirements(
    state: &AppState,
    auto_prepare_database: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !auto_prepare_database {
        ensure_database_schema_is_current(state).await?;
        ensure_database_backfills_are_current(state).await?;
        return Ok(());
    }

    info!(
        "auto database preparation enabled; applying pending migrations and backfills before serving traffic"
    );

    let Some(pending_migrations) = state.prepare_database_for_startup().await? else {
        return Ok(());
    };
    if !pending_migrations.is_empty() {
        let next = pending_migrations
            .first()
            .expect("pending migrations should have a first element");
        info!(
            pending_migrations = pending_migrations.len(),
            next_version = next.version,
            next_description = %next.description,
            pending_versions = %format_pending_migrations(&pending_migrations),
            "running database migrations during service startup..."
        );
        if state.run_database_migrations().await? {
            info!("database migrations complete during service startup");
        }
    }

    let Some(pending_backfills) = state.pending_database_backfills().await? else {
        return Ok(());
    };
    if pending_backfills.is_empty() {
        return Ok(());
    }

    let next = pending_backfills
        .first()
        .expect("pending backfills should have a first element");
    info!(
        pending_backfills = pending_backfills.len(),
        next_version = next.version,
        next_description = %next.description,
        pending_versions = %format_pending_backfills(&pending_backfills),
        "running database backfills during service startup..."
    );
    if state.run_database_backfills().await? {
        info!("database backfills complete during service startup");
    }

    Ok(())
}

fn format_pending_migrations(
    pending: &[aether_data::lifecycle::migrate::PendingMigrationInfo],
) -> String {
    pending
        .iter()
        .map(|migration| format!("{} ({})", migration.version, migration.description))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_pending_backfills(
    pending: &[aether_data::lifecycle::backfill::PendingBackfillInfo],
) -> String {
    pending
        .iter()
        .map(|backfill| format!("{} ({})", backfill.version, backfill.description))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn ensure_database_backfills_are_current(
    state: &AppState,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(pending) = state.pending_database_backfills().await? else {
        return Ok(());
    };
    if pending.is_empty() {
        return Ok(());
    }

    let next = pending
        .first()
        .expect("pending backfills should have a first element");
    Err(pending_backfills_error(pending.len(), next.version, &next.description).into())
}

async fn ensure_database_schema_is_current(
    state: &AppState,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(pending) = state.prepare_database_for_startup().await? else {
        return Ok(());
    };
    if pending.is_empty() {
        return Ok(());
    }

    let next = pending
        .first()
        .expect("pending migrations should have a first element");
    Err(pending_schema_error(pending.len(), next.version, &next.description).into())
}

fn pending_schema_error(
    pending_count: usize,
    next_version: i64,
    next_description: &str,
) -> std::io::Error {
    std::io::Error::other(format!(
        "database schema is behind by {} migration(s); next pending migration is {} ({})\nrun `aether-gateway --migrate` before starting the service",
        pending_count,
        next_version,
        next_description
    ))
}

fn pending_backfills_error(
    pending_count: usize,
    next_version: i64,
    next_description: &str,
) -> std::io::Error {
    std::io::Error::other(format!(
        "database backfills are behind by {} backfill(s); next pending backfill is {} ({})\nrun `aether-gateway --apply-backfills` before starting the service",
        pending_count,
        next_version,
        next_description
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_database_backfills_are_current, ensure_database_schema_is_current,
        pending_backfills_error, pending_schema_error, resolve_healthcheck_url, Args,
        DatabaseDriverArg, DeploymentTopologyArg, GatewayDataArgs, GatewayFrontdoorArgs,
        GatewayLogDestinationArg, GatewayLogFormatArg, GatewayLogRotationArg, GatewayLoggingArgs,
        GatewayRateLimitArgs, GatewayUsageArgs, NodeRoleArg, RuntimeBackendArg,
        VideoTaskTruthSourceArg,
    };
    use aether_data::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};
    use aether_gateway::AppState;

    fn test_args() -> Args {
        Args {
            command: None,
            app_port: 8084,
            healthcheck: false,
            healthcheck_timeout_ms: 3_000,
            deployment_topology: DeploymentTopologyArg::SingleNode,
            node_role: NodeRoleArg::All,
            migrate: false,
            apply_backfills: false,
            auto_prepare_database: false,
            static_dir: None,
            video_task_truth_source_mode: VideoTaskTruthSourceArg::PythonSyncReport,
            video_task_poller_interval_ms: 5_000,
            video_task_poller_batch_size: 32,
            video_task_store_path: None,
            max_in_flight_requests: None,
            distributed_request_limit: None,
            distributed_request_redis_url: None,
            distributed_request_redis_key_prefix: None,
            distributed_request_lease_ttl_ms: 30_000,
            distributed_request_renew_interval_ms: 10_000,
            distributed_request_command_timeout_ms: 1_000,
            runtime_backend: None,
            runtime_redis_url: None,
            runtime_redis_key_prefix: None,
            data: GatewayDataArgs {
                database_driver: None,
                database_url: None,
                postgres_url: None,
                encryption_key: None,
                redis_url: None,
                redis_key_prefix: None,
                postgres_min_connections: 4,
                postgres_max_connections: 20,
                postgres_acquire_timeout_ms: 10_000,
                postgres_idle_timeout_ms: 30_000,
                postgres_max_lifetime_ms: 1_800_000,
                postgres_statement_cache_capacity: 100,
                postgres_require_ssl: false,
            },
            usage: GatewayUsageArgs {
                queue_stream_key: "usage:events".to_string(),
                queue_group: "usage_consumers".to_string(),
                queue_dlq_stream_key: "usage:events:dlq".to_string(),
                queue_stream_maxlen: 2_000,
                queue_batch_size: 200,
                queue_block_ms: 500,
                queue_reclaim_idle_ms: 30_000,
                queue_reclaim_count: 200,
                queue_reclaim_interval_ms: 5_000,
            },
            frontdoor: GatewayFrontdoorArgs {
                environment: "development".to_string(),
                cors_origins: None,
                cors_allow_credentials: true,
            },
            rate_limit: GatewayRateLimitArgs {
                bucket_seconds: 60,
                key_ttl_seconds: 120,
                fail_open: true,
            },
            logging: GatewayLoggingArgs {
                log_format: GatewayLogFormatArg::Pretty,
                log_destination: GatewayLogDestinationArg::Stdout,
                log_dir: None,
                log_rotation: GatewayLogRotationArg::Daily,
                log_retention_days: 7,
                log_max_files: 30,
            },
        }
    }

    #[test]
    fn resolves_healthcheck_url_from_app_port() {
        assert_eq!(
            resolve_healthcheck_url(8084).unwrap(),
            "http://127.0.0.1:8084/health"
        );
    }

    #[test]
    fn rejects_zero_app_port() {
        let error = resolve_healthcheck_url(0).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn explicit_migrate_runtime_config_enables_data_logs() {
        let mut args = test_args();
        args.migrate = true;
        let config = args.runtime_config().expect("runtime config should build");
        assert_eq!(
            config.default_log_filter,
            "aether_gateway=info,aether_data=info"
        );
    }

    #[test]
    fn normal_runtime_config_keeps_gateway_only_logs() {
        let config = test_args()
            .runtime_config()
            .expect("runtime config should build");
        assert_eq!(config.default_log_filter, "aether_gateway=info");
    }

    #[test]
    fn apply_backfills_runtime_config_enables_data_logs() {
        let mut args = test_args();
        args.apply_backfills = true;
        let config = args.runtime_config().expect("runtime config should build");
        assert_eq!(
            config.default_log_filter,
            "aether_gateway=info,aether_data=info"
        );
    }

    #[test]
    fn auto_prepare_database_runtime_config_enables_data_logs() {
        let mut args = test_args();
        args.auto_prepare_database = true;
        let config = args.runtime_config().expect("runtime config should build");
        assert_eq!(
            config.default_log_filter,
            "aether_gateway=info,aether_data=info"
        );
    }

    #[test]
    fn sqlite_database_defaults_to_memory_runtime_backend() {
        let args = test_args();
        let database = SqlDatabaseConfig::new(
            DatabaseDriver::Sqlite,
            "sqlite://./data/aether.db".to_string(),
            SqlPoolConfig::default(),
        )
        .expect("sqlite config should build");

        assert_eq!(
            args.effective_runtime_backend(Some(&database), Some("redis://127.0.0.1/0")),
            RuntimeBackendArg::Memory
        );
    }

    #[test]
    fn memory_runtime_data_config_keeps_redis_out_of_data_layer() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Sqlite);
        args.data.database_url = Some("sqlite://./data/aether.db".to_string());
        args.data.redis_url = Some("redis://127.0.0.1/0".to_string());

        let config = args.data.to_config();

        assert_eq!(
            config
                .database()
                .expect("database should be configured")
                .driver,
            DatabaseDriver::Sqlite
        );
    }

    #[test]
    fn redis_runtime_config_owns_redis_connection() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Postgres);
        args.data.database_url = Some("postgres://postgres:postgres@localhost/aether".to_string());
        args.data.redis_url = Some("redis://127.0.0.1/0".to_string());

        let config = args.runtime_state_config(
            RuntimeBackendArg::Redis,
            args.data.effective_redis_url().as_deref(),
        );

        assert_eq!(
            config
                .redis
                .as_ref()
                .expect("redis should be configured for runtime state")
                .url,
            "redis://127.0.0.1/0"
        );
    }

    #[test]
    fn redis_url_defaults_to_redis_runtime_backend_for_server_database() {
        let args = test_args();
        let database = SqlDatabaseConfig::new(
            DatabaseDriver::Postgres,
            "postgres://postgres:postgres@localhost/aether".to_string(),
            SqlPoolConfig::default(),
        )
        .expect("postgres config should build");

        assert_eq!(
            args.effective_runtime_backend(Some(&database), Some("redis://127.0.0.1/0")),
            RuntimeBackendArg::Redis
        );
    }

    #[test]
    fn mysql_database_with_redis_defaults_to_redis_runtime_backend() {
        let args = test_args();
        let database = SqlDatabaseConfig::new(
            DatabaseDriver::Mysql,
            "mysql://aether:aether@localhost:3306/aether".to_string(),
            SqlPoolConfig::default(),
        )
        .expect("mysql config should build");

        assert_eq!(
            args.effective_runtime_backend(Some(&database), Some("redis://127.0.0.1/0")),
            RuntimeBackendArg::Redis
        );
    }

    #[test]
    fn sqlite_database_allows_explicit_redis_runtime_backend_when_redis_is_configured() {
        let mut args = test_args();
        args.runtime_backend = Some(RuntimeBackendArg::Redis);
        let database = SqlDatabaseConfig::new(
            DatabaseDriver::Sqlite,
            "sqlite://./data/aether.db".to_string(),
            SqlPoolConfig::default(),
        )
        .expect("sqlite config should build");

        assert_eq!(
            args.effective_runtime_backend(Some(&database), Some("redis://127.0.0.1/0")),
            RuntimeBackendArg::Redis
        );
        super::validate_deployment_topology(
            &args,
            Some(&database),
            Some("redis://127.0.0.1/0"),
            RuntimeBackendArg::Redis,
        )
        .expect("single-node sqlite should allow explicit redis runtime");
    }

    #[test]
    fn single_node_sqlite_without_redis_allows_memory_runtime_backend() {
        let args = test_args();
        let database = SqlDatabaseConfig::new(
            DatabaseDriver::Sqlite,
            "sqlite://./data/aether.db".to_string(),
            SqlPoolConfig::default(),
        )
        .expect("sqlite config should build");

        super::validate_deployment_topology(
            &args,
            Some(&database),
            None,
            RuntimeBackendArg::Memory,
        )
        .expect("single-node sqlite memory runtime should be accepted");
    }

    #[test]
    fn multi_node_rejects_memory_runtime_backend() {
        let mut args = test_args();
        args.deployment_topology = DeploymentTopologyArg::MultiNode;
        args.node_role = NodeRoleArg::Frontdoor;
        let database = SqlDatabaseConfig::new(
            DatabaseDriver::Postgres,
            "postgres://postgres:postgres@localhost/aether".to_string(),
            SqlPoolConfig::default(),
        )
        .expect("postgres config should build");

        let error = super::validate_deployment_topology(
            &args,
            Some(&database),
            Some("redis://127.0.0.1/0"),
            RuntimeBackendArg::Memory,
        )
        .expect_err("multi-node memory runtime should be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("AETHER_RUNTIME_BACKEND=memory"));
    }

    #[test]
    fn multi_node_rejects_missing_redis_runtime_backend() {
        let mut args = test_args();
        args.deployment_topology = DeploymentTopologyArg::MultiNode;
        args.node_role = NodeRoleArg::Frontdoor;
        let database = SqlDatabaseConfig::new(
            DatabaseDriver::Postgres,
            "postgres://postgres:postgres@localhost/aether".to_string(),
            SqlPoolConfig::default(),
        )
        .expect("postgres config should build");

        let error = super::validate_deployment_topology(
            &args,
            Some(&database),
            None,
            RuntimeBackendArg::Redis,
        )
        .expect_err("multi-node should require redis");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("REDIS_URL"));
    }

    #[test]
    fn multi_node_rejects_sqlite_database_backend() {
        let mut args = test_args();
        args.deployment_topology = DeploymentTopologyArg::MultiNode;
        args.node_role = NodeRoleArg::Frontdoor;
        let database = SqlDatabaseConfig::new(
            DatabaseDriver::Sqlite,
            "sqlite://./data/aether.db".to_string(),
            SqlPoolConfig::default(),
        )
        .expect("sqlite config should build");

        let error = super::validate_deployment_topology(
            &args,
            Some(&database),
            Some("redis://127.0.0.1/0"),
            RuntimeBackendArg::Redis,
        )
        .expect_err("multi-node sqlite should be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("AETHER_DATABASE_DRIVER=sqlite"));
    }

    #[test]
    fn pending_schema_error_mentions_explicit_migrate_command() {
        let error = pending_schema_error(2, 20260413020000, "squash usage schema split");
        let message = error.to_string();
        assert!(message.contains("database schema is behind by 2 migration(s)"));
        assert!(message.contains("20260413020000"));
        assert!(message.contains("squash usage schema split"));
        assert!(message.contains("aether-gateway --migrate"));
    }

    #[test]
    fn pending_backfills_error_mentions_explicit_apply_backfills_command() {
        let message = pending_backfills_error(
            1,
            20260422110000,
            "backfill stats aggregate read path support",
        )
        .to_string();
        assert!(message.contains("database backfills are behind by 1 backfill(s)"));
        assert!(message.contains("20260422110000"));
        assert!(message.contains("backfill stats aggregate read path support"));
        assert!(message.contains("aether-gateway --apply-backfills"));
        assert!(message.contains("before starting the service"));
    }

    #[tokio::test]
    async fn ensure_database_schema_is_current_is_noop_without_database_pool() {
        let state = AppState::new().expect("state should build");
        ensure_database_schema_is_current(&state)
            .await
            .expect("disabled data backend should not block startup");
    }

    #[tokio::test]
    async fn ensure_database_backfills_are_current_is_noop_without_database_pool() {
        let state = AppState::new().expect("state should build");
        ensure_database_backfills_are_current(&state)
            .await
            .expect("disabled data backend should not block startup");
    }

    #[tokio::test]
    async fn auto_prepare_database_is_noop_without_database_pool() {
        let state = AppState::new().expect("state should build");
        super::prepare_database_startup_requirements(&state, true)
            .await
            .expect("disabled data backend should not block startup");
    }

    #[tokio::test]
    async fn explicit_migrate_requires_database_url() {
        let args = test_args();
        let error = super::run_explicit_migrations(&args)
            .await
            .expect_err("missing database URL should fail");
        let message = error.to_string();
        assert!(message.contains("AETHER_DATABASE_DRIVER/AETHER_DATABASE_URL"));
        assert!(message.contains("--migrate"));
    }

    #[tokio::test]
    async fn explicit_migrate_does_not_depend_on_app_port_validation() {
        let mut args = test_args();
        args.app_port = 0;

        let error = super::run_explicit_migrations(&args)
            .await
            .expect_err("missing database URL should fail before any app port validation");
        let message = error.to_string();
        assert!(message.contains("AETHER_DATABASE_DRIVER/AETHER_DATABASE_URL"));
        assert!(!message.contains("APP_PORT"));
    }

    #[tokio::test]
    async fn explicit_backfills_require_database_url() {
        let args = test_args();
        let error = super::run_explicit_backfills(&args)
            .await
            .expect_err("missing database URL should fail");
        let message = error.to_string();
        assert!(message.contains("AETHER_DATABASE_DRIVER/AETHER_DATABASE_URL"));
        assert!(message.contains("--apply-backfills"));
    }

    #[tokio::test]
    async fn explicit_backfills_are_noop_for_sqlite_database() {
        let mut args = test_args();
        let database_path = std::env::temp_dir().join(format!(
            "aether-sqlite-backfill-noop-{}-{}.db",
            std::process::id(),
            crate::current_unix_secs().expect("clock should be available")
        ));
        args.data.database_driver = Some(DatabaseDriverArg::Sqlite);
        args.data.database_url = Some(format!("sqlite://{}", database_path.display()));

        super::run_explicit_migrations(&args)
            .await
            .expect("sqlite migrations should run before backfills");
        super::run_explicit_backfills(&args)
            .await
            .expect("sqlite backfills should be an explicit no-op");
        let _ = std::fs::remove_file(database_path);
    }
}
