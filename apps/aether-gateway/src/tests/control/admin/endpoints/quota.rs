use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::proxy_nodes::InMemoryProxyNodeRepository;
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use axum::body::{to_bytes, Body};
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::StatusCode;
use serde_json::json;

use super::super::super::{
    build_router_with_state, build_state_with_execution_runtime_override, sample_endpoint,
    sample_key, sample_proxy_node, start_server,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

#[tokio::test]
async fn gateway_refreshes_admin_provider_quota_locally_for_codex_with_trusted_admin_principal() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeRequest {
        url: String,
        authorization: String,
        provider_api_format: String,
        total_ms: Option<u64>,
    }

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-codex/refresh-quota",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                    &to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read"),
                )
                .expect("plan should parse");
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeRequest {
                    url: plan.url.clone(),
                    authorization: plan
                        .headers
                        .get("authorization")
                        .cloned()
                        .unwrap_or_default(),
                    provider_api_format: plan.provider_api_format.clone(),
                    total_ms: plan
                        .timeouts
                        .as_ref()
                        .and_then(|timeouts| timeouts.total_ms),
                });
                let result = aether_contracts::ExecutionResult {
                    request_id: plan.request_id,
                    candidate_id: None,
                    status_code: 200,
                    headers: BTreeMap::from([
                        (
                            "x-codex-primary-reset-after-seconds".to_string(),
                            "18000".to_string(),
                        ),
                        (
                            "x-codex-primary-reset-at".to_string(),
                            "1900000000".to_string(),
                        ),
                        (
                            "x-codex-secondary-reset-after-seconds".to_string(),
                            "604800".to_string(),
                        ),
                        (
                            "x-codex-secondary-reset-at".to_string(),
                            "1900500000".to_string(),
                        ),
                    ]),
                    body: Some(aether_contracts::ResponseBody {
                        json_body: Some(json!({
                            "plan_type": "plus",
                            "rate_limit": {
                                "primary_window": {
                                    "used_percent": 12.5,
                                    "window_minutes": 300
                                },
                                "secondary_window": {
                                    "used_percent": 55.0,
                                    "window_minutes": 10080
                                }
                            },
                            "credits": {
                                "has_credits": true,
                                "balance": 42.0,
                                "unlimited": false
                            }
                        })),
                        body_bytes_b64: None,
                    }),
                    telemetry: None,
                    error: None,
                };
                (StatusCode::OK, Json(result))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-codex".to_string(),
            "codex".to_string(),
            Some("https://example.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")],
        vec![sample_endpoint(
            "endpoint-codex-cli",
            "provider-codex",
            "openai:responses",
            "https://chatgpt.com/backend-api",
        )],
        vec![sample_key(
            "key-codex-a",
            "provider-codex",
            "openai:responses",
            "sk-codex-123",
        )],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url.clone())
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-codex/refresh-quota"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 0);
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["results"][0]["status"], "success");
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["provider_type"],
        "codex"
    );
    assert_eq!(payload["results"][0]["quota_snapshot"]["plan_type"], "plus");
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["reset_at"],
        1_900_000_000u64
    );
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["credits"]["balance"],
        json!(42.0)
    );
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["windows"]
            .as_array()
            .map(Vec::len),
        Some(2usize)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime request should be captured");
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://chatgpt.com/backend-api/wham/usage"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-codex-123"
    );
    assert_eq!(
        seen_execution_runtime_request.provider_api_format,
        "openai:responses"
    );
    assert_eq!(seen_execution_runtime_request.total_ms, Some(30_000));

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].oauth_invalid_reason, None);
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("codex"))
            .and_then(|value| value.get("plan_type")),
        Some(&json!("plus"))
    );
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("codex"))
            .and_then(|value| value.get("primary_used_percent")),
        Some(&json!(55.0))
    );
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("codex"))
            .and_then(|value| value.get("primary_reset_at")),
        Some(&json!(1_900_500_000u64))
    );
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("codex"))
            .and_then(|value| value.get("secondary_used_percent")),
        Some(&json!(12.5))
    );
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("codex"))
            .and_then(|value| value.get("secondary_reset_at")),
        Some(&json!(1_900_000_000u64))
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_refreshes_admin_provider_quota_for_codex_proxy_with_extended_timeout() {
    let upstream =
        Router::new().route(
            "/api/admin/endpoints/providers/provider-codex/refresh-quota",
            any(|_request: Request| async move {
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }),
        );

    let seen_execution_runtime = Arc::new(Mutex::new(None::<aether_contracts::ExecutionPlan>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                    &to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read"),
                )
                .expect("plan should parse");
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(plan.clone());
                let result = aether_contracts::ExecutionResult {
                    request_id: plan.request_id,
                    candidate_id: None,
                    status_code: 200,
                    headers: BTreeMap::new(),
                    body: Some(aether_contracts::ResponseBody {
                        json_body: Some(json!({
                            "plan_type": "plus",
                            "rate_limit": {
                                "primary_window": {
                                    "used_percent": 12.5,
                                    "reset_after_seconds": 18000,
                                    "reset_at": 1_900_000_000u64,
                                    "window_minutes": 300
                                }
                            }
                        })),
                        body_bytes_b64: None,
                    }),
                    telemetry: None,
                    error: None,
                };
                (StatusCode::OK, Json(result))
            }
        }),
    );

    let mut provider = StoredProviderCatalogProvider::new(
        "provider-codex".to_string(),
        "codex".to_string(),
        Some("https://example.com".to_string()),
        "codex".to_string(),
    )
    .expect("provider should build");
    provider.proxy = Some(json!({
        "node_id": "proxy-node-codex-quota",
        "enabled": true
    }));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-codex-cli",
            "provider-codex",
            "openai:responses",
            "https://chatgpt.com/backend-api",
        )],
        vec![sample_key(
            "key-codex-a",
            "provider-codex",
            "openai:responses",
            "sk-codex-123",
        )],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-codex-quota");
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url.clone())
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-codex/refresh-quota"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let plan = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime request should be captured");
    assert_eq!(
        plan.proxy
            .as_ref()
            .and_then(|proxy| proxy.node_id.as_deref()),
        Some("proxy-node-codex-quota")
    );
    let timeouts = plan.timeouts.expect("timeouts should exist");
    assert_eq!(timeouts.connect_ms, Some(60_000));
    assert_eq!(timeouts.read_ms, Some(60_000));
    assert_eq!(timeouts.write_ms, Some(60_000));
    assert_eq!(timeouts.pool_ms, Some(60_000));
    assert_eq!(timeouts.total_ms, Some(60_000));

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_refreshes_admin_provider_quota_locally_for_kiro_with_trusted_admin_principal() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeRequest {
        url: String,
        authorization: String,
        provider_api_format: String,
    }

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-kiro/refresh-quota",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                    &to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read"),
                )
                .expect("plan should parse");
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeRequest {
                    url: plan.url.clone(),
                    authorization: plan
                        .headers
                        .get("authorization")
                        .cloned()
                        .unwrap_or_default(),
                    provider_api_format: plan.provider_api_format.clone(),
                });
                let result = aether_contracts::ExecutionResult {
                    request_id: plan.request_id,
                    candidate_id: None,
                    status_code: 200,
                    headers: BTreeMap::new(),
                    body: Some(aether_contracts::ResponseBody {
                        json_body: Some(json!({
                            "subscriptionInfo": {
                                "subscriptionTitle": "KIRO PRO+"
                            },
                            "usageBreakdownList": [{
                                "currentUsageWithPrecision": 5.0,
                                "usageLimitWithPrecision": 20.0,
                                "nextDateReset": 1_900_000_000u64
                            }],
                            "desktopUserInfo": {
                                "email": "dev@example.com"
                            }
                        })),
                        body_bytes_b64: None,
                    }),
                    telemetry: None,
                    error: None,
                };
                (StatusCode::OK, Json(result))
            }
        }),
    );

    let encrypted_auth_config = encrypt_python_fernet_plaintext(
        DEVELOPMENT_ENCRYPTION_KEY,
        r#"{
            "access_token":"kiro-access-token",
            "api_region":"us-west-2",
            "machine_id":"123e4567-e89b-12d3-a456-426614174000",
            "kiro_version":"1.2.3"
        }"#,
    )
    .expect("auth config ciphertext should build");
    let encrypted_api_key =
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
            .expect("api key ciphertext should build");
    let key = StoredProviderCatalogKey::new(
        "key-kiro-a".to_string(),
        "provider-kiro".to_string(),
        "default".to_string(),
        "bearer".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!(["claude:messages"])),
        encrypted_api_key,
        Some(encrypted_auth_config),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build");

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-kiro".to_string(),
            "kiro".to_string(),
            Some("https://example.com".to_string()),
            "kiro".to_string(),
        )
        .expect("provider should build")],
        vec![sample_endpoint(
            "endpoint-kiro-cli",
            "provider-kiro",
            "claude:messages",
            "https://q.us-west-2.amazonaws.com",
        )],
        vec![key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url.clone())
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-kiro/refresh-quota"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 0);
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["results"][0]["status"], "success");
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["provider_type"],
        "kiro"
    );
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["plan_type"],
        "KIRO PRO+"
    );
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["windows"][0]["remaining_value"],
        json!(15.0)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime request should be captured");
    assert!(seen_execution_runtime_request
        .url
        .starts_with("https://q.us-west-2.amazonaws.com/getUsageLimits?"),);
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer kiro-access-token"
    );
    assert_eq!(
        seen_execution_runtime_request.provider_api_format,
        "kiro:usage"
    );

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-kiro-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].oauth_invalid_reason, None);
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("kiro"))
            .and_then(|value| value.get("subscription_title")),
        Some(&json!("KIRO PRO+"))
    );
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("kiro"))
            .and_then(|value| value.get("remaining")),
        Some(&json!(15.0))
    );
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("kiro"))
            .and_then(|value| value.get("email")),
        Some(&json!("dev@example.com"))
    );
    assert_eq!(
        reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(|value| value.get("quota"))
            .and_then(|value| value.get("provider_type")),
        Some(&json!("kiro"))
    );
    assert_eq!(
        reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(|value| value.get("quota"))
            .and_then(|value| value.get("usage_ratio")),
        Some(&json!(0.25))
    );
    assert_eq!(
        reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(|value| value.get("quota"))
            .and_then(|value| value.get("plan_type")),
        Some(&json!("KIRO PRO+"))
    );
    assert_eq!(
        reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(|value| value.get("quota"))
            .and_then(|value| value.get("windows"))
            .and_then(|value| value.get(0))
            .and_then(|value| value.get("remaining_value")),
        Some(&json!(15.0))
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_reports_codex_quota_runtime_failures_locally_without_falling_back_to_admin_passthrough(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-codex/refresh-quota",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| async move {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Body::from("runtime unavailable"),
            )
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-codex".to_string(),
            "codex".to_string(),
            Some("https://example.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")],
        vec![sample_endpoint(
            "endpoint-codex-cli",
            "provider-codex",
            "openai:responses",
            "https://chatgpt.com/backend-api",
        )],
        vec![sample_key(
            "key-codex-a",
            "provider-codex",
            "openai:responses",
            "sk-codex-123",
        )],
    ));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url.clone())
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-codex/refresh-quota"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], 0);
    assert_eq!(payload["failed"], 1);
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["results"][0]["status"], "error");
    assert_eq!(payload["results"][0]["status_code"], 502);
    assert!(payload["results"][0]["message"]
        .as_str()
        .expect("message should be string")
        .contains("wham/usage 请求执行失败: execution runtime returned HTTP 500"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].oauth_invalid_reason, None);
    assert_eq!(reloaded[0].upstream_metadata, None);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_refreshes_admin_provider_quota_locally_for_antigravity_with_trusted_admin_principal(
) {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeRequest {
        url: String,
        authorization: String,
        provider_api_format: String,
        request_body: Option<serde_json::Value>,
    }

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-antigravity/refresh-quota",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                    &to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read"),
                )
                .expect("plan should parse");
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeRequest {
                    url: plan.url.clone(),
                    authorization: plan
                        .headers
                        .get("authorization")
                        .cloned()
                        .unwrap_or_default(),
                    provider_api_format: plan.provider_api_format.clone(),
                    request_body: plan.body.json_body.clone(),
                });
                let result = aether_contracts::ExecutionResult {
                    request_id: plan.request_id,
                    candidate_id: None,
                    status_code: 200,
                    headers: BTreeMap::new(),
                    body: Some(aether_contracts::ResponseBody {
                        json_body: Some(json!({
                            "models": {
                                "claude-sonnet-4": {
                                    "displayName": "Claude Sonnet 4",
                                    "quotaInfo": {
                                        "remainingFraction": 0.25,
                                        "resetTime": "2026-03-27T00:00:00Z"
                                    }
                                },
                                "gemini-2.5-pro": {
                                    "displayName": "Gemini 2.5 Pro"
                                }
                            }
                        })),
                        body_bytes_b64: None,
                    }),
                    telemetry: None,
                    error: None,
                };
                (StatusCode::OK, Json(result))
            }
        }),
    );

    let encrypted_auth_config = encrypt_python_fernet_plaintext(
        DEVELOPMENT_ENCRYPTION_KEY,
        r#"{
            "project_id":"project-ant-123",
            "client_version":"1.18.4",
            "session_id":"session-ant-1"
        }"#,
    )
    .expect("auth config ciphertext should build");
    let key = StoredProviderCatalogKey::new(
        "key-antigravity-a".to_string(),
        "provider-antigravity".to_string(),
        "default".to_string(),
        "oauth".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!(["gemini:generate_content"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "ya29.ant-token")
            .expect("api key ciphertext should build"),
        Some(encrypted_auth_config),
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build");

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-antigravity".to_string(),
            "antigravity".to_string(),
            Some("https://example.com".to_string()),
            "antigravity".to_string(),
        )
        .expect("provider should build")],
        vec![sample_endpoint(
            "endpoint-antigravity-chat",
            "provider-antigravity",
            "gemini:generate_content",
            "https://daily-cloudcode-pa.googleapis.com",
        )],
        vec![key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url.clone())
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-antigravity/refresh-quota"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 0);
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["results"][0]["status"], "success");
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["provider_type"],
        "antigravity"
    );
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["usage_ratio"],
        json!(0.75)
    );
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["windows"]
            .as_array()
            .map(Vec::len),
        Some(1usize)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime request should be captured");
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer ya29.ant-token"
    );
    assert_eq!(
        seen_execution_runtime_request.provider_api_format,
        "antigravity:fetch_available_models"
    );
    assert_eq!(
        seen_execution_runtime_request.request_body,
        Some(json!({ "project": "project-ant-123" }))
    );

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-antigravity-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].oauth_invalid_reason, None);
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("antigravity"))
            .and_then(|value| value.get("models"))
            .and_then(|value| value.get("claude-sonnet-4"))
            .and_then(|value| value.get("remaining_fraction")),
        Some(&json!(0.25))
    );
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("antigravity"))
            .and_then(|value| value.get("models"))
            .and_then(|value| value.get("claude-sonnet-4"))
            .and_then(|value| value.get("used_percent")),
        Some(&json!(75.0))
    );
    assert_eq!(
        reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(|value| value.get("quota"))
            .and_then(|value| value.get("provider_type")),
        Some(&json!("antigravity"))
    );
    assert_eq!(
        reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(|value| value.get("quota"))
            .and_then(|value| value.get("usage_ratio")),
        Some(&json!(0.75))
    );
    assert_eq!(
        reloaded[0]
            .status_snapshot
            .as_ref()
            .and_then(|value| value.get("quota"))
            .and_then(|value| value.get("windows"))
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(1usize)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
