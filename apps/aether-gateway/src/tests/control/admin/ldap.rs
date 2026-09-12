use std::sync::{Arc, Mutex};

use aether_data::repository::auth_modules::{
    InMemoryAuthModuleReadRepository, StoredOAuthProviderModuleConfig,
};
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{build_router_with_state, sample_ldap_module_config, start_server, AppState};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

#[tokio::test]
async fn gateway_handles_admin_ldap_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/ldap/config",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        Some(sample_ldap_module_config()),
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_auth_module_repository_for_tests(
                auth_module_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/ldap/config"))
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
    assert_eq!(payload["server_url"], "ldaps://ldap.example.com");
    assert_eq!(payload["bind_dn"], "cn=admin,dc=example,dc=com");
    assert_eq!(payload["base_dn"], "dc=example,dc=com");
    assert_eq!(payload["has_bind_password"], json!(true));
    assert_eq!(payload["user_search_filter"], "(uid={username})");
    assert_eq!(payload["username_attr"], "uid");
    assert_eq!(payload["email_attr"], "mail");
    assert_eq!(payload["display_name_attr"], "displayName");
    assert_eq!(payload["is_enabled"], json!(true));
    assert_eq!(payload["is_exclusive"], json!(false));
    assert_eq!(payload["use_starttls"], json!(true));
    assert_eq!(payload["connect_timeout"], json!(10));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_updates_admin_ldap_config_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/ldap/config",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        Some(sample_ldap_module_config()),
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_auth_module_repository_for_tests(
                auth_module_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let update_response = client
        .put(format!("{gateway_url}/api/admin/ldap/config"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "server_url": "mockldap://ldap.internal.example.com",
            "bind_dn": "cn=svc,dc=example,dc=com",
            "bind_password": "secret123",
            "base_dn": "ou=people,dc=example,dc=com",
            "user_search_filter": "(uid={username})",
            "username_attr": "uid",
            "email_attr": "mail",
            "display_name_attr": "cn",
            "is_enabled": true,
            "is_exclusive": false,
            "use_starttls": false,
            "connect_timeout": 20
        }))
        .send()
        .await
        .expect("update request should succeed");

    let status = update_response.status();
    let update_payload: serde_json::Value = update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={update_payload}");
    assert_eq!(update_payload["message"], "LDAP配置更新成功");

    let get_response = client
        .get(format!("{gateway_url}/api/admin/ldap/config"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    let status = get_response.status();
    let payload: serde_json::Value = get_response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(
        payload["server_url"],
        "mockldap://ldap.internal.example.com"
    );
    assert_eq!(payload["bind_dn"], "cn=svc,dc=example,dc=com");
    assert_eq!(payload["base_dn"], "ou=people,dc=example,dc=com");
    assert_eq!(payload["display_name_attr"], "cn");
    assert_eq!(payload["has_bind_password"], json!(true));
    assert_eq!(payload["connect_timeout"], json!(20));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_tests_admin_ldap_connection_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/ldap/test",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_auth_module_repository_for_tests(
                auth_module_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/ldap/test"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "server_url": "mockldap://ldap.example.com",
            "bind_dn": "cn=admin,dc=example,dc=com",
            "bind_password": "secret123",
            "base_dn": "dc=example,dc=com",
            "use_starttls": false,
            "connect_timeout": 10
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(payload["message"], "连接成功");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_ldap_filter_and_attribute_injection_on_admin_update() {
    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        Some(sample_ldap_module_config()),
    ));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_auth_module_repository_for_tests(
                auth_module_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for (search_filter, username_attr, expected_detail) in [
        (
            "(uid={username})(objectClass=*)",
            "uid",
            "搜索过滤器格式无效",
        ),
        (
            "(uid={username})",
            "uid)(|(objectClass=*)",
            "用户名属性必须是有效的 LDAP 属性名称",
        ),
    ] {
        let response = client
            .put(format!("{gateway_url}/api/admin/ldap/config"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({
                "server_url": "mockldap://ldap.internal.example.com",
                "bind_dn": "cn=svc,dc=example,dc=com",
                "bind_password": "secret123",
                "base_dn": "ou=people,dc=example,dc=com",
                "user_search_filter": search_filter,
                "username_attr": username_attr,
                "email_attr": "mail",
                "display_name_attr": "cn",
                "is_enabled": true,
                "is_exclusive": false,
                "use_starttls": false,
                "connect_timeout": 20
            }))
            .send()
            .await
            .expect("invalid LDAP update should complete locally");

        let status = response.status();
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(status, StatusCode::BAD_REQUEST, "payload={payload}");
        assert!(
            payload["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(expected_detail)),
            "payload={payload}"
        );
    }

    gateway_handle.abort();
}
