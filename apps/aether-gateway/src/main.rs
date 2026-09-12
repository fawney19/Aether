#[cfg(all(not(target_env = "msvc"), feature = "jemalloc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::fs;
#[cfg(unix)]
use std::io::Write;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::{body::Body, extract::Request};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder as HyperServerBuilder,
    service::TowerToHyperService,
};
use tokio_util::sync::CancellationToken;
use tower::{Service as _, ServiceExt as _};
use tracing::{debug, info, warn};

/// Coordinates the connection-level deadline that covers protocol detection and
/// the first request header block. Hyper's HTTP/1 timer starts only after the
/// auto protocol detector has finished, while HTTP/2 has no equivalent header
/// timer. Keeping this gate outside the parser closes that initial gap without
/// imposing a deadline on request or response bodies.
#[derive(Clone)]
struct GatewayFirstRequestGate {
    seen: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl GatewayFirstRequestGate {
    fn new() -> Self {
        Self {
            seen: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn mark_seen(&self) {
        if !self.seen.swap(true, Ordering::Release) {
            self.notify.notify_one();
        }
    }

    fn is_seen(&self) -> bool {
        self.seen.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
struct GatewayFirstRequestService<S> {
    inner: S,
    gate: GatewayFirstRequestGate,
}

impl<S, Request> tower::Service<Request> for GatewayFirstRequestService<S>
where
    S: tower::Service<Request>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        self.gate.mark_seen();
        self.inner.call(request)
    }
}

/// Drive one Hyper connection while enforcing the deadline for its first
/// request. This is kept generic so the timeout behavior can be regression
/// tested independently from the listener and application state.
async fn drive_gateway_connection<F, E>(
    connection: F,
    first_request_gate: GatewayFirstRequestGate,
    first_request_timeout: std::time::Duration,
) -> Result<(), E>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    if first_request_gate.is_seen() {
        return connection.await;
    }

    let mut connection = Box::pin(connection);
    let first_request_timeout = tokio::time::sleep(first_request_timeout);
    tokio::pin!(first_request_timeout);
    let first_request_notified = first_request_gate.notify.notified();
    tokio::pin!(first_request_notified);

    tokio::select! {
        result = &mut connection => result,
        _ = &mut first_request_timeout => {
            if first_request_gate.is_seen() {
                (&mut connection).await
            } else {
                tracing::debug!(
                    "gateway connection closed before the first request header completed"
                );
                Ok(())
            }
        }
        _ = &mut first_request_notified => (&mut connection).await,
    }
}

use aether_crypto::warm_python_fernet_secret;
use aether_data::lifecycle::export::{
    copy_database_records, export_database_jsonl, import_database_jsonl_with_options,
    DataCopyOptions, DataImportOptions, ExportDomain, MAX_JSONL_INPUT_BYTES,
};
use aether_data::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};
use aether_gateway::{
    attach_static_frontend, build_router_with_state,
    prewarm_direct_h2c_sender_cache_from_env_for_startup, set_gateway_frontdoor_app_port, AppState,
    FrontdoorCorsConfig, FrontdoorUserRpmConfig, GatewayDataConfig, UsageRuntimeConfig,
    VideoTaskTruthSourceMode,
};
use aether_gateway_frontdoor::{http_connection_limit, HttpConnectionBudget};
use aether_runtime::{
    init_service_runtime, FileLoggingConfig, LogDestination, LogFormat, LogRotation,
    ServiceRuntimeConfig,
};
use aether_runtime_state::{
    RedisClientConfig, RuntimeSemaphoreConfig, RuntimeState, RuntimeStateBackendMode,
    RuntimeStateConfig,
};

const MIN_GATEWAY_DATA_ENCRYPTION_KEY_BYTES: usize = 32;
const INSECURE_GATEWAY_DATA_ENCRYPTION_KEYS: &[&str] = &[
    "change-this-to-another-secure-random-string",
    "change-this-to-a-secure-random-string",
    "dev-encryption-key-do-not-use-in-production",
];

fn validate_gateway_data_encryption_key(value: Option<&str>) -> Result<(), &'static str> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    if value.len() < MIN_GATEWAY_DATA_ENCRYPTION_KEY_BYTES {
        return Err("gateway data encryption key must contain at least 32 bytes");
    }
    if INSECURE_GATEWAY_DATA_ENCRYPTION_KEYS.contains(&value) {
        return Err(
            "gateway data encryption key must not use a published example or development value",
        );
    }
    Ok(())
}

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
    Postgres,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum DatabaseModeArg {
    Auto,
    VerifyOnly,
}

fn resolve_database_mode(
    configured: Option<DatabaseModeArg>,
    legacy_auto_prepare: Option<bool>,
) -> DatabaseModeArg {
    if let Some(configured) = configured {
        return configured;
    }
    if let Some(legacy_auto_prepare) = legacy_auto_prepare {
        return if legacy_auto_prepare {
            DatabaseModeArg::Auto
        } else {
            DatabaseModeArg::VerifyOnly
        };
    }
    DatabaseModeArg::Auto
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
    AuthModules,
    OAuthProviders,
    UserOAuthLinks,
    UserGroups,
    UserGroupMembers,
    ProxyNodes,
    SystemConfigs,
    Wallets,
    Usage,
    Billing,
    Auxiliary,
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
            ExportDomainArg::AuthModules => ExportDomain::AuthModules,
            ExportDomainArg::OAuthProviders => ExportDomain::OAuthProviders,
            ExportDomainArg::UserOAuthLinks => ExportDomain::UserOAuthLinks,
            ExportDomainArg::UserGroups => ExportDomain::UserGroups,
            ExportDomainArg::UserGroupMembers => ExportDomain::UserGroupMembers,
            ExportDomainArg::ProxyNodes => ExportDomain::ProxyNodes,
            ExportDomainArg::SystemConfigs => ExportDomain::SystemConfigs,
            ExportDomainArg::Wallets => ExportDomain::Wallets,
            ExportDomainArg::Usage => ExportDomain::Usage,
            ExportDomainArg::Billing => ExportDomain::Billing,
            ExportDomainArg::Auxiliary => ExportDomain::Auxiliary,
        }
    }
}

impl From<DatabaseDriverArg> for DatabaseDriver {
    fn from(value: DatabaseDriverArg) -> Self {
        match value {
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

    const fn isolates_background_database(self) -> bool {
        matches!(self, Self::All)
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
// Per-process default for server SQL backends. Keep this below common
// database server max_connections defaults; operators can override with
// AETHER_GATEWAY_DATA_POSTGRES_{MIN,MAX}_CONNECTIONS after sizing the DB.
const AUTO_SERVER_SQL_POOL_CONNECTIONS_PER_CPU: u32 = 4;
const AUTO_SERVER_SQL_POOL_MIN_CONNECTIONS_FLOOR: u32 = 4;
const AUTO_SERVER_SQL_POOL_MIN_CONNECTIONS_CAP: u32 = 16;
const AUTO_SERVER_SQL_POOL_MAX_CONNECTIONS_FLOOR: u32 = 32;
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
// These limits protect the connection parser from slow-header and header-bomb
// attacks. They apply to request metadata only and do not cap body size or
// HTTP/2 stream concurrency.
const DEFAULT_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS: u64 = 30_000;
const MIN_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS: u64 = 1_000;
const MAX_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_GATEWAY_HTTP_HEADER_MAX_BYTES: usize = 64 * 1024;
const MIN_GATEWAY_HTTP_HEADER_MAX_BYTES: usize = 8 * 1024;
const MAX_GATEWAY_HTTP_HEADER_MAX_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_GATEWAY_HTTP_MAX_HEADERS: usize = 256;
const MIN_GATEWAY_HTTP_MAX_HEADERS: usize = 16;
const MAX_GATEWAY_HTTP_MAX_HEADERS: usize = 4_096;
const AUTO_GATEWAY_REQUESTS_PER_CPU: usize = 1_024;
const MIN_AUTO_GATEWAY_REQUEST_CONCURRENCY: usize = 512;
const MAX_AUTO_GATEWAY_REQUEST_CONCURRENCY: usize = 65_536;
const AUTO_GATEWAY_REQUEST_FD_DIVISOR: usize = 2;
const AUTO_GATEWAY_REQUEST_FD_RESERVE: usize = 256;
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

#[cfg(test)]
fn automatic_gateway_request_concurrency_for_parallelism(parallelism: usize) -> usize {
    automatic_gateway_request_concurrency_for_capacity(parallelism, None)
}

fn automatic_gateway_request_concurrency_for_capacity(
    parallelism: usize,
    fd_soft_limit: Option<usize>,
) -> usize {
    let cpu_limit = parallelism
        .max(1)
        .saturating_mul(AUTO_GATEWAY_REQUESTS_PER_CPU)
        .clamp(
            MIN_AUTO_GATEWAY_REQUEST_CONCURRENCY,
            MAX_AUTO_GATEWAY_REQUEST_CONCURRENCY,
        );
    let fd_limit = fd_soft_limit
        .map(|limit| {
            limit
                .saturating_sub(AUTO_GATEWAY_REQUEST_FD_RESERVE)
                .checked_div(AUTO_GATEWAY_REQUEST_FD_DIVISOR)
                .unwrap_or(1)
                .max(1)
        })
        .unwrap_or(MAX_AUTO_GATEWAY_REQUEST_CONCURRENCY);
    cpu_limit.min(fd_limit).max(1)
}

fn automatic_gateway_request_concurrency() -> usize {
    automatic_gateway_request_concurrency_for_capacity(
        available_parallelism_usize(),
        soft_fd_limit(),
    )
}

fn soft_fd_limit() -> Option<usize> {
    #[cfg(unix)]
    {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let result = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
        if result == 0 {
            return usize::try_from(limit.rlim_cur).ok();
        }
    }
    None
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

fn usage_database_config_for_role<'a>(
    node_role: NodeRoleArg,
    database: Option<&'a SqlDatabaseConfig>,
    isolated_background_database: Option<&'a SqlDatabaseConfig>,
) -> Option<&'a SqlDatabaseConfig> {
    if node_role.isolates_background_database() {
        isolated_background_database.or(database)
    } else {
        database
    }
}

fn usage_queue_worker_database_cap(
    node_role: NodeRoleArg,
    database: Option<&SqlDatabaseConfig>,
    database_is_isolated: bool,
) -> usize {
    let Some(database) = database else {
        return MAX_USAGE_QUEUE_WORKERS;
    };

    let max_connections = database.pool.max_connections.max(1) as usize;
    // An isolated pool is already a dedicated background budget. Applying the shared-pool
    // divisor a second time would underutilize that pool.
    if database_is_isolated {
        return max_connections
            .saturating_sub(1)
            .max(1)
            .clamp(1, MAX_USAGE_QUEUE_WORKERS);
    }
    let divisor = if matches!(node_role, NodeRoleArg::Background) {
        AUTO_USAGE_QUEUE_WORKERS_DB_SHARE_BACKGROUND
    } else {
        AUTO_USAGE_QUEUE_WORKERS_DB_SHARE_ALL
    };
    max_connections
        .saturating_add(divisor - 1)
        .checked_div(divisor)
        .unwrap_or(1)
        .clamp(1, MAX_USAGE_QUEUE_WORKERS)
}

fn usage_worker_record_concurrency_database_cap(
    node_role: NodeRoleArg,
    database: Option<&SqlDatabaseConfig>,
    database_is_isolated: bool,
) -> Option<usize> {
    let database = database?;

    let max_connections = database.pool.max_connections.max(1) as usize;
    // The isolated background pool has already been carved out of the foreground pool. Keep one
    // connection available for maintenance/health work and use the rest for usage persistence.
    if database_is_isolated {
        return Some(
            max_connections
                .saturating_sub(1)
                .max(1)
                .clamp(1, MAX_USAGE_QUEUE_WORKERS),
        );
    }
    let divisor = if matches!(node_role, NodeRoleArg::Background) {
        AUTO_USAGE_WORKER_RECORD_DB_SHARE_BACKGROUND
    } else {
        AUTO_USAGE_WORKER_RECORD_DB_SHARE_ALL
    };
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
    database_is_isolated: bool,
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
        .min(usage_queue_worker_database_cap(
            node_role,
            database,
            database_is_isolated,
        ))
        .clamp(1, MAX_USAGE_QUEUE_WORKERS)
}

fn automatic_usage_queue_workers(
    node_role: NodeRoleArg,
    max_in_flight_requests: Option<usize>,
    distributed_request_limit: Option<usize>,
    database: Option<&SqlDatabaseConfig>,
    database_is_isolated: bool,
) -> usize {
    automatic_usage_queue_workers_for_parallelism(
        available_parallelism_usize(),
        node_role,
        max_in_flight_requests,
        distributed_request_limit,
        database,
        database_is_isolated,
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
        DatabaseDriver::Postgres => {
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
    #[arg(long, env = "AETHER_DATABASE_DRIVER", global = true)]
    database_driver: Option<DatabaseDriverArg>,

    #[arg(long, env = "AETHER_DATABASE_URL", global = true)]
    database_url: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_POSTGRES_URL", global = true)]
    postgres_url: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_ENCRYPTION_KEY", global = true)]
    encryption_key: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_REDIS_URL", global = true)]
    redis_url: Option<String>,

    #[arg(long, env = "AETHER_GATEWAY_DATA_REDIS_KEY_PREFIX", global = true)]
    redis_key_prefix: Option<String>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS",
        global = true
    )]
    postgres_min_connections: Option<u32>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS",
        global = true
    )]
    postgres_max_connections: Option<u32>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_ACQUIRE_TIMEOUT_MS",
        global = true
    )]
    postgres_acquire_timeout_ms: Option<u64>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_IDLE_TIMEOUT_MS",
        global = true
    )]
    postgres_idle_timeout_ms: Option<u64>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_MAX_LIFETIME_MS",
        global = true
    )]
    postgres_max_lifetime_ms: Option<u64>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_STATEMENT_CACHE_CAPACITY",
        global = true
    )]
    postgres_statement_cache_capacity: Option<usize>,

    #[arg(
        long,
        env = "AETHER_GATEWAY_DATA_POSTGRES_REQUIRE_SSL",
        default_value_t = false,
        global = true
    )]
    postgres_require_ssl: bool,
}

impl GatewayDataArgs {
    fn validate_encryption_key(&self) -> Result<(), std::io::Error> {
        validate_gateway_data_encryption_key(self.effective_encryption_key().as_deref())
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))
    }

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
        let legacy_postgres_url = self
            .postgres_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let generic_database_url = std::env::var("DATABASE_URL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        resolve_database_url(
            self.effective_database_driver(),
            configured_url,
            legacy_postgres_url,
            generic_database_url,
        )
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
            require_ssl: self.postgres_require_ssl,
        }
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

fn resolve_database_url(
    driver: Option<DatabaseDriver>,
    configured_url: Option<String>,
    legacy_postgres_url: Option<String>,
    generic_database_url: Option<String>,
) -> Option<String> {
    if configured_url.is_some() {
        return configured_url;
    }

    match driver {
        Some(DatabaseDriver::Postgres) | None => legacy_postgres_url.or(generic_database_url),
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
        env = "AETHER_GATEWAY_USAGE_QUEUE_PAYLOAD_MAX_BYTES",
        default_value_t = 1024 * 1024
    )]
    queue_payload_max_bytes: usize,

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
        env = "AETHER_GATEWAY_USAGE_TERMINAL_SUBMISSION_MAX_IN_FLIGHT",
        default_value_t = 1_024
    )]
    terminal_submission_max_in_flight: u64,

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
        database_is_isolated: bool,
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
            database_is_isolated,
        )
    }

    fn effective_queue_worker_max_count(
        &self,
        node_role: NodeRoleArg,
        database: Option<&SqlDatabaseConfig>,
        worker_count: usize,
        database_is_isolated: bool,
    ) -> usize {
        if !self.queue_worker_autoscale_enabled {
            return worker_count.clamp(1, MAX_USAGE_QUEUE_WORKERS);
        }
        self.queue_worker_max_count
            .unwrap_or_else(|| {
                usage_queue_worker_database_cap(node_role, database, database_is_isolated)
            })
            .max(1)
            .min(usage_queue_worker_database_cap(
                node_role,
                database,
                database_is_isolated,
            ))
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
        database_is_isolated: bool,
    ) -> Option<usize> {
        if let Some(limit) = self.worker_record_concurrency_limit {
            if limit == 0 {
                return None;
            }
            return Some(
                limit
                    .min(MAX_USAGE_QUEUE_WORKERS)
                    .min(
                        usage_worker_record_concurrency_database_cap(
                            node_role,
                            database,
                            database_is_isolated,
                        )
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
        usage_worker_record_concurrency_database_cap(node_role, database, database_is_isolated)
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
            queue_payload_max_bytes: self.queue_payload_max_bytes,
            consumer_batch_size: self.queue_batch_size.max(1),
            consumer_block_ms: self.queue_block_ms.max(1),
            reclaim_idle_ms: self.queue_reclaim_idle_ms.max(1),
            reclaim_count: self.queue_reclaim_count.max(1),
            reclaim_interval_ms: self.queue_reclaim_interval_ms.max(1),
            terminal_submission_max_in_flight: self.terminal_submission_max_in_flight.max(1),
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

    /// Explicitly allow requests when the shared RPM backend is unavailable.
    /// Keep the secure fail-closed behavior as the production default.
    #[arg(long, env = "RATE_LIMIT_FAIL_OPEN", default_value_t = false)]
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
    /// Inspect or prepare the configured database.
    Db(DatabaseCommandArgs),
}

#[derive(ClapArgs, Debug, Clone)]
struct DatabaseCommandArgs {
    #[command(subcommand)]
    command: DatabaseCommand,
}

#[derive(Subcommand, Debug, Clone)]
enum DatabaseCommand {
    /// Show whether schema migrations and data backfills are current.
    Status,
    /// Apply pending schema migrations and data backfills.
    Prepare,
}

#[derive(ClapArgs, Debug, Clone)]
struct DataExportArgs {
    #[arg(long)]
    output: PathBuf,

    /// Atomically replace an existing regular output owned by the current user.
    #[arg(long)]
    overwrite: bool,

    #[arg(long, value_enum, value_delimiter = ',')]
    domains: Vec<ExportDomainArg>,
}

#[derive(ClapArgs, Debug, Clone)]
struct DataImportArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(
        long,
        help = "Preserve passwords and API/management credentials from a trusted import; imported sessions remain revoked. Without this flag identity credentials are revoked."
    )]
    preserve_credentials: bool,
}

#[derive(ClapArgs, Debug, Clone)]
struct DataCopyArgs {
    #[arg(long, value_enum)]
    source_driver: DatabaseDriverArg,

    #[arg(long)]
    source_url: String,

    /// Permit a cleartext source connection for a non-loopback database.
    /// Leave unset to require TLS for remote Postgres URLs.
    #[arg(long)]
    source_allow_insecure: bool,

    #[arg(long, value_enum)]
    target_driver: DatabaseDriverArg,

    #[arg(long)]
    target_url: String,

    /// Permit a cleartext target connection for a non-loopback database.
    /// Leave unset to require TLS for remote Postgres URLs.
    #[arg(long)]
    target_allow_insecure: bool,

    #[arg(long, value_enum, value_delimiter = ',')]
    domains: Vec<ExportDomainArg>,

    #[arg(long)]
    omit_request_body_details: bool,
    #[arg(
        long,
        help = "Preserve passwords and API/management credentials from the trusted source; imported sessions remain revoked. The target must use the source encryption key."
    )]
    preserve_credentials: bool,
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

    #[arg(
        long,
        env = "AETHER_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS",
        default_value_t = DEFAULT_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS
    )]
    /// Maximum time allowed to receive one complete HTTP request header block.
    http_header_read_timeout_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_HTTP_HEADER_MAX_BYTES",
        default_value_t = DEFAULT_GATEWAY_HTTP_HEADER_MAX_BYTES
    )]
    /// Maximum HTTP request header bytes (HTTP/2 uses decompressed list size).
    http_header_max_bytes: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_HTTP_MAX_HEADERS",
        default_value_t = DEFAULT_GATEWAY_HTTP_MAX_HEADERS
    )]
    /// Maximum number of HTTP/1 request header fields.
    http_max_headers: usize,

    #[arg(
        long,
        env = "AETHER_GATEWAY_HTTP_SHUTDOWN_TIMEOUT_MS",
        default_value_t = 30_000
    )]
    /// Grace period for HTTP requests and upgraded connections before forced close.
    http_shutdown_timeout_ms: u64,

    #[arg(
        long,
        env = "AETHER_GATEWAY_USAGE_SHUTDOWN_TIMEOUT_MS",
        default_value_t = 30_000
    )]
    /// Additional time for request finalizers and local usage buffers to persist.
    usage_shutdown_timeout_ms: u64,

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

    #[arg(long, hide = true, default_value_t = false)]
    migrate: bool,

    #[arg(long, hide = true, default_value_t = false)]
    apply_backfills: bool,

    /// Database startup policy. Defaults to auto when neither this nor the legacy setting is set.
    #[arg(long, env = "AETHER_GATEWAY_DATABASE_MODE", value_enum)]
    database_mode: Option<DatabaseModeArg>,

    /// Legacy compatibility switch. Prefer --database-mode.
    #[arg(
        long,
        env = "AETHER_GATEWAY_AUTO_PREPARE_DATABASE",
        hide = true,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    auto_prepare_database: Option<bool>,

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

    /// Maximum accepted HTTP TCP connections across all listener shards, including upgrades.
    /// Unset or 0 follows request plus WebSocket capacity, bounded by the FD allowance.
    #[arg(long, env = "AETHER_GATEWAY_MAX_HTTP_CONNECTIONS")]
    max_http_connections: Option<usize>,

    /// Maximum number of long-lived public WebSocket connections. When unset,
    /// this follows `max_in_flight_requests` while remaining an independent
    /// gate. Set `AETHER_GATEWAY_MAX_WEBSOCKET_CONNECTIONS` to override it.
    #[arg(long, env = "AETHER_GATEWAY_MAX_WEBSOCKET_CONNECTIONS")]
    max_websocket_connections: Option<usize>,

    #[arg(long, env = "AETHER_GATEWAY_DISTRIBUTED_REQUEST_LIMIT")]
    distributed_request_limit: Option<usize>,

    /// Optional distributed limit for long-lived WebSocket connections. When
    /// omitted, the distributed request limit is reused; set it to 0 to keep
    /// WebSocket admission local-only.
    #[arg(long, env = "AETHER_GATEWAY_DISTRIBUTED_WEBSOCKET_CONNECTION_LIMIT")]
    distributed_websocket_connection_limit: Option<usize>,

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
        default_value_t = 2_000
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
    fn effective_database_mode(&self) -> DatabaseModeArg {
        resolve_database_mode(self.database_mode, self.auto_prepare_database)
    }

    fn effective_runtime_backend(
        &self,
        _database: Option<&SqlDatabaseConfig>,
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
        let default_log_filter = "aether_gateway=info,aether_data=info";
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

fn gateway_http_header_read_timeout_ms(timeout_ms: u64) -> u64 {
    timeout_ms.clamp(
        MIN_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS,
        MAX_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS,
    )
}

fn gateway_http_header_max_bytes(bytes: usize) -> usize {
    bytes.clamp(
        MIN_GATEWAY_HTTP_HEADER_MAX_BYTES,
        MAX_GATEWAY_HTTP_HEADER_MAX_BYTES,
    )
}

fn gateway_http_max_headers(headers: usize) -> usize {
    headers.clamp(MIN_GATEWAY_HTTP_MAX_HEADERS, MAX_GATEWAY_HTTP_MAX_HEADERS)
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

#[derive(Clone, Copy)]
struct GatewayHttpLimits {
    http2_max_concurrent_streams: u32,
    http_header_read_timeout_ms: u64,
    http_header_max_bytes: usize,
    http_max_headers: usize,
}

async fn serve_gateway_router(
    listeners: Vec<tokio::net::TcpListener>,
    router: axum::Router,
    connection_budget: Arc<HttpConnectionBudget>,
    limits: GatewayHttpLimits,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error>> {
    let limits = GatewayHttpLimits {
        http2_max_concurrent_streams: gateway_http2_max_concurrent_streams(
            limits.http2_max_concurrent_streams,
        ),
        http_header_read_timeout_ms: gateway_http_header_read_timeout_ms(
            limits.http_header_read_timeout_ms,
        ),
        http_header_max_bytes: gateway_http_header_max_bytes(limits.http_header_max_bytes),
        http_max_headers: gateway_http_max_headers(limits.http_max_headers),
    };
    let mut servers = tokio::task::JoinSet::new();
    for listener in listeners {
        let router = router.clone();
        let connection_budget = Arc::clone(&connection_budget);
        let shutdown = shutdown.clone();
        servers.spawn(async move {
            serve_gateway_listener(listener, router, connection_budget, limits, shutdown).await
        });
    }
    let mut failure = None;
    while let Some(result) = servers.join_next().await {
        let result = result.unwrap_or_else(|err| {
            Err(std::io::Error::other(format!(
                "gateway listener task failed: {err}"
            )))
        });
        if let Err(error) = result {
            failure.get_or_insert(error);
            shutdown.cancel();
            connection_budget.force_close();
        }
    }
    if let Some(error) = failure {
        return Err(error.into());
    }
    // Hyper hands upgrades to application tasks; their IO still owns this budget.
    while connection_budget.snapshot().in_flight != 0 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Ok(())
}

async fn serve_gateway_listener(
    listener: tokio::net::TcpListener,
    router: axum::Router,
    connection_budget: Arc<HttpConnectionBudget>,
    limits: GatewayHttpLimits,
    shutdown: CancellationToken,
) -> Result<(), std::io::Error> {
    let GatewayHttpLimits {
        http2_max_concurrent_streams,
        http_header_read_timeout_ms,
        http_header_max_bytes,
        http_max_headers,
    } = limits;
    let mut make_service = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let (io, remote_addr) = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = connections.join_next(), if !connections.is_empty() => continue,
            accepted = connection_budget.accept(&listener) => accepted,
        };
        let Ok(io) = connection_budget.try_admit(io) else {
            tokio::task::yield_now().await;
            continue;
        };
        let tower_service = make_service
            .call(remote_addr)
            .await
            .unwrap_or_else(|err| match err {})
            .map_request(|req: Request<Incoming>| req.map(Body::new));
        let first_request_gate = GatewayFirstRequestGate::new();
        let hyper_service = TowerToHyperService::new(GatewayFirstRequestService {
            inner: tower_service,
            gate: first_request_gate.clone(),
        });
        let io = TokioIo::new(io);

        let shutdown = shutdown.clone();
        let connection_budget = Arc::clone(&connection_budget);
        connections.spawn(async move {
            let mut builder = HyperServerBuilder::new(TokioExecutor::new());
            // Hyper's HTTP/1 header timer is opt-in when using the custom
            // connection builder. Configure both protocol parsers explicitly:
            // HTTP/1 gets a slow-header deadline and bounded parser buffer;
            // HTTP/2 gets a decompressed header-list limit. The timer is
            // connection metadata protection and does not affect request body
            // streaming or the configured stream concurrency.
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(std::time::Duration::from_millis(
                    http_header_read_timeout_ms,
                ))
                .max_buf_size(http_header_max_bytes)
                .max_headers(http_max_headers);
            builder.http2().enable_connect_protocol();
            builder
                .http2()
                .timer(TokioTimer::new())
                .max_concurrent_streams(http2_max_concurrent_streams)
                .max_header_list_size(u32::try_from(http_header_max_bytes).unwrap_or(u32::MAX));

            // The auto builder reads the HTTP/2 preface before Hyper's H1
            // header timer starts, and H2 has no header-read timer of its own.
            // Race the whole connection until the first valid request reaches
            // the service so a peer cannot hold a socket open while dribbling
            // protocol bytes or an initial header block. Once the gate opens,
            // request and response bodies remain fully streaming.
            let connection = builder.serve_connection_with_upgrades(io, hyper_service);
            tokio::pin!(connection);
            let draining_connection = async {
                tokio::select! {
                    result = &mut connection => result,
                    _ = shutdown.cancelled() => {
                        connection.as_mut().graceful_shutdown();
                        connection.await
                    }
                }
            };
            let connection_result = tokio::select! {
                biased;
                _ = connection_budget.wait_for_forced_close() => Ok(()),
                result = drive_gateway_connection(
                    draining_connection,
                    first_request_gate,
                    std::time::Duration::from_millis(http_header_read_timeout_ms),
                ) => result,
            };
            if let Err(err) = connection_result {
                tracing::trace!(error = ?err, "gateway connection closed with error");
            }
        });
    }
    drop(listener);
    while connections.join_next().await.is_some() {}
    Ok(())
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
        .no_proxy()
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

    if args
        .video_task_store_path
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "AETHER_GATEWAY_VIDEO_TASK_STORE_PATH must be unset when AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY=multi-node; use shared SQL-backed state instead",
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
    let _log_shutdown = aether_runtime::LogShutdownGuard::new();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(GATEWAY_TOKIO_WORKER_STACK_SIZE_BYTES)
        .build()?;
    let result = runtime.block_on(run());
    aether_usage_runtime::shutdown_usage_background_runtime(std::time::Duration::from_secs(5));
    result
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if let Some(command) = args.command.as_ref() {
        init_service_runtime(args.runtime_config()?)?;
        // Data export/import can decrypt and persist sensitive credentials;
        // apply the same encryption-key policy as the normal gateway path
        // before touching the selected database.
        if matches!(command, DataCommand::Export(_) | DataCommand::Import(_)) {
            args.data.validate_encryption_key()?;
        }
        return run_data_command(command, &args.data).await;
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
    args.data.validate_encryption_key()?;
    let data_config = args.data.to_config();
    let isolate_background_database = args.node_role.isolates_background_database();
    let background_database_config = if isolate_background_database {
        data_config.background_database_config()
    } else {
        None
    };
    let usage_database_is_isolated =
        isolate_background_database && background_database_config.is_some();
    let usage_database_config = usage_database_config_for_role(
        args.node_role,
        data_config.database(),
        background_database_config.as_ref(),
    );
    let request_concurrency_limit = args
        .max_in_flight_requests
        .filter(|limit| *limit > 0)
        .unwrap_or_else(automatic_gateway_request_concurrency);
    let websocket_connection_limit = args
        .max_websocket_connections
        .filter(|limit| *limit > 0)
        .unwrap_or(request_concurrency_limit);
    let http_connection_limit = http_connection_limit(
        args.max_http_connections,
        request_concurrency_limit,
        websocket_connection_limit,
        soft_fd_limit(),
    );
    let http_connection_budget = Arc::new(HttpConnectionBudget::new(http_connection_limit));
    let distributed_websocket_connection_limit = match args.distributed_websocket_connection_limit {
        Some(limit) if limit > 0 => Some(limit),
        Some(_) => None,
        None => args.distributed_request_limit.filter(|limit| *limit > 0),
    };
    let usage_queue_request_concurrency_hint = usage_queue_request_concurrency_hint(
        Some(request_concurrency_limit),
        args.distributed_request_limit,
    );
    let usage_queue_request_concurrency_hint_source =
        if args.max_in_flight_requests.is_some() || args.distributed_request_limit.is_some() {
            "explicit"
        } else {
            "auto"
        };
    let usage_queue_workers = args.usage.effective_queue_workers(
        args.node_role,
        Some(request_concurrency_limit),
        args.distributed_request_limit,
        usage_database_config,
        usage_database_is_isolated,
    );
    let usage_queue_worker_max_count = args.usage.effective_queue_worker_max_count(
        args.node_role,
        usage_database_config,
        usage_queue_workers,
        usage_database_is_isolated,
    );
    let usage_worker_record_concurrency_limit =
        args.usage.effective_worker_record_concurrency_limit(
            args.node_role,
            usage_database_config,
            usage_database_is_isolated,
        );
    let usage_config = args.usage.to_config(
        usage_queue_workers,
        usage_queue_worker_max_count,
        usage_worker_record_concurrency_limit,
    );
    let usage_blocking_stream_lanes = args.usage.runtime_state_blocking_stream_lanes(
        args.node_role,
        usage_database_config,
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
        usage_queue_request_concurrency_hint_source,
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
        usage_queue_request_concurrency_hint_source,
        max_in_flight_requests = request_concurrency_limit,
        max_in_flight_requests_source = if args.max_in_flight_requests.is_some() {
            "explicit"
        } else {
            "auto"
        },
        distributed_request_limit = args.distributed_request_limit.unwrap_or_default(),
        max_websocket_connections = websocket_connection_limit,
        max_websocket_connections_source = if args.max_websocket_connections.is_some() {
            "explicit"
        } else {
            "request_concurrency_fallback"
        },
        distributed_websocket_connection_limit =
            distributed_websocket_connection_limit.unwrap_or_default(),
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
        data_postgres_configured = sql_database_config
            .as_ref()
            .is_some_and(|database| database.driver == DatabaseDriver::Postgres),
        runtime_redis_configured = matches!(runtime_backend, RuntimeBackendArg::Redis),
        data_redis_url_supplied = data_redis_url.is_some(),
        data_has_encryption_key = data_config.encryption_key().is_some(),
        data_postgres_require_ssl = args.data.postgres_require_ssl,
        "aether-gateway startup configuration"
    );

    let mut state = AppState::new()?
        .with_runtime_state(runtime_state)
        .with_data_config_and_background_isolation(data_config, isolate_background_database)?
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
    state = state
        .with_request_concurrency_limit(request_concurrency_limit)
        .with_websocket_connection_limit(websocket_connection_limit)
        .with_http_connection_budget(Arc::clone(&http_connection_budget));
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
    if let Some(limit) = distributed_websocket_connection_limit {
        let distributed_gate = state
            .runtime_state()
            .semaphore(
                "gateway_websocket_connections_distributed",
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
        state = state.with_distributed_websocket_connection_gate(distributed_gate);
    }
    if matches!(args.deployment_topology, DeploymentTopologyArg::MultiNode)
        && !state.has_usage_data_writer()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage persistence requires a configured shared SQL data backend; set AETHER_DATABASE_DRIVER and AETHER_DATABASE_URL before starting aether-gateway",
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
    prepare_database_startup_requirements(&state, args.effective_database_mode()).await?;
    state.warm_database_pools().await?;
    let reset_stale_proxy_nodes = state.reset_stale_proxy_node_tunnel_statuses().await?;
    if reset_stale_proxy_nodes > 0 {
        info!(
            reset_stale_proxy_nodes,
            "reset stale tunnel-connected proxy nodes on startup"
        );
    }
    state.bootstrap_admin_from_env().await?;
    match state.ensure_system_default_routing_group().await {
        Ok(Some(group)) => {
            info!(
                group_id = %group.id,
                group_name = %group.name,
                "created system default routing group from routing strategy defaults"
            );
        }
        Ok(None) => {}
        Err(err) => return Err(err.into()),
    }
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
    if state.prewarm_metric_snapshot().await {
        info!("gateway metric snapshot prewarmed");
    } else {
        warn!(
            "gateway metric snapshot prewarm did not complete; continuing with fail-open metrics"
        );
    }
    let listen_backlog = gateway_listen_backlog(args.listen_backlog);
    let listener_shards = gateway_listener_shards(args.listener_shards);
    let listeners = gateway_listeners(bind_addr, listen_backlog, listener_shards)?;
    let public_base_url = resolve_local_http_base_url(app_port)?;
    let frontdoor_health_url = format!("{public_base_url}/_gateway/health");
    let shutdown_state = state.clone();
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
        max_http_connections = http_connection_limit,
        http2_max_concurrent_streams = gateway_http2_max_concurrent_streams(args.http2_max_concurrent_streams),
        public_url = %public_base_url,
        healthcheck_url = %frontdoor_health_url,
        legacy_route_policy = "fail_closed",
        "aether-gateway ready"
    );

    let shutdown = CancellationToken::new();
    let serve_result = {
        let server = serve_gateway_router(
            listeners,
            router,
            Arc::clone(&http_connection_budget),
            GatewayHttpLimits {
                http2_max_concurrent_streams: args.http2_max_concurrent_streams,
                http_header_read_timeout_ms: args.http_header_read_timeout_ms,
                http_header_max_bytes: args.http_header_max_bytes,
                http_max_headers: args.http_max_headers,
            },
            shutdown.clone(),
        );
        tokio::pin!(server);
        tokio::select! {
            result = &mut server => result,
            signal = aether_runtime::wait_for_shutdown_signal() => {
                signal?;
                info!("shutdown signal received, draining gateway requests");
                shutdown.cancel();
                match tokio::time::timeout(
                    std::time::Duration::from_millis(args.http_shutdown_timeout_ms),
                    &mut server,
                ).await {
                    Ok(result) => result,
                    Err(_) => {
                        warn!(
                            event_name = "gateway_http_shutdown_deadline",
                            connections = http_connection_budget.snapshot().in_flight,
                            "HTTP drain deadline reached; closing remaining sockets"
                        );
                        http_connection_budget.force_close();
                        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await {
                            Ok(result) => result,
                            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut,
                                "gateway connection tasks did not stop after forced close").into()),
                        }
                    }
                }
            }
        }
    };
    let usage_result = shutdown_state
        .shutdown_usage_runtime(std::time::Duration::from_millis(
            args.usage_shutdown_timeout_ms,
        ))
        .await;
    if let Some(background_tasks) = background_tasks {
        background_tasks.shutdown().await;
    }
    serve_result?;
    usage_result?;
    info!(
        event_name = "gateway_shutdown_complete",
        "gateway local persistence drained"
    );
    Ok(())
}

async fn run_data_command(
    command: &DataCommand,
    data: &GatewayDataArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        DataCommand::Export(args) => run_data_export(args, data).await,
        DataCommand::Import(args) => run_data_import(args, data).await,
        DataCommand::Copy(args) => run_data_copy(args).await,
        DataCommand::Db(args) => run_database_command(args, data).await,
    }
}

async fn run_database_command(
    args: &DatabaseCommandArgs,
    data: &GatewayDataArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    match args.command {
        DatabaseCommand::Status => run_database_status(data).await,
        DatabaseCommand::Prepare => run_database_prepare(data).await,
    }
}

fn database_maintenance_state(
    data: &GatewayDataArgs,
) -> Result<(DatabaseDriver, AppState), Box<dyn std::error::Error>> {
    let database = required_sql_database_config(data)?;
    let driver = database.driver;
    let state = AppState::new()?.with_data_config(data.to_config())?;
    Ok((driver, state))
}

async fn run_database_status(data: &GatewayDataArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (driver, state) = database_maintenance_state(data)?;
    let pending_migrations = state
        .pending_database_migrations()
        .await?
        .unwrap_or_default();

    if let Some(next) = pending_migrations.first() {
        println!("database {driver}: preparation required");
        println!("pending migrations: {}", pending_migrations.len());
        println!("next migration: {} ({})", next.version, next.description);
        println!("pending backfills: not checked until migrations are current");
        println!("run `aether-gateway db prepare`");
        return Ok(());
    }

    let pending_backfills = state
        .pending_database_backfills()
        .await?
        .unwrap_or_default();
    if let Some(next) = pending_backfills.first() {
        println!("database {driver}: preparation required");
        println!("pending migrations: 0");
        println!("pending backfills: {}", pending_backfills.len());
        println!("next backfill: {} ({})", next.version, next.description);
        println!("run `aether-gateway db prepare`");
        return Ok(());
    }

    println!("database {driver}: ready (schema and backfills are current)");
    Ok(())
}

async fn run_database_prepare(data: &GatewayDataArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (driver, state) = database_maintenance_state(data)?;
    prepare_database_startup_requirements(&state, DatabaseModeArg::Auto).await?;
    println!("database {driver}: ready (schema and backfills are current)");
    Ok(())
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

async fn run_data_export(
    args: &DataExportArgs,
    data: &GatewayDataArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let database = required_sql_database_config(data)?;
    let driver = database.driver;
    let domains = requested_export_domains(args);
    let created_at_unix_secs = current_unix_secs()?;
    let encoded = export_database_jsonl(database, domains, created_at_unix_secs).await?;

    write_atomic_private_export(&args.output, encoded.as_bytes(), args.overwrite)?;
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

fn write_atomic_private_export(path: &Path, bytes: &[u8], overwrite: bool) -> io::Result<()> {
    #[cfg(not(unix))]
    {
        let _ = (path, bytes, overwrite);
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "private atomic database exports currently require Unix filesystem checks",
        ));
    }

    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let file_name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "export path must name a file")
        })?;
        let input_parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent = open_private_export_directory(input_parent)?;
        let output_name = CString::new(file_name.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "export path must not contain an embedded NUL byte",
            )
        })?;

        // Check the target through the already-open parent directory. The
        // later renameat/linkat calls use that same descriptor, so replacing a
        // writable ancestor cannot redirect the export to another directory.
        if let Some(stat) = private_export_stat_at(&parent, &output_name)? {
            let effective_uid = unsafe { libc::geteuid() };
            if stat.st_mode & libc::S_IFMT != libc::S_IFREG
                || stat.st_uid != effective_uid
                || stat.st_nlink != 1
            {
                return Err(io::Error::other(
                    "export output must be a regular, single-link file owned by the current user",
                ));
            }
            if !overwrite {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "export output already exists; pass --overwrite to replace it",
                ));
            }
        }

        let temporary_name = CString::new(format!(
            ".aether-data-export-{}-{}.tmp",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
        .expect("generated temporary export name cannot contain NUL");
        // O_EXCL + O_NOFOLLOW makes creation of the temporary file independent
        // of any attacker-controlled directory entry with the same name.
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                temporary_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut file = unsafe { fs::File::from_raw_fd(descriptor) };
        let result = (|| -> io::Result<()> {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);

            if overwrite {
                // renameat replaces the directory entry and never dereferences
                // a destination symlink. No attacker-selected file is opened
                // or truncated even if the target changed after the check.
                private_export_rename_at(&parent, &temporary_name, &output_name)?;
            } else {
                private_export_link_at(&parent, &temporary_name, &output_name).map_err(
                    |error| {
                        if error.kind() == io::ErrorKind::AlreadyExists {
                            io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                "export output already exists; pass --overwrite to replace it",
                            )
                        } else {
                            error
                        }
                    },
                )?;
                private_export_unlink_at(&parent, &temporary_name)?;
            }
            parent.sync_all()
        })();
        if result.is_err() {
            let _ = private_export_unlink_at(&parent, &temporary_name);
        }
        result
    }
}

#[cfg(unix)]
fn open_private_export_directory(path: &Path) -> io::Result<fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    // Walk one component at a time and retain the final descriptor. We allow
    // trusted system symlink components (for example macOS `/var`), but
    // validate the directory reached by every open before continuing. The
    // descriptor remains pinned even if the symlink is exchanged later.
    let mut directory = fs::File::open(if path.is_absolute() { "/" } else { "." })?;
    validate_private_export_directory_fd(&directory, path)?;
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "export path must not contain '..' components",
                ))
            }
            Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "export path uses an unsupported prefix",
                ))
            }
        };
        let name = CString::new(name.as_bytes()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "export directory path must not contain an embedded NUL byte",
            )
        })?;
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let next = unsafe { fs::File::from_raw_fd(descriptor) };
        validate_private_export_directory_fd(&next, path)?;
        directory = next;
    }
    Ok(directory)
}

#[cfg(unix)]
fn validate_private_export_directory_fd(
    directory: &fs::File,
    display_path: &Path,
) -> io::Result<()> {
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;

    let mut stat = MaybeUninit::<libc::stat>::uninit();
    let result = unsafe { libc::fstat(directory.as_raw_fd(), stat.as_mut_ptr()) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { stat.assume_init() };
    let effective_uid = unsafe { libc::geteuid() };
    let mode = stat.st_mode;
    if mode & libc::S_IFMT != libc::S_IFDIR
        || (stat.st_uid != effective_uid && stat.st_uid != 0)
        || (mode & 0o022 != 0 && mode & 0o1000 == 0)
    {
        return Err(io::Error::other(format!(
            "export output directory '{}' has unsafe ownership or permissions",
            display_path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn private_export_stat_at(
    parent: &fs::File,
    name: &std::ffi::CStr,
) -> io::Result<Option<libc::stat>> {
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;

    let mut stat = MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Ok(Some(unsafe { stat.assume_init() }));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn private_export_link_at(
    parent: &fs::File,
    source: &std::ffi::CStr,
    destination: &std::ffi::CStr,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let result = unsafe {
        libc::linkat(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn private_export_rename_at(
    parent: &fs::File,
    source: &std::ffi::CStr,
    destination: &std::ffi::CStr,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let result = unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            destination.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn private_export_unlink_at(parent: &fs::File, name: &std::ffi::CStr) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

async fn run_data_import(
    args: &DataImportArgs,
    data: &GatewayDataArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let database = required_sql_database_config(data)?;
    let driver = database.driver;
    let input_path = args.input.clone();
    let input = tokio::task::spawn_blocking(move || read_data_import_input(&input_path)).await??;
    if !args.preserve_credentials {
        warn!("identity credentials will be revoked; use --preserve-credentials only for trusted recovery or migration");
    }
    let imported = import_database_jsonl_with_options(
        database,
        &input,
        DataImportOptions {
            preserve_credentials: args.preserve_credentials,
        },
    )
    .await?;

    info!(
        driver = %driver,
        input = %args.input.display(),
        imported,
        preserve_credentials = args.preserve_credentials,
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

/// Read a CLI JSONL import through a descriptor that cannot be redirected by a
/// later path replacement. The parser itself also enforces these limits, but
/// bounding the file read first prevents an oversized input from being held in
/// memory before validation starts.
fn read_data_import_input(path: &Path) -> io::Result<String> {
    read_data_import_input_with_limit(path, MAX_JSONL_INPUT_BYTES)
}

fn read_data_import_input_with_limit(path: &Path, limit: usize) -> io::Result<String> {
    let mut file = open_data_import_file(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "data import input '{}' must be a regular file",
                path.display()
            ),
        ));
    }

    let limit_u64 = u64::try_from(limit).unwrap_or(u64::MAX);
    if metadata.len() > limit_u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "data import input '{}' exceeds the {} byte limit",
                path.display(),
                limit
            ),
        ));
    }

    // A file can grow after metadata() returns. Reading one extra byte catches
    // that race without allowing the input buffer to exceed the configured
    // parser budget.
    let read_limit = limit.saturating_add(1);
    // Do not reserve the whole metadata length: sparse or concurrently grown
    // files can advertise a huge size while containing little data, and a
    // single capacity reservation would otherwise become a local DoS vector.
    const MAX_INITIAL_IMPORT_READ_CAPACITY: usize = 8 * 1024 * 1024;
    let initial_capacity = usize::try_from(metadata.len())
        .unwrap_or(limit)
        .min(limit)
        .min(MAX_INITIAL_IMPORT_READ_CAPACITY);
    let mut bytes = Vec::with_capacity(initial_capacity.min(read_limit));
    // Read in fixed-size chunks instead of `read_to_end`: the latter may use a
    // file's attacker-controlled size hint to reserve a large buffer before
    // the limit check runs. Reserve only the exact next chunk so capacity
    // stays close to the configured `limit + 1` budget.
    let mut chunk = [0_u8; 32 * 1024];
    while bytes.len() < read_limit {
        let remaining = read_limit - bytes.len();
        let chunk_len = remaining.min(chunk.len());
        let read = file.read(&mut chunk[..chunk_len])?;
        if read == 0 {
            break;
        }
        bytes.try_reserve_exact(read).map_err(|error| {
            io::Error::other(format!(
                "data import input buffer allocation failed: {error}"
            ))
        })?;
        bytes.extend_from_slice(&chunk[..read]);
    }
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "data import input '{}' exceeds the {} byte limit",
                path.display(),
                limit
            ),
        ));
    }

    String::from_utf8(bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "data import input '{}' is not valid UTF-8: {error}",
                path.display()
            ),
        )
    })
}

#[cfg(unix)]
fn open_data_import_file(path: &Path) -> io::Result<fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "data import input path must name a file",
        )
    })?;
    let input_parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // Keep the directory descriptor alive through openat. This prevents a
    // concurrent rename of an ancestor from changing which directory is used.
    let parent = open_private_export_directory(input_parent)?;
    let file_name = CString::new(file_name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "data import input path must not contain an embedded NUL byte",
        )
    })?;

    // O_NONBLOCK is important even though imports require regular files:
    // opening a FIFO without it can block before fstat has a chance to reject
    // the special file. O_NOFOLLOW makes the final path component race-free.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            file_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ELOOP) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "data import input '{}' must not be a symbolic link",
                    path.display()
                ),
            ));
        }
        return Err(error);
    }

    // SAFETY: openat returned a new descriptor owned by this function; no
    // other owner exists and File closes it on every return path.
    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
}

#[cfg(not(unix))]
fn open_data_import_file(path: &Path) -> io::Result<fs::File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "data import input '{}' must not be a symbolic link",
                path.display()
            ),
        ));
    }
    fs::OpenOptions::new().read(true).open(path)
}

fn copy_database_host_is_literal_loopback(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let Ok(address) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    match address {
        std::net::IpAddr::V4(address) => address.is_loopback(),
        std::net::IpAddr::V6(address) => {
            address.is_loopback()
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| mapped.is_loopback())
        }
    }
}

fn copy_database_url_is_literal_loopback(
    driver: DatabaseDriver,
    url: &str,
    label: &str,
) -> Result<bool, io::Error> {
    // Parse with the same SQLx driver that will open the pool. This preserves
    // query-parameter overrides such as PostgreSQL `host`/`hostaddr` and
    // PostgreSQL Unix `socket` paths, which URL authority inspection
    // alone would miss.
    match driver {
        DatabaseDriver::Postgres => {
            let options = url
                .parse::<sqlx::postgres::PgConnectOptions>()
                .map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{label} database URL is invalid: {error}"),
                    )
                })?;
            Ok(options.get_socket().is_some()
                || copy_database_host_is_literal_loopback(options.get_host()))
        }
    }
}

fn copy_database_config(
    driver: DatabaseDriverArg,
    url: &str,
    label: &str,
    allow_insecure: bool,
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
    // A loopback exception preserves the existing local-development workflow,
    // while every named/remote SQL host defaults to an encrypted connection.
    // `allow_insecure` is deliberately endpoint-specific so a local source
    // does not silently downgrade a remote target (or vice versa).
    let literal_loopback = copy_database_url_is_literal_loopback(driver, url, label)?;
    let require_ssl = !allow_insecure && !literal_loopback;
    Ok(SqlDatabaseConfig::new(
        driver,
        url,
        SqlPoolConfig {
            require_ssl,
            ..SqlPoolConfig::default()
        },
    )?)
}

async fn run_data_copy(args: &DataCopyArgs) -> Result<(), Box<dyn std::error::Error>> {
    let source = copy_database_config(
        args.source_driver,
        &args.source_url,
        "source",
        args.source_allow_insecure,
    )?;
    let target = copy_database_config(
        args.target_driver,
        &args.target_url,
        "target",
        args.target_allow_insecure,
    )?;
    let source_driver = source.driver;
    let target_driver = target.driver;
    let domains = requested_domains(&args.domains);
    let created_at_unix_secs = current_unix_secs()?;
    if !args.preserve_credentials {
        warn!("identity credentials will be revoked; use --preserve-credentials only for trusted recovery or migration");
    }
    let imported = copy_database_records(
        source,
        target,
        domains,
        created_at_unix_secs,
        DataCopyOptions {
            omit_request_body_details: args.omit_request_body_details,
            preserve_credentials: args.preserve_credentials,
        },
    )
    .await?;

    info!(
        source_driver = %source_driver,
        target_driver = %target_driver,
        imported,
        preserve_credentials = args.preserve_credentials,
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

    args.data.validate_encryption_key()?;
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
    args.data.validate_encryption_key()?;
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
    database_mode: DatabaseModeArg,
) -> Result<(), Box<dyn std::error::Error>> {
    if matches!(database_mode, DatabaseModeArg::VerifyOnly) {
        ensure_database_schema_is_current(state).await?;
        ensure_database_backfills_are_current(state).await?;
        return Ok(());
    }

    info!("database preparation enabled; applying pending migrations and backfills");

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
            "running database migrations during database preparation..."
        );
        if state.run_database_migrations().await? {
            info!("database migrations complete");
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
        "running database backfills during database preparation..."
    );
    if state.run_database_backfills().await? {
        info!("database backfills complete");
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
    let Some(pending) = state.pending_database_migrations().await? else {
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
        "database schema is behind by {} migration(s); next pending migration is {} ({})\nrun `aether-gateway db prepare` before starting the service",
        pending_count, next_version, next_description
    ))
}

fn pending_backfills_error(
    pending_count: usize,
    next_version: i64,
    next_description: &str,
) -> std::io::Error {
    std::io::Error::other(format!(
        "database backfills are behind by {} backfill(s); next pending backfill is {} ({})\nrun `aether-gateway db prepare` before starting the service",
        pending_count, next_version, next_description
    ))
}

#[cfg(test)]
mod tests {
    mod shutdown {
        include!("shutdown_tests.rs");
    }

    use super::{
        automatic_gateway_request_concurrency_for_capacity,
        automatic_gateway_request_concurrency_for_parallelism, automatic_sql_pool_config,
        automatic_sql_pool_config_for_parallelism, automatic_usage_queue_workers_for_parallelism,
        copy_database_config, ensure_database_backfills_are_current,
        ensure_database_schema_is_current, pending_backfills_error, pending_schema_error,
        read_data_import_input_with_limit, resolve_database_mode, resolve_healthcheck_url,
        usage_database_config_for_role, validate_gateway_data_encryption_key,
        write_atomic_private_export, Args, DataCommand, DatabaseCommand, DatabaseDriverArg,
        DatabaseModeArg, DeploymentTopologyArg, GatewayDataArgs, GatewayFrontdoorArgs,
        GatewayLogDestinationArg, GatewayLogFormatArg, GatewayLogRotationArg, GatewayLoggingArgs,
        GatewayRateLimitArgs, GatewayUsageArgs, NodeRoleArg, RuntimeBackendArg,
        VideoTaskTruthSourceArg, DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS,
        DEFAULT_GATEWAY_HTTP_HEADER_MAX_BYTES, DEFAULT_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS,
        DEFAULT_GATEWAY_HTTP_MAX_HEADERS, DEFAULT_GATEWAY_LISTENER_SHARDS,
        DEFAULT_GATEWAY_LISTEN_BACKLOG, MAX_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS,
        MAX_GATEWAY_LISTENER_SHARDS, MAX_GATEWAY_LISTEN_BACKLOG,
        MIN_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS, MIN_GATEWAY_LISTEN_BACKLOG,
    };
    use aether_data::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};
    use aether_gateway::AppState;
    use bytes::Bytes;
    use clap::Parser;
    use http_body_util::{BodyExt, Full};
    use hyper::body::Incoming as HyperIncoming;
    use hyper::{Request as HyperRequest, Response as HyperResponse};
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::conn::auto::Builder as HyperServerBuilder;
    use std::convert::Infallible;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_args() -> Args {
        Args {
            command: None,
            app_port: 8084,
            listen_backlog: DEFAULT_GATEWAY_LISTEN_BACKLOG,
            listener_shards: DEFAULT_GATEWAY_LISTENER_SHARDS,
            http2_max_concurrent_streams: DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS,
            http_header_read_timeout_ms: DEFAULT_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS,
            http_header_max_bytes: DEFAULT_GATEWAY_HTTP_HEADER_MAX_BYTES,
            http_max_headers: DEFAULT_GATEWAY_HTTP_MAX_HEADERS,
            http_shutdown_timeout_ms: 30_000,
            usage_shutdown_timeout_ms: 30_000,
            healthcheck: false,
            healthcheck_timeout_ms: 3_000,
            deployment_topology: DeploymentTopologyArg::SingleNode,
            node_role: NodeRoleArg::All,
            migrate: false,
            apply_backfills: false,
            database_mode: None,
            auto_prepare_database: None,
            static_dir: None,
            video_task_truth_source_mode: VideoTaskTruthSourceArg::PythonSyncReport,
            video_task_poller_interval_ms: 5_000,
            video_task_poller_batch_size: 32,
            video_task_store_path: None,
            max_in_flight_requests: None,
            max_http_connections: None,
            max_websocket_connections: None,
            distributed_request_limit: None,
            distributed_websocket_connection_limit: None,
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
                queue_payload_max_bytes: 1024 * 1024,
                queue_batch_size: 128,
                queue_block_ms: 500,
                queue_reclaim_idle_ms: 60_000,
                queue_reclaim_count: 128,
                queue_reclaim_interval_ms: 5_000,
                terminal_submission_max_in_flight: 1_024,
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
                fail_open: false,
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
    fn clamps_http_header_security_settings_without_touching_stream_concurrency() {
        assert_eq!(
            super::gateway_http_header_read_timeout_ms(0),
            super::MIN_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS
        );
        assert_eq!(
            super::gateway_http_header_read_timeout_ms(u64::MAX),
            super::MAX_GATEWAY_HTTP_HEADER_READ_TIMEOUT_MS
        );
        assert_eq!(
            super::gateway_http_header_max_bytes(1),
            super::MIN_GATEWAY_HTTP_HEADER_MAX_BYTES
        );
        assert_eq!(
            super::gateway_http_header_max_bytes(usize::MAX),
            super::MAX_GATEWAY_HTTP_HEADER_MAX_BYTES
        );
        assert_eq!(
            super::gateway_http_max_headers(0),
            super::MIN_GATEWAY_HTTP_MAX_HEADERS
        );
        assert_eq!(
            super::gateway_http_max_headers(usize::MAX),
            super::MAX_GATEWAY_HTTP_MAX_HEADERS
        );
        assert_eq!(
            super::gateway_http2_max_concurrent_streams(
                super::DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS
            ),
            16_384
        );
    }

    #[test]
    fn auto_gateway_request_concurrency_scales_and_clamps() {
        assert_eq!(
            automatic_gateway_request_concurrency_for_parallelism(1),
            1_024
        );
        assert_eq!(
            automatic_gateway_request_concurrency_for_parallelism(4),
            4_096
        );
        assert_eq!(
            automatic_gateway_request_concurrency_for_parallelism(64),
            65_536
        );
    }

    #[test]
    fn auto_gateway_request_concurrency_respects_fd_budget() {
        assert_eq!(
            automatic_gateway_request_concurrency_for_capacity(64, Some(16_384)),
            8_064
        );
        assert_eq!(
            automatic_gateway_request_concurrency_for_capacity(64, Some(1_024)),
            384
        );
        assert_eq!(
            automatic_gateway_request_concurrency_for_capacity(64, None),
            65_536
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
    fn normal_runtime_config_includes_database_lifecycle_logs() {
        let config = test_args()
            .runtime_config()
            .expect("runtime config should build");
        assert_eq!(
            config.default_log_filter,
            "aether_gateway=info,aether_data=info"
        );
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
        args.auto_prepare_database = Some(true);
        let config = args.runtime_config().expect("runtime config should build");
        assert_eq!(
            config.default_log_filter,
            "aether_gateway=info,aether_data=info"
        );
    }

    #[test]
    fn database_mode_defaults_to_auto_and_preserves_legacy_false() {
        assert_eq!(resolve_database_mode(None, None), DatabaseModeArg::Auto);
        assert_eq!(
            resolve_database_mode(None, Some(false)),
            DatabaseModeArg::VerifyOnly
        );
        assert_eq!(
            resolve_database_mode(Some(DatabaseModeArg::Auto), Some(false)),
            DatabaseModeArg::Auto
        );
    }

    #[test]
    fn parses_database_commands_and_verify_only_mode() {
        let status = Args::try_parse_from(["aether-gateway", "db", "status"])
            .expect("db status should parse");
        assert!(matches!(
            status.command,
            Some(DataCommand::Db(args))
                if matches!(args.command, DatabaseCommand::Status)
        ));

        let verify_only =
            Args::try_parse_from(["aether-gateway", "--database-mode", "verify-only"])
                .expect("verify-only mode should parse");
        assert_eq!(
            verify_only.effective_database_mode(),
            DatabaseModeArg::VerifyOnly
        );

        let legacy_false =
            Args::try_parse_from(["aether-gateway", "--auto-prepare-database=false"])
                .expect("legacy false setting should parse");
        assert_eq!(
            legacy_false.effective_database_mode(),
            DatabaseModeArg::VerifyOnly
        );

        let prepare = Args::try_parse_from(["aether-gateway", "db", "prepare"])
            .expect("db prepare should parse");
        assert!(matches!(
            prepare.command,
            Some(DataCommand::Db(args))
                if matches!(args.command, DatabaseCommand::Prepare)
        ));
    }

    #[test]
    fn database_arguments_are_global_for_database_commands() {
        let before = Args::try_parse_from([
            "aether-gateway",
            "--database-driver",
            "postgres",
            "--database-url",
            "postgres://localhost/before",
            "db",
            "status",
        ])
        .expect("database arguments before db should parse");
        assert_eq!(
            before.data.database_url.as_deref(),
            Some("postgres://localhost/before")
        );

        let after = Args::try_parse_from([
            "aether-gateway",
            "db",
            "prepare",
            "--database-driver",
            "postgres",
            "--database-url",
            "postgres://localhost/after",
        ])
        .expect("database arguments after db prepare should parse");
        assert_eq!(
            after.data.database_url.as_deref(),
            Some("postgres://localhost/after")
        );
    }

    #[test]
    fn postgres_legacy_url_keeps_precedence_over_generic_database_url() {
        let url = super::resolve_database_url(
            Some(DatabaseDriver::Postgres),
            None,
            Some("postgres://legacy/aether".to_string()),
            Some("postgres://generic/aether".to_string()),
        );

        assert_eq!(url.as_deref(), Some("postgres://legacy/aether"));
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
        assert_eq!(two_cpu.max_connections, 32);

        let four_cpu = automatic_sql_pool_config_for_parallelism(DatabaseDriver::Postgres, 4);
        assert_eq!(four_cpu.min_connections, 4);
        assert_eq!(four_cpu.max_connections, 32);

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
    fn gateway_database_pool_isolation_and_usage_capacity_follow_role() {
        assert!(NodeRoleArg::All.isolates_background_database());
        assert!(!NodeRoleArg::Frontdoor.isolates_background_database());
        assert!(!NodeRoleArg::Background.isolates_background_database());

        let database = test_database(DatabaseDriver::Postgres, 20);
        let isolated_background = test_database(DatabaseDriver::Postgres, 4);
        assert_eq!(
            usage_database_config_for_role(
                NodeRoleArg::All,
                Some(&database),
                Some(&isolated_background),
            )
            .expect("all-role usage database")
            .pool
            .max_connections,
            4
        );
        for role in [NodeRoleArg::Frontdoor, NodeRoleArg::Background] {
            assert_eq!(
                usage_database_config_for_role(role, Some(&database), Some(&isolated_background),)
                    .expect("single-pool role usage database")
                    .pool
                    .max_connections,
                20
            );
        }
    }

    #[test]
    fn gateway_usage_queue_payload_limit_preserves_cli_override_and_rejects_zero() {
        let command = <Args as clap::CommandFactory>::command();
        let argument = command
            .get_arguments()
            .find(|argument| argument.get_id() == "queue_payload_max_bytes")
            .expect("usage payload argument must be registered");
        assert_eq!(
            argument.get_env(),
            Some(std::ffi::OsStr::new(
                "AETHER_GATEWAY_USAGE_QUEUE_PAYLOAD_MAX_BYTES"
            ))
        );
        assert_eq!(argument.get_default_values()[0].to_str(), Some("1048576"));
        let args = Args::try_parse_from(["aether-gateway", "--queue-payload-max-bytes", "32768"])
            .expect("explicit usage payload limit should parse");
        let config = args.usage.to_config(4, 8, Some(4));
        assert_eq!(config.queue_payload_max_bytes, 32_768);
        assert!(config.validate().is_ok());

        let mut args = test_args();
        assert_eq!(
            args.usage.to_config(4, 8, Some(4)).queue_payload_max_bytes,
            1024 * 1024
        );
        args.usage.queue_payload_max_bytes = 0;
        let config = args.usage.to_config(4, 8, Some(4));
        assert_eq!(config.queue_payload_max_bytes, 0);
        assert!(config.validate().is_err());
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
            false,
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
            false,
        );

        assert_eq!(workers, 4);
    }

    #[test]
    fn gateway_usage_queue_worker_autoscale_max_uses_database_cap() {
        let mut args = test_args();
        args.usage.queue_workers = None;
        let database = test_database(DatabaseDriver::Postgres, 40);

        let workers = args.usage.effective_queue_workers(
            args.node_role,
            Some(1_024),
            None,
            Some(&database),
            false,
        );
        let max_workers = args.usage.effective_queue_worker_max_count(
            args.node_role,
            Some(&database),
            workers,
            false,
        );

        assert_eq!(workers, 8);
        assert_eq!(max_workers, 10);
    }

    #[test]
    fn gateway_usage_queue_worker_autoscale_max_respects_explicit_override() {
        let mut args = test_args();
        args.usage.queue_workers = None;
        args.usage.queue_worker_max_count = Some(32);
        let database = test_database(DatabaseDriver::Postgres, 200);

        let workers = args.usage.effective_queue_workers(
            args.node_role,
            Some(1_024),
            None,
            Some(&database),
            false,
        );
        let max_workers = args.usage.effective_queue_worker_max_count(
            args.node_role,
            Some(&database),
            workers,
            false,
        );

        assert_eq!(workers, 8);
        assert_eq!(max_workers, 32);
    }

    #[test]
    fn gateway_usage_worker_record_concurrency_defaults_to_pool_reserve_share() {
        let args = test_args();
        let database = test_database(DatabaseDriver::Postgres, 64);

        assert_eq!(
            args.usage.effective_worker_record_concurrency_limit(
                NodeRoleArg::All,
                Some(&database),
                false,
            ),
            Some(8)
        );
        assert_eq!(
            args.usage.effective_worker_record_concurrency_limit(
                NodeRoleArg::Background,
                Some(&database),
                false,
            ),
            Some(16)
        );
    }

    #[test]
    fn gateway_usage_isolated_database_uses_dedicated_capacity_once() {
        let mut args = test_args();
        args.usage.queue_workers = None;
        let database = test_database(DatabaseDriver::Postgres, 64);
        let isolated_background = test_database(DatabaseDriver::Postgres, 8);
        let usage_database = usage_database_config_for_role(
            NodeRoleArg::All,
            Some(&database),
            Some(&isolated_background),
        )
        .expect("isolated usage database");

        let workers = args.usage.effective_queue_workers(
            NodeRoleArg::All,
            Some(5_000),
            None,
            Some(usage_database),
            true,
        );
        let max_workers = args.usage.effective_queue_worker_max_count(
            NodeRoleArg::All,
            Some(usage_database),
            workers,
            true,
        );

        assert_eq!(usage_database.pool.max_connections, 8);
        assert_eq!(workers, 7);
        assert_eq!(max_workers, 7);
        assert_eq!(
            args.usage.effective_worker_record_concurrency_limit(
                NodeRoleArg::All,
                Some(usage_database),
                true,
            ),
            Some(7)
        );
    }

    #[test]
    fn gateway_usage_worker_record_concurrency_can_be_explicitly_disabled() {
        let mut args = test_args();
        args.usage.worker_record_concurrency_limit = Some(0);
        let database = test_database(DatabaseDriver::Postgres, 64);

        assert_eq!(
            args.usage.effective_worker_record_concurrency_limit(
                NodeRoleArg::All,
                Some(&database),
                false,
            ),
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
            false,
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
            false,
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
            false,
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
            false,
        );

        assert_eq!(workers, 10);
    }

    #[test]
    fn gateway_data_pool_explicit_values_override_auto_sizing() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Postgres);
        args.data.database_url = Some("postgres://localhost/aether".to_string());
        args.data.postgres_min_connections = Some(2);
        args.data.postgres_max_connections = Some(8);
        args.data.postgres_acquire_timeout_ms = Some(2_000);

        let database = args
            .data
            .effective_sql_database_config()
            .expect("postgres database config should build");

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
    fn memory_runtime_data_config_keeps_redis_out_of_data_layer() {
        let mut args = test_args();
        args.data.database_driver = Some(DatabaseDriverArg::Postgres);
        args.data.database_url = Some("postgres://localhost/aether".to_string());
        args.data.redis_url = Some("redis://127.0.0.1/0".to_string());

        let config = args.data.to_config();

        assert_eq!(
            config
                .database()
                .expect("database should be configured")
                .driver,
            DatabaseDriver::Postgres
        );
    }

    #[test]
    fn gateway_data_encryption_key_rejects_weak_and_published_values() {
        assert!(validate_gateway_data_encryption_key(None).is_ok());
        assert!(
            validate_gateway_data_encryption_key(Some("0123456789abcdef0123456789abcdef")).is_ok()
        );

        for insecure in [
            "short-secret",
            "change-this-to-another-secure-random-string",
            "change-this-to-a-secure-random-string",
            "dev-encryption-key-do-not-use-in-production",
        ] {
            assert!(
                validate_gateway_data_encryption_key(Some(insecure)).is_err(),
                "accepted insecure key: {insecure}"
            );
        }
    }

    #[test]
    fn data_copy_requires_tls_for_remote_sql_but_preserves_loopback_compatibility() {
        let remote_postgres = copy_database_config(
            DatabaseDriverArg::Postgres,
            "postgres://user:pass@db.example/aether",
            "source",
            false,
        )
        .expect("remote postgres config should build");
        assert!(remote_postgres.pool.require_ssl);

        for url in [
            "postgres://user:pass@localhost/aether",
            "postgres://user:pass@127.42.17.9/aether",
            "postgres://user:pass@[::1]/aether",
            "postgres://user:pass@[::ffff:127.0.0.1]/aether",
        ] {
            let config = copy_database_config(DatabaseDriverArg::Postgres, url, "source", false)
                .expect("literal loopback config should build");
            assert!(
                !config.pool.require_ssl,
                "loopback URL unexpectedly requires TLS: {url}"
            );
        }

        let explicitly_insecure = copy_database_config(
            DatabaseDriverArg::Postgres,
            "postgres://user:pass@db.example/aether",
            "source",
            true,
        )
        .expect("explicit insecure opt-out should build");
        assert!(!explicitly_insecure.pool.require_ssl);
    }

    #[test]
    fn data_copy_tls_policy_is_conservative_for_non_loopback_and_query_hosts() {
        let query_remote = copy_database_config(
            DatabaseDriverArg::Postgres,
            "postgres:///aether?host=db.example",
            "source",
            false,
        )
        .expect("query-host postgres config should build");
        assert!(query_remote.pool.require_ssl);

        let authority_loopback_query_remote = copy_database_config(
            DatabaseDriverArg::Postgres,
            "postgres://user:pass@localhost/aether?host=db.example",
            "source",
            false,
        )
        .expect("query-host override config should build");
        assert!(authority_loopback_query_remote.pool.require_ssl);

        let authority_loopback_hostaddr_remote = copy_database_config(
            DatabaseDriverArg::Postgres,
            "postgres://user:pass@localhost/aether?hostaddr=192.0.2.10",
            "source",
            false,
        )
        .expect("hostaddr override config should build");
        assert!(authority_loopback_hostaddr_remote.pool.require_ssl);

        let query_loopback = copy_database_config(
            DatabaseDriverArg::Postgres,
            "postgres://user:pass@db.example/aether?host=127.0.0.1",
            "source",
            false,
        )
        .expect("query-loopback config should build");
        assert!(!query_loopback.pool.require_ssl);

        let hostless = copy_database_config(
            DatabaseDriverArg::Postgres,
            "postgres:///aether",
            "source",
            false,
        )
        .expect("hostless postgres config should build");
        // SQLx resolves a hostless PostgreSQL URL to its local socket or
        // localhost default; that path is safe to keep plaintext for local
        // development, just like an explicit loopback URL.
        assert!(!hostless.pool.require_ssl);
    }

    #[test]
    fn data_copy_cli_accepts_independent_insecure_opt_outs() {
        let parsed = Args::try_parse_from([
            "aether-gateway",
            "copy",
            "--source-driver",
            "postgres",
            "--source-url",
            "postgres://user:pass@db.example/aether",
            "--source-allow-insecure",
            "--target-driver",
            "postgres",
            "--target-url",
            "postgres://user:pass@db.example/aether",
        ])
        .expect("copy command should parse endpoint-specific TLS flags");

        let Some(DataCommand::Copy(copy)) = parsed.command else {
            panic!("expected copy command");
        };
        assert!(copy.source_allow_insecure);
        assert!(!copy.target_allow_insecure);
        assert!(!copy.preserve_credentials);
    }

    #[test]
    fn data_import_and_copy_require_explicit_credential_preservation() {
        for preserve in [false, true] {
            let mut import_args = vec!["aether-gateway", "import", "--input", "trusted.jsonl"];
            let mut copy_args = vec![
                "aether-gateway",
                "copy",
                "--source-driver",
                "postgres",
                "--source-url",
                "postgres://localhost/source",
                "--target-driver",
                "postgres",
                "--target-url",
                "postgres://localhost/target",
            ];
            if preserve {
                import_args.push("--preserve-credentials");
                copy_args.push("--preserve-credentials");
            }
            let Some(DataCommand::Import(import)) =
                Args::try_parse_from(import_args).unwrap().command
            else {
                panic!("expected import command");
            };
            let Some(DataCommand::Copy(copy)) = Args::try_parse_from(copy_args).unwrap().command
            else {
                panic!("expected copy command");
            };
            assert_eq!(import.preserve_credentials, preserve);
            assert_eq!(copy.preserve_credentials, preserve);
        }
    }

    #[cfg(unix)]
    #[test]
    fn database_export_output_is_private_atomic_and_no_clobber_by_default() {
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

        let root = std::env::temp_dir().join(format!(
            "aether-data-export-output-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = root.join("export.jsonl");

        write_atomic_private_export(&output, b"first\n", false).unwrap();
        let metadata = std::fs::symlink_metadata(&output).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        assert!(write_atomic_private_export(&output, b"second\n", false).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"first\n");

        write_atomic_private_export(&output, b"second\n", true).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"second\n");

        let victim = root.join("victim");
        std::fs::write(&victim, b"known-good").unwrap();
        std::fs::remove_file(&output).unwrap();
        symlink(&victim, &output).unwrap();
        assert!(write_atomic_private_export(&output, b"replace\n", true).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");

        std::fs::remove_file(&output).unwrap();
        std::fs::hard_link(&victim, &output).unwrap();
        assert!(write_atomic_private_export(&output, b"replace\n", true).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"known-good");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn database_export_rejects_an_unsafe_symbolic_link_parent_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = std::env::temp_dir().join(format!(
            "aether-data-export-parent-link-test-{}",
            uuid::Uuid::new_v4()
        ));
        let real_parent = root.join("real");
        let linked_parent = root.join("linked");
        std::fs::create_dir_all(&real_parent).unwrap();
        // A symlink into a directory writable by other users must not become
        // an escape hatch for the private export.
        std::fs::set_permissions(&real_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        symlink(&real_parent, &linked_parent).unwrap();

        let output = linked_parent.join("export.jsonl");
        assert!(write_atomic_private_export(&output, b"must not be written", false).is_err());
        assert!(!real_parent.join("export.jsonl").exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn database_import_input_is_bounded_and_rejects_final_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = std::env::temp_dir().join(format!(
            "aether-data-import-input-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();

        let input = root.join("input.jsonl");
        std::fs::write(&input, b"0123456789").unwrap();
        let error = read_data_import_input_with_limit(&input, 4)
            .expect_err("an input larger than the configured limit must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("4 byte limit"));

        let target = root.join("target.jsonl");
        std::fs::write(&target, b"safe\n").unwrap();
        let link = root.join("input-link.jsonl");
        symlink(&target, &link).unwrap();
        let error = read_data_import_input_with_limit(&link, 1024)
            .expect_err("a final symbolic link must not be followed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("must not be a symbolic link"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn database_import_input_rejects_fifo_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "aether-data-import-fifo-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fifo = root.join("input.fifo");
        let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        let result = unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) };
        assert_eq!(
            result,
            0,
            "mkfifo failed: {}",
            std::io::Error::last_os_error()
        );

        // O_NONBLOCK in open_data_import_file means this call reaches the
        // regular-file check immediately even when no FIFO writer exists.
        let error = read_data_import_input_with_limit(&fifo, 1024)
            .expect_err("FIFO input must be rejected before reading");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("regular file"));

        std::fs::remove_dir_all(root).unwrap();
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
    fn pending_schema_error_mentions_database_prepare_command() {
        let error = pending_schema_error(2, 20260413020000, "squash usage schema split");
        let message = error.to_string();
        assert!(message.contains("database schema is behind by 2 migration(s)"));
        assert!(message.contains("20260413020000"));
        assert!(message.contains("squash usage schema split"));
        assert!(message.contains("aether-gateway db prepare"));
    }

    #[test]
    fn pending_backfills_error_mentions_database_prepare_command() {
        let message = pending_backfills_error(
            1,
            20260422110000,
            "backfill stats aggregate read path support",
        )
        .to_string();
        assert!(message.contains("database backfills are behind by 1 backfill(s)"));
        assert!(message.contains("20260422110000"));
        assert!(message.contains("backfill stats aggregate read path support"));
        assert!(message.contains("aether-gateway db prepare"));
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
        super::prepare_database_startup_requirements(&state, DatabaseModeArg::Auto)
            .await
            .expect("disabled data backend should not block startup");
    }

    #[tokio::test]
    async fn database_prepare_requires_database_url() {
        let data = test_args().data;
        let error = super::run_database_prepare(&data)
            .await
            .expect_err("missing database URL should fail");
        assert!(error
            .to_string()
            .contains("AETHER_DATABASE_DRIVER/AETHER_DATABASE_URL"));
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
    async fn first_request_gate_closes_a_connection_that_never_reaches_service() {
        let gate = super::GatewayFirstRequestGate::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            super::drive_gateway_connection(
                std::future::pending::<Result<(), ()>>(),
                gate,
                std::time::Duration::from_millis(5),
            ),
        )
        .await
        .expect("first-request deadline should fire promptly");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn first_request_gate_does_not_deadline_a_streaming_connection() {
        let gate = super::GatewayFirstRequestGate::new();
        gate.mark_seen();
        // Model a body that remains active after request headers arrive.
        let connection = async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok::<(), ()>(())
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), async move {
            super::drive_gateway_connection(connection, gate, std::time::Duration::from_millis(5))
                .await
        })
        .await
        .expect("streaming connection should finish");
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn first_request_deadline_covers_partial_http1_and_h2_preface() {
        let prefixes: &[&[u8]] = &[
            b"G",
            b"GET / HTTP/1.1\r\nHost: localhost\r\n",
            b"PRI * HTTP/2.0\r\n\r\nSM\r\n",
        ];
        for prefix in prefixes {
            let (mut client, server) = tokio::io::duplex(16 * 1024);
            client
                .write_all(prefix)
                .await
                .expect("fixture prefix should be writable");
            let gate = super::GatewayFirstRequestGate::new();
            let service = tower::service_fn(|_request: HyperRequest<HyperIncoming>| async {
                Ok::<_, Infallible>(HyperResponse::new(Full::new(Bytes::from_static(b"ok"))))
            });
            let mut builder = HyperServerBuilder::new(TokioExecutor::new());
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(std::time::Duration::from_millis(10))
                .max_buf_size(super::MIN_GATEWAY_HTTP_HEADER_MAX_BYTES)
                .max_headers(super::MIN_GATEWAY_HTTP_MAX_HEADERS);
            builder
                .http2()
                .timer(TokioTimer::new())
                .max_concurrent_streams(super::DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS)
                .max_header_list_size(super::MIN_GATEWAY_HTTP_HEADER_MAX_BYTES as u32);

            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                super::drive_gateway_connection(
                    builder.serve_connection_with_upgrades(
                        TokioIo::new(server),
                        super::TowerToHyperService::new(super::GatewayFirstRequestService {
                            inner: service,
                            gate: gate.clone(),
                        }),
                    ),
                    gate,
                    std::time::Duration::from_millis(5),
                ),
            )
            .await
            .expect("partial protocol input should hit the first-request deadline");
            assert!(
                result.is_ok(),
                "deadline should close without a parser error"
            );

            let mut byte = [0u8; 1];
            let read =
                tokio::time::timeout(std::time::Duration::from_secs(1), client.read(&mut byte))
                    .await
                    .expect("timed-out connection should close its peer");
            assert!(matches!(read, Ok(0) | Err(_)));
        }
    }

    #[tokio::test]
    async fn first_request_deadline_does_not_cut_off_a_delayed_http1_body() {
        let (mut client, server) = tokio::io::duplex(16 * 1024);
        let gate = super::GatewayFirstRequestGate::new();
        let (headers_seen_tx, mut headers_seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let service = tower::service_fn(move |request: HyperRequest<HyperIncoming>| {
            let _ = headers_seen_tx.send(());
            async move {
                let body = request
                    .into_body()
                    .collect()
                    .await
                    .expect("test body should decode")
                    .to_bytes();
                Ok::<_, Infallible>(HyperResponse::new(Full::new(body)))
            }
        });
        let mut builder = HyperServerBuilder::new(TokioExecutor::new());
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(std::time::Duration::from_millis(20))
            .max_buf_size(super::MIN_GATEWAY_HTTP_HEADER_MAX_BYTES)
            .max_headers(super::MIN_GATEWAY_HTTP_MAX_HEADERS);
        builder
            .http2()
            .timer(TokioTimer::new())
            .max_concurrent_streams(super::DEFAULT_GATEWAY_HTTP2_MAX_CONCURRENT_STREAMS)
            .max_header_list_size(super::MIN_GATEWAY_HTTP_HEADER_MAX_BYTES as u32);

        let server_task = tokio::spawn(async move {
            super::drive_gateway_connection(
                builder.serve_connection_with_upgrades(
                    TokioIo::new(server),
                    super::TowerToHyperService::new(super::GatewayFirstRequestService {
                        inner: service,
                        gate: gate.clone(),
                    }),
                ),
                gate,
                std::time::Duration::from_millis(20),
            )
            .await
        });
        client
            .write_all(
                b"POST / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 5\r\n\r\n",
            )
            .await
            .expect("request headers should be writable");
        tokio::time::timeout(std::time::Duration::from_secs(1), headers_seen_rx.recv())
            .await
            .expect("request headers should reach the service")
            .expect("service notification should remain available");
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        client
            .write_all(b"hello")
            .await
            .expect("body should remain writable after the header deadline");
        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.read_to_end(&mut response),
        )
        .await
        .expect("streaming response should complete")
        .expect("response should be readable");
        let result = server_task
            .await
            .expect("server connection task should join");
        assert!(result.is_ok());
        assert!(response
            .windows(b"hello".len())
            .any(|window| window == b"hello"));
    }
}
