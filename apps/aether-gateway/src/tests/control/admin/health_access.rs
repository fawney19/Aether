use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth_modules::InMemoryAuthModuleReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::management_tokens::{
    InMemoryManagementTokenRepository, ManagementTokenListQuery, ManagementTokenReadRepository,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use axum::body::Body;
use axum::routing::{any, get, patch, put};
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, hash_management_token, issue_test_admin_access_token,
    sample_bound_key, sample_endpoint, sample_ldap_module_config, sample_management_token,
    sample_oauth_module_provider, sample_provider, sample_request_candidate, start_server,
    AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::control::all_assignable_management_token_permissions;
use crate::data::GatewayDataState;

const ADMIN_ENDPOINT_HEALTH_DATA_UNAVAILABLE_DETAIL: &str =
    "Admin endpoint health data unavailable";

async fn assert_admin_modules_status_with_smtp_password(
    stored_password: &str,
    notification_ready: bool,
    server_chan_enabled: bool,
) -> AppState {
    let data = GatewayDataState::with_auth_module_reader_for_tests(Arc::new(
        InMemoryAuthModuleReadRepository::seed(Vec::new(), None),
    ))
    .with_provider_catalog_reader(Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )))
    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
    .with_system_config_values_for_tests(vec![
        ("module.management_tokens.enabled".to_string(), json!(true)),
        (
            "module.important_notification.enabled".to_string(),
            json!(true),
        ),
        (
            "module.important_notification.email_enabled".to_string(),
            json!(true),
        ),
        (
            "module.important_notification.email_recipients".to_string(),
            json!("ops@example.com"),
        ),
        (
            "module.server_chan_push.enabled".to_string(),
            json!(server_chan_enabled),
        ),
        (
            "module.server_chan_push.send_key".to_string(),
            json!(if server_chan_enabled {
                "SCT-test-send-key"
            } else {
                ""
            }),
        ),
        ("smtp_host".to_string(), json!("smtp.example.com")),
        ("smtp_port".to_string(), json!(587)),
        ("smtp_user".to_string(), json!("ops@example.com")),
        ("smtp_password".to_string(), json!(stored_password)),
        ("smtp_use_tls".to_string(), json!(true)),
        ("smtp_from_email".to_string(), json!("ops@example.com")),
    ]);
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state.clone())).await;
    let client = reqwest::Client::new();

    for path in [
        "/api/admin/modules/status",
        "/api/admin/modules/status/important_notification",
    ] {
        let response = client
            .get(format!("{gateway_url}{path}"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("module status request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: serde_json::Value = response.json().await.expect("module status should parse");
        assert!(!payload.to_string().contains(stored_password));
        let notification = if path == "/api/admin/modules/status" {
            assert_eq!(
                payload
                    .as_object()
                    .expect("module list should be an object")
                    .len(),
                14
            );
            assert_eq!(payload["management_tokens"]["active"], json!(true));
            &payload["important_notification"]
        } else {
            &payload
        };
        assert_eq!(notification["enabled"], json!(true));
        assert_eq!(notification["config_validated"], json!(notification_ready));
        assert_eq!(notification["active"], json!(notification_ready));
        assert_eq!(notification["config_error"].is_null(), notification_ready);
    }
    gateway_handle.abort();

    assert_eq!(
        crate::important_notification::important_notification_dispatch_ready_for_item(
            &state,
            crate::important_notification::PROVIDER_QUOTA_ALERT_ITEM_KEY,
        )
        .await
        .expect("SMTP errors should not abort notification readiness"),
        notification_ready
    );
    let summary = crate::maintenance::perform_provider_quota_alert_once(&state)
        .await
        .expect("SMTP errors should not abort the quota alert worker");
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.alerted, 0);
    state
}

#[tokio::test]
async fn gateway_handles_admin_modules_status_with_legacy_smtp_password() {
    let ciphertext =
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "legacy-smtp-password")
            .expect("legacy SMTP password should encrypt");
    let state = assert_admin_modules_status_with_smtp_password(&ciphertext, true, false).await;
    let stored = state
        .read_system_config_json_value_strong("smtp_password")
        .await
        .unwrap()
        .unwrap();
    assert!(stored
        .as_str()
        .unwrap()
        .starts_with("aether-smtp-password-v3:"));
    let smtp = crate::email_delivery::read_smtp_delivery_config(&state)
        .await
        .expect("migrated SMTP config should load")
        .expect("SMTP should be configured");
    assert_eq!(smtp.password.as_deref(), Some("legacy-smtp-password"));
}

#[tokio::test]
async fn gateway_handles_admin_modules_status_with_invalid_smtp_password() {
    let ciphertext =
        encrypt_python_fernet_plaintext("unavailable-historical-key", "legacy-smtp-password")
            .expect("unknown-key SMTP password should encrypt");
    let state = assert_admin_modules_status_with_smtp_password(&ciphertext, false, false).await;
    assert_eq!(
        state
            .read_system_config_json_value_strong("smtp_password")
            .await
            .unwrap(),
        Some(json!(ciphertext))
    );
}

#[tokio::test]
async fn gateway_handles_admin_modules_status_with_invalid_smtp_and_working_push() {
    assert_admin_modules_status_with_smtp_password("aether-smtp-password-v3:invalid", true, true)
        .await;
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_health_api_formats_when_readers_unavailable()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/api-formats",
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
        .get(format!(
            "{gateway_url}/api/admin/endpoints/health/api-formats?lookback_hours=6&per_format_limit=60"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        ADMIN_ENDPOINT_HEALTH_DATA_UNAVAILABLE_DETAIL
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_health_api_formats_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/api-formats",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_bound_key(
            "key-openai",
            "provider-openai",
            "openai:chat",
            "sk-test",
        )],
    ));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time should be after epoch")
        .as_secs() as i64;
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_request_candidate(
            "cand-openai-success",
            "req-openai-success",
            "endpoint-openai",
            RequestCandidateStatus::Success,
            now_unix_secs - 3_000,
            Some(now_unix_secs - 2_980),
        ),
        sample_request_candidate(
            "cand-openai-failed",
            "req-openai-failed",
            "endpoint-openai",
            RequestCandidateStatus::Failed,
            now_unix_secs - 2_000,
            Some(now_unix_secs - 1_980),
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_and_request_candidate_reader_for_tests(
                    provider_catalog_repository,
                    request_candidate_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/health/api-formats?lookback_hours=6&per_format_limit=60"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "body={body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    let formats = payload["formats"]
        .as_array()
        .expect("formats should be an array");
    assert_eq!(formats.len(), 1);
    assert_eq!(formats[0]["api_format"], "openai:chat");
    assert_eq!(formats[0]["provider_count"], 1);
    assert_eq!(formats[0]["key_count"], 1);
    assert_eq!(formats[0]["total_attempts"], 2);
    assert_eq!(formats[0]["success_count"], 1);
    assert_eq!(formats[0]["failed_count"], 1);
    assert_eq!(formats[0]["skipped_count"], 0);
    assert!(formats[0].get("api_path").is_none());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_health_summary_when_reader_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/summary",
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
        .get(format!("{gateway_url}/api/admin/endpoints/health/summary"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["detail"],
        ADMIN_ENDPOINT_HEALTH_DATA_UNAVAILABLE_DETAIL
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_health_summary_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/summary",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![
            sample_endpoint(
                "endpoint-openai-healthy",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
            )
            .with_health_score(0.9),
            sample_endpoint(
                "endpoint-openai-unhealthy",
                "provider-openai",
                "openai:chat",
                "https://api.openai.example",
            )
            .with_health_score(0.2),
        ],
        vec![
            sample_bound_key(
                "key-openai-active",
                "provider-openai",
                "openai:chat",
                "sk-test",
            )
            .with_health_fields(
                Some(json!({"openai:chat": {"health_score": 0.9}})),
                Some(json!({"openai:chat": {"open": false}})),
            ),
            sample_bound_key(
                "key-openai-circuit",
                "provider-openai",
                "openai:chat",
                "sk-test-2",
            )
            .with_health_fields(
                Some(json!({"openai:chat": {"health_score": 0.3}})),
                Some(json!({"openai:chat": {"open": true}})),
            ),
        ],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/endpoints/health/summary"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "body={body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    assert_eq!(payload["endpoints"]["total"], 2);
    assert_eq!(payload["endpoints"]["active"], 2);
    assert_eq!(payload["endpoints"]["unhealthy"], 1);
    assert_eq!(payload["keys"]["total"], 2);
    assert_eq!(payload["keys"]["active"], 2);
    assert_eq!(payload["keys"]["unhealthy"], 1);
    assert_eq!(payload["keys"]["circuit_open"], 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
#[tokio::test]
async fn gateway_handles_admin_key_health_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/key/key-openai",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![
            sample_bound_key("key-openai", "provider-openai", "openai:chat", "sk-test")
                .with_rate_limit_fields(None, None, None, None, None, None, None, Some(10), Some(7))
                .with_usage_fields(Some(3), Some(2100))
                .with_health_fields(
                    Some(json!({"openai:chat": {
                        "health_score": 0.7,
                        "consecutive_failures": 2,
                        "last_failure_at": "2026-03-26T12:00:00+00:00"
                    }})),
                    Some(json!({"openai:chat": {
                        "open": true,
                        "open_at": "2026-03-26T12:01:00+00:00",
                        "next_probe_at": "2099-03-26T12:05:00+00:00",
                        "half_open_until": null,
                        "half_open_successes": 1,
                        "half_open_failures": 0
                    }})),
                ),
        ],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/health/key/key-openai?api_format=openai:chat"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "body={body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    assert_eq!(payload["key_id"], "key-openai");
    assert_eq!(payload["key_is_active"], true);
    assert_eq!(payload["key_statistics"]["request_count"], 10);
    assert_eq!(payload["key_statistics"]["success_count"], 7);
    assert_eq!(payload["key_statistics"]["error_count"], 3);
    assert_eq!(payload["key_statistics"]["avg_response_time_ms"], 300.0);
    assert_eq!(payload["api_format"], "openai:chat");
    assert_eq!(payload["key_health_score"], 0.7);
    assert_eq!(payload["key_consecutive_failures"], 2);
    assert_eq!(payload["key_last_failure_at"], "2026-03-26T12:00:00+00:00");
    assert_eq!(payload["circuit_breaker_open"], true);
    assert_eq!(
        payload["circuit_breaker_open_at"],
        "2026-03-26T12:01:00+00:00"
    );
    assert_eq!(payload["next_probe_at"], "2099-03-26T12:05:00+00:00");
    assert_eq!(payload["half_open_successes"], 1);
    assert_eq!(payload["half_open_failures"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_admin_key_health_summary_treats_expired_unix_circuit_as_closed() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/key/key-openai",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![
            sample_bound_key("key-openai", "provider-openai", "openai:chat", "sk-test")
                .with_health_fields(
                    Some(json!({"openai:chat": {
                        "health_score": 0.7,
                        "consecutive_failures": 2
                    }})),
                    Some(json!({"openai:chat": {
                        "open": true,
                        "open_at": "2026-03-26T12:01:00+00:00",
                        "next_probe_at_unix_secs": 1u64
                    }})),
                ),
        ],
    ));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_reader_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/health/key/key-openai"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::OK, "body={body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    let circuit = &payload["health_by_format"]["openai:chat"]["circuit_breaker"];
    assert_eq!(payload["any_circuit_open"], false);
    assert_eq!(circuit["open"], false);
    assert_eq!(circuit["state"], "closed");
    assert_eq!(payload["key_health_score"], 0.7);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_recovers_admin_key_health_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/keys/key-openai",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![
            sample_bound_key("key-openai", "provider-openai", "openai:chat", "sk-test")
                .with_health_fields(
                    Some(json!({"openai:chat": {
                        "health_score": 0.2,
                        "consecutive_failures": 4,
                        "last_failure_at": "2026-03-26T12:00:00+00:00"
                    }})),
                    Some(json!({"openai:chat": {
                        "open": true,
                        "open_at": "2026-03-26T12:01:00+00:00",
                        "next_probe_at": "2099-03-26T12:05:00+00:00",
                        "half_open_until": null,
                        "half_open_successes": 0,
                        "half_open_failures": 1
                    }})),
                ),
        ],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{gateway_url}/api/admin/endpoints/health/keys/key-openai?api_format=openai:chat"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["message"], "Key 的 openai:chat 格式已恢复");
    assert_eq!(payload["details"]["api_format"], "openai:chat");
    assert_eq!(payload["details"]["health_score"], 1.0);
    assert_eq!(payload["details"]["circuit_breaker_open"], false);
    assert_eq!(payload["details"]["is_active"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let recovered_key = provider_catalog_repository
        .list_keys_by_ids(&["key-openai".to_string()])
        .await
        .expect("key should read")
        .into_iter()
        .next()
        .expect("key should exist");
    assert_eq!(recovered_key.is_active, true);
    assert_eq!(
        recovered_key.health_by_format,
        Some(json!({"openai:chat": {
            "health_score": 1.0,
            "consecutive_failures": 0,
            "last_failure_at": null
        }}))
    );
    assert_eq!(
        recovered_key.circuit_breaker_by_format,
        Some(json!({"openai:chat": {
            "open": false,
            "open_at": null,
            "next_probe_at": null,
            "half_open_until": null,
            "half_open_successes": 0,
            "half_open_failures": 0
        }}))
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_recovers_all_admin_key_health_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/keys",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![
            sample_bound_key(
                "key-openai-circuit",
                "provider-openai",
                "openai:chat",
                "sk-test",
            )
            .with_health_fields(
                Some(json!({"openai:chat": {"health_score": 0.3}})),
                Some(json!({"openai:chat": {"open": true}})),
            ),
            sample_bound_key(
                "key-openai-healthy",
                "provider-openai",
                "openai:chat",
                "sk-test-2",
            )
            .with_health_fields(
                Some(json!({"openai:chat": {"health_score": 0.9}})),
                Some(json!({"openai:chat": {"open": false}})),
            ),
        ],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .patch(format!("{gateway_url}/api/admin/endpoints/health/keys"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["recovered_count"], 1);
    assert_eq!(payload["recovered_keys"][0]["key_id"], "key-openai-circuit");
    assert_eq!(
        payload["recovered_keys"][0]["provider_id"],
        "provider-openai"
    );
    assert_eq!(
        payload["recovered_keys"][0]["api_formats"],
        json!(["openai:chat"])
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let keys = provider_catalog_repository
        .list_keys_by_ids(&[
            "key-openai-circuit".to_string(),
            "key-openai-healthy".to_string(),
        ])
        .await
        .expect("keys should read");
    let circuit_key = keys
        .iter()
        .find(|key| key.id == "key-openai-circuit")
        .expect("circuit key should exist");
    let healthy_key = keys
        .iter()
        .find(|key| key.id == "key-openai-healthy")
        .expect("healthy key should exist");
    assert_eq!(circuit_key.health_by_format, Some(json!({})));
    assert_eq!(circuit_key.circuit_breaker_by_format, Some(json!({})));
    assert_eq!(
        healthy_key.circuit_breaker_by_format,
        Some(json!({"openai:chat": {"open": false}}))
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_health_status_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/endpoints/health/status",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-openai", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-openai",
            "provider-openai",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_bound_key(
            "key-openai",
            "provider-openai",
            "openai:chat",
            "sk-test",
        )],
    ));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time should be after epoch")
        .as_secs() as i64;
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_request_candidate(
            "cand-openai-success",
            "req-openai-success",
            "endpoint-openai",
            RequestCandidateStatus::Success,
            now_unix_secs - 3_000,
            Some(now_unix_secs - 2_980),
        ),
        sample_request_candidate(
            "cand-openai-failed",
            "req-openai-failed",
            "endpoint-openai",
            RequestCandidateStatus::Failed,
            now_unix_secs - 2_000,
            Some(now_unix_secs - 1_980),
        ),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_and_request_candidate_reader_for_tests(
                    provider_catalog_repository,
                    request_candidate_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/endpoints/health/status?lookback_hours=6"
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
    let formats = payload.as_array().expect("payload should be an array");
    assert_eq!(formats.len(), 1);
    assert_eq!(formats[0]["api_format"], "openai:chat");
    assert_eq!(formats[0]["display_name"], "OpenAI Chat");
    assert_eq!(formats[0]["total_endpoints"], 1);
    assert_eq!(formats[0]["total_keys"], 1);
    assert_eq!(formats[0]["active_keys"], 1);
    assert_eq!(formats[0]["provider_count"], 1);
    assert_eq!(formats[0]["health_score"], 0.5);
    assert_eq!(
        formats[0]["timeline"]
            .as_array()
            .expect("timeline should be an array")
            .len(),
        60
    );
    assert!(formats[0]["time_range_start"].is_string());
    assert!(formats[0]["time_range_end"].is_string());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_modules_status_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/modules/status",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        vec![sample_oauth_module_provider("linuxdo", "Linux DO")],
        Some(sample_ldap_module_config()),
    ));
    let data_state = GatewayDataState::with_auth_module_reader_for_tests(auth_module_repository)
        .with_system_config_values_for_tests(vec![
            ("module.oauth.enabled".to_string(), json!(true)),
            ("module.management_tokens.enabled".to_string(), json!(true)),
            (
                "module.important_notification.email_enabled".to_string(),
                json!(true),
            ),
            (
                "module.important_notification.email_recipients".to_string(),
                json!("ops@example.com"),
            ),
            ("smtp_host".to_string(), json!("smtp.example.com")),
            ("smtp_from_email".to_string(), json!("ops@example.com")),
        ]);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/modules/status"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["oauth"]["enabled"], json!(true));
    assert_eq!(payload["oauth"]["active"], json!(true));
    assert_eq!(payload["oauth"]["config_validated"], json!(true));
    assert_eq!(payload["management_tokens"]["active"], json!(true));
    assert_eq!(payload["chat_pii_redaction"]["enabled"], json!(false));
    assert_eq!(
        payload["chat_pii_redaction"]["display_name"],
        "敏感信息保护"
    );
    assert_eq!(
        payload["chat_pii_redaction"]["config_validated"],
        json!(true)
    );
    assert_eq!(
        payload["chat_pii_redaction"]["admin_route"],
        "/admin/modules/chat-pii-redaction"
    );
    assert_eq!(
        payload["important_notification"]["config_validated"],
        json!(true)
    );
    assert_eq!(
        payload["important_notification"]["admin_route"],
        "/admin/notification-service"
    );
    assert_eq!(payload["server_chan_push"]["display_name"], "Server 酱推送");
    assert_eq!(
        payload["server_chan_push"]["admin_route"],
        "/admin/modules/server-chan"
    );
    assert_eq!(payload["bark_push"]["display_name"], "Bark 推送");
    assert_eq!(payload["bark_push"]["admin_route"], "/admin/modules/bark");
    assert_eq!(payload["s3_backup"]["display_name"], "S3 备份");
    assert_eq!(
        payload["s3_backup"]["admin_route"],
        "/admin/modules/s3-backup"
    );
    assert_eq!(
        payload["s3_backup"]["admin_menu_group"],
        serde_json::Value::Null
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_modules_status_locally_with_bearer_admin_session() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/modules/status",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        vec![sample_oauth_module_provider("linuxdo", "Linux DO")],
        Some(sample_ldap_module_config()),
    ));
    let data_state = GatewayDataState::with_auth_module_reader_for_tests(auth_module_repository)
        .with_system_config_values_for_tests(vec![
            ("module.oauth.enabled".to_string(), json!(true)),
            ("module.management_tokens.enabled".to_string(), json!(true)),
            (
                "module.important_notification.email_enabled".to_string(),
                json!(true),
            ),
            (
                "module.important_notification.email_recipients".to_string(),
                json!("ops@example.com"),
            ),
            ("smtp_host".to_string(), json!("smtp.example.com")),
            ("smtp_from_email".to_string(), json!("ops@example.com")),
        ]);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state);
    let access_token = issue_test_admin_access_token(&state, "device-admin-modules").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/modules/status"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-modules")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["oauth"]["enabled"], json!(true));
    assert_eq!(payload["oauth"]["active"], json!(true));
    assert_eq!(payload["oauth"]["config_validated"], json!(true));
    assert_eq!(payload["management_tokens"]["active"], json!(true));
    assert_eq!(
        payload["important_notification"]["config_validated"],
        json!(true)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_module_status_detail_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/modules/status/oauth",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        vec![sample_oauth_module_provider("linuxdo", "Linux DO")],
        None,
    ));
    let data_state = GatewayDataState::with_auth_module_reader_for_tests(auth_module_repository)
        .with_system_config_values_for_tests(vec![(
            "module.oauth.enabled".to_string(),
            json!(true),
        )]);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/modules/status/oauth"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["name"], "oauth");
    assert_eq!(payload["display_name"], "OAuth 登录");
    assert_eq!(payload["enabled"], json!(true));
    assert_eq!(payload["active"], json!(true));
    assert_eq!(payload["config_validated"], json!(true));
    assert_eq!(payload["admin_route"], "/admin/oauth");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_chat_pii_redaction_module_status_detail_locally_with_trusted_admin_principal(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/modules/status/chat_pii_redaction",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::default());
    let data_state = GatewayDataState::with_auth_module_reader_for_tests(auth_module_repository)
        .with_system_config_values_for_tests(vec![(
            "module.chat_pii_redaction.enabled".to_string(),
            json!(true),
        )]);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/modules/status/chat_pii_redaction"
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
    assert_eq!(payload["name"], "chat_pii_redaction");
    assert_eq!(payload["display_name"], "敏感信息保护");
    assert_eq!(payload["enabled"], json!(true));
    assert_eq!(payload["active"], json!(true));
    assert_eq!(payload["config_validated"], json!(true));
    assert_eq!(payload["admin_route"], "/admin/modules/chat-pii-redaction");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_sets_admin_module_enabled_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/modules/status/management_tokens/enabled",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::default());
    let data_state = GatewayDataState::with_auth_module_reader_for_tests(auth_module_repository)
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!(
            "{gateway_url}/api/admin/modules/status/management_tokens/enabled"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "enabled": true }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["name"], "management_tokens");
    assert_eq!(payload["enabled"], json!(true));
    assert_eq!(payload["active"], json!(true));
    assert_eq!(payload["config_validated"], json!(true));

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/modules/status/management_tokens"
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
    assert_eq!(payload["enabled"], json!(true));
    assert_eq!(payload["active"], json!(true));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_manages_model_directives_module_from_module_management() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/modules/status/model_directives/enabled",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::default());
    let data_state = GatewayDataState::with_auth_module_reader_for_tests(auth_module_repository)
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/modules/status/model_directives"
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
    assert_eq!(payload["name"], "model_directives");
    assert_eq!(payload["display_name"], "模型后缀参数");
    assert_eq!(payload["enabled"], json!(false));
    assert_eq!(payload["active"], json!(false));
    assert_eq!(payload["config_validated"], json!(true));
    assert_eq!(payload["admin_route"], "/admin/model-directives");

    let response = reqwest::Client::new()
        .put(format!(
            "{gateway_url}/api/admin/modules/status/model_directives/enabled"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "enabled": true }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["name"], "model_directives");
    assert_eq!(payload["enabled"], json!(true));
    assert_eq!(payload["active"], json!(true));

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/system/configs/enable_model_directives"
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
    assert_eq!(payload["key"], "enable_model_directives");
    assert_eq!(payload["value"], json!(true));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_management_tokens_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/management-tokens",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryManagementTokenRepository::seed(vec![
        sample_management_token("mt-admin-1", "user-1", "alice", true),
        sample_management_token("mt-admin-2", "user-2", "bob", false),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_management_token_repository_for_tests(repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/management-tokens?is_active=true&skip=0&limit=50"
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
    let items = payload["items"].as_array().expect("items should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(payload["total"], 1);
    assert_eq!(items[0]["id"], "mt-admin-1");
    assert_eq!(items[0]["user"]["username"], "alice");
    assert_eq!(items[0]["token_display"], "ae_test...****");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_constrained_full_management_token_creating_unconstrained_child() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/management-tokens",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let state = AppState::new().expect("gateway should build");
    let admin_user = state
        .create_local_auth_user_with_settings(
            Some("management-full@example.com".to_string()),
            true,
            "admin".to_string(),
            "hash".to_string(),
            "admin".to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("admin user should be created")
        .expect("admin user should exist");
    let raw_token = "ae-management-full-access";
    let mut management_token =
        sample_management_token("mt-admin-full", &admin_user.id, "management-full", true);
    management_token.token.allowed_ips = Some(json!(["127.0.0.1"]));
    management_token.token.expires_at_unix_secs = Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_secs()
            + 3_600,
    );
    management_token.token.permissions = Some(json!(all_assignable_management_token_permissions()));
    let legacy_raw_token = "ae-management-legacy-full-access";
    let mut legacy_management_token = sample_management_token(
        "mt-admin-legacy-full",
        &admin_user.id,
        "management-legacy-full",
        true,
    );
    legacy_management_token.token.allowed_ips = Some(json!(["127.0.0.1"]));
    legacy_management_token.token.expires_at_unix_secs =
        management_token.token.expires_at_unix_secs;
    legacy_management_token.token.permissions = None;
    let management_token_repository =
        Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
            vec![management_token, legacy_management_token],
            vec![
                (
                    hash_management_token(raw_token),
                    "mt-admin-full".to_string(),
                ),
                (
                    hash_management_token(legacy_raw_token),
                    "mt-admin-legacy-full".to_string(),
                ),
            ],
        ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(state.with_data_state_for_tests(
        GatewayDataState::with_management_token_repository_for_tests(Arc::clone(
            &management_token_repository,
        )),
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/management-tokens"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(raw_token)
        .json(&json!({
            "name": "unconstrained-child",
            "permissions": all_assignable_management_token_permissions(),
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.expect("body should read");
    assert_eq!(status, StatusCode::FORBIDDEN, "body={body}");
    let payload: serde_json::Value = serde_json::from_str(&body).expect("json body should parse");
    assert_eq!(payload["detail"], "management token permission denied");
    assert_eq!(
        payload["required_permission"],
        "admin:management_tokens:admin"
    );

    let legacy_response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/management-tokens"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(legacy_raw_token)
        .json(&json!({
            "name": "unconstrained-legacy-child",
            "permissions": all_assignable_management_token_permissions(),
        }))
        .send()
        .await
        .expect("legacy request should succeed");
    let legacy_status = legacy_response.status();
    let legacy_body = legacy_response.text().await.expect("body should read");
    assert_eq!(legacy_status, StatusCode::FORBIDDEN, "body={legacy_body}");
    let legacy_payload: serde_json::Value =
        serde_json::from_str(&legacy_body).expect("json body should parse");
    assert_eq!(
        legacy_payload["detail"],
        "management token permission denied"
    );
    assert_eq!(
        legacy_payload["required_permission"],
        "admin:management_tokens:admin"
    );

    let tokens = management_token_repository
        .list_management_tokens(&ManagementTokenListQuery {
            user_id: None,
            is_active: None,
            offset: 0,
            limit: 10,
        })
        .await
        .expect("management token list should succeed");
    assert_eq!(
        tokens.total, 2,
        "unconstrained children must not be created"
    );
    let mut token_ids = tokens
        .items
        .iter()
        .map(|item| item.token.id.as_str())
        .collect::<Vec<_>>();
    token_ids.sort_unstable();
    assert_eq!(token_ids, vec!["mt-admin-full", "mt-admin-legacy-full"]);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    drop(upstream_url);
}
