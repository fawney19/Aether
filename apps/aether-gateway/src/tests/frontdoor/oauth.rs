use crate::tests::{
    any, build_router, build_router_with_state, start_server, AppState, Arc, Body, Mutex, Request,
    Router, StatusCode,
};
use aether_data::repository::oauth_providers::{
    InMemoryOAuthProviderRepository, StoredOAuthProviderConfig,
};
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, StoredUserSessionRecord,
};
use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
use base64::Engine as _;
use hmac::Mac as _;
use sha2::{Digest, Sha256};

fn sample_identity_oauth_provider(provider_type: &str) -> StoredOAuthProviderConfig {
    StoredOAuthProviderConfig::new(
        provider_type.to_string(),
        "Linux DO".to_string(),
        "client-id".to_string(),
        "https://backend.example.com/oauth/callback".to_string(),
        "https://frontend.example.com/auth/callback".to_string(),
    )
    .expect("oauth provider config should build")
    .with_config_fields(
        None,
        Some("https://connect.linux.do/oauth2/authorize".to_string()),
        Some("https://connect.linux.do/oauth2/token".to_string()),
        Some("https://connect.linux.do/api/user".to_string()),
        Some(vec!["openid".to_string()]),
        Some(serde_json::json!({"email": "email"})),
        None,
        None,
        true,
    )
}

fn oauth_login_cookie_name(state_nonce: &str) -> String {
    format!(
        "__Host-aether_oauth_login_{:x}",
        Sha256::digest(state_nonce.as_bytes())
    )
}

fn oauth_state_from_authorize_location(location: &str) -> String {
    url::Url::parse(location)
        .expect("authorize location should be a URL")
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
        .expect("authorize location should include state")
}

fn cookie_pair_from_set_cookie(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .expect("Set-Cookie should include a cookie pair")
        .to_string()
}

fn build_oauth_test_access_token(
    user: &StoredUserAuthRecord,
    session_id: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> String {
    let header = serde_json::json!({ "alg": "HS256", "typ": "JWT" });
    let payload = serde_json::json!({
        "exp": expires_at.timestamp(),
        "type": "access",
        "user_id": user.id,
        "role": user.role,
        "created_at": user.created_at.map(|value| value.to_rfc3339()),
        "session_id": session_id,
    });
    let header_segment = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&header).expect("JWT header should serialize"));
    let payload_segment = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).expect("JWT payload should serialize"));
    let signing_input = format!("{header_segment}.{payload_segment}");
    let secret = std::env::var("JWT_SECRET_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "aether-rust-test-jwt-secret-32-bytes-minimum".to_string());
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("test JWT secret should be valid");
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();
    format!(
        "{signing_input}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    )
}

#[tokio::test]
async fn gateway_serves_oauth_public_providers_locally_without_hitting_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/oauth/providers"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["providers"].as_array().map(Vec::len), Some(0));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_accepts_oauth_authorize_device_id_header_without_hitting_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/oauth/linuxdo/authorize"))
        .header("x-client-device-id", "device-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "OAuth Provider 不存在或已禁用");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_blocks_configured_oauth_provider_when_oauth_module_disabled() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_identity_oauth_provider("linuxdo"),
    ]));
    let data_state =
        crate::data::GatewayDataState::with_oauth_provider_repository_for_tests(repository)
            .with_system_config_values_for_tests(vec![(
                "module.oauth.enabled".to_string(),
                serde_json::json!(false),
            )]);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let providers_response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/oauth/providers"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(providers_response.status(), StatusCode::OK);
    let providers_payload: serde_json::Value = providers_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        providers_payload["providers"].as_array().map(Vec::len),
        Some(0)
    );

    let authorize_response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/oauth/linuxdo/authorize"))
        .header("x-client-device-id", "device-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(authorize_response.status(), StatusCode::NOT_FOUND);
    let authorize_payload: serde_json::Value = authorize_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(authorize_payload["detail"], "OAuth Provider 不存在或已禁用");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_serves_configured_oauth_provider_when_oauth_module_enabled() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_identity_oauth_provider("linuxdo"),
    ]));
    let data_state =
        crate::data::GatewayDataState::with_oauth_provider_repository_for_tests(repository)
            .with_system_config_values_for_tests(vec![(
                "module.oauth.enabled".to_string(),
                serde_json::json!(true),
            )]);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let providers_response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/oauth/providers"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(providers_response.status(), StatusCode::OK);
    let providers_payload: serde_json::Value = providers_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        providers_payload["providers"],
        serde_json::json!([{
            "provider_type": "linuxdo",
            "display_name": "Linux DO"
        }])
    );

    let authorize_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client should build");
    let authorize_response = authorize_client
        .get(format!("{gateway_url}/api/oauth/linuxdo/authorize"))
        .header("x-client-device-id", "device-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(authorize_response.status(), StatusCode::FOUND);
    let location = authorize_response
        .headers()
        .get(http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("location should exist");
    assert!(location.starts_with("https://connect.linux.do/oauth2/authorize?"));
    assert!(location.contains("client_id=client-id"));
    assert!(location.contains("redirect_uri=https%3A%2F%2Fbackend.example.com%2Foauth%2Fcallback"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_oauth_callback_without_browser_binding_without_consuming_state() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_identity_oauth_provider("linuxdo"),
    ]));
    let data_state =
        crate::data::GatewayDataState::with_oauth_provider_repository_for_tests(repository)
            .with_system_config_values_for_tests(vec![(
                "module.oauth.enabled".to_string(),
                serde_json::json!(true),
            )]);
    let runtime_state = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state)
        .with_runtime_state(runtime_state);

    let binding_hash = format!("{:x}", Sha256::digest(b"correct-cookie"));
    let missing_cookie_state = crate::oauth::StoredIdentityOAuthState::login(
        "linuxdo",
        "device-csrf-test",
        Some("pkce-verifier".to_string()),
        Some(binding_hash.clone()),
    );
    let wrong_cookie_state = crate::oauth::StoredIdentityOAuthState::login(
        "linuxdo",
        "device-csrf-test",
        Some("pkce-verifier".to_string()),
        Some(binding_hash),
    );
    crate::oauth::save_identity_oauth_state(&state, &missing_cookie_state)
        .await
        .expect("missing-cookie state should save");
    crate::oauth::save_identity_oauth_state(&state, &wrong_cookie_state)
        .await
        .expect("wrong-cookie state should save");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client should build");

    for (state_nonce, cookie_header) in [
        (missing_cookie_state.nonce.as_str(), None),
        (
            wrong_cookie_state.nonce.as_str(),
            Some(format!(
                "{}=wrong-cookie",
                oauth_login_cookie_name(&wrong_cookie_state.nonce)
            )),
        ),
    ] {
        let mut request = client.get(format!(
            "{gateway_url}/api/oauth/linuxdo/callback?code=provider-code&state={state_nonce}"
        ));
        if let Some(cookie_header) = cookie_header.as_deref() {
            request = request.header(http::header::COOKIE, cookie_header);
        }
        let response = request
            .send()
            .await
            .expect("callback request should succeed");
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("invalid callback should redirect");
        assert!(
            location.contains("invalid_state"),
            "unexpected location: {location}"
        );
        let expected_cookie_name = oauth_login_cookie_name(state_nonce);
        let clear_cookies = response
            .headers()
            .get_all(http::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>();
        assert_eq!(clear_cookies.len(), 1);
        assert!(clear_cookies[0].starts_with(&format!("{expected_cookie_name}=;")));
        assert!(clear_cookies[0].contains("Max-Age=0"));
    }

    for state_nonce in [&missing_cookie_state.nonce, &wrong_cookie_state.nonce] {
        assert!(state
            .runtime_kv_get(&crate::oauth::identity_oauth_state_storage_key(state_nonce))
            .await
            .expect("OAuth state lookup should succeed")
            .is_some());

        let cookie_header = format!("{}=correct-cookie", oauth_login_cookie_name(state_nonce));
        let response = client
            .get(format!(
                "{gateway_url}/api/oauth/linuxdo/callback?error=access_denied&state={state_nonce}"
            ))
            .header(http::header::COOKIE, &cookie_header)
            .send()
            .await
            .expect("bound callback request should succeed");
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("denied callback should redirect");
        assert!(location.contains("authorization_denied"));
        assert!(state
            .runtime_kv_get(&crate::oauth::identity_oauth_state_storage_key(state_nonce))
            .await
            .expect("OAuth state lookup should succeed")
            .is_none());

        let replay = client
            .get(format!(
                "{gateway_url}/api/oauth/linuxdo/callback?error=access_denied&state={state_nonce}"
            ))
            .header(http::header::COOKIE, cookie_header)
            .send()
            .await
            .expect("replayed callback request should succeed");
        let replay_location = replay
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("replayed callback should redirect");
        assert!(replay_location.contains("invalid_state"));
    }
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_keeps_parallel_oauth_login_cookies_independent_and_clears_only_consumed_state() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );
    let repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_identity_oauth_provider("linuxdo"),
    ]));
    let data_state =
        crate::data::GatewayDataState::with_oauth_provider_repository_for_tests(repository)
            .with_system_config_values_for_tests(vec![(
                "module.oauth.enabled".to_string(),
                serde_json::json!(true),
            )]);
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state)
        .with_runtime_state(Arc::new(RuntimeState::memory(
            MemoryRuntimeStateConfig::default(),
        )));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client should build");
    let first_authorize = client
        .get(format!("{gateway_url}/api/oauth/linuxdo/authorize"))
        .header("x-client-device-id", "parallel-device");
    let second_authorize = client
        .get(format!("{gateway_url}/api/oauth/linuxdo/authorize"))
        .header("x-client-device-id", "parallel-device");
    let (first_response, second_response) =
        tokio::join!(first_authorize.send(), second_authorize.send());
    let first_response = first_response.expect("first authorize request should succeed");
    let second_response = second_response.expect("second authorize request should succeed");
    assert_eq!(first_response.status(), StatusCode::FOUND);
    assert_eq!(second_response.status(), StatusCode::FOUND);

    let first_location = first_response
        .headers()
        .get(http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("first authorize location should exist");
    let second_location = second_response
        .headers()
        .get(http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("second authorize location should exist");
    let first_state = oauth_state_from_authorize_location(first_location);
    let second_state = oauth_state_from_authorize_location(second_location);
    assert_ne!(first_state, second_state);

    let first_cookie_name = oauth_login_cookie_name(&first_state);
    let second_cookie_name = oauth_login_cookie_name(&second_state);
    assert_ne!(first_cookie_name, second_cookie_name);
    let first_cookie = first_response
        .headers()
        .get(http::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(cookie_pair_from_set_cookie)
        .expect("first authorize response should set a login cookie");
    let second_cookie = second_response
        .headers()
        .get(http::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(cookie_pair_from_set_cookie)
        .expect("second authorize response should set a login cookie");
    assert!(first_cookie.starts_with(&format!("{first_cookie_name}=")));
    assert!(second_cookie.starts_with(&format!("{second_cookie_name}=")));

    let combined_cookie_header = format!("{first_cookie}; {second_cookie}");
    let first_callback = client
        .get(format!(
            "{gateway_url}/api/oauth/linuxdo/callback?error=access_denied&state={first_state}"
        ))
        .header(http::header::COOKIE, combined_cookie_header)
        .send()
        .await
        .expect("first callback request should succeed");
    assert_eq!(first_callback.status(), StatusCode::FOUND);
    let first_callback_location = first_callback
        .headers()
        .get(http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("first callback location should exist");
    assert!(first_callback_location.contains("authorization_denied"));
    let first_clears = first_callback
        .headers()
        .get_all(http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert_eq!(first_clears.len(), 1);
    assert!(first_clears[0].starts_with(&format!("{first_cookie_name}=;")));
    assert!(!first_clears[0].starts_with(&format!("{second_cookie_name}=;")));

    let second_callback = client
        .get(format!(
            "{gateway_url}/api/oauth/linuxdo/callback?error=access_denied&state={second_state}"
        ))
        .header(http::header::COOKIE, second_cookie)
        .send()
        .await
        .expect("second callback request should succeed");
    assert_eq!(second_callback.status(), StatusCode::FOUND);
    let second_callback_location = second_callback
        .headers()
        .get(http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("second callback location should exist");
    assert!(second_callback_location.contains("authorization_denied"));
    let second_clears = second_callback
        .headers()
        .get_all(http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    assert_eq!(second_clears.len(), 1);
    assert!(second_clears[0].starts_with(&format!("{second_cookie_name}=;")));
    assert!(!second_clears[0].starts_with(&format!("{first_cookie_name}=;")));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_requires_auth_for_oauth_user_bindable_providers_without_hitting_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/user/oauth/bindable-providers"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "缺少用户凭证");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_oauth_account_lists_are_never_cacheable() {
    let provider_repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_identity_oauth_provider("linuxdo"),
    ]));
    let now = chrono::Utc::now();
    let user = StoredUserAuthRecord::new(
        "oauth-list-user".to_string(),
        Some("oauth-list@example.com".to_string()),
        true,
        "oauth-list-user".to_string(),
        Some("unused-password-hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(now),
        Some(now),
    )
    .expect("test user should build");
    let session_id = "oauth-list-session";
    let device_id = "oauth-list-device";
    let session = StoredUserSessionRecord::new(
        session_id.to_string(),
        user.id.clone(),
        device_id.to_string(),
        None,
        StoredUserSessionRecord::hash_refresh_token("unused-refresh-token"),
        None,
        None,
        Some(now),
        Some(now + chrono::Duration::days(1)),
        None,
        None,
        Some("127.0.0.1".to_string()),
        Some("oauth-list-test".to_string()),
        Some(now),
        Some(now),
    )
    .expect("test session should build");
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users([user.clone()]));
    let data_state = crate::data::GatewayDataState::with_oauth_provider_repository_for_tests(
        provider_repository,
    )
    .with_system_config_values_for_tests(vec![(
        "module.oauth.enabled".to_string(),
        serde_json::json!(true),
    )])
    .with_user_reader(user_repository);
    let access_token =
        build_oauth_test_access_token(&user, session_id, now + chrono::Duration::hours(1));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state)
        .with_auth_users_for_tests([user])
        .with_auth_session_for_tests(session);
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for path in [
        "/api/user/oauth/bindable-providers",
        "/api/user/oauth/links",
    ] {
        let response = client
            .get(format!("{gateway_url}{path}"))
            .header("authorization", format!("Bearer {access_token}"))
            .header("x-client-device-id", device_id)
            .send()
            .await
            .expect("OAuth account list request should succeed");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response
                .headers()
                .get(http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store"),
            "{path}"
        );
        assert_eq!(
            response
                .headers()
                .get(http::header::PRAGMA)
                .and_then(|value| value.to_str().ok()),
            Some("no-cache"),
            "{path}"
        );
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_requires_auth_for_oauth_user_bind_token_without_hitting_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/user/oauth/linuxdo/bind-token"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "缺少用户凭证");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_oauth_bind_token_returns_authorize_url_and_stores_bound_browser_state() {
    let repository = Arc::new(InMemoryOAuthProviderRepository::seed(vec![
        sample_identity_oauth_provider("linuxdo"),
    ]));
    let data_state =
        crate::data::GatewayDataState::with_oauth_provider_repository_for_tests(repository)
            .with_system_config_values_for_tests(vec![(
                "module.oauth.enabled".to_string(),
                serde_json::json!(true),
            )]);
    let now = chrono::Utc::now();
    let user = StoredUserAuthRecord::new(
        "oauth-bind-user".to_string(),
        Some("oauth-bind@example.com".to_string()),
        true,
        "oauth-bind-user".to_string(),
        Some("unused-password-hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(now),
        Some(now),
    )
    .expect("test user should build");
    let session_id = "oauth-bind-session";
    let device_id = "oauth-bind-device";
    let session = StoredUserSessionRecord::new(
        session_id.to_string(),
        user.id.clone(),
        device_id.to_string(),
        None,
        StoredUserSessionRecord::hash_refresh_token("unused-refresh-token"),
        None,
        None,
        Some(now),
        Some(now + chrono::Duration::days(1)),
        None,
        None,
        Some("127.0.0.1".to_string()),
        Some("oauth-bind-test".to_string()),
        Some(now),
        Some(now),
    )
    .expect("test session should build");
    let access_token =
        build_oauth_test_access_token(&user, session_id, now + chrono::Duration::hours(1));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state)
        .with_auth_users_for_tests([user.clone()])
        .with_auth_session_for_tests(session);
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/user/oauth/linuxdo/bind-token"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", device_id)
        .header("user-agent", "AetherOAuthBindTest/1.0")
        .send()
        .await
        .expect("bind-token request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get(http::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("bind-token response should set the browser binding cookie")
        .to_string();
    assert!(set_cookie.contains("Path=/"));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    let cookie_pair = cookie_pair_from_set_cookie(&set_cookie);
    let (cookie_name, browser_binding) = cookie_pair
        .split_once('=')
        .expect("browser binding cookie should contain a value");

    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert!(payload.get("bind_token").is_none());
    let authorize_url = payload["authorize_url"]
        .as_str()
        .expect("bind-token response should include authorize_url");
    let state_nonce = oauth_state_from_authorize_location(authorize_url);
    assert_eq!(cookie_name, oauth_login_cookie_name(&state_nonce));

    let raw_state = state
        .runtime_kv_get(&crate::oauth::identity_oauth_state_storage_key(
            &state_nonce,
        ))
        .await
        .expect("OAuth state lookup should succeed")
        .expect("OAuth bind state should be stored");
    assert!(crate::handlers::shared::runtime_secret_payload_is_sealed(
        &raw_state
    ));
    assert!(!raw_state.contains("pkce_verifier"));
    let stored = crate::oauth::load_identity_oauth_state(&state, &state_nonce)
        .await
        .expect("OAuth state lookup should succeed")
        .expect("OAuth state should decrypt");
    assert_eq!(stored.mode, crate::oauth::IdentityOAuthStateMode::Bind);
    assert_eq!(stored.provider_type, "linuxdo");
    assert_eq!(stored.client_device_id, device_id);
    assert_eq!(stored.bind_user_id.as_deref(), Some(user.id.as_str()));
    assert_eq!(stored.bind_session_id.as_deref(), Some(session_id));
    let expected_binding_hash = format!("{:x}", Sha256::digest(browser_binding.as_bytes()));
    assert_eq!(
        stored.browser_binding_hash.as_deref(),
        Some(expected_binding_hash.as_str())
    );

    gateway_handle.abort();
}
