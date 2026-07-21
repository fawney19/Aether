mod control_plane;
mod hub;
mod local_relay;
pub mod protocol;
mod proxy_conn;

use std::sync::Arc;

use aether_gateway_tunnel::{
    resolve_proxy_max_streams, resolve_proxy_node_name, resolve_proxy_protocol_version,
};
use aether_runtime::{
    hold_admission_permit_until, prometheus_response, service_up_sample, AdmissionPermit,
    ConcurrencyError, ConcurrencyGate, ConcurrencySnapshot, MetricKind, MetricLabel, MetricSample,
};
use aether_runtime_state::{RuntimeSemaphore, RuntimeSemaphoreError, RuntimeSemaphoreSnapshot};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use dashmap::DashMap;
use tracing::warn;

use crate::{data::GatewayDataState, middleware};

pub use control_plane::ControlPlaneClient;
pub use hub::{ConnConfig, HubRouter, LocalBodyEvent, ProxyConn};
pub use local_relay::relay_request;
pub(crate) use local_relay::{open_direct_relay_stream, DirectRelayResponse};

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<HubRouter>,
    pub proxy_conn_cfg: ConnConfig,
    pub max_streams: usize,
    data: Arc<GatewayDataState>,
    request_gate: Option<Arc<ConcurrencyGate>>,
    distributed_request_gate: Option<Arc<RuntimeSemaphore>>,
    secure_tunnel_keys: Arc<DashMap<String, String>>,
}

#[derive(Debug)]
enum RequestAdmissionError {
    Local(ConcurrencyError),
    Distributed(RuntimeSemaphoreError),
}

impl AppState {
    pub fn new(
        control_plane: ControlPlaneClient,
        proxy_conn_cfg: ConnConfig,
        max_streams: usize,
    ) -> Self {
        Self {
            hub: HubRouter::new(control_plane),
            proxy_conn_cfg,
            max_streams,
            data: Arc::new(GatewayDataState::disabled()),
            request_gate: None,
            distributed_request_gate: None,
            secure_tunnel_keys: Arc::new(DashMap::new()),
        }
    }

    pub(crate) fn register_secure_tunnel_key(
        &self,
        node_id: impl Into<String>,
        key: impl Into<String>,
    ) {
        self.secure_tunnel_keys.insert(node_id.into(), key.into());
    }

    pub(crate) fn secure_tunnel_key(&self, node_id: &str) -> Option<String> {
        self.secure_tunnel_keys
            .get(node_id)
            .map(|entry| entry.value().clone())
    }

    async fn secure_tunnel_key_for_node(&self, node_id: &str) -> Option<String> {
        if let Some(key) = self.secure_tunnel_key(node_id) {
            return Some(key);
        }
        let key = self
            .data
            .find_proxy_node(node_id)
            .await
            .ok()
            .flatten()
            .and_then(|node| {
                node.proxy_metadata.and_then(|metadata| {
                    metadata
                        .pointer("/tunnel_security/encryption_key")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
            });
        if let Some(key) = key.as_ref() {
            self.register_secure_tunnel_key(node_id.to_string(), key.clone());
        }
        key
    }

    pub(crate) fn with_data(mut self, data: Arc<GatewayDataState>) -> Self {
        self.data = data;
        self
    }

    pub fn with_request_concurrency_limit(mut self, limit: Option<usize>) -> Self {
        self.request_gate = limit
            .filter(|limit| *limit > 0)
            .map(|limit| Arc::new(ConcurrencyGate::new("tunnel_requests", limit)));
        self
    }

    pub fn with_distributed_request_gate(mut self, gate: RuntimeSemaphore) -> Self {
        self.distributed_request_gate = Some(Arc::new(gate));
        self
    }

    fn request_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.request_gate.as_ref().map(|gate| gate.snapshot())
    }

    async fn distributed_request_concurrency_snapshot(
        &self,
    ) -> Result<Option<RuntimeSemaphoreSnapshot>, RuntimeSemaphoreError> {
        match self.distributed_request_gate.as_ref() {
            Some(gate) => gate.snapshot().await.map(Some),
            None => Ok(None),
        }
    }

    async fn metric_samples(&self) -> Vec<MetricSample> {
        let mut samples = vec![service_up_sample("aether-tunnel-standalone")];
        if let Some(snapshot) = self.request_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples("tunnel_requests"));
        }
        if let Some(gate) = self.distributed_request_gate.as_ref() {
            match gate.snapshot().await {
                Ok(snapshot) => {
                    samples.extend(snapshot.to_metric_samples("tunnel_requests_distributed"));
                }
                Err(_) => samples.push(
                    MetricSample::new(
                        "concurrency_unavailable",
                        "Whether the distributed concurrency gate is currently unavailable.",
                        MetricKind::Gauge,
                        1,
                    )
                    .with_labels(vec![MetricLabel::new(
                        "gate",
                        "tunnel_requests_distributed",
                    )]),
                ),
            }
        }
        samples.extend(self.hub.stats().to_metric_samples());
        samples
    }

    async fn try_acquire_request_permit(
        &self,
    ) -> Result<Option<AdmissionPermit>, RequestAdmissionError> {
        let local = self
            .request_gate
            .as_ref()
            .map(|gate| gate.try_acquire())
            .transpose()
            .map_err(RequestAdmissionError::Local)?;
        let distributed = match self.distributed_request_gate.as_ref() {
            Some(gate) => Some(
                gate.try_acquire()
                    .await
                    .map_err(RequestAdmissionError::Distributed)?,
            ),
            None => None,
        };
        Ok(AdmissionPermit::from_parts(local, distributed))
    }
}

pub fn build_router_with_state(state: AppState) -> Router {
    middleware::apply_cf_header_stripping(
        Router::new()
            .route("/health", get(health))
            .route("/metrics", get(metrics))
            .route("/stats", get(stats))
            .route("/api/internal/proxy-tunnel", get(ws_proxy))
            .route(
                "/api/internal/tunnel/relay/{node_id}",
                post(local_relay::relay_request),
            )
            .with_state(state),
    )
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let request_concurrency = state.request_concurrency_snapshot().map(|snapshot| {
        serde_json::json!({
            "limit": snapshot.limit,
            "in_flight": snapshot.in_flight,
            "available_permits": snapshot.available_permits,
            "high_watermark": snapshot.high_watermark,
            "rejected": snapshot.rejected,
        })
    });
    let distributed_request_concurrency = state
        .distributed_request_concurrency_snapshot()
        .await
        .ok()
        .flatten()
        .map(|snapshot| {
            serde_json::json!({
                "limit": snapshot.limit,
                "in_flight": snapshot.in_flight,
                "available_permits": snapshot.available_permits,
                "high_watermark": snapshot.high_watermark,
                "rejected": snapshot.rejected,
            })
        });
    Json(serde_json::json!({
        "status": "ok",
        "request_concurrency": request_concurrency,
        "distributed_request_concurrency": distributed_request_concurrency,
    }))
}

async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.hub.stats())
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    prometheus_response(&state.metric_samples().await)
}

pub async fn ws_proxy(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let node_id = headers
        .get("x-node-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();

    let node_name = resolve_proxy_node_name(&headers, &node_id);

    let max_streams = resolve_proxy_max_streams(&headers, state.max_streams);
    let protocol_version = resolve_proxy_protocol_version(&headers);
    let tunnel_security = headers
        .get(aether_contracts::tunnel_security::TUNNEL_SECURITY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let security_session = headers
        .get(aether_contracts::tunnel_security::TUNNEL_SECURITY_SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    if node_id.is_empty() {
        warn!("proxy connection rejected: missing X-Node-ID header");
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }
    let stored_security_key = state.secure_tunnel_key_for_node(&node_id).await;
    let (security_key, security_session) = match tunnel_security.as_deref() {
        Some(aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED) => {
            match stored_security_key {
                Some(key) => {
                    let Some(session) = security_session else {
                        warn!(node_id = %node_id, "secure tunnel requested without a security session");
                        return axum::http::StatusCode::BAD_REQUEST.into_response();
                    };
                    (Some(key), session)
                }
                None => {
                    warn!(node_id = %node_id, "secure tunnel requested but no PSK is registered");
                    return axum::http::StatusCode::UNAUTHORIZED.into_response();
                }
            }
        }
        Some(_) => return axum::http::StatusCode::BAD_REQUEST.into_response(),
        None if stored_security_key.is_some() => {
            warn!(node_id = %node_id, "proxy connection rejected: stored secure tunnel key requires encrypted frames");
            return axum::http::StatusCode::UNAUTHORIZED.into_response();
        }
        None => (None, String::new()),
    };

    let request_permit = match state.try_acquire_request_permit().await {
        Ok(permit) => permit,
        Err(RequestAdmissionError::Local(ConcurrencyError::Saturated { .. }))
        | Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::Saturated { .. }))
        | Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::Unavailable { .. })) => {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        Err(RequestAdmissionError::Local(ConcurrencyError::Closed { gate })) => {
            warn!(
                gate = gate,
                "standalone tunnel relay request concurrency gate is closed"
            );
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::InvalidConfiguration(
            message,
        ))) => {
            warn!(
                error = %message,
                "standalone tunnel relay distributed request gate is invalid"
            );
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    ws.max_frame_size(64 * 1024 * 1024)
        .on_upgrade(move |socket| {
            hold_admission_permit_until(request_permit, async move {
                proxy_conn::handle_proxy_connection(
                    socket,
                    state.hub,
                    node_id,
                    node_name,
                    max_streams,
                    protocol_version,
                    security_key,
                    security_session,
                    state.proxy_conn_cfg,
                )
                .await
            })
        })
        .into_response()
}
