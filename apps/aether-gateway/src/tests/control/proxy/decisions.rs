use super::super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, hash_api_key,
    sample_bound_key, sample_currently_usable_auth_snapshot, sample_endpoint, sample_provider,
    start_server, AppState, GatewayDataState, InMemoryAuthApiKeySnapshotRepository,
    InMemoryProviderCatalogReadRepository, Json, Router,
};
use aether_contracts::{ExecutionPlan, ExecutionResult, ResponseBody};
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use base64::Engine as _;
use http::StatusCode;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
fn run_decisions_proxy_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("decisions proxy test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn decisions_success_state(execution_runtime_url: String, client_api_format: &str) -> AppState {
    let mut snapshot =
        sample_currently_usable_auth_snapshot("key-decisions-success", "user-decisions-success");
    snapshot.user_allowed_providers = None;
    snapshot.api_key_allowed_providers = None;
    snapshot.user_allowed_api_formats = Some(vec![client_api_format.to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec![client_api_format.to_string()]);
    snapshot.user_allowed_models = Some(vec!["decision-alias".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["decision-alias".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-decisions-success")),
        snapshot,
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            decisions_candidate_row(),
        ]));
    let mut provider = sample_provider("provider-decisions", "Gemini Decisions", 1);
    provider.provider_type = "gemini".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-decisions",
            "provider-decisions",
            "gemini:decisions",
            "https://generativelanguage.googleapis.com/v1beta",
        )],
        vec![sample_bound_key(
            "key-upstream-decisions",
            "provider-decisions",
            "gemini:decisions",
            "sk-upstream-decisions",
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

fn decisions_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-decisions".to_string(),
        provider_name: "Gemini Decisions".to_string(),
        provider_type: "gemini".to_string(),
        provider_priority: 1,
        provider_is_active: true,
        endpoint_id: "endpoint-decisions".to_string(),
        endpoint_api_format: "gemini:decisions".to_string(),
        endpoint_api_family: Some("gemini".to_string()),
        endpoint_kind: Some("decisions".to_string()),
        endpoint_is_active: true,
        key_id: "key-upstream-decisions".to_string(),
        key_name: "default".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["gemini:decisions".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 50,
        key_global_priority_by_format: None,
        model_id: "model-decisions-preview".to_string(),
        global_model_id: "global-decisions-preview".to_string(),
        global_model_name: "decision-alias".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(false),
        model_provider_model_name: "gemini-decision-1".to_string(),
        model_provider_model_mappings: None,
        model_supports_streaming: Some(false),
        model_is_active: true,
        model_is_available: true,
    }
}

#[test]
fn decisions_proxy_preserves_questions_multimodal_input_answers_and_model_mapping() {
    run_decisions_proxy_test("decisions_proxy", || async {
        let answers = json!({
            "positive": {"type": "noul", "noul": 0.98, "confidence": 0.98},
            "route": {"type": "choice", "choice": "support", "probabilities": {"support": 0.9, "billing": 0.1}, "confidence": 0.9},
            "severity": {"type": "score", "score": 0.2, "legend": {"0": "low", "1": "high"}, "probabilities": {"0": 0.8, "1": 0.2}, "confidence": 0.8}
        });
        let expected_response = json!({"answers": answers, "usage": {
            "total_input_tokens": 120, "total_output_tokens": 0, "total_tokens": 120,
            "input_tokens_by_modality": [{"modality": "text", "tokens": 20}, {"modality": "image", "tokens": 100}]
        }});
        for (input, status_code) in [
            (json!("Hello, my order arrived damaged."), 200),
            (
                json!([
                    {"type": "text", "text": "Review these files"},
                    {"type": "image", "data": "aW1hZ2U=", "mime_type": "image/png", "resolution": "low"},
                    {"type": "audio", "uri": "https://media.example/audio.mp3", "mime_type": "audio/mpeg"},
                    {"type": "video", "uri": "https://media.example/video.mp4", "mime_type": "video/mp4"},
                    {"type": "document", "uri": "https://media.example/document.pdf", "mime_type": "application/pdf"}
                ]),
                200,
            ),
            (json!("Invalid upstream request"), 400),
        ] {
            let expected_response = if status_code == 400 {
                json!({"error": {"code": 400, "status": "INVALID_ARGUMENT", "message": "Invalid question criteria"}})
            } else {
                expected_response.clone()
            };
            let request = json!({"model": "decision-alias", "input": input, "questions": {
                "positive": {"type": "noul", "instructions": "Is the item damaged?"},
                "route": {"type": "choice", "instructions": "Choose the queue", "criteria": {"support": "Damaged items", "billing": "Invoices"}},
                "severity": {"type": "score", "instructions": "Rate severity", "criteria": ["low", "high"]}
            }});
            let mut upstream_request = request.clone();
            upstream_request["model"] = json!("gemini-decision-1");
            let reply = expected_response.clone();
            let execution_runtime = Router::new().route(
                "/v1/execute/sync",
                any(move |Json(plan): Json<ExecutionPlan>| {
                    let request = upstream_request.clone();
                    let reply = reply.clone();
                    async move {
                        assert_eq!(plan.client_api_format, "gemini:decisions");
                        assert_eq!(plan.provider_api_format, "gemini:decisions");
                        assert_eq!(
                            plan.url,
                            "https://generativelanguage.googleapis.com/v1beta/decisions"
                        );
                        assert!(!plan.stream);
                        let actual = plan.body.json_body.clone().unwrap_or_else(|| {
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(plan.body.body_bytes_b64.as_deref().unwrap())
                                .unwrap();
                            serde_json::from_slice(&bytes).unwrap()
                        });
                        assert_eq!(actual, request);
                        Json(ExecutionResult {
                            request_id: plan.request_id,
                            candidate_id: plan.candidate_id,
                            status_code,
                            headers: BTreeMap::from([(
                                "content-type".to_string(),
                                "application/json".to_string(),
                            )]),
                            response_observation: None,
                            body: Some(ResponseBody {
                                json_body: Some(reply),
                                body_bytes_b64: None,
                            }),
                            telemetry: None,
                            error: None,
                        })
                    }
                }),
            );
            let (runtime_url, runtime_handle) = start_server(execution_runtime).await;
            let state = decisions_success_state(runtime_url, "gemini:decisions");
            let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
            let response = reqwest::Client::new()
                .post(format!("{gateway_url}/v1beta/decisions"))
                .header("x-goog-api-key", "sk-decisions-success")
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), status_code);
            let payload: serde_json::Value = response.json().await.unwrap();
            assert_eq!(payload, expected_response);
            gateway_handle.abort();
            runtime_handle.abort();
        }
    });
}

#[test]
fn decisions_proxy_requires_api_key_authentication() {
    run_decisions_proxy_test("decisions_auth", || async {
        let state = decisions_success_state("http://127.0.0.1:1".to_string(), "gemini:decisions");
        let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
        let response = reqwest::Client::new()
            .post(format!("{gateway_url}/v1beta/decisions"))
            .json(
                &json!({"model": "decision-alias", "input": "Hello", "questions": {
                    "greeting": {"type": "noul", "instructions": "Is this a greeting?"}
                }}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        gateway_handle.abort();
    });
}
