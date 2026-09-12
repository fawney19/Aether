use std::sync::Arc;

use aether_data::repository::management_tokens::{
    InMemoryManagementTokenRepository, StoredManagementToken, StoredManagementTokenUserSummary,
    StoredManagementTokenWithUser,
};
use base64::Engine as _;
use hmac::Mac;
use sha2::{Digest, Sha256};

use super::{
    build_router_with_state, send_request, start_server, AppState, Body, Request, StatusCode,
    OPERATIONAL_ADMIN_DEVICE_ID,
};
use crate::data::GatewayDataState;

fn hash_token(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

async fn issue_operational_session_access_token(
    state: &AppState,
    client_device_id: &str,
    role: &str,
) -> String {
    issue_operational_session_access_token_and_user(state, client_device_id, role)
        .await
        .0
}

async fn issue_operational_session_access_token_and_user(
    state: &AppState,
    client_device_id: &str,
    role: &str,
) -> (String, aether_data::repository::users::StoredUserAuthRecord) {
    let user = state
        .create_local_auth_user_with_settings(
            Some(format!("operational-{role}@example.com")),
            true,
            format!("operational_{role}"),
            "hash".to_string(),
            role.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("operational user should be created")
        .expect("operational user should exist");
    let now = chrono::Utc::now();
    let session_id = format!("session-operational-{role}");
    let refresh_token = format!("refresh-{session_id}");
    let session = crate::data::state::StoredUserSessionRecord::new(
        session_id.clone(),
        user.id.clone(),
        client_device_id.to_string(),
        None,
        crate::data::state::StoredUserSessionRecord::hash_refresh_token(&refresh_token),
        None,
        None,
        Some(now),
        Some(now + chrono::Duration::days(7)),
        None,
        None,
        Some("127.0.0.1".to_string()),
        Some("operational-auth-test".to_string()),
        Some(now),
        Some(now),
    )
    .expect("session should build");
    state
        .create_user_session(session)
        .await
        .expect("session should persist")
        .expect("session should exist");

    let header = serde_json::json!({ "alg": "HS256", "typ": "JWT" });
    let payload = serde_json::json!({
        "user_id": user.id,
        "role": role,
        "created_at": user.created_at.map(|value| value.to_rfc3339()),
        "session_id": session_id,
        "exp": (now + chrono::Duration::hours(12)).timestamp(),
        "type": "access",
    });
    let header_segment = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&header)
            .expect("jwt header should serialize")
            .as_slice(),
    );
    let payload_segment = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&payload)
            .expect("jwt payload should serialize")
            .as_slice(),
    );
    let signing_input = format!("{header_segment}.{payload_segment}");
    let secret = std::env::var("JWT_SECRET_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "aether-rust-test-jwt-secret-32-bytes-minimum".to_string());
    let mut mac =
        hmac::Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("jwt secret should build");
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();
    (
        format!(
            "{signing_input}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_slice())
        ),
        user,
    )
}

fn management_token(
    token_id: &str,
    user_id: &str,
    permissions: &[&str],
) -> StoredManagementTokenWithUser {
    let token = StoredManagementToken::new(
        token_id.to_string(),
        user_id.to_string(),
        token_id.to_string(),
    )
    .expect("management token should build")
    .with_permissions(Some(serde_json::json!(permissions)))
    .with_runtime_fields(Some(4_102_444_800), None, None, 0, true);
    let user = StoredManagementTokenUserSummary::new(
        user_id.to_string(),
        Some("operational-admin@example.com".to_string()),
        "operational_admin".to_string(),
        "admin".to_string(),
    )
    .expect("management token user should build");
    StoredManagementTokenWithUser::new(token, user)
}

async fn state_with_management_tokens(
    tokens: Vec<(&'static str, StoredManagementTokenWithUser)>,
) -> AppState {
    state_with_management_tokens_for_role(tokens, "admin").await
}

async fn state_with_management_tokens_for_role(
    tokens: Vec<(&'static str, StoredManagementTokenWithUser)>,
    role: &str,
) -> AppState {
    let state = AppState::new().expect("gateway state should build");
    let user = state
        .create_local_auth_user_with_settings(
            Some("operational-admin@example.com".to_string()),
            true,
            "operational_admin".to_string(),
            "hash".to_string(),
            role.to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("admin user should be created")
        .expect("admin user should exist");
    let items = tokens
        .iter()
        .map(|(_, token)| {
            let mut token = token.clone();
            token.token.user_id = user.id.clone();
            token.user.id = user.id.clone();
            token
        })
        .collect::<Vec<_>>();
    let hashes = tokens
        .into_iter()
        .zip(items.iter())
        .map(|((raw, _), token)| (hash_token(raw), token.token.id.clone()))
        .collect::<Vec<_>>();
    let repository = Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
        items, hashes,
    ));
    state.with_data_state_for_tests(
        GatewayDataState::with_management_token_repository_for_tests(repository),
    )
}

#[tokio::test]
async fn operational_session_requires_matching_device_id() {
    let state = AppState::new().expect("gateway state should build");
    let access_token =
        super::control::issue_shared_test_admin_access_token(&state, OPERATIONAL_ADMIN_DEVICE_ID)
            .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/_gateway/metrics"))
        .bearer_auth(access_token)
        .header("x-client-device-id", "different-device")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    gateway_handle.abort();
}

#[tokio::test]
async fn operational_session_rejects_a_stale_user_security_version() {
    let state = AppState::new().expect("gateway state should build");
    let (access_token, user) = issue_operational_session_access_token_and_user(
        &state,
        OPERATIONAL_ADMIN_DEVICE_ID,
        "admin",
    )
    .await;
    let user = user
        .with_security_version(1)
        .expect("admin security version should update");
    let state = state.with_auth_users_for_tests([user]);
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/_gateway/metrics"))
        .bearer_auth(access_token)
        .header("x-client-device-id", OPERATIONAL_ADMIN_DEVICE_ID)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    gateway_handle.abort();
}

#[tokio::test]
async fn operational_session_rejects_a_replaced_user_identity() {
    let state = AppState::new().expect("gateway state should build");
    let (access_token, mut user) = issue_operational_session_access_token_and_user(
        &state,
        OPERATIONAL_ADMIN_DEVICE_ID,
        "admin",
    )
    .await;
    user.created_at = user
        .created_at
        .map(|created_at| created_at + chrono::Duration::days(1));
    let state = state.with_auth_users_for_tests([user]);
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/_gateway/metrics"))
        .bearer_auth(access_token)
        .header("x-client-device-id", OPERATIONAL_ADMIN_DEVICE_ID)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    gateway_handle.abort();
}

#[tokio::test]
async fn operational_routes_reject_duplicate_authorization_headers() {
    let raw_token = "ae-operational-duplicate-authorization";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-duplicate-authorization",
            "placeholder-user",
            &["admin:monitoring:read"],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let mut request = Request::builder()
        .uri("/_gateway/metrics")
        .body(Body::empty())
        .expect("request should build");
    request.headers_mut().append(
        http::header::AUTHORIZATION,
        format!("Bearer {raw_token}")
            .parse()
            .expect("authorization should parse"),
    );
    request.headers_mut().append(
        http::header::AUTHORIZATION,
        "Bearer ae-attacker-selected-token"
            .parse()
            .expect("authorization should parse"),
    );

    let response = send_request(gateway, request).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
}

#[tokio::test]
async fn monitoring_management_token_cannot_read_video_tasks() {
    let raw_token = "ae-operational-monitoring-read";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-monitoring-read",
            "placeholder-user",
            &["admin:monitoring:read"],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let metrics = client
        .get(format!("{gateway_url}/_gateway/metrics"))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("metrics request should succeed");
    assert_eq!(metrics.status(), StatusCode::OK);
    assert_eq!(
        metrics
            .headers()
            .get(http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );

    let video_tasks = client
        .get(format!("{gateway_url}/_gateway/async-tasks/video-tasks"))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("video task request should succeed");
    assert_eq!(video_tasks.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = video_tasks.json().await.expect("response should parse");
    assert_eq!(payload["required_permission"], "admin:video_tasks:read");

    gateway_handle.abort();
}

#[tokio::test]
async fn monitoring_read_management_token_cannot_read_request_candidate_traces() {
    let raw_token = "ae-operational-candidate-monitoring-read";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-candidate-monitoring-read",
            "placeholder-user",
            &["admin:monitoring:read"],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for path in [
        "/_gateway/audit/request-candidates/not-found",
        "/_gateway/audit/decision-trace/not-found",
    ] {
        let response = client
            .get(format!("{gateway_url}{path}"))
            .bearer_auth(raw_token)
            .send()
            .await
            .expect("candidate trace request should succeed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "path: {path}");
        let payload: serde_json::Value = response.json().await.expect("response should parse");
        assert_eq!(
            payload["required_permission"], "admin:monitoring:admin",
            "path: {path}"
        );
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn monitoring_admin_management_token_may_reach_request_candidate_trace_handlers() {
    let raw_token = "ae-operational-candidate-monitoring-admin";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-candidate-monitoring-admin",
            "placeholder-user",
            &["admin:monitoring:admin"],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for path in [
        "/_gateway/audit/request-candidates/not-found",
        "/_gateway/audit/decision-trace/not-found",
    ] {
        let response = client
            .get(format!("{gateway_url}{path}"))
            .bearer_auth(raw_token)
            .send()
            .await
            .expect("candidate trace request should succeed");

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "path: {path}");
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn usage_management_token_cannot_read_audit_bundle_candidate_trace() {
    let raw_token = "ae-operational-usage-read";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-usage-read",
            "placeholder-user",
            &["admin:usage:read"],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/request-audit/not-found"
        ))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("audit bundle request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("response should parse");
    assert_eq!(payload["required_permission"], "admin:monitoring:admin");
    gateway_handle.abort();
}

#[tokio::test]
async fn usage_and_api_key_management_token_cannot_read_audit_bundle_candidate_trace() {
    let raw_token = "ae-operational-audit-bundle-read";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-audit-bundle-read",
            "placeholder-user",
            &["admin:usage:read", "admin:api_keys:read"],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/request-audit/not-found"
        ))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("audit bundle request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("response should parse");
    assert_eq!(payload["required_permission"], "admin:monitoring:admin");
    gateway_handle.abort();
}

#[tokio::test]
async fn three_permission_management_token_may_reach_audit_bundle_handler() {
    let raw_token = "ae-operational-audit-bundle-full-read";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-audit-bundle-full-read",
            "placeholder-user",
            &[
                "admin:monitoring:admin",
                "admin:usage:read",
                "admin:api_keys:read",
            ],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/_gateway/audit/request-audit/not-found"
        ))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("audit bundle request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    gateway_handle.abort();
}

#[tokio::test]
async fn downgraded_audit_admin_management_token_cannot_read_full_admin_audit_routes() {
    let raw_token = "ae-operational-downgraded-audit-admin";
    let state = state_with_management_tokens_for_role(
        vec![(
            raw_token,
            management_token(
                "operational-downgraded-audit-admin",
                "placeholder-user",
                &[
                    "admin:monitoring:admin",
                    "admin:usage:read",
                    "admin:api_keys:read",
                ],
            ),
        )],
        "audit_admin",
    )
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for path in [
        "/_gateway/audit/request-candidates/not-found",
        "/_gateway/audit/decision-trace/not-found",
        "/_gateway/audit/request-audit/not-found",
    ] {
        let response = client
            .get(format!("{gateway_url}{path}"))
            .bearer_auth(raw_token)
            .send()
            .await
            .expect("full-admin audit request should complete");

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "path: {path}");
        let payload: serde_json::Value = response.json().await.expect("response should parse");
        assert_eq!(
            payload["required_permission"], "admin:monitoring:admin",
            "path: {path}"
        );
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn audit_admin_session_cannot_read_candidate_audit_routes() {
    let state = AppState::new().expect("gateway state should build");
    let access_token =
        issue_operational_session_access_token(&state, OPERATIONAL_ADMIN_DEVICE_ID, "audit_admin")
            .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let usage = client
        .get(format!(
            "{gateway_url}/_gateway/audit/request-usage/not-found"
        ))
        .bearer_auth(&access_token)
        .header("x-client-device-id", OPERATIONAL_ADMIN_DEVICE_ID)
        .send()
        .await
        .expect("usage audit request should succeed");
    assert_eq!(usage.status(), StatusCode::NOT_FOUND);

    for path in [
        "/_gateway/audit/request-candidates/not-found",
        "/_gateway/audit/decision-trace/not-found",
        "/_gateway/audit/request-audit/not-found",
    ] {
        let response = client
            .get(format!("{gateway_url}{path}"))
            .bearer_auth(&access_token)
            .header("x-client-device-id", OPERATIONAL_ADMIN_DEVICE_ID)
            .send()
            .await
            .expect("candidate audit request should succeed");

        assert_eq!(response.status(), StatusCode::FORBIDDEN, "path: {path}");
        let payload: serde_json::Value = response.json().await.expect("response should parse");
        assert_eq!(
            payload["required_permission"], "admin:monitoring:admin",
            "path: {path}"
        );
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn malformed_management_token_ip_rules_fail_closed() {
    let raw_token = "ae-operational-malformed-ip-rules";
    let mut stored = management_token(
        "operational-malformed-ip-rules",
        "placeholder-user",
        &["admin:monitoring:read"],
    );
    stored.token.allowed_ips = Some(serde_json::json!(["!not-an-ip"]));
    let state = state_with_management_tokens(vec![(raw_token, stored)]).await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/_gateway/metrics"))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("metrics request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    gateway_handle.abort();
}

#[tokio::test]
async fn json_null_management_token_permissions_fail_closed() {
    let raw_token = "ae-operational-json-null-permissions";
    let mut stored = management_token(
        "operational-json-null-permissions",
        "placeholder-user",
        &["admin:monitoring:read"],
    );
    stored.token.permissions = Some(serde_json::Value::Null);
    let state = state_with_management_tokens(vec![(raw_token, stored)]).await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/_gateway/metrics"))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("metrics request should complete");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    gateway_handle.abort();
}

#[tokio::test]
async fn video_read_management_token_cannot_cancel_tasks() {
    let raw_token = "ae-operational-video-read";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-video-read",
            "placeholder-user",
            &["admin:video_tasks:read"],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/_gateway/async-tasks/video-tasks/not-found/cancel"
        ))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("cancel request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("response should parse");
    assert_eq!(payload["required_permission"], "admin:video_tasks:write");
    gateway_handle.abort();
}

#[tokio::test]
async fn video_admin_management_token_may_reach_cancel_handler() {
    let raw_token = "ae-operational-video-admin";
    let state = state_with_management_tokens(vec![(
        raw_token,
        management_token(
            "operational-video-admin",
            "placeholder-user",
            &["admin:video_tasks:admin"],
        ),
    )])
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/_gateway/async-tasks/video-tasks/not-found/cancel"
        ))
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("cancel request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    gateway_handle.abort();
}
