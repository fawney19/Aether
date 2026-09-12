use aether_http::{apply_http_client_config, HttpClientConfig};
use futures_util::future::BoxFuture;
use reqwest::Client;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_contracts::tunnel_security::{
    sign_tunnel_control_plane_request_for_generation, TUNNEL_CONTROL_PLANE_GENERATION_HEADER,
    TUNNEL_CONTROL_PLANE_NODE_ID_HEADER, TUNNEL_CONTROL_PLANE_NONCE_HEADER,
    TUNNEL_CONTROL_PLANE_SIGNATURE_HEADER, TUNNEL_CONTROL_PLANE_TIMESTAMP_HEADER,
};
use aether_gateway_tunnel::{TUNNEL_HEARTBEAT_PATH, TUNNEL_NODE_STATUS_PATH};

use super::hub::ProxyConn;

const MAX_CONTROL_PLANE_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_CONTROL_PLANE_BASE_URL_BYTES: usize = 2 * 1024;
pub(crate) const CONTROL_PLANE_CREDENTIAL_REVOKED: &str = "proxy tunnel credential revoked";
pub(crate) const CONTROL_PLANE_CREDENTIAL_UNAVAILABLE: &str =
    "proxy tunnel credential validation unavailable";

type HeartbeatAckCallback =
    dyn Fn(Arc<ProxyConn>, Vec<u8>) -> BoxFuture<'static, Result<Vec<u8>, String>> + Send + Sync;
type NodeStatusCallback = dyn Fn(Arc<ProxyConn>, bool, usize, u64) -> BoxFuture<'static, Result<(), String>>
    + Send
    + Sync;

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
        // The control-plane base URL is operator supplied, but it is used for
        // requests carrying a tunnel credential.  Reject URL-controlled
        // request components (userinfo/query/fragment) before concatenating
        // endpoint paths; otherwise a typo such as `?token=...` can leak
        // credentials or change the signed request target.  Keep the
        // established support for HTTP and private deployment hosts—the
        // standalone tunnel commonly talks to an in-cluster gateway.
        let Some(base_url) = normalize_control_plane_base_url(&base_url) else {
            return Self {
                inner: Arc::new(ControlPlaneMode::Http {
                    client: None,
                    base_url: String::new(),
                }),
            };
        };
        let client = apply_http_client_config(
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none()),
            &HttpClientConfig {
                request_timeout_ms: Some(10_000),
                user_agent: Some("aether-tunnel-standalone/control-plane".to_string()),
                ..HttpClientConfig::default()
            },
        )
        .build()
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
        HeartbeatAck: Fn(Arc<ProxyConn>, Vec<u8>) -> BoxFuture<'static, Result<Vec<u8>, String>>
            + Send
            + Sync
            + 'static,
        PushNodeStatus: Fn(Arc<ProxyConn>, bool, usize, u64) -> BoxFuture<'static, Result<(), String>>
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

    pub async fn heartbeat_ack(
        &self,
        authenticated_node_id: &str,
        authenticated_key: Option<&str>,
        authenticated_generation: &str,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        match self.inner.as_ref() {
            ControlPlaneMode::Disabled => Ok(b"{}".to_vec()),
            ControlPlaneMode::Http { .. } => {
                self.heartbeat_ack_http(
                    authenticated_node_id,
                    authenticated_key,
                    authenticated_generation,
                    payload,
                )
                .await
            }
            ControlPlaneMode::Local { .. } => {
                Err("local heartbeat callback requires connection credential binding".to_string())
            }
        }
    }

    pub async fn heartbeat_ack_for_connection(
        &self,
        connection: Arc<ProxyConn>,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        match self.inner.as_ref() {
            ControlPlaneMode::Disabled => Ok(b"{}".to_vec()),
            ControlPlaneMode::Http { .. } => {
                self.heartbeat_ack_http(
                    &connection.node_id,
                    connection.authenticated_key.as_deref(),
                    &connection.node_generation,
                    payload,
                )
                .await
            }
            ControlPlaneMode::Local { heartbeat_ack, .. } => {
                heartbeat_ack(connection, payload.to_vec()).await
            }
        }
    }

    async fn heartbeat_ack_http(
        &self,
        authenticated_node_id: &str,
        authenticated_key: Option<&str>,
        authenticated_generation: &str,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        let ControlPlaneMode::Http { client, base_url } = self.inner.as_ref() else {
            return Err("HTTP heartbeat callback is unavailable".to_string());
        };
        let Some(client) = client else {
            return Err("heartbeat callback HTTP client is unavailable".to_string());
        };
        let authenticated_key = authenticated_key
            .ok_or_else(|| "heartbeat callback is missing authenticated tunnel key".to_string())?;
        let url = format!("{}{TUNNEL_HEARTBEAT_PATH}", base_url.trim_end_matches('/'));
        let request = client
            .post(&url)
            .header("content-type", "application/json")
            .body(payload.to_vec());
        let response = sign_control_plane_request(
            request,
            authenticated_key,
            TUNNEL_HEARTBEAT_PATH,
            authenticated_node_id,
            authenticated_generation,
            payload,
        )?
        .send()
        .await
        .map_err(|error| {
            format!(
                "heartbeat callback request failed ({})",
                control_plane_reqwest_error_kind(&error)
            )
        })?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(CONTROL_PLANE_CREDENTIAL_REVOKED.to_string());
        }
        if !response.status().is_success() {
            return Err(format!(
                "heartbeat callback failed with status {}",
                response.status()
            ));
        }
        aether_http::read_response_bytes_with_limit(response, MAX_CONTROL_PLANE_RESPONSE_BYTES)
            .await
            .map_err(|e| format!("heartbeat callback body read failed: {e}"))
    }

    pub async fn push_node_status(
        &self,
        node_id: &str,
        authenticated_key: Option<&str>,
        authenticated_generation: &str,
        connected: bool,
        conn_count: usize,
        observed_at_unix_secs: u64,
    ) -> Result<(), String> {
        match self.inner.as_ref() {
            ControlPlaneMode::Disabled => Ok(()),
            ControlPlaneMode::Http { .. } => {
                self.push_node_status_http(
                    node_id,
                    authenticated_key,
                    authenticated_generation,
                    connected,
                    conn_count,
                    observed_at_unix_secs,
                )
                .await
            }
            ControlPlaneMode::Local { .. } => {
                Err("local node-status callback requires connection credential binding".to_string())
            }
        }
    }

    pub async fn push_node_status_for_connection(
        &self,
        connection: Arc<ProxyConn>,
        connected: bool,
        conn_count: usize,
        observed_at_unix_secs: u64,
    ) -> Result<(), String> {
        match self.inner.as_ref() {
            ControlPlaneMode::Disabled => Ok(()),
            ControlPlaneMode::Http { .. } => {
                self.push_node_status_http(
                    &connection.node_id,
                    connection.authenticated_key.as_deref(),
                    &connection.node_generation,
                    connected,
                    conn_count,
                    observed_at_unix_secs,
                )
                .await
            }
            ControlPlaneMode::Local {
                push_node_status, ..
            } => push_node_status(connection, connected, conn_count, observed_at_unix_secs).await,
        }
    }

    async fn push_node_status_http(
        &self,
        node_id: &str,
        authenticated_key: Option<&str>,
        authenticated_generation: &str,
        connected: bool,
        conn_count: usize,
        observed_at_unix_secs: u64,
    ) -> Result<(), String> {
        let ControlPlaneMode::Http { client, base_url } = self.inner.as_ref() else {
            return Err("HTTP node-status callback is unavailable".to_string());
        };
        let Some(client) = client else {
            return Err("node-status callback HTTP client is unavailable".to_string());
        };
        let authenticated_key = authenticated_key.ok_or_else(|| {
            "node-status callback is missing authenticated tunnel key".to_string()
        })?;
        let url = format!(
            "{}{TUNNEL_NODE_STATUS_PATH}",
            base_url.trim_end_matches('/')
        );
        let payload = serde_json::to_vec(&serde_json::json!({
            "node_id": node_id,
            "connected": connected,
            "conn_count": conn_count,
            "observed_at_unix_secs": observed_at_unix_secs,
        }))
        .map_err(|e| format!("node-status callback serialization failed: {e}"))?;
        let request = client
            .post(&url)
            .header("content-type", "application/json")
            .body(payload.clone());
        let response = sign_control_plane_request(
            request,
            authenticated_key,
            TUNNEL_NODE_STATUS_PATH,
            node_id,
            authenticated_generation,
            &payload,
        )?
        .send()
        .await
        .map_err(|error| {
            format!(
                "node-status callback request failed ({})",
                control_plane_reqwest_error_kind(&error)
            )
        })?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(CONTROL_PLANE_CREDENTIAL_REVOKED.to_string());
        }
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

fn normalize_control_plane_base_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > MAX_CONTROL_PLANE_BASE_URL_BYTES
        || raw.bytes().any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return None;
    }
    let parsed = url::Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    Some(raw.trim_end_matches('/').to_string())
}

pub(crate) fn is_credential_revoked_error(error: &str) -> bool {
    error == CONTROL_PLANE_CREDENTIAL_REVOKED
}

/// Return a stable transport category without rendering reqwest's error.
///
/// `reqwest::Error`'s `Display` implementation may include the complete URL
/// (including path/query components).  Control-plane errors are logged by the
/// tunnel hub, so forwarding that value could disclose operator deployment
/// details or credentials embedded in a path.  Callers should use this helper
/// whenever a control-plane request fails.
fn control_plane_reqwest_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else {
        "transport"
    }
}

fn sign_control_plane_request(
    request: reqwest::RequestBuilder,
    authenticated_key: &str,
    path: &str,
    node_id: &str,
    tunnel_generation: &str,
    body: &[u8],
) -> Result<reqwest::RequestBuilder, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())?
        .as_secs();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let signature = sign_tunnel_control_plane_request_for_generation(
        authenticated_key,
        "POST",
        path,
        node_id,
        tunnel_generation,
        timestamp,
        &nonce,
        body,
    )
    .map_err(|error| format!("invalid authenticated tunnel key: {error}"))?;
    Ok(request
        .header(TUNNEL_CONTROL_PLANE_NODE_ID_HEADER, node_id)
        .header(TUNNEL_CONTROL_PLANE_GENERATION_HEADER, tunnel_generation)
        .header(TUNNEL_CONTROL_PLANE_TIMESTAMP_HEADER, timestamp)
        .header(TUNNEL_CONTROL_PLANE_NONCE_HEADER, nonce)
        .header(TUNNEL_CONTROL_PLANE_SIGNATURE_HEADER, signature))
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{
        body::Body,
        http::{header, Response, StatusCode},
        routing::post,
        Router,
    };
    use base64::Engine as _;

    use super::{
        control_plane_reqwest_error_kind, normalize_control_plane_base_url, ControlPlaneClient,
    };
    use aether_gateway_tunnel::{TUNNEL_HEARTBEAT_PATH, TUNNEL_NODE_STATUS_PATH};

    #[test]
    fn control_plane_base_url_rejects_credential_and_request_components() {
        for value in [
            "https://user:secret@gateway.example",
            "https://gateway.example?token=secret",
            "https://gateway.example/control#fragment",
            "file:///tmp/gateway",
            "",
        ] {
            assert!(
                normalize_control_plane_base_url(value).is_none(),
                "unsafe control-plane URL should be rejected: {value:?}"
            );
        }
    }

    #[test]
    fn control_plane_base_url_preserves_deployment_path_and_trims_slashes() {
        assert_eq!(
            normalize_control_plane_base_url(" https://gateway.example/control/// "),
            Some("https://gateway.example/control".to_string())
        );
        assert_eq!(
            normalize_control_plane_base_url("http://127.0.0.1:8084/"),
            Some("http://127.0.0.1:8084".to_string())
        );
    }

    #[tokio::test]
    async fn control_plane_transport_errors_do_not_render_request_url() {
        let error = reqwest::Client::new()
            .get("ftp://user:secret@example.invalid/control-plane")
            .send()
            .await
            .expect_err("unsupported control-plane scheme should fail before a request");
        let rendered = format!(
            "control-plane request failed ({})",
            control_plane_reqwest_error_kind(&error)
        );
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("example.invalid"));
        assert!(rendered.starts_with("control-plane request failed ("));
    }

    #[tokio::test]
    async fn signed_control_plane_requests_never_follow_redirects() {
        let redirected_hits = Arc::new(AtomicUsize::new(0));
        let redirected_hits_for_route = Arc::clone(&redirected_hits);
        let redirected_listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("redirect target listener should bind");
        let redirected_addr = redirected_listener
            .local_addr()
            .expect("redirect target address should resolve");
        let redirected_app = Router::new().fallback(move || {
            let hits = Arc::clone(&redirected_hits_for_route);
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        });
        let redirected_server = tokio::spawn(async move {
            axum::serve(redirected_listener, redirected_app)
                .await
                .expect("redirect target server should run");
        });

        let source_listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("redirect source listener should bind");
        let source_addr = source_listener
            .local_addr()
            .expect("redirect source address should resolve");
        let location = format!("http://{redirected_addr}/captured");
        let redirect = move || {
            let location = location.clone();
            async move {
                Response::builder()
                    .status(StatusCode::TEMPORARY_REDIRECT)
                    .header(header::LOCATION, location)
                    .body(Body::empty())
                    .expect("redirect response should build")
            }
        };
        let source_app = Router::new()
            .route(TUNNEL_HEARTBEAT_PATH, post(redirect.clone()))
            .route(TUNNEL_NODE_STATUS_PATH, post(redirect));
        let source_server = tokio::spawn(async move {
            axum::serve(source_listener, source_app)
                .await
                .expect("redirect source server should run");
        });

        let client = ControlPlaneClient::new(format!("http://{source_addr}"));
        let key = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
        let heartbeat = client
            .heartbeat_ack(
                "node-1",
                Some(&key),
                "generation-1",
                br#"{"node_id":"node-1"}"#,
            )
            .await
            .expect_err("heartbeat redirect should be returned as an error");
        assert!(heartbeat.contains("307 Temporary Redirect"));
        let status = client
            .push_node_status("node-1", Some(&key), "generation-1", true, 1, 1)
            .await
            .expect_err("node-status redirect should be returned as an error");
        assert!(status.contains("307 Temporary Redirect"));
        assert_eq!(redirected_hits.load(Ordering::SeqCst), 0);

        source_server.abort();
        redirected_server.abort();
    }
}
