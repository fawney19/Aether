use std::net::SocketAddr;
use std::time::Duration;

use aether_gateway::{
    build_tunnel_runtime_router_with_state, TunnelConnConfig, TunnelControlPlaneClient,
    TunnelRuntimeState,
};
use aether_runtime::{
    init_service_runtime, DistributedConcurrencyGate, RedisDistributedConcurrencyConfig,
    ServiceRuntimeConfig,
};
use clap::Parser;
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "aether-tunnel-runtime-harness",
    about = "Standalone tunnel relay harness backed by aether-gateway tunnel runtime"
)]
struct Args {
    #[arg(
        long,
        default_value = "0.0.0.0:8085",
        env = "AETHER_TUNNEL_STANDALONE_BIND"
    )]
    bind: String,

    #[arg(
        long,
        default_value_t = 15,
        env = "AETHER_TUNNEL_STANDALONE_PING_INTERVAL"
    )]
    ping_interval: u64,

    #[arg(
        long,
        default_value_t = 2048,
        env = "AETHER_TUNNEL_STANDALONE_MAX_STREAMS"
    )]
    max_streams: usize,

    #[arg(
        long,
        default_value_t = 512,
        env = "AETHER_TUNNEL_STANDALONE_OUTBOUND_QUEUE_CAPACITY"
    )]
    outbound_queue_capacity: usize,

    #[arg(
        long,
        default_value = "http://127.0.0.1:8084",
        env = "AETHER_TUNNEL_STANDALONE_APP_BASE_URL"
    )]
    app_base_url: String,

    #[arg(long, env = "AETHER_TUNNEL_STANDALONE_MAX_IN_FLIGHT_REQUESTS")]
    max_in_flight_requests: Option<usize>,

    #[arg(long, env = "AETHER_TUNNEL_STANDALONE_DISTRIBUTED_REQUEST_LIMIT")]
    distributed_request_limit: Option<usize>,

    #[arg(long, env = "AETHER_TUNNEL_STANDALONE_DISTRIBUTED_REQUEST_REDIS_URL")]
    distributed_request_redis_url: Option<String>,

    #[arg(
        long,
        env = "AETHER_TUNNEL_STANDALONE_DISTRIBUTED_REQUEST_REDIS_KEY_PREFIX"
    )]
    distributed_request_redis_key_prefix: Option<String>,

    #[arg(
        long,
        env = "AETHER_TUNNEL_STANDALONE_DISTRIBUTED_REQUEST_LEASE_TTL_MS",
        default_value_t = 30_000
    )]
    distributed_request_lease_ttl_ms: u64,

    #[arg(
        long,
        env = "AETHER_TUNNEL_STANDALONE_DISTRIBUTED_REQUEST_RENEW_INTERVAL_MS",
        default_value_t = 10_000
    )]
    distributed_request_renew_interval_ms: u64,

    #[arg(
        long,
        env = "AETHER_TUNNEL_STANDALONE_DISTRIBUTED_REQUEST_COMMAND_TIMEOUT_MS",
        default_value_t = 1_000
    )]
    distributed_request_command_timeout_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_service_runtime(ServiceRuntimeConfig::new(
        "aether-tunnel-standalone",
        "aether_gateway=info",
    ))?;

    let args = Args::parse();
    let outbound_queue_capacity = args.outbound_queue_capacity.clamp(8, 4096);
    let ping_interval = Duration::from_secs(args.ping_interval);
    let mut state = TunnelRuntimeState::new(
        TunnelControlPlaneClient::new(args.app_base_url),
        TunnelConnConfig {
            ping_interval,
            idle_timeout: Duration::from_secs(0),
            outbound_queue_capacity,
        },
        args.max_streams,
    )
    .with_request_concurrency_limit(args.max_in_flight_requests);

    if let Some(limit) = args.distributed_request_limit.filter(|limit| *limit > 0) {
        let redis_url = args
            .distributed_request_redis_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "AETHER_TUNNEL_STANDALONE_DISTRIBUTED_REQUEST_REDIS_URL is required when distributed request limit is enabled",
                )
            })?;
        state = state.with_distributed_request_gate(DistributedConcurrencyGate::new_redis(
            "tunnel_requests_distributed",
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

    let app = build_tunnel_runtime_router_with_state(state);
    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    info!(bind = %args.bind, "tunnel runtime harness started");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
