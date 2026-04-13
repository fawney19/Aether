//! Application lifecycle: initialization, task orchestration, and shutdown.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use aether_http::{jittered_delay_for_retry, HttpRetryConfig};
use aether_runtime::{
    init_reloadable_service_tracing, wait_for_shutdown_signal, ConcurrencyGate,
    DistributedConcurrencyGate, RedisDistributedConcurrencyConfig,
};
use arc_swap::ArcSwap;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::config::{Config, ServerEntry};
use crate::net;
use crate::registration::client::AetherClient;
use crate::runtime::{self, DynamicConfig};
use crate::state::{AppState, ProxyMetrics, ServerContext};
use crate::upstream_client;
use crate::{hardware, target_filter, tunnel};

type TaskHandles = Arc<Mutex<Vec<JoinHandle<()>>>>;

/// Run the full application lifecycle after config has been parsed.
pub async fn run(mut config: Config, servers: Vec<ServerEntry>) -> anyhow::Result<()> {
    config.validate()?;
    init_tracing(&config);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        node_name = %config.node_name,
        server_count = servers.len(),
        "aether-proxy starting (tunnel mode)"
    );

    // Resolve public IP (best-effort for region info)
    let public_ip = match &config.public_ip {
        Some(ip) => ip.clone(),
        None => net::detect_public_ip()
            .await
            .unwrap_or_else(|_| "0.0.0.0".to_string()),
    };

    // Auto-detect region if not configured
    if config.node_region.is_none() {
        if let Some(region) = net::detect_region(&public_ip).await {
            config.node_region = Some(region);
        }
    }

    // Collect hardware info (once at startup, sent during registration)
    let hw_info = hardware::collect();

    // Auto-detect tunnel_max_streams from hardware if not explicitly set
    if config.tunnel_max_streams.is_none() {
        let auto = (hw_info.estimated_max_concurrency / 10).clamp(64, 1024) as u32;
        config.tunnel_max_streams = Some(auto);
        info!(
            tunnel_max_streams = auto,
            "auto-detected tunnel_max_streams from hardware"
        );
    }

    info!(
        max_concurrency = hw_info.estimated_max_concurrency,
        "hardware info collected"
    );

    let dns_cache = Arc::new(target_filter::DnsCache::new(
        Duration::from_secs(config.dns_cache_ttl_secs),
        config.dns_cache_capacity,
    ));

    // Build Hyper client for tunnel upstream requests (shared).
    // DNS still flows through validated addresses from DnsCache, while the
    // custom connector exposes per-request connect/TLS timing when available.
    let upstream_client = upstream_client::build_upstream_client(&config, Arc::clone(&dns_cache));
    let upstream_http1_client =
        upstream_client::build_http1_only_upstream_client(&config, Arc::clone(&dns_cache));

    // Register with each Aether server and build per-server contexts.
    // Wrapped in Arc<Mutex> so retry_failed_registrations can append later.
    let server_contexts: Arc<Mutex<Vec<Arc<ServerContext>>>> = Arc::new(Mutex::new(Vec::new()));
    let mut failed_entries: Vec<(String, ServerEntry)> = Vec::new();
    for (i, entry) in servers.iter().enumerate() {
        let label = if servers.len() == 1 {
            "server".to_string()
        } else {
            format!("server-{}", i)
        };
        let node_name = entry
            .node_name
            .clone()
            .unwrap_or_else(|| config.node_name.clone());
        let client = Arc::new(AetherClient::new(
            &config,
            &entry.aether_url,
            &entry.management_token,
        ));
        match client
            .register(&config, &node_name, &public_ip, Some(&hw_info))
            .await
        {
            Ok(node_id) => {
                info!(server = %label, node_id = %node_id, url = %entry.aether_url, node_name = %node_name, "registered");
                server_contexts.lock().await.push(build_server_context(
                    &config, &label, entry, client, &node_name, node_id,
                ));
            }
            Err(e) => {
                warn!(
                    server = %label,
                    url = %entry.aether_url,
                    error = %e,
                    "registration failed, will retry in background"
                );
                failed_entries.push((label, entry.clone()));
            }
        }
    }

    let ctx_count = server_contexts.lock().await.len();
    if ctx_count == 0 && failed_entries.is_empty() {
        anyhow::bail!("no servers configured");
    }
    if ctx_count == 0 {
        warn!(
            failed_servers = failed_entries.len(),
            "no servers registered successfully at startup; continuing with background recovery"
        );
    }

    // Build shared application state
    let tunnel_tls_config = Arc::new(crate::tunnel::client::build_tls_config());
    let mut state = AppState {
        config: Arc::new(config),
        dns_cache,
        upstream_client,
        upstream_http1_client,
        tunnel_tls_config,
        stream_gate: None,
        distributed_stream_gate: None,
    };
    if let Some(limit) = state.config.max_in_flight_streams {
        state = state
            .with_stream_concurrency_gate(Arc::new(ConcurrencyGate::new("proxy_streams", limit)));
    }
    if let Some(limit) = state.config.distributed_stream_limit {
        let redis_url = state
            .config
            .distributed_stream_redis_url
            .clone()
            .expect("distributed stream redis url should be validated");
        let distributed_gate = DistributedConcurrencyGate::new_redis(
            "proxy_streams_distributed",
            limit,
            RedisDistributedConcurrencyConfig {
                url: redis_url,
                key_prefix: state.config.distributed_stream_redis_key_prefix.clone(),
                lease_ttl_ms: state.config.distributed_stream_lease_ttl_ms,
                renew_interval_ms: state.config.distributed_stream_renew_interval_ms,
                command_timeout_ms: Some(state.config.distributed_stream_command_timeout_ms),
            },
        )?;
        state = state.with_distributed_stream_concurrency_gate(Arc::new(distributed_gate));
    }
    let state = Arc::new(state);

    // Shutdown signal channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    info!(
        active_servers = server_contexts.lock().await.len(),
        "running in tunnel mode"
    );

    // Spawn tunnel connections per server (pool_size connections each)
    let pool_size = state.config.tunnel_connections.max(1) as usize;
    let tunnel_handles: TaskHandles = Arc::new(Mutex::new(Vec::new()));
    let retry_handles: TaskHandles = Arc::new(Mutex::new(Vec::new()));
    for server in server_contexts.lock().await.iter() {
        spawn_tunnel_pool(
            Arc::clone(&state),
            Arc::clone(server),
            pool_size,
            shutdown_rx.clone(),
            Arc::clone(&tunnel_handles),
        )
        .await;
    }

    // Spawn background retry for failed server registrations
    if !failed_entries.is_empty() {
        spawn_registration_recovery_tasks(
            Arc::clone(&state),
            Arc::clone(&server_contexts),
            failed_entries,
            public_ip.clone(),
            hw_info.clone(),
            pool_size,
            shutdown_rx.clone(),
            Arc::clone(&tunnel_handles),
            Arc::clone(&retry_handles),
        )
        .await;
    }

    // Wait for shutdown signal
    wait_for_shutdown().await;
    info!("shutdown signal received, cleaning up...");
    let _ = shutdown_tx.send(true);
    await_all_handles(&retry_handles).await;

    // Graceful unregister from all servers (including retry-registered ones)
    for server in server_contexts.lock().await.iter() {
        let node_id = server.node_id.read().unwrap().clone();
        if let Err(e) = server.aether_client.unregister(&node_id).await {
            error!(
                server = %server.server_label,
                error = %e,
                "unregister failed during shutdown"
            );
        }
    }

    // Wait for all tunnel tasks
    await_all_handles(&tunnel_handles).await;

    info!("aether-proxy stopped");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn spawn_registration_recovery_tasks(
    state: Arc<AppState>,
    server_contexts: Arc<Mutex<Vec<Arc<ServerContext>>>>,
    failed: Vec<(String, ServerEntry)>,
    public_ip: String,
    hw_info: crate::hardware::HardwareInfo,
    pool_size: usize,
    shutdown: watch::Receiver<bool>,
    tunnel_handles: TaskHandles,
    retry_handles: TaskHandles,
) {
    let mut handles = Vec::with_capacity(failed.len());
    for (label, entry) in failed {
        let retry_state = Arc::clone(&state);
        let retry_contexts = Arc::clone(&server_contexts);
        let retry_public_ip = public_ip.clone();
        let retry_hw_info = hw_info.clone();
        let retry_shutdown = shutdown.clone();
        let retry_tunnels = Arc::clone(&tunnel_handles);
        handles.push(tokio::spawn(async move {
            retry_failed_registration(
                retry_state,
                retry_contexts,
                label,
                entry,
                retry_public_ip,
                retry_hw_info,
                pool_size,
                retry_shutdown,
                retry_tunnels,
            )
            .await;
        }));
    }
    retry_handles.lock().await.extend(handles);
}

/// Background task that retries registration for a single server until either
/// registration succeeds or shutdown is requested.
#[allow(clippy::too_many_arguments)]
async fn retry_failed_registration(
    state: Arc<AppState>,
    server_contexts: Arc<Mutex<Vec<Arc<ServerContext>>>>,
    label: String,
    entry: ServerEntry,
    public_ip: String,
    hw_info: crate::hardware::HardwareInfo,
    pool_size: usize,
    mut shutdown: watch::Receiver<bool>,
    tunnel_handles: TaskHandles,
) {
    let node_name = entry
        .node_name
        .clone()
        .unwrap_or_else(|| state.config.node_name.clone());
    let client = Arc::new(AetherClient::new(
        &state.config,
        &entry.aether_url,
        &entry.management_token,
    ));
    let retry_policy = registration_retry_policy(&state.config);
    let mut attempt = 0u32;

    loop {
        if *shutdown.borrow() {
            info!(server = %label, "shutdown during registration retry");
            return;
        }

        attempt = attempt.saturating_add(1);
        let delay = jittered_delay_for_retry(retry_policy, attempt.saturating_sub(1));

        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = shutdown.changed() => {
                info!(server = %label, "shutdown during registration retry");
                return;
            }
        }

        match client
            .register(&state.config, &node_name, &public_ip, Some(&hw_info))
            .await
        {
            Ok(node_id) => {
                info!(server = %label, node_id = %node_id, attempt, "registration retry succeeded");
                let server = build_server_context(
                    &state.config,
                    &label,
                    &entry,
                    client,
                    &node_name,
                    node_id,
                );
                server_contexts.lock().await.push(Arc::clone(&server));
                spawn_tunnel_pool(
                    Arc::clone(&state),
                    server,
                    pool_size,
                    shutdown,
                    tunnel_handles,
                )
                .await;
                return;
            }
            Err(e) => {
                let next_delay =
                    jittered_delay_for_retry(retry_policy, attempt.min(u32::MAX.saturating_sub(1)));
                warn!(
                    server = %label,
                    attempt,
                    next_delay_ms = next_delay.as_millis(),
                    error = %e,
                    "registration retry failed"
                );
            }
        }
    }
}

fn registration_retry_policy(config: &Config) -> HttpRetryConfig {
    HttpRetryConfig {
        max_attempts: u32::MAX,
        base_delay_ms: config.aether_retry_base_delay_ms,
        max_delay_ms: config.aether_retry_max_delay_ms,
    }
    .normalized()
}

fn build_server_context(
    config: &Config,
    label: &str,
    entry: &ServerEntry,
    client: Arc<AetherClient>,
    node_name: &str,
    node_id: String,
) -> Arc<ServerContext> {
    let mut dynamic = DynamicConfig::from_config(config);
    dynamic.node_name = node_name.to_string();
    Arc::new(ServerContext {
        server_label: label.to_string(),
        aether_url: entry.aether_url.clone(),
        management_token: entry.management_token.clone(),
        node_name: node_name.to_string(),
        node_id: Arc::new(RwLock::new(node_id)),
        aether_client: client,
        dynamic: Arc::new(ArcSwap::from_pointee(dynamic)),
        active_connections: Arc::new(AtomicU64::new(0)),
        metrics: Arc::new(ProxyMetrics::new()),
    })
}

async fn spawn_tunnel_pool(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    pool_size: usize,
    shutdown: watch::Receiver<bool>,
    tunnel_handles: TaskHandles,
) {
    let mut handles = Vec::with_capacity(pool_size);
    for conn_idx in 0..pool_size {
        let s = Arc::clone(&state);
        let srv = Arc::clone(&server);
        let rx = shutdown.clone();
        handles.push(tokio::spawn(async move {
            tunnel::run(&s, &srv, conn_idx, rx).await;
        }));
    }
    tunnel_handles.lock().await.extend(handles);
}

async fn await_all_handles(handles: &TaskHandles) {
    let mut pending = handles.lock().await.drain(..).collect::<Vec<_>>();
    while let Some(handle) = pending.pop() {
        let _ = handle.await;
    }
}

fn init_tracing(config: &Config) {
    let reloader = init_reloadable_service_tracing(
        &config.log_level,
        config
            .service_runtime_config()
            .expect("proxy service runtime config should be valid"),
    )
    .expect("proxy tracing should initialize");
    runtime::set_log_reloader(reloader);
}

async fn wait_for_shutdown() {
    wait_for_shutdown_signal()
        .await
        .expect("failed to install shutdown signal handler");
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Once;

    use axum::extract::State as AxumState;
    use axum::http::StatusCode as AxumStatusCode;
    use axum::routing::post;
    use axum::Router;
    use serde_json::json;

    use crate::config::{
        ProxyLogDestinationArg, ProxyLogRotationArg, DEFAULT_REDIRECT_REPLAY_BUDGET_BYTES,
    };
    use crate::hardware::HardwareInfo;
    use crate::state::AppState as ProxyAppState;
    use crate::target_filter::DnsCache;

    use super::*;

    #[tokio::test]
    async fn registration_recovery_survives_all_startup_failures_and_connects_later() {
        ensure_rustls_provider();

        let gateway_port = reserve_local_port().expect("gateway port should reserve");
        let gateway_base_url = format!("http://127.0.0.1:{gateway_port}");
        let state = sample_state(sample_config(&gateway_base_url));
        let server_contexts: Arc<Mutex<Vec<Arc<ServerContext>>>> = Arc::new(Mutex::new(Vec::new()));
        let tunnel_handles: TaskHandles = Arc::new(Mutex::new(Vec::new()));
        let retry_handles: TaskHandles = Arc::new(Mutex::new(Vec::new()));
        let register_hits = Arc::new(AtomicUsize::new(0));
        let failed = vec![(
            "server".to_string(),
            ServerEntry {
                aether_url: gateway_base_url.clone(),
                management_token: "token".to_string(),
                node_name: Some("node-recovery".to_string()),
            },
        )];
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        spawn_registration_recovery_tasks(
            Arc::clone(&state),
            Arc::clone(&server_contexts),
            failed,
            "127.0.0.1".to_string(),
            sample_hardware_info(),
            1,
            shutdown_rx.clone(),
            Arc::clone(&tunnel_handles),
            Arc::clone(&retry_handles),
        )
        .await;

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            server_contexts.lock().await.is_empty(),
            "registration should still be pending while gateway is down"
        );

        let gateway_handle =
            start_fake_gateway_on_port_retry(gateway_port, Arc::clone(&register_hits))
                .await
                .expect("gateway should start");

        let server = wait_for_registered_server(&server_contexts).await;
        assert_eq!(server.server_label, "server");
        assert_eq!(server.node_id.read().unwrap().as_str(), "node-recovery");
        assert!(
            register_hits.load(Ordering::SeqCst) >= 1,
            "fake control plane should observe at least one register request"
        );

        let _ = shutdown_tx.send(true);
        await_all_handles(&retry_handles).await;
        await_all_handles(&tunnel_handles).await;
        gateway_handle.abort();
    }

    async fn wait_for_registered_server(
        server_contexts: &Arc<Mutex<Vec<Arc<ServerContext>>>>,
    ) -> Arc<ServerContext> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(server) = server_contexts.lock().await.first().cloned() {
                return server;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "server context did not appear after registration recovery"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn start_fake_gateway_on_port_retry(
        port: u16,
        register_hits: Arc<AtomicUsize>,
    ) -> Result<tokio::task::JoinHandle<()>, std::io::Error> {
        let mut attempts = 0usize;
        loop {
            match start_fake_gateway_on_port(port, Arc::clone(&register_hits)).await {
                Ok(server) => return Ok(server),
                Err(err) => {
                    attempts += 1;
                    if attempts >= 20 {
                        return Err(err);
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    async fn start_fake_gateway_on_port(
        port: u16,
        register_hits: Arc<AtomicUsize>,
    ) -> Result<tokio::task::JoinHandle<()>, std::io::Error> {
        let router = Router::new()
            .route("/api/admin/proxy-nodes/register", post(fake_register))
            .with_state(register_hits);
        spawn_router_on_port(port, router).await
    }

    async fn spawn_router_on_port(
        port: u16,
        app: Router,
    ) -> Result<tokio::task::JoinHandle<()>, std::io::Error> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        Ok(tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .expect("gateway test server should run");
        }))
    }

    fn reserve_local_port() -> Result<u16, std::io::Error> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        Ok(port)
    }

    async fn fake_register(
        AxumState(register_hits): AxumState<Arc<AtomicUsize>>,
    ) -> (AxumStatusCode, axum::Json<serde_json::Value>) {
        register_hits.fetch_add(1, Ordering::SeqCst);
        (
            AxumStatusCode::OK,
            axum::Json(json!({ "node_id": "node-recovery" })),
        )
    }

    fn sample_hardware_info() -> HardwareInfo {
        HardwareInfo {
            cpu_cores: 2,
            total_memory_mb: 2048,
            os_info: "test".to_string(),
            fd_limit: 1024,
            estimated_max_concurrency: 512,
        }
    }

    fn sample_state(config: Config) -> Arc<ProxyAppState> {
        let config = Arc::new(config);
        let dns_cache = Arc::new(DnsCache::new(Duration::from_secs(60), 128));
        let upstream_client =
            upstream_client::build_upstream_client(&config, Arc::clone(&dns_cache));
        let upstream_http1_client =
            upstream_client::build_http1_only_upstream_client(&config, Arc::clone(&dns_cache));
        Arc::new(ProxyAppState {
            config,
            dns_cache,
            upstream_client,
            upstream_http1_client,
            tunnel_tls_config: Arc::new(crate::tunnel::client::build_tls_config()),
            stream_gate: None,
            distributed_stream_gate: None,
        })
    }

    fn sample_config(aether_url: &str) -> Config {
        Config {
            aether_url: aether_url.to_string(),
            management_token: "token".to_string(),
            public_ip: None,
            node_name: "proxy-test".to_string(),
            node_region: None,
            heartbeat_interval: 1,
            allowed_ports: vec![80, 443],
            aether_request_timeout_secs: 10,
            aether_connect_timeout_secs: 2,
            aether_pool_max_idle_per_host: 8,
            aether_pool_idle_timeout_secs: 90,
            aether_tcp_keepalive_secs: 60,
            aether_tcp_nodelay: true,
            aether_http2: true,
            aether_retry_max_attempts: 1,
            aether_retry_base_delay_ms: 50,
            aether_retry_max_delay_ms: 100,
            max_concurrent_connections: None,
            max_in_flight_streams: None,
            distributed_stream_limit: None,
            distributed_stream_redis_url: None,
            distributed_stream_redis_key_prefix: None,
            distributed_stream_lease_ttl_ms: 30_000,
            distributed_stream_renew_interval_ms: 10_000,
            distributed_stream_command_timeout_ms: 1_000,
            dns_cache_ttl_secs: 60,
            dns_cache_capacity: 128,
            upstream_connect_timeout_secs: 30,
            upstream_pool_max_idle_per_host: 4,
            upstream_pool_idle_timeout_secs: 60,
            upstream_tcp_keepalive_secs: 60,
            upstream_tcp_nodelay: true,
            redirect_replay_budget_bytes: DEFAULT_REDIRECT_REPLAY_BUDGET_BYTES,
            log_level: "info".to_string(),
            log_destination: ProxyLogDestinationArg::Stdout,
            log_dir: None,
            log_rotation: ProxyLogRotationArg::Daily,
            log_retention_days: 7,
            log_max_files: 30,
            tunnel_reconnect_base_ms: 50,
            tunnel_reconnect_max_ms: 250,
            tunnel_ping_interval_secs: 1,
            tunnel_max_streams: Some(8),
            tunnel_connect_timeout_secs: 2,
            tunnel_tcp_keepalive_secs: 30,
            tunnel_tcp_nodelay: true,
            tunnel_stale_timeout_secs: 5,
            tunnel_connections: 1,
        }
    }

    fn ensure_rustls_provider() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }
}
