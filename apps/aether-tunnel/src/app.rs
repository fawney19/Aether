//! Application lifecycle: initialization, task orchestration, and shutdown.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use aether_http::{jittered_delay_for_retry, HttpRetryConfig};
use aether_runtime::{
    init_reloadable_service_tracing, prometheus_response, wait_for_shutdown_signal, ConcurrencyGate,
};
use aether_runtime_state::{RedisClientConfig, RuntimeSemaphoreConfig, RuntimeState};
use arc_swap::ArcSwap;
use axum::extract::State as AxumState;
use axum::routing::get;
use axum::{Json, Router};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::config::{
    effective_tunnel_security, validate_tunnel_encryption_key, Config, ServerEntry,
    TunnelPoolSizing,
};
use crate::net;
use crate::registration::client::AetherClient;
use crate::runtime::{self, DynamicConfig};
use crate::state::{AppState, ServerContext, TunnelMetrics, TunnelRequestMetrics};
use crate::upstream_client;
use crate::{hardware, target_filter, tunnel};

type TaskHandles = Arc<Mutex<Vec<JoinHandle<()>>>>;

const AUTO_STREAM_LIMIT_MIN: usize = 16;
// Keep the automatic fallback large enough for real load while still
// protecting tiny nodes from overcommitting by default.
const AUTO_STREAM_LIMIT_MAX: usize = 2048;
// Bias toward throughput: let a 16-core class box auto-land near 2k streams,
// then rely on FD / memory estimates and the hard cap to keep smaller hosts safe.
const AUTO_STREAM_LIMIT_PER_CPU: u64 = 128;
const AUTO_STREAM_LIMIT_MEMORY_MB_PER_STREAM: u64 = 4;
const AUTO_STREAM_LIMIT_ESTIMATED_DIVISOR: u64 = 12;

#[derive(Debug, Clone, Copy)]
struct TunnelPoolPolicy {
    min_connections: usize,
    max_connections: usize,
    max_streams_per_tunnel: usize,
    scale_check_interval: Duration,
    scale_up_threshold_percent: u32,
    scale_down_threshold_percent: u32,
    scale_down_grace: Duration,
}

impl TunnelPoolPolicy {
    fn from_config(config: &Config, sizing: TunnelPoolSizing) -> Self {
        Self {
            min_connections: sizing.initial_connections.max(1) as usize,
            max_connections: sizing
                .max_connections
                .max(sizing.initial_connections)
                .max(1) as usize,
            max_streams_per_tunnel: config.tunnel_max_streams.unwrap_or(128).max(1) as usize,
            scale_check_interval: Duration::from_millis(config.tunnel_scale_check_interval_ms),
            scale_up_threshold_percent: config.tunnel_scale_up_threshold_percent,
            scale_down_threshold_percent: config.tunnel_scale_down_threshold_percent,
            scale_down_grace: Duration::from_secs(config.tunnel_scale_down_grace_secs),
        }
    }

    fn scale_up_high_water_mark(&self) -> u64 {
        occupancy_threshold(self.max_streams_per_tunnel, self.scale_up_threshold_percent)
    }

    fn scale_down_low_water_mark(&self) -> u64 {
        occupancy_threshold(
            self.max_streams_per_tunnel,
            self.scale_down_threshold_percent,
        )
    }
}

struct ManagedTunnel {
    slot_id: usize,
    drain_tx: watch::Sender<bool>,
    handle: JoinHandle<()>,
    draining: bool,
}

#[derive(Clone)]
struct DiagnosticsState {
    state: Arc<AppState>,
    server_contexts: Arc<Mutex<Vec<Arc<ServerContext>>>>,
}

/// Run the full application lifecycle after config has been parsed.
pub async fn run(mut config: Config, servers: Vec<ServerEntry>) -> anyhow::Result<()> {
    config.validate()?;
    init_tracing(&config);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        node_name = %config.node_name,
        server_count = servers.len(),
        "aether-tunnel starting (tunnel mode)"
    );
    if let Some(proxy_url) = config.effective_aether_outbound_proxy_url() {
        if let Ok(proxy) = crate::egress_proxy::UpstreamProxyConfig::parse(proxy_url) {
            info!(
                aether_outbound_proxy_url = %proxy.redacted_url(),
                "Aether control and tunnel egress proxy configured"
            );
        }
    }
    if let Some(proxy_url) = config
        .upstream_proxy_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(proxy) = crate::egress_proxy::UpstreamProxyConfig::parse(proxy_url) {
            info!(
                upstream_proxy_url = %proxy.redacted_url(),
                "provider upstream egress proxy configured"
            );
        }
    }

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

    if config.max_in_flight_streams.is_none() {
        let auto = auto_max_in_flight_streams(&hw_info);
        config.max_in_flight_streams = Some(auto);
        info!(
            max_in_flight_streams = auto,
            "auto-detected max_in_flight_streams from hardware"
        );
    }

    // Auto-detect tunnel_max_streams from hardware if not explicitly set
    if config.tunnel_max_streams.is_none() {
        let stream_limit = config
            .max_in_flight_streams
            .unwrap_or_else(|| auto_max_in_flight_streams(&hw_info));
        let auto = stream_limit.clamp(1, 1024) as u32;
        config.tunnel_max_streams = Some(auto);
        info!(
            tunnel_max_streams = auto,
            "auto-detected tunnel_max_streams from stream admission limit"
        );
    }
    let tunnel_pool_sizing = config.resolve_tunnel_pool_sizing(&hw_info)?;
    let tunnel_pool_policy = TunnelPoolPolicy::from_config(&config, tunnel_pool_sizing);
    info!(
        tunnel_connections_initial = tunnel_pool_policy.min_connections,
        tunnel_connections_max = tunnel_pool_policy.max_connections,
        tunnel_max_streams = tunnel_pool_policy.max_streams_per_tunnel,
        scale_check_interval_ms = tunnel_pool_policy.scale_check_interval.as_millis(),
        scale_up_threshold_percent = tunnel_pool_policy.scale_up_threshold_percent,
        scale_down_threshold_percent = tunnel_pool_policy.scale_down_threshold_percent,
        scale_down_grace_secs = tunnel_pool_policy.scale_down_grace.as_secs(),
        auto_sizing = config.tunnel_connections.is_none(),
        "resolved tunnel pool policy"
    );

    info!(
        max_concurrency = hw_info.estimated_max_concurrency,
        "hardware info collected"
    );

    let dns_cache = Arc::new(target_filter::DnsCache::new(
        Duration::from_secs(config.dns_cache_ttl_secs),
        config.dns_cache_capacity,
    ));

    let config = Arc::new(config);

    // Build a profile-keyed Hyper client pool for tunnel upstream requests.
    let upstream_client_pool =
        upstream_client::UpstreamClientPool::new(Arc::clone(&config), Arc::clone(&dns_cache));
    let resource_monitor = Arc::new(hardware::RuntimeResourceMonitor::new());

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
        let tunnel_security = effective_tunnel_security(
            &entry.aether_url,
            entry.tunnel_security,
            entry.tunnel_encryption_key.as_deref(),
        );
        if tunnel_security == crate::config::TunnelSecurity::NonTlsRequired {
            if entry.aether_url.trim_start().starts_with("http://") {
                warn!(
                    server = %label,
                    url = %entry.aether_url,
                    "secure tunnel frame encryption starts after registration; deliver install and registration credentials over HTTPS or another trusted bootstrap channel"
                );
            }
            let key = entry
                .tunnel_encryption_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("tunnel_encryption_key must be set for secure non-TLS tunnel")
                })?;
            validate_tunnel_encryption_key(key)?;
        }
        match client
            .register(&config, entry, &node_name, &public_ip, Some(&hw_info))
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
        config,
        dns_cache,
        upstream_client_pool,
        tunnel_tls_config,
        resource_monitor,
        stream_gate: None,
        distributed_stream_gate: None,
    };
    if let Some(limit) = state.config.max_in_flight_streams {
        state = state
            .with_stream_concurrency_gate(Arc::new(ConcurrencyGate::new("tunnel_streams", limit)));
    }
    if let Some(limit) = state.config.distributed_stream_limit {
        let redis_url = state
            .config
            .distributed_stream_redis_url
            .clone()
            .expect("distributed stream redis url should be validated");
        let runtime = RuntimeState::redis(
            RedisClientConfig {
                url: redis_url,
                key_prefix: state.config.distributed_stream_redis_key_prefix.clone(),
            },
            Some(state.config.distributed_stream_command_timeout_ms),
        )
        .await?;
        let distributed_gate = runtime.semaphore(
            "tunnel_streams_distributed",
            limit,
            RuntimeSemaphoreConfig {
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

    let diagnostics_handle = if let Some(bind_addr) = state.config.diagnostics_bind {
        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        Some(spawn_diagnostics_server(
            listener,
            DiagnosticsState {
                state: Arc::clone(&state),
                server_contexts: Arc::clone(&server_contexts),
            },
            shutdown_rx.clone(),
        )?)
    } else {
        None
    };

    info!(
        active_servers = server_contexts.lock().await.len(),
        "running in tunnel mode"
    );

    // Spawn tunnel pool manager per server.
    let tunnel_handles: TaskHandles = Arc::new(Mutex::new(Vec::new()));
    let retry_handles: TaskHandles = Arc::new(Mutex::new(Vec::new()));
    for server in server_contexts.lock().await.iter() {
        spawn_tunnel_pool_manager(
            Arc::clone(&state),
            Arc::clone(server),
            tunnel_pool_policy,
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
            tunnel_pool_policy,
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
    if let Some(handle) = diagnostics_handle {
        let _ = handle.await;
    }
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

    info!("aether-tunnel stopped");
    Ok(())
}

fn spawn_diagnostics_server(
    listener: tokio::net::TcpListener,
    diagnostics_state: DiagnosticsState,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<JoinHandle<()>> {
    let bind_addr = listener.local_addr()?;
    let app = Router::new()
        .route("/health", get(diagnostics_health))
        .route("/metrics", get(diagnostics_metrics))
        .route("/stats", get(diagnostics_stats))
        .with_state(diagnostics_state);

    info!(bind = %bind_addr, "tunnel diagnostics server listening");
    Ok(tokio::spawn(async move {
        let graceful_shutdown = async move {
            while !*shutdown.borrow() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
        };
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(graceful_shutdown)
            .await
        {
            error!(error = %error, "tunnel diagnostics server exited with error");
        }
    }))
}

async fn diagnostics_health(
    AxumState(diagnostics): AxumState<DiagnosticsState>,
) -> Json<serde_json::Value> {
    let servers = diagnostics.server_contexts.lock().await.clone();
    let active_connections = servers
        .iter()
        .map(|server| server.active_connections.load(Ordering::Acquire))
        .sum::<u64>();
    let stream_concurrency = diagnostics
        .state
        .stream_concurrency_snapshot()
        .map(concurrency_snapshot_json);
    let distributed_stream_concurrency =
        distributed_stream_concurrency_json(&diagnostics.state).await;

    Json(serde_json::json!({
        "status": "ok",
        "service": "aether-tunnel",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol_version": aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION,
        "server_count": servers.len(),
        "active_connections": active_connections,
        "stream_concurrency": stream_concurrency,
        "distributed_stream_concurrency": distributed_stream_concurrency,
    }))
}

async fn diagnostics_metrics(
    AxumState(diagnostics): AxumState<DiagnosticsState>,
) -> impl axum::response::IntoResponse {
    let mut samples = diagnostics.state.metric_samples().await;
    let servers = diagnostics.server_contexts.lock().await.clone();
    for server in servers {
        samples.extend(server.metric_samples());
    }
    prometheus_response(&samples)
}

async fn diagnostics_stats(
    AxumState(diagnostics): AxumState<DiagnosticsState>,
) -> Json<serde_json::Value> {
    let servers = diagnostics.server_contexts.lock().await.clone();
    let active_connections = servers
        .iter()
        .map(|server| server.active_connections.load(Ordering::Acquire))
        .sum::<u64>();
    let server_stats = servers
        .iter()
        .map(|server| diagnostics_server_stats(server))
        .collect::<Vec<_>>();
    let stream_concurrency = diagnostics
        .state
        .stream_concurrency_snapshot()
        .map(concurrency_snapshot_json);
    let distributed_stream_concurrency =
        distributed_stream_concurrency_json(&diagnostics.state).await;

    Json(serde_json::json!({
        "status": "ok",
        "service": "aether-tunnel",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol_version": aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION,
        "capacities": {
            "max_concurrent_connections": diagnostics.state.config.max_concurrent_connections,
            "max_in_flight_streams": diagnostics.state.config.max_in_flight_streams,
            "distributed_stream_limit": diagnostics.state.config.distributed_stream_limit,
            "tunnel_max_streams": diagnostics.state.config.tunnel_max_streams,
            "tunnel_connections": diagnostics.state.config.tunnel_connections,
            "tunnel_connections_max": diagnostics.state.config.tunnel_connections_max,
            "diagnostics_bind": diagnostics.state.config.diagnostics_bind.map(|addr| addr.to_string()),
        },
        "server_count": servers.len(),
        "active_connections": active_connections,
        "stream_concurrency": stream_concurrency,
        "distributed_stream_concurrency": distributed_stream_concurrency,
        "resource_usage": diagnostics.state.resource_monitor.snapshot(),
        "servers": server_stats,
    }))
}

fn diagnostics_server_stats(server: &ServerContext) -> serde_json::Value {
    let node_id = server.node_id.read().unwrap().clone();
    let dynamic = server.dynamic.load();
    serde_json::json!({
        "server": server.server_label.clone(),
        "node_id": node_id,
        "node_name": dynamic.node_name.clone(),
        "active_connections": server.active_connections.load(Ordering::Acquire),
        "request_metrics": server.metrics.snapshot(),
        "tunnel_metrics": server.tunnel_metrics.snapshot(),
        "recent_tunnel_errors": server.tunnel_metrics.recent_errors(16),
    })
}

fn concurrency_snapshot_json(snapshot: aether_runtime::ConcurrencySnapshot) -> serde_json::Value {
    serde_json::json!({
        "limit": snapshot.limit,
        "in_flight": snapshot.in_flight,
        "available_permits": snapshot.available_permits,
        "high_watermark": snapshot.high_watermark,
        "rejected_total": snapshot.rejected,
    })
}

fn runtime_semaphore_snapshot_json(
    snapshot: aether_runtime_state::RuntimeSemaphoreSnapshot,
) -> serde_json::Value {
    serde_json::json!({
        "limit": snapshot.limit,
        "in_flight": snapshot.in_flight,
        "available_permits": snapshot.available_permits,
        "high_watermark": snapshot.high_watermark,
        "rejected_total": snapshot.rejected,
    })
}

async fn distributed_stream_concurrency_json(state: &AppState) -> Option<serde_json::Value> {
    match state.distributed_stream_concurrency_snapshot().await {
        Ok(Some(snapshot)) => Some(runtime_semaphore_snapshot_json(snapshot)),
        Ok(None) => None,
        Err(error) => Some(serde_json::json!({ "error": error.to_string() })),
    }
}

#[allow(clippy::too_many_arguments)]
async fn spawn_registration_recovery_tasks(
    state: Arc<AppState>,
    server_contexts: Arc<Mutex<Vec<Arc<ServerContext>>>>,
    failed: Vec<(String, ServerEntry)>,
    public_ip: String,
    hw_info: crate::hardware::HardwareInfo,
    tunnel_pool_policy: TunnelPoolPolicy,
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
                tunnel_pool_policy,
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
    tunnel_pool_policy: TunnelPoolPolicy,
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
            .register(
                &state.config,
                &entry,
                &node_name,
                &public_ip,
                Some(&hw_info),
            )
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
                spawn_tunnel_pool_manager(
                    Arc::clone(&state),
                    server,
                    tunnel_pool_policy,
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

fn auto_max_in_flight_streams(hw_info: &crate::hardware::HardwareInfo) -> usize {
    let by_cpu = u64::from(hw_info.cpu_cores.max(1)).saturating_mul(AUTO_STREAM_LIMIT_PER_CPU);
    let by_memory = hw_info
        .total_memory_mb
        .max(AUTO_STREAM_LIMIT_MEMORY_MB_PER_STREAM)
        / AUTO_STREAM_LIMIT_MEMORY_MB_PER_STREAM;
    let by_estimate = hw_info
        .estimated_max_concurrency
        .max(AUTO_STREAM_LIMIT_ESTIMATED_DIVISOR)
        / AUTO_STREAM_LIMIT_ESTIMATED_DIVISOR;
    let raw = by_cpu.min(by_memory).min(by_estimate).max(1);
    usize::try_from(raw)
        .unwrap_or(AUTO_STREAM_LIMIT_MAX)
        .clamp(AUTO_STREAM_LIMIT_MIN, AUTO_STREAM_LIMIT_MAX)
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
        tunnel_security: effective_tunnel_security(
            &entry.aether_url,
            entry.tunnel_security,
            entry.tunnel_encryption_key.as_deref(),
        ),
        tunnel_encryption_key: entry.tunnel_encryption_key.clone(),
        node_name: node_name.to_string(),
        node_id: Arc::new(RwLock::new(node_id)),
        aether_client: client,
        dynamic: Arc::new(ArcSwap::from_pointee(dynamic)),
        active_connections: Arc::new(AtomicU64::new(0)),
        metrics: Arc::new(TunnelRequestMetrics::new()),
        tunnel_metrics: Arc::new(TunnelMetrics::new()),
    })
}

async fn spawn_tunnel_pool_manager(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    policy: TunnelPoolPolicy,
    shutdown: watch::Receiver<bool>,
    tunnel_handles: TaskHandles,
) {
    let handle = tokio::spawn(async move {
        run_tunnel_pool_manager(state, server, policy, shutdown).await;
    });
    tunnel_handles.lock().await.push(handle);
}

async fn run_tunnel_pool_manager(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    policy: TunnelPoolPolicy,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tunnels = BTreeMap::<usize, ManagedTunnel>::new();
    ensure_tunnel_capacity(
        &mut tunnels,
        policy.min_connections,
        &policy,
        &state,
        &server,
        &shutdown,
    );
    let mut ticker = tokio::time::interval(policy.scale_check_interval);
    ticker.tick().await;
    let mut low_load_since: Option<Instant> = None;

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!(server = %server.server_label, "tunnel pool manager shutting down");
                break;
            }
            _ = ticker.tick() => {
                reap_finished_tunnels(&mut tunnels).await;

                let available = tunnels.values().filter(|tunnel| !tunnel.draining).count();
                if available < policy.min_connections {
                    ensure_tunnel_capacity(
                        &mut tunnels,
                        policy.min_connections,
                        &policy,
                        &state,
                        &server,
                        &shutdown,
                    );
                    low_load_since = None;
                    continue;
                }

                let active_connections = server.active_connections.load(Ordering::Acquire);
                let desired_connections = desired_tunnel_connections(active_connections, &policy);
                if desired_connections > available {
                    ensure_tunnel_capacity(
                        &mut tunnels,
                        desired_connections,
                        &policy,
                        &state,
                        &server,
                        &shutdown,
                    );
                    info!(
                        server = %server.server_label,
                        active_connections,
                        available_connections = available,
                        target_connections = desired_connections,
                        "scaled tunnel pool up"
                    );
                    low_load_since = None;
                    continue;
                }

                if should_scale_down(active_connections, available, &policy) {
                    match low_load_since {
                        Some(since) if since.elapsed() >= policy.scale_down_grace => {
                            if request_tunnel_drain(&mut tunnels, policy.min_connections) {
                                info!(
                                    server = %server.server_label,
                                    active_connections,
                                    available_connections = available,
                                    "requested tunnel drain for scale-down"
                                );
                            }
                            low_load_since = None;
                        }
                        None => {
                            low_load_since = Some(Instant::now());
                        }
                        Some(_) => {}
                    }
                } else {
                    low_load_since = None;
                }
            }
        }
    }

    for tunnel in tunnels.values_mut() {
        let _ = tunnel.drain_tx.send(true);
        tunnel.draining = true;
    }
    while !tunnels.is_empty() {
        reap_finished_tunnels(&mut tunnels).await;
        if !tunnels.is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn ensure_tunnel_capacity(
    tunnels: &mut BTreeMap<usize, ManagedTunnel>,
    target_connections: usize,
    policy: &TunnelPoolPolicy,
    state: &Arc<AppState>,
    server: &Arc<ServerContext>,
    shutdown: &watch::Receiver<bool>,
) {
    let target_connections = target_connections.min(policy.max_connections);
    while tunnels.values().filter(|tunnel| !tunnel.draining).count() < target_connections {
        let Some(slot_id) = next_available_tunnel_slot(tunnels, policy.max_connections) else {
            break;
        };
        tunnels.insert(
            slot_id,
            spawn_managed_tunnel(
                Arc::clone(state),
                Arc::clone(server),
                slot_id,
                shutdown.clone(),
            ),
        );
    }
}

fn spawn_managed_tunnel(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    slot_id: usize,
    shutdown: watch::Receiver<bool>,
) -> ManagedTunnel {
    let (drain_tx, drain_rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        tunnel::run(&state, &server, slot_id, shutdown, drain_rx).await;
    });
    ManagedTunnel {
        slot_id,
        drain_tx,
        handle,
        draining: false,
    }
}

async fn reap_finished_tunnels(tunnels: &mut BTreeMap<usize, ManagedTunnel>) {
    let finished_slots = tunnels
        .iter()
        .filter_map(|(slot_id, tunnel)| tunnel.handle.is_finished().then_some(*slot_id))
        .collect::<Vec<_>>();

    for slot_id in finished_slots {
        if let Some(tunnel) = tunnels.remove(&slot_id) {
            let _ = tunnel.handle.await;
        }
    }
}

fn next_available_tunnel_slot(
    tunnels: &BTreeMap<usize, ManagedTunnel>,
    max_connections: usize,
) -> Option<usize> {
    (0..max_connections).find(|slot_id| !tunnels.contains_key(slot_id))
}

fn request_tunnel_drain(
    tunnels: &mut BTreeMap<usize, ManagedTunnel>,
    min_connections: usize,
) -> bool {
    let available = tunnels.values().filter(|tunnel| !tunnel.draining).count();
    if available <= min_connections {
        return false;
    }

    let Some((_, tunnel)) = tunnels
        .iter_mut()
        .rev()
        .find(|(_, tunnel)| tunnel.slot_id != 0 && !tunnel.draining)
    else {
        return false;
    };

    if tunnel.drain_tx.send(true).is_ok() {
        tunnel.draining = true;
        return true;
    }
    false
}

fn desired_tunnel_connections(active_connections: u64, policy: &TunnelPoolPolicy) -> usize {
    let required = div_ceil_u64(active_connections.max(1), policy.scale_up_high_water_mark());
    required.clamp(policy.min_connections as u64, policy.max_connections as u64) as usize
}

fn should_scale_down(
    active_connections: u64,
    available_connections: usize,
    policy: &TunnelPoolPolicy,
) -> bool {
    if available_connections <= policy.min_connections {
        return false;
    }
    active_connections
        <= (available_connections as u64)
            .saturating_sub(1)
            .saturating_mul(policy.scale_down_low_water_mark())
}

fn occupancy_threshold(max_streams_per_tunnel: usize, percent: u32) -> u64 {
    div_ceil_u64(
        (max_streams_per_tunnel as u64).saturating_mul(percent as u64),
        100,
    )
    .max(1)
}

fn div_ceil_u64(value: u64, divisor: u64) -> u64 {
    if divisor == 0 {
        return value;
    }
    value.saturating_add(divisor.saturating_sub(1)) / divisor
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
            .expect("tunnel service runtime config should be valid"),
    )
    .expect("tunnel tracing should initialize");
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
    use axum::routing::{get, post};
    use axum::Router;
    use serde_json::json;

    use crate::config::{
        TunnelLogDestinationArg, TunnelLogRotationArg, DEFAULT_REDIRECT_REPLAY_BUDGET_BYTES,
    };
    use crate::hardware::HardwareInfo;
    use crate::state::AppState as TunnelAppState;
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
                tunnel_security: None,
                tunnel_encryption_key: None,
            },
        )];
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let tunnel_pool_policy =
            TunnelPoolPolicy::from_config(&state.config, sample_tunnel_pool_sizing());

        spawn_registration_recovery_tasks(
            Arc::clone(&state),
            Arc::clone(&server_contexts),
            failed,
            "127.0.0.1".to_string(),
            sample_hardware_info(),
            tunnel_pool_policy,
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

    #[tokio::test]
    async fn diagnostics_routes_report_health_metrics_and_stats() {
        ensure_rustls_provider();

        let state = sample_state(sample_config("https://aether.example.com"));
        let server = sample_registered_server(&state, "server", "node-diagnostics");
        let server_contexts = Arc::new(Mutex::new(vec![server]));
        let router = Router::new()
            .route("/health", get(diagnostics_health))
            .route("/metrics", get(diagnostics_metrics))
            .route("/stats", get(diagnostics_stats))
            .with_state(DiagnosticsState {
                state: Arc::clone(&state),
                server_contexts,
            });
        let port = reserve_local_port().expect("diagnostics port should reserve");
        let handle = spawn_router_on_port(port, router)
            .await
            .expect("diagnostics test server should start");
        let client = reqwest::Client::new();
        let base_url = format!("http://127.0.0.1:{port}");

        let health: serde_json::Value = client
            .get(format!("{base_url}/health"))
            .send()
            .await
            .expect("health request should send")
            .error_for_status()
            .expect("health response should be success")
            .json()
            .await
            .expect("health response should parse");
        assert_eq!(health["status"], "ok");
        assert_eq!(health["service"], "aether-tunnel");
        assert_eq!(health["server_count"], 1);

        let metrics = client
            .get(format!("{base_url}/metrics"))
            .send()
            .await
            .expect("metrics request should send")
            .error_for_status()
            .expect("metrics response should be success")
            .text()
            .await
            .expect("metrics response should read");
        assert!(metrics.contains("service_up{service=\"aether-tunnel\"} 1"));
        assert!(metrics.contains("tunnel_active_connections{server=\"server\"} 0"));

        let stats: serde_json::Value = client
            .get(format!("{base_url}/stats"))
            .send()
            .await
            .expect("stats request should send")
            .error_for_status()
            .expect("stats response should be success")
            .json()
            .await
            .expect("stats response should parse");
        assert_eq!(stats["status"], "ok");
        assert_eq!(stats["protocol_version"], 2);
        assert_eq!(stats["servers"][0]["node_id"], "node-diagnostics");

        handle.abort();
    }

    #[test]
    fn desired_tunnel_connections_expands_when_load_crosses_high_water() {
        let policy = TunnelPoolPolicy {
            min_connections: 1,
            max_connections: 6,
            max_streams_per_tunnel: 1024,
            scale_check_interval: Duration::from_secs(1),
            scale_up_threshold_percent: 70,
            scale_down_threshold_percent: 35,
            scale_down_grace: Duration::from_secs(15),
        };

        assert_eq!(desired_tunnel_connections(1, &policy), 1);
        assert_eq!(desired_tunnel_connections(2_000, &policy), 3);
        assert_eq!(desired_tunnel_connections(5_000, &policy), 6);
    }

    #[test]
    fn should_scale_down_requires_load_to_fit_remaining_tunnels() {
        let policy = TunnelPoolPolicy {
            min_connections: 1,
            max_connections: 6,
            max_streams_per_tunnel: 1024,
            scale_check_interval: Duration::from_secs(1),
            scale_up_threshold_percent: 70,
            scale_down_threshold_percent: 35,
            scale_down_grace: Duration::from_secs(15),
        };

        assert!(!should_scale_down(800, 3, &policy));
        assert!(should_scale_down(600, 3, &policy));
        assert!(!should_scale_down(200, 1, &policy));
    }

    #[test]
    fn auto_stream_limit_is_conservative_for_tiny_nodes() {
        let hw = HardwareInfo {
            cpu_cores: 1,
            total_memory_mb: 183,
            os_info: "test".to_string(),
            fd_limit: 65_535,
            estimated_max_concurrency: 2_000,
        };

        assert_eq!(auto_max_in_flight_streams(&hw), 45);
    }

    #[test]
    fn auto_stream_limit_scales_to_high_band_on_mid_size_nodes() {
        let hw = HardwareInfo {
            cpu_cores: 16,
            total_memory_mb: 65_536,
            os_info: "test".to_string(),
            fd_limit: 1_048_576,
            estimated_max_concurrency: 500_000,
        };

        assert_eq!(auto_max_in_flight_streams(&hw), AUTO_STREAM_LIMIT_MAX);
    }

    #[test]
    fn auto_stream_limit_caps_large_nodes() {
        let hw = HardwareInfo {
            cpu_cores: 64,
            total_memory_mb: 262_144,
            os_info: "test".to_string(),
            fd_limit: 1_048_576,
            estimated_max_concurrency: 500_000,
        };

        assert_eq!(auto_max_in_flight_streams(&hw), AUTO_STREAM_LIMIT_MAX);
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

    fn sample_tunnel_pool_sizing() -> TunnelPoolSizing {
        TunnelPoolSizing {
            initial_connections: 1,
            max_connections: 1,
        }
    }

    fn sample_state(config: Config) -> Arc<TunnelAppState> {
        let config = Arc::new(config);
        let dns_cache = Arc::new(DnsCache::new(Duration::from_secs(60), 128));
        let upstream_client_pool =
            upstream_client::UpstreamClientPool::new(Arc::clone(&config), Arc::clone(&dns_cache));
        Arc::new(TunnelAppState {
            config,
            dns_cache,
            upstream_client_pool,
            tunnel_tls_config: Arc::new(crate::tunnel::client::build_tls_config()),
            resource_monitor: Arc::new(crate::hardware::RuntimeResourceMonitor::new()),
            stream_gate: None,
            distributed_stream_gate: None,
        })
    }

    fn sample_registered_server(
        state: &Arc<TunnelAppState>,
        label: &str,
        node_id: &str,
    ) -> Arc<ServerContext> {
        let entry = ServerEntry {
            aether_url: state.config.aether_url.clone(),
            management_token: state.config.management_token.clone(),
            node_name: Some(state.config.node_name.clone()),
            tunnel_security: Some(state.config.tunnel_security),
            tunnel_encryption_key: state.config.tunnel_encryption_key.clone(),
        };
        let client = Arc::new(AetherClient::new(
            &state.config,
            &state.config.aether_url,
            &state.config.management_token,
        ));
        build_server_context(
            &state.config,
            label,
            &entry,
            client,
            &state.config.node_name,
            node_id.to_string(),
        )
    }

    fn sample_config(aether_url: &str) -> Config {
        Config {
            aether_url: aether_url.to_string(),
            management_token: "token".to_string(),
            public_ip: None,
            node_name: "tunnel-test".to_string(),
            tunnel_security: crate::config::TunnelSecurity::Off,
            tunnel_encryption_key: None,
            node_region: None,
            heartbeat_interval: 1,
            allowed_ports: vec![80, 443],
            allow_private_targets: false,
            aether_request_timeout_secs: 10,
            aether_connect_timeout_secs: 2,
            aether_pool_max_idle_per_host: 8,
            aether_pool_idle_timeout_secs: 90,
            aether_tcp_keepalive_secs: 60,
            aether_tcp_nodelay: true,
            aether_http2: true,
            aether_outbound_proxy_url: None,
            aether_retry_max_attempts: 1,
            aether_retry_base_delay_ms: 50,
            aether_retry_max_delay_ms: 100,
            diagnostics_bind: None,
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
            upstream_client_pool_capacity: crate::config::DEFAULT_UPSTREAM_CLIENT_POOL_CAPACITY,
            upstream_tcp_keepalive_secs: 60,
            upstream_tcp_nodelay: true,
            upstream_proxy_url: None,
            redirect_replay_budget_bytes: DEFAULT_REDIRECT_REPLAY_BUDGET_BYTES,
            emit_proxy_timing_header: true,
            log_level: "info".to_string(),
            log_destination: TunnelLogDestinationArg::Stdout,
            log_dir: None,
            log_rotation: TunnelLogRotationArg::Daily,
            log_retention_days: 7,
            log_max_files: 30,
            tunnel_reconnect_base_ms: 50,
            tunnel_reconnect_max_ms: 250,
            tunnel_ping_interval_ms: 1_000,
            tunnel_max_streams: Some(8),
            tunnel_connect_timeout_ms: 2_000,
            tunnel_ipv4_only: false,
            tunnel_ipv6_only: false,
            tunnel_tcp_keepalive_secs: 30,
            tunnel_tcp_nodelay: true,
            tunnel_stale_timeout_ms: 5_000,
            tunnel_connections: Some(1),
            tunnel_connections_max: Some(1),
            tunnel_scale_check_interval_ms: 1_000,
            tunnel_scale_up_threshold_percent: 70,
            tunnel_scale_down_threshold_percent: 35,
            tunnel_scale_down_grace_secs: 15,
        }
    }

    fn ensure_rustls_provider() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }
}
