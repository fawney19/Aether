use std::time::Duration;

use aether_gateway::{
    build_tunnel_runtime_router_with_state, TunnelConnConfig, TunnelControlPlaneClient,
    TunnelRuntimeState,
};
use aether_runtime_state::RuntimeSemaphore;

use crate::server::SpawnedServer;

#[derive(Debug, Clone)]
pub struct TunnelHarnessConfig {
    pub max_streams: usize,
    pub ping_interval: Duration,
    pub outbound_queue_capacity: usize,
    pub max_in_flight_requests: Option<usize>,
    pub distributed_request_gate: Option<RuntimeSemaphore>,
}

impl Default for TunnelHarnessConfig {
    fn default() -> Self {
        Self {
            max_streams: 128,
            ping_interval: Duration::from_secs(15),
            outbound_queue_capacity: 128,
            max_in_flight_requests: None,
            distributed_request_gate: None,
        }
    }
}

#[derive(Debug)]
pub struct TunnelHarness {
    server: SpawnedServer,
}

impl TunnelHarness {
    pub async fn start(config: TunnelHarnessConfig) -> Result<Self, String> {
        Self::start_with_server(config, None).await
    }

    pub async fn start_on_port(config: TunnelHarnessConfig, port: u16) -> Result<Self, String> {
        Self::start_with_server(config, Some(port)).await
    }

    async fn start_with_server(
        config: TunnelHarnessConfig,
        port: Option<u16>,
    ) -> Result<Self, String> {
        let state = TunnelRuntimeState::new(
            TunnelControlPlaneClient::disabled(),
            TunnelConnConfig {
                ping_interval: config.ping_interval,
                idle_timeout: Duration::from_secs(0),
                outbound_queue_capacity: config.outbound_queue_capacity,
            },
            config.max_streams,
        )
        .with_request_concurrency_limit(config.max_in_flight_requests);
        let state = if let Some(gate) = config.distributed_request_gate {
            state.with_distributed_request_gate(gate)
        } else {
            state
        };
        let router = build_tunnel_runtime_router_with_state(state);
        let server = match port {
            Some(port) => SpawnedServer::start_on_port(port, router)
                .await
                .map_err(|err| format!("failed to start tunnel harness: {err}"))?,
            None => SpawnedServer::start(router)
                .await
                .map_err(|err| format!("failed to start tunnel harness: {err}"))?,
        };
        Ok(Self { server })
    }

    pub fn base_url(&self) -> &str {
        self.server.base_url()
    }

    pub fn port(&self) -> u16 {
        self.server.port()
    }
}
