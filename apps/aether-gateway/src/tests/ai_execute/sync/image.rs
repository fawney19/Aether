use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    to_bytes, Arc, Body, Json, Mutex, Request, Router, StatusCode, TRACE_ID_HEADER,
};
use crate::ai_serving::CODEX_OPENAI_IMAGE_INTERNAL_MODEL;
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
use base64::Engine as _;
use sha2::{Digest, Sha256};

const IMAGE_SYNC_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_image_sync_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(IMAGE_SYNC_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("image sync test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn gateway_converts_openai_image_sync_to_gemini_image_provider() {
    run_image_sync_test(
        "gateway_converts_openai_image_sync_to_gemini_image_provider",
        gateway_converts_openai_image_sync_to_gemini_image_provider_impl,
    );
}

async fn gateway_converts_openai_image_sync_to_gemini_image_provider_impl() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        trace_id: String,
        url: String,
        auth_header_value: String,
        has_model_field: bool,
        prompt: String,
        response_modalities: Vec<String>,
        image_size: String,
    }

    fn hash_api_key(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai", "google"])),
            Some(serde_json::json!(["openai:image"])),
            Some(serde_json::json!(["gpt-image-2"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(serde_json::json!(["openai", "google"])),
            Some(serde_json::json!(["openai:image"])),
            Some(serde_json::json!(["gpt-image-2"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-gemini-image-bridge-1".to_string(),
            provider_name: "google".to_string(),
            provider_type: "google".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-gemini-image-bridge-1".to_string(),
            endpoint_api_format: "gemini:generate_content".to_string(),
            endpoint_api_family: Some("gemini".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: "key-gemini-image-bridge-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["gemini:generate_content".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({
                "gemini:generate_content": 1
            })),
            model_id: "model-gemini-image-bridge-1".to_string(),
            global_model_id: "global-model-gemini-image-bridge-1".to_string(),
            global_model_name: "gpt-image-2".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gemini-2.5-flash-image-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gemini-2.5-flash-image-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["gemini:generate_content".to_string()]),
                endpoint_ids: None,
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-gemini-image-bridge-1".to_string(),
            "google".to_string(),
            Some("https://generativelanguage.googleapis.com".to_string()),
            "google".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            None,
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-gemini-image-bridge-1".to_string(),
            "provider-gemini-image-bridge-1".to_string(),
            "gemini:generate_content".to_string(),
            Some("gemini".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://generativelanguage.googleapis.com".to_string(),
            None,
            Some(serde_json::json!([
                {"action":"drop","path":"model"}
            ])),
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-gemini-image-bridge-1".to_string(),
            "provider-gemini-image-bridge-1".to_string(),
            "prod".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["gemini:generate_content"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-gemini-image")
                .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"gemini:generate_content": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                let body_json = payload
                    .get("body")
                    .and_then(|value| value.get("json_body"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeSyncRequest {
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    url: payload
                        .get("url")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    auth_header_value: payload
                        .get("headers")
                        .and_then(|value| value.get("x-goog-api-key"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    has_model_field: body_json.get("model").is_some(),
                    prompt: body_json
                        .get("contents")
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("parts"))
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("text"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    response_modalities: body_json
                        .get("generationConfig")
                        .and_then(|value| value.get("responseModalities"))
                        .and_then(|value| value.as_array())
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                    image_size: body_json
                        .get("generationConfig")
                        .and_then(|value| value.get("imageSize"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
                Json(json!({
                    "request_id": "trace-openai-image-to-gemini-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "modelVersion": "gemini-2.5-flash-image-upstream",
                            "usageMetadata": {
                                "promptTokenCount": 11,
                                "candidatesTokenCount": 22,
                                "totalTokenCount": 33
                            },
                            "candidates": [{
                                "content": {
                                    "role": "model",
                                    "parts": [
                                        {"text": "revised kite prompt"},
                                        {"inlineData": {"mimeType": "image/png", "data": "aGVsbG8="}}
                                    ]
                                },
                                "finishReason": "STOP"
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 37
                    }
                }))
            }
        }),
    );

    let client_api_key = "sk-client-openai-image-to-gemini";
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(client_api_key)),
        sample_auth_snapshot(
            "key-openai-image-client-bridge-1",
            "user-openai-image-bridge-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
    ));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::new(InMemoryRequestCandidateRepository::default()),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {client_api_key}"),
        )
        .header(TRACE_ID_HEADER, "trace-openai-image-to-gemini-123")
        .body(
            "{\"model\":\"gpt-image-2\",\"prompt\":\"Draw a red kite\",\"size\":\"1024x1024\",\"response_format\":\"b64_json\"}",
        )
        .send()
        .await
        .expect("request should succeed");

    let response_status = response.status();
    let response_body = response.text().await.expect("body should read");
    assert_eq!(response_status, StatusCode::OK, "{response_body}");
    let response_json: serde_json::Value =
        serde_json::from_str(&response_body).expect("body should parse");
    assert_eq!(response_json["data"][0]["b64_json"], "aGVsbG8=");
    assert_eq!(
        response_json["data"][0]["revised_prompt"],
        "revised kite prompt"
    );
    assert_eq!(response_json["model"], "gemini-2.5-flash-image-upstream");
    assert_eq!(response_json["usage"]["input_tokens"], 11);
    assert_eq!(response_json["usage"]["output_tokens"], 22);

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-openai-image-to-gemini-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image-upstream:generateContent"
    );
    assert_eq!(
        seen_execution_runtime_request.auth_header_value,
        "sk-upstream-gemini-image"
    );
    assert!(!seen_execution_runtime_request.has_model_field);
    assert_eq!(seen_execution_runtime_request.prompt, "Draw a red kite");
    assert_eq!(
        seen_execution_runtime_request.response_modalities,
        vec!["TEXT".to_string(), "IMAGE".to_string()]
    );
    assert_eq!(seen_execution_runtime_request.image_size, "1024x1024");

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_converts_gemini_image_sync_to_openai_image_provider() {
    run_image_sync_test(
        "gateway_converts_gemini_image_sync_to_openai_image_provider",
        gateway_converts_gemini_image_sync_to_openai_image_provider_impl,
    );
}

async fn gateway_converts_gemini_image_sync_to_openai_image_provider_impl() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        trace_id: String,
        url: String,
        authorization: String,
        model: String,
        action: String,
        prompt: String,
        image_url: String,
        request_stream: bool,
    }

    fn hash_api_key(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["gemini", "openai"])),
            Some(serde_json::json!(["gemini:generate_content"])),
            Some(serde_json::json!(["gemini-image"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(serde_json::json!(["gemini", "openai"])),
            Some(serde_json::json!(["gemini:generate_content"])),
            Some(serde_json::json!(["gemini-image"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-image-bridge-1".to_string(),
            provider_name: "openai".to_string(),
            provider_type: "openai".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-image-bridge-1".to_string(),
            endpoint_api_format: "openai:image".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("image".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-image-bridge-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:image".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:image": 1})),
            model_id: "model-openai-image-bridge-1".to_string(),
            global_model_id: "global-model-openai-image-bridge-1".to_string(),
            global_model_name: "gemini-image".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-image-2-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-image-2-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:image".to_string()]),
                endpoint_ids: None,
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-image-bridge-1".to_string(),
            "openai".to_string(),
            Some("https://api.openai.com/v1".to_string()),
            "openai".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            None,
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-openai-image-bridge-1".to_string(),
            "provider-openai-image-bridge-1".to_string(),
            "openai:image".to_string(),
            Some("openai".to_string()),
            Some("image".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.com/v1".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-image-bridge-1".to_string(),
            "provider-openai-image-bridge-1".to_string(),
            "prod".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:image"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-openai-image")
                .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"openai:image": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                let body_json = payload
                    .get("body")
                    .and_then(|value| value.get("json_body"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let content = body_json
                    .get("input")
                    .and_then(|value| value.get(0))
                    .and_then(|value| value.get("content"))
                    .cloned()
                    .unwrap_or_else(|| json!([]));
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeSyncRequest {
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    url: payload
                        .get("url")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    authorization: payload
                        .get("headers")
                        .and_then(|value| value.get("authorization"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    model: body_json
                        .get("model")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    action: body_json
                        .get("tools")
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("action"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    prompt: content
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|item| {
                            item.get("type").and_then(|value| value.as_str()) == Some("input_text")
                        })
                        .and_then(|item| item.get("text"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    image_url: content
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|item| {
                            item.get("type").and_then(|value| value.as_str()) == Some("input_image")
                        })
                        .and_then(|item| item.get("image_url"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    request_stream: body_json
                        .get("stream")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(true),
                });
                Json(json!({
                    "request_id": "trace-gemini-image-to-openai-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "resp_img_bridge_123",
                            "object": "response",
                            "model": "gpt-image-2-upstream",
                            "status": "completed",
                            "usage": {
                                "input_tokens": 3,
                                "output_tokens": 4,
                                "total_tokens": 7
                            },
                            "output": [{
                                "type": "image_generation_call",
                                "status": "completed",
                                "output_format": "png",
                                "revised_prompt": "converted gemini prompt",
                                "result": "aGVsbG8="
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 43
                    }
                }))
            }
        }),
    );

    let client_api_key = "client-gemini-image-to-openai";
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(client_api_key)),
        sample_auth_snapshot(
            "key-gemini-image-client-bridge-1",
            "user-gemini-image-bridge-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
    ));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::new(InMemoryRequestCandidateRepository::default()),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/v1beta/models/gemini-image:generateContent?key={client_api_key}"
        ))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(TRACE_ID_HEADER, "trace-gemini-image-to-openai-123")
        .body(
            "{\"generationConfig\":{\"responseModalities\":[\"TEXT\",\"IMAGE\"]},\"contents\":[{\"role\":\"user\",\"parts\":[{\"text\":\"Change the background\"},{\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"aGVsbG8=\"}}]}]}",
        )
        .send()
        .await
        .expect("request should succeed");

    let response_status = response.status();
    let response_body = response.text().await.expect("body should read");
    assert_eq!(response_status, StatusCode::OK, "{response_body}");
    let response_json: serde_json::Value =
        serde_json::from_str(&response_body).expect("body should parse");
    assert_eq!(response_json["modelVersion"], "gpt-image-2-upstream");
    assert_eq!(
        response_json["candidates"][0]["content"]["parts"][0]["text"],
        "converted gemini prompt"
    );
    assert_eq!(
        response_json["candidates"][0]["content"]["parts"][1]["inlineData"]["mimeType"],
        "image/png"
    );
    assert_eq!(
        response_json["candidates"][0]["content"]["parts"][1]["inlineData"]["data"],
        "aGVsbG8="
    );
    assert_eq!(response_json["usageMetadata"]["promptTokenCount"], 3);
    assert_eq!(response_json["usageMetadata"]["candidatesTokenCount"], 4);

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-gemini-image-to-openai-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.openai.com/v1/images/generations"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-image"
    );
    assert_eq!(seen_execution_runtime_request.model, "gpt-image-2-upstream");
    assert_eq!(seen_execution_runtime_request.action, "edit");
    assert_eq!(
        seen_execution_runtime_request.prompt,
        "Change the background"
    );
    assert_eq!(
        seen_execution_runtime_request.image_url,
        "data:image/png;base64,aGVsbG8="
    );
    assert!(!seen_execution_runtime_request.request_stream);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_executes_codex_image_sync_via_local_decision_gate_after_oauth_refresh() {
    run_image_sync_test(
        "gateway_executes_codex_image_sync_via_local_decision_gate_after_oauth_refresh",
        gateway_executes_codex_image_sync_via_local_decision_gate_after_oauth_refresh_impl,
    );
}

async fn gateway_executes_codex_image_sync_via_local_decision_gate_after_oauth_refresh_impl() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        trace_id: String,
        url: String,
        model: String,
        authorization: String,
        x_client_request_id: String,
        prompt: String,
        content_is_string: bool,
        tool_type: String,
        tool_size: String,
        tool_quality: String,
        tool_background: String,
        tool_choice_type: String,
        tool_has_n: bool,
        request_stream: bool,
        plan_stream: bool,
    }

    #[derive(Debug, Clone)]
    struct SeenRefreshRequest {
        content_type: String,
        body: String,
    }

    fn hash_api_key(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai", "codex"])),
            Some(serde_json::json!(["openai:image"])),
            Some(serde_json::json!(["gpt-image-2"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(serde_json::json!(["openai", "codex"])),
            Some(serde_json::json!(["openai:image"])),
            Some(serde_json::json!(["gpt-image-2"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-codex-image-local-1".to_string(),
            provider_name: "codex".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-codex-image-local-1".to_string(),
            endpoint_api_format: "openai:image".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("image".to_string()),
            endpoint_is_active: true,
            key_id: "key-codex-image-local-1".to_string(),
            key_name: "oauth".to_string(),
            key_auth_type: "oauth".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:image".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:image": 1})),
            model_id: "model-codex-image-local-1".to_string(),
            global_model_id: "global-model-codex-image-local-1".to_string(),
            global_model_name: "gpt-image-2".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-image-2".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-image-2".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:image".to_string()]),
                endpoint_ids: None,
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-codex-image-local-1".to_string(),
            "codex".to_string(),
            Some("https://chatgpt.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            None,
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-codex-image-local-1".to_string(),
            "provider-codex-image-local-1".to_string(),
            "openai:image".to_string(),
            Some("openai".to_string()),
            Some("image".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://chatgpt.com/backend-api/codex".to_string(),
            None,
            None,
            Some(2),
            None,
            Some(serde_json::json!({"upstream_stream_policy":"force_stream"})),
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"rt-codex-image-local-123"}"#,
        )
        .expect("auth config should encrypt");
        StoredProviderCatalogKey::new(
            "key-codex-image-local-1".to_string(),
            "provider-codex-image-local-1".to_string(),
            "oauth".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:image"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
                .expect("placeholder api key should encrypt"),
            Some(encrypted_auth_config),
            None,
            Some(serde_json::json!({"openai:image": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let seen_refresh = Arc::new(Mutex::new(None::<SeenRefreshRequest>));
    let seen_refresh_clone = Arc::clone(&seen_refresh);
    let refresh_hits = Arc::new(Mutex::new(0usize));
    let refresh_hits_clone = Arc::clone(&refresh_hits);

    let refresh = Router::new().route(
        "/oauth/token",
        any(move |request: Request| {
            let seen_refresh_inner = Arc::clone(&seen_refresh_clone);
            let refresh_hits_inner = Arc::clone(&refresh_hits_clone);
            async move {
                *refresh_hits_inner.lock().expect("mutex should lock") += 1;
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                *seen_refresh_inner.lock().expect("mutex should lock") = Some(SeenRefreshRequest {
                    content_type: parts
                        .headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: String::from_utf8(raw_body.to_vec())
                        .expect("refresh body should be utf8"),
                });
                Json(json!({
                    "access_token": "refreshed-codex-image-access-token",
                    "refresh_token": "rt-codex-image-local-456",
                    "token_type": "Bearer",
                    "expires_in": 3600
                }))
            }
        }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeSyncRequest {
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    url: payload
                        .get("url")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    model: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("model"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    authorization: payload
                        .get("headers")
                        .and_then(|value| value.get("authorization"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    x_client_request_id: payload
                        .get("headers")
                        .and_then(|value| value.get("x-client-request-id"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    prompt: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("input"))
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("content"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    content_is_string: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("input"))
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("content"))
                        .is_some_and(|value| value.is_string()),
                    tool_type: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("tools"))
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("type"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tool_size: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("tools"))
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("size"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tool_quality: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("tools"))
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("quality"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tool_background: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("tools"))
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.get("background"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tool_choice_type: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("tool_choice"))
                        .and_then(|value| value.get("type"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tool_has_n: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("tools"))
                        .and_then(|value| value.get(0))
                        .and_then(|value| value.as_object())
                        .is_some_and(|object| object.contains_key("n")),
                    request_stream: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("stream"))
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false),
                    plan_stream: payload
                        .get("stream")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false),
                });
                Json(json!({
                    "request_id": "trace-codex-image-local-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "text/event-stream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                            concat!(
                                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img_123\",\"created_at\":1776839946}}\n\n",
                                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"status\":\"generating\",\"output_format\":\"png\",\"quality\":\"medium\",\"size\":\"1024x1024\",\"revised_prompt\":\"中国历史视觉海报\",\"result\":\"aGVsbG8=\"}}\n\n",
                                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_123\",\"object\":\"response\",\"model\":\"__CODEX_IMAGE_MODEL__\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":2440,\"output_tokens\":184,\"total_tokens\":2624},\"tool_usage\":{\"image_gen\":{\"input_tokens\":171,\"input_tokens_details\":{\"image_tokens\":0,\"text_tokens\":171},\"output_tokens\":1372,\"output_tokens_details\":{\"image_tokens\":1372,\"text_tokens\":0},\"total_tokens\":1543}}}}\n\n",
                                "data: [DONE]\n\n"
                            )
                            .replace(
                                "__CODEX_IMAGE_MODEL__",
                                CODEX_OPENAI_IMAGE_INTERNAL_MODEL,
                            )
                        )
                    },
                    "telemetry": {
                        "elapsed_ms": 41
                    }
                }))
            }
        }),
    );

    let client_api_key = "sk-client-codex-image-local";
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(client_api_key)),
        sample_auth_snapshot("key-codex-image-client-123", "user-codex-image-client-123"),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
    ));

    let (refresh_url, refresh_handle) = start_server(refresh).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", format!("{refresh_url}/oauth/token")),
            ),
        ]);
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url.clone())
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository.clone(),
                Arc::new(InMemoryRequestCandidateRepository::default()),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
        .with_oauth_refresh_coordinator_for_tests(oauth_refresh);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {client_api_key}"),
        )
        .header(TRACE_ID_HEADER, "trace-codex-image-local-123")
        .body("{\"model\":\"gpt-image-2\",\"prompt\":\"生成一张中国历史视觉海报\",\"size\":\"1024x1024\",\"n\":1,\"response_format\":\"b64_json\"}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let response_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(response_json["created"], 1776839946);
    assert_eq!(response_json["data"][0]["b64_json"], "aGVsbG8=");
    assert_eq!(
        response_json["data"][0]["revised_prompt"],
        "中国历史视觉海报"
    );
    assert_eq!(response_json["usage"]["input_tokens"], 171);
    assert_eq!(response_json["usage"]["output_tokens"], 1372);

    let seen_refresh_request = seen_refresh
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("refresh request should be captured");
    assert_eq!(
        seen_refresh_request.content_type,
        "application/x-www-form-urlencoded"
    );
    assert!(seen_refresh_request
        .body
        .contains("grant_type=refresh_token"));
    assert!(seen_refresh_request
        .body
        .contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
    assert!(seen_refresh_request
        .body
        .contains("refresh_token=rt-codex-image-local-123"));
    assert_eq!(*refresh_hits.lock().expect("mutex should lock"), 1);

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-codex-image-local-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        seen_execution_runtime_request.model,
        CODEX_OPENAI_IMAGE_INTERNAL_MODEL
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer refreshed-codex-image-access-token"
    );
    assert_eq!(
        seen_execution_runtime_request.x_client_request_id,
        "trace-codex-image-local-123"
    );
    assert_eq!(
        seen_execution_runtime_request.prompt,
        "生成一张中国历史视觉海报"
    );
    assert!(seen_execution_runtime_request.content_is_string);
    assert_eq!(seen_execution_runtime_request.tool_type, "image_generation");
    assert_eq!(seen_execution_runtime_request.tool_size, "1024x1024");
    assert_eq!(seen_execution_runtime_request.tool_quality, "high");
    assert_eq!(seen_execution_runtime_request.tool_background, "auto");
    assert_eq!(
        seen_execution_runtime_request.tool_choice_type,
        "image_generation"
    );
    assert!(!seen_execution_runtime_request.tool_has_n);
    assert!(seen_execution_runtime_request.request_stream);
    assert!(!seen_execution_runtime_request.plan_stream);

    let persisted_transport_state =
        crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
            provider_catalog_repository,
            DEVELOPMENT_ENCRYPTION_KEY,
        );
    let persisted_transport = persisted_transport_state
        .read_provider_transport_snapshot(
            "provider-codex-image-local-1",
            "endpoint-codex-image-local-1",
            "key-codex-image-local-1",
        )
        .await
        .expect("provider transport should read")
        .expect("provider transport should exist");
    assert_eq!(
        persisted_transport.key.decrypted_api_key,
        "refreshed-codex-image-access-token"
    );
    assert!(persisted_transport.key.expires_at_unix_secs.is_some());

    gateway_handle.abort();
    execution_runtime_handle.abort();
    refresh_handle.abort();
}

#[test]
fn gateway_plans_chatgpt_web_image_sync_with_internal_web_executor_url() {
    run_image_sync_test(
        "gateway_plans_chatgpt_web_image_sync_with_internal_web_executor_url",
        gateway_plans_chatgpt_web_image_sync_with_internal_web_executor_url_impl,
    );
}

async fn gateway_plans_chatgpt_web_image_sync_with_internal_web_executor_url_impl() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        trace_id: String,
        url: String,
        marker: String,
        authorization: String,
        operation: String,
        model: String,
        web_model: String,
        prompt: String,
        size: String,
        ratio: String,
    }

    fn hash_api_key(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai", "chatgpt_web"])),
            Some(serde_json::json!(["openai:image"])),
            Some(serde_json::json!(["gpt-image-2"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(serde_json::json!(["openai", "chatgpt_web"])),
            Some(serde_json::json!(["openai:image"])),
            Some(serde_json::json!(["gpt-image-2"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-chatgpt-web-image-plan-1".to_string(),
            provider_name: "ChatGPT Web".to_string(),
            provider_type: "chatgpt_web".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-chatgpt-web-image-plan-1".to_string(),
            endpoint_api_format: "openai:image".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("image".to_string()),
            endpoint_is_active: true,
            key_id: "key-chatgpt-web-image-plan-1".to_string(),
            key_name: "manual bearer".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:image".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:image": 1})),
            model_id: "model-chatgpt-web-image-plan-1".to_string(),
            global_model_id: "global-model-chatgpt-web-image-plan-1".to_string(),
            global_model_name: "gpt-image-2".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-image-2".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-image-2".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:image".to_string()]),
                endpoint_ids: None,
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-chatgpt-web-image-plan-1".to_string(),
            "ChatGPT Web".to_string(),
            Some("https://chatgpt.com".to_string()),
            "chatgpt_web".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            None,
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-chatgpt-web-image-plan-1".to_string(),
            "provider-chatgpt-web-image-plan-1".to_string(),
            "openai:image".to_string(),
            Some("openai".to_string()),
            Some("image".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://chatgpt.com".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-chatgpt-web-image-plan-1".to_string(),
            "provider-chatgpt-web-image-plan-1".to_string(),
            "manual bearer".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:image"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "chatgpt-web-access-token")
                .expect("access token should encrypt"),
            None,
            None,
            Some(serde_json::json!({"openai:image": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                let body = payload
                    .get("body")
                    .and_then(|value| value.get("json_body"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeSyncRequest {
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    url: payload
                        .get("url")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    marker: payload
                        .get("headers")
                        .and_then(|value| value.get("x-aether-chatgpt-web-image"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    authorization: payload
                        .get("headers")
                        .and_then(|value| value.get("authorization"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    operation: body
                        .get("operation")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    model: body
                        .get("model")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    web_model: body
                        .get("web_model")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    prompt: body
                        .get("prompt")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    size: body
                        .get("size")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    ratio: body
                        .get("ratio")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
                Json(json!({
                    "request_id": "trace-chatgpt-web-image-plan-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "text/event-stream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                            concat!(
                                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_chatgpt_web_123\",\"type\":\"image_generation_call\",\"output_format\":\"png\",\"result\":\"aGVsbG8=\"}}\n\n",
                                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_chatgpt_web_123\",\"object\":\"response\",\"model\":\"gpt-image-2\",\"status\":\"completed\",\"output\":[]}}\n\n",
                                "data: [DONE]\n\n"
                            )
                        )
                    },
                    "telemetry": {
                        "elapsed_ms": 41
                    }
                }))
            }
        }),
    );

    let client_api_key = "sk-client-chatgpt-web-image-plan";
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(client_api_key)),
        sample_auth_snapshot(
            "key-chatgpt-web-image-client-123",
            "user-chatgpt-web-image-client-123",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
    ));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::new(InMemoryRequestCandidateRepository::default()),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/generations"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {client_api_key}"),
        )
        .header(TRACE_ID_HEADER, "trace-chatgpt-web-image-plan-123")
        .body("{\"model\":\"gpt-image-2\",\"prompt\":\"生成一张测试图\",\"size\":\"1024x1024\",\"response_format\":\"b64_json\"}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let response_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(response_json["data"][0]["b64_json"], "aGVsbG8=");

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-chatgpt-web-image-plan-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://chatgpt.com/__aether/chatgpt-web-image"
    );
    assert!(!seen_execution_runtime_request.url.contains("/v1/responses"));
    assert_eq!(seen_execution_runtime_request.marker, "1");
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer chatgpt-web-access-token"
    );
    assert_eq!(seen_execution_runtime_request.operation, "generate");
    assert_eq!(seen_execution_runtime_request.model, "gpt-image-2");
    assert_eq!(seen_execution_runtime_request.web_model, "gpt-5-5-thinking");
    assert_eq!(seen_execution_runtime_request.prompt, "生成一张测试图");
    assert_eq!(seen_execution_runtime_request.size, "1024x1024");
    assert_eq!(seen_execution_runtime_request.ratio, "1:1");

    gateway_handle.abort();
    execution_runtime_handle.abort();
}
