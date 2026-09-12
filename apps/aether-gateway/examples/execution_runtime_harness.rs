use std::path::PathBuf;

use clap::Parser;
use tracing::info;

use aether_gateway::{serve_execution_runtime_tcp, serve_execution_runtime_unix};
use aether_runtime::{init_service_runtime, ServiceRuntimeConfig};
use aether_runtime_state::{RedisClientConfig, RuntimeSemaphoreConfig, RuntimeState};

#[derive(Parser, Debug)]
#[command(
    name = "execution-runtime-harness",
    about = "Internal execution runtime harness for Aether tests"
)]
struct Args {
    #[arg(
        long,
        env = "AETHER_EXECUTION_RUNTIME_TRANSPORT",
        default_value = "unix_socket"
    )]
    transport: String,

    #[arg(
        long,
        env = "AETHER_EXECUTION_RUNTIME_BIND",
        default_value = "127.0.0.1:5219"
    )]
    bind: String,

    #[arg(
        long,
        env = "AETHER_EXECUTION_RUNTIME_UNIX_SOCKET",
        default_value = "/tmp/aether-execution-runtime/aether-execution-runtime.sock"
    )]
    unix_socket: PathBuf,

    #[arg(long, env = "AETHER_EXECUTION_RUNTIME_MAX_IN_FLIGHT_REQUESTS")]
    max_in_flight_requests: Option<usize>,

    #[arg(long, env = "AETHER_EXECUTION_RUNTIME_DISTRIBUTED_REQUEST_LIMIT")]
    distributed_request_limit: Option<usize>,

    #[arg(long, env = "AETHER_EXECUTION_RUNTIME_DISTRIBUTED_REQUEST_REDIS_URL")]
    distributed_request_redis_url: Option<String>,

    #[arg(
        long,
        env = "AETHER_EXECUTION_RUNTIME_DISTRIBUTED_REQUEST_REDIS_KEY_PREFIX"
    )]
    distributed_request_redis_key_prefix: Option<String>,

    #[arg(
        long,
        env = "AETHER_EXECUTION_RUNTIME_DISTRIBUTED_REQUEST_LEASE_TTL_MS",
        default_value_t = 30_000
    )]
    distributed_request_lease_ttl_ms: u64,

    #[arg(
        long,
        env = "AETHER_EXECUTION_RUNTIME_DISTRIBUTED_REQUEST_RENEW_INTERVAL_MS",
        default_value_t = 10_000
    )]
    distributed_request_renew_interval_ms: u64,

    #[arg(
        long,
        env = "AETHER_EXECUTION_RUNTIME_DISTRIBUTED_REQUEST_COMMAND_TIMEOUT_MS",
        default_value_t = 1_000
    )]
    distributed_request_command_timeout_ms: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _log_shutdown = aether_runtime::LogShutdownGuard::new();
    run()
}

#[tokio::main]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    init_service_runtime(ServiceRuntimeConfig::new(
        "aether-execution-runtime-harness",
        "aether_gateway=info",
    ))?;

    let args = Args::parse();
    let distributed_request_gate = match args.distributed_request_limit.filter(|limit| *limit > 0) {
        Some(limit) => {
            let redis_url = args
                .distributed_request_redis_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                    "AETHER_EXECUTION_RUNTIME_DISTRIBUTED_REQUEST_REDIS_URL is required when distributed request limit is enabled",
                )
                })?;
            let runtime = RuntimeState::redis(
                RedisClientConfig {
                    url: redis_url.to_string(),
                    key_prefix: args
                        .distributed_request_redis_key_prefix
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                },
                Some(args.distributed_request_command_timeout_ms.max(1)),
            )
            .await?;
            Some(runtime.semaphore(
                "execution_runtime_requests_distributed",
                limit,
                RuntimeSemaphoreConfig {
                    lease_ttl_ms: args.distributed_request_lease_ttl_ms.max(1),
                    renew_interval_ms: args.distributed_request_renew_interval_ms.max(1),
                    command_timeout_ms: Some(args.distributed_request_command_timeout_ms.max(1)),
                },
            )?)
        }
        None => None,
    };

    match args.transport.trim().to_ascii_lowercase().as_str() {
        "unix_socket" | "unix" | "uds" => {
            info!(
                socket = %args.unix_socket.display(),
                "aether execution-runtime harness started"
            );
            serve_execution_runtime_unix(
                &args.unix_socket,
                args.max_in_flight_requests,
                distributed_request_gate.clone(),
            )
            .await?;
        }
        "tcp" => {
            info!(
                bind = %args.bind,
                max_in_flight_requests = args.max_in_flight_requests.unwrap_or_default(),
                distributed_request_limit = args.distributed_request_limit.unwrap_or_default(),
                "aether execution-runtime harness started"
            );
            serve_execution_runtime_tcp(
                &args.bind,
                args.max_in_flight_requests,
                distributed_request_gate,
            )
            .await?;
        }
        other => {
            return Err(format!("unsupported execution runtime transport: {other}").into());
        }
    }

    Ok(())
}
