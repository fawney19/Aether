//! Tunnel heartbeat: sends metrics over the tunnel, processes ACKs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::registration::client::RemoteConfig;
use crate::runtime;
use crate::state::ServerContext;

use super::protocol::{Frame, MsgType};
use super::writer::FrameSender;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
static UPGRADE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static NON_ROOT_UPGRADE_WARNED: AtomicBool = AtomicBool::new(false);

enum AckDecision {
    Accept {
        heartbeat_id: Option<u64>,
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
/// resetting shared atomic metrics via `swap(0)`.
pub fn spawn_noop() -> HeartbeatHandle {
    let (ack_tx, _) = tokio::sync::mpsc::channel::<Bytes>(1);
    // receiver is immediately dropped; on_ack() calls will silently fail
    HeartbeatHandle { ack_tx }
}

#[derive(Debug, Clone, Copy, Default)]
struct HeartbeatSnapshot {
    requests: u64,
    latency_ns: u64,
    failed: u64,
    dns_failures: u64,
    stream_errors: u64,
}

/// Spawn the heartbeat task. Returns a handle for forwarding ACKs.
pub fn spawn(
    _config: Arc<Config>,
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
        // Snapshot is only cleared after receiving an ACK, which avoids losing
        // interval counters when ACK/frame delivery is temporarily unstable.
        let mut pending: Option<(u64, HeartbeatSnapshot)> = None;
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
                    let (heartbeat_id, snapshot) = if let Some((id, snap)) = pending {
                        (id, snap)
                    } else {
                        let snap = collect_snapshot(&server);
                        let id = next_heartbeat_id;
                        next_heartbeat_id = next_heartbeat_id.wrapping_add(1);
                        if next_heartbeat_id == 0 {
                            next_heartbeat_id = 1;
                        }
                        pending = Some((id, snap));
                        (id, snap)
                    };

                    let payload = build_heartbeat_payload(
                        &server,
                        &heartbeat_session_id,
                        heartbeat_id,
                        snapshot
                    );
                    let frame = Frame::control(MsgType::HeartbeatData, payload);
                    if frame_tx.send(frame).await.is_err() {
                        if let Some((_, snap)) = pending.take() {
                            restore_snapshot(&server, snap);
                        }
                        break; // Writer closed
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
                            if let Some((pending_id, _)) = pending {
                                match ack_id {
                                    Some(id) if id == pending_id => {
                                        pending = None;
                                    }
                                    None => {
                                        // Backward-compatible with servers that don't echo
                                        // heartbeat_id in ACK payload yet.
                                        pending = None;
                                    }
                                    _ => {}
                                }
                            }
                            maybe_trigger_upgrade(upgrade_to);
                        }
                        AckDecision::Ignore => {}
                    }
                }
                _ = shutdown.changed() => {
                    debug!("heartbeat task shutting down");
                    if let Some((_, snap)) = pending.take() {
                        restore_snapshot(&server, snap);
                    }
                    break;
                }
            }
        }
    });

    HeartbeatHandle { ack_tx }
}

fn collect_snapshot(server: &ServerContext) -> HeartbeatSnapshot {
    HeartbeatSnapshot {
        requests: server.metrics.total_requests.swap(0, Ordering::AcqRel),
        latency_ns: server.metrics.total_latency_ns.swap(0, Ordering::AcqRel),
        failed: server.metrics.failed_requests.swap(0, Ordering::AcqRel),
        dns_failures: server.metrics.dns_failures.swap(0, Ordering::AcqRel),
        stream_errors: server.metrics.stream_errors.swap(0, Ordering::AcqRel),
    }
}

fn restore_snapshot(server: &ServerContext, snap: HeartbeatSnapshot) {
    if snap.requests > 0 {
        server
            .metrics
            .total_requests
            .fetch_add(snap.requests, Ordering::Release);
    }
    if snap.latency_ns > 0 {
        server
            .metrics
            .total_latency_ns
            .fetch_add(snap.latency_ns, Ordering::Release);
    }
    if snap.failed > 0 {
        server
            .metrics
            .failed_requests
            .fetch_add(snap.failed, Ordering::Release);
    }
    if snap.dns_failures > 0 {
        server
            .metrics
            .dns_failures
            .fetch_add(snap.dns_failures, Ordering::Release);
    }
    if snap.stream_errors > 0 {
        server
            .metrics
            .stream_errors
            .fetch_add(snap.stream_errors, Ordering::Release);
    }
}

fn build_heartbeat_payload(
    server: &ServerContext,
    heartbeat_session_id: &str,
    heartbeat_id: u64,
    snapshot: HeartbeatSnapshot,
) -> Bytes {
    let node_id = server.node_id.read().unwrap().clone();

    let avg_latency_ms = if snapshot.requests > 0 {
        Some(snapshot.latency_ns as f64 / snapshot.requests as f64 / 1_000_000.0)
    } else {
        None
    };

    let payload = serde_json::json!({
        "node_id": node_id,
        "heartbeat_session_id": heartbeat_session_id,
        "heartbeat_id": heartbeat_id,
        "active_connections": server.active_connections.load(Ordering::Acquire),
        "total_requests": snapshot.requests,
        "avg_latency_ms": avg_latency_ms,
        "failed_requests": snapshot.failed,
        "dns_failures": snapshot.dns_failures,
        "stream_errors": snapshot.stream_errors,
        "proxy_metadata": {
            "version": CURRENT_VERSION,
        },
    });

    Bytes::from(serde_json::to_vec(&payload).unwrap_or_default())
}

fn handle_ack(server: &ServerContext, payload: &[u8]) -> AckDecision {
    if payload.is_empty() {
        return AckDecision::Accept {
            heartbeat_id: None,
            upgrade_to: None,
        };
    }

    #[derive(serde::Deserialize)]
    struct AckPayload {
        #[serde(default)]
        remote_config: Option<RemoteConfig>,
        #[serde(default)]
        config_version: u64,
        #[serde(default)]
        heartbeat_id: Option<u64>,
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
            AckDecision::Ignore
        }
    }
}

fn normalize_upgrade_target(raw: String) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.strip_prefix("proxy-v").unwrap_or(trimmed);
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
