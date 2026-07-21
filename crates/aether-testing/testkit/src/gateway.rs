use aether_gateway::{build_router_with_state, AppState, GatewayDataConfig};
use aether_runtime_state::RuntimeSemaphore;

use crate::server::SpawnedServer;

pub const GATEWAY_HARNESS_API_KEY: &str = "sk-aether-openai-chat-pressure";

#[derive(Debug, Clone)]
pub struct GatewayHarnessConfig {
    pub upstream_base_url: String,
    pub data_config: Option<GatewayDataConfig>,
    pub max_in_flight_requests: Option<usize>,
    pub distributed_request_gate: Option<RuntimeSemaphore>,
    pub tunnel_instance_id: Option<String>,
    pub tunnel_relay_base_url: Option<String>,
}

impl GatewayHarnessConfig {
    pub fn new(upstream_base_url: impl Into<String>) -> Self {
        Self {
            upstream_base_url: upstream_base_url.into(),
            data_config: None,
            max_in_flight_requests: None,
            distributed_request_gate: None,
            tunnel_instance_id: None,
            tunnel_relay_base_url: None,
        }
    }
}

#[derive(Debug)]
pub struct GatewayHarness {
    server: SpawnedServer,
}

impl GatewayHarness {
    pub async fn start(config: GatewayHarnessConfig) -> Result<Self, String> {
        Self::start_with_server(config, None).await
    }

    pub async fn start_on_port(config: GatewayHarnessConfig, port: u16) -> Result<Self, String> {
        Self::start_with_server(config, Some(port)).await
    }

    async fn start_with_server(
        config: GatewayHarnessConfig,
        port: Option<u16>,
    ) -> Result<Self, String> {
        let mut state = match config.data_config {
            Some(data_config) => AppState::new()
                .map_err(|err| format!("failed to build gateway harness state: {err}"))?
                .with_data_config(data_config)
                .map_err(|err| format!("failed to configure gateway harness data state: {err}"))?,
            None => aether_gateway::testkit::build_openai_chat_pressure_state(
                aether_gateway::testkit::OpenAiChatPressureStateConfig::new(vec![format!(
                    "{}/v1",
                    config.upstream_base_url.trim_end_matches('/')
                )]),
            )?,
        };
        if let Some(instance_id) = config.tunnel_instance_id {
            state = state.with_tunnel_identity(instance_id, config.tunnel_relay_base_url);
        }
        if let Some(limit) = config.max_in_flight_requests {
            state = state.with_request_concurrency_limit(limit);
        }
        if let Some(gate) = config.distributed_request_gate {
            state = state.with_distributed_request_concurrency_gate(gate);
        }
        let router = build_router_with_state(state);
        let server = match port {
            Some(port) => SpawnedServer::start_on_port(port, router)
                .await
                .map_err(|err| format!("failed to start gateway harness: {err}"))?,
            None => SpawnedServer::start(router)
                .await
                .map_err(|err| format!("failed to start gateway harness: {err}"))?,
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
