use super::{
    hash_api_key, sample_models_candidate_row, unrestricted_models_snapshot,
    InMemoryAuthApiKeySnapshotRepository, InMemoryMinimalCandidateSelectionReadRepository,
    InMemoryVideoTaskRepository, UpsertVideoTask, VideoTaskLookupKey, VideoTaskReadRepository,
    VideoTaskStatus, VideoTaskWriteRepository, DEVELOPMENT_ENCRYPTION_KEY,
};
use crate::tests::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    to_bytes, AppState, Arc, Body, Json, Mutex, Request, Router, StatusCode, EXECUTION_PATH_HEADER,
    EXECUTION_PATH_LOCAL_AI_PUBLIC,
};
use axum::response::IntoResponse;

fn gemini_operation_status_label(status: VideoTaskStatus) -> &'static str {
    match status {
        VideoTaskStatus::Pending => "Pending",
        VideoTaskStatus::Submitted => "Submitted",
        VideoTaskStatus::Queued => "Queued",
        VideoTaskStatus::Processing => "Processing",
        VideoTaskStatus::Completed => "Completed",
        VideoTaskStatus::Failed => "Failed",
        VideoTaskStatus::Cancelled => "Cancelled",
        VideoTaskStatus::Expired => "Expired",
        VideoTaskStatus::Deleted => "Deleted",
    }
}

fn sample_gemini_video_task(
    id: &str,
    short_id: &str,
    user_id: &str,
    api_key_id: &str,
    external_task_id: &str,
    status: VideoTaskStatus,
) -> UpsertVideoTask {
    let completed = matches!(status, VideoTaskStatus::Completed);
    UpsertVideoTask {
        id: id.to_string(),
        short_id: Some(short_id.to_string()),
        request_id: format!("request-{id}"),
        user_id: Some(user_id.to_string()),
        api_key_id: Some(api_key_id.to_string()),
        username: Some(format!("user-{user_id}")),
        api_key_name: Some("video-key".to_string()),
        external_task_id: Some(external_task_id.to_string()),
        provider_id: Some("provider-gemini-video-local-1".to_string()),
        endpoint_id: Some("endpoint-gemini-video-local-1".to_string()),
        key_id: Some("key-gemini-video-local-1".to_string()),
        client_api_format: Some("gemini:video".to_string()),
        provider_api_format: Some("gemini:video".to_string()),
        format_converted: false,
        model: Some("veo-3".to_string()),
        prompt: Some("gemini video prompt".to_string()),
        original_request_body: Some(json!({"prompt": "gemini video prompt"})),
        duration_seconds: Some(8),
        resolution: Some("720p".to_string()),
        aspect_ratio: Some("16:9".to_string()),
        size: Some("720p".to_string()),
        status,
        progress_percent: if completed { 100 } else { 50 },
        progress_message: None,
        retry_count: 0,
        poll_interval_seconds: 10,
        next_poll_at_unix_secs: (!completed).then_some(124),
        poll_count: 0,
        max_poll_count: 360,
        created_at_unix_ms: 123,
        submitted_at_unix_secs: Some(123),
        completed_at_unix_secs: completed.then_some(124),
        updated_at_unix_secs: 124,
        error_code: None,
        error_message: None,
        video_url: None,
        request_metadata: Some(json!({
            "rust_local_snapshot": {
                "Gemini": {
                    "local_short_id": short_id,
                    "upstream_operation_name": external_task_id,
                    "user_id": user_id,
                    "api_key_id": api_key_id,
                    "model": "veo-3",
                    "status": gemini_operation_status_label(status),
                    "progress_percent": if completed { 100 } else { 50 },
                    "error_code": null,
                    "error_message": null,
                    "metadata": {},
                    "persistence": {
                        "request_id": format!("request-{id}"),
                        "username": format!("user-{user_id}"),
                        "api_key_name": "video-key",
                        "client_api_format": "gemini:video",
                        "provider_api_format": "gemini:video",
                        "original_request_body": {
                            "prompt": "gemini video prompt"
                        },
                        "format_converted": false
                    },
                    "transport": {
                        "upstream_base_url": "https://generativelanguage.googleapis.com",
                        "provider_name": "gemini-video",
                        "provider_id": "provider-gemini-video-local-1",
                        "endpoint_id": "endpoint-gemini-video-local-1",
                        "key_id": "key-gemini-video-local-1",
                        "headers": {
                            "x-goog-api-key": "sk-upstream-gemini-video",
                            "content-type": "application/json"
                        },
                        "content_type": "application/json",
                        "model_name": "veo-3-upstream",
                        "proxy": null,
                        "tls_profile": null,
                        "timeouts": null
                    }
                }
            }
        })),
    }
}

#[tokio::test]
async fn gateway_handles_public_openai_models_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-openai-models")),
        unrestricted_models_snapshot("key-1", "user-1"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row("provider-openai", "openai", "openai:chat", "gpt-5", 10),
            sample_models_candidate_row("provider-openai", "openai", "openai:chat", "gpt-4.1", 10),
        ]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                    candidate_repository,
                    auth_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/v1/models"))
        .header("authorization", "Bearer sk-openai-models")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["object"], "list");
    assert_eq!(payload["data"][0]["id"], "gpt-4.1");
    assert_eq!(payload["data"][1]["id"], "gpt-5");
    assert_eq!(payload["data"][0]["owned_by"], "openai");
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_handles_public_openai_models_with_cross_format_candidates_without_hitting_fallback_probe(
) {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-openai-models-cross-format")),
        unrestricted_models_snapshot("key-1", "user-1"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-claude",
                "claude",
                "claude:chat",
                "claude-3-7-sonnet",
                10,
            ),
        ]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                    candidate_repository,
                    auth_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let list_response = client
        .get(format!("{gateway_url}/v1/models"))
        .header("authorization", "Bearer sk-openai-models-cross-format")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert_eq!(list_payload["object"], "list");
    assert_eq!(list_payload["data"][0]["id"], "claude-3-7-sonnet");
    assert_eq!(list_payload["data"][0]["owned_by"], "claude");

    let detail_response = client
        .get(format!("{gateway_url}/v1/models/claude-3-7-sonnet"))
        .header("authorization", "Bearer sk-openai-models-cross-format")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail_payload: serde_json::Value = detail_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(detail_payload["id"], "claude-3-7-sonnet");
    assert_eq!(detail_payload["owned_by"], "claude");

    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_handles_public_claude_models_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-claude-models")),
        unrestricted_models_snapshot("key-claude", "user-claude"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-claude",
                "claude",
                "claude:chat",
                "claude-3-7-sonnet",
                10,
            ),
            sample_models_candidate_row(
                "provider-claude",
                "claude",
                "claude:chat",
                "claude-3-5-haiku",
                10,
            ),
        ]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                    candidate_repository,
                    auth_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/v1/models?limit=1"))
        .header("x-api-key", "sk-claude-models")
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["data"][0]["id"], "claude-3-5-haiku");
    assert_eq!(payload["first_id"], "claude-3-5-haiku");
    assert_eq!(payload["last_id"], "claude-3-5-haiku");
    assert_eq!(payload["has_more"], true);
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_handles_public_gemini_models_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-gemini-models")),
        unrestricted_models_snapshot("key-gemini", "user-gemini"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-gemini",
                "gemini",
                "gemini:chat",
                "gemini-2.5-flash",
                10,
            ),
            sample_models_candidate_row(
                "provider-gemini",
                "gemini",
                "gemini:chat",
                "gemini-2.5-pro",
                10,
            ),
        ]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                    candidate_repository,
                    auth_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/v1beta/models?pageSize=1&key=sk-gemini-models"
        ))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["models"][0]["name"], "models/gemini-2.5-flash");
    assert_eq!(payload["nextPageToken"], "1");
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_handles_claude_count_tokens_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-claude-count")),
        unrestricted_models_snapshot("key-claude-count", "user-claude-count"),
    )]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(auth_repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages/count_tokens"))
        .header("x-api-key", "sk-claude-count")
        .header("anthropic-version", "2023-06-01")
        .body(
            serde_json::to_vec(&json!({
                "model": "claude-sonnet-4-5",
                "system": [{"type": "text", "text": "abcdefghijklmnop"}],
                "messages": [
                    {
                        "role": "user",
                        "content": "abcdefghijkl"
                    },
                    {
                        "role": "assistant",
                        "content": [
                            {"type": "text", "text": "abcdefgh"},
                            {"type": "tool_use", "name": "ignored", "input": {"city": "SF"}}
                        ]
                    }
                ]
            }))
            .expect("request body should encode"),
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AI_PUBLIC)
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["input_tokens"], 17);
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_invalid_claude_count_tokens_payload_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-claude-count-invalid")),
        unrestricted_models_snapshot("key-claude-count-invalid", "user-claude-count-invalid"),
    )]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(auth_repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages/count_tokens"))
        .header("x-api-key", "sk-claude-count-invalid")
        .body(
            serde_json::to_vec(&json!({
                "model": "claude-sonnet-4-5",
                "messages": [{"role": "system", "content": "bad"}]
            }))
            .expect("request body should encode"),
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AI_PUBLIC)
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "Invalid token count payload");
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_gpt_image_2_on_chat_completions_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-openai-chat-image-model")),
        unrestricted_models_snapshot(
            "key-openai-chat-image-model",
            "user-openai-chat-image-model",
        ),
    )]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(auth_repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header("authorization", "Bearer sk-openai-chat-image-model")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_vec(&json!({
                "model": "gpt-image-2",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .expect("request body should encode"),
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AI_PUBLIC)
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        "gpt-image-2 仅支持通过 /v1/images/generations 或 /v1/images/edits 调用"
    );
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_image_request_with_n_greater_than_one_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-openai-image-n")),
        unrestricted_models_snapshot("key-openai-image-n", "user-openai-image-n"),
    )]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(auth_repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .header("authorization", "Bearer sk-openai-image-n")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_vec(&json!({
                "model": "gpt-image-2",
                "prompt": "draw",
                "n": 2,
                "response_format": "b64_json"
            }))
            .expect("request body should encode"),
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AI_PUBLIC)
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "图片接口当前仅支持 n=1");
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_handles_gemini_operation_detail_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-gemini-operation-detail")),
        unrestricted_models_snapshot(
            "key-gemini-operation-detail",
            "user-gemini-operation-detail",
        ),
    )]));
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_gemini_video_task(
            "task-gemini-operation-detail",
            "opshort123",
            "user-gemini-operation-detail",
            "key-gemini-operation-detail",
            "operations/ext-op-123",
            VideoTaskStatus::Completed,
        ))
        .await
        .expect("upsert should succeed");

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_video_task_repository_for_tests(
                    auth_repository,
                    repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/v1beta/operations/opshort123?key=sk-gemini-operation-detail"
        ))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AI_PUBLIC)
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["name"], "models/veo-3/operations/opshort123");
    assert_eq!(payload["done"], true);
    assert_eq!(
        payload["response"]["generateVideoResponse"]["generatedSamples"][0]["video"]["uri"],
        "/v1beta/files/aev_opshort123:download?alt=media"
    );
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_lists_gemini_operations_without_hitting_fallback_probe() {
    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-gemini-operation-list")),
        unrestricted_models_snapshot("key-gemini-operation-list", "user-gemini-operation-list"),
    )]));
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_gemini_video_task(
            "task-gemini-operation-list-1",
            "opshort-list-1",
            "user-gemini-operation-list",
            "key-gemini-operation-list",
            "operations/ext-list-1",
            VideoTaskStatus::Completed,
        ))
        .await
        .expect("upsert should succeed");
    repository
        .upsert(sample_gemini_video_task(
            "task-gemini-operation-list-2",
            "opshort-list-2",
            "user-gemini-operation-list",
            "key-gemini-operation-list",
            "operations/ext-list-2",
            VideoTaskStatus::Processing,
        ))
        .await
        .expect("upsert should succeed");
    repository
        .upsert(sample_gemini_video_task(
            "task-gemini-operation-list-other",
            "opshort-list-other",
            "user-other",
            "key-other",
            "operations/ext-list-other",
            VideoTaskStatus::Completed,
        ))
        .await
        .expect("upsert should succeed");

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_video_task_repository_for_tests(
                    auth_repository,
                    repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/v1beta/operations?key=sk-gemini-operation-list"
        ))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AI_PUBLIC)
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let operations = payload["operations"]
        .as_array()
        .expect("operations should be an array");
    assert_eq!(operations.len(), 2);
    let operation_names = operations
        .iter()
        .map(|value| {
            value["name"]
                .as_str()
                .expect("operation name should be a string")
                .to_string()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        operation_names,
        std::collections::BTreeSet::from([
            "models/veo-3/operations/opshort-list-1".to_string(),
            "models/veo-3/operations/opshort-list-2".to_string(),
        ])
    );
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_cancels_gemini_operation_without_hitting_fallback_probe() {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SeenExecutionRuntimeSyncRequest {
        method: String,
        url: String,
        api_key: String,
    }

    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Json(json!({"proxied": true}))).into_response()
            }
        }),
    );

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeSyncRequest {
                    method: payload
                        .get("method")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    url: payload
                        .get("url")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    api_key: payload
                        .get("headers")
                        .and_then(|value| value.get("x-goog-api-key"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
                Json(json!({
                    "request_id": "trace-gemini-operation-cancel",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {}
                    },
                    "telemetry": {
                        "elapsed_ms": 12
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-gemini-operation-cancel")),
        unrestricted_models_snapshot(
            "key-gemini-operation-cancel",
            "user-gemini-operation-cancel",
        ),
    )]));
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_gemini_video_task(
            "task-gemini-operation-cancel",
            "opshort-cancel",
            "user-gemini-operation-cancel",
            "key-gemini-operation-cancel",
            "operations/ext-op-123",
            VideoTaskStatus::Submitted,
        ))
        .await
        .expect("upsert should succeed");

    let (fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_video_task_repository_for_tests(
                    auth_repository,
                    Arc::clone(&repository),
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/v1beta/operations/opshort-cancel:cancel"
        ))
        .header("x-goog-api-key", "sk-gemini-operation-cancel")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AI_PUBLIC)
    );
    assert_eq!(
        response
            .json::<serde_json::Value>()
            .await
            .expect("json body should parse"),
        json!({})
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(seen_execution_runtime_request.method, "POST");
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://generativelanguage.googleapis.com/v1beta/models/veo-3/operations/ext-op-123:cancel"
    );
    assert_eq!(
        seen_execution_runtime_request.api_key,
        "sk-upstream-gemini-video"
    );

    let stored = repository
        .find(VideoTaskLookupKey::Id("task-gemini-operation-cancel"))
        .await
        .expect("task lookup should succeed")
        .expect("task should exist");
    assert_eq!(stored.status, VideoTaskStatus::Cancelled);
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    fallback_probe_handle.abort();
}
