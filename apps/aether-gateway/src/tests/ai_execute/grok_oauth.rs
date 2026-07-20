use std::collections::BTreeMap;

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    strip_sse_keepalive_comments, to_bytes, Arc, Body, HeaderValue, Json, Mutex, Request, Response,
    Router, StatusCode, TRACE_ID_HEADER,
};

const GROK_OAUTH_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;
const CLIENT_API_KEY: &str = "sk-client-grok-oauth";
const GROK_ACCESS_TOKEN: &str = "grok-access-token";

#[derive(Debug, Clone)]
struct SeenGrokOAuthRequest {
    execution_mode: &'static str,
    trace_id: String,
    url: String,
    headers: BTreeMap<String, String>,
    body: serde_json::Value,
}

impl SeenGrokOAuthRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

fn run_grok_oauth_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(GROK_OAUTH_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("Grok OAuth test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn sample_auth_snapshot() -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        "user-grok-oauth".to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(json!(["openai", "grok_oauth"])),
        Some(json!(["openai:chat", "openai:responses"])),
        Some(json!(["grok-4"])),
        "client-key-grok-oauth".to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(json!(["openai", "grok_oauth"])),
        Some(json!(["openai:chat", "openai:responses"])),
        Some(json!(["grok-4"])),
    )
    .expect("auth snapshot should build")
}

fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-grok-oauth".to_string(),
        provider_name: "Grok OAuth".to_string(),
        provider_type: "grok_oauth".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: "endpoint-grok-oauth-responses".to_string(),
        endpoint_api_format: "openai:responses".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("cli".to_string()),
        endpoint_is_active: true,
        key_id: "key-grok-oauth".to_string(),
        key_name: "oauth".to_string(),
        key_auth_type: "oauth".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:responses".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(json!({"openai:responses": 1})),
        model_id: "model-grok-oauth".to_string(),
        global_model_id: "global-model-grok-oauth".to_string(),
        global_model_name: "grok-4".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "grok-4".to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: "grok-4".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:responses".to_string()]),
            endpoint_ids: None,
            operations: None,
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn sample_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-grok-oauth".to_string(),
        "Grok OAuth".to_string(),
        Some("https://x.ai".to_string()),
        "grok_oauth".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        true,
        None,
        Some(2),
        None,
        Some(20.0),
        None,
        None,
    )
}

fn sample_endpoint() -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        "endpoint-grok-oauth-responses".to_string(),
        "provider-grok-oauth".to_string(),
        "openai:responses".to_string(),
        Some("openai".to_string()),
        Some("cli".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        "https://cli-chat-proxy.grok.com/v1".to_string(),
        None,
        None,
        Some(2),
        None,
        Some(json!({"upstream_stream_policy": "force_stream"})),
        None,
        None,
    )
    .expect("endpoint transport should build")
}

fn sample_provider_key() -> StoredProviderCatalogKey {
    let auth_config = json!({
        "provider_type": "grok_oauth",
        "refresh_token": "grok-refresh-token",
        "expires_at": 4_102_444_800_u64,
        "headers": {
            "X-XAI-Token-Auth": "xai-grok-cli",
            "x-grok-client-version": "0.2.93",
            "User-Agent": "xai-grok-workspace/0.2.93"
        }
    });
    StoredProviderCatalogKey::new(
        "key-grok-oauth".to_string(),
        "provider-grok-oauth".to_string(),
        "oauth".to_string(),
        "oauth".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!(["openai:responses"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, GROK_ACCESS_TOKEN)
            .expect("access token should encrypt"),
        Some(
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, &auth_config.to_string())
                .expect("auth config should encrypt"),
        ),
        None,
        Some(json!({"openai:responses": 1})),
        None,
        Some(4_102_444_800),
        None,
        None,
    )
    .expect("key transport should build")
}

async fn capture_execution_request(
    request: Request,
    execution_mode: &'static str,
    seen_requests: Arc<Mutex<Vec<SeenGrokOAuthRequest>>>,
) {
    let (parts, body) = request.into_parts();
    let raw_body = to_bytes(body, usize::MAX)
        .await
        .expect("execution request body should read");
    let payload: serde_json::Value =
        serde_json::from_slice(&raw_body).expect("execution request should parse");
    let headers = payload
        .get("headers")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            value
                .as_str()
                .map(|value| (name.to_ascii_lowercase(), value.to_string()))
        })
        .collect();

    seen_requests
        .lock()
        .expect("seen requests mutex should lock")
        .push(SeenGrokOAuthRequest {
            execution_mode,
            trace_id: parts
                .headers
                .get(TRACE_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            url: payload
                .get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            headers,
            body: payload
                .pointer("/body/json_body")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        });
}

fn grok_response_event() -> serde_json::Value {
    json!({
        "type": "response.completed",
        "response": {
            "id": "resp-grok-oauth",
            "object": "response",
            "model": "grok-4",
            "status": "completed",
            "output": [{
                "type": "message",
                "id": "msg-grok-oauth",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello from Grok",
                    "annotations": []
                }]
            }],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 4,
                "total_tokens": 7
            }
        }
    })
}

fn grok_response_sse() -> String {
    format!(
        "event: response.completed\ndata: {}\n\n",
        grok_response_event()
    )
}

fn sync_execution_response() -> Json<serde_json::Value> {
    Json(json!({
        "request_id": "execution-grok-oauth-sync",
        "status_code": 200,
        "headers": {"content-type": "text/event-stream"},
        "body": {
            "body_bytes_b64": STANDARD.encode(grok_response_sse())
        },
        "telemetry": {"elapsed_ms": 21}
    }))
}

fn stream_execution_response() -> Response {
    let frames = [
        json!({
            "type": "headers",
            "payload": {
                "kind": "headers",
                "status_code": 200,
                "headers": {"content-type": "text/event-stream"}
            }
        }),
        json!({
            "type": "data",
            "payload": {
                "kind": "data",
                "text": grok_response_sse()
            }
        }),
        json!({
            "type": "telemetry",
            "payload": {
                "kind": "telemetry",
                "telemetry": {"elapsed_ms": 23, "ttfb_ms": 7}
            }
        }),
        json!({"type": "eof", "payload": {"kind": "eof"}}),
    ]
    .into_iter()
    .map(|frame| format!("{frame}\n"))
    .collect::<String>();
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(frames))
        .expect("stream execution response should build");
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    response
}

fn assert_grok_oauth_contract(request: &SeenGrokOAuthRequest) {
    assert_eq!(request.url, "https://cli-chat-proxy.grok.com/v1/responses");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer grok-access-token")
    );
    assert_eq!(request.header("x-xai-token-auth"), Some("xai-grok-cli"));
    assert_eq!(request.header("x-grok-client-version"), Some("0.2.93"));
    assert_eq!(
        request.header("user-agent"),
        Some("xai-grok-workspace/0.2.93")
    );
    assert_eq!(request.body["model"], "grok-4");
    assert_eq!(request.body["stream"], true);
}

#[test]
fn gateway_routes_grok_oauth_responses_and_chat_through_cli_responses_contract() {
    run_grok_oauth_test(
        "gateway_routes_grok_oauth_responses_and_chat_through_cli_responses_contract",
        gateway_routes_grok_oauth_responses_and_chat_through_cli_responses_contract_impl,
    );
}

async fn gateway_routes_grok_oauth_responses_and_chat_through_cli_responses_contract_impl() {
    let seen_requests = Arc::new(Mutex::new(Vec::<SeenGrokOAuthRequest>::new()));
    let sync_seen_requests = Arc::clone(&seen_requests);
    let stream_seen_requests = Arc::clone(&seen_requests);
    let execution_runtime = Router::new()
        .route(
            "/v1/execute/sync",
            any(move |request: Request| {
                let seen_requests = Arc::clone(&sync_seen_requests);
                async move {
                    capture_execution_request(request, "sync", seen_requests).await;
                    sync_execution_response()
                }
            }),
        )
        .route(
            "/v1/execute/stream",
            any(move |request: Request| {
                let seen_requests = Arc::clone(&stream_seen_requests);
                async move {
                    capture_execution_request(request, "stream", seen_requests).await;
                    stream_execution_response()
                }
            }),
        );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(CLIENT_API_KEY)),
        sample_auth_snapshot(),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_provider_key()],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                request_candidate_repository,
                DEVELOPMENT_ENCRYPTION_KEY,
            )
            .with_system_config_values_for_tests(vec![
                ("scheduling_mode".to_string(), json!("fixed_order")),
                ("provider_priority_mode".to_string(), json!("global_key")),
            ]),
        );
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(gateway_state)).await;
    let client = reqwest::Client::new();

    let responses = client
        .post(format!("{gateway_url}/v1/responses"))
        .bearer_auth(CLIENT_API_KEY)
        .header(TRACE_ID_HEADER, "trace-grok-oauth-responses")
        .json(&json!({"model": "grok-4", "input": "Direct prompt"}))
        .send()
        .await
        .expect("Responses request should succeed");
    assert_eq!(responses.status(), StatusCode::OK);
    let responses_json: serde_json::Value =
        responses.json().await.expect("Responses body should parse");
    assert_eq!(responses_json["id"], "resp-grok-oauth");
    assert_eq!(
        responses_json["output"][0]["content"][0]["text"],
        "Hello from Grok"
    );

    let chat_sync = client
        .post(format!("{gateway_url}/v1/chat/completions"))
        .bearer_auth(CLIENT_API_KEY)
        .header(TRACE_ID_HEADER, "trace-grok-oauth-chat-sync")
        .json(&json!({
            "model": "grok-4",
            "messages": [
                {"role": "system", "content": "Answer briefly."},
                {"role": "user", "content": "Say hello"}
            ],
            "stream": false
        }))
        .send()
        .await
        .expect("Chat sync request should succeed");
    assert_eq!(chat_sync.status(), StatusCode::OK);
    let chat_sync_json: serde_json::Value =
        chat_sync.json().await.expect("Chat sync body should parse");
    assert_eq!(chat_sync_json["object"], "chat.completion");
    assert_eq!(
        chat_sync_json["choices"][0]["message"]["content"],
        "Hello from Grok"
    );

    let chat_stream = client
        .post(format!("{gateway_url}/v1/chat/completions"))
        .bearer_auth(CLIENT_API_KEY)
        .header(TRACE_ID_HEADER, "trace-grok-oauth-chat-stream")
        .json(&json!({
            "model": "grok-4",
            "messages": [{"role": "user", "content": "Stream hello"}],
            "stream": true
        }))
        .send()
        .await
        .expect("Chat stream request should succeed");
    assert_eq!(chat_stream.status(), StatusCode::OK);
    let chat_stream_text = strip_sse_keepalive_comments(
        &chat_stream
            .text()
            .await
            .expect("Chat stream body should read"),
    );
    assert!(chat_stream_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(chat_stream_text.contains("\"content\":\"Hello from Grok\""));
    assert!(chat_stream_text.contains("data: [DONE]"));

    let seen_requests = seen_requests
        .lock()
        .expect("seen requests mutex should lock")
        .clone();
    assert_eq!(seen_requests.len(), 3);
    for request in &seen_requests {
        assert_grok_oauth_contract(request);
    }

    let direct = seen_requests
        .iter()
        .find(|request| request.trace_id == "trace-grok-oauth-responses")
        .expect("direct Responses execution should be captured");
    assert_eq!(direct.execution_mode, "sync");
    assert_eq!(direct.body["input"], "Direct prompt");

    let chat_sync = seen_requests
        .iter()
        .find(|request| request.trace_id == "trace-grok-oauth-chat-sync")
        .expect("Chat sync execution should be captured");
    assert_eq!(chat_sync.execution_mode, "sync");
    assert_eq!(chat_sync.body["instructions"], "Answer briefly.");
    assert_eq!(
        chat_sync.body["input"][0]["content"][0]["text"],
        "Say hello"
    );

    let chat_stream = seen_requests
        .iter()
        .find(|request| request.trace_id == "trace-grok-oauth-chat-stream")
        .expect("Chat stream execution should be captured");
    assert_eq!(chat_stream.execution_mode, "stream");
    assert_eq!(
        chat_stream.body["input"][0]["content"][0]["text"],
        "Stream hello"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}
