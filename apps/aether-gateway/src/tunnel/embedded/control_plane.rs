use aether_http::{build_http_client, HttpClientConfig};
use futures_util::future::BoxFuture;
use reqwest::Client;
use std::sync::Arc;

type HeartbeatAckCallback =
    dyn Fn(Vec<u8>) -> BoxFuture<'static, Result<Vec<u8>, String>> + Send + Sync;
type NodeStatusCallback =
    dyn Fn(String, bool, usize, u64) -> BoxFuture<'static, Result<(), String>> + Send + Sync;

enum ControlPlaneMode {
    Disabled,
    Http {
        client: Option<Client>,
        base_url: String,
    },
    Local {
        heartbeat_ack: Arc<HeartbeatAckCallback>,
        push_node_status: Arc<NodeStatusCallback>,
    },
}

#[derive(Clone)]
pub struct ControlPlaneClient {
    inner: Arc<ControlPlaneMode>,
}

impl ControlPlaneClient {
    pub fn new(base_url: String) -> Self {
        let client = build_http_client(&HttpClientConfig {
            request_timeout_ms: Some(10_000),
            user_agent: Some("aether-tunnel-standalone/control-plane".to_string()),
            ..HttpClientConfig::default()
        })
        .ok();
        Self {
            inner: Arc::new(ControlPlaneMode::Http { client, base_url }),
        }
    }

    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(ControlPlaneMode::Disabled),
        }
    }

    pub fn local<HeartbeatAck, PushNodeStatus>(
        heartbeat_ack: HeartbeatAck,
        push_node_status: PushNodeStatus,
    ) -> Self
    where
        HeartbeatAck:
            Fn(Vec<u8>) -> BoxFuture<'static, Result<Vec<u8>, String>> + Send + Sync + 'static,
        PushNodeStatus: Fn(String, bool, usize, u64) -> BoxFuture<'static, Result<(), String>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            inner: Arc::new(ControlPlaneMode::Local {
                heartbeat_ack: Arc::new(heartbeat_ack),
                push_node_status: Arc::new(push_node_status),
            }),
        }
    }

    pub async fn heartbeat_ack(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        match self.inner.as_ref() {
            ControlPlaneMode::Disabled => Ok(b"{}".to_vec()),
            ControlPlaneMode::Http { client, base_url } => {
                let Some(client) = client else {
                    return Ok(b"{}".to_vec());
                };
                let url = format!(
                    "{}/api/internal/tunnel/heartbeat",
                    base_url.trim_end_matches('/')
                );
                let response = client
                    .post(&url)
                    .header("content-type", "application/json")
                    .body(payload.to_vec())
                    .send()
                    .await
                    .map_err(|e| format!("heartbeat callback request failed: {e}"))?;
                if !response.status().is_success() {
                    return Err(format!(
                        "heartbeat callback failed with status {}",
                        response.status()
                    ));
                }
                response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(|e| format!("heartbeat callback body read failed: {e}"))
            }
            ControlPlaneMode::Local { heartbeat_ack, .. } => heartbeat_ack(payload.to_vec()).await,
        }
    }

    pub async fn push_node_status(
        &self,
        node_id: &str,
        connected: bool,
        conn_count: usize,
        observed_at_unix_secs: u64,
    ) -> Result<(), String> {
        match self.inner.as_ref() {
            ControlPlaneMode::Disabled => Ok(()),
            ControlPlaneMode::Http { client, base_url } => {
                let Some(client) = client else {
                    return Ok(());
                };
                let url = format!(
                    "{}/api/internal/tunnel/node-status",
                    base_url.trim_end_matches('/')
                );
                let response = client
                    .post(&url)
                    .json(&serde_json::json!({
                        "node_id": node_id,
                        "connected": connected,
                        "conn_count": conn_count,
                        "observed_at_unix_secs": observed_at_unix_secs,
                    }))
                    .send()
                    .await
                    .map_err(|e| format!("node-status callback request failed: {e}"))?;
                if response.status().is_success() {
                    Ok(())
                } else {
                    Err(format!(
                        "node-status callback failed with status {}",
                        response.status()
                    ))
                }
            }
            ControlPlaneMode::Local {
                push_node_status, ..
            } => {
                push_node_status(
                    node_id.to_string(),
                    connected,
                    conn_count,
                    observed_at_unix_secs,
                )
                .await
            }
        }
    }
}
