use super::{
    any, build_router, build_router_with_execution_runtime_override, json, start_server, Arc, Body,
    HeaderValue, Json, Mutex, Request, Response, Router, StatusCode, DEPENDENCY_REASON_HEADER,
    EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS,
    LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER, TRACE_ID_HEADER,
};

#[tokio::test]
async fn gateway_locally_denies_openai_chat_after_repeated_execution_runtime_misses_without_control_execute_opt_in(
) {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let public_execution_paths = Arc::new(Mutex::new(Vec::<String>::new()));
    let public_execution_paths_clone = Arc::clone(&public_execution_paths);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/resolve",
            any(|_request: Request| async move {
                Json(json!({
                    "action": "proxy_public",
                    "route_class": "ai_public",
                    "route_family": "openai",
                    "route_kind": "chat",
                    "auth_endpoint_signature": "openai:chat",
                    "execution_runtime_candidate": true,
                    "public_path": "/v1/chat/completions"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("{\"fallback\":true}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                let public_execution_paths_inner = Arc::clone(&public_execution_paths_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    public_execution_paths_inner
                        .lock()
                        .expect("mutex should lock")
                        .push(
                            request
                                .headers()
                                .get(EXECUTION_PATH_HEADER)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        );
                    let mut response = Response::builder()
                        .status(StatusCode::ACCEPTED)
                        .body(Body::from("{\"proxied\":true}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }
            }),
        );

    let execution_runtime = Router::new();

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_execution_runtime_override(execution_runtime_url);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    for trace_id in ["trace-openai-chat-bypass-1", "trace-openai-chat-bypass-2"] {
        let response = client
            .post(format!("{gateway_url}/v1/chat/completions"))
            .header(http::header::CONTENT_TYPE, "application/json")
            .header(TRACE_ID_HEADER, trace_id)
            .body("{\"model\":\"gpt-5\",\"messages\":[]}")
            .send()
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(EXECUTION_PATH_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS)
        );
        assert_eq!(
            response
                .headers()
                .get(DEPENDENCY_REASON_HEADER)
                .and_then(|value| value.to_str().ok()),
            None
        );
        assert_eq!(
            response
                .headers()
                .get(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("missing_auth_context")
        );
        let payload: serde_json::Value = response.json().await.expect("body should parse");
        assert_eq!(payload["error"]["type"], "http_error");
        assert_eq!(
            payload["error"]["message"],
            "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商"
        );
    }

    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        *public_execution_paths.lock().expect("mutex should lock"),
        Vec::<String>::new()
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_openai_chat_when_control_api_is_configured_without_opt_in() {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let public_execution_path = Arc::new(Mutex::new(None::<String>));
    let public_execution_path_clone = Arc::clone(&public_execution_path);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/resolve",
            any(|_request: Request| async move {
                Json(json!({
                    "action": "proxy_public",
                    "route_class": "ai_public",
                    "route_family": "openai",
                    "route_kind": "chat",
                    "auth_endpoint_signature": "openai:chat",
                    "execution_runtime_candidate": true,
                    "public_path": "/v1/chat/completions"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("{\"fallback\":true}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                let public_execution_path_inner = Arc::clone(&public_execution_path_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    *public_execution_path_inner
                        .lock()
                        .expect("mutex should lock") = Some(
                        request
                            .headers()
                            .get(EXECUTION_PATH_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    let mut response = Response::builder()
                        .status(StatusCode::ACCEPTED)
                        .body(Body::from("{\"proxied\":true}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS)
    );
    assert_eq!(
        response
            .headers()
            .get(DEPENDENCY_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        None
    );
    assert_eq!(
        response
            .headers()
            .get(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("missing_auth_context")
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商"
    );
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        public_execution_path
            .lock()
            .expect("mutex should lock")
            .as_deref(),
        None
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_openai_chat_stream_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let public_execution_path = Arc::new(Mutex::new(None::<String>));
    let public_execution_path_clone = Arc::clone(&public_execution_path);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/resolve",
            any(|_request: Request| async move {
                Json(json!({
                    "action": "proxy_public",
                    "route_class": "ai_public",
                    "route_family": "openai",
                    "route_kind": "chat",
                    "auth_endpoint_signature": "openai:chat",
                    "execution_runtime_candidate": true,
                    "public_path": "/v1/chat/completions"
                }))
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                let public_execution_path_inner = Arc::clone(&public_execution_path_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    *public_execution_path_inner
                        .lock()
                        .expect("mutex should lock") = Some(
                        request
                            .headers()
                            .get(EXECUTION_PATH_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    let mut response = Response::builder()
                        .status(StatusCode::ACCEPTED)
                        .body(Body::from("{\"proxied\":true}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }
            }),
        );

    let execution_runtime = Router::new();

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_execution_runtime_override(execution_runtime_url);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS)
    );
    assert_eq!(
        response
            .headers()
            .get(DEPENDENCY_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        None
    );
    assert_eq!(
        response
            .headers()
            .get(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("missing_auth_context")
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商"
    );
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        public_execution_path
            .lock()
            .expect("mutex should lock")
            .as_deref(),
        None
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

async fn assert_ai_route_locally_denied_after_execution_runtime_miss(
    public_path: &'static str,
    route_family: &'static str,
    route_kind: &'static str,
    endpoint_signature: &'static str,
    request_body: &'static str,
    expected_message: &'static str,
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss_with_request(
        reqwest::Method::POST,
        public_path,
        public_path,
        route_family,
        route_kind,
        endpoint_signature,
        Some(request_body),
        expected_message,
    )
    .await;
}

async fn assert_ai_route_locally_denied_after_execution_runtime_miss_with_request(
    method: reqwest::Method,
    route_path: &'static str,
    request_path: &'static str,
    route_family: &'static str,
    route_kind: &'static str,
    endpoint_signature: &'static str,
    request_body: Option<&'static str>,
    expected_message: &'static str,
) {
    let control_execute_hits = Arc::new(Mutex::new(0usize));
    let control_execute_hits_clone = Arc::clone(&control_execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let public_execution_path = Arc::new(Mutex::new(None::<String>));
    let public_execution_path_clone = Arc::clone(&public_execution_path);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/resolve",
            any(move |_request: Request| async move {
                Json(json!({
                    "action": "proxy_public",
                    "route_class": "ai_public",
                    "route_family": route_family,
                    "route_kind": route_kind,
                    "auth_endpoint_signature": endpoint_signature,
                    "execution_runtime_candidate": true,
                    "public_path": route_path
                }))
            }),
        )
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let control_execute_hits_inner = Arc::clone(&control_execute_hits_clone);
                async move {
                    *control_execute_hits_inner
                        .lock()
                        .expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("{\"fallback\":true}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }
            }),
        )
        .route(
            route_path,
            any(move |request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                let public_execution_path_inner = Arc::clone(&public_execution_path_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    *public_execution_path_inner
                        .lock()
                        .expect("mutex should lock") = Some(
                        request
                            .headers()
                            .get(EXECUTION_PATH_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    let mut response = Response::builder()
                        .status(StatusCode::ACCEPTED)
                        .body(Body::from("{\"proxied\":true}"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    );
                    response
                }
            }),
        );

    let execution_runtime = Router::new();

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_execution_runtime_override(execution_runtime_url);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut request =
        reqwest::Client::new().request(method, format!("{gateway_url}{request_path}"));
    if let Some(request_body) = request_body {
        request = request
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(request_body);
    }
    let response = request.send().await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS)
    );
    assert_eq!(
        response
            .headers()
            .get(DEPENDENCY_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        None
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(payload["error"]["message"], expected_message);
    assert_eq!(*control_execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        public_execution_path
            .lock()
            .expect("mutex should lock")
            .as_deref(),
        None
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_openai_responses_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1/responses",
        "openai",
        "cli",
        "openai:cli",
        "{\"model\":\"gpt-5\",\"input\":\"hello\"}",
        "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_claude_messages_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1/messages",
        "claude",
        "chat",
        "claude:chat",
        "{\"model\":\"claude-sonnet-4-5\",\"messages\":[]}",
        "请求缺少本地执行所需的认证、模型或配置上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_openai_responses_stream_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1/responses",
        "openai",
        "cli",
        "openai:cli",
        "{\"model\":\"gpt-5\",\"input\":\"hello\",\"stream\":true}",
        "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_claude_messages_stream_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1/messages",
        "claude",
        "chat",
        "claude:chat",
        "{\"model\":\"claude-sonnet-4-5\",\"messages\":[],\"stream\":true}",
        "请求缺少本地执行所需的认证、模型或配置上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_openai_compact_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1/responses/compact",
        "openai",
        "compact",
        "openai:compact",
        "{\"model\":\"gpt-5\",\"input\":\"hello\"}",
        "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_openai_compact_stream_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1/responses/compact",
        "openai",
        "compact",
        "openai:compact",
        "{\"model\":\"gpt-5\",\"input\":\"hello\",\"stream\":true}",
        "请求缺少有效的用户或 API Key 认证上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_gemini_generate_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1beta/models/gemini-2.5-pro:generateContent",
        "gemini",
        "chat",
        "gemini:chat",
        "{\"contents\":[]}",
        "请求缺少本地执行所需的认证、模型或配置上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_gemini_v1_generate_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1/models/gemini-2.5-pro:generateContent",
        "gemini",
        "chat",
        "gemini:chat",
        "{\"contents\":[]}",
        "请求缺少本地执行所需的认证、模型或配置上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_gemini_stream_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
        "gemini",
        "chat",
        "gemini:chat",
        "{\"contents\":[]}",
        "请求缺少本地执行所需的认证、模型或配置上下文，无法选择上游提供商",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_openai_video_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1/videos",
        "openai",
        "video",
        "openai:video",
        "{\"model\":\"sora-2\"}",
        "当前 OpenAI Video 请求无法在本地执行：没有匹配到可用的执行路径",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_gemini_video_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss(
        "/v1beta/models/veo-3:predictLongRunning",
        "gemini",
        "video",
        "gemini:video",
        "{\"instances\":[]}",
        "当前 Gemini Public 请求无法在本地执行：没有匹配到可用的执行路径",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_gemini_files_root_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss_with_request(
        reqwest::Method::GET,
        "/v1beta/files",
        "/v1beta/files?view=BASIC",
        "gemini",
        "files",
        "gemini:chat",
        None,
        "当前 Gemini Files 请求无法在本地执行：没有匹配到可用的执行路径",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_gemini_files_download_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss_with_request(
        reqwest::Method::GET,
        "/v1beta/files/file-123:download",
        "/v1beta/files/file-123:download?alt=media",
        "gemini",
        "files",
        "gemini:chat",
        None,
        "当前 Gemini Files 请求无法在本地执行：没有匹配到可用的执行路径",
    )
    .await;
}

#[tokio::test]
async fn gateway_locally_denies_gemini_files_upload_after_execution_runtime_miss_without_control_execute_opt_in(
) {
    assert_ai_route_locally_denied_after_execution_runtime_miss_with_request(
        reqwest::Method::POST,
        "/upload/v1beta/files",
        "/upload/v1beta/files?uploadType=resumable",
        "gemini",
        "files",
        "gemini:chat",
        Some("{\"file\":{}}"),
        "当前 Gemini Files 请求无法在本地执行：没有匹配到可用的执行路径",
    )
    .await;
}
