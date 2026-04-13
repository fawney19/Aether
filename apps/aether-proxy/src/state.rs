//! Shared application state passed to all subsystems.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use aether_runtime::{
    AdmissionPermit, ConcurrencyError, ConcurrencyGate, ConcurrencySnapshot,
    DistributedConcurrencyError, DistributedConcurrencyGate, DistributedConcurrencySnapshot,
};

use crate::config::Config;
use crate::registration::client::AetherClient;
use crate::runtime::SharedDynamicConfig;
use crate::target_filter::DnsCache;
use crate::upstream_client::UpstreamClient;

/// Central application state shared across all servers/tunnels.
pub struct AppState {
    pub config: Arc<Config>,
    /// DNS cache for upstream target resolution (shared).
    pub dns_cache: Arc<DnsCache>,
    /// Hyper client for tunnel upstream requests with validated DNS and connection timing.
    pub upstream_client: UpstreamClient,
    /// Dedicated Hyper client that forces HTTP/1.1 for upstreams that break on H2/ALPN.
    pub upstream_http1_client: UpstreamClient,
    /// Shared TLS config for tunnel WebSocket connections (avoids re-parsing root CAs on each reconnect).
    pub tunnel_tls_config: Arc<rustls::ClientConfig>,
    /// Optional per-process stream admission gate.
    pub stream_gate: Option<Arc<ConcurrencyGate>>,
    /// Optional cross-instance stream admission gate.
    pub distributed_stream_gate: Option<Arc<DistributedConcurrencyGate>>,
}

/// Per-server state: one instance per Aether server connection.
pub struct ServerContext {
    /// Human-readable label for logging (e.g. "server-0").
    pub server_label: String,
    /// Aether server URL for this connection.
    pub aether_url: String,
    /// Management token for this server.
    pub management_token: String,
    /// Resolved node name at registration time (per-server override or global fallback).
    /// After startup, the active node_name is read from `dynamic` (may be updated remotely).
    #[allow(dead_code)]
    pub node_name: String,
    /// Node ID assigned by this Aether server.
    pub node_id: Arc<RwLock<String>>,
    /// API client for this server.
    pub aether_client: Arc<AetherClient>,
    /// Dynamic config from this server's heartbeat ACKs.
    pub dynamic: SharedDynamicConfig,
    /// Per-server active connection count.
    pub active_connections: Arc<AtomicU64>,
    /// Per-server request/latency metrics.
    pub metrics: Arc<ProxyMetrics>,
}

/// Aggregate metrics for reporting to Aether.
pub struct ProxyMetrics {
    pub total_requests: AtomicU64,
    /// Cumulative connection-establishment latency in nanoseconds
    /// (DNS + TCP/TLS + TTFB, excludes response body streaming).
    pub total_latency_ns: AtomicU64,
    pub failed_requests: AtomicU64,
    pub dns_failures: AtomicU64,
    pub stream_errors: AtomicU64,
}

impl ProxyMetrics {
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            total_latency_ns: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            dns_failures: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
        }
    }

    /// Record a completed request with its connection-establishment latency
    /// (DNS + TCP/TLS + TTFB, excludes response body streaming).
    pub fn record_request(&self, connect_elapsed: Duration) {
        let nanos = u64::try_from(connect_elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.total_requests.fetch_add(1, Ordering::Release);
        self.total_latency_ns.fetch_add(nanos, Ordering::Release);
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProxyAdmissionError {
    #[error("proxy stream admission saturated at {limit} for gate {gate}")]
    Saturated { gate: &'static str, limit: usize },
    #[error("proxy stream admission unavailable for gate {gate}: {message}")]
    Unavailable {
        gate: &'static str,
        limit: usize,
        message: String,
    },
}

impl AppState {
    pub fn with_stream_concurrency_gate(mut self, gate: Arc<ConcurrencyGate>) -> Self {
        self.stream_gate = Some(gate);
        self
    }

    pub fn with_distributed_stream_concurrency_gate(
        mut self,
        gate: Arc<DistributedConcurrencyGate>,
    ) -> Self {
        self.distributed_stream_gate = Some(gate);
        self
    }

    pub fn stream_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.stream_gate.as_ref().map(|gate| gate.snapshot())
    }

    pub async fn distributed_stream_concurrency_snapshot(
        &self,
    ) -> Result<Option<DistributedConcurrencySnapshot>, DistributedConcurrencyError> {
        match &self.distributed_stream_gate {
            Some(gate) => gate.snapshot().await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn try_acquire_stream_permit(
        &self,
    ) -> Result<Option<AdmissionPermit>, ProxyAdmissionError> {
        let local = match &self.stream_gate {
            Some(gate) => Some(gate.try_acquire().map_err(|err| {
                match err {
                    ConcurrencyError::Saturated { gate, limit } => {
                        ProxyAdmissionError::Saturated { gate, limit }
                    }
                    ConcurrencyError::Closed { gate } => ProxyAdmissionError::Unavailable {
                        gate,
                        limit: self
                            .stream_gate
                            .as_ref()
                            .map(|inner| inner.snapshot().limit)
                            .unwrap_or(0),
                        message: "local stream gate is closed".to_string(),
                    },
                }
            })?),
            None => None,
        };

        let distributed = match &self.distributed_stream_gate {
            Some(gate) => Some(gate.try_acquire().await.map_err(|err| {
                match err {
                    DistributedConcurrencyError::Saturated { gate, limit } => {
                        ProxyAdmissionError::Saturated { gate, limit }
                    }
                    DistributedConcurrencyError::Unavailable {
                        gate,
                        limit,
                        message,
                    } => ProxyAdmissionError::Unavailable {
                        gate,
                        limit,
                        message,
                    },
                    DistributedConcurrencyError::InvalidConfiguration(message) => {
                        ProxyAdmissionError::Unavailable {
                            gate: "proxy_streams_distributed",
                            limit: self
                                .distributed_stream_gate
                                .as_ref()
                                .map(|inner| inner.limit())
                                .unwrap_or(0),
                            message,
                        }
                    }
                }
            })?),
            None => None,
        };

        Ok(AdmissionPermit::from_parts(local, distributed))
    }
}
