use super::*;
use crate::GatewayError;
use axum::{body::Body, routing::any, Json, Router};
use http::{Response, StatusCode};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;

const KEY: &str = "key-openai-chat-pressure-0";

fn queue_state(base_url: String, timeout: Duration, capacity: usize) -> AppState {
    queue_state_with_targets(vec![base_url], timeout, capacity)
}

fn queue_state_with_targets(targets: Vec<String>, timeout: Duration, capacity: usize) -> AppState {
    let config = OpenAiChatPressureStateConfig::new(targets);
    let auth = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(&config.client_api_key)),
        openai_chat_pressure_auth_snapshot(&config),
    )]));
    let mut keys = openai_chat_pressure_keys(&config).unwrap();
    for key in &mut keys {
        key.concurrent_limit = Some(1);
    }
    let mut candidates = openai_chat_pressure_candidates(&config);
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.key_internal_priority = 5 + i32::try_from(index).unwrap();
    }
    let catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![openai_chat_pressure_provider()],
        openai_chat_pressure_endpoints(&config),
        keys,
    ));
    let data = GatewayDataState::with_openai_chat_pressure_repositories_for_testkit(
        auth,
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(
            candidates,
        )),
        catalog,
        Arc::new(InMemoryRequestCandidateRepository::default()),
        Arc::new(InMemoryUsageReadRepository::default()),
        DEVELOPMENT_ENCRYPTION_KEY,
    );
    let mut state = AppState::new()
        .unwrap()
        .with_data_state_for_tests(data)
        .with_usage_runtime_for_tests(crate::usage::UsageRuntimeConfig {
            enabled: true,
            lifecycle_enqueue_delay_ms: 0,
            ..Default::default()
        });
    state.provider_concurrency_queue = Arc::new(
        crate::provider_concurrency_queue::ProviderConcurrencyQueue::new(timeout, capacity),
    );
    state.upstream_execution_gate = Some(Arc::new(aether_runtime::ConcurrencyGate::new(
        "gateway_upstream_execution",
        2,
    )));
    state
}

async fn wait_until_queued(state: &AppState) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !state.provider_concurrency_queue.is_waiting(KEY) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("request must reach provider queue");
}

async fn request(url: String, stream: bool) -> (StatusCode, String) {
    let response = reqwest::Client::new().post(format!("{url}/v1/chat/completions"))
        .bearer_auth("sk-aether-openai-chat-pressure")
        .json(&serde_json::json!({"model": "gpt-5", "messages": [{"role": "user", "content": "hello"}], "stream": stream}))
        .send().await.unwrap();
    (response.status(), response.text().await.unwrap())
}

async fn usage_api(url: &str, route: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .get(format!("{url}/api/admin/usage/{route}"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(
            crate::constants::TRUSTED_ADMIN_USER_ID_HEADER,
            "admin-user-123",
        )
        .header(crate::constants::TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(
            crate::constants::TRUSTED_ADMIN_SESSION_ID_HEADER,
            "session-123",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn queued_usage(url: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let payload = usage_api(url, "active").await;
            if let Some(row) = payload["requests"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["is_queued"] == true)
            {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("queued request must be visible through HTTP usage API")
}

async fn terminal_usage(url: &str, usage_id: &str, status: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let payload = usage_api(url, &format!("active?ids={usage_id}")).await;
            let row = &payload["requests"][0];
            assert_eq!(row["is_queued"], false, "{payload}");
            if row["status"] == status {
                return row.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("queued request must reach expected terminal usage status")
}

fn response(stream: bool) -> Response<Body> {
    if stream {
        Response::builder().header("content-type", "text/event-stream").body(Body::from(
            "data: {\"id\":\"chatcmpl-queue\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5-upstream\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"queued\"},\"finish_reason\":null}]}\n\ndata: {\"id\":\"chatcmpl-queue\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5-upstream\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        )).unwrap()
    } else {
        Response::builder().header("content-type", "application/json").body(Body::from(serde_json::json!({
            "id": "chatcmpl-queue", "object": "chat.completion", "model": "gpt-5-upstream",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "queued"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }).to_string())).unwrap()
    }
}

#[test]
fn provider_concurrency_queue_http_sync_and_stream_resume_after_release() {
    crate::tests::run_async_test_on_large_stack(
        "provider-queue-http",
        16 * 1024 * 1024,
        || async {
            for stream in [false, true] {
                let entered = Arc::new(Semaphore::new(0));
                let release = Arc::new(Semaphore::new(0));
                let hits = Arc::new(AtomicUsize::new(0));
                let app = Router::new().route(
                    "/chat/completions",
                    any({
                        let entered = entered.clone();
                        let release = release.clone();
                        let hits = hits.clone();
                        move || {
                            let entered = entered.clone();
                            let release = release.clone();
                            let hits = hits.clone();
                            async move {
                                hits.fetch_add(1, Ordering::SeqCst);
                                entered.add_permits(1);
                                if stream {
                                    let body = async_stream::stream! {
                                        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"data: {\"id\":\"chatcmpl-queue\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5-upstream\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"queued\"},\"finish_reason\":null}]}\n\n"));
                                        release.acquire().await.unwrap().forget();
                                        yield Ok(bytes::Bytes::from_static(b"data: {\"id\":\"chatcmpl-queue\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5-upstream\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"));
                                    };
                                    return Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(body)).unwrap();
                                }
                                release.acquire().await.unwrap().forget();
                                response(stream)
                            }
                        }
                    }),
                );
                let (provider_url, provider) = crate::tests::start_server(app).await;
                let state = queue_state(provider_url, Duration::from_secs(3), 2);
                let (url, gateway) =
                    crate::tests::start_server(crate::build_router_with_state(state.clone())).await;
                let first = tokio::spawn(request(url.clone(), stream));
                tokio::time::timeout(Duration::from_secs(3), entered.acquire())
                    .await
                    .unwrap()
                    .unwrap()
                    .forget();
                let second = tokio::spawn(request(url.clone(), stream));
                wait_until_queued(&state).await;
                let queued = queued_usage(&url).await;
                assert_eq!(queued["status"], "pending");
                let records = usage_api(&url, "records").await;
                assert!(
                    records["records"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|row| row["id"] == queued["id"] && row["is_queued"] == true),
                    "{records}"
                );
                assert_eq!(hits.load(Ordering::SeqCst), 1);
                assert!(
                    state
                        .upstream_execution_gate
                        .as_ref()
                        .unwrap()
                        .snapshot()
                        .in_flight
                        <= 1
                );
                release.add_permits(2);
                for task in [first, second] {
                    let (status, body) = tokio::time::timeout(Duration::from_secs(5), task)
                        .await
                        .unwrap()
                        .unwrap();
                    assert_eq!(status, StatusCode::OK, "{body}");
                    assert!(body.contains("queued"), "{body}");
                }
                assert_eq!(hits.load(Ordering::SeqCst), 2);
                let request_id = queued["id"].as_str().unwrap();
                terminal_usage(&url, request_id, "completed").await;
                gateway.abort();
                provider.abort();
            }
        },
    );
}

#[test]
fn provider_concurrency_queue_key_opt_out_http_rejects_without_queue_status() {
    crate::tests::run_async_test_on_large_stack(
        "provider-queue-key-opt-out",
        16 * 1024 * 1024,
        || async {
            let hits = Arc::new(AtomicUsize::new(0));
            let (provider_url, provider) = crate::tests::start_server(Router::new().route(
                "/chat/completions",
                any({
                    let hits = hits.clone();
                    move || {
                        let hits = hits.clone();
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            response(false)
                        }
                    }
                }),
            ))
            .await;
            let state = queue_state(provider_url, Duration::from_secs(15), 2);
            let mut key = state
                .read_provider_catalog_keys_by_ids(&[KEY.to_string()])
                .await
                .unwrap()
                .remove(0);
            key.concurrency_queue_enabled = false;
            assert!(state
                .update_provider_catalog_key(&key)
                .await
                .unwrap()
                .is_some());
            let gate = state
                .runtime_state
                .keyed_semaphore(
                    "provider_key",
                    KEY,
                    1,
                    aether_runtime_state::RuntimeSemaphoreConfig::default(),
                )
                .unwrap();
            let held = gate.try_acquire().await.unwrap();
            let (url, gateway) =
                crate::tests::start_server(crate::build_router_with_state(state.clone())).await;
            for stream in [false, true] {
                let (status, body) =
                    tokio::time::timeout(Duration::from_secs(2), request(url.clone(), stream))
                        .await
                        .expect("disabled key must not wait for the 15-second queue timeout");
                assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
                assert!(!state.provider_concurrency_queue.is_waiting(KEY));
                assert_eq!(hits.load(Ordering::SeqCst), 0);
            }
            let records = usage_api(&url, "records").await;
            for row in records["records"].as_array().unwrap() {
                assert_eq!(row["is_queued"], false, "{records}");
            }
            held.release().await.unwrap();
            let (status, body) = request(url, false).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(hits.load(Ordering::SeqCst), 1);
            gateway.abort();
            provider.abort();
        },
    );
}

#[test]
fn provider_concurrency_queue_http_full_and_timeout_return_429() {
    crate::tests::run_async_test_on_large_stack(
        "provider-queue-http-429",
        16 * 1024 * 1024,
        || async {
            let (provider_url, provider) = crate::tests::start_server(Router::new().route(
                "/chat/completions",
                any(|| async { Json(serde_json::json!({})) }),
            ))
            .await;
            let state = queue_state(provider_url, Duration::from_millis(400), 1);
            let gate = state
                .runtime_state
                .keyed_semaphore(
                    "provider_key",
                    KEY,
                    1,
                    aether_runtime_state::RuntimeSemaphoreConfig::default(),
                )
                .unwrap();
            let held = gate.try_acquire().await.unwrap();
            let (url, gateway) =
                crate::tests::start_server(crate::build_router_with_state(state.clone())).await;
            let waiting = tokio::spawn(request(url.clone(), false));
            wait_until_queued(&state).await;
            let queued = queued_usage(&url).await;
            assert_eq!(
                state
                    .upstream_execution_gate
                    .as_ref()
                    .unwrap()
                    .snapshot()
                    .in_flight,
                0
            );
            let (status, body) = request(url.clone(), false).await;
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
            let (status, body) = waiting.await.unwrap();
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
            let request_id = queued["id"].as_str().unwrap();
            terminal_usage(&url, request_id, "failed").await;
            assert_eq!(gate.snapshot().await.unwrap().in_flight, 1);
            held.release().await.unwrap();
            gateway.abort();
            provider.abort();
        },
    );
}

#[test]
fn provider_concurrency_queue_rechecks_disabled_key_before_upstream() {
    crate::tests::run_async_test_on_large_stack(
        "provider-queue-disabled",
        16 * 1024 * 1024,
        || async {
            let hits = Arc::new(AtomicUsize::new(0));
            let (provider_url, provider) = crate::tests::start_server(Router::new().route(
                "/chat/completions",
                any({
                    let hits = hits.clone();
                    move || {
                        let hits = hits.clone();
                        async move {
                            hits.fetch_add(1, Ordering::SeqCst);
                            response(false)
                        }
                    }
                }),
            ))
            .await;
            let state = queue_state(provider_url, Duration::from_secs(3), 1);
            let gate = state
                .runtime_state
                .keyed_semaphore(
                    "provider_key",
                    KEY,
                    1,
                    aether_runtime_state::RuntimeSemaphoreConfig::default(),
                )
                .unwrap();
            let held = gate.try_acquire().await.unwrap();
            let (url, gateway) =
                crate::tests::start_server(crate::build_router_with_state(state.clone())).await;
            let waiting = tokio::spawn(request(url.clone(), false));
            wait_until_queued(&state).await;
            let queued = queued_usage(&url).await;
            let mut key = state
                .read_provider_catalog_keys_by_ids(&[KEY.to_string()])
                .await
                .unwrap()
                .remove(0);
            key.is_active = false;
            assert!(state
                .update_provider_catalog_key(&key)
                .await
                .unwrap()
                .is_some());
            held.release().await.unwrap();
            let (status, body) = waiting.await.unwrap();
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
            assert_eq!(hits.load(Ordering::SeqCst), 0);
            terminal_usage(&url, queued["id"].as_str().unwrap(), "failed").await;
            assert_eq!(gate.snapshot().await.unwrap().in_flight, 0);
            gateway.abort();
            provider.abort();
        },
    );
}

#[test]
fn provider_concurrency_queue_timeout_can_fall_back_to_successful_usage() {
    crate::tests::run_async_test_on_large_stack(
        "provider-queue-fallback-usage",
        16 * 1024 * 1024,
        || async {
            for stream in [false, true] {
                let hits = Arc::new(AtomicUsize::new(0));
                let (provider_url, provider) = crate::tests::start_server(Router::new().route(
                    "/chat/completions",
                    any({
                        let hits = hits.clone();
                        move || {
                            let hits = hits.clone();
                            async move {
                                hits.fetch_add(1, Ordering::SeqCst);
                                response(stream)
                            }
                        }
                    }),
                ))
                .await;
                let state = queue_state_with_targets(
                    vec![provider_url.clone(), provider_url],
                    Duration::from_secs(1),
                    1,
                );
                let gate = state
                    .runtime_state
                    .keyed_semaphore(
                        "provider_key",
                        KEY,
                        1,
                        aether_runtime_state::RuntimeSemaphoreConfig::default(),
                    )
                    .unwrap();
                let held = gate.try_acquire().await.unwrap();
                let fallback_gate = state
                    .runtime_state
                    .keyed_semaphore(
                        "provider_key",
                        "key-openai-chat-pressure-1",
                        1,
                        aether_runtime_state::RuntimeSemaphoreConfig::default(),
                    )
                    .unwrap();
                let fallback_held = fallback_gate.try_acquire().await.unwrap();
                let (url, gateway) =
                    crate::tests::start_server(crate::build_router_with_state(state.clone())).await;
                let waiting = tokio::spawn(request(url.clone(), stream));
                let queued = queued_usage(&url).await;
                assert_eq!(queued["status"], "pending");
                assert_eq!(hits.load(Ordering::SeqCst), 0);
                fallback_held.release().await.unwrap();
                let (status, body) = waiting.await.unwrap();
                assert_eq!(status, StatusCode::OK, "{body}");
                assert_eq!(hits.load(Ordering::SeqCst), 1);
                terminal_usage(&url, queued["id"].as_str().unwrap(), "completed").await;
                let records = usage_api(&url, "records").await;
                let record = records["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["id"] == queued["id"])
                    .unwrap();
                assert_eq!(record["status"], "completed", "{record}");
                assert_eq!(record["is_queued"], false, "{record}");
                held.release().await.unwrap();
                gateway.abort();
                provider.abort();
            }
        },
    );
}

#[test]
fn provider_concurrency_queue_cancelled_and_failed_admission_are_terminal_in_http_usage() {
    crate::tests::run_async_test_on_large_stack(
        "provider-queue-cancelled-usage",
        16 * 1024 * 1024,
        || async {
            for (after_admission, fail_after_admission) in
                [(false, false), (true, false), (true, true)]
            {
                let state =
                    queue_state("http://127.0.0.1:1".to_string(), Duration::from_secs(5), 1);
                let gate = state
                    .runtime_state
                    .keyed_semaphore(
                        "provider_key",
                        KEY,
                        1,
                        aether_runtime_state::RuntimeSemaphoreConfig::default(),
                    )
                    .unwrap();
                let mut held = Some(gate.try_acquire().await.unwrap());
                let (url, gateway) =
                    crate::tests::start_server(crate::build_router_with_state(state.clone())).await;
                let waiter_state = state.clone();
                let (admitted, ready) = tokio::sync::oneshot::channel();
                let waiter = tokio::spawn(async move {
                    let plan = aether_contracts::ExecutionPlan {
                        request_id: "req-queue-cancelled".to_string(),
                        candidate_id: None,
                        provider_name: Some("OpenAI".to_string()),
                        provider_id: "provider-openai-chat-pressure".to_string(),
                        endpoint_id: "endpoint-openai-chat-pressure-0".to_string(),
                        key_id: KEY.to_string(),
                        method: "POST".to_string(),
                        url: "http://127.0.0.1:1/chat/completions".to_string(),
                        headers: Default::default(),
                        content_type: Some("application/json".to_string()),
                        content_encoding: None,
                        body: aether_contracts::RequestBody::from_json(
                            serde_json::json!({"model": "gpt-5"}),
                        ),
                        stream: false,
                        client_api_format: "openai:chat".to_string(),
                        provider_api_format: "openai:chat".to_string(),
                        model_name: Some("gpt-5".to_string()),
                        proxy: None,
                        transport_profile: None,
                        timeouts: None,
                    };
                    if after_admission {
                        let admission =
                            crate::provider_pool_demand::prepare_provider_execution_admission(
                                &waiter_state,
                                &plan,
                                None,
                            )
                            .await
                            .unwrap();
                        admitted.send(()).unwrap();
                        if fail_after_admission {
                            admission
                                .scope(async {
                                    Err::<(), _>(GatewayError::Internal(
                                        "admission test failure".to_string(),
                                    ))
                                })
                                .await
                        } else {
                            let _admission = admission;
                            std::future::pending().await
                        }
                    } else {
                        waiter_state
                            .provider_concurrency_queue
                            .acquire_observed(
                                &[gate],
                                KEY,
                                true,
                                Some((&waiter_state, &plan, None)),
                            )
                            .await
                            .map(|_| ())
                            .map_err(|error| GatewayError::Internal(error.to_string()))
                    }
                });
                let queued = queued_usage(&url).await;
                if after_admission {
                    held.take().unwrap().release().await.unwrap();
                    tokio::time::timeout(Duration::from_secs(3), ready)
                        .await
                        .unwrap()
                        .unwrap();
                    let payload = usage_api(
                        &url,
                        &format!("active?ids={}", queued["id"].as_str().unwrap()),
                    )
                    .await;
                    assert_eq!(payload["requests"][0]["is_queued"], false, "{payload}");
                    if !fail_after_admission {
                        assert_eq!(payload["requests"][0]["status"], "pending", "{payload}");
                    }
                }
                if fail_after_admission {
                    assert!(matches!(
                        waiter.await.unwrap(),
                        Err(GatewayError::Internal(_))
                    ));
                    terminal_usage(&url, queued["id"].as_str().unwrap(), "failed").await;
                    gateway.abort();
                    continue;
                }
                waiter.abort();
                assert!(waiter.await.unwrap_err().is_cancelled());
                let usage_id = queued["id"].as_str().unwrap();
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        let payload = usage_api(&url, &format!("active?ids={usage_id}")).await;
                        let row = &payload["requests"][0];
                        assert_eq!(row["is_queued"], false, "{payload}");
                        if row["status"] == "cancelled" {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("cancelled queued request must become terminal");
                if let Some(held) = held {
                    held.release().await.unwrap();
                }
                gateway.abort();
            }
        },
    );
}
