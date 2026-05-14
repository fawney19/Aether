use std::path::Path;
use std::sync::Arc;

use aether_contracts::ExecutionPlan;
use aether_runtime::{
    maybe_hold_axum_response_permit, prometheus_response, service_up_sample, AdmissionPermit,
    ConcurrencyError, ConcurrencyGate, ConcurrencySnapshot, MetricKind, MetricLabel, MetricSample,
};
use aether_runtime_state::{RuntimeSemaphore, RuntimeSemaphoreError, RuntimeSemaphoreSnapshot};
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use thiserror::Error;

use crate::execution_runtime::{
    build_direct_execution_frame_stream, DirectSyncExecutionRuntime, ExecutionRuntimeTransportError,
};
use crate::middleware;

const EXECUTION_RUNTIME_COMPONENT: &str = "aether-gateway-execution-runtime";
const REQUEST_GATE_NAME: &str = "execution_runtime_requests";
const DISTRIBUTED_REQUEST_GATE_NAME: &str = "execution_runtime_requests_distributed";

#[derive(Debug, Clone, Default)]
struct ExecutionRuntimeAppState {
    execution_runtime: DirectSyncExecutionRuntime,
    request_gate: Option<Arc<ConcurrencyGate>>,
    distributed_request_gate: Option<Arc<RuntimeSemaphore>>,
}

impl ExecutionRuntimeAppState {
    fn with_request_concurrency_limit(limit: Option<usize>) -> Self {
        Self {
            execution_runtime: DirectSyncExecutionRuntime::new(),
            request_gate: limit
                .filter(|limit| *limit > 0)
                .map(|limit| Arc::new(ConcurrencyGate::new(REQUEST_GATE_NAME, limit))),
            distributed_request_gate: None,
        }
    }

    fn with_distributed_request_gate(mut self, gate: RuntimeSemaphore) -> Self {
        self.distributed_request_gate = Some(Arc::new(gate));
        self
    }

    fn request_concurrency_snapshot(&self) -> Option<ConcurrencySnapshot> {
        self.request_gate.as_ref().map(|gate| gate.snapshot())
    }

    async fn distributed_request_concurrency_snapshot(
        &self,
    ) -> Result<Option<RuntimeSemaphoreSnapshot>, RuntimeSemaphoreError> {
        match self.distributed_request_gate.as_ref() {
            Some(gate) => gate.snapshot().await.map(Some),
            None => Ok(None),
        }
    }

    async fn metric_samples(&self) -> Vec<MetricSample> {
        let mut samples = vec![service_up_sample(EXECUTION_RUNTIME_COMPONENT)];
        if let Some(snapshot) = self.request_concurrency_snapshot() {
            samples.extend(snapshot.to_metric_samples(REQUEST_GATE_NAME));
        }
        if let Some(gate) = self.distributed_request_gate.as_ref() {
            match gate.snapshot().await {
                Ok(snapshot) => {
                    samples.extend(snapshot.to_metric_samples(DISTRIBUTED_REQUEST_GATE_NAME));
                }
                Err(_) => samples.push(
                    MetricSample::new(
                        "concurrency_unavailable",
                        "Whether the distributed concurrency gate is currently unavailable.",
                        MetricKind::Gauge,
                        1,
                    )
                    .with_labels(vec![MetricLabel::new(
                        "gate",
                        DISTRIBUTED_REQUEST_GATE_NAME,
                    )]),
                ),
            }
        }
        samples
    }

    async fn try_acquire_request_permit(
        &self,
    ) -> Result<Option<AdmissionPermit>, RequestAdmissionError> {
        let local = self
            .request_gate
            .as_ref()
            .map(|gate| gate.try_acquire())
            .transpose()
            .map_err(RequestAdmissionError::Local)?;
        let distributed = match self.distributed_request_gate.as_ref() {
            Some(gate) => Some(
                gate.try_acquire()
                    .await
                    .map_err(RequestAdmissionError::Distributed)?,
            ),
            None => None,
        };
        Ok(AdmissionPermit::from_parts(local, distributed))
    }
}

pub fn build_execution_runtime_router() -> Router {
    build_execution_runtime_router_with_request_concurrency_limit(None)
}

pub fn build_execution_runtime_router_with_request_concurrency_limit(
    limit: Option<usize>,
) -> Router {
    build_execution_runtime_router_with_request_gates(limit, None)
}

pub fn build_execution_runtime_router_with_request_gates(
    limit: Option<usize>,
    distributed_gate: Option<RuntimeSemaphore>,
) -> Router {
    let state = match distributed_gate {
        Some(gate) => ExecutionRuntimeAppState::with_request_concurrency_limit(limit)
            .with_distributed_request_gate(gate),
        None => ExecutionRuntimeAppState::with_request_concurrency_limit(limit),
    };
    middleware::apply_cf_header_stripping(
        Router::new()
            .route("/health", get(health))
            .route("/metrics", get(metrics))
            .route("/v1/execute/sync", post(execute_sync))
            .route("/v1/execute/stream", post(execute_stream))
            .with_state(state),
    )
}

pub async fn serve_execution_runtime_tcp(
    bind: &str,
    max_in_flight_requests: Option<usize>,
    distributed_request_gate: Option<RuntimeSemaphore>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(
        listener,
        build_execution_runtime_router_with_request_gates(
            max_in_flight_requests,
            distributed_request_gate,
        ),
    )
    .await?;
    Ok(())
}

#[cfg(unix)]
pub async fn serve_execution_runtime_unix(
    socket_path: &Path,
    max_in_flight_requests: Option<usize>,
    distributed_request_gate: Option<RuntimeSemaphore>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = tokio::net::UnixListener::bind(socket_path)?;
    axum::serve(
        listener,
        build_execution_runtime_router_with_request_gates(
            max_in_flight_requests,
            distributed_request_gate,
        ),
    )
    .await?;
    Ok(())
}

#[cfg(not(unix))]
pub async fn serve_execution_runtime_unix(
    _socket_path: &Path,
    _max_in_flight_requests: Option<usize>,
    _distributed_request_gate: Option<RuntimeSemaphore>,
) -> Result<(), Box<dyn std::error::Error>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix sockets are not supported on this platform",
    )
    .into())
}

async fn health(State(state): State<ExecutionRuntimeAppState>) -> impl IntoResponse {
    let request_concurrency = state.request_concurrency_snapshot().map(|snapshot| {
        json!({
            "limit": snapshot.limit,
            "in_flight": snapshot.in_flight,
            "available_permits": snapshot.available_permits,
            "high_watermark": snapshot.high_watermark,
            "rejected": snapshot.rejected,
        })
    });
    let distributed_request_concurrency = state
        .distributed_request_concurrency_snapshot()
        .await
        .ok()
        .flatten()
        .map(|snapshot| {
            json!({
                "limit": snapshot.limit,
                "in_flight": snapshot.in_flight,
                "available_permits": snapshot.available_permits,
                "high_watermark": snapshot.high_watermark,
                "rejected": snapshot.rejected,
            })
        });
    Json(json!({
        "status": "ok",
        "component": EXECUTION_RUNTIME_COMPONENT,
        "request_concurrency": request_concurrency,
        "distributed_request_concurrency": distributed_request_concurrency,
    }))
}

async fn metrics(State(state): State<ExecutionRuntimeAppState>) -> Response {
    prometheus_response(&state.metric_samples().await)
}

async fn execute_sync(
    State(state): State<ExecutionRuntimeAppState>,
    request: Request,
) -> Result<Response, ExecutionRuntimeAppError> {
    let request_permit = acquire_request_permit(&state).await?;
    let plan = parse_request_json::<ExecutionPlan>(request).await?;
    let result = state
        .execution_runtime
        .execute_sync(&plan)
        .await
        .map_err(|err| ExecutionRuntimeAppError(ExecutionRuntimeServerError::Transport(err)))?;
    Ok(maybe_hold_axum_response_permit(
        Json(result).into_response(),
        request_permit,
    ))
}

async fn execute_stream(
    State(state): State<ExecutionRuntimeAppState>,
    request: Request,
) -> Result<Response, ExecutionRuntimeAppError> {
    let request_permit = acquire_request_permit(&state).await?;
    let plan = parse_request_json::<ExecutionPlan>(request).await?;
    let execution = state
        .execution_runtime
        .execute_stream(&plan)
        .await
        .map_err(|err| ExecutionRuntimeAppError(ExecutionRuntimeServerError::Transport(err)))?;

    let mut response = Response::new(Body::from_stream(build_direct_execution_frame_stream(
        execution,
    )));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/x-ndjson"),
    );
    Ok(maybe_hold_axum_response_permit(response, request_permit))
}

async fn acquire_request_permit(
    state: &ExecutionRuntimeAppState,
) -> Result<Option<AdmissionPermit>, ExecutionRuntimeAppError> {
    match state.try_acquire_request_permit().await {
        Ok(permit) => Ok(permit),
        Err(RequestAdmissionError::Local(ConcurrencyError::Saturated { gate, limit }))
        | Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::Saturated {
            gate,
            limit,
        }))
        | Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::Unavailable {
            gate,
            limit,
            ..
        })) => Err(ExecutionRuntimeAppError(
            ExecutionRuntimeServerError::Overloaded { gate, limit },
        )),
        Err(RequestAdmissionError::Local(ConcurrencyError::Closed { gate })) => Err(
            ExecutionRuntimeAppError(ExecutionRuntimeServerError::RequestRead(format!(
                "execution runtime request concurrency gate {gate} is closed"
            ))),
        ),
        Err(RequestAdmissionError::Distributed(RuntimeSemaphoreError::InvalidConfiguration(
            message,
        ))) => Err(ExecutionRuntimeAppError(
            ExecutionRuntimeServerError::RequestRead(message),
        )),
    }
}

#[derive(Debug)]
enum RequestAdmissionError {
    Local(ConcurrencyError),
    Distributed(RuntimeSemaphoreError),
}

async fn parse_request_json<T>(request: Request) -> Result<T, ExecutionRuntimeAppError>
where
    T: serde::de::DeserializeOwned,
{
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|err| {
            ExecutionRuntimeAppError(ExecutionRuntimeServerError::RequestRead(err.to_string()))
        })?;
    serde_json::from_slice(&body).map_err(|err| {
        ExecutionRuntimeAppError(ExecutionRuntimeServerError::InvalidRequestJson(err))
    })
}

fn build_overloaded_response(message: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": {
                "type": "overloaded",
                "message": message,
            }
        })),
    )
        .into_response()
}

#[derive(Debug, Error)]
enum ExecutionRuntimeServerError {
    #[error("failed to read execution runtime request body: {0}")]
    RequestRead(String),
    #[error("execution runtime request body is not valid JSON: {0}")]
    InvalidRequestJson(serde_json::Error),
    #[error("execution runtime overloaded: gate {gate} saturated at {limit}")]
    Overloaded { gate: &'static str, limit: usize },
    #[error(transparent)]
    Transport(#[from] ExecutionRuntimeTransportError),
}

#[derive(Debug)]
struct ExecutionRuntimeAppError(ExecutionRuntimeServerError);

impl IntoResponse for ExecutionRuntimeAppError {
    fn into_response(self) -> Response {
        let status_code = match self.0 {
            ExecutionRuntimeServerError::RequestRead(_)
            | ExecutionRuntimeServerError::InvalidRequestJson(_) => StatusCode::BAD_REQUEST,
            ExecutionRuntimeServerError::Overloaded { .. } => {
                return build_overloaded_response(&self.0.to_string());
            }
            ExecutionRuntimeServerError::Transport(
                ExecutionRuntimeTransportError::StreamUnsupported
                | ExecutionRuntimeTransportError::RequestBodyRequired
                | ExecutionRuntimeTransportError::BodyDecode(_)
                | ExecutionRuntimeTransportError::UnsupportedContentEncoding(_)
                | ExecutionRuntimeTransportError::ProxyUnsupported
                | ExecutionRuntimeTransportError::InvalidMethod(_)
                | ExecutionRuntimeTransportError::InvalidHeaderName(_)
                | ExecutionRuntimeTransportError::InvalidHeaderValue(_)
                | ExecutionRuntimeTransportError::InvalidProxy(_)
                | ExecutionRuntimeTransportError::UnsupportedTransportProfile(_)
                | ExecutionRuntimeTransportError::BodyEncode(_),
            ) => StatusCode::BAD_REQUEST,
            ExecutionRuntimeServerError::Transport(
                ExecutionRuntimeTransportError::ClientBuild(_)
                | ExecutionRuntimeTransportError::UpstreamRequest(_)
                | ExecutionRuntimeTransportError::RelayError(_)
                | ExecutionRuntimeTransportError::InvalidJson(_),
            ) => StatusCode::BAD_GATEWAY,
        };

        (
            status_code,
            Json(json!({
                "error": self.0.to_string(),
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_execution_runtime_router_with_request_concurrency_limit,
        build_execution_runtime_router_with_request_gates, DISTRIBUTED_REQUEST_GATE_NAME,
    };
    use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
    use aether_runtime_state::{
        MemoryRuntimeStateConfig, RuntimeSemaphore, RuntimeSemaphoreConfig, RuntimeState,
    };
    use axum::body::{Body, Bytes};
    use axum::response::Response;
    use axum::routing::any;
    use axum::{extract::Request, Router};
    use http::StatusCode;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn distributed_gate(gate: &'static str, limit: usize) -> RuntimeSemaphore {
        RuntimeState::memory(MemoryRuntimeStateConfig::default())
            .semaphore(gate, limit, RuntimeSemaphoreConfig::default())
            .expect("distributed semaphore")
    }

    async fn start_server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server should run");
        });
        (format!("http://{addr}"), handle)
    }

    fn stream_plan(url: String) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".into(),
            candidate_id: Some("cand-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "GET".into(),
            url,
            headers: std::collections::BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-4.1".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(30_000),
                ..ExecutionTimeouts::default()
            }),
        }
    }

    #[tokio::test]
    async fn execution_runtime_rejects_second_in_flight_stream_request_with_overload() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let upstream_hits_clone = Arc::clone(&upstream_hits);
        let upstream = Router::new().route(
            "/slow",
            any(move |_request: Request| {
                let upstream_hits = Arc::clone(&upstream_hits_clone);
                async move {
                    upstream_hits.fetch_add(1, Ordering::SeqCst);
                    let stream = async_stream::stream! {
                        yield Ok::<_, Infallible>(Bytes::from_static(b"chunk-1"));
                        futures_util::future::pending::<()>().await;
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from_stream(stream))
                        .expect("response should build")
                }
            }),
        );
        let (upstream_url, upstream_handle) = start_server(upstream).await;
        let runtime = build_execution_runtime_router_with_request_concurrency_limit(Some(1));
        let (runtime_url, runtime_handle) = start_server(runtime).await;

        let client = reqwest::Client::new();
        let first_response = client
            .post(format!("{runtime_url}/v1/execute/stream"))
            .json(&stream_plan(format!("{upstream_url}/slow")))
            .send()
            .await
            .expect("first request should succeed");

        for _ in 0..50 {
            if upstream_hits.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

        let second_response = client
            .post(format!("{runtime_url}/v1/execute/stream"))
            .json(&stream_plan(format!("{upstream_url}/slow")))
            .send()
            .await
            .expect("second request should complete");

        assert_eq!(second_response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            second_response
                .json::<serde_json::Value>()
                .await
                .expect("json body should decode")["error"]["type"],
            "overloaded"
        );
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

        drop(first_response);
        runtime_handle.abort();
        upstream_handle.abort();
    }

    #[tokio::test]
    async fn execution_runtime_rejects_second_in_flight_stream_request_with_distributed_overload() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let upstream_hits_clone = Arc::clone(&upstream_hits);
        let upstream = Router::new().route(
            "/slow",
            any(move |_request: Request| {
                let upstream_hits = Arc::clone(&upstream_hits_clone);
                async move {
                    upstream_hits.fetch_add(1, Ordering::SeqCst);
                    let stream = async_stream::stream! {
                        yield Ok::<_, Infallible>(Bytes::from_static(b"chunk-1"));
                        futures_util::future::pending::<()>().await;
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from_stream(stream))
                        .expect("response should build")
                }
            }),
        );
        let (upstream_url, upstream_handle) = start_server(upstream).await;
        let distributed_gate = distributed_gate(DISTRIBUTED_REQUEST_GATE_NAME, 1);
        let runtime_a =
            build_execution_runtime_router_with_request_gates(None, Some(distributed_gate.clone()));
        let runtime_b =
            build_execution_runtime_router_with_request_gates(None, Some(distributed_gate));
        let (runtime_a_url, runtime_a_handle) = start_server(runtime_a).await;
        let (runtime_b_url, runtime_b_handle) = start_server(runtime_b).await;

        let client = reqwest::Client::new();
        let first_response = client
            .post(format!("{runtime_a_url}/v1/execute/stream"))
            .json(&stream_plan(format!("{upstream_url}/slow")))
            .send()
            .await
            .expect("first request should succeed");

        for _ in 0..50 {
            if upstream_hits.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

        let second_response = client
            .post(format!("{runtime_b_url}/v1/execute/stream"))
            .json(&stream_plan(format!("{upstream_url}/slow")))
            .send()
            .await
            .expect("second request should complete");

        assert_eq!(second_response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            second_response
                .json::<serde_json::Value>()
                .await
                .expect("json body should decode")["error"]["type"],
            "overloaded"
        );
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

        drop(first_response);
        runtime_a_handle.abort();
        runtime_b_handle.abort();
        upstream_handle.abort();
    }

    #[tokio::test]
    async fn execution_runtime_exposes_request_concurrency_metrics() {
        let runtime = build_execution_runtime_router_with_request_gates(
            Some(4),
            Some(distributed_gate(DISTRIBUTED_REQUEST_GATE_NAME, 6)),
        );
        let (runtime_url, runtime_handle) = start_server(runtime).await;

        let response = reqwest::Client::new()
            .get(format!("{runtime_url}/metrics"))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        let body = response.text().await.expect("body should read");
        assert!(body.contains("service_up{service=\"aether-gateway-execution-runtime\"} 1"));
        assert!(
            body.contains("concurrency_available_permits{gate=\"execution_runtime_requests\"} 4")
        );
        assert!(body.contains(
            "concurrency_available_permits{gate=\"execution_runtime_requests_distributed\"} 6"
        ));

        runtime_handle.abort();
    }
}
