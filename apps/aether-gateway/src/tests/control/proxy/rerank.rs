use std::collections::BTreeMap;
use std::sync::Arc;

use aether_contracts::{ExecutionPlan, ExecutionResult, ResponseBody};
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use http::StatusCode;
use serde_json::json;

use super::super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, hash_api_key,
    sample_currently_usable_auth_snapshot, sample_endpoint, sample_key, sample_provider,
    start_server, AppState, GatewayDataState, InMemoryAuthApiKeySnapshotRepository,
    InMemoryProviderCatalogReadRepository, Json, Router,
};
use crate::constants::{
    CONTROL_ENDPOINT_SIGNATURE_HEADER, CONTROL_EXECUTION_RUNTIME_HEADER,
    CONTROL_ROUTE_FAMILY_HEADER, CONTROL_ROUTE_KIND_HEADER, EXECUTION_PATH_HEADER,
    EXECUTION_PATH_LOCAL_AUTH_DENIED,
};

fn rerank_success_state(execution_runtime_url: String) -> AppState {
    let mut snapshot =
        sample_currently_usable_auth_snapshot("key-rerank-success", "user-rerank-success");
    snapshot.user_allowed_providers = None;
    snapshot.api_key_allowed_providers = None;
    snapshot.user_allowed_api_formats = Some(vec!["openai:rerank".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:rerank".to_string()]);
    snapshot.user_allowed_models = Some(vec!["bge-reranker-base".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["bge-reranker-base".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-rerank-success")),
        snapshot,
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            rerank_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-rerank", "OpenAI Rerank", 1)],
        vec![sample_endpoint(
            "endpoint-rerank",
            "provider-rerank",
            "openai:rerank",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-upstream-rerank",
            "provider-rerank",
            "openai:rerank",
            "sk-upstream-rerank",
        )],
    ));
    let data_state =
        GatewayDataState::with_provider_catalog_and_minimal_candidate_selection_for_tests(
            provider_catalog_repository,
            candidate_repository,
        )
        .with_auth_api_key_reader(auth_repository)
        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(data_state)
}

fn rerank_execution_runtime() -> Router {
    Router::new().route(
        "/v1/execute/sync",
        any(|Json(plan): Json<ExecutionPlan>| async move {
            assert_rerank_execution_plan(&plan);
            Json(rerank_execution_result(&plan))
        }),
    )
}

fn rerank_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-rerank".to_string(),
        provider_name: "OpenAI Rerank".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 1,
        provider_is_active: true,
        endpoint_id: "endpoint-rerank".to_string(),
        endpoint_api_format: "openai:rerank".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("rerank".to_string()),
        endpoint_is_active: true,
        key_id: "key-upstream-rerank".to_string(),
        key_name: "default".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:rerank".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 50,
        key_global_priority_by_format: None,
        model_id: "model-rerank-base".to_string(),
        global_model_id: "global-rerank-base".to_string(),
        global_model_name: "bge-reranker-base".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(false),
        model_provider_model_name: "upstream-rerank".to_string(),
        model_provider_model_mappings: None,
        model_supports_streaming: Some(false),
        model_is_active: true,
        model_is_available: true,
    }
}

fn assert_rerank_execution_plan(plan: &ExecutionPlan) {
    assert_eq!(plan.client_api_format, "openai:rerank");
    assert_eq!(plan.provider_api_format, "openai:rerank");
    assert_eq!(plan.method, "POST");
    assert_eq!(plan.url, "https://api.openai.example/rerank");
    assert_eq!(plan.model_name.as_deref(), Some("bge-reranker-base"));
    let body = plan.body.json_body.as_ref().expect("json request body");
    assert_eq!(body["model"], "upstream-rerank");
    assert_eq!(body["query"], "hello");
    assert_eq!(body["documents"], json!(["hello world", "goodbye"]));
    assert_eq!(body["top_n"], 1);
}

fn rerank_execution_result(plan: &ExecutionPlan) -> ExecutionResult {
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(ResponseBody {
            json_body: Some(json!({
                "model": "upstream-rerank",
                "results": [
                    {"index": 0, "relevance_score": 0.98, "document": {"text": "hello world"}}
                ],
                "usage": {"total_tokens": 8}
            })),
            body_bytes_b64: None,
        }),
        telemetry: None,
        error: None,
    }
}

#[tokio::test]
async fn rerank_route_accepts_openai_payload() {
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(rerank_execution_runtime()).await;
    let gateway = build_router_with_state(rerank_success_state(execution_runtime_url));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/rerank"))
        .header(http::header::AUTHORIZATION, "Bearer sk-rerank-success")
        .json(&json!({
            "model": "bge-reranker-base",
            "query": "hello",
            "documents": ["hello world", "goodbye"],
            "top_n": 1,
            "return_documents": true
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ROUTE_FAMILY_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("openai")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ROUTE_KIND_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("rerank")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ENDPOINT_SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("openai:rerank")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTION_RUNTIME_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["results"][0]["index"], 0);
    assert_eq!(payload["results"][0]["relevance_score"], 0.98);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn rerank_route_rejects_invalid_local_payloads() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let cases = [
        ("{", "Rerank request JSON body is invalid"),
        (
            r#"{"query":"hello","documents":["doc"]}"#,
            "Rerank request model is required",
        ),
        (
            r#"{"model":"bge-reranker-base","documents":["doc"]}"#,
            "Rerank request query is required",
        ),
        (
            r#"{"model":"bge-reranker-base","query":"hello","documents":[]}"#,
            "Rerank request documents are required",
        ),
        (
            r#"{"model":"bge-reranker-base","query":"hello","messages":[]}"#,
            "Rerank request must use query/documents, not chat messages",
        ),
        (
            r#"{"model":"bge-reranker-base","query":"hello","documents":["doc"],"top_n":0}"#,
            "Rerank request top_n must be a positive integer",
        ),
        (
            r#"{"model":"bge-reranker-base","query":"hello","documents":["doc"],"stream":true}"#,
            "Rerank requests do not support streaming",
        ),
    ];

    for (body, expected_detail) in cases {
        let response = client
            .post(format!("{gateway_url}/v1/rerank"))
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(CONTROL_ROUTE_KIND_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("rerank")
        );
        let payload: serde_json::Value = response.json().await.expect("body should parse");
        assert_eq!(payload["detail"], expected_detail);
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn rerank_route_rejects_non_json_content_type() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/rerank"))
        .header(http::header::CONTENT_TYPE, "text/plain")
        .body(r#"{"model":"bge-reranker-base","query":"hello","documents":["doc"]}"#)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(
        payload["detail"],
        "Rerank request content-type must be application/json"
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn rerank_route_rejects_chat_only_api_format() {
    let mut snapshot = sample_currently_usable_auth_snapshot("key-rerank-2", "user-rerank-2");
    snapshot.user_allowed_api_formats = Some(vec!["openai:chat".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:chat".to_string()]);
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-rerank-format-guard")),
        snapshot,
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/rerank"))
        .header(http::header::AUTHORIZATION, "Bearer sk-rerank-format-guard")
        .json(&json!({
            "model": "bge-reranker-base",
            "query": "hello",
            "documents": ["doc"]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(
        payload["error"]["message"],
        "当前用户、用户组或密钥的访问控制策略不允许访问 openai:rerank 格式"
    );

    gateway_handle.abort();
}
