mod control_plane;
mod hub;
mod local_relay;
mod protocol;
mod proxy_conn;

use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use tracing::{info, warn};

use crate::control_plane::ControlPlaneClient;
use crate::hub::{ConnConfig, HubRouter};
use crate::local_relay::relay_request;

#[derive(Parser, Debug)]
#[command(name = "aether-hub", about = "Tunnel Hub for Aether")]
struct Args {
    /// Bind address
    #[arg(long, default_value = "0.0.0.0:8085", env = "TUNNEL_HUB_BIND")]
    bind: String,

    /// Proxy-side idle timeout in seconds (0 to disable)
    #[arg(long, default_value_t = 0, env = "TUNNEL_HUB_PROXY_IDLE_TIMEOUT")]
    proxy_idle_timeout: u64,

    /// Ping interval in seconds (for both sides)
    #[arg(long, default_value_t = 15, env = "TUNNEL_HUB_PING_INTERVAL")]
    ping_interval: u64,

    /// Max concurrent streams per proxy connection
    #[arg(long, default_value_t = 2048, env = "TUNNEL_HUB_MAX_STREAMS")]
    max_streams: usize,

    /// Per-connection outbound queue capacity before treating the socket as congested
    #[arg(
        long,
        default_value_t = 128,
        env = "TUNNEL_HUB_OUTBOUND_QUEUE_CAPACITY"
    )]
    outbound_queue_capacity: usize,

    /// Local Aether app base URL for control-plane callbacks
    #[arg(
        long,
        default_value = "http://127.0.0.1:8084",
        env = "TUNNEL_HUB_APP_BASE_URL"
    )]
    app_base_url: String,
}

#[derive(Clone)]
pub struct AppState {
    pub hub: std::sync::Arc<HubRouter>,
    pub proxy_conn_cfg: ConnConfig,
    pub max_streams: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aether_hub=info".into()),
        )
        .init();

    let args = Args::parse();

    let hub = HubRouter::new(ControlPlaneClient::new(args.app_base_url));
    let outbound_queue_capacity = args.outbound_queue_capacity.clamp(8, 4096);
    let ping_interval = Duration::from_secs(args.ping_interval);
    let state = AppState {
        hub,
        proxy_conn_cfg: ConnConfig {
            ping_interval,
            idle_timeout: Duration::from_secs(args.proxy_idle_timeout),
            outbound_queue_capacity,
        },
        max_streams: args.max_streams,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/stats", get(stats))
        .route("/proxy", get(ws_proxy))
        .route("/local/relay/{node_id}", post(relay_request))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    info!(bind = %args.bind, "aether-hub started");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP endpoints
// ---------------------------------------------------------------------------

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.hub.stats())
}

// ---------------------------------------------------------------------------
// WebSocket endpoints
// ---------------------------------------------------------------------------

async fn ws_proxy(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let node_id = headers
        .get("x-node-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();

    let node_name = headers
        .get("x-node-name")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(&node_id)
        .trim()
        .to_string();

    let max_streams: usize = headers
        .get("x-tunnel-max-streams")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(state.max_streams)
        .clamp(64, 2048);

    if node_id.is_empty() {
        warn!("proxy connection rejected: missing X-Node-ID header");
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }

    ws.max_frame_size(64 * 1024 * 1024)
        .on_upgrade(move |socket| {
            proxy_conn::handle_proxy_connection(
                socket,
                state.hub,
                node_id,
                node_name,
                max_streams,
                state.proxy_conn_cfg,
            )
        })
        .into_response()
}
