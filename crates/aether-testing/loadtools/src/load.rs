// HTTP load generation is intentionally independent from gateway internals.
use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use http::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};
use tokio::sync::Mutex;

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

#[derive(Debug, Clone, Copy, Default, serde::Serialize, PartialEq, Eq)]
pub enum HttpLoadProbeResponseMode {
    #[default]
    HeadersOnly,
    FirstBodyByte,
    FullBody,
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
    pub total_requests: usize,
    pub concurrency: usize,
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
    pub total_requests: usize,
    pub concurrency: usize,
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

pub async fn run_http_load_probe(
    config: &HttpLoadProbeConfig,
) -> Result<HttpLoadProbeResult, String> {
    config.validate()?;
    run_http_load_probe_against_urls(config, std::slice::from_ref(&config.url))
        .await
        .map(|result| HttpLoadProbeResult {
            url: result
                .target_urls
                .into_iter()
                .next()
                .unwrap_or_else(|| config.url.clone()),
            method: result.method,
            response_mode: result.response_mode,
            total_requests: result.total_requests,
            concurrency: result.concurrency,
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
    config.validate()?;
    if urls.is_empty() {
        return Err("multi-url load probe requires at least one target url".to_string());
    }
    run_http_load_probe_against_urls(config, urls).await
}

async fn run_http_load_probe_against_urls(
    config: &HttpLoadProbeConfig,
    urls: &[String],
) -> Result<MultiUrlHttpLoadProbeResult, String> {
    let effective_client_shards = effective_probe_client_shards(config);
    let clients = Arc::new(build_probe_clients(config, effective_client_shards)?);
    let total_requests = config.total_requests;
    let request_headers = build_request_header_sets(config)?;
    let request_body = config.body.clone().map(Arc::new);
    let response_mode = config.response_mode;
    let first_body_hold = config.first_body_hold;
    let start_ramp = config.start_ramp;
    warmup_probe_connections(config, Arc::clone(&clients)).await?;
    let mut runtime_sampler = BenchmarkRuntimeSampler::new();
    let started_at = Instant::now();

    let next_request = Arc::new(AtomicUsize::new(0));
    let latencies_ms = Arc::new(Mutex::new(Vec::with_capacity(config.total_requests)));
    let header_latencies_ms = Arc::new(Mutex::new(Vec::with_capacity(config.total_requests)));
    let first_body_latencies_ms = Arc::new(Mutex::new(Vec::with_capacity(config.total_requests)));
    let status_counts = Arc::new(Mutex::new(BTreeMap::<u16, usize>::new()));
    let error_counts = Arc::new(Mutex::new(BTreeMap::<String, usize>::new()));
    let error_samples = Arc::new(Mutex::new(Vec::<HttpLoadProbeErrorSample>::new()));
    let non_success_status_samples = Arc::new(Mutex::new(Vec::<HttpLoadProbeStatusSample>::new()));
    let target_request_counts = Arc::new(Mutex::new(BTreeMap::<String, usize>::new()));
    let failed_requests = Arc::new(AtomicUsize::new(0));
    let completed_requests = Arc::new(AtomicUsize::new(0));

    let mut workers = tokio::task::JoinSet::new();
    for worker_index in 0..config.concurrency {
        let client = clients[worker_index % clients.len()].clone();
        let next_request = Arc::clone(&next_request);
        let latencies_ms = Arc::clone(&latencies_ms);
        let header_latencies_ms = Arc::clone(&header_latencies_ms);
        let first_body_latencies_ms = Arc::clone(&first_body_latencies_ms);
        let status_counts = Arc::clone(&status_counts);
        let error_counts = Arc::clone(&error_counts);
        let error_samples = Arc::clone(&error_samples);
        let non_success_status_samples = Arc::clone(&non_success_status_samples);
        let target_request_counts = Arc::clone(&target_request_counts);
        let failed_requests = Arc::clone(&failed_requests);
        let completed_requests = Arc::clone(&completed_requests);
        let method = config.method.clone();
        let urls = urls.to_vec();
        let request_headers = Arc::clone(&request_headers);
        let request_body = request_body.clone();
        let start_delay = worker_start_delay(start_ramp, worker_index, config.concurrency);

        workers.spawn(async move {
            if !start_delay.is_zero() {
                tokio::time::sleep(start_delay).await;
            }
            loop {
                let current = next_request.fetch_add(1, Ordering::AcqRel);
                if current >= total_requests {
                    break;
                }

                let started_at = Instant::now();
                let url = urls[current % urls.len()].clone();
                let mut request = client.request(method.clone(), &url);
                let headers = &request_headers[current % request_headers.len()];
                for (name, value) in headers.iter() {
                    request = request.header(name, value);
                }
                if let Some(body) = request_body.as_ref() {
                    request = request.body(body.as_ref().clone());
                }
                match request.send().await {
                    Ok(response) => {
                        let headers_latency_ms = started_at.elapsed().as_millis() as u64;
                        let status = response.status().as_u16();
                        let body_result = observe_response_body(
                            response,
                            response_mode,
                            started_at,
                            first_body_hold,
                        )
                        .await;
                        match body_result {
                            Err(error) => {
                                failed_requests.fetch_add(1, Ordering::AcqRel);
                                record_load_error(
                                    &error_counts,
                                    &error_samples,
                                    current,
                                    &url,
                                    started_at.elapsed().as_millis() as u64,
                                    error,
                                )
                                .await;
                            }
                            Ok(observation) => {
                                if !(200..300).contains(&status) {
                                    record_non_success_status_sample(
                                        &non_success_status_samples,
                                        current,
                                        &url,
                                        status,
                                        started_at.elapsed().as_millis() as u64,
                                        observation.body_sample.as_deref().unwrap_or_default(),
                                    )
                                    .await;
                                }
                                let mut counts = status_counts.lock().await;
                                *counts.entry(status).or_insert(0) += 1;
                                drop(counts);
                                let mut target_counts = target_request_counts.lock().await;
                                *target_counts.entry(url).or_insert(0) += 1;
                                if let Some(first_body_latency_ms) =
                                    observation.first_body_latency_ms
                                {
                                    first_body_latencies_ms
                                        .lock()
                                        .await
                                        .push(first_body_latency_ms);
                                }
                            }
                        }
                        let latency_ms = started_at.elapsed().as_millis() as u64;
                        latencies_ms.lock().await.push(latency_ms);
                        header_latencies_ms.lock().await.push(headers_latency_ms);
                        completed_requests.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(err) => {
                        let latency_ms = started_at.elapsed().as_millis() as u64;
                        latencies_ms.lock().await.push(latency_ms);
                        failed_requests.fetch_add(1, Ordering::AcqRel);
                        record_load_error(
                            &error_counts,
                            &error_samples,
                            current,
                            &url,
                            latency_ms,
                            classify_reqwest_error("send", &err),
                        )
                        .await;
                        completed_requests.fetch_add(1, Ordering::AcqRel);
                    }
                }
            }
        });
    }

    while let Some(result) = workers.join_next().await {
        result.map_err(|err| format!("load probe worker task failed: {err}"))?;
    }

    let status_counts = status_counts.lock().await.clone();
    let error_counts = error_counts.lock().await.clone();
    let error_samples = error_samples.lock().await.clone();
    let non_success_status_samples = non_success_status_samples.lock().await.clone();
    let target_request_counts = target_request_counts.lock().await.clone();
    let mut latencies = latencies_ms.lock().await.clone();
    let mut header_latencies = header_latencies_ms.lock().await.clone();
    let mut first_body_latencies = first_body_latencies_ms.lock().await.clone();
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
        completed_requests.load(Ordering::Acquire) as u64
    } else {
        ((completed_requests.load(Ordering::Acquire) as u64) * 1_000) / duration_ms.max(1)
    };

    Ok(MultiUrlHttpLoadProbeResult {
        target_urls: urls.to_vec(),
        target_request_counts,
        method: config.method.as_str().to_string(),
        response_mode: config.response_mode,
        total_requests: config.total_requests,
        concurrency: config.concurrency,
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
        completed_requests: completed_requests.load(Ordering::Acquire),
        failed_requests: failed_requests.load(Ordering::Acquire),
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
    let next_request = Arc::new(AtomicUsize::new(0));
    let mut workers = tokio::task::JoinSet::new();
    let concurrency = config.concurrency.min(config.warmup_connections).max(1);
    for worker_index in 0..concurrency {
        let client = clients[worker_index % clients.len()].clone();
        let next_request = Arc::clone(&next_request);
        let warmup_url = warmup_url.to_string();
        let total = config.warmup_connections;
        workers.spawn(async move {
            loop {
                let current = next_request.fetch_add(1, Ordering::AcqRel);
                if current >= total {
                    break;
                }
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

async fn record_load_error(
    error_counts: &Arc<Mutex<BTreeMap<String, usize>>>,
    error_samples: &Arc<Mutex<Vec<HttpLoadProbeErrorSample>>>,
    request_index: usize,
    url: &str,
    elapsed_ms: u64,
    error: ClassifiedLoadError,
) {
    let mut counts = error_counts.lock().await;
    *counts.entry(error.key).or_insert(0) += 1;
    drop(counts);

    let mut samples = error_samples.lock().await;
    if samples.len() < MAX_ERROR_SAMPLES {
        samples.push(HttpLoadProbeErrorSample {
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

async fn record_non_success_status_sample(
    non_success_status_samples: &Arc<Mutex<Vec<HttpLoadProbeStatusSample>>>,
    request_index: usize,
    url: &str,
    status: u16,
    elapsed_ms: u64,
    body: &str,
) {
    let mut samples = non_success_status_samples.lock().await;
    if samples.len() < MAX_STATUS_SAMPLES {
        samples.push(HttpLoadProbeStatusSample {
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
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|err| classify_reqwest_error("body", &err))?
            {
                if first_body_latency_ms.is_none() {
                    first_body_latency_ms = Some(started_at.elapsed().as_millis() as u64);
                }
                append_body_sample(&mut body_sample, &chunk, MAX_STATUS_SAMPLE_BODY_CHARS);
            }
            if first_body_latency_ms.is_none() {
                return Err(ClassifiedLoadError::static_body(
                    "empty_body",
                    "response body ended before the first chunk",
                ));
            }
            Ok(BodyObservation {
                first_body_latency_ms,
                body_sample: Some(String::from_utf8_lossy(&body_sample).into_owned()),
            })
        }
    }
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
        build_headers, build_request_header_sets, run_http_load_probe, summarize_latencies,
        worker_start_delay, HttpLoadProbeConfig, HttpLoadProbeResponseMode,
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
}
