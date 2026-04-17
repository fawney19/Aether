use std::path::PathBuf;
use std::time::Duration;

use aether_gateway::tunnel_protocol as protocol;
use aether_testkit::{
    init_test_runtime_for, run_http_load_probe, HttpLoadProbeConfig, HttpLoadProbeResponseMode,
    HttpLoadProbeResult, TunnelHarness, TunnelHarnessConfig,
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
    timeout: Duration,
    output_path: Option<PathBuf>,
}

impl Default for GatewayTunnelBaselineConfig {
    fn default() -> Self {
        Self {
            total_requests: 200,
            concurrency: 20,
            timeout: Duration::from_secs(10),
            output_path: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct GatewayTunnelBaselineReport {
    suite: &'static str,
    scenario: HttpLoadProbeResult,
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
    let tunnel = TunnelHarness::start(TunnelHarnessConfig::default()).await?;
    let peer = connect_protocol_peer(tunnel.base_url()).await?;

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
        body: Some(relay_envelope()),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
    })
    .await
    .map_err(std::io::Error::other)?;

    drop(peer);

    Ok(GatewayTunnelBaselineReport {
        suite: "gateway_tunnel_stream_baseline",
        scenario: result,
    })
}

fn relay_envelope() -> Vec<u8> {
    let meta = protocol::RequestMeta {
        method: "POST".to_string(),
        url: "https://baseline.example/v1/chat/completions".to_string(),
        headers: std::collections::HashMap::from([(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        timeout: 30,
        follow_redirects: None,
        http1_only: false,
    };
    let meta_json = serde_json::to_vec(&meta).expect("tunnel relay metadata should serialize");
    let body = br#"{"model":"gpt-5","messages":[{"role":"user","content":"hello"}]}"#;
    let mut envelope = Vec::with_capacity(4 + meta_json.len() + body.len());
    envelope.extend_from_slice(&(meta_json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_json);
    envelope.extend_from_slice(body);
    envelope
}

async fn connect_protocol_peer(
    tunnel_base_url: &str,
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
        "x-node-name",
        http::HeaderValue::from_static("proxy-baseline"),
    );
    request.headers_mut().insert(
        "x-tunnel-max-streams",
        http::HeaderValue::from_static("128"),
    );

    let (socket, _response) = tokio_tungstenite::connect_async(request).await?;
    let (mut sink, mut stream) = socket.split();
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
        protocol::REQUEST_BODY if header.flags & protocol::FLAG_END_STREAM != 0 => {
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
        "usage: cargo run -p aether-testkit --bin gateway_tunnel_stream_baseline -- [--requests 200] [--concurrency 20] [--timeout-ms 10000] [--output /tmp/gateway_tunnel_baseline.json]"
    );
}
