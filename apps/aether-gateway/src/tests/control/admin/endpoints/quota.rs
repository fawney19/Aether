use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aether_crypto::{
    decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::proxy_nodes::InMemoryProxyNodeRepository;
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use axum::body::{to_bytes, Body};
use axum::routing::{any, post};
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

const PROVIDER_QUOTA_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_provider_quota_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(PROVIDER_QUOTA_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("provider quota test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

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

    let seen_execution_runtime = Arc::new(Mutex::new(Vec::<SeenExecutionRuntimeRequest>::new()));
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
                seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(SeenExecutionRuntimeRequest {
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
                    response_observation: None,
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

    let seen_execution_runtime_requests = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone();
    assert_eq!(seen_execution_runtime_requests.len(), 2);
    assert_eq!(
        seen_execution_runtime_requests[0].url,
        "https://chatgpt.com/backend-api/wham/usage"
    );
    assert_eq!(
        seen_execution_runtime_requests[1].url,
        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits"
    );
    for request in seen_execution_runtime_requests {
        assert_eq!(request.authorization, "Bearer sk-codex-123");
        assert_eq!(request.provider_api_format, "openai:responses");
        assert_eq!(request.total_ms, Some(30_000));
    }

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

#[test]
fn gateway_codex_quota_refresh_persists_after_automatic_oauth_token_refresh() {
    run_provider_quota_test(
        "gateway_codex_quota_refresh_persists_after_automatic_oauth_token_refresh",
        gateway_codex_quota_refresh_persists_after_automatic_oauth_token_refresh_impl,
    );
}

async fn gateway_codex_quota_refresh_persists_after_automatic_oauth_token_refresh_impl() {
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_server = Router::new().route(
        "/oauth/token",
        post({
            let token_hits = Arc::clone(&token_hits);
            move || {
                let token_hits = Arc::clone(&token_hits);
                async move {
                    *token_hits.lock().expect("mutex should lock") += 1;
                    Json(json!({
                        "access_token": "refreshed-codex-access-token",
                        "refresh_token": "rotated-codex-refresh-token",
                        "token_type": "Bearer",
                        "expires_in": 3_600,
                        "account_id": "acct-quota-refresh",
                        "plan_type": "plus"
                    }))
                }
            }
        }),
    );

    let seen_requests = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any({
            let seen_requests = Arc::clone(&seen_requests);
            move |request: Request| {
                let seen_requests = Arc::clone(&seen_requests);
                async move {
                    let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                        &to_bytes(request.into_body(), usize::MAX)
                            .await
                            .expect("body should read"),
                    )
                    .expect("plan should parse");
                    seen_requests.lock().expect("mutex should lock").push((
                        plan.url.clone(),
                        plan.headers
                            .get("authorization")
                            .cloned()
                            .unwrap_or_default(),
                    ));
                    let body_json = match plan.url.as_str() {
                        "https://chatgpt.com/backend-api/wham/usage" => json!({
                            "plan_type": "plus",
                            "rate_limit": {
                                "primary_window": {
                                    "used_percent": 23.0,
                                    "reset_at": 1_900_000_000u64,
                                    "window_minutes": 300
                                }
                            }
                        }),
                        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits" => {
                            json!({"available_count": 0, "credits": []})
                        }
                        url => panic!("unexpected execution runtime URL: {url}"),
                    };
                    let result = aether_contracts::ExecutionResult {
                        request_id: plan.request_id,
                        candidate_id: None,
                        status_code: 200,
                        headers: BTreeMap::new(),
                        response_observation: None,
                        body: Some(aether_contracts::ResponseBody {
                            json_body: Some(body_json),
                            body_bytes_b64: None,
                        }),
                        telemetry: None,
                        error: None,
                    };
                    (StatusCode::OK, Json(result))
                }
            }
        }),
    );

    let mut key = sample_key(
        "key-codex-expired-quota",
        "provider-codex-expired-quota",
        "openai:responses",
        "expired-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.expires_at_unix_secs = Some(1);
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            &json!({
                "provider_type": "codex",
                "refresh_token": "expired-codex-refresh-token",
                "expires_at": 1,
                "account_id": "acct-quota-refresh",
                "plan_type": "plus"
            })
            .to_string(),
        )
        .expect("auth config should encrypt"),
    );
    key.upstream_metadata = Some(json!({
        "codex": {"credential_generation": "credential-quota-refresh"}
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-codex-expired-quota".to_string(),
            "codex".to_string(),
            Some("https://example.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")],
        vec![sample_endpoint(
            "endpoint-codex-expired-quota",
            "provider-codex-expired-quota",
            "openai:responses",
            "https://chatgpt.com/backend-api",
        )],
        vec![key],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
            ),
        ]);
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-codex-expired-quota/refresh-quota"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], 1, "payload={payload}");
    assert_eq!(payload["failed"], 0, "payload={payload}");
    assert_eq!(payload["results"][0]["status"], "success");
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);
    assert_eq!(
        seen_requests.lock().expect("mutex should lock").as_slice(),
        [
            (
                "https://chatgpt.com/backend-api/wham/usage".to_string(),
                "Bearer refreshed-codex-access-token".to_string(),
            ),
            (
                "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits".to_string(),
                "Bearer refreshed-codex-access-token".to_string(),
            ),
        ]
    );

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-expired-quota".to_string()])
        .await
        .expect("key should reload");
    let persisted = reloaded.first().expect("key should remain installed");
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should persist"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "refreshed-codex-access-token");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should persist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["refresh_token"], "rotated-codex-refresh-token");
    assert_eq!(
        persisted
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/codex/credential_generation")),
        Some(&json!("credential-quota-refresh"))
    );
    assert_eq!(
        persisted
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/codex/primary_used_percent")),
        Some(&json!(23.0))
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_codex_reset_credit_retries_until_same_window_usage_drop_is_authoritative() {
    run_provider_quota_test(
        "gateway_codex_reset_credit_retries_until_same_window_usage_drop_is_authoritative",
        gateway_codex_reset_credit_retries_until_same_window_usage_drop_is_authoritative_impl,
    );
}

async fn gateway_codex_reset_credit_retries_until_same_window_usage_drop_is_authoritative_impl() {
    const RESET_FENCE_UNIX_MS: u64 = 1_800_000_000_000;
    const RESET_AT_UNIX_SECS: u64 = 2_000_000_000;

    let usage_hits = Arc::new(Mutex::new(0usize));
    let detail_hits = Arc::new(Mutex::new(0usize));
    let seen_urls = Arc::new(Mutex::new(Vec::<String>::new()));
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any({
            let usage_hits = Arc::clone(&usage_hits);
            let detail_hits = Arc::clone(&detail_hits);
            let seen_urls = Arc::clone(&seen_urls);
            move |request: Request| {
                let usage_hits = Arc::clone(&usage_hits);
                let detail_hits = Arc::clone(&detail_hits);
                let seen_urls = Arc::clone(&seen_urls);
                async move {
                    let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                        &to_bytes(request.into_body(), usize::MAX)
                            .await
                            .expect("body should read"),
                    )
                    .expect("plan should parse");
                    seen_urls
                        .lock()
                        .expect("mutex should lock")
                        .push(plan.url.clone());
                    assert_eq!(
                        plan.headers.get("authorization").map(String::as_str),
                        Some("Bearer codex-reset-access-token")
                    );

                    let (body_json, response_observation) = match plan.url.as_str() {
                        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume" => {
                            assert_eq!(plan.method, "POST");
                            assert_eq!(
                                plan.body.json_body,
                                Some(json!({"redeem_request_id": "reset-e2e"}))
                            );
                            (
                                json!({"outcome": "reset"}),
                                Some(aether_contracts::ExecutionResponseObservation {
                                    request_started_at_unix_ms: RESET_FENCE_UNIX_MS - 100,
                                    response_headers_observed_at_unix_ms: RESET_FENCE_UNIX_MS,
                                    request_order_id: "consume-reset-e2e".to_string(),
                                }),
                            )
                        }
                        "https://chatgpt.com/backend-api/wham/usage" => {
                            let hit = {
                                let mut hits = usage_hits.lock().expect("mutex should lock");
                                *hits += 1;
                                *hits
                            };
                            let used_percent = match hit {
                                1 => 100.0,
                                2 => 0.0,
                                _ => panic!("unexpected wham/usage request #{hit}"),
                            };
                            (
                                json!({
                                    "plan_type": "plus",
                                    "rate_limit": {
                                        "primary_window": {
                                            "used_percent": used_percent,
                                            "reset_at": RESET_AT_UNIX_SECS,
                                            "window_minutes": 300
                                        }
                                    }
                                }),
                                Some(aether_contracts::ExecutionResponseObservation {
                                    request_started_at_unix_ms: RESET_FENCE_UNIX_MS
                                        + (hit as u64 * 1_000),
                                    response_headers_observed_at_unix_ms: RESET_FENCE_UNIX_MS
                                        + (hit as u64 * 1_000)
                                        + 100,
                                    request_order_id: format!("usage-reset-e2e-{hit}"),
                                }),
                            )
                        }
                        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits" => {
                            *detail_hits.lock().expect("mutex should lock") += 1;
                            (json!({"available_count": 0, "credits": []}), None)
                        }
                        url => panic!("unexpected execution runtime URL: {url}"),
                    };
                    let result = aether_contracts::ExecutionResult {
                        request_id: plan.request_id,
                        candidate_id: None,
                        status_code: 200,
                        headers: BTreeMap::new(),
                        response_observation,
                        body: Some(aether_contracts::ResponseBody {
                            json_body: Some(body_json),
                            body_bytes_b64: None,
                        }),
                        telemetry: None,
                        error: None,
                    };
                    (StatusCode::OK, Json(result))
                }
            }
        }),
    );

    let mut key = sample_key(
        "key-codex-reset",
        "provider-codex-reset",
        "openai:responses",
        "codex-reset-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.expires_at_unix_secs = Some(4_102_444_800);
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            &json!({
                "provider_type": "codex",
                "refresh_token": "codex-reset-refresh-token",
                "expires_at": 4_102_444_800u64,
                "account_id": "acct-reset-e2e",
                "plan_type": "plus"
            })
            .to_string(),
        )
        .expect("auth config should encrypt"),
    );
    key.upstream_metadata = Some(json!({
        "codex": {
            "credential_generation": "credential-reset-e2e",
            "plan_type": "plus",
            "primary_used_percent": 100.0,
            "primary_reset_at": RESET_AT_UNIX_SECS,
            "primary_window_minutes": 300,
            "updated_at": (RESET_FENCE_UNIX_MS / 1_000) - 1,
            "account_quota_request_started_at_unix_ms": RESET_FENCE_UNIX_MS - 1_000,
            "account_quota_request_id": "usage-before-reset"
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-codex-reset".to_string(),
            "codex".to_string(),
            Some("https://example.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")],
        vec![sample_endpoint(
            "endpoint-codex-reset",
            "provider-codex-reset",
            "openai:responses",
            "https://chatgpt.com/backend-api",
        )],
        vec![key],
    ));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
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
            "{gateway_url}/api/admin/endpoints/keys/key-codex-reset/codex-reset-credit/consume"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "idempotency_key": "reset-e2e",
            "expected_credential_generation": "credential-reset-e2e"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["status"], "success", "payload={payload}");
    assert_eq!(payload["outcome"], "reset");
    assert_eq!(payload["refresh_status"], "success");
    assert_eq!(*usage_hits.lock().expect("mutex should lock"), 2);
    assert_eq!(*detail_hits.lock().expect("mutex should lock"), 2);
    assert_eq!(
        seen_urls.lock().expect("mutex should lock").as_slice(),
        [
            "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume",
            "https://chatgpt.com/backend-api/wham/usage",
            "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits",
            "https://chatgpt.com/backend-api/wham/usage",
            "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits",
        ]
    );

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-reset".to_string()])
        .await
        .expect("key should reload");
    let codex = reloaded[0]
        .upstream_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("codex"))
        .expect("codex metadata should persist");
    assert_eq!(codex["primary_used_percent"], json!(0.0));
    assert_eq!(codex["primary_reset_at"], json!(RESET_AT_UNIX_SECS));
    assert_eq!(codex["account_quota_reset_pending"], json!(false));
    assert_eq!(
        codex["account_quota_reset_processed_ids"],
        json!(["reset-e2e"])
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_marks_codex_quota_exhausted_when_wham_usage_returns_payment_required() {
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-codex/refresh-quota",
        any(move |_request: Request| async move {
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| async move {
            let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                &to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should read"),
            )
            .expect("plan should parse");
            let result = aether_contracts::ExecutionResult {
                request_id: plan.request_id,
                candidate_id: None,
                status_code: 402,
                headers: BTreeMap::new(),
                response_observation: None,
                body: Some(aether_contracts::ResponseBody {
                    json_body: Some(json!({
                        "error": {
                            "message": "payment required"
                        }
                    })),
                    body_bytes_b64: None,
                }),
                telemetry: None,
                error: None,
            };
            (StatusCode::OK, Json(result))
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
        .header(GATEWAY_HEADER, "rust-phase3b")
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
    assert_eq!(payload["results"][0]["status"], "quota_exhausted");
    assert_eq!(payload["results"][0]["status_code"], 402);
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["provider_type"],
        "codex"
    );
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["exhausted"],
        json!(true)
    );

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-a".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].oauth_invalid_at_unix_secs, None);
    assert_eq!(reloaded[0].oauth_invalid_reason, None);
    assert_eq!(
        reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(|value| value.get("codex"))
            .and_then(|value| value.get("primary_used_percent")),
        Some(&json!(100.0))
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_auto_removes_codex_key_when_quota_proves_oauth_invalid() {
    let upstream = Router::new().route(
        "/api/admin/endpoints/providers/provider-codex/refresh-quota",
        any(move |_request: Request| async move {
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| async move {
            let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                &to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should read"),
            )
            .expect("plan should parse");
            let result = aether_contracts::ExecutionResult {
                request_id: plan.request_id,
                candidate_id: None,
                status_code: 401,
                headers: BTreeMap::new(),
                response_observation: None,
                body: Some(aether_contracts::ResponseBody {
                    json_body: Some(json!({
                        "error": {
                            "message": "session expired"
                        }
                    })),
                    body_bytes_b64: None,
                }),
                telemetry: None,
                error: None,
            };
            (StatusCode::OK, Json(result))
        }),
    );

    let mut provider = StoredProviderCatalogProvider::new(
        "provider-codex".to_string(),
        "codex".to_string(),
        Some("https://example.com".to_string()),
        "codex".to_string(),
    )
    .expect("provider should build");
    provider.config = Some(json!({
        "pool_advanced": {
            "auto_remove_banned_keys": true
        }
    }));

    let mut key = sample_key(
        "key-codex-expired",
        "provider-codex",
        "openai:responses",
        "stale-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.expires_at_unix_secs = Some(4_102_444_800);
    key.oauth_invalid_at_unix_secs = Some(1);
    key.oauth_invalid_reason = Some(
        "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 无效、已过期或已撤销，请重新登录授权"
            .to_string(),
    );
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"invalid-refresh-token","expires_at":4102444800}"#,
        )
        .expect("auth config should encrypt"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-codex-cli",
            "provider-codex",
            "openai:responses",
            "https://chatgpt.com/backend-api",
        )],
        vec![key],
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
        .header(GATEWAY_HEADER, "rust-phase3b")
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
    assert_eq!(payload["auto_removed"], 1, "payload={payload}");
    assert_eq!(payload["results"][0]["status"], "auth_invalid");
    assert_eq!(payload["results"][0]["auto_removed"], true);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-expired".to_string()])
        .await
        .expect("keys should read");
    assert!(reloaded.is_empty());

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_refreshes_admin_provider_quota_locally_for_requested_codex_keys_only() {
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

    let seen_authorizations = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_authorizations_clone = Arc::clone(&seen_authorizations);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_authorizations_inner = Arc::clone(&seen_authorizations_clone);
            async move {
                let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                    &to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read"),
                )
                .expect("plan should parse");
                seen_authorizations_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(
                        plan.headers
                            .get("authorization")
                            .cloned()
                            .unwrap_or_default(),
                    );
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
                    ]),
                    response_observation: None,
                    body: Some(aether_contracts::ResponseBody {
                        json_body: Some(json!({
                            "plan_type": "plus",
                            "rate_limit": {
                                "primary_window": {
                                    "used_percent": 12.5,
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
        vec![
            sample_key(
                "key-codex-a",
                "provider-codex",
                "openai:responses",
                "sk-codex-a",
            ),
            sample_key(
                "key-codex-b",
                "provider-codex",
                "openai:responses",
                "sk-codex-b",
            ),
        ],
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
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "key_ids": ["key-codex-a"] }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 0);
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["results"][0]["key_id"], "key-codex-a");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        seen_authorizations
            .lock()
            .expect("mutex should lock")
            .clone(),
        vec![
            "Bearer sk-codex-a".to_string(),
            "Bearer sk-codex-a".to_string(),
        ]
    );

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-a".to_string(), "key-codex-b".to_string()])
        .await
        .expect("keys should read");
    let key_a = reloaded
        .iter()
        .find(|key| key.id == "key-codex-a")
        .expect("selected key should reload");
    let key_b = reloaded
        .iter()
        .find(|key| key.id == "key-codex-b")
        .expect("unselected key should reload");
    assert!(key_a.upstream_metadata.is_some());
    assert_eq!(key_b.upstream_metadata, None);

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
                    response_observation: None,
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
                    response_observation: None,
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
        .starts_with("https://management.us-west-2.kiro.dev/getUsageLimits?"),);
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
async fn gateway_auto_removes_kiro_quota_refresh_after_terminal_oauth_refresh_failure() {
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/refreshToken",
        post(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "message": "refresh token invalid"
                    })),
                )
            }
        }),
    );
    let execution_hits = Arc::new(Mutex::new(0usize));
    let execution_hits_clone = Arc::clone(&execution_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let execution_hits_inner = Arc::clone(&execution_hits_clone);
            async move {
                *execution_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Body::from("unexpected execution runtime hit"),
                )
            }
        }),
    );

    let encrypted_auth_config = encrypt_python_fernet_plaintext(
        DEVELOPMENT_ENCRYPTION_KEY,
        &json!({
            "provider_type": "kiro",
            "auth_method": "social",
            "refresh_token": "r".repeat(120),
            "machine_id": "123e4567-e89b-12d3-a456-426614174000",
            "kiro_version": "1.2.3",
            "expires_at": 1u64
        })
        .to_string(),
    )
    .expect("auth config ciphertext should build");
    let encrypted_api_key =
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
            .expect("api key ciphertext should build");
    let mut key = StoredProviderCatalogKey::new(
        "key-kiro-expired-refresh-failed".to_string(),
        "provider-kiro-auto-remove".to_string(),
        "default".to_string(),
        "oauth".to_string(),
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
    key.expires_at_unix_secs = Some(1);

    let mut provider = StoredProviderCatalogProvider::new(
        "provider-kiro-auto-remove".to_string(),
        "kiro".to_string(),
        Some("https://example.com".to_string()),
        "kiro".to_string(),
    )
    .expect("provider should build");
    provider.config = Some(json!({
        "pool_advanced": {
            "auto_remove_banned_keys": true
        }
    }));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![sample_endpoint(
            "endpoint-kiro-cli",
            "provider-kiro-auto-remove",
            "claude:messages",
            "https://q.us-west-2.amazonaws.com",
        )],
        vec![key],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::kiro::KiroOAuthRefreshAdapter::default()
                    .with_refresh_base_urls(Some(token_url), None),
            )
                as Arc<dyn crate::provider_transport::oauth_refresh::LocalOAuthRefreshAdapter>,
        ]);
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url.clone())
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/endpoints/providers/provider-kiro-auto-remove/refresh-quota"
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
    assert_eq!(payload["failed"], 0);
    assert_eq!(payload["auto_removed"], 1);
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["results"][0]["status"], "auto_removed");
    assert_eq!(payload["results"][0]["auto_removed"], true);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);
    assert_eq!(*execution_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-kiro-expired-refresh-failed".to_string()])
        .await
        .expect("keys should read");
    assert!(reloaded.is_empty());

    gateway_handle.abort();
    execution_runtime_handle.abort();
    token_handle.abort();
}

#[tokio::test]
async fn gateway_refresh_kiro_quota_reconciles_missing_fixed_endpoint_before_refresh() {
    let seen_endpoint_id = Arc::new(Mutex::new(None::<String>));
    let seen_endpoint_id_clone = Arc::clone(&seen_endpoint_id);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_endpoint_id_inner = Arc::clone(&seen_endpoint_id_clone);
            async move {
                let plan: aether_contracts::ExecutionPlan = serde_json::from_slice(
                    &to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read"),
                )
                .expect("plan should parse");
                *seen_endpoint_id_inner.lock().expect("mutex should lock") =
                    Some(plan.endpoint_id.clone());
                let result = aether_contracts::ExecutionResult {
                    request_id: plan.request_id,
                    candidate_id: None,
                    status_code: 200,
                    headers: BTreeMap::new(),
                    response_observation: None,
                    body: Some(aether_contracts::ResponseBody {
                        json_body: Some(json!({
                            "subscriptionInfo": {
                                "subscriptionTitle": "KIRO PRO"
                            },
                            "usageBreakdownList": [{
                                "currentUsageWithPrecision": 1.0,
                                "usageLimitWithPrecision": 10.0,
                                "nextDateReset": 1_900_000_000u64
                            }]
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
        r#"{"access_token":"kiro-access-token","api_region":"us-west-2"}"#,
    )
    .expect("auth config ciphertext should build");
    let encrypted_api_key =
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
            .expect("api key ciphertext should build");
    let key = StoredProviderCatalogKey::new(
        "key-kiro-reconcile".to_string(),
        "provider-kiro-reconcile".to_string(),
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
            "provider-kiro-reconcile".to_string(),
            "kiro".to_string(),
            Some("https://example.com".to_string()),
            "kiro".to_string(),
        )
        .expect("provider should build")],
        Vec::new(),
        vec![key],
    ));

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
            "{gateway_url}/api/admin/endpoints/providers/provider-kiro-reconcile/refresh-quota"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
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
    assert_eq!(payload["results"][0]["status"], "success");

    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&["provider-kiro-reconcile".to_string()])
        .await
        .expect("endpoints should read");
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].api_format, "claude:messages");
    assert_eq!(endpoints[0].base_url, "https://runtime.{region}.kiro.dev");
    assert_eq!(
        *seen_endpoint_id.lock().expect("mutex should lock"),
        Some(endpoints[0].id.clone())
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_refreshes_admin_provider_quota_locally_for_gemini_cli_with_trusted_admin_principal(
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
        "/api/admin/endpoints/providers/provider-gemini-cli/refresh-quota",
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
                    response_observation: None,
                    body: Some(aether_contracts::ResponseBody {
                        json_body: Some(json!({
                            "buckets": [
                                {
                                    "modelId": "gemini-2.5-pro",
                                    "tokenType": "model",
                                    "displayName": "Gemini 2.5 Pro",
                                    "remainingFraction": 0.25,
                                    "resetTime": "2030-01-01T00:00:00Z",
                                    "isExhausted": false
                                },
                                {
                                    "modelId": "gemini-2.5-flash",
                                    "tokenType": "model",
                                    "displayName": "Gemini 2.5 Flash",
                                    "quotaInfo": {
                                        "remainingFraction": 0.0,
                                        "resetTime": "2030-01-01T01:00:00Z",
                                        "isExhausted": true
                                    }
                                }
                            ]
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

    let mut key = sample_key(
        "key-gemini-cli-quota",
        "provider-gemini-cli",
        "gemini:generate_content",
        "cached-gemini-cli-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"gemini_cli","project_id":"gemini-cli-project-1"}"#,
        )
        .expect("auth config should encrypt"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-gemini-cli".to_string(),
            "gemini_cli".to_string(),
            Some("https://example.com".to_string()),
            "gemini_cli".to_string(),
        )
        .expect("provider should build")],
        vec![sample_endpoint(
            "endpoint-gemini-cli-quota",
            "provider-gemini-cli",
            "gemini:generate_content",
            "https://cloudcode-pa.googleapis.com",
        )],
        vec![key],
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
            "{gateway_url}/api/admin/endpoints/providers/provider-gemini-cli/refresh-quota"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
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
    assert_eq!(payload["results"][0]["status"], "success");
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["provider_type"],
        "gemini_cli"
    );
    assert_eq!(
        payload["results"][0]["quota_snapshot"]["windows"][0]["model"],
        "gemini-2.5-pro"
    );

    let seen_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime request should be captured");
    assert_eq!(
        seen_request.url,
        "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota"
    );
    assert_eq!(seen_request.authorization, "Bearer cached-gemini-cli-token");
    assert_eq!(
        seen_request.provider_api_format,
        "gemini_cli:retrieve_user_quota"
    );
    assert_eq!(
        seen_request.request_body,
        Some(json!({
            "project": "gemini-cli-project-1"
        }))
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-gemini-cli-quota".to_string()])
        .await
        .expect("keys should read");
    assert_eq!(reloaded.len(), 1);
    let upstream_metadata = reloaded[0]
        .upstream_metadata
        .as_ref()
        .expect("upstream metadata should persist");
    assert_eq!(
        upstream_metadata["gemini_cli"]["quota_by_model"]["gemini-2.5-pro"]["remaining_fraction"],
        json!(0.25)
    );
    assert_eq!(
        upstream_metadata["gemini_cli"]["quota_by_model"]["gemini-2.5-flash"]["is_exhausted"],
        json!(true)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_refresh_quota_reconciles_unsupported_fixed_provider_endpoints_before_clear_message(
) {
    let cases = [
        (
            "provider-claude-code-reconcile",
            "claude_code",
            1usize,
            "claude:messages",
            "https://api.anthropic.com/v1",
            "Claude Code 暂不支持自动刷新额度",
        ),
        (
            "provider-vertex-ai-reconcile",
            "vertex_ai",
            2usize,
            "gemini:generate_content",
            "https://aiplatform.googleapis.com",
            "Vertex AI 暂不支持自动刷新额度",
        ),
    ];

    let providers = cases
        .iter()
        .map(|(provider_id, provider_type, _, _, _, _)| {
            StoredProviderCatalogProvider::new(
                (*provider_id).to_string(),
                (*provider_type).to_string(),
                Some("https://example.com".to_string()),
                (*provider_type).to_string(),
            )
            .expect("provider should build")
        })
        .collect::<Vec<_>>();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        providers,
        Vec::new(),
        Vec::new(),
    ));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override("http://127.0.0.1:1")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for (provider_id, _, endpoint_count, api_format, base_url, message_prefix) in cases {
        let response = client
            .post(format!(
                "{gateway_url}/api/admin/endpoints/providers/{provider_id}/refresh-quota"
            ))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["success"], 0);
        assert_eq!(payload["failed"], 0);
        assert_eq!(payload["total"], 0);
        assert!(payload["message"]
            .as_str()
            .expect("message should be string")
            .starts_with(message_prefix));

        let endpoints = provider_catalog_repository
            .list_endpoints_by_provider_ids(&[provider_id.to_string()])
            .await
            .expect("endpoints should read");
        assert_eq!(endpoints.len(), endpoint_count);
        assert!(endpoints
            .iter()
            .any(|endpoint| endpoint.api_format == api_format && endpoint.base_url == base_url));
    }

    gateway_handle.abort();
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

#[test]
fn gateway_refreshes_admin_provider_quota_locally_for_antigravity_with_trusted_admin_principal() {
    run_provider_quota_test(
        "gateway_refreshes_admin_provider_quota_locally_for_antigravity_with_trusted_admin_principal",
        gateway_refreshes_admin_provider_quota_locally_for_antigravity_with_trusted_admin_principal_inner,
    );
}

async fn gateway_refreshes_admin_provider_quota_locally_for_antigravity_with_trusted_admin_principal_inner(
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
                    response_observation: None,
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
