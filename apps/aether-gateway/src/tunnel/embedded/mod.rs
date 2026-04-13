mod control_plane;
mod hub;
mod local_relay;
pub mod protocol;
mod proxy_conn;

use std::sync::Arc;

use aether_runtime::{
    hold_admission_permit_until, prometheus_response, service_up_sample, AdmissionPermit,
    ConcurrencyError, ConcurrencyGate, ConcurrencySnapshot, DistributedConcurrencyError,
    DistributedConcurrencyGate, DistributedConcurrencySnapshot, MetricKind, MetricLabel,
    MetricSample,
};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;
use tracing::warn;

use crate::data::GatewayDataState;

pub use control_plane::ControlPlaneClient;
pub use hub::{ConnConfig, HubRouter, ProxyConn};
pub use local_relay::relay_request;

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<HubRouter>,
    pub proxy_conn_cfg: ConnConfig,
    pub max_streams: usize,
    data: Arc<GatewayDataState>,
    request_gate: Option<Arc<ConcurrencyGate>>,
    distributed_request_gate: Option<Arc<DistributedConcurrencyGate>>,
}

#[derive(Debug)]
enum RequestAdmissionError {
    Local(ConcurrencyError),
    Distributed(DistributedConcurrencyError),
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
        }
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

    pub fn with_distributed_request_gate(mut self, gate: DistributedConcurrencyGate) -> Self {
        self.distributed_request_gate = Some(Arc::new(gate));
        self
    }

    fn request_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.request_gate.as_ref().map(|gate| gate.snapshot())
    }

    async fn distributed_request_concurrency_snapshot(
        &self,
    ) -> Result<Option<DistributedConcurrencySnapshot>, DistributedConcurrencyError> {
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
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/stats", get(stats))
        .route("/api/internal/proxy-tunnel", get(ws_proxy))
        .route(
            "/api/internal/tunnel/relay/{node_id}",
            post(local_relay::relay_request),
        )
        .with_state(state)
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

    let request_permit = match state.try_acquire_request_permit().await {
        Ok(permit) => permit,
        Err(RequestAdmissionError::Local(ConcurrencyError::Saturated { .. }))
        | Err(RequestAdmissionError::Distributed(DistributedConcurrencyError::Saturated {
            ..
        }))
        | Err(RequestAdmissionError::Distributed(DistributedConcurrencyError::Unavailable {
            ..
        })) => return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Err(RequestAdmissionError::Local(ConcurrencyError::Closed { gate })) => {
            warn!(
                gate = gate,
                "standalone tunnel relay request concurrency gate is closed"
            );
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(RequestAdmissionError::Distributed(
            DistributedConcurrencyError::InvalidConfiguration(message),
        )) => {
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
                    state.proxy_conn_cfg,
                )
                .await
            })
        })
        .into_response()
}
