use std::path::PathBuf;
use std::time::Duration;

use aether_gateway::tunnel_protocol as protocol;
use aether_gateway::GatewayDataConfig;
use aether_testkit::{
    init_test_runtime_for, reserve_local_port, run_http_load_probe, wait_until, GatewayHarness,
    GatewayHarnessConfig, HttpLoadProbeConfig, HttpLoadProbeResponseMode, HttpLoadProbeResult,
    ManagedPostgresServer, ManagedRedisServer,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use serde::Serialize;
use sqlx::{Connection, Executor, PgConnection};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";
const NODE_ID: &str = "node-owner-relay-baseline";

#[derive(Debug, Clone)]
struct MultiInstanceOwnerRelayBaselineConfig {
    total_requests: usize,
    concurrency: usize,
    timeout: Duration,
    chunk_delay: Duration,
    output_path: Option<PathBuf>,
    redis_url: Option<String>,
    postgres_url: Option<String>,
}

impl Default for MultiInstanceOwnerRelayBaselineConfig {
    fn default() -> Self {
        Self {
            total_requests: 200,
            concurrency: 20,
            timeout: Duration::from_secs(10),
            chunk_delay: Duration::ZERO,
            output_path: None,
            redis_url: None,
            postgres_url: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct MultiInstanceOwnerRelayBaselineReport {
    suite: &'static str,
    redis_url: String,
    postgres_url: String,
    owner_instance_id: &'static str,
    forwarder_instance_id: &'static str,
    direct_owner_relay: HttpLoadProbeResult,
    remote_owner_relay: HttpLoadProbeResult,
    relay_overhead_ms: RelayOverheadSnapshot,
}

#[derive(Debug, Serialize)]
struct RelayOverheadSnapshot {
    p50_delta_ms: i64,
    p95_delta_ms: i64,
    max_delta_ms: i64,
    mean_delta_ms: i64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("multi-instance-owner-relay-baseline");
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
    config: &MultiInstanceOwnerRelayBaselineConfig,
) -> Result<MultiInstanceOwnerRelayBaselineReport, Box<dyn std::error::Error>> {
    let managed_redis = if config.redis_url.is_none() {
        Some(ManagedRedisServer::start().await?)
    } else {
        None
    };
    let redis_url = config
        .redis_url
        .clone()
        .or_else(|| {
            managed_redis
                .as_ref()
                .map(|server| server.redis_url().to_string())
        })
        .expect("redis url should be resolved");

    let managed_postgres = if config.postgres_url.is_none() {
        Some(ManagedPostgresServer::start().await?)
    } else {
        None
    };
    let postgres_url = config
        .postgres_url
        .clone()
        .or_else(|| {
            managed_postgres
                .as_ref()
                .map(|server| server.database_url().to_string())
        })
        .expect("postgres url should be resolved");

    ensure_owner_relay_schema(&postgres_url).await?;

    let key_prefix = format!("aether-owner-relay-baseline-{}", std::process::id());
    let shared_data = GatewayDataConfig::from_postgres_url(postgres_url.clone(), false)
        .with_redis_url(redis_url.clone(), Some(key_prefix));

    let owner_port = reserve_local_port()?;
    let forwarder_port = reserve_local_port()?;
    let owner_base_url = format!("http://127.0.0.1:{owner_port}");
    let forwarder_base_url = format!("http://127.0.0.1:{forwarder_port}");

    let owner_gateway = GatewayHarness::start_on_port(
        GatewayHarnessConfig {
            upstream_base_url: "http://127.0.0.1:1".to_string(),
            data_config: Some(shared_data.clone()),
            max_in_flight_requests: None,
            distributed_request_gate: None,
            tunnel_instance_id: Some("gateway-owner".to_string()),
            tunnel_relay_base_url: Some(owner_base_url.clone()),
        },
        owner_port,
    )
    .await?;
    let forwarder_gateway = GatewayHarness::start_on_port(
        GatewayHarnessConfig {
            upstream_base_url: "http://127.0.0.1:1".to_string(),
            data_config: Some(shared_data),
            max_in_flight_requests: None,
            distributed_request_gate: None,
            tunnel_instance_id: Some("gateway-forwarder".to_string()),
            tunnel_relay_base_url: Some(forwarder_base_url.clone()),
        },
        forwarder_port,
    )
    .await?;

    let peer = connect_protocol_peer(owner_gateway.base_url(), config.chunk_delay).await?;

    wait_for_owner_attachment(&forwarder_base_url).await?;

    let direct_owner_relay = run_http_load_probe(&HttpLoadProbeConfig {
        url: format!(
            "{owner_base}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}",
            owner_base = owner_gateway.base_url()
        ),
        method: Method::POST,
        headers: relay_headers(),
        body: Some(relay_envelope()),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
    })
    .await
    .map_err(std::io::Error::other)?;

    let remote_owner_relay = run_http_load_probe(&HttpLoadProbeConfig {
        url: format!(
            "{forwarder_base}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}",
            forwarder_base = forwarder_gateway.base_url()
        ),
        method: Method::POST,
        headers: relay_headers(),
        body: Some(relay_envelope()),
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        timeout: config.timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
    })
    .await
    .map_err(std::io::Error::other)?;

    drop(peer);
    drop(forwarder_gateway);
    drop(owner_gateway);

    Ok(MultiInstanceOwnerRelayBaselineReport {
        suite: "multi_instance_owner_relay_baseline",
        redis_url,
        postgres_url,
        owner_instance_id: "gateway-owner",
        forwarder_instance_id: "gateway-forwarder",
        relay_overhead_ms: RelayOverheadSnapshot {
            p50_delta_ms: remote_owner_relay.p50_ms as i64 - direct_owner_relay.p50_ms as i64,
            p95_delta_ms: remote_owner_relay.p95_ms as i64 - direct_owner_relay.p95_ms as i64,
            max_delta_ms: remote_owner_relay.max_ms as i64 - direct_owner_relay.max_ms as i64,
            mean_delta_ms: remote_owner_relay.mean_ms as i64 - direct_owner_relay.mean_ms as i64,
        },
        direct_owner_relay,
        remote_owner_relay,
    })
}

async fn ensure_owner_relay_schema(postgres_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = PgConnection::connect(postgres_url).await?;
    connection
        .execute(
            r#"
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'proxynodestatus') THEN
        CREATE TYPE proxynodestatus AS ENUM ('online', 'offline');
    END IF;
END
$$;
"#,
        )
        .await?;
    connection
        .execute(
            r#"
CREATE TABLE IF NOT EXISTS system_configs (
    id VARCHAR(36) PRIMARY KEY,
    key VARCHAR(100) UNIQUE NOT NULL,
    value JSON NOT NULL,
    description TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
)
"#,
        )
        .await?;
    connection
        .execute(
            r#"
CREATE TABLE IF NOT EXISTS proxy_nodes (
    id VARCHAR(36) PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    ip VARCHAR(512) NOT NULL,
    port INTEGER NOT NULL,
    region VARCHAR(100) NULL,
    is_manual BOOLEAN NOT NULL DEFAULT FALSE,
    proxy_url VARCHAR(500) NULL,
    proxy_username VARCHAR(255) NULL,
    proxy_password VARCHAR(500) NULL,
    status proxynodestatus NOT NULL DEFAULT 'online',
    registered_by VARCHAR(36) NULL,
    last_heartbeat_at TIMESTAMPTZ NULL,
    heartbeat_interval INTEGER NOT NULL DEFAULT 30,
    active_connections INTEGER NOT NULL DEFAULT 0,
    total_requests BIGINT NOT NULL DEFAULT 0,
    avg_latency_ms DOUBLE PRECISION NULL,
    failed_requests BIGINT NOT NULL DEFAULT 0,
    dns_failures BIGINT NOT NULL DEFAULT 0,
    stream_errors BIGINT NOT NULL DEFAULT 0,
    proxy_metadata JSON NULL,
    hardware_info JSON NULL,
    estimated_max_concurrency INTEGER NULL,
    tunnel_mode BOOLEAN NOT NULL DEFAULT FALSE,
    tunnel_connected BOOLEAN NOT NULL DEFAULT FALSE,
    tunnel_connected_at TIMESTAMPTZ NULL,
    remote_config JSON NULL,
    config_version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
)
"#,
        )
        .await?;
    connection
        .execute(
            r#"
CREATE TABLE IF NOT EXISTS proxy_node_events (
    id BIGSERIAL PRIMARY KEY,
    node_id VARCHAR(36) NOT NULL,
    event_type TEXT NOT NULL,
    detail TEXT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
)
"#,
        )
        .await?;
    connection.close().await?;
    Ok(())
}

async fn wait_for_owner_attachment(forwarder_base_url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|err| format!("failed to build readiness client: {err}"))?;
    let target_url = format!("{forwarder_base_url}{TUNNEL_RELAY_PATH_PREFIX}/{NODE_ID}");
    let ready = wait_until(Duration::from_secs(10), Duration::from_millis(100), || {
        let client = client.clone();
        let target_url = target_url.clone();
        async move {
            let response = client
                .post(target_url)
                .header("content-type", "application/octet-stream")
                .body(relay_envelope())
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => match response.text().await {
                    Ok(body) => body == "owner-relay-ok",
                    Err(_) => false,
                },
                _ => false,
            }
        }
    })
    .await;
    if ready {
        Ok(())
    } else {
        Err("timed out waiting for owner attachment propagation".to_string())
    }
}

fn relay_headers() -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([(
        "content-type".to_string(),
        "application/octet-stream".to_string(),
    )])
}

fn relay_envelope() -> Vec<u8> {
    let meta = protocol::RequestMeta {
        method: "POST".to_string(),
        url: "https://owner-relay.example/v1/chat/completions".to_string(),
        headers: std::collections::HashMap::from([(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        timeout: 30,
        follow_redirects: None,
        http1_only: false,
    };
    let meta_json = serde_json::to_vec(&meta).expect("owner relay metadata should serialize");
    let body = br#"{"model":"gpt-5","messages":[{"role":"user","content":"owner relay"}]}"#;
    let mut envelope = Vec::with_capacity(4 + meta_json.len() + body.len());
    envelope.extend_from_slice(&(meta_json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_json);
    envelope.extend_from_slice(body);
    envelope
}

async fn connect_protocol_peer(
    gateway_base_url: &str,
    chunk_delay: Duration,
) -> Result<tokio::task::JoinHandle<()>, Box<dyn std::error::Error>> {
    let ws_url = format!(
        "{}{}",
        gateway_base_url.replace("http://", "ws://"),
        PROXY_TUNNEL_PATH
    );
    let request = ws_url.into_client_request()?;
    let mut request = request;
    request
        .headers_mut()
        .insert("x-node-id", http::HeaderValue::from_static(NODE_ID));
    request.headers_mut().insert(
        "x-node-name",
        http::HeaderValue::from_static("proxy-owner-relay-baseline"),
    );
    request.headers_mut().insert(
        "x-tunnel-max-streams",
        http::HeaderValue::from_static("256"),
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
                    if handle_binary_frame(&mut sink, data.to_vec(), chunk_delay)
                        .await
                        .is_err() =>
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
    chunk_delay: Duration,
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

            for chunk in [b"owner-".as_slice(), b"relay-".as_slice(), b"ok".as_slice()] {
                if !chunk_delay.is_zero() {
                    tokio::time::sleep(chunk_delay).await;
                }
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
) -> Result<MultiInstanceOwnerRelayBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = MultiInstanceOwnerRelayBaselineConfig::default();
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
            "--chunk-delay-ms" => {
                config.chunk_delay =
                    Duration::from_millis(next_value(&mut iter, "--chunk-delay-ms")?.parse()?)
            }
            "--redis-url" => config.redis_url = Some(next_value(&mut iter, "--redis-url")?),
            "--postgres-url" => {
                config.postgres_url = Some(next_value(&mut iter, "--postgres-url")?)
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

    if config.total_requests == 0 || config.concurrency == 0 || config.timeout.is_zero() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "owner relay baseline numeric settings must be positive",
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
    println!(
        "usage: cargo run -p aether-testkit --bin multi_instance_owner_relay_baseline -- [--requests 200] [--concurrency 20] [--timeout-ms 10000] [--chunk-delay-ms 0] [--redis-url redis://127.0.0.1:6379/0] [--postgres-url postgres://127.0.0.1:5432/postgres] [--output /tmp/multi_instance_owner_relay_baseline.json]"
    );
}
