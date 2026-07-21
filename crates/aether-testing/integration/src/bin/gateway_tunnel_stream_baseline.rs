// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use aether_gateway::tunnel_protocol as protocol;
use aether_testkit::{
    fetch_prometheus_samples, find_metric_value_u64, init_test_runtime_for, run_http_load_probe,
    HttpLoadProbeConfig, HttpLoadProbeResponseMode, HttpLoadProbeResult, PrometheusSample,
    TunnelHarness, TunnelHarnessConfig,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use serde::Serialize;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";

#[derive(Debug, Clone)]
struct GatewayTunnelBaselineConfig {
    total_requests: usize,
    concurrency: usize,
    request_body_bytes: usize,
    tunnel_connections: usize,
    outbound_queue_capacity: usize,
    close_one_connection_after: Option<Duration>,
    require_acceptance: bool,
    timeout: Duration,
    output_path: Option<PathBuf>,
}

impl Default for GatewayTunnelBaselineConfig {
    fn default() -> Self {
        Self {
            total_requests: 200,
            concurrency: 20,
            request_body_bytes: 6 * 1024 * 1024,
            tunnel_connections: 4,
            outbound_queue_capacity: 512,
            close_one_connection_after: None,
            require_acceptance: false,
            timeout: Duration::from_secs(10),
            output_path: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct GatewayTunnelBaselineReport {
    suite: &'static str,
    config: GatewayTunnelEffectiveConfig,
    scenario: HttpLoadProbeResult,
    tunnel_metrics: TunnelMetricsSnapshot,
    acceptance: AcceptanceReport,
}

#[derive(Debug, Serialize)]
struct GatewayTunnelEffectiveConfig {
    total_requests: usize,
    concurrency: usize,
    request_body_bytes: usize,
    tunnel_connections: usize,
    outbound_queue_capacity: usize,
    close_one_connection_after_ms: Option<u64>,
    timeout_ms: u64,
}

#[derive(Debug, Serialize)]
struct TunnelMetricsSnapshot {
    proxy_connections: u64,
    active_streams: u64,
    outbound_queue_depth_total: u64,
    outbound_queue_depth_max: u64,
    outbound_queue_capacity_total: u64,
    outbound_queue_rejected_full_total: u64,
    outbound_queue_rejected_closed_total: u64,
    proxy_connection_congested_total: u64,
    proxy_connection_write_latency_last_us_max: u64,
    proxy_connection_write_latency_ewma_us_max: u64,
    body_backpressure_total: u64,
    flow_window_blocked_ms: u64,
    connection_health_score: u64,
    stream_reset_total: u64,
    stream_reset_reasons: BTreeMap<String, u64>,
    drain_total: u64,
    drain_reasons: BTreeMap<String, u64>,
    scheduler_selected_conn_total: u64,
    proxy_connections_protocol_v1: u64,
    proxy_connections_protocol_v2: u64,
    proxy_connections_protocol_v3: u64,
}

#[derive(Debug, Serialize)]
struct AcceptanceReport {
    required: bool,
    passed: bool,
    success_rate_bps: u64,
    min_success_rate_bps: u64,
    congestion_free: bool,
    no_queue_full_rejections: bool,
    reasons: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("gateway-tunnel-stream-baseline");
    let config = parse_args(std::env::args().skip(1).collect())?;
    let report = run_suite(&config).await?;
    let raw = serde_json::to_string_pretty(&report)?;
    println!("{raw}");
    if let Some(path) = config.output_path.as_ref() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{raw}\n"))?;
    }
    Ok(())
}

async fn run_suite(
    config: &GatewayTunnelBaselineConfig,
) -> Result<GatewayTunnelBaselineReport, Box<dyn std::error::Error>> {
    let tunnel = TunnelHarness::start(TunnelHarnessConfig {
        outbound_queue_capacity: config.outbound_queue_capacity,
        ..TunnelHarnessConfig::default()
    })
    .await?;
    let mut peers = connect_protocol_peers(tunnel.base_url(), config.tunnel_connections).await?;
    let fault_injection = config.close_one_connection_after.map(|delay| {
        let peer = peers.pop();
        tokio::spawn(async move {
            if let Some(peer) = peer {
                tokio::time::sleep(delay).await;
                peer.abort();
            }
        })
    });

    let result = run_http_load_probe(&HttpLoadProbeConfig {
        url: format!(
            "{tunnel_base}{TUNNEL_RELAY_PATH_PREFIX}/node-baseline",
            tunnel_base = tunnel.base_url()
        ),
        method: Method::POST,
        headers: std::collections::BTreeMap::from([(
            "content-type".to_string(),
            "application/octet-stream".to_string(),
        )]),
        body: Some(relay_envelope(config.request_body_bytes)),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
    })
    .await
    .map_err(std::io::Error::other)?;

    let tunnel_metrics = capture_tunnel_metrics(tunnel.base_url()).await?;
    if let Some(task) = fault_injection {
        let _ = task.await;
    }
    drop(peers);
    let acceptance = evaluate_acceptance(config, &result, &tunnel_metrics);
    if config.require_acceptance && !acceptance.passed {
        return Err(std::io::Error::other(format!(
            "gateway tunnel acceptance failed: {:?}",
            acceptance.reasons
        ))
        .into());
    }

    Ok(GatewayTunnelBaselineReport {
        suite: "gateway_tunnel_stream_baseline",
        config: GatewayTunnelEffectiveConfig {
            total_requests: config.total_requests,
            concurrency: config.concurrency,
            request_body_bytes: config.request_body_bytes,
            tunnel_connections: config.tunnel_connections,
            outbound_queue_capacity: config.outbound_queue_capacity,
            close_one_connection_after_ms: config
                .close_one_connection_after
                .map(|duration| duration.as_millis() as u64),
            timeout_ms: config.timeout.as_millis() as u64,
        },
        scenario: result,
        tunnel_metrics,
        acceptance,
    })
}

fn relay_envelope(request_body_bytes: usize) -> Vec<u8> {
    let meta = protocol::RequestMeta {
        method: "POST".to_string(),
        url: "https://baseline.example/v1/chat/completions".to_string(),
        headers: std::collections::HashMap::from([(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        stream: true,
        request_timeout_ms: None,
        stream_first_byte_timeout_ms: None,
        timeout: 30,
        follow_redirects: None,
        http1_only: false,
        provider_id: None,
        endpoint_id: None,
        key_id: None,
        transport_profile: None,
    };
    let meta_json = serde_json::to_vec(&meta).expect("tunnel relay metadata should serialize");
    let body = vec![b'x'; request_body_bytes];
    let mut envelope = Vec::with_capacity(4 + meta_json.len() + body.len());
    envelope.extend_from_slice(&(meta_json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_json);
    envelope.extend_from_slice(&body);
    envelope
}

async fn connect_protocol_peers(
    tunnel_base_url: &str,
    count: usize,
) -> Result<Vec<tokio::task::JoinHandle<()>>, Box<dyn std::error::Error>> {
    let count = count.max(1);
    let mut peers = Vec::with_capacity(count);
    for index in 0..count {
        peers.push(connect_protocol_peer(tunnel_base_url, index).await?);
    }
    Ok(peers)
}

async fn connect_protocol_peer(
    tunnel_base_url: &str,
    index: usize,
) -> Result<tokio::task::JoinHandle<()>, Box<dyn std::error::Error>> {
    let ws_url = format!(
        "{}{}",
        tunnel_base_url.replace("http://", "ws://"),
        PROXY_TUNNEL_PATH
    );
    let request = ws_url.into_client_request()?;
    let mut request = request;
    request
        .headers_mut()
        .insert("x-node-id", http::HeaderValue::from_static("node-baseline"));
    request.headers_mut().insert(
        aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
        http::HeaderValue::from_static(
            aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION_STR,
        ),
    );
    request.headers_mut().insert(
        "x-node-name",
        http::HeaderValue::from_str(&format!("proxy-baseline-{index}"))?,
    );
    request.headers_mut().insert(
        "x-tunnel-max-streams",
        http::HeaderValue::from_static("128"),
    );

    let (socket, _response) = tokio_tungstenite::connect_async(request).await?;
    let (mut sink, mut stream) = socket.split();
    sink.send(Message::Binary(
        protocol::encode_hello(&protocol::HelloPayload {
            protocol_version: aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION,
            capabilities: vec![
                "flow-control".to_string(),
                "reset-stream".to_string(),
                "graceful-drain".to_string(),
            ],
            session_id: Some(format!("baseline-session-{index}")),
            replica_id: Some(format!("baseline-replica-{index}")),
        })
        .into(),
    ))
    .await?;
    sink.send(Message::Binary(
        protocol::encode_settings(&protocol::SettingsPayload {
            initial_stream_window_bytes: 4 * 1024 * 1024,
            min_window_update_bytes: 1024 * 1024,
            drain_deadline_ms: 30_000,
        })
        .into(),
    ))
    .await?;
    Ok(tokio::spawn(async move {
        while let Some(message) = stream.next().await {
            let Ok(message) = message else {
                break;
            };
            match message {
                Message::Binary(data)
                    if handle_binary_frame(&mut sink, data.to_vec()).await.is_err() =>
                {
                    break;
                }
                Message::Ping(payload)
                    if sink.send(Message::Pong(payload.clone())).await.is_err() =>
                {
                    break;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = sink.close().await;
    }))
}

async fn capture_tunnel_metrics(
    base_url: &str,
) -> Result<TunnelMetricsSnapshot, Box<dyn std::error::Error>> {
    let samples = fetch_prometheus_samples(&format!("{base_url}/metrics"))
        .await
        .map_err(std::io::Error::other)?;
    Ok(TunnelMetricsSnapshot {
        proxy_connections: find_metric_value_u64(&samples, "tunnel_proxy_connections", &[])
            .unwrap_or_default(),
        active_streams: find_metric_value_u64(&samples, "tunnel_active_streams", &[])
            .unwrap_or_default(),
        outbound_queue_depth_total: find_metric_value_u64(
            &samples,
            "tunnel_proxy_outbound_queue_depth_total",
            &[],
        )
        .unwrap_or_default(),
        outbound_queue_depth_max: find_metric_value_u64(
            &samples,
            "tunnel_proxy_outbound_queue_depth_max",
            &[],
        )
        .unwrap_or_default(),
        outbound_queue_capacity_total: find_metric_value_u64(
            &samples,
            "tunnel_proxy_outbound_queue_capacity_total",
            &[],
        )
        .unwrap_or_default(),
        outbound_queue_rejected_full_total: find_metric_value_u64(
            &samples,
            "tunnel_proxy_outbound_queue_rejected_full_total",
            &[],
        )
        .unwrap_or_default(),
        outbound_queue_rejected_closed_total: find_metric_value_u64(
            &samples,
            "tunnel_proxy_outbound_queue_rejected_closed_total",
            &[],
        )
        .unwrap_or_default(),
        proxy_connection_congested_total: find_metric_value_u64(
            &samples,
            "tunnel_proxy_connection_congested_total",
            &[],
        )
        .unwrap_or_default(),
        proxy_connection_write_latency_last_us_max: find_metric_value_u64(
            &samples,
            "tunnel_proxy_connection_write_latency_last_us_max",
            &[],
        )
        .unwrap_or_default(),
        proxy_connection_write_latency_ewma_us_max: find_metric_value_u64(
            &samples,
            "tunnel_proxy_connection_write_latency_ewma_us_max",
            &[],
        )
        .unwrap_or_default(),
        body_backpressure_total: find_metric_value_u64(
            &samples,
            "tunnel_body_backpressure_total",
            &[],
        )
        .unwrap_or_default(),
        flow_window_blocked_ms: find_metric_value_u64(
            &samples,
            "tunnel_flow_window_blocked_ms",
            &[],
        )
        .unwrap_or_default(),
        connection_health_score: find_metric_value_u64(
            &samples,
            "tunnel_connection_health_score",
            &[],
        )
        .unwrap_or_default(),
        stream_reset_total: find_metric_value_u64(
            &samples,
            "tunnel_stream_reset_total",
            &[("reason", "all")],
        )
        .unwrap_or_default(),
        stream_reset_reasons: collect_reason_metrics(&samples, "tunnel_stream_reset_total"),
        drain_total: find_metric_value_u64(&samples, "tunnel_drain_total", &[("reason", "all")])
            .unwrap_or_default(),
        drain_reasons: collect_reason_metrics(&samples, "tunnel_drain_total"),
        scheduler_selected_conn_total: find_metric_value_u64(
            &samples,
            "tunnel_scheduler_selected_conn_total",
            &[],
        )
        .unwrap_or_default(),
        proxy_connections_protocol_v1: find_metric_value_u64(
            &samples,
            "tunnel_proxy_connections_protocol_v1",
            &[],
        )
        .unwrap_or_default(),
        proxy_connections_protocol_v2: find_metric_value_u64(
            &samples,
            "tunnel_proxy_connections_protocol_v2",
            &[],
        )
        .unwrap_or_default(),
        proxy_connections_protocol_v3: find_metric_value_u64(
            &samples,
            "tunnel_proxy_connections_protocol_v3",
            &[],
        )
        .unwrap_or_default(),
    })
}

fn collect_reason_metrics(
    samples: &[PrometheusSample],
    metric_name: &str,
) -> BTreeMap<String, u64> {
    samples
        .iter()
        .filter(|sample| {
            (sample.name == metric_name || sample.name.ends_with(&format!("_{metric_name}")))
                && sample
                    .labels
                    .get("reason")
                    .is_some_and(|reason| reason != "all")
        })
        .filter_map(|sample| {
            let reason = sample.labels.get("reason")?.clone();
            let value = sample.value.parse::<u64>().ok()?;
            Some((reason, value))
        })
        .collect()
}

fn evaluate_acceptance(
    config: &GatewayTunnelBaselineConfig,
    result: &HttpLoadProbeResult,
    metrics: &TunnelMetricsSnapshot,
) -> AcceptanceReport {
    let min_success_rate_bps = if config.require_acceptance { 9_950 } else { 1 };
    let successful_statuses = result
        .status_counts
        .iter()
        .filter(|(status, _)| (200u16..300u16).contains(status))
        .map(|(_, count)| *count)
        .sum::<usize>();
    let responses_received = result.status_counts.values().sum::<usize>();
    let failures_without_response = result.total_requests.saturating_sub(responses_received);
    let failures_after_response = result
        .failed_requests
        .saturating_sub(failures_without_response);
    // Body failures now retain their HTTP status in the load report. Use a conservative lower
    // bound so a truncated 2xx response cannot make tunnel acceptance pass as a success.
    let successful_requests = successful_statuses.saturating_sub(failures_after_response);
    let success_rate_bps = if result.total_requests == 0 {
        0
    } else {
        ((successful_requests as u128) * 10_000 / (result.total_requests as u128)) as u64
    };
    let congestion_free = metrics.proxy_connection_congested_total == 0;
    let no_queue_full_rejections = metrics.outbound_queue_rejected_full_total == 0;
    let mut reasons = Vec::new();
    if success_rate_bps < min_success_rate_bps {
        reasons.push(format!(
            "success rate {} bps below required {} bps",
            success_rate_bps, min_success_rate_bps
        ));
    }
    if !congestion_free {
        reasons.push(format!(
            "connection congestion total is {}",
            metrics.proxy_connection_congested_total
        ));
    }
    if !no_queue_full_rejections {
        reasons.push(format!(
            "outbound queue full rejections total is {}",
            metrics.outbound_queue_rejected_full_total
        ));
    }

    AcceptanceReport {
        required: config.require_acceptance,
        passed: reasons.is_empty(),
        success_rate_bps,
        min_success_rate_bps,
        congestion_free,
        no_queue_full_rejections,
        reasons,
    }
}

async fn handle_binary_frame<S>(
    sink: &mut S,
    data: Vec<u8>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let Some(header) = protocol::FrameHeader::parse(&data) else {
        return Ok(());
    };
    match header.msg_type {
        protocol::PING => {
            let payload = protocol::frame_payload_by_header(&data, &header).unwrap_or(&[]);
            sink.send(Message::Binary(protocol::encode_pong(payload).into()))
                .await?;
        }
        protocol::REQUEST_HEADERS => {
            let payload = protocol::decode_payload(&data, &header).unwrap_or_default();
            let _ = serde_json::from_slice::<protocol::RequestMeta>(&payload);
        }
        protocol::REQUEST_BODY => {
            let payload = protocol::decode_payload(&data, &header).unwrap_or_default();
            if !payload.is_empty() {
                sink.send(Message::Binary(
                    protocol::encode_window_update(header.stream_id, payload.len() as u32).into(),
                ))
                .await?;
            }
            if header.flags & protocol::FLAG_END_STREAM == 0 {
                return Ok(());
            }
            let response_meta = protocol::ResponseMeta {
                status: 200,
                headers: vec![(
                    "content-type".to_string(),
                    "text/plain; charset=utf-8".to_string(),
                )],
            };
            let response_meta_json =
                serde_json::to_vec(&response_meta).expect("response metadata should serialize");
            sink.send(Message::Binary(
                protocol::encode_frame(
                    header.stream_id,
                    protocol::RESPONSE_HEADERS,
                    0,
                    &response_meta_json,
                )
                .into(),
            ))
            .await?;

            for chunk in [
                b"baseline-".as_slice(),
                b"tunnel-".as_slice(),
                b"stream".as_slice(),
            ] {
                sink.send(Message::Binary(
                    protocol::encode_frame(header.stream_id, protocol::RESPONSE_BODY, 0, chunk)
                        .into(),
                ))
                .await?;
            }

            sink.send(Message::Binary(
                protocol::encode_frame(header.stream_id, protocol::STREAM_END, 0, &[]).into(),
            ))
            .await?;
        }
        _ => {}
    }
    Ok(())
}

fn parse_args(
    args: Vec<String>,
) -> Result<GatewayTunnelBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = GatewayTunnelBaselineConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--requests" => config.total_requests = next_value(&mut iter, "--requests")?.parse()?,
            "--concurrency" => {
                config.concurrency = next_value(&mut iter, "--concurrency")?.parse()?
            }
            "--body-bytes" => {
                config.request_body_bytes = next_value(&mut iter, "--body-bytes")?.parse()?
            }
            "--tunnel-connections" => {
                config.tunnel_connections =
                    next_value(&mut iter, "--tunnel-connections")?.parse()?
            }
            "--outbound-queue-capacity" => {
                config.outbound_queue_capacity =
                    next_value(&mut iter, "--outbound-queue-capacity")?.parse()?
            }
            "--close-one-connection-after-ms" => {
                config.close_one_connection_after = Some(Duration::from_millis(
                    next_value(&mut iter, "--close-one-connection-after-ms")?.parse()?,
                ))
            }
            "--require-acceptance" => {
                config.require_acceptance = true;
            }
            "--timeout-ms" => {
                config.timeout =
                    Duration::from_millis(next_value(&mut iter, "--timeout-ms")?.parse()?)
            }
            "--output" => {
                config.output_path = Some(PathBuf::from(next_value(&mut iter, "--output")?))
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown argument: {other}"),
                )
                .into());
            }
        }
    }
    if config.request_body_bytes == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--body-bytes must be positive",
        )
        .into());
    }
    if config.tunnel_connections == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--tunnel-connections must be positive",
        )
        .into());
    }
    if config.outbound_queue_capacity == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--outbound-queue-capacity must be positive",
        )
        .into());
    }
    if config
        .close_one_connection_after
        .is_some_and(|duration| duration.is_zero())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "--close-one-connection-after-ms must be positive",
        )
        .into());
    }
    Ok(config)
}

fn next_value(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    iter.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("missing value for {flag}"),
        )
        .into()
    })
}

fn print_usage() {
    eprintln!(
        "usage: cargo run -p aether-integration-tests --bin gateway_tunnel_stream_baseline -- [--requests 200] [--concurrency 20] [--body-bytes 6291456] [--tunnel-connections 4] [--outbound-queue-capacity 512] [--close-one-connection-after-ms 1000] [--require-acceptance] [--timeout-ms 10000] [--output /tmp/gateway_tunnel_baseline.json]"
    );
}
