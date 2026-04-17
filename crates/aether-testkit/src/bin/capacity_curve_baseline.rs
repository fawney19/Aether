use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use aether_gateway::tunnel_protocol as protocol;
use aether_testkit::{
    fetch_prometheus_samples, find_metric_value_u64, init_test_runtime_for, run_http_load_probe,
    ExecutionRuntimeHarness, ExecutionRuntimeHarnessConfig, GatewayHarness, GatewayHarnessConfig,
    HttpLoadProbeConfig, HttpLoadProbeResponseMode, HttpLoadProbeResult, SpawnedServer,
    TunnelHarness, TunnelHarnessConfig,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use serde::Serialize;
use serde_json::json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";

#[derive(Debug, Clone)]
struct CapacityCurveBaselineConfig {
    points: Vec<usize>,
    requests_per_point_multiplier: usize,
    sync_delay: Duration,
    stream_chunk_delay: Duration,
    tunnel_hold: Duration,
    timeout: Duration,
    saturation_latency_multiplier: u64,
    output_path: Option<PathBuf>,
}

impl Default for CapacityCurveBaselineConfig {
    fn default() -> Self {
        Self {
            points: vec![8, 16, 32, 64, 128, 256],
            requests_per_point_multiplier: 8,
            sync_delay: Duration::from_millis(75),
            stream_chunk_delay: Duration::from_millis(25),
            tunnel_hold: Duration::from_millis(75),
            timeout: Duration::from_secs(10),
            saturation_latency_multiplier: 4,
            output_path: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct CapacityCurveBaselineReport {
    suite: &'static str,
    gateway_sync: CapacityCurveScenarioReport,
    gateway_stream: CapacityCurveScenarioReport,
    execution_runtime_sync: CapacityCurveScenarioReport,
    execution_runtime_stream: CapacityCurveScenarioReport,
    gateway_tunnel_stream: CapacityCurveScenarioReport,
}

#[derive(Debug, Serialize)]
struct CapacityCurveScenarioReport {
    name: String,
    gate: String,
    latency_budget_ms: u64,
    points: Vec<CapacityCurvePointResult>,
    saturation_point: Option<CapacityCurveSaturationPoint>,
}

#[derive(Debug, Serialize)]
struct CapacityCurvePointResult {
    limit: usize,
    concurrency: usize,
    total_requests: usize,
    duration_ms: u64,
    successful_requests: usize,
    rejected_requests: usize,
    failed_requests: usize,
    throughput_rps: u64,
    p50_ms: u64,
    p95_ms: u64,
    max_ms: u64,
    mean_ms: u64,
    metrics: GateMetricSnapshot,
}

#[derive(Debug, Serialize)]
struct CapacityCurveSaturationPoint {
    limit: usize,
    concurrency: usize,
    reason: String,
    p95_ms: u64,
    rejected_requests: usize,
    failed_requests: usize,
    high_watermark: u64,
}

#[derive(Debug, Serialize)]
struct GateMetricSnapshot {
    in_flight: u64,
    available_permits: u64,
    high_watermark: u64,
    rejected_total: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("capacity-curve-baseline");
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
    config: &CapacityCurveBaselineConfig,
) -> Result<CapacityCurveBaselineReport, Box<dyn std::error::Error>> {
    let upstream = SpawnedServer::start(build_delayed_upstream(
        config.sync_delay,
        config.stream_chunk_delay,
    ))
    .await?;

    Ok(CapacityCurveBaselineReport {
        suite: "capacity_curve_baseline",
        gateway_sync: run_gateway_curve(
            "gateway_proxy_sync",
            "gateway_requests",
            false,
            upstream.base_url(),
            config,
        )
        .await?,
        gateway_stream: run_gateway_curve(
            "gateway_proxy_stream",
            "gateway_requests",
            true,
            upstream.base_url(),
            config,
        )
        .await?,
        execution_runtime_sync: run_execution_runtime_curve(
            "execution_runtime_sync",
            "execution_runtime_requests",
            false,
            upstream.base_url(),
            config,
        )
        .await?,
        execution_runtime_stream: run_execution_runtime_curve(
            "execution_runtime_stream",
            "execution_runtime_requests",
            true,
            upstream.base_url(),
            config,
        )
        .await?,
        gateway_tunnel_stream: run_tunnel_curve("gateway_tunnel_stream", "tunnel_requests", config)
            .await?,
    })
}

async fn run_gateway_curve(
    scenario_name: &str,
    gate_name: &str,
    stream: bool,
    upstream_base_url: &str,
    config: &CapacityCurveBaselineConfig,
) -> Result<CapacityCurveScenarioReport, Box<dyn std::error::Error>> {
    let latency_budget_ms = scenario_latency_budget_ms(
        if stream {
            config.stream_chunk_delay.saturating_mul(3u32)
        } else {
            config.sync_delay
        },
        config.saturation_latency_multiplier,
    );
    let mut points = Vec::new();
    for limit in &config.points {
        let gateway = GatewayHarness::start(GatewayHarnessConfig {
            upstream_base_url: upstream_base_url.to_string(),
            data_config: None,
            max_in_flight_requests: Some(*limit),
            distributed_request_gate: None,
            tunnel_instance_id: None,
            tunnel_relay_base_url: None,
        })
        .await?;
        let total_requests = total_requests_for_limit(*limit, config.requests_per_point_multiplier);
        let probe = chat_probe_config(
            format!("{}/v1/chat/completions", gateway.base_url()),
            stream,
            total_requests,
            *limit,
            config.timeout,
        );
        let started_at = Instant::now();
        let result = run_http_load_probe(&probe)
            .await
            .map_err(std::io::Error::other)?;
        let duration_ms = started_at.elapsed().as_millis() as u64;
        let metrics = capture_gate_metrics(
            &format!("{}/_gateway/metrics", gateway.base_url()),
            gate_name,
        )
        .await?;
        points.push(capacity_point(
            *limit,
            total_requests,
            duration_ms,
            result,
            metrics,
        ));
    }

    Ok(CapacityCurveScenarioReport {
        name: scenario_name.to_string(),
        gate: gate_name.to_string(),
        latency_budget_ms,
        saturation_point: detect_saturation_point(&points, latency_budget_ms),
        points,
    })
}

async fn run_execution_runtime_curve(
    scenario_name: &str,
    gate_name: &str,
    stream: bool,
    upstream_base_url: &str,
    config: &CapacityCurveBaselineConfig,
) -> Result<CapacityCurveScenarioReport, Box<dyn std::error::Error>> {
    let latency_budget_ms = scenario_latency_budget_ms(
        if stream {
            config.stream_chunk_delay.saturating_mul(3u32)
        } else {
            config.sync_delay
        },
        config.saturation_latency_multiplier,
    );
    let mut points = Vec::new();
    for limit in &config.points {
        let runtime = ExecutionRuntimeHarness::start(ExecutionRuntimeHarnessConfig {
            max_in_flight_requests: Some(*limit),
            distributed_request_gate: None,
        })
        .await?;
        let total_requests = total_requests_for_limit(*limit, config.requests_per_point_multiplier);
        let probe = execution_probe_config(
            format!(
                "{}/v1/execute/{}",
                runtime.base_url(),
                if stream { "stream" } else { "sync" }
            ),
            execution_plan(format!("{upstream_base_url}/v1/chat/completions"), stream),
            total_requests,
            *limit,
            config.timeout,
        );
        let started_at = Instant::now();
        let result = run_http_load_probe(&probe)
            .await
            .map_err(std::io::Error::other)?;
        let duration_ms = started_at.elapsed().as_millis() as u64;
        let metrics =
            capture_gate_metrics(&format!("{}/metrics", runtime.base_url()), gate_name).await?;
        points.push(capacity_point(
            *limit,
            total_requests,
            duration_ms,
            result,
            metrics,
        ));
    }

    Ok(CapacityCurveScenarioReport {
        name: scenario_name.to_string(),
        gate: gate_name.to_string(),
        latency_budget_ms,
        saturation_point: detect_saturation_point(&points, latency_budget_ms),
        points,
    })
}

async fn run_tunnel_curve(
    scenario_name: &str,
    gate_name: &str,
    config: &CapacityCurveBaselineConfig,
) -> Result<CapacityCurveScenarioReport, Box<dyn std::error::Error>> {
    let latency_budget_ms =
        scenario_latency_budget_ms(config.tunnel_hold, config.saturation_latency_multiplier);
    let mut points = Vec::new();
    for limit in &config.points {
        let relay_concurrency = (*limit).saturating_sub(1).max(1);
        let tunnel = TunnelHarness::start(TunnelHarnessConfig {
            max_streams: (*limit).max(128),
            ping_interval: Duration::from_secs(15),
            outbound_queue_capacity: 128,
            max_in_flight_requests: Some(*limit),
            distributed_request_gate: None,
        })
        .await?;
        let peer = connect_protocol_peer(tunnel.base_url(), config.tunnel_hold).await?;
        let total_requests =
            total_requests_for_limit(relay_concurrency, config.requests_per_point_multiplier);
        let probe = HttpLoadProbeConfig {
            url: format!(
                "{tunnel_base}{TUNNEL_RELAY_PATH_PREFIX}/node-baseline",
                tunnel_base = tunnel.base_url()
            ),
            method: Method::POST,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "application/octet-stream".to_string(),
            )]),
            body: Some(relay_envelope()),
            total_requests,
            concurrency: relay_concurrency,
            timeout: config.timeout,
            response_mode: HttpLoadProbeResponseMode::FullBody,
        };
        let started_at = Instant::now();
        let result = run_http_load_probe(&probe)
            .await
            .map_err(std::io::Error::other)?;
        let duration_ms = started_at.elapsed().as_millis() as u64;
        let metrics =
            capture_gate_metrics(&format!("{}/metrics", tunnel.base_url()), gate_name).await?;
        points.push(capacity_point(
            *limit,
            total_requests,
            duration_ms,
            result,
            metrics,
        ));
        drop(peer);
    }

    Ok(CapacityCurveScenarioReport {
        name: scenario_name.to_string(),
        gate: gate_name.to_string(),
        latency_budget_ms,
        saturation_point: detect_saturation_point(&points, latency_budget_ms),
        points,
    })
}

fn capacity_point(
    limit: usize,
    total_requests: usize,
    duration_ms: u64,
    result: HttpLoadProbeResult,
    metrics: GateMetricSnapshot,
) -> CapacityCurvePointResult {
    let rejected_requests = result.status_counts.get(&503).copied().unwrap_or_default();
    let successful_requests = result
        .status_counts
        .iter()
        .filter(|(status, _)| **status >= 200 && **status < 300)
        .map(|(_, count)| *count)
        .sum::<usize>();
    let throughput_rps = if duration_ms == 0 {
        successful_requests as u64
    } else {
        ((successful_requests as u64) * 1_000) / duration_ms.max(1)
    };

    CapacityCurvePointResult {
        limit,
        concurrency: result.concurrency,
        total_requests,
        duration_ms,
        successful_requests,
        rejected_requests,
        failed_requests: result.failed_requests,
        throughput_rps,
        p50_ms: result.p50_ms,
        p95_ms: result.p95_ms,
        max_ms: result.max_ms,
        mean_ms: result.mean_ms,
        metrics,
    }
}

fn detect_saturation_point(
    points: &[CapacityCurvePointResult],
    latency_budget_ms: u64,
) -> Option<CapacityCurveSaturationPoint> {
    points.iter().find_map(|point| {
        let reason = if point.failed_requests > 0 {
            Some("failures_observed")
        } else if point.rejected_requests > 0 {
            Some("admission_rejections_observed")
        } else if point.p95_ms > latency_budget_ms {
            Some("latency_budget_exceeded")
        } else {
            None
        }?;
        Some(CapacityCurveSaturationPoint {
            limit: point.limit,
            concurrency: point.concurrency,
            reason: reason.to_string(),
            p95_ms: point.p95_ms,
            rejected_requests: point.rejected_requests,
            failed_requests: point.failed_requests,
            high_watermark: point.metrics.high_watermark,
        })
    })
}

async fn capture_gate_metrics(
    metrics_url: &str,
    gate_name: &str,
) -> Result<GateMetricSnapshot, Box<dyn std::error::Error>> {
    let samples = fetch_prometheus_samples(metrics_url)
        .await
        .map_err(std::io::Error::other)?;
    Ok(GateMetricSnapshot {
        in_flight: find_metric_value_u64(&samples, "concurrency_in_flight", &[("gate", gate_name)])
            .unwrap_or_default(),
        available_permits: find_metric_value_u64(
            &samples,
            "concurrency_available_permits",
            &[("gate", gate_name)],
        )
        .unwrap_or_default(),
        high_watermark: find_metric_value_u64(
            &samples,
            "concurrency_high_watermark",
            &[("gate", gate_name)],
        )
        .unwrap_or_default(),
        rejected_total: find_metric_value_u64(
            &samples,
            "concurrency_rejected_total",
            &[("gate", gate_name)],
        )
        .unwrap_or_default(),
    })
}

fn scenario_latency_budget_ms(base: Duration, multiplier: u64) -> u64 {
    (base.as_millis() as u64).saturating_mul(multiplier.max(1))
}

fn total_requests_for_limit(limit: usize, multiplier: usize) -> usize {
    limit.saturating_mul(multiplier.max(1))
}

fn execution_probe_config(
    url: String,
    plan: ExecutionPlan,
    total_requests: usize,
    concurrency: usize,
    timeout: Duration,
) -> HttpLoadProbeConfig {
    HttpLoadProbeConfig {
        url,
        method: Method::POST,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(
            serde_json::to_vec(&plan).expect("execution plan should serialize for capacity curve"),
        ),
        total_requests,
        concurrency,
        timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
    }
}

fn chat_probe_config(
    url: String,
    stream: bool,
    total_requests: usize,
    concurrency: usize,
    timeout: Duration,
) -> HttpLoadProbeConfig {
    HttpLoadProbeConfig {
        url,
        method: Method::POST,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(
            serde_json::to_vec(&json!({
                "model": "gpt-5",
                "messages": [{"role": "user", "content": "hello"}],
                "stream": stream,
            }))
            .expect("chat body should serialize"),
        ),
        total_requests,
        concurrency,
        timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
    }
}

fn execution_plan(url: String, stream: bool) -> ExecutionPlan {
    ExecutionPlan {
        request_id: if stream {
            "capacity-curve-stream-request".to_string()
        } else {
            "capacity-curve-sync-request".to_string()
        },
        candidate_id: Some(if stream {
            "capacity-curve-stream-candidate".to_string()
        } else {
            "capacity-curve-sync-candidate".to_string()
        }),
        provider_name: Some("openai".to_string()),
        provider_id: "provider-capacity".to_string(),
        endpoint_id: "endpoint-capacity".to_string(),
        key_id: "key-capacity".to_string(),
        method: "POST".to_string(),
        url,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": stream,
        })),
        stream,
        client_api_format: "openai:chat".to_string(),
        provider_api_format: "openai:chat".to_string(),
        model_name: Some("gpt-5".to_string()),
        proxy: None,
        tls_profile: None,
        timeouts: Some(ExecutionTimeouts {
            connect_ms: Some(2_000),
            read_ms: Some(10_000),
            first_byte_ms: Some(5_000),
            total_ms: Some(10_000),
            ..ExecutionTimeouts::default()
        }),
    }
}

fn build_delayed_upstream(sync_delay: Duration, stream_chunk_delay: Duration) -> Router {
    Router::new().route(
        "/v1/chat/completions",
        any(move |request: Request| {
            let sync_delay = sync_delay;
            let stream_chunk_delay = stream_chunk_delay;
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX)
                    .await
                    .expect("capacity upstream body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).unwrap_or_else(|_| json!({}));
                let stream = payload
                    .get("stream")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if stream {
                    let body = async_stream::stream! {
                        tokio::time::sleep(stream_chunk_delay).await;
                        yield Ok::<_, Infallible>(Bytes::from_static(
                            b"data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
                        ));
                        tokio::time::sleep(stream_chunk_delay).await;
                        yield Ok::<_, Infallible>(Bytes::from_static(
                            b"data: {\"id\":\"chunk-2\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
                        ));
                        tokio::time::sleep(stream_chunk_delay).await;
                        yield Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(http::header::CONTENT_TYPE, "text/event-stream")
                        .body(Body::from_stream(body))
                        .expect("capacity upstream stream response should build")
                } else {
                    tokio::time::sleep(sync_delay).await;
                    Json(json!({
                        "id": "chatcmpl-capacity",
                        "object": "chat.completion",
                        "model": payload.get("model").and_then(|value| value.as_str()).unwrap_or("gpt-5"),
                        "choices": [{"message": {"role": "assistant", "content": "hello"}}]
                    }))
                    .into_response()
                }
            }
        }),
    )
}

fn relay_envelope() -> Vec<u8> {
    let meta = protocol::RequestMeta {
        method: "POST".to_string(),
        url: "https://capacity.example/v1/chat/completions".to_string(),
        headers: std::collections::HashMap::from([(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        timeout: 30,
        follow_redirects: None,
        http1_only: false,
    };
    let meta_json = serde_json::to_vec(&meta).expect("hub relay metadata should serialize");
    let body = br#"{"model":"gpt-5","messages":[{"role":"user","content":"hello"}]}"#;
    let mut envelope = Vec::with_capacity(4 + meta_json.len() + body.len());
    envelope.extend_from_slice(&(meta_json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_json);
    envelope.extend_from_slice(body);
    envelope
}

async fn connect_protocol_peer(
    tunnel_base_url: &str,
    hold: Duration,
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
        http::HeaderValue::from_static("512"),
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
                    if handle_binary_frame(&mut sink, data.to_vec(), hold)
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
    hold: Duration,
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
            tokio::time::sleep(hold).await;
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
                b"capacity-".as_slice(),
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
) -> Result<CapacityCurveBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = CapacityCurveBaselineConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--points" => {
                config.points = next_value(&mut iter, "--points")?
                    .split(',')
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| value.trim().parse::<usize>())
                    .collect::<Result<Vec<_>, _>>()?;
            }
            "--requests-per-point-multiplier" => {
                config.requests_per_point_multiplier =
                    next_value(&mut iter, "--requests-per-point-multiplier")?.parse()?
            }
            "--sync-delay-ms" => {
                config.sync_delay =
                    Duration::from_millis(next_value(&mut iter, "--sync-delay-ms")?.parse()?)
            }
            "--stream-chunk-delay-ms" => {
                config.stream_chunk_delay = Duration::from_millis(
                    next_value(&mut iter, "--stream-chunk-delay-ms")?.parse()?,
                )
            }
            "--tunnel-hold-ms" => {
                config.tunnel_hold =
                    Duration::from_millis(next_value(&mut iter, "--tunnel-hold-ms")?.parse()?)
            }
            "--timeout-ms" => {
                config.timeout =
                    Duration::from_millis(next_value(&mut iter, "--timeout-ms")?.parse()?)
            }
            "--saturation-latency-multiplier" => {
                config.saturation_latency_multiplier =
                    next_value(&mut iter, "--saturation-latency-multiplier")?.parse()?
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
    if config.points.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "capacity curve requires at least one point",
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
        "usage: cargo run -p aether-testkit --bin capacity_curve_baseline -- [--points 8,16,32,64,128,256] [--requests-per-point-multiplier 8] [--sync-delay-ms 75] [--stream-chunk-delay-ms 25] [--tunnel-hold-ms 75] [--timeout-ms 10000] [--saturation-latency-multiplier 4] [--output /tmp/capacity_curve_baseline.json]"
    );
}
