use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes};
use axum::routing::any;
use axum::{extract::Request, Router};
use http::{HeaderMap, HeaderValue, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;

use super::super::super::send_request;
use super::super::{build_router_with_state, start_server, AppState};
use crate::admin_api::{
    maybe_build_local_admin_security_response, AdminAppState, AdminRequestContext,
};
use crate::audit::AdminAuditEvent;
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER,
    TRUSTED_ADMIN_USER_ID_HEADER, TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::control::resolve_public_request_context;

#[tokio::test]
async fn gateway_blocks_blacklisted_ip_before_routing() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_security_blacklist_for_tests([(
                "127.0.0.1".to_string(),
                "blocked".to_string(),
            )]),
    );
    let request = Request::builder()
        .uri("/api/public/system")
        .body(Body::empty())
        .expect("request should build");

    let response = send_request(gateway, request).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload = response
        .into_body()
        .collect()
        .await
        .expect("body should collect")
        .to_bytes();
    let payload: serde_json::Value =
        serde_json::from_slice(&payload).expect("response should be json");
    assert_eq!(payload["error"]["message"], "当前 IP 已被禁止访问");
}

#[tokio::test]
async fn gateway_blocks_forwarded_ip_from_trusted_proxy() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_security_blacklist_for_tests([(
                "203.0.113.8".to_string(),
                "blocked".to_string(),
            )]),
    );
    let request = Request::builder()
        .uri("/api/public/system")
        .header("x-real-ip", "203.0.113.8")
        .body(Body::empty())
        .expect("request should build");

    let response = send_request(gateway, request).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_security_whitelist_matches_cidr() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_admin_security_whitelist_for_tests(["203.0.113.0/24".to_string()]);

    assert!(state
        .admin_security_ip_whitelisted("203.0.113.8".parse().expect("valid ip"))
        .await
        .expect("whitelist check should succeed"));
    assert!(!state
        .admin_security_ip_whitelisted("198.51.100.8".parse().expect("valid ip"))
        .await
        .expect("whitelist check should succeed"));
}

#[tokio::test]
async fn admin_security_blacklist_cache_tracks_local_mutations() {
    let state = AppState::new().expect("gateway should build");
    let ip_address = "203.0.113.9".parse().expect("valid ip");

    assert!(!state
        .admin_security_ip_blacklisted(ip_address)
        .await
        .expect("initial blacklist check should succeed"));
    state
        .add_admin_security_blacklist("203.0.113.9", "manual", None)
        .await
        .expect("blacklist add should succeed");
    assert!(state
        .admin_security_ip_blacklisted(ip_address)
        .await
        .expect("cached blacklist check should succeed"));
    state
        .remove_admin_security_blacklist("203.0.113.9")
        .await
        .expect("blacklist remove should succeed");
    assert!(!state
        .admin_security_ip_blacklisted(ip_address)
        .await
        .expect("updated blacklist check should succeed"));
}

#[tokio::test]
async fn admin_security_whitelist_cache_invalidates_after_mutation() {
    let state = AppState::new().expect("gateway should build");
    let ip_address = "203.0.113.10".parse().expect("valid ip");

    assert!(!state
        .admin_security_ip_whitelisted(ip_address)
        .await
        .expect("initial whitelist check should succeed"));
    state
        .add_admin_security_whitelist("203.0.113.0/24")
        .await
        .expect("whitelist add should succeed");
    assert!(state
        .admin_security_ip_whitelisted(ip_address)
        .await
        .expect("updated whitelist check should succeed"));
    state
        .remove_admin_security_whitelist("203.0.113.0/24")
        .await
        .expect("whitelist remove should succeed");
    assert!(!state
        .admin_security_ip_whitelisted(ip_address)
        .await
        .expect("removed whitelist check should succeed"));
}

async fn send_admin_security_request(
    gateway: Router,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value, usize) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        path,
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let mut request = client
        .request(method, format!("{gateway_url}{path}"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123");
    if let Some(body) = body {
        request = request.json(&body);
    }

    let response = request.send().await.expect("request should succeed");
    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let upstream_count = *upstream_hits.lock().expect("mutex should lock");

    gateway_handle.abort();
    upstream_handle.abort();

    (status, payload, upstream_count)
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

async fn local_admin_security_response(
    state: &AppState,
    method: http::Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> axum::response::Response<Body> {
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
    maybe_build_local_admin_security_response(
        &AdminAppState::new(state),
        &AdminRequestContext::new(&request_context),
        body_bytes.as_ref(),
    )
    .await
    .expect("local security response should build")
    .expect("security route should resolve locally")
}

#[tokio::test]
async fn gateway_handles_admin_security_blacklist_add_locally_with_trusted_admin_principal() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::POST,
        "/api/admin/security/ip/blacklist",
        Some(json!({ "ip_address": "1.2.3.4", "reason": "manual", "ttl": 60 })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], "IP 1.2.3.4 已加入黑名单");
    assert_eq!(payload["reason"], "manual");
    assert_eq!(payload["ttl"], 60);
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn gateway_rejects_invalid_admin_security_blacklist_ip() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::POST,
        "/api/admin/security/ip/blacklist",
        Some(json!({
            "ip_address": "not-an-ip",
            "reason": "invalid"
        })),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(payload["detail"], "请求数据验证失败");
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn local_admin_security_blacklist_add_attaches_explicit_audit() {
    let state = AppState::new().expect("gateway should build");
    let response = local_admin_security_response(
        &state,
        http::Method::POST,
        "/api/admin/security/ip/blacklist",
        Some(json!({ "ip_address": "1.2.3.4", "reason": "manual", "ttl": 60 })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("blacklist add should attach audit");
    assert_eq!(audit.event_name, "admin_security_blacklist_added");
    assert_eq!(audit.action, "add_security_blacklist_entry");
    assert_eq!(audit.target_type, "security_blacklist_entry");
    assert_eq!(audit.target_id, "1.2.3.4");
}

#[tokio::test]
async fn gateway_handles_admin_security_blacklist_remove_locally_with_trusted_admin_principal() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_security_blacklist_for_tests([(
                "1.2.3.4".to_string(),
                "manual".to_string(),
            )]),
    );

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::DELETE,
        "/api/admin/security/ip/blacklist/1.2.3.4",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], "IP 1.2.3.4 已从黑名单移除");
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn gateway_rejects_admin_security_blacklist_remove_without_ip_address() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::DELETE,
        "/api/admin/security/ip/blacklist/",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(payload["detail"], "缺少 ip_address");
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn gateway_handles_admin_security_blacklist_stats_locally_with_trusted_admin_principal() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_security_blacklist_for_tests([
                ("1.2.3.4".to_string(), "manual".to_string()),
                ("5.6.7.8".to_string(), "abuse".to_string()),
            ]),
    );

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::GET,
        "/api/admin/security/ip/blacklist/stats",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["available"], true);
    assert_eq!(payload["total"], 2);
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn local_admin_security_blacklist_list_attaches_explicit_audit() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_admin_security_blacklist_for_tests([
            ("5.6.7.8".to_string(), "abuse".to_string()),
            ("1.2.3.4".to_string(), "manual".to_string()),
        ]);
    let response = local_admin_security_response(
        &state,
        http::Method::GET,
        "/api/admin/security/ip/blacklist",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("blacklist list should attach audit");
    assert_eq!(audit.event_name, "admin_security_blacklist_viewed");
    assert_eq!(audit.action, "view_security_blacklist");
    assert_eq!(audit.target_type, "security_blacklist");
    assert_eq!(audit.target_id, "global");
}

#[tokio::test]
async fn gateway_handles_admin_security_whitelist_add_locally_with_trusted_admin_principal() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::POST,
        "/api/admin/security/ip/whitelist",
        Some(json!({ "ip_address": "1.2.3.4" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], "IP 1.2.3.4 已加入白名单");
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn local_admin_security_whitelist_add_attaches_explicit_audit() {
    let state = AppState::new().expect("gateway should build");
    let response = local_admin_security_response(
        &state,
        http::Method::POST,
        "/api/admin/security/ip/whitelist",
        Some(json!({ "ip_address": "1.2.3.4" })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("whitelist add should attach audit");
    assert_eq!(audit.event_name, "admin_security_whitelist_added");
    assert_eq!(audit.action, "add_security_whitelist_entry");
    assert_eq!(audit.target_type, "security_whitelist_entry");
    assert_eq!(audit.target_id, "1.2.3.4");
}

#[tokio::test]
async fn gateway_handles_admin_security_whitelist_remove_locally_with_trusted_admin_principal() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_security_whitelist_for_tests(["1.2.3.4".to_string()]),
    );

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::DELETE,
        "/api/admin/security/ip/whitelist/1.2.3.4",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], "IP 1.2.3.4 已从白名单移除");
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn gateway_removes_percent_encoded_whitelist_cidr() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_security_whitelist_for_tests(["10.0.0.0/24".to_string()]),
    );

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::DELETE,
        "/api/admin/security/ip/whitelist/10.0.0.0%2F24",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["success"], true);
    assert_eq!(payload["message"], "IP 10.0.0.0/24 已从白名单移除");
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn gateway_rejects_admin_security_whitelist_remove_without_ip_address() {
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::DELETE,
        "/api/admin/security/ip/whitelist/",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(payload["detail"], "缺少 ip_address");
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn gateway_handles_admin_security_whitelist_list_locally_with_trusted_admin_principal() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_security_whitelist_for_tests([
                "10.0.0.0/24".to_string(),
                "1.2.3.4".to_string(),
            ]),
    );

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::GET,
        "/api/admin/security/ip/whitelist",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["whitelist"], json!(["1.2.3.4", "10.0.0.0/24"]));
    assert_eq!(payload["total"], 2);
    assert_eq!(upstream_count, 0);
}

#[tokio::test]
async fn local_admin_security_whitelist_list_attaches_explicit_audit() {
    let state = AppState::new()
        .expect("gateway should build")
        .with_admin_security_whitelist_for_tests([
            "10.0.0.0/24".to_string(),
            "1.2.3.4".to_string(),
        ]);
    let response = local_admin_security_response(
        &state,
        http::Method::GET,
        "/api/admin/security/ip/whitelist",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("whitelist list should attach audit");
    assert_eq!(audit.event_name, "admin_security_whitelist_viewed");
    assert_eq!(audit.action, "view_security_whitelist");
    assert_eq!(audit.target_type, "security_whitelist");
    assert_eq!(audit.target_id, "global");
}

#[tokio::test]
async fn gateway_handles_admin_security_blacklist_list_locally_with_trusted_admin_principal() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_admin_security_blacklist_for_tests([
                ("5.6.7.8".to_string(), "abuse".to_string()),
                ("1.2.3.4".to_string(), "manual".to_string()),
            ]),
    );

    let (status, payload, upstream_count) = send_admin_security_request(
        gateway,
        reqwest::Method::GET,
        "/api/admin/security/ip/blacklist",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(payload["total"], 2);
    let items = payload["items"].as_array().expect("items array exists");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["ip_address"], "1.2.3.4");
    assert_eq!(items[0]["reason"], "manual");
    assert_eq!(items[1]["ip_address"], "5.6.7.8");
    assert_eq!(items[1]["reason"], "abuse");
    assert_eq!(upstream_count, 0);
}
