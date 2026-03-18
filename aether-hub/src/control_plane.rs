use reqwest::Client;

#[derive(Clone)]
pub struct ControlPlaneClient {
    client: Option<Client>,
    base_url: String,
}

impl ControlPlaneClient {
    pub fn new(base_url: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .ok();
        Self { client, base_url }
    }

    pub fn disabled() -> Self {
        Self {
            client: None,
            base_url: String::new(),
        }
    }

    pub async fn heartbeat_ack(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let Some(client) = &self.client else {
            return Ok(b"{}".to_vec());
        };
        let url = format!(
            "{}/api/internal/hub/heartbeat",
            self.base_url.trim_end_matches('/')
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

    pub async fn push_node_status(
        &self,
        node_id: &str,
        connected: bool,
        conn_count: usize,
    ) -> Result<(), String> {
        let Some(client) = &self.client else {
            return Ok(());
        };
        let url = format!(
            "{}/api/internal/hub/node-status",
            self.base_url.trim_end_matches('/')
        );
        let response = client
            .post(&url)
            .json(&serde_json::json!({
                "node_id": node_id,
                "connected": connected,
                "conn_count": conn_count,
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
}
