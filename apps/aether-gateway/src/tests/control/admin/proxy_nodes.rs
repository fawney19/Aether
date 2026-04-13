use std::sync::{Arc, Mutex};

use aether_data::repository::management_tokens::InMemoryManagementTokenRepository;
use aether_data::repository::proxy_nodes::{
    InMemoryProxyNodeRepository, ProxyNodeHeartbeatMutation, StoredProxyNodeEvent,
};
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::json;

use super::super::{
    build_router_with_state, hash_management_token, sample_management_token, sample_proxy_node,
    start_server, AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;
use crate::maintenance::{
    record_proxy_upgrade_traffic_success, skip_proxy_upgrade_rollout_node,
    start_proxy_upgrade_rollout,
};

#[tokio::test]
async fn gateway_handles_admin_proxy_nodes_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/proxy-nodes",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut manual_node = sample_proxy_node("proxy-node-manual");
    manual_node.name = "alpha-manual".to_string();
    manual_node.status = "online".to_string();
    manual_node.is_manual = true;
    manual_node.tunnel_mode = false;
    manual_node.tunnel_connected = false;
    manual_node.proxy_url = Some("http://proxy.example:8080".to_string());
    manual_node.proxy_username = Some("alice".to_string());
    manual_node.proxy_password = Some("supersecret".to_string());
    manual_node.last_heartbeat_at_unix_secs = None;
    manual_node.tunnel_connected_at_unix_secs = None;

    let mut tunnel_node = sample_proxy_node("proxy-node-tunnel");
    tunnel_node.name = "zeta-tunnel".to_string();
    tunnel_node.status = "offline".to_string();

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        tunnel_node,
        manual_node,
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                proxy_node_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes?status=online&skip=0&limit=10"
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
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["skip"], 0);
    assert_eq!(payload["limit"], 10);
    assert!(payload["rollout"].is_null());

    let items = payload["items"].as_array().expect("items should be array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "proxy-node-manual");
    assert_eq!(items[0]["name"], "alpha-manual");
    assert_eq!(items[0]["status"], "online");
    assert_eq!(items[0]["is_manual"], true);
    assert_eq!(items[0]["proxy_url"], "http://proxy.example:8080");
    assert_eq!(items[0]["proxy_username"], "alice");
    assert_eq!(items[0]["proxy_password"], "su****et");
    assert!(items[0]["created_at"].is_string());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_reports_active_proxy_upgrade_rollout_in_proxy_node_list() {
    let mut alpha = sample_proxy_node("node-alpha");
    alpha.name = "alpha".to_string();
    alpha.status = "online".to_string();
    alpha.tunnel_connected = true;
    alpha.remote_config = None;
    alpha.proxy_metadata = Some(json!({ "version": "1.9.0" }));

    let mut beta = sample_proxy_node("node-beta");
    beta.name = "beta".to_string();
    beta.status = "online".to_string();
    beta.tunnel_connected = true;
    beta.remote_config = None;
    beta.proxy_metadata = Some(json!({ "version": "1.9.0" }));

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![beta, alpha]));
    let data_state = GatewayDataState::with_proxy_node_repository_for_tests(proxy_node_repository)
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let rollout = start_proxy_upgrade_rollout(
        &data_state,
        "2.0.0".to_string(),
        2,
        120,
        Some(crate::maintenance::ProxyUpgradeRolloutProbeConfig {
            url: "https://probe.example/health".to_string(),
            timeout_secs: 15,
        }),
    )
    .await
    .expect("rollout should start");
    assert_eq!(rollout.updated, 2);

    data_state
        .apply_proxy_node_heartbeat(&ProxyNodeHeartbeatMutation {
            node_id: "node-alpha".to_string(),
            heartbeat_interval: None,
            active_connections: None,
            total_requests_delta: None,
            avg_latency_ms: None,
            failed_requests_delta: None,
            dns_failures_delta: None,
            stream_errors_delta: None,
            proxy_metadata: None,
            proxy_version: Some("2.0.0".to_string()),
        })
        .await
        .expect("heartbeat should apply");
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes?skip=0&limit=10"
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
    assert_eq!(payload["rollout"]["version"], "2.0.0");
    assert_eq!(payload["rollout"]["batch_size"], 2);
    assert_eq!(payload["rollout"]["cooldown_secs"], 120);
    assert_eq!(
        payload["rollout"]["probe"]["url"],
        "https://probe.example/health"
    );
    assert_eq!(payload["rollout"]["probe"]["timeout_secs"], 15);
    assert_eq!(
        payload["rollout"]["pending_node_ids"],
        json!(["node-alpha", "node-beta"])
    );
    assert_eq!(payload["rollout"]["completed_node_ids"], json!([]));
    assert_eq!(payload["rollout"]["conflict_node_ids"], json!([]));
    assert_eq!(payload["rollout"]["blocked"], true);
    assert!(payload["rollout"]["started_at"].is_string());
    assert!(payload["rollout"]["last_dispatched_at"].is_string());
    assert!(payload["rollout"]["updated_at"].is_string());

    let tracked_nodes = payload["rollout"]["tracked_nodes"]
        .as_array()
        .expect("tracked_nodes should be array");
    assert_eq!(tracked_nodes.len(), 2);
    let alpha_status = tracked_nodes
        .iter()
        .find(|tracked| tracked["node_id"] == "node-alpha")
        .expect("alpha status should exist");
    assert_eq!(alpha_status["state"], "awaiting_traffic");
    assert!(alpha_status["version_confirmed_at"].is_string());
    assert!(alpha_status["traffic_confirmed_at"].is_null());

    let beta_status = tracked_nodes
        .iter()
        .find(|tracked| tracked["node_id"] == "node-beta")
        .expect("beta status should exist");
    assert_eq!(beta_status["state"], "awaiting_version");
    assert!(beta_status["version_confirmed_at"].is_null());
    assert!(beta_status["traffic_confirmed_at"].is_null());

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_cancels_active_proxy_upgrade_rollout_locally() {
    let mut alpha = sample_proxy_node("node-alpha");
    alpha.name = "alpha".to_string();
    alpha.status = "online".to_string();
    alpha.tunnel_connected = true;
    alpha.remote_config = None;

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![alpha]));
    let data_state = GatewayDataState::with_proxy_node_repository_for_tests(proxy_node_repository)
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let rollout = start_proxy_upgrade_rollout(&data_state, "2.0.0".to_string(), 1, 120, None)
        .await
        .expect("rollout should start");
    assert!(rollout.rollout_active);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state.clone()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/proxy-nodes/upgrade/cancel"
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
    assert_eq!(payload["cancelled"], true);
    assert_eq!(payload["version"], "2.0.0");
    assert_eq!(payload["pending_node_ids"], json!(["node-alpha"]));
    assert_eq!(payload["conflict_node_ids"], json!([]));

    let list_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes?skip=0&limit=10"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert!(list_payload["rollout"].is_null());

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_clears_proxy_upgrade_rollout_conflicts_locally() {
    let mut alpha = sample_proxy_node("node-alpha");
    alpha.name = "alpha".to_string();
    alpha.status = "online".to_string();
    alpha.tunnel_connected = true;
    alpha.remote_config = None;

    let mut beta = sample_proxy_node("node-beta");
    beta.name = "beta".to_string();
    beta.status = "online".to_string();
    beta.tunnel_connected = true;
    beta.remote_config = None;

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![beta, alpha]));
    let data_state = GatewayDataState::with_proxy_node_repository_for_tests(proxy_node_repository)
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let rollout = start_proxy_upgrade_rollout(&data_state, "2.0.0".to_string(), 1, 120, None)
        .await
        .expect("rollout should start");
    assert_eq!(rollout.updated, 1);

    data_state
        .update_proxy_node_remote_config(
            &aether_data::repository::proxy_nodes::ProxyNodeRemoteConfigMutation {
                node_id: "node-beta".to_string(),
                node_name: None,
                allowed_ports: None,
                log_level: None,
                heartbeat_interval: None,
                scheduling_state: None,
                upgrade_to: Some(Some("3.0.0".to_string())),
            },
        )
        .await
        .expect("conflict target should update");

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state.clone()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/proxy-nodes/upgrade/clear-conflicts"
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
    assert_eq!(payload["cleared"], 1);
    assert_eq!(payload["node_ids"], json!(["node-beta"]));
    assert_eq!(payload["blocked"], true);
    assert_eq!(payload["pending_node_ids"], json!(["node-alpha"]));

    let updated_beta = data_state
        .find_proxy_node("node-beta")
        .await
        .expect("node lookup should succeed")
        .expect("beta should exist");
    let beta_upgrade_to = updated_beta
        .remote_config
        .as_ref()
        .and_then(|value| value.get("upgrade_to"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    assert!(beta_upgrade_to.is_null());

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_skips_proxy_upgrade_rollout_node_and_advances_next_wave_locally() {
    let mut alpha = sample_proxy_node("node-alpha");
    alpha.name = "alpha".to_string();
    alpha.status = "online".to_string();
    alpha.tunnel_connected = true;
    alpha.remote_config = None;

    let mut beta = sample_proxy_node("node-beta");
    beta.name = "beta".to_string();
    beta.status = "online".to_string();
    beta.tunnel_connected = true;
    beta.remote_config = None;

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![beta, alpha]));
    let data_state = GatewayDataState::with_proxy_node_repository_for_tests(proxy_node_repository)
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    let rollout = start_proxy_upgrade_rollout(&data_state, "2.0.0".to_string(), 1, 120, None)
        .await
        .expect("rollout should start");
    assert_eq!(rollout.node_ids, vec!["node-alpha"]);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state.clone()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/proxy-nodes/node-alpha/upgrade/skip"
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
    assert_eq!(payload["node_id"], "node-alpha");
    assert_eq!(payload["skipped_node_ids"], json!(["node-alpha"]));
    assert_eq!(payload["updated"], 1);
    assert_eq!(payload["pending_node_ids"], json!(["node-beta"]));

    let alpha_after = data_state
        .find_proxy_node("node-alpha")
        .await
        .expect("node lookup should succeed")
        .expect("alpha should exist");
    let alpha_upgrade_to = alpha_after
        .remote_config
        .as_ref()
        .and_then(|value| value.get("upgrade_to"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    assert!(alpha_upgrade_to.is_null());

    let list_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes?skip=0&limit=10"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert_eq!(
        list_payload["rollout"]["skipped_node_ids"],
        json!(["node-alpha"])
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_retries_proxy_upgrade_rollout_node_locally() {
    let mut alpha = sample_proxy_node("node-alpha");
    alpha.name = "alpha".to_string();
    alpha.status = "online".to_string();
    alpha.tunnel_connected = true;
    alpha.remote_config = None;

    let mut beta = sample_proxy_node("node-beta");
    beta.name = "beta".to_string();
    beta.status = "online".to_string();
    beta.tunnel_connected = true;
    beta.remote_config = None;

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![beta, alpha]));
    let data_state = GatewayDataState::with_proxy_node_repository_for_tests(proxy_node_repository)
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    start_proxy_upgrade_rollout(&data_state, "2.0.0".to_string(), 1, 120, None)
        .await
        .expect("rollout should start");
    let _ = skip_proxy_upgrade_rollout_node(&data_state, "node-alpha")
        .await
        .expect("skip should succeed");

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state.clone()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/proxy-nodes/node-alpha/upgrade/retry"
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
    assert_eq!(payload["node_id"], "node-alpha");
    assert_eq!(payload["skipped_node_ids"], json!([]));
    assert_eq!(payload["blocked"], true);

    let alpha_after = data_state
        .find_proxy_node("node-alpha")
        .await
        .expect("node lookup should succeed")
        .expect("alpha should exist");
    let alpha_upgrade_to = alpha_after
        .remote_config
        .as_ref()
        .and_then(|value| value.get("upgrade_to"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(alpha_upgrade_to, Some("2.0.0"));

    let list_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes?skip=0&limit=10"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert_eq!(list_payload["rollout"]["skipped_node_ids"], json!([]));
    let tracked_nodes = list_payload["rollout"]["tracked_nodes"]
        .as_array()
        .expect("tracked nodes should be array");
    let alpha_status = tracked_nodes
        .iter()
        .find(|tracked| tracked["node_id"] == "node-alpha")
        .expect("alpha should be tracked again");
    assert_eq!(alpha_status["state"], "awaiting_version");

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_restores_skipped_proxy_upgrade_rollout_nodes_locally() {
    let mut alpha = sample_proxy_node("node-alpha");
    alpha.name = "alpha".to_string();
    alpha.status = "online".to_string();
    alpha.tunnel_connected = true;
    alpha.remote_config = None;

    let mut beta = sample_proxy_node("node-beta");
    beta.name = "beta".to_string();
    beta.status = "online".to_string();
    beta.tunnel_connected = true;
    beta.remote_config = None;

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![beta, alpha]));
    let data_state = GatewayDataState::with_proxy_node_repository_for_tests(proxy_node_repository)
        .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());

    start_proxy_upgrade_rollout(&data_state, "2.0.0".to_string(), 1, 120, None)
        .await
        .expect("rollout should start");
    let _ = skip_proxy_upgrade_rollout_node(&data_state, "node-alpha")
        .await
        .expect("skip should succeed");

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state.clone()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/proxy-nodes/upgrade/restore-skipped"
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
    assert_eq!(payload["restored"], 1);
    assert_eq!(payload["node_ids"], json!(["node-alpha"]));
    assert_eq!(payload["skipped_node_ids"], json!([]));
    assert_eq!(payload["updated"], 0);
    assert_eq!(payload["blocked"], true);
    assert_eq!(payload["pending_node_ids"], json!(["node-beta"]));

    let list_response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes?skip=0&limit=10"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    assert_eq!(list_payload["rollout"]["skipped_node_ids"], json!([]));

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_registers_and_unregisters_proxy_nodes_locally_with_management_token_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/proxy-nodes/register",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let raw_token = "ae_proxy_register_test";
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::default());
    let state = AppState::new().expect("gateway should build");
    let admin_user = state
        .create_local_auth_user_with_settings(
            Some("proxy-admin@example.com".to_string()),
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
    let mut management_token =
        sample_management_token("token-proxy-register", &admin_user.id, "proxy-admin", true);
    management_token.token.allowed_ips = None;
    let management_token_repository =
        Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
            vec![management_token],
            vec![(
                hash_management_token(raw_token),
                "token-proxy-register".to_string(),
            )],
        ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = state.with_data_state_for_tests(
        GatewayDataState::with_management_token_repository_for_tests(management_token_repository)
            .attach_proxy_node_repository_for_tests(proxy_node_repository),
    );
    let token_lookup = state
        .get_management_token_with_user_by_hash(&hash_management_token(raw_token))
        .await
        .expect("token lookup should succeed");
    assert!(token_lookup.is_some());
    let user_lookup = state
        .find_user_auth_by_id(&admin_user.id)
        .await
        .expect("user lookup should succeed");
    assert!(user_lookup.is_some());
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let register_response = client
        .post(format!("{gateway_url}/api/admin/proxy-nodes/register"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(raw_token)
        .json(&json!({
            "name": "proxy-1",
            "ip": "1.1.1.1",
            "port": 0,
            "heartbeat_interval": 30,
            "tunnel_mode": true
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(register_response.status(), StatusCode::OK);
    let register_payload: serde_json::Value = register_response
        .json()
        .await
        .expect("json body should parse");
    let node_id = register_payload["node_id"]
        .as_str()
        .expect("node_id should be present")
        .to_string();
    assert_eq!(register_payload["node"]["name"], "proxy-1");
    assert_eq!(register_payload["node"]["status"], "offline");
    assert_eq!(
        register_payload["node"]["registered_by"],
        json!(admin_user.id)
    );

    let unregister_response = client
        .post(format!("{gateway_url}/api/admin/proxy-nodes/unregister"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .bearer_auth(raw_token)
        .json(&json!({ "node_id": node_id }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(unregister_response.status(), StatusCode::OK);
    let unregister_payload: serde_json::Value = unregister_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(unregister_payload["message"], "unregistered");

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_proxy_node_events_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/proxy-nodes/node-1/events",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed_with_events(
        vec![sample_proxy_node("node-1")],
        vec![
            StoredProxyNodeEvent {
                id: 1,
                node_id: "node-1".to_string(),
                event_type: "connected".to_string(),
                detail: Some("older".to_string()),
                created_at_unix_ms: Some(1_710_000_000),
            },
            StoredProxyNodeEvent {
                id: 2,
                node_id: "node-1".to_string(),
                event_type: "disconnected".to_string(),
                detail: Some("newer".to_string()),
                created_at_unix_ms: Some(1_710_000_100),
            },
        ],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                proxy_node_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes/node-1/events?limit=1"
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
    assert_eq!(items[0]["id"], 2);
    assert_eq!(items[0]["event_type"], "disconnected");
    assert_eq!(items[0]["detail"], "newer");
    assert_eq!(items[0]["created_at"], "2024-03-09T16:01:40Z");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_updates_proxy_node_config_and_batches_upgrade_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new()
        .route(
            "/api/admin/proxy-nodes/node-online/config",
            any(move |_request: Request| {
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                async move {
                    *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        )
        .route(
            "/api/admin/proxy-nodes/upgrade",
            any(|| async move { (StatusCode::OK, Body::from("unexpected upstream hit")) }),
        );

    let mut online_node = sample_proxy_node("node-online");
    online_node.status = "online".to_string();
    online_node.tunnel_connected = true;
    let mut online_node_2 = sample_proxy_node("node-zeta");
    online_node_2.name = "zeta-online".to_string();
    online_node_2.status = "online".to_string();
    online_node_2.tunnel_connected = true;
    online_node_2.remote_config = None;
    let mut offline_node = sample_proxy_node("node-offline");
    offline_node.status = "offline".to_string();
    offline_node.tunnel_connected = false;
    offline_node.remote_config = None;
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        online_node,
        online_node_2,
        offline_node,
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let data_state =
        GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&proxy_node_repository))
            .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state.clone()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let config_response = client
        .put(format!(
            "{gateway_url}/api/admin/proxy-nodes/node-online/config"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "node_name": "edge-online",
            "allowed_ports": [443, 8443],
            "log_level": "info",
            "heartbeat_interval": 45,
            "upgrade_to": null
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(config_response.status(), StatusCode::OK);
    let config_payload: serde_json::Value = config_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(config_payload["node_id"], "node-online");
    assert_eq!(config_payload["config_version"], 8);
    assert_eq!(config_payload["node"]["name"], "edge-online");
    assert_eq!(config_payload["remote_config"]["log_level"], "info");
    assert!(config_payload["remote_config"].get("upgrade_to").is_none());

    let upgrade_response = client
        .post(format!("{gateway_url}/api/admin/proxy-nodes/upgrade"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "version": "2.0.0", "cooldown_secs": 0 }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(upgrade_response.status(), StatusCode::OK);
    let upgrade_payload: serde_json::Value = upgrade_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(upgrade_payload["version"], "2.0.0");
    assert_eq!(upgrade_payload["batch_size"], 1);
    assert_eq!(upgrade_payload["updated"], 1);
    assert_eq!(upgrade_payload["skipped"], 1);
    assert_eq!(upgrade_payload["blocked"], false);
    assert_eq!(upgrade_payload["pending_node_ids"], json!(["node-online"]));
    assert_eq!(upgrade_payload["node_ids"], json!(["node-online"]));
    assert_eq!(upgrade_payload["completed"], 0);
    assert_eq!(upgrade_payload["remaining"], 2);
    assert_eq!(upgrade_payload["rollout_active"], true);

    let blocked_upgrade_response = client
        .post(format!("{gateway_url}/api/admin/proxy-nodes/upgrade"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "version": "2.0.0", "cooldown_secs": 0 }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(blocked_upgrade_response.status(), StatusCode::OK);
    let blocked_upgrade_payload: serde_json::Value = blocked_upgrade_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(blocked_upgrade_payload["updated"], 0);
    assert_eq!(blocked_upgrade_payload["blocked"], true);
    assert_eq!(
        blocked_upgrade_payload["pending_node_ids"],
        json!(["node-online"])
    );

    let heartbeat_response = client
        .post(format!("{gateway_url}/api/internal/tunnel/heartbeat"))
        .json(&json!({
            "node_id": "node-online",
            "heartbeat_interval": 45,
            "active_connections": 3,
            "total_requests": 5,
            "avg_latency_ms": 10.0,
            "proxy_metadata": { "arch": "arm64" },
            "proxy_version": "2.0.0"
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(heartbeat_response.status(), StatusCode::OK);
    let heartbeat_payload: serde_json::Value = heartbeat_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(heartbeat_payload["config_version"], 10);
    assert!(heartbeat_payload.get("upgrade_to").is_none());
    assert_eq!(heartbeat_payload["remote_config"]["allowed_ports"][1], 8443);
    assert_eq!(heartbeat_payload["remote_config"]["log_level"], "info");

    let second_upgrade_response = client
        .post(format!("{gateway_url}/api/admin/proxy-nodes/upgrade"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "version": "2.0.0", "cooldown_secs": 0 }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(second_upgrade_response.status(), StatusCode::OK);
    let second_upgrade_payload: serde_json::Value = second_upgrade_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(second_upgrade_payload["batch_size"], 1);
    assert_eq!(second_upgrade_payload["updated"], 0);
    assert_eq!(second_upgrade_payload["skipped"], 2);
    assert_eq!(second_upgrade_payload["blocked"], true);
    assert_eq!(
        second_upgrade_payload["pending_node_ids"],
        json!(["node-online"])
    );
    assert_eq!(second_upgrade_payload["node_ids"], json!([]));
    assert_eq!(second_upgrade_payload["completed"], 0);
    assert_eq!(second_upgrade_payload["remaining"], 3);
    assert_eq!(second_upgrade_payload["rollout_active"], true);

    assert!(
        record_proxy_upgrade_traffic_success(&data_state, "node-online")
            .await
            .expect("traffic confirmation should be recorded")
    );

    let third_upgrade_response = client
        .post(format!("{gateway_url}/api/admin/proxy-nodes/upgrade"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "version": "2.0.0", "cooldown_secs": 0 }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(third_upgrade_response.status(), StatusCode::OK);
    let third_upgrade_payload: serde_json::Value = third_upgrade_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(third_upgrade_payload["batch_size"], 1);
    assert_eq!(third_upgrade_payload["updated"], 1);
    assert_eq!(third_upgrade_payload["skipped"], 1);
    assert_eq!(third_upgrade_payload["blocked"], false);
    assert_eq!(
        third_upgrade_payload["pending_node_ids"],
        json!(["node-zeta"])
    );
    assert_eq!(third_upgrade_payload["node_ids"], json!(["node-zeta"]));
    assert_eq!(third_upgrade_payload["completed"], 1);
    assert_eq!(third_upgrade_payload["remaining"], 1);
    assert_eq!(third_upgrade_payload["rollout_active"], true);

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_marks_draining_proxy_nodes_unschedulable_and_rollout_skips_them() {
    let mut alpha = sample_proxy_node("node-alpha");
    alpha.name = "alpha".to_string();
    alpha.status = "online".to_string();
    alpha.tunnel_connected = true;
    alpha.remote_config = None;

    let mut zeta = sample_proxy_node("node-zeta");
    zeta.name = "zeta".to_string();
    zeta.status = "online".to_string();
    zeta.tunnel_connected = true;
    zeta.remote_config = None;

    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![zeta, alpha]));
    let data_state =
        GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&proxy_node_repository))
            .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new());
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let config_response = client
        .put(format!(
            "{gateway_url}/api/admin/proxy-nodes/node-alpha/config"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "scheduling_state": "draining"
        }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(config_response.status(), StatusCode::OK);
    let config_payload: serde_json::Value = config_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(
        config_payload["remote_config"]["scheduling_state"],
        "draining"
    );

    let list_response = client
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes?skip=0&limit=10"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_payload: serde_json::Value =
        list_response.json().await.expect("json body should parse");
    let alpha_payload = list_payload["items"]
        .as_array()
        .expect("items should be array")
        .iter()
        .find(|item| item["id"] == "node-alpha")
        .expect("alpha should exist");
    assert_eq!(
        alpha_payload["remote_config"]["scheduling_state"],
        "draining"
    );

    let upgrade_response = client
        .post(format!("{gateway_url}/api/admin/proxy-nodes/upgrade"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "version": "2.0.0", "batch_size": 2, "cooldown_secs": 0 }))
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(upgrade_response.status(), StatusCode::OK);
    let upgrade_payload: serde_json::Value = upgrade_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(upgrade_payload["updated"], 1);
    assert_eq!(upgrade_payload["node_ids"], json!(["node-zeta"]));
    assert_eq!(upgrade_payload["pending_node_ids"], json!(["node-zeta"]));

    let rollout_response = client
        .get(format!(
            "{gateway_url}/api/admin/proxy-nodes?skip=0&limit=10"
        ))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");
    let rollout_payload: serde_json::Value = rollout_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(rollout_payload["rollout"]["online_eligible_total"], 1);

    gateway_handle.abort();
}
