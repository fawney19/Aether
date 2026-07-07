#[cfg(all(not(target_env = "msvc"), feature = "jemalloc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::path::PathBuf;
use std::sync::Arc;

use axum::{body::Body, extract::Request};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder as HyperServerBuilder,
    service::TowerToHyperService,
};
use tower::{Service as _, ServiceExt as _};
use tracing::{debug, info, warn};

use aether_crypto::warm_python_fernet_secret;
use aether_data::lifecycle::export::{
    copy_database_records, export_database_jsonl, import_database_jsonl, DataCopyOptions,
    ExportDomain,
};
use aether_data::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig, DEFAULT_SQLITE_DATABASE_URL};
use aether_gateway::{
    attach_static_frontend, build_router_with_state,
    prewarm_direct_h2c_sender_cache_from_env_for_startup, set_gateway_frontdoor_app_port, AppState,
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
const DEFAULT_SQL_POOL_ACQUIRE_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_SQL_POOL_IDLE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_SQL_POOL_MAX_LIFETIME_MS: u64 = 30 * 60_000;
const DEFAULT_SQL_POOL_STATEMENT_CACHE_CAPACITY: usize = 100;
const DEFAULT_SQLITE_POOL_MAX_CONNECTIONS: u32 = 1;
// Per-process default for server SQL backends. Keep this below common
// database server max_connections defaults; operators can override with
// AETHER_GATEWAY_DATA_POSTGRES_{MIN,MAX}_CONNECTIONS after sizing the DB.
const AUTO_SERVER_SQL_POOL_CONNECTIONS_PER_CPU: u32 = 4;
const AUTO_SERVER_SQL_POOL_MIN_CONNECTIONS_FLOOR: u32 = 4;
const AUTO_SERVER_SQL_POOL_MIN_CONNECTIONS_CAP: u32 = 16;
const AUTO_SERVER_SQL_POOL_MAX_CONNECTIONS_FLOOR: u32 = 20;
const AUTO_SERVER_SQL_POOL_MAX_CONNECTIONS_CAP: u32 = 100;
const DEFAULT_USAGE_QUEUE_WORKERS_CAP: usize = 8;
const AUTO_USAGE_QUEUE_WORKERS_MIN: usize = 2;
const AUTO_USAGE_QUEUE_WORKERS_REQUESTS_PER_WORKER: usize = 128;
const AUTO_USAGE_QUEUE_WORKERS_DB_SHARE_ALL: usize = 4;
const AUTO_USAGE_QUEUE_WORKERS_DB_SHARE_BACKGROUND: usize = 2;
const AUTO_USAGE_WORKER_RECORD_DB_SHARE_ALL: usize = 8;
const AUTO_USAGE_WORKER_RECORD_DB_SHARE_BACKGROUND: usize = 4;
const MAX_USAGE_QUEUE_WORKERS: usize = 64;
const DEFAULT_GATEWAY_LISTEN_BACKLOG: i32 = 65_535;
const MIN_GATEWAY_LISTEN_BACKLOG: i32 = 128;
const MAX_GATEWAY_LISTEN_BACKLOG: i32 = 65_535;
const DEFAULT_GATEWAY_LISTENER_SHARDS: usize = 0;
const MAX_GATEWAY_LISTENER_SHARDS: usize = 64;
const DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS: u32 = 16_384;
const MIN_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS: u32 = 200;
const MAX_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS: u32 = 1_000_000;
fn env_var_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn available_parallelism_u32() -> u32 {
    u32::try_from(available_parallelism_usize())
        .unwrap_or(u32::MAX)
        .max(1)
}

fn available_parallelism_usize() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(AUTO_SERVER_SQL_POOL_MIN_CONNECTIONS_FLOOR as usize)
        .max(1)
}

fn usage_queue_request_concurrency_hint(
    max_in_flight_requests: Option<usize>,
    distributed_request_limit: Option<usize>,
) -> Option<usize> {
    match (
        max_in_flight_requests.filter(|limit| *limit > 0),
        distributed_request_limit.filter(|limit| *limit > 0),
    ) {
        (Some(local), Some(distributed)) => Some(local.min(distributed)),
        (Some(local), None) => Some(local),
        (None, Some(distributed)) => Some(distributed),
        (None, None) => None,
    }
}

fn usage_queue_workers_for_request_concurrency(request_concurrency: usize) -> usize {
    let workers = request_concurrency
        .saturating_add(AUTO_USAGE_QUEUE_WORKERS_REQUESTS_PER_WORKER - 1)
        / AUTO_USAGE_QUEUE_WORKERS_REQUESTS_PER_WORKER;
    workers.clamp(AUTO_USAGE_QUEUE_WORKERS_MIN, MAX_USAGE_QUEUE_WORKERS)
}

fn usage_queue_worker_database_cap(
    node_role: NodeRoleArg,
    database: Option<&SqlDatabaseConfig>,
) -> usize {
    let Some(database) = database else {
        return MAX_USAGE_QUEUE_WORKERS;
    };
    if database.driver == DatabaseDriver::Sqlite {
        return 1;
    }

    let divisor = if matches!(node_role, NodeRoleArg::Background) {
        AUTO_USAGE_QUEUE_WORKERS_DB_SHARE_BACKGROUND
    } else {
        AUTO_USAGE_QUEUE_WORKERS_DB_SHARE_ALL
    };
    let max_connections = database.pool.max_connections.max(1) as usize;
    max_connections
        .saturating_add(divisor - 1)
        .checked_div(divisor)
        .unwrap_or(1)
        .clamp(1, MAX_USAGE_QUEUE_WORKERS)
}

fn usage_worker_record_concurrency_database_cap(
    node_role: NodeRoleArg,
    database: Option<&SqlDatabaseConfig>,
) -> Option<usize> {
    let database = database?;
    if database.driver == DatabaseDriver::Sqlite {
        return Some(1);
    }

    let divisor = if matches!(node_role, NodeRoleArg::Background) {
        AUTO_USAGE_WORKER_RECORD_DB_SHARE_BACKGROUND
    } else {
        AUTO_USAGE_WORKER_RECORD_DB_SHARE_ALL
    };
    let max_connections = database.pool.max_connections.max(1) as usize;
    Some(
        max_connections
            .checked_div(divisor.max(1))
            .unwrap_or(1)
            .clamp(1, MAX_USAGE_QUEUE_WORKERS),
    )
}

fn automatic_usage_queue_workers_for_parallelism(
    parallelism: usize,
    node_role: NodeRoleArg,
    max_in_flight_requests: Option<usize>,
    distributed_request_limit: Option<usize>,
    database: Option<&SqlDatabaseConfig>,
) -> usize {
    let cpu_default = parallelism.max(1).clamp(
        AUTO_USAGE_QUEUE_WORKERS_MIN,
        DEFAULT_USAGE_QUEUE_WORKERS_CAP,
    );
    let requested =
        usage_queue_request_concurrency_hint(max_in_flight_requests, distributed_request_limit)
            .map(usage_queue_workers_for_request_concurrency)
            .unwrap_or(cpu_default);
    requested
        .min(usage_queue_worker_database_cap(node_role, database))
        .clamp(1, MAX_USAGE_QUEUE_WORKERS)
}

fn automatic_usage_queue_workers(
    node_role: NodeRoleArg,
    max_in_flight_requests: Option<usize>,
    distributed_request_limit: Option<usize>,
    database: Option<&SqlDatabaseConfig>,
) -> usize {
    automatic_usage_queue_workers_for_parallelism(
        available_parallelism_usize(),
        node_role,
        max_in_flight_requests,
        distributed_request_limit,
        database,
    )
}

fn automatic_sql_pool_config(driver: DatabaseDriver) -> SqlPoolConfig {
    automatic_sql_pool_config_for_parallelism(driver, available_parallelism_u32())
}

fn automatic_sql_pool_config_for_parallelism(
    driver: DatabaseDriver,
    parallelism: u32,
) -> SqlPoolConfig {
    let (min_connections, max_connections) = match driver {
        DatabaseDriver::Sqlite => (1, DEFAULT_SQLITE_POOL_MAX_CONNECTIONS),
        DatabaseDriver::Mysql | DatabaseDriver::Postgres => {
            let cpu_count = parallelism.max(1);
            let max_connections = cpu_count
                .saturating_mul(AUTO_SERVER_SQL_POOL_CONNECTIONS_PER_CPU)
                .clamp(
                    AUTO_SERVER_SQL_POOL_MAX_CONNECTIONS_FLOOR,
                    AUTO_SERVER_SQL_POOL_MAX_CONNECTIONS_CAP,
                );
            let min_connections = cpu_count
                .clamp(
                    AUTO_SERVER_SQL_POOL_MIN_CONNECTIONS_FLOOR,
                    AUTO_SERVER_SQL_POOL_MIN_CONNECTIONS_CAP,
                )
                .min(max_connections);
            (min_connections, max_connections)
        }
    };

    SqlPoolConfig {
        min_connections,
        max_connections,
        acquire_timeout_ms: DEFAULT_SQL_POOL_ACQUIRE_TIMEOUT_MS,
        idle_timeout_ms: DEFAULT_SQL_POOL_IDLE_TIMEOUT_MS,
        max_lifetime_ms: DEFAULT_SQL_POOL_MAX_LIFETIME_MS,
        statement_cache_capacity: DEFAULT_SQL_POOL_STATEMENT_CACHE_CAPACITY,
        require_ssl: false,
    }
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

    #[arg(long, env = "AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS")]
    postgres_min_connections: Option<u32>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS")]
    postgres_max_connections: Option<u32>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_POSTGRES_ACQUIRE_TIMEOUT_MS")]
    postgres_acquire_timeout_ms: Option<u64>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_POSTGRES_IDLE_TIMEOUT_MS")]
    postgres_idle_timeout_ms: Option<u64>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_LIFETIME_MS")]
    postgres_max_lifetime_ms: Option<u64>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_POSTGRES_STATEMENT_CACHE_CAPACITY")]
    postgres_statement_cache_capacity: Option<usize>,

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
            pool: self.effective_sql_pool_config(driver),
        })
    }

    fn effective_sql_pool_config(&self, driver: DatabaseDriver) -> SqlPoolConfig {
        let auto = automatic_sql_pool_config(driver);
        let mut min_connections = self
            .postgres_min_connections
            .unwrap_or(auto.min_connections);
        let mut max_connections = self
            .postgres_max_connections
            .unwrap_or(auto.max_connections)
            .max(1);

        match (self.postgres_min_connections, self.postgres_max_connections) {
            (None, Some(_)) if min_connections > max_connections => {
                min_connections = max_connections;
            }
            (Some(_), None) if max_connections < min_connections => {
                max_connections = min_connections.max(1);
            }
            _ => {}
        }

        SqlPoolConfig {
            min_connections,
            max_connections,
            acquire_timeout_ms: self
                .postgres_acquire_timeout_ms
                .unwrap_or(auto.acquire_timeout_ms),
            idle_timeout_ms: self
                .postgres_idle_timeout_ms
                .unwrap_or(auto.idle_timeout_ms),
            max_lifetime_ms: self
                .postgres_max_lifetime_ms
                .unwrap_or(auto.max_lifetime_ms),
            statement_cache_capacity: self
                .postgres_statement_cache_capacity
                .unwrap_or(auto.statement_cache_capacity),
            require_ssl: driver != DatabaseDriver::Sqlite && self.postgres_require_ssl,
        }
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
        env = "AETHER_GATEWAY_USAGE_QUEUE_TERMINAL_EVENTS",
        default_value_t = true
    )]
    queue_terminal_events: bool,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_LIFECYCLE_EVENTS",
        default_value_t = true
    )]
    queue_lifecycle_events: bool,

    #[arg(long, env = "AETHER_GATEWAY_USAGE_QUEUE_WORKERS", value_name = "COUNT")]
    queue_workers: Option<usize>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_WORKER_AUTOSCALE_ENABLED",
        default_value_t = true
    )]
    queue_worker_autoscale_enabled: bool,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_WORKER_MAX_COUNT",
        value_name = "COUNT",
        default_value = "32"
    )]
    queue_worker_max_count: Option<usize>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_WORKER_RECORD_CONCURRENCY_LIMIT",
        value_name = "COUNT",
        default_value = "32"
    )]
    worker_record_concurrency_limit: Option<usize>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_WORKER_SCALE_INTERVAL_MS",
        default_value_t = 1_000
    )]
    queue_worker_scale_interval_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_WORKER_IDLE_SCALE_DOWN_TICKS",
        default_value_t = 30
    )]
    queue_worker_idle_scale_down_ticks: u64,

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
        default_value_t = 200_000
    )]
    queue_stream_maxlen: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_BATCH_SIZE",
        default_value_t = 128
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
        default_value_t = 60_000
    )]
    queue_reclaim_idle_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_COUNT",
        default_value_t = 128
    )]
    queue_reclaim_count: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_QUEUE_RECLAIM_INTERVAL_MS",
        default_value_t = 5_000
    )]
    queue_reclaim_interval_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_TERMINAL_ENQUEUE_MAX_IN_FLIGHT",
        default_value_t = 1_024
    )]
    terminal_enqueue_max_in_flight: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_MAX_IN_FLIGHT",
        default_value_t = 512
    )]
    lifecycle_enqueue_max_in_flight: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_LIFECYCLE_ENQUEUE_DELAY_MS",
        default_value_t = 1_000
    )]
    lifecycle_enqueue_delay_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_RETRY_DEFERRED_LIFECYCLE_EVENTS",
        default_value_t = true
    )]
    retry_deferred_lifecycle_events: bool,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_BUFFER_CAPACITY",
        default_value_t = 131_072
    )]
    enqueue_retry_buffer_capacity: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_WORKERS",
        default_value_t = 8
    )]
    enqueue_retry_workers: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_INITIAL_BACKOFF_MS",
        default_value_t = 3_000
    )]
    enqueue_retry_initial_backoff_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_ENQUEUE_RETRY_MAX_BACKOFF_MS",
        default_value_t = 10_000
    )]
    enqueue_retry_max_backoff_ms: u64,
}

impl GatewayUsageArgs {
    fn effective_queue_workers(
        &self,
        node_role: NodeRoleArg,
        max_in_flight_requests: Option<usize>,
        distributed_request_limit: Option<usize>,
        database: Option<&SqlDatabaseConfig>,
    ) -> usize {
        if let Some(queue_workers) = self.queue_workers {
            return queue_workers.clamp(1, MAX_USAGE_QUEUE_WORKERS);
        }
        if !self.queue_terminal_events && !self.queue_lifecycle_events {
            return 1;
        }
        automatic_usage_queue_workers(
            node_role,
            max_in_flight_requests,
            distributed_request_limit,
            database,
        )
    }

    fn effective_queue_worker_max_count(
        &self,
        node_role: NodeRoleArg,
        database: Option<&SqlDatabaseConfig>,
        worker_count: usize,
    ) -> usize {
        if !self.queue_worker_autoscale_enabled {
            return worker_count.clamp(1, MAX_USAGE_QUEUE_WORKERS);
        }
        self.queue_worker_max_count
            .unwrap_or_else(|| usage_queue_worker_database_cap(node_role, database))
            .max(1)
            .min(usage_queue_worker_database_cap(node_role, database))
            .clamp(worker_count.max(1), MAX_USAGE_QUEUE_WORKERS)
    }

    fn runtime_state_blocking_stream_lanes(
        &self,
        node_role: NodeRoleArg,
        database: Option<&SqlDatabaseConfig>,
        worker_max_count: usize,
    ) -> Option<usize> {
        if !node_role.spawns_background_tasks()
            || (!self.queue_terminal_events && !self.queue_lifecycle_events)
            || database.is_none()
        {
            return None;
        }
        Some(worker_max_count.clamp(1, MAX_USAGE_QUEUE_WORKERS))
    }

    fn effective_worker_record_concurrency_limit(
        &self,
        node_role: NodeRoleArg,
        database: Option<&SqlDatabaseConfig>,
    ) -> Option<usize> {
        if let Some(limit) = self.worker_record_concurrency_limit {
            if limit == 0 {
                return None;
            }
            return Some(
                limit
                    .min(MAX_USAGE_QUEUE_WORKERS)
                    .min(
                        usage_worker_record_concurrency_database_cap(node_role, database)
                            .unwrap_or(MAX_USAGE_QUEUE_WORKERS),
                    )
                    .max(1),
            );
        }
        if !node_role.spawns_background_tasks()
            || (!self.queue_terminal_events && !self.queue_lifecycle_events)
        {
            return None;
        }
        usage_worker_record_concurrency_database_cap(node_role, database)
    }

    fn to_config(
        &self,
        worker_count: usize,
        worker_max_count: usize,
        worker_record_concurrency_limit: Option<usize>,
    ) -> UsageRuntimeConfig {
        UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: self.queue_terminal_events,
            queue_lifecycle_events: self.queue_lifecycle_events,
            worker_count: worker_count.clamp(1, MAX_USAGE_QUEUE_WORKERS),
            worker_autoscale_enabled: self.queue_worker_autoscale_enabled,
            worker_max_count: worker_max_count.clamp(worker_count.max(1), MAX_USAGE_QUEUE_WORKERS),
            worker_record_concurrency_limit,
            worker_scale_interval_ms: self.queue_worker_scale_interval_ms.max(1),
            worker_idle_scale_down_ticks: self.queue_worker_idle_scale_down_ticks.max(1),
            stream_key: self.queue_stream_key.trim().to_string(),
            consumer_group: self.queue_group.trim().to_string(),
            dlq_stream_key: self.queue_dlq_stream_key.trim().to_string(),
            stream_maxlen: self.queue_stream_maxlen.max(1),
            consumer_batch_size: self.queue_batch_size.max(1),
            consumer_block_ms: self.queue_block_ms.max(1),
            reclaim_idle_ms: self.queue_reclaim_idle_ms.max(1),
            reclaim_count: self.queue_reclaim_count.max(1),
            reclaim_interval_ms: self.queue_reclaim_interval_ms.max(1),
            terminal_enqueue_max_in_flight: self.terminal_enqueue_max_in_flight.max(1),
            lifecycle_enqueue_max_in_flight: self.lifecycle_enqueue_max_in_flight.max(1),
            lifecycle_enqueue_delay_ms: self.lifecycle_enqueue_delay_ms,
            retry_deferred_lifecycle_events: self.retry_deferred_lifecycle_events,
            enqueue_retry_buffer_capacity: self.enqueue_retry_buffer_capacity.max(1),
            enqueue_retry_workers: self.enqueue_retry_workers.clamp(1, 64),
            enqueue_retry_initial_backoff_ms: self.enqueue_retry_initial_backoff_ms.max(1),
            enqueue_retry_max_backoff_ms: self
                .enqueue_retry_max_backoff_ms
                .max(self.enqueue_retry_initial_backoff_ms.max(1)),
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

    #[arg(
        long,
        env = "AETHER_GATEWAY_LISTEN_BACKLOG",
        default_value_t = DEFAULT_GATEWAY_LISTEN_BACKLOG
    )]
    listen_backlog: i32,

    #[arg(
        long,
        env = "AETHER_GATEWAY_LISTENER_SHARDS",
        default_value_t = DEFAULT_GATEWAY_LISTENER_SHARDS
    )]
    /// Number of SO_REUSEPORT listener shards. 0 selects a high-concurrency default.
    listener_shards: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS",
        default_value_t = DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS
    )]
    http2_max_concurrent_streams: u32,

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

    #[arg(
        long,
        env = "AETHER_RUNTIME_COMMAND_TIMEOUT_MS",
        default_value_t = 1_000
    )]
    runtime_command_timeout_ms: u64,

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
        blocking_stream_lanes: Option<usize>,
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
            command_timeout_ms: Some(self.runtime_command_timeout_ms.max(1)),
            blocking_stream_lanes,
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

fn gateway_listen_backlog(backlog: i32) -> i32 {
    backlog.clamp(MIN_GATEWAY_LISTEN_BACKLOG, MAX_GATEWAY_LISTEN_BACKLOG)
}

fn gateway_auto_listener_shards() -> usize {
    #[cfg(unix)]
    {
        std::thread::available_parallelism()
            .map(|parallelism| parallelism.get().saturating_mul(2))
            .unwrap_or(16)
            .clamp(8, 16)
            .min(MAX_GATEWAY_LISTENER_SHARDS)
    }

    #[cfg(not(unix))]
    {
        1
    }
}

fn gateway_listener_shards(shards: usize) -> usize {
    if shards == 0 {
        return gateway_auto_listener_shards();
    }
    shards.clamp(1, MAX_GATEWAY_LISTENER_SHARDS)
}

fn gateway_http2_max_concurrent_streams(streams: u32) -> u32 {
    streams.clamp(
        MIN_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS,
        MAX_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS,
    )
}

fn gateway_listener(
    bind_addr: std::net::SocketAddr,
    backlog: i32,
    reuse_port: bool,
) -> Result<tokio::net::TcpListener, std::io::Error> {
    let domain = match bind_addr {
        std::net::SocketAddr::V4(_) => socket2::Domain::IPV4,
        std::net::SocketAddr::V6(_) => socket2::Domain::IPV6,
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    if reuse_port {
        set_gateway_listener_reuse_port(&socket)?;
    }
    socket.set_nonblocking(true)?;
    socket.set_tcp_nodelay(true)?;
    socket.bind(&bind_addr.into())?;
    socket.listen(gateway_listen_backlog(backlog))?;
    tokio::net::TcpListener::from_std(socket.into())
}

#[cfg(unix)]
fn set_gateway_listener_reuse_port(socket: &socket2::Socket) -> Result<(), std::io::Error> {
    socket.set_reuse_port(true)
}

#[cfg(not(unix))]
fn set_gateway_listener_reuse_port(_socket: &socket2::Socket) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "AETHER_GATEWAY_LISTENER_SHARDS > 1 requires SO_REUSEPORT support",
    ))
}

fn gateway_listeners(
    bind_addr: std::net::SocketAddr,
    backlog: i32,
    shards: usize,
) -> Result<Vec<tokio::net::TcpListener>, std::io::Error> {
    let shards = gateway_listener_shards(shards);
    let mut listeners = Vec::with_capacity(shards);
    for _ in 0..shards {
        listeners.push(gateway_listener(bind_addr, backlog, shards > 1)?);
    }
    Ok(listeners)
}

async fn serve_gateway_router(
    listeners: Vec<tokio::net::TcpListener>,
    router: axum::Router,
    http2_max_concurrent_streams: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let http2_max_concurrent_streams =
        gateway_http2_max_concurrent_streams(http2_max_concurrent_streams);
    let mut servers = tokio::task::JoinSet::new();
    for listener in listeners {
        let router = router.clone();
        servers.spawn(async move {
            serve_gateway_listener(listener, router, http2_max_concurrent_streams).await
        });
    }
    if let Some(result) = servers.join_next().await {
        servers.abort_all();
        let serve_result = result
            .map_err(|err| std::io::Error::other(format!("gateway listener task failed: {err}")))?;
        serve_result?;
    }
    Ok(())
}

async fn serve_gateway_listener(
    listener: tokio::net::TcpListener,
    router: axum::Router,
    http2_max_concurrent_streams: u32,
) -> Result<(), std::io::Error> {
    let mut make_service = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
    loop {
        let (io, remote_addr) = listener.accept().await?;
        let tower_service = make_service
            .call(remote_addr)
            .await
            .unwrap_or_else(|err| match err {})
            .map_request(|req: Request<Incoming>| req.map(Body::new));
        let hyper_service = TowerToHyperService::new(tower_service);
        let io = TokioIo::new(io);

        tokio::spawn(async move {
            let mut builder = HyperServerBuilder::new(TokioExecutor::new());
            builder.http2().enable_connect_protocol();
            builder
                .http2()
                .max_concurrent_streams(http2_max_concurrent_streams);
            if let Err(err) = builder
                .serve_connection_with_upgrades(io, hyper_service)
                .await
            {
                tracing::trace!(error = ?err, "gateway connection closed with error");
            }
        });
    }
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
    let usage_queue_request_concurrency_hint = usage_queue_request_concurrency_hint(
        args.max_in_flight_requests,
        args.distributed_request_limit,
    );
    let usage_queue_workers = args.usage.effective_queue_workers(
        args.node_role,
        args.max_in_flight_requests,
        args.distributed_request_limit,
        sql_database_config.as_ref(),
    );
    let usage_queue_worker_max_count = args.usage.effective_queue_worker_max_count(
        args.node_role,
        sql_database_config.as_ref(),
        usage_queue_workers,
    );
    let usage_worker_record_concurrency_limit = args
        .usage
        .effective_worker_record_concurrency_limit(args.node_role, sql_database_config.as_ref());
    let usage_config = args.usage.to_config(
        usage_queue_workers,
        usage_queue_worker_max_count,
        usage_worker_record_concurrency_limit,
    );
    let usage_blocking_stream_lanes = args.usage.runtime_state_blocking_stream_lanes(
        args.node_role,
        sql_database_config.as_ref(),
        usage_config.worker_max_count,
    );
    let runtime_state = Arc::new(
        RuntimeState::from_config(args.runtime_state_config(
            runtime_backend,
            data_redis_url.as_deref(),
            usage_blocking_stream_lanes,
        ))
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
        usage_queue_workers = usage_config.worker_count,
        usage_queue_worker_autoscale_enabled = usage_config.worker_autoscale_enabled,
        usage_queue_worker_max_count = usage_config.worker_max_count,
        usage_worker_record_concurrency_limit = usage_config
            .worker_record_concurrency_limit
            .unwrap_or_default(),
        usage_queue_request_concurrency_hint =
            usage_queue_request_concurrency_hint.unwrap_or_default(),
        usage_queue_request_concurrency_hint_source = if usage_queue_request_concurrency_hint.is_some() {
            "explicit"
        } else {
            "none"
        },
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
        usage_queue_workers = usage_config.worker_count,
        usage_queue_workers_source = if args.usage.queue_workers.is_some() {
            "explicit"
        } else {
            "auto"
        },
        usage_queue_worker_autoscale_enabled = usage_config.worker_autoscale_enabled,
        usage_queue_worker_max_count = usage_config.worker_max_count,
        usage_worker_record_concurrency_limit = usage_config
            .worker_record_concurrency_limit
            .unwrap_or_default(),
        usage_queue_request_concurrency_hint =
            usage_queue_request_concurrency_hint.unwrap_or_default(),
        usage_queue_request_concurrency_hint_source =
            if usage_queue_request_concurrency_hint.is_some() {
                "explicit"
            } else {
                "none"
            },
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
        data_database_pool_min_connections = sql_database_config
            .as_ref()
            .map(|database| database.pool.min_connections)
            .unwrap_or_default(),
        data_database_pool_max_connections = sql_database_config
            .as_ref()
            .map(|database| database.pool.max_connections)
            .unwrap_or_default(),
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
        .with_usage_runtime_config(usage_config)?
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
    match state.prewarm_chat_pii_redaction_runtime_config().await {
        Ok(enabled) => {
            info!(
                chat_pii_redaction_enabled = enabled,
                "prewarmed chat pii redaction runtime config"
            );
        }
        Err(err) => {
            warn!(
                error = %err,
                "failed to prewarm chat pii redaction runtime config"
            );
        }
    }
    match prewarm_direct_h2c_sender_cache_from_env_for_startup().await {
        Ok(Some(report)) => {
            if report.failed_targets > 0 {
                warn!(
                    requested_urls = report.requested_urls,
                    unique_targets = report.unique_targets,
                    warmed_targets = report.warmed_targets,
                    failed_targets = report.failed_targets,
                    ready_required = report.ready_required,
                    first_error = ?report.first_error,
                    "direct h2c sender cache prewarm completed with failures"
                );
            } else {
                info!(
                    requested_urls = report.requested_urls,
                    unique_targets = report.unique_targets,
                    warmed_targets = report.warmed_targets,
                    ready_required = report.ready_required,
                    "direct h2c sender cache prewarmed"
                );
            }
        }
        Ok(None) => {}
        Err(err) => {
            return Err(std::io::Error::other(err).into());
        }
    }

    let background_tasks = if args.node_role.spawns_background_tasks() {
        Some(state.spawn_background_tasks())
    } else {
        info!(
            node_role = args.node_role.as_str(),
            "background workers disabled for this node role"
        );
        None
    };
    let listen_backlog = gateway_listen_backlog(args.listen_backlog);
    let listener_shards = gateway_listener_shards(args.listener_shards);
    let listeners = gateway_listeners(bind_addr, listen_backlog, listener_shards)?;
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
        listen_backlog,
        listener_shards,
        http2_max_concurrent_streams = gateway_http2_max_concurrent_streams(args.http2_max_concurrent_streams),
        public_url = %public_base_url,
        healthcheck_url = %frontdoor_health_url,
        legacy_route_policy = "fail_closed",
        "aether-gateway ready"
    );

    serve_gateway_router(listeners, router, args.http2_max_concurrent_streams).await?;
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
        pending_count, next_version, next_description
    ))
}

fn pending_backfills_error(
    pending_count: usize,
    next_version: i64,
    next_description: &str,
) -> std::io::Error {
    std::io::Error::other(format!(
        "database backfills are behind by {} backfill(s); next pending backfill is {} ({})\nrun `aether-gateway --apply-backfills` before starting the service",
        pending_count, next_version, next_description
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        automatic_sql_pool_config, automatic_sql_pool_config_for_parallelism,
        automatic_usage_queue_workers_for_parallelism, ensure_database_backfills_are_current,
        ensure_database_schema_is_current, pending_backfills_error, pending_schema_error,
        resolve_healthcheck_url, Args, DatabaseDriverArg, DeploymentTopologyArg, GatewayDataArgs,
        GatewayFrontdoorArgs, GatewayLogDestinationArg, GatewayLogFormatArg, GatewayLogRotationArg,
        GatewayLoggingArgs, GatewayRateLimitArgs, GatewayUsageArgs, NodeRoleArg, RuntimeBackendArg,
        VideoTaskTruthSourceArg, DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS,
        DEFAULT_GATEWAY_LISTENER_SHARDS, DEFAULT_GATEWAY_LISTEN_BACKLOG,
        MAX_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS, MAX_GATEWAY_LISTENER_SHARDS,
        MAX_GATEWAY_LISTEN_BACKLOG, MIN_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS,
        MIN_GATEWAY_LISTEN_BACKLOG,
    };
    use aether_data::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};
    use aether_gateway::AppState;

    fn test_args() -> Args {
        Args {
            command: None,
            app_port: 8084,
            listen_backlog: DEFAULT_GATEWAY_LISTEN_BACKLOG,
            listener_shards: DEFAULT_GATEWAY_LISTENER_SHARDS,
            http2_max_concurrent_streams: DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS,
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
            runtime_command_timeout_ms: 1_000,
            data: GatewayDataArgs {
                database_driver: None,
                database_url: None,
                postgres_url: None,
                encryption_key: None,
                redis_url: None,
                redis_key_prefix: None,
                postgres_min_connections: None,
                postgres_max_connections: None,
                postgres_acquire_timeout_ms: None,
                postgres_idle_timeout_ms: None,
                postgres_max_lifetime_ms: None,
                postgres_statement_cache_capacity: None,
                postgres_require_ssl: false,
            },
            usage: GatewayUsageArgs {
                queue_terminal_events: true,
                queue_lifecycle_events: true,
                queue_workers: Some(4),
                queue_worker_autoscale_enabled: true,
                queue_worker_max_count: Some(32),
                worker_record_concurrency_limit: Some(32),
                queue_worker_scale_interval_ms: 1_000,
                queue_worker_idle_scale_down_ticks: 30,
                queue_stream_key: "usage:events".to_string(),
                queue_group: "usage_consumers".to_string(),
                queue_dlq_stream_key: "usage:events:dlq".to_string(),
                queue_stream_maxlen: 200_000,
                queue_batch_size: 128,
                queue_block_ms: 500,
                queue_reclaim_idle_ms: 60_000,
                queue_reclaim_count: 128,
                queue_reclaim_interval_ms: 5_000,
                terminal_enqueue_max_in_flight: 1_024,
                lifecycle_enqueue_max_in_flight: 512,
                lifecycle_enqueue_delay_ms: 1_000,
                retry_deferred_lifecycle_events: true,
                enqueue_retry_buffer_capacity: 131_072,
                enqueue_retry_workers: 8,
                enqueue_retry_initial_backoff_ms: 3_000,
                enqueue_retry_max_backoff_ms: 10_000,
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

    fn test_database(driver: DatabaseDriver, max_connections: u32) -> SqlDatabaseConfig {
        let url = match driver {
            DatabaseDriver::Sqlite => "sqlite://./data/aether.db",
            DatabaseDriver::Mysql => "mysql://root:root@localhost/aether",
            DatabaseDriver::Postgres => "postgres://postgres:postgres@localhost/aether",
        };
        let max_connections = max_connections.max(1);
        SqlDatabaseConfig::new(
            driver,
            url,
            SqlPoolConfig {
                min_connections: 1,
                max_connections,
                ..SqlPoolConfig::default()
            },
        )
        .expect("test database config should build")
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
    fn clamps_gateway_listen_backlog() {
        assert_eq!(
            super::gateway_listen_backlog(MIN_GATEWAY_LISTEN_BACKLOG - 1),
            MIN_GATEWAY_LISTEN_BACKLOG
        );
        assert_eq!(
            super::gateway_listen_backlog(DEFAULT_GATEWAY_LISTEN_BACKLOG),
            DEFAULT_GATEWAY_LISTEN_BACKLOG
        );
        assert_eq!(
            super::gateway_listen_backlog(MAX_GATEWAY_LISTEN_BACKLOG + 1),
            MAX_GATEWAY_LISTEN_BACKLOG
        );
    }

    #[test]
    fn clamps_gateway_listener_shards() {
        let auto_shards = super::gateway_listener_shards(0);
        assert!((1..=MAX_GATEWAY_LISTENER_SHARDS).contains(&auto_shards));
        assert_eq!(super::gateway_listener_shards(1), 1);
        assert_eq!(
            super::gateway_listener_shards(DEFAULT_GATEWAY_LISTENER_SHARDS),
            auto_shards
        );
        assert_eq!(
            super::gateway_listener_shards(MAX_GATEWAY_LISTENER_SHARDS + 1),
            MAX_GATEWAY_LISTENER_SHARDS
        );
    }

    #[test]
    fn clamps_gateway_http2_max_concurrent_streams() {
        assert_eq!(
            super::gateway_http2_max_concurrent_streams(
                MIN_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS - 1
            ),
            MIN_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS
        );
        assert_eq!(
            super::gateway_http2_max_concurrent_streams(
                DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS
            ),
            DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS
        );
        assert_eq!(
            super::gateway_http2_max_concurrent_streams(
                MAX_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS + 1
            ),
            MAX_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS
        );
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
    fn gateway_data_pool_auto_sizes_sqlite_to_single_connection() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Sqlite);
        args.data.database_url = Some("sqlite://./data/aether.db".to_string());

        let database = args
            .data
            .effective_sql_database_config()
            .expect("sqlite database config should build");

        assert_eq!(database.driver, DatabaseDriver::Sqlite);
        assert_eq!(database.pool.min_connections, 1);
        assert_eq!(database.pool.max_connections, 1);
    }

    #[test]
    fn gateway_data_pool_auto_sizes_server_databases_from_runtime_cpu() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Postgres);
        args.data.database_url = Some("postgres://postgres:postgres@localhost/aether".to_string());

        let database = args
            .data
            .effective_sql_database_config()
            .expect("postgres database config should build");
        let auto = automatic_sql_pool_config(DatabaseDriver::Postgres);

        assert_eq!(database.driver, DatabaseDriver::Postgres);
        assert_eq!(database.pool.min_connections, auto.min_connections);
        assert_eq!(database.pool.max_connections, auto.max_connections);
    }

    #[test]
    fn gateway_data_pool_cpu_sizing_examples() {
        let two_cpu = automatic_sql_pool_config_for_parallelism(DatabaseDriver::Postgres, 2);
        assert_eq!(two_cpu.min_connections, 4);
        assert_eq!(two_cpu.max_connections, 20);

        let eight_cpu = automatic_sql_pool_config_for_parallelism(DatabaseDriver::Postgres, 8);
        assert_eq!(eight_cpu.min_connections, 8);
        assert_eq!(eight_cpu.max_connections, 32);

        let sixteen_cpu = automatic_sql_pool_config_for_parallelism(DatabaseDriver::Postgres, 16);
        assert_eq!(sixteen_cpu.min_connections, 16);
        assert_eq!(sixteen_cpu.max_connections, 64);

        let many_cpu = automatic_sql_pool_config_for_parallelism(DatabaseDriver::Postgres, 32);
        assert_eq!(many_cpu.min_connections, 16);
        assert_eq!(many_cpu.max_connections, 100);
    }

    #[test]
    fn gateway_usage_queue_workers_manual_override_wins_and_is_capped() {
        let mut args = test_args();
        args.usage.queue_workers = Some(72);
        let database = test_database(DatabaseDriver::Postgres, 100);

        let workers = args.usage.effective_queue_workers(
            NodeRoleArg::All,
            Some(10_000),
            None,
            Some(&database),
        );

        assert_eq!(workers, 64);
        assert_eq!(args.usage.to_config(workers, 64, Some(8)).worker_count, 64);
    }

    #[test]
    fn gateway_usage_queue_workers_auto_uses_cpu_default_without_concurrency_hint() {
        let database = test_database(DatabaseDriver::Postgres, 100);

        let workers = automatic_usage_queue_workers_for_parallelism(
            4,
            NodeRoleArg::All,
            None,
            None,
            Some(&database),
        );

        assert_eq!(workers, 4);
    }

    #[test]
    fn gateway_usage_queue_worker_autoscale_max_uses_database_cap() {
        let mut args = test_args();
        args.usage.queue_workers = None;
        let database = test_database(DatabaseDriver::Postgres, 40);

        let workers =
            args.usage
                .effective_queue_workers(args.node_role, Some(1_024), None, Some(&database));
        let max_workers =
            args.usage
                .effective_queue_worker_max_count(args.node_role, Some(&database), workers);

        assert_eq!(workers, 8);
        assert_eq!(max_workers, 10);
    }

    #[test]
    fn gateway_usage_queue_worker_autoscale_max_respects_explicit_override() {
        let mut args = test_args();
        args.usage.queue_workers = None;
        args.usage.queue_worker_max_count = Some(32);
        let database = test_database(DatabaseDriver::Postgres, 200);

        let workers =
            args.usage
                .effective_queue_workers(args.node_role, Some(1_024), None, Some(&database));
        let max_workers =
            args.usage
                .effective_queue_worker_max_count(args.node_role, Some(&database), workers);

        assert_eq!(workers, 8);
        assert_eq!(max_workers, 32);
    }

    #[test]
    fn gateway_usage_worker_record_concurrency_defaults_to_pool_reserve_share() {
        let args = test_args();
        let database = test_database(DatabaseDriver::Postgres, 64);

        assert_eq!(
            args.usage
                .effective_worker_record_concurrency_limit(NodeRoleArg::All, Some(&database)),
            Some(8)
        );
        assert_eq!(
            args.usage.effective_worker_record_concurrency_limit(
                NodeRoleArg::Background,
                Some(&database)
            ),
            Some(16)
        );
    }

    #[test]
    fn gateway_usage_worker_record_concurrency_can_be_explicitly_disabled() {
        let mut args = test_args();
        args.usage.worker_record_concurrency_limit = Some(0);
        let database = test_database(DatabaseDriver::Postgres, 64);

        assert_eq!(
            args.usage
                .effective_worker_record_concurrency_limit(NodeRoleArg::All, Some(&database)),
            None
        );
    }

    #[test]
    fn gateway_usage_queue_blocking_stream_lanes_only_expand_when_worker_can_spawn() {
        let database = test_database(DatabaseDriver::Postgres, 100);
        let args = test_args();

        assert_eq!(
            args.usage
                .runtime_state_blocking_stream_lanes(NodeRoleArg::All, Some(&database), 10,),
            Some(10)
        );
        assert_eq!(
            args.usage.runtime_state_blocking_stream_lanes(
                NodeRoleArg::Frontdoor,
                Some(&database),
                10,
            ),
            None
        );
        assert_eq!(
            args.usage
                .runtime_state_blocking_stream_lanes(NodeRoleArg::All, None, 10),
            None
        );

        let mut disabled_queue_args = args;
        disabled_queue_args.usage.queue_terminal_events = false;
        disabled_queue_args.usage.queue_lifecycle_events = false;
        assert_eq!(
            disabled_queue_args
                .usage
                .runtime_state_blocking_stream_lanes(NodeRoleArg::All, Some(&database), 10,),
            None
        );
    }

    #[test]
    fn gateway_usage_queue_workers_auto_scales_from_request_concurrency() {
        let database = test_database(DatabaseDriver::Postgres, 100);

        let workers = automatic_usage_queue_workers_for_parallelism(
            8,
            NodeRoleArg::All,
            Some(1_536),
            None,
            Some(&database),
        );

        assert_eq!(workers, 12);
    }

    #[test]
    fn gateway_usage_queue_workers_auto_respects_effective_request_limit() {
        let database = test_database(DatabaseDriver::Postgres, 100);

        let workers = automatic_usage_queue_workers_for_parallelism(
            8,
            NodeRoleArg::All,
            Some(2_048),
            Some(256),
            Some(&database),
        );

        assert_eq!(workers, 2);
    }

    #[test]
    fn gateway_usage_queue_workers_auto_is_capped_by_database_pool() {
        let database = test_database(DatabaseDriver::Postgres, 20);

        let workers = automatic_usage_queue_workers_for_parallelism(
            16,
            NodeRoleArg::All,
            Some(5_000),
            None,
            Some(&database),
        );

        assert_eq!(workers, 5);
    }

    #[test]
    fn gateway_usage_queue_workers_auto_gives_background_nodes_more_pool_budget() {
        let database = test_database(DatabaseDriver::Postgres, 20);

        let workers = automatic_usage_queue_workers_for_parallelism(
            16,
            NodeRoleArg::Background,
            Some(5_000),
            None,
            Some(&database),
        );

        assert_eq!(workers, 10);
    }

    #[test]
    fn gateway_usage_queue_workers_auto_uses_single_worker_for_sqlite() {
        let database = test_database(DatabaseDriver::Sqlite, 1);

        let workers = automatic_usage_queue_workers_for_parallelism(
            16,
            NodeRoleArg::All,
            Some(5_000),
            None,
            Some(&database),
        );

        assert_eq!(workers, 1);
    }

    #[test]
    fn gateway_data_pool_explicit_values_override_auto_sizing() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Sqlite);
        args.data.database_url = Some("sqlite://./data/aether.db".to_string());
        args.data.postgres_min_connections = Some(2);
        args.data.postgres_max_connections = Some(8);
        args.data.postgres_acquire_timeout_ms = Some(2_000);

        let database = args
            .data
            .effective_sql_database_config()
            .expect("sqlite database config should build");

        assert_eq!(database.pool.min_connections, 2);
        assert_eq!(database.pool.max_connections, 8);
        assert_eq!(database.pool.acquire_timeout_ms, 2_000);
    }

    #[test]
    fn gateway_data_pool_partial_max_override_clamps_auto_minimum() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Postgres);
        args.data.database_url = Some("postgres://postgres:postgres@localhost/aether".to_string());
        args.data.postgres_max_connections = Some(2);

        let database = args
            .data
            .effective_sql_database_config()
            .expect("postgres database config should build");

        assert_eq!(database.pool.min_connections, 2);
        assert_eq!(database.pool.max_connections, 2);
    }

    #[test]
    fn gateway_data_pool_partial_min_override_raises_auto_maximum() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Postgres);
        args.data.database_url = Some("postgres://postgres:postgres@localhost/aether".to_string());
        args.data.postgres_min_connections = Some(128);

        let database = args
            .data
            .effective_sql_database_config()
            .expect("postgres database config should build");

        assert_eq!(database.pool.min_connections, 128);
        assert_eq!(database.pool.max_connections, 128);
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
            Some(7),
        );

        assert_eq!(config.blocking_stream_lanes, Some(7));
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
