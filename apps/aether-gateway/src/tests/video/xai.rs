use super::*;
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};

fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "video-user".to_string(),
        Some("video@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(json!(["openai"])),
        Some(json!(["openai:video"])),
        Some(json!(["video-model"])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(json!(["openai"])),
        Some(json!(["openai:video"])),
        Some(json!(["video-model"])),
    )
    .expect("auth snapshot should build")
}

fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-openai-video-local-1".to_string(),
        provider_name: "openai".to_string(),
        provider_type: "xai".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: "endpoint-openai-video-local-1".to_string(),
        endpoint_api_format: "openai:video".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("video".to_string()),
        endpoint_is_active: true,
        key_id: "key-openai-video-local-1".to_string(),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:video".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(json!({"openai:video": 1})),
        model_id: "model-openai-video-local-1".to_string(),
        global_model_id: "global-model-openai-video-local-1".to_string(),
        global_model_name: "video-model".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(false),
        model_provider_model_name: "grok-imagine-video".to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: "grok-imagine-video".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:video".to_string()]),
            endpoint_ids: None,
            operations: None,
        }]),
        model_supports_streaming: Some(false),
        model_is_active: true,
        model_is_available: true,
    }
}

#[tokio::test]
async fn xai_video_native_and_compatibility_http_lifecycle() {
    Box::pin(assert_xai_video_http_lifecycle(Arc::new(
        InMemoryVideoTaskRepository::default(),
    )))
    .await;
}

#[tokio::test]
async fn xai_video_native_and_compatibility_http_lifecycle_postgres() {
    let configured_database_url = std::env::var("AETHER_TEST_DATABASE_URL").ok();
    let managed_database = if configured_database_url.is_none() {
        Some(
            aether_testkit::ManagedPostgresServer::start()
                .await
                .expect("temporary PostgreSQL should start"),
        )
    } else {
        None
    };
    let database_url = configured_database_url.unwrap_or_else(|| {
        managed_database
            .as_ref()
            .expect("managed test database should exist")
            .database_url()
            .to_string()
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("test database should connect");
    aether_data::driver::postgres::run_migrations(&pool)
        .await
        .expect("test database should migrate");
    // Preserve the production column constraints and unique indexes while isolating test rows.
    sqlx::query("CREATE TEMP TABLE video_tasks (LIKE public.video_tasks INCLUDING ALL)")
        .execute(&pool)
        .await
        .expect("isolated video task table should be created");
    let repository =
        Arc::new(aether_data::repository::video_tasks::SqlxVideoTaskRepository::new(pool.clone()));
    Box::pin(assert_xai_video_http_lifecycle(repository)).await;
    pool.close().await;
}

async fn assert_xai_video_http_lifecycle<T>(repository: Arc<T>)
where
    T: aether_data_contracts::repository::video_tasks::VideoTaskRepository + 'static,
{
    let static_dir = std::env::temp_dir().join(format!(
        "aether-xai-video-static-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&static_dir).unwrap();
    std::fs::write(
        static_dir.join("index.html"),
        "<html>Aether test frontend</html>",
    )
    .unwrap();
    let seen = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    // Exercise the real HTTP executor, including production method gates, instead of
    // the test execution-runtime override that used to hide rejected GET requests.
    let video_url = Arc::new(Mutex::new(String::new()));
    let runtime = Router::new()
        .route("/v1/videos/{operation}", any({
            let seen = seen.clone();
            let calls = calls.clone();
            let video_url = video_url.clone();
            move |request: Request| {
                let seen = seen.clone();
                let calls = calls.clone();
                let video_url = video_url.clone();
                async move {
                    let (parts, body) = request.into_parts();
                    assert_eq!(parts.headers["authorization"], "Bearer upstream-video-key");
                    let bytes = to_bytes(body, usize::MAX).await.unwrap();
                    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(json!(null));
                    seen.lock().unwrap().push(json!({
                        "method": parts.method.as_str(),
                        "url": parts.uri.path(),
                        "body": {"json_body": body}
                    }));
                    let response = if parts.method == http::Method::POST {
                        json!({"request_id":"upstream-video-id", "provider_extension":{"accepted":true}})
                    } else {
                        assert_eq!(parts.uri.path(), "/v1/videos/upstream-video-id");
                        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            json!({"status":"pending"})
                        } else {
                            json!({"status":"done", "model":"grok-imagine-video", "video":{"url":video_url.lock().unwrap().clone(), "duration":6, "respect_moderation":true}, "provider_extension":"preserved"})
                        }
                    };
                    Json(response)
                }
            }
        }))
        .route("/test.mp4", any(|request: Request| async move {
            assert!(request.headers().get("authorization").is_none());
            assert!(request.headers().get("x-xai-token-auth").is_none());
            ([("content-type", "video/mp4")], "test-video-bytes")
        }));
    let (runtime_url, runtime_handle) = start_server(runtime).await;
    let expected_video_url = format!("{runtime_url}/test.mp4");
    *video_url.lock().unwrap() = expected_video_url.clone();
    let state_factory = || {
        let auth = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![
            (
                Some(format!("{:x}", Sha256::digest(b"owner-key"))),
                sample_auth_snapshot("owner-api-key", "owner"),
            ),
            (
                Some(format!("{:x}", Sha256::digest(b"foreign-key"))),
                sample_auth_snapshot("foreign-api-key", "foreign"),
            ),
        ]));
        let candidates = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
        let catalog = video_provider_catalog_repository_with_proxy(
            "provider-openai-video-local-1",
            "xai",
            "endpoint-openai-video-local-1",
            "openai:video",
            "http://video-provider.invalid/v1",
            "key-openai-video-local-1",
            "upstream-video-key",
            Some(json!({"enabled":true,"node_id":"video-proxy"})),
        );
        AppState::new().expect("gateway should build").with_video_task_truth_source_mode(VideoTaskTruthSourceMode::RustAuthoritative).with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth, candidates, catalog, Arc::new(InMemoryRequestCandidateRepository::default()), DEVELOPMENT_ENCRYPTION_KEY
            ).attach_video_task_repository_for_tests(repository.clone())
             .attach_proxy_node_repository_for_tests(video_proxy_node_repository_at_url(["video-proxy"], &runtime_url))
        )
    };
    let router_factory =
        || crate::attach_static_frontend(build_router_with_state(state_factory()), &static_dir);
    let (gateway_url, gateway_handle) = start_server(router_factory()).await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(&gateway_url)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "<html>Aether test frontend</html>"
    );
    for (path, native) in [
        ("/v1/videos/generations", true),
        ("/v1/videos", true),
        ("/v1/videos/edits", true),
        ("/v1/videos/extensions", true),
        ("/openai/v1/videos", false),
    ] {
        calls.store(0, Ordering::SeqCst);
        let body = if native {
            json!({"model":"video-model","prompt":"A cat","duration":6,"aspect_ratio":"1:1","video":{"url":"https://example.com/input.mp4"},"future_option":true})
        } else {
            json!({"model":"video-model","prompt":"A cat","seconds":"6","size":"1280x720"})
        };
        let response = client
            .post(format!("{gateway_url}{path}"))
            .bearer_auth("owner-key")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let result: serde_json::Value = response.json().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{path}: {result}");
        let id = result[if native { "request_id" } else { "id" }]
            .as_str()
            .unwrap();
        assert_ne!(id, "upstream-video-id");
        if native {
            assert!(result.get("id").is_none());
            assert_eq!(result["provider_extension"]["accepted"], true);
        } else {
            assert_eq!(result["status"], "queued");
        }
        let request = seen.lock().unwrap().last().unwrap().clone();
        let suffix = if path.ends_with("/edits") {
            "edits"
        } else if path.ends_with("/extensions") {
            "extensions"
        } else {
            "generations"
        };
        assert_eq!(request["url"], format!("/v1/videos/{suffix}"));
        assert_eq!(request["body"]["json_body"]["model"], "grok-imagine-video");
        assert_eq!(request["body"]["json_body"]["duration"], 6);
        if native {
            assert_eq!(request["body"]["json_body"]["future_option"], true);
        } else {
            assert_eq!(request["body"]["json_body"]["aspect_ratio"], "16:9");
            assert_eq!(request["body"]["json_body"]["resolution"], "720p");
            assert!(request["body"]["json_body"].get("seconds").is_none());
            assert!(request["body"]["json_body"].get("size").is_none());
        }
        let query = format!(
            "{gateway_url}{}/{id}",
            if native {
                "/v1/videos"
            } else {
                "/openai/v1/videos"
            }
        );
        let before = seen.lock().unwrap().len();
        let denied = client
            .get(&query)
            .bearer_auth("foreign-key")
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
        assert_eq!(seen.lock().unwrap().len(), before);
        let denied_content = client
            .get(format!("{gateway_url}/openai/v1/videos/{id}/content"))
            .bearer_auth("foreign-key")
            .send()
            .await
            .unwrap();
        assert_eq!(denied_content.status(), StatusCode::NOT_FOUND);
        assert_eq!(seen.lock().unwrap().len(), before);
        let pending: serde_json::Value = client
            .get(&query)
            .bearer_auth("owner-key")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            pending["status"],
            if native { "pending" } else { "queued" },
            "{path}: {pending}"
        );
        let done: serde_json::Value = client
            .get(&query)
            .bearer_auth("owner-key")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(done["status"], if native { "done" } else { "completed" });
        if native {
            assert_eq!(done["video"]["respect_moderation"], true);
            assert_eq!(done["provider_extension"], "preserved");
        } else {
            assert_eq!(done["video_url"], expected_video_url);
        }
        let stored = repository
            .find(VideoTaskLookupKey::Id(id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.client_api_format.as_deref(),
            Some(if native { "xai:video" } else { "openai:video" })
        );
        assert_eq!(
            stored.external_task_id.as_deref(),
            Some("upstream-video-id")
        );
        assert!(stored.request_metadata.is_none());
        assert!(stored.original_request_body.is_none());
        // A new gateway instance must reconstruct the pinned provider/credential and protocol.
        let (restart_url, restart_handle) = start_server(router_factory()).await;
        let restored: serde_json::Value = client
            .get(format!(
                "{restart_url}{}/{id}",
                if native {
                    "/v1/videos"
                } else {
                    "/openai/v1/videos"
                }
            ))
            .bearer_auth("owner-key")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(restored["status"], done["status"]);
        if native {
            assert_eq!(restored["video"]["respect_moderation"], true);
        }
        let compat: serde_json::Value = client
            .get(format!("{restart_url}/openai/v1/videos/{id}"))
            .bearer_auth("owner-key")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(compat["status"], "completed");
        assert_eq!(compat["video_url"], expected_video_url);
        let native_view: serde_json::Value = client
            .get(format!("{restart_url}/v1/videos/{id}"))
            .bearer_auth("owner-key")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(native_view["status"], "done");
        assert_eq!(native_view["video"]["respect_moderation"], true);
        for prefix in ["/v1/videos", "/openai/v1/videos"] {
            let content = client
                .get(format!("{restart_url}{prefix}/{id}/content"))
                .bearer_auth("owner-key")
                .send()
                .await
                .unwrap();
            assert_eq!(content.status(), StatusCode::OK);
            assert_eq!(content.headers()["content-type"], "video/mp4");
            assert_eq!(content.bytes().await.unwrap(), "test-video-bytes");
        }
        restart_handle.abort();
    }
    let before = seen.lock().unwrap().len();
    let bad = client
        .post(format!("{gateway_url}/openai/v1/videos"))
        .bearer_auth("owner-key")
        .json(&json!({"model":"video-model","prompt":"cat","seconds":"wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(seen.lock().unwrap().len(), before);
    gateway_handle.abort();
    runtime_handle.abort();
    std::fs::remove_dir_all(&static_dir).unwrap();
}
