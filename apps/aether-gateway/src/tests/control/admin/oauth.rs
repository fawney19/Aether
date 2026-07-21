use std::sync::{Arc, Mutex};

use aether_contracts::{
    ExecutionPlan, EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
};
use aether_crypto::{
    decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY,
};
use aether_data::repository::management_tokens::{
    InMemoryManagementTokenRepository, ManagementTokenReadRepository,
};
use aether_data::repository::oauth_providers::{
    InMemoryOAuthProviderRepository, OAuthProviderReadRepository,
};
use aether_data::repository::pool_scores::InMemoryPoolMemberScoreRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::proxy_nodes::InMemoryProxyNodeRepository;
use aether_data_contracts::repository::pool_scores::{
    GetPoolMemberScoresByIdsQuery, PoolMemberHardState, PoolMemberIdentity, PoolScoreReadRepository,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, ProviderCatalogWriteRepository, StoredProviderCatalogEndpoint,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, patch, post, put};
use axum::{extract::Request, Json, Router};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;

use super::super::{
    build_router_with_state, build_state_with_execution_runtime_override, hash_management_token,
    sample_endpoint, sample_key, sample_management_token, sample_oauth_provider_config,
    sample_provider, sample_proxy_node, start_server, AppState,
};
use crate::admin_api::{
    maybe_build_local_admin_provider_oauth_response, AdminAppState, AdminRequestContext,
};
use crate::ai_serving::{provider_key_pool_score_id, provider_key_pool_score_scope};
use crate::audit::AdminAuditEvent;
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER,
    TRUSTED_ADMIN_USER_ID_HEADER, TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::control::resolve_public_request_context;
use crate::data::GatewayDataState;

const ADMIN_OAUTH_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_admin_oauth_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(ADMIN_OAUTH_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("admin oauth test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn trusted_admin_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(GATEWAY_HEADER, HeaderValue::from_static("rust-phase3b"));
    headers.insert(
        TRUSTED_ADMIN_USER_ID_HEADER,
        HeaderValue::from_static("admin-user-123"),
    );
    headers.insert(
        TRUSTED_ADMIN_USER_ROLE_HEADER,
        HeaderValue::from_static("admin"),
    );
    headers.insert(
        TRUSTED_ADMIN_SESSION_ID_HEADER,
        HeaderValue::from_static("session-123"),
    );
    headers.insert(
        TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER,
        HeaderValue::from_static("management-token-123"),
    );
    headers
}

async fn local_admin_provider_oauth_response(
    state: &AppState,
    method: http::Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> Response<Body> {
    let headers = trusted_admin_headers();
    let request_context = resolve_public_request_context(
        state,
        &method,
        &uri.parse().expect("uri should parse"),
        &headers,
        "trace-123",
    )
    .await
    .expect("request context should resolve");
    let body_bytes = body.map(|value| Bytes::from(value.to_string()));
    maybe_build_local_admin_provider_oauth_response(
        &AdminAppState::new(state),
        &AdminRequestContext::new(&request_context),
        body_bytes.as_ref(),
    )
    .await
    .expect("local provider oauth response should build")
    .expect("provider oauth route should resolve locally")
}

fn sample_kiro_device_access_token(email: &str) -> String {
    use base64::Engine as _;

    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        json!({
            "email": email,
            "sub": "kiro-user-123",
        })
        .to_string(),
    );
    format!("{header}.{payload}.sig")
}

fn sample_kiro_device_access_token_without_email() -> String {
    use base64::Engine as _;

    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        json!({
            "sub": "kiro-user-123",
        })
        .to_string(),
    );
    format!("{header}.{payload}.sig")
}

fn sample_codex_access_token_with_profile_email(email: &str, account_id: &str) -> String {
    use base64::Engine as _;

    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        json!({
            "iss": "https://auth.openai.com",
            "aud": ["https://api.openai.com/v1"],
            "exp": 2_000_000_000u64,
            "https://api.openai.com/profile": {
                "email": email,
                "email_verified": true,
            },
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
            },
        })
        .to_string(),
    );
    format!("{header}.{payload}.sig")
}

fn codex_import_token_execution_result(request_id: &str) -> serde_json::Value {
    json!({
        "request_id": request_id,
        "status_code": 200,
        "headers": {
            "content-type": "application/json"
        },
        "body": {
            "json_body": {
                "access_token": "imported-codex-access-token",
                "refresh_token": "imported-codex-refresh-token",
                "token_type": "Bearer",
                "expires_in": 1800,
                "scope": "openid email profile offline_access",
                "email": "alice@example.com",
                "account_id": "acct-codex-123",
                "plan_type": "plus"
            }
        }
    })
}

fn codex_quota_execution_result(request_id: &str) -> serde_json::Value {
    json!({
        "request_id": request_id,
        "status_code": 200,
        "headers": {},
        "body": {
            "json_body": {
                "user": {
                    "id": "user-codex-123",
                    "email": "alice@example.com"
                },
                "account": {
                    "id": "acct-codex-123",
                    "plan_type": "plus"
                },
                "plan": {
                    "type": "Plus",
                    "title": "ChatGPT Plus"
                }
            }
        }
    })
}

fn windsurf_register_user_execution_result(request_id: &str) -> serde_json::Value {
    json!({
        "request_id": request_id,
        "status_code": 200,
        "headers": {
            "content-type": "application/json"
        },
        "body": {
            "json_body": {
                "sessionToken": "devin-session-token$registered",
                "name": "Windsurf User",
                "apiServerUrl": "https://server.codeium.com"
            }
        }
    })
}

fn assert_single_provider_oauth_refresh_token_plan<'a>(
    plans: &'a [ExecutionPlan],
) -> &'a ExecutionPlan {
    let token_plans = plans
        .iter()
        .filter(|plan| plan.request_id == "provider-oauth:refresh-token")
        .collect::<Vec<_>>();
    assert_eq!(
        token_plans.len(),
        1,
        "expected exactly one provider-oauth refresh-token plan, got {:?}",
        plans
            .iter()
            .map(|plan| plan.request_id.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        plans.iter().all(|plan| {
            plan.request_id == "provider-oauth:refresh-token"
                || plan.request_id.starts_with("codex-quota:")
        }),
        "unexpected execution plans: {:?}",
        plans
            .iter()
            .map(|plan| plan.request_id.as_str())
            .collect::<Vec<_>>()
    );
    token_plans[0]
}

#[test]
fn gateway_handles_admin_provider_oauth_supported_types_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_supported_types_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_oauth_supported_types_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_supported_types_locally_with_trusted_admin_principal_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-oauth/supported-types",
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
            "{gateway_url}/api/admin/provider-oauth/supported-types"
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
    let items = payload.as_array().expect("items should be array");
    assert_eq!(items.len(), 6);
    assert_eq!(items[0]["provider_type"], "claude_code");
    assert_eq!(items[1]["provider_type"], "codex");
    assert_eq!(items[2]["provider_type"], "chatgpt_web");
    assert_eq!(items[3]["provider_type"], "gemini_cli");
    assert_eq!(items[4]["provider_type"], "antigravity");
    assert_eq!(items[5]["provider_type"], "windsurf");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_oauth_device_authorize_for_windsurf_browser() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_device_authorize_for_windsurf_browser",
        gateway_handles_admin_provider_oauth_device_authorize_for_windsurf_browser_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_device_authorize_for_windsurf_browser_impl() {
    let mut provider = sample_provider("provider-windsurf", "windsurf", 10);
    provider.provider_type = "windsurf".to_string();
    let endpoint = sample_endpoint(
        "endpoint-windsurf-chat",
        "provider-windsurf",
        "openai:chat",
        "https://server.codeium.com",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_provider_oauth_response(
        &state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-windsurf/device-authorize",
        Some(json!({
            "auth_type": "browser",
            "login_option": "github",
            "proxy_node_id": "proxy-node-windsurf"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    let session_id = payload["session_id"]
        .as_str()
        .expect("session_id should exist");
    assert_eq!(payload["auth_type"], "browser");
    assert_eq!(payload["login_option"], "github");
    assert_eq!(payload["redirect_uri"], "show-auth-token");
    assert_eq!(payload["callback_required"], true);

    let authorization_url = payload["verification_uri_complete"]
        .as_str()
        .expect("authorization url should exist");
    let parsed = url::Url::parse(authorization_url).expect("authorization url should parse");
    let params = parsed
        .query_pairs()
        .into_owned()
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        parsed.as_str().split('?').next(),
        Some("https://windsurf.com/windsurf/signin")
    );
    assert_eq!(
        params.get("response_type").map(String::as_str),
        Some("token")
    );
    assert_eq!(params.get("state").map(String::as_str), Some(session_id));
    assert_eq!(
        params.get("redirect_uri").map(String::as_str),
        Some("show-auth-token")
    );
    assert_eq!(
        params.get("redirect_parameters_type").map(String::as_str),
        Some("query")
    );

    let stored = state
        .load_provider_oauth_device_session_for_tests(&format!("device_auth_session:{session_id}"))
        .expect("device session should be stored");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["provider_id"], "provider-windsurf");
    assert_eq!(stored["auth_type"], "browser");
    assert_eq!(stored["social_provider"], "github");
    assert_eq!(stored["redirect_uri"], "show-auth-token");
    assert_eq!(stored["proxy_node_id"], "proxy-node-windsurf");
    assert_eq!(stored["status"], "pending");
}

#[test]
fn gateway_rejects_generic_oauth_start_for_windsurf_provider() {
    run_admin_oauth_test(
        "gateway_rejects_generic_oauth_start_for_windsurf_provider",
        gateway_rejects_generic_oauth_start_for_windsurf_provider_impl,
    );
}

async fn gateway_rejects_generic_oauth_start_for_windsurf_provider_impl() {
    let mut provider = sample_provider("provider-windsurf", "windsurf", 10);
    provider.provider_type = "windsurf".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_provider_oauth_response(
        &state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-windsurf/start",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert!(
        payload["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("浏览器登录")),
        "payload={payload}"
    );
}

#[test]
fn gateway_handles_admin_provider_oauth_device_poll_for_windsurf_one_time_token() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_device_poll_for_windsurf_one_time_token",
        gateway_handles_admin_provider_oauth_device_poll_for_windsurf_one_time_token_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_device_poll_for_windsurf_one_time_token_impl() {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                if plan.request_id == "provider-oauth:windsurf-register:new" {
                    return Json(windsurf_register_user_execution_result(&plan.request_id));
                }
                Json(json!({
                    "request_id": plan.request_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {}
                    }
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-windsurf", "windsurf", 10);
    provider.provider_type = "windsurf".to_string();
    let endpoint = sample_endpoint(
        "endpoint-windsurf-chat",
        "provider-windsurf",
        "openai:chat",
        "https://server.codeium.com",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));
    let mut proxy_node = sample_proxy_node("proxy-node-windsurf");
    proxy_node.status = "online".to_string();
    proxy_node.is_manual = true;
    proxy_node.tunnel_mode = false;
    proxy_node.tunnel_connected = false;
    proxy_node.proxy_url = Some("http://proxy.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![proxy_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository.clone(),
            )
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
            .attach_proxy_node_repository_for_tests(proxy_node_repository),
        )
        .with_provider_oauth_device_session_entry_for_tests(
            "session-windsurf",
            json!({
                "provider_id": "provider-windsurf",
                "region": "",
                "client_id": "",
                "client_secret": "",
                "device_code": "",
                "auth_type": "browser",
                "social_provider": "google",
                "code_verifier": null,
                "redirect_uri": "show-auth-token",
                "machine_id": "123e4567-e89b-12d3-a456-426614174000",
                "interval": 5,
                "expires_at_unix_secs": 4_102_444_800u64,
                "status": "pending",
                "proxy_node_id": "proxy-node-windsurf",
                "created_at_unix_ms": 1_711_000_000u64,
                "key_id": null,
                "email": null,
                "replaced": false,
                "error_msg": null,
            }),
        );

    let response = local_admin_provider_oauth_response(
        &state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-windsurf/device-poll",
        Some(json!({
            "session_id": "session-windsurf",
            "token": "ott$browser-token"
        })),
    )
    .await;
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["status"], "authorized");
    assert_eq!(payload["replaced"], false);

    let stored = state
        .load_provider_oauth_device_session_for_tests("device_auth_session:session-windsurf")
        .expect("device session should persist");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["status"], "authorized");
    let key_id = stored["key_id"]
        .as_str()
        .expect("key_id should be stored")
        .to_string();
    assert_eq!(payload["key_id"], key_id);

    let persisted = provider_catalog_repository
        .list_keys_by_ids(std::slice::from_ref(&key_id))
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("persisted key should exist");
    assert_eq!(persisted.auth_type, "oauth");
    assert_eq!(
        persisted.proxy,
        Some(json!({"node_id": "proxy-node-windsurf", "enabled": true}))
    );
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "devin-session-token$registered");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["provider_type"], "windsurf");
    assert_eq!(auth_config["auth_method"], "browser");
    assert_eq!(auth_config["register_source"], "new");
    assert_eq!(auth_config["social_provider"], "google");

    {
        let plans = execution_plans.lock().expect("mutex should lock");
        let register_plan = plans
            .iter()
            .find(|plan| plan.request_id == "provider-oauth:windsurf-register:new")
            .expect("register plan should execute");
        assert_eq!(register_plan.method, "POST");
        assert_eq!(
            register_plan.content_type.as_deref(),
            Some("application/proto")
        );
        assert!(register_plan.body.json_body.is_none());
        let encoded_body = register_plan
            .body
            .body_bytes_b64
            .as_deref()
            .expect("register body should be bytes");
        use base64::Engine as _;
        let body_bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded_body)
            .expect("register body should decode");
        let mut expected_body = vec![0x0a, "ott$browser-token".len() as u8];
        expected_body.extend_from_slice(b"ott$browser-token");
        assert_eq!(body_bytes, expected_body);
        assert_eq!(
            register_plan
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.node_id.as_deref()),
            Some("proxy-node-windsurf")
        );
    }

    execution_runtime_handle.abort();
}

#[test]
fn gateway_rejects_windsurf_callback_state_mismatch_and_missing_token() {
    run_admin_oauth_test(
        "gateway_rejects_windsurf_callback_state_mismatch_and_missing_token",
        gateway_rejects_windsurf_callback_state_mismatch_and_missing_token_impl,
    );
}

async fn gateway_rejects_windsurf_callback_state_mismatch_and_missing_token_impl() {
    let mut provider = sample_provider("provider-windsurf", "windsurf", 10);
    provider.provider_type = "windsurf".to_string();
    let endpoint = sample_endpoint(
        "endpoint-windsurf-chat",
        "provider-windsurf",
        "openai:chat",
        "https://server.codeium.com",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ))
        .with_provider_oauth_device_session_entry_for_tests(
            "session-windsurf",
            json!({
                "provider_id": "provider-windsurf",
                "region": "",
                "client_id": "",
                "client_secret": "",
                "device_code": "",
                "auth_type": "browser",
                "social_provider": "google",
                "code_verifier": null,
                "redirect_uri": "show-auth-token",
                "machine_id": "123e4567-e89b-12d3-a456-426614174000",
                "interval": 5,
                "expires_at_unix_secs": 4_102_444_800u64,
                "status": "pending",
                "proxy_node_id": null,
                "created_at_unix_ms": 1_711_000_000u64,
                "key_id": null,
                "email": null,
                "replaced": false,
                "error_msg": null,
            }),
        );

    let response = local_admin_provider_oauth_response(
        &state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-windsurf/device-poll",
        Some(json!({
            "session_id": "session-windsurf",
            "callback_url": "https://windsurf.com/show-auth-token?token=ott$wrong-state&state=wrong-state"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], "error");
    assert!(payload["error"]
        .as_str()
        .is_some_and(|error| error.contains("state")));

    let response = local_admin_provider_oauth_response(
        &state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-windsurf/device-poll",
        Some(json!({
            "session_id": "session-windsurf",
            "callback_url": "https://windsurf.com/show-auth-token?state=session-windsurf"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], "error");
    assert!(payload["error"]
        .as_str()
        .is_some_and(|error| error.contains("token")));
}

#[test]
fn gateway_handles_admin_provider_oauth_device_authorize_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_device_authorize_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_oauth_device_authorize_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_device_authorize_locally_with_trusted_admin_principal_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let register_hits = Arc::new(Mutex::new(0usize));
    let register_hits_clone = Arc::clone(&register_hits);
    let authorize_hits = Arc::new(Mutex::new(0usize));
    let authorize_hits_clone = Arc::clone(&authorize_hits);
    let oidc_server = Router::new()
        .route(
            "/client/register",
            post(move |_request: Request| {
                let register_hits_inner = Arc::clone(&register_hits_clone);
                async move {
                    *register_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({
                        "clientId": "kiro-device-client",
                        "clientSecret": "kiro-device-secret",
                    }))
                }
            }),
        )
        .route(
            "/device_authorization",
            post(move |_request: Request| {
                let authorize_hits_inner = Arc::clone(&authorize_hits_clone);
                async move {
                    *authorize_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({
                        "deviceCode": "device-code-123",
                        "userCode": "USER-CODE",
                        "verificationUri": "https://device.example.com/verify",
                        "verificationUriComplete": "https://device.example.com/verify?user_code=USER-CODE",
                        "expiresIn": 600,
                        "interval": 5,
                    }))
                }
            }),
        );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (oidc_url, oidc_handle) = start_server(oidc_server).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ))
        .with_provider_oauth_device_session_entry_for_tests(
            "seed-session",
            json!({"status":"seed"}),
        )
        .with_provider_oauth_token_url_for_tests(
            "kiro_device_register",
            format!("{oidc_url}/client/register"),
        )
        .with_provider_oauth_token_url_for_tests(
            "kiro_device_authorize",
            format!("{oidc_url}/device_authorization"),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/device-authorize"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "start_url": "https://view.awsapps.com/start",
            "region": "us-east-1",
            "proxy_node_id": "proxy-node-kiro",
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    let session_id = payload["session_id"]
        .as_str()
        .expect("session_id should exist")
        .to_string();
    assert_eq!(payload["user_code"], "USER-CODE");
    assert_eq!(
        payload["verification_uri_complete"],
        "https://device.example.com/verify?user_code=USER-CODE"
    );
    assert_eq!(payload["expires_in"], 600);
    assert_eq!(payload["interval"], 5);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*register_hits.lock().expect("mutex should lock"), 1);
    assert_eq!(*authorize_hits.lock().expect("mutex should lock"), 1);

    let stored = state
        .load_provider_oauth_device_session_for_tests(&format!("device_auth_session:{session_id}"))
        .expect("device session should be stored");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["provider_id"], "provider-kiro");
    assert_eq!(stored["region"], "us-east-1");
    assert_eq!(stored["client_id"], "kiro-device-client");
    assert_eq!(stored["client_secret"], "kiro-device-secret");
    assert_eq!(stored["device_code"], "device-code-123");
    assert_eq!(stored["proxy_node_id"], "proxy-node-kiro");
    assert_eq!(stored["status"], "pending");

    gateway_handle.abort();
    oidc_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_oauth_device_authorize_for_kiro_google_social() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_device_authorize_for_kiro_google_social",
        gateway_handles_admin_provider_oauth_device_authorize_for_kiro_google_social_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_device_authorize_for_kiro_google_social_impl() {
    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ))
        .with_provider_oauth_token_url_for_tests(
            "kiro_social_portal",
            "https://portal.example.com/signin",
        );

    let response = local_admin_provider_oauth_response(
        &state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-kiro/device-authorize",
        Some(json!({
            "auth_type": "google"
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    let session_id = payload["session_id"]
        .as_str()
        .expect("session_id should exist");
    assert_eq!(payload["auth_type"], "google");
    assert_eq!(payload["callback_required"], true);
    assert_eq!(payload["redirect_uri"], "http://localhost:49153");

    let authorization_url = payload["verification_uri_complete"]
        .as_str()
        .expect("authorization url should exist");
    let parsed = url::Url::parse(authorization_url).expect("authorization url should parse");
    let params = parsed
        .query_pairs()
        .into_owned()
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        parsed.as_str().split('?').next(),
        Some("https://portal.example.com/signin")
    );
    assert_eq!(
        params.get("redirect_uri").map(String::as_str),
        Some("http://localhost:49153")
    );
    assert_eq!(params.get("state").map(String::as_str), Some(session_id));
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert_eq!(
        params.get("redirect_from").map(String::as_str),
        Some("KiroIDE")
    );
    assert_eq!(
        params.get("login_option").map(String::as_str),
        Some("google")
    );
    assert!(params
        .get("code_challenge")
        .is_some_and(|value| !value.is_empty()));

    let stored = state
        .load_provider_oauth_device_session_for_tests(&format!("device_auth_session:{session_id}"))
        .expect("device session should be stored");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["provider_id"], "provider-kiro");
    assert_eq!(stored["auth_type"], "social");
    assert_eq!(stored["social_provider"], "Google");
    assert_eq!(stored["redirect_uri"], "http://localhost:49153");
    assert!(stored["code_verifier"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert!(stored["machine_id"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(stored["status"], "pending");
}

#[test]
fn gateway_handles_admin_provider_oauth_device_poll_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_device_poll_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_oauth_device_poll_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_device_poll_locally_with_trusted_admin_principal_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let access_token = sample_kiro_device_access_token("kiro@example.com");
    let expected_access_token = access_token.clone();
    let token_server = Router::new().route(
        "/token",
        post(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            let access_token_inner = access_token.clone();
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "accessToken": access_token_inner,
                    "refreshToken": "kiro-device-refresh-token",
                    "expiresIn": 1800,
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository.clone(),
            )
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_provider_oauth_device_session_entry_for_tests(
            "session-123",
            json!({
                "provider_id": "provider-kiro",
                "region": "us-east-1",
                "client_id": "kiro-device-client",
                "client_secret": "kiro-device-secret",
                "device_code": "device-code-123",
                "interval": 5,
                "expires_at_unix_secs": 4_102_444_800u64,
                "status": "pending",
                "proxy_node_id": "proxy-node-kiro",
                "created_at_unix_ms": 1_711_000_000u64,
                "key_id": null,
                "email": null,
                "replaced": false,
                "error_msg": null,
            }),
        )
        .with_provider_oauth_token_url_for_tests("kiro_device_poll", format!("{token_url}/token"))
        .with_provider_oauth_token_url_for_tests("kiro_idc_refresh", token_url.to_string());
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/device-poll"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "session_id": "session-123"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["status"], "authorized");
    assert_eq!(payload["email"], "kiro@example.com");
    assert_eq!(payload["replaced"], false);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 2);

    let stored = state
        .load_provider_oauth_device_session_for_tests("device_auth_session:session-123")
        .expect("device session should persist");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["status"], "authorized");
    assert_eq!(stored["email"], "kiro@example.com");
    assert_eq!(stored["replaced"], false);
    let key_id = stored["key_id"]
        .as_str()
        .expect("key_id should be stored")
        .to_string();
    assert_eq!(payload["key_id"], key_id);

    let persisted = provider_catalog_repository
        .list_keys_by_ids(std::slice::from_ref(&key_id))
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("persisted key should exist");
    assert_eq!(persisted.auth_type, "oauth");
    assert_eq!(
        persisted.proxy,
        Some(json!({"node_id": "proxy-node-kiro", "enabled": true}))
    );
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, expected_access_token);
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["provider_type"], "kiro");
    assert_eq!(auth_config["auth_method"], "idc");
    assert_eq!(auth_config["refresh_token"], "kiro-device-refresh-token");
    assert_eq!(auth_config["email"], "kiro@example.com");
    assert_eq!(auth_config["client_id"], "kiro-device-client");
    assert_eq!(auth_config["client_secret"], "kiro-device-secret");
    assert_eq!(auth_config["region"], "us-east-1");

    gateway_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_oauth_device_poll_for_kiro_social_callback() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_device_poll_for_kiro_social_callback",
        gateway_handles_admin_provider_oauth_device_poll_for_kiro_social_callback_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_device_poll_for_kiro_social_callback_impl() {
    let token_requests = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let token_requests_clone = Arc::clone(&token_requests);
    let access_token = sample_kiro_device_access_token("social@example.com");
    let expected_access_token = access_token.clone();
    let token_server = Router::new().route(
        "/oauth/token",
        post(move |request: Request| {
            let token_requests_inner = Arc::clone(&token_requests_clone);
            let access_token_inner = access_token.clone();
            async move {
                let user_agent = request
                    .headers()
                    .get("user-agent")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                let raw_body = String::from_utf8(
                    to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read")
                        .to_vec(),
                )
                .expect("body should be utf8");
                token_requests_inner
                    .lock()
                    .expect("mutex should lock")
                    .push((user_agent, raw_body));
                Json(json!({
                    "accessToken": access_token_inner,
                    "refreshToken": "kiro-social-refresh-token",
                    "profileArn": "arn:aws:kiro:profile/social",
                    "idToken": "id-token-123",
                    "tokenType": "Bearer",
                    "expiresIn": 1800,
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository.clone(),
            )
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_provider_oauth_device_session_entry_for_tests(
            "session-social",
            json!({
                "provider_id": "provider-kiro",
                "region": "us-east-1",
                "client_id": "",
                "client_secret": "",
                "device_code": "",
                "auth_type": "social",
                "social_provider": "Github",
                "code_verifier": "verifier-123",
                "redirect_uri": "http://localhost:49153",
                "machine_id": "123e4567-e89b-12d3-a456-426614174000",
                "interval": 5,
                "expires_at_unix_secs": 4_102_444_800u64,
                "status": "pending",
                "proxy_node_id": null,
                "created_at_unix_ms": 1_711_000_000u64,
                "key_id": null,
                "email": null,
                "replaced": false,
                "error_msg": null,
            }),
        )
        .with_provider_oauth_token_url_for_tests(
            "kiro_social_token",
            format!("{token_url}/oauth/token"),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/device-poll"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "session_id": "session-social",
            "callback_url": "http://localhost:49153/signin/callback?login_option=github&code=social-code-123&state=session-social"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["status"], "authorized");
    assert_eq!(payload["email"], "social@example.com");
    assert_eq!(payload["replaced"], false);

    {
        let requests = token_requests.lock().expect("mutex should lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].0,
            "KiroIDE-0.6.18-123e4567-e89b-12d3-a456-426614174000"
        );
        assert!(requests[0].1.contains("\"code\":\"social-code-123\""));
        assert!(requests[0].1.contains("\"code_verifier\":\"verifier-123\""));
        assert!(requests[0].1.contains(
            "\"redirect_uri\":\"http://localhost:49153/signin/callback?login_option=github\""
        ));
    }

    let stored = state
        .load_provider_oauth_device_session_for_tests("device_auth_session:session-social")
        .expect("device session should persist");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["status"], "authorized");
    assert_eq!(stored["email"], "social@example.com");
    let key_id = stored["key_id"]
        .as_str()
        .expect("key_id should be stored")
        .to_string();
    assert_eq!(payload["key_id"], key_id);

    let persisted = provider_catalog_repository
        .list_keys_by_ids(std::slice::from_ref(&key_id))
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("persisted key should exist");
    assert_eq!(persisted.name, "social@example.com (Github)");
    assert_eq!(persisted.auth_type, "oauth");
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, expected_access_token);
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["provider_type"], "kiro");
    assert_eq!(auth_config["provider"], "Github");
    assert_eq!(auth_config["auth_method"], "social");
    assert_eq!(auth_config["refresh_token"], "kiro-social-refresh-token");
    assert_eq!(auth_config["profile_arn"], "arn:aws:kiro:profile/social");
    assert_eq!(auth_config["email"], "social@example.com");
    assert_eq!(
        auth_config["machine_id"],
        "123e4567-e89b-12d3-a456-426614174000"
    );
    assert_eq!(auth_config["kiro_version"], "0.6.18");

    gateway_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_keeps_admin_provider_oauth_device_poll_pending_for_authorization_pending_error() {
    run_admin_oauth_test(
        "gateway_keeps_admin_provider_oauth_device_poll_pending_for_authorization_pending_error",
        gateway_keeps_admin_provider_oauth_device_poll_pending_for_authorization_pending_error_impl,
    );
}

async fn gateway_keeps_admin_provider_oauth_device_poll_pending_for_authorization_pending_error_impl(
) {
    let token_server = Router::new().route(
        "/token",
        post(move |_request: Request| async move {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "authorization_pending",
                    "error_description": "waiting for user confirmation",
                })),
            )
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ))
        .with_provider_oauth_device_session_entry_for_tests(
            "session-pending",
            json!({
                "provider_id": "provider-kiro",
                "region": "us-east-1",
                "client_id": "kiro-device-client",
                "client_secret": "kiro-device-secret",
                "device_code": "device-code-123",
                "interval": 5,
                "expires_at_unix_secs": 4_102_444_800u64,
                "status": "pending",
                "proxy_node_id": null,
                "created_at_unix_ms": 1_711_000_000u64,
                "key_id": null,
                "email": null,
                "replaced": false,
                "error_msg": null,
            }),
        )
        .with_provider_oauth_token_url_for_tests("kiro_device_poll", format!("{token_url}/token"));

    let response = local_admin_provider_oauth_response(
        &state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-kiro/device-poll",
        Some(json!({ "session_id": "session-pending" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.extensions().get::<AdminAuditEvent>().is_none(),
        "pending state should not attach terminal audit"
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], "pending");
    assert_eq!(payload["replaced"], false);

    let stored = state
        .load_provider_oauth_device_session_for_tests("device_auth_session:session-pending")
        .expect("device session should persist");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["status"], "pending");
    assert_eq!(stored["error_msg"], serde_json::Value::Null);

    token_handle.abort();
}

#[test]
fn gateway_revalidates_kiro_device_poll_via_idc_refresh_and_backfills_email() {
    run_admin_oauth_test(
        "gateway_revalidates_kiro_device_poll_via_idc_refresh_and_backfills_email",
        gateway_revalidates_kiro_device_poll_via_idc_refresh_and_backfills_email_impl,
    );
}

async fn gateway_revalidates_kiro_device_poll_via_idc_refresh_and_backfills_email_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let token_requests_clone = Arc::clone(&token_requests);
    let initial_access_token = sample_kiro_device_access_token_without_email();
    let refreshed_access_token = sample_kiro_device_access_token_without_email();
    let expected_refreshed_access_token = refreshed_access_token.clone();
    let token_server = Router::new().route(
        "/token",
        post(move |request: Request| {
            let token_requests_inner = Arc::clone(&token_requests_clone);
            let initial_access_token_inner = initial_access_token.clone();
            let refreshed_access_token_inner = refreshed_access_token.clone();
            async move {
                let raw_body = String::from_utf8(
                    to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read")
                        .to_vec(),
                )
                .expect("body should be utf8");
                token_requests_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(raw_body.clone());
                if raw_body.contains("urn:ietf:params:oauth:grant-type:device_code") {
                    return Json(json!({
                        "accessToken": initial_access_token_inner,
                        "refreshToken": "kiro-device-refresh-token-initial",
                        "expiresIn": 1800,
                    }))
                    .into_response();
                }
                if raw_body.contains("\"grantType\":\"refresh_token\"") {
                    return Json(json!({
                        "accessToken": refreshed_access_token_inner,
                        "refreshToken": "kiro-device-refresh-token-rotated",
                        "expiresIn": 2400,
                    }))
                    .into_response();
                }
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "unexpected_request",
                        "body": raw_body,
                    })),
                )
                    .into_response()
            }
        }),
    );

    let usage_hits = Arc::new(Mutex::new(0usize));
    let usage_hits_clone = Arc::clone(&usage_hits);
    let usage_server = Router::new().route(
        "/getUsageLimits",
        get(move |_request: Request| {
            let usage_hits_inner = Arc::clone(&usage_hits_clone);
            async move {
                *usage_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "subscriptionInfo": {
                        "subscriptionTitle": "KIRO PRO+"
                    },
                    "usageBreakdownList": [{
                        "currentUsageWithPrecision": 5.0,
                        "usageLimitWithPrecision": 20.0,
                        "nextDateReset": 1_900_000_000u64
                    }],
                    "desktopUserInfo": {
                        "email": "kiro-usage@example.com"
                    }
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let (usage_url, usage_handle) = start_server(usage_server).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository.clone(),
            )
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_provider_oauth_device_session_entry_for_tests(
            "session-refresh-email",
            json!({
                "provider_id": "provider-kiro",
                "region": "us-east-1",
                "client_id": "kiro-device-client",
                "client_secret": "kiro-device-secret",
                "device_code": "device-code-123",
                "interval": 5,
                "expires_at_unix_secs": 4_102_444_800u64,
                "status": "pending",
                "proxy_node_id": "proxy-node-kiro",
                "created_at_unix_ms": 1_711_000_000u64,
                "key_id": null,
                "email": null,
                "replaced": false,
                "error_msg": null,
            }),
        )
        .with_provider_oauth_token_url_for_tests("kiro_device_poll", format!("{token_url}/token"))
        .with_provider_oauth_token_url_for_tests("kiro_idc_refresh", token_url.to_string())
        .with_provider_oauth_token_url_for_tests(
            "kiro_device_email",
            format!("{usage_url}/getUsageLimits"),
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/device-poll"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "session_id": "session-refresh-email"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["status"], "authorized");
    assert_eq!(payload["email"], "kiro-usage@example.com");
    assert_eq!(payload["replaced"], false);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*usage_hits.lock().expect("mutex should lock"), 1);
    {
        let requests = token_requests.lock().expect("mutex should lock");
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .any(|body| body.contains("urn:ietf:params:oauth:grant-type:device_code")),
            "requests={requests:?}"
        );
        assert!(
            requests
                .iter()
                .any(|body| body.contains("\"grantType\":\"refresh_token\"")),
            "requests={requests:?}"
        );
    }

    let stored = state
        .load_provider_oauth_device_session_for_tests("device_auth_session:session-refresh-email")
        .expect("device session should persist");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["status"], "authorized");
    assert_eq!(stored["email"], "kiro-usage@example.com");
    assert_eq!(stored["replaced"], false);
    let key_id = stored["key_id"]
        .as_str()
        .expect("key_id should be stored")
        .to_string();
    assert_eq!(payload["key_id"], key_id);

    let persisted = provider_catalog_repository
        .list_keys_by_ids(std::slice::from_ref(&key_id))
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("persisted key should exist");
    assert_eq!(persisted.auth_type, "oauth");
    assert_eq!(
        persisted.proxy,
        Some(json!({"node_id": "proxy-node-kiro", "enabled": true}))
    );
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, expected_refreshed_access_token);
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["provider_type"], "kiro");
    assert_eq!(auth_config["auth_method"], "idc");
    assert_eq!(
        auth_config["refresh_token"],
        "kiro-device-refresh-token-rotated"
    );
    assert_eq!(auth_config["email"], "kiro-usage@example.com");
    assert_eq!(auth_config["client_id"], "kiro-device-client");
    assert_eq!(auth_config["client_secret"], "kiro-device-secret");
    assert_eq!(auth_config["region"], "us-east-1");

    gateway_handle.abort();
    usage_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn local_admin_provider_oauth_device_poll_attaches_audit_only_when_transition_reaches_terminal_state(
) {
    run_admin_oauth_test(
        "local_admin_provider_oauth_device_poll_attaches_audit_only_when_transition_reaches_terminal_state",
        local_admin_provider_oauth_device_poll_attaches_audit_only_when_transition_reaches_terminal_state_impl,
    );
}

async fn local_admin_provider_oauth_device_poll_attaches_audit_only_when_transition_reaches_terminal_state_impl(
) {
    let access_token = sample_kiro_device_access_token("kiro@example.com");
    let token_server = Router::new().route(
        "/token",
        post(move |_request: Request| {
            let access_token_inner = access_token.clone();
            async move {
                Json(json!({
                    "accessToken": access_token_inner,
                    "refreshToken": "kiro-device-refresh-token",
                    "expiresIn": 1800,
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));
    let (token_url, token_handle) = start_server(token_server).await;

    let terminal_state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository.clone(),
            )
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_provider_oauth_device_session_entry_for_tests(
            "session-terminal",
            json!({
                "provider_id": "provider-kiro",
                "region": "us-east-1",
                "client_id": "kiro-device-client",
                "client_secret": "kiro-device-secret",
                "device_code": "device-code-123",
                "interval": 5,
                "expires_at_unix_secs": 4_102_444_800u64,
                "status": "pending",
                "proxy_node_id": null,
                "created_at_unix_ms": 1_711_000_000u64,
                "key_id": null,
                "email": null,
                "replaced": false,
                "error_msg": null,
            }),
        )
        .with_provider_oauth_token_url_for_tests("kiro_device_poll", format!("{token_url}/token"))
        .with_provider_oauth_token_url_for_tests("kiro_idc_refresh", token_url.to_string());

    let terminal_response = local_admin_provider_oauth_response(
        &terminal_state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-kiro/device-poll",
        Some(json!({ "session_id": "session-terminal" })),
    )
    .await;
    assert_eq!(terminal_response.status(), StatusCode::OK);
    let terminal_audit = terminal_response
        .extensions()
        .get::<AdminAuditEvent>()
        .expect("terminal transition should attach audit");
    assert_eq!(
        terminal_audit.event_name,
        "admin_provider_oauth_device_authorization_completed"
    );
    assert_eq!(
        terminal_audit.action,
        "poll_provider_oauth_device_authorization_terminal_state"
    );
    assert_eq!(terminal_audit.target_type, "provider_oauth_device_session");
    assert_eq!(terminal_audit.target_id, "session-terminal");

    let non_terminal_state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ))
        .with_provider_oauth_device_session_entry_for_tests(
            "session-existing-authorized",
            json!({
                "provider_id": "provider-kiro",
                "region": "us-east-1",
                "client_id": "kiro-device-client",
                "client_secret": "kiro-device-secret",
                "device_code": "device-code-123",
                "interval": 5,
                "expires_at_unix_secs": 4_102_444_800u64,
                "status": "authorized",
                "proxy_node_id": null,
                "created_at_unix_ms": 1_711_000_000u64,
                "key_id": "key-existing",
                "email": "kiro@example.com",
                "replaced": false,
                "error_msg": null,
            }),
        );

    let non_terminal_response = local_admin_provider_oauth_response(
        &non_terminal_state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-kiro/device-poll",
        Some(json!({ "session_id": "session-existing-authorized" })),
    )
    .await;
    assert_eq!(non_terminal_response.status(), StatusCode::OK);
    assert!(
        non_terminal_response
            .extensions()
            .get::<AdminAuditEvent>()
            .is_none(),
        "existing terminal session should not attach a new audit event"
    );

    token_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_oauth_device_authorize_via_execution_runtime_proxy_node() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_device_authorize_via_execution_runtime_proxy_node",
        gateway_handles_admin_provider_oauth_device_authorize_via_execution_runtime_proxy_node_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_device_authorize_via_execution_runtime_proxy_node_impl(
) {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                assert_eq!(proxy.node_id.as_deref(), Some("proxy-node-kiro"));
                assert_eq!(
                    plan.headers.get("host").map(String::as_str),
                    Some("oidc.us-east-1.amazonaws.com")
                );
                assert_eq!(
                    plan.headers
                        .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                        .map(String::as_str),
                    Some("true")
                );
                if plan.request_id == "kiro_device_register" {
                    assert_eq!(plan.url, "https://oidc.us-east-1.amazonaws.com/client/register");
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "clientId": "kiro-device-client",
                                "clientSecret": "kiro-device-secret"
                            }
                        }
                    }))
                } else {
                    assert_eq!(plan.request_id, "kiro_device_authorize");
                    assert_eq!(
                        plan.url,
                        "https://oidc.us-east-1.amazonaws.com/device_authorization"
                    );
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "deviceCode": "device-code-123",
                                "userCode": "USER-CODE",
                                "verificationUri": "https://device.example.com/verify",
                                "verificationUriComplete": "https://device.example.com/verify?user_code=USER-CODE",
                                "expiresIn": 600,
                                "interval": 5
                            }
                        }
                    }))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-kiro");
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog_repository)
                .attach_proxy_node_repository_for_tests(proxy_node_repository),
        )
        .with_provider_oauth_device_session_entry_for_tests(
            "seed-session",
            json!({"status":"seed"}),
        )
        .with_provider_oauth_token_url_for_tests(
            "kiro_device_register",
            "https://oidc.us-east-1.amazonaws.com/client/register",
        )
        .with_provider_oauth_token_url_for_tests(
            "kiro_device_authorize",
            "https://oidc.us-east-1.amazonaws.com/device_authorization",
        );
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/device-authorize"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "start_url": "https://view.awsapps.com/start",
            "region": "us-east-1",
            "proxy_node_id": "proxy-node-kiro",
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    let session_id = payload["session_id"]
        .as_str()
        .expect("session_id should exist")
        .to_string();
    assert_eq!(payload["user_code"], "USER-CODE");
    assert_eq!(payload["expires_in"], 600);
    assert_eq!(payload["interval"], 5);

    let stored = state
        .load_provider_oauth_device_session_for_tests(&format!("device_auth_session:{session_id}"))
        .expect("device session should be stored");
    let stored: serde_json::Value =
        serde_json::from_str(&stored).expect("device session json should parse");
    assert_eq!(stored["proxy_node_id"], "proxy-node-kiro");

    let plans = execution_plans.lock().expect("mutex should lock");
    assert_eq!(plans.len(), 2);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_oauth_start_key_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_start_key_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_oauth_start_key_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_start_key_locally_with_trusted_admin_principal_impl()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-oauth/keys/key-codex-oauth/start",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-oauth",
        "provider-codex",
        "openai:chat",
        "oauth-access-token",
    );
    key.auth_type = "oauth".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                provider_catalog_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth/start"
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
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(
        payload["redirect_uri"],
        "http://localhost:1455/auth/callback"
    );
    assert!(payload["authorization_url"]
        .as_str()
        .is_some_and(|url| url.contains("state=")));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_oauth_start_provider_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_start_provider_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_oauth_start_provider_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_start_provider_locally_with_trusted_admin_principal_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-oauth/providers/provider-codex/start",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                provider_catalog_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/start"
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
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(
        payload["redirect_uri"],
        "http://localhost:1455/auth/callback"
    );
    assert!(payload["authorization_url"]
        .as_str()
        .is_some_and(|url| url.contains("state=")));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_oauth_batch_import_task_status_locally_with_trusted_admin_principal(
) {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_batch_import_task_status_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_oauth_batch_import_task_status_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_batch_import_task_status_locally_with_trusted_admin_principal_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks/task-123",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_provider_oauth_batch_task_entry_for_tests(
                "task-123",
                json!({
                    "task_id": "task-123",
                    "provider_id": "provider-codex",
                    "provider_type": "codex",
                    "status": "completed",
                    "total": 2,
                    "processed": 2,
                    "success": 1,
                    "failed": 1,
                    "created_count": 0,
                    "replaced_count": 1,
                    "progress_percent": 100,
                    "message": "导入完成：成功 1，失败 1",
                    "error": null,
                    "error_samples": [{
                        "index": 1,
                        "status": "error",
                        "error": "Token 验证失败: invalid_grant",
                        "replaced": false
                    }],
                    "created_at": 1700000001u64,
                    "started_at": 1700000002u64,
                    "finished_at": 1700000003u64,
                    "updated_at": 1700000004u64,
                }),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks/task-123"
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
    assert_eq!(payload["task_id"], "task-123");
    assert_eq!(payload["provider_id"], "provider-codex");
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["total"], 2);
    assert_eq!(payload["processed"], 2);
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 1);
    assert_eq!(payload["created_count"], 0);
    assert_eq!(payload["replaced_count"], 1);
    assert_eq!(payload["progress_percent"], 100);
    assert_eq!(payload["error_samples"].as_array().map(Vec::len), Some(1));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn local_admin_provider_oauth_batch_task_status_attaches_audit_only_for_terminal_states() {
    run_admin_oauth_test(
        "local_admin_provider_oauth_batch_task_status_attaches_audit_only_for_terminal_states",
        local_admin_provider_oauth_batch_task_status_attaches_audit_only_for_terminal_states_impl,
    );
}

async fn local_admin_provider_oauth_batch_task_status_attaches_audit_only_for_terminal_states_impl()
{
    let completed_state = AppState::new()
        .expect("gateway should build")
        .with_provider_oauth_batch_task_entry_for_tests(
            "task-completed",
            json!({
                "task_id": "task-completed",
                "provider_id": "provider-codex",
                "provider_type": "codex",
                "status": "completed",
                "total": 1,
                "processed": 1,
                "success": 1,
                "failed": 0,
                "progress_percent": 100,
                "message": "导入完成",
                "error": null,
                "error_samples": [],
                "created_at": 1700000001u64,
                "started_at": 1700000002u64,
                "finished_at": 1700000003u64,
                "updated_at": 1700000004u64,
            }),
        );
    let completed_response = local_admin_provider_oauth_response(
        &completed_state,
        http::Method::GET,
        "/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks/task-completed",
        None,
    )
    .await;
    assert_eq!(completed_response.status(), StatusCode::OK);
    let completed_audit = completed_response
        .extensions()
        .get::<AdminAuditEvent>()
        .expect("completed task should attach audit");
    assert_eq!(
        completed_audit.event_name,
        "admin_provider_oauth_batch_task_completed_viewed"
    );
    assert_eq!(
        completed_audit.action,
        "view_provider_oauth_batch_task_terminal_state"
    );
    assert_eq!(completed_audit.target_type, "provider_oauth_batch_task");
    assert_eq!(completed_audit.target_id, "provider-codex:task-completed");

    let processing_state = AppState::new()
        .expect("gateway should build")
        .with_provider_oauth_batch_task_entry_for_tests(
            "task-processing",
            json!({
                "task_id": "task-processing",
                "provider_id": "provider-codex",
                "provider_type": "codex",
                "status": "processing",
                "total": 3,
                "processed": 1,
                "success": 1,
                "failed": 0,
                "progress_percent": 33,
                "message": "处理中",
                "error": null,
                "error_samples": [],
                "created_at": 1700000001u64,
                "started_at": 1700000002u64,
                "finished_at": null,
                "updated_at": 1700000003u64,
            }),
        );
    let processing_response = local_admin_provider_oauth_response(
        &processing_state,
        http::Method::GET,
        "/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks/task-processing",
        None,
    )
    .await;
    assert_eq!(processing_response.status(), StatusCode::OK);
    assert!(
        processing_response
            .extensions()
            .get::<AdminAuditEvent>()
            .is_none(),
        "processing task status should not attach audit"
    );
}

#[test]
fn gateway_batch_imports_admin_provider_oauth_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_batch_imports_admin_provider_oauth_locally_with_trusted_admin_principal",
        gateway_batch_imports_admin_provider_oauth_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_batch_imports_admin_provider_oauth_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_hits = Arc::new(Mutex::new(Vec::<String>::new()));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        any(move |request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                let raw_body = String::from_utf8(
                    to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("body should read")
                        .to_vec(),
                )
                .expect("body should be utf8");
                token_hits_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(raw_body.clone());
                if raw_body.contains("refresh_token=batch-refresh-success") {
                    Json(json!({
                        "access_token": "batch-imported-codex-access-token",
                        "refresh_token": "batch-imported-codex-refresh-token",
                        "token_type": "Bearer",
                        "expires_in": 1800,
                        "scope": "openid email profile offline_access",
                        "email": "batch-alice@example.com",
                        "account_id": "acct-batch-123",
                        "account_user_id": "acct-user-batch-123",
                        "plan_type": "plus",
                        "user_id": "user-batch-123",
                    }))
                    .into_response()
                } else {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": "invalid_grant",
                            "error_description": "refresh token invalid",
                        })),
                    )
                        .into_response()
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut existing_key = sample_key(
        "key-codex-batch-duplicate",
        "provider-codex",
        "openai:chat",
        "stale-batch-access-token",
    );
    existing_key.auth_type = "oauth".to_string();
    existing_key.is_active = false;
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","email":"batch-alice@example.com","account_id":"acct-batch-123","account_user_id":"acct-user-batch-123","plan_type":"plus","refresh_token":"old-refresh-token"}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/batch-import"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "credentials": "batch-refresh-success\nbatch-refresh-error\n",
            "proxy_node_id": "proxy-node-batch-import"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 2);
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 1);
    let results = payload["results"]
        .as_array()
        .expect("results should be array");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["status"], "success");
    assert_eq!(results[0]["key_id"], "key-codex-batch-duplicate");
    assert_eq!(results[0]["replaced"], true);
    assert_eq!(results[1]["status"], "error");
    assert!(
        results[1]["error"]
            .as_str()
            .expect("error should be string")
            .contains("Token 验证失败"),
        "payload={payload}"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(token_hits.lock().expect("mutex should lock").len(), 2);

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-batch-duplicate".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert!(persisted.is_active);
    assert_eq!(
        persisted.proxy,
        Some(json!({"node_id": "proxy-node-batch-import", "enabled": true}))
    );
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "batch-imported-codex-access-token");

    gateway_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_batch_imports_chatgpt_web_access_tokens_with_pool_hints() {
    run_admin_oauth_test(
        "gateway_batch_imports_chatgpt_web_access_tokens_with_pool_hints",
        gateway_batch_imports_chatgpt_web_access_tokens_with_pool_hints_impl,
    );
}

async fn gateway_batch_imports_chatgpt_web_access_tokens_with_pool_hints_impl() {
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        any(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "unexpected refresh exchange"})),
                )
            }
        }),
    );

    let mut provider = sample_provider("provider-chatgpt-web", "chatgpt_web", 10);
    provider.provider_type = "chatgpt_web".to_string();
    provider.config = Some(json!({"pool_advanced": {}}));
    let endpoint = sample_endpoint(
        "endpoint-chatgpt-web-image",
        "provider-chatgpt-web",
        "openai:image",
        "https://chatgpt.com",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));
    let pool_score_repository = Arc::new(InMemoryPoolMemberScoreRepository::default());

    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_pool_score_repository_for_tests(Arc::clone(&pool_score_repository))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests(
                "chatgpt_web",
                format!("{token_url}/oauth/token"),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let credentials = json!([
        {
            "accessToken": "chatgpt-web-batch-access-token",
            "expiresAt": 2_100_000_000u64,
            "email": "pool-image@example.com",
            "accountId": "acct-pool-image",
            "accountUserId": "user-pool-image__acct-pool-image",
            "planType": "plus",
            "userId": "user-pool-image"
        }
    ])
    .to_string();

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-chatgpt-web/batch-import"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "credentials": credentials,
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 0);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-chatgpt-web".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert_eq!(persisted.expires_at_unix_secs, Some(2_100_000_000));
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "chatgpt-web-batch-access-token");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["provider_type"], "chatgpt_web");
    assert_eq!(auth_config["access_token_import_temporary"], true);
    assert_eq!(auth_config["email"], "pool-image@example.com");
    assert_eq!(auth_config["account_id"], "acct-pool-image");
    assert_eq!(
        auth_config["account_user_id"],
        "user-pool-image__acct-pool-image"
    );
    assert_eq!(auth_config["plan_type"], "plus");
    assert_eq!(auth_config["user_id"], "user-pool-image");

    let score_scope = provider_key_pool_score_scope();
    let score_identity =
        PoolMemberIdentity::provider_api_key("provider-chatgpt-web", persisted.id.clone());
    let scores = pool_score_repository
        .get_pool_member_scores_by_ids(&GetPoolMemberScoresByIdsQuery {
            ids: vec![provider_key_pool_score_id(&score_identity, &score_scope)],
        })
        .await
        .expect("pool score should load");
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].member_id, persisted.id);
    assert_eq!(scores[0].hard_state, PoolMemberHardState::Unknown);
    assert!(scores[0].score > 0.0);

    gateway_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_starts_admin_provider_oauth_batch_import_task_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_starts_admin_provider_oauth_batch_import_task_locally_with_trusted_admin_principal",
        gateway_starts_admin_provider_oauth_batch_import_task_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_starts_admin_provider_oauth_batch_import_task_locally_with_trusted_admin_principal_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        any(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "access_token": "task-imported-codex-access-token",
                    "refresh_token": "task-imported-codex-refresh-token",
                    "token_type": "Bearer",
                    "expires_in": 1800,
                    "scope": "openid email profile offline_access",
                    "email": "task-alice@example.com",
                    "account_id": "acct-task-123",
                    "account_user_id": "acct-user-task-123",
                    "plan_type": "plus",
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let submit_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "credentials": "task-batch-refresh-success"
        }))
        .send()
        .await
        .expect("submit request should succeed");

    assert_eq!(submit_response.status(), StatusCode::OK);
    let submit_payload: serde_json::Value = submit_response
        .json()
        .await
        .expect("submit payload should parse");
    assert_eq!(submit_payload["status"], "submitted");
    assert_eq!(submit_payload["total"], 1);
    let task_id = submit_payload["task_id"]
        .as_str()
        .expect("task id should exist")
        .to_string();

    let mut status_payload = serde_json::Value::Null;
    for _ in 0..40 {
        let response = client
            .get(format!(
                "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks/{task_id}"
            ))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("status request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        status_payload = response.json().await.expect("status payload should parse");
        if status_payload["status"] == "completed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert_eq!(
        status_payload["status"], "completed",
        "payload={status_payload}"
    );
    assert_eq!(status_payload["total"], 1);
    assert_eq!(status_payload["processed"], 1);
    assert_eq!(status_payload["success"], 1);
    assert_eq!(status_payload["failed"], 0);
    assert_eq!(status_payload["created_count"], 1);
    assert_eq!(status_payload["replaced_count"], 0);
    assert_eq!(status_payload["progress_percent"], 100);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-codex".to_string()])
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);

    gateway_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_updates_admin_provider_oauth_batch_import_task_progress() {
    run_admin_oauth_test(
        "gateway_updates_admin_provider_oauth_batch_import_task_progress",
        gateway_updates_admin_provider_oauth_batch_import_task_progress_impl,
    );
}

async fn gateway_updates_admin_provider_oauth_batch_import_task_progress_impl() {
    let upstream = Router::new().fallback(any(|| async {
        (StatusCode::OK, Body::from("quota refresh body"))
    }));

    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        any(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                let hit = {
                    let mut guard = token_hits_inner.lock().expect("mutex should lock");
                    *guard += 1;
                    *guard
                };
                if hit == 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Json(json!({
                    "access_token": format!("progress-codex-access-token-{hit}"),
                    "refresh_token": format!("progress-codex-refresh-token-{hit}"),
                    "token_type": "Bearer",
                    "expires_in": 1800,
                    "scope": "openid email profile offline_access",
                    "email": format!("progress-{hit}@example.com"),
                    "account_id": format!("acct-progress-{hit}"),
                    "account_user_id": format!("acct-user-progress-{hit}"),
                    "plan_type": "plus",
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let submit_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "credentials": "progress-refresh-one\nprogress-refresh-two"
        }))
        .send()
        .await
        .expect("submit request should succeed");

    assert_eq!(submit_response.status(), StatusCode::OK);
    let submit_payload: serde_json::Value = submit_response
        .json()
        .await
        .expect("submit payload should parse");
    let task_id = submit_payload["task_id"]
        .as_str()
        .expect("task id should exist")
        .to_string();

    let mut progress_payload = serde_json::Value::Null;
    for _ in 0..50 {
        let response = client
            .get(format!(
                "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks/{task_id}"
            ))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("status request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        progress_payload = response.json().await.expect("status payload should parse");
        if progress_payload["status"] == "processing" && progress_payload["processed"] == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    assert_eq!(
        progress_payload["status"], "processing",
        "payload={progress_payload}"
    );
    assert_eq!(progress_payload["total"], 2);
    assert_eq!(progress_payload["processed"], 1);
    assert_eq!(progress_payload["success"], 1);
    assert_eq!(progress_payload["failed"], 0);
    assert_eq!(progress_payload["created_count"], 1);
    assert_eq!(progress_payload["progress_percent"], 50);

    let mut completed_payload = progress_payload;
    for _ in 0..50 {
        let response = client
            .get(format!(
                "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/batch-import/tasks/{task_id}"
            ))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("status request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        completed_payload = response.json().await.expect("status payload should parse");
        if completed_payload["status"] == "completed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    assert_eq!(
        completed_payload["status"], "completed",
        "payload={completed_payload}"
    );
    assert_eq!(completed_payload["processed"], 2);
    assert_eq!(completed_payload["success"], 2);
    assert_eq!(completed_payload["progress_percent"], 100);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 2);

    gateway_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_completes_admin_provider_oauth_key_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_completes_admin_provider_oauth_key_locally_with_trusted_admin_principal",
        gateway_completes_admin_provider_oauth_key_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_completes_admin_provider_oauth_key_locally_with_trusted_admin_principal_impl() {
    #[derive(Debug, Clone)]
    struct SeenTokenRequest {
        content_type: String,
        body: String,
    }

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let seen_token = Arc::new(Mutex::new(None::<SeenTokenRequest>));
    let seen_token_clone = Arc::clone(&seen_token);
    let token_server = Router::new().route(
        "/oauth/token",
        any(move |request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            let seen_token_inner = Arc::clone(&seen_token_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                *seen_token_inner.lock().expect("mutex should lock") = Some(SeenTokenRequest {
                    content_type: parts
                        .headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: String::from_utf8(raw_body.to_vec())
                        .expect("token request body should be utf8"),
                });
                Json(json!({
                    "access_token": "new-codex-access-token",
                    "refresh_token": "new-codex-refresh-token",
                    "token_type": "Bearer",
                    "expiresAt": 4_102_444_800u64,
                    "email": "alice@example.com",
                    "account_id": "acct-codex-123",
                    "plan_type": "plus",
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();

    let mut key = sample_key(
        "key-codex-oauth",
        "provider-codex",
        "openai:chat",
        "__placeholder__",
    );
    key.auth_type = "oauth".to_string();

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![],
        vec![key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_state_entry_for_tests(
                "nonce-codex-123",
                json!({
                    "nonce": "nonce-codex-123",
                    "key_id": "key-codex-oauth",
                    "provider_id": "provider-codex",
                    "provider_type": "codex",
                    "pkce_verifier": "verifier-codex-123",
                }),
            )
            .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth/complete"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "callback_url": "http://localhost:1455/auth/callback?code=code-codex-123&state=nonce-codex-123"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["has_refresh_token"], true);
    assert_eq!(payload["expires_at"], 4_102_444_800u64);
    assert_eq!(payload["email"], "alice@example.com");
    assert_eq!(payload["account_state_recheck_attempted"], false);
    assert_eq!(
        payload["account_state_recheck_error"],
        serde_json::Value::Null
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let seen_token = seen_token
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("token request should be recorded");
    assert_eq!(seen_token.content_type, "application/x-www-form-urlencoded");
    assert!(seen_token.body.contains("grant_type=authorization_code"));
    assert!(seen_token
        .body
        .contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
    assert!(seen_token.body.contains("code=code-codex-123"));
    assert!(seen_token.body.contains("code_verifier=verifier-codex-123"));

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-oauth".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert_eq!(persisted.expires_at_unix_secs, Some(4_102_444_800));
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "new-codex-access-token");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(auth_config["refresh_token"], "new-codex-refresh-token");
    assert_eq!(auth_config["expires_at"], 4_102_444_800u64);
    assert_eq!(auth_config["email"], "alice@example.com");
    assert_eq!(auth_config["account_id"], "acct-codex-123");
    assert_eq!(auth_config["plan_type"], "plus");

    gateway_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_completes_admin_provider_oauth_provider_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_completes_admin_provider_oauth_provider_locally_with_trusted_admin_principal",
        gateway_completes_admin_provider_oauth_provider_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_completes_admin_provider_oauth_provider_locally_with_trusted_admin_principal_impl()
{
    #[derive(Debug, Clone)]
    struct SeenTokenRequest {
        content_type: String,
        body: String,
    }

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let seen_token = Arc::new(Mutex::new(None::<SeenTokenRequest>));
    let seen_token_clone = Arc::clone(&seen_token);
    let token_server = Router::new().route(
        "/oauth/token",
        any(move |request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            let seen_token_inner = Arc::clone(&seen_token_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                *seen_token_inner.lock().expect("mutex should lock") = Some(SeenTokenRequest {
                    content_type: parts
                        .headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: String::from_utf8(raw_body.to_vec())
                        .expect("token request body should be utf8"),
                });
                Json(json!({
                    "access_token": "provider-codex-access-token",
                    "refresh_token": "provider-codex-refresh-token",
                    "token_type": "Bearer",
                    "expires_at": 4_102_444_800u64,
                    "email": "alice@example.com",
                    "account_id": "acct-codex-123",
                    "plan_type": "plus",
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut existing_key = sample_key(
        "key-codex-inactive-duplicate",
        "provider-codex",
        "openai:chat",
        "stale-codex-access-token",
    );
    existing_key.auth_type = "oauth".to_string();
    existing_key.is_active = false;
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","refresh_token":"old-refresh-token"}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_state_entry_for_tests(
                "nonce-provider-codex-123",
                json!({
                    "nonce": "nonce-provider-codex-123",
                    "key_id": "",
                    "provider_id": "provider-codex",
                    "provider_type": "codex",
                    "pkce_verifier": "verifier-provider-codex-123",
                }),
            )
            .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/complete"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "callback_url": "http://localhost:1455/auth/callback?code=provider-code-123&state=nonce-provider-codex-123",
            "proxy_node_id": "proxy-node-codex-oauth",
            "name": "should-not-override-inactive-name"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["key_id"], "key-codex-inactive-duplicate");
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["has_refresh_token"], true);
    assert_eq!(payload["expires_at"], 4_102_444_800u64);
    assert_eq!(payload["email"], "alice@example.com");
    assert_eq!(payload["replaced"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let seen_token = seen_token
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("token request should be recorded");
    assert_eq!(seen_token.content_type, "application/x-www-form-urlencoded");
    assert!(seen_token.body.contains("grant_type=authorization_code"));
    assert!(seen_token.body.contains("code=provider-code-123"));
    assert!(seen_token
        .body
        .contains("code_verifier=verifier-provider-codex-123"));

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-inactive-duplicate".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert!(persisted.is_active);
    assert_eq!(persisted.expires_at_unix_secs, Some(4_102_444_800));
    assert_eq!(
        persisted.proxy,
        Some(json!({"node_id": "proxy-node-codex-oauth", "enabled": true}))
    );
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "provider-codex-access-token");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(auth_config["refresh_token"], "provider-codex-refresh-token");
    assert_eq!(auth_config["expires_at"], 4_102_444_800u64);
    assert_eq!(auth_config["email"], "alice@example.com");
    assert_eq!(auth_config["account_id"], "acct-codex-123");
    assert_eq!(auth_config["plan_type"], "plus");

    gateway_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_imports_admin_provider_oauth_refresh_token_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_imports_admin_provider_oauth_refresh_token_locally_with_trusted_admin_principal",
        gateway_imports_admin_provider_oauth_refresh_token_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_imports_admin_provider_oauth_refresh_token_locally_with_trusted_admin_principal_impl(
) {
    #[derive(Debug, Clone)]
    struct SeenTokenRequest {
        content_type: String,
        body: String,
    }

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let seen_token = Arc::new(Mutex::new(None::<SeenTokenRequest>));
    let seen_token_clone = Arc::clone(&seen_token);
    let token_server = Router::new().route(
        "/oauth/token",
        post(move |headers: HeaderMap, body: Bytes| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            let seen_token_inner = Arc::clone(&seen_token_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                *seen_token_inner.lock().expect("mutex should lock") = Some(SeenTokenRequest {
                    content_type: headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: String::from_utf8(body.to_vec()).unwrap_or_default(),
                });
                Json(json!({
                    "access_token": "imported-codex-access-token",
                    "refresh_token": "imported-codex-refresh-token",
                    "token_type": "Bearer",
                    "expires_in": 1800,
                    "scope": "openid email profile offline_access",
                    "email": "alice@example.com",
                    "account_id": "acct-codex-123",
                    "plan_type": "plus",
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut existing_key = sample_key(
        "key-codex-import-duplicate",
        "provider-codex",
        "openai:chat",
        "stale-imported-access-token",
    );
    existing_key.auth_type = "oauth".to_string();
    existing_key.is_active = false;
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","refresh_token":"old-refresh-token"}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "refresh_token": "provider-import-refresh-token",
            "proxy_node_id": "proxy-node-codex-import",
            "name": "should-not-override-inactive-name"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["key_id"], "key-codex-import-duplicate");
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["has_refresh_token"], true);
    assert_eq!(payload["email"], "alice@example.com");
    assert_eq!(payload["replaced"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let seen_token = seen_token
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("token request should be recorded");
    assert_eq!(seen_token.content_type, "application/x-www-form-urlencoded");
    assert!(seen_token.body.contains("grant_type=refresh_token"));
    assert!(seen_token
        .body
        .contains("refresh_token=provider-import-refresh-token"));

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-import-duplicate".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert!(persisted.is_active);
    assert_eq!(
        persisted.proxy,
        Some(json!({"node_id": "proxy-node-codex-import", "enabled": true}))
    );
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "imported-codex-access-token");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(auth_config["refresh_token"], "imported-codex-refresh-token");
    assert_eq!(auth_config["email"], "alice@example.com");
    assert_eq!(auth_config["account_id"], "acct-codex-123");
    assert_eq!(auth_config["plan_type"], "plus");

    gateway_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_imports_codex_access_token_without_refresh_token_as_temporary_account() {
    run_admin_oauth_test(
        "gateway_imports_codex_access_token_without_refresh_token_as_temporary_account",
        gateway_imports_codex_access_token_without_refresh_token_as_temporary_account_impl,
    );
}

async fn gateway_imports_codex_access_token_without_refresh_token_as_temporary_account_impl() {
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        post(move || {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "unexpected refresh exchange"})),
                )
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let access_token =
        sample_codex_access_token_with_profile_email("profile@example.com", "acct-profile-123");

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "access_token": access_token,
            "name": "temporary-codex-access-token",
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["has_refresh_token"], false);
    assert_eq!(payload["temporary"], true);
    assert_eq!(payload["email"], "profile@example.com");
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-codex".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert_eq!(persisted.expires_at_unix_secs, Some(2_000_000_000));
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, access_token);
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(auth_config["access_token_import_temporary"], true);
    assert_eq!(auth_config["email"], "profile@example.com");
    assert_eq!(auth_config["account_id"], "acct-profile-123");
    assert!(auth_config.get("refresh_token").is_none());

    gateway_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_imports_codex_header_authorization_without_overwriting_payload_access_token() {
    run_admin_oauth_test(
        "gateway_imports_codex_header_authorization_without_overwriting_payload_access_token",
        gateway_imports_codex_header_authorization_without_overwriting_payload_access_token_impl,
    );
}

async fn gateway_imports_codex_header_authorization_without_overwriting_payload_access_token_impl()
{
    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let access_token =
        sample_codex_access_token_with_profile_email("profile@example.com", "acct-profile-123");

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "access_token": access_token,
            "headers": {
                "authorization": "Bearer imported-session-token",
                "chatgpt-account-id": "acct-header"
            },
            "name": "temporary-codex-header-auth",
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");

    let reloaded = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-codex".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, access_token);
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["email"], "profile@example.com");
    assert_eq!(auth_config["access_token"], access_token);
    assert_eq!(
        auth_config["headers"]["authorization"],
        "Bearer imported-session-token"
    );
    assert_eq!(auth_config["headers"]["chatgpt-account-id"], "acct-header");

    gateway_handle.abort();
}

#[test]
fn gateway_imports_chatgpt_web_access_token_without_refresh_token_as_temporary_account() {
    run_admin_oauth_test(
        "gateway_imports_chatgpt_web_access_token_without_refresh_token_as_temporary_account",
        gateway_imports_chatgpt_web_access_token_without_refresh_token_as_temporary_account_impl,
    );
}

async fn gateway_imports_chatgpt_web_access_token_without_refresh_token_as_temporary_account_impl()
{
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        post(move || {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "unexpected refresh exchange"})),
                )
            }
        }),
    );

    let mut provider = sample_provider("provider-chatgpt-web", "chatgpt_web", 10);
    provider.provider_type = "chatgpt_web".to_string();
    let endpoint = sample_endpoint(
        "endpoint-chatgpt-web-image",
        "provider-chatgpt-web",
        "openai:image",
        "https://chatgpt.com",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests(
                "chatgpt_web",
                format!("{token_url}/oauth/token"),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let access_token =
        sample_codex_access_token_with_profile_email("image@example.com", "acct-image-123");

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-chatgpt-web/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "access_token": access_token,
            "name": "temporary-chatgpt-web-access-token",
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["provider_type"], "chatgpt_web");
    assert_eq!(payload["has_refresh_token"], false);
    assert_eq!(payload["temporary"], true);
    assert_eq!(payload["email"], "image@example.com");
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 0);

    let reloaded = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-chatgpt-web".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert_eq!(persisted.expires_at_unix_secs, Some(2_000_000_000));
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, access_token);
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["provider_type"], "chatgpt_web");
    assert_eq!(auth_config["access_token_import_temporary"], true);
    assert_eq!(auth_config["email"], "image@example.com");
    assert_eq!(auth_config["account_id"], "acct-image-123");
    assert!(auth_config.get("refresh_token").is_none());

    gateway_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_imports_codex_access_token_with_payload_expires_at_when_token_has_no_exp() {
    run_admin_oauth_test(
        "gateway_imports_codex_access_token_with_payload_expires_at_when_token_has_no_exp",
        gateway_imports_codex_access_token_with_payload_expires_at_when_token_has_no_exp_impl,
    );
}

async fn gateway_imports_codex_access_token_with_payload_expires_at_when_token_has_no_exp_impl() {
    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-responses",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
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
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "access_token": "opaque-codex-access-token",
            "expiresAt": 2_100_000_000u64,
            "name": "temporary-codex-opaque-access-token",
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["expires_at"], 2_100_000_000u64);

    let reloaded = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-codex".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert_eq!(persisted.expires_at_unix_secs, Some(2_100_000_000));
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["expires_at"], 2_100_000_000u64);
    assert_eq!(auth_config["access_token_import_temporary"], true);

    gateway_handle.abort();
}

#[test]
fn gateway_imports_admin_provider_oauth_refresh_token_over_active_expired_duplicate() {
    run_admin_oauth_test(
        "gateway_imports_admin_provider_oauth_refresh_token_over_active_expired_duplicate",
        gateway_imports_admin_provider_oauth_refresh_token_over_active_expired_duplicate_impl,
    );
}

async fn gateway_imports_admin_provider_oauth_refresh_token_over_active_expired_duplicate_impl() {
    #[derive(Debug, Clone)]
    struct SeenTokenRequest {
        content_type: String,
        body: String,
    }

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let seen_token = Arc::new(Mutex::new(None::<SeenTokenRequest>));
    let seen_token_clone = Arc::clone(&seen_token);
    let token_server = Router::new().route(
        "/oauth/token",
        post(move |headers: HeaderMap, body: Bytes| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            let seen_token_inner = Arc::clone(&seen_token_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                *seen_token_inner.lock().expect("mutex should lock") = Some(SeenTokenRequest {
                    content_type: headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: String::from_utf8(body.to_vec()).unwrap_or_default(),
                });
                Json(json!({
                    "access_token": "imported-expired-codex-access-token",
                    "refresh_token": "imported-expired-codex-refresh-token",
                    "token_type": "Bearer",
                    "expires_in": 1800,
                    "scope": "openid email profile offline_access",
                    "email": "alice@example.com",
                    "account_id": "acct-codex-123",
                    "plan_type": "plus",
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut existing_key = sample_key(
        "key-codex-import-expired-duplicate",
        "provider-codex",
        "openai:chat",
        "stale-imported-access-token",
    );
    existing_key.auth_type = "oauth".to_string();
    existing_key.is_active = true;
    existing_key.expires_at_unix_secs = Some(1);
    existing_key.oauth_invalid_at_unix_secs = Some(1_700_000_000);
    existing_key.oauth_invalid_reason =
        Some("[REFRESH_FAILED] refresh_token 无效、已过期或已撤销，请重新登录授权".to_string());
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","refresh_token":"old-refresh-token","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (token_url, token_handle) = start_server(token_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "refresh_token": "provider-import-refresh-token",
            "proxy_node_id": "proxy-node-codex-import",
            "name": "should-not-override-active-expired-name"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["key_id"], "key-codex-import-expired-duplicate");
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["has_refresh_token"], true);
    assert_eq!(payload["email"], "alice@example.com");
    assert_eq!(payload["replaced"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let seen_token = seen_token
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("token request should be recorded");
    assert_eq!(seen_token.content_type, "application/x-www-form-urlencoded");
    assert!(seen_token.body.contains("grant_type=refresh_token"));
    assert!(seen_token
        .body
        .contains("refresh_token=provider-import-refresh-token"));

    let reloaded = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-import-expired-duplicate".to_string()])
        .await
        .expect("keys should load");
    let persisted = reloaded.first().expect("persisted key should exist");
    assert!(persisted.is_active);
    assert_eq!(
        persisted.proxy,
        Some(json!({"node_id": "proxy-node-codex-import", "enabled": true}))
    );
    assert_eq!(persisted.oauth_invalid_at_unix_secs, None);
    assert_eq!(persisted.oauth_invalid_reason, None);
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "imported-expired-codex-access-token");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        persisted
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should be stored"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(
        auth_config["refresh_token"],
        "imported-expired-codex-refresh-token"
    );
    assert_eq!(auth_config["email"], "alice@example.com");
    assert_eq!(auth_config["account_id"], "acct-codex-123");
    assert_eq!(auth_config["plan_type"], "plus");

    gateway_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_import_invalidate_cached_oauth_entry_before_followup_resolution() {
    run_admin_oauth_test(
        "gateway_import_invalidate_cached_oauth_entry_before_followup_resolution",
        gateway_import_invalidate_cached_oauth_entry_before_followup_resolution_impl,
    );
}

async fn gateway_import_invalidate_cached_oauth_entry_before_followup_resolution_impl() {
    let token_server = Router::new().route(
        "/oauth/token",
        post(move |body: Bytes| async move {
            let body_text = String::from_utf8(body.to_vec()).unwrap_or_default();
            if body_text.contains("refresh_token=old-refresh-token") {
                Json(json!({
                    "access_token": "cached-old-codex-access-token",
                    "refresh_token": "cached-old-refresh-token",
                    "token_type": "Bearer",
                    "expires_in": 1800,
                    "scope": "openid email profile offline_access",
                    "email": "alice@example.com",
                    "account_id": "acct-codex-123",
                    "plan_type": "plus",
                }))
            } else {
                assert!(
                    body_text.contains("refresh_token=provider-import-refresh-token"),
                    "unexpected token request body: {body_text}"
                );
                Json(json!({
                    "access_token": "imported-fresh-codex-access-token",
                    "refresh_token": "imported-fresh-refresh-token",
                    "token_type": "Bearer",
                    "expires_in": 1800,
                    "scope": "openid email profile offline_access",
                    "email": "alice@example.com",
                    "account_id": "acct-codex-123",
                    "plan_type": "plus",
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut existing_key = sample_key(
        "key-codex-import-cache-duplicate",
        "provider-codex",
        "openai:chat",
        "stale-imported-access-token",
    );
    existing_key.auth_type = "oauth".to_string();
    existing_key.expires_at_unix_secs = Some(1);
    existing_key.oauth_invalid_at_unix_secs = Some(1_700_000_000);
    existing_key.oauth_invalid_reason = Some("[OAUTH_EXPIRED] token invalidated".to_string());
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","refresh_token":"old-refresh-token","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
            ),
        ]);
    let app_state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository.clone(),
            )
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_provider_oauth_token_url_for_tests("codex", format!("{token_url}/oauth/token"))
        .with_oauth_refresh_coordinator_for_tests(oauth_refresh);

    let stale_transport = app_state
        .read_provider_transport_snapshot(
            "provider-codex",
            "endpoint-codex-chat",
            "key-codex-import-cache-duplicate",
        )
        .await
        .expect("transport should load")
        .expect("transport should exist");
    let cached_entry = app_state
        .force_local_oauth_refresh_entry(&stale_transport)
        .await
        .expect("initial refresh should succeed")
        .expect("initial refresh should return cached entry");
    assert_eq!(
        cached_entry.auth_header_value,
        "Bearer cached-old-codex-access-token"
    );
    let mut replaceable_key = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-import-cache-duplicate".to_string()])
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("key should exist");
    replaceable_key.oauth_invalid_at_unix_secs = Some(1_700_000_000);
    replaceable_key.oauth_invalid_reason = Some("[OAUTH_EXPIRED] token invalidated".to_string());
    provider_catalog_repository
        .update_key(&replaceable_key)
        .await
        .expect("key should update");

    let gateway = build_router_with_state(app_state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "refresh_token": "provider-import-refresh-token",
            "name": "should-not-override-cache-duplicate-name"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["key_id"], "key-codex-import-cache-duplicate");
    assert_eq!(payload["replaced"], true);

    let fresh_transport = app_state
        .read_provider_transport_snapshot(
            "provider-codex",
            "endpoint-codex-chat",
            "key-codex-import-cache-duplicate",
        )
        .await
        .expect("transport should load")
        .expect("transport should exist");
    let resolved = app_state
        .resolve_local_oauth_request_auth(&fresh_transport)
        .await
        .expect("oauth auth should resolve")
        .expect("oauth auth should exist");
    match resolved {
        crate::provider_transport::LocalResolvedOAuthRequestAuth::Header { value, .. } => {
            assert_eq!(value, "Bearer imported-fresh-codex-access-token");
        }
        crate::provider_transport::LocalResolvedOAuthRequestAuth::Kiro(_) => {
            panic!("codex should resolve to header auth")
        }
    }

    gateway_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_rejects_kiro_single_refresh_token_import_with_clear_error() {
    run_admin_oauth_test(
        "gateway_rejects_kiro_single_refresh_token_import_with_clear_error",
        gateway_rejects_kiro_single_refresh_token_import_with_clear_error_impl,
    );
}

async fn gateway_rejects_kiro_single_refresh_token_import_with_clear_error_impl() {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![{
            let mut provider = sample_provider("provider-kiro", "kiro", 10);
            provider.provider_type = "kiro".to_string();
            provider
        }],
        vec![sample_endpoint(
            "endpoint-kiro-chat",
            "provider-kiro",
            "kiro:generateAssistantResponse",
            "https://service.kiro.dev",
        )],
        Vec::new(),
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
            provider_catalog_repository,
        ));

    let response = local_admin_provider_oauth_response(
        &state,
        http::Method::POST,
        "/api/admin/provider-oauth/providers/provider-kiro/import-refresh-token",
        Some(json!({
            "refresh_token": "kiro-refresh-token"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("json body should parse");
    assert_eq!(
        payload["detail"],
        json!("Kiro 不支持单条 Refresh Token 导入，请使用批量导入或设备授权。")
    );
}

#[test]
fn gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_proxy_node() {
    run_admin_oauth_test(
        "gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_proxy_node",
        gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_proxy_node_impl,
    );
}

async fn gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_proxy_node_impl()
{
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                assert_eq!(proxy.node_id.as_deref(), Some("proxy-node-codex-import"));
                let timeouts = plan.timeouts.as_ref().expect("timeouts should exist");
                assert_eq!(timeouts.connect_ms, Some(60_000));
                assert_eq!(timeouts.read_ms, Some(60_000));
                assert_eq!(timeouts.write_ms, Some(60_000));
                assert_eq!(timeouts.pool_ms, Some(60_000));
                assert_eq!(timeouts.total_ms, Some(60_000));
                if plan.request_id == "provider-oauth:refresh-token" {
                    assert_eq!(plan.method, "POST");
                    assert_eq!(plan.url, "https://oauth.example/oauth/token");
                    assert_eq!(
                        plan.headers.get("content-type").map(String::as_str),
                        Some("application/x-www-form-urlencoded")
                    );
                    assert_eq!(
                        plan.headers
                            .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                            .map(String::as_str),
                        Some("true")
                    );
                    Json(codex_import_token_execution_result(&plan.request_id))
                } else {
                    assert!(
                        plan.request_id.starts_with("codex-quota:"),
                        "unexpected execution plan: {}",
                        plan.request_id
                    );
                    Json(codex_quota_execution_result(&plan.request_id))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-codex-import");
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "refresh_token": "provider-import-refresh-token",
            "proxy_node_id": "proxy-node-codex-import",
            "name": "codex-import"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["has_refresh_token"], true);
    assert_eq!(payload["email"], "alice@example.com");
    assert_eq!(payload["replaced"], false);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-codex".to_string()])
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0].proxy,
        Some(json!({"node_id": "proxy-node-codex-import", "enabled": true}))
    );

    let plans = execution_plans.lock().expect("mutex should lock");
    let token_plan = assert_single_provider_oauth_refresh_token_plan(&plans);
    assert_eq!(
        token_plan
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.node_id.as_deref()),
        Some("proxy-node-codex-import")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_provider_proxy_before_system_proxy(
) {
    run_admin_oauth_test(
        "gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_provider_proxy_before_system_proxy",
        gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_provider_proxy_before_system_proxy_impl,
    );
}

async fn gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_provider_proxy_before_system_proxy_impl(
) {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                assert_eq!(proxy.node_id.as_deref(), Some("proxy-node-codex-provider"));
                if plan.request_id == "provider-oauth:refresh-token" {
                    Json(codex_import_token_execution_result(&plan.request_id))
                } else {
                    assert!(
                        plan.request_id.starts_with("codex-quota:"),
                        "unexpected execution plan: {}",
                        plan.request_id
                    );
                    Json(codex_quota_execution_result(&plan.request_id))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    provider.proxy = Some(json!({"node_id":"proxy-node-codex-provider","enabled":true}));
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));
    let mut provider_node = sample_proxy_node("proxy-node-codex-provider");
    provider_node.status = "online".to_string();
    provider_node.is_manual = true;
    provider_node.tunnel_mode = false;
    provider_node.tunnel_connected = false;
    provider_node.proxy_url = Some("http://proxy-provider.example:8080".to_string());
    let mut system_node = sample_proxy_node("proxy-node-codex-system");
    system_node.status = "online".to_string();
    system_node.is_manual = true;
    system_node.tunnel_mode = false;
    system_node.tunnel_connected = false;
    system_node.proxy_url = Some("http://proxy-system.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        provider_node,
        system_node,
    ]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_system_config_values_for_tests(vec![(
                    "system_proxy_node_id".to_string(),
                    json!("proxy-node-codex-system"),
                )])
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "refresh_token": "provider-import-refresh-token",
            "name": "codex-import"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["provider_type"], "codex");

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-codex".to_string()])
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].proxy, None);

    let plans = execution_plans.lock().expect("mutex should lock");
    let token_plan = assert_single_provider_oauth_refresh_token_plan(&plans);
    assert_eq!(
        token_plan
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.node_id.as_deref()),
        Some("proxy-node-codex-provider")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_system_proxy() {
    run_admin_oauth_test(
        "gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_system_proxy",
        gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_system_proxy_impl,
    );
}

async fn gateway_imports_admin_provider_oauth_refresh_token_via_execution_runtime_system_proxy_impl(
) {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                let proxy = plan.proxy.as_ref().expect("proxy snapshot should exist");
                assert_eq!(proxy.node_id.as_deref(), Some("proxy-node-codex-system"));
                if plan.request_id == "provider-oauth:refresh-token" {
                    Json(codex_import_token_execution_result(&plan.request_id))
                } else {
                    assert!(
                        plan.request_id.starts_with("codex-quota:"),
                        "unexpected execution plan: {}",
                        plan.request_id
                    );
                    Json(codex_quota_execution_result(&plan.request_id))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-codex-system");
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_system_config_values_for_tests(vec![(
                    "system_proxy_node_id".to_string(),
                    json!("proxy-node-codex-system"),
                )])
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "refresh_token": "provider-import-refresh-token",
            "name": "codex-import"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["provider_type"], "codex");

    let plans = execution_plans.lock().expect("mutex should lock");
    let token_plan = assert_single_provider_oauth_refresh_token_plan(&plans);
    assert_eq!(
        token_plan
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.node_id.as_deref()),
        Some("proxy-node-codex-system")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_import_refresh_token_surfaces_execution_runtime_error_detail() {
    run_admin_oauth_test(
        "gateway_import_refresh_token_surfaces_execution_runtime_error_detail",
        gateway_import_refresh_token_surfaces_execution_runtime_error_detail_impl,
    );
}

async fn gateway_import_refresh_token_surfaces_execution_runtime_error_detail_impl() {
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-chat",
        "provider-codex",
        "openai:chat",
        "https://chatgpt.com/backend-api/codex",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-codex/import-refresh-token"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "refresh_token": "provider-import-refresh-token",
            "name": "codex-import"
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::BAD_REQUEST, "payload={payload}");
    assert!(
        payload["detail"]
            .as_str()
            .expect("detail should be string")
            .contains("execution runtime returned HTTP 500"),
        "payload={payload}"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_batch_imports_admin_provider_oauth_kiro_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_batch_imports_admin_provider_oauth_kiro_locally_with_trusted_admin_principal",
        gateway_batch_imports_admin_provider_oauth_kiro_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_batch_imports_admin_provider_oauth_kiro_locally_with_trusted_admin_principal_impl()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let refresh_hits = Arc::new(Mutex::new(0usize));
    let refresh_hits_clone = Arc::clone(&refresh_hits);
    let refresh_server = Router::new().route(
        "/refreshToken",
        post(move |_request: Request| {
            let refresh_hits_inner = Arc::clone(&refresh_hits_clone);
            async move {
                *refresh_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "accessToken": sample_kiro_device_access_token("kiro-batch@example.com"),
                    "refreshToken": "kiro-batch-refresh-token-new",
                    "expiresIn": 1800,
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let endpoint = sample_endpoint(
        "endpoint-kiro-chat",
        "provider-kiro",
        "kiro:generateAssistantResponse",
        "https://service.kiro.dev",
    );

    let mut existing_key = sample_key(
        "key-kiro-batch-duplicate",
        "provider-kiro",
        "kiro:generateAssistantResponse",
        "stale-kiro-access-token",
    );
    existing_key.auth_type = "oauth".to_string();
    existing_key.is_active = false;
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"kiro","auth_method":"social","email":"kiro-batch@example.com","refresh_token":"kiro-batch-refresh-old"}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (refresh_url, refresh_handle) = start_server(refresh_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests(
                "kiro_social_refresh",
                refresh_url.to_string(),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/batch-import"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "credentials": "kiro-batch-refresh-token",
            "proxy_node_id": "proxy-node-kiro-batch"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("payload should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 0);
    let results = payload["results"]
        .as_array()
        .expect("results should be array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "success");
    assert_eq!(results[0]["key_id"], "key-kiro-batch-duplicate");
    assert_eq!(results[0]["auth_method"], "social");
    assert_eq!(results[0]["replaced"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*refresh_hits.lock().expect("mutex should lock"), 1);

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-kiro-batch-duplicate".to_string()])
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("persisted key should exist");
    assert!(stored_key.is_active);
    assert_eq!(
        stored_key.proxy,
        Some(json!({"node_id": "proxy-node-kiro-batch", "enabled": true}))
    );
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["provider_type"], "kiro");
    assert_eq!(auth_config["auth_method"], "social");
    assert_eq!(auth_config["email"], "kiro-batch@example.com");
    assert_eq!(auth_config["refresh_token"], "kiro-batch-refresh-token-new");

    gateway_handle.abort();
    refresh_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_batch_imports_admin_provider_oauth_kiro_over_active_expired_duplicate() {
    run_admin_oauth_test(
        "gateway_batch_imports_admin_provider_oauth_kiro_over_active_expired_duplicate",
        gateway_batch_imports_admin_provider_oauth_kiro_over_active_expired_duplicate_impl,
    );
}

async fn gateway_batch_imports_admin_provider_oauth_kiro_over_active_expired_duplicate_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let refresh_hits = Arc::new(Mutex::new(0usize));
    let refresh_hits_clone = Arc::clone(&refresh_hits);
    let refresh_server = Router::new().route(
        "/refreshToken",
        post(move |_request: Request| {
            let refresh_hits_inner = Arc::clone(&refresh_hits_clone);
            async move {
                *refresh_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "accessToken": sample_kiro_device_access_token("kiro-batch@example.com"),
                    "refreshToken": "kiro-batch-refresh-token-replaced",
                    "expiresIn": 1800,
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let endpoint = sample_endpoint(
        "endpoint-kiro-chat",
        "provider-kiro",
        "kiro:generateAssistantResponse",
        "https://service.kiro.dev",
    );

    let mut existing_key = sample_key(
        "key-kiro-batch-expired-duplicate",
        "provider-kiro",
        "kiro:generateAssistantResponse",
        "stale-kiro-access-token",
    );
    existing_key.auth_type = "oauth".to_string();
    existing_key.is_active = true;
    existing_key.oauth_invalid_at_unix_secs = Some(1_700_000_000);
    existing_key.oauth_invalid_reason = Some("Kiro Token 无效或已过期".to_string());
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"kiro","auth_method":"social","email":"kiro-batch@example.com","refresh_token":"kiro-batch-refresh-old"}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (refresh_url, refresh_handle) = start_server(refresh_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests(
                "kiro_social_refresh",
                refresh_url.to_string(),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/batch-import"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "credentials": "kiro-batch-refresh-token",
            "proxy_node_id": "proxy-node-kiro-batch"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("payload should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 0);
    let results = payload["results"]
        .as_array()
        .expect("results should be array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["status"], "success");
    assert_eq!(results[0]["key_id"], "key-kiro-batch-expired-duplicate");
    assert_eq!(results[0]["auth_method"], "social");
    assert_eq!(results[0]["replaced"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*refresh_hits.lock().expect("mutex should lock"), 1);

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-kiro-batch-expired-duplicate".to_string()])
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("persisted key should exist");
    assert!(stored_key.is_active);
    assert_eq!(
        stored_key.proxy,
        Some(json!({"node_id": "proxy-node-kiro-batch", "enabled": true}))
    );
    assert_eq!(stored_key.oauth_invalid_at_unix_secs, None);
    assert_eq!(stored_key.oauth_invalid_reason, None);
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["provider_type"], "kiro");
    assert_eq!(auth_config["auth_method"], "social");
    assert_eq!(auth_config["email"], "kiro-batch@example.com");
    assert_eq!(
        auth_config["refresh_token"],
        "kiro-batch-refresh-token-replaced"
    );

    gateway_handle.abort();
    refresh_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_batch_imports_admin_provider_oauth_kiro_via_execution_runtime_proxy_node() {
    run_admin_oauth_test(
        "gateway_batch_imports_admin_provider_oauth_kiro_via_execution_runtime_proxy_node",
        gateway_batch_imports_admin_provider_oauth_kiro_via_execution_runtime_proxy_node_impl,
    );
}

async fn gateway_batch_imports_admin_provider_oauth_kiro_via_execution_runtime_proxy_node_impl() {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                assert_eq!(
                    plan.proxy
                        .as_ref()
                        .and_then(|proxy| proxy.node_id.as_deref()),
                    Some("proxy-node-kiro-batch-runtime")
                );
                if plan.request_id == "provider-oauth:kiro-social-refresh" {
                    assert_eq!(plan.url, "https://oauth.example/refreshToken");
                    assert_eq!(
                        plan.headers
                            .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
                            .map(String::as_str),
                        Some("true")
                    );
                    return Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "accessToken": sample_kiro_device_access_token("kiro-runtime@example.com"),
                                "refreshToken": "kiro-runtime-refresh-token-new",
                                "expiresIn": 1800,
                            }
                        }
                    }));
                }

                assert_eq!(plan.request_id, "kiro-quota:key-kiro-batch-runtime");
                Json(json!({
                    "request_id": plan.request_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "subscriptionInfo": {
                                "subscriptionTitle": "KIRO PRO+"
                            },
                            "usageBreakdownList": [{
                                "currentUsageWithPrecision": 5.0,
                                "usageLimitWithPrecision": 20.0,
                                "nextDateReset": 1_900_000_000u64
                            }],
                            "desktopUserInfo": {
                                "email": "kiro-runtime@example.com"
                            }
                        }
                    }
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let endpoint = sample_endpoint(
        "endpoint-kiro-chat",
        "provider-kiro",
        "kiro:generateAssistantResponse",
        "https://service.kiro.dev",
    );

    let mut existing_key = sample_key(
        "key-kiro-batch-runtime",
        "provider-kiro",
        "kiro:generateAssistantResponse",
        "stale-kiro-runtime-access-token",
    );
    existing_key.auth_type = "oauth".to_string();
    existing_key.is_active = false;
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"kiro","auth_method":"social","email":"kiro-runtime@example.com","refresh_token":"kiro-runtime-refresh-token-old"}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));
    let mut manual_node = sample_proxy_node("proxy-node-kiro-batch-runtime");
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![manual_node]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests(
                "kiro_social_refresh",
                "https://oauth.example",
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/batch-import"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "credentials": "kiro-runtime-refresh-token-old",
            "proxy_node_id": "proxy-node-kiro-batch-runtime"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("payload should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["success"], 1);
    assert_eq!(payload["failed"], 0);
    assert_eq!(payload["results"][0]["status"], "success");
    assert_eq!(payload["results"][0]["key_id"], "key-kiro-batch-runtime");
    assert_eq!(payload["results"][0]["replaced"], true);

    for _ in 0..40 {
        let plan_count = execution_plans.lock().expect("mutex should lock").len();
        if plan_count == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    {
        let plans = execution_plans.lock().expect("mutex should lock");
        assert_eq!(plans.len(), 2);
    }

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-kiro-batch-runtime".to_string()])
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("persisted key should exist");
    assert!(stored_key.is_active);
    assert_eq!(
        stored_key.proxy,
        Some(json!({"node_id": "proxy-node-kiro-batch-runtime", "enabled": true}))
    );
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["email"], "kiro-runtime@example.com");
    assert_eq!(
        auth_config["refresh_token"],
        "kiro-runtime-refresh-token-new"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_starts_admin_provider_oauth_kiro_batch_import_task_locally_with_trusted_admin_principal()
{
    run_admin_oauth_test(
        "gateway_starts_admin_provider_oauth_kiro_batch_import_task_locally_with_trusted_admin_principal",
        gateway_starts_admin_provider_oauth_kiro_batch_import_task_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_starts_admin_provider_oauth_kiro_batch_import_task_locally_with_trusted_admin_principal_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let refresh_hits = Arc::new(Mutex::new(0usize));
    let refresh_hits_clone = Arc::clone(&refresh_hits);
    let refresh_server = Router::new().route(
        "/refreshToken",
        post(move |_request: Request| {
            let refresh_hits_inner = Arc::clone(&refresh_hits_clone);
            async move {
                *refresh_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "accessToken": sample_kiro_device_access_token("kiro-task@example.com"),
                    "refreshToken": "kiro-task-refresh-token-new",
                    "expiresIn": 1800,
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-kiro", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let endpoint = sample_endpoint(
        "endpoint-kiro-chat",
        "provider-kiro",
        "kiro:generateAssistantResponse",
        "https://service.kiro.dev",
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (refresh_url, refresh_handle) = start_server(refresh_server).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_provider_oauth_token_url_for_tests(
                "kiro_social_refresh",
                refresh_url.to_string(),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let submit_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/batch-import/tasks"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "credentials": "kiro-task-refresh-token"
        }))
        .send()
        .await
        .expect("submit request should succeed");

    assert_eq!(submit_response.status(), StatusCode::OK);
    let submit_payload: serde_json::Value = submit_response
        .json()
        .await
        .expect("submit payload should parse");
    assert_eq!(submit_payload["status"], "submitted");
    let task_id = submit_payload["task_id"]
        .as_str()
        .expect("task id should exist")
        .to_string();

    let mut status_payload = serde_json::Value::Null;
    for _ in 0..40 {
        let response = client
            .get(format!(
                "{gateway_url}/api/admin/provider-oauth/providers/provider-kiro/batch-import/tasks/{task_id}"
            ))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("status request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        status_payload = response.json().await.expect("status payload should parse");
        if status_payload["status"] == "completed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert_eq!(
        status_payload["status"], "completed",
        "payload={status_payload}"
    );
    assert_eq!(status_payload["total"], 1);
    assert_eq!(status_payload["processed"], 1);
    assert_eq!(status_payload["success"], 1);
    assert_eq!(status_payload["failed"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*refresh_hits.lock().expect("mutex should lock"), 1);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&["provider-kiro".to_string()])
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);

    gateway_handle.abort();
    refresh_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_marks_lazy_codex_oauth_refresh_failures_as_invalid() {
    run_admin_oauth_test(
        "gateway_marks_lazy_codex_oauth_refresh_failures_as_invalid",
        gateway_marks_lazy_codex_oauth_refresh_failures_as_invalid_impl,
    );
}

async fn gateway_marks_lazy_codex_oauth_refresh_failures_as_invalid_impl() {
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        post(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": {
                            "message": "Your refresh token has already been used to generate a new access token. Please try signing in again.",
                            "type": "invalid_request_error",
                            "param": serde_json::Value::Null,
                            "code": "refresh_token_reused"
                        }
                    })),
                )
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-lazy",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.expires_at_unix_secs = Some(1);
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"used-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
            ),
        ]);
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository.clone(),
            )
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_oauth_refresh_coordinator_for_tests(oauth_refresh);

    let transport = state
        .read_provider_transport_snapshot(
            "provider-codex",
            "endpoint-codex-cli",
            "key-codex-oauth-lazy",
        )
        .await
        .expect("transport snapshot should load")
        .expect("transport snapshot should exist");
    let resolved = state
        .resolve_local_oauth_request_auth(&transport)
        .await
        .expect("refresh-token reuse should degrade into oauth-unavailable");

    assert_eq!(resolved, None);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-oauth-lazy".to_string()])
        .await
        .expect("keys should list")
        .into_iter()
        .next()
        .expect("oauth key should exist");
    assert!(stored_key.oauth_invalid_at_unix_secs.is_some());
    assert_eq!(
        stored_key.oauth_invalid_reason.as_deref(),
        Some("[REFRESH_FAILED] Token 续期失败 (401): refresh_token 已被使用并轮换，请重新登录授权")
    );
    let oauth_snapshot = stored_key
        .status_snapshot
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|snapshot| snapshot.get("oauth"))
        .and_then(serde_json::Value::as_object)
        .expect("oauth status snapshot should exist");
    assert_eq!(
        oauth_snapshot
            .get("code")
            .and_then(serde_json::Value::as_str),
        Some("invalid")
    );
    assert_eq!(
        oauth_snapshot
            .get("source")
            .and_then(serde_json::Value::as_str),
        Some("oauth_refresh")
    );
    assert_eq!(
        oauth_snapshot
            .get("requires_reauth")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );

    token_handle.abort();
}

#[test]
fn gateway_refreshes_admin_provider_oauth_key_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_refreshes_admin_provider_oauth_key_locally_with_trusted_admin_principal",
        gateway_refreshes_admin_provider_oauth_key_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_refreshes_admin_provider_oauth_key_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        any(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "access_token": "refreshed-codex-access-token",
                    "refresh_token": "refreshed-codex-refresh-token",
                    "token_type": "Bearer",
                    "expires_in": 1800,
                    "scope": "openid email profile offline_access",
                }))
            }
        }),
    );

    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeRequest {
        url: String,
        authorization: String,
    }

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
                });
                let result = aether_contracts::ExecutionResult {
                    request_id: plan.request_id,
                    candidate_id: None,
                    status_code: 401,
                    headers: std::collections::BTreeMap::new(),
                    body: None,
                    telemetry: None,
                    error: None,
                };
                (StatusCode::OK, Json(result))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-refresh",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.oauth_invalid_at_unix_secs = Some(1_700_000_000);
    key.oauth_invalid_reason = Some("[REFRESH_FAILED] stale token".to_string());
    key.status_snapshot = Some(json!({
        "oauth": {
            "code": "expired",
            "label": "已过期",
            "reason": "Access Token 已过期，等待自动续期",
            "expires_at": 1u64,
            "invalid_at": 1_700_000_000u64,
            "source": "expires_at",
            "requires_reauth": false,
            "expiring_soon": false
        },
        "account": {
            "code": "ok",
            "label": null,
            "reason": null,
            "blocked": false,
            "source": null,
            "recoverable": false
        },
        "quota": {
            "code": "unknown",
            "label": null,
            "reason": null,
            "exhausted": false,
            "usage_ratio": null,
            "updated_at": null,
            "reset_seconds": null,
            "plan_type": null
        }
    }));
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"old-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
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
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-refresh/refresh"
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
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["has_refresh_token"], true);
    assert_eq!(payload["email"], "alice@example.com");
    let account_state_recheck_attempted = payload["account_state_recheck_attempted"]
        .as_bool()
        .expect("account_state_recheck_attempted should be bool");
    if account_state_recheck_attempted {
        let account_state_recheck_error = payload["account_state_recheck_error"]
            .as_str()
            .expect("account_state_recheck_error should be string when attempted");
        assert!(
            account_state_recheck_error == "wham/usage API 返回状态码 401"
                || account_state_recheck_error == "wham/usage API 返回状态码 403"
                || account_state_recheck_error.starts_with("wham/usage 请求执行失败:"),
            "unexpected account_state_recheck_error: {account_state_recheck_error}"
        );
    } else {
        assert_eq!(
            payload["account_state_recheck_error"],
            serde_json::Value::Null
        );
    }
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);
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
        "Bearer refreshed-codex-access-token"
    );

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-oauth-refresh".to_string()])
        .await
        .expect("keys should list")
        .into_iter()
        .next()
        .expect("refreshed key should exist");
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .expect("refreshed api key should decrypt");
    assert_eq!(decrypted_api_key, "refreshed-codex-access-token");
    if account_state_recheck_attempted
        && payload["account_state_recheck_error"] == "wham/usage API 返回状态码 401"
    {
        assert!(stored_key.oauth_invalid_at_unix_secs.is_some());
        assert_eq!(
            stored_key.oauth_invalid_reason.as_deref(),
            Some("[OAUTH_EXPIRED] Codex Token 已过期 (401)")
        );
    } else if account_state_recheck_attempted
        && payload["account_state_recheck_error"] == "wham/usage API 返回状态码 403"
    {
        assert!(stored_key.oauth_invalid_at_unix_secs.is_some());
        assert!(stored_key
            .oauth_invalid_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("(403)")));
    } else {
        assert_eq!(stored_key.oauth_invalid_at_unix_secs, None);
        assert_eq!(stored_key.oauth_invalid_reason, None);
    }

    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("refreshed auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(
        auth_config["refresh_token"],
        serde_json::Value::String("refreshed-codex-refresh-token".to_string())
    );
    assert_eq!(
        auth_config["email"],
        serde_json::Value::String("alice@example.com".to_string())
    );
    let status_snapshot = stored_key
        .status_snapshot
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .expect("status snapshot should exist");
    let oauth_snapshot = status_snapshot
        .get("oauth")
        .and_then(serde_json::Value::as_object)
        .expect("oauth snapshot should exist");
    assert_eq!(
        oauth_snapshot.get("expires_at"),
        auth_config.get("expires_at")
    );
    if stored_key
        .oauth_invalid_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("[OAUTH_EXPIRED]"))
    {
        assert_eq!(
            oauth_snapshot
                .get("code")
                .and_then(serde_json::Value::as_str),
            Some("expired")
        );
        assert_eq!(
            oauth_snapshot
                .get("label")
                .and_then(serde_json::Value::as_str),
            Some("已过期")
        );
        assert_eq!(
            oauth_snapshot.get("reason"),
            Some(&json!("Codex Token 已过期 (401)"))
        );
        assert_eq!(
            oauth_snapshot
                .get("requires_reauth")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            oauth_snapshot
                .get("expiring_soon")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    } else if stored_key.oauth_invalid_reason.is_some() {
        assert_eq!(
            oauth_snapshot
                .get("code")
                .and_then(serde_json::Value::as_str),
            Some("invalid")
        );
        assert_eq!(
            oauth_snapshot
                .get("label")
                .and_then(serde_json::Value::as_str),
            Some("已失效")
        );
        assert_eq!(
            oauth_snapshot
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| !reason.trim().is_empty()),
            true
        );
        assert_eq!(
            oauth_snapshot
                .get("requires_reauth")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            oauth_snapshot
                .get("expiring_soon")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    } else {
        assert_eq!(
            oauth_snapshot
                .get("code")
                .and_then(serde_json::Value::as_str),
            Some("expiring")
        );
        assert_eq!(
            oauth_snapshot
                .get("label")
                .and_then(serde_json::Value::as_str),
            Some("即将过期")
        );
        assert_eq!(oauth_snapshot.get("reason"), Some(&serde_json::Value::Null));
        assert_eq!(
            oauth_snapshot
                .get("requires_reauth")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            oauth_snapshot
                .get("expiring_soon")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    gateway_handle.abort();
    execution_runtime_handle.abort();
    token_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_manual_codex_oauth_refresh_reconciles_missing_fixed_endpoint() {
    run_admin_oauth_test(
        "gateway_manual_codex_oauth_refresh_reconciles_missing_fixed_endpoint",
        gateway_manual_codex_oauth_refresh_reconciles_missing_fixed_endpoint_impl,
    );
}

async fn gateway_manual_codex_oauth_refresh_reconciles_missing_fixed_endpoint_impl() {
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        post(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "access_token": "refreshed-codex-access-token",
                    "refresh_token": "refreshed-codex-refresh-token",
                    "expires_in": 3600,
                    "token_type": "Bearer"
                }))
            }
        }),
    );

    let seen_endpoint_id = Arc::new(Mutex::new(None::<String>));
    let seen_endpoint_id_clone = Arc::clone(&seen_endpoint_id);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_endpoint_id_inner = Arc::clone(&seen_endpoint_id_clone);
            async move {
                let plan: ExecutionPlan = serde_json::from_slice(
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
                    headers: std::collections::BTreeMap::new(),
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

    let mut provider = sample_provider("provider-codex-missing-endpoint", "codex", 10);
    provider.provider_type = "codex".to_string();
    let mut key = sample_key(
        "key-codex-missing-endpoint-refresh",
        "provider-codex-missing-endpoint",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"old-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        Vec::new(),
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
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-missing-endpoint-refresh/refresh"
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
    assert_eq!(payload["provider_type"], "codex");
    assert_eq!(payload["has_refresh_token"], true);
    assert_eq!(payload["account_state_recheck_attempted"], true);
    assert_eq!(
        payload["account_state_recheck_error"],
        serde_json::Value::Null
    );
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&["provider-codex-missing-endpoint".to_string()])
        .await
        .expect("endpoints should read");
    let responses_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.api_format == "openai:responses")
        .expect("openai responses endpoint should be reconciled");
    assert!(endpoints
        .iter()
        .any(|endpoint| endpoint.api_format == "openai:search"));
    assert_eq!(
        responses_endpoint.base_url,
        "https://chatgpt.com/backend-api/codex"
    );
    assert_eq!(
        *seen_endpoint_id.lock().expect("mutex should lock"),
        Some(responses_endpoint.id.clone())
    );

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-missing-endpoint-refresh".to_string()])
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("refreshed key should exist");
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_api_key
            .as_deref()
            .expect("api key should exist"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, "refreshed-codex-access-token");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(
        auth_config["refresh_token"],
        "refreshed-codex-refresh-token"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_manual_kiro_oauth_refresh_reconciles_missing_fixed_endpoint() {
    run_admin_oauth_test(
        "gateway_manual_kiro_oauth_refresh_reconciles_missing_fixed_endpoint",
        gateway_manual_kiro_oauth_refresh_reconciles_missing_fixed_endpoint_impl,
    );
}

async fn gateway_manual_kiro_oauth_refresh_reconciles_missing_fixed_endpoint_impl() {
    run_gateway_manual_kiro_oauth_refresh_maintenance_endpoint_test(None, None, true).await;
}

#[test]
fn gateway_manual_kiro_oauth_refresh_uses_disabled_fixed_endpoint_for_maintenance() {
    run_admin_oauth_test(
        "gateway_manual_kiro_oauth_refresh_uses_disabled_fixed_endpoint_for_maintenance",
        gateway_manual_kiro_oauth_refresh_uses_disabled_fixed_endpoint_for_maintenance_impl,
    );
}

async fn gateway_manual_kiro_oauth_refresh_uses_disabled_fixed_endpoint_for_maintenance_impl() {
    let mut endpoint = sample_endpoint(
        "endpoint-kiro-disabled-maintenance",
        "provider-kiro-oauth-refresh",
        "claude:messages",
        "https://q.{region}.amazonaws.com",
    );
    endpoint.is_active = false;

    run_gateway_manual_kiro_oauth_refresh_maintenance_endpoint_test(
        Some(endpoint),
        Some("endpoint-kiro-disabled-maintenance"),
        false,
    )
    .await;
}

async fn run_gateway_manual_kiro_oauth_refresh_maintenance_endpoint_test(
    initial_endpoint: Option<StoredProviderCatalogEndpoint>,
    expected_endpoint_id: Option<&str>,
    expected_endpoint_active: bool,
) {
    let refreshed_access_token = sample_kiro_device_access_token("kiro-refresh@example.com");
    let expected_access_token = refreshed_access_token.clone();
    let refreshed_refresh_token = "s".repeat(120);
    let expected_refresh_token = refreshed_refresh_token.clone();
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/refreshToken",
        post(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            let access_token_inner = refreshed_access_token.clone();
            let refresh_token_inner = refreshed_refresh_token.clone();
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "accessToken": access_token_inner,
                    "refreshToken": refresh_token_inner,
                    "expiresIn": 3600,
                    "profileArn": "arn:aws:kiro:profile/manual-refresh"
                }))
            }
        }),
    );

    let seen_endpoint_id = Arc::new(Mutex::new(None::<String>));
    let seen_endpoint_id_clone = Arc::clone(&seen_endpoint_id);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_endpoint_id_inner = Arc::clone(&seen_endpoint_id_clone);
            async move {
                let plan: ExecutionPlan = serde_json::from_slice(
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
                    headers: std::collections::BTreeMap::new(),
                    body: Some(aether_contracts::ResponseBody {
                        json_body: Some(json!({
                            "subscriptionInfo": {
                                "subscriptionTitle": "KIRO PRO"
                            },
                            "usageBreakdownList": [{
                                "currentUsageWithPrecision": 2.0,
                                "usageLimitWithPrecision": 10.0,
                                "nextDateReset": 1_900_000_000u64
                            }],
                            "desktopUserInfo": {
                                "email": "kiro-refresh@example.com"
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

    let mut provider = sample_provider("provider-kiro-oauth-refresh", "kiro", 10);
    provider.provider_type = "kiro".to_string();
    let mut key = sample_key(
        "key-kiro-oauth-refresh",
        "provider-kiro-oauth-refresh",
        "claude:messages",
        "stale-kiro-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
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
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        initial_endpoint.into_iter().collect(),
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
            "{gateway_url}/api/admin/provider-oauth/keys/key-kiro-oauth-refresh/refresh"
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
    assert_eq!(payload["provider_type"], "kiro");
    assert_eq!(payload["has_refresh_token"], true);
    assert_eq!(payload["account_state_recheck_attempted"], true);
    assert_eq!(
        payload["account_state_recheck_error"],
        serde_json::Value::Null
    );
    assert!(
        *token_hits.lock().expect("mutex should lock") >= 1,
        "Kiro refresh endpoint should be called"
    );

    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&["provider-kiro-oauth-refresh".to_string()])
        .await
        .expect("endpoints should read");
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].api_format, "claude:messages");
    assert_eq!(endpoints[0].base_url, "https://q.{region}.amazonaws.com");
    assert_eq!(endpoints[0].is_active, expected_endpoint_active);
    if let Some(expected_endpoint_id) = expected_endpoint_id {
        assert_eq!(endpoints[0].id, expected_endpoint_id);
    }
    assert_eq!(
        *seen_endpoint_id.lock().expect("mutex should lock"),
        Some(endpoints[0].id.clone())
    );

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-kiro-oauth-refresh".to_string()])
        .await
        .expect("keys should load")
        .into_iter()
        .next()
        .expect("refreshed key should exist");
    let decrypted_api_key = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_api_key
            .as_deref()
            .expect("api key should exist"),
    )
    .expect("api key should decrypt");
    assert_eq!(decrypted_api_key, expected_access_token);
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(auth_config["provider_type"], "kiro");
    assert_eq!(auth_config["refresh_token"], expected_refresh_token);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_marks_manual_oauth_refresh_failures_as_invalid_in_pool_payload() {
    run_admin_oauth_test(
        "gateway_marks_manual_oauth_refresh_failures_as_invalid_in_pool_payload",
        gateway_marks_manual_oauth_refresh_failures_as_invalid_in_pool_payload_impl,
    );
}

async fn gateway_marks_manual_oauth_refresh_failures_as_invalid_in_pool_payload_impl() {
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        post(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": {
                            "message": "Your refresh token has already been used to generate a new access token. Please try signing in again.",
                            "type": "invalid_request_error",
                            "param": serde_json::Value::Null,
                            "code": "refresh_token_reused"
                        }
                    })),
                )
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-refresh-invalid",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.expires_at_unix_secs = Some(4_102_444_800);
    key.status_snapshot = Some(json!({
        "oauth": {
            "code": "valid",
            "label": "有效",
            "reason": serde_json::Value::Null,
            "expires_at": 4_102_444_800u64,
            "invalid_at": serde_json::Value::Null,
            "source": "expires_at",
            "requires_reauth": false,
            "expiring_soon": false
        },
        "account": {
            "code": "ok",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "blocked": false,
            "source": serde_json::Value::Null,
            "recoverable": false
        },
        "quota": {
            "code": "unknown",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "exhausted": false,
            "usage_ratio": serde_json::Value::Null,
            "updated_at": serde_json::Value::Null,
            "reset_seconds": serde_json::Value::Null,
            "plan_type": serde_json::Value::Null
        }
    }));
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"used-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":4102444800}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));

    let (token_url, token_handle) = start_server(token_server).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
            ),
        ]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let refresh_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-refresh-invalid/refresh"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("refresh request should succeed");

    assert_eq!(refresh_response.status(), StatusCode::BAD_REQUEST);
    let refresh_payload: serde_json::Value = refresh_response
        .json()
        .await
        .expect("refresh payload should parse");
    assert_eq!(
        refresh_payload["detail"],
        json!("Token 刷新失败：refresh_token 已被使用并轮换，请重新登录授权")
    );
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let pool_response = client
        .get(format!(
            "{gateway_url}/api/admin/pool/provider-codex/keys?page=1&page_size=50&status=all"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("pool request should succeed");

    assert_eq!(pool_response.status(), StatusCode::OK);
    let pool_payload: serde_json::Value = pool_response
        .json()
        .await
        .expect("pool payload should parse");
    let keys = pool_payload["keys"]
        .as_array()
        .expect("keys should be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0]["oauth_invalid_reason"],
        json!(
            "[REFRESH_FAILED] Token 续期失败 (401): refresh_token 已被使用并轮换，请重新登录授权"
        )
    );
    assert_eq!(
        keys[0]["status_snapshot"]["oauth"]["code"],
        json!("reauth_required")
    );
    assert_eq!(
        keys[0]["status_snapshot"]["oauth"]["reason"],
        json!("Token 续期失败 (401): refresh_token 已被使用并轮换，请重新登录授权")
    );
    assert_eq!(
        keys[0]["status_snapshot"]["oauth"]["source"],
        json!("oauth_refresh")
    );
    assert_eq!(
        keys[0]["status_snapshot"]["oauth"]["requires_reauth"],
        json!(true)
    );
    assert_eq!(
        keys[0]["status_snapshot"]["oauth"]["usable_until_expiry"],
        json!(true)
    );

    gateway_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_auto_removes_manual_oauth_refresh_failure_after_access_token_expiry() {
    run_admin_oauth_test(
        "gateway_auto_removes_manual_oauth_refresh_failure_after_access_token_expiry",
        gateway_auto_removes_manual_oauth_refresh_failure_after_access_token_expiry_impl,
    );
}

async fn gateway_auto_removes_manual_oauth_refresh_failure_after_access_token_expiry_impl() {
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let token_server = Router::new().route(
        "/oauth/token",
        post(move |_request: Request| {
            let token_hits_inner = Arc::clone(&token_hits_clone);
            async move {
                *token_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({
                        "error": {
                            "message": "Could not validate your refresh token. Please try signing in again.",
                            "type": "invalid_request_error",
                            "param": serde_json::Value::Null,
                            "code": "refresh_token_expired"
                        }
                    })),
                )
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10).with_transport_fields(
        true,
        false,
        true,
        None,
        None,
        None,
        None,
        None,
        Some(json!({
            "pool_advanced": {
                "enabled": true,
                "auto_remove_banned_keys": true
            }
        })),
    );
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );
    let mut key = sample_key(
        "key-codex-oauth-refresh-expired",
        "provider-codex",
        "openai:responses",
        "expired-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.expires_at_unix_secs = Some(1);
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"expired-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));
    let (token_url, token_handle) = start_server(token_server).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
            ),
        ]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let refresh_response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-refresh-expired/refresh"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("refresh request should succeed");

    assert_eq!(refresh_response.status(), StatusCode::OK);
    let refresh_payload: serde_json::Value = refresh_response
        .json()
        .await
        .expect("refresh payload should parse");
    assert_eq!(refresh_payload["status"], json!("auto_removed"));
    assert_eq!(refresh_payload["message"], json!("已自动删除"));
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);

    let keys = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-oauth-refresh-expired".to_string()])
        .await
        .expect("keys should read");
    assert!(keys.is_empty());

    gateway_handle.abort();
    token_handle.abort();
}

#[test]
fn gateway_refreshes_admin_provider_oauth_key_locally_via_execution_runtime_provider_proxy_before_system_proxy(
) {
    run_admin_oauth_test(
        "gateway_refreshes_admin_provider_oauth_key_locally_via_execution_runtime_provider_proxy_before_system_proxy",
        gateway_refreshes_admin_provider_oauth_key_locally_via_execution_runtime_provider_proxy_before_system_proxy_impl,
    );
}

async fn gateway_refreshes_admin_provider_oauth_key_locally_via_execution_runtime_provider_proxy_before_system_proxy_impl(
) {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                if plan.request_id == "provider-oauth:local-refresh-token" {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "access_token": "refreshed-codex-access-token",
                                "refresh_token": "refreshed-codex-refresh-token",
                                "token_type": "Bearer",
                                "expires_in": 1800,
                                "scope": "openid email profile offline_access",
                                "email": "alice@example.com",
                                "account_id": "acct-codex-123",
                                "plan_type": "plus"
                            }
                        }
                    }))
                } else {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {}
                        }
                    }))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    provider.proxy = Some(json!({"node_id":"proxy-node-provider","enabled":true}));
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-refresh-provider",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"old-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));
    let mut provider_node = sample_proxy_node("proxy-node-provider");
    provider_node.status = "online".to_string();
    provider_node.is_manual = true;
    provider_node.tunnel_mode = false;
    provider_node.tunnel_connected = false;
    provider_node.proxy_url = Some("http://proxy-provider.example:8080".to_string());
    let mut system_node = sample_proxy_node("proxy-node-system");
    system_node.status = "online".to_string();
    system_node.is_manual = true;
    system_node.tunnel_mode = false;
    system_node.tunnel_connected = false;
    system_node.proxy_url = Some("http://proxy-system.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        provider_node,
        system_node,
    ]));

    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
            ),
        ]);

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_system_config_values_for_tests(vec![(
                    "system_proxy_node_id".to_string(),
                    json!("proxy-node-system"),
                )])
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-refresh-provider/refresh"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let plans = execution_plans.lock().expect("mutex should lock");
    let refresh_plan = plans
        .iter()
        .find(|plan| plan.request_id == "provider-oauth:local-refresh-token")
        .expect("local refresh plan should exist");
    assert_eq!(
        refresh_plan
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.node_id.as_deref()),
        Some("proxy-node-provider")
    );
    assert_eq!(
        refresh_plan
            .headers
            .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
            .map(String::as_str),
        Some("true")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_refreshes_admin_provider_oauth_key_tunnel_proxy_with_direct_refresh_controls() {
    run_admin_oauth_test(
        "gateway_refreshes_admin_provider_oauth_key_tunnel_proxy_with_direct_refresh_controls",
        gateway_refreshes_admin_provider_oauth_key_tunnel_proxy_with_direct_refresh_controls_impl,
    );
}

async fn gateway_refreshes_admin_provider_oauth_key_tunnel_proxy_with_direct_refresh_controls_impl()
{
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                if plan.request_id == "provider-oauth:local-refresh-token" {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "access_token": "refreshed-codex-access-token",
                                "refresh_token": "refreshed-codex-refresh-token",
                                "token_type": "Bearer",
                                "expires_in": 1800,
                                "scope": "openid email profile offline_access",
                                "email": "alice@example.com",
                                "account_id": "acct-codex-123",
                                "plan_type": "plus"
                            }
                        }
                    }))
                } else {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {}
                        }
                    }))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    provider.proxy = Some(json!({
        "mode": "tunnel",
        "node_id": "proxy-node-tunnel",
        "enabled": true,
        "tunnel_base_url": "http://gateway-owner.internal"
    }));
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-refresh-tunnel",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"old-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));

    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
            ),
        ]);

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-refresh-tunnel/refresh"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let plans = execution_plans.lock().expect("mutex should lock");
    let refresh_plan = plans
        .iter()
        .find(|plan| plan.request_id == "provider-oauth:local-refresh-token")
        .expect("local refresh plan should exist");
    assert_eq!(
        refresh_plan
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.mode.as_deref()),
        Some("tunnel")
    );
    assert_eq!(
        refresh_plan
            .headers
            .get(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER)
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        refresh_plan
            .headers
            .get(EXECUTION_REQUEST_HTTP1_ONLY_HEADER)
            .map(String::as_str),
        Some("true")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_consecutive_manual_oauth_refresh_uses_rotated_refresh_token() {
    run_admin_oauth_test(
        "gateway_consecutive_manual_oauth_refresh_uses_rotated_refresh_token",
        gateway_consecutive_manual_oauth_refresh_uses_rotated_refresh_token_impl,
    );
}

async fn gateway_consecutive_manual_oauth_refresh_uses_rotated_refresh_token_impl() {
    let refresh_request_bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let refresh_request_bodies_clone = Arc::clone(&refresh_request_bodies);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let refresh_request_bodies_inner = Arc::clone(&refresh_request_bodies_clone);
            async move {
                if plan.request_id == "provider-oauth:local-refresh-token" {
                    use base64::Engine as _;

                    let body_text = plan
                        .body
                        .body_bytes_b64
                        .as_deref()
                        .and_then(|body| {
                            base64::engine::general_purpose::STANDARD.decode(body).ok()
                        })
                        .and_then(|body| String::from_utf8(body).ok())
                        .unwrap_or_default();
                    refresh_request_bodies_inner
                        .lock()
                        .expect("mutex should lock")
                        .push(body_text.clone());

                    let (access_token, refresh_token) =
                        if body_text.contains("refresh_token=old-codex-refresh-token") {
                            (
                                "refreshed-codex-access-token",
                                "rotated-codex-refresh-token",
                            )
                        } else if body_text.contains("refresh_token=rotated-codex-refresh-token") {
                            (
                                "refreshed-codex-access-token-2",
                                "rotated-codex-refresh-token-2",
                            )
                        } else {
                            return Json(json!({
                                "request_id": plan.request_id,
                                "status_code": 401,
                                "headers": {
                                    "content-type": "application/json"
                                },
                                "body": {
                                    "json_body": {
                                        "error": {
                                            "message": "unexpected refresh token"
                                        }
                                    }
                                }
                            }));
                        };

                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "access_token": access_token,
                                "refresh_token": refresh_token,
                                "token_type": "Bearer",
                                "expires_in": 1800,
                                "scope": "openid email profile offline_access",
                                "email": "alice@example.com",
                                "account_id": "acct-codex-123",
                                "plan_type": "plus"
                            }
                        }
                    }))
                } else {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {}
                        }
                    }))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-consecutive-refresh",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"old-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));

    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
            ),
        ]);

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
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
    let client = reqwest::Client::new();

    for _ in 0..2 {
        let response = client
            .post(format!(
                "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-consecutive-refresh/refresh"
            ))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
    }

    let bodies = refresh_request_bodies
        .lock()
        .expect("mutex should lock")
        .clone();
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies[0].contains("refresh_token=old-codex-refresh-token"),
        "unexpected first refresh body: {}",
        bodies[0]
    );
    assert!(
        bodies[1].contains("refresh_token=rotated-codex-refresh-token"),
        "unexpected second refresh body: {}",
        bodies[1]
    );

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-oauth-consecutive-refresh".to_string()])
        .await
        .expect("keys should list")
        .into_iter()
        .next()
        .expect("refreshed key should exist");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(
        auth_config["refresh_token"],
        "rotated-codex-refresh-token-2"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_concurrent_manual_oauth_refresh_uses_rotated_refresh_token_after_lock_wait() {
    run_admin_oauth_test(
        "gateway_concurrent_manual_oauth_refresh_uses_rotated_refresh_token_after_lock_wait",
        gateway_concurrent_manual_oauth_refresh_uses_rotated_refresh_token_after_lock_wait_impl,
    );
}

async fn gateway_concurrent_manual_oauth_refresh_uses_rotated_refresh_token_after_lock_wait_impl() {
    let refresh_request_bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let refresh_request_bodies_clone = Arc::clone(&refresh_request_bodies);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let refresh_request_bodies_inner = Arc::clone(&refresh_request_bodies_clone);
            async move {
                if plan.request_id == "provider-oauth:local-refresh-token" {
                    use base64::Engine as _;

                    let body_text = plan
                        .body
                        .body_bytes_b64
                        .as_deref()
                        .and_then(|body| {
                            base64::engine::general_purpose::STANDARD.decode(body).ok()
                        })
                        .and_then(|body| String::from_utf8(body).ok())
                        .unwrap_or_default();
                    refresh_request_bodies_inner
                        .lock()
                        .expect("mutex should lock")
                        .push(body_text.clone());

                    let (access_token, refresh_token, delay_ms) =
                        if body_text.contains("refresh_token=old-codex-refresh-token") {
                            (
                                "refreshed-codex-access-token",
                                "rotated-codex-refresh-token",
                                200u64,
                            )
                        } else if body_text.contains("refresh_token=rotated-codex-refresh-token") {
                            (
                                "refreshed-codex-access-token-2",
                                "rotated-codex-refresh-token-2",
                                0u64,
                            )
                        } else {
                            return Json(json!({
                                "request_id": plan.request_id,
                                "status_code": 401,
                                "headers": {
                                    "content-type": "application/json"
                                },
                                "body": {
                                    "json_body": {
                                        "error": {
                                            "message": "Could not validate your refresh token. Please try signing in again."
                                        }
                                    }
                                }
                            }));
                        };

                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }

                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "access_token": access_token,
                                "refresh_token": refresh_token,
                                "token_type": "Bearer",
                                "expires_in": 1800,
                                "scope": "openid email profile offline_access",
                                "email": "alice@example.com",
                                "account_id": "acct-codex-123",
                                "plan_type": "plus"
                            }
                        }
                    }))
                } else {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {}
                        }
                    }))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    provider.proxy = Some(json!({"node_id":"proxy-node-provider","enabled":true}));
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-concurrent-refresh",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.proxy = Some(json!({"node_id":"proxy-node-key","enabled":true}));
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"old-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));
    let mut key_node = sample_proxy_node("proxy-node-key");
    key_node.status = "online".to_string();
    key_node.is_manual = true;
    key_node.tunnel_mode = false;
    key_node.tunnel_connected = false;
    key_node.proxy_url = Some("http://proxy-key.example:8080".to_string());
    let mut provider_node = sample_proxy_node("proxy-node-provider");
    provider_node.status = "online".to_string();
    provider_node.is_manual = true;
    provider_node.tunnel_mode = false;
    provider_node.tunnel_connected = false;
    provider_node.proxy_url = Some("http://proxy-provider.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        key_node,
        provider_node,
    ]));

    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
            ),
        ]);

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let refresh_url = format!(
        "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-concurrent-refresh/refresh"
    );

    let request_a = client
        .post(refresh_url.clone())
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send();
    let request_b = client
        .post(refresh_url)
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send();
    let (response_a, response_b) = tokio::join!(request_a, request_b);
    let response_a = response_a.expect("first request should succeed");
    let response_b = response_b.expect("second request should succeed");

    assert_eq!(response_a.status(), StatusCode::OK);
    assert_eq!(response_b.status(), StatusCode::OK);

    let bodies = refresh_request_bodies
        .lock()
        .expect("mutex should lock")
        .clone();
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies[0].contains("refresh_token=old-codex-refresh-token"),
        "unexpected first refresh body: {}",
        bodies[0]
    );
    assert!(
        bodies[1].contains("refresh_token=rotated-codex-refresh-token"),
        "unexpected second refresh body: {}",
        bodies[1]
    );

    let stored_key = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-oauth-concurrent-refresh".to_string()])
        .await
        .expect("keys should list")
        .into_iter()
        .next()
        .expect("refreshed key should exist");
    let decrypted_auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        stored_key
            .encrypted_auth_config
            .as_deref()
            .expect("auth config should exist"),
    )
    .expect("auth config should decrypt");
    let auth_config: serde_json::Value =
        serde_json::from_str(&decrypted_auth_config).expect("auth config should parse");
    assert_eq!(
        auth_config["refresh_token"],
        "rotated-codex-refresh-token-2"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_manual_oauth_refresh_prefers_fresher_transport_auth_config_over_stale_runtime_cache() {
    run_admin_oauth_test(
        "gateway_manual_oauth_refresh_prefers_fresher_transport_auth_config_over_stale_runtime_cache",
        gateway_manual_oauth_refresh_prefers_fresher_transport_auth_config_over_stale_runtime_cache_impl,
    );
}

async fn gateway_manual_oauth_refresh_prefers_fresher_transport_auth_config_over_stale_runtime_cache_impl(
) {
    let refresh_request_bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let refresh_request_bodies_clone = Arc::clone(&refresh_request_bodies);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let refresh_request_bodies_inner = Arc::clone(&refresh_request_bodies_clone);
            async move {
                if plan.request_id == "provider-oauth:local-refresh-token" {
                    use base64::Engine as _;

                    let body_text = plan
                        .body
                        .body_bytes_b64
                        .as_deref()
                        .and_then(|body| {
                            base64::engine::general_purpose::STANDARD.decode(body).ok()
                        })
                        .and_then(|body| String::from_utf8(body).ok())
                        .unwrap_or_default();
                    refresh_request_bodies_inner
                        .lock()
                        .expect("mutex should lock")
                        .push(body_text.clone());

                    if body_text.contains("refresh_token=old-codex-refresh-token") {
                        return Json(json!({
                            "request_id": plan.request_id,
                            "status_code": 200,
                            "headers": {
                                "content-type": "application/json"
                            },
                            "body": {
                                "json_body": {
                                    "access_token": "cached-codex-access-token",
                                    "refresh_token": "cached-codex-refresh-token",
                                    "token_type": "Bearer",
                                    "expires_in": 1800,
                                    "scope": "openid email profile offline_access",
                                    "email": "alice@example.com",
                                    "account_id": "acct-codex-123",
                                    "plan_type": "plus"
                                }
                            }
                        }));
                    }

                    if body_text.contains("refresh_token=fresh-codex-refresh-token") {
                        return Json(json!({
                            "request_id": plan.request_id,
                            "status_code": 200,
                            "headers": {
                                "content-type": "application/json"
                            },
                            "body": {
                                "json_body": {
                                    "access_token": "fresh-codex-access-token-2",
                                    "refresh_token": "fresh-codex-refresh-token-2",
                                    "token_type": "Bearer",
                                    "expires_in": 1800,
                                    "scope": "openid email profile offline_access",
                                    "email": "alice@example.com",
                                    "account_id": "acct-codex-123",
                                    "plan_type": "plus"
                                }
                            }
                        }));
                    }

                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 401,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "error": {
                                    "message": "Could not validate your refresh token. Please try signing in again."
                                }
                            }
                        }
                    }))
                } else {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {}
                        }
                    }))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-stale-cache",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"old-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1,"updated_at":1700000001}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));

    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
            ),
        ]);

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let app_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_repository_for_tests(
                provider_catalog_repository.clone(),
            )
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_oauth_refresh_coordinator_for_tests(oauth_refresh);

    let stale_transport = app_state
        .read_provider_transport_snapshot(
            "provider-codex",
            "endpoint-codex-cli",
            "key-codex-oauth-stale-cache",
        )
        .await
        .expect("transport should load")
        .expect("transport should exist");
    let cached_entry = app_state
        .force_local_oauth_refresh_entry(&stale_transport)
        .await
        .expect("initial refresh should succeed")
        .expect("initial refresh should return cached entry");
    assert_eq!(
        cached_entry.auth_header_value,
        "Bearer cached-codex-access-token"
    );

    let mut updated_key = provider_catalog_repository
        .list_keys_by_ids(&["key-codex-oauth-stale-cache".to_string()])
        .await
        .expect("keys should list")
        .into_iter()
        .next()
        .expect("key should exist");
    updated_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"fresh-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1,"updated_at":4102444810}"#,
        )
        .expect("updated auth config ciphertext should build"),
    );
    provider_catalog_repository
        .update_key(&updated_key)
        .await
        .expect("key should update");

    let gateway = build_router_with_state(app_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-stale-cache/refresh"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let bodies = refresh_request_bodies
        .lock()
        .expect("mutex should lock")
        .clone();
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies[0].contains("refresh_token=old-codex-refresh-token"),
        "unexpected first refresh body: {}",
        bodies[0]
    );
    assert!(
        bodies[1].contains("refresh_token=fresh-codex-refresh-token"),
        "unexpected second refresh body: {}",
        bodies[1]
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_refreshes_admin_provider_oauth_key_locally_via_execution_runtime_key_proxy_before_system_proxy(
) {
    run_admin_oauth_test(
        "gateway_refreshes_admin_provider_oauth_key_locally_via_execution_runtime_key_proxy_before_system_proxy",
        gateway_refreshes_admin_provider_oauth_key_locally_via_execution_runtime_key_proxy_before_system_proxy_impl,
    );
}

async fn gateway_refreshes_admin_provider_oauth_key_locally_via_execution_runtime_key_proxy_before_system_proxy_impl(
) {
    let execution_plans = Arc::new(Mutex::new(Vec::<ExecutionPlan>::new()));
    let execution_plans_clone = Arc::clone(&execution_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_plans_inner = Arc::clone(&execution_plans_clone);
            async move {
                execution_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(plan.clone());
                if plan.request_id == "provider-oauth:local-refresh-token" {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "access_token": "refreshed-codex-access-token",
                                "refresh_token": "refreshed-codex-refresh-token",
                                "token_type": "Bearer",
                                "expires_in": 1800,
                                "scope": "openid email profile offline_access",
                                "email": "alice@example.com",
                                "account_id": "acct-codex-123",
                                "plan_type": "plus"
                            }
                        }
                    }))
                } else {
                    Json(json!({
                        "request_id": plan.request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {}
                        }
                    }))
                }
            }
        }),
    );

    let mut provider = sample_provider("provider-codex", "codex", 10);
    provider.provider_type = "codex".to_string();
    provider.proxy = Some(json!({"node_id":"proxy-node-provider","enabled":true}));
    let endpoint = sample_endpoint(
        "endpoint-codex-cli",
        "provider-codex",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );

    let mut key = sample_key(
        "key-codex-oauth-refresh",
        "provider-codex",
        "openai:responses",
        "stale-codex-access-token",
    );
    key.auth_type = "oauth".to_string();
    key.proxy = Some(json!({"node_id":"proxy-node-key","enabled":true}));
    key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"old-codex-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config ciphertext should build"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![key],
    ));
    let mut key_node = sample_proxy_node("proxy-node-key");
    key_node.status = "online".to_string();
    key_node.is_manual = true;
    key_node.tunnel_mode = false;
    key_node.tunnel_connected = false;
    key_node.proxy_url = Some("http://proxy-key.example:8080".to_string());
    let mut provider_node = sample_proxy_node("proxy-node-provider");
    provider_node.status = "online".to_string();
    provider_node.is_manual = true;
    provider_node.tunnel_mode = false;
    provider_node.tunnel_connected = false;
    provider_node.proxy_url = Some("http://proxy-provider.example:8080".to_string());
    let mut system_node = sample_proxy_node("proxy-node-system");
    system_node.status = "online".to_string();
    system_node.is_manual = true;
    system_node.tunnel_mode = false;
    system_node.tunnel_connected = false;
    system_node.proxy_url = Some("http://proxy-system.example:8080".to_string());
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        key_node,
        provider_node,
        system_node,
    ]));

    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", "https://oauth.example/oauth/token"),
            ),
        ]);

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .attach_proxy_node_repository_for_tests(proxy_node_repository)
                .with_system_config_values_for_tests(vec![(
                    "system_proxy_node_id".to_string(),
                    json!("proxy-node-system"),
                )])
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-codex-oauth-refresh/refresh"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let plans = execution_plans.lock().expect("mutex should lock");
    let refresh_plan = plans
        .iter()
        .find(|plan| plan.request_id == "provider-oauth:local-refresh-token")
        .expect("local refresh plan should exist");
    assert_eq!(
        refresh_plan
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.node_id.as_deref()),
        Some("proxy-node-key")
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_handles_admin_provider_oauth_unavailable_routes_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_provider_oauth_unavailable_routes_locally_with_trusted_admin_principal",
        gateway_handles_admin_provider_oauth_unavailable_routes_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_provider_oauth_unavailable_routes_locally_with_trusted_admin_principal_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    for path in [
        "/api/admin/provider-oauth/providers/provider-123/import-refresh-token",
        "/api/admin/provider-oauth/providers/provider-123/batch-import",
        "/api/admin/provider-oauth/providers/provider-123/batch-import/tasks",
        "/api/admin/provider-oauth/providers/provider-123/device-authorize",
        "/api/admin/provider-oauth/providers/provider-123/device-poll",
    ] {
        let response = client
            .post(format!("{gateway_url}{path}"))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["detail"], "Admin provider OAuth data unavailable");
    }

    let refresh_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-123/refresh"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(refresh_response.status(), StatusCode::NOT_FOUND);
    let refresh_payload: serde_json::Value = refresh_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(refresh_payload["detail"], "Key 不存在");

    let complete_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/keys/key-123/complete"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(complete_response.status(), StatusCode::BAD_REQUEST);
    let complete_payload: serde_json::Value = complete_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(complete_payload["detail"], "请求体必须是合法的 JSON 对象");

    let provider_complete_response = client
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-123/complete"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(provider_complete_response.status(), StatusCode::BAD_REQUEST);
    let provider_complete_payload: serde_json::Value = provider_complete_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        provider_complete_payload["detail"],
        "请求体必须是合法的 JSON 对象"
    );

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_oauth_supported_types_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_oauth_supported_types_locally_with_trusted_admin_principal",
        gateway_handles_admin_oauth_supported_types_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_oauth_supported_types_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/oauth/supported-types",
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
        .get(format!("{gateway_url}/api/admin/oauth/supported-types"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload.as_array().expect("items should be array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["provider_type"], "linuxdo");
    assert_eq!(items[0]["display_name"], "Linux Do");
    assert_eq!(items[1]["provider_type"], "custom_oidc");
    assert_eq!(items[1]["display_name"], "Custom OIDC");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_oauth_provider_list_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_oauth_provider_list_locally_with_trusted_admin_principal",
        gateway_handles_admin_oauth_provider_list_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_oauth_provider_list_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/oauth/providers",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_oauth_provider_config("linuxdo"),
    ]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_oauth_provider_repository_for_tests(
                repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/oauth/providers"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload.as_array().expect("items should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["provider_type"], "linuxdo");
    assert_eq!(items[0]["has_secret"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_upserts_admin_oauth_provider_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_upserts_admin_oauth_provider_locally_with_trusted_admin_principal",
        gateway_upserts_admin_oauth_provider_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_upserts_admin_oauth_provider_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/oauth/providers/linuxdo",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::default());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_oauth_provider_repository_for_tests(
                repository.clone(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!("{gateway_url}/api/admin/oauth/providers/linuxdo"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "display_name": "Linux Do",
            "client_id": "client-id",
            "client_secret": "secret-value",
            "authorization_url_override": "https://connect.linux.do/oauth2/authorize",
            "token_url_override": "https://connect.linux.do/oauth2/token",
            "userinfo_url_override": "https://connect.linux.do/api/user",
            "scopes": ["openid", "profile"],
            "redirect_uri": "https://backend.example.com/oauth/callback",
            "frontend_callback_url": "https://frontend.example.com/auth/callback",
            "attribute_mapping": {"email": "email"},
            "extra_config": {"team": true},
            "is_enabled": true,
            "force": false
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["provider_type"], "linuxdo");
    assert_eq!(payload["has_secret"], true);
    assert_eq!(payload["is_enabled"], true);
    let stored = repository
        .get_oauth_provider_config("linuxdo")
        .await
        .expect("lookup should succeed")
        .expect("provider should exist");
    assert!(stored.client_secret_encrypted.is_some());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_rejects_custom_oidc_without_allowed_domains() {
    run_admin_oauth_test(
        "gateway_rejects_custom_oidc_without_allowed_domains",
        gateway_rejects_custom_oidc_without_allowed_domains_impl,
    );
}

async fn gateway_rejects_custom_oidc_without_allowed_domains_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/oauth/providers/custom_oidc",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::default());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_oauth_provider_repository_for_tests(
                repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!(
            "{gateway_url}/api/admin/oauth/providers/custom_oidc"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "display_name": "Custom OIDC",
            "client_id": "custom-client",
            "authorization_url_override": "https://idp.example.com/oauth/authorize",
            "token_url_override": "https://idp.example.com/oauth/token",
            "userinfo_url_override": "https://idp.example.com/oauth/userinfo",
            "scopes": ["openid", "profile", "email"],
            "redirect_uri": "https://backend.example.com/oauth/callback",
            "frontend_callback_url": "https://frontend.example.com/auth/callback",
            "attribute_mapping": {"sub": "sub", "email": "email"},
            "extra_config": {},
            "is_enabled": true
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["error"]["message"],
        "custom_oidc 必须在 extra_config.allowed_domains 配置域名白名单"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_upserts_custom_oidc_with_allowed_domains() {
    run_admin_oauth_test(
        "gateway_upserts_custom_oidc_with_allowed_domains",
        gateway_upserts_custom_oidc_with_allowed_domains_impl,
    );
}

async fn gateway_upserts_custom_oidc_with_allowed_domains_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/oauth/providers/custom_oidc",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::default());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_oauth_provider_repository_for_tests(
                repository.clone(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .put(format!(
            "{gateway_url}/api/admin/oauth/providers/custom_oidc"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "display_name": "Custom OIDC",
            "client_id": "custom-client",
            "authorization_url_override": "https://idp.example.com/oauth/authorize",
            "token_url_override": "https://idp.example.com/oauth/token",
            "userinfo_url_override": "https://idp.example.com/oauth/userinfo",
            "scopes": ["openid", "profile", "email"],
            "redirect_uri": "https://backend.example.com/oauth/callback",
            "frontend_callback_url": "https://frontend.example.com/auth/callback",
            "attribute_mapping": {"sub": "id", "email": "profile.email"},
            "extra_config": {"allowed_domains": ["idp.example.com"]},
            "is_enabled": true
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["provider_type"], "custom_oidc");
    assert_eq!(payload["is_enabled"], true);

    let stored = repository
        .get_oauth_provider_config("custom_oidc")
        .await
        .expect("lookup should succeed")
        .expect("provider should exist");
    assert_eq!(
        stored.authorization_url_override.as_deref(),
        Some("https://idp.example.com/oauth/authorize")
    );
    assert_eq!(
        stored
            .extra_config
            .as_ref()
            .and_then(|value| {
                value
                    .get("allowed_domains")
                    .and_then(serde_json::Value::as_array)
            })
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_upserts_multiple_custom_oidc_configs() {
    run_admin_oauth_test(
        "gateway_upserts_multiple_custom_oidc_configs",
        gateway_upserts_multiple_custom_oidc_configs_impl,
    );
}

async fn gateway_upserts_multiple_custom_oidc_configs_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let repository = Arc::new(InMemoryOAuthProviderRepository::default());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_oauth_provider_repository_for_tests(
                repository.clone(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    for (provider_type, display_name, host) in [
        ("custom_oidc_work", "Work OIDC", "work-idp.example.com"),
        (
            "custom_oidc_personal",
            "Personal OIDC",
            "personal-idp.example.com",
        ),
    ] {
        let response = reqwest::Client::new()
            .put(format!(
                "{gateway_url}/api/admin/oauth/providers/{provider_type}"
            ))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({
                "display_name": display_name,
                "client_id": format!("{provider_type}-client"),
                "authorization_url_override": format!("https://{host}/oauth/authorize"),
                "token_url_override": format!("https://{host}/oauth/token"),
                "userinfo_url_override": format!("https://{host}/oauth/userinfo"),
                "scopes": ["openid", "profile", "email"],
                "redirect_uri": format!("https://backend.example.com/api/oauth/{provider_type}/callback"),
                "frontend_callback_url": "https://frontend.example.com/auth/callback",
                "attribute_mapping": {"sub": "id", "email": "profile.email"},
                "extra_config": {"allowed_domains": [host]},
                "is_enabled": true
            }))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["provider_type"], provider_type);
        assert_eq!(payload["display_name"], display_name);
    }

    let stored = repository
        .list_oauth_provider_configs()
        .await
        .expect("list should succeed");
    assert_eq!(stored.len(), 2);
    assert!(stored
        .iter()
        .any(|provider| provider.provider_type == "custom_oidc_work"));
    assert!(stored
        .iter()
        .any(|provider| provider.provider_type == "custom_oidc_personal"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_tests_admin_oauth_linuxdo_endpoints_locally_with_configured_secret() {
    run_admin_oauth_test(
        "gateway_tests_admin_oauth_linuxdo_endpoints_locally_with_configured_secret",
        gateway_tests_admin_oauth_linuxdo_endpoints_locally_with_configured_secret_impl,
    );
}

async fn gateway_tests_admin_oauth_linuxdo_endpoints_locally_with_configured_secret_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/oauth/providers/linuxdo/test",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let authorization_hits = Arc::new(Mutex::new(0usize));
    let authorization_hits_clone = Arc::clone(&authorization_hits);
    let token_hits = Arc::new(Mutex::new(0usize));
    let token_hits_clone = Arc::clone(&token_hits);
    let oauth_endpoints = Router::new()
        .route(
            "/oauth2/authorize",
            any(move |_request: Request| {
                let authorization_hits_inner = Arc::clone(&authorization_hits_clone);
                async move {
                    *authorization_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::BAD_REQUEST, Body::from("missing query"))
                }
            }),
        )
        .route(
            "/oauth2/token",
            any(move |_request: Request| {
                let token_hits_inner = Arc::clone(&token_hits_clone);
                async move {
                    *token_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::METHOD_NOT_ALLOWED,
                        Body::from("method not allowed"),
                    )
                }
            }),
        );

    let repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_oauth_provider_config("linuxdo"),
    ]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (oauth_url, oauth_handle) = start_server(oauth_endpoints).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_oauth_provider_repository_for_tests(
                repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/oauth/providers/linuxdo/test"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "client_id": "client-id",
            "authorization_url_override": format!("{oauth_url}/oauth2/authorize"),
            "token_url_override": format!("{oauth_url}/oauth2/token"),
            "redirect_uri": "http://localhost:8084/api/oauth/linuxdo/callback"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["authorization_url_reachable"], true);
    assert_eq!(payload["token_url_reachable"], true);
    assert_eq!(payload["secret_status"], "configured");
    assert_eq!(*authorization_hits.lock().expect("mutex should lock"), 1);
    assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    oauth_handle.abort();
    let _ = upstream_url;
}

#[test]
fn gateway_tests_admin_oauth_linuxdo_reports_invalid_endpoint_urls() {
    run_admin_oauth_test(
        "gateway_tests_admin_oauth_linuxdo_reports_invalid_endpoint_urls",
        gateway_tests_admin_oauth_linuxdo_reports_invalid_endpoint_urls_impl,
    );
}

async fn gateway_tests_admin_oauth_linuxdo_reports_invalid_endpoint_urls_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/oauth/providers/linuxdo/test",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::default());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_oauth_provider_repository_for_tests(
                repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/oauth/providers/linuxdo/test"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "client_id": "client-id",
            "authorization_url_override": "not-a-url",
            "token_url_override": "not-a-url",
            "redirect_uri": "http://localhost:8084/api/oauth/linuxdo/callback"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["authorization_url_reachable"], false);
    assert_eq!(payload["token_url_reachable"], false);
    assert_eq!(payload["secret_status"], "not_provided");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    let _ = upstream_url;
}

#[test]
fn gateway_deletes_admin_oauth_provider_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_deletes_admin_oauth_provider_locally_with_trusted_admin_principal",
        gateway_deletes_admin_oauth_provider_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_deletes_admin_oauth_provider_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/oauth/providers/linuxdo",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_oauth_provider_config("linuxdo"),
    ]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_oauth_provider_repository_for_tests(
                repository.clone(),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!("{gateway_url}/api/admin/oauth/providers/linuxdo"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["message"], "删除成功");
    assert!(repository
        .get_oauth_provider_config("linuxdo")
        .await
        .expect("lookup should succeed")
        .is_none());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_admin_management_token_detail_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_handles_admin_management_token_detail_locally_with_trusted_admin_principal",
        gateway_handles_admin_management_token_detail_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_handles_admin_management_token_detail_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/management-tokens/{token_id}",
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
            "{gateway_url}/api/admin/management-tokens/mt-admin-1"
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
    assert_eq!(payload["id"], "mt-admin-1");
    assert_eq!(payload["user"]["email"], "alice@example.com");
    assert_eq!(payload["usage_count"], 7);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_creates_updates_and_regenerates_admin_management_token_locally_with_permissions() {
    run_admin_oauth_test(
        "gateway_creates_updates_and_regenerates_admin_management_token_locally_with_permissions",
        gateway_creates_updates_and_regenerates_admin_management_token_locally_with_permissions_impl,
    );
}

async fn gateway_creates_updates_and_regenerates_admin_management_token_locally_with_permissions_impl(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let repository = Arc::new(InMemoryManagementTokenRepository::default());
    let state = AppState::new().expect("gateway should build");
    let admin_user = state
        .create_local_auth_user_with_settings(
            Some("management-admin@example.com".to_string()),
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

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(state.with_data_state_for_tests(
        GatewayDataState::with_management_token_repository_for_tests(repository.clone()),
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let create_response = client
        .post(format!("{gateway_url}/api/admin/management-tokens"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, admin_user.id.as_str())
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "admin-token",
            "description": "admin token",
            "allowed_ips": ["127.0.0.1"],
            "permissions": ["admin:usage:read", "admin:pool:write"],
            "expires_at": "2099-01-01T00:00:00Z",
        }))
        .send()
        .await
        .expect("request should succeed");

    let create_status = create_response.status();
    let create_body = create_response.text().await.expect("body should read");
    assert_eq!(create_status, StatusCode::CREATED, "{create_body}");
    let create_payload: serde_json::Value =
        serde_json::from_str(&create_body).expect("json body should parse");
    let token_id = create_payload["data"]["id"]
        .as_str()
        .expect("token id should exist")
        .to_string();
    assert_eq!(create_payload["message"], "Management Token 创建成功");
    assert_eq!(create_payload["data"]["user"]["id"], json!(admin_user.id));
    assert_eq!(
        create_payload["data"]["permissions"],
        json!(["admin:pool:write", "admin:usage:read"])
    );
    let created_secret = create_payload["token"].as_str().unwrap_or_default();
    assert!(created_secret.starts_with("ae-"));

    let update_response = client
        .put(format!(
            "{gateway_url}/api/admin/management-tokens/{token_id}"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, admin_user.id.as_str())
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "name": "admin-token-updated",
            "description": null,
            "allowed_ips": null,
            "permissions": ["admin:usage:read"],
            "expires_at": null,
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(update_response.status(), StatusCode::OK);
    let update_payload: serde_json::Value = update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(update_payload["message"], "更新成功");
    assert_eq!(update_payload["data"]["name"], "admin-token-updated");
    assert_eq!(
        update_payload["data"]["permissions"],
        json!(["admin:usage:read"])
    );
    assert_eq!(
        update_payload["data"]["allowed_ips"],
        serde_json::Value::Null
    );

    let regenerate_response = client
        .post(format!(
            "{gateway_url}/api/admin/management-tokens/{token_id}/regenerate"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, admin_user.id.as_str())
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(regenerate_response.status(), StatusCode::OK);
    let regenerate_payload: serde_json::Value = regenerate_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(regenerate_payload["message"], "Token 已重新生成");
    assert!(regenerate_payload["token"]
        .as_str()
        .unwrap_or_default()
        .starts_with("ae-"));
    assert_eq!(
        repository
            .get_management_token_with_user(&token_id)
            .await
            .expect("lookup should succeed")
            .expect("token should remain")
            .token
            .permissions,
        Some(json!(["admin:usage:read"]))
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    drop(upstream_url);
}

#[test]
fn gateway_allows_management_token_with_pool_write_for_provider_oauth_batch_import() {
    run_admin_oauth_test(
        "gateway_allows_management_token_with_pool_write_for_provider_oauth_batch_import",
        gateway_allows_management_token_with_pool_write_for_provider_oauth_batch_import_impl,
    );
}

async fn gateway_allows_management_token_with_pool_write_for_provider_oauth_batch_import_impl() {
    let raw_token = "ae-provider-oauth-batch-pool-write";
    let state = AppState::new().expect("gateway should build");
    let admin_user = state
        .create_local_auth_user_with_settings(
            Some("provider-oauth-pool@example.com".to_string()),
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
    let mut management_token = sample_management_token(
        "token-provider-oauth-batch-pool",
        &admin_user.id,
        "provider-oauth-pool",
        true,
    );
    management_token.token.allowed_ips = None;
    management_token.token.permissions = Some(json!(["admin:pool:read", "admin:pool:write"]));
    let management_token_repository =
        Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
            vec![management_token],
            vec![(
                hash_management_token(raw_token),
                "token-provider-oauth-batch-pool".to_string(),
            )],
        ));

    let gateway = build_router_with_state(state.with_data_state_for_tests(
        GatewayDataState::with_management_token_repository_for_tests(management_token_repository),
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-123/batch-import"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{payload}");
    assert_eq!(payload["detail"], "Admin provider OAuth data unavailable");

    gateway_handle.abort();
}

#[test]
fn gateway_rejects_management_token_without_pool_write_for_provider_oauth_batch_import() {
    run_admin_oauth_test(
        "gateway_rejects_management_token_without_pool_write_for_provider_oauth_batch_import",
        gateway_rejects_management_token_without_pool_write_for_provider_oauth_batch_import_impl,
    );
}

async fn gateway_rejects_management_token_without_pool_write_for_provider_oauth_batch_import_impl()
{
    let raw_token = "ae-provider-oauth-batch-pool-denied";
    let state = AppState::new().expect("gateway should build");
    let admin_user = state
        .create_local_auth_user_with_settings(
            Some("provider-oauth-pool-denied@example.com".to_string()),
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
    let mut management_token = sample_management_token(
        "token-provider-oauth-batch-denied",
        &admin_user.id,
        "provider-oauth-denied",
        true,
    );
    management_token.token.allowed_ips = None;
    management_token.token.permissions = Some(json!(["admin:usage:read"]));
    let management_token_repository =
        Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
            vec![management_token],
            vec![(
                hash_management_token(raw_token),
                "token-provider-oauth-batch-denied".to_string(),
            )],
        ));

    let gateway = build_router_with_state(state.with_data_state_for_tests(
        GatewayDataState::with_management_token_repository_for_tests(management_token_repository),
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/provider-oauth/providers/provider-123/batch-import"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "management token permission denied");
    assert_eq!(payload["required_permission"], "admin:pool:write");
    assert_eq!(payload["route_family"], "provider_oauth_manage");

    gateway_handle.abort();
}

#[test]
fn gateway_deletes_admin_management_token_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_deletes_admin_management_token_locally_with_trusted_admin_principal",
        gateway_deletes_admin_management_token_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_deletes_admin_management_token_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/management-tokens/{token_id}",
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
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_management_token_repository_for_tests(repository.clone()),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{gateway_url}/api/admin/management-tokens/mt-admin-1"
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
    assert_eq!(payload["message"], "删除成功");
    assert_eq!(
        repository
            .get_management_token_with_user("mt-admin-1")
            .await
            .expect("lookup should succeed"),
        None
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_toggles_admin_management_token_locally_with_trusted_admin_principal() {
    run_admin_oauth_test(
        "gateway_toggles_admin_management_token_locally_with_trusted_admin_principal",
        gateway_toggles_admin_management_token_locally_with_trusted_admin_principal_impl,
    );
}

async fn gateway_toggles_admin_management_token_locally_with_trusted_admin_principal_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/management-tokens/{token_id}/status",
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
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_management_token_repository_for_tests(repository.clone()),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{gateway_url}/api/admin/management-tokens/mt-admin-1/status"
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
    assert_eq!(payload["message"], "Token 已禁用");
    assert_eq!(payload["data"]["is_active"], false);
    assert_eq!(
        repository
            .get_management_token_with_user("mt-admin-1")
            .await
            .expect("lookup should succeed")
            .expect("token should remain")
            .token
            .is_active,
        false
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_rejects_partial_management_token_for_admin_management_token_routes() {
    run_admin_oauth_test(
        "gateway_rejects_partial_management_token_for_admin_management_token_routes",
        gateway_rejects_partial_management_token_for_admin_management_token_routes_impl,
    );
}

async fn gateway_rejects_partial_management_token_for_admin_management_token_routes_impl() {
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

    let raw_token = "ae-management-partial-access";
    let state = AppState::new().expect("gateway should build");
    let admin_user = state
        .create_local_auth_user_with_settings(
            Some("management-partial@example.com".to_string()),
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
    let mut management_token = sample_management_token(
        "mt-admin-partial",
        &admin_user.id,
        "management-partial",
        true,
    );
    management_token.token.allowed_ips = None;
    management_token.token.permissions = Some(json!(["admin:usage:read"]));
    let repository = Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
        vec![management_token],
        vec![(
            hash_management_token(raw_token),
            "mt-admin-partial".to_string(),
        )],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(state.with_data_state_for_tests(
        GatewayDataState::with_management_token_repository_for_tests(repository),
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/management-tokens"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(raw_token)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "management token permission denied");
    assert_eq!(
        payload["required_permission"],
        "admin:management_tokens:read"
    );
    assert_eq!(payload["route_family"], "management_tokens_manage");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
