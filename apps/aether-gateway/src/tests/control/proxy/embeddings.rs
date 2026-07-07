use std::collections::BTreeMap;
use std::sync::Arc;

use aether_contracts::{ExecutionPlan, ExecutionResult, ResponseBody};
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
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
use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;

const EMBEDDING_PROXY_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_embedding_proxy_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(EMBEDDING_PROXY_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("embedding proxy test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn embedding_success_state(execution_runtime_url: String) -> AppState {
    let mut snapshot =
        sample_currently_usable_auth_snapshot("key-embedding-success", "user-embedding-success");
    snapshot.user_allowed_providers = None;
    snapshot.api_key_allowed_providers = None;
    snapshot.user_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.user_allowed_models = Some(vec!["text-embedding-3-small".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["text-embedding-3-small".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-embedding-success")),
        snapshot,
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            embedding_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider(
            "provider-embedding",
            "OpenAI Embeddings",
            1,
        )],
        vec![sample_endpoint(
            "endpoint-embedding",
            "provider-embedding",
            "openai:embedding",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-upstream-embedding",
            "provider-embedding",
            "openai:embedding",
            "sk-upstream-embedding",
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

fn embedding_execution_runtime() -> Router {
    Router::new().route(
        "/v1/execute/sync",
        any(|Json(plan): Json<ExecutionPlan>| async move {
            assert_embedding_execution_plan(&plan);
            Json(embedding_execution_result(&plan))
        }),
    )
}

fn gemini_embedding_success_state(
    execution_runtime_url: String,
    client_api_format: &str,
) -> AppState {
    let mut snapshot = sample_currently_usable_auth_snapshot(
        "key-gemini-embedding-success",
        "user-gemini-embedding-success",
    );
    snapshot.user_allowed_providers = None;
    snapshot.api_key_allowed_providers = None;
    snapshot.user_allowed_api_formats = Some(vec![client_api_format.to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec![client_api_format.to_string()]);
    snapshot.user_allowed_models = Some(vec!["gemini-embedding-2-preview".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["gemini-embedding-2-preview".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-gemini-embedding-success")),
        snapshot,
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            gemini_embedding_candidate_row(),
        ]));
    let mut provider = sample_provider("provider-gemini-embedding", "Gemini Embeddings", 1);
    provider.provider_type = "gemini".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-gemini-embedding",
            "provider-gemini-embedding",
            "gemini:embedding",
            "https://generativelanguage.googleapis.com/v1beta",
        )],
        vec![sample_key(
            "key-upstream-gemini-embedding",
            "provider-gemini-embedding",
            "gemini:embedding",
            "sk-upstream-gemini-embedding",
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

fn vertex_gemini_embedding_success_state(execution_runtime_url: String) -> AppState {
    let mut snapshot = sample_currently_usable_auth_snapshot(
        "key-vertex-gemini-embedding-success",
        "user-vertex-gemini-embedding-success",
    );
    snapshot.user_allowed_providers = None;
    snapshot.api_key_allowed_providers = Some(vec!["openai".to_string(), "vertex_ai".to_string()]);
    snapshot.user_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.user_allowed_models = Some(vec!["gemini-embedding-2-preview".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["gemini-embedding-2-preview".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-vertex-gemini-embedding-success")),
        snapshot,
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            vertex_gemini_embedding_candidate_row(),
        ]));
    let mut provider = sample_provider("provider-vertex-gemini-embedding", "Vertex AI", 1);
    provider.provider_type = "vertex_ai".to_string();
    let mut key = sample_key(
        "key-upstream-vertex-gemini-embedding",
        "provider-vertex-gemini-embedding",
        "gemini:embedding",
        "sk-upstream-vertex-gemini-embedding",
    );
    key.allowed_models = Some(json!(["gemini-embedding-2"]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-vertex-gemini-embedding",
            "provider-vertex-gemini-embedding",
            "gemini:embedding",
            "https://aiplatform.googleapis.com",
        )],
        vec![key],
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

fn aliyun_embedding_success_state(execution_runtime_url: String) -> AppState {
    let mut snapshot = sample_currently_usable_auth_snapshot(
        "key-aliyun-embedding-success",
        "user-aliyun-embedding-success",
    );
    snapshot.user_allowed_providers = None;
    snapshot.api_key_allowed_providers = None;
    snapshot.user_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.user_allowed_models = Some(vec!["qwen3-vl-embedding".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["qwen3-vl-embedding".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-aliyun-embedding-success")),
        snapshot,
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            aliyun_embedding_candidate_row(),
        ]));
    let mut provider = sample_provider("provider-aliyun-embedding", "Aliyun DashScope", 1);
    provider.provider_type = "aliyun".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-aliyun-embedding",
            "provider-aliyun-embedding",
            "aliyun:multimodal_embedding",
            "https://dashscope.aliyuncs.com",
        )],
        vec![sample_key(
            "key-upstream-aliyun-embedding",
            "provider-aliyun-embedding",
            "aliyun:multimodal_embedding",
            "sk-upstream-aliyun-embedding",
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

fn mixed_embedding_success_state(execution_runtime_url: String) -> AppState {
    let mut snapshot = sample_currently_usable_auth_snapshot(
        "key-mixed-embedding-success",
        "user-mixed-embedding-success",
    );
    snapshot.user_allowed_providers = None;
    snapshot.api_key_allowed_providers = None;
    snapshot.user_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.user_allowed_models = Some(vec!["qwen3-vl-embedding".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["qwen3-vl-embedding".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-mixed-embedding-success")),
        snapshot,
    )]));

    let mut openai_candidate = embedding_candidate_row();
    openai_candidate.model_id = "model-openai-qwen-vl-embedding".to_string();
    openai_candidate.global_model_id = "global-qwen3-vl-embedding".to_string();
    openai_candidate.global_model_name = "qwen3-vl-embedding".to_string();
    openai_candidate.model_provider_model_name = "openai-qwen-fallback".to_string();
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            openai_candidate,
            aliyun_embedding_candidate_row(),
        ]));

    let mut aliyun_provider = sample_provider("provider-aliyun-embedding", "Aliyun DashScope", 1);
    aliyun_provider.provider_type = "aliyun".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-embedding", "OpenAI Embeddings", 1),
            aliyun_provider,
        ],
        vec![
            sample_endpoint(
                "endpoint-embedding",
                "provider-embedding",
                "openai:embedding",
                "https://api.openai.example",
            ),
            sample_endpoint(
                "endpoint-aliyun-embedding",
                "provider-aliyun-embedding",
                "aliyun:multimodal_embedding",
                "https://dashscope.aliyuncs.com",
            ),
        ],
        vec![
            sample_key(
                "key-upstream-embedding",
                "provider-embedding",
                "openai:embedding",
                "sk-upstream-embedding",
            ),
            sample_key(
                "key-upstream-aliyun-embedding",
                "provider-aliyun-embedding",
                "aliyun:multimodal_embedding",
                "sk-upstream-aliyun-embedding",
            ),
        ],
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

fn gemini_embedding_conversion_execution_runtime() -> Router {
    Router::new().route(
        "/v1/execute/sync",
        any(|Json(plan): Json<ExecutionPlan>| async move {
            assert_openai_to_gemini_embedding_execution_plan(&plan);
            Json(gemini_embedding_execution_result(&plan))
        }),
    )
}

fn aliyun_embedding_conversion_execution_runtime(
    expected_contents: serde_json::Value,
    expected_parameters: Option<serde_json::Value>,
) -> Router {
    let expected_contents = Arc::new(expected_contents);
    let expected_parameters = Arc::new(expected_parameters);
    Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let expected_contents = Arc::clone(&expected_contents);
            let expected_parameters = Arc::clone(&expected_parameters);
            async move {
                assert_openai_to_aliyun_embedding_execution_plan(
                    &plan,
                    &expected_contents,
                    expected_parameters.as_ref().as_ref(),
                );
                Json(aliyun_embedding_execution_result(&plan))
            }
        }),
    )
}

fn vertex_gemini_embedding_conversion_execution_runtime() -> Router {
    Router::new().route(
        "/v1/execute/sync",
        any(|Json(plan): Json<ExecutionPlan>| async move {
            assert_openai_to_vertex_gemini_embedding_execution_plan(&plan);
            Json(vertex_gemini_embedding_execution_result(&plan))
        }),
    )
}

fn gemini_embedding_batch_conversion_execution_runtime() -> Router {
    Router::new().route(
        "/v1/execute/sync",
        any(|Json(plan): Json<ExecutionPlan>| async move {
            assert_openai_to_gemini_batch_embedding_execution_plan(&plan);
            Json(gemini_batch_embedding_execution_result(&plan))
        }),
    )
}

fn gemini_embedding_native_execution_runtime() -> Router {
    Router::new().route(
        "/v1/execute/sync",
        any(|Json(plan): Json<ExecutionPlan>| async move {
            assert_native_gemini_embedding_execution_plan(&plan);
            Json(gemini_embedding_execution_result(&plan))
        }),
    )
}

fn embedding_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-embedding".to_string(),
        provider_name: "OpenAI Embeddings".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 1,
        provider_is_active: true,
        endpoint_id: "endpoint-embedding".to_string(),
        endpoint_api_format: "openai:embedding".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("embedding".to_string()),
        endpoint_is_active: true,
        key_id: "key-upstream-embedding".to_string(),
        key_name: "default".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:embedding".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 50,
        key_global_priority_by_format: None,
        model_id: "model-embedding-small".to_string(),
        global_model_id: "global-embedding-small".to_string(),
        global_model_name: "text-embedding-3-small".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(false),
        model_provider_model_name: "upstream-embedding".to_string(),
        model_provider_model_mappings: None,
        model_supports_streaming: Some(false),
        model_is_active: true,
        model_is_available: true,
    }
}

fn gemini_embedding_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-gemini-embedding".to_string(),
        provider_name: "Gemini Embeddings".to_string(),
        provider_type: "gemini".to_string(),
        provider_priority: 1,
        provider_is_active: true,
        endpoint_id: "endpoint-gemini-embedding".to_string(),
        endpoint_api_format: "gemini:embedding".to_string(),
        endpoint_api_family: Some("gemini".to_string()),
        endpoint_kind: Some("embedding".to_string()),
        endpoint_is_active: true,
        key_id: "key-upstream-gemini-embedding".to_string(),
        key_name: "default".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["gemini:embedding".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 50,
        key_global_priority_by_format: None,
        model_id: "model-gemini-embedding-preview".to_string(),
        global_model_id: "global-gemini-embedding-preview".to_string(),
        global_model_name: "gemini-embedding-2-preview".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(false),
        model_provider_model_name: "gemini-embedding-2-preview".to_string(),
        model_provider_model_mappings: None,
        model_supports_streaming: Some(false),
        model_is_active: true,
        model_is_available: true,
    }
}

fn vertex_gemini_embedding_candidate_row() -> StoredMinimalCandidateSelectionRow {
    let mut row = gemini_embedding_candidate_row();
    row.provider_id = "provider-vertex-gemini-embedding".to_string();
    row.provider_name = "Vertex AI".to_string();
    row.provider_type = "vertex_ai".to_string();
    row.endpoint_id = "endpoint-vertex-gemini-embedding".to_string();
    row.key_id = "key-upstream-vertex-gemini-embedding".to_string();
    row.key_name = "default".to_string();
    row.key_allowed_models = Some(vec!["gemini-embedding-2".to_string()]);
    row.model_provider_model_name = "gemini-embedding-2".to_string();
    row
}

fn aliyun_embedding_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-aliyun-embedding".to_string(),
        provider_name: "Aliyun DashScope".to_string(),
        provider_type: "aliyun".to_string(),
        provider_priority: 1,
        provider_is_active: true,
        endpoint_id: "endpoint-aliyun-embedding".to_string(),
        endpoint_api_format: "aliyun:multimodal_embedding".to_string(),
        endpoint_api_family: Some("aliyun".to_string()),
        endpoint_kind: Some("multimodal_embedding".to_string()),
        endpoint_is_active: true,
        key_id: "key-upstream-aliyun-embedding".to_string(),
        key_name: "default".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["aliyun:multimodal_embedding".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 50,
        key_global_priority_by_format: None,
        model_id: "model-qwen3-vl-embedding".to_string(),
        global_model_id: "global-qwen3-vl-embedding".to_string(),
        global_model_name: "qwen3-vl-embedding".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(false),
        model_provider_model_name: "qwen3-vl-embedding".to_string(),
        model_provider_model_mappings: None,
        model_supports_streaming: Some(false),
        model_is_active: true,
        model_is_available: true,
    }
}

fn assert_embedding_execution_plan(plan: &ExecutionPlan) {
    assert_eq!(plan.client_api_format, "openai:embedding");
    assert_eq!(plan.provider_api_format, "openai:embedding");
    assert_eq!(plan.method, "POST");
    assert_eq!(plan.url, "https://api.openai.example/embeddings");
    assert_eq!(plan.model_name.as_deref(), Some("text-embedding-3-small"));
    let body = plan.body.json_body.as_ref().expect("json request body");
    assert_eq!(body["model"], "upstream-embedding");
    assert!(body.get("input").is_some());
}

fn assert_openai_to_aliyun_embedding_execution_plan(
    plan: &ExecutionPlan,
    expected_contents: &serde_json::Value,
    expected_parameters: Option<&serde_json::Value>,
) {
    assert_eq!(plan.client_api_format, "openai:embedding");
    assert_eq!(plan.provider_api_format, "aliyun:multimodal_embedding");
    assert_eq!(plan.method, "POST");
    assert_eq!(
        plan.url,
        "https://dashscope.aliyuncs.com/api/v1/services/embeddings/multimodal-embedding/multimodal-embedding"
    );
    assert_eq!(
        plan.headers.get("authorization").map(String::as_str),
        Some("Bearer sk-upstream-aliyun-embedding")
    );
    assert_eq!(plan.model_name.as_deref(), Some("qwen3-vl-embedding"));
    assert!(!plan.stream);
    let body = plan.body.json_body.as_ref().expect("json request body");
    assert_eq!(body["model"], "qwen3-vl-embedding");
    assert_eq!(&body["input"]["contents"], expected_contents);
    match expected_parameters {
        Some(expected) => assert_eq!(&body["parameters"], expected),
        None => assert!(body.get("parameters").is_none()),
    }
    assert!(body.get("messages").is_none());
}

fn assert_openai_to_gemini_embedding_execution_plan(plan: &ExecutionPlan) {
    assert_eq!(plan.client_api_format, "openai:embedding");
    assert_eq!(plan.provider_api_format, "gemini:embedding");
    assert_eq!(plan.method, "POST");
    assert_eq!(
        plan.url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2-preview:embedContent"
    );
    assert_eq!(
        plan.headers.get("x-goog-api-key").map(String::as_str),
        Some("sk-upstream-gemini-embedding")
    );
    assert_eq!(
        plan.model_name.as_deref(),
        Some("gemini-embedding-2-preview")
    );
    assert!(!plan.stream);
    let body = plan.body.json_body.as_ref().expect("json request body");
    assert_eq!(body["model"], "gemini-embedding-2-preview");
    assert_eq!(body["content"]["parts"][0]["text"], "hello");
    assert!(body.get("input").is_none());
    assert!(body.get("messages").is_none());
}

fn assert_openai_to_vertex_gemini_embedding_execution_plan(plan: &ExecutionPlan) {
    assert_eq!(plan.provider_id, "provider-vertex-gemini-embedding");
    assert_eq!(plan.client_api_format, "openai:embedding");
    assert_eq!(plan.provider_api_format, "gemini:embedding");
    assert_eq!(plan.method, "POST");
    assert_eq!(
        plan.url,
        "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-embedding-2:predict?key=sk-upstream-vertex-gemini-embedding"
    );
    assert_eq!(
        plan.model_name.as_deref(),
        Some("gemini-embedding-2-preview")
    );
    assert!(!plan.stream);
    let body = plan.body.json_body.as_ref().expect("json request body");
    assert!(
        body.get("model").is_none(),
        "Vertex predict carries the model in the path; the body must not repeat it"
    );
    assert_eq!(body["instances"][0]["content"], "hello");
    assert!(body.get("content").is_none());
    assert!(body.get("input").is_none());
    assert!(body.get("messages").is_none());
}

fn assert_openai_to_gemini_batch_embedding_execution_plan(plan: &ExecutionPlan) {
    assert_eq!(plan.client_api_format, "openai:embedding");
    assert_eq!(plan.provider_api_format, "gemini:embedding");
    assert_eq!(plan.method, "POST");
    assert_eq!(
        plan.url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2-preview:batchEmbedContents"
    );
    assert_eq!(
        plan.headers.get("x-goog-api-key").map(String::as_str),
        Some("sk-upstream-gemini-embedding")
    );
    assert!(!plan.stream);
    let body = plan.body.json_body.as_ref().expect("json request body");
    assert!(body.get("model").is_none());
    let requests = body["requests"].as_array().expect("batch requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["model"], "models/gemini-embedding-2-preview");
    assert_eq!(requests[0]["content"]["parts"][0]["text"], "hello");
    assert_eq!(requests[1]["model"], "models/gemini-embedding-2-preview");
    assert_eq!(requests[1]["content"]["parts"][0]["text"], "world");
    assert!(body.get("input").is_none());
    assert!(body.get("messages").is_none());
}

fn assert_native_gemini_embedding_execution_plan(plan: &ExecutionPlan) {
    assert_eq!(plan.client_api_format, "gemini:embedding");
    assert_eq!(plan.provider_api_format, "gemini:embedding");
    assert_eq!(plan.method, "POST");
    assert_eq!(
        plan.url,
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2-preview:embedContent"
    );
    assert_eq!(
        plan.headers.get("x-goog-api-key").map(String::as_str),
        Some("sk-upstream-gemini-embedding")
    );
    assert_eq!(
        plan.model_name.as_deref(),
        Some("gemini-embedding-2-preview")
    );
    assert!(!plan.stream);
    let body = plan.body.json_body.as_ref().expect("json request body");
    assert_eq!(body["content"]["parts"][0]["text"], "hello");
    assert!(body.get("input").is_none());
    assert!(body.get("messages").is_none());
}

fn embedding_execution_result(plan: &ExecutionPlan) -> ExecutionResult {
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(ResponseBody {
            json_body: Some(json!({
                "object": "list",
                "model": "upstream-embedding",
                "data": [
                    {"object": "embedding", "index": 0, "embedding": [0.1, 0.2, 0.3]}
                ],
                "usage": {"prompt_tokens": 4, "total_tokens": 4}
            })),
            body_bytes_b64: None,
        }),
        telemetry: None,
        error: None,
    }
}

fn gemini_embedding_execution_result(plan: &ExecutionPlan) -> ExecutionResult {
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(ResponseBody {
            json_body: Some(json!({
                "model": "gemini-embedding-2-preview",
                "embedding": {
                    "values": [0.1, 0.2, 0.3]
                },
                "usageMetadata": {
                    "promptTokenCount": 4,
                    "totalTokenCount": 4
                }
            })),
            body_bytes_b64: None,
        }),
        telemetry: None,
        error: None,
    }
}

fn vertex_gemini_embedding_execution_result(plan: &ExecutionPlan) -> ExecutionResult {
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(ResponseBody {
            json_body: Some(json!({
                "predictions": [
                    {
                        "embeddings": {
                            "values": [0.1, 0.2, 0.3]
                        }
                    }
                ],
                "deployedModelId": "gemini-embedding-2"
            })),
            body_bytes_b64: None,
        }),
        telemetry: None,
        error: None,
    }
}

fn gemini_batch_embedding_execution_result(plan: &ExecutionPlan) -> ExecutionResult {
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(ResponseBody {
            json_body: Some(json!({
                "model": "gemini-embedding-2-preview",
                "embeddings": [
                    {"values": [0.1, 0.2, 0.3]},
                    {"values": [0.4, 0.5, 0.6]}
                ],
                "usageMetadata": {
                    "promptTokenCount": 8,
                    "totalTokenCount": 8
                }
            })),
            body_bytes_b64: None,
        }),
        telemetry: None,
        error: None,
    }
}

fn aliyun_embedding_execution_result(plan: &ExecutionPlan) -> ExecutionResult {
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: Some(ResponseBody {
            json_body: Some(json!({
                "output": {
                    "embeddings": [
                        {
                            "index": 0,
                            "embedding": [0.1, 0.2, 0.3],
                            "type": "fusion"
                        }
                    ]
                },
                "usage": {
                    "input_tokens": 432,
                    "input_tokens_details": {
                        "image_tokens": 402,
                        "text_tokens": 30
                    },
                    "output_tokens": 1,
                    "total_tokens": 433
                },
                "request_id": "aliyun-request-1"
            })),
            body_bytes_b64: None,
        }),
        telemetry: None,
        error: None,
    }
}

#[test]
fn embeddings_route_accepts_openai_payload() {
    run_embedding_proxy_test(
        "embeddings_route_accepts_openai_payload",
        embeddings_route_accepts_openai_payload_impl,
    );
}

async fn embeddings_route_accepts_openai_payload_impl() {
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(embedding_execution_runtime()).await;
    let gateway = build_router_with_state(embedding_success_state(execution_runtime_url));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(http::header::AUTHORIZATION, "Bearer sk-embedding-success")
        .json(&json!({
            "model": "text-embedding-3-small",
            "input": ["hello", "world"]
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
        Some("embedding")
    );
    assert_ne!(
        response
            .headers()
            .get(CONTROL_ROUTE_KIND_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("chat")
    );
    assert_ne!(
        response
            .headers()
            .get(CONTROL_ROUTE_KIND_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("responses")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ENDPOINT_SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("openai:embedding")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTION_RUNTIME_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["object"], "list");
    assert_eq!(payload["data"][0]["object"], "embedding");
    assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_converts_openai_payload_to_gemini_embedding_provider() {
    run_embedding_proxy_test(
        "embeddings_route_converts_openai_payload_to_gemini_embedding_provider",
        embeddings_route_converts_openai_payload_to_gemini_embedding_provider_impl,
    );
}

async fn embeddings_route_converts_openai_payload_to_gemini_embedding_provider_impl() {
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(gemini_embedding_conversion_execution_runtime()).await;
    let gateway = build_router_with_state(gemini_embedding_success_state(
        execution_runtime_url,
        "openai:embedding",
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-gemini-embedding-success",
        )
        .json(&json!({
            "model": "gemini-embedding-2-preview",
            "input": "hello"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ENDPOINT_SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("openai:embedding")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTION_RUNTIME_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["object"], "list");
    assert_eq!(payload["model"], "gemini-embedding-2-preview");
    assert_eq!(payload["data"][0]["object"], "embedding");
    assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
    assert_eq!(payload["usage"]["prompt_tokens"], json!(4));
    assert_eq!(payload["usage"]["total_tokens"], json!(4));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_converts_openai_payload_to_vertex_gemini_embedding_provider() {
    run_embedding_proxy_test(
        "embeddings_route_converts_openai_payload_to_vertex_gemini_embedding_provider",
        embeddings_route_converts_openai_payload_to_vertex_gemini_embedding_provider_impl,
    );
}

async fn embeddings_route_converts_openai_payload_to_vertex_gemini_embedding_provider_impl() {
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(vertex_gemini_embedding_conversion_execution_runtime()).await;
    let gateway =
        build_router_with_state(vertex_gemini_embedding_success_state(execution_runtime_url));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-vertex-gemini-embedding-success",
        )
        .json(&json!({
            "model": "gemini-embedding-2-preview",
            "input": "hello"
        }))
        .send()
        .await
        .expect("request should succeed");

    let endpoint_signature = response
        .headers()
        .get(CONTROL_ENDPOINT_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status = response.status();
    let body_text = response.text().await.expect("body should read");
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected response body: {body_text}"
    );
    assert_eq!(endpoint_signature.as_deref(), Some("openai:embedding"));
    let payload: serde_json::Value = serde_json::from_str(&body_text).expect("body should parse");
    assert_eq!(payload["object"], "list");
    assert_eq!(payload["model"], "gemini-embedding-2");
    assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_converts_openai_batch_payload_to_gemini_batch_endpoint() {
    run_embedding_proxy_test(
        "embeddings_route_converts_openai_batch_payload_to_gemini_batch_endpoint",
        embeddings_route_converts_openai_batch_payload_to_gemini_batch_endpoint_impl,
    );
}

async fn embeddings_route_converts_openai_batch_payload_to_gemini_batch_endpoint_impl() {
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(gemini_embedding_batch_conversion_execution_runtime()).await;
    let gateway = build_router_with_state(gemini_embedding_success_state(
        execution_runtime_url,
        "openai:embedding",
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-gemini-embedding-success",
        )
        .json(&json!({
            "model": "gemini-embedding-2-preview",
            "input": ["hello", "world"]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ENDPOINT_SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("openai:embedding")
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["object"], "list");
    assert_eq!(payload["data"].as_array().map(Vec::len), Some(2));
    assert_eq!(payload["data"][0]["index"], json!(0));
    assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
    assert_eq!(payload["data"][1]["index"], json!(1));
    assert_eq!(payload["data"][1]["embedding"], json!([0.4, 0.5, 0.6]));
    assert_eq!(payload["usage"]["prompt_tokens"], json!(8));
    assert_eq!(payload["usage"]["total_tokens"], json!(8));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_converts_text_payload_to_aliyun_embedding_provider() {
    run_embedding_proxy_test(
        "embeddings_route_converts_text_payload_to_aliyun_embedding_provider",
        embeddings_route_converts_text_payload_to_aliyun_embedding_provider_impl,
    );
}

async fn embeddings_route_converts_text_payload_to_aliyun_embedding_provider_impl() {
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(aliyun_embedding_conversion_execution_runtime(
            json!([{ "text": "hello" }]),
            Some(json!({ "dimension": 1024 })),
        ))
        .await;
    let gateway = build_router_with_state(aliyun_embedding_success_state(execution_runtime_url));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-aliyun-embedding-success",
        )
        .json(&json!({
            "model": "qwen3-vl-embedding",
            "input": "hello",
            "dimensions": 1024
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ENDPOINT_SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("openai:embedding")
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["object"], "list");
    assert_eq!(payload["request_id"], "aliyun-request-1");
    assert_eq!(payload["model"], "qwen3-vl-embedding");
    assert_eq!(payload["data"][0]["object"], "embedding");
    assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
    assert_eq!(payload["data"][0]["type"], "fusion");
    assert_eq!(payload["usage"]["prompt_tokens"], json!(432));
    assert_eq!(payload["usage"]["completion_tokens"], json!(1));
    assert_eq!(payload["usage"]["total_tokens"], json!(433));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_converts_multimodal_payload_to_aliyun_embedding_provider() {
    run_embedding_proxy_test(
        "embeddings_route_converts_multimodal_payload_to_aliyun_embedding_provider",
        embeddings_route_converts_multimodal_payload_to_aliyun_embedding_provider_impl,
    );
}

async fn embeddings_route_converts_multimodal_payload_to_aliyun_embedding_provider_impl() {
    let expected_contents = json!([
        { "text": "white running shoes" },
        { "image": "https://example.com/shoe.png" },
        { "multi_images": ["https://example.com/a.png", "https://example.com/b.png"] }
    ]);
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(aliyun_embedding_conversion_execution_runtime(
            expected_contents.clone(),
            Some(json!({ "res_level": 2, "max_video_frames": 64 })),
        ))
        .await;
    let gateway = build_router_with_state(aliyun_embedding_success_state(execution_runtime_url));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-aliyun-embedding-success",
        )
        .json(&json!({
            "model": "qwen3-vl-embedding",
            "input": expected_contents,
            "parameters": {
                "res_level": 2,
                "max_video_frames": 64
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
    assert_eq!(payload["data"][0]["type"], "fusion");

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_skips_openai_candidate_for_multimodal_payload() {
    run_embedding_proxy_test(
        "embeddings_route_skips_openai_candidate_for_multimodal_payload",
        embeddings_route_skips_openai_candidate_for_multimodal_payload_impl,
    );
}

async fn embeddings_route_skips_openai_candidate_for_multimodal_payload_impl() {
    let expected_contents = json!([
        { "text": "white running shoes" },
        { "image": "https://example.com/shoe.png" }
    ]);
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(aliyun_embedding_conversion_execution_runtime(
            expected_contents.clone(),
            Some(json!({ "enable_fusion": true })),
        ))
        .await;
    let gateway = build_router_with_state(mixed_embedding_success_state(execution_runtime_url));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-mixed-embedding-success",
        )
        .json(&json!({
            "model": "qwen3-vl-embedding",
            "input": expected_contents,
            "parameters": {
                "enable_fusion": true
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
    assert_eq!(payload["data"][0]["type"], "fusion");

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_converts_fusion_payload_to_aliyun_embedding_provider() {
    run_embedding_proxy_test(
        "embeddings_route_converts_fusion_payload_to_aliyun_embedding_provider",
        embeddings_route_converts_fusion_payload_to_aliyun_embedding_provider_impl,
    );
}

async fn embeddings_route_converts_fusion_payload_to_aliyun_embedding_provider_impl() {
    let expected_contents = json!([
        {
            "text": "white running shoes",
            "image": "https://example.com/shoe.png"
        },
        { "video": "https://example.com/demo.mp4" }
    ]);
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(aliyun_embedding_conversion_execution_runtime(
            expected_contents.clone(),
            Some(json!({ "enable_fusion": true })),
        ))
        .await;
    let gateway = build_router_with_state(aliyun_embedding_success_state(execution_runtime_url));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-aliyun-embedding-success",
        )
        .json(&json!({
            "model": "qwen3-vl-embedding",
            "input": expected_contents,
            "parameters": {
                "enable_fusion": true
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
    assert_eq!(payload["data"][0]["type"], "fusion");

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gemini_embed_content_route_uses_native_gemini_embedding_provider() {
    run_embedding_proxy_test(
        "gemini_embed_content_route_uses_native_gemini_embedding_provider",
        gemini_embed_content_route_uses_native_gemini_embedding_provider_impl,
    );
}

async fn gemini_embed_content_route_uses_native_gemini_embedding_provider_impl() {
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(gemini_embedding_native_execution_runtime()).await;
    let gateway = build_router_with_state(gemini_embedding_success_state(
        execution_runtime_url,
        "gemini:embedding",
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/v1beta/models/gemini-embedding-2-preview:embedContent"
        ))
        .header("x-goog-api-key", "sk-gemini-embedding-success")
        .json(&json!({
            "content": {
                "parts": [{"text": "hello"}]
            }
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
        Some("gemini")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ROUTE_KIND_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("embedding")
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ENDPOINT_SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("gemini:embedding")
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["embedding"]["values"], json!([0.1, 0.2, 0.3]));
    assert_eq!(payload["model"], "gemini-embedding-2-preview");

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_accepts_all_canonical_input_shapes() {
    run_embedding_proxy_test(
        "embeddings_route_accepts_all_canonical_input_shapes",
        embeddings_route_accepts_all_canonical_input_shapes_impl,
    );
}

async fn embeddings_route_accepts_all_canonical_input_shapes_impl() {
    let (execution_runtime_url, execution_runtime_handle) =
        start_server(embedding_execution_runtime()).await;
    let gateway = build_router_with_state(embedding_success_state(execution_runtime_url));
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for input in [
        json!("hello"),
        json!(["hello", "world"]),
        json!([1, 2, 3]),
        json!([[1, 2], [3, 4]]),
    ] {
        let response = client
            .post(format!("{gateway_url}/v1/embeddings"))
            .header(http::header::AUTHORIZATION, "Bearer sk-embedding-success")
            .json(&json!({
                "model": "text-embedding-3-small",
                "input": input
            }))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTROL_ENDPOINT_SIGNATURE_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("openai:embedding")
        );
        let payload: serde_json::Value = response.json().await.expect("body should parse");
        assert_eq!(payload["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
    }

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn embeddings_route_rejects_invalid_local_payloads() {
    run_embedding_proxy_test(
        "embeddings_route_rejects_invalid_local_payloads",
        embeddings_route_rejects_invalid_local_payloads_impl,
    );
}

async fn embeddings_route_rejects_invalid_local_payloads_impl() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let cases = [
        ("{", "Embedding request JSON body is invalid"),
        (
            r#"{"input":"hello"}"#,
            "Embedding request model is required",
        ),
        (
            r#"{"model":"text-embedding-3-small","input":[]}"#,
            "Embedding request input is required",
        ),
        (
            r#"{"model":"text-embedding-3-small","messages":[]}"#,
            "Embedding request must use input, not chat messages",
        ),
        (
            r#"{"model":" ","input":"hello"}"#,
            "Embedding request model is required",
        ),
        (
            r#"{"model":"text-embedding-3-small","input":[[1],[]]}"#,
            "Embedding request input is required",
        ),
        (
            r#"{"model":"text-embedding-3-small","input":[{}]}"#,
            "Embedding request input is required",
        ),
        (
            r#"{"model":"text-embedding-3-small","input":[{"image":" "} ]}"#,
            "Embedding request input is required",
        ),
        (
            r#"{"model":"text-embedding-3-small","input":"hello","stream":true}"#,
            "Embedding requests do not support streaming",
        ),
    ];

    for (body, expected_detail) in cases {
        let response = client
            .post(format!("{gateway_url}/v1/embeddings"))
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
            Some("embedding")
        );
        let payload: serde_json::Value = response.json().await.expect("body should parse");
        assert_eq!(payload["detail"], expected_detail);
    }

    gateway_handle.abort();
}

#[test]
fn embeddings_route_rejects_non_json_content_type() {
    run_embedding_proxy_test(
        "embeddings_route_rejects_non_json_content_type",
        embeddings_route_rejects_non_json_content_type_impl,
    );
}

async fn embeddings_route_rejects_non_json_content_type_impl() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(http::header::CONTENT_TYPE, "text/plain")
        .body(r#"{"model":"text-embedding-3-small","input":"hello"}"#)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(
        payload["detail"],
        "Embedding request content-type must be application/json"
    );

    gateway_handle.abort();
}

#[test]
fn embeddings_route_rejects_chat_only_model() {
    run_embedding_proxy_test(
        "embeddings_route_rejects_chat_only_model",
        embeddings_route_rejects_chat_only_model_impl,
    );
}

async fn embeddings_route_rejects_chat_only_model_impl() {
    let mut snapshot = sample_currently_usable_auth_snapshot("key-embedding-1", "user-embedding-1");
    snapshot.user_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:embedding".to_string()]);
    snapshot.user_allowed_models = Some(vec!["text-embedding-3-small".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["text-embedding-3-small".to_string()]);
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-embedding-model-guard")),
        snapshot,
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-embedding-model-guard",
        )
        .json(&json!({
            "model": "gpt-5",
            "input": "hello"
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
        "当前用户、用户组或密钥的访问控制策略不允许访问模型 gpt-5"
    );

    gateway_handle.abort();
}

#[test]
fn embeddings_route_rejects_chat_only_api_format() {
    run_embedding_proxy_test(
        "embeddings_route_rejects_chat_only_api_format",
        embeddings_route_rejects_chat_only_api_format_impl,
    );
}

async fn embeddings_route_rejects_chat_only_api_format_impl() {
    let mut snapshot = sample_currently_usable_auth_snapshot("key-embedding-2", "user-embedding-2");
    snapshot.user_allowed_api_formats = Some(vec!["openai:chat".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:chat".to_string()]);
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-embedding-format-guard")),
        snapshot,
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/embeddings"))
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-embedding-format-guard",
        )
        .json(&json!({
            "model": "text-embedding-3-small",
            "input": "hello"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(
        payload["error"]["message"],
        "当前用户、用户组或密钥的访问控制策略不允许访问 openai:embedding 格式"
    );

    gateway_handle.abort();
}
