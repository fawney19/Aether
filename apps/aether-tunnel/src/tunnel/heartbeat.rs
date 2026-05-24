//! Tunnel heartbeat: sends metrics over the tunnel, processes ACKs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::registration::client::RemoteConfig;
use crate::runtime;
use crate::state::{AppState, ServerContext, TunnelRequestMetricsSnapshot};

use super::protocol::{Frame, MsgType};
use super::writer::FrameSender;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
static UPGRADE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static NON_ROOT_UPGRADE_WARNED: AtomicBool = AtomicBool::new(false);

enum AckDecision {
    Accept {
        heartbeat_id: u64,
        upgrade_to: Option<String>,
    },
    Ignore,
}

/// Handle for the dispatcher to forward HeartbeatAck frames.
#[derive(Clone)]
pub struct HeartbeatHandle {
    ack_tx: tokio::sync::mpsc::Sender<Bytes>,
}

impl HeartbeatHandle {
    pub async fn on_ack(&self, payload: Bytes) {
        let _ = self.ack_tx.send(payload).await;
    }
}

/// Create a no-op heartbeat handle that silently discards ACKs.
/// Used for non-primary tunnel connections (conn_idx > 0) to avoid
/// duplicating heartbeat ACK processing.
pub fn spawn_noop() -> HeartbeatHandle {
    let (ack_tx, _) = tokio::sync::mpsc::channel::<Bytes>(1);
    // receiver is immediately dropped; on_ack() calls will silently fail
    HeartbeatHandle { ack_tx }
}

#[derive(Debug, Clone, Copy, Default)]
struct HeartbeatSnapshot {
    cumulative: TunnelRequestMetricsSnapshot,
    window: TunnelRequestMetricsSnapshot,
}

#[derive(Debug, Clone, Copy)]
struct PendingHeartbeat {
    heartbeat_id: u64,
    snapshot: HeartbeatSnapshot,
    cumulative: TunnelRequestMetricsSnapshot,
    sent_at: Option<Instant>,
}

/// Spawn the heartbeat task. Returns a handle for forwarding ACKs.
pub fn spawn(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    frame_tx: FrameSender,
    mut shutdown: watch::Receiver<bool>,
) -> HeartbeatHandle {
    let (ack_tx, mut ack_rx) = tokio::sync::mpsc::channel::<Bytes>(4);

    tokio::spawn(async move {
        // Read initial interval from dynamic config (may be updated by remote config).
        let initial_interval = Duration::from_secs(server.dynamic.load().heartbeat_interval);
        let mut current_interval = initial_interval;
        // At most one in-flight heartbeat snapshot is tracked at a time.
        // We keep the last ACKed cumulative snapshot so each payload can
        // report both monotonic totals and the delta since the previous ACK.
        let mut pending: Option<PendingHeartbeat> = None;
        let mut last_acked_snapshot = TunnelRequestMetricsSnapshot::default();
        let mut next_heartbeat_id: u64 = 1;
        let heartbeat_session_id = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );

        // Skip first immediate tick by sleeping first.
        tokio::time::sleep(current_interval).await;

        loop {
            tokio::select! {
                _ = tokio::time::sleep(current_interval) => {
                    let pending_entry = if let Some(entry) = pending {
                        entry
                    } else {
                        let cumulative = server.metrics.snapshot();
                        let id = next_heartbeat_id;
                        next_heartbeat_id = next_heartbeat_id.wrapping_add(1);
                        if next_heartbeat_id == 0 {
                            next_heartbeat_id = 1;
                        }
                        let window = cumulative.delta_since(last_acked_snapshot);
                        let entry = PendingHeartbeat {
                            heartbeat_id: id,
                            snapshot: HeartbeatSnapshot { cumulative, window },
                            cumulative,
                            sent_at: None,
                        };
                        pending = Some(entry);
                        entry
                    };

                    let payload = build_heartbeat_payload(
                        &state,
                        &server,
                        &heartbeat_session_id,
                        pending_entry.heartbeat_id,
                        pending_entry.snapshot
                    ).await;
                    let frame = Frame::control(MsgType::HeartbeatData, payload);
                    if frame_tx.send(frame).await.is_err() {
                        break; // Writer closed
                    }
                    server.tunnel_metrics.record_heartbeat_sent();
                    if let Some(mut entry) = pending {
                        entry.sent_at = Some(Instant::now());
                        pending = Some(entry);
                    }
                    debug!("sent heartbeat data");

                    // Re-read interval from dynamic config (remote config may have
                    // updated it since the last heartbeat).
                    let new_interval = Duration::from_secs(
                        server.dynamic.load().heartbeat_interval
                    );
                    if new_interval != current_interval {
                        debug!(
                            old_secs = current_interval.as_secs(),
                            new_secs = new_interval.as_secs(),
                            "heartbeat interval updated from dynamic config"
                        );
                        current_interval = new_interval;
                    }
                }
                Some(ack_payload) = ack_rx.recv() => {
                    match handle_ack(&server, &ack_payload) {
                        AckDecision::Accept {
                            heartbeat_id: ack_id,
                            upgrade_to,
                        } => {
                            if let Some(entry) = pending {
                                if ack_id == entry.heartbeat_id {
                                    if let Some(sent_at) = entry.sent_at {
                                        server.tunnel_metrics.record_heartbeat_ack(sent_at.elapsed());
                                    }
                                    last_acked_snapshot = entry.cumulative;
                                    pending = None;
                                }
                            }
                            maybe_trigger_upgrade(upgrade_to);
                        }
                        AckDecision::Ignore => {}
                    }
                }
                _ = shutdown.changed() => {
                    debug!("heartbeat task shutting down");
                    break;
                }
            }
        }
    });

    HeartbeatHandle { ack_tx }
}

async fn build_heartbeat_payload(
    state: &AppState,
    server: &ServerContext,
    heartbeat_session_id: &str,
    heartbeat_id: u64,
    snapshot: HeartbeatSnapshot,
) -> Bytes {
    let node_id = server.node_id.read().unwrap().clone();
    let tunnel_snapshot = server.tunnel_metrics.snapshot();
    let recent_errors = server.tunnel_metrics.recent_errors(8);
    let resource_usage = state.resource_monitor.snapshot();

    let cumulative = snapshot.cumulative;
    let window = snapshot.window;
    let cumulative_metrics = serde_json::json!({
        "total_requests": cumulative.total_requests,
        "total_latency_ns": cumulative.total_latency_ns,
        "avg_latency_ms": cumulative.average_latency_ms(),
        "failed_requests": cumulative.failed_requests,
        "dns_failures": cumulative.dns_failures,
        "stream_errors": cumulative.stream_errors,
        "slow_requests": cumulative.slow_requests,
    });
    let window_metrics = serde_json::json!({
        "total_requests": window.total_requests,
        "total_latency_ns": window.total_latency_ns,
        "avg_latency_ms": window.average_latency_ms(),
        "failed_requests": window.failed_requests,
        "dns_failures": window.dns_failures,
        "stream_errors": window.stream_errors,
        "slow_requests": window.slow_requests,
    });
    let local_admission = state.stream_concurrency_snapshot().map(|snapshot| {
        serde_json::json!({
            "limit": snapshot.limit,
            "in_flight": snapshot.in_flight,
            "available_permits": snapshot.available_permits,
            "high_watermark": snapshot.high_watermark,
            "rejected_total": snapshot.rejected,
        })
    });
    let distributed_admission = match state.distributed_stream_concurrency_snapshot().await {
        Ok(Some(snapshot)) => Some(serde_json::json!({
            "limit": snapshot.limit,
            "in_flight": snapshot.in_flight,
            "available_permits": snapshot.available_permits,
            "high_watermark": snapshot.high_watermark,
            "rejected_total": snapshot.rejected,
        })),
        Ok(None) => None,
        Err(err) => Some(serde_json::json!({
            "error": err.to_string(),
        })),
    };
    let admission = match (local_admission, distributed_admission) {
        (None, None) => None,
        (local, distributed) => Some(serde_json::json!({
            "local_streams": local,
            "distributed_streams": distributed,
        })),
    };

    let payload = serde_json::json!({
        "node_id": node_id,
        "heartbeat_session_id": heartbeat_session_id,
        "heartbeat_id": heartbeat_id,
        "heartbeat_interval": server.dynamic.load().heartbeat_interval,
        "active_connections": server.active_connections.load(Ordering::Acquire),
        "total_requests": cumulative.total_requests,
        "avg_latency_ms": cumulative.average_latency_ms(),
        "failed_requests": cumulative.failed_requests,
        "dns_failures": cumulative.dns_failures,
        "stream_errors": cumulative.stream_errors,
        "slow_requests": cumulative.slow_requests,
        "window_total_requests": window.total_requests,
        "window_total_latency_ns": window.total_latency_ns,
        "window_avg_latency_ms": window.average_latency_ms(),
        "window_failed_requests": window.failed_requests,
        "window_dns_failures": window.dns_failures,
        "window_stream_errors": window.stream_errors,
        "window_slow_requests": window.slow_requests,
        "proxy_metrics": {
            "cumulative": cumulative_metrics,
            "window": window_metrics,
        },
        "proxy_metadata": {
            "version": CURRENT_VERSION,
            "admission": admission,
            "resource_usage": resource_usage,
            "tunnel_metrics": {
                "connect_attempts": tunnel_snapshot.connect_attempts,
                "connect_successes": tunnel_snapshot.connect_successes,
                "connect_errors": tunnel_snapshot.connect_errors,
                "disconnects": tunnel_snapshot.disconnects,
                "last_connected_at_unix_secs": tunnel_snapshot.last_connected_at_unix_secs,
                "last_disconnected_at_unix_secs": tunnel_snapshot.last_disconnected_at_unix_secs,
                "last_connected_duration_ms": tunnel_snapshot.last_connected_duration_ms,
                "connected_duration_total_ms": tunnel_snapshot.connected_duration_total_ms,
                "heartbeat_sent": tunnel_snapshot.heartbeat_sent,
                "heartbeat_ack": tunnel_snapshot.heartbeat_ack,
                "heartbeat_rtt_last_ms": tunnel_snapshot.heartbeat_rtt_last_ms,
                "heartbeat_rtt_avg_ms": tunnel_snapshot.heartbeat_rtt_avg_ms(),
                "ws_in_frames": tunnel_snapshot.ws_in_frames,
                "ws_in_bytes": tunnel_snapshot.ws_in_bytes,
                "ws_out_frames": tunnel_snapshot.ws_out_frames,
                "ws_out_bytes": tunnel_snapshot.ws_out_bytes,
                "error_events_total": tunnel_snapshot.error_events_total,
            },
            "recent_tunnel_errors": recent_errors,
        },
    });

    Bytes::from(serde_json::to_vec(&payload).unwrap_or_default())
}

fn handle_ack(server: &ServerContext, payload: &[u8]) -> AckDecision {
    if payload.is_empty() {
        warn!("received empty heartbeat ACK");
        server
            .tunnel_metrics
            .record_error("heartbeat_ack_empty", "received empty heartbeat ACK");
        return AckDecision::Ignore;
    }

    #[derive(serde::Deserialize)]
    struct AckPayload {
        #[serde(default)]
        remote_config: Option<RemoteConfig>,
        #[serde(default)]
        config_version: u64,
        heartbeat_id: u64,
        #[serde(default)]
        upgrade_to: Option<String>,
    }

    match serde_json::from_slice::<AckPayload>(payload) {
        Ok(ack) => {
            if let Some(ref rc) = ack.remote_config {
                runtime::apply_remote_config(&server.dynamic, rc, ack.config_version);
            }
            AckDecision::Accept {
                heartbeat_id: ack.heartbeat_id,
                upgrade_to: ack.upgrade_to.and_then(normalize_upgrade_target),
            }
        }
        Err(e) => {
            warn!(error = %e, "failed to parse heartbeat ACK");
            server
                .tunnel_metrics
                .record_error("heartbeat_ack_parse", &e.to_string());
            AckDecision::Ignore
        }
    }
}

fn normalize_upgrade_target(raw: String) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed
        .strip_prefix("tunnel-v")
        .or_else(|| trimmed.strip_prefix("proxy-v"))
        .unwrap_or(trimmed);
    if normalized == CURRENT_VERSION {
        return None;
    }
    Some(normalized.to_string())
}

fn maybe_trigger_upgrade(version: Option<String>) {
    let Some(target_version) = version else {
        return;
    };
    if !crate::setup::service::is_root() {
        if NON_ROOT_UPGRADE_WARNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            warn!(
                target_version = %target_version,
                "remote upgrade skipped: root privileges are required"
            );
        }
        return;
    }
    if UPGRADE_IN_PROGRESS
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        debug!(target_version = %target_version, "upgrade already in progress, ignoring");
        return;
    }

    tokio::spawn(async move {
        info!(target_version = %target_version, "received remote upgrade instruction");
        match crate::setup::upgrade::perform_upgrade(&target_version).await {
            Ok(()) => {
                info!(target_version = %target_version, "remote upgrade finished");
            }
            Err(e) => {
                warn!(
                    target_version = %target_version,
                    error = %e,
                    "remote upgrade failed"
                );
                UPGRADE_IN_PROGRESS.store(false, Ordering::Release);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, RwLock};

    use arc_swap::ArcSwap;
    use clap::Parser;

    use super::{build_heartbeat_payload, handle_ack, AckDecision, HeartbeatSnapshot};
    use crate::registration::client::AetherClient;
    use crate::runtime::DynamicConfig;
    use crate::state::{AppState, ServerContext, TunnelMetrics, TunnelRequestMetrics};

    fn sample_config() -> Arc<crate::config::Config> {
        Arc::new(crate::config::Config::parse_from([
            "aether-tunnel",
            "--aether-url",
            "https://example.com",
            "--management-token",
            "ae_test",
            "--node-name",
            "tunnel-test",
        ]))
    }

    fn sample_server() -> Arc<ServerContext> {
        let config = sample_config();
        Arc::new(ServerContext {
            server_label: "heartbeat-test".to_string(),
            aether_url: config.aether_url.clone(),
            management_token: config.management_token.clone(),
            tunnel_security: config.tunnel_security,
            tunnel_encryption_key: config.tunnel_encryption_key.clone(),
            node_name: config.node_name.clone(),
            node_id: Arc::new(RwLock::new("node-123".to_string())),
            aether_client: Arc::new(AetherClient::new(
                &config,
                &config.aether_url,
                &config.management_token,
            )),
            dynamic: Arc::new(ArcSwap::from_pointee(DynamicConfig::from_config(&config))),
            active_connections: Arc::new(AtomicU64::new(0)),
            metrics: Arc::new(TunnelRequestMetrics::new()),
            tunnel_metrics: Arc::new(TunnelMetrics::new()),
        })
    }

    fn sample_state(config: Arc<crate::config::Config>) -> AppState {
        let dns_cache = Arc::new(crate::target_filter::DnsCache::new(
            std::time::Duration::from_secs(config.dns_cache_ttl_secs),
            config.dns_cache_capacity,
        ));
        AppState {
            config: Arc::clone(&config),
            dns_cache: Arc::clone(&dns_cache),
            upstream_client_pool: crate::upstream_client::UpstreamClientPool::new(
                config, dns_cache,
            ),
            tunnel_tls_config: Arc::new(crate::tunnel::client::build_tls_config()),
            resource_monitor: Arc::new(crate::hardware::RuntimeResourceMonitor::new()),
            stream_gate: None,
            distributed_stream_gate: None,
        }
    }

    #[test]
    fn heartbeat_ack_requires_heartbeat_id() {
        let server = sample_server();
        let decision = handle_ack(
            &server,
            br#"{"config_version":1,"remote_config":{"heartbeat_interval":9}}"#,
        );

        assert!(matches!(decision, AckDecision::Ignore));
        assert_eq!(server.dynamic.load().heartbeat_interval, 5);
    }

    #[test]
    fn heartbeat_ack_applies_remote_config_with_heartbeat_id() {
        let server = sample_server();
        let decision = handle_ack(
            &server,
            br#"{"heartbeat_id":7,"config_version":1,"remote_config":{"heartbeat_interval":9}}"#,
        );

        assert!(matches!(
            decision,
            AckDecision::Accept {
                heartbeat_id: 7,
                upgrade_to: None
            }
        ));
        assert_eq!(server.dynamic.load().heartbeat_interval, 9);
    }

    #[tokio::test]
    async fn heartbeat_payload_reports_resource_usage_and_tunnel_error_diagnostics() {
        let config = sample_config();
        let server = sample_server();
        server
            .tunnel_metrics
            .record_error("ws_write_error", "IO error: Connection reset by peer");
        let state = sample_state(config);

        let payload = build_heartbeat_payload(
            &state,
            &server,
            "session-1",
            42,
            HeartbeatSnapshot::default(),
        )
        .await;
        let payload: serde_json::Value =
            serde_json::from_slice(&payload).expect("heartbeat payload should be JSON");
        let resource_usage = payload
            .pointer("/proxy_metadata/resource_usage")
            .and_then(serde_json::Value::as_object)
            .expect("resource usage should be reported");
        assert!(resource_usage.contains_key("system_cpu_usage_percent"));
        assert!(resource_usage.contains_key("process_memory_bytes"));

        let recent_error = payload
            .pointer("/proxy_metadata/recent_tunnel_errors/0")
            .and_then(serde_json::Value::as_object)
            .expect("recent tunnel error should be reported");
        assert!(recent_error
            .get("timestamp_unix_ms")
            .and_then(serde_json::Value::as_u64)
            .is_some());
        assert_eq!(
            recent_error
                .get("component")
                .and_then(serde_json::Value::as_str),
            Some("tunnel_write")
        );
        assert_eq!(
            recent_error
                .get("severity")
                .and_then(serde_json::Value::as_str),
            Some("error")
        );
    }
}
