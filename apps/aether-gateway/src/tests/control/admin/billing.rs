use std::sync::{Arc, Mutex};

use aether_data::repository::users::InMemoryUserReadRepository;
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{build_router_with_state, start_server, AppState};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

async fn assert_admin_billing_get_returns_local(
    path: &str,
) -> (serde_json::Value, Arc<Mutex<usize>>) {
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
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}{path}"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    gateway_handle.abort();
    upstream_handle.abort();
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(status, StatusCode::OK);
    (payload, upstream_hits)
}

async fn send_admin_billing_request(
    gateway_url: &str,
    method: http::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> reqwest::Response {
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
    request.send().await.expect("request should succeed")
}

#[tokio::test]
async fn gateway_handles_admin_billing_presets_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/billing/presets",
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
        .get(format!("{gateway_url}/api/admin/billing/presets"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["items"][0]["name"], "aether-core");
    assert_eq!(payload["items"][0]["version"], "1.0");
    assert_eq!(payload["items"][0]["collector_count"], 22);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_billing_apply_preset_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/billing/presets/apply",
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
            .with_admin_billing_collectors_for_tests(
                Vec::<crate::AdminBillingCollectorRecord>::new(),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let merge_response = send_admin_billing_request(
        &gateway_url,
        http::Method::POST,
        "/api/admin/billing/presets/apply",
        Some(json!({"preset":"aether-core","mode":"merge"})),
    )
    .await;
    assert_eq!(merge_response.status(), StatusCode::OK);
    let merge_payload: serde_json::Value =
        merge_response.json().await.expect("json body should parse");
    assert_eq!(merge_payload["ok"], json!(true));
    assert_eq!(merge_payload["preset"], json!("aether-core"));
    assert_eq!(merge_payload["mode"], json!("merge"));
    assert_eq!(merge_payload["created"], json!(22));
    assert_eq!(merge_payload["updated"], json!(0));
    assert_eq!(merge_payload["skipped"], json!(0));
    assert_eq!(merge_payload["errors"], json!([]));

    let merge_again_response = send_admin_billing_request(
        &gateway_url,
        http::Method::POST,
        "/api/admin/billing/presets/apply",
        Some(json!({"preset":"default","mode":"merge"})),
    )
    .await;
    assert_eq!(merge_again_response.status(), StatusCode::OK);
    let merge_again_payload: serde_json::Value = merge_again_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(merge_again_payload["ok"], json!(true));
    assert_eq!(merge_again_payload["preset"], json!("aether-core"));
    assert_eq!(merge_again_payload["created"], json!(0));
    assert_eq!(merge_again_payload["updated"], json!(0));
    assert_eq!(merge_again_payload["skipped"], json!(22));

    let overwrite_response = send_admin_billing_request(
        &gateway_url,
        http::Method::POST,
        "/api/admin/billing/presets/apply",
        Some(json!({"preset":"aether-core","mode":"overwrite"})),
    )
    .await;
    assert_eq!(overwrite_response.status(), StatusCode::OK);
    let overwrite_payload: serde_json::Value = overwrite_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(overwrite_payload["ok"], json!(true));
    assert_eq!(overwrite_payload["preset"], json!("aether-core"));
    assert_eq!(overwrite_payload["mode"], json!("overwrite"));
    assert_eq!(overwrite_payload["created"], json!(0));
    assert_eq!(overwrite_payload["updated"], json!(22));
    assert_eq!(overwrite_payload["skipped"], json!(0));
    assert_eq!(overwrite_payload["errors"], json!([]));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_conflict_for_admin_billing_apply_preset_when_backend_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/billing/presets/apply",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let mut state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(crate::data::GatewayDataState::with_user_reader_for_tests(
            Arc::new(InMemoryUserReadRepository::seed_auth_users(Vec::new())),
        ));
    state.admin_billing_collector_store = None;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = send_admin_billing_request(
        &gateway_url,
        http::Method::POST,
        "/api/admin/billing/presets/apply",
        Some(json!({"preset":"aether-core","mode":"merge"})),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_code"], "read_only_mode");
    assert_eq!(payload["detail"], "当前为只读模式，无法应用计费预设");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_billing_rule_routes_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream_hits_clone_detail = Arc::clone(&upstream_hits);
    let upstream = Router::new()
        .route(
            "/api/admin/billing/rules",
            any(move |_request: Request| {
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                async move {
                    *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        )
        .route(
            "/api/admin/billing/rules/rule-1",
            any(move |_request: Request| {
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone_detail);
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
            .with_admin_billing_rules_for_tests([
                crate::AdminBillingRuleRecord {
                    id: "rule-1".to_string(),
                    name: "chat-input".to_string(),
                    task_type: "chat".to_string(),
                    global_model_id: Some("global-gpt-5".to_string()),
                    model_id: Some("model-1".to_string()),
                    expression: "input_tokens * 0.000001".to_string(),
                    variables: json!({ "base": 1.5 }),
                    dimension_mappings: json!({
                        "input_tokens": { "source": "dimension", "dimension": "input_tokens" }
                    }),
                    is_enabled: true,
                    created_at_unix_ms: 1_710_000_000,
                    updated_at_unix_secs: 1_710_000_100,
                },
                crate::AdminBillingRuleRecord {
                    id: "rule-2".to_string(),
                    name: "image-output".to_string(),
                    task_type: "image".to_string(),
                    global_model_id: None,
                    model_id: Some("model-2".to_string()),
                    expression: "images * 0.01".to_string(),
                    variables: json!({}),
                    dimension_mappings: json!({
                        "images": { "source": "dimension", "dimension": "image_count" }
                    }),
                    is_enabled: false,
                    created_at_unix_ms: 1_710_000_000,
                    updated_at_unix_secs: 1_710_000_050,
                },
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let list_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/billing/rules?task_type=chat&is_enabled=true&page=1&page_size=1"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert_eq!(list_payload["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(list_payload["items"][0]["id"], "rule-1");
    assert_eq!(list_payload["items"][0]["name"], "chat-input");
    assert_eq!(list_payload["items"][0]["task_type"], "chat");
    assert_eq!(list_payload["items"][0]["is_enabled"], true);
    assert_eq!(list_payload["total"], 1);
    assert_eq!(list_payload["page"], 1);
    assert_eq!(list_payload["page_size"], 1);
    assert_eq!(list_payload["pages"], 1);

    let detail_response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/billing/rules/rule-1"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail_payload: serde_json::Value = detail_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(detail_payload["id"], "rule-1");
    assert_eq!(detail_payload["name"], "chat-input");
    assert_eq!(detail_payload["task_type"], "chat");
    assert_eq!(detail_payload["global_model_id"], "global-gpt-5");
    assert_eq!(detail_payload["model_id"], "model-1");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    gateway_handle.abort();
    upstream_handle.abort();

    let upstream =
        Router::new().route(
            "/api/admin/billing/rules",
            any(|_request: Request| async move {
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }),
        );
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let create_response = send_admin_billing_request(
        &gateway_url,
        http::Method::POST,
        "/api/admin/billing/rules",
        Some(json!({
            "name": "rule",
            "task_type": "chat",
            "model_id": "model-1",
            "expression": "input_tokens * 0.000001",
            "variables": { "base": 1.5 },
            "dimension_mappings": {
                "input_tokens": { "source": "dimension", "dimension": "input_tokens" }
            },
            "is_enabled": true
        })),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::OK);
    let create_payload: serde_json::Value = create_response
        .json()
        .await
        .expect("json body should parse");
    let rule_id = create_payload["id"]
        .as_str()
        .expect("rule id should exist")
        .to_string();
    assert_eq!(create_payload["name"], json!("rule"));
    assert_eq!(create_payload["model_id"], json!("model-1"));

    let update_response = send_admin_billing_request(
        &gateway_url,
        http::Method::PUT,
        &format!("/api/admin/billing/rules/{rule_id}"),
        Some(json!({
            "name": "rule-updated",
            "task_type": "video",
            "global_model_id": "global-model-1",
            "expression": "max(output_tokens, 1)",
            "variables": { "factor": 2 },
            "dimension_mappings": {
                "output_tokens": { "source": "dimension", "dimension": "output_tokens" }
            },
            "is_enabled": false
        })),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_payload: serde_json::Value = update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(update_payload["id"], json!(rule_id));
    assert_eq!(update_payload["name"], json!("rule-updated"));
    assert_eq!(update_payload["task_type"], json!("video"));
    assert_eq!(update_payload["global_model_id"], json!("global-model-1"));
    assert_eq!(update_payload["model_id"], serde_json::Value::Null);
    assert_eq!(update_payload["is_enabled"], json!(false));

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_conflict_for_admin_billing_rule_create_when_backend_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/billing/rules",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let mut state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(crate::data::GatewayDataState::with_user_reader_for_tests(
            Arc::new(InMemoryUserReadRepository::seed_auth_users(Vec::new())),
        ));
    state.admin_billing_rule_store = None;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = send_admin_billing_request(
        &gateway_url,
        http::Method::POST,
        "/api/admin/billing/rules",
        Some(json!({
            "name": "rule",
            "task_type": "chat",
            "model_id": "model-1",
            "expression": "input_tokens * 0.000001",
            "variables": {},
            "dimension_mappings": {},
            "is_enabled": true
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["error_code"], "read_only_mode");
    assert_eq!(payload["detail"], "当前为只读模式，无法创建计费规则");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_billing_collector_routes_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream_hits_clone_list = Arc::clone(&upstream_hits);
    let upstream = Router::new()
        .route(
            "/api/admin/billing/collectors",
            any(move |_request: Request| {
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone_list);
                async move {
                    *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        )
        .route(
            "/api/admin/billing/collectors/collector-1",
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
            .with_admin_billing_collectors_for_tests([
                crate::AdminBillingCollectorRecord {
                    id: "collector-1".to_string(),
                    api_format: "OPENAI".to_string(),
                    task_type: "chat".to_string(),
                    dimension_name: "tokens".to_string(),
                    source_type: "request".to_string(),
                    source_path: Some("usage.input_tokens".to_string()),
                    value_type: "float".to_string(),
                    transform_expression: None,
                    default_value: None,
                    priority: 10,
                    is_enabled: true,
                    created_at_unix_ms: 1_710_000_000,
                    updated_at_unix_secs: 1_710_000_100,
                },
                crate::AdminBillingCollectorRecord {
                    id: "collector-2".to_string(),
                    api_format: "OPENAI".to_string(),
                    task_type: "chat".to_string(),
                    dimension_name: "latency".to_string(),
                    source_type: "computed".to_string(),
                    source_path: None,
                    value_type: "int".to_string(),
                    transform_expression: Some("max(latency_ms, 1)".to_string()),
                    default_value: Some("1".to_string()),
                    priority: 5,
                    is_enabled: false,
                    created_at_unix_ms: 1_710_000_000,
                    updated_at_unix_secs: 1_710_000_050,
                },
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let list_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/billing/collectors?api_format=OPENAI&task_type=chat&is_enabled=true&page=1&page_size=10"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert_eq!(list_payload["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(list_payload["items"][0]["id"], "collector-1");
    assert_eq!(list_payload["items"][0]["dimension_name"], "tokens");
    assert_eq!(list_payload["total"], 1);
    assert_eq!(list_payload["page"], 1);
    assert_eq!(list_payload["page_size"], 10);
    assert_eq!(list_payload["pages"], 1);

    let detail_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/billing/collectors/collector-1"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail_payload: serde_json::Value = detail_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(detail_payload["id"], "collector-1");
    assert_eq!(detail_payload["api_format"], "OPENAI");
    assert_eq!(detail_payload["source_path"], "usage.input_tokens");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    gateway_handle.abort();
    upstream_handle.abort();

    let upstream =
        Router::new().route(
            "/api/admin/billing/collectors",
            any(|_request: Request| async move {
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }),
        );
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let create_response = send_admin_billing_request(
        &gateway_url,
        http::Method::POST,
        "/api/admin/billing/collectors",
        Some(json!({
            "api_format": "openai",
            "task_type": "chat",
            "dimension_name": "tokens",
            "source_type": "request",
            "source_path": "usage.input_tokens",
            "value_type": "float",
            "priority": 10,
            "is_enabled": true
        })),
    )
    .await;
    assert_eq!(create_response.status(), StatusCode::OK);
    let create_payload: serde_json::Value = create_response
        .json()
        .await
        .expect("json body should parse");
    let collector_id = create_payload["id"]
        .as_str()
        .expect("collector id should exist")
        .to_string();
    assert_eq!(create_payload["api_format"], json!("OPENAI"));
    assert_eq!(create_payload["source_path"], json!("usage.input_tokens"));

    let update_response = send_admin_billing_request(
        &gateway_url,
        http::Method::PUT,
        &format!("/api/admin/billing/collectors/{collector_id}"),
        Some(json!({
            "api_format": "openai",
            "task_type": "chat",
            "dimension_name": "tokens",
            "source_type": "computed",
            "source_path": null,
            "value_type": "int",
            "transform_expression": "max(total_tokens, 1)",
            "default_value": "1",
            "priority": 12,
            "is_enabled": false
        })),
    )
    .await;
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_payload: serde_json::Value = update_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(update_payload["id"], json!(collector_id));
    assert_eq!(update_payload["source_type"], json!("computed"));
    assert_eq!(update_payload["source_path"], serde_json::Value::Null);
    assert_eq!(
        update_payload["transform_expression"],
        json!("max(total_tokens, 1)")
    );
    assert_eq!(update_payload["default_value"], json!("1"));
    assert_eq!(update_payload["is_enabled"], json!(false));

    gateway_handle.abort();
    upstream_handle.abort();
}
