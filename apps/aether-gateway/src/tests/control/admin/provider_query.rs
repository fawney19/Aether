use std::sync::{Arc, Mutex};

use aether_contracts::ExecutionPlan;
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use base64::Engine as _;
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, build_state_with_execution_runtime_override, sample_endpoint,
    sample_key, sample_provider, start_server, AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

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

#[tokio::test]
async fn gateway_handles_admin_provider_query_models_fetches_upstream_for_selected_key() {
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
            "https://api.openai.example".to_string(),
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

#[tokio::test]
async fn gateway_handles_admin_provider_query_models_with_openai_responses_endpoint() {
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
            "https://api.openai.example".to_string(),
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
    assert_eq!(payload["success"], json!(false));
    assert_eq!(
        payload["data"]["error"],
        json!("No active endpoints found for this provider")
    );
    assert_eq!(payload["data"]["from_cache"], json!(false));
    assert_eq!(payload["data"]["models"], json!([]));
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        0
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_query_models_respecting_key_api_formats() {
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
                assert_eq!(plan.provider_api_format, "openai:cli");
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
                "https://api.openai.example".to_string(),
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
                "openai:cli".to_string(),
                Some("cli".to_string()),
                Some("secondary".to_string()),
                true,
            )
            .expect("endpoint should build")
            .with_transport_fields(
                "https://api.openai.example".to_string(),
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
            "openai:cli",
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
        json!(["openai:cli"])
    );
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_provider_query_models_aggregating_active_keys() {
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
            "https://api.openai.example".to_string(),
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

#[tokio::test]
async fn gateway_handles_admin_provider_query_models_for_fixed_provider_without_endpoint() {
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
            "openai:cli",
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

#[tokio::test]
async fn gateway_handles_admin_provider_query_test_model_locally_with_trusted_admin_principal() {
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
            "https://api.openai.example",
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

#[tokio::test]
async fn gateway_handles_admin_provider_query_test_model_failover_locally_with_trusted_admin_principal(
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
            "https://api.openai.example",
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

#[tokio::test]
async fn gateway_handles_admin_provider_query_test_model_for_kiro_locally() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-kiro");
            assert_eq!(plan.endpoint_id, "endpoint-kiro-cli");
            assert_eq!(plan.key_id, "key-kiro-primary");
            assert_eq!(plan.provider_api_format, "claude:cli");
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
        "claude:cli",
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
            "claude:cli".to_string(),
            Some("claude".to_string()),
            Some("cli".to_string()),
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
            "api_format": "claude:cli"
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

#[tokio::test]
async fn gateway_handles_admin_provider_query_test_model_failover_for_kiro_locally() {
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
        let mut key = sample_key(id, "provider-kiro", "claude:cli", "__placeholder__");
        key.auth_type = "oauth".to_string();
        key.encrypted_auth_config = Some(
            aether_crypto::encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{
                    "provider_type":"kiro",
                    "auth_method":"idc",
                    "access_token":"cached-kiro-token",
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
            "claude:cli".to_string(),
            Some("claude".to_string()),
            Some("cli".to_string()),
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
            "api_format": "claude:cli",
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

#[tokio::test]
async fn gateway_retries_kiro_failover_after_http_error_without_message() {
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
        let mut key = sample_key(id, "provider-kiro", "claude:cli", "__placeholder__");
        key.auth_type = "oauth".to_string();
        key.encrypted_auth_config = Some(
            aether_crypto::encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{
                    "provider_type":"kiro",
                    "auth_method":"idc",
                    "access_token":"cached-kiro-token",
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
            "claude:cli".to_string(),
            Some("claude".to_string()),
            Some("cli".to_string()),
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
            "api_format": "claude:cli",
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

#[tokio::test]
async fn gateway_handles_non_kiro_multi_model_failover_locally() {
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
            "https://api.openai.example",
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

#[tokio::test]
async fn gateway_handles_openai_cli_test_model_locally() {
    let prompt = "Tell me whether the CLI request preserved this prompt.";
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-openai");
            assert_eq!(plan.endpoint_id, "endpoint-openai-cli");
            assert_eq!(plan.key_id, "key-openai-cli");
            assert_eq!(plan.provider_api_format, "openai:cli");
            assert_eq!(plan.url, "https://tiger.bookapi.cc/codex/responses");
            assert!(!plan.stream);
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
            assert_eq!(
                plan.body
                    .json_body
                    .as_ref()
                    .and_then(|body| body.get("instructions")),
                Some(&json!("You are ChatGPT."))
            );
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
                .is_some());
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
                                "content": "Hello from OpenAI CLI"
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
            "openai:cli",
            "https://tiger.bookapi.cc/codex",
        )],
        vec![sample_key(
            "key-openai-cli",
            "provider-openai",
            "openai:cli",
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
            "api_format": "openai:cli",
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
        json!("Hello from OpenAI CLI")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_returns_stub_for_transport_unsupported_non_kiro_provider() {
    let mut provider = sample_provider("provider-antigravity", "Antigravity", 10);
    provider.provider_type = "antigravity".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-antigravity-gemini",
            "provider-antigravity",
            "gemini:chat",
            "https://cloudcode-pa.googleapis.com",
        )],
        vec![sample_key(
            "key-antigravity-gemini",
            "provider-antigravity",
            "gemini:chat",
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
            "api_format": "gemini:chat"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], json!(false));
    assert_eq!(
        payload["error"],
        json!("Rust local provider-query failover simulation is not configured")
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_prefers_supported_non_kiro_endpoint_when_api_format_is_omitted() {
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
                "openai:cli",
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
            ),
        ],
        vec![
            sample_key(
                "key-openai-cli",
                "provider-openai",
                "openai:cli",
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

#[tokio::test]
async fn gateway_prefers_transport_supported_non_kiro_endpoint_when_api_format_is_omitted() {
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
        "https://api.openai.example",
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
                "https://api.openai.example",
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

#[tokio::test]
async fn gateway_prefers_supported_non_kiro_endpoint_with_compatible_key_when_api_format_is_omitted(
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
                "gemini:chat",
                "https://api.gemini.example",
            ),
            sample_endpoint(
                "endpoint-openai-chat",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
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

#[tokio::test]
async fn gateway_uses_compatible_cli_endpoint_when_api_format_is_omitted() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-openai-cli");
            assert_eq!(plan.provider_api_format, "openai:cli");
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
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-openai-cli",
                "provider-openai",
                "openai:cli",
                "https://api.openai.example",
            ),
        ],
        vec![sample_key(
            "key-openai-cli",
            "provider-openai",
            "openai:cli",
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

#[tokio::test]
async fn gateway_uses_runnable_cli_endpoint_after_chat_preference_when_api_format_is_omitted() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-openai-cli-runnable");
            assert_eq!(plan.provider_api_format, "openai:cli");
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
        "https://api.openai.example",
    );
    unsupported_chat_endpoint.header_rules = Some(json!({"invalid": true}));
    let cli_endpoint = sample_endpoint(
        "endpoint-openai-cli-runnable",
        "provider-openai",
        "openai:cli",
        "https://api.openai.example",
    );
    let mut shared_key = sample_key(
        "key-openai-shared",
        "provider-openai",
        "openai:chat",
        "sk-test-shared",
    );
    shared_key.api_formats = Some(json!(["openai:chat", "openai:cli"]));
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

#[tokio::test]
async fn gateway_handles_openai_cli_test_model_failover_locally() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-openai");
            assert_eq!(plan.endpoint_id, "endpoint-openai-cli");
            assert_eq!(plan.key_id, "key-openai-cli");
            assert_eq!(plan.provider_api_format, "openai:cli");
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
                                "content": "OpenAI CLI failover path succeeded"
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
            "openai:cli",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-openai-cli",
            "provider-openai",
            "openai:cli",
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
            "api_format": "openai:cli"
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
        json!("OpenAI CLI failover path succeeded")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_handles_claude_cli_test_model_locally() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-claude");
            assert_eq!(plan.endpoint_id, "endpoint-claude-cli");
            assert_eq!(plan.key_id, "key-claude-cli");
            assert_eq!(plan.provider_api_format, "claude:cli");
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
            "claude:cli",
            "https://api.anthropic.example",
        )],
        vec![sample_key(
            "key-claude-cli",
            "provider-claude",
            "claude:cli",
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
            "api_format": "claude:cli"
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

#[tokio::test]
async fn gateway_uses_compatible_claude_cli_endpoint_when_api_format_is_omitted() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-claude-cli");
            assert_eq!(plan.provider_api_format, "claude:cli");
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
            "claude:cli",
            "https://api.anthropic.example",
        )],
        vec![sample_key(
            "key-claude-cli",
            "provider-claude",
            "claude:cli",
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

#[tokio::test]
async fn gateway_handles_claude_cli_test_model_failover_locally() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-claude");
            assert_eq!(plan.endpoint_id, "endpoint-claude-cli");
            assert_eq!(plan.key_id, "key-claude-cli");
            assert_eq!(plan.provider_api_format, "claude:cli");
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
            "claude:cli",
            "https://api.anthropic.example",
        )],
        vec![sample_key(
            "key-claude-cli",
            "provider-claude",
            "claude:cli",
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
            "api_format": "claude:cli"
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

#[tokio::test]
async fn gateway_handles_gemini_cli_test_model_locally() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-gemini");
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.key_id, "key-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:cli");
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
            "gemini:cli",
            "https://generativelanguage.googleapis.com",
        )],
        vec![sample_key(
            "key-gemini-cli",
            "provider-gemini",
            "gemini:cli",
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
            "api_format": "gemini:cli"
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

#[tokio::test]
async fn gateway_handles_gemini_cli_test_model_with_oauth_header_fallback() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-gemini");
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.key_id, "key-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:cli");
            assert_eq!(
                plan.url,
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent"
            );
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
        "gemini:cli",
        "cached-gemini-cli-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        aether_crypto::encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"gemini_cli"}"#,
        )
        .expect("auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-cli",
            "provider-gemini",
            "gemini:cli",
            "https://generativelanguage.googleapis.com",
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
            "api_format": "gemini:cli"
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

#[tokio::test]
async fn gateway_uses_compatible_gemini_cli_endpoint_when_api_format_is_omitted() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:cli");
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
            "gemini:cli",
            "https://generativelanguage.googleapis.com",
        )],
        vec![sample_key(
            "key-gemini-cli",
            "provider-gemini",
            "gemini:cli",
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

#[tokio::test]
async fn gateway_handles_gemini_cli_test_model_failover_locally() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| async move {
            assert_eq!(plan.provider_id, "provider-gemini");
            assert_eq!(plan.endpoint_id, "endpoint-gemini-cli");
            assert_eq!(plan.key_id, "key-gemini-cli");
            assert_eq!(plan.provider_api_format, "gemini:cli");
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
            "gemini:cli",
            "https://generativelanguage.googleapis.com",
        )],
        vec![sample_key(
            "key-gemini-cli",
            "provider-gemini",
            "gemini:cli",
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
            "api_format": "gemini:cli"
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

#[tokio::test]
async fn gateway_handles_admin_provider_query_test_model_failover_with_single_model_name_alias() {
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
            "https://api.openai.example",
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

#[tokio::test]
async fn gateway_retries_non_kiro_failover_after_http_error_without_message() {
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
            "https://api.openai.example",
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

#[tokio::test]
async fn gateway_preserves_non_success_status_for_test_model_local_wrapper() {
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

#[tokio::test]
async fn gateway_rejects_admin_provider_query_invalid_json_body() {
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

#[tokio::test]
async fn gateway_rejects_admin_provider_query_test_model_without_provider_id() {
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

#[tokio::test]
async fn gateway_rejects_admin_provider_query_test_model_without_model() {
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

#[tokio::test]
async fn gateway_rejects_admin_provider_query_test_model_failover_without_provider_id() {
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

#[tokio::test]
async fn gateway_rejects_admin_provider_query_test_model_failover_without_models() {
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
