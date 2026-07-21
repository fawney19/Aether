// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_gateway::tunnel_protocol as protocol;
use aether_testkit::{
    fetch_prometheus_samples, find_metric_value_u64, init_test_runtime_for,
    BenchmarkRuntimeSampler, BenchmarkRuntimeSnapshot, TunnelHarness, TunnelHarnessConfig,
};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use serde::Serialize;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const PROXY_TUNNEL_PATH: &str = "/api/internal/proxy-tunnel";
const TUNNEL_RELAY_PATH_PREFIX: &str = "/api/internal/tunnel/relay";

type TunnelWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type TunnelSink = SplitSink<TunnelWs, Message>;

#[derive(Debug, Clone)]
struct LlmStreamStabilityConfig {
    total_streams: usize,
    concurrency: usize,
    chunks_per_stream: usize,
    stream_duration_secs: Option<u64>,
    chunk_bytes: usize,
    chunk_delay: Duration,
    jitter: Duration,
    first_byte_delay: Duration,
    idle_timeout: Duration,
    request_timeout: Option<Duration>,
    request_gate_limit: Option<usize>,
    tunnel_max_streams: usize,
    outbound_queue_capacity: usize,
    ping_interval: Duration,
    node_id: String,
    output_path: Option<PathBuf>,
}

impl Default for LlmStreamStabilityConfig {
    fn default() -> Self {
        Self {
            total_streams: 120,
            concurrency: 30,
            chunks_per_stream: 240,
            stream_duration_secs: None,
            chunk_bytes: 1024,
            chunk_delay: Duration::from_millis(250),
            jitter: Duration::ZERO,
            first_byte_delay: Duration::from_millis(100),
            idle_timeout: Duration::from_secs(10),
            request_timeout: None,
            request_gate_limit: None,
            tunnel_max_streams: 256,
            outbound_queue_capacity: 512,
            ping_interval: Duration::from_secs(5),
            node_id: "node-llm-stability".to_string(),
            output_path: None,
        }
    }
}

impl LlmStreamStabilityConfig {
    fn validate(&self) -> Result<(), String> {
        if self.total_streams == 0 {
            return Err("--streams must be positive".to_string());
        }
        if self.concurrency == 0 {
            return Err("--concurrency must be positive".to_string());
        }
        if self.chunk_bytes == 0 {
            return Err("--chunk-bytes must be positive".to_string());
        }
        if self.chunk_delay.is_zero() {
            return Err("--chunk-delay-ms must be positive".to_string());
        }
        if self.stream_duration_secs == Some(0) {
            return Err("--stream-seconds must be positive".to_string());
        }
        if self.chunks_per_stream == 0 && self.stream_duration_secs.is_none() {
            return Err("--chunks must be positive".to_string());
        }
        if self.idle_timeout.is_zero() {
            return Err("--idle-timeout-ms must be positive".to_string());
        }
        if self
            .request_timeout
            .is_some_and(|timeout| timeout.is_zero())
        {
            return Err("--request-timeout-ms must be positive".to_string());
        }
        if self.tunnel_max_streams == 0 {
            return Err("--tunnel-max-streams must be positive".to_string());
        }
        if self.ping_interval.is_zero() {
            return Err("--ping-interval-ms must be positive".to_string());
        }
        if self.request_gate_limit == Some(0) {
            return Err("--request-gate-limit must be positive".to_string());
        }
        if self.outbound_queue_capacity == 0 {
            return Err("--outbound-queue-capacity must be positive".to_string());
        }
        if self.node_id.trim().is_empty() {
            return Err("--node-id cannot be empty".to_string());
        }
        Ok(())
    }

    fn effective_chunks_per_stream(&self) -> usize {
        if let Some(seconds) = self.stream_duration_secs {
            let delay_ms = duration_ms_u64(self.chunk_delay).max(1);
            let stream_ms = seconds.saturating_mul(1_000);
            stream_ms.div_ceil(delay_ms).max(1) as usize
        } else {
            self.chunks_per_stream.max(1)
        }
    }

    fn effective_request_timeout(&self) -> Duration {
        self.request_timeout.unwrap_or_else(|| {
            let chunks = self.effective_chunks_per_stream() as u64;
            let chunk_window_ms =
                duration_ms_u64(self.chunk_delay).saturating_add(duration_ms_u64(self.jitter));
            let stream_ms = duration_ms_u64(self.first_byte_delay)
                .saturating_add(chunks.saturating_mul(chunk_window_ms))
                .saturating_add(30_000);
            Duration::from_millis(stream_ms.max(1))
        })
    }

    fn effective_request_gate_limit(&self) -> usize {
        self.request_gate_limit
            .unwrap_or_else(|| self.concurrency.saturating_add(1).max(1))
    }

    fn report(&self) -> EffectiveConfigReport {
        EffectiveConfigReport {
            total_streams: self.total_streams,
            concurrency: self.concurrency,
            chunks_per_stream: self.effective_chunks_per_stream(),
            chunk_bytes: self.chunk_bytes,
            chunk_delay_ms: duration_ms_u64(self.chunk_delay),
            jitter_ms: duration_ms_u64(self.jitter),
            first_byte_delay_ms: duration_ms_u64(self.first_byte_delay),
            idle_timeout_ms: duration_ms_u64(self.idle_timeout),
            request_timeout_ms: duration_ms_u64(self.effective_request_timeout()),
            request_gate_limit: self.effective_request_gate_limit(),
            tunnel_max_streams: self.tunnel_max_streams,
            outbound_queue_capacity: self.outbound_queue_capacity,
            ping_interval_ms: duration_ms_u64(self.ping_interval),
            node_id: self.node_id.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct EffectiveConfigReport {
    total_streams: usize,
    concurrency: usize,
    chunks_per_stream: usize,
    chunk_bytes: usize,
    chunk_delay_ms: u64,
    jitter_ms: u64,
    first_byte_delay_ms: u64,
    idle_timeout_ms: u64,
    request_timeout_ms: u64,
    request_gate_limit: usize,
    tunnel_max_streams: usize,
    outbound_queue_capacity: usize,
    ping_interval_ms: u64,
    node_id: String,
}

#[derive(Debug, Serialize)]
struct LlmStreamStabilityReport {
    suite: &'static str,
    config: EffectiveConfigReport,
    load: StreamLoadReport,
    peer: PeerStatsSnapshot,
    tunnel_metrics_before: TunnelMetricsSnapshot,
    tunnel_metrics_after: TunnelMetricsSnapshot,
}

#[derive(Debug, Serialize)]
struct StreamLoadReport {
    total_streams: usize,
    concurrency: usize,
    expected_chunks_per_stream: usize,
    expected_token_events: u64,
    duration_ms: u64,
    streams_per_sec: f64,
    body_bytes_per_sec: f64,
    body_mib_per_sec: f64,
    total_body_bytes: u64,
    successful_streams: usize,
    failed_streams: usize,
    truncated_streams: usize,
    missing_done_streams: usize,
    extra_chunk_streams: usize,
    total_token_events: u64,
    lost_token_events: u64,
    status_counts: BTreeMap<u16, usize>,
    error_counts: BTreeMap<String, usize>,
    first_body_byte_p50_ms: u64,
    first_body_byte_p95_ms: u64,
    first_body_byte_p99_ms: u64,
    stream_duration_p50_ms: u64,
    stream_duration_p95_ms: u64,
    stream_duration_p99_ms: u64,
    stream_duration_max_ms: u64,
    runtime: BenchmarkRuntimeSnapshot,
}

#[derive(Debug, Clone)]
struct StreamOutcome {
    status: Option<u16>,
    ok: bool,
    error_kind: Option<&'static str>,
    token_events: usize,
    done: bool,
    body_bytes: u64,
    first_body_byte_ms: Option<u64>,
    duration_ms: u64,
}

#[derive(Debug, Serialize)]
struct PeerStatsSnapshot {
    request_headers_received: usize,
    request_body_end_received: usize,
    responses_started: usize,
    responses_completed: usize,
    responses_cancelled: usize,
    response_send_errors: usize,
    stream_errors_received: usize,
    body_chunks_sent: usize,
    done_events_sent: usize,
    body_bytes_sent: u64,
    pings_received: usize,
    pongs_sent: usize,
    reader_errors: usize,
}

#[derive(Debug, Default)]
struct PeerStats {
    request_headers_received: AtomicUsize,
    request_body_end_received: AtomicUsize,
    responses_started: AtomicUsize,
    responses_completed: AtomicUsize,
    responses_cancelled: AtomicUsize,
    response_send_errors: AtomicUsize,
    stream_errors_received: AtomicUsize,
    body_chunks_sent: AtomicUsize,
    done_events_sent: AtomicUsize,
    body_bytes_sent: AtomicU64,
    pings_received: AtomicUsize,
    pongs_sent: AtomicUsize,
    reader_errors: AtomicUsize,
}

impl PeerStats {
    fn snapshot(&self) -> PeerStatsSnapshot {
        PeerStatsSnapshot {
            request_headers_received: self.request_headers_received.load(Ordering::Acquire),
            request_body_end_received: self.request_body_end_received.load(Ordering::Acquire),
            responses_started: self.responses_started.load(Ordering::Acquire),
            responses_completed: self.responses_completed.load(Ordering::Acquire),
            responses_cancelled: self.responses_cancelled.load(Ordering::Acquire),
            response_send_errors: self.response_send_errors.load(Ordering::Acquire),
            stream_errors_received: self.stream_errors_received.load(Ordering::Acquire),
            body_chunks_sent: self.body_chunks_sent.load(Ordering::Acquire),
            done_events_sent: self.done_events_sent.load(Ordering::Acquire),
            body_bytes_sent: self.body_bytes_sent.load(Ordering::Acquire),
            pings_received: self.pings_received.load(Ordering::Acquire),
            pongs_sent: self.pongs_sent.load(Ordering::Acquire),
            reader_errors: self.reader_errors.load(Ordering::Acquire),
        }
    }
}

#[derive(Debug, Serialize)]
struct TunnelMetricsSnapshot {
    proxy_connections: u64,
    proxy_connections_available: u64,
    active_streams: u64,
    request_gate_in_flight: u64,
    request_gate_available_permits: u64,
    request_gate_high_watermark: u64,
    request_gate_rejected_total: u64,
    outbound_queue_depth_total: u64,
    outbound_queue_depth_max: u64,
    outbound_queue_capacity_total: u64,
    outbound_queue_rejected_full_total: u64,
    outbound_queue_rejected_closed_total: u64,
    proxy_connection_congested_total: u64,
    proxy_connection_write_latency_last_us_max: u64,
    proxy_connection_write_latency_ewma_us_max: u64,
    proxy_connections_protocol_v1: u64,
    proxy_connections_protocol_v2: u64,
    proxy_soft_avoid_selection_total: u64,
    proxy_selection_retry_total: u64,
    proxy_selection_unavailable_total: u64,
}

struct ProtocolPeer {
    handle: tokio::task::JoinHandle<()>,
    sink: Arc<Mutex<TunnelSink>>,
    stats: Arc<PeerStats>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("llm-stream-stability-baseline");
    let config = parse_args(std::env::args().skip(1).collect())?;
    config.validate().map_err(std::io::Error::other)?;
    let output_path = config.output_path.clone();
    let report = run_suite(config).await?;
    let raw = serde_json::to_string_pretty(&report)?;
    println!("{raw}");
    if let Some(path) = output_path.as_ref() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("{raw}\n"))?;
    }
    Ok(())
}

async fn run_suite(
    config: LlmStreamStabilityConfig,
) -> Result<LlmStreamStabilityReport, Box<dyn std::error::Error>> {
    let chunks_per_stream = config.effective_chunks_per_stream();
    let config = Arc::new(config);
    let tunnel = TunnelHarness::start(TunnelHarnessConfig {
        max_streams: config.tunnel_max_streams,
        ping_interval: config.ping_interval,
        outbound_queue_capacity: config.outbound_queue_capacity,
        max_in_flight_requests: Some(config.effective_request_gate_limit()),
        distributed_request_gate: None,
    })
    .await?;

    let peer = connect_protocol_peer(tunnel.base_url(), config.clone(), chunks_per_stream).await?;
    let tunnel_metrics_before = capture_tunnel_metrics(tunnel.base_url()).await?;
    let load = run_stream_load(tunnel.base_url(), config.clone(), chunks_per_stream).await?;
    let tunnel_metrics_after = capture_tunnel_metrics(tunnel.base_url()).await?;
    let ProtocolPeer {
        mut handle,
        sink,
        stats,
    } = peer;
    let peer_stats = stats.snapshot();
    let _ = send_ws_message(&sink, Message::Close(None)).await;
    tokio::select! {
        _ = &mut handle => {}
        _ = tokio::time::sleep(Duration::from_millis(250)) => {
            handle.abort();
            let _ = handle.await;
        }
    }

    Ok(LlmStreamStabilityReport {
        suite: "llm_stream_stability_baseline",
        config: config.report(),
        load,
        peer: peer_stats,
        tunnel_metrics_before,
        tunnel_metrics_after,
    })
}

async fn run_stream_load(
    tunnel_base_url: &str,
    config: Arc<LlmStreamStabilityConfig>,
    chunks_per_stream: usize,
) -> Result<StreamLoadReport, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(config.effective_request_timeout())
        .build()?;
    let url = format!(
        "{tunnel_base}{TUNNEL_RELAY_PATH_PREFIX}/{node_id}",
        tunnel_base = tunnel_base_url,
        node_id = config.node_id
    );
    let started_at = Instant::now();
    let mut runtime_sampler = BenchmarkRuntimeSampler::new();
    let next_stream = Arc::new(AtomicUsize::new(0));
    let outcomes = Arc::new(Mutex::new(Vec::with_capacity(config.total_streams)));
    let mut workers = tokio::task::JoinSet::new();

    for _ in 0..config.concurrency {
        let client = client.clone();
        let url = url.clone();
        let config = config.clone();
        let next_stream = next_stream.clone();
        let outcomes = outcomes.clone();
        workers.spawn(async move {
            loop {
                let stream_index = next_stream.fetch_add(1, Ordering::AcqRel);
                if stream_index >= config.total_streams {
                    break;
                }
                let outcome =
                    run_one_stream(&client, &url, &config, stream_index, chunks_per_stream).await;
                outcomes.lock().await.push(outcome);
            }
        });
    }

    while let Some(result) = workers.join_next().await {
        result.map_err(|err| format!("stream load worker failed: {err}"))?;
    }

    let duration_ms = started_at.elapsed().as_millis() as u64;
    let runtime = runtime_sampler.snapshot();
    let outcomes = outcomes.lock().await.clone();
    Ok(summarize_outcomes(
        outcomes,
        config.total_streams,
        config.concurrency,
        chunks_per_stream,
        duration_ms,
        runtime,
    ))
}

async fn run_one_stream(
    client: &reqwest::Client,
    url: &str,
    config: &LlmStreamStabilityConfig,
    stream_index: usize,
    chunks_per_stream: usize,
) -> StreamOutcome {
    let started_at = Instant::now();
    let response = match client
        .request(Method::POST, url)
        .header("content-type", "application/octet-stream")
        .body(relay_envelope(config, stream_index))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return StreamOutcome {
                status: None,
                ok: false,
                error_kind: Some("request_error"),
                token_events: 0,
                done: false,
                body_bytes: 0,
                first_body_byte_ms: None,
                duration_ms: started_at.elapsed().as_millis() as u64,
            };
        }
    };

    let status = response.status().as_u16();
    if !response.status().is_success() {
        return StreamOutcome {
            status: Some(status),
            ok: false,
            error_kind: Some("non_2xx_status"),
            token_events: 0,
            done: false,
            body_bytes: 0,
            first_body_byte_ms: None,
            duration_ms: started_at.elapsed().as_millis() as u64,
        };
    }

    let mut body_stream = response.bytes_stream();
    let mut progress = SseProgress::default();
    let mut body_bytes = 0u64;
    let mut first_body_byte_ms = None;

    loop {
        let next_chunk = tokio::time::timeout(config.idle_timeout, body_stream.next()).await;
        let next_chunk = match next_chunk {
            Ok(next_chunk) => next_chunk,
            Err(_) => {
                return StreamOutcome {
                    status: Some(status),
                    ok: false,
                    error_kind: Some("idle_timeout"),
                    token_events: progress.token_events(),
                    done: progress.done,
                    body_bytes,
                    first_body_byte_ms,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                };
            }
        };
        match next_chunk {
            Some(Ok(chunk)) => {
                if first_body_byte_ms.is_none() {
                    first_body_byte_ms = Some(started_at.elapsed().as_millis() as u64);
                }
                body_bytes = body_bytes.saturating_add(chunk.len() as u64);
                progress.ingest(&chunk);
            }
            Some(Err(_)) => {
                return StreamOutcome {
                    status: Some(status),
                    ok: false,
                    error_kind: Some("body_error"),
                    token_events: progress.token_events(),
                    done: progress.done,
                    body_bytes,
                    first_body_byte_ms,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                };
            }
            None => break,
        }
    }

    let token_events = progress.token_events();
    let error_kind = if token_events < chunks_per_stream {
        Some("truncated")
    } else if token_events > chunks_per_stream {
        Some("extra_chunks")
    } else if !progress.done {
        Some("missing_done")
    } else {
        None
    };

    StreamOutcome {
        status: Some(status),
        ok: error_kind.is_none(),
        error_kind,
        token_events,
        done: progress.done,
        body_bytes,
        first_body_byte_ms,
        duration_ms: started_at.elapsed().as_millis() as u64,
    }
}

fn summarize_outcomes(
    outcomes: Vec<StreamOutcome>,
    total_streams: usize,
    concurrency: usize,
    chunks_per_stream: usize,
    duration_ms: u64,
    runtime: BenchmarkRuntimeSnapshot,
) -> StreamLoadReport {
    let mut status_counts = BTreeMap::new();
    let mut error_counts = BTreeMap::new();
    let mut first_body_byte_latencies = Vec::new();
    let mut stream_durations = Vec::with_capacity(outcomes.len());
    let mut successful_streams = 0usize;
    let mut total_body_bytes = 0u64;
    let mut total_token_events = 0u64;

    for outcome in &outcomes {
        if let Some(status) = outcome.status {
            *status_counts.entry(status).or_insert(0) += 1;
        }
        if outcome.ok {
            successful_streams += 1;
        } else {
            let kind = outcome.error_kind.unwrap_or("unknown");
            *error_counts.entry(kind.to_string()).or_insert(0) += 1;
        }
        if let Some(first_body_byte_ms) = outcome.first_body_byte_ms {
            first_body_byte_latencies.push(first_body_byte_ms);
        }
        stream_durations.push(outcome.duration_ms);
        total_body_bytes = total_body_bytes.saturating_add(outcome.body_bytes);
        total_token_events = total_token_events.saturating_add(outcome.token_events as u64);
    }

    first_body_byte_latencies.sort_unstable();
    stream_durations.sort_unstable();

    let expected_token_events = (total_streams as u64).saturating_mul(chunks_per_stream as u64);
    let lost_token_events = expected_token_events.saturating_sub(total_token_events);
    let duration_secs = (duration_ms.max(1) as f64) / 1_000.0;

    StreamLoadReport {
        total_streams,
        concurrency,
        expected_chunks_per_stream: chunks_per_stream,
        expected_token_events,
        duration_ms,
        streams_per_sec: (outcomes.len() as f64) / duration_secs,
        body_bytes_per_sec: (total_body_bytes as f64) / duration_secs,
        body_mib_per_sec: (total_body_bytes as f64) / duration_secs / 1_048_576.0,
        total_body_bytes,
        successful_streams,
        failed_streams: outcomes.len().saturating_sub(successful_streams),
        truncated_streams: count_error(&error_counts, "truncated"),
        missing_done_streams: outcomes.iter().filter(|outcome| !outcome.done).count(),
        extra_chunk_streams: count_error(&error_counts, "extra_chunks"),
        total_token_events,
        lost_token_events,
        status_counts,
        error_counts,
        first_body_byte_p50_ms: percentile(&first_body_byte_latencies, 50),
        first_body_byte_p95_ms: percentile(&first_body_byte_latencies, 95),
        first_body_byte_p99_ms: percentile(&first_body_byte_latencies, 99),
        stream_duration_p50_ms: percentile(&stream_durations, 50),
        stream_duration_p95_ms: percentile(&stream_durations, 95),
        stream_duration_p99_ms: percentile(&stream_durations, 99),
        stream_duration_max_ms: *stream_durations.last().unwrap_or(&0),
        runtime,
    }
}

fn count_error(error_counts: &BTreeMap<String, usize>, kind: &str) -> usize {
    error_counts.get(kind).copied().unwrap_or_default()
}

async fn connect_protocol_peer(
    tunnel_base_url: &str,
    config: Arc<LlmStreamStabilityConfig>,
    chunks_per_stream: usize,
) -> Result<ProtocolPeer, Box<dyn std::error::Error>> {
    let ws_url = format!(
        "{}{}",
        tunnel_base_url.replace("http://", "ws://"),
        PROXY_TUNNEL_PATH
    );
    let request = ws_url.into_client_request()?;
    let mut request = request;
    request.headers_mut().insert(
        "x-node-id",
        http::HeaderValue::from_str(config.node_id.as_str())?,
    );
    request.headers_mut().insert(
        aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
        http::HeaderValue::from_static(
            aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION_STR,
        ),
    );
    request.headers_mut().insert(
        "x-node-name",
        http::HeaderValue::from_static("llm-stream-stability"),
    );
    request.headers_mut().insert(
        "x-tunnel-max-streams",
        http::HeaderValue::from_str(&config.tunnel_max_streams.to_string())?,
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
            session_id: Some("llm-stream-stability-session".to_string()),
            replica_id: Some("llm-stream-stability-replica".to_string()),
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
    let sink = Arc::new(Mutex::new(sink));
    let stats = Arc::new(PeerStats::default());
    let cancelled = Arc::new(Mutex::new(HashSet::new()));
    let started_streams = Arc::new(Mutex::new(HashSet::new()));
    let stats_for_task = stats.clone();
    let config_for_task = config.clone();
    let sink_for_task = sink.clone();
    let handle = tokio::spawn(async move {
        while let Some(message) = stream.next().await {
            let message = match message {
                Ok(message) => message,
                Err(_) => {
                    stats_for_task.reader_errors.fetch_add(1, Ordering::AcqRel);
                    break;
                }
            };
            match message {
                Message::Binary(data) => {
                    handle_peer_binary_frame(
                        sink_for_task.clone(),
                        data.to_vec(),
                        config_for_task.clone(),
                        stats_for_task.clone(),
                        cancelled.clone(),
                        started_streams.clone(),
                        chunks_per_stream,
                    )
                    .await;
                }
                Message::Ping(payload) => {
                    stats_for_task.pings_received.fetch_add(1, Ordering::AcqRel);
                    if send_ws_message(&sink_for_task, Message::Pong(payload.clone()))
                        .await
                        .is_ok()
                    {
                        stats_for_task.pongs_sent.fetch_add(1, Ordering::AcqRel);
                    } else {
                        stats_for_task
                            .response_send_errors
                            .fetch_add(1, Ordering::AcqRel);
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        let _ = sink_for_task.lock().await.close().await;
    });

    Ok(ProtocolPeer {
        handle,
        sink,
        stats,
    })
}

async fn handle_peer_binary_frame(
    sink: Arc<Mutex<TunnelSink>>,
    data: Vec<u8>,
    config: Arc<LlmStreamStabilityConfig>,
    stats: Arc<PeerStats>,
    cancelled: Arc<Mutex<HashSet<u32>>>,
    started_streams: Arc<Mutex<HashSet<u32>>>,
    chunks_per_stream: usize,
) {
    let Some(header) = protocol::FrameHeader::parse(&data) else {
        return;
    };
    match header.msg_type {
        protocol::PING => {
            stats.pings_received.fetch_add(1, Ordering::AcqRel);
            let payload = protocol::frame_payload_by_header(&data, &header).unwrap_or(&[]);
            if send_ws_message(
                &sink,
                Message::Binary(protocol::encode_pong(payload).into()),
            )
            .await
            .is_ok()
            {
                stats.pongs_sent.fetch_add(1, Ordering::AcqRel);
            } else {
                stats.response_send_errors.fetch_add(1, Ordering::AcqRel);
            }
        }
        protocol::REQUEST_HEADERS => {
            stats
                .request_headers_received
                .fetch_add(1, Ordering::AcqRel);
            let payload = protocol::decode_payload(&data, &header).unwrap_or_default();
            let _ = serde_json::from_slice::<protocol::RequestMeta>(&payload);
        }
        protocol::REQUEST_BODY => {
            let payload = protocol::decode_payload(&data, &header).unwrap_or_default();
            if !payload.is_empty() {
                let _ = send_ws_message(
                    &sink,
                    Message::Binary(
                        protocol::encode_window_update(header.stream_id, payload.len() as u32)
                            .into(),
                    ),
                )
                .await;
            }
            if header.flags & protocol::FLAG_END_STREAM == 0 {
                return;
            }
            stats
                .request_body_end_received
                .fetch_add(1, Ordering::AcqRel);
            let mut started = started_streams.lock().await;
            if !started.insert(header.stream_id) {
                return;
            }
            drop(started);
            tokio::spawn(send_llm_stream_response(
                sink,
                header.stream_id,
                config,
                stats,
                cancelled,
                chunks_per_stream,
            ));
        }
        protocol::STREAM_ERROR => {
            stats.stream_errors_received.fetch_add(1, Ordering::AcqRel);
            cancelled.lock().await.insert(header.stream_id);
        }
        protocol::RESET_STREAM => {
            stats.stream_errors_received.fetch_add(1, Ordering::AcqRel);
            cancelled.lock().await.insert(header.stream_id);
        }
        _ => {}
    }
}

async fn send_llm_stream_response(
    sink: Arc<Mutex<TunnelSink>>,
    stream_id: u32,
    config: Arc<LlmStreamStabilityConfig>,
    stats: Arc<PeerStats>,
    cancelled: Arc<Mutex<HashSet<u32>>>,
    chunks_per_stream: usize,
) {
    stats.responses_started.fetch_add(1, Ordering::AcqRel);
    let response_meta = protocol::ResponseMeta {
        status: 200,
        headers: vec![
            ("content-type".to_string(), "text/event-stream".to_string()),
            ("cache-control".to_string(), "no-cache".to_string()),
        ],
    };
    let response_meta_json =
        serde_json::to_vec(&response_meta).expect("response metadata should serialize");
    if send_ws_message(
        &sink,
        Message::Binary(
            protocol::encode_frame(
                stream_id,
                protocol::RESPONSE_HEADERS,
                0,
                &response_meta_json,
            )
            .into(),
        ),
    )
    .await
    .is_err()
    {
        stats.response_send_errors.fetch_add(1, Ordering::AcqRel);
        return;
    }

    if !config.first_byte_delay.is_zero() {
        tokio::time::sleep(config.first_byte_delay).await;
    }

    for chunk_index in 0..chunks_per_stream {
        if is_cancelled(&cancelled, stream_id).await {
            stats.responses_cancelled.fetch_add(1, Ordering::AcqRel);
            return;
        }
        if chunk_index > 0 {
            tokio::time::sleep(chunk_delay_for(&config, stream_id, chunk_index)).await;
        }
        let chunk = build_sse_chunk(stream_id, chunk_index, config.chunk_bytes);
        let chunk_len = chunk.len() as u64;
        if send_ws_message(
            &sink,
            Message::Binary(
                protocol::encode_frame(stream_id, protocol::RESPONSE_BODY, 0, &chunk).into(),
            ),
        )
        .await
        .is_err()
        {
            stats.response_send_errors.fetch_add(1, Ordering::AcqRel);
            return;
        }
        stats.body_chunks_sent.fetch_add(1, Ordering::AcqRel);
        stats.body_bytes_sent.fetch_add(chunk_len, Ordering::AcqRel);
    }

    if is_cancelled(&cancelled, stream_id).await {
        stats.responses_cancelled.fetch_add(1, Ordering::AcqRel);
        return;
    }
    let done = b"data: [DONE]\n\n";
    if send_ws_message(
        &sink,
        Message::Binary(protocol::encode_frame(stream_id, protocol::RESPONSE_BODY, 0, done).into()),
    )
    .await
    .is_err()
    {
        stats.response_send_errors.fetch_add(1, Ordering::AcqRel);
        return;
    }
    stats.done_events_sent.fetch_add(1, Ordering::AcqRel);
    stats
        .body_bytes_sent
        .fetch_add(done.len() as u64, Ordering::AcqRel);

    if send_ws_message(
        &sink,
        Message::Binary(protocol::encode_frame(stream_id, protocol::STREAM_END, 0, &[]).into()),
    )
    .await
    .is_err()
    {
        stats.response_send_errors.fetch_add(1, Ordering::AcqRel);
        return;
    }
    stats.responses_completed.fetch_add(1, Ordering::AcqRel);
}

async fn send_ws_message(
    sink: &Arc<Mutex<TunnelSink>>,
    message: Message,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    sink.lock().await.send(message).await
}

async fn is_cancelled(cancelled: &Arc<Mutex<HashSet<u32>>>, stream_id: u32) -> bool {
    cancelled.lock().await.contains(&stream_id)
}

fn chunk_delay_for(
    config: &LlmStreamStabilityConfig,
    stream_id: u32,
    chunk_index: usize,
) -> Duration {
    if config.jitter.is_zero() {
        return config.chunk_delay;
    }
    let jitter_ms = duration_ms_u64(config.jitter);
    let seed = (stream_id as u64)
        .wrapping_mul(1_103_515_245)
        .wrapping_add((chunk_index as u64).wrapping_mul(12_345));
    config
        .chunk_delay
        .saturating_add(Duration::from_millis(seed % jitter_ms.saturating_add(1)))
}

fn relay_envelope(config: &LlmStreamStabilityConfig, stream_index: usize) -> Vec<u8> {
    let request_timeout = config.effective_request_timeout();
    let meta = protocol::RequestMeta {
        method: "POST".to_string(),
        url: "https://llm-stream-stability.example/v1/chat/completions".to_string(),
        headers: HashMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("accept".to_string(), "text/event-stream".to_string()),
        ]),
        stream: true,
        request_timeout_ms: None,
        stream_first_byte_timeout_ms: Some(duration_ms_u64(config.idle_timeout)),
        timeout: request_timeout.as_secs().max(1),
        follow_redirects: None,
        http1_only: false,
        provider_id: None,
        endpoint_id: None,
        key_id: None,
        transport_profile: None,
    };
    let meta_json = serde_json::to_vec(&meta).expect("tunnel relay metadata should serialize");
    let body = serde_json::to_vec(&json!({
        "model": "llm-stream-stability",
        "stream": true,
        "messages": [
            {
                "role": "user",
                "content": format!("stream stability probe {stream_index}")
            }
        ]
    }))
    .expect("request body should serialize");
    let mut envelope = Vec::with_capacity(4 + meta_json.len() + body.len());
    envelope.extend_from_slice(&(meta_json.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&meta_json);
    envelope.extend_from_slice(&body);
    envelope
}

fn build_sse_chunk(stream_id: u32, chunk_index: usize, min_bytes: usize) -> Vec<u8> {
    let prefix = format!(
        "event: token\ndata: {{\"stream_id\":{stream_id},\"chunk\":{chunk_index},\"delta\":\""
    );
    let suffix = "\"}\n\n";
    let mut chunk = Vec::with_capacity(min_bytes.max(prefix.len() + suffix.len()));
    chunk.extend_from_slice(prefix.as_bytes());
    let target_payload_len = min_bytes.saturating_sub(prefix.len() + suffix.len());
    chunk.extend(std::iter::repeat_n(b'x', target_payload_len));
    chunk.extend_from_slice(suffix.as_bytes());
    chunk
}

#[derive(Debug, Default)]
struct SseProgress {
    buffer: Vec<u8>,
    events: usize,
    done: bool,
}

impl SseProgress {
    fn ingest(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
        while let Some(index) = find_event_boundary(&self.buffer) {
            let event = &self.buffer[..index];
            if event
                .windows(b"[DONE]".len())
                .any(|window| window == b"[DONE]")
            {
                self.done = true;
            } else {
                self.events += 1;
            }
            self.buffer.drain(..index + 2);
        }
        const MAX_BUFFERED_EVENT_BYTES: usize = 1024 * 1024;
        if self.buffer.len() > MAX_BUFFERED_EVENT_BYTES {
            let drain_len = self.buffer.len() - MAX_BUFFERED_EVENT_BYTES;
            self.buffer.drain(..drain_len);
        }
    }

    fn token_events(&self) -> usize {
        self.events
    }
}

fn find_event_boundary(buffer: &[u8]) -> Option<usize> {
    buffer.windows(2).position(|window| window == b"\n\n")
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
        proxy_connections_available: find_metric_value_u64(
            &samples,
            "tunnel_proxy_connections_available",
            &[],
        )
        .unwrap_or_default(),
        active_streams: find_metric_value_u64(&samples, "tunnel_active_streams", &[])
            .unwrap_or_default(),
        request_gate_in_flight: find_metric_value_u64(
            &samples,
            "concurrency_in_flight",
            &[("gate", "tunnel_requests")],
        )
        .unwrap_or_default(),
        request_gate_available_permits: find_metric_value_u64(
            &samples,
            "concurrency_available_permits",
            &[("gate", "tunnel_requests")],
        )
        .unwrap_or_default(),
        request_gate_high_watermark: find_metric_value_u64(
            &samples,
            "concurrency_high_watermark",
            &[("gate", "tunnel_requests")],
        )
        .unwrap_or_default(),
        request_gate_rejected_total: find_metric_value_u64(
            &samples,
            "concurrency_rejected_total",
            &[("gate", "tunnel_requests")],
        )
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
        proxy_soft_avoid_selection_total: find_metric_value_u64(
            &samples,
            "tunnel_proxy_soft_avoid_selection_total",
            &[],
        )
        .unwrap_or_default(),
        proxy_selection_retry_total: find_metric_value_u64(
            &samples,
            "tunnel_proxy_selection_retry_total",
            &[],
        )
        .unwrap_or_default(),
        proxy_selection_unavailable_total: find_metric_value_u64(
            &samples,
            "tunnel_proxy_selection_unavailable_total",
            &[],
        )
        .unwrap_or_default(),
    })
}

fn percentile(values: &[u64], percentile: u8) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let last_index = values.len() - 1;
    let rank = ((last_index as f64) * (percentile as f64 / 100.0)).round() as usize;
    values[rank.min(last_index)]
}

fn duration_ms_u64(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn parse_args(args: Vec<String>) -> Result<LlmStreamStabilityConfig, Box<dyn std::error::Error>> {
    let mut config = LlmStreamStabilityConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--streams" => config.total_streams = next_value(&mut iter, "--streams")?.parse()?,
            "--concurrency" => {
                config.concurrency = next_value(&mut iter, "--concurrency")?.parse()?
            }
            "--chunks" => {
                config.chunks_per_stream = next_value(&mut iter, "--chunks")?.parse()?;
                config.stream_duration_secs = None;
            }
            "--stream-seconds" => {
                config.stream_duration_secs =
                    Some(next_value(&mut iter, "--stream-seconds")?.parse()?)
            }
            "--chunk-bytes" => {
                config.chunk_bytes = next_value(&mut iter, "--chunk-bytes")?.parse()?
            }
            "--chunk-delay-ms" => {
                config.chunk_delay =
                    Duration::from_millis(next_value(&mut iter, "--chunk-delay-ms")?.parse()?)
            }
            "--jitter-ms" => {
                config.jitter =
                    Duration::from_millis(next_value(&mut iter, "--jitter-ms")?.parse()?)
            }
            "--first-byte-delay-ms" => {
                config.first_byte_delay =
                    Duration::from_millis(next_value(&mut iter, "--first-byte-delay-ms")?.parse()?)
            }
            "--idle-timeout-ms" => {
                config.idle_timeout =
                    Duration::from_millis(next_value(&mut iter, "--idle-timeout-ms")?.parse()?)
            }
            "--request-timeout-ms" => {
                config.request_timeout = Some(Duration::from_millis(
                    next_value(&mut iter, "--request-timeout-ms")?.parse()?,
                ))
            }
            "--request-gate-limit" => {
                config.request_gate_limit =
                    Some(next_value(&mut iter, "--request-gate-limit")?.parse()?)
            }
            "--tunnel-max-streams" => {
                config.tunnel_max_streams =
                    next_value(&mut iter, "--tunnel-max-streams")?.parse()?
            }
            "--outbound-queue-capacity" => {
                config.outbound_queue_capacity =
                    next_value(&mut iter, "--outbound-queue-capacity")?.parse()?
            }
            "--ping-interval-ms" => {
                config.ping_interval =
                    Duration::from_millis(next_value(&mut iter, "--ping-interval-ms")?.parse()?)
            }
            "--node-id" => config.node_id = next_value(&mut iter, "--node-id")?,
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
        "usage: cargo run -p aether-integration-tests --bin llm_stream_stability_baseline -- [--streams 120] [--concurrency 30] [--chunks 240 | --stream-seconds 60] [--chunk-bytes 1024] [--chunk-delay-ms 250] [--jitter-ms 0] [--first-byte-delay-ms 100] [--idle-timeout-ms 10000] [--request-timeout-ms auto] [--request-gate-limit concurrency+1] [--tunnel-max-streams 256] [--outbound-queue-capacity 512] [--ping-interval-ms 5000] [--output /tmp/llm_stream_stability.json]"
    );
}

#[cfg(test)]
mod tests {
    use super::{
        build_sse_chunk, parse_args, summarize_outcomes, BenchmarkRuntimeSampler, SseProgress,
        StreamOutcome,
    };

    #[test]
    fn sse_progress_counts_split_events_and_done() {
        let mut progress = SseProgress::default();
        progress.ingest(b"data: one\n");
        progress.ingest(b"\ndata: two\n\ndata: [DONE]\n\n");
        assert_eq!(progress.token_events(), 2);
        assert!(progress.done);
    }

    #[test]
    fn sse_chunk_respects_minimum_size() {
        let chunk = build_sse_chunk(7, 42, 512);
        assert!(chunk.len() >= 512);
        assert!(chunk.ends_with(b"\"}\n\n"));
    }

    #[test]
    fn stream_seconds_overrides_chunks_in_effective_config() {
        let config = parse_args(vec![
            "--chunks".to_string(),
            "100".to_string(),
            "--stream-seconds".to_string(),
            "2".to_string(),
            "--chunk-delay-ms".to_string(),
            "250".to_string(),
        ])
        .expect("args should parse");
        assert_eq!(config.effective_chunks_per_stream(), 8);
    }

    #[test]
    fn summarize_outcomes_detects_loss() {
        let runtime = BenchmarkRuntimeSampler::new().snapshot();
        let report = summarize_outcomes(
            vec![
                StreamOutcome {
                    status: Some(200),
                    ok: true,
                    error_kind: None,
                    token_events: 3,
                    done: true,
                    body_bytes: 300,
                    first_body_byte_ms: Some(10),
                    duration_ms: 30,
                },
                StreamOutcome {
                    status: Some(200),
                    ok: false,
                    error_kind: Some("truncated"),
                    token_events: 1,
                    done: false,
                    body_bytes: 100,
                    first_body_byte_ms: Some(20),
                    duration_ms: 40,
                },
            ],
            2,
            2,
            3,
            50,
            runtime,
        );
        assert_eq!(report.successful_streams, 1);
        assert_eq!(report.failed_streams, 1);
        assert_eq!(report.truncated_streams, 1);
        assert_eq!(report.lost_token_events, 2);
    }
}
