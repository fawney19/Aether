use std::sync::{Arc, Mutex};

use aether_contracts::ExecutionPlan;
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, StoredProviderCatalogEndpoint,
};
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use base64::Engine as _;
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, build_state_with_execution_runtime_override,
    sample_admin_provider_model, sample_endpoint, sample_key, sample_provider, start_server,
    AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

const PROVIDER_QUERY_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_provider_query_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(PROVIDER_QUERY_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("provider query test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

fn encode_string_header(name: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(7);
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    out
}

fn encode_frame(headers: Vec<u8>, payload: Vec<u8>) -> Vec<u8> {
    let total_len = 12 + headers.len() + payload.len() + 4;
    let header_len = headers.len();
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&(total_len as u32).to_be_bytes());
    out.extend_from_slice(&(header_len as u32).to_be_bytes());
    let prelude_crc = crc32(&out[..8]);
    out.extend_from_slice(&prelude_crc.to_be_bytes());
    out.extend_from_slice(&headers);
    out.extend_from_slice(&payload);
    let message_crc = crc32(&out);
    out.extend_from_slice(&message_crc.to_be_bytes());
    out
}

fn encode_kiro_event_frame(event_type: &str, payload: serde_json::Value) -> Vec<u8> {
    let mut headers = encode_string_header(":message-type", "event");
    headers.extend_from_slice(&encode_string_header(":event-type", event_type));
    let payload = serde_json::to_vec(&payload).expect("payload should encode");
    encode_frame(headers, payload)
}

fn encode_kiro_exception_frame(exception_type: &str) -> Vec<u8> {
    let mut headers = encode_string_header(":message-type", "exception");
    headers.extend_from_slice(&encode_string_header(":exception-type", exception_type));
    encode_frame(headers, Vec::new())
}

async fn assert_admin_provider_query_route(
    path: &str,
    request_payload: serde_json::Value,
    expected_status: StatusCode,
    expected_payload_assertions: impl FnOnce(&serde_json::Value),
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        path,
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}{path}"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&request_payload)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), expected_status);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    expected_payload_assertions(&payload);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_models_fetches_upstream_for_selected_key() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_models_fetches_upstream_for_selected_key",
        gateway_handles_admin_provider_query_models_fetches_upstream_for_selected_key_impl,
    );
}

async fn gateway_handles_admin_provider_query_models_fetches_upstream_for_selected_key_impl() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(plan.url, "https://api.openai.example/v1/models");
                assert_eq!(
                    plan.headers.get("authorization").map(String::as_str),
                    Some("Bearer sk-test")
                );
                Json(json!({
                    "request_id": "req-provider-query-selected",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "data": [{
                                "id": "LLM-Research/Llama-4-Maverick-17B-128E-Instruct",
                                "object": "",
                                "owned_by": "system",
                                "created": 1732517497u64
                            }]
                        }
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![StoredProviderCatalogEndpoint::new(
            "endpoint-openai-chat".to_string(),
            "provider-openai".to_string(),
            "openai:chat".to_string(),
            Some("chat".to_string()),
            Some("primary".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.example/v1".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")],
        vec![sample_key(
            "key-openai-selected",
            "provider-openai",
            "openai:chat",
            "sk-test",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/models"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "api_key_id": "key-openai-selected"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["provider"]["id"], "provider-openai");
    assert_eq!(payload["provider"]["name"], "OpenAI");
    assert_eq!(payload["provider"]["display_name"], "OpenAI");
    assert_eq!(payload["data"]["error"], serde_json::Value::Null);
    assert_eq!(payload["data"]["from_cache"], json!(false));
    assert_eq!(payload["data"]["keys_total"], serde_json::Value::Null);
    let models = payload["data"]["models"]
        .as_array()
        .expect("models should be an array");
    assert_eq!(models.len(), 1);
    assert_eq!(
        models[0]["id"],
        json!("LLM-Research/Llama-4-Maverick-17B-128E-Instruct")
    );
    assert_eq!(models[0]["owned_by"], json!("system"));
    assert_eq!(models[0]["api_formats"], json!(["openai:chat"]));
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_models_fetches_windsurf_model_configs() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_models_fetches_windsurf_model_configs",
        gateway_handles_admin_provider_query_models_fetches_windsurf_model_configs_impl,
    );
}

async fn gateway_handles_admin_provider_query_models_fetches_windsurf_model_configs_impl() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(plan.method, "POST");
                assert_eq!(
                    plan.url,
                    "https://server.codeium.com/exa.api_server_pb.ApiServerService/GetCascadeModelConfigs"
                );
                assert_eq!(plan.client_api_format, "openai:chat");
                assert_eq!(plan.provider_api_format, "windsurf:model_configs");
                assert_eq!(plan.model_name.as_deref(), Some("GetCascadeModelConfigs"));
                assert_eq!(
                    plan.headers.get("connect-protocol-version").map(String::as_str),
                    Some("1")
                );
                assert_eq!(
                    plan.body
                        .json_body
                        .as_ref()
                        .and_then(|body| body.get("metadata"))
                        .and_then(|metadata| metadata.get("apiKey")),
                    Some(&json!("devin-session-token$abc"))
                );
                Json(json!({
                    "request_id": "req-provider-query-windsurf",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "clientModelConfigs": [{
                                "modelUid": "claude-sonnet-4-6",
                                "label": "Claude Sonnet 4.6",
                                "provider": "anthropic",
                                "supportsImages": true,
                                "creditMultiplier": 4
                            }],
                            "defaultOverrideModelConfig": {
                                "modelUid": "claude-sonnet-4-6"
                            }
                        }
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-windsurf", "Windsurf", 10);
    provider.provider_type = "windsurf".to_string();
    let mut windsurf_key = sample_key(
        "key-windsurf-selected",
        "provider-windsurf",
        "openai:chat",
        "devin-session-token$abc",
    );
    windsurf_key.auth_type = "oauth".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![StoredProviderCatalogEndpoint::new(
            "endpoint-windsurf-chat".to_string(),
            "provider-windsurf".to_string(),
            "openai:chat".to_string(),
            Some("chat".to_string()),
            Some("primary".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://server.codeium.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")],
        vec![windsurf_key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/models"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-windsurf",
            "api_key_id": "key-windsurf-selected"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["data"]["error"], serde_json::Value::Null);
    assert_eq!(payload["data"]["from_cache"], json!(false));
    let models = payload["data"]["models"]
        .as_array()
        .expect("models should be an array");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], json!("claude-sonnet-4-6"));
    assert_eq!(
        models[0]["api_formats"],
        json!(["openai:chat", "openai:responses", "claude:messages"])
    );
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_models_with_openai_responses_endpoint() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_models_with_openai_responses_endpoint",
        gateway_handles_admin_provider_query_models_with_openai_responses_endpoint_impl,
    );
}

async fn gateway_handles_admin_provider_query_models_with_openai_responses_endpoint_impl() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(plan.endpoint_id, "endpoint-openai-responses");
                assert_eq!(plan.provider_api_format, "openai:responses");
                Json(json!({
                    "request_id": "req-provider-query-responses",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "data": [{
                                "id": "gpt-4.1",
                                "object": "model",
                                "owned_by": "system",
                                "created": 1732517497u64
                            }]
                        }
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![StoredProviderCatalogEndpoint::new(
            "endpoint-openai-responses".to_string(),
            "provider-openai".to_string(),
            "openai:responses".to_string(),
            Some("responses".to_string()),
            Some("primary".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.example/v1".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")],
        vec![sample_key(
            "key-openai-responses",
            "provider-openai",
            "openai:responses",
            "sk-test-responses",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/models"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "api_key_id": "key-openai-responses"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["data"]["error"], serde_json::Value::Null);
    assert_eq!(payload["data"]["from_cache"], json!(false));
    assert_eq!(payload["data"]["models"][0]["id"], json!("gpt-4.1"));
    assert_eq!(
        payload["data"]["models"][0]["api_formats"],
        json!(["openai:responses"])
    );
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_models_falls_back_to_codex_preset_when_token_invalidated() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_models_falls_back_to_codex_preset_when_token_invalidated",
        gateway_handles_admin_provider_query_models_falls_back_to_codex_preset_when_token_invalidated_impl,
    );
}

async fn gateway_handles_admin_provider_query_models_falls_back_to_codex_preset_when_token_invalidated_impl(
) {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(
                    plan.url,
                    "https://chatgpt.com/backend-api/codex/models?client_version=0.144.1"
                );
                Json(json!({
                    "request_id": "req-provider-query-codex-invalidated",
                    "status_code": 403,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "error": {
                                "message": "Your authentication token has been invalidated. Please sign in again."
                            }
                        }
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-codex", "Codex", 10);
    provider.provider_type = "codex".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-codex-responses",
            "provider-codex",
            "openai:responses",
            "https://chatgpt.com/backend-api/codex",
        )],
        vec![sample_key(
            "key-codex-invalidated",
            "provider-codex",
            "openai:responses",
            "invalidated-token",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/models"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-codex",
            "api_key_id": "key-codex-invalidated"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["data"]["error"], serde_json::Value::Null);
    let warning = payload["data"]["warning"]
        .as_str()
        .expect("Codex fallback warning should be present");
    assert!(warning.contains("Codex 动态模型目录不可用"));
    assert!(warning.contains("invalidated"));
    let model_ids = payload["data"]["models"]
        .as_array()
        .expect("models should be an array")
        .iter()
        .map(|model| model["id"].as_str().expect("model id"))
        .collect::<Vec<_>>();
    assert_eq!(
        model_ids,
        vec![
            "codex-auto-review",
            "gpt-5.2",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.5",
            "gpt-5.6-luna",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
        ]
    );
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_models_respecting_key_api_formats() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_models_respecting_key_api_formats",
        gateway_handles_admin_provider_query_models_respecting_key_api_formats_impl,
    );
}

async fn gateway_handles_admin_provider_query_models_respecting_key_api_formats_impl() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(plan.endpoint_id, "endpoint-openai-cli");
                assert_eq!(plan.provider_api_format, "openai:responses");
                Json(json!({
                    "request_id": "req-provider-query-cli",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "data": [{
                                "id": "gpt-5-cli",
                                "object": "model",
                                "owned_by": "system",
                                "created": 1732517497u64
                            }]
                        }
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![
            StoredProviderCatalogEndpoint::new(
                "endpoint-openai-chat".to_string(),
                "provider-openai".to_string(),
                "openai:chat".to_string(),
                Some("chat".to_string()),
                Some("primary".to_string()),
                true,
            )
            .expect("endpoint should build")
            .with_transport_fields(
                "https://api.openai.example/v1".to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("endpoint transport should build"),
            StoredProviderCatalogEndpoint::new(
                "endpoint-openai-cli".to_string(),
                "provider-openai".to_string(),
                "openai:responses".to_string(),
                Some("cli".to_string()),
                Some("secondary".to_string()),
                true,
            )
            .expect("endpoint should build")
            .with_transport_fields(
                "https://api.openai.example/v1".to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("endpoint transport should build"),
        ],
        vec![sample_key(
            "key-openai-cli",
            "provider-openai",
            "openai:responses",
            "sk-test-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/models"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "api_key_id": "key-openai-cli"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["data"]["error"], serde_json::Value::Null);
    assert_eq!(payload["data"]["from_cache"], json!(false));
    assert_eq!(
        payload["data"]["models"][0]["api_formats"],
        json!(["openai:responses"])
    );
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_models_aggregating_active_keys() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_models_aggregating_active_keys",
        gateway_handles_admin_provider_query_models_aggregating_active_keys_impl,
    );
}

async fn gateway_handles_admin_provider_query_models_aggregating_active_keys_impl() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(plan.url, "https://api.openai.example/v1/models");
                let auth = plan
                    .headers
                    .get("authorization")
                    .map(String::as_str)
                    .unwrap_or_default()
                    .to_string();
                let body = if auth == "Bearer sk-test-1" {
                    json!({
                        "data": [{
                            "id": "gpt-5",
                            "api_formats": ["openai:chat"],
                            "object": "model",
                            "owned_by": "system",
                            "created": 1732517497u64
                        }]
                    })
                } else {
                    json!({
                        "data": [{
                            "id": "gpt-4.1",
                            "api_formats": ["openai:chat"],
                            "object": "model",
                            "owned_by": "system",
                            "created": 1732517498u64
                        }]
                    })
                };
                Json(json!({
                    "request_id": format!("req-provider-query-{auth}"),
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": body
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![StoredProviderCatalogEndpoint::new(
            "endpoint-openai-chat".to_string(),
            "provider-openai".to_string(),
            "openai:chat".to_string(),
            Some("chat".to_string()),
            Some("primary".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.example/v1".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")],
        vec![
            sample_key(
                "key-openai-1",
                "provider-openai",
                "openai:chat",
                "sk-test-1",
            ),
            sample_key(
                "key-openai-2",
                "provider-openai",
                "openai:chat",
                "sk-test-2",
            ),
        ],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/models"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["data"]["from_cache"], json!(false));
    assert_eq!(payload["data"]["keys_total"], json!(2));
    assert_eq!(payload["data"]["keys_cached"], json!(0));
    assert_eq!(payload["data"]["keys_fetched"], json!(2));
    let models = payload["data"]["models"]
        .as_array()
        .expect("models should be an array");
    assert_eq!(models.len(), 2);
    let model_ids = models
        .iter()
        .map(|model| model["id"].as_str().expect("id should exist"))
        .collect::<Vec<_>>();
    assert_eq!(model_ids, vec!["gpt-4.1", "gpt-5"]);
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        2
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_models_for_fixed_provider_without_endpoint() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_models_for_fixed_provider_without_endpoint",
        gateway_handles_admin_provider_query_models_for_fixed_provider_without_endpoint_impl,
    );
}

async fn gateway_handles_admin_provider_query_models_for_fixed_provider_without_endpoint_impl() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                Json(json!({
                    "request_id": "unexpected",
                    "status_code": 500
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-codex", "Codex", 10);
    provider.provider_type = "codex".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![sample_key(
            "key-codex-oauth",
            "provider-codex",
            "openai:responses",
            "sk-test-codex",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/models"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-codex",
            "api_key_id": "key-codex-oauth"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["data"]["error"], serde_json::Value::Null);
    assert_eq!(payload["data"]["from_cache"], json!(false));
    let models = payload["data"]["models"]
        .as_array()
        .expect("models should be an array");
    assert!(models.iter().any(|model| model["id"] == "gpt-5.4"));
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        0
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_test_model_locally_with_trusted_admin_principal() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_test_model_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_query_test_model_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_query_test_model_locally_with_trusted_admin_principal_impl()
{
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-openai");
            assert_eq!(plan.endpoint_id, "endpoint-openai-chat");
            assert_eq!(plan.key_id, "key-openai-primary");
            assert_eq!(plan.provider_api_format, "openai:chat");
            assert_eq!(plan.model_name.as_deref(), Some("gpt-4.1"));
            assert!(!plan.stream);
            assert_eq!(
                plan.headers.get("content-type").map(String::as_str),
                Some("application/json")
            );
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer sk-test-primary")
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("model")),
                Some(&json!("gpt-4.1"))
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-test-model",
                        "object": "chat.completion",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Hello from OpenAI"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 18
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example/v1",
        )],
        vec![sample_key(
            "key-openai-primary",
            "provider-openai",
            "openai:chat",
            "sk-test-primary",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "model": "gpt-4.1",
            "api_format": "openai:chat"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["provider"]["id"], json!("provider-openai"));
    assert_eq!(payload["model"], json!("gpt-4.1"));
    assert_eq!(payload["error"], serde_json::Value::Null);
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from OpenAI")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_embedding_model_test() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_embedding_model_test",
        gateway_handles_admin_provider_query_embedding_model_test_impl,
    );
}

async fn gateway_handles_admin_provider_query_embedding_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-siliconflow");
            assert_eq!(plan.endpoint_id, "endpoint-siliconflow-embedding");
            assert_eq!(plan.key_id, "key-siliconflow-embedding");
            assert_eq!(plan.client_api_format, "openai:embedding");
            assert_eq!(plan.provider_api_format, "openai:embedding");
            assert_eq!(plan.url, "https://api.siliconflow.example/embeddings");
            assert_eq!(plan.model_name.as_deref(), Some("Qwen/Qwen3-Embedding-4B"));
            assert!(!plan.stream);
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer sk-siliconflow-embedding")
            );
            let body = plan.body.json_body.as_ref().expect("json body");
            assert_eq!(body["model"], json!("Qwen/Qwen3-Embedding-4B"));
            assert_eq!(body["input"], json!("This is a test embedding input."));
            assert!(
                body.get("stream").is_none(),
                "embedding provider body must not carry stream"
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "object": "list",
                        "model": "Qwen/Qwen3-Embedding-4B",
                        "data": [{
                            "object": "embedding",
                            "index": 0,
                            "embedding": [0.1, 0.2, 0.3]
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 24
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-siliconflow", "SiliconFlow", 10);
    provider.provider_type = "custom".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-siliconflow-embedding",
            "provider-siliconflow",
            "openai:embedding",
            "https://api.siliconflow.example",
        )],
        vec![sample_key(
            "key-siliconflow-embedding",
            "provider-siliconflow",
            "openai:embedding",
            "sk-siliconflow-embedding",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-siliconflow",
            "model": "Qwen/Qwen3-Embedding-4B",
            "api_format": "openai:embedding",
            "endpoint_id": "endpoint-siliconflow-embedding",
            "request_body": {
                "model": "Qwen/Qwen3-Embedding-4B",
                "input": "This is a test embedding input."
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["error"], serde_json::Value::Null);
    assert_eq!(payload["attempts"][0]["status"], json!("success"));
    assert_eq!(
        payload["attempts"][0]["response_body"]["data"][0]["object"],
        json!("embedding")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_doubao_text_embedding_model_test() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_doubao_text_embedding_model_test",
        gateway_handles_admin_provider_query_doubao_text_embedding_model_test_impl,
    );
}

async fn gateway_handles_admin_provider_query_doubao_text_embedding_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-doubao");
            assert_eq!(plan.endpoint_id, "endpoint-doubao-embedding");
            assert_eq!(plan.key_id, "key-doubao-embedding");
            assert_eq!(plan.client_api_format, "openai:embedding");
            assert_eq!(plan.provider_api_format, "doubao:embedding");
            assert_eq!(plan.url, "https://ark.volces.example/api/v3/embeddings");
            assert_eq!(
                plan.model_name.as_deref(),
                Some("doubao-embedding-text-240515")
            );
            assert!(!plan.stream);
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer sk-doubao-embedding")
            );
            let body = plan.body.json_body.as_ref().expect("json body");
            assert_eq!(body["model"], json!("doubao-embedding-text-240515"));
            assert_eq!(body["input"], json!(["This is a test embedding input."]));
            assert!(
                body.get("stream").is_none(),
                "doubao embedding provider body must not carry stream"
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "object": "list",
                        "model": "doubao-embedding-text-240515",
                        "data": [{
                            "object": "embedding",
                            "index": 0,
                            "embedding": [0.1, 0.2, 0.3]
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 28
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-doubao", "Doubao", 10);
    provider.provider_type = "doubao".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-doubao-embedding",
            "provider-doubao",
            "doubao:embedding",
            "https://ark.volces.example/api/v3",
        )],
        vec![sample_key(
            "key-doubao-embedding",
            "provider-doubao",
            "doubao:embedding",
            "sk-doubao-embedding",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-doubao",
            "model": "doubao-embedding-text-240515",
            "api_format": "doubao:embedding",
            "endpoint_id": "endpoint-doubao-embedding",
            "request_body": {
                "model": "doubao-embedding-text-240515",
                "input": "This is a test embedding input."
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["error"], serde_json::Value::Null);
    assert_eq!(payload["attempts"][0]["status"], json!("success"));
    assert_eq!(
        payload["attempts"][0]["request_body"]["input"],
        json!(["This is a test embedding input."])
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_gemini_embedding_model_test() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_gemini_embedding_model_test",
        gateway_handles_admin_provider_query_gemini_embedding_model_test_impl,
    );
}

async fn gateway_handles_admin_provider_query_gemini_embedding_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-gemini");
            assert_eq!(plan.endpoint_id, "endpoint-gemini-embedding");
            assert_eq!(plan.key_id, "key-gemini-embedding");
            assert_eq!(plan.client_api_format, "openai:embedding");
            assert_eq!(plan.provider_api_format, "gemini:embedding");
            assert_eq!(
                plan.url,
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:embedContent"
            );
            assert_eq!(plan.model_name.as_deref(), Some("gemini-embedding-001"));
            assert!(!plan.stream);
            assert_eq!(
                plan.headers.get("x-goog-api-key").map(String::as_str),
                Some("sk-gemini-embedding")
            );
            let body = plan.body.json_body.as_ref().expect("json body");
            assert_eq!(body["model"], json!("gemini-embedding-001"));
            assert_eq!(
                body["content"]["parts"][0]["text"],
                json!("This is a test embedding input.")
            );
            assert!(body.get("requests").is_none());
            assert!(
                body.get("stream").is_none(),
                "gemini embedding provider body must not carry stream"
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "model": "gemini-embedding-001",
                        "embedding": {
                            "values": [0.1, 0.2, 0.3]
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 27
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-gemini", "Gemini", 10);
    provider.provider_type = "gemini".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-embedding",
            "provider-gemini",
            "gemini:embedding",
            "https://generativelanguage.googleapis.com/v1beta",
        )],
        vec![sample_key(
            "key-gemini-embedding",
            "provider-gemini",
            "gemini:embedding",
            "sk-gemini-embedding",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-gemini",
            "model": "gemini-embedding-001",
            "api_format": "gemini:embedding",
            "endpoint_id": "endpoint-gemini-embedding",
            "request_body": {
                "model": "gemini-embedding-001",
                "input": "This is a test embedding input."
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["error"], serde_json::Value::Null);
    assert_eq!(payload["attempts"][0]["status"], json!("success"));
    assert_eq!(
        payload["attempts"][0]["response_body"]["embedding"]["values"][0],
        json!(0.1)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_vertex_gemini_embedding_model_test() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_vertex_gemini_embedding_model_test",
        gateway_handles_admin_provider_query_vertex_gemini_embedding_model_test_impl,
    );
}

async fn gateway_handles_admin_provider_query_vertex_gemini_embedding_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-vertex-ai");
            assert_eq!(plan.endpoint_id, "endpoint-vertex-gemini-embedding");
            assert_eq!(plan.key_id, "key-vertex-gemini-embedding");
            assert_eq!(plan.client_api_format, "openai:embedding");
            assert_eq!(plan.provider_api_format, "gemini:embedding");
            assert_eq!(
                plan.url,
                "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-embedding-2:predict?key=sk-vertex-gemini-embedding"
            );
            assert_eq!(plan.model_name.as_deref(), Some("gemini-embedding-2"));
            assert!(!plan.stream);
            let body = plan.body.json_body.as_ref().expect("json body");
            assert!(
                body.get("model").is_none(),
                "Vertex predict carries the model in the URL path; the test body must not repeat it"
            );
            assert_eq!(
                body["instances"][0]["content"],
                json!("This is a test embedding input.")
            );
            assert!(body.get("content").is_none());
            assert!(body.get("requests").is_none());
            assert!(
                body.get("stream").is_none(),
                "gemini embedding provider body must not carry stream"
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "predictions": [
                            {
                                "embeddings": {
                                    "values": [0.1, 0.2, 0.3]
                                }
                            }
                        ],
                        "deployedModelId": "gemini-embedding-2"
                    }
                },
                "telemetry": {
                    "elapsed_ms": 27
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-vertex-ai", "Vertex AI", 10);
    provider.provider_type = "vertex_ai".to_string();
    let mut key = sample_key(
        "key-vertex-gemini-embedding",
        "provider-vertex-ai",
        "gemini:embedding",
        "sk-vertex-gemini-embedding",
    );
    key.allowed_models = Some(json!(["gemini-embedding-2"]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-vertex-gemini-embedding",
            "provider-vertex-ai",
            "gemini:embedding",
            "https://aiplatform.googleapis.com",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-vertex-ai",
            "model": "gemini-embedding-2",
            "api_format": "gemini:embedding",
            "endpoint_id": "endpoint-vertex-gemini-embedding",
            "request_body": {
                "model": "gemini-embedding-2",
                "input": "This is a test embedding input."
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["error"], serde_json::Value::Null);
    assert_eq!(payload["attempts"][0]["status"], json!("success"));
    assert_eq!(
        payload["attempts"][0]["request_body"]["instances"][0]["content"],
        json!("This is a test embedding input.")
    );
    assert_eq!(
        payload["attempts"][0]["endpoint_product"],
        json!("Vertex AI")
    );
    assert_eq!(
        payload["attempts"][0]["endpoint_variant"],
        json!("vertex_native")
    );
    assert_eq!(payload["attempts"][0]["endpoint_action"], json!("predict"));
    assert_eq!(
        payload["attempts"][0]["endpoint_batch_strategy"],
        json!("single_instance")
    );
    assert!(
        payload["attempts"][0]["request_body"]
            .get("model")
            .is_none(),
        "attempt debug payload must expose the exact Vertex body without a duplicate model"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_jina_embedding_model_test() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_jina_embedding_model_test",
        gateway_handles_admin_provider_query_jina_embedding_model_test_impl,
    );
}

async fn gateway_handles_admin_provider_query_jina_embedding_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-jina-embedding");
            assert_eq!(plan.endpoint_id, "endpoint-jina-embedding");
            assert_eq!(plan.key_id, "key-jina-embedding");
            assert_eq!(plan.client_api_format, "openai:embedding");
            assert_eq!(plan.provider_api_format, "jina:embedding");
            assert_eq!(plan.url, "https://api.jina.example/embeddings");
            assert_eq!(plan.model_name.as_deref(), Some("jina-embeddings-v3"));
            assert!(!plan.stream);
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer sk-jina-embedding")
            );
            let body = plan.body.json_body.as_ref().expect("json body");
            assert_eq!(body["model"], json!("jina-embeddings-v3"));
            assert_eq!(body["task"], json!("text-matching"));
            assert_eq!(body["input"], json!("This is a test embedding input."));
            assert!(
                body.get("stream").is_none(),
                "jina embedding provider body must not carry stream"
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "model": "jina-embeddings-v3",
                        "data": [{
                            "object": "embedding",
                            "index": 0,
                            "embedding": [0.1, 0.2, 0.3]
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 29
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-jina-embedding", "Jina", 10);
    provider.provider_type = "jina".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-jina-embedding",
            "provider-jina-embedding",
            "jina:embedding",
            "https://api.jina.example",
        )],
        vec![sample_key(
            "key-jina-embedding",
            "provider-jina-embedding",
            "jina:embedding",
            "sk-jina-embedding",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-jina-embedding",
            "model": "jina-embeddings-v3",
            "api_format": "jina:embedding",
            "endpoint_id": "endpoint-jina-embedding",
            "request_body": {
                "model": "jina-embeddings-v3",
                "input": "This is a test embedding input."
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["error"], serde_json::Value::Null);
    assert_eq!(payload["attempts"][0]["status"], json!("success"));
    assert_eq!(
        payload["attempts"][0]["request_body"]["task"],
        json!("text-matching")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_openai_rerank_model_test() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_openai_rerank_model_test",
        gateway_handles_admin_provider_query_openai_rerank_model_test_impl,
    );
}

async fn gateway_handles_admin_provider_query_openai_rerank_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-openai-rerank");
            assert_eq!(plan.endpoint_id, "endpoint-openai-rerank");
            assert_eq!(plan.key_id, "key-openai-rerank");
            assert_eq!(plan.client_api_format, "openai:rerank");
            assert_eq!(plan.provider_api_format, "openai:rerank");
            assert_eq!(plan.url, "https://api.openai.example/v1/rerank");
            assert_eq!(plan.model_name.as_deref(), Some("bge-reranker-base"));
            assert!(!plan.stream);
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer sk-openai-rerank")
            );
            let body = plan.body.json_body.as_ref().expect("json body");
            assert_eq!(body["model"], json!("bge-reranker-base"));
            assert_eq!(body["query"], json!("Apple"));
            assert_eq!(
                body["documents"],
                json!(["apple", "banana", "fruit", "vegetable"])
            );
            assert_eq!(body["return_documents"], json!(true));
            assert_eq!(body["top_n"], json!(4));
            assert!(
                body.get("stream").is_none(),
                "openai rerank provider body must not carry stream"
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "model": "bge-reranker-base",
                        "results": [{
                            "index": 0,
                            "relevance_score": 0.91
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 32
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai-rerank", "OpenAI Rerank", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-rerank",
            "provider-openai-rerank",
            "openai:rerank",
            "https://api.openai.example/v1",
        )],
        vec![sample_key(
            "key-openai-rerank",
            "provider-openai-rerank",
            "openai:rerank",
            "sk-openai-rerank",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai-rerank",
            "model": "bge-reranker-base",
            "api_format": "openai:rerank",
            "endpoint_id": "endpoint-openai-rerank",
            "request_body": {
                "model": "bge-reranker-base",
                "query": "Apple",
                "documents": ["apple", "banana", "fruit", "vegetable"],
                "return_documents": true,
                "top_n": 4
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["error"], serde_json::Value::Null);
    assert_eq!(payload["attempts"][0]["status"], json!("success"));
    assert_eq!(
        payload["attempts"][0]["response_body"]["results"][0]["relevance_score"],
        json!(0.91)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_rerank_model_test() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_rerank_model_test",
        gateway_handles_admin_provider_query_rerank_model_test_impl,
    );
}

async fn gateway_handles_admin_provider_query_rerank_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-jina");
            assert_eq!(plan.endpoint_id, "endpoint-jina-rerank");
            assert_eq!(plan.key_id, "key-jina-rerank");
            assert_eq!(plan.client_api_format, "openai:rerank");
            assert_eq!(plan.provider_api_format, "jina:rerank");
            assert_eq!(plan.url, "https://api.jina.example/rerank");
            assert_eq!(
                plan.model_name.as_deref(),
                Some("jina-reranker-v2-base-multilingual")
            );
            assert!(!plan.stream);
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer sk-jina-rerank")
            );
            let body = plan.body.json_body.as_ref().expect("json body");
            assert_eq!(body["model"], json!("jina-reranker-v2-base-multilingual"));
            assert_eq!(body["query"], json!("Apple"));
            assert_eq!(
                body["documents"],
                json!(["apple", "banana", "fruit", "vegetable"])
            );
            assert_eq!(body["return_documents"], json!(true));
            assert_eq!(body["top_n"], json!(4));
            assert!(
                body.get("stream").is_none(),
                "rerank provider body must not carry stream"
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "model": "jina-reranker-v2-base-multilingual",
                        "results": [{
                            "index": 0,
                            "relevance_score": 0.93
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 31
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-jina", "Jina", 10);
    provider.provider_type = "jina".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-jina-rerank",
            "provider-jina",
            "jina:rerank",
            "https://api.jina.example",
        )],
        vec![sample_key(
            "key-jina-rerank",
            "provider-jina",
            "jina:rerank",
            "sk-jina-rerank",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-jina",
            "model": "jina-reranker-v2-base-multilingual",
            "api_format": "jina:rerank",
            "endpoint_id": "endpoint-jina-rerank",
            "request_body": {
                "model": "jina-reranker-v2-base-multilingual",
                "query": "Apple",
                "documents": ["apple", "banana", "fruit", "vegetable"],
                "return_documents": true,
                "top_n": 4
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["error"], serde_json::Value::Null);
    assert_eq!(payload["attempts"][0]["status"], json!("success"));
    assert_eq!(
        payload["attempts"][0]["response_body"]["results"][0]["relevance_score"],
        json!(0.93)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_maps_admin_provider_model_before_model_list_test_request() {
    run_provider_query_test(
        "gateway_maps_admin_provider_model_before_model_list_test_request",
        gateway_maps_admin_provider_model_before_model_list_test_request_impl,
    );
}

async fn gateway_maps_admin_provider_model_before_model_list_test_request_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-minimax");
            assert_eq!(plan.endpoint_id, "endpoint-minimax-chat");
            assert_eq!(plan.key_id, "key-minimax-primary");
            assert_eq!(plan.provider_api_format, "openai:chat");
            let requested_model = plan
                .model_name
                .as_deref()
                .expect("test plan should carry model name");
            assert!(
                matches!(
                    requested_model,
                    "MiniMax-M2.7-highspeed" | "MiniMax-M2.7-balanced" | "claude-opus-4-6"
                ),
                "unexpected requested model: {requested_model}"
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("model")),
                Some(&json!(requested_model))
            );
            let response_model = if requested_model.starts_with("MiniMax-M2.7") {
                "MiniMax-M2.7"
            } else {
                requested_model
            };
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-minimax-mapped-model",
                        "object": "chat.completion",
                        "model": response_model,
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Hello from MiniMax"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 22
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-minimax", "MiniMax", 10);
    provider.provider_type = "custom".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-minimax-chat",
            "provider-minimax",
            "openai:chat",
            "https://api.minimax.example",
        )],
        vec![sample_key(
            "key-minimax-primary",
            "provider-minimax",
            "openai:chat",
            "sk-minimax-primary",
        )],
    ));
    let mut provider_model = sample_admin_provider_model(
        "model-minimax-claude-opus",
        "provider-minimax",
        "global-claude-opus-4-6",
        "claude-opus-4-6",
    );
    provider_model.global_model_name = Some("claude-opus-4-6".to_string());
    provider_model.global_model_display_name = Some("Claude Opus 4.6".to_string());
    provider_model.provider_model_mappings = Some(json!([
        {
            "name": "ignored-anthropic-model",
            "priority": 1,
            "api_formats": ["anthropic:messages"],
        },
        {
            "name": "MiniMax-M2.7-highspeed",
            "priority": 2,
            "api_formats": ["openai:chat"],
            "endpoint_ids": ["endpoint-minimax-chat"],
        },
        {
            "name": "MiniMax-M2.7-balanced",
            "priority": 3,
            "api_formats": ["openai:chat"],
            "endpoint_ids": ["endpoint-minimax-chat"],
        }
    ]));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_provider_models(vec![provider_model]),
    );

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_transport_reader_for_tests(
                    provider_catalog_repository,
                    DEVELOPMENT_ENCRYPTION_KEY.to_string(),
                )
                .with_global_model_repository_for_tests(global_model_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-minimax",
            "model": "claude-opus-4-6",
            "api_format": "openai:chat",
            "endpoint_id": "endpoint-minimax-chat"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["model"], json!("claude-opus-4-6"));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from MiniMax")
    );
    assert_eq!(
        payload["attempts"][0]["effective_model"],
        json!("MiniMax-M2.7-highspeed")
    );
    assert_eq!(
        payload["attempts"][0]["request_body"]["model"],
        json!("MiniMax-M2.7-highspeed")
    );
    assert_eq!(
        payload["attempts"][0]["response_body"]["model"],
        json!("MiniMax-M2.7")
    );

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-minimax",
            "model": "claude-opus-4-6",
            "api_format": "openai:chat",
            "endpoint_id": "endpoint-minimax-chat",
            "mapped_model_name": "MiniMax-M2.7-balanced",
            "request_body": {
                "model": "stale-model-from-ui",
                "messages": [{
                    "role": "user",
                    "content": "custom prompt"
                }]
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["attempts"][0]["effective_model"],
        json!("MiniMax-M2.7-balanced")
    );
    assert_eq!(
        payload["attempts"][0]["request_body"]["model"],
        json!("MiniMax-M2.7-balanced")
    );

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-minimax",
            "model": "claude-opus-4-6",
            "api_format": "openai:chat",
            "endpoint_id": "endpoint-minimax-chat",
            "mapped_model_name": "ignored-anthropic-model"
        }))
        .send()
        .await
        .expect("request should fail");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-minimax",
            "mode": "global",
            "model": "claude-opus-4-6",
            "failover_models": ["claude-opus-4-6"],
            "api_format": "openai:chat",
            "endpoint_id": "endpoint-minimax-chat",
            "apply_model_mapping": false
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["model"], json!("claude-opus-4-6"));
    assert_eq!(
        payload["attempts"][0]["effective_model"],
        json!("claude-opus-4-6")
    );
    assert_eq!(
        payload["attempts"][0]["request_body"]["model"],
        json!("claude-opus-4-6")
    );
    assert_eq!(
        payload["attempts"][0]["response_body"]["model"],
        json!("claude-opus-4-6")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_streams_codex_openai_responses_upstream_for_admin_pool_model_test() {
    run_provider_query_test(
        "gateway_streams_codex_openai_responses_upstream_for_admin_pool_model_test",
        gateway_streams_codex_openai_responses_upstream_for_admin_pool_model_test_impl,
    );
}

async fn gateway_streams_codex_openai_responses_upstream_for_admin_pool_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-codex");
            assert_eq!(plan.endpoint_id, "endpoint-codex-responses");
            assert_eq!(plan.key_id, "key-codex-primary");
            assert_eq!(plan.provider_api_format, "openai:responses");
            assert_eq!(plan.model_name.as_deref(), Some("gpt-5.3-codex-spark"));
            assert!(plan.stream, "Codex openai:responses must stream upstream");
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("stream")),
                Some(&json!(true))
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "resp-codex-model-test",
                        "model": "gpt-5.3-codex-spark",
                        "output_text": "ok"
                    }
                },
                "telemetry": {
                    "elapsed_ms": 18
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-codex", "Codex", 10);
    provider.provider_type = "codex".to_string();
    let mut endpoint = sample_endpoint(
        "endpoint-codex-responses",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );
    endpoint.config = Some(json!({"upstream_stream_policy": "force_stream"}));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![sample_key(
            "key-codex-primary",
            "provider-codex",
            "openai:responses",
            "sk-codex-primary",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-codex",
            "mode": "pool",
            "model": "gpt-5.3-codex-spark",
            "failover_models": ["gpt-5.3-codex-spark"],
            "api_format": "openai:responses",
            "endpoint_id": "endpoint-codex-responses",
            "request_body": {
                "model": "gpt-5.3-codex-spark",
                "input": "hello",
                "stream": true
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["attempts"][0]["request_body"]["stream"],
        json!(true)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_executes_codex_search_admin_pool_model_test_with_search_contract() {
    run_provider_query_test(
        "gateway_executes_codex_search_admin_pool_model_test_with_search_contract",
        gateway_executes_codex_search_admin_pool_model_test_with_search_contract_impl,
    );
}

async fn gateway_executes_codex_search_admin_pool_model_test_with_search_contract_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-codex-search");
            assert_eq!(plan.endpoint_id, "endpoint-codex-search");
            assert_eq!(plan.key_id, "key-codex-search");
            assert_eq!(plan.client_api_format, "openai:search");
            assert_eq!(plan.provider_api_format, "openai:search");
            assert_eq!(
                plan.url,
                "https://chatgpt.com/backend-api/codex/alpha/search"
            );
            assert_eq!(plan.model_name.as_deref(), Some("gpt-5.6-sol"));
            assert!(!plan.stream, "Codex Search is a synchronous JSON protocol");
            assert_eq!(
                plan.timeouts
                    .as_ref()
                    .and_then(|timeouts| timeouts.total_ms),
                Some(900_000)
            );
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer codex-search-access-token")
            );
            assert_eq!(
                plan.headers.get("chatgpt-account-id").map(String::as_str),
                Some("account-search-admin")
            );
            assert_eq!(
                plan.headers.get("x-openai-fedramp").map(String::as_str),
                Some("true")
            );
            assert_eq!(
                plan.headers.get("originator").map(String::as_str),
                Some("codex_cli_rs")
            );
            assert!(plan
                .headers
                .get("user-agent")
                .is_some_and(|value| value.starts_with("codex_cli_rs/")));
            assert!(!plan.headers.contains_key("openai-beta"));
            assert!(!plan
                .headers
                .contains_key("x-openai-internal-codex-responses-lite"));
            assert_ne!(
                plan.headers.get("accept").map(String::as_str),
                Some("text/event-stream")
            );

            let body = plan.body.json_body.as_ref().expect("search json body");
            assert_eq!(
                body["id"],
                json!("aether-model-test-provider-query-search-trace")
            );
            assert_eq!(body["model"], json!("gpt-5.6-sol"));
            assert_eq!(body["input"], json!("find current OpenAI documentation"));
            assert_eq!(
                body["commands"]["search_query"][0]["q"],
                json!("OpenAI Codex Search")
            );
            assert!(body.get("stream").is_none());
            assert!(body.get("store").is_none());
            assert!(body.get("service_tier").is_none());
            assert!(body.get("unknown_field").is_none());

            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "output": "search result"
                    }
                },
                "telemetry": {
                    "elapsed_ms": 21
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-codex-search", "Codex Search", 10);
    provider.provider_type = "codex".to_string();
    provider.request_timeout_secs = Some(900.0);
    let mut endpoint = sample_endpoint(
        "endpoint-codex-search",
        "provider-codex-search",
        "openai:search",
        "https://chatgpt.com/backend-api/codex",
    );
    endpoint.config = Some(json!({"upstream_stream_policy": "force_stream"}));
    let mut key = sample_key(
        "key-codex-search",
        "provider-codex-search",
        "openai:search",
        "codex-search-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","account_id":"account-search-admin","is_fedramp":true}"#,
        )
        .expect("auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-codex-search",
            "mode": "pool",
            "model": "gpt-5.6-sol",
            "failover_models": ["gpt-5.6-sol"],
            "api_format": "openai:search",
            "endpoint_id": "endpoint-codex-search",
            "request_id": "provider-query-search-trace",
            "request_body": {
                "model": "gpt-5.6-sol",
                "input": "find current OpenAI documentation",
                "commands": {
                    "search_query": [{"q": "OpenAI Codex Search"}]
                },
                "max_output_tokens": 256,
                "stream": true,
                "store": false,
                "service_tier": "priority",
                "unknown_field": true
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true), "payload={payload}");
    assert_eq!(
        payload["attempts"][0]["request_body"]["id"],
        json!("aether-model-test-provider-query-search-trace")
    );
    assert_eq!(
        payload["attempts"][0]["response_body"]["output"],
        json!("search result")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_routes_grok_responses_admin_pool_model_test_through_grok_runtime() {
    run_provider_query_test(
        "gateway_routes_grok_responses_admin_pool_model_test_through_grok_runtime",
        gateway_routes_grok_responses_admin_pool_model_test_through_grok_runtime_impl,
    );
}

async fn gateway_routes_grok_responses_admin_pool_model_test_through_grok_runtime_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-grok");
            assert_eq!(plan.endpoint_id, "endpoint-grok-responses");
            assert_eq!(plan.key_id, "key-grok-oauth");
            assert_eq!(plan.client_api_format, "openai:responses");
            assert_eq!(plan.provider_api_format, "openai:responses");
            assert_eq!(plan.url, "https://grok.com/rest/app-chat/conversations/new");
            assert_eq!(plan.model_name.as_deref(), Some("grok-4.20-fast"));
            assert!(plan.stream, "Grok model test should request a stream");
            assert_eq!(
                plan.headers
                    .get(aether_provider_transport::GROK_INTERNAL_HEADER)
                    .map(String::as_str),
                Some("1")
            );
            assert_eq!(
                plan.headers.get("cookie").map(String::as_str),
                Some("sso=grok-sso; sso-rw=grok-rw")
            );
            let body = plan.body.json_body.as_ref().expect("json body");
            assert_eq!(body["model"], json!("grok-4.20-fast"));
            assert_eq!(body["input"], json!("Hello! This is a test message."));
            assert_eq!(
                body["messages"][0]["content"],
                json!("stale chat-shaped frontend body")
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "resp-grok-model-test",
                        "model": "grok-4.20-fast",
                        "output_text": "ok"
                    }
                },
                "telemetry": {
                    "elapsed_ms": 18
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-grok", "Grok", 10);
    provider.provider_type = "grok".to_string();
    provider.config = Some(json!({"pool_advanced": {}}));
    let mut key = sample_key(
        "key-grok-oauth",
        "provider-grok",
        "openai:responses",
        "__placeholder__",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{
                "provider_type":"grok",
                "sso_token":"grok-sso",
                "sso_rw_token":"grok-rw"
            }"#,
        )
        .expect("auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-grok-responses",
            "provider-grok",
            "openai:responses",
            "https://grok.com",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-grok",
            "mode": "pool",
            "model": "grok-4.20-fast",
            "failover_models": ["grok-4.20-fast"],
            "api_format": "openai:responses",
            "endpoint_id": "endpoint-grok-responses",
            "request_body": {
                "model": "grok-4.20-fast",
                "messages": [{
                    "role": "user",
                    "content": "stale chat-shaped frontend body"
                }],
                "stream": true
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["attempts"][0]["status"], json!("success"));
    assert_eq!(
        payload["attempts"][0]["request_body"]["message"],
        json!("Hello! This is a test message.")
    );
    assert_eq!(
        payload["attempts"][0]["request_headers"][aether_provider_transport::GROK_INTERNAL_HEADER],
        json!("1")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_streams_windsurf_connect_upstream_for_admin_model_test() {
    run_provider_query_test(
        "gateway_streams_windsurf_connect_upstream_for_admin_model_test",
        gateway_streams_windsurf_connect_upstream_for_admin_model_test_impl,
    );
}

async fn gateway_streams_windsurf_connect_upstream_for_admin_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-windsurf");
            assert_eq!(plan.endpoint_id, "endpoint-windsurf-chat");
            assert_eq!(plan.key_id, "key-windsurf-primary");
            assert_eq!(plan.provider_api_format, "openai:chat");
            assert_eq!(plan.content_type.as_deref(), Some("application/connect+json"));
            assert!(plan.stream, "Windsurf Connect model test must stream upstream");
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("stream")),
                Some(&json!(true))
            );
            let windsurf_payload = serde_json::to_vec(&json!({
                "chatMessage": {
                    "text": "ok"
                }
            }))
            .expect("windsurf payload should encode");
            let mut windsurf_frame = vec![0u8];
            windsurf_frame.extend_from_slice(&(windsurf_payload.len() as u32).to_be_bytes());
            windsurf_frame.extend_from_slice(&windsurf_payload);
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/connect+json"
                },
                "body": {
                    "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(windsurf_frame)
                },
                "telemetry": {
                    "elapsed_ms": 24
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-windsurf", "Windsurf", 10);
    provider.provider_type = "windsurf".to_string();
    let mut key = sample_key(
        "key-windsurf-primary",
        "provider-windsurf",
        "openai:chat",
        "devin-session-token$abc",
    );
    key.auth_type = "oauth".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-windsurf-chat",
            "provider-windsurf",
            "openai:chat",
            "https://server.codeium.com",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-windsurf",
            "model": "claude-opus-4-7-medium",
            "api_format": "openai:chat",
            "endpoint_id": "endpoint-windsurf-chat",
            "request_body": {
                "model": "claude-opus-4-7-medium",
                "messages": [{
                    "role": "user",
                    "content": "Hello! This is a test message."
                }],
                "max_tokens": 30,
                "temperature": 0.7,
                "stream": true
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["attempts"][0]["request_body"]["stream"],
        json!(true)
    );
    assert_eq!(
        payload["attempts"][0]["response_body"]["choices"][0]["message"]["content"],
        json!("ok")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_uses_pool_scheduler_order_for_admin_pool_model_test() {
    run_provider_query_test(
        "gateway_uses_pool_scheduler_order_for_admin_pool_model_test",
        gateway_uses_pool_scheduler_order_for_admin_pool_model_test_impl,
    );
}

async fn gateway_uses_pool_scheduler_order_for_admin_pool_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.key_id, "key-codex-plus");
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "resp-codex-plus",
                        "model": "gpt-5.4-mini",
                        "output_text": "ok"
                    }
                },
                "telemetry": {
                    "elapsed_ms": 21
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-codex", "Codex", 10);
    provider.provider_type = "codex".to_string();
    provider.config = Some(json!({
        "pool_advanced": {
            "scheduling_presets": [
                {"preset": "plus_first", "enabled": true}
            ]
        }
    }));
    let mut free_key = sample_key(
        "key-codex-free",
        "provider-codex",
        "openai:responses",
        "sk-codex-free",
    );
    free_key.internal_priority = 0;
    free_key.status_snapshot = Some(json!({
        "quota": {
            "provider_type": "codex",
            "code": "limited",
            "plan_type": "free",
            "usage_ratio": 0.1
        }
    }));
    let mut plus_key = sample_key(
        "key-codex-plus",
        "provider-codex",
        "openai:responses",
        "sk-codex-plus",
    );
    plus_key.internal_priority = 100;
    plus_key.status_snapshot = Some(json!({
        "quota": {
            "provider_type": "codex",
            "code": "limited",
            "plan_type": "plus",
            "usage_ratio": 0.1
        }
    }));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-codex-responses",
            "provider-codex",
            "openai:responses",
            "https://chatgpt.com/backend-api/codex",
        )],
        vec![free_key, plus_key],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_request_candidate_repository_for_tests(
                    request_candidate_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY.to_string())
                .attach_provider_catalog_repository_for_tests(provider_catalog_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-codex",
            "mode": "pool",
            "model": "gpt-5.4-mini",
            "failover_models": ["gpt-5.4-mini"],
            "api_format": "openai:responses",
            "endpoint_id": "endpoint-codex-responses",
            "request_id": "provider-query-model-test-trace-123",
            "request_body": {
                "model": "gpt-5.4-mini",
                "input": "hello",
                "stream": true
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["attempts"][0]["key_id"], json!("key-codex-plus"));
    assert_eq!(
        payload["candidate_summary"]["winning_key_id"],
        json!("key-codex-plus")
    );
    assert_eq!(payload["candidate_summary"]["total_candidates"], json!(2));
    assert_eq!(payload["candidate_summary"]["attempted"], json!(1));
    assert_eq!(payload["candidate_summary"]["unused"], json!(1));
    let stored_candidates = request_candidate_repository
        .list_by_request_id("provider-query-model-test-trace-123")
        .await
        .expect("model-test trace candidates should read");
    assert_eq!(stored_candidates.len(), 2);
    assert_eq!(
        stored_candidates[0].key_id.as_deref(),
        Some("key-codex-plus")
    );
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);
    assert_eq!(
        stored_candidates[1].key_id.as_deref(),
        Some("key-codex-free")
    );
    assert_eq!(stored_candidates[1].status, RequestCandidateStatus::Unused);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_test_model_failover_locally_with_trusted_admin_principal() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_test_model_failover_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_query_test_model_failover_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_query_test_model_failover_locally_with_trusted_admin_principal_impl(
) {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            let auth = plan
                .headers
                .get("authorization")
                .map(String::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(plan.provider_id, "provider-openai");
            assert_eq!(plan.endpoint_id, "endpoint-openai-chat");
            assert_eq!(plan.provider_api_format, "openai:chat");
            assert_eq!(plan.model_name.as_deref(), Some("gpt-4.1"));
            assert_eq!(
                plan.headers.get("x-test-header").map(String::as_str),
                Some("from-admin")
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("messages"))
                    .and_then(|messages| messages.as_array())
                    .and_then(|messages| messages.first())
                    .and_then(|message| message.get("content")),
                Some(&json!("custom prompt"))
            );
            let payload = if auth == "Bearer sk-test-first" {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 429,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "error": {
                                "message": "too many requests"
                            }
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 11
                    }
                })
            } else {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-failover",
                            "object": "chat.completion",
                            "choices": [{
                                "message": {
                                    "role": "assistant",
                                    "content": "Recovered from OpenAI failover"
                                }
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 27
                    }
                })
            };
            Json(payload)
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example/v1",
        )],
        vec![
            sample_key(
                "key-openai-first",
                "provider-openai",
                "openai:chat",
                "sk-test-first",
            ),
            sample_key(
                "key-openai-second",
                "provider-openai",
                "openai:chat",
                "sk-test-second",
            ),
        ],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "mode": "direct",
            "model_name": "gpt-4.1",
            "failover_models": ["gpt-4.1"],
            "api_format": "openai:chat",
            "request_headers": {
                "x-test-header": "from-admin"
            },
            "request_body": {
                "model": "ignored-model",
                "messages": [{
                    "role": "user",
                    "content": "custom prompt"
                }]
            },
            "request_id": "provider-test-openai"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_candidates"], json!(2));
    assert_eq!(payload["total_attempts"], json!(2));
    assert_eq!(payload["candidate_summary"]["total_candidates"], json!(2));
    assert_eq!(payload["candidate_summary"]["attempted"], json!(2));
    assert_eq!(payload["candidate_summary"]["failed"], json!(1));
    assert_eq!(payload["candidate_summary"]["success"], json!(1));
    assert_eq!(
        payload["candidate_summary"]["stop_reason"],
        json!("first_success")
    );
    let attempts = payload["attempts"]
        .as_array()
        .expect("attempts should be an array");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["status"], json!("failed"));
    assert_eq!(attempts[0]["status_code"], json!(429));
    assert_eq!(attempts[1]["status"], json!("success"));
    assert_eq!(attempts[1]["key_id"], json!("key-openai-second"));
    assert_eq!(attempts[1]["request_body"]["model"], json!("gpt-4.1"));
    assert_eq!(payload["data"]["stream"], json!(false));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Recovered from OpenAI failover")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_test_model_for_kiro_locally() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_test_model_for_kiro_locally",
        gateway_handles_admin_provider_query_test_model_for_kiro_locally_impl,
    );
}

async fn gateway_handles_admin_provider_query_test_model_for_kiro_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-kiro");
            assert_eq!(plan.endpoint_id, "endpoint-kiro-cli");
            assert_eq!(plan.key_id, "key-kiro-primary");
            assert_eq!(plan.provider_api_format, "claude:messages");
            assert_eq!(plan.model_name.as_deref(), Some("claude-sonnet-4-upstream"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/vnd.amazon.eventstream"
                },
                "body": {
                    "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                        [
                            encode_kiro_event_frame("assistantResponseEvent", json!({"content": "Hello from Kiro"})),
                            encode_kiro_exception_frame("ContentLengthExceededException"),
                        ]
                        .concat()
                    )
                },
                "telemetry": {
                    "elapsed_ms": 42
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-kiro", "Kiro", 10);
    provider.provider_type = "kiro".to_string();
    let mut key = sample_key(
        "key-kiro-primary",
        "provider-kiro",
        "claude:messages",
        "__placeholder__",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{
                "provider_type":"kiro",
                "auth_method":"idc",
                "access_token":"cached-kiro-token",
                "expires_at":4102444800,
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                "machine_id":"123e4567-e89b-12d3-a456-426614174000",
                "api_region":"us-east-1",
                "client_id":"client-id",
                "client_secret":"client-secret"
            }"#,
        )
        .expect("auth config should encrypt"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![StoredProviderCatalogEndpoint::new(
            "endpoint-kiro-cli".to_string(),
            "provider-kiro".to_string(),
            "claude:messages".to_string(),
            Some("claude".to_string()),
            Some("messages".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://q.{region}.amazonaws.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-kiro",
            "model_name": "claude-sonnet-4-upstream",
            "api_format": "claude:messages"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["provider"]["id"], json!("provider-kiro"));
    assert_eq!(payload["model"], json!("claude-sonnet-4-upstream"));
    assert_eq!(
        payload["data"]["response"]["content"][0]["text"],
        json!("Hello from Kiro")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_uses_kiro_mapped_model_name_for_explicit_model_mapping_test() {
    run_provider_query_test(
        "gateway_uses_kiro_mapped_model_name_for_explicit_model_mapping_test",
        gateway_uses_kiro_mapped_model_name_for_explicit_model_mapping_test_impl,
    );
}

async fn gateway_uses_kiro_mapped_model_name_for_explicit_model_mapping_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-kiro");
            assert_eq!(plan.endpoint_id, "endpoint-kiro-cli");
            assert_eq!(plan.key_id, "key-kiro-primary");
            assert_eq!(plan.provider_api_format, "claude:messages");
            assert_eq!(plan.model_name.as_deref(), Some("claude-haiku-4.5"));
            let body = plan.body.json_body.as_ref().expect("json body");
            assert_eq!(
                body.pointer("/conversationState/currentMessage/userInputMessage/modelId"),
                Some(&json!("claude-haiku-4.5"))
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/vnd.amazon.eventstream"
                },
                "body": {
                    "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                        [
                            encode_kiro_event_frame("assistantResponseEvent", json!({"content": "Hello from Kiro mapped model"})),
                            encode_kiro_exception_frame("ContentLengthExceededException"),
                        ]
                        .concat()
                    )
                },
                "telemetry": {
                    "elapsed_ms": 42
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-kiro", "Kiro", 10);
    provider.provider_type = "kiro".to_string();
    let mut key = sample_key(
        "key-kiro-primary",
        "provider-kiro",
        "claude:messages",
        "__placeholder__",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{
                "provider_type":"kiro",
                "auth_method":"idc",
                "access_token":"cached-kiro-token",
                "expires_at":4102444800,
                "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                "machine_id":"123e4567-e89b-12d3-a456-426614174000",
                "api_region":"us-east-1",
                "client_id":"client-id",
                "client_secret":"client-secret"
            }"#,
        )
        .expect("auth config should encrypt"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![StoredProviderCatalogEndpoint::new(
            "endpoint-kiro-cli".to_string(),
            "provider-kiro".to_string(),
            "claude:messages".to_string(),
            Some("claude".to_string()),
            Some("messages".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://q.{region}.amazonaws.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")],
        vec![key],
    ));
    let mut provider_model = sample_admin_provider_model(
        "model-kiro-haiku",
        "provider-kiro",
        "global-kiro-haiku",
        "claude-haiku-4-5-20251001",
    );
    provider_model.provider_model_mappings = Some(json!([{
        "name": "claude-haiku-4.5",
        "priority": 1,
        "api_formats": ["claude:messages"],
        "endpoint_ids": ["endpoint-kiro-cli"]
    }]));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_provider_models(vec![provider_model]),
    );

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_transport_reader_for_tests(
                    provider_catalog_repository,
                    DEVELOPMENT_ENCRYPTION_KEY.to_string(),
                )
                .with_global_model_repository_for_tests(global_model_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-kiro",
            "mode": "pool",
            "model_name": "claude-haiku-4-5-20251001",
            "failover_models": ["claude-haiku-4-5-20251001"],
            "mapped_model_name": "claude-haiku-4.5",
            "api_format": "claude:messages",
            "endpoint_id": "endpoint-kiro-cli",
            "request_id": "provider-test-kiro-mapped"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["attempts"][0]["effective_model"],
        json!("claude-haiku-4.5")
    );
    assert_eq!(
        payload["attempts"][0]["request_body"]
            .pointer("/conversationState/currentMessage/userInputMessage/modelId"),
        Some(&json!("claude-haiku-4.5"))
    );
    assert_eq!(
        payload["data"]["response"]["content"][0]["text"],
        json!("Hello from Kiro mapped model")
    );

    let direct_response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-kiro",
            "mode": "direct",
            "model_name": "claude-haiku-4-5-20251001",
            "mapped_model_name": "claude-haiku-4.5",
            "api_format": "claude:messages",
            "endpoint_id": "endpoint-kiro-cli",
            "request_id": "provider-test-kiro-mapped-direct"
        }))
        .send()
        .await
        .expect("direct request should succeed");

    assert_eq!(direct_response.status(), StatusCode::OK);
    let direct_payload: serde_json::Value = direct_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(direct_payload["success"], json!(true));
    assert_eq!(
        direct_payload["data"]["response"]["content"][0]["text"],
        json!("Hello from Kiro mapped model")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_test_model_failover_for_kiro_locally() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_test_model_failover_for_kiro_locally",
        gateway_handles_admin_provider_query_test_model_failover_for_kiro_locally_impl,
    );
}

async fn gateway_handles_admin_provider_query_test_model_failover_for_kiro_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            let payload = if plan.key_id == "key-kiro-first" {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 429,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "message": "too many requests"
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 11
                    }
                })
            } else {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/vnd.amazon.eventstream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                            [
                                encode_kiro_event_frame("assistantResponseEvent", json!({"content": "Recovered from failover"})),
                                encode_kiro_exception_frame("ContentLengthExceededException"),
                            ]
                            .concat()
                        )
                    },
                    "telemetry": {
                        "elapsed_ms": 27
                    }
                })
            };
            Json(payload)
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-kiro", "Kiro", 10);
    provider.provider_type = "kiro".to_string();
    let build_key = |id: &str| {
        let mut key = sample_key(id, "provider-kiro", "claude:messages", "__placeholder__");
        key.auth_type = "oauth".to_string();
        key.encrypted_auth_config = Some(
            aether_crypto::encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{
                    "provider_type":"kiro",
                    "auth_method":"idc",
                    "access_token":"cached-kiro-token",
                    "expires_at":4102444800,
                    "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                    "machine_id":"123e4567-e89b-12d3-a456-426614174000",
                    "api_region":"us-east-1",
                    "client_id":"client-id",
                    "client_secret":"client-secret"
                }"#,
            )
            .expect("auth config should encrypt"),
        );
        key
    };

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![StoredProviderCatalogEndpoint::new(
            "endpoint-kiro-cli".to_string(),
            "provider-kiro".to_string(),
            "claude:messages".to_string(),
            Some("claude".to_string()),
            Some("messages".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://q.{region}.amazonaws.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")],
        vec![build_key("key-kiro-first"), build_key("key-kiro-second")],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-kiro",
            "mode": "direct",
            "model_name": "claude-sonnet-4-upstream",
            "failover_models": ["claude-sonnet-4-upstream"],
            "api_format": "claude:messages",
            "request_id": "provider-test-kiro"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_candidates"], json!(2));
    assert_eq!(payload["total_attempts"], json!(2));
    assert_eq!(payload["candidate_summary"]["total_candidates"], json!(2));
    assert_eq!(payload["candidate_summary"]["attempted"], json!(2));
    assert_eq!(payload["candidate_summary"]["failed"], json!(1));
    assert_eq!(payload["candidate_summary"]["success"], json!(1));
    assert_eq!(
        payload["candidate_summary"]["stop_reason"],
        json!("first_success")
    );
    let attempts = payload["attempts"]
        .as_array()
        .expect("attempts should be an array");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["status"], json!("failed"));
    assert_eq!(attempts[0]["status_code"], json!(429));
    assert_eq!(attempts[1]["status"], json!("success"));
    assert_eq!(
        payload["data"]["response"]["content"][0]["text"],
        json!("Recovered from failover")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_retries_kiro_failover_after_http_error_without_message() {
    run_provider_query_test(
        "gateway_retries_kiro_failover_after_http_error_without_message",
        gateway_retries_kiro_failover_after_http_error_without_message_impl,
    );
}

async fn gateway_retries_kiro_failover_after_http_error_without_message_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            let payload = if plan.key_id == "key-kiro-first" {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 500,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {}
                    },
                    "telemetry": {
                        "elapsed_ms": 9
                    }
                })
            } else {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/vnd.amazon.eventstream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                            [
                                encode_kiro_event_frame("assistantResponseEvent", json!({"content": "Recovered from Kiro empty error"})),
                                encode_kiro_exception_frame("ContentLengthExceededException"),
                            ]
                            .concat()
                        )
                    },
                    "telemetry": {
                        "elapsed_ms": 21
                    }
                })
            };
            Json(payload)
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-kiro", "Kiro", 10);
    provider.provider_type = "kiro".to_string();
    let build_key = |id: &str| {
        let mut key = sample_key(id, "provider-kiro", "claude:messages", "__placeholder__");
        key.auth_type = "oauth".to_string();
        key.encrypted_auth_config = Some(
            aether_crypto::encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{
                    "provider_type":"kiro",
                    "auth_method":"idc",
                    "access_token":"cached-kiro-token",
                    "expires_at":4102444800,
                    "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                    "machine_id":"123e4567-e89b-12d3-a456-426614174000",
                    "api_region":"us-east-1",
                    "client_id":"client-id",
                    "client_secret":"client-secret"
                }"#,
            )
            .expect("auth config should encrypt"),
        );
        key
    };

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![StoredProviderCatalogEndpoint::new(
            "endpoint-kiro-cli".to_string(),
            "provider-kiro".to_string(),
            "claude:messages".to_string(),
            Some("claude".to_string()),
            Some("messages".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://q.{region}.amazonaws.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")],
        vec![build_key("key-kiro-first"), build_key("key-kiro-second")],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-kiro",
            "mode": "direct",
            "model_name": "claude-sonnet-4-upstream",
            "failover_models": ["claude-sonnet-4-upstream"],
            "api_format": "claude:messages",
            "request_id": "provider-test-kiro-empty-error"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_attempts"], json!(2));
    let attempts = payload["attempts"]
        .as_array()
        .expect("attempts should be an array");
    assert_eq!(attempts[0]["status"], json!("failed"));
    assert_eq!(attempts[0]["status_code"], json!(500));
    assert_eq!(attempts[1]["status"], json!("success"));
    assert_eq!(
        payload["data"]["response"]["content"][0]["text"],
        json!("Recovered from Kiro empty error")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_non_kiro_multi_model_failover_locally() {
    run_provider_query_test(
        "gateway_handles_non_kiro_multi_model_failover_locally",
        gateway_handles_non_kiro_multi_model_failover_locally_impl,
    );
}

async fn gateway_handles_non_kiro_multi_model_failover_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            let payload = if plan.model_name.as_deref() == Some("gpt-4.1") {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 500,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "error": {
                                "message": "primary model failed"
                            }
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 8
                    }
                })
            } else {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-multi-model",
                            "choices": [{
                                "message": {
                                    "role": "assistant",
                                    "content": "Recovered with fallback model"
                                }
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 12
                    }
                })
            };
            Json(payload)
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example/v1",
        )],
        vec![sample_key(
            "key-openai-primary",
            "provider-openai",
            "openai:chat",
            "sk-test-primary",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "failover_models": ["gpt-4.1", "gpt-4o-mini"],
            "api_format": "openai:chat"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_attempts"], json!(2));
    let attempts = payload["attempts"]
        .as_array()
        .expect("attempts should be an array");
    assert_eq!(attempts[0]["effective_model"], json!("gpt-4.1"));
    assert_eq!(attempts[1]["effective_model"], json!("gpt-4o-mini"));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Recovered with fallback model")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_openai_responses_test_model_locally() {
    run_provider_query_test(
        "gateway_handles_openai_responses_test_model_locally",
        gateway_handles_openai_responses_test_model_locally_impl,
    );
}

async fn gateway_handles_openai_responses_test_model_locally_impl() {
    let prompt = "Tell me whether the CLI request preserved this prompt.";
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-openai");
            assert_eq!(plan.endpoint_id, "endpoint-openai-cli");
            assert_eq!(plan.key_id, "key-openai-cli");
            assert_eq!(plan.provider_api_format, "openai:responses");
            assert_eq!(plan.url, "https://tiger.bookapi.cc/codex/responses");
            assert!(plan.stream);
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer sk-test-cli")
            );
            assert_eq!(
                plan.headers.get("x-stainless-runtime").map(String::as_str),
                Some("node")
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("model")),
                Some(&json!("gpt-5.4-mini"))
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("stream")),
                Some(&json!(true))
            );
            assert!(plan
                .body
                .json_body
                .as_ref()
                .and_then(|body| body.get("input"))
                .is_some());
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("input"))
                    .and_then(|input| input.as_array())
                    .and_then(|items| items.first())
                    .and_then(|item| item.get("type"))
                    .and_then(|value| value.as_str()),
                Some("message")
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("input"))
                    .and_then(|input| input.as_array())
                    .and_then(|items| items.first())
                    .and_then(|item| item.get("content"))
                    .and_then(|content| content.as_array())
                    .and_then(|parts| parts.first())
                    .and_then(|part| part.get("text"))
                    .and_then(|value| value.as_str()),
                Some(prompt)
            );
            assert!(plan
                .body
                .json_body
                .as_ref()
                .and_then(|body| body.get("instructions"))
                .is_none());
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("store")),
                Some(&json!(false))
            );
            assert!(plan
                .body
                .json_body
                .as_ref()
                .and_then(|body| body.get("prompt_cache_key"))
                .is_none());
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-openai-cli-test-model",
                        "object": "chat.completion",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Hello from OpenAI Responses"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 17
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "codex".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-cli",
            "provider-openai",
            "openai:responses",
            "https://tiger.bookapi.cc/codex",
        )],
        vec![sample_key(
            "key-openai-cli",
            "provider-openai",
            "openai:responses",
            "sk-test-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "model": "gpt-5.4-mini",
            "api_format": "openai:responses",
            "message": prompt,
            "request_headers": {
                "x-stainless-runtime": "node"
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from OpenAI Responses")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_openai_image_test_model_locally() {
    run_provider_query_test(
        "gateway_handles_openai_image_test_model_locally",
        gateway_handles_openai_image_test_model_locally_impl,
    );
}

async fn gateway_handles_openai_image_test_model_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-openai");
            assert_eq!(plan.endpoint_id, "endpoint-openai-image");
            assert_eq!(plan.key_id, "key-openai-image");
            assert_eq!(plan.client_api_format, "openai:image");
            assert_eq!(plan.provider_api_format, "openai:image");
            assert_eq!(plan.model_name.as_deref(), Some("gpt-image-1"));
            assert_eq!(plan.url, "https://api.openai.example/v1/images/generations");
            assert!(!plan.stream);
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer sk-test-image")
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("model")),
                Some(&json!("gpt-image-1"))
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("prompt"))
                    .and_then(|value| value.as_str()),
                Some("Draw a small blue square")
            );
            assert!(plan
                .body
                .json_body
                .as_ref()
                .is_some_and(|body| body.get("stream").is_none()));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "created": 1776839946,
                        "model": "gpt-image-1",
                        "data": [{
                            "b64_json": "aGVsbG8=",
                            "revised_prompt": "revised prompt"
                        }],
                        "usage": {
                            "input_tokens": 171,
                            "output_tokens": 1372,
                            "total_tokens": 1543
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 19
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "codex".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-image",
            "provider-openai",
            "openai:image",
            "https://api.openai.example/v1",
        )],
        vec![sample_key(
            "key-openai-image",
            "provider-openai",
            "openai:image",
            "sk-test-image",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "model": "gpt-image-1",
            "api_format": "openai:image",
            "message": "Draw a small blue square"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["data"][0]["b64_json"],
        json!("aGVsbG8=")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_reports_transport_unsupported_reason_for_non_kiro_provider() {
    run_provider_query_test(
        "gateway_reports_transport_unsupported_reason_for_non_kiro_provider",
        gateway_reports_transport_unsupported_reason_for_non_kiro_provider_impl,
    );
}

async fn gateway_reports_transport_unsupported_reason_for_non_kiro_provider_impl() {
    let mut provider = sample_provider("provider-antigravity", "Antigravity", 10);
    provider.provider_type = "antigravity".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-antigravity-gemini",
            "provider-antigravity",
            "gemini:generate_content",
            "https://cloudcode-pa.googleapis.com",
        )],
        vec![sample_key(
            "key-antigravity-gemini",
            "provider-antigravity",
            "gemini:generate_content",
            "sk-test-antigravity",
        )],
    ));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-antigravity",
            "model": "gemini-2.5-pro",
            "api_format": "gemini:generate_content"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(false));
    assert_eq!(
        payload["error"],
        json!(
            "Rust local provider-query model test cannot execute endpoint format gemini:generate_content (transport_antigravity_auth_config_missing)"
        )
    );

    gateway_handle.abort();
}

#[test]
fn gateway_handles_antigravity_endpoint_test_model_locally() {
    run_provider_query_test(
        "gateway_handles_antigravity_endpoint_test_model_locally",
        gateway_handles_antigravity_endpoint_test_model_locally_impl,
    );
}

async fn gateway_handles_antigravity_endpoint_test_model_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-antigravity");
            assert_eq!(plan.endpoint_id, "endpoint-antigravity-gemini");
            assert_eq!(plan.key_id, "key-antigravity-gemini");
            assert_eq!(plan.provider_api_format, "gemini:generate_content");
            assert_eq!(
                plan.url,
                "https://antigravity.googleapis.com/v1internal:generateContent"
            );
            assert_eq!(plan.stream, false);
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("project"))
                    .and_then(|value| value.as_str()),
                Some("project-ant-123")
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("requestType"))
                    .and_then(|value| value.as_str()),
                Some("endpoint_test")
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("model"))
                    .and_then(|value| value.as_str()),
                Some("gemini-2.5-pro")
            );
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("request"))
                    .and_then(|request| request.get("contents"))
                    .and_then(|contents| contents.as_array())
                    .and_then(|items| items.first())
                    .and_then(|item| item.get("parts"))
                    .and_then(|parts| parts.as_array())
                    .and_then(|parts| parts.first())
                    .and_then(|part| part.get("text"))
                    .and_then(|value| value.as_str()),
                Some("Say hello")
            );
            assert_eq!(
                plan.headers.get("x-client-name").map(String::as_str),
                Some("antigravity")
            );
            assert!(plan.headers.contains_key("x-goog-api-client"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "response": {
                            "candidates": [{
                                "content": {
                                    "parts": [
                                        {"text": "Hello from Antigravity EndpointTest"}
                                    ],
                                    "role": "model"
                                },
                                "finishReason": "STOP",
                                "index": 0
                            }],
                            "modelVersion": "claude-sonnet-4-5",
                            "usageMetadata": {
                                "promptTokenCount": 2,
                                "candidatesTokenCount": 3,
                                "totalTokenCount": 5
                            }
                        },
                        "responseId": "resp-antigravity-test-123"
                    }
                },
                "telemetry": {
                    "elapsed_ms": 23
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-antigravity", "Antigravity", 10);
    provider.provider_type = "antigravity".to_string();
    let mut key = sample_key(
        "key-antigravity-gemini",
        "provider-antigravity",
        "gemini:generate_content",
        "cached-antigravity-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{
                "provider_type":"antigravity",
                "project_id":"project-ant-123",
                "client_version":"1.2.3",
                "session_id":"sess-ant-123",
                "refresh_token":"rt-ant-123"
            }"#,
        )
        .expect("auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-antigravity-gemini",
            "provider-antigravity",
            "gemini:generate_content",
            "https://antigravity.googleapis.com",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-antigravity",
            "model": "gemini-2.5-pro",
            "api_format": "gemini:generate_content",
            "message": "Say hello"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from Antigravity EndpointTest")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_hydrates_antigravity_project_id_from_load_code_assist_for_test_model() {
    run_provider_query_test(
        "gateway_hydrates_antigravity_project_id_from_load_code_assist_for_test_model",
        gateway_hydrates_antigravity_project_id_from_load_code_assist_for_test_model_impl,
    );
}

async fn gateway_hydrates_antigravity_project_id_from_load_code_assist_for_test_model_impl() {
    let seen_urls = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_urls_clone = Arc::clone(&seen_urls);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let seen_urls_inner = Arc::clone(&seen_urls_clone);
            async move {
                seen_urls_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.url.clone());
                if plan.url == "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist"
                {
                    assert_eq!(plan.model_name.as_deref(), Some("loadCodeAssist"));
                    assert_eq!(
                        plan.headers.get("authorization").map(String::as_str),
                        Some("Bearer cached-antigravity-token")
                    );
                    assert_eq!(
                        plan.body.json_body.as_ref().and_then(|body| body
                            .get("metadata")
                            .and_then(|metadata| metadata.get("pluginType"))),
                        Some(&json!("GEMINI"))
                    );
                    return Json(json!({
                        "request_id": plan.request_id,
                        "candidate_id": plan.candidate_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "cloudaicompanionProject": {
                                    "id": "project-from-antigravity-load-code-assist"
                                },
                                "currentTier": {
                                    "id": "free"
                                }
                            }
                        }
                    }));
                }

                assert_eq!(
                    plan.url,
                    "https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent"
                );
                assert_eq!(plan.provider_id, "provider-antigravity");
                assert_eq!(plan.endpoint_id, "endpoint-antigravity-gemini");
                assert_eq!(plan.key_id, "key-antigravity-gemini");
                assert_eq!(plan.provider_api_format, "gemini:generate_content");
                assert!(!plan.stream);
                assert_eq!(
                    plan.body.json_body.as_ref().unwrap()["project"],
                    json!("project-from-antigravity-load-code-assist")
                );
                assert_eq!(
                    plan.body.json_body.as_ref().unwrap()["requestType"],
                    json!("endpoint_test")
                );
                assert_eq!(
                    plan.body.json_body.as_ref().unwrap()["model"],
                    json!("gemini-3.1-flash-lite")
                );
                Json(json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "response": {
                                "candidates": [{
                                    "content": {
                                        "parts": [
                                            {"text": "Hello from hydrated Antigravity"}
                                        ],
                                        "role": "model"
                                    },
                                    "finishReason": "STOP",
                                    "index": 0
                                }],
                                "modelVersion": "gemini-3.1-flash-lite",
                                "usageMetadata": {
                                    "promptTokenCount": 2,
                                    "candidatesTokenCount": 4,
                                    "totalTokenCount": 6
                                }
                            },
                            "responseId": "resp-antigravity-hydrated-test-123"
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 23
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-antigravity", "Antigravity", 10);
    provider.provider_type = "antigravity".to_string();
    let mut key = sample_key(
        "key-antigravity-gemini",
        "provider-antigravity",
        "gemini:generate_content",
        "cached-antigravity-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"antigravity","refresh_token":"rt-antigravity-123"}"#,
        )
        .expect("auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-antigravity-gemini",
            "provider-antigravity",
            "gemini:generate_content",
            "https://daily-cloudcode-pa.googleapis.com",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-antigravity",
            "model": "gemini-3.1-flash-lite",
            "api_format": "gemini:generate_content",
            "message": "Say hello"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from hydrated Antigravity")
    );
    assert_eq!(
        *seen_urls.lock().expect("mutex should lock"),
        vec![
            "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist".to_string(),
            "https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent".to_string(),
        ]
    );
    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-antigravity-gemini".to_string()])
        .await
        .expect("key should reload");
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("antigravity"))
            .and_then(|metadata| metadata.get("project_id")),
        Some(&json!("project-from-antigravity-load-code-assist"))
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_prefers_supported_non_kiro_endpoint_when_api_format_is_omitted() {
    run_provider_query_test(
        "gateway_prefers_supported_non_kiro_endpoint_when_api_format_is_omitted",
        gateway_prefers_supported_non_kiro_endpoint_when_api_format_is_omitted_impl,
    );
}

async fn gateway_prefers_supported_non_kiro_endpoint_when_api_format_is_omitted_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-openai-chat");
            assert_eq!(plan.provider_api_format, "openai:chat");
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-preferred-endpoint",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Selected supported endpoint"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 10
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![
            sample_endpoint(
                "endpoint-openai-cli",
                "provider-openai",
                "openai:responses",
                "https://api.openai.example/v1",
            ),
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example/v1",
            ),
        ],
        vec![
            sample_key(
                "key-openai-cli",
                "provider-openai",
                "openai:responses",
                "sk-test-cli",
            ),
            sample_key(
                "key-openai-chat",
                "provider-openai",
                "openai:chat",
                "sk-test-chat",
            ),
        ],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "model": "gpt-5.4-mini"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Selected supported endpoint")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_prefers_transport_supported_non_kiro_endpoint_when_api_format_is_omitted() {
    run_provider_query_test(
        "gateway_prefers_transport_supported_non_kiro_endpoint_when_api_format_is_omitted",
        gateway_prefers_transport_supported_non_kiro_endpoint_when_api_format_is_omitted_impl,
    );
}

async fn gateway_prefers_transport_supported_non_kiro_endpoint_when_api_format_is_omitted_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-openai-chat-supported");
            assert_eq!(plan.provider_api_format, "openai:chat");
            assert_eq!(
                plan.headers.get("content-type").map(String::as_str),
                Some("application/json")
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-supported-transport",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Selected locally supported endpoint"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 12
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let mut unsupported_endpoint = sample_endpoint(
        "endpoint-openai-chat-unsupported",
        "provider-openai",
        "openai:chat",
        "https://api.openai.example/v1",
    );
    unsupported_endpoint.header_rules = Some(json!({"invalid": true}));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![
            unsupported_endpoint,
            sample_endpoint(
                "endpoint-openai-chat-supported",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example/v1",
            ),
        ],
        vec![sample_key(
            "key-openai-chat",
            "provider-openai",
            "openai:chat",
            "sk-test-chat",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "model": "gpt-5.4-mini"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Selected locally supported endpoint")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_prefers_supported_non_kiro_endpoint_with_compatible_key_when_api_format_is_omitted() {
    run_provider_query_test(
        "gateway_prefers_supported_non_kiro_endpoint_with_compatible_key_when_api_format_is_omitted",
        gateway_prefers_supported_non_kiro_endpoint_with_compatible_key_when_api_format_is_omitted_impl,
    );
}

async fn gateway_prefers_supported_non_kiro_endpoint_with_compatible_key_when_api_format_is_omitted_impl(
) {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-openai-chat");
            assert_eq!(plan.provider_api_format, "openai:chat");
            assert_eq!(plan.key_id, "key-openai-chat");
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-key-compatible-endpoint",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Selected endpoint with compatible key"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 11
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![
            sample_endpoint(
                "endpoint-gemini-chat",
                "provider-openai",
                "gemini:generate_content",
                "https://api.gemini.example",
            ),
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example/v1",
            ),
        ],
        vec![sample_key(
            "key-openai-chat",
            "provider-openai",
            "openai:chat",
            "sk-test-chat",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "model": "gpt-5.4-mini"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Selected endpoint with compatible key")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_uses_compatible_cli_endpoint_when_api_format_is_omitted() {
    run_provider_query_test(
        "gateway_uses_compatible_cli_endpoint_when_api_format_is_omitted",
        gateway_uses_compatible_cli_endpoint_when_api_format_is_omitted_impl,
    );
}

async fn gateway_uses_compatible_cli_endpoint_when_api_format_is_omitted_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-openai-cli");
            assert_eq!(plan.provider_api_format, "openai:responses");
            assert_eq!(plan.key_id, "key-openai-cli");
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-cli-only-endpoint",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Selected compatible CLI endpoint"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 13
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example/v1",
            ),
            sample_endpoint(
                "endpoint-openai-cli",
                "provider-openai",
                "openai:responses",
                "https://api.openai.example/v1",
            ),
        ],
        vec![sample_key(
            "key-openai-cli",
            "provider-openai",
            "openai:responses",
            "sk-test-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "model": "gpt-5.4-mini"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Selected compatible CLI endpoint")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_uses_runnable_cli_endpoint_after_chat_preference_when_api_format_is_omitted() {
    run_provider_query_test(
        "gateway_uses_runnable_cli_endpoint_after_chat_preference_when_api_format_is_omitted",
        gateway_uses_runnable_cli_endpoint_after_chat_preference_when_api_format_is_omitted_impl,
    );
}

async fn gateway_uses_runnable_cli_endpoint_after_chat_preference_when_api_format_is_omitted_impl()
{
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-openai-cli-runnable");
            assert_eq!(plan.provider_api_format, "openai:responses");
            assert_eq!(plan.key_id, "key-openai-shared");
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-cli-runnable-after-chat-preference",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Selected runnable CLI endpoint after unsupported chat"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 18
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let mut unsupported_chat_endpoint = sample_endpoint(
        "endpoint-openai-chat-unsupported",
        "provider-openai",
        "openai:chat",
        "https://api.openai.example/v1",
    );
    unsupported_chat_endpoint.header_rules = Some(json!({"invalid": true}));
    let cli_endpoint = sample_endpoint(
        "endpoint-openai-cli-runnable",
        "provider-openai",
        "openai:responses",
        "https://api.openai.example/v1",
    );
    let mut shared_key = sample_key(
        "key-openai-shared",
        "provider-openai",
        "openai:chat",
        "sk-test-shared",
    );
    shared_key.api_formats = Some(json!(["openai:chat", "openai:responses"]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![unsupported_chat_endpoint, cli_endpoint],
        vec![shared_key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "model": "gpt-5.4-mini"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Selected runnable CLI endpoint after unsupported chat")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_openai_responses_test_model_failover_locally() {
    run_provider_query_test(
        "gateway_handles_openai_responses_test_model_failover_locally",
        gateway_handles_openai_responses_test_model_failover_locally_impl,
    );
}

async fn gateway_handles_openai_responses_test_model_failover_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-openai");
            assert_eq!(plan.endpoint_id, "endpoint-openai-cli");
            assert_eq!(plan.key_id, "key-openai-cli");
            assert_eq!(plan.provider_api_format, "openai:responses");
            assert_eq!(plan.model_name.as_deref(), Some("gpt-5.4-mini"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-openai-cli-failover",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "OpenAI Responses failover path succeeded"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 15
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-cli",
            "provider-openai",
            "openai:responses",
            "https://api.openai.example/v1",
        )],
        vec![sample_key(
            "key-openai-cli",
            "provider-openai",
            "openai:responses",
            "sk-test-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "failover_models": ["gpt-5.4-mini"],
            "api_format": "openai:responses"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_attempts"], json!(1));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("OpenAI Responses failover path succeeded")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_claude_cli_test_model_locally() {
    run_provider_query_test(
        "gateway_handles_claude_cli_test_model_locally",
        gateway_handles_claude_cli_test_model_locally_impl,
    );
}

async fn gateway_handles_claude_cli_test_model_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-claude");
            assert_eq!(plan.endpoint_id, "endpoint-claude-cli");
            assert_eq!(plan.key_id, "key-claude-cli");
            assert_eq!(plan.provider_api_format, "claude:messages");
            assert_eq!(plan.url, "https://api.anthropic.example/v1/messages");
            assert_eq!(plan.model_name.as_deref(), Some("claude-sonnet-4-5"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-claude-cli-test-model",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Hello from Claude CLI"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 14
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-claude", "Claude", 10);
    provider.provider_type = "anthropic".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-claude-cli",
            "provider-claude",
            "claude:messages",
            "https://api.anthropic.example/v1",
        )],
        vec![sample_key(
            "key-claude-cli",
            "provider-claude",
            "claude:messages",
            "sk-test-claude-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-claude",
            "model": "claude-sonnet-4-5",
            "api_format": "claude:messages"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from Claude CLI")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_uses_compatible_claude_cli_endpoint_when_api_format_is_omitted() {
    run_provider_query_test(
        "gateway_uses_compatible_claude_cli_endpoint_when_api_format_is_omitted",
        gateway_uses_compatible_claude_cli_endpoint_when_api_format_is_omitted_impl,
    );
}

async fn gateway_uses_compatible_claude_cli_endpoint_when_api_format_is_omitted_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-claude-cli");
            assert_eq!(plan.provider_api_format, "claude:messages");
            assert_eq!(plan.key_id, "key-claude-cli");
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-claude-cli-only-endpoint",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Selected compatible Claude CLI endpoint"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 12
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-claude", "Claude", 10);
    provider.provider_type = "anthropic".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-claude-cli",
            "provider-claude",
            "claude:messages",
            "https://api.anthropic.example/v1",
        )],
        vec![sample_key(
            "key-claude-cli",
            "provider-claude",
            "claude:messages",
            "sk-test-claude-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-claude",
            "model": "claude-sonnet-4-5"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Selected compatible Claude CLI endpoint")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_claude_cli_test_model_failover_locally() {
    run_provider_query_test(
        "gateway_handles_claude_cli_test_model_failover_locally",
        gateway_handles_claude_cli_test_model_failover_locally_impl,
    );
}

async fn gateway_handles_claude_cli_test_model_failover_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-claude");
            assert_eq!(plan.endpoint_id, "endpoint-claude-cli");
            assert_eq!(plan.key_id, "key-claude-cli");
            assert_eq!(plan.provider_api_format, "claude:messages");
            assert_eq!(plan.model_name.as_deref(), Some("claude-sonnet-4-5"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-claude-cli-failover",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Claude CLI failover path succeeded"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 16
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-claude", "Claude", 10);
    provider.provider_type = "anthropic".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-claude-cli",
            "provider-claude",
            "claude:messages",
            "https://api.anthropic.example/v1",
        )],
        vec![sample_key(
            "key-claude-cli",
            "provider-claude",
            "claude:messages",
            "sk-test-claude-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-claude",
            "failover_models": ["claude-sonnet-4-5"],
            "api_format": "claude:messages"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_attempts"], json!(1));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Claude CLI failover path succeeded")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_gemini_cli_test_model_locally() {
    run_provider_query_test(
        "gateway_handles_gemini_cli_test_model_locally",
        gateway_handles_gemini_cli_test_model_locally_impl,
    );
}

async fn gateway_handles_gemini_cli_test_model_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-gemini");
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.key_id, "key-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:generate_content");
            assert_eq!(
                plan.url,
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent"
            );
            assert_eq!(plan.model_name.as_deref(), Some("gemini-2.5-pro"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-gemini-cli-test-model",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Hello from Gemini CLI"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 19
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-gemini", "Gemini", 10);
    provider.provider_type = "google".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-cli",
            "provider-gemini",
            "gemini:generate_content",
            "https://generativelanguage.googleapis.com",
        )],
        vec![sample_key(
            "key-gemini-cli",
            "provider-gemini",
            "gemini:generate_content",
            "sk-test-gemini-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-gemini",
            "model": "gemini-2.5-pro",
            "api_format": "gemini:generate_content"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from Gemini CLI")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_gemini_cli_test_model_with_oauth_header_fallback() {
    run_provider_query_test(
        "gateway_handles_gemini_cli_test_model_with_oauth_header_fallback",
        gateway_handles_gemini_cli_test_model_with_oauth_header_fallback_impl,
    );
}

async fn gateway_handles_gemini_cli_test_model_with_oauth_header_fallback_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-gemini");
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.key_id, "key-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:generate_content");
            assert!(!plan.stream);
            assert_eq!(
                plan.url,
                "https://cloudcode-pa.googleapis.com/v1internal:generateContent"
            );
            assert_eq!(
                plan.body.json_body.as_ref().unwrap()["project"],
                json!("project-1")
            );
            assert_eq!(
                plan.body.json_body.as_ref().unwrap()["model"],
                json!("gemini-2.5-pro")
            );
            assert!(plan.body.json_body.as_ref().unwrap()["request"]
                .get("contents")
                .is_some());
            assert_eq!(
                plan.headers.get("authorization").map(String::as_str),
                Some("Bearer cached-gemini-cli-token")
            );
            assert!(!plan.headers.contains_key("x-goog-api-key"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-gemini-cli-test-model",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Hello from Gemini CLI"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 19
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-gemini", "Gemini", 10);
    provider.provider_type = "gemini_cli".to_string();
    let mut key = sample_key(
        "key-gemini-cli",
        "provider-gemini",
        "gemini:generate_content",
        "cached-gemini-cli-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"gemini_cli","project_id":"project-1"}"#,
        )
        .expect("auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-cli",
            "provider-gemini",
            "gemini:generate_content",
            "https://cloudcode-pa.googleapis.com",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-gemini",
            "model": "gemini-2.5-pro",
            "api_format": "gemini:generate_content"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from Gemini CLI")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_hydrates_gemini_cli_project_id_from_load_code_assist_for_test_model() {
    run_provider_query_test(
        "gateway_hydrates_gemini_cli_project_id_from_load_code_assist_for_test_model",
        gateway_hydrates_gemini_cli_project_id_from_load_code_assist_for_test_model_impl,
    );
}

async fn gateway_hydrates_gemini_cli_project_id_from_load_code_assist_for_test_model_impl() {
    let seen_urls = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_urls_clone = Arc::clone(&seen_urls);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let seen_urls_inner = Arc::clone(&seen_urls_clone);
            async move {
                seen_urls_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.url.clone());
                if plan.url == "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist" {
                    assert_eq!(plan.model_name.as_deref(), Some("loadCodeAssist"));
                    assert_eq!(
                        plan.headers.get("authorization").map(String::as_str),
                        Some("Bearer cached-gemini-cli-token")
                    );
                    assert_eq!(
                        plan.body.json_body.as_ref().and_then(|body| body
                            .get("metadata")
                            .and_then(|metadata| metadata.get("pluginType"))),
                        Some(&json!("GEMINI"))
                    );
                    return Json(json!({
                        "request_id": plan.request_id,
                        "candidate_id": plan.candidate_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "cloudaicompanionProject": {
                                    "id": "project-from-load-code-assist"
                                },
                                "currentTier": {
                                    "id": "free"
                                }
                            }
                        }
                    }));
                }

                assert_eq!(
                    plan.url,
                    "https://cloudcode-pa.googleapis.com/v1internal:generateContent"
                );
                assert!(!plan.stream);
                assert_eq!(
                    plan.body.json_body.as_ref().unwrap()["project"],
                    json!("project-from-load-code-assist")
                );
                assert_eq!(
                    plan.body.json_body.as_ref().unwrap()["model"],
                    json!("gemini-2.5-pro")
                );
                Json(json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-gemini-cli-test-model",
                            "choices": [{
                                "message": {
                                    "role": "assistant",
                                    "content": "Hello from hydrated Gemini CLI"
                                }
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 19
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-gemini", "Gemini", 10);
    provider.provider_type = "gemini_cli".to_string();
    let mut key = sample_key(
        "key-gemini-cli",
        "provider-gemini",
        "gemini:generate_content",
        "cached-gemini-cli-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"gemini_cli","refresh_token":"rt-gemini-cli-123"}"#,
        )
        .expect("auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-cli",
            "provider-gemini",
            "gemini:generate_content",
            "https://cloudcode-pa.googleapis.com",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
                    &provider_catalog_repository,
                ))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-gemini",
            "model": "gemini-2.5-pro",
            "api_format": "gemini:generate_content"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from hydrated Gemini CLI")
    );
    assert_eq!(
        *seen_urls.lock().expect("mutex should lock"),
        vec![
            "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist".to_string(),
            "https://cloudcode-pa.googleapis.com/v1internal:generateContent".to_string(),
        ]
    );
    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-gemini-cli".to_string()])
        .await
        .expect("key should reload");
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("gemini_cli"))
            .and_then(|metadata| metadata.get("project_id")),
        Some(&json!("project-from-load-code-assist"))
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_uses_compatible_gemini_cli_endpoint_when_api_format_is_omitted() {
    run_provider_query_test(
        "gateway_uses_compatible_gemini_cli_endpoint_when_api_format_is_omitted",
        gateway_uses_compatible_gemini_cli_endpoint_when_api_format_is_omitted_impl,
    );
}

async fn gateway_uses_compatible_gemini_cli_endpoint_when_api_format_is_omitted_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:generate_content");
            assert_eq!(plan.key_id, "key-gemini-cli");
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-gemini-cli-only-endpoint",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Selected compatible Gemini CLI endpoint"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 21
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-gemini", "Gemini", 10);
    provider.provider_type = "google".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-cli",
            "provider-gemini",
            "gemini:generate_content",
            "https://generativelanguage.googleapis.com",
        )],
        vec![sample_key(
            "key-gemini-cli",
            "provider-gemini",
            "gemini:generate_content",
            "sk-test-gemini-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-gemini",
            "model": "gemini-2.5-pro"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Selected compatible Gemini CLI endpoint")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_gemini_cli_test_model_failover_locally() {
    run_provider_query_test(
        "gateway_handles_gemini_cli_test_model_failover_locally",
        gateway_handles_gemini_cli_test_model_failover_locally_impl,
    );
}

async fn gateway_handles_gemini_cli_test_model_failover_locally_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-gemini");
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.key_id, "key-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:generate_content");
            assert_eq!(plan.model_name.as_deref(), Some("gemini-2.5-pro"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-gemini-cli-failover",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Gemini CLI failover path succeeded"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 23
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-gemini", "Gemini", 10);
    provider.provider_type = "google".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-cli",
            "provider-gemini",
            "gemini:generate_content",
            "https://generativelanguage.googleapis.com",
        )],
        vec![sample_key(
            "key-gemini-cli",
            "provider-gemini",
            "gemini:generate_content",
            "sk-test-gemini-cli",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-gemini",
            "failover_models": ["gemini-2.5-pro"],
            "api_format": "gemini:generate_content"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_attempts"], json!(1));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Gemini CLI failover path succeeded")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_unwraps_gemini_cli_v1internal_response_for_failover_model_test() {
    run_provider_query_test(
        "gateway_unwraps_gemini_cli_v1internal_response_for_failover_model_test",
        gateway_unwraps_gemini_cli_v1internal_response_for_failover_model_test_impl,
    );
}

async fn gateway_unwraps_gemini_cli_v1internal_response_for_failover_model_test_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-gemini-cli");
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.key_id, "key-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:generate_content");
            assert!(!plan.stream);
            assert_eq!(
                plan.url,
                "https://cloudcode-pa.googleapis.com/v1internal:generateContent"
            );
            assert_eq!(
                plan.body.json_body.as_ref().unwrap()["project"],
                json!("project-1")
            );
            assert_eq!(
                plan.body.json_body.as_ref().unwrap()["model"],
                json!("gemini-3-flash-preview")
            );
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "response": {
                            "candidates": [{
                                "content": {
                                    "parts": [{"text":"Gemini CLI v1internal failover response"}],
                                    "role": "model"
                                },
                                "finishReason": "STOP",
                                "index": 0
                            }],
                            "modelVersion": "gemini-3-flash-preview",
                            "usageMetadata": {
                                "promptTokenCount": 2,
                                "candidatesTokenCount": 5,
                                "totalTokenCount": 7
                            }
                        },
                        "remainingCredits": 123,
                        "consumedCredits": 1,
                        "traceId": "trace-gemini-cli-1"
                    }
                },
                "telemetry": {
                    "elapsed_ms": 23
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-gemini-cli", "Gemini CLI", 10);
    provider.provider_type = "gemini_cli".to_string();
    let mut key = sample_key(
        "key-gemini-cli",
        "provider-gemini-cli",
        "gemini:generate_content",
        "cached-gemini-cli-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"gemini_cli","project_id":"project-1"}"#,
        )
        .expect("auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-cli",
            "provider-gemini-cli",
            "gemini:generate_content",
            "https://cloudcode-pa.googleapis.com",
        )],
        vec![key],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-gemini-cli",
            "failover_models": ["gemini-3-flash-preview"],
            "api_format": "gemini:generate_content"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_attempts"], json!(1));
    assert_eq!(
        payload["data"]["response"]["candidates"][0]["content"]["parts"][0]["text"],
        json!("Gemini CLI v1internal failover response")
    );
    assert!(payload["data"]["response"].get("response").is_none());

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_query_test_model_failover_with_single_model_name_alias() {
    run_provider_query_test(
        "gateway_handles_admin_provider_query_test_model_failover_with_single_model_name_alias",
        gateway_handles_admin_provider_query_test_model_failover_with_single_model_name_alias_impl,
    );
}

async fn gateway_handles_admin_provider_query_test_model_failover_with_single_model_name_alias_impl(
) {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.model_name.as_deref(), Some("gpt-4.1"));
            Json(json!({
                "request_id": plan.request_id,
                "candidate_id": plan.candidate_id,
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-alias",
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "Alias path succeeded"
                            }
                        }]
                    }
                },
                "telemetry": {
                    "elapsed_ms": 9
                }
            }))
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example/v1",
        )],
        vec![sample_key(
            "key-openai-alias",
            "provider-openai",
            "openai:chat",
            "sk-test-alias",
        )],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "failover_models": ["gpt-4.1"],
            "api_format": "openai:chat"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["model"], json!("gpt-4.1"));
    assert_eq!(payload["total_attempts"], json!(1));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Alias path succeeded")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_retries_non_kiro_failover_after_http_error_without_message() {
    run_provider_query_test(
        "gateway_retries_non_kiro_failover_after_http_error_without_message",
        gateway_retries_non_kiro_failover_after_http_error_without_message_impl,
    );
}

async fn gateway_retries_non_kiro_failover_after_http_error_without_message_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            let auth = plan
                .headers
                .get("authorization")
                .map(String::as_str)
                .unwrap_or_default()
                .to_string();
            let payload = if auth == "Bearer sk-test-first" {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 500,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {}
                    },
                    "telemetry": {
                        "elapsed_ms": 7
                    }
                })
            } else {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-retry",
                            "choices": [{
                                "message": {
                                    "role": "assistant",
                                    "content": "Recovered after empty error"
                                }
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 13
                    }
                })
            };
            Json(payload)
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example/v1",
        )],
        vec![
            sample_key(
                "key-openai-first",
                "provider-openai",
                "openai:chat",
                "sk-test-first",
            ),
            sample_key(
                "key-openai-second",
                "provider-openai",
                "openai:chat",
                "sk-test-second",
            ),
        ],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "failover_models": ["gpt-4.1"],
            "api_format": "openai:chat"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_attempts"], json!(2));
    let attempts = payload["attempts"]
        .as_array()
        .expect("attempts should be an array");
    assert_eq!(attempts[0]["status"], json!("failed"));
    assert_eq!(attempts[0]["status_code"], json!(500));
    assert_eq!(attempts[1]["status"], json!("success"));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_retries_non_kiro_failover_after_success_status_without_body() {
    run_provider_query_test(
        "gateway_retries_non_kiro_failover_after_success_status_without_body",
        gateway_retries_non_kiro_failover_after_success_status_without_body_impl,
    );
}

async fn gateway_retries_non_kiro_failover_after_success_status_without_body_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            let auth = plan
                .headers
                .get("authorization")
                .map(String::as_str)
                .unwrap_or_default()
                .to_string();
            let payload = if auth == "Bearer sk-test-first" {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {},
                    "telemetry": {
                        "elapsed_ms": 6
                    }
                })
            } else {
                json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-retry-empty-body",
                            "choices": [{
                                "message": {
                                    "role": "assistant",
                                    "content": "Recovered after empty success body"
                                }
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 14
                    }
                })
            };
            Json(payload)
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let mut provider = sample_provider("provider-openai", "OpenAI", 10);
    provider.provider_type = "openai".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-openai-chat",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example/v1",
        )],
        vec![
            sample_key(
                "key-openai-first",
                "provider-openai",
                "openai:chat",
                "sk-test-first",
            ),
            sample_key(
                "key-openai-second",
                "provider-openai",
                "openai:chat",
                "sk-test-second",
            ),
        ],
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(GatewayDataState::with_provider_transport_reader_for_tests(
                provider_catalog_repository,
                DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-query/test-model-failover"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-openai",
            "failover_models": ["gpt-4.1"],
            "api_format": "openai:chat"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["total_attempts"], json!(2));
    assert_eq!(
        payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Recovered after empty success body")
    );
    let attempts = payload["attempts"]
        .as_array()
        .expect("attempts should be an array");
    assert_eq!(attempts[0]["status"], json!("failed"));
    assert_eq!(attempts[0]["status_code"], json!(200));
    assert_eq!(
        attempts[0]["error_message"],
        json!("Provider returned HTTP 200 without a model-test response body")
    );
    assert_eq!(attempts[1]["status"], json!("success"));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_preserves_non_success_status_for_test_model_local_wrapper() {
    run_provider_query_test(
        "gateway_preserves_non_success_status_for_test_model_local_wrapper",
        gateway_preserves_non_success_status_for_test_model_local_wrapper_impl,
    );
}

async fn gateway_preserves_non_success_status_for_test_model_local_wrapper_impl() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": "provider-does-not-exist",
            "model": "gpt-4.1"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], json!("Provider not found"));

    gateway_handle.abort();
}

#[test]
fn gateway_rejects_admin_provider_query_invalid_json_body() {
    run_provider_query_test(
        "gateway_rejects_admin_provider_query_invalid_json_body",
        gateway_rejects_admin_provider_query_invalid_json_body_impl,
    );
}

async fn gateway_rejects_admin_provider_query_invalid_json_body_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-query/models",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/provider-query/models"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], json!("Invalid JSON request body"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_rejects_admin_provider_query_test_model_without_provider_id() {
    run_provider_query_test(
        "gateway_rejects_admin_provider_query_test_model_without_provider_id",
        gateway_rejects_admin_provider_query_test_model_without_provider_id_impl,
    );
}

async fn gateway_rejects_admin_provider_query_test_model_without_provider_id_impl() {
    assert_admin_provider_query_route(
        "/api/admin/provider-query/test-model",
        json!({ "model": "gpt-4.1" }),
        StatusCode::BAD_REQUEST,
        |payload| {
            assert_eq!(payload["detail"], json!("provider_id is required"));
        },
    )
    .await;
}

#[test]
fn gateway_rejects_admin_provider_query_test_model_without_model() {
    run_provider_query_test(
        "gateway_rejects_admin_provider_query_test_model_without_model",
        gateway_rejects_admin_provider_query_test_model_without_model_impl,
    );
}

async fn gateway_rejects_admin_provider_query_test_model_without_model_impl() {
    assert_admin_provider_query_route(
        "/api/admin/provider-query/test-model",
        json!({ "provider_id": "provider-openai" }),
        StatusCode::BAD_REQUEST,
        |payload| {
            assert_eq!(payload["detail"], json!("model is required"));
        },
    )
    .await;
}

#[test]
fn gateway_rejects_admin_provider_query_test_model_failover_without_provider_id() {
    run_provider_query_test(
        "gateway_rejects_admin_provider_query_test_model_failover_without_provider_id",
        gateway_rejects_admin_provider_query_test_model_failover_without_provider_id_impl,
    );
}

async fn gateway_rejects_admin_provider_query_test_model_failover_without_provider_id_impl() {
    assert_admin_provider_query_route(
        "/api/admin/provider-query/test-model-failover",
        json!({ "failover_models": ["gpt-4.1"] }),
        StatusCode::BAD_REQUEST,
        |payload| {
            assert_eq!(payload["detail"], json!("provider_id is required"));
        },
    )
    .await;
}

#[test]
fn gateway_rejects_admin_provider_query_test_model_failover_without_models() {
    run_provider_query_test(
        "gateway_rejects_admin_provider_query_test_model_failover_without_models",
        gateway_rejects_admin_provider_query_test_model_failover_without_models_impl,
    );
}

async fn gateway_rejects_admin_provider_query_test_model_failover_without_models_impl() {
    assert_admin_provider_query_route(
        "/api/admin/provider-query/test-model-failover",
        json!({ "provider_id": "provider-openai", "failover_models": [] }),
        StatusCode::BAD_REQUEST,
        |payload| {
            assert_eq!(
                payload["detail"],
                json!("failover_models should not be empty")
            );
        },
    )
    .await;
}
