use std::sync::{Arc, Mutex};

use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, StoredUserSummary,
};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use axum::body::{Body, Bytes};
use axum::routing::{any, get, post};
use axum::{extract::Request, Router};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;

use super::super::{
    build_router_with_state, issue_test_admin_access_token, sample_endpoint, sample_key,
    sample_provider, start_server, AppState,
};
use crate::admin_api::{
    maybe_build_local_admin_usage_response, AdminAppState, AdminRequestContext,
};
use crate::audit::AdminAuditEvent;
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER,
    TRUSTED_ADMIN_USER_ID_HEADER, TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::control::resolve_public_request_context;
use crate::data::GatewayDataState;

const ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL: &str = "Admin usage data unavailable";
const DAY_1_UNIX_SECS: i64 = 1_711_000_000;
const DAY_2_UNIX_SECS: i64 = 1_711_086_400;

fn admin_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
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

async fn local_admin_usage_response(
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
    maybe_build_local_admin_usage_response(
        &AdminAppState::new(state),
        &AdminRequestContext::new(&request_context),
        body_bytes.as_ref(),
    )
    .await
    .expect("local usage response should build")
    .expect("usage route should resolve locally")
}

async fn start_usage_upstream(
    path: &'static str,
) -> (String, Arc<Mutex<usize>>, tokio::task::JoinHandle<()>) {
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
    (upstream_url, upstream_hits, upstream_handle)
}

async fn assert_admin_usage_route_returns_local_503(
    data_state: GatewayDataState,
    method: http::Method,
    path: &'static str,
    body: Option<serde_json::Value>,
    expected_detail: &str,
) {
    let (_upstream_url, upstream_hits, upstream_handle) = start_usage_upstream(path).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut request =
        admin_request(reqwest::Client::new().request(method, format!("{gateway_url}{path}")));
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], expected_detail);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

fn sample_usage_row(
    id: &str,
    request_id: &str,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
    api_key_name: Option<&str>,
    provider_name: &str,
    model: &str,
    status: &str,
    input_tokens: i32,
    output_tokens: i32,
    total_cost_usd: f64,
    actual_total_cost_usd: f64,
    created_at_unix_ms: i64,
) -> StoredRequestUsageAudit {
    let mut usage = StoredRequestUsageAudit::new(
        id.to_string(),
        request_id.to_string(),
        user_id.map(str::to_string),
        api_key_id.map(str::to_string),
        user_id.map(|value| format!("user-{value}")),
        api_key_name.map(str::to_string),
        provider_name.to_string(),
        model.to_string(),
        Some(format!("{model}-target")),
        Some("provider-1".to_string()),
        Some("endpoint-1".to_string()),
        Some("provider-key-1".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        Some("openai:chat".to_string()),
        Some("openai".to_string()),
        Some("chat".to_string()),
        false,
        status == "streaming",
        input_tokens,
        output_tokens,
        input_tokens + output_tokens,
        total_cost_usd,
        actual_total_cost_usd,
        Some(if status == "failed" { 500 } else { 200 }),
        (status == "failed").then(|| "request failed".to_string()),
        None,
        Some(420),
        Some(120),
        status.to_string(),
        "settled".to_string(),
        created_at_unix_ms,
        created_at_unix_ms + 1,
        Some(created_at_unix_ms + 2),
    )
    .expect("usage row should build")
    .with_cache_input_tokens(15, 5);
    usage.cache_creation_ephemeral_5m_input_tokens = 6;
    usage.cache_creation_ephemeral_1h_input_tokens = 9;
    usage
}

fn sample_user_summary(id: &str, username: &str) -> StoredUserSummary {
    StoredUserSummary::new(
        id.to_string(),
        username.to_string(),
        Some(format!("{username}@example.com")),
        "user".to_string(),
        true,
        false,
    )
    .expect("user summary should build")
}

fn sample_auth_user(id: &str, username: &str, role: &str, is_active: bool) -> StoredUserAuthRecord {
    StoredUserAuthRecord::new(
        id.to_string(),
        Some(format!("{username}@example.com")),
        true,
        username.to_string(),
        Some("hash".to_string()),
        role.to_string(),
        "local".to_string(),
        None,
        None,
        None,
        is_active,
        false,
        Some(chrono::Utc::now()),
        Some(chrono::Utc::now()),
    )
    .expect("auth user should build")
}

fn sample_auth_api_key_snapshot(
    user_id: &str,
    api_key_id: &str,
    api_key_name: &str,
) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        format!("snapshot-{user_id}"),
        Some(format!("snapshot-{user_id}@example.com")),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        None,
        None,
        None,
        api_key_id.to_string(),
        Some(api_key_name.to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        None,
        None,
        None,
        None,
    )
    .expect("auth snapshot should build")
}

fn recent_unix_secs(minutes_ago: u64) -> i64 {
    let now = chrono::Utc::now().timestamp();
    now.saturating_sub((minutes_ago * 60) as i64)
}

#[tokio::test]
async fn gateway_handles_admin_usage_stats_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/stats").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-1",
            "req-1",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            120,
            30,
            0.3,
            0.36,
            DAY_1_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-2",
            "req-2",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "failed",
            40,
            10,
            0.1,
            0.12,
            DAY_2_UNIX_SECS,
        ),
    ]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/usage/stats?start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total_requests"], 2);
    assert_eq!(payload["total_tokens"], 240);
    assert_eq!(payload["error_count"], 1);
    assert_eq!(payload["cache_stats"]["cache_creation_tokens"], 30);
    assert_eq!(
        payload["cache_stats"]["cache_creation_ephemeral_5m_tokens"],
        12
    );
    assert_eq!(
        payload["cache_stats"]["cache_creation_ephemeral_1h_tokens"],
        18
    );
    assert_eq!(payload["cache_stats"]["cache_read_tokens"], 10);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_defaults_admin_usage_stats_to_bounded_recent_window_when_query_missing() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/stats").await;

    let recent_created_at = recent_unix_secs(5);
    let stale_created_at = recent_unix_secs(60 * 48);
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-recent",
            "req-recent",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            120,
            30,
            0.3,
            0.36,
            recent_created_at,
        ),
        sample_usage_row(
            "usage-stale",
            "req-stale",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "failed",
            40,
            10,
            0.1,
            0.12,
            stale_created_at,
        ),
    ]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage/stats")))
            .send()
            .await
            .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total_requests"], 1);
    assert_eq!(payload["total_tokens"], 170);
    assert_eq!(payload["error_count"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_usage_stats_without_usage_reader() {
    assert_admin_usage_route_returns_local_503(
        GatewayDataState::disabled(),
        http::Method::GET,
        "/api/admin/usage/stats",
        None,
        ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
    )
    .await;
}

#[tokio::test]
async fn gateway_handles_admin_usage_aggregation_stats_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/aggregation/stats").await;

    let mut usage_1 = sample_usage_row(
        "usage-1",
        "req-1",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        120,
        30,
        0.3,
        0.36,
        DAY_1_UNIX_SECS,
    );
    usage_1.provider_id = Some("provider-openai".to_string());
    usage_1.total_tokens = usage_1.input_tokens;

    let mut usage_2 = sample_usage_row(
        "usage-2",
        "req-2",
        Some("user-2"),
        Some("key-2"),
        Some("secondary"),
        "OpenAI",
        "gpt-5",
        "completed",
        40,
        10,
        0.1,
        0.12,
        DAY_2_UNIX_SECS,
    );
    usage_2.provider_id = Some("provider-openai".to_string());
    usage_2.total_tokens = usage_2.input_tokens;

    let mut usage_3 = sample_usage_row(
        "usage-3",
        "req-3",
        Some("user-2"),
        Some("key-2"),
        Some("secondary"),
        "Anthropic",
        "claude-3-7",
        "completed",
        60,
        20,
        0.2,
        0.24,
        DAY_2_UNIX_SECS,
    );
    usage_3.provider_id = Some("provider-anthropic".to_string());
    usage_3.total_tokens = usage_3.input_tokens;
    usage_3.api_format = Some("claude:cli".to_string());
    usage_3.api_family = Some("claude".to_string());
    usage_3.endpoint_api_format = Some("claude:cli".to_string());
    usage_3.provider_api_family = Some("claude".to_string());

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        usage_1, usage_2, usage_3,
    ]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/aggregation/stats?group_by=model&limit=10&start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload.as_array().expect("array response");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["model"], "gpt-5");
    assert_eq!(items[0]["request_count"], 2);
    assert_eq!(items[0]["output_tokens"], 40);
    assert_eq!(items[0]["effective_input_tokens"], 150);
    assert_eq!(items[0]["total_input_context"], 160);
    assert_eq!(items[0]["cache_creation_tokens"], 30);
    assert_eq!(items[0]["cache_creation_ephemeral_5m_tokens"], 12);
    assert_eq!(items[0]["cache_creation_ephemeral_1h_tokens"], 18);
    assert_eq!(items[0]["cache_hit_rate"], 6.25);
    assert_eq!(items[1]["model"], "claude-3-7");
    assert_eq!(items[1]["output_tokens"], 20);

    let provider_response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/aggregation/stats?group_by=provider&limit=10&start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(provider_response.status(), StatusCode::OK);
    let provider_payload: serde_json::Value = provider_response
        .json()
        .await
        .expect("json body should parse");
    let provider_items = provider_payload.as_array().expect("array response");
    assert_eq!(provider_items.len(), 2);
    assert_eq!(provider_items[0]["provider"], "OpenAI");
    assert_eq!(provider_items[0]["output_tokens"], 40);
    assert_eq!(provider_items[1]["provider"], "Anthropic");
    assert_eq!(provider_items[1]["output_tokens"], 20);

    let api_format_response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/aggregation/stats?group_by=api_format&limit=10&start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(api_format_response.status(), StatusCode::OK);
    let api_format_payload: serde_json::Value = api_format_response
        .json()
        .await
        .expect("json body should parse");
    let api_format_items = api_format_payload.as_array().expect("array response");
    assert_eq!(api_format_items.len(), 2);
    assert_eq!(api_format_items[0]["api_format"], "openai:chat");
    assert_eq!(api_format_items[0]["output_tokens"], 40);
    assert_eq!(api_format_items[1]["api_format"], "claude:cli");
    assert_eq!(api_format_items[1]["output_tokens"], 20);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_service_unavailable_for_admin_usage_replay_without_provider_catalog_reader(
) {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![]));
    assert_admin_usage_route_returns_local_503(
        GatewayDataState::with_usage_reader_for_tests(usage_repository),
        http::Method::POST,
        "/api/admin/usage/usage-1/replay",
        Some(json!({})),
        ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
    )
    .await;
}

#[tokio::test]
async fn gateway_handles_admin_usage_aggregation_stats_locally_with_bearer_admin_session() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/aggregation/stats").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-1",
            "req-1",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            120,
            30,
            0.3,
            0.36,
            DAY_1_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-2",
            "req-2",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "OpenAI",
            "gpt-5",
            "completed",
            40,
            10,
            0.1,
            0.12,
            DAY_2_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-3",
            "req-3",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "completed",
            60,
            20,
            0.2,
            0.24,
            DAY_2_UNIX_SECS,
        ),
    ]));

    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
            usage_repository,
        ));
    let access_token = issue_test_admin_access_token(&state, "device-admin-usage").await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/usage/aggregation/stats?group_by=user&start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-admin-usage")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let items = payload.as_array().expect("array response");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["user_id"], "user-2");
    assert_eq!(items[0]["request_count"], 2);
    assert_eq!(items[1]["user_id"], "user-1");
    assert_eq!(items[1]["request_count"], 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_heatmap_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/heatmap").await;
    let now_unix_secs = chrono::Utc::now().timestamp();

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-1",
            "req-1",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            120,
            30,
            0.3,
            0.36,
            now_unix_secs.saturating_sub(600),
        ),
        sample_usage_row(
            "usage-2",
            "req-2",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "completed",
            40,
            10,
            0.1,
            0.12,
            now_unix_secs.saturating_sub(480),
        ),
    ]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage/heatmap")))
            .send()
            .await
            .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total_days"].as_u64(), Some(365));
    assert_eq!(payload["max_requests"].as_u64(), Some(2));
    let days = payload["days"].as_array().expect("days should be array");
    assert_eq!(days.len(), 365);
    let latest = days
        .iter()
        .rev()
        .find(|day| day["requests"].as_u64().unwrap_or(0) > 0)
        .expect("latest non-empty heatmap day should exist");
    assert_eq!(latest["requests"], 2);
    assert_eq!(latest["total_tokens"], 240);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_active_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/active").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-pending",
            "req-pending",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "pending",
            10,
            0,
            0.0,
            0.0,
            DAY_2_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-done",
            "req-done",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "completed",
            40,
            10,
            0.1,
            0.12,
            DAY_1_UNIX_SECS,
        ),
    ]));
    let mut provider_key = sample_key("provider-key-1", "provider-1", "openai:chat", "sk-upstream");
    provider_key.name = "upstream-primary".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "OpenAI", 10)],
        vec![sample_endpoint(
            "endpoint-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![provider_key],
    ));
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-key-1".to_string()),
        sample_auth_api_key_snapshot("user-1", "key-1", "fresh-primary"),
    )]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_usage_reader_for_tests(usage_repository)
                    .with_provider_catalog_reader(provider_catalog_repository)
                    .with_auth_api_key_reader(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/usage/active?start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0"
        )))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["requests"].as_array().expect("array").len(), 1);
    assert_eq!(payload["requests"][0]["id"], "usage-pending");
    assert_eq!(payload["requests"][0]["effective_input_tokens"], 5);
    assert_eq!(payload["requests"][0]["provider"], "OpenAI");
    assert_eq!(payload["requests"][0]["api_key_name"], "fresh-primary");
    assert_eq!(
        payload["requests"][0]["provider_key_name"],
        "upstream-primary"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_defaults_admin_usage_active_to_recent_window_when_query_missing() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/active").await;

    let recent_created_at = recent_unix_secs(3);
    let stale_created_at = recent_unix_secs(60 * 48);
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-recent-pending",
            "req-recent-pending",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "pending",
            10,
            0,
            0.0,
            0.0,
            recent_created_at,
        ),
        sample_usage_row(
            "usage-stale-pending",
            "req-stale-pending",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "pending",
            40,
            10,
            0.1,
            0.12,
            stale_created_at,
        ),
        sample_usage_row(
            "usage-recent-completed",
            "req-recent-completed",
            Some("user-3"),
            Some("key-3"),
            Some("tertiary"),
            "Google",
            "gemini-2.5-pro",
            "completed",
            20,
            5,
            0.2,
            0.22,
            recent_created_at + 1,
        ),
    ]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage/active")))
            .send()
            .await
            .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let requests = payload["requests"].as_array().expect("array");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["id"], "usage-recent-pending");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_active_ids_for_terminal_updates() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/active").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-completed",
        "req-completed",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        20,
        5,
        0.2,
        0.24,
        recent_unix_secs(2),
    )]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/active?ids=usage-completed"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let requests = payload["requests"].as_array().expect("array");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["id"], "usage-completed");
    assert_eq!(requests[0]["status"], "completed");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_records_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/records").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-a",
            "req-a",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            120,
            30,
            0.3,
            0.36,
            DAY_1_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-b",
            "req-b",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "failed",
            40,
            10,
            0.1,
            0.12,
            DAY_2_UNIX_SECS,
        ),
    ]));
    let mut provider_key = sample_key("provider-key-1", "provider-1", "openai:chat", "sk-upstream");
    provider_key.name = "upstream-primary".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "OpenAI", 10)],
        vec![sample_endpoint(
            "endpoint-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![provider_key],
    ));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![
        sample_user_summary("user-1", "alice"),
        sample_user_summary("user-2", "bob"),
    ]));
    let data_state = GatewayDataState::with_usage_reader_for_tests(usage_repository)
        .with_user_reader(user_repository)
        .with_provider_catalog_reader(provider_catalog_repository);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/records?start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0&status=failed&provider=Anthropic&limit=10&offset=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["records"][0]["id"], "usage-b");
    assert_eq!(payload["records"][0]["username"], "bob");
    assert_eq!(payload["records"][0]["user_email"], "bob@example.com");
    assert_eq!(
        payload["records"][0]["provider_key_name"],
        "upstream-primary"
    );
    assert_eq!(payload["records"][0]["effective_input_tokens"], 35);
    assert_eq!(payload["records"][0]["first_byte_time_ms"], 120);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_records_with_provider_key_name_fallback_from_request_metadata()
{
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/records").await;

    let mut usage_row = sample_usage_row(
        "usage-fallback",
        "req-fallback",
        Some("user-1"),
        Some("key-1"),
        Some("client-key"),
        "OpenAI",
        "gpt-5",
        "completed",
        12,
        8,
        0.02,
        0.02,
        DAY_1_UNIX_SECS,
    );
    usage_row.provider_api_key_id = None;
    usage_row.request_metadata = Some(json!({
        "request_id": "req-fallback",
        "candidate_id": "cand-fallback",
        "key_name": "upstream-fallback",
    }));

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![usage_row]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![sample_user_summary(
        "user-1", "alice",
    )]));
    let data_state = GatewayDataState::with_usage_reader_for_tests(usage_repository)
        .with_user_reader(user_repository);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/records?start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["records"][0]["provider_key_name"],
        "upstream-fallback"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_records_with_snapshot_first_user_and_api_key_names() {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/records").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-snapshot",
        "req-snapshot",
        Some("user-2"),
        Some("key-2"),
        Some("legacy-secondary"),
        "Anthropic",
        "claude-3-7",
        "failed",
        40,
        10,
        0.1,
        0.12,
        DAY_2_UNIX_SECS,
    )]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![sample_user_summary(
        "user-2",
        "fresh-bob",
    )]));
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-key-2".to_string()),
        sample_auth_api_key_snapshot("user-2", "key-2", "fresh-secondary"),
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_usage_reader_for_tests(usage_repository)
                    .with_user_reader(user_repository)
                    .with_auth_api_key_reader(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/records?start_date=2024-03-21&end_date=2024-03-22&tz_offset_minutes=0&search=fresh-secondary&username=fresh-bob&limit=10&offset=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["records"][0]["id"], "usage-snapshot");
    assert_eq!(payload["records"][0]["username"], "fresh-bob");
    assert_eq!(payload["records"][0]["api_key"]["name"], "fresh-secondary");
    assert_eq!(payload["records"][0]["api_key_name"], "fresh-secondary");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_records_without_legacy_username_fallback_when_user_reader_exists(
) {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/records").await;

    let mut usage = sample_usage_row(
        "usage-no-legacy-username",
        "req-no-legacy-username",
        Some("user-missing"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        10,
        2,
        0.01,
        0.012,
        DAY_1_UNIX_SECS,
    );
    usage.username = Some("stale-alice".to_string());

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_usage_reader_for_tests(Arc::new(
                    InMemoryUsageReadRepository::seed(vec![usage]),
                ))
                .with_user_reader(Arc::new(
                    InMemoryUserReadRepository::seed_auth_users(Vec::<StoredUserAuthRecord>::new()),
                )),
            )
            .without_auth_user_store_for_tests(),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/records?username=stale-alice&limit=10&offset=0"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 0);
    assert!(payload["records"].as_array().expect("array").is_empty());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_detail_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/usage-1").await;

    let mut usage = sample_usage_row(
        "usage-1",
        "req-1",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        120,
        30,
        0.3,
        0.36,
        DAY_1_UNIX_SECS,
    );
    usage.request_headers = Some(json!({
        "Content-Type": "application/json",
        "X-Trace": "req-1",
    }));
    usage.provider_request_headers = Some(json!({
        "Content-Type": "application/json",
        "Authorization": "Bearer provider-real",
    }));
    usage.request_body = Some(json!({
        "model": "gpt-5",
        "messages": [
            {
                "role": "user",
                "content": "show real request body"
            }
        ],
        "stream": false,
    }));
    usage.provider_request_body = Some(json!({
        "model": "gpt-5-target",
        "stream": false,
        "temperature": 0.2,
    }));
    usage.response_headers = Some(json!({
        "Content-Type": "application/json",
        "X-Upstream": "openai",
    }));
    usage.response_body = Some(json!({
        "id": "resp-1",
        "usage": {
            "prompt_tokens": 120,
            "completion_tokens": 30,
            "total_tokens": 150
        }
    }));
    usage.client_response_headers = Some(json!({
        "Content-Type": "application/json",
        "X-Request-Id": "req-1",
    }));
    usage.client_response_body = Some(json!({
        "id": "resp-1",
        "output_text": "hello",
    }));
    usage.request_metadata = Some(json!({
        "trace_id": "trace-123",
        "candidate_id": "cand-detail-1",
        "candidate_index": 2,
        "key_name": "upstream-primary",
        "planner_kind": "claude_cli_sync",
        "route_family": "claude",
        "route_kind": "cli",
        "execution_path": "local_execution_runtime_miss",
        "local_execution_runtime_miss_reason": "all_candidates_skipped",
        "billing_snapshot_schema_version": "v2",
        "billing_snapshot_status": "resolved",
        "rate_multiplier": 0.5,
        "is_free_tier": false,
        "input_price_per_1m": 3.0,
        "output_price_per_1m": 15.0,
        "cache_creation_price_per_1m": 3.75,
        "cache_read_price_per_1m": 0.3,
        "price_per_request": 0.02,
        "billing_snapshot": {
            "resolved_variables": {
                "output_price_per_1m": 11.0
            }
        }
    }));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![usage]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![sample_user_summary(
        "user-1", "alice",
    )]));
    let data_state = GatewayDataState::with_usage_reader_for_tests(usage_repository)
        .with_user_reader(user_repository);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/usage-1?include_bodies=false"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], "usage-1");
    assert_eq!(payload["request_id"], "req-1");
    assert_eq!(payload["user"]["id"], "user-1");
    assert_eq!(payload["user"]["email"], "alice@example.com");
    assert_eq!(payload["api_key"]["id"], "key-1");
    assert_eq!(payload["api_key"]["name"], "primary");
    assert_eq!(payload["provider"], "OpenAI");
    assert_eq!(payload["model"], "gpt-5");
    assert_eq!(payload["effective_input_tokens"], 115);
    assert_eq!(payload["total_tokens"], 170);
    assert_eq!(payload["cache_creation_cost"], 0.0);
    assert_eq!(payload["cache_read_cost"], 0.0);
    assert_eq!(
        payload["request_headers"]["Content-Type"],
        "application/json"
    );
    assert_eq!(payload["request_headers"]["X-Trace"], "req-1");
    assert_eq!(
        payload["provider_request_headers"]["Content-Type"],
        "application/json"
    );
    assert_eq!(
        payload["provider_request_headers"]["Authorization"],
        "Bearer provider-real"
    );
    assert_eq!(payload["response_headers"]["X-Upstream"], "openai");
    assert_eq!(payload["client_response_headers"]["X-Request-Id"], "req-1");
    assert!(payload["metadata"]["trace_id"].is_null());
    assert_eq!(payload["trace"]["trace_id"], "trace-123");
    assert!(payload["metadata"]["request_preview_source"].is_null());
    assert!(payload["metadata"]["original_request_body_available"].is_null());
    assert!(payload["metadata"]["original_response_body_available"].is_null());
    assert!(payload["metadata"]["candidate_id"].is_null());
    assert!(payload["metadata"]["candidate_index"].is_null());
    assert!(payload["metadata"]["key_name"].is_null());
    assert!(payload["metadata"]["planner_kind"].is_null());
    assert!(payload["metadata"]["route_family"].is_null());
    assert!(payload["metadata"]["route_kind"].is_null());
    assert!(payload["metadata"]["execution_path"].is_null());
    assert!(payload["metadata"]["local_execution_runtime_miss_reason"].is_null());
    assert!(payload["metadata"]["billing_snapshot"].is_null());
    assert!(payload["metadata"]["billing_snapshot_schema_version"].is_null());
    assert!(payload["metadata"]["billing_snapshot_status"].is_null());
    assert!(payload["metadata"]["rate_multiplier"].is_null());
    assert!(payload["metadata"]["is_free_tier"].is_null());
    assert!(payload["metadata"]["input_price_per_1m"].is_null());
    assert!(payload["metadata"]["output_price_per_1m"].is_null());
    assert!(payload["metadata"]["cache_creation_price_per_1m"].is_null());
    assert!(payload["metadata"]["cache_read_price_per_1m"].is_null());
    assert!(payload["metadata"]["price_per_request"].is_null());
    assert_eq!(payload["body_capture"]["request"]["storage"], "inline");
    assert_eq!(payload["body_capture"]["request"]["available"], true);
    assert_eq!(
        payload["body_capture"]["request"]["preview_source"],
        "stored_original"
    );
    assert_eq!(
        payload["body_capture"]["provider_request"]["storage"],
        "inline"
    );
    assert_eq!(payload["body_capture"]["response"]["storage"], "inline");
    assert_eq!(
        payload["body_capture"]["client_response"]["storage"],
        "inline"
    );
    assert_eq!(payload["routing"]["candidate_id"], "cand-detail-1");
    assert_eq!(payload["routing"]["candidate_index"], 2);
    assert_eq!(payload["routing"]["key_name"], "upstream-primary");
    assert_eq!(payload["routing"]["planner_kind"], "claude_cli_sync");
    assert_eq!(payload["routing"]["route_family"], "claude");
    assert_eq!(payload["routing"]["route_kind"], "cli");
    assert_eq!(
        payload["routing"]["execution_path"],
        "local_execution_runtime_miss"
    );
    assert_eq!(
        payload["routing"]["local_execution_runtime_miss_reason"],
        "all_candidates_skipped"
    );
    assert_eq!(
        payload["settlement"]["billing_snapshot_schema_version"],
        "v2"
    );
    assert_eq!(
        payload["settlement"]["billing_snapshot"]["resolved_variables"]["output_price_per_1m"],
        11.0
    );
    assert_eq!(payload["settlement"]["billing_snapshot_status"], "resolved");
    assert_eq!(payload["settlement"]["rate_multiplier"], 0.5);
    assert_eq!(payload["settlement"]["is_free_tier"], false);
    assert_eq!(payload["settlement"]["input_price_per_1m"], 3.0);
    assert_eq!(payload["settlement"]["output_price_per_1m"], 15.0);
    assert_eq!(payload["settlement"]["cache_creation_price_per_1m"], 3.75);
    assert_eq!(payload["settlement"]["cache_read_price_per_1m"], 0.3);
    assert_eq!(payload["settlement"]["price_per_request"], 0.02);
    assert_eq!(payload["has_request_body"], true);
    assert_eq!(payload["has_provider_request_body"], true);
    assert_eq!(payload["has_response_body"], true);
    assert_eq!(payload["has_client_response_body"], true);
    assert!(payload["request_body"].is_null());
    assert!(payload["provider_request_body"].is_null());
    assert!(payload["response_body"].is_null());
    assert!(payload["client_response_body"].is_null());

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage/usage-1")))
            .send()
            .await
            .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["has_request_body"], true);
    assert_eq!(payload["has_provider_request_body"], true);
    assert_eq!(payload["has_response_body"], true);
    assert_eq!(payload["has_client_response_body"], true);
    assert_eq!(payload["request_body"]["model"], "gpt-5");
    assert_eq!(payload["request_body"]["stream"], false);
    assert_eq!(
        payload["request_body"]["messages"][0]["content"],
        "show real request body"
    );
    assert_eq!(payload["provider_request_body"]["temperature"], 0.2);
    assert_eq!(payload["response_body"]["id"], "resp-1");
    assert_eq!(payload["client_response_body"]["output_text"], "hello");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_detail_with_ref_backed_bodies() {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/usage-ref-detail").await;

    let mut usage = sample_usage_row(
        "usage-ref-detail",
        "req-ref-detail",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        120,
        30,
        0.3,
        0.36,
        DAY_1_UNIX_SECS,
    );
    usage.request_body = Some(json!({
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "ref-backed request body"}],
        "stream": false,
    }));
    usage.provider_request_body = Some(json!({
        "model": "gpt-5-target",
        "temperature": 0.2,
        "stream": false,
    }));
    usage.response_body = Some(json!({
        "id": "resp-ref-detail",
        "usage": {"total_tokens": 150},
    }));
    usage.client_response_body = Some(json!({
        "id": "resp-ref-detail",
        "output_text": "hello from ref",
    }));
    usage.request_metadata = Some(json!({
        "trace_id": "trace-ref-detail",
    }));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed_with_detached_bodies(
        vec![usage],
    ));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![sample_user_summary(
        "user-1", "alice",
    )]));
    let data_state = GatewayDataState::with_usage_reader_for_tests(usage_repository)
        .with_user_reader(user_repository);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage/usage-ref-detail")),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert!(payload["metadata"]["trace_id"].is_null());
    assert_eq!(payload["trace"]["trace_id"], "trace-ref-detail");
    assert!(payload["metadata"]["request_preview_source"].is_null());
    assert!(payload["metadata"]["original_request_body_available"].is_null());
    assert!(payload["metadata"]["original_response_body_available"].is_null());
    assert_eq!(payload["body_capture"]["request"]["storage"], "reference");
    assert_eq!(payload["body_capture"]["request"]["available"], true);
    assert_eq!(
        payload["body_capture"]["request"]["preview_source"],
        "stored_reference"
    );
    assert_eq!(
        payload["body_capture"]["provider_request"]["storage"],
        "reference"
    );
    assert_eq!(payload["body_capture"]["response"]["storage"], "reference");
    assert_eq!(
        payload["body_capture"]["client_response"]["storage"],
        "reference"
    );
    assert_eq!(
        payload["body_capture"]["request"]["body_ref"],
        "usage://request/req-ref-detail/request_body"
    );
    assert_eq!(
        payload["body_capture"]["provider_request"]["body_ref"],
        "usage://request/req-ref-detail/provider_request_body"
    );
    assert_eq!(
        payload["body_capture"]["response"]["body_ref"],
        "usage://request/req-ref-detail/response_body"
    );
    assert_eq!(
        payload["body_capture"]["client_response"]["body_ref"],
        "usage://request/req-ref-detail/client_response_body"
    );
    assert_eq!(payload["request_body"]["model"], "gpt-5");
    assert_eq!(
        payload["request_body"]["messages"][0]["content"],
        "ref-backed request body"
    );
    assert_eq!(payload["provider_request_body"]["temperature"], 0.2);
    assert_eq!(payload["response_body"]["id"], "resp-ref-detail");
    assert_eq!(
        payload["client_response_body"]["output_text"],
        "hello from ref"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn local_admin_usage_detail_attaches_explicit_audit() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-audit-detail",
        "req-audit-detail",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        120,
        30,
        0.3,
        0.36,
        DAY_1_UNIX_SECS,
    )]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![sample_user_summary(
        "user-1", "alice",
    )]));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_usage_reader_for_tests(usage_repository)
                .with_user_reader(user_repository),
        );

    let response = local_admin_usage_response(
        &state,
        http::Method::GET,
        "/api/admin/usage/usage-audit-detail",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("usage detail should attach audit");
    assert_eq!(audit.event_name, "admin_usage_detail_viewed");
    assert_eq!(audit.action, "view_usage_detail");
    assert_eq!(audit.target_type, "usage_record");
    assert_eq!(audit.target_id, "usage-audit-detail");
}

#[tokio::test]
async fn gateway_returns_bad_request_for_admin_usage_detail_with_empty_usage_id() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/").await;
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage/")))
            .send()
            .await
            .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "usage_id 无效");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_replay_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/usage/usage-1/replay",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-1",
        "req-1",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "success",
        120,
        50,
        0.42,
        0.37,
        DAY_1_UNIX_SECS,
    )]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "OpenAI", 10)],
        vec![sample_endpoint(
            "endpoint-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.com/v1",
        )],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_usage_reader_for_tests(usage_repository)
                    .with_provider_catalog_reader(provider_catalog_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new()
            .post(format!("{gateway_url}/api/admin/usage/usage-1/replay"))
            .json(&json!({
                "provider_id": "provider-1"
            })),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["usage_id"], "usage-1");
    assert_eq!(payload["request_id"], "req-1");
    assert_eq!(payload["mode"], "same_endpoint_reuse");
    assert_eq!(payload["target_provider_id"], "provider-1");
    assert_eq!(payload["target_endpoint_id"], "endpoint-1");
    assert_eq!(payload["target_api_format"], "openai:chat");
    assert_eq!(payload["resolved_model"], "gpt-5");
    assert_eq!(payload["mapping_source"], "none");
    assert_eq!(payload["method"], "POST");
    assert_eq!(payload["url"], "https://api.openai.com/v1/chat/completions");
    assert_eq!(
        payload["request_headers"]["Content-Type"],
        "application/json"
    );
    assert_eq!(payload["request_body"]["model"], "gpt-5");
    assert_eq!(payload["request_body"]["target_model"], "gpt-5-target");
    assert_eq!(payload["request_body"]["request_type"], "chat");
    assert_eq!(payload["request_body"]["api_format"], "openai:chat");
    assert_eq!(payload["request_body"]["stream"], false);
    assert_eq!(payload["original_request_body_available"], false);
    assert_eq!(
        payload["body_capture"]["request"]["preview_source"],
        "local_reconstruction"
    );
    assert_eq!(payload["body_capture"]["request"]["storage"], "missing");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_replay_with_ref_backed_request_body() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/usage/usage-ref-replay/replay",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let mut usage = sample_usage_row(
        "usage-ref-replay",
        "req-ref-replay",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "success",
        120,
        50,
        0.42,
        0.37,
        DAY_1_UNIX_SECS,
    );
    usage.request_body = Some(json!({
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "replay from ref"}],
        "stream": false,
    }));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed_with_detached_bodies(
        vec![usage],
    ));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "OpenAI", 10)],
        vec![sample_endpoint(
            "endpoint-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.com/v1",
        )],
        vec![],
    ));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_usage_reader_for_tests(usage_repository)
                    .with_provider_catalog_reader(provider_catalog_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new()
            .post(format!(
                "{gateway_url}/api/admin/usage/usage-ref-replay/replay"
            ))
            .json(&json!({
                "provider_id": "provider-1"
            })),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["request_body"]["model"], "gpt-5");
    assert_eq!(
        payload["request_body"]["messages"][0]["content"],
        "replay from ref"
    );
    assert_eq!(payload["original_request_body_available"], true);
    assert_eq!(
        payload["body_capture"]["request"]["preview_source"],
        "stored_reference"
    );
    assert_eq!(payload["body_capture"]["request"]["storage"], "reference");
    assert_eq!(
        payload["body_capture"]["request"]["body_ref"],
        "usage://request/req-ref-replay/request_body"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn local_admin_usage_replay_attaches_explicit_audit() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-audit-replay",
        "req-audit-replay",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "success",
        120,
        50,
        0.42,
        0.37,
        DAY_1_UNIX_SECS,
    )]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "OpenAI", 10)],
        vec![sample_endpoint(
            "endpoint-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.com/v1",
        )],
        vec![],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_usage_reader_for_tests(usage_repository)
                .with_provider_catalog_reader(provider_catalog_repository),
        );

    let response = local_admin_usage_response(
        &state,
        http::Method::POST,
        "/api/admin/usage/usage-audit-replay/replay",
        Some(json!({
            "provider_id": "provider-1"
        })),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("usage replay should attach audit");
    assert_eq!(audit.event_name, "admin_usage_replay_preview_generated");
    assert_eq!(audit.action, "preview_usage_replay");
    assert_eq!(audit.target_type, "usage_record");
    assert_eq!(audit.target_id, "usage-audit-replay");
}

#[tokio::test]
async fn gateway_handles_admin_usage_curl_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/usage-1/curl").await;

    let mut usage = sample_usage_row(
        "usage-1",
        "req-1",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        120,
        30,
        0.3,
        0.36,
        DAY_1_UNIX_SECS,
    );
    usage.request_body = Some(json!({
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "client body"}],
        "stream": false,
    }));
    usage.provider_request_headers = Some(json!({
        "Content-Type": "application/json",
        "Authorization": "Bearer provider-real",
    }));
    usage.provider_request_body = Some(json!({
        "model": "gpt-5-target",
        "stream": false,
        "temperature": 0.2,
    }));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![usage]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "provider-key-1",
            "provider-1",
            "openai:chat",
            "sk-test",
        )],
    ));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                    provider_catalog_repository,
                    usage_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage/usage-1/curl")),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["method"], "POST");
    assert_eq!(
        payload["url"],
        "https://api.openai.example/v1/chat/completions"
    );
    assert_eq!(payload["headers"]["Content-Type"], "application/json");
    assert_eq!(payload["headers"]["Authorization"], "Bearer provider-real");
    assert_eq!(payload["body"]["model"], "gpt-5-target");
    assert_eq!(payload["body"]["temperature"], 0.2);
    assert_eq!(payload["body"]["stream"], false);
    assert_eq!(payload["original_request_body_available"], true);
    assert_eq!(payload["body_capture"]["body_source"], "provider_request");
    assert_eq!(payload["body_capture"]["request"]["storage"], "inline");
    assert_eq!(
        payload["body_capture"]["request"]["preview_source"],
        "stored_original"
    );
    assert_eq!(
        payload["body_capture"]["provider_request"]["storage"],
        "inline"
    );
    let curl = payload["curl"].as_str().expect("curl should be string");
    assert!(curl.contains("curl"));
    assert!(curl.contains("https://api.openai.example/v1/chat/completions"));
    assert!(curl.contains("Content-Type: application/json"));
    assert!(curl.contains("Authorization: Bearer provider-real"));
    assert!(curl.contains("\"model\":\"gpt-5-target\""));
    assert!(curl.contains("\"temperature\":0.2"));
    assert!(curl.contains("\"stream\":false"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_curl_with_ref_backed_provider_request_body() {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/usage-ref-curl/curl").await;

    let mut usage = sample_usage_row(
        "usage-ref-curl",
        "req-ref-curl",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        120,
        30,
        0.3,
        0.36,
        DAY_1_UNIX_SECS,
    );
    usage.request_body = Some(json!({
        "model": "gpt-5",
        "messages": [{"role": "user", "content": "client body"}],
        "stream": false,
    }));
    usage.provider_request_headers = Some(json!({
        "Content-Type": "application/json",
        "Authorization": "Bearer provider-real",
    }));
    usage.provider_request_body = Some(json!({
        "model": "gpt-5-target",
        "stream": false,
        "temperature": 0.2,
    }));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed_with_detached_bodies(
        vec![usage],
    ));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "provider-key-1",
            "provider-1",
            "openai:chat",
            "sk-test",
        )],
    ));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                    provider_catalog_repository,
                    usage_repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage/usage-ref-curl/curl")),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["body"]["model"], "gpt-5-target");
    assert_eq!(payload["body"]["temperature"], 0.2);
    assert_eq!(payload["original_request_body_available"], true);
    assert_eq!(payload["body_capture"]["body_source"], "provider_request");
    assert_eq!(payload["body_capture"]["request"]["storage"], "reference");
    assert_eq!(
        payload["body_capture"]["request"]["preview_source"],
        "stored_reference"
    );
    assert_eq!(
        payload["body_capture"]["provider_request"]["storage"],
        "reference"
    );
    assert_eq!(
        payload["body_capture"]["provider_request"]["body_ref"],
        "usage://request/req-ref-curl/provider_request_body"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn local_admin_usage_curl_attaches_explicit_audit() {
    let mut usage = sample_usage_row(
        "usage-audit-curl",
        "req-audit-curl",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        120,
        30,
        0.3,
        0.36,
        DAY_1_UNIX_SECS,
    );
    usage.provider_request_headers = Some(json!({
        "Content-Type": "application/json",
        "Authorization": "Bearer provider-real",
    }));
    usage.provider_request_body = Some(json!({
        "model": "gpt-5-target",
        "stream": false,
    }));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![usage]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "provider-key-1",
            "provider-1",
            "openai:chat",
            "sk-test",
        )],
    ));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                provider_catalog_repository,
                usage_repository,
            ),
        );

    let response = local_admin_usage_response(
        &state,
        http::Method::GET,
        "/api/admin/usage/usage-audit-curl/curl",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("usage curl should attach audit");
    assert_eq!(audit.event_name, "admin_usage_curl_viewed");
    assert_eq!(audit.action, "view_usage_curl_replay");
    assert_eq!(audit.target_type, "usage_record");
    assert_eq!(audit.target_id, "usage-audit-curl");
}

#[tokio::test]
async fn gateway_returns_bad_request_for_admin_usage_curl_with_empty_usage_id() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage//curl").await;
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response =
        admin_request(reqwest::Client::new().get(format!("{gateway_url}/api/admin/usage//curl")))
            .send()
            .await
            .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "usage_id 无效");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_cache_affinity_hit_analysis_locally_with_trusted_admin_principal(
) {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/cache-affinity/hit-analysis").await;
    let mut cache_hit = sample_usage_row(
        "usage-cache-hit",
        "req-cache-hit",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        100,
        20,
        0.15,
        0.18,
        recent_unix_secs(50),
    )
    .with_cache_input_tokens(10, 50);
    cache_hit.cache_read_cost_usd = 0.02;
    cache_hit.cache_creation_cost_usd = 0.01;

    let mut cache_miss = sample_usage_row(
        "usage-cache-miss",
        "req-cache-miss",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        40,
        10,
        0.05,
        0.06,
        recent_unix_secs(10),
    )
    .with_cache_input_tokens(5, 0);
    cache_miss.cache_creation_cost_usd = 0.005;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        cache_hit, cache_miss,
    ]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/cache-affinity/hit-analysis?hours=24&user_id=user-1"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["analysis_period_hours"], 24);
    assert_eq!(payload["total_requests"], 2);
    assert_eq!(payload["requests_with_cache_hit"], 1);
    assert_eq!(payload["request_cache_hit_rate"], 50.0);
    assert_eq!(payload["total_input_tokens"], 140);
    assert_eq!(payload["total_cache_read_tokens"], 50);
    assert_eq!(payload["total_cache_creation_tokens"], 15);
    assert_eq!(payload["token_cache_hit_rate"], 35.71);
    assert_eq!(payload["total_cache_read_cost_usd"], 0.02);
    assert_eq!(payload["total_cache_creation_cost_usd"], 0.015);
    assert_eq!(payload["estimated_savings_usd"], 0.18);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_cache_affinity_interval_timeline_locally_with_trusted_admin_principal(
) {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/cache-affinity/interval-timeline").await;
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-1",
            "req-1",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            10,
            2,
            0.01,
            0.012,
            recent_unix_secs(55),
        ),
        sample_usage_row(
            "usage-2",
            "req-2",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            12,
            3,
            0.01,
            0.012,
            recent_unix_secs(50),
        ),
        sample_usage_row(
            "usage-3",
            "req-3",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "completed",
            15,
            4,
            0.02,
            0.024,
            recent_unix_secs(46),
        ),
        sample_usage_row(
            "usage-4",
            "req-4",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "completed",
            18,
            5,
            0.02,
            0.024,
            recent_unix_secs(41),
        ),
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![
        sample_user_summary("user-1", "alice"),
        sample_user_summary("user-2", "bob"),
    ]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_usage_reader_for_tests(usage_repository)
                    .with_user_reader(user_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/cache-affinity/interval-timeline?hours=24&limit=100&include_user_info=true"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["analysis_period_hours"], 24);
    assert_eq!(payload["total_points"], 2);
    assert_eq!(payload["points"].as_array().expect("array").len(), 2);
    assert_eq!(payload["points"][0]["user_id"], "user-1");
    assert_eq!(payload["points"][0]["y"], 5.0);
    assert_eq!(payload["points"][1]["user_id"], "user-2");
    assert_eq!(payload["users"]["user-1"], "alice");
    assert_eq!(payload["users"]["user-2"], "bob");
    assert_eq!(
        payload["models"].as_array().expect("array"),
        &vec![json!("claude-3-7"), json!("gpt-5")]
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_cache_affinity_interval_timeline_with_auth_user_fallback() {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/cache-affinity/interval-timeline").await;
    let mut usage_one = sample_usage_row(
        "usage-auth-fallback-1",
        "req-auth-fallback-1",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        10,
        2,
        0.01,
        0.012,
        recent_unix_secs(55),
    );
    usage_one.username = Some("stale-alice".to_string());
    let mut usage_two = sample_usage_row(
        "usage-auth-fallback-2",
        "req-auth-fallback-2",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        12,
        3,
        0.01,
        0.012,
        recent_unix_secs(50),
    );
    usage_two.username = Some("stale-alice".to_string());

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        usage_one, usage_two,
    ]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            ))
            .with_auth_users_for_tests([sample_auth_user("user-1", "fresh-alice", "user", true)]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/cache-affinity/interval-timeline?hours=24&limit=100&include_user_info=true"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total_points"], 1);
    assert_eq!(payload["points"][0]["user_id"], "user-1");
    assert_eq!(payload["users"]["user-1"], "fresh-alice");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_cache_affinity_interval_timeline_without_legacy_username_fallback_when_user_reader_exists(
) {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/cache-affinity/interval-timeline").await;
    let mut usage_one = sample_usage_row(
        "usage-auth-reader-miss-1",
        "req-auth-reader-miss-1",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        10,
        2,
        0.01,
        0.012,
        recent_unix_secs(55),
    );
    usage_one.username = Some("stale-alice".to_string());
    let mut usage_two = sample_usage_row(
        "usage-auth-reader-miss-2",
        "req-auth-reader-miss-2",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        "completed",
        12,
        3,
        0.01,
        0.012,
        recent_unix_secs(50),
    );
    usage_two.username = Some("stale-alice".to_string());

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        usage_one, usage_two,
    ]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            ))
            .with_auth_users_for_tests([sample_auth_user("user-2", "fresh-bob", "user", true)]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/cache-affinity/interval-timeline?hours=24&limit=100&include_user_info=true"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total_points"], 1);
    assert_eq!(payload["points"][0]["user_id"], "user-1");
    assert!(payload["users"]
        .as_object()
        .expect("users should be object")
        .get("user-1")
        .is_none());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_usage_cache_affinity_ttl_analysis_locally_with_trusted_admin_principal(
) {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_usage_upstream("/api/admin/usage/cache-affinity/ttl-analysis").await;
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-1",
            "req-1",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            10,
            2,
            0.01,
            0.012,
            recent_unix_secs(62),
        ),
        sample_usage_row(
            "usage-2",
            "req-2",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            12,
            3,
            0.01,
            0.012,
            recent_unix_secs(58),
        ),
        sample_usage_row(
            "usage-3",
            "req-3",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            "completed",
            13,
            3,
            0.01,
            0.012,
            recent_unix_secs(55),
        ),
        sample_usage_row(
            "usage-4",
            "req-4",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "completed",
            10,
            2,
            0.01,
            0.012,
            recent_unix_secs(100),
        ),
        sample_usage_row(
            "usage-5",
            "req-5",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "completed",
            12,
            3,
            0.01,
            0.012,
            recent_unix_secs(40),
        ),
        sample_usage_row(
            "usage-6",
            "req-6",
            Some("user-2"),
            Some("key-2"),
            Some("secondary"),
            "Anthropic",
            "claude-3-7",
            "completed",
            13,
            3,
            0.01,
            0.012,
            recent_unix_secs(0),
        ),
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![
        sample_user_summary("user-1", "alice"),
        sample_user_summary("user-2", "bob"),
    ]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_usage_reader_for_tests(usage_repository)
                    .with_user_reader(user_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/usage/cache-affinity/ttl-analysis?hours=24"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["analysis_period_hours"], 24);
    assert_eq!(payload["total_users_analyzed"], 2);
    assert_eq!(payload["ttl_distribution"]["5min"], 1);
    assert_eq!(payload["ttl_distribution"]["60min"], 1);
    let users = payload["users"].as_array().expect("array");
    assert_eq!(users.len(), 2);
    let users_by_id: std::collections::BTreeMap<_, _> = users
        .iter()
        .map(|item| {
            (
                item["group_id"].as_str().expect("group_id").to_string(),
                item.clone(),
            )
        })
        .collect();
    assert_eq!(users_by_id["user-2"]["recommended_ttl_minutes"], json!(60));
    assert_eq!(users_by_id["user-1"]["recommended_ttl_minutes"], json!(5));
    assert_eq!(users_by_id["user-1"]["username"], "alice");
    assert_eq!(users_by_id["user-1"]["email"], "alice@example.com");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
