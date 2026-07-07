use std::sync::atomic::{AtomicUsize, Ordering};

use super::usage::{
    hash_api_key, sample_local_openai_auth_snapshot, sample_local_openai_candidate_row,
    sample_local_openai_endpoint, sample_local_openai_key, sample_local_openai_provider,
};
use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, start_server,
    wait_until, AppState, Arc, Body, Bytes, GatewayFallbackMetricKind, GatewayFallbackReason,
    HeaderValue, Infallible, Request, Response, Router, StatusCode,
    EXECUTION_PATH_DISTRIBUTED_OVERLOADED, EXECUTION_PATH_HEADER,
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS, EXECUTION_PATH_LOCAL_OVERLOADED,
};
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::auth::InMemoryAuthApiKeySnapshotRepository;
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_runtime_state::{
    MemoryRuntimeStateConfig, RuntimeSemaphore, RuntimeSemaphoreConfig, RuntimeState,
};

use crate::data::GatewayDataState;

const CONCURRENCY_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_concurrency_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(CONCURRENCY_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("concurrency test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn memory_runtime_semaphore(gate: &'static str, limit: usize) -> RuntimeSemaphore {
    RuntimeState::memory(MemoryRuntimeStateConfig::default())
        .semaphore(gate, limit, RuntimeSemaphoreConfig::default())
        .expect("memory runtime semaphore should build")
}

fn sample_decision() -> crate::control::GatewayControlDecision {
    crate::control::GatewayControlDecision {
        public_path: "/v1/chat/completions".to_string(),
        public_query_string: None,
        route_class: Some("ai_public".to_string()),
        route_family: Some("openai".to_string()),
        route_kind: Some("chat".to_string()),
        request_auth_channel: None,
        auth_endpoint_signature: None,
        execution_runtime_candidate: true,
        auth_context: None,
        admin_principal: None,
        local_auth_rejection: None,
    }
}

fn build_local_openai_gateway_state(
    execution_runtime_override_base_url: impl Into<String>,
) -> AppState {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-concurrency")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-concurrency-1",
            "user-openai-concurrency-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_local_openai_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_local_openai_provider()],
        vec![sample_local_openai_endpoint()],
        vec![sample_local_openai_key()],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    build_state_with_execution_runtime_override(execution_runtime_override_base_url)
        .with_data_state_for_tests(
            GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                request_candidate_repository,
                usage_repository,
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
}

#[test]
fn gateway_rejects_second_in_flight_stream_request_with_distributed_overload() {
    run_concurrency_test(
        "gateway_rejects_second_in_flight_stream_request_with_distributed_overload",
        gateway_rejects_second_in_flight_stream_request_with_distributed_overload_impl,
    );
}

async fn gateway_rejects_second_in_flight_stream_request_with_distributed_overload_impl() {
    let execution_runtime_hits = Arc::new(AtomicUsize::new(0));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(move |_request: Request| {
            let execution_runtime_hits = Arc::clone(&execution_runtime_hits_clone);
            async move {
                execution_runtime_hits.fetch_add(1, Ordering::SeqCst);
                let stream = async_stream::stream! {
                    yield Ok::<_, Infallible>(Bytes::from_static(
                        b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                    ));
                    yield Ok::<_, Infallible>(Bytes::from_static(
                        b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-concurrency-123\\\"}\\n\\n\"}}\n",
                    ));
                    futures_util::future::pending::<()>().await;
                };
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from_stream(stream))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-ndjson"),
                );
                response
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let distributed_gate = memory_runtime_semaphore("gateway_requests_distributed", 1);
    let gateway_a = build_router_with_state(
        build_local_openai_gateway_state(execution_runtime_url.clone())
            .with_distributed_request_concurrency_gate(distributed_gate.clone()),
    );
    let gateway_b = build_router_with_state(
        build_local_openai_gateway_state(execution_runtime_url)
            .with_distributed_request_concurrency_gate(distributed_gate),
    );
    let (gateway_a_url, gateway_a_handle) = start_server(gateway_a).await;
    let (gateway_b_url, gateway_b_handle) = start_server(gateway_b).await;

    let client = reqwest::Client::new();
    let first_response = client
        .post(format!("{gateway_a_url}/v1/chat/completions"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-concurrency",
        )
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("first request should succeed");

    wait_until(500, || execution_runtime_hits.load(Ordering::SeqCst) == 1).await;

    let second_response = client
        .post(format!("{gateway_b_url}/v1/chat/completions"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-concurrency",
        )
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("second request should complete");

    assert_eq!(second_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        second_response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_DISTRIBUTED_OVERLOADED)
    );
    assert_eq!(
        second_response
            .json::<serde_json::Value>()
            .await
            .expect("json body should decode")["error"]["details"]["gate"],
        "gateway_requests_distributed"
    );
    assert_eq!(execution_runtime_hits.load(Ordering::SeqCst), 1);

    drop(first_response);
    gateway_a_handle.abort();
    gateway_b_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_rejects_second_in_flight_stream_request_with_local_overload() {
    run_concurrency_test(
        "gateway_rejects_second_in_flight_stream_request_with_local_overload",
        gateway_rejects_second_in_flight_stream_request_with_local_overload_impl,
    );
}

async fn gateway_rejects_second_in_flight_stream_request_with_local_overload_impl() {
    let execution_runtime_hits = Arc::new(AtomicUsize::new(0));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(move |_request: Request| {
            let execution_runtime_hits = Arc::clone(&execution_runtime_hits_clone);
            async move {
                execution_runtime_hits.fetch_add(1, Ordering::SeqCst);
                let stream = async_stream::stream! {
                    yield Ok::<_, Infallible>(Bytes::from_static(
                        b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                    ));
                    yield Ok::<_, Infallible>(Bytes::from_static(
                        b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-concurrency-123\\\"}\\n\\n\"}}\n",
                    ));
                    futures_util::future::pending::<()>().await;
                };
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from_stream(stream))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-ndjson"),
                );
                response
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_local_openai_gateway_state(execution_runtime_url).with_request_concurrency_limit(1),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let first_response = client
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-concurrency",
        )
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("first request should succeed");

    wait_until(500, || execution_runtime_hits.load(Ordering::SeqCst) == 1).await;

    let second_response = client
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-concurrency",
        )
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("second request should complete");

    assert_eq!(second_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        second_response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_OVERLOADED)
    );
    assert_eq!(
        second_response
            .json::<serde_json::Value>()
            .await
            .expect("json body should decode")["error"]["type"],
        "overloaded"
    );
    assert_eq!(execution_runtime_hits.load(Ordering::SeqCst), 1);

    drop(first_response);
    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_exposes_request_concurrency_metrics() {
    run_concurrency_test(
        "gateway_exposes_request_concurrency_metrics",
        gateway_exposes_request_concurrency_metrics_impl,
    );
}

async fn gateway_exposes_request_concurrency_metrics_impl() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_request_concurrency_limit(3)
            .with_distributed_request_concurrency_gate(memory_runtime_semaphore(
                "gateway_requests_distributed",
                5,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/_gateway/metrics"))
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
    assert!(body.contains("service_up{service=\"aether-gateway\"} 1"));
    assert!(body.contains("concurrency_in_flight{gate=\"gateway_requests\"} 0"));
    assert!(body.contains("concurrency_available_permits{gate=\"gateway_requests\"} 3"));
    assert!(body.contains("concurrency_in_flight{gate=\"gateway_requests_distributed\"} 0"));
    assert!(body.contains("concurrency_available_permits{gate=\"gateway_requests_distributed\"} 5"));
    assert!(body.contains("tunnel_proxy_connections 0"));
    assert!(body.contains("tunnel_nodes 0"));
    assert!(body.contains("tunnel_active_streams 0"));
    assert!(body.contains("gateway_process_cpu_usage_basis_points "));
    assert!(body.contains("gateway_process_memory_bytes "));
    assert!(body.contains("gateway_process_threads "));
    assert!(body.contains("gateway_process_open_fds "));
    assert!(body.contains("gateway_process_fd_limit "));
    assert!(body.contains("gateway_process_socket_fds "));
    assert!(body.contains("gateway_allocator_observability_available "));
    assert!(body.contains("gateway_allocator_allocated_bytes "));
    assert!(body.contains("gateway_allocator_active_bytes "));
    assert!(body.contains("gateway_allocator_resident_bytes "));
    assert!(body.contains("gateway_allocator_active_to_allocated_basis_points "));
    assert!(body.contains("gateway_network_observability_available "));
    assert!(body.contains("gateway_network_received_bytes_total "));
    assert!(body.contains("gateway_tcp_state_observability_available "));
    assert!(body.contains("gateway_host_tcp_established_connections "));
    assert!(body.contains("gateway_process_tcp_established_connections "));
    assert!(body.contains("postgres_observability_available{driver=\"postgres\"} 0"));
    assert!(body.contains("postgres_observability_unavailable{driver=\"postgres\"} 0"));
    assert!(body.contains("postgres_lock_waiting_connections{driver=\"postgres\"} 0"));
    assert!(body.contains("postgres_oldest_active_query_age_ms{driver=\"postgres\"} 0"));
    assert!(body.contains("postgres_oldest_transaction_age_ms{driver=\"postgres\"} 0"));
    assert!(body.contains("redis_runtime_enabled{backend=\"redis\"} 0"));
    assert!(body.contains("redis_runtime_health_unavailable{backend=\"redis\"} 0"));
    assert!(body.contains("redis_runtime_connected_clients{backend=\"redis\"} 0"));
    assert!(body.contains("redis_runtime_used_memory_bytes{backend=\"redis\"} 0"));
    assert!(body.contains("usage_runtime_queue_worker_read_batches_total 0"));
    assert!(body.contains("usage_runtime_queue_worker_read_entries_total 0"));
    assert!(body.contains("usage_runtime_queue_worker_reclaimed_entries_total 0"));
    assert!(body.contains("usage_runtime_queue_worker_acked_entries_total 0"));
    assert!(body.contains("usage_runtime_queue_worker_dead_lettered_entries_total 0"));
    assert!(body.contains("usage_runtime_queue_worker_process_failures_total 0"));
    assert!(body.contains("usage_runtime_queue_worker_read_failures_total 0"));
    assert!(body.contains("usage_runtime_queue_worker_reclaim_failures_total 0"));
    assert!(body.contains("usage_queue_health_unavailable 0"));
    assert!(
        body.contains("usage_queue_enabled{stream=\"usage:events\",group=\"usage_consumers\"} 0")
    );
    assert!(body
        .contains("usage_queue_configured{stream=\"usage:events\",group=\"usage_consumers\"} 0"));
    assert!(body.contains("usage_queue_dlq_length{stream=\"usage:events:dlq\"} 0"));
    assert!(body.contains("usage_counter_health_unavailable 0"));
    assert!(body.contains("usage_counter_outbox_pending_rows 0"));
    assert!(body.contains("usage_counter_outbox_oldest_pending_age_seconds 0"));
    assert!(body.contains("usage_counter_outbox_flush_batches_total 0"));
    assert!(body.contains("usage_counter_outbox_flush_rows_claimed_total 0"));
    assert!(body.contains("usage_counter_outbox_flush_failed_batches_total 0"));
    assert!(body.contains("usage_counter_outbox_cleanup_rows_total 0"));
    assert!(body.contains("usage_counter_outbox_cleanup_failed_batches_total 0"));
    assert!(body.contains("gateway_background_tasks_active 0"));
    assert!(body.contains("gateway_background_tasks_supervised_total 0"));
    assert!(body.contains("gateway_background_tasks_unexpected_exits_total 0"));
    assert!(body.contains("gateway_background_tasks_panicked_total 0"));
    assert!(body.contains("gateway_background_tasks_aborted_total 0"));
    assert!(body.contains("gateway_tokio_runtime_observability_available 1"));
    assert!(body.contains("gateway_tokio_runtime_workers "));
    assert!(body.contains("gateway_tokio_runtime_alive_tasks "));
    assert!(body.contains("gateway_tokio_runtime_global_queue_depth "));

    gateway_handle.abort();
}

#[test]
fn gateway_exposes_fallback_metrics() {
    run_concurrency_test(
        "gateway_exposes_fallback_metrics",
        gateway_exposes_fallback_metrics_impl,
    );
}

async fn gateway_exposes_fallback_metrics_impl() {
    let state = AppState::new().expect("gateway state should build");
    let decision = sample_decision();
    state.record_fallback_metric(
        GatewayFallbackMetricKind::DecisionRemote,
        Some(&decision),
        Some("openai_chat_sync"),
        None,
        GatewayFallbackReason::LocalDecisionMiss,
    );
    state.record_fallback_metric(
        GatewayFallbackMetricKind::PlanFallback,
        Some(&decision),
        Some("openai_chat_sync"),
        None,
        GatewayFallbackReason::RemoteDecisionMiss,
    );
    state.record_fallback_metric(
        GatewayFallbackMetricKind::LocalExecutionRuntimeMiss,
        Some(&decision),
        None,
        Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS),
        GatewayFallbackReason::LocalExecutionPathRequired,
    );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/_gateway/metrics"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("body should read");
    let decision_remote = body
        .lines()
        .find(|line| line.starts_with("decision_remote_total{"))
        .expect("decision_remote_total sample should be rendered");
    assert!(decision_remote.contains("route_class=\"ai_public\""));
    assert!(decision_remote.contains("route_family=\"openai\""));
    assert!(decision_remote.contains("route_kind=\"chat\""));
    assert!(decision_remote.contains("plan_kind=\"openai_chat_sync\""));
    assert!(decision_remote.contains("execution_path=\"none\""));
    assert!(decision_remote.contains("reason=\"local_decision_miss\""));
    assert!(decision_remote.ends_with(" 1"));

    let plan_fallback = body
        .lines()
        .find(|line| line.starts_with("plan_fallback_total{"))
        .expect("plan_fallback_total sample should be rendered");
    assert!(plan_fallback.contains("route_class=\"ai_public\""));
    assert!(plan_fallback.contains("route_family=\"openai\""));
    assert!(plan_fallback.contains("route_kind=\"chat\""));
    assert!(plan_fallback.contains("plan_kind=\"openai_chat_sync\""));
    assert!(plan_fallback.contains("execution_path=\"none\""));
    assert!(plan_fallback.contains("reason=\"remote_decision_miss\""));
    assert!(plan_fallback.ends_with(" 1"));

    let local_execution_runtime_miss = body
        .lines()
        .find(|line| line.starts_with("local_execution_runtime_miss_total{"))
        .expect("local_execution_runtime_miss_total sample should be rendered");
    assert!(local_execution_runtime_miss.contains("route_class=\"ai_public\""));
    assert!(local_execution_runtime_miss.contains("route_family=\"openai\""));
    assert!(local_execution_runtime_miss.contains("route_kind=\"chat\""));
    assert!(local_execution_runtime_miss.contains("plan_kind=\"none\""));
    assert!(local_execution_runtime_miss.contains(&format!(
        "execution_path=\"{}\"",
        EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS
    )));
    assert!(local_execution_runtime_miss.contains("reason=\"local_execution_path_required\""));
    assert!(local_execution_runtime_miss.ends_with(" 1"));

    gateway_handle.abort();
}
