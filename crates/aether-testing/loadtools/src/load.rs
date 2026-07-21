// HTTP load generation is intentionally independent from gateway internals.
use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};

use crate::runtime::{BenchmarkRuntimeSampler, BenchmarkRuntimeSnapshot};

const MAX_ERROR_SAMPLES: usize = 32;
const MAX_STATUS_SAMPLES: usize = 32;
const MAX_STATUS_SAMPLE_BODY_CHARS: usize = 512;
const FIRST_BODY_BACKGROUND_DRAIN_CHUNKS_ENV: &str =
    "AETHER_TESTKIT_FIRST_BODY_BACKGROUND_DRAIN_CHUNKS";
const FIRST_BODY_BACKGROUND_DRAIN_MS_ENV: &str = "AETHER_TESTKIT_FIRST_BODY_BACKGROUND_DRAIN_MS";
const FIRST_BODY_AUTO_CLIENT_SHARDS_MAX_ENV: &str =
    "AETHER_TESTKIT_FIRST_BODY_AUTO_CLIENT_SHARDS_MAX";
const DEFAULT_FIRST_BODY_BACKGROUND_DRAIN_CHUNKS: usize = 64;
const DEFAULT_FIRST_BODY_BACKGROUND_DRAIN_MS: u64 = 100;
const DEFAULT_FIRST_BODY_AUTO_CLIENT_SHARDS_MAX: usize = 512;
const MAX_SSE_CONTROL_LINE_BYTES: usize = 4 * 1024;
const SSE_COMPLETION_EVENT_NAMES: &[&str] = &[
    "response.completed",
    "response.done",
    "response.incomplete",
    "message.completed",
    "message_stop",
];
const SSE_ERROR_EVENT_NAMES: &[&str] = &["error", "response.failed"];

#[derive(Debug, Clone, Copy, Default, serde::Serialize, PartialEq, Eq)]
pub enum HttpLoadProbeResponseMode {
    #[default]
    HeadersOnly,
    FirstBodyByte,
    FullBody,
}

/// Optional checks applied while consuming a probe response body.
///
/// The default load-probe API keeps its historical behavior. Callers that
/// exercise an SSE endpoint can opt into protocol-level completion checking
/// without changing the request configuration shared by existing probes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HttpLoadProbeOptions {
    pub require_sse_done: bool,
}

#[derive(Debug, Clone)]
pub struct HttpLoadProbeConfig {
    pub url: String,
    pub warmup_url: Option<String>,
    pub method: Method,
    pub headers: BTreeMap<String, String>,
    pub header_sets: Vec<BTreeMap<String, String>>,
    pub body: Option<Vec<u8>>,
    pub total_requests: usize,
    pub concurrency: usize,
    /// Number of successful warmup requests. For HTTP/2, one request per
    /// client shard is enough to establish that shard's multiplexed socket;
    /// this value is intentionally not a promise about TCP connection count.
    pub warmup_connections: usize,
    pub timeout: Duration,
    pub connect_timeout: Option<Duration>,
    pub response_mode: HttpLoadProbeResponseMode,
    pub client_shards: usize,
    pub pool_max_idle_per_host: Option<usize>,
    pub start_ramp: Duration,
    pub http1_only: bool,
    pub http2_prior_knowledge: bool,
    pub first_body_hold: Duration,
}

impl Default for HttpLoadProbeConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            warmup_url: None,
            method: Method::GET,
            headers: BTreeMap::new(),
            header_sets: Vec::new(),
            body: None,
            total_requests: 100,
            concurrency: 10,
            warmup_connections: 0,
            timeout: Duration::from_secs(30),
            connect_timeout: None,
            response_mode: HttpLoadProbeResponseMode::HeadersOnly,
            client_shards: 1,
            pool_max_idle_per_host: None,
            start_ramp: Duration::ZERO,
            http1_only: false,
            http2_prior_knowledge: false,
            first_body_hold: Duration::ZERO,
        }
    }
}

impl HttpLoadProbeConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("load probe url cannot be empty".to_string());
        }
        if self.total_requests == 0 {
            return Err("load probe total_requests must be positive".to_string());
        }
        if self.concurrency == 0 {
            return Err("load probe concurrency must be positive".to_string());
        }
        if self.timeout.is_zero() {
            return Err("load probe timeout must be positive".to_string());
        }
        if matches!(self.connect_timeout, Some(timeout) if timeout.is_zero()) {
            return Err("load probe connect_timeout must be positive when set".to_string());
        }
        if self.client_shards == 0 {
            return Err("load probe client_shards must be positive".to_string());
        }
        for (index, headers) in self.header_sets.iter().enumerate() {
            if headers.is_empty() {
                return Err(format!("load probe header_sets[{index}] cannot be empty"));
            }
        }
        if self.http1_only && self.http2_prior_knowledge {
            return Err(
                "load probe cannot enable both http1_only and http2_prior_knowledge".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct HttpLoadProbeErrorSample {
    pub request_index: usize,
    pub url: String,
    pub phase: String,
    pub kind: String,
    pub elapsed_ms: u64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct HttpLoadProbeStatusSample {
    pub request_index: usize,
    pub url: String,
    pub status: u16,
    pub elapsed_ms: u64,
    pub body: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct HttpLoadProbeResult {
    pub url: String,
    pub method: String,
    pub response_mode: HttpLoadProbeResponseMode,
    pub require_sse_done: bool,
    pub total_requests: usize,
    pub concurrency: usize,
    /// Highest number of requests concurrently owned by the load probe.
    ///
    /// This is a client-side measurement. It proves how many request tasks
    /// remained in flight (including response-body draining), but it does not
    /// by itself prove that the gateway admitted the same number.
    pub max_in_flight_requests: usize,
    /// Number of successful warmup requests. For HTTP/2, one request per
    /// client shard is enough to establish that shard's multiplexed socket;
    /// this value is intentionally not a promise about TCP connection count.
    pub warmup_connections: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warmup_url: Option<String>,
    pub client_shards: usize,
    pub start_ramp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_max_idle_per_host: Option<usize>,
    pub http1_only: bool,
    pub http2_prior_knowledge: bool,
    pub timeout_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    pub first_body_hold_ms: u64,
    pub duration_ms: u64,
    pub throughput_rps: u64,
    pub p99_ms: u64,
    pub completed_requests: usize,
    pub failed_requests: usize,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
    pub mean_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_p50_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_p95_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_p99_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_body_p50_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_body_p95_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_body_p99_ms: Option<u64>,
    pub runtime: BenchmarkRuntimeSnapshot,
    pub status_counts: BTreeMap<u16, usize>,
    pub error_counts: BTreeMap<String, usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub error_samples: Vec<HttpLoadProbeErrorSample>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_success_status_samples: Vec<HttpLoadProbeStatusSample>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct MultiUrlHttpLoadProbeResult {
    pub target_urls: Vec<String>,
    pub target_request_counts: BTreeMap<String, usize>,
    pub method: String,
    pub response_mode: HttpLoadProbeResponseMode,
    pub require_sse_done: bool,
    pub total_requests: usize,
    pub concurrency: usize,
    /// Highest number of requests concurrently owned by the load probe.
    ///
    /// This is a client-side measurement. It proves how many request tasks
    /// remained in flight (including response-body draining), but it does not
    /// by itself prove that the gateway admitted the same number.
    pub max_in_flight_requests: usize,
    pub warmup_connections: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warmup_url: Option<String>,
    pub client_shards: usize,
    pub start_ramp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_max_idle_per_host: Option<usize>,
    pub http1_only: bool,
    pub http2_prior_knowledge: bool,
    pub timeout_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    pub first_body_hold_ms: u64,
    pub duration_ms: u64,
    pub throughput_rps: u64,
    pub p99_ms: u64,
    pub completed_requests: usize,
    pub failed_requests: usize,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
    pub mean_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_p50_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_p95_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_p99_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_body_p50_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_body_p95_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_body_p99_ms: Option<u64>,
    pub runtime: BenchmarkRuntimeSnapshot,
    pub status_counts: BTreeMap<u16, usize>,
    pub error_counts: BTreeMap<String, usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub error_samples: Vec<HttpLoadProbeErrorSample>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_success_status_samples: Vec<HttpLoadProbeStatusSample>,
}

#[derive(Debug, Default)]
struct ProbeWorkerStats {
    latencies_ms: Vec<u64>,
    header_latencies_ms: Vec<u64>,
    first_body_latencies_ms: Vec<u64>,
    status_counts: BTreeMap<u16, usize>,
    error_counts: BTreeMap<String, usize>,
    error_samples: Vec<HttpLoadProbeErrorSample>,
    non_success_status_samples: Vec<HttpLoadProbeStatusSample>,
    target_request_counts: BTreeMap<String, usize>,
    failed_requests: usize,
    completed_requests: usize,
}

pub async fn run_http_load_probe(
    config: &HttpLoadProbeConfig,
) -> Result<HttpLoadProbeResult, String> {
    run_http_load_probe_with_options(config, HttpLoadProbeOptions::default()).await
}

pub async fn run_http_load_probe_with_options(
    config: &HttpLoadProbeConfig,
    options: HttpLoadProbeOptions,
) -> Result<HttpLoadProbeResult, String> {
    config.validate()?;
    validate_probe_options(config, options)?;
    run_http_load_probe_against_urls(config, std::slice::from_ref(&config.url), options)
        .await
        .map(|result| HttpLoadProbeResult {
            url: result
                .target_urls
                .into_iter()
                .next()
                .unwrap_or_else(|| config.url.clone()),
            method: result.method,
            response_mode: result.response_mode,
            require_sse_done: result.require_sse_done,
            total_requests: result.total_requests,
            concurrency: result.concurrency,
            max_in_flight_requests: result.max_in_flight_requests,
            warmup_connections: result.warmup_connections,
            warmup_url: result.warmup_url,
            client_shards: result.client_shards,
            start_ramp_ms: result.start_ramp_ms,
            pool_max_idle_per_host: result.pool_max_idle_per_host,
            http1_only: result.http1_only,
            http2_prior_knowledge: result.http2_prior_knowledge,
            timeout_ms: result.timeout_ms,
            connect_timeout_ms: result.connect_timeout_ms,
            first_body_hold_ms: result.first_body_hold_ms,
            duration_ms: result.duration_ms,
            throughput_rps: result.throughput_rps,
            p99_ms: result.p99_ms,
            completed_requests: result.completed_requests,
            failed_requests: result.failed_requests,
            p50_ms: result.p50_ms,
            p95_ms: result.p95_ms,
            max_ms: result.max_ms,
            mean_ms: result.mean_ms,
            headers_p50_ms: result.headers_p50_ms,
            headers_p95_ms: result.headers_p95_ms,
            headers_p99_ms: result.headers_p99_ms,
            first_body_p50_ms: result.first_body_p50_ms,
            first_body_p95_ms: result.first_body_p95_ms,
            first_body_p99_ms: result.first_body_p99_ms,
            runtime: result.runtime,
            status_counts: result.status_counts,
            error_counts: result.error_counts,
            error_samples: result.error_samples,
            non_success_status_samples: result.non_success_status_samples,
        })
}

pub async fn run_multi_url_http_load_probe(
    config: &HttpLoadProbeConfig,
    urls: &[String],
) -> Result<MultiUrlHttpLoadProbeResult, String> {
    run_multi_url_http_load_probe_with_options(config, urls, HttpLoadProbeOptions::default()).await
}

pub async fn run_multi_url_http_load_probe_with_options(
    config: &HttpLoadProbeConfig,
    urls: &[String],
    options: HttpLoadProbeOptions,
) -> Result<MultiUrlHttpLoadProbeResult, String> {
    config.validate()?;
    validate_probe_options(config, options)?;
    if urls.is_empty() {
        return Err("multi-url load probe requires at least one target url".to_string());
    }
    run_http_load_probe_against_urls(config, urls, options).await
}

fn validate_probe_options(
    config: &HttpLoadProbeConfig,
    options: HttpLoadProbeOptions,
) -> Result<(), String> {
    if options.require_sse_done && config.response_mode != HttpLoadProbeResponseMode::FullBody {
        return Err(
            "load probe --require-sse-done requires --response-mode full so the body can be consumed"
                .to_string(),
        );
    }
    Ok(())
}

async fn run_http_load_probe_against_urls(
    config: &HttpLoadProbeConfig,
    urls: &[String],
    options: HttpLoadProbeOptions,
) -> Result<MultiUrlHttpLoadProbeResult, String> {
    let effective_client_shards = effective_probe_client_shards(config);
    let clients = Arc::new(build_probe_clients(config, effective_client_shards)?);
    let target_urls = Arc::new(urls.to_vec());
    let total_requests = config.total_requests;
    let request_headers = build_request_header_sets(config)?;
    let request_body = config.body.clone().map(Bytes::from);
    let response_mode = config.response_mode;
    let require_sse_done = options.require_sse_done;
    let first_body_hold = config.first_body_hold;
    let start_ramp = config.start_ramp;
    warmup_probe_connections(config, Arc::clone(&clients)).await?;
    let mut runtime_sampler = BenchmarkRuntimeSampler::new();
    let started_at = Instant::now();

    let next_request = Arc::new(AtomicUsize::new(0));
    let in_flight_requests = Arc::new(AtomicUsize::new(0));
    let max_in_flight_requests = Arc::new(AtomicUsize::new(0));

    let mut workers = tokio::task::JoinSet::new();
    for worker_index in 0..config.concurrency {
        let client = clients[worker_index % clients.len()].clone();
        let next_request = Arc::clone(&next_request);
        let in_flight_requests = Arc::clone(&in_flight_requests);
        let max_in_flight_requests = Arc::clone(&max_in_flight_requests);
        let method = config.method.clone();
        let urls = Arc::clone(&target_urls);
        let request_headers = Arc::clone(&request_headers);
        let request_body = request_body.clone();
        let start_delay = worker_start_delay(start_ramp, worker_index, config.concurrency);

        workers.spawn(async move {
            let mut stats = ProbeWorkerStats::default();
            if !start_delay.is_zero() {
                tokio::time::sleep(start_delay).await;
            }
            loop {
                let current = next_request.fetch_add(1, Ordering::AcqRel);
                if current >= total_requests {
                    break;
                }

                let current_in_flight = in_flight_requests.fetch_add(1, Ordering::AcqRel) + 1;
                max_in_flight_requests.fetch_max(current_in_flight, Ordering::AcqRel);
                let started_at = Instant::now();
                let url = urls[current % urls.len()].clone();
                let mut request = client.request(method.clone(), &url);
                let headers = &request_headers[current % request_headers.len()];
                for (name, value) in headers.iter() {
                    request = request.header(name, value);
                }
                if let Some(body) = request_body.as_ref() {
                    request = request.body(body.clone());
                }
                match request.send().await {
                    Ok(response) => {
                        let headers_latency_ms = started_at.elapsed().as_millis() as u64;
                        let status = response.status().as_u16();
                        // Count the HTTP response before consuming its body. A stream can
                        // terminate at the protocol layer while still having a valid 2xx status.
                        *stats.status_counts.entry(status).or_insert(0) += 1;
                        let body_result = observe_response_body(
                            response,
                            response_mode,
                            started_at,
                            first_body_hold,
                            require_sse_done && (200..300).contains(&status),
                        )
                        .await;
                        match body_result {
                            Err(error) => {
                                stats.failed_requests += 1;
                                record_load_error(
                                    &mut stats.error_counts,
                                    &mut stats.error_samples,
                                    current,
                                    &url,
                                    started_at.elapsed().as_millis() as u64,
                                    error,
                                );
                            }
                            Ok(observation) => {
                                if !(200..300).contains(&status) {
                                    record_non_success_status_sample(
                                        &mut stats.non_success_status_samples,
                                        current,
                                        &url,
                                        status,
                                        started_at.elapsed().as_millis() as u64,
                                        observation.body_sample.as_deref().unwrap_or_default(),
                                    );
                                }
                                *stats.target_request_counts.entry(url).or_insert(0) += 1;
                                if let Some(first_body_latency_ms) =
                                    observation.first_body_latency_ms
                                {
                                    stats.first_body_latencies_ms.push(first_body_latency_ms);
                                }
                            }
                        }
                        let latency_ms = started_at.elapsed().as_millis() as u64;
                        stats.latencies_ms.push(latency_ms);
                        stats.header_latencies_ms.push(headers_latency_ms);
                        stats.completed_requests += 1;
                    }
                    Err(err) => {
                        let latency_ms = started_at.elapsed().as_millis() as u64;
                        stats.latencies_ms.push(latency_ms);
                        stats.failed_requests += 1;
                        record_load_error(
                            &mut stats.error_counts,
                            &mut stats.error_samples,
                            current,
                            &url,
                            latency_ms,
                            classify_reqwest_error("send", &err),
                        );
                        stats.completed_requests += 1;
                    }
                }
                in_flight_requests.fetch_sub(1, Ordering::AcqRel);
            }
            stats
        });
    }

    let mut aggregate = ProbeWorkerStats::default();
    while let Some(result) = workers.join_next().await {
        let worker = result.map_err(|err| format!("load probe worker task failed: {err}"))?;
        merge_probe_worker_stats(&mut aggregate, worker);
    }

    let ProbeWorkerStats {
        mut latencies_ms,
        header_latencies_ms: mut header_latencies,
        first_body_latencies_ms: mut first_body_latencies,
        status_counts,
        error_counts,
        error_samples,
        non_success_status_samples,
        target_request_counts,
        failed_requests,
        completed_requests,
    } = aggregate;
    let mut latencies = std::mem::take(&mut latencies_ms);
    latencies.sort_unstable();
    header_latencies.sort_unstable();
    first_body_latencies.sort_unstable();
    let (p50_ms, p95_ms, p99_ms, max_ms, mean_ms) = summarize_latencies(&latencies);
    let (headers_p50_ms, headers_p95_ms, headers_p99_ms, _, _) =
        summarize_latencies(&header_latencies);
    let (first_body_p50_ms, first_body_p95_ms, first_body_p99_ms, _, _) =
        summarize_latencies(&first_body_latencies);
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let throughput_rps = if duration_ms == 0 {
        completed_requests as u64
    } else {
        ((completed_requests as u64) * 1_000) / duration_ms.max(1)
    };

    Ok(MultiUrlHttpLoadProbeResult {
        target_urls: urls.to_vec(),
        target_request_counts,
        method: config.method.as_str().to_string(),
        response_mode: config.response_mode,
        require_sse_done,
        total_requests: config.total_requests,
        concurrency: config.concurrency,
        max_in_flight_requests: max_in_flight_requests.load(Ordering::Acquire),
        warmup_connections: config.warmup_connections,
        warmup_url: config.warmup_url.clone(),
        client_shards: effective_client_shards,
        start_ramp_ms: config.start_ramp.as_millis() as u64,
        pool_max_idle_per_host: config.pool_max_idle_per_host,
        http1_only: config.http1_only,
        http2_prior_knowledge: config.http2_prior_knowledge,
        timeout_ms: config.timeout.as_millis() as u64,
        connect_timeout_ms: config
            .connect_timeout
            .map(|timeout| timeout.as_millis() as u64),
        first_body_hold_ms: config.first_body_hold.as_millis() as u64,
        duration_ms,
        throughput_rps,
        p99_ms,
        completed_requests,
        failed_requests,
        p50_ms,
        p95_ms,
        max_ms,
        mean_ms,
        headers_p50_ms: (!header_latencies.is_empty()).then_some(headers_p50_ms),
        headers_p95_ms: (!header_latencies.is_empty()).then_some(headers_p95_ms),
        headers_p99_ms: (!header_latencies.is_empty()).then_some(headers_p99_ms),
        first_body_p50_ms: (!first_body_latencies.is_empty()).then_some(first_body_p50_ms),
        first_body_p95_ms: (!first_body_latencies.is_empty()).then_some(first_body_p95_ms),
        first_body_p99_ms: (!first_body_latencies.is_empty()).then_some(first_body_p99_ms),
        runtime: runtime_sampler.snapshot(),
        status_counts,
        error_counts,
        error_samples,
        non_success_status_samples,
    })
}

fn effective_probe_client_shards(config: &HttpLoadProbeConfig) -> usize {
    let configured = config.client_shards.max(1);
    if configured != 1
        || config.http1_only
        || !config.http2_prior_knowledge
        || config.response_mode != HttpLoadProbeResponseMode::FirstBodyByte
    {
        return configured;
    }
    let max_auto = env_usize(
        FIRST_BODY_AUTO_CLIENT_SHARDS_MAX_ENV,
        DEFAULT_FIRST_BODY_AUTO_CLIENT_SHARDS_MAX,
    )
    .max(1);
    config.concurrency.max(1).min(max_auto)
}

fn build_probe_clients(
    config: &HttpLoadProbeConfig,
    client_shards: usize,
) -> Result<Vec<Client>, String> {
    let mut clients = Vec::with_capacity(client_shards);
    for _ in 0..client_shards {
        let mut builder = Client::builder().timeout(config.timeout);
        if let Some(connect_timeout) = config.connect_timeout {
            builder = builder.connect_timeout(connect_timeout);
        }
        if let Some(pool_max_idle_per_host) = config.pool_max_idle_per_host {
            builder = builder.pool_max_idle_per_host(pool_max_idle_per_host);
        }
        if config.http1_only {
            builder = builder.http1_only();
        }
        if config.http2_prior_knowledge {
            builder = builder.http2_prior_knowledge();
        }
        clients.push(
            builder
                .build()
                .map_err(|err| format!("failed to build load probe http client: {err}"))?,
        );
    }
    Ok(clients)
}

fn worker_start_delay(start_ramp: Duration, worker_index: usize, concurrency: usize) -> Duration {
    if start_ramp.is_zero() || concurrency <= 1 || worker_index == 0 {
        return Duration::ZERO;
    }
    let ramp_nanos = start_ramp.as_nanos();
    let offset_nanos = ramp_nanos
        .saturating_mul(worker_index as u128)
        .checked_div((concurrency - 1) as u128)
        .unwrap_or_default();
    Duration::from_nanos(offset_nanos.min(u64::MAX as u128) as u64)
}

async fn warmup_probe_connections(
    config: &HttpLoadProbeConfig,
    clients: Arc<Vec<Client>>,
) -> Result<(), String> {
    if config.warmup_connections == 0 {
        return Ok(());
    }
    let warmup_url = config.warmup_url.as_deref().unwrap_or(config.url.as_str());
    let mut workers = tokio::task::JoinSet::new();
    let used_client_count = clients.len().min(config.concurrency);
    for (client_index, requests) in
        warmup_request_distribution(used_client_count, config.warmup_connections)
            .into_iter()
            .enumerate()
    {
        let client = clients[client_index].clone();
        let warmup_url = warmup_url.to_string();
        workers.spawn(async move {
            for _ in 0..requests {
                let response = client
                    .get(&warmup_url)
                    .send()
                    .await
                    .map_err(|err| format!("warmup request failed: {err}"))?;
                let response = response
                    .error_for_status()
                    .map_err(|err| format!("warmup request returned error status: {err}"))?;
                response
                    .bytes()
                    .await
                    .map_err(|err| format!("warmup response body failed: {err}"))?;
            }
            Ok::<(), String>(())
        });
    }
    while let Some(result) = workers.join_next().await {
        result
            .map_err(|err| format!("warmup worker task failed: {err}"))?
            .map_err(|err| format!("failed to warm load probe connections: {err}"))?;
    }
    Ok(())
}

fn warmup_request_distribution(client_count: usize, total_requests: usize) -> Vec<usize> {
    if client_count == 0 || total_requests == 0 {
        return Vec::new();
    }
    let shard_count = client_count.min(total_requests);
    let requests_per_shard = total_requests / shard_count;
    let shards_with_extra_request = total_requests % shard_count;
    (0..shard_count)
        .map(|index| requests_per_shard + usize::from(index < shards_with_extra_request))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassifiedLoadError {
    key: String,
    phase: String,
    kind: String,
    message: String,
    source: Option<String>,
}

impl ClassifiedLoadError {
    fn static_body(kind: &str, message: &str) -> Self {
        Self {
            key: format!("body:{kind}"),
            phase: "body".to_string(),
            kind: kind.to_string(),
            message: message.to_string(),
            source: None,
        }
    }
}

fn merge_probe_worker_stats(target: &mut ProbeWorkerStats, mut source: ProbeWorkerStats) {
    target.latencies_ms.append(&mut source.latencies_ms);
    target
        .header_latencies_ms
        .append(&mut source.header_latencies_ms);
    target
        .first_body_latencies_ms
        .append(&mut source.first_body_latencies_ms);
    merge_counts(&mut target.status_counts, source.status_counts);
    merge_counts(&mut target.error_counts, source.error_counts);
    merge_counts(
        &mut target.target_request_counts,
        source.target_request_counts,
    );
    append_bounded(
        &mut target.error_samples,
        source.error_samples,
        MAX_ERROR_SAMPLES,
    );
    append_bounded(
        &mut target.non_success_status_samples,
        source.non_success_status_samples,
        MAX_STATUS_SAMPLES,
    );
    target.failed_requests = target
        .failed_requests
        .saturating_add(source.failed_requests);
    target.completed_requests = target
        .completed_requests
        .saturating_add(source.completed_requests);
}

fn merge_counts<K: Ord>(target: &mut BTreeMap<K, usize>, source: BTreeMap<K, usize>) {
    for (key, count) in source {
        let current = target.entry(key).or_insert(0);
        *current = current.saturating_add(count);
    }
}

fn append_bounded<T>(target: &mut Vec<T>, source: Vec<T>, limit: usize) {
    let remaining = limit.saturating_sub(target.len());
    target.extend(source.into_iter().take(remaining));
}

fn record_load_error(
    error_counts: &mut BTreeMap<String, usize>,
    error_samples: &mut Vec<HttpLoadProbeErrorSample>,
    request_index: usize,
    url: &str,
    elapsed_ms: u64,
    error: ClassifiedLoadError,
) {
    *error_counts.entry(error.key).or_insert(0) += 1;

    if error_samples.len() < MAX_ERROR_SAMPLES {
        error_samples.push(HttpLoadProbeErrorSample {
            request_index,
            url: url.to_string(),
            phase: error.phase,
            kind: error.kind,
            elapsed_ms,
            message: error.message,
            source: error.source,
        });
    }
}

fn record_non_success_status_sample(
    non_success_status_samples: &mut Vec<HttpLoadProbeStatusSample>,
    request_index: usize,
    url: &str,
    status: u16,
    elapsed_ms: u64,
    body: &str,
) {
    if non_success_status_samples.len() < MAX_STATUS_SAMPLES {
        non_success_status_samples.push(HttpLoadProbeStatusSample {
            request_index,
            url: url.to_string(),
            status,
            elapsed_ms,
            body: compact_error_text(body, MAX_STATUS_SAMPLE_BODY_CHARS),
        });
    }
}

fn classify_reqwest_error(phase: &str, err: &reqwest::Error) -> ClassifiedLoadError {
    let kind = if err.is_timeout() && err.is_connect() {
        "connect_timeout"
    } else if err.is_timeout() && err.is_body() {
        "body_timeout"
    } else if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if err.is_body() {
        "body"
    } else if err.is_request() {
        "request"
    } else if err.is_decode() {
        "decode"
    } else if err.is_redirect() {
        "redirect"
    } else {
        "other"
    };

    ClassifiedLoadError {
        key: format!("{phase}:{kind}"),
        phase: phase.to_string(),
        kind: kind.to_string(),
        message: compact_error_text(err.to_string(), 240),
        source: error_source_chain(err, 240),
    }
}

fn classify_incomplete_sse(body_error: Option<&reqwest::Error>) -> ClassifiedLoadError {
    let mut incomplete = ClassifiedLoadError::static_body(
        "sse_incomplete",
        "SSE response body ended without [DONE] or a recognized completion event",
    );
    if let Some(body_error) = body_error {
        let body_error = classify_reqwest_error("body", body_error);
        incomplete.message = format!("{}: {}", incomplete.message, body_error.message);
        incomplete.source = body_error.source;
    }
    incomplete
}

fn classify_sse_error() -> ClassifiedLoadError {
    ClassifiedLoadError::static_body(
        "sse_error",
        "SSE response body contained an in-band error event",
    )
}

fn error_source_chain(err: &(dyn StdError + 'static), max_chars: usize) -> Option<String> {
    let mut sources = Vec::new();
    let mut next = err.source();
    while let Some(source) = next {
        sources.push(compact_error_text(source.to_string(), max_chars));
        if sources.len() >= 4 {
            break;
        }
        next = source.source();
    }
    (!sources.is_empty()).then(|| compact_error_text(sources.join(" | "), max_chars))
}

fn compact_error_text(value: impl AsRef<str>, max_chars: usize) -> String {
    let mut compact = value
        .as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if compact.chars().count() > max_chars {
        compact = compact.chars().take(max_chars.saturating_sub(1)).collect();
        compact.push_str("...");
    }
    compact
}

async fn observe_response_body(
    mut response: reqwest::Response,
    response_mode: HttpLoadProbeResponseMode,
    started_at: Instant,
    first_body_hold: Duration,
    require_sse_done: bool,
) -> Result<BodyObservation, ClassifiedLoadError> {
    match response_mode {
        HttpLoadProbeResponseMode::HeadersOnly => Ok(BodyObservation::default()),
        HttpLoadProbeResponseMode::FirstBodyByte => {
            let first = response
                .chunk()
                .await
                .map_err(|err| classify_reqwest_error("body", &err))?
                .ok_or_else(|| {
                    ClassifiedLoadError::static_body(
                        "empty_body",
                        "response body ended before the first chunk",
                    )
                })?;
            let first_body_latency_ms = started_at.elapsed().as_millis() as u64;
            let body_sample = compact_bytes_sample(&first, MAX_STATUS_SAMPLE_BODY_CHARS);
            if !first_body_hold.is_zero() {
                tokio::time::sleep(first_body_hold).await;
            }
            drain_first_body_response_tail(response).await?;
            Ok(BodyObservation {
                first_body_latency_ms: Some(first_body_latency_ms),
                body_sample: Some(body_sample),
            })
        }
        HttpLoadProbeResponseMode::FullBody => {
            let mut first_body_latency_ms = None;
            let mut body_sample = Vec::new();
            let mut sse_completion = SseCompletionDetector::default();
            loop {
                let chunk = match response.chunk().await {
                    Ok(Some(chunk)) => chunk,
                    Ok(None) => break,
                    Err(err) => {
                        if require_sse_done {
                            if sse_completion.has_error() {
                                return Err(classify_sse_error());
                            }
                            if first_body_latency_ms.is_some() && !sse_completion.is_complete() {
                                return Err(classify_incomplete_sse(Some(&err)));
                            }
                        }
                        return Err(classify_reqwest_error("body", &err));
                    }
                };
                if first_body_latency_ms.is_none() {
                    first_body_latency_ms = Some(started_at.elapsed().as_millis() as u64);
                }
                if require_sse_done {
                    sse_completion.observe(&chunk);
                }
                append_body_sample(&mut body_sample, &chunk, MAX_STATUS_SAMPLE_BODY_CHARS);
            }
            sse_completion.finish();
            if first_body_latency_ms.is_none() {
                return Err(ClassifiedLoadError::static_body(
                    "empty_body",
                    "response body ended before the first chunk",
                ));
            }
            if require_sse_done {
                if sse_completion.has_error() {
                    return Err(classify_sse_error());
                }
                if !sse_completion.is_complete() {
                    return Err(classify_incomplete_sse(None));
                }
            }
            Ok(BodyObservation {
                first_body_latency_ms,
                body_sample: Some(String::from_utf8_lossy(&body_sample).into_owned()),
            })
        }
    }
}

#[derive(Debug, Default)]
struct SseCompletionDetector {
    complete: bool,
    error: bool,
    pending_complete: bool,
    pending_error: bool,
    line: Vec<u8>,
    line_overflowed: bool,
}

impl SseCompletionDetector {
    fn observe(&mut self, chunk: &[u8]) {
        if self.error {
            return;
        }

        for &byte in chunk {
            if byte == b'\n' {
                if !self.line_overflowed {
                    if trim_ascii_whitespace(&self.line).is_empty() {
                        self.commit_event();
                    } else {
                        self.pending_error |= sse_line_is_error(&self.line);
                        self.pending_complete |= sse_line_is_completion(&self.line);
                    }
                }
                self.line.clear();
                self.line_overflowed = false;
                if self.error {
                    return;
                }
            } else if !self.line_overflowed {
                if self.line.len() < MAX_SSE_CONTROL_LINE_BYTES {
                    self.line.push(byte);
                } else {
                    self.line.clear();
                    self.line_overflowed = true;
                }
            }
        }
    }

    fn finish(&mut self) {
        // An SSE event is dispatched only after its terminating blank line. Do not commit a
        // partial final line or an event that was truncated between its last field and separator.
        self.line.clear();
        self.line_overflowed = false;
        self.pending_complete = false;
        self.pending_error = false;
    }

    fn commit_event(&mut self) {
        if self.pending_error {
            self.error = true;
        } else if self.pending_complete {
            self.complete = true;
        }
        self.pending_complete = false;
        self.pending_error = false;
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn has_error(&self) -> bool {
        self.error
    }
}

fn sse_line_is_error(line: &[u8]) -> bool {
    let line = trim_ascii_whitespace(line);
    let Some(separator) = line.iter().position(|byte| *byte == b':') else {
        return false;
    };
    let field = trim_ascii_whitespace(&line[..separator]);
    let value = trim_ascii_whitespace(&line[separator + 1..]);

    if field == b"event" {
        return std::str::from_utf8(value)
            .ok()
            .is_some_and(is_sse_error_event_name);
    }
    if field != b"data"
        || (!contains_bytes(value, b"\"error\"")
            && !SSE_ERROR_EVENT_NAMES
                .iter()
                .any(|name| contains_bytes(value, name.as_bytes())))
    {
        return false;
    }

    serde_json::from_slice::<serde_json::Value>(value)
        .ok()
        .is_some_and(|payload| {
            payload.as_object().is_some_and(|payload| {
                payload.get("error").is_some_and(|error| !error.is_null())
                    || payload
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(is_sse_error_event_name)
            })
        })
}

fn is_sse_error_event_name(name: &str) -> bool {
    SSE_ERROR_EVENT_NAMES.contains(&name)
}

fn sse_line_is_completion(line: &[u8]) -> bool {
    let line = trim_ascii_whitespace(line);
    let Some(separator) = line.iter().position(|byte| *byte == b':') else {
        return false;
    };
    let field = trim_ascii_whitespace(&line[..separator]);
    let value = trim_ascii_whitespace(&line[separator + 1..]);

    if field == b"event" {
        return std::str::from_utf8(value)
            .ok()
            .is_some_and(is_sse_completion_event_name);
    }
    if field != b"data" {
        return false;
    }
    if value == b"[DONE]" {
        return true;
    }
    if !SSE_COMPLETION_EVENT_NAMES
        .iter()
        .any(|name| contains_bytes(value, name.as_bytes()))
    {
        return false;
    }

    serde_json::from_slice::<serde_json::Value>(value)
        .ok()
        .and_then(|payload| {
            payload
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(is_sse_completion_event_name)
        })
        .unwrap_or(false)
}

fn is_sse_completion_event_name(name: &str) -> bool {
    SSE_COMPLETION_EVENT_NAMES.contains(&name)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn trim_ascii_whitespace(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn compact_bytes_sample(bytes: &[u8], max_chars: usize) -> String {
    compact_error_text(String::from_utf8_lossy(bytes), max_chars)
}

fn append_body_sample(target: &mut Vec<u8>, chunk: &[u8], max_chars: usize) {
    if target.len() >= max_chars {
        return;
    }
    let remaining = max_chars.saturating_sub(target.len());
    target.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
}

async fn drain_first_body_response_tail(
    mut response: reqwest::Response,
) -> Result<(), ClassifiedLoadError> {
    let max_chunks = env_usize(
        FIRST_BODY_BACKGROUND_DRAIN_CHUNKS_ENV,
        DEFAULT_FIRST_BODY_BACKGROUND_DRAIN_CHUNKS,
    );
    if max_chunks == 0 {
        return Ok(());
    }
    let timeout = Duration::from_millis(env_u64(
        FIRST_BODY_BACKGROUND_DRAIN_MS_ENV,
        DEFAULT_FIRST_BODY_BACKGROUND_DRAIN_MS,
    ));
    let drain = async {
        for _ in 0..max_chunks {
            let Some(chunk) = response
                .chunk()
                .await
                .map_err(|err| classify_reqwest_error("body", &err))?
            else {
                break;
            };
            drop(chunk);
        }
        Ok(())
    };
    if timeout.is_zero() {
        return drain.await;
    }
    match tokio::time::timeout(timeout, drain).await {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

fn env_usize(key: &str, default_value: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default_value)
}

fn env_u64(key: &str, default_value: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default_value)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BodyObservation {
    first_body_latency_ms: Option<u64>,
    body_sample: Option<String>,
}

fn build_request_header_sets(config: &HttpLoadProbeConfig) -> Result<Arc<Vec<HeaderMap>>, String> {
    let raw_sets = if config.header_sets.is_empty() {
        vec![config.headers.clone()]
    } else {
        config.header_sets.clone()
    };
    let mut sets = Vec::with_capacity(raw_sets.len().max(1));
    for headers in raw_sets {
        sets.push(build_headers(&headers)?);
    }
    if sets.is_empty() {
        sets.push(HeaderMap::new());
    }
    Ok(Arc::new(sets))
}

fn build_headers(headers: &BTreeMap<String, String>) -> Result<HeaderMap, String> {
    let mut result = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::try_from(name.as_str())
            .map_err(|err| format!("invalid load probe header name `{name}`: {err}"))?;
        let value = HeaderValue::from_str(value)
            .map_err(|err| format!("invalid load probe header value for `{name}`: {err}"))?;
        result.insert(name, value);
    }
    Ok(result)
}

fn summarize_latencies(latencies: &[u64]) -> (u64, u64, u64, u64, u64) {
    if latencies.is_empty() {
        return (0, 0, 0, 0, 0);
    }

    let max_ms = *latencies.last().unwrap_or(&0);
    let mean_ms = latencies.iter().sum::<u64>() / latencies.len() as u64;
    let p50_ms = percentile(latencies, 50);
    let p95_ms = percentile(latencies, 95);
    let p99_ms = percentile(latencies, 99);
    (p50_ms, p95_ms, p99_ms, max_ms, mean_ms)
}

fn percentile(latencies: &[u64], percentile: u8) -> u64 {
    if latencies.is_empty() {
        return 0;
    }
    let last_index = latencies.len() - 1;
    let rank = ((last_index as f64) * (percentile as f64 / 100.0)).round() as usize;
    latencies[rank.min(last_index)]
}

#[cfg(test)]
mod tests {
    use super::{
        build_headers, build_request_header_sets, run_http_load_probe,
        run_http_load_probe_with_options, summarize_latencies, validate_probe_options,
        warmup_request_distribution, worker_start_delay, HttpLoadProbeConfig, HttpLoadProbeOptions,
        HttpLoadProbeResponseMode, SseCompletionDetector,
    };
    use reqwest::Method;
    use std::collections::BTreeMap;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn validates_probe_config() {
        assert!(HttpLoadProbeConfig {
            url: String::new(),
            ..HttpLoadProbeConfig::default()
        }
        .validate()
        .is_err());
        assert!(HttpLoadProbeConfig {
            total_requests: 0,
            ..HttpLoadProbeConfig::default()
        }
        .validate()
        .is_err());
        assert!(HttpLoadProbeConfig {
            concurrency: 0,
            ..HttpLoadProbeConfig::default()
        }
        .validate()
        .is_err());
        assert!(HttpLoadProbeConfig {
            timeout: Duration::ZERO,
            ..HttpLoadProbeConfig::default()
        }
        .validate()
        .is_err());
        assert!(HttpLoadProbeConfig {
            connect_timeout: Some(Duration::ZERO),
            ..HttpLoadProbeConfig::default()
        }
        .validate()
        .is_err());
        assert!(HttpLoadProbeConfig {
            client_shards: 0,
            ..HttpLoadProbeConfig::default()
        }
        .validate()
        .is_err());
        assert!(HttpLoadProbeConfig {
            http1_only: true,
            http2_prior_knowledge: true,
            ..HttpLoadProbeConfig::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn summarizes_latency_distribution() {
        let (p50_ms, p95_ms, p99_ms, max_ms, mean_ms) =
            summarize_latencies(&[10, 20, 30, 40, 50, 60, 70, 80, 90, 100]);
        assert_eq!(p50_ms, 60);
        assert_eq!(p95_ms, 100);
        assert_eq!(p99_ms, 100);
        assert_eq!(max_ms, 100);
        assert_eq!(mean_ms, 55);
    }

    #[test]
    fn default_probe_config_is_reasonable() {
        let config = HttpLoadProbeConfig::default();
        assert_eq!(config.method, Method::GET);
        assert!(config.warmup_url.is_none());
        assert!(config.headers.is_empty());
        assert!(config.header_sets.is_empty());
        assert!(config.body.is_none());
        assert_eq!(config.total_requests, 100);
        assert_eq!(config.concurrency, 10);
        assert_eq!(config.warmup_connections, 0);
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.connect_timeout, None);
        assert_eq!(config.response_mode, HttpLoadProbeResponseMode::HeadersOnly);
        assert_eq!(config.client_shards, 1);
        assert_eq!(config.pool_max_idle_per_host, None);
        assert_eq!(config.start_ramp, Duration::ZERO);
        assert!(!config.http1_only);
        assert!(!config.http2_prior_knowledge);
        assert_eq!(config.first_body_hold, Duration::ZERO);
    }

    #[test]
    fn sse_completion_check_requires_full_body_mode() {
        let options = HttpLoadProbeOptions {
            require_sse_done: true,
        };
        assert!(validate_probe_options(&HttpLoadProbeConfig::default(), options).is_err());

        let config = HttpLoadProbeConfig {
            response_mode: HttpLoadProbeResponseMode::FullBody,
            ..HttpLoadProbeConfig::default()
        };
        assert!(validate_probe_options(&config, options).is_ok());
    }

    #[test]
    fn detects_sse_completion_markers_across_chunks() {
        let mut done = SseCompletionDetector::default();
        done.observe(b"data: [DO");
        assert!(!done.is_complete());
        done.observe(b"NE]\n\n");
        assert!(done.is_complete());

        let mut explicit_event = SseCompletionDetector::default();
        explicit_event.observe(b"event: response.comp");
        assert!(!explicit_event.is_complete());
        explicit_event.observe(b"leted\ndata: {}\n\n");
        assert!(explicit_event.is_complete());

        let mut explicit_data_type = SseCompletionDetector::default();
        explicit_data_type.observe(b"data: {\"type\":\"message_stop\"}\n\n");
        assert!(explicit_data_type.is_complete());

        let mut incomplete_response = SseCompletionDetector::default();
        incomplete_response.observe(b"event: response.incomplete\ndata: {}\n\n");
        assert!(incomplete_response.is_complete());
        assert!(!incomplete_response.has_error());

        let mut truncated_event = SseCompletionDetector::default();
        truncated_event.observe(b"event: response.completed\n");
        assert!(!truncated_event.is_complete());
        truncated_event.observe(b"\n");
        assert!(truncated_event.is_complete());

        let mut content_only = SseCompletionDetector::default();
        content_only.observe(b"data: {\"delta\":{\"content\":\"[DONE] response.completed\"}}\n\n");
        assert!(!content_only.is_complete());
    }

    #[test]
    fn detects_in_band_sse_errors_across_chunks() {
        let mut data_error = SseCompletionDetector::default();
        data_error.observe(b"data: {\"err");
        assert!(!data_error.has_error());
        data_error.observe(
            b"or\":{\"type\":\"execution_runtime_stream_read_error\"}}\n\ndata: [DONE]\n\n",
        );
        assert!(data_error.has_error());

        let mut event_error = SseCompletionDetector::default();
        event_error.observe(b"event:err");
        assert!(!event_error.has_error());
        event_error.observe(b"or\ndata: {\"message\":\"upstream failed\"}\n\ndata: [DONE]\n\n");
        assert!(event_error.has_error());

        let mut response_failed = SseCompletionDetector::default();
        response_failed
            .observe(b"data: [DONE]\n\ndata: {\"type\":\"response.failed\",\"response\":{}}\n\n");
        assert!(response_failed.has_error());

        let mut null_error = SseCompletionDetector::default();
        null_error.observe(b"data: {\"error\":null}\n\ndata: [DONE]\n\n");
        assert!(!null_error.has_error());
        assert!(null_error.is_complete());

        let mut nested_error = SseCompletionDetector::default();
        nested_error
            .observe(b"data: {\"delta\":{\"error\":\"quoted output\"}}\n\ndata: [DONE]\n\n");
        assert!(!nested_error.has_error());
        assert!(nested_error.is_complete());
    }

    #[tokio::test]
    async fn missing_sse_completion_is_failed_without_losing_http_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener address should resolve");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("test connection should arrive");
            let mut request = [0_u8; 1024];
            let _ = stream
                .read(&mut request)
                .await
                .expect("test request should read");
            let body = "data: {\"partial\":true}\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("test response should write");
        });
        let config = HttpLoadProbeConfig {
            url: format!("http://{address}/stream"),
            total_requests: 1,
            concurrency: 1,
            response_mode: HttpLoadProbeResponseMode::FullBody,
            ..HttpLoadProbeConfig::default()
        };

        let result = run_http_load_probe_with_options(
            &config,
            HttpLoadProbeOptions {
                require_sse_done: true,
            },
        )
        .await
        .expect("load probe should complete");
        server.await.expect("test server should stop");

        assert!(result.require_sse_done);
        assert_eq!(result.completed_requests, 1);
        assert_eq!(result.max_in_flight_requests, 1);
        assert_eq!(result.failed_requests, 1);
        assert_eq!(result.status_counts.get(&200), Some(&1));
        assert_eq!(result.error_counts.get("body:sse_incomplete"), Some(&1));
        assert_eq!(result.error_samples[0].phase, "body");
        assert_eq!(result.error_samples[0].kind, "sse_incomplete");
    }

    #[tokio::test]
    async fn in_band_sse_error_is_failed_even_when_done_follows() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener address should resolve");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("test connection should arrive");
            let mut request = [0_u8; 1024];
            let _ = stream
                .read(&mut request)
                .await
                .expect("test request should read");
            let body = concat!(
                "data: {\"error\":{\"type\":\"execution_runtime_stream_read_error\",",
                "\"code\":502}}\n\n",
                "data: [DONE]\n\n",
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("test response should write");
        });
        let config = HttpLoadProbeConfig {
            url: format!("http://{address}/stream"),
            total_requests: 1,
            concurrency: 1,
            response_mode: HttpLoadProbeResponseMode::FullBody,
            ..HttpLoadProbeConfig::default()
        };

        let result = run_http_load_probe_with_options(
            &config,
            HttpLoadProbeOptions {
                require_sse_done: true,
            },
        )
        .await
        .expect("load probe should complete");
        server.await.expect("test server should stop");

        assert_eq!(result.completed_requests, 1);
        assert_eq!(result.failed_requests, 1);
        assert_eq!(result.status_counts.get(&200), Some(&1));
        assert_eq!(result.error_counts.get("body:sse_error"), Some(&1));
        assert_eq!(result.error_samples[0].phase, "body");
        assert_eq!(result.error_samples[0].kind, "sse_error");
    }

    #[tokio::test]
    async fn result_serializes_effective_request_timeout_ms() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener address should resolve");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("test connection should arrive");
            let mut request = [0_u8; 1024];
            let _ = stream
                .read(&mut request)
                .await
                .expect("test request should read");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("test response should write");
        });
        let config = HttpLoadProbeConfig {
            url: format!("http://{address}/health"),
            total_requests: 1,
            concurrency: 1,
            timeout: Duration::from_secs(120),
            ..HttpLoadProbeConfig::default()
        };

        let result = run_http_load_probe(&config)
            .await
            .expect("load probe should complete");
        server.await.expect("test server should stop");

        assert_eq!(result.timeout_ms, 120_000);
        let serialized = serde_json::to_value(result).expect("result should serialize");
        assert_eq!(
            serialized
                .get("timeout_ms")
                .and_then(|value| value.as_u64()),
            Some(120_000)
        );
    }

    #[test]
    fn validates_probe_headers() {
        let mut headers = BTreeMap::new();
        headers.insert("x-aether-test".to_string(), "ok".to_string());
        let built = build_headers(&headers).expect("headers should build");
        assert_eq!(
            built
                .get("x-aether-test")
                .and_then(|value| value.to_str().ok()),
            Some("ok")
        );
        let invalid = BTreeMap::from([("bad header".to_string(), "ok".to_string())]);
        assert!(build_headers(&invalid).is_err());
    }

    #[test]
    fn validates_header_sets() {
        let mut config = HttpLoadProbeConfig {
            url: "http://127.0.0.1/".to_string(),
            header_sets: vec![BTreeMap::new()],
            ..HttpLoadProbeConfig::default()
        };
        assert!(config.validate().is_err());

        config.header_sets = vec![BTreeMap::from([(
            "authorization".to_string(),
            "Bearer test".to_string(),
        )])];
        assert!(config.validate().is_ok());

        let sets = build_request_header_sets(&config).expect("header set should build");
        assert_eq!(sets.len(), 1);
        assert_eq!(
            sets[0]
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer test")
        );
    }

    #[test]
    fn spreads_worker_start_delay_across_ramp() {
        assert_eq!(
            worker_start_delay(Duration::from_millis(900), 0, 10),
            Duration::ZERO
        );
        assert_eq!(
            worker_start_delay(Duration::from_millis(900), 5, 10),
            Duration::from_millis(500)
        );
        assert_eq!(
            worker_start_delay(Duration::from_millis(900), 9, 10),
            Duration::from_millis(900)
        );
        assert_eq!(
            worker_start_delay(Duration::from_millis(900), 3, 1),
            Duration::ZERO
        );
    }

    #[test]
    fn distributes_warmup_requests_across_each_used_client_shard() {
        assert_eq!(warmup_request_distribution(0, 10), Vec::<usize>::new());
        assert_eq!(warmup_request_distribution(4, 0), Vec::<usize>::new());
        assert_eq!(warmup_request_distribution(4, 4), vec![1, 1, 1, 1]);
        assert_eq!(warmup_request_distribution(4, 6), vec![2, 2, 1, 1]);
        assert_eq!(warmup_request_distribution(4, 10), vec![3, 3, 2, 2]);
        assert_eq!(warmup_request_distribution(8, 3), vec![1, 1, 1]);
    }
}
