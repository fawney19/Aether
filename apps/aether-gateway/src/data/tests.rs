use std::sync::Arc;

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data::DataLayerError;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskLookupKey, VideoTaskStatus, VideoTaskWriteRepository,
};
use aether_scheduler_core::{
    enumerate_minimal_candidate_selection, EnumerateMinimalCandidateSelectionInput,
    SchedulerAuthConstraints,
};
use serde_json::json;

use super::{GatewayDataConfig, GatewayDataState};
use crate::AppState;

#[test]
fn disabled_gateway_data_state_has_no_backends() {
    let state = GatewayDataState::from_config(GatewayDataConfig::disabled())
        .expect("disabled config should build");

    assert!(!state.has_backends());
    assert!(!state.has_auth_api_key_reader());
    assert!(!state.has_minimal_candidate_selection_reader());
    assert!(!state.has_request_candidate_reader());
    assert!(!state.has_provider_catalog_reader());
    assert!(!state.has_proxy_node_reader());
    assert!(!state.has_proxy_node_writer());
    assert!(!state.has_usage_reader());
    assert!(!state.has_video_task_reader());
}

#[tokio::test]
async fn postgres_gateway_data_state_builds_video_task_reader() {
    let state = GatewayDataState::from_config(GatewayDataConfig::from_postgres_url(
        "postgres://localhost/aether",
        false,
    ))
    .expect("postgres-backed state should build");

    assert!(state.has_backends());
    assert!(state.has_auth_api_key_reader());
    assert!(state.has_minimal_candidate_selection_reader());
    assert!(state.has_request_candidate_reader());
    assert!(state.has_provider_catalog_reader());
    assert!(state.has_proxy_node_reader());
    assert!(state.has_proxy_node_writer());
    assert!(state.has_usage_reader());
    assert!(state.has_video_task_reader());
}

#[tokio::test]
async fn data_state_find_uses_configured_read_repository() {
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-1".to_string(),
            short_id: Some("short-task-1".to_string()),
            request_id: "request-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("user".to_string()),
            api_key_name: Some("primary".to_string()),
            external_task_id: Some("ext-task-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("provider-key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("hello".to_string()),
            original_request_body: Some(json!({"prompt": "hello"})),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: VideoTaskStatus::Queued,
            progress_percent: 0,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: Some(100),
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: 100,
            submitted_at_unix_secs: Some(100),
            completed_at_unix_secs: None,
            updated_at_unix_secs: 100,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: None,
        })
        .await
        .expect("upsert should succeed");

    let state = GatewayDataState::with_video_task_reader_for_tests(repository);

    let task = state
        .find_video_task(VideoTaskLookupKey::Id("task-1"))
        .await
        .expect("find should succeed");

    assert_eq!(task.expect("task should exist").id, "task-1");
}

#[tokio::test]
async fn app_state_wires_gateway_data_state_from_config() {
    let state = AppState::new()
        .expect("app state should build")
        .with_data_config(GatewayDataConfig::from_postgres_url(
            "postgres://localhost/aether",
            false,
        ))
        .expect("data config should wire");

    assert!(state.data.has_backends());
    assert!(state.data.has_auth_api_key_reader());
    assert!(state.data.has_minimal_candidate_selection_reader());
    assert!(state.data.has_request_candidate_reader());
    assert!(state.data.has_provider_catalog_reader());
    assert!(state.data.has_proxy_node_reader());
    assert!(state.data.has_proxy_node_writer());
    assert!(state.data.has_usage_reader());
    assert!(state.data.has_video_task_reader());
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
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-4.1"])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(200),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-4.1"])),
    )
    .expect("auth snapshot should build")
}

#[tokio::test]
async fn data_state_reads_auth_api_key_snapshot_from_reader() {
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_auth_snapshot("key-1", "user-1"),
    )]));
    let state = GatewayDataState::with_auth_api_key_reader_for_tests(repository);

    let snapshot = state
        .read_auth_api_key_snapshot("user-1", "key-1", 150)
        .await
        .expect("read should succeed")
        .expect("snapshot should exist");

    assert_eq!(snapshot.user_id, "user-1");
    assert_eq!(snapshot.api_key_id, "key-1");
    assert_eq!(snapshot.username, "alice");
    assert_eq!(
        snapshot.api_key_allowed_models,
        Some(vec!["gpt-4.1".to_string()])
    );
    assert!(snapshot.currently_usable);
}

fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-1".to_string(),
        "OpenAI".to_string(),
        Some("https://openai.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
}

fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        "endpoint-1".to_string(),
        "provider-1".to_string(),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
}

fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        "provider-key-1".to_string(),
        "provider-1".to_string(),
        "prod-key".to_string(),
        "api_key".to_string(),
        Some(serde_json::json!({"cache_1h": true})),
        true,
    )
    .expect("key should build")
}

fn sample_request_usage(request_id: &str) -> StoredRequestUsageAudit {
    StoredRequestUsageAudit::new(
        "usage-1".to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        "OpenAI".to_string(),
        "gpt-4.1".to_string(),
        Some("gpt-4.1-mini".to_string()),
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
        false,
        120,
        40,
        160,
        0.24,
        0.36,
        Some(200),
        None,
        None,
        Some(450),
        Some(120),
        "completed".to_string(),
        "settled".to_string(),
        100,
        101,
        Some(102),
    )
    .expect("usage should build")
}

fn sample_minimal_candidate_selection_row(
    provider_id: &str,
    provider_name: &str,
    provider_priority: i32,
    key_id: &str,
    key_name: &str,
    key_internal_priority: i32,
) -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: provider_id.to_string(),
        provider_name: provider_name.to_string(),
        provider_type: "custom".to_string(),
        provider_priority,
        provider_is_active: true,
        endpoint_id: format!("endpoint-{provider_id}"),
        endpoint_api_format: "openai:chat".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_is_active: true,
        key_id: key_id.to_string(),
        key_name: key_name.to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:chat".to_string()]),
        key_allowed_models: None,
        key_capabilities: Some(serde_json::json!({"cache_1h": true})),
        key_internal_priority,
        key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 3})),
        model_id: format!("model-{provider_id}"),
        global_model_id: "global-model-1".to_string(),
        global_model_name: "gpt-4.1".to_string(),
        global_model_mappings: Some(vec!["gpt-4\\.1-.*".to_string()]),
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "gpt-4.1-upstream".to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: "gpt-4.1-canary".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:chat".to_string()]),
        }]),
        model_supports_streaming: None,
        model_is_active: true,
        model_is_available: true,
    }
}

#[tokio::test]
async fn data_state_reads_decision_trace_with_provider_catalog_metadata() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            RequestCandidateStatus::Failed,
            None,
            false,
            Some(502),
            None,
            None,
            Some(37),
            Some(1),
            None,
            Some(serde_json::json!({"cache_1h": true})),
            100_000,
            Some(101_000),
            Some(102_000),
        )
        .expect("candidate should build"),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
    ));
    let state = GatewayDataState::with_decision_trace_readers_for_tests(
        request_candidates,
        provider_catalog,
    );

    let trace = state
        .read_decision_trace("req-1", true)
        .await
        .expect("trace should read")
        .expect("trace should exist");

    assert_eq!(trace.request_id, "req-1");
    assert_eq!(trace.total_candidates, 1);
    assert_eq!(trace.candidates[0].provider_name.as_deref(), Some("OpenAI"));
    assert_eq!(
        trace.candidates[0].endpoint_api_format.as_deref(),
        Some("openai:chat")
    );
    assert_eq!(
        trace.candidates[0].provider_key_auth_type.as_deref(),
        Some("api_key")
    );
    assert_eq!(
        trace.candidates[0].provider_key_capabilities,
        Some(serde_json::json!({"cache_1h": true}))
    );
}

#[tokio::test]
async fn data_state_reads_request_usage_audit_from_reader() {
    let repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_request_usage("req-usage-1"),
    ]));
    let state = GatewayDataState::with_usage_reader_for_tests(repository);

    let usage = state
        .read_request_usage_audit("req-usage-1")
        .await
        .expect("read should succeed")
        .expect("usage should exist");

    assert_eq!(usage.request_id, "req-usage-1");
    assert_eq!(usage.provider_name, "OpenAI");
    assert_eq!(usage.total_tokens, 160);
    assert_eq!(usage.total_cost_usd, 0.24);
    assert!(usage.has_format_conversion);
}

#[tokio::test]
async fn data_state_reads_request_audit_bundle_from_multiple_readers() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_auth_snapshot("api-key-1", "user-1"),
    )]));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        StoredRequestCandidate::new(
            "cand-1".to_string(),
            "req-usage-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            RequestCandidateStatus::Success,
            None,
            false,
            Some(200),
            None,
            None,
            Some(37),
            Some(1),
            None,
            Some(serde_json::json!({"cache_1h": true})),
            100_000,
            Some(101_000),
            Some(102_000),
        )
        .expect("candidate should build"),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
    ));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_request_usage("req-usage-1"),
    ]));
    let state = GatewayDataState::with_request_audit_readers_for_tests(
        auth_repository,
        request_candidates,
        provider_catalog,
        usage_repository,
    );

    let bundle = state
        .read_request_audit_bundle("req-usage-1", true, 150)
        .await
        .expect("bundle should read")
        .expect("bundle should exist");

    assert_eq!(bundle.request_id, "req-usage-1");
    assert_eq!(
        bundle
            .usage
            .as_ref()
            .and_then(|usage| usage.target_model.as_deref()),
        Some("gpt-4.1-mini")
    );
    assert_eq!(
        bundle
            .decision_trace
            .as_ref()
            .and_then(|trace| trace.candidates.first())
            .and_then(|candidate| candidate.provider_name.as_deref()),
        Some("OpenAI")
    );
    assert_eq!(
        bundle
            .auth_snapshot
            .as_ref()
            .map(|snapshot| snapshot.currently_usable),
        Some(true)
    );
}

#[tokio::test]
async fn data_state_reads_decrypted_provider_transport_snapshot() {
    let encrypted_api_key =
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-live-openai")
            .expect("api key ciphertext should build");
    let encrypted_auth_config = encrypt_python_fernet_plaintext(
        DEVELOPMENT_ENCRYPTION_KEY,
        "{\"refresh_token\":\"rt-1\",\"project\":\"demo\"}",
    )
    .expect("auth config ciphertext should build");
    let provider = sample_provider_catalog_provider().with_transport_fields(
        true,
        false,
        true,
        Some(32),
        Some(3),
        Some(serde_json::json!({"url":"http://provider-proxy"})),
        Some(20.0),
        Some(8.0),
        Some(serde_json::json!({"region":"global"})),
    );
    let endpoint = sample_provider_catalog_endpoint()
        .with_transport_fields(
            "https://api.openai.com".to_string(),
            Some(serde_json::json!([{"action":"set","key":"x-test","value":"1"}])),
            Some(serde_json::json!([{"action":"drop","path":"stream"}])),
            Some(2),
            Some("/v1/chat/completions".to_string()),
            Some(serde_json::json!({"api_version":"v1"})),
            Some(serde_json::json!({"allow":["openai:chat"]})),
            Some(serde_json::json!({"url":"http://endpoint-proxy"})),
        )
        .expect("endpoint transport should build");
    let key = sample_provider_catalog_key()
        .with_transport_fields(
            Some(serde_json::json!(["openai:chat", "openai:responses"])),
            encrypted_api_key,
            Some(encrypted_auth_config),
            Some(serde_json::json!({"openai:chat": 0.8})),
            Some(serde_json::json!({"openai:chat": 1})),
            Some(serde_json::json!(["gpt-4.1", "gpt-4.1-mini"])),
            Some(1_800_000_000),
            Some(serde_json::json!({"node_id":"proxy-node-1"})),
            Some(serde_json::json!({"tls_profile":"chrome_136"})),
        )
        .expect("key transport should build");
    let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));
    let state = GatewayDataState::with_provider_transport_reader_for_tests(
        repository,
        DEVELOPMENT_ENCRYPTION_KEY.to_string(),
    );

    let snapshot = state
        .read_provider_transport_snapshot("provider-1", "endpoint-1", "provider-key-1")
        .await
        .expect("snapshot read should succeed")
        .expect("snapshot should exist");

    assert_eq!(snapshot.provider.name, "OpenAI");
    assert_eq!(snapshot.endpoint.base_url, "https://api.openai.com");
    assert_eq!(
        snapshot.key.api_formats,
        Some(vec![
            "openai:chat".to_string(),
            "openai:responses".to_string()
        ])
    );
    assert_eq!(snapshot.key.decrypted_api_key, "sk-live-openai");
    assert_eq!(
        snapshot.key.decrypted_auth_config.as_deref(),
        Some("{\"refresh_token\":\"rt-1\",\"project\":\"demo\"}")
    );
}

#[tokio::test]
async fn data_state_reads_minimal_candidate_selection_with_auth_filters() {
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_minimal_candidate_selection_row(
                "provider-2",
                "OtherProvider",
                20,
                "key-2",
                "key-two",
                20,
            ),
            sample_minimal_candidate_selection_row(
                "provider-1",
                "OpenAI",
                10,
                "key-1",
                "key-one",
                10,
            ),
            StoredMinimalCandidateSelectionRow {
                key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 4})),
                key_allowed_models: Some(vec!["gpt-4.1-edge".to_string()]),
                ..sample_minimal_candidate_selection_row(
                    "provider-1",
                    "OpenAI",
                    10,
                    "key-3",
                    "key-three",
                    30,
                )
            },
        ]));
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_auth_snapshot("api-key-1", "user-1"),
    )]));
    let state = GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
        candidate_selection_repository,
        auth_repository,
    );
    let auth_snapshot = state
        .read_auth_api_key_snapshot("user-1", "api-key-1", 150)
        .await
        .expect("auth snapshot should read")
        .expect("auth snapshot should exist");

    let rows = state
        .list_minimal_candidate_selection_rows("openai:chat", "gpt-4.1")
        .await
        .expect("minimal candidate selection rows should read");
    let auth_constraints = SchedulerAuthConstraints {
        allowed_providers: auth_snapshot
            .effective_allowed_providers()
            .map(|items| items.to_vec()),
        allowed_api_formats: auth_snapshot
            .effective_allowed_api_formats()
            .map(|items| items.to_vec()),
        allowed_models: auth_snapshot
            .effective_allowed_models()
            .map(|items| items.to_vec()),
    };

    let selection =
        enumerate_minimal_candidate_selection(EnumerateMinimalCandidateSelectionInput {
            rows,
            normalized_api_format: "openai:chat",
            requested_model_name: "gpt-4.1",
            resolved_global_model_name: "gpt-4.1",
            require_streaming: false,
            required_capabilities: None,
            auth_constraints: Some(&auth_constraints),
        })
        .expect("selection should read");

    assert_eq!(selection.len(), 2);
    assert_eq!(selection[0].provider_id, "provider-1");
    assert_eq!(selection[0].selected_provider_model_name, "gpt-4.1-canary");
    assert_eq!(selection[0].mapping_matched_model, None);
    assert_eq!(selection[1].key_id, "key-3");
    assert_eq!(
        selection[1].selected_provider_model_name,
        "gpt-4.1-edge".to_string()
    );
    assert_eq!(
        selection[1].mapping_matched_model,
        Some("gpt-4.1-edge".to_string())
    );
}

#[tokio::test]
async fn maps_openai_video_task_repository_row_into_read_response() {
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-1".to_string(),
            short_id: Some("short-task-1".to_string()),
            request_id: "request-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("user".to_string()),
            api_key_name: Some("primary".to_string()),
            external_task_id: Some("ext-task-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("provider-key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("hello".to_string()),
            original_request_body: Some(json!({"prompt": "hello"})),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: VideoTaskStatus::Processing,
            progress_percent: 45,
            progress_message: Some("working".to_string()),
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: Some(120),
            poll_count: 1,
            max_poll_count: 360,
            created_at_unix_ms: 100,
            submitted_at_unix_secs: Some(100),
            completed_at_unix_secs: None,
            updated_at_unix_secs: 120,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: None,
        })
        .await
        .expect("upsert should succeed");

    let state = GatewayDataState::with_video_task_reader_for_tests(repository);
    let response = state
        .read_video_task_response(Some("openai"), "/v1/videos/task-1")
        .await
        .expect("read should succeed")
        .expect("read response should exist");

    assert_eq!(response.status_code, 200);
    assert_eq!(response.body_json["id"], "task-1");
    assert_eq!(response.body_json["status"], "processing");
    assert_eq!(response.body_json["created_at"], 100);
}

#[tokio::test]
async fn maps_gemini_video_task_repository_row_into_read_response() {
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-1".to_string(),
            short_id: Some("localshort123".to_string()),
            request_id: "request-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("user".to_string()),
            api_key_name: Some("primary".to_string()),
            external_task_id: Some("operations/ext-task-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("provider-key-1".to_string()),
            client_api_format: Some("gemini:video".to_string()),
            provider_api_format: Some("gemini:video".to_string()),
            format_converted: false,
            model: Some("veo-3".to_string()),
            prompt: Some("hello".to_string()),
            original_request_body: Some(json!({"prompt": "hello"})),
            duration_seconds: Some(8),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("720p".to_string()),
            status: VideoTaskStatus::Completed,
            progress_percent: 100,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 4,
            max_poll_count: 360,
            created_at_unix_ms: 100,
            submitted_at_unix_secs: Some(100),
            completed_at_unix_secs: Some(120),
            updated_at_unix_secs: 120,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: Some(json!({
                "rust_local_snapshot": {
                    "metadata": {
                        "generateVideoResponse": {
                            "generatedSamples": [
                                {
                                    "video": {
                                        "uri": "/v1beta/files/aev_localshort123:download?alt=media"
                                    }
                                }
                            ]
                        }
                    }
                }
            })),
        })
        .await
        .expect("upsert should succeed");

    let state = GatewayDataState::with_video_task_reader_for_tests(repository);
    let response = state
        .read_video_task_response(
            Some("gemini"),
            "/v1beta/models/veo-3/operations/localshort123",
        )
        .await
        .expect("read should succeed")
        .expect("read response should exist");

    assert_eq!(response.status_code, 200);
    assert_eq!(
        response.body_json["name"],
        "models/veo-3/operations/localshort123"
    );
    assert_eq!(response.body_json["done"], true);
}

fn sample_request_candidate(
    id: &str,
    request_id: &str,
    candidate_index: i32,
    status: RequestCandidateStatus,
    started_at_unix_ms: Option<i64>,
    latency_ms: Option<i32>,
    status_code: Option<i32>,
) -> StoredRequestCandidate {
    StoredRequestCandidate::new(
        id.to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        candidate_index,
        0,
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        status,
        None,
        false,
        status_code,
        None,
        None,
        latency_ms,
        Some(1),
        None,
        None,
        100 + i64::from(candidate_index),
        started_at_unix_ms,
        started_at_unix_ms.map(|value| value + 1),
    )
    .expect("candidate should build")
}

#[tokio::test]
async fn data_state_reads_request_candidate_trace_from_reader() {
    let repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_request_candidate(
            "cand-1",
            "req-1",
            0,
            RequestCandidateStatus::Pending,
            None,
            None,
            None,
        ),
        sample_request_candidate(
            "cand-2",
            "req-1",
            1,
            RequestCandidateStatus::Success,
            Some(101),
            Some(42),
            Some(200),
        ),
    ]));
    let state = GatewayDataState::with_request_candidate_reader_for_tests(repository);

    let trace = state
        .read_request_candidate_trace("req-1", true)
        .await
        .expect("trace should succeed")
        .expect("trace should exist");

    assert_eq!(trace.request_id, "req-1");
    assert_eq!(trace.total_candidates, 1);
    assert_eq!(
        trace.final_status,
        super::candidates::RequestCandidateFinalStatus::Success
    );
    assert_eq!(trace.total_latency_ms, 42);
    assert_eq!(trace.candidates[0].id, "cand-2");
}
