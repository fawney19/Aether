use aether_http::{build_http_client, jittered_delay_for_retry, HttpClientConfig, HttpRetryConfig};
use aether_runtime::summarize_text_payload;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{debug, error, info};

use crate::config::{effective_tunnel_security, Config, ServerEntry, TunnelSecurity};
use crate::hardware::HardwareInfo;

#[derive(Debug, Serialize)]
struct RegisterRequest {
    name: String,
    ip: String,
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<String>,
    heartbeat_interval: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    hardware_info: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_max_concurrency: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel_security: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tunnel_encryption_key: Option<String>,
    tunnel_mode: bool,
}

#[derive(Debug, Deserialize)]
pub struct RegisterResponse {
    pub node_id: String,
}

/// Remote configuration pushed by the Aether management backend.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteConfig {
    pub node_name: Option<String>,
    pub allowed_ports: Option<Vec<u16>>,
    pub log_level: Option<String>,
    pub heartbeat_interval: Option<u64>,
}

#[derive(Debug, Serialize)]
struct UnregisterRequest {
    node_id: String,
}

/// Aether API client for tunnel node lifecycle management.
pub struct AetherClient {
    http: Client,
    base_url: String,
    token: String,
    retry: HttpRetryConfig,
}

impl AetherClient {
    pub fn new(config: &Config, aether_url: &str, management_token: &str) -> Self {
        let http = build_http_client(&HttpClientConfig {
            connect_timeout_ms: Some(config.aether_connect_timeout_secs.saturating_mul(1_000)),
            request_timeout_ms: Some(config.aether_request_timeout_secs.saturating_mul(1_000)),
            pool_idle_timeout_ms: Some(config.aether_pool_idle_timeout_secs.saturating_mul(1_000)),
            pool_max_idle_per_host: Some(config.aether_pool_max_idle_per_host),
            tcp_keepalive_ms: if config.aether_tcp_keepalive_secs > 0 {
                Some(config.aether_tcp_keepalive_secs.saturating_mul(1_000))
            } else {
                None
            },
            tcp_nodelay: config.aether_tcp_nodelay,
            http2_adaptive_window: config.aether_http2,
            user_agent: Some(format!("aether-tunnel/{}", env!("CARGO_PKG_VERSION"))),
            proxy_url: config
                .effective_aether_outbound_proxy_url()
                .map(str::to_string),
            ..HttpClientConfig::default()
        })
        .expect("failed to create HTTP client");

        let retry = HttpRetryConfig {
            max_attempts: config.aether_retry_max_attempts,
            base_delay_ms: config.aether_retry_base_delay_ms,
            max_delay_ms: config.aether_retry_max_delay_ms,
        }
        .normalized();

        Self {
            http,
            base_url: aether_url.trim_end_matches('/').to_string(),
            token: management_token.to_string(),
            retry,
        }
    }

    /// Register this node with Aether (idempotent upsert by ip:port).
    ///
    /// Returns the stable node_id assigned by Aether.
    pub async fn register(
        &self,
        config: &Config,
        server: &ServerEntry,
        node_name: &str,
        public_ip: &str,
        hw: Option<&HardwareInfo>,
    ) -> anyhow::Result<String> {
        let url = format!("{}/api/admin/proxy-nodes/register", self.base_url);
        let effective_security = effective_tunnel_security(
            &server.aether_url,
            server.tunnel_security,
            server.tunnel_encryption_key.as_deref(),
        );
        let body = RegisterRequest {
            name: node_name.to_string(),
            ip: public_ip.to_string(),
            port: 0,
            region: config.node_region.clone(),
            heartbeat_interval: config.heartbeat_interval,
            hardware_info: hw.and_then(|h| serde_json::to_value(h).ok()),
            estimated_max_concurrency: hw.map(|h| h.estimated_max_concurrency),
            proxy_metadata: Some(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
            })),
            tunnel_security: (effective_security == TunnelSecurity::NonTlsRequired)
                .then(|| effective_security.to_string()),
            tunnel_encryption_key: (effective_security == TunnelSecurity::NonTlsRequired)
                .then(|| server.tunnel_encryption_key.clone())
                .flatten(),
            tunnel_mode: true,
        };

        info!(
            url = %url,
            name = %body.name,
            ip = %body.ip,
            "registering with Aether"
        );

        let resp = self
            .send_with_retry(
                || {
                    self.http
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", self.token))
                        .json(&body)
                },
                "register",
            )
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let summary = summarize_text_payload(&text);
            anyhow::bail!(
                "register failed (HTTP {}): response body redacted (bytes={}, sha256={})",
                status,
                summary.bytes,
                summary.sha256
            );
        }

        let data: RegisterResponse = resp.json().await?;
        info!(node_id = %data.node_id, "registered successfully");
        Ok(data.node_id)
    }

    /// Unregister this node from Aether (graceful shutdown).
    pub async fn unregister(&self, node_id: &str) -> anyhow::Result<()> {
        let url = format!("{}/api/admin/proxy-nodes/unregister", self.base_url);
        let body = UnregisterRequest {
            node_id: node_id.to_string(),
        };

        info!(node_id = %node_id, "unregistering from Aether");

        let resp = self
            .send_with_retry(
                || {
                    self.http
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", self.token))
                        .json(&body)
                },
                "unregister",
            )
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                info!(node_id = %node_id, "unregistered successfully");
                Ok(())
            }
            Ok(r) => {
                let status = r.status();
                let text = r.text().await.unwrap_or_default();
                let summary = summarize_text_payload(&text);
                error!(
                    status = %status,
                    body_bytes = summary.bytes,
                    body_sha256 = %summary.sha256,
                    "unregister failed"
                );
                anyhow::bail!(
                    "unregister failed (HTTP {}): response body redacted (bytes={}, sha256={})",
                    status,
                    summary.bytes,
                    summary.sha256
                );
            }
            Err(e) => {
                // Best-effort during shutdown
                error!(error = %e, "unregister request failed");
                anyhow::bail!("unregister request failed: {}", e);
            }
        }
    }

    async fn send_with_retry<F>(
        &self,
        mut make_req: F,
        label: &str,
    ) -> Result<reqwest::Response, reqwest::Error>
    where
        F: FnMut() -> reqwest::RequestBuilder,
    {
        let mut attempt: u32 = 0;

        loop {
            attempt = attempt.saturating_add(1);
            let resp = make_req().send().await;
            match resp {
                Ok(resp) => {
                    if should_retry_status(resp.status()) && attempt < self.retry.max_attempts {
                        let sleep_for = jittered_delay_for_retry(self.retry, attempt - 1);
                        debug!(
                            attempt,
                            status = %resp.status(),
                            sleep_ms = sleep_for.as_millis(),
                            label,
                            "Aether request retrying"
                        );
                        sleep(sleep_for).await;
                        continue;
                    }
                    return Ok(resp);
                }
                Err(e) => {
                    if attempt < self.retry.max_attempts {
                        let sleep_for = jittered_delay_for_retry(self.retry, attempt - 1);
                        debug!(
                            attempt,
                            error = %e,
                            sleep_ms = sleep_for.as_millis(),
                            label,
                            "Aether request retrying"
                        );
                        sleep(sleep_for).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }
}

fn should_retry_status(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
}
