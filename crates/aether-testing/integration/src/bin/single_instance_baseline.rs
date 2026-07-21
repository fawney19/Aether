// Gateway-backed benchmark scenarios live outside the reusable testkit.
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
use aether_testkit::{
    init_test_runtime_for, run_http_load_probe, ExecutionRuntimeHarness,
    ExecutionRuntimeHarnessConfig, GatewayHarness, GatewayHarnessConfig, HttpLoadProbeConfig,
    HttpLoadProbeResponseMode, HttpLoadProbeResult, SpawnedServer, GATEWAY_HARNESS_API_KEY,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use reqwest::Method;
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone)]
struct SingleInstanceBaselineConfig {
    sync_requests: usize,
    sync_concurrency: usize,
    stream_requests: usize,
    stream_concurrency: usize,
    timeout: Duration,
    output_path: Option<PathBuf>,
}

impl Default for SingleInstanceBaselineConfig {
    fn default() -> Self {
        Self {
            sync_requests: 200,
            sync_concurrency: 20,
            stream_requests: 100,
            stream_concurrency: 10,
            timeout: Duration::from_secs(10),
            output_path: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct NamedBaselineResult {
    name: String,
    result: HttpLoadProbeResult,
}

#[derive(Debug, Serialize)]
struct SingleInstanceBaselineReport {
    suite: &'static str,
    scenarios: Vec<NamedBaselineResult>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_test_runtime_for("single-instance-baseline");
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
    config: &SingleInstanceBaselineConfig,
) -> Result<SingleInstanceBaselineReport, Box<dyn std::error::Error>> {
    let upstream = SpawnedServer::start(build_fake_upstream()).await?;
    let gateway = GatewayHarness::start(GatewayHarnessConfig::new(upstream.base_url())).await?;
    let runtime = ExecutionRuntimeHarness::start(ExecutionRuntimeHarnessConfig::default()).await?;

    let gateway_sync = run_http_load_probe(&gateway_sync_probe_config(gateway.base_url(), config))
        .await
        .map_err(std::io::Error::other)?;
    let gateway_stream =
        run_http_load_probe(&gateway_stream_probe_config(gateway.base_url(), config))
            .await
            .map_err(std::io::Error::other)?;
    let execution_runtime_sync = run_http_load_probe(&execution_runtime_sync_probe_config(
        runtime.base_url(),
        upstream.base_url(),
        config,
    ))
    .await
    .map_err(std::io::Error::other)?;
    let execution_runtime_stream = run_http_load_probe(&execution_runtime_stream_probe_config(
        runtime.base_url(),
        upstream.base_url(),
        config,
    ))
    .await
    .map_err(std::io::Error::other)?;

    Ok(SingleInstanceBaselineReport {
        suite: "single_instance_baseline",
        scenarios: vec![
            NamedBaselineResult {
                name: "gateway_proxy_sync".to_string(),
                result: gateway_sync,
            },
            NamedBaselineResult {
                name: "gateway_proxy_stream".to_string(),
                result: gateway_stream,
            },
            NamedBaselineResult {
                name: "execution_runtime_sync".to_string(),
                result: execution_runtime_sync,
            },
            NamedBaselineResult {
                name: "execution_runtime_stream".to_string(),
                result: execution_runtime_stream,
            },
        ],
    })
}

fn gateway_sync_probe_config(
    gateway_base_url: &str,
    config: &SingleInstanceBaselineConfig,
) -> HttpLoadProbeConfig {
    let mut probe = chat_probe_config(
        format!("{gateway_base_url}/v1/chat/completions"),
        false,
        config.sync_requests,
        config.sync_concurrency,
        config.timeout,
    );
    probe.response_mode = HttpLoadProbeResponseMode::FullBody;
    probe
}

fn gateway_stream_probe_config(
    gateway_base_url: &str,
    config: &SingleInstanceBaselineConfig,
) -> HttpLoadProbeConfig {
    let mut probe = chat_probe_config(
        format!("{gateway_base_url}/v1/chat/completions"),
        true,
        config.stream_requests,
        config.stream_concurrency,
        config.timeout,
    );
    probe.response_mode = HttpLoadProbeResponseMode::FullBody;
    probe
}

fn execution_runtime_sync_probe_config(
    runtime_base_url: &str,
    upstream_base_url: &str,
    config: &SingleInstanceBaselineConfig,
) -> HttpLoadProbeConfig {
    execution_probe_config(
        format!("{runtime_base_url}/v1/execute/sync"),
        execution_plan(format!("{upstream_base_url}/v1/chat/completions"), false),
        config.sync_requests,
        config.sync_concurrency,
        config.timeout,
    )
}

fn execution_runtime_stream_probe_config(
    runtime_base_url: &str,
    upstream_base_url: &str,
    config: &SingleInstanceBaselineConfig,
) -> HttpLoadProbeConfig {
    execution_probe_config(
        format!("{runtime_base_url}/v1/execute/stream"),
        execution_plan(format!("{upstream_base_url}/v1/chat/completions"), true),
        config.stream_requests,
        config.stream_concurrency,
        config.timeout,
    )
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
            serde_json::to_vec(&plan).expect("execution plan should serialize for load probe"),
        ),
        total_requests,
        concurrency,
        timeout,
        response_mode: HttpLoadProbeResponseMode::FullBody,
        ..HttpLoadProbeConfig::default()
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
        headers: BTreeMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            (
                "authorization".to_string(),
                format!("Bearer {GATEWAY_HARNESS_API_KEY}"),
            ),
        ]),
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
        ..HttpLoadProbeConfig::default()
    }
}

fn execution_plan(url: String, stream: bool) -> ExecutionPlan {
    ExecutionPlan {
        request_id: if stream {
            "baseline-stream-request".to_string()
        } else {
            "baseline-sync-request".to_string()
        },
        candidate_id: Some(if stream {
            "baseline-stream-candidate".to_string()
        } else {
            "baseline-sync-candidate".to_string()
        }),
        provider_name: Some("openai".to_string()),
        provider_id: "provider-baseline".to_string(),
        endpoint_id: "endpoint-baseline".to_string(),
        key_id: "key-baseline".to_string(),
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
        transport_profile: None,
        timeouts: Some(ExecutionTimeouts {
            connect_ms: Some(2_000),
            read_ms: Some(10_000),
            first_byte_ms: Some(5_000),
            total_ms: Some(10_000),
            ..ExecutionTimeouts::default()
        }),
    }
}

fn build_fake_upstream() -> Router {
    Router::new().route(
        "/v1/chat/completions",
        any(|request: Request| async move {
            let (_parts, body) = request.into_parts();
            let raw_body = to_bytes(body, usize::MAX)
                .await
                .expect("fake upstream body should read");
            let payload: serde_json::Value =
                serde_json::from_slice(&raw_body).unwrap_or_else(|_| json!({}));
            let stream = payload
                .get("stream")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if stream {
                let body = futures_util::stream::iter([
                    Ok::<_, Infallible>(Bytes::from_static(
                        b"data: {\"id\":\"chunk-1\",\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
                    )),
                    Ok::<_, Infallible>(Bytes::from_static(
                        b"data: {\"id\":\"chunk-2\",\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
                    )),
                    Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n")),
                ]);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(http::header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(body))
                    .expect("fake upstream stream response should build")
            } else {
                Json(json!({
                    "id": "chatcmpl-baseline",
                    "object": "chat.completion",
                    "model": payload.get("model").and_then(|value| value.as_str()).unwrap_or("gpt-5"),
                    "choices": [{"message": {"role": "assistant", "content": "hello"}}]
                }))
                .into_response()
            }
        }),
    )
}

fn parse_args(
    args: Vec<String>,
) -> Result<SingleInstanceBaselineConfig, Box<dyn std::error::Error>> {
    let mut config = SingleInstanceBaselineConfig::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--sync-requests" => {
                config.sync_requests = next_value(&mut iter, "--sync-requests")?.parse()?
            }
            "--sync-concurrency" => {
                config.sync_concurrency = next_value(&mut iter, "--sync-concurrency")?.parse()?
            }
            "--stream-requests" => {
                config.stream_requests = next_value(&mut iter, "--stream-requests")?.parse()?
            }
            "--stream-concurrency" => {
                config.stream_concurrency =
                    next_value(&mut iter, "--stream-concurrency")?.parse()?
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
        "usage: cargo run -p aether-integration-tests --bin single_instance_baseline -- [--sync-requests 200] [--sync-concurrency 20] [--stream-requests 100] [--stream-concurrency 10] [--timeout-ms 10000] [--output /tmp/baseline.json]"
    );
}
