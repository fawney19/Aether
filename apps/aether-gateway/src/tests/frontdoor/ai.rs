use super::{
    hash_api_key, sample_endpoint, sample_key, sample_models_candidate_row, sample_provider,
    unrestricted_models_snapshot, InMemoryAuthApiKeySnapshotRepository,
    InMemoryMinimalCandidateSelectionReadRepository, InMemoryRequestCandidateRepository,
    InMemoryVideoTaskRepository, StoredAuthApiKeySnapshot, UpsertVideoTask, VideoTaskLookupKey,
    VideoTaskReadRepository, VideoTaskStatus, VideoTaskWriteRepository, DEVELOPMENT_ENCRYPTION_KEY,
};
use crate::image_capabilities::openai_image_gateway_max_generation_count;
use crate::tests::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    to_bytes, wait_until, AppState, Arc, Body, Json, Mutex, Request, Router, StatusCode,
    EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_AI_PUBLIC,
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS,
};
use aether_contracts::{ExecutionResult, ExecutionTelemetry, ResponseBody};
use aether_crypto::encrypt_python_fernet_plaintext;
use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::DataLayerError;
use aether_data_contracts::repository::auth::AuthApiKeyWriteRepository;
use aether_data_contracts::repository::candidate_selection::{
    MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
    StoredPoolKeyCandidateRowsByKeyIdsQuery, StoredPoolKeyCandidateRowsQuery,
    StoredRequestedModelCandidateRowsQuery,
};
use aether_data_contracts::repository::global_models::{
    StoredAdminGlobalModel, UpdateAdminGlobalModelRecord,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::usage::{UsageAuditListQuery, UsageReadRepository};
use async_trait::async_trait;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use base64::Engine as _;
use futures_util::SinkExt;
use std::collections::HashMap;
use std::future::pending;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::oneshot;
use wreq::ws::message::Message as WreqWsMessage;

fn codex_models_snapshot(
    api_key_id: &str,
    user_id: &str,
    allowed_models: &[&str],
) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(json!(["codex"])),
        Some(json!(["openai:responses"])),
        Some(json!(allowed_models)),
        api_key_id.to_string(),
        Some("codex-models".to_string()),
        true,
        false,
        false,
        Some(10),
        Some(5),
        Some(4_102_444_800),
        Some(json!(["codex"])),
        Some(json!(["openai:responses"])),
        Some(json!(allowed_models)),
    )
    .expect("Codex models auth snapshot should build")
}

fn codex_live_snapshot(
    api_key_id: &str,
    user_id: &str,
    allowed_models: &[&str],
) -> StoredAuthApiKeySnapshot {
    let mut snapshot = codex_models_snapshot(api_key_id, user_id, allowed_models);
    snapshot.user_allowed_api_formats = Some(vec!["codex:live".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["codex:live".to_string()]);
    snapshot
}

fn sample_codex_models_candidate_row(
    provider_id: &str,
    global_model_name: &str,
    source_model_name: &str,
) -> StoredMinimalCandidateSelectionRow {
    let mut row = sample_models_candidate_row(
        provider_id,
        "codex",
        "openai:responses",
        global_model_name,
        10,
    );
    row.provider_type = "codex".to_string();
    row.key_auth_type = "oauth".to_string();
    row.model_provider_model_name = source_model_name.to_string();
    row.model_provider_model_mappings = Some(vec![
        aether_data_contracts::repository::candidate_selection::StoredProviderModelMapping {
            name: source_model_name.to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:responses".to_string()]),
            endpoint_ids: None,
            operations: None,
        },
    ]);
    row
}

fn sample_codex_live_candidate_row(
    provider_id: &str,
    global_model_name: &str,
    source_model_name: &str,
) -> StoredMinimalCandidateSelectionRow {
    let mut row =
        sample_codex_models_candidate_row(provider_id, global_model_name, source_model_name);
    row.endpoint_api_format = "codex:live".to_string();
    row.endpoint_api_family = Some("codex".to_string());
    row.endpoint_kind = Some("live".to_string());
    row.key_api_formats = Some(vec!["codex:live".to_string()]);
    if let Some(mappings) = row.model_provider_model_mappings.as_mut() {
        for mapping in mappings {
            mapping.api_formats = Some(vec!["codex:live".to_string()]);
        }
    }
    row
}

fn complete_codex_model_card(source_model_name: &str) -> serde_json::Value {
    json!({
        "id": source_model_name,
        "api_formats": ["openai:responses"],
        "slug": source_model_name,
        "display_name": "GPT-5.6-Sol",
        "description": "Frontier coding model",
        "default_reasoning_level": "low",
        "supported_reasoning_levels": [
            {"effort": "low", "description": "Low"},
            {"effort": "medium", "description": "Medium"},
            {"effort": "high", "description": "High"},
            {"effort": "xhigh", "description": "XHigh"},
            {"effort": "max", "description": "Max"},
            {"effort": "ultra", "description": "Ultra"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1,
        "availability_nux": null,
        "upgrade": null,
        "base_instructions": "Use the current Codex instructions.",
        "model_messages": null,
        "available_in_plans": ["plus", "pro"],
        "support_verbosity": true,
        "default_verbosity": "low",
        "apply_patch_tool_type": "freeform",
        "truncation_policy": {"mode": "tokens", "limit": 10000},
        "supports_parallel_tool_calls": true,
        "experimental_supported_tools": [],
        "minimal_client_version": "0.144.0",
        "future_capability": {"enabled": true}
    })
}

fn codex_catalog_provider(provider_id: &str) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        provider_id.to_string(),
        "codex".to_string(),
        Some("https://chatgpt.com".to_string()),
        "codex".to_string(),
    )
    .expect("Codex provider should build")
}

fn codex_catalog_endpoint(provider_id: &str, endpoint_id: &str) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        endpoint_id.to_string(),
        provider_id.to_string(),
        "openai:responses".to_string(),
        Some("openai".to_string()),
        Some("responses".to_string()),
        true,
    )
    .expect("Codex endpoint should build")
    .with_transport_fields(
        "https://chatgpt.example/backend-api/codex".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("Codex endpoint transport should build")
}

fn codex_live_catalog_endpoint(
    provider_id: &str,
    endpoint_id: &str,
) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        endpoint_id.to_string(),
        provider_id.to_string(),
        "codex:live".to_string(),
        Some("codex".to_string()),
        Some("live".to_string()),
        true,
    )
    .expect("Codex Live endpoint should build")
    .with_transport_fields(
        "https://chatgpt.example/backend-api/codex".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("Codex Live endpoint transport should build")
}

fn codex_catalog_key(
    provider_id: &str,
    key_id: &str,
    allowed_models: &[&str],
) -> StoredProviderCatalogKey {
    let mut key = StoredProviderCatalogKey::new(
        key_id.to_string(),
        provider_id.to_string(),
        "manual".to_string(),
        "bearer".to_string(),
        None,
        true,
    )
    .expect("Codex key should build")
    .with_transport_fields(
        Some(json!(["openai:responses"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "oauth-upstream-secret")
            .expect("Codex test token should encrypt"),
        None,
        None,
        None,
        Some(json!(allowed_models)),
        None,
        None,
        None,
    )
    .expect("Codex key transport should build");
    key.auto_fetch_models = false;
    key.locked_models = Some(json!(["manual-locked-model"]));
    key.model_include_patterns = Some(json!(["gpt-future-*"]));
    key.model_exclude_patterns = Some(json!(["gpt-future-denied"]));
    key
}

fn codex_live_catalog_key(
    provider_id: &str,
    key_id: &str,
    allowed_models: &[&str],
) -> StoredProviderCatalogKey {
    let mut key = codex_catalog_key(provider_id, key_id, allowed_models);
    key.api_formats = Some(json!(["codex:live"]));
    key
}

fn codex_catalog_execution_result(
    plan: &aether_contracts::ExecutionPlan,
    status_code: u16,
    body: serde_json::Value,
    etag: Option<&str>,
) -> ExecutionResult {
    let mut headers = std::collections::BTreeMap::from([(
        "content-type".to_string(),
        "application/json".to_string(),
    )]);
    if let Some(etag) = etag {
        headers.insert("ETag".to_string(), etag.to_string());
    }
    ExecutionResult {
        request_id: plan.request_id.clone(),
        candidate_id: plan.candidate_id.clone(),
        status_code,
        headers,
        response_observation: None,
        body: Some(ResponseBody {
            json_body: Some(body),
            body_bytes_b64: None,
        }),
        telemetry: Some(ExecutionTelemetry {
            ttfb_ms: Some(1),
            elapsed_ms: Some(2),
            upstream_bytes: None,
        }),
        error: None,
    }
}

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
                        "transport_profile": null,
                        "timeouts": null
                    }
                }
            }
        })),
    }
}

struct PendingMinimalCandidateSelectionReadRepository;

impl PendingMinimalCandidateSelectionReadRepository {
    async fn pending_rows(
        &self,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        pending::<Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError>>().await
    }
}

#[async_trait]
impl MinimalCandidateSelectionReadRepository for PendingMinimalCandidateSelectionReadRepository {
    async fn list_for_exact_api_format(
        &self,
        _api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.pending_rows().await
    }

    async fn list_for_exact_api_format_and_global_model(
        &self,
        _api_format: &str,
        _global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.pending_rows().await
    }

    async fn list_for_exact_api_format_and_requested_model(
        &self,
        _api_format: &str,
        _requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.pending_rows().await
    }

    async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        _query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.pending_rows().await
    }

    async fn list_pool_key_rows_for_group(
        &self,
        _query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.pending_rows().await
    }

    async fn list_pool_key_rows_for_group_key_ids(
        &self,
        _query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.pending_rows().await
    }
}

struct CachedToggleMinimalCandidateSelectionReadRepository {
    row: StoredMinimalCandidateSelectionRow,
    active: AtomicBool,
    cached_rows_by_api_format: Mutex<HashMap<String, Vec<StoredMinimalCandidateSelectionRow>>>,
}

impl CachedToggleMinimalCandidateSelectionReadRepository {
    fn new(row: StoredMinimalCandidateSelectionRow) -> Self {
        Self {
            row,
            active: AtomicBool::new(true),
            cached_rows_by_api_format: Mutex::new(HashMap::new()),
        }
    }

    fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::SeqCst);
    }

    fn rows_for_api_format(&self, api_format: &str) -> Vec<StoredMinimalCandidateSelectionRow> {
        let api_format = api_format.trim().to_string();
        let mut cached = self
            .cached_rows_by_api_format
            .lock()
            .expect("candidate row cache lock");
        if let Some(rows) = cached.get(&api_format) {
            return rows.clone();
        }

        let rows = if self.active.load(Ordering::SeqCst)
            && self
                .row
                .endpoint_api_format
                .eq_ignore_ascii_case(&api_format)
        {
            vec![self.row.clone()]
        } else {
            Vec::new()
        };
        cached.insert(api_format, rows.clone());
        rows
    }
}

#[async_trait]
impl MinimalCandidateSelectionReadRepository
    for CachedToggleMinimalCandidateSelectionReadRepository
{
    fn clear_local_cache(&self) {
        self.cached_rows_by_api_format
            .lock()
            .expect("candidate row cache lock")
            .clear();
    }

    async fn list_for_exact_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Ok(self.rows_for_api_format(api_format))
    }

    async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Ok(self
            .rows_for_api_format(api_format)
            .into_iter()
            .filter(|row| row.global_model_name == global_model_name)
            .collect())
    }

    async fn list_for_exact_api_format_and_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Ok(self
            .rows_for_api_format(api_format)
            .into_iter()
            .filter(|row| row.global_model_name == requested_model_name)
            .collect())
    }

    async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Ok(self
            .rows_for_api_format(&query.api_format)
            .into_iter()
            .filter(|row| row.global_model_name == query.requested_model_name)
            .skip(query.offset as usize)
            .take(query.limit as usize)
            .collect())
    }

    async fn list_pool_key_rows_for_group(
        &self,
        _query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Ok(Vec::new())
    }

    async fn list_pool_key_rows_for_group_key_ids(
        &self,
        _query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        Ok(Vec::new())
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
    assert_eq!(payload["data"][0]["owned_by"], "aether");
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[test]
fn gateway_versioned_models_fail_closed_when_cached_auth_becomes_unusable_or_missing() {
    std::thread::Builder::new()
        .name("codex-model-catalog-auth-race".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Codex auth race test runtime should build")
                .block_on(run_versioned_models_auth_race_scenario());
        })
        .expect("Codex auth race test thread should spawn")
        .join()
        .expect("Codex auth race test thread should finish");
}

async fn run_versioned_models_auth_race_scenario() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-codex-models-auth-race")),
        codex_models_snapshot(
            "key-codex-models-auth-race",
            "user-codex-models-auth-race",
            &["future-alias"],
        ),
    )]));
    let candidate_repository = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(
        Vec::new(),
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                    candidate_repository,
                    auth_repository.clone(),
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let models_url = format!("{gateway_url}/v1/models?client_version=0.145.2");

    let warm_response = client
        .get(&models_url)
        .header("authorization", "Bearer sk-codex-models-auth-race")
        .send()
        .await
        .expect("initial versioned models request should succeed");
    assert_eq!(warm_response.status(), StatusCode::OK);

    assert!(auth_repository
        .set_user_api_key_locked(
            "user-codex-models-auth-race",
            "key-codex-models-auth-race",
            true,
        )
        .await
        .expect("locking the API key should succeed"));
    let locked_response = client
        .get(&models_url)
        .header("authorization", "Bearer sk-codex-models-auth-race")
        .send()
        .await
        .expect("locked versioned models request should complete");
    assert_eq!(locked_response.status(), StatusCode::UNAUTHORIZED);

    assert!(auth_repository
        .delete_user_api_key("user-codex-models-auth-race", "key-codex-models-auth-race",)
        .await
        .expect("deleting the API key should succeed"));
    let missing_response = client
        .get(&models_url)
        .header("authorization", "Bearer sk-codex-models-auth-race")
        .send()
        .await
        .expect("missing versioned models request should complete");
    assert_eq!(missing_response.status(), StatusCode::UNAUTHORIZED);

    gateway_handle.abort();
}

#[test]
fn gateway_serves_codex_model_cards_for_versioned_models_requests() {
    std::thread::Builder::new()
        .name("codex-model-catalog-frontdoor".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Codex frontdoor test runtime should build")
                .block_on(run_versioned_codex_model_cards_frontdoor_scenario());
        })
        .expect("Codex frontdoor test thread should spawn")
        .join()
        .expect("Codex frontdoor test thread should finish");
}

async fn run_versioned_codex_model_cards_frontdoor_scenario() {
    const PROVIDER_ID: &str = "provider-codex-models";
    const CATALOG_KEY_ID: &str = "key-provider-codex-models";
    const CATALOG_ENDPOINT_ID: &str = "endpoint-provider-codex-models";
    const SOURCE_MODELS: &[&str] = &[
        "gpt-future-dynamic",
        "gpt-future-legacy",
        "gpt-future-second",
        "gpt-hidden-direct",
    ];
    const GLOBAL_MODELS: &[&str] = &["future-alias", "legacy-alias"];

    let mut codex_rows = vec![
        sample_codex_models_candidate_row(PROVIDER_ID, "future-alias", "gpt-future-dynamic"),
        sample_codex_models_candidate_row(PROVIDER_ID, "legacy-alias", "gpt-future-legacy"),
        sample_codex_models_candidate_row(PROVIDER_ID, "second-alias", "gpt-future-second"),
        sample_codex_models_candidate_row(PROVIDER_ID, "hidden-alias", "gpt-hidden-direct"),
    ];
    for row in &mut codex_rows {
        row.key_allowed_models = Some(
            SOURCE_MODELS
                .iter()
                .map(|value| value.to_string())
                .collect(),
        );
    }
    let mut all_candidate_rows = codex_rows.clone();
    all_candidate_rows.push(sample_models_candidate_row(
        "provider-openai-responses",
        "openai",
        "openai:responses",
        "custom-responses-model",
        20,
    ));
    let candidate_repository = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(
        all_candidate_rows,
    ));
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![
        (
            Some(hash_api_key("sk-codex-models")),
            codex_models_snapshot("key-codex-models", "user-codex-models", GLOBAL_MODELS),
        ),
        (
            Some(hash_api_key("sk-standard-models")),
            unrestricted_models_snapshot("key-standard-models", "user-standard-models"),
        ),
        (
            Some(hash_api_key("sk-codex-legacy-only")),
            codex_models_snapshot(
                "key-codex-legacy-only",
                "user-codex-legacy-only",
                &["legacy-alias"],
            ),
        ),
        (
            Some(hash_api_key("sk-codex-hidden-mixed")),
            codex_models_snapshot(
                "key-codex-hidden-mixed",
                "user-codex-hidden-mixed",
                &["future-alias", "hidden-alias"],
            ),
        ),
        (
            Some(hash_api_key("sk-codex-hidden-only")),
            codex_models_snapshot(
                "key-codex-hidden-only",
                "user-codex-hidden-only",
                &["hidden-alias"],
            ),
        ),
        (
            Some(hash_api_key("sk-codex-second-mixed")),
            codex_models_snapshot(
                "key-codex-second-mixed",
                "user-codex-second-mixed",
                &["future-alias", "second-alias"],
            ),
        ),
    ]));
    let original_catalog_key = codex_catalog_key(PROVIDER_ID, CATALOG_KEY_ID, SOURCE_MODELS);
    assert!(!original_catalog_key.auto_fetch_models);
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![codex_catalog_provider(PROVIDER_ID)],
        vec![codex_catalog_endpoint(PROVIDER_ID, CATALOG_ENDPOINT_ID)],
        vec![original_catalog_key.clone()],
    ));
    let rows_before = candidate_repository
        .list_for_exact_api_format("openai:responses")
        .await
        .expect("candidate rows should load before request");

    let catalog_generation = Arc::new(AtomicUsize::new(0));
    let catalog_hits = Arc::new(AtomicUsize::new(0));
    let captured_plans = Arc::new(Mutex::new(Vec::<(String, Option<String>)>::new()));
    let generation_for_runtime = Arc::clone(&catalog_generation);
    let hits_for_runtime = Arc::clone(&catalog_hits);
    let plans_for_runtime = Arc::clone(&captured_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let generation_for_request = Arc::clone(&generation_for_runtime);
            let hits_for_request = Arc::clone(&hits_for_runtime);
            let plans_for_request = Arc::clone(&plans_for_runtime);
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX)
                    .await
                    .expect("execution runtime request body should read");
                let plan: aether_contracts::ExecutionPlan =
                    serde_json::from_slice(&raw_body).expect("execution runtime plan should parse");
                plans_for_request
                    .lock()
                    .expect("plans mutex")
                    .push((plan.url.clone(), plan.headers.get("user-agent").cloned()));
                if plan.url.contains("/models?") {
                    hits_for_request.fetch_add(1, Ordering::SeqCst);
                    let generation = generation_for_request.load(Ordering::SeqCst);
                    if generation == 1 {
                        return Json(codex_catalog_execution_result(
                            &plan,
                            503,
                            json!({"error":{"message":"temporary catalog outage"}}),
                            None,
                        ));
                    }

                    let mut current = complete_codex_model_card("gpt-future-dynamic");
                    let current_object = current.as_object_mut().expect("current card object");
                    current_object.remove("base_instructions");
                    current_object.insert(
                        "model_messages".to_string(),
                        json!({"instructions_template":"Use future dynamic instructions."}),
                    );
                    current_object.insert(
                        "future_capability".to_string(),
                        json!({"mode":"opaque-current"}),
                    );

                    let mut legacy = complete_codex_model_card("gpt-future-legacy");
                    legacy["base_instructions"] = json!("Use legacy future instructions.");
                    legacy["future_capability"] = json!({"mode":"opaque-legacy"});

                    let mut models = vec![current, legacy];
                    if generation >= 2 {
                        let mut second = complete_codex_model_card("gpt-future-second");
                        second
                            .as_object_mut()
                            .expect("second card object")
                            .remove("base_instructions");
                        second["model_messages"] =
                            json!({"instructions_template":"Use second future instructions."});
                        second["future_capability"] = json!({"mode":"added-without-code-change"});
                        models.push(second);
                        models.push(complete_codex_model_card("gpt-future-unmapped"));
                    }
                    let etag = if generation >= 2 {
                        "\"catalog-etag-v2\""
                    } else {
                        "\"catalog-etag-v1\""
                    };
                    return Json(codex_catalog_execution_result(
                        &plan,
                        200,
                        json!({"models": models, "future_top_level": true}),
                        Some(etag),
                    ));
                }

                Json(codex_catalog_execution_result(
                    &plan,
                    200,
                    json!({
                        "id": "resp-future-dynamic",
                        "object": "response",
                        "model": "gpt-future-dynamic",
                        "output": [],
                        "usage": {
                            "input_tokens": 1,
                            "output_tokens": 2,
                            "total_tokens": 3
                        }
                    }),
                    Some("\"catalog-etag-v2\""),
                ))
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                candidate_repository.clone(),
                auth_repository,
            )
            .attach_provider_catalog_repository_for_tests(provider_catalog_repository.clone())
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );

    let configured_rows = state
        .list_minimal_candidate_selection_rows_for_api_format("openai:responses")
        .await
        .expect("configured Codex candidate rows should be readable");
    assert_eq!(
        configured_rows
            .iter()
            .filter(|row| row.provider_type == "codex")
            .count(),
        codex_rows.len()
    );
    let resolved_auth =
        aether_data_contracts::repository::auth::ResolvedAuthApiKeySnapshot::from_stored(
            codex_models_snapshot("key-codex-models", "user-codex-models", GLOBAL_MODELS),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
    let eligible_rows = crate::handlers::public::filter_eligible_model_rows(
        configured_rows.clone(),
        Some(&resolved_auth),
        "openai:responses",
    );
    assert_eq!(
        eligible_rows
            .iter()
            .filter(|row| row.provider_type == "codex")
            .count(),
        GLOBAL_MODELS.len(),
        "Codex fixture rows must survive the same provider/model/key authorization filters as the route"
    );
    let actual_auth = state
        .data
        .read_auth_api_key_snapshot(
            "user-codex-models",
            "key-codex-models",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        )
        .await
        .expect("Codex auth snapshot read should succeed")
        .expect("Codex auth snapshot should exist");
    assert_eq!(
        crate::handlers::public::filter_eligible_model_rows(
            configured_rows.clone(),
            Some(&actual_auth),
            "openai:responses",
        )
        .iter()
        .filter(|row| row.provider_type == "codex")
        .count(),
        GLOBAL_MODELS.len(),
        "stored Codex auth snapshot must preserve every authorized manual mapping"
    );
    assert!(
        <AppState as crate::model_fetch::CodexCatalogRuntime>::read_codex_catalog_transport_snapshot(
            &state,
            PROVIDER_ID,
            CATALOG_ENDPOINT_ID,
            CATALOG_KEY_ID,
        )
        .await
        .expect("Codex catalog transport lookup should succeed")
        .is_some(),
        "Codex catalog transport must be available even when auto_fetch_models is disabled"
    );

    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let codex_response = client
        .get(format!(
            "{gateway_url}/v1/models?client_version=0.145.2-beta.7%2Bdesktop.9"
        ))
        .header("authorization", "Bearer sk-codex-models")
        .send()
        .await
        .expect("Codex models request should succeed");
    assert_eq!(codex_response.status(), StatusCode::OK);
    let codex_etag = codex_response
        .headers()
        .get(http::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let codex_payload: serde_json::Value = codex_response
        .json()
        .await
        .expect("Codex models body should parse");
    assert_eq!(
        catalog_hits.load(Ordering::SeqCst),
        1,
        "cold Codex request must reach the upstream catalog once; payload={codex_payload}"
    );
    assert_eq!(codex_etag.as_deref(), Some("\"catalog-etag-v1\""));
    assert_eq!(codex_payload["models"].as_array().map(Vec::len), Some(2));
    let current_card = codex_payload["models"]
        .as_array()
        .and_then(|models| models.iter().find(|model| model["slug"] == "future-alias"))
        .expect("current model card should be projected");
    assert_eq!(
        current_card["model_messages"]["instructions_template"],
        "Use future dynamic instructions."
    );
    assert_eq!(
        current_card["future_capability"],
        json!({"mode":"opaque-current"})
    );
    assert_eq!(current_card["available_in_plans"], json!(["plus", "pro"]));
    assert!(current_card.get("base_instructions").is_none());
    assert!(current_card.get("id").is_none());
    assert!(current_card.get("api_formats").is_none());
    let legacy_card = codex_payload["models"]
        .as_array()
        .and_then(|models| models.iter().find(|model| model["slug"] == "legacy-alias"))
        .expect("legacy model card should be projected");
    assert_eq!(
        legacy_card["base_instructions"],
        "Use legacy future instructions."
    );
    assert_eq!(
        legacy_card["future_capability"],
        json!({"mode":"opaque-legacy"})
    );
    assert!(codex_payload["models"]
        .as_array()
        .is_some_and(
            |models| models.iter().all(|model| model["slug"] != "hidden-alias"
                && model["slug"] != "second-alias"
                && model["slug"] != "gpt-future-unmapped")
        ));
    assert!(codex_payload.get("object").is_none());
    assert_eq!(catalog_hits.load(Ordering::SeqCst), 1);

    let restricted_response = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-legacy-only")
        .send()
        .await
        .expect("restricted Codex models request should succeed");
    assert_eq!(restricted_response.status(), StatusCode::OK);
    let restricted_payload: serde_json::Value = restricted_response
        .json()
        .await
        .expect("restricted Codex models body should parse");
    assert_eq!(
        restricted_payload["models"].as_array().map(|models| models
            .iter()
            .map(|model| model["slug"].as_str().unwrap_or_default())
            .collect::<Vec<_>>()),
        Some(vec!["legacy-alias"])
    );
    assert_eq!(catalog_hits.load(Ordering::SeqCst), 1);

    let incomplete_authorized_response = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-hidden-mixed")
        .send()
        .await
        .expect("incomplete authorized Codex catalog request should succeed");
    assert_eq!(incomplete_authorized_response.status(), StatusCode::OK);
    assert_eq!(
        incomplete_authorized_response
            .headers()
            .get(http::header::ETAG)
            .and_then(|value| value.to_str().ok()),
        Some("\"catalog-etag-v1\"")
    );
    let incomplete_authorized_payload: serde_json::Value = incomplete_authorized_response
        .json()
        .await
        .expect("incomplete authorized Codex body should parse");
    assert_eq!(
        incomplete_authorized_payload["models"]
            .as_array()
            .map(|models| models
                .iter()
                .map(|model| model["slug"].as_str().unwrap_or_default())
                .collect::<Vec<_>>()),
        Some(vec!["future-alias"]),
        "one hidden or not-yet-described mapping must not erase valid dynamic cards"
    );
    assert_eq!(catalog_hits.load(Ordering::SeqCst), 1);

    let hidden_only_response = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-hidden-only")
        .send()
        .await
        .expect("hidden-only authorized Codex catalog request should succeed");
    assert_eq!(hidden_only_response.status(), StatusCode::OK);
    assert!(hidden_only_response
        .headers()
        .get(http::header::ETAG)
        .is_none());
    let hidden_only_payload: serde_json::Value = hidden_only_response
        .json()
        .await
        .expect("hidden-only authorized Codex body should parse");
    assert_eq!(
        hidden_only_payload["models"].as_array().map(Vec::len),
        Some(0),
        "a model absent from the authoritative upstream catalog must not receive a fabricated card"
    );
    assert_eq!(catalog_hits.load(Ordering::SeqCst), 1);

    let pending_second_response = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-second-mixed")
        .send()
        .await
        .expect("not-yet-published authorized model request should succeed");
    assert_eq!(pending_second_response.status(), StatusCode::OK);
    let pending_second_payload: serde_json::Value = pending_second_response
        .json()
        .await
        .expect("not-yet-published authorized model body should parse");
    assert_eq!(
        pending_second_payload["models"]
            .as_array()
            .map(|models| models
                .iter()
                .map(|model| model["slug"].as_str().unwrap_or_default())
                .collect::<Vec<_>>()),
        Some(vec!["future-alias"])
    );
    assert_eq!(catalog_hits.load(Ordering::SeqCst), 1);

    let captured_catalog_plan = captured_plans
        .lock()
        .expect("plans mutex")
        .iter()
        .find(|(url, _)| url.contains("/models?"))
        .cloned()
        .expect("catalog execution plan should be captured");
    assert_eq!(
        captured_catalog_plan.0,
        "https://chatgpt.example/backend-api/codex/models?client_version=0.145.2"
    );
    assert_eq!(
        captured_catalog_plan.1.as_deref(),
        Some("codex_cli_rs/0.145.2")
    );

    let fresh_response = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-models")
        .send()
        .await
        .expect("fresh Codex models request should succeed");
    assert_eq!(fresh_response.status(), StatusCode::OK);
    assert_eq!(catalog_hits.load(Ordering::SeqCst), 1);

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    catalog_generation.store(1, Ordering::SeqCst);
    let stale_started = std::time::Instant::now();
    let stale_response = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-models")
        .send()
        .await
        .expect("stale Codex models request should succeed");
    assert_eq!(stale_response.status(), StatusCode::OK);
    assert!(stale_started.elapsed() < std::time::Duration::from_millis(400));
    let stale_payload: serde_json::Value = stale_response
        .json()
        .await
        .expect("stale body should parse");
    assert!(stale_payload["models"]
        .as_array()
        .is_some_and(|models| models.iter().any(|model| model["slug"] == "future-alias")));
    wait_until(1_000, || catalog_hits.load(Ordering::SeqCst) >= 2).await;

    let failed_refresh_lkg_response = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-models")
        .send()
        .await
        .expect("failed refresh should keep serving LKG");
    let failed_refresh_lkg_payload: serde_json::Value = failed_refresh_lkg_response
        .json()
        .await
        .expect("failed refresh LKG body should parse");
    assert!(failed_refresh_lkg_payload["models"]
        .as_array()
        .is_some_and(|models| models.iter().any(|model| model["slug"] == "future-alias")));
    assert_eq!(catalog_hits.load(Ordering::SeqCst), 2);

    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    catalog_generation.store(2, Ordering::SeqCst);
    let refresh_trigger = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-models")
        .send()
        .await
        .expect("recovered refresh trigger should succeed");
    assert_eq!(refresh_trigger.status(), StatusCode::OK);
    wait_until(1_000, || catalog_hits.load(Ordering::SeqCst) >= 3).await;

    let updated_response = client
        .get(format!("{gateway_url}/v1/models?client_version=0.145.2"))
        .header("authorization", "Bearer sk-codex-second-mixed")
        .send()
        .await
        .expect("updated catalog request should succeed");
    assert_eq!(
        updated_response
            .headers()
            .get(http::header::ETAG)
            .and_then(|value| value.to_str().ok()),
        Some("\"catalog-etag-v2\"")
    );
    let updated_payload: serde_json::Value = updated_response
        .json()
        .await
        .expect("updated body should parse");
    assert!(updated_payload["models"]
        .as_array()
        .is_some_and(|models| models.iter().any(|model| {
            model["slug"] == "second-alias"
                && model["future_capability"] == json!({"mode":"added-without-code-change"})
        })));
    assert!(updated_payload["models"]
        .as_array()
        .is_some_and(|models| models.iter().all(|model| {
            model["slug"] != "hidden-alias" && model["slug"] != "gpt-future-unmapped"
        })));

    let stored_keys = provider_catalog_repository
        .list_keys_by_ids(&[CATALOG_KEY_ID.to_string()])
        .await
        .expect("catalog key should remain readable");
    assert_eq!(stored_keys.as_slice(), &[original_catalog_key]);
    let rows_after = candidate_repository
        .list_for_exact_api_format("openai:responses")
        .await
        .expect("candidate rows should load after request");
    assert_eq!(rows_after, rows_before);

    let inference_response = client
        .post(format!("{gateway_url}/v1/responses"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("authorization", "Bearer sk-codex-models")
        .body(r#"{"model":"future-alias","input":"hello","store":false}"#)
        .send()
        .await
        .expect("manual alias inference should succeed");
    assert_eq!(inference_response.status(), StatusCode::OK);
    let inference_payload: serde_json::Value = inference_response
        .json()
        .await
        .expect("inference body should parse");
    assert_eq!(inference_payload["model"], "gpt-future-dynamic");

    let standard_response = client
        .get(format!("{gateway_url}/v1/models"))
        .header("authorization", "Bearer sk-standard-models")
        .send()
        .await
        .expect("standard models request should succeed");
    assert_eq!(standard_response.status(), StatusCode::OK);
    let standard_payload: serde_json::Value = standard_response
        .json()
        .await
        .expect("standard models body should parse");
    assert_eq!(standard_payload["object"], "list");
    assert!(standard_payload["data"].is_array());
    assert!(standard_payload.get("models").is_none());

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[derive(Debug)]
struct ObservedOpenAiRealtimeWebSocket {
    request_target: String,
    authorization: Option<String>,
    route_header: Option<String>,
    session_update: serde_json::Value,
    audio_append: serde_json::Value,
    binary_frame: Vec<u8>,
}

#[test]
fn gateway_relays_openai_realtime_audio_and_future_events_opaquely() {
    super::run_frontdoor_async_test(
        "openai-realtime-websocket-frontdoor",
        run_openai_realtime_websocket_frontdoor_scenario(),
    );
}

async fn run_openai_realtime_websocket_frontdoor_scenario() {
    const PROVIDER_ID: &str = "provider-openai-realtime";
    const ENDPOINT_ID: &str = "endpoint-provider-openai-realtime";
    const UPSTREAM_KEY_ID: &str = "key-provider-openai-realtime";
    const CLIENT_MODEL: &str = "realtime-client-alias";
    const PROVIDER_MODEL: &str = "gpt-realtime-future";

    let (observed_tx, observed_rx) = oneshot::channel();
    let upstream_state = Arc::new(Mutex::new(Some(observed_tx)));
    let upstream = Router::new()
        .route("/v1/realtime", get(mock_openai_realtime_websocket))
        .with_state(upstream_state);
    let (upstream_url, upstream_handle) = start_server(upstream).await;

    let mut row =
        sample_models_candidate_row(PROVIDER_ID, "openai", "openai:realtime", CLIENT_MODEL, 10);
    row.endpoint_api_family = Some("openai".to_string());
    row.endpoint_kind = Some("realtime".to_string());
    row.key_allowed_models = Some(vec![PROVIDER_MODEL.to_string()]);
    row.model_provider_model_name = PROVIDER_MODEL.to_string();
    row.model_provider_model_mappings = Some(vec![
        aether_data_contracts::repository::candidate_selection::StoredProviderModelMapping {
            name: PROVIDER_MODEL.to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:realtime".to_string()]),
            endpoint_ids: None,
            operations: None,
        },
    ]);
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            row,
        ]));

    let mut downstream_snapshot =
        unrestricted_models_snapshot("gateway-key-openai-realtime", "user-openai-realtime");
    downstream_snapshot.user_allowed_providers = Some(vec!["openai".to_string()]);
    downstream_snapshot.api_key_allowed_providers = Some(vec!["openai".to_string()]);
    downstream_snapshot.user_allowed_api_formats = Some(vec!["openai:realtime".to_string()]);
    downstream_snapshot.api_key_allowed_api_formats = Some(vec!["openai:realtime".to_string()]);
    downstream_snapshot.user_allowed_models = Some(vec![CLIENT_MODEL.to_string()]);
    downstream_snapshot.api_key_allowed_models = Some(vec![CLIENT_MODEL.to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-openai-realtime")),
        downstream_snapshot,
    )]));

    let provider = sample_provider(PROVIDER_ID, "openai", 10);
    let mut endpoint = sample_endpoint(
        ENDPOINT_ID,
        PROVIDER_ID,
        "openai:realtime",
        format!("{upstream_url}/v1").as_str(),
    );
    endpoint.api_family = Some("openai".to_string());
    endpoint.endpoint_kind = Some("realtime".to_string());
    endpoint.header_rules = Some(json!([
        {"action": "set", "key": "x-upstream-realtime-route", "value": "opaque"}
    ]));
    let mut upstream_key = sample_key(
        UPSTREAM_KEY_ID,
        PROVIDER_ID,
        "openai:realtime",
        "realtime-upstream-secret",
    );
    upstream_key.allowed_models = Some(json!([PROVIDER_MODEL]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![upstream_key],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_repository,
                provider_catalog_repository,
                request_candidate_repository,
                Arc::clone(&usage_repository),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
        .with_usage_runtime_for_tests(crate::usage::UsageRuntimeConfig {
            enabled: true,
            ..crate::usage::UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut handshake_headers = HeaderMap::new();
    handshake_headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer sk-openai-realtime"),
    );
    let invalid_model_response = wreq::Client::new()
        .websocket(format!(
            "{}/v1/realtime?model={CLIENT_MODEL}&model=duplicate",
            gateway_url.replacen("http://", "ws://", 1)
        ))
        .headers(handshake_headers.clone())
        .send()
        .await
        .expect("invalid Realtime model query should return an HTTP response");
    assert_eq!(invalid_model_response.status(), StatusCode::BAD_REQUEST);

    let rejected_upstream_response = wreq::Client::new()
        .websocket(format!(
            "{}/v1/realtime?upstream_reject=1&model={CLIENT_MODEL}",
            gateway_url.replacen("http://", "ws://", 1)
        ))
        .headers(handshake_headers.clone())
        .send()
        .await
        .expect("rejected upstream Realtime handshake should stay an HTTP response");
    assert_eq!(rejected_upstream_response.status(), StatusCode::BAD_GATEWAY);

    let response = wreq::Client::new()
        .websocket(format!(
            "{}/v1/realtime?trace=opaque&model={CLIENT_MODEL}",
            gateway_url.replacen("http://", "ws://", 1)
        ))
        .headers(handshake_headers)
        .send()
        .await
        .expect("Realtime gateway WebSocket handshake should complete");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = response
        .into_websocket()
        .await
        .expect("Realtime gateway response should upgrade");

    let session_update = json!({
        "type": "session.update",
        "session": {
            "modalities": ["audio", "text"],
            "future_session_capability": {"opaque": true, "revision": 23}
        },
        "future_event_field": [1, {"nested": true}]
    });
    let audio_append = json!({
        "type": "input_audio_buffer.append",
        "audio": "AQIDBA==",
        "future_audio_field": {"codec_revision": 7}
    });
    socket
        .send(WreqWsMessage::Text(session_update.to_string().into()))
        .await
        .expect("Realtime session.update should send");
    socket
        .send(WreqWsMessage::Text(audio_append.to_string().into()))
        .await
        .expect("Realtime audio append should send");
    socket
        .send(WreqWsMessage::Binary(vec![0, 1, 2, 255].into()))
        .await
        .expect("Realtime binary frame should send");

    let audio_delta = receive_realtime_message(&mut socket).await;
    let WreqWsMessage::Text(audio_delta) = audio_delta else {
        panic!("Realtime audio delta should remain a text frame");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(audio_delta.as_str())
            .expect("Realtime audio delta should remain valid JSON"),
        json!({
            "type": "response.audio.delta",
            "delta": "BQYHCA==",
            "future_server_field": {"opaque": true, "revision": 29}
        })
    );
    match receive_realtime_message(&mut socket).await {
        WreqWsMessage::Binary(data) => assert_eq!(data.as_ref(), &[9, 8, 7]),
        other => panic!("Realtime binary response changed frame type: {other:?}"),
    }
    let response_done = receive_realtime_message(&mut socket).await;
    let WreqWsMessage::Text(response_done) = response_done else {
        panic!("Realtime response.done should remain a text frame");
    };
    let response_done: serde_json::Value = serde_json::from_str(response_done.as_str())
        .expect("Realtime response.done should remain valid JSON");
    assert_eq!(response_done["type"], "response.done");
    assert_eq!(response_done["future_done_field"]["opaque"], true);
    assert_eq!(response_done["response"]["usage"]["input_tokens"], 12);
    assert_eq!(
        response_done["response"]["usage"]["output_token_details"]["audio_tokens"],
        3
    );
    match receive_realtime_message(&mut socket).await {
        WreqWsMessage::Close(_) => {}
        other => panic!("Realtime upstream close changed frame type: {other:?}"),
    }

    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), observed_rx)
        .await
        .expect("mock Realtime upstream should report before timeout")
        .expect("mock Realtime observation channel should remain open");
    assert_eq!(
        observed.request_target,
        format!("/v1/realtime?trace=opaque&model={PROVIDER_MODEL}")
    );
    assert_eq!(
        observed.authorization.as_deref(),
        Some("Bearer realtime-upstream-secret")
    );
    assert_eq!(observed.route_header.as_deref(), Some("opaque"));
    assert_eq!(observed.session_update, session_update);
    assert_eq!(observed.audio_append, audio_append);
    assert_eq!(observed.binary_frame, vec![0, 1, 2, 255]);

    let realtime_usage = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let records = usage_repository
                .list_usage_audits(&UsageAuditListQuery::default())
                .await
                .expect("Realtime usage audit list should load");
            if let Some(record) = records
                .into_iter()
                .find(|record| record.request_type.as_deref() == Some("realtime"))
            {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Realtime session usage audit should be persisted before timeout");
    assert_eq!(realtime_usage.status, "completed");
    assert_eq!(
        realtime_usage.api_format.as_deref(),
        Some("openai:realtime")
    );
    assert_eq!(
        realtime_usage.endpoint_api_format.as_deref(),
        Some("openai:realtime")
    );
    assert!(realtime_usage.is_websocket());
    assert_eq!(
        realtime_usage.websocket_transport(),
        Some("openai_realtime")
    );
    assert!(realtime_usage.usage_available());
    assert!(!realtime_usage.usage_pricing_available());
    assert_eq!(realtime_usage.billing_status, "void");
    assert_eq!(realtime_usage.input_tokens, 12);
    assert_eq!(realtime_usage.output_tokens, 7);
    assert_eq!(realtime_usage.total_tokens, 19);
    assert_eq!(realtime_usage.cache_read_input_tokens, 4);
    assert_eq!(realtime_usage.total_cost_usd, 0.0);
    assert_eq!(realtime_usage.actual_total_cost_usd, 0.0);
    let realtime_metadata = realtime_usage
        .request_metadata
        .as_ref()
        .expect("Realtime usage metadata should be present");
    assert_eq!(
        realtime_metadata["realtime_session"]["usage_scope"],
        "response_done"
    );
    assert_eq!(
        realtime_metadata["realtime_session"]["input_audio_tokens"],
        5
    );
    assert_eq!(
        realtime_metadata["realtime_session"]["output_audio_tokens"],
        3
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

async fn mock_openai_realtime_websocket(
    State(observed): State<Arc<Mutex<Option<oneshot::Sender<ObservedOpenAiRealtimeWebSocket>>>>>,
    uri: Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    if uri.query().is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .any(|(name, value)| name == "upstream_reject" && value == "1")
    }) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request_target = uri.to_string();
    let authorization = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let route_header = headers
        .get("x-upstream-realtime-route")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    ws.on_upgrade(move |mut socket| async move {
        let session_update = receive_axum_live_json(&mut socket).await;
        let audio_append = receive_axum_live_json(&mut socket).await;
        let binary_frame =
            match tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv())
                .await
                .expect("mock Realtime upstream should receive binary frame before timeout")
                .expect("mock Realtime upstream should remain open")
                .expect("mock Realtime binary frame should be readable")
            {
                AxumWsMessage::Binary(data) => data.to_vec(),
                other => panic!("mock Realtime upstream expected binary frame, got {other:?}"),
            };

        socket
            .send(AxumWsMessage::Text(
                json!({
                    "type": "response.audio.delta",
                    "delta": "BQYHCA==",
                    "future_server_field": {"opaque": true, "revision": 29}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("mock Realtime audio delta should send");
        socket
            .send(AxumWsMessage::Binary(vec![9, 8, 7].into()))
            .await
            .expect("mock Realtime binary response should send");
        socket
            .send(AxumWsMessage::Text(
                json!({
                    "type": "response.done",
                    "future_done_field": {"opaque": true},
                    "response": {
                        "id": "resp_realtime_frontdoor",
                        "usage": {
                            "input_tokens": 12,
                            "output_tokens": 7,
                            "total_tokens": 19,
                            "input_token_details": {"cached_tokens": 4, "audio_tokens": 5},
                            "output_token_details": {"audio_tokens": 3}
                        }
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("mock Realtime response.done should send");
        socket
            .send(AxumWsMessage::Close(None))
            .await
            .expect("mock Realtime close should send");

        if let Some(sender) = observed
            .lock()
            .expect("mock Realtime observation mutex should lock")
            .take()
        {
            let _ = sender.send(ObservedOpenAiRealtimeWebSocket {
                request_target,
                authorization,
                route_header,
                session_update,
                audio_append,
                binary_frame,
            });
        }
    })
    .into_response()
}

async fn receive_realtime_message(socket: &mut wreq::ws::WebSocket) -> WreqWsMessage {
    tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv())
        .await
        .expect("Realtime gateway should send a frame before timeout")
        .expect("Realtime gateway socket should remain open")
        .expect("Realtime gateway frame should be readable")
}

#[test]
fn gateway_creates_bound_codex_live_oauth_calls_with_legacy_responses_mapping() {
    super::run_frontdoor_async_test(
        "codex-live-oauth-frontdoor",
        run_codex_live_oauth_frontdoor_scenario(CodexLiveWebRtcTestDialect::LegacyLive),
    );
}

#[test]
fn gateway_creates_bound_codex_realtime_oauth_calls_with_legacy_responses_mapping() {
    super::run_frontdoor_async_test(
        "codex-realtime-oauth-frontdoor",
        run_codex_live_oauth_frontdoor_scenario(CodexLiveWebRtcTestDialect::Realtime),
    );
}

async fn run_codex_live_oauth_frontdoor_scenario(dialect: CodexLiveWebRtcTestDialect) {
    const PROVIDER_ID: &str = "provider-codex-live";
    const ENDPOINT_ID: &str = "endpoint-provider-codex-live";
    const UPSTREAM_KEY_ID: &str = "key-provider-codex-live";
    const CLIENT_MODEL: &str = "live-future-alias";
    const PROVIDER_MODEL: &str = "gpt-future-live";
    const CALL_ID: &str = "rtc_frontdoor_live";

    let mut row = sample_codex_live_candidate_row(PROVIDER_ID, CLIENT_MODEL, PROVIDER_MODEL);
    // Existing Codex associations predate the dedicated Live format. The
    // provider-aware compatibility rule must carry their Responses-scoped
    // source mapping through the complete scheduler, not only the repository
    // fast path.
    if let Some(mappings) = row.model_provider_model_mappings.as_mut() {
        for mapping in mappings {
            mapping.api_formats = Some(vec!["openai:responses".to_string()]);
        }
    }
    row.key_allowed_models = Some(vec![PROVIDER_MODEL.to_string()]);
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            row,
        ]));
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-codex-live")),
        codex_live_snapshot("gateway-key-codex-live", "user-codex-live", &[CLIENT_MODEL]),
    )]));

    let mut provider = codex_catalog_provider(PROVIDER_ID);
    provider.config = Some(json!({
        "responses_websocket": {"enabled": true},
        "codex": {"fingerprint_convergence_enabled": true}
    }));
    let mut endpoint = codex_live_catalog_endpoint(PROVIDER_ID, ENDPOINT_ID);
    endpoint.base_url = "https://chatgpt.com/backend-api/codex".to_string();
    let mut upstream_key = codex_live_catalog_key(PROVIDER_ID, UPSTREAM_KEY_ID, &[PROVIDER_MODEL]);
    upstream_key.auth_type = "oauth".to_string();
    upstream_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"account_id":"account-live-1","is_fedramp":true}"#,
        )
        .expect("Codex Live auth config should encrypt"),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![upstream_key],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    let captured_plan = Arc::new(Mutex::new(None::<aether_contracts::ExecutionPlan>));
    let captured_plan_for_runtime = Arc::clone(&captured_plan);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let captured_plan_for_request = Arc::clone(&captured_plan_for_runtime);
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX)
                    .await
                    .expect("Live execution runtime request body should read");
                let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(&raw_body)
                    .expect("Live execution runtime plan should parse");
                *captured_plan_for_request
                    .lock()
                    .expect("Live plan mutex should lock") = Some(plan.clone());
                Json(ExecutionResult {
                    request_id: plan.request_id,
                    candidate_id: plan.candidate_id,
                    status_code: 201,
                    headers: std::collections::BTreeMap::from([
                        ("Content-Type".to_string(), "application/sdp".to_string()),
                        (
                            "LOCATION".to_string(),
                            format!("https://api.openai.com/v1/live/{CALL_ID}"),
                        ),
                    ]),
                    response_observation: None,
                    body: Some(ResponseBody {
                        json_body: None,
                        body_bytes_b64: Some(
                            base64::engine::general_purpose::STANDARD
                                .encode(b"v=0\r\no=upstream-answer"),
                        ),
                    }),
                    telemetry: Some(ExecutionTelemetry {
                        ttfb_ms: Some(1),
                        elapsed_ms: Some(2),
                        upstream_bytes: Some(24),
                    }),
                    error: None,
                })
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_repository,
                provider_catalog_repository,
                request_candidate_repository,
                Arc::clone(&usage_repository),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
        .with_usage_runtime_for_tests(crate::usage::UsageRuntimeConfig {
            enabled: true,
            ..crate::usage::UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let boundary = "aether-live-frontdoor";
    let offer_sdp = "v=0\r\no=client-offer";
    let session = json!({
        "model": CLIENT_MODEL,
        "instructions": "Keep this opaque",
        "future_capability": {
            "revision": 7,
            "nested": [true, {"mode": "future"}]
        }
    });
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\n{offer_sdp}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--{boundary}--\r\n",
        session
    );
    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}{}",
            dialect.call_create_request_target()
        ))
        .header("authorization", "Bearer sk-codex-live")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("session-id", "client-session-live")
        .header("openai-alpha", "client-must-not-control-this")
        .body(body)
        .send()
        .await
        .expect("Codex Live call creation should complete");
    assert_eq!(response.status(), StatusCode::CREATED);
    let downstream_location = dialect.call_location(CALL_ID);
    assert_eq!(
        response
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some(downstream_location.as_str())
    );
    assert_eq!(
        response
            .bytes()
            .await
            .expect("SDP answer should read")
            .as_ref(),
        b"v=0\r\no=upstream-answer"
    );

    let plan = captured_plan
        .lock()
        .expect("Live plan mutex should lock")
        .clone()
        .expect("Live call must reach the execution runtime");
    let url = url::Url::parse(plan.url.as_str()).expect("Live call URL should parse");
    assert_eq!(url.path(), "/backend-api/codex/realtime/calls");
    assert_eq!(
        url.query_pairs().collect::<HashMap<_, _>>(),
        HashMap::from([
            ("intent".into(), "quicksilver".into()),
            ("architecture".into(), "avas".into()),
        ])
    );
    assert_eq!(plan.method, "POST");
    assert_eq!(plan.content_type.as_deref(), Some("application/json"));
    assert!(!plan.stream);
    assert!(plan.body.json_body.is_none());
    let provider_body_bytes = base64::engine::general_purpose::STANDARD
        .decode(
            plan.body
                .body_bytes_b64
                .as_deref()
                .expect("OAuth Live call must preserve the exact JSON wire bytes"),
        )
        .expect("OAuth Live JSON body should decode");
    let provider_body: serde_json::Value = serde_json::from_slice(&provider_body_bytes)
        .expect("OAuth Live call must use the JSON call contract");
    assert_eq!(provider_body["sdp"], offer_sdp);
    assert_eq!(provider_body["session"]["model"], PROVIDER_MODEL);
    assert_eq!(
        provider_body["session"]["future_capability"],
        session["future_capability"]
    );
    assert_eq!(provider_body["session"]["instructions"], "Keep this opaque");
    assert_eq!(
        plan.headers.get("openai-alpha").map(String::as_str),
        Some(dialect.alpha_header())
    );
    assert_eq!(
        plan.headers.get("originator").map(String::as_str),
        Some("codex_cli_rs")
    );
    assert_eq!(
        plan.headers.get("chatgpt-account-id").map(String::as_str),
        Some("account-live-1")
    );
    assert_eq!(
        plan.headers.get("x-openai-fedramp").map(String::as_str),
        Some("true")
    );
    let converged_session = plan
        .headers
        .get("x-session-id")
        .expect("Live must provide a converged session ID");
    assert_ne!(converged_session, "client-session-live");
    assert_eq!(plan.headers.get("thread-id"), Some(converged_session));
    uuid::Uuid::parse_str(converged_session).expect("converged session ID must be a UUID");

    let call_create_usage = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(usage) = usage_repository
                .find_by_request_id(plan.request_id.as_str())
                .await
                .expect("Live call-create usage read should succeed")
            {
                break usage;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("Live call-create usage should be persisted before timeout");
    assert_eq!(call_create_usage.status, "completed");
    assert_eq!(call_create_usage.billing_status, "void");
    assert_eq!(call_create_usage.status_code, Some(201));
    assert_eq!(call_create_usage.request_type.as_deref(), Some("live"));
    assert_eq!(call_create_usage.api_format.as_deref(), Some("codex:live"));
    assert_eq!(call_create_usage.model, CLIENT_MODEL);
    assert!(!call_create_usage.is_stream);
    assert!(!call_create_usage.is_websocket());
    assert_eq!(call_create_usage.websocket_transport(), None);
    assert!(!call_create_usage.usage_available());
    assert!(!call_create_usage.usage_pricing_available());
    assert_eq!(call_create_usage.total_tokens, 0);
    assert_eq!(call_create_usage.total_cost_usd, 0.0);
    assert_eq!(call_create_usage.actual_total_cost_usd, 0.0);
    assert!(call_create_usage.request_headers.is_none());
    assert!(call_create_usage.request_body.is_none());
    assert!(call_create_usage.provider_request_headers.is_none());
    assert!(call_create_usage.provider_request_body.is_none());
    let serialized_call_create_usage =
        serde_json::to_string(&call_create_usage).expect("Live call-create usage should serialize");
    for sentinel in [
        offer_sdp,
        "Keep this opaque",
        "sk-codex-live",
        "oauth-upstream-secret",
    ] {
        assert!(!serialized_call_create_usage.contains(sentinel));
    }

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[derive(Debug)]
struct ObservedCodexLiveWebSocket {
    request_target: String,
    authorization: Option<String>,
    alpha: Option<String>,
    originator: Option<String>,
    session_id: Option<String>,
    initial_event: serde_json::Value,
    event_after_turn_done: serde_json::Value,
}

#[derive(Debug, Clone, Copy)]
enum CodexLiveDirectTestDialect {
    LegacyLive,
    RealtimeV1,
    RealtimeV2,
}

impl CodexLiveDirectTestDialect {
    fn public_path(self) -> &'static str {
        match self {
            Self::LegacyLive => "/v1/live",
            Self::RealtimeV1 | Self::RealtimeV2 => "/v1/realtime",
        }
    }

    fn query_prefix(self) -> &'static str {
        match self {
            Self::RealtimeV1 => "intent=quicksilver&",
            Self::LegacyLive | Self::RealtimeV2 => "",
        }
    }

    fn expected_upstream_target(self, provider_model: &str) -> String {
        match self {
            Self::LegacyLive => format!("/v1/live?model={provider_model}"),
            Self::RealtimeV1 => {
                format!("/v1/realtime?intent=quicksilver&model={provider_model}")
            }
            Self::RealtimeV2 => format!("/v1/realtime?model={provider_model}"),
        }
    }

    fn expected_alpha(self) -> Option<&'static str> {
        match self {
            Self::LegacyLive => Some("quicksilver=v2"),
            Self::RealtimeV1 => Some("quicksilver=v1"),
            Self::RealtimeV2 => None,
        }
    }
}

#[test]
fn gateway_relays_codex_live_api_key_websocket_opaquely() {
    super::run_frontdoor_async_test(
        "codex-live-api-key-websocket-frontdoor",
        run_codex_live_api_key_websocket_frontdoor_scenario(CodexLiveDirectTestDialect::LegacyLive),
    );
}

#[test]
fn gateway_relays_codex_realtime_v2_api_key_websocket_opaquely() {
    super::run_frontdoor_async_test(
        "codex-realtime-v2-api-key-websocket-frontdoor",
        run_codex_live_api_key_websocket_frontdoor_scenario(CodexLiveDirectTestDialect::RealtimeV2),
    );
}

#[test]
fn gateway_relays_codex_realtime_v1_api_key_websocket_opaquely() {
    super::run_frontdoor_async_test(
        "codex-realtime-v1-api-key-websocket-frontdoor",
        run_codex_live_api_key_websocket_frontdoor_scenario(CodexLiveDirectTestDialect::RealtimeV1),
    );
}

async fn run_codex_live_api_key_websocket_frontdoor_scenario(dialect: CodexLiveDirectTestDialect) {
    const PROVIDER_ID: &str = "provider-codex-live-api-key";
    const ENDPOINT_ID: &str = "endpoint-provider-codex-live-api-key";
    const UPSTREAM_KEY_ID: &str = "key-provider-codex-live-api-key";
    const CLIENT_MODEL: &str = "live-websocket-alias";
    const PROVIDER_MODEL: &str = "gpt-future-live-websocket";

    let (observed_tx, observed_rx) = oneshot::channel();
    let upstream_state = Arc::new(Mutex::new(Some(observed_tx)));
    let upstream = Router::new()
        .route("/v1/live", get(mock_codex_live_websocket))
        .route("/v1/realtime", get(mock_codex_live_websocket))
        .with_state(upstream_state);
    let (upstream_url, upstream_handle) = start_server(upstream).await;

    let mut row = sample_codex_live_candidate_row(PROVIDER_ID, CLIENT_MODEL, PROVIDER_MODEL);
    row.provider_name = "openai".to_string();
    row.provider_type = "openai".to_string();
    row.key_auth_type = "api_key".to_string();
    row.key_allowed_models = Some(vec![PROVIDER_MODEL.to_string()]);
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            row,
        ]));
    let mut downstream_snapshot = codex_live_snapshot(
        "gateway-key-codex-live-websocket",
        "user-codex-live-websocket",
        &[CLIENT_MODEL],
    );
    downstream_snapshot.user_allowed_providers = Some(vec!["openai".to_string()]);
    downstream_snapshot.api_key_allowed_providers = Some(vec!["openai".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-codex-live-websocket")),
        downstream_snapshot,
    )]));

    let mut provider = codex_catalog_provider(PROVIDER_ID);
    provider.provider_type = "openai".to_string();
    provider.config = Some(json!({"responses_websocket": {"enabled": true}}));
    let mut endpoint = codex_live_catalog_endpoint(PROVIDER_ID, ENDPOINT_ID);
    endpoint.base_url = format!("{upstream_url}/v1");
    let mut upstream_key = codex_live_catalog_key(PROVIDER_ID, UPSTREAM_KEY_ID, &[PROVIDER_MODEL]);
    upstream_key.auth_type = "api_key".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![upstream_key],
    ));

    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                candidate_repository,
                auth_repository,
            )
            .attach_provider_catalog_repository_for_tests(provider_catalog_repository)
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut handshake_headers = HeaderMap::new();
    handshake_headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer sk-codex-live-websocket"),
    );
    handshake_headers.insert(
        http::HeaderName::from_static("x-session-id"),
        http::HeaderValue::from_static("stable-live-session"),
    );
    handshake_headers.insert(
        http::HeaderName::from_static("openai-alpha"),
        http::HeaderValue::from_static("client-value-must-be-replaced"),
    );
    if matches!(dialect, CodexLiveDirectTestDialect::RealtimeV2) {
        handshake_headers.insert(
            http::HeaderName::from_static("originator"),
            http::HeaderValue::from_static("codex_work_desktop"),
        );
    }
    let invalid_model_response = wreq::Client::new()
        .websocket(format!(
            "{}{}?{}model={CLIENT_MODEL}&model=second-model",
            gateway_url.replacen("http://", "ws://", 1),
            dialect.public_path(),
            dialect.query_prefix(),
        ))
        .headers(handshake_headers.clone())
        .send()
        .await
        .expect("invalid Live model handshake should return an HTTP response");
    assert_eq!(invalid_model_response.status(), StatusCode::BAD_REQUEST);

    let websocket_url = format!(
        "{}{}?{}foo=bar&model={CLIENT_MODEL}&trace=1",
        gateway_url.replacen("http://", "ws://", 1),
        dialect.public_path(),
        dialect.query_prefix(),
    );
    let response = wreq::Client::new()
        .websocket(websocket_url)
        .headers(handshake_headers)
        .send()
        .await
        .expect("Codex Live gateway WebSocket handshake should complete");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = response
        .into_websocket()
        .await
        .expect("Codex Live gateway response should upgrade");

    let initial_event = json!({
        "type": "session.update",
        "session": {
            "model": CLIENT_MODEL,
            "instructions": "Relay this Live configuration"
        },
        "future_client_field": {
            "opaque": true,
            "revision": 9,
            "nested": [1, {"mode": "future"}]
        }
    });
    socket
        .send(WreqWsMessage::text(initial_event.to_string()))
        .await
        .expect("initial Live session.update should send");

    let future_event = receive_codex_live_json(&mut socket).await;
    assert_eq!(
        future_event,
        json!({
            "type": "future.live.event",
            "future_capability": {"enabled": true, "revision": 11}
        })
    );
    let turn_done = receive_codex_live_json(&mut socket).await;
    assert_eq!(
        turn_done,
        json!({
            "type": "turn.done",
            "turn": {"id": "turn-live-1"},
            "future_turn_field": "retained"
        })
    );

    let event_after_turn_done = json!({
        "type": "future.client.after_turn_done",
        "future_payload": {"still_connected": true}
    });
    socket
        .send(WreqWsMessage::text(event_after_turn_done.to_string()))
        .await
        .expect("Live socket should remain writable after turn.done");

    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), observed_rx)
        .await
        .expect("mock upstream should observe the post-turn event before timeout")
        .expect("mock upstream observation channel should remain open");
    assert_eq!(
        observed.request_target,
        dialect.expected_upstream_target(PROVIDER_MODEL)
    );
    assert_eq!(
        observed.authorization.as_deref(),
        Some("Bearer oauth-upstream-secret")
    );
    assert_eq!(observed.alpha.as_deref(), dialect.expected_alpha());
    assert_eq!(
        observed.originator.as_deref(),
        matches!(dialect, CodexLiveDirectTestDialect::RealtimeV2).then_some("codex_work_desktop")
    );
    assert_eq!(observed.session_id.as_deref(), Some("stable-live-session"));
    let mut expected_initial_event = initial_event;
    expected_initial_event["session"]["model"] = json!(PROVIDER_MODEL);
    assert_eq!(observed.initial_event, expected_initial_event);
    assert_eq!(observed.event_after_turn_done, event_after_turn_done);

    drop(socket);
    gateway_handle.abort();
    upstream_handle.abort();
}

#[derive(Debug)]
struct ObservedCodexLiveSideband {
    request_target: String,
    authorization: Option<String>,
    alpha: Option<String>,
    session_id: Option<String>,
    first_client_event: serde_json::Value,
    session_update: serde_json::Value,
}

#[derive(Debug, Clone, Copy)]
enum CodexLiveWebRtcTestDialect {
    LegacyLive,
    Realtime,
}

impl CodexLiveWebRtcTestDialect {
    fn call_create_path(self) -> &'static str {
        match self {
            Self::LegacyLive => "/v1/live",
            Self::Realtime => "/v1/realtime/calls",
        }
    }

    fn call_location(self, call_id: &str) -> String {
        format!("{}/{call_id}", self.call_create_path())
    }

    fn call_create_request_target(self) -> &'static str {
        match self {
            Self::LegacyLive => "/v1/live",
            Self::Realtime => "/v1/realtime/calls?intent=quicksilver&architecture=avas",
        }
    }

    fn alpha_header(self) -> &'static str {
        match self {
            Self::LegacyLive => "quicksilver=v2",
            Self::Realtime => "quicksilver=v1",
        }
    }

    fn sideband_path(self, call_id: &str) -> String {
        match self {
            Self::LegacyLive => format!("/v1/live/{call_id}"),
            Self::Realtime => format!("/v1/realtime?intent=quicksilver&call_id={call_id}"),
        }
    }
}

#[test]
fn gateway_creates_and_relays_bound_codex_live_api_key_sideband() {
    super::run_frontdoor_async_test(
        "codex-live-api-key-sideband-frontdoor",
        run_codex_live_api_key_sideband_frontdoor_scenario(CodexLiveWebRtcTestDialect::LegacyLive),
    );
}

#[test]
fn gateway_creates_and_relays_bound_codex_live_api_key_realtime_calls_sideband() {
    super::run_frontdoor_async_test(
        "codex-live-api-key-realtime-calls-sideband-frontdoor",
        run_codex_live_api_key_sideband_frontdoor_scenario(CodexLiveWebRtcTestDialect::Realtime),
    );
}

async fn run_codex_live_api_key_sideband_frontdoor_scenario(dialect: CodexLiveWebRtcTestDialect) {
    const PROVIDER_ID: &str = "provider-codex-live-sideband";
    const ENDPOINT_ID: &str = "endpoint-provider-codex-live-sideband";
    const UPSTREAM_KEY_ID: &str = "key-provider-codex-live-sideband";
    const CLIENT_MODEL: &str = "live-sideband-alias";
    const PROVIDER_MODEL: &str = "gpt-future-live-sideband";
    const CALL_ID: &str = "rtc_live_sideband_1";

    let (sideband_observed_tx, sideband_observed_rx) = oneshot::channel();
    let upstream_state = Arc::new(Mutex::new(Some(sideband_observed_tx)));
    let upstream = Router::new()
        .route(
            "/v1/live/{call_id}",
            get(mock_codex_live_sideband_websocket),
        )
        .route("/v1/realtime", get(mock_codex_live_sideband_websocket))
        .with_state(upstream_state);
    let (upstream_url, upstream_handle) = start_server(upstream).await;

    let captured_plan = Arc::new(Mutex::new(None::<aether_contracts::ExecutionPlan>));
    let captured_plan_for_runtime = Arc::clone(&captured_plan);
    let upstream_location = format!("{upstream_url}{}", dialect.call_location(CALL_ID));
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let captured_plan_for_request = Arc::clone(&captured_plan_for_runtime);
            let upstream_location = upstream_location.clone();
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX)
                    .await
                    .expect("Live API-key execution runtime body should read");
                let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(&raw_body)
                    .expect("Live API-key execution plan should parse");
                *captured_plan_for_request
                    .lock()
                    .expect("Live API-key plan mutex should lock") = Some(plan.clone());
                Json(ExecutionResult {
                    request_id: plan.request_id,
                    candidate_id: plan.candidate_id,
                    status_code: 201,
                    headers: std::collections::BTreeMap::from([
                        ("content-type".to_string(), "application/sdp".to_string()),
                        ("location".to_string(), upstream_location),
                        ("x-future-live-header".to_string(), "preserved".to_string()),
                    ]),
                    response_observation: None,
                    body: Some(ResponseBody {
                        json_body: None,
                        body_bytes_b64: Some(
                            base64::engine::general_purpose::STANDARD
                                .encode(b"v=0\r\no=api-key-upstream-answer"),
                        ),
                    }),
                    telemetry: Some(ExecutionTelemetry {
                        ttfb_ms: Some(1),
                        elapsed_ms: Some(2),
                        upstream_bytes: Some(34),
                    }),
                    error: None,
                })
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let mut row = sample_codex_live_candidate_row(PROVIDER_ID, CLIENT_MODEL, PROVIDER_MODEL);
    row.provider_name = "openai".to_string();
    row.provider_type = "openai".to_string();
    row.key_auth_type = "api_key".to_string();
    row.key_allowed_models = Some(vec![PROVIDER_MODEL.to_string()]);
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            row,
        ]));
    let mut downstream_snapshot = codex_live_snapshot(
        "gateway-key-codex-live-sideband",
        "user-codex-live-sideband",
        &[CLIENT_MODEL],
    );
    downstream_snapshot.user_allowed_providers = Some(vec!["openai".to_string()]);
    downstream_snapshot.api_key_allowed_providers = Some(vec!["openai".to_string()]);
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-codex-live-sideband")),
        downstream_snapshot,
    )]));

    let mut provider = codex_catalog_provider(PROVIDER_ID);
    provider.provider_type = "openai".to_string();
    provider.config = Some(json!({"responses_websocket": {"enabled": true}}));
    let mut endpoint = codex_live_catalog_endpoint(PROVIDER_ID, ENDPOINT_ID);
    endpoint.base_url = format!("{upstream_url}/v1");
    let mut upstream_key = codex_live_catalog_key(PROVIDER_ID, UPSTREAM_KEY_ID, &[PROVIDER_MODEL]);
    upstream_key.auth_type = "api_key".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![upstream_key],
    ));
    let state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                candidate_repository,
                auth_repository,
            )
            .attach_provider_catalog_repository_for_tests(provider_catalog_repository)
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let sideband_path = dialect.sideband_path(CALL_ID);
    let sideband_url = format!(
        "{}{}",
        gateway_url.replacen("http://", "ws://", 1),
        sideband_path
    );
    let mut sideband_headers = HeaderMap::new();
    sideband_headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_static("Bearer sk-codex-live-sideband"),
    );
    sideband_headers.insert(
        http::HeaderName::from_static("x-session-id"),
        http::HeaderValue::from_static("stable-live-sideband-session"),
    );
    let missing_binding_response = wreq::Client::new()
        .websocket(sideband_url.clone())
        .headers(sideband_headers.clone())
        .send()
        .await
        .expect("missing Live sideband binding should return an HTTP response");
    assert_eq!(missing_binding_response.status(), StatusCode::NOT_FOUND);

    let boundary = "aether-live-api-key-sideband";
    let offer_sdp = "v=0\r\no=api-key-client-offer";
    let session = json!({
        "model": CLIENT_MODEL,
        "instructions": "Preserve this API-key Live session",
        "future_session_capability": {
            "revision": 13,
            "nested": [true, {"mode": "opaque"}]
        }
    });
    let multipart_body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\n{offer_sdp}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--{boundary}--\r\n",
        session
    );
    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}{}",
            dialect.call_create_request_target()
        ))
        .header("authorization", "Bearer sk-codex-live-sideband")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("x-session-id", "stable-live-sideband-session")
        .header("openai-alpha", "client-value-must-be-replaced")
        .body(multipart_body)
        .send()
        .await
        .expect("Codex Live API-key call creation should complete");
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some(dialect.call_location(CALL_ID).as_str())
    );
    assert_eq!(
        response
            .headers()
            .get("x-future-live-header")
            .and_then(|value| value.to_str().ok()),
        Some("preserved")
    );
    assert_eq!(
        response
            .bytes()
            .await
            .expect("Live API-key SDP answer should read")
            .as_ref(),
        b"v=0\r\no=api-key-upstream-answer"
    );

    let plan = captured_plan
        .lock()
        .expect("Live API-key plan mutex should lock")
        .clone()
        .expect("Live API-key call should reach execution runtime");
    let plan_url = url::Url::parse(plan.url.as_str()).expect("Live API-key URL should parse");
    assert_eq!(plan_url.path(), dialect.call_create_path());
    match dialect {
        CodexLiveWebRtcTestDialect::LegacyLive => assert!(plan_url.query().is_none()),
        CodexLiveWebRtcTestDialect::Realtime => assert_eq!(
            plan_url.query_pairs().collect::<HashMap<_, _>>(),
            HashMap::from([
                ("intent".into(), "quicksilver".into()),
                ("architecture".into(), "avas".into()),
            ])
        ),
    }
    assert_eq!(plan.method, "POST");
    assert!(!plan.stream);
    assert!(plan
        .content_type
        .as_deref()
        .is_some_and(|value| value.starts_with("multipart/form-data; boundary=")));
    assert!(plan.body.json_body.is_none());
    let provider_multipart = base64::engine::general_purpose::STANDARD
        .decode(
            plan.body
                .body_bytes_b64
                .as_deref()
                .expect("API-key Live call should preserve multipart wire bytes"),
        )
        .expect("provider multipart body should decode");
    let provider_multipart =
        String::from_utf8(provider_multipart).expect("provider multipart should be UTF-8");
    assert!(provider_multipart.contains(offer_sdp));
    assert!(provider_multipart.contains(PROVIDER_MODEL));
    assert!(!provider_multipart.contains(CLIENT_MODEL));
    assert!(provider_multipart.contains("future_session_capability"));
    assert_eq!(
        plan.headers.get("authorization").map(String::as_str),
        Some("Bearer oauth-upstream-secret")
    );
    assert_eq!(
        plan.headers.get("openai-alpha").map(String::as_str),
        Some(dialect.alpha_header())
    );
    assert_eq!(
        plan.headers.get("x-session-id").map(String::as_str),
        Some("stable-live-sideband-session")
    );
    assert_eq!(
        plan.headers.get("accept").map(String::as_str),
        Some("application/sdp")
    );

    let sideband_response = wreq::Client::new()
        .websocket(sideband_url)
        .headers(sideband_headers.clone())
        .send()
        .await
        .expect("Codex Live sideband handshake should complete");
    assert_eq!(sideband_response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut sideband = sideband_response
        .into_websocket()
        .await
        .expect("Codex Live sideband response should upgrade");

    // The client deliberately sends nothing before this receive. If sideband
    // incorrectly reused the direct-WebSocket session.update bootstrap, this
    // event could not arrive.
    let ready_event = receive_codex_live_json(&mut sideband).await;
    assert_eq!(
        ready_event,
        json!({
            "type": "future.sideband.ready",
            "future_capability": {"opaque": true, "revision": 17}
        })
    );
    let conflicting_response = wreq::Client::new()
        .websocket(format!(
            "{}{}",
            gateway_url.replacen("http://", "ws://", 1),
            dialect.sideband_path(CALL_ID)
        ))
        .headers(sideband_headers)
        .send()
        .await
        .expect("duplicate Live sideband attachment should return an HTTP response");
    assert_eq!(conflicting_response.status(), StatusCode::CONFLICT);

    let opaque_command = json!({
        "type": "future.sideband.command",
        "future_payload": {"without_session_update": true}
    });
    sideband
        .send(WreqWsMessage::text(opaque_command.to_string()))
        .await
        .expect("opaque sideband command should send without session.update");
    let sideband_session_update = json!({
        "type": "session.update",
        "session": {
            "model": "untrusted-client-model",
            "future_session_field": {"opaque": true}
        },
        "future_event_field": [1, 2, 3]
    });
    sideband
        .send(WreqWsMessage::text(sideband_session_update.to_string()))
        .await
        .expect("sideband session.update should send after an opaque frame");

    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), sideband_observed_rx)
        .await
        .expect("mock sideband should observe the opaque command before timeout")
        .expect("mock sideband observation channel should remain open");
    assert_eq!(observed.request_target, sideband_path);
    assert_eq!(
        observed.authorization.as_deref(),
        Some("Bearer oauth-upstream-secret")
    );
    assert_eq!(observed.alpha.as_deref(), Some(dialect.alpha_header()));
    assert_eq!(
        observed.session_id.as_deref(),
        Some("stable-live-sideband-session")
    );
    assert_eq!(observed.first_client_event, opaque_command);
    let mut expected_sideband_session_update = sideband_session_update;
    expected_sideband_session_update["session"]["model"] = json!(PROVIDER_MODEL);
    assert_eq!(observed.session_update, expected_sideband_session_update);

    drop(sideband);
    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

async fn mock_codex_live_sideband_websocket(
    State(observed): State<Arc<Mutex<Option<oneshot::Sender<ObservedCodexLiveSideband>>>>>,
    uri: Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let request_target = uri.to_string();
    let authorization = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let alpha = headers
        .get("openai-alpha")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let session_id = headers
        .get("x-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    ws.on_upgrade(move |mut socket| async move {
        socket
            .send(AxumWsMessage::Text(
                json!({
                    "type": "future.sideband.ready",
                    "future_capability": {"opaque": true, "revision": 17}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("mock upstream sideband ready event should send");
        let first_client_event = receive_axum_live_json(&mut socket).await;
        let session_update = receive_axum_live_json(&mut socket).await;
        let observation = ObservedCodexLiveSideband {
            request_target,
            authorization,
            alpha,
            session_id,
            first_client_event,
            session_update,
        };
        if let Some(sender) = observed
            .lock()
            .expect("mock sideband observation mutex should lock")
            .take()
        {
            let _ = sender.send(observation);
        }
    })
}

async fn mock_codex_live_websocket(
    State(observed): State<Arc<Mutex<Option<oneshot::Sender<ObservedCodexLiveWebSocket>>>>>,
    uri: Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let request_target = uri.to_string();
    let authorization = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let alpha = headers
        .get("openai-alpha")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let originator = headers
        .get("originator")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let session_id = headers
        .get("x-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    ws.on_upgrade(move |mut socket| async move {
        let initial_event = receive_axum_live_json(&mut socket).await;
        socket
            .send(AxumWsMessage::Text(
                json!({
                    "type": "future.live.event",
                    "future_capability": {"enabled": true, "revision": 11}
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("mock upstream future event should send");
        socket
            .send(AxumWsMessage::Text(
                json!({
                    "type": "turn.done",
                    "turn": {"id": "turn-live-1"},
                    "future_turn_field": "retained"
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("mock upstream turn.done should send");
        let event_after_turn_done = receive_axum_live_json(&mut socket).await;
        let observation = ObservedCodexLiveWebSocket {
            request_target,
            authorization,
            alpha,
            originator,
            session_id,
            initial_event,
            event_after_turn_done,
        };
        if let Some(sender) = observed
            .lock()
            .expect("mock upstream observation mutex should lock")
            .take()
        {
            let _ = sender.send(observation);
        }
    })
}

async fn receive_axum_live_json(socket: &mut WebSocket) -> serde_json::Value {
    let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv())
        .await
        .expect("mock upstream should receive a Live event before timeout")
        .expect("mock upstream socket should remain open")
        .expect("mock upstream Live frame should be readable");
    match message {
        AxumWsMessage::Text(text) => {
            serde_json::from_str(text.as_str()).expect("mock upstream Live event should be JSON")
        }
        other => panic!("mock upstream expected text Live event, got {other:?}"),
    }
}

async fn receive_codex_live_json(socket: &mut wreq::ws::WebSocket) -> serde_json::Value {
    let message = tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv())
        .await
        .expect("Codex Live gateway should send an event before timeout")
        .expect("Codex Live gateway socket should remain open")
        .expect("Codex Live gateway frame should be readable");
    match message {
        WreqWsMessage::Text(text) => serde_json::from_str(text.as_str())
            .expect("Codex Live gateway text event should be JSON"),
        other => panic!("Codex Live gateway expected text event, got {other:?}"),
    }
}

#[tokio::test]
async fn gateway_openai_models_list_drops_disabled_global_model_after_cache_invalidation() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-openai-models-cache")),
        unrestricted_models_snapshot("key-models-cache", "user-models-cache"),
    )]));
    let row = sample_models_candidate_row(
        "provider-openai-cache",
        "openai",
        "openai:chat",
        "gpt-5",
        10,
    );
    let global_model_id = row.global_model_id.clone();
    let candidate_repository = Arc::new(CachedToggleMinimalCandidateSelectionReadRepository::new(
        row.clone(),
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new()).with_admin_global_models(vec![
            StoredAdminGlobalModel::new(
                global_model_id.clone(),
                row.global_model_name.clone(),
                "GPT 5".to_string(),
                true,
                None,
                None,
                None,
                None,
                0,
                1,
                0,
                Some(1_711_000_000),
                Some(1_711_000_000),
            )
            .expect("global model should build"),
        ]),
    );
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_minimal_candidate_selection_and_auth_for_tests(
                candidate_repository.clone(),
                auth_repository,
            )
            .with_global_model_repository_for_tests(global_model_repository),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{gateway_url}/v1/models"))
        .header("authorization", "Bearer sk-openai-models-cache")
        .send()
        .await
        .expect("initial models request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["data"][0]["id"], "gpt-5");

    candidate_repository.set_active(false);
    let disabled_global_model = UpdateAdminGlobalModelRecord::new(
        global_model_id,
        "GPT 5".to_string(),
        false,
        None,
        None,
        None,
        None,
    )
    .expect("global model update record should build");
    state
        .update_admin_global_model(&disabled_global_model)
        .await
        .expect("global model update should succeed")
        .expect("global model should update");

    let response = client
        .get(format!("{gateway_url}/v1/models"))
        .header("authorization", "Bearer sk-openai-models-cache")
        .send()
        .await
        .expect("models request after disable should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["data"]
            .as_array()
            .expect("data should be an array")
            .len(),
        0
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_returns_empty_openai_models_when_candidate_rows_stall() {
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
        Some(hash_api_key("sk-openai-models-stalled")),
        unrestricted_models_snapshot("key-stalled", "user-stalled"),
    )]));
    let candidate_repository = Arc::new(PendingMinimalCandidateSelectionReadRepository);

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

    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("client should build")
        .get(format!("{gateway_url}/v1/models"))
        .header("authorization", "Bearer sk-openai-models-stalled")
        .send()
        .await
        .expect("request should return before client timeout");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["object"], "list");
    assert_eq!(
        payload["data"]
            .as_array()
            .expect("data should be an array")
            .len(),
        0
    );
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_returns_not_found_for_openai_model_detail_when_candidate_rows_stall() {
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
        Some(hash_api_key("sk-openai-model-detail-stalled")),
        unrestricted_models_snapshot("key-detail-stalled", "user-detail-stalled"),
    )]));
    let candidate_repository = Arc::new(PendingMinimalCandidateSelectionReadRepository);

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

    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("client should build")
        .get(format!("{gateway_url}/v1/models/gpt-stalled"))
        .header("authorization", "Bearer sk-openai-model-detail-stalled")
        .send()
        .await
        .expect("request should return before client timeout");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error"]["code"], "model_not_found");
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
                "claude:messages",
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
    assert_eq!(list_payload["data"][0]["owned_by"], "aether");

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
    assert_eq!(detail_payload["owned_by"], "aether");

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
                "claude:messages",
                "claude-3-7-sonnet",
                10,
            ),
            sample_models_candidate_row(
                "provider-claude",
                "claude",
                "claude:messages",
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
                "gemini:generate_content",
                "gemini-2.5-flash",
                10,
            ),
            sample_models_candidate_row(
                "provider-gemini",
                "gemini",
                "gemini:generate_content",
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
async fn gateway_handles_antigravity_v1internal_control_plane_without_proxying() {
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

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let user_settings = json!({
        "preferredModelId": "gemini-3.1-flash-lite",
        "theme": "dark"
    });
    let requests = vec![
        (
            "/v1internal:loadCodeAssist",
            json!({"metadata": {"ideType": "ANTIGRAVITY_CLI"}}),
        ),
        (
            "/v1internal:fetchAvailableModels",
            json!({"project": "aether-antigravity-local"}),
        ),
        (
            "/v1internal:retrieveUserQuotaSummary",
            json!({"project": "aether-antigravity-local"}),
        ),
        (
            "/v1internal:fetchUserInfo",
            json!({"project": "aether-antigravity-local"}),
        ),
        (
            "/v1internal:fetchAdminControls",
            json!({"project": "aether-antigravity-local"}),
        ),
        ("/v1internal:listExperiments", json!({})),
        (
            "/v1internal:recordCodeAssistMetrics",
            json!({
                "project": "aether-antigravity-local",
                "requestId": "opaque-request-id",
                "metrics": []
            }),
        ),
        (
            "/v1internal:writeTrajectoryAcls",
            json!({"trajectoryId": "trajectory-ant-123"}),
        ),
        (
            "/v1internal:setUserSettings",
            json!({"userSettings": user_settings.clone()}),
        ),
    ];

    for (path, request_body) in requests {
        let response = client
            .post(format!("{gateway_url}{path}"))
            .header("authorization", "Bearer ant-access-token")
            .header("user-agent", "antigravity/cli/1.0.2 linux/arm64")
            .json(&request_body)
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK, "path {path}");
        assert_eq!(
            response
                .headers()
                .get(EXECUTION_PATH_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(EXECUTION_PATH_LOCAL_AI_PUBLIC),
            "path {path}"
        );
        let payload: serde_json::Value = response.json().await.expect("json body should parse");

        match path {
            "/v1internal:loadCodeAssist" => {
                assert_eq!(
                    payload["cloudaicompanionProject"],
                    "aether-antigravity-local"
                );
                assert_eq!(payload["currentTier"]["id"], "free-tier");
                assert_eq!(payload["currentTier"]["name"], "Antigravity");
                assert_eq!(payload["paidTier"]["id"], "g1-pro-tier");
                assert_eq!(payload["gcpManaged"], false);
                assert_eq!(payload["allowedTiers"][0]["id"], "free-tier");
                assert_eq!(payload["allowedTiers"][0]["isDefault"], true);
                assert_eq!(payload["allowedTiers"][1]["id"], "standard-tier");
                assert_eq!(
                    payload["upgradeSubscriptionUri"],
                    "https://codeassist.google.com/upgrade"
                );
            }
            "/v1internal:fetchAvailableModels" => {
                assert_eq!(payload["defaultAgentModelId"], "gemini-3.5-flash-low");
                assert_eq!(
                    payload["tieredModelIds"]["flash"],
                    json!(["gemini-3-flash-agent"])
                );
                assert_eq!(
                    payload["tieredModelIds"]["pro"],
                    json!(["gemini-3.1-pro-low"])
                );
                assert_eq!(
                    payload["models"]["gemini-3-flash-agent"]["displayName"],
                    "Gemini 3.5 Flash (High)"
                );
                assert_eq!(
                    payload["models"]["gemini-3.5-flash-low"]["displayName"],
                    "Gemini 3.5 Flash (Medium)"
                );
                assert_eq!(
                    payload["models"]["gemini-3.5-flash-extra-low"]["displayName"],
                    "Gemini 3.5 Flash (Low)"
                );
                assert_eq!(
                    payload["models"]["gemini-pro-agent"]["displayName"],
                    "Gemini 3.1 Pro (High)"
                );
                assert_eq!(
                    payload["models"]["claude-opus-4-6-thinking"]["displayName"],
                    "Claude Opus 4.6 (Thinking)"
                );
                assert_eq!(
                    payload["models"]["gpt-oss-120b-medium"]["displayName"],
                    "GPT-OSS 120B (Medium)"
                );
                assert_eq!(
                    payload["models"]["gemini-pro-agent"]["model"],
                    "MODEL_PLACEHOLDER_M16"
                );
                assert_eq!(
                    payload["models"]["gemini-3.1-pro-high"]["model"],
                    "MODEL_PLACEHOLDER_M37"
                );
                assert_eq!(
                    payload["models"]["gemini-3.5-flash-extra-low"]["model"],
                    "MODEL_PLACEHOLDER_M187"
                );
                assert_eq!(
                    payload["models"]["claude-sonnet-4-6"]["apiProvider"],
                    "API_PROVIDER_ANTHROPIC_VERTEX"
                );
                assert_eq!(
                    payload["models"]["gpt-oss-120b-medium"]["apiProvider"],
                    "API_PROVIDER_OPENAI_VERTEX"
                );
                assert_eq!(
                    payload["models"]["gemini-3.5-flash-low"]["apiProvider"],
                    "API_PROVIDER_GOOGLE_GEMINI"
                );
                assert_eq!(
                    payload["models"]["gemini-2.5-flash-lite"]["model"],
                    "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE"
                );
                assert_eq!(
                    payload["agentModelSorts"][0]["groups"][0]["modelIds"],
                    json!([
                        "gemini-3.5-flash-low",
                        "gemini-3-flash-agent",
                        "gemini-3.5-flash-extra-low",
                        "gemini-3.1-pro-low",
                        "gemini-pro-agent",
                        "claude-sonnet-4-6",
                        "claude-opus-4-6-thinking",
                        "gpt-oss-120b-medium"
                    ])
                );
                assert_eq!(
                    payload["deprecatedModelIds"]["gemini-3.1-pro-high"]["newModelId"],
                    "gemini-pro-agent"
                );
                assert_eq!(payload["commandModelIds"], json!(["gemini-3-flash"]));
                assert_eq!(
                    payload["imageGenerationModelIds"],
                    json!(["gemini-3.1-flash-image"])
                );
                assert_eq!(payload["tabModelIds"], json!(["chat_20706", "chat_23310"]));
                assert_eq!(payload["mqueryModelIds"], json!(["gemini-3.1-flash-lite"]));
                assert_eq!(
                    payload["webSearchModelIds"],
                    json!(["gemini-3.1-flash-lite"])
                );
                assert_eq!(
                    payload["commitMessageModelIds"],
                    json!(["gemini-3.1-flash-lite"])
                );
            }
            "/v1internal:fetchUserInfo" => {
                assert_eq!(payload["regionCode"], "US");
                assert_eq!(
                    payload["userSettings"]["preferredModelId"],
                    "gemini-3.5-flash-low"
                );
            }
            "/v1internal:retrieveUserQuotaSummary" => {
                assert_eq!(payload["description"], "");
                assert_eq!(payload["groups"], json!([]));
            }
            "/v1internal:fetchAdminControls" => {
                assert_eq!(payload, json!({}));
            }
            "/v1internal:listExperiments" => {
                assert_eq!(payload["experimentIds"], json!([]));
                assert_eq!(payload["flags"], json!([]));
            }
            "/v1internal:recordCodeAssistMetrics" => {
                assert_eq!(payload, json!({}));
            }
            "/v1internal:writeTrajectoryAcls" => {
                assert_eq!(payload, json!({}));
            }
            "/v1internal:setUserSettings" => {
                assert_eq!(payload["userSettings"], user_settings);
            }
            other => panic!("unexpected path {other}"),
        }
    }

    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_does_not_locally_reject_image_model_name_on_chat_completions() {
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
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository)
                    .with_system_default_routing_group_for_tests(),
            ),
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

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS)
    );
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_image_request_above_gateway_limit_without_hitting_fallback_probe() {
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
                "model": "grok-imagine-image-lite",
                "prompt": "draw",
                "n": openai_image_gateway_max_generation_count() + 1,
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
    assert_eq!(
        payload["detail"],
        format!(
            "当前图片反代仅支持 n=1..{}",
            openai_image_gateway_max_generation_count()
        )
    );
    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_does_not_mount_image_variation_route_without_hitting_fallback_probe() {
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
        Some(hash_api_key("sk-openai-image-variation")),
        unrestricted_models_snapshot("key-openai-image-variation", "user-openai-image-variation"),
    )]));

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(auth_repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/images/variations"))
        .header("authorization", "Bearer sk-openai-image-variation")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(
            serde_json::to_vec(&json!({
                "model": "dall-e-2",
                "response_format": "url"
            }))
            .expect("request body should encode"),
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
