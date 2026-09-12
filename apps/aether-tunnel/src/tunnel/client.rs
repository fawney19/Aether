//! WebSocket tunnel client: connect, authenticate, and run the tunnel.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tracing::{debug, info, warn};

use crate::config::aether_url_for_log;
use crate::egress_proxy::{
    connect_target_via_proxy, IpFamily, ProxyConnectOptions, UpstreamProxyConfig,
};
use crate::state::{AppState, ServerContext};
use aether_contracts::tunnel::{
    HelloPayload, SettingsPayload, CURRENT_TUNNEL_PROTOCOL_VERSION, TUNNEL_NODE_NAME_B64_HEADER,
    TUNNEL_PROTOCOL_VERSION_HEADER,
};
use aether_contracts::tunnel_security::{
    sign_tunnel_security_handshake_for_generation, SecureFrameCodec, TunnelSecurityRole,
    TUNNEL_GENERATION_HEADER, TUNNEL_SECURITY_HEADER, TUNNEL_SECURITY_NON_TLS_REQUIRED,
    TUNNEL_SECURITY_PROOF_NONCE_HEADER, TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER,
    TUNNEL_SECURITY_PROOF_TIMESTAMP_HEADER, TUNNEL_SECURITY_SESSION_HEADER,
};

use super::{dispatcher, heartbeat, writer};

/// Outcome of a tunnel session.
pub enum TunnelOutcome {
    /// Graceful shutdown requested by the local process.
    Shutdown,
    /// Remote side disconnected or connection lost — should reconnect.
    Disconnected,
}

/// Connect to Aether's WebSocket tunnel endpoint and run until disconnected.
///
/// `conn_idx` identifies which connection in the pool this is (0-based).
/// Only connection 0 sends heartbeats to avoid resetting shared metrics.
pub async fn connect_and_run(
    state: &Arc<AppState>,
    server: &Arc<ServerContext>,
    conn_idx: usize,
    shutdown: &mut watch::Receiver<bool>,
    drain: watch::Receiver<bool>,
) -> Result<TunnelOutcome, anyhow::Error> {
    let ws_url = build_tunnel_url(server);
    debug!(url = %aether_url_for_log(&ws_url), conn = conn_idx, "connecting tunnel");

    // Build WebSocket request with auth headers
    let mut request = ws_url.clone().into_client_request()?;
    let headers = request.headers_mut();
    if server.tunnel_security != crate::config::TunnelSecurity::NonTlsRequired {
        insert_ascii_header(
            headers,
            "Authorization",
            &format!("Bearer {}", server.management_token),
            "management_token",
        )?;
    }
    headers.insert(
        TUNNEL_PROTOCOL_VERSION_HEADER,
        http::HeaderValue::from_str(&CURRENT_TUNNEL_PROTOCOL_VERSION.to_string())?,
    );
    let node_id = server.node_id.read().unwrap().clone();
    insert_ascii_header(headers, "X-Node-Id", &node_id, "node_id")?;
    insert_ascii_header(
        headers,
        TUNNEL_GENERATION_HEADER,
        &server.tunnel_generation,
        "tunnel_generation",
    )?;
    let security_session =
        if server.tunnel_security == crate::config::TunnelSecurity::NonTlsRequired {
            let key = server
                .tunnel_encryption_key
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("secure tunnel requires tunnel_encryption_key"))?;
            let session = uuid::Uuid::new_v4().simple().to_string();
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| anyhow::anyhow!("system clock is before the Unix epoch"))?
                .as_secs();
            insert_tunnel_security_handshake_headers(
                headers,
                key,
                &node_id,
                &server.tunnel_generation,
                &session,
                CURRENT_TUNNEL_PROTOCOL_VERSION,
                timestamp,
                &nonce,
            )?;
            session
        } else {
            String::new()
        };
    // Use dynamic node_name (may be updated by remote config) instead of
    // the static server.node_name, so that remote name changes take effect
    // on the next reconnect.
    let dynamic_node_name = server.dynamic.load().node_name.clone();
    insert_node_name_headers(headers, &dynamic_node_name)?;
    // Advertise per-connection max concurrent streams so the backend can
    // respect the proxy's capacity limit.
    let max_streams = state.config.tunnel_max_streams.unwrap_or(128);
    headers.insert("X-Tunnel-Max-Streams", http::HeaderValue::from(max_streams));

    // Parse host:port from URL
    let uri: http::Uri = ws_url.parse()?;
    let host = uri
        .host()
        .ok_or_else(|| anyhow::anyhow!("missing host in tunnel URL"))?;
    let is_tls = uri.scheme_str() == Some("wss");
    let port = uri.port_u16().unwrap_or(if is_tls { 443 } else { 80 });

    // TCP connect with timeout
    let connect_timeout = state
        .config
        .tunnel_connect_timeout()
        .expect("validated config should resolve tunnel connect timeout");
    let tcp_stream = connect_tunnel_tcp(state, host, port, connect_timeout).await?;

    // Configure TCP parameters via socket2
    configure_tcp_socket(&tcp_stream, state);

    // WebSocket upgrade (with TLS if wss://)
    let connector = if is_tls {
        Some(tokio_tungstenite::Connector::Rustls(Arc::clone(
            &state.tunnel_tls_config,
        )))
    } else {
        None
    };
    // Match Python-side _MAX_FRAME_SIZE (64 MiB) to prevent tungstenite's
    // default 16 MiB limit from rejecting large AI API payloads (multi-image
    // base64 requests can exceed 16 MiB).
    let ws_config = WebSocketConfig {
        max_frame_size: Some(64 << 20),
        max_message_size: Some(64 << 20),
        ..Default::default()
    };
    let handshake_timeout = connect_timeout;
    let (ws_stream, _response) = tokio::time::timeout(
        handshake_timeout,
        tokio_tungstenite::client_async_tls_with_config(
            request,
            tcp_stream,
            Some(ws_config),
            connector,
        ),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "tunnel WebSocket handshake timeout ({}ms)",
            handshake_timeout.as_millis()
        )
    })??;
    let security = if server.tunnel_security == crate::config::TunnelSecurity::NonTlsRequired {
        let key = server
            .tunnel_encryption_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("secure tunnel requires tunnel_encryption_key"))?;
        Some(Arc::new(SecureFrameCodec::new(
            key,
            &security_session,
            TunnelSecurityRole::Client,
        )?))
    } else {
        None
    };
    let stale_timeout = state
        .config
        .tunnel_stale_timeout()
        .expect("validated config should resolve tunnel stale timeout");
    let ping_interval = state
        .config
        .tunnel_ping_interval()
        .expect("validated config should resolve tunnel ping interval");
    debug!(
        conn = conn_idx,
        tcp_keepalive_secs = state.config.tunnel_tcp_keepalive_secs,
        tcp_nodelay = state.config.tunnel_tcp_nodelay,
        connect_timeout_ms = connect_timeout.as_millis(),
        stale_timeout_ms = stale_timeout.as_millis(),
        ping_interval_ms = ping_interval.as_millis(),
        "tunnel connected"
    );
    server.tunnel_metrics.record_connect_success();
    let connected_at = Instant::now();

    // NOTE: reconnect_attempts reset is handled by the caller (mod.rs)
    // based on how long the connection stayed alive.

    // Split into read/write halves
    let (ws_sink, ws_read) = futures_util::StreamExt::split(ws_stream);

    // Spawn writer task (with WebSocket ping keepalive)
    let (frame_tx, writer_handle) = writer::spawn_writer_with_metrics_and_security(
        ws_sink,
        ping_interval,
        Some(Arc::clone(&server.tunnel_metrics)),
        security.clone(),
    );
    let mut writer_handle = super::task::SessionTask::new(writer_handle);
    send_protocol_v3_hello(&frame_tx, &security_session, state).await;
    let (session_drain_tx, session_drain_rx) = watch::channel(*drain.borrow());
    let forward_drain_tx = session_drain_tx.clone();
    let mut external_drain = drain;
    let forward_drain = super::task::SessionTask::new(tokio::spawn(async move {
        loop {
            if *external_drain.borrow() {
                let _ = forward_drain_tx.send(true);
                break;
            }
            if external_drain.changed().await.is_err() {
                break;
            }
        }
    }));
    let drain_signal = super::task::SessionTask::new(spawn_drain_signal(
        conn_idx,
        frame_tx.clone(),
        session_drain_rx.clone(),
        state.config.tunnel_drain_deadline_ms,
    ));

    // Spawn heartbeat task (only for primary connection to avoid
    // resetting shared atomic metrics via swap(0))
    let hb_handle = if conn_idx == 0 {
        heartbeat::spawn(
            Arc::clone(state),
            Arc::clone(server),
            frame_tx.clone(),
            shutdown.clone(),
        )
    } else {
        heartbeat::spawn_noop()
    };

    // Run dispatcher (blocks until disconnect or shutdown).
    // Also watch for writer exit — if the write half dies (e.g. the peer
    // closed the connection) but the read half stays open, dispatcher would
    // block forever on `ws_stream.next()`.  Monitoring `writer_handle`
    // ensures we detect this and trigger a reconnect promptly.
    let state_clone = Arc::clone(state);
    let server_clone = Arc::clone(server);
    let outcome = {
        let dispatch = dispatcher::run_with_security(
            state_clone,
            server_clone,
            ws_read,
            frame_tx.clone(),
            hb_handle,
            session_drain_rx,
            security.clone(),
        );
        tokio::pin!(dispatch);
        tokio::select! {
        result = &mut dispatch => {
            match result {
                Ok(()) => Ok(TunnelOutcome::Disconnected),
                Err(e) => {
                    server
                        .tunnel_metrics
                        .record_error("dispatcher_error", &e.to_string());
                    Err(e)
                }
            }
        }
        writer_result = &mut writer_handle => {
            frame_tx.close();
            let _ = tokio::time::timeout(Duration::from_secs(1), &mut dispatch).await;
            match writer_result {
                Ok(()) => warn!("writer task exited normally, triggering reconnect"),
                Err(e) => {
                    if e.is_panic() {
                        tracing::error!(error = %e, "writer task panicked, triggering reconnect");
                        server
                            .tunnel_metrics
                            .record_error("writer_task_panic", &e.to_string());
                    } else {
                        warn!(error = %e, "writer task cancelled, triggering reconnect");
                        server
                            .tunnel_metrics
                            .record_error("writer_task_cancelled", &e.to_string());
                    }
                }
            }
            Ok(TunnelOutcome::Disconnected)
        }
        _ = shutdown.changed() => {
            debug!("shutdown during tunnel dispatch");
            let _ = session_drain_tx.send(true);
            let deadline = Duration::from_millis(state.config.tunnel_drain_deadline_ms).saturating_add(Duration::from_secs(1));
            let _ = tokio::time::timeout(deadline, &mut dispatch).await;
            Ok(TunnelOutcome::Shutdown)
        }
        }
    };

    // Drop our sender; the writer will exit once all stream handler clones
    // are also dropped (i.e. after they finish their in-flight work).
    drop(frame_tx);
    forward_drain.abort();
    let _ = forward_drain.await;
    if !drain_signal.is_finished() {
        drain_signal.abort();
        let _ = drain_signal.await;
    }

    if !writer_handle.is_finished() {
        let flush_timeout = if *session_drain_tx.borrow() {
            Duration::from_millis(state.config.tunnel_drain_deadline_ms)
        } else {
            Duration::from_secs(1)
        };
        if tokio::time::timeout(flush_timeout, &mut writer_handle)
            .await
            .is_err()
        {
            writer_handle.abort();
            let _ = writer_handle.await;
        }
    }

    let connected_for = connected_at.elapsed();
    match &outcome {
        Ok(TunnelOutcome::Shutdown) => info!(
            conn = conn_idx,
            connected_duration_ms = connected_for.as_millis() as u64,
            close_reason = "shutdown",
            "tunnel session ending"
        ),
        Ok(TunnelOutcome::Disconnected) => info!(
            conn = conn_idx,
            connected_duration_ms = connected_for.as_millis() as u64,
            close_reason = "disconnected",
            "tunnel session ending"
        ),
        Err(error) => warn!(
            conn = conn_idx,
            connected_duration_ms = connected_for.as_millis() as u64,
            close_reason = "error",
            error = %error,
            "tunnel session ending"
        ),
    }

    server.tunnel_metrics.record_disconnect(connected_for);

    debug!("tunnel disconnected");
    outcome
}

async fn send_protocol_v3_hello(
    frame_tx: &writer::FrameSender,
    security_session: &str,
    state: &Arc<AppState>,
) {
    let hello = super::protocol::Frame::control(
        super::protocol::MsgType::Hello,
        serde_json::to_vec(&HelloPayload {
            protocol_version: CURRENT_TUNNEL_PROTOCOL_VERSION,
            capabilities: vec![
                "flow-control".to_string(),
                "reset-stream".to_string(),
                "graceful-drain".to_string(),
                "load-report".to_string(),
            ],
            session_id: Some(security_session.to_string()),
            replica_id: None,
        })
        .expect("hello payload should serialize"),
    );
    let settings = super::protocol::Frame::control(
        super::protocol::MsgType::Settings,
        serde_json::to_vec(&SettingsPayload {
            initial_stream_window_bytes: state.config.tunnel_stream_initial_window_bytes,
            min_window_update_bytes: state
                .config
                .tunnel_stream_initial_window_bytes
                .saturating_div(4)
                .max(1),
            drain_deadline_ms: state.config.tunnel_drain_deadline_ms,
        })
        .expect("settings payload should serialize"),
    );
    let _ = tokio::time::timeout(Duration::from_millis(250), frame_tx.send(hello)).await;
    let _ = tokio::time::timeout(Duration::from_millis(250), frame_tx.send(settings)).await;
}

fn spawn_drain_signal(
    conn_idx: usize,
    frame_tx: writer::FrameSender,
    mut drain: watch::Receiver<bool>,
    drain_deadline_ms: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if !*drain.borrow() {
            loop {
                if drain.changed().await.is_err() {
                    return;
                }
                if *drain.borrow() {
                    break;
                }
            }
        }

        debug!(conn = conn_idx, "sending GOAWAY for tunnel drain");
        match tokio::time::timeout(
            Duration::from_millis(250),
            frame_tx.send(super::protocol::Frame::control(
                super::protocol::MsgType::GoAway,
                serde_json::to_vec(&aether_contracts::tunnel::GoAwayPayload {
                    last_accepted_stream_id: u32::MAX,
                    drain_deadline_ms,
                    reason: "tunnel drain requested".to_string(),
                })
                .expect("goaway payload should serialize"),
            )),
        )
        .await
        {
            Ok(Ok(())) => info!(conn = conn_idx, "sent GOAWAY for tunnel drain"),
            Ok(Err(error)) => warn!(
                conn = conn_idx,
                error = ?error,
                "failed to queue GOAWAY for tunnel drain"
            ),
            Err(_) => warn!(
                conn = conn_idx,
                "timed out queueing GOAWAY for tunnel drain"
            ),
        }
    })
}

async fn connect_tunnel_tcp(
    state: &Arc<AppState>,
    host: &str,
    port: u16,
    connect_timeout: Duration,
) -> Result<TcpStream, anyhow::Error> {
    if let Some(proxy_url) = state.config.effective_aether_outbound_proxy_url() {
        let proxy = UpstreamProxyConfig::parse(proxy_url)
            .map_err(|err| anyhow::anyhow!("Aether outbound proxy URL invalid: {err}"))?;
        debug!(
            proxy_url = %proxy.redacted_url(),
            host = %host,
            port = port,
            "connecting tunnel via Aether egress proxy"
        );
        return tokio::time::timeout(
            connect_timeout,
            connect_target_via_proxy(
                &proxy,
                host,
                port,
                ProxyConnectOptions {
                    connect_timeout,
                    tcp_nodelay: state.config.tunnel_tcp_nodelay,
                    tcp_keepalive: (state.config.tunnel_tcp_keepalive_secs > 0)
                        .then(|| Duration::from_secs(state.config.tunnel_tcp_keepalive_secs)),
                    ip_family: state.config.tunnel_ip_family(),
                },
            ),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "tunnel outbound proxy TCP connect timeout ({}ms)",
                connect_timeout.as_millis()
            )
        })?
        .map_err(anyhow::Error::from);
    }

    let ip_family = state.config.tunnel_ip_family();
    tokio::time::timeout(
        connect_timeout,
        connect_direct_tunnel_tcp(host, port, ip_family),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "tunnel TCP connect timeout ({}ms)",
            connect_timeout.as_millis()
        )
    })?
    .map_err(anyhow::Error::from)
}

async fn connect_direct_tunnel_tcp(
    host: &str,
    port: u16,
    ip_family: IpFamily,
) -> io::Result<TcpStream> {
    let resolved =
        aether_http::lookup_host_with_limits(host, port, aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT)
            .await
            .map_err(|err| io::Error::other(format!("tunnel DNS failed: {err}")))?;
    let addrs = filter_socket_addrs(resolved, ip_family);

    if addrs.is_empty() {
        return Err(io::Error::other(ip_family.no_address_message("tunnel")));
    }

    let mut last_error = None;
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| io::Error::other("tunnel DNS returned no addresses")))
}

fn filter_socket_addrs(
    addrs: impl IntoIterator<Item = SocketAddr>,
    ip_family: IpFamily,
) -> Vec<SocketAddr> {
    addrs
        .into_iter()
        .filter(|addr| ip_family.allows(*addr))
        .collect()
}

/// Configure TCP keepalive and NODELAY on an established socket.
fn configure_tcp_socket(stream: &TcpStream, state: &Arc<AppState>) {
    let sock_ref = socket2::SockRef::from(stream);

    if state.config.tunnel_tcp_keepalive_secs > 0 {
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(state.config.tunnel_tcp_keepalive_secs))
            .with_interval(Duration::from_secs(5));
        #[cfg(not(target_os = "windows"))]
        let keepalive = keepalive.with_retries(3);
        if let Err(e) = sock_ref.set_tcp_keepalive(&keepalive) {
            warn!(error = %e, "failed to set TCP keepalive on tunnel socket");
        }
    }

    if state.config.tunnel_tcp_nodelay {
        if let Err(e) = sock_ref.set_nodelay(true) {
            warn!(error = %e, "failed to set TCP_NODELAY on tunnel socket");
        }
    }
}

/// Build rustls ClientConfig with system root certificates.
pub fn build_tls_config() -> rustls::ClientConfig {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

fn build_tunnel_url(server: &ServerContext) -> String {
    let base = server.aether_url.trim_end_matches('/');
    let ws_base = if base.starts_with("https://") {
        base.replacen("https://", "wss://", 1)
    } else if base.starts_with("http://") {
        base.replacen("http://", "ws://", 1)
    } else {
        format!("wss://{}", base)
    };
    format!("{}/api/internal/proxy-tunnel", ws_base)
}

fn insert_ascii_header(
    headers: &mut http::HeaderMap,
    name: &'static str,
    value: &str,
    field: &str,
) -> anyhow::Result<()> {
    if !value.is_ascii() {
        anyhow::bail!(
            "{field} contains non-ASCII characters and cannot be sent in the WebSocket handshake"
        );
    }
    let value = http::HeaderValue::from_str(value)
        .map_err(|err| anyhow::anyhow!("{field} is not a valid WebSocket header value: {err}"))?;
    headers.insert(name, value);
    Ok(())
}

// The handshake transcript has a fixed set of wire fields. Keep the explicit
// arguments and ordering so the client remains interoperable with existing
// tunnel servers.
#[allow(clippy::too_many_arguments)]
fn insert_tunnel_security_handshake_headers(
    headers: &mut http::HeaderMap,
    key: &str,
    node_id: &str,
    tunnel_generation: &str,
    session: &str,
    protocol_version: u8,
    timestamp_unix_secs: u64,
    nonce: &str,
) -> anyhow::Result<()> {
    let signature = sign_tunnel_security_handshake_for_generation(
        key,
        node_id,
        tunnel_generation,
        TUNNEL_SECURITY_NON_TLS_REQUIRED,
        session,
        protocol_version,
        timestamp_unix_secs,
        nonce,
    )?;
    headers.insert(
        TUNNEL_SECURITY_HEADER,
        http::HeaderValue::from_static(TUNNEL_SECURITY_NON_TLS_REQUIRED),
    );
    insert_ascii_header(
        headers,
        TUNNEL_SECURITY_SESSION_HEADER,
        session,
        "tunnel security session",
    )?;
    insert_ascii_header(
        headers,
        TUNNEL_SECURITY_PROOF_TIMESTAMP_HEADER,
        &timestamp_unix_secs.to_string(),
        "tunnel security proof timestamp",
    )?;
    insert_ascii_header(
        headers,
        TUNNEL_SECURITY_PROOF_NONCE_HEADER,
        nonce,
        "tunnel security proof nonce",
    )?;
    insert_ascii_header(
        headers,
        TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER,
        &signature,
        "tunnel security proof signature",
    )?;
    Ok(())
}

fn insert_node_name_headers(headers: &mut http::HeaderMap, node_name: &str) -> anyhow::Result<()> {
    if node_name.is_ascii() {
        return insert_ascii_header(headers, "X-Node-Name", node_name, "node_name");
    }

    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(node_name.as_bytes());
    let value = http::HeaderValue::from_str(&encoded).map_err(|err| {
        anyhow::anyhow!("encoded node_name is not a valid WebSocket header value: {err}")
    })?;
    headers.insert(TUNNEL_NODE_NAME_B64_HEADER, value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    fn mixed_addrs() -> Vec<SocketAddr> {
        vec![
            SocketAddr::from((Ipv6Addr::LOCALHOST, 443)),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 443)),
        ]
    }

    #[test]
    fn filter_socket_addrs_keeps_all_addresses_by_default() {
        let addrs = filter_socket_addrs(mixed_addrs(), IpFamily::Any);

        assert_eq!(addrs.len(), 2);
        assert!(addrs[0].is_ipv6());
        assert!(addrs[1].is_ipv4());
    }

    #[test]
    fn filter_socket_addrs_keeps_only_ipv4_addresses() {
        let addrs = filter_socket_addrs(mixed_addrs(), IpFamily::Ipv4Only);

        assert_eq!(addrs, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 443))]);
    }

    #[test]
    fn filter_socket_addrs_keeps_only_ipv6_addresses() {
        let addrs = filter_socket_addrs(mixed_addrs(), IpFamily::Ipv6Only);

        assert_eq!(addrs, vec![SocketAddr::from((Ipv6Addr::LOCALHOST, 443))]);
    }

    #[test]
    fn node_name_header_uses_legacy_header_for_ascii() {
        let mut headers = http::HeaderMap::new();

        insert_node_name_headers(&mut headers, "edge-1").expect("header should insert");

        assert_eq!(
            headers
                .get("x-node-name")
                .and_then(|value| value.to_str().ok()),
            Some("edge-1")
        );
        assert!(headers.get(TUNNEL_NODE_NAME_B64_HEADER).is_none());
    }

    #[test]
    fn node_name_header_encodes_non_ascii_name() {
        let mut headers = http::HeaderMap::new();

        insert_node_name_headers(&mut headers, "日本节点").expect("header should insert");

        assert!(headers.get("x-node-name").is_none());
        let encoded = headers
            .get(TUNNEL_NODE_NAME_B64_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("encoded node name header should be present");
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .expect("encoded value should decode");
        assert_eq!(decoded, "日本节点".as_bytes());
    }

    #[test]
    fn tunnel_security_headers_include_verifiable_psk_proof() {
        let key = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
        let mut headers = http::HeaderMap::new();
        insert_tunnel_security_handshake_headers(
            &mut headers,
            &key,
            "node-1",
            "generation-1",
            "0123456789abcdef0123456789abcdef",
            CURRENT_TUNNEL_PROTOCOL_VERSION,
            1_700_000_000,
            "abcdef0123456789abcdef0123456789",
        )
        .expect("security proof headers");

        let signature = headers[TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER]
            .to_str()
            .expect("signature header");
        assert_eq!(
            headers[TUNNEL_SECURITY_HEADER],
            TUNNEL_SECURITY_NON_TLS_REQUIRED
        );
        assert_eq!(
            headers[TUNNEL_SECURITY_PROOF_TIMESTAMP_HEADER],
            "1700000000"
        );
        assert!(
            aether_contracts::tunnel_security::verify_tunnel_security_handshake_for_generation(
                &key,
                "node-1",
                "generation-1",
                TUNNEL_SECURITY_NON_TLS_REQUIRED,
                "0123456789abcdef0123456789abcdef",
                CURRENT_TUNNEL_PROTOCOL_VERSION,
                1_700_000_000,
                "abcdef0123456789abcdef0123456789",
                signature,
            )
        );
    }
}
