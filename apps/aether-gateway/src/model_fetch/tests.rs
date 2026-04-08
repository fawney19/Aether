use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_contracts::ExecutionPlan;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::global_models::{
    AdminProviderModelListQuery, GlobalModelReadRepository, StoredAdminGlobalModel,
    StoredAdminProviderModel,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
    StoredProviderCatalogProvider,
};
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use serde_json::json;

use super::{perform_model_fetch_once, ModelFetchRunSummary};
use crate::AppState;

async fn start_server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let addr = listener.local_addr().expect("local addr should resolve");
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("server should run");
    });
    (format!("http://{addr}"), handle)
}

fn build_state_with_execution_runtime_override(
    execution_runtime_override_base_url: impl Into<String>,
) -> AppState {
    AppState::new()
        .expect("gateway should build")
        .with_execution_runtime_override_base_url(execution_runtime_override_base_url)
}

fn sample_provider(provider_id: &str) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        provider_id.to_string(),
        "openai".to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(true, false, true, None, None, None, None, None, None)
}

fn sample_endpoint(
    provider_id: &str,
    endpoint_id: &str,
    base_url: &str,
) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        endpoint_id.to_string(),
        provider_id.to_string(),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        base_url.to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("endpoint transport should build")
}

fn sample_key(provider_id: &str, key_id: &str) -> StoredProviderCatalogKey {
    let mut key = StoredProviderCatalogKey::new(
        key_id.to_string(),
        provider_id.to_string(),
        "primary".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!(["openai:chat"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "live-secret-api-key")
            .expect("api key should encrypt"),
        None,
        None,
        None,
        Some(json!(["gpt-4.1"])),
        None,
        None,
        None,
    )
    .expect("key transport should build");
    key.auto_fetch_models = true;
    key.locked_models = Some(json!(["locked-model"]));
    key.model_include_patterns = Some(json!(["gpt-*"]));
    key.model_exclude_patterns = Some(json!(["gpt-beta"]));
    key
}

fn sample_global_model(id: &str, name: &str, mappings: &[&str]) -> StoredAdminGlobalModel {
    StoredAdminGlobalModel::new(
        id.to_string(),
        name.to_string(),
        name.to_string(),
        true,
        None,
        None,
        None,
        Some(json!({ "model_mappings": mappings })),
        0,
        0,
        0,
        Some(1_711_000_000),
        Some(1_711_000_000),
    )
    .expect("global model should build")
}

fn sample_provider_model(
    id: &str,
    provider_id: &str,
    global_model_id: &str,
    provider_model_name: &str,
    global_model_name: &str,
    mappings: &[&str],
) -> StoredAdminProviderModel {
    StoredAdminProviderModel::new(
        id.to_string(),
        provider_id.to_string(),
        global_model_id.to_string(),
        provider_model_name.to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        true,
        true,
        None,
        Some(1_711_000_000),
        Some(1_711_000_000),
        Some(global_model_name.to_string()),
        Some(global_model_name.to_string()),
        None,
        None,
        Some(json!({ "model_mappings": mappings })),
    )
    .expect("provider model should build")
}

struct TestEnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl Drop for TestEnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.as_deref() {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn set_test_env_var(key: &'static str, value: &str) -> TestEnvVarGuard {
    let previous = std::env::var(key).ok();
    std::env::set_var(key, value);
    TestEnvVarGuard { key, previous }
}

#[tokio::test]
async fn gateway_model_fetch_updates_key_and_syncs_provider_model_whitelist_associations() {
    let seen_execution_runtime_plan = Arc::new(Mutex::new(None::<ExecutionPlan>));
    let seen_execution_runtime_plan_clone = Arc::clone(&seen_execution_runtime_plan);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let seen_execution_runtime_plan = Arc::clone(&seen_execution_runtime_plan_clone);
            async move {
                *seen_execution_runtime_plan
                    .lock()
                    .expect("mutex should lock") = Some(plan);
                Json(json!({
                    "request_id": "req-model-fetch-key-1",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "data": [
                                {"id": "gpt-5"},
                                {"id": "gpt-beta"},
                                {"id": "other-model"}
                            ]
                        }
                    }
                }))
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai")],
        vec![sample_endpoint(
            "provider-openai",
            "endpoint-openai",
            "https://api.openai.example",
        )],
        vec![sample_key("provider-openai", "key-openai")],
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![
                sample_global_model("global-model-gpt5", "gpt-5", &["gpt-5"]),
                sample_global_model("global-model-gpt4", "gpt-4.1", &["gpt-4\\.1"]),
            ])
            .with_admin_provider_models(vec![sample_provider_model(
                "provider-model-gpt4",
                "provider-openai",
                "global-model-gpt4",
                "gpt-4.1",
                "gpt-4.1",
                &["gpt-4\\.1"],
            )]),
    );
    let data_state = crate::data::GatewayDataState::disabled()
        .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository))
        .with_global_model_repository_for_tests(Arc::clone(&global_model_repository))
        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
    let state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(data_state);

    let summary = perform_model_fetch_once(&state)
        .await
        .expect("model fetch should succeed");
    assert_eq!(
        summary,
        ModelFetchRunSummary {
            attempted: 1,
            succeeded: 1,
            failed: 0,
            skipped: 0,
        }
    );

    let seen_plan = seen_execution_runtime_plan
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime plan should be captured");
    assert_eq!(seen_plan.method, "GET");
    assert_eq!(seen_plan.url, "https://api.openai.example/v1/models");
    assert_eq!(
        seen_plan.headers.get("authorization").map(String::as_str),
        Some("Bearer live-secret-api-key")
    );

    let updated_key = provider_catalog_repository
        .list_keys_by_ids(&["key-openai".to_string()])
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("updated key should exist");
    assert_eq!(
        updated_key.allowed_models,
        Some(json!(["gpt-5", "locked-model"]))
    );
    assert_eq!(updated_key.last_models_fetch_error, None);
    assert!(updated_key.last_models_fetch_at_unix_secs.is_some());

    let provider_models = global_model_repository
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: "provider-openai".to_string(),
            is_active: None,
            offset: 0,
            limit: 10_000,
        })
        .await
        .expect("provider models should load");
    assert_eq!(provider_models.len(), 1);
    assert_eq!(
        provider_models[0].global_model_name.as_deref(),
        Some("gpt-5")
    );
    assert_eq!(provider_models[0].provider_model_name, "gpt-5");

    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_model_fetch_updates_key_and_syncs_provider_model_whitelist_associations_without_execution_runtime_override(
) {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SeenUpstreamRequest {
        method: String,
        authorization: String,
    }

    let seen_upstream = Arc::new(Mutex::new(None::<SeenUpstreamRequest>));
    let seen_upstream_clone = Arc::clone(&seen_upstream);
    let upstream = Router::new().route(
        "/v1/models",
        any(move |request: Request| {
            let seen_upstream_inner = Arc::clone(&seen_upstream_clone);
            async move {
                let authorization = request
                    .headers()
                    .get(http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                *seen_upstream_inner.lock().expect("mutex should lock") =
                    Some(SeenUpstreamRequest {
                        method: request.method().as_str().to_string(),
                        authorization,
                    });
                Json(json!({
                    "data": [
                        {"id": "gpt-5"},
                        {"id": "gpt-beta"},
                        {"id": "other-model"}
                    ]
                }))
            }
        }),
    );
    let (upstream_url, upstream_handle) = start_server(upstream).await;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai")],
        vec![sample_endpoint(
            "provider-openai",
            "endpoint-openai",
            &upstream_url,
        )],
        vec![sample_key("provider-openai", "key-openai")],
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![
                sample_global_model("global-model-gpt5", "gpt-5", &["gpt-5"]),
                sample_global_model("global-model-gpt4", "gpt-4.1", &["gpt-4\\.1"]),
            ])
            .with_admin_provider_models(vec![sample_provider_model(
                "provider-model-gpt4",
                "provider-openai",
                "global-model-gpt4",
                "gpt-4.1",
                "gpt-4.1",
                &["gpt-4\\.1"],
            )]),
    );
    let data_state = crate::data::GatewayDataState::disabled()
        .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository))
        .with_global_model_repository_for_tests(Arc::clone(&global_model_repository))
        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(data_state);

    let summary = perform_model_fetch_once(&state)
        .await
        .expect("model fetch should succeed");
    assert_eq!(
        summary,
        ModelFetchRunSummary {
            attempted: 1,
            succeeded: 1,
            failed: 0,
            skipped: 0,
        }
    );

    assert_eq!(
        seen_upstream.lock().expect("mutex should lock").clone(),
        Some(SeenUpstreamRequest {
            method: "GET".to_string(),
            authorization: "Bearer live-secret-api-key".to_string(),
        })
    );

    let updated_key = provider_catalog_repository
        .list_keys_by_ids(&["key-openai".to_string()])
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("updated key should exist");
    assert_eq!(
        updated_key.allowed_models,
        Some(json!(["gpt-5", "locked-model"]))
    );
    assert_eq!(updated_key.last_models_fetch_error, None);
    assert!(updated_key.last_models_fetch_at_unix_secs.is_some());

    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_background_model_fetch_updates_key_and_syncs_provider_model_whitelist_associations(
) {
    let _startup_enabled = set_test_env_var("MODEL_FETCH_STARTUP_ENABLED", "true");
    let _startup_delay = set_test_env_var("MODEL_FETCH_STARTUP_DELAY_SECONDS", "0");

    let seen_execution_runtime_plan = Arc::new(Mutex::new(None::<ExecutionPlan>));
    let seen_execution_runtime_plan_clone = Arc::clone(&seen_execution_runtime_plan);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let seen_execution_runtime_plan = Arc::clone(&seen_execution_runtime_plan_clone);
            async move {
                *seen_execution_runtime_plan
                    .lock()
                    .expect("mutex should lock") = Some(plan);
                Json(json!({
                    "request_id": "req-model-fetch-key-1",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "data": [
                                {"id": "gpt-5"},
                                {"id": "gpt-beta"},
                                {"id": "other-model"}
                            ]
                        }
                    }
                }))
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai")],
        vec![sample_endpoint(
            "provider-openai",
            "endpoint-openai",
            "https://api.openai.example",
        )],
        vec![sample_key("provider-openai", "key-openai")],
    ));
    let global_model_repository = Arc::new(
        InMemoryGlobalModelReadRepository::seed(Vec::new())
            .with_admin_global_models(vec![
                sample_global_model("global-model-gpt5", "gpt-5", &["gpt-5"]),
                sample_global_model("global-model-gpt4", "gpt-4.1", &["gpt-4\\.1"]),
            ])
            .with_admin_provider_models(vec![sample_provider_model(
                "provider-model-gpt4",
                "provider-openai",
                "global-model-gpt4",
                "gpt-4.1",
                "gpt-4.1",
                &["gpt-4\\.1"],
            )]),
    );
    let data_state = crate::data::GatewayDataState::disabled()
        .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository))
        .with_global_model_repository_for_tests(Arc::clone(&global_model_repository))
        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(data_state);

    let background_tasks = gateway_state.spawn_background_tasks();
    assert!(
        !background_tasks.is_empty(),
        "model fetch worker should spawn"
    );

    let updated_key = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let updated_key = provider_catalog_repository
                .list_keys_by_ids(&["key-openai".to_string()])
                .await
                .expect("keys should load")
                .into_iter()
                .next()
                .expect("updated key should exist");
            if updated_key.last_models_fetch_at_unix_secs.is_some() {
                break updated_key;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background worker should fetch models on startup");

    assert_eq!(
        updated_key.allowed_models,
        Some(json!(["gpt-5", "locked-model"]))
    );
    assert_eq!(updated_key.last_models_fetch_error, None);

    let provider_models = global_model_repository
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: "provider-openai".to_string(),
            is_active: None,
            offset: 0,
            limit: 10_000,
        })
        .await
        .expect("provider models should load");
    assert_eq!(provider_models.len(), 1);
    assert_eq!(
        provider_models[0].global_model_name.as_deref(),
        Some("gpt-5")
    );

    let seen_plan = seen_execution_runtime_plan
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime plan should be captured");
    assert_eq!(seen_plan.url, "https://api.openai.example/v1/models");

    for handle in background_tasks {
        handle.abort();
    }
    execution_runtime_handle.abort();
}
