use std::sync::{Arc, Mutex};

use aether_data::repository::auth::{
    AuthApiKeyExportSummary, AuthApiKeyLookupKey, AuthApiKeyReadRepository,
    InMemoryAuthApiKeySnapshotRepository, StandaloneApiKeyExportListQuery,
    StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, StoredUserSummary,
};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use async_trait::async_trait;
use axum::body::Body;
use axum::routing::{any, get};
use axum::{extract::Request, Router};
use chrono::Utc;
use http::StatusCode;

use super::super::{
    build_router_with_state, sample_currently_usable_auth_snapshot, sample_provider, start_server,
    AppState,
};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

const DAY_0_UNIX_SECS: i64 = 1_710_913_600;
const DAY_1_UNIX_SECS: i64 = 1_711_000_000;
const DAY_2_UNIX_SECS: i64 = 1_711_086_400;

fn admin_request(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
}

async fn start_stats_upstream(
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

fn sample_usage_row(
    id: &str,
    request_id: &str,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
    api_key_name: Option<&str>,
    provider_name: &str,
    model: &str,
    input_tokens: i32,
    output_tokens: i32,
    total_cost_usd: f64,
    actual_total_cost_usd: f64,
    created_at_unix_ms: i64,
) -> StoredRequestUsageAudit {
    StoredRequestUsageAudit::new(
        id.to_string(),
        request_id.to_string(),
        user_id.map(str::to_string),
        api_key_id.map(str::to_string),
        user_id.map(|value| format!("user-{value}")),
        api_key_name.map(str::to_string),
        provider_name.to_string(),
        model.to_string(),
        None,
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
        false,
        input_tokens,
        output_tokens,
        input_tokens + output_tokens,
        total_cost_usd,
        actual_total_cost_usd,
        Some(200),
        None,
        None,
        Some(400),
        Some(120),
        "completed".to_string(),
        "settled".to_string(),
        created_at_unix_ms,
        created_at_unix_ms + 1,
        Some(created_at_unix_ms + 2),
    )
    .expect("usage row should build")
}

fn sample_user_summary(id: &str, username: &str, role: &str, is_active: bool) -> StoredUserSummary {
    StoredUserSummary::new(
        id.to_string(),
        username.to_string(),
        Some(format!("{username}@example.com")),
        role.to_string(),
        is_active,
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
        Some(Utc::now()),
        Some(Utc::now()),
    )
    .expect("auth user should build")
}

fn sample_api_key_snapshot(
    api_key_id: &str,
    user_id: &str,
    api_key_name: &str,
) -> StoredAuthApiKeySnapshot {
    let mut snapshot = sample_currently_usable_auth_snapshot(api_key_id, user_id);
    snapshot.api_key_name = Some(api_key_name.to_string());
    snapshot
}

fn recent_unix_secs(minutes_ago: u64) -> i64 {
    let now = chrono::Utc::now().timestamp();
    now.saturating_sub((minutes_ago * 60) as i64)
}

#[derive(Debug)]
struct PartialListAuthApiKeyRepository {
    lookup: InMemoryAuthApiKeySnapshotRepository,
}

#[async_trait]
impl AuthApiKeyReadRepository for PartialListAuthApiKeyRepository {
    async fn find_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, aether_data::DataLayerError> {
        self.lookup.find_api_key_snapshot(key).await
    }

    async fn list_api_key_snapshots_by_ids(
        &self,
        _api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, aether_data::DataLayerError> {
        Ok(Vec::new())
    }

    async fn list_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
        self.lookup.list_export_api_keys_by_user_ids(user_ids).await
    }

    async fn list_export_api_keys_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
        self.lookup.list_export_api_keys_by_ids(api_key_ids).await
    }

    async fn list_export_standalone_api_keys_page(
        &self,
        query: &StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
        self.lookup
            .list_export_standalone_api_keys_page(query)
            .await
    }

    async fn count_export_standalone_api_keys(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, aether_data::DataLayerError> {
        self.lookup
            .count_export_standalone_api_keys(is_active)
            .await
    }

    async fn summarize_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, aether_data::DataLayerError> {
        self.lookup
            .summarize_export_api_keys_by_user_ids(user_ids, now_unix_secs)
            .await
    }

    async fn summarize_export_non_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, aether_data::DataLayerError> {
        self.lookup
            .summarize_export_non_standalone_api_keys(now_unix_secs)
            .await
    }

    async fn summarize_export_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, aether_data::DataLayerError> {
        self.lookup
            .summarize_export_standalone_api_keys(now_unix_secs)
            .await
    }

    async fn find_export_standalone_api_key_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
        self.lookup
            .find_export_standalone_api_key_by_id(api_key_id)
            .await
    }

    async fn list_export_standalone_api_keys(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, aether_data::DataLayerError> {
        self.lookup.list_export_standalone_api_keys().await
    }
}

#[tokio::test]
async fn gateway_handles_admin_stats_provider_quota_usage_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/providers/quota-usage").await;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10).with_billing_fields(
                Some("monthly_quota".to_string()),
                Some(100.0),
                Some(25.0),
                Some(1),
                Some(1_711_000_000),
                Some(4_102_444_800),
            ),
            sample_provider("provider-anthropic", "anthropic", 20).with_billing_fields(
                Some("monthly_quota".to_string()),
                Some(50.0),
                Some(40.0),
                Some(1),
                Some(1_711_000_000),
                Some(4_102_444_800),
            ),
        ],
        vec![],
        vec![],
    ));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_provider_catalog_reader_for_tests(
                provider_catalog_repository,
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/stats/providers/quota-usage"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["providers"].as_array().map(|items| items.len()),
        Some(2)
    );
    assert_eq!(payload["providers"][0]["id"], "provider-anthropic");
    assert_eq!(payload["providers"][0]["usage_percent"], 80.0);
    assert_eq!(payload["providers"][1]["id"], "provider-openai");
    assert_eq!(payload["providers"][1]["quota_usd"], 100.0);
    assert_eq!(payload["providers"][1]["used_usd"], 25.0);
    assert_eq!(payload["providers"][1]["remaining_usd"], 75.0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_provider_quota_usage_locally_without_provider_catalog_reader()
{
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/providers/quota-usage").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(reqwest::Client::new().get(format!(
        "{gateway_url}/api/admin/stats/providers/quota-usage"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["providers"], serde_json::Value::Array(vec![]));
    assert_eq!(payload["data_source_available"], false);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_comparison_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/comparison").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-current",
            "req-current",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            120,
            30,
            0.3,
            0.36,
            DAY_1_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-previous",
            "req-previous",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            100,
            20,
            0.2,
            0.24,
            DAY_0_UNIX_SECS,
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
            "{gateway_url}/api/admin/stats/comparison?current_start=2024-03-21&current_end=2024-03-21&comparison_type=period&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["current"]["total_requests"], 1);
    assert_eq!(payload["current"]["total_tokens"], 150);
    assert_eq!(payload["current"]["total_cost"], 0.3);
    assert_eq!(payload["comparison"]["total_requests"], 1);
    assert_eq!(payload["comparison"]["total_cost"], 0.2);
    assert_eq!(payload["current_start"], "2024-03-21");
    assert_eq!(payload["comparison_start"], "2024-03-20");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_comparison_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/comparison").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/comparison?current_start=2024-03-21&current_end=2024-03-21&comparison_type=period&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["current"]["total_requests"], 0);
    assert_eq!(payload["comparison"]["total_requests"], 0);
    assert_eq!(
        payload["change_percent"]["total_cost"],
        serde_json::Value::Null
    );
    assert_eq!(payload["current_start"], "2024-03-21");
    assert_eq!(payload["comparison_start"], "2024-03-20");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_cost_forecast_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/cost/forecast").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-forecast-a",
            "req-forecast-a",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            100,
            20,
            0.1,
            0.1,
            DAY_1_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-forecast-b",
            "req-forecast-b",
            Some("user-1"),
            Some("key-1"),
            Some("primary"),
            "OpenAI",
            "gpt-5",
            120,
            40,
            0.2,
            0.2,
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
            "{gateway_url}/api/admin/stats/cost/forecast?start_date=2024-03-21&end_date=2024-03-22&forecast_days=2&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["history"].as_array().map(|items| items.len()),
        Some(2)
    );
    assert_eq!(payload["history"][0]["date"], "2024-03-21");
    assert_eq!(payload["history"][0]["total_cost"], 0.1);
    assert_eq!(payload["history"][1]["date"], "2024-03-22");
    assert_eq!(payload["history"][1]["total_cost"], 0.2);
    assert_eq!(
        payload["forecast"].as_array().map(|items| items.len()),
        Some(2)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_cost_forecast_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/cost/forecast").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/stats/cost/forecast")),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["history"], serde_json::Value::Array(vec![]));
    assert_eq!(payload["forecast"], serde_json::Value::Array(vec![]));
    assert_eq!(payload["slope"], 0.0);
    assert_eq!(payload["intercept"], 0.0);
    assert_eq!(payload["data_source_available"], false);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_error_distribution_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/errors/distribution").await;

    let mut rate_limit_a = sample_usage_row(
        "usage-error-a",
        "req-error-a",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        20,
        10,
        0.02,
        0.02,
        DAY_1_UNIX_SECS,
    );
    rate_limit_a.status_code = Some(429);
    rate_limit_a.error_category = Some("rate_limit".to_string());
    rate_limit_a.error_message = Some("rate limited".to_string());

    let mut auth_error = sample_usage_row(
        "usage-error-b",
        "req-error-b",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        20,
        10,
        0.02,
        0.02,
        DAY_1_UNIX_SECS + 60,
    );
    auth_error.status_code = Some(401);
    auth_error.error_category = Some("auth".to_string());
    auth_error.error_message = Some("bad key".to_string());

    let mut rate_limit_b = sample_usage_row(
        "usage-error-c",
        "req-error-c",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        20,
        10,
        0.02,
        0.02,
        DAY_1_UNIX_SECS + 120,
    );
    rate_limit_b.status_code = Some(429);
    rate_limit_b.error_category = Some("rate_limit".to_string());
    rate_limit_b.error_message = Some("rate limited again".to_string());

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        rate_limit_a,
        auth_error,
        rate_limit_b,
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
            "{gateway_url}/api/admin/stats/errors/distribution?start_date=2024-03-21&end_date=2024-03-21&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["distribution"][0]["category"], "rate_limit");
    assert_eq!(payload["distribution"][0]["count"], 2);
    assert_eq!(payload["distribution"][1]["category"], "auth");
    assert_eq!(payload["distribution"][1]["count"], 1);
    assert_eq!(payload["trend"][0]["date"], "2024-03-21");
    assert_eq!(payload["trend"][0]["total"], 3);
    assert_eq!(
        payload["trend"][0]["categories"]["rate_limit"],
        serde_json::Value::from(2)
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_error_distribution_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/errors/distribution").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/errors/distribution?start_date=2024-03-21&end_date=2024-03-21&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["distribution"], serde_json::Value::Array(vec![]));
    assert_eq!(payload["trend"], serde_json::Value::Array(vec![]));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_defaults_admin_stats_error_distribution_to_bounded_recent_window_when_query_missing(
) {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/errors/distribution").await;

    let mut recent_error = sample_usage_row(
        "usage-error-recent",
        "req-error-recent",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        20,
        10,
        0.02,
        0.02,
        recent_unix_secs(5),
    );
    recent_error.status_code = Some(429);
    recent_error.error_category = Some("rate_limit".to_string());
    recent_error.error_message = Some("rate limited".to_string());

    let mut stale_error = sample_usage_row(
        "usage-error-stale",
        "req-error-stale",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        20,
        10,
        0.02,
        0.02,
        recent_unix_secs(60 * 48),
    );
    stale_error.status_code = Some(401);
    stale_error.error_category = Some("auth".to_string());
    stale_error.error_message = Some("bad key".to_string());

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        recent_error,
        stale_error,
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
        reqwest::Client::new().get(format!("{gateway_url}/api/admin/stats/errors/distribution")),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["distribution"].as_array().map(|items| items.len()),
        Some(1)
    );
    assert_eq!(payload["distribution"][0]["category"], "rate_limit");
    assert_eq!(payload["distribution"][0]["count"], 1);
    assert_eq!(payload["trend"][0]["total"], 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_performance_percentiles_locally_with_trusted_admin_principal()
{
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/performance/percentiles").await;

    let usage: Vec<_> = (1..=10)
        .map(|index| {
            let mut row = sample_usage_row(
                &format!("usage-perf-{index}"),
                &format!("req-perf-{index}"),
                Some("user-1"),
                Some("key-1"),
                Some("primary"),
                "OpenAI",
                "gpt-5",
                10,
                5,
                0.01,
                0.01,
                DAY_1_UNIX_SECS + i64::from(index),
            );
            row.response_time_ms = Some((index * 100) as u64);
            row.first_byte_time_ms = Some((index * 10) as u64);
            row
        })
        .collect();
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(usage));

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
            "{gateway_url}/api/admin/stats/performance/percentiles?start_date=2024-03-21&end_date=2024-03-21&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload.as_array().map(|items| items.len()), Some(1));
    assert_eq!(payload[0]["date"], "2024-03-21");
    assert_eq!(payload[0]["p50_response_time_ms"], 550);
    assert_eq!(payload[0]["p90_response_time_ms"], 910);
    assert_eq!(payload[0]["p99_response_time_ms"], 991);
    assert_eq!(payload[0]["p50_first_byte_time_ms"], 55);
    assert_eq!(payload[0]["p90_first_byte_time_ms"], 91);
    assert_eq!(payload[0]["p99_first_byte_time_ms"], 99);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_performance_percentiles_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/performance/percentiles").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/performance/percentiles?start_date=2024-03-21&end_date=2024-03-21&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, serde_json::Value::Array(vec![]));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_time_series_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/time-series").await;

    let mut first_day = sample_usage_row(
        "usage-ts-a",
        "req-ts-a",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        100,
        20,
        0.1,
        0.1,
        DAY_1_UNIX_SECS,
    );
    first_day.cache_creation_input_tokens = 5;
    first_day.cache_read_input_tokens = 7;

    let mut second_day = sample_usage_row(
        "usage-ts-b",
        "req-ts-b",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        60,
        40,
        0.2,
        0.2,
        DAY_2_UNIX_SECS,
    );
    second_day.cache_creation_input_tokens = 2;
    second_day.cache_read_input_tokens = 3;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        first_day, second_day,
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
            "{gateway_url}/api/admin/stats/time-series?start_date=2024-03-21&end_date=2024-03-22&granularity=day&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload.as_array().map(|items| items.len()), Some(2));
    assert_eq!(payload[0]["date"], "2024-03-21");
    assert_eq!(payload[0]["total_requests"], 1);
    assert_eq!(payload[0]["input_tokens"], 100);
    assert_eq!(payload[0]["cache_creation_tokens"], 5);
    assert_eq!(payload[0]["cache_read_tokens"], 7);
    assert_eq!(payload[1]["date"], "2024-03-22");
    assert_eq!(payload[1]["total_cost"], 0.2);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_time_series_hourly_locally_with_model_filter() {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/time-series").await;

    let mut matching_row = sample_usage_row(
        "usage-ts-hour-match",
        "req-ts-hour-match",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        80,
        20,
        0.12,
        0.12,
        1_710_997_800,
    );
    matching_row.cache_creation_input_tokens = 4;
    matching_row.cache_read_input_tokens = 6;

    let other_model_row = sample_usage_row(
        "usage-ts-hour-other",
        "req-ts-hour-other",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-4o-mini",
        200,
        40,
        0.22,
        0.22,
        1_710_998_400,
    );

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        matching_row,
        other_model_row,
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
            "{gateway_url}/api/admin/stats/time-series?start_date=2024-03-21&end_date=2024-03-21&granularity=hour&model=gpt-5&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload.as_array().map(|items| items.len()), Some(24));
    assert_eq!(payload[5]["date"], "2024-03-21T05:00:00+00:00");
    assert_eq!(payload[5]["total_requests"], 1);
    assert_eq!(payload[5]["input_tokens"], 80);
    assert_eq!(payload[5]["cache_creation_tokens"], 4);
    assert_eq!(payload[5]["cache_read_tokens"], 6);
    assert_eq!(payload[6]["total_requests"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_time_series_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/time-series").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/time-series?start_date=2024-03-21&end_date=2024-03-21&granularity=day&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, serde_json::Value::Array(vec![]));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_cost_savings_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/cost/savings").await;

    let mut usage_row = sample_usage_row(
        "usage-cache-a",
        "req-cache-a",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        40,
        10,
        0.03,
        0.03,
        DAY_1_UNIX_SECS,
    );
    usage_row.cache_creation_input_tokens = 20;
    usage_row.cache_read_input_tokens = 100;
    usage_row.cache_creation_cost_usd = 0.001;
    usage_row.cache_read_cost_usd = 0.002;
    usage_row.output_price_per_1m = Some(50.0);

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![usage_row]));

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
            "{gateway_url}/api/admin/stats/cost/savings?start_date=2024-03-21&end_date=2024-03-21&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["cache_read_tokens"], 100);
    assert_eq!(payload["cache_read_cost"], 0.002);
    assert_eq!(payload["cache_creation_cost"], 0.001);
    assert_eq!(payload["estimated_full_cost"], 0.005);
    assert_eq!(payload["cache_savings"], 0.003);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_cost_savings_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/cost/savings").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/cost/savings?start_date=2024-03-21&end_date=2024-03-21&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["cache_read_tokens"], 0);
    assert_eq!(payload["cache_read_cost"], 0.0);
    assert_eq!(payload["cache_creation_cost"], 0.0);
    assert_eq!(payload["estimated_full_cost"], 0.0);
    assert_eq!(payload["cache_savings"], 0.0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_models_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/models").await;

    let mut openai_row = sample_usage_row(
        "usage-model-a",
        "req-model-a",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        100,
        50,
        0.4,
        0.4,
        DAY_1_UNIX_SECS,
    );
    openai_row.cache_creation_input_tokens = 10;
    openai_row.cache_read_input_tokens = 20;

    let claude_row = sample_usage_row(
        "usage-model-b",
        "req-model-b",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "Anthropic",
        "claude-3-5-sonnet",
        70,
        30,
        0.2,
        0.2,
        DAY_1_UNIX_SECS + 5,
    );

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        openai_row, claude_row,
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
            "{gateway_url}/api/admin/stats/leaderboard/models?start_date=2024-03-21&end_date=2024-03-21&metric=tokens&order=desc&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 2);
    assert_eq!(payload["metric"], "tokens");
    assert_eq!(payload["items"][0]["rank"], 1);
    assert_eq!(payload["items"][0]["id"], "gpt-5");
    assert_eq!(payload["items"][0]["value"], 180);
    assert_eq!(payload["items"][1]["id"], "claude-3-5-sonnet");
    assert_eq!(payload["items"][1]["value"], 100);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_models_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/models").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/leaderboard/models?start_date=2024-03-21&end_date=2024-03-21&metric=tokens&order=desc&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["items"], serde_json::Value::Array(vec![]));
    assert_eq!(payload["total"], 0);
    assert_eq!(payload["metric"], "tokens");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_defaults_admin_stats_leaderboard_models_to_bounded_recent_window_when_query_missing(
) {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/models").await;

    let recent_row = sample_usage_row(
        "usage-model-recent",
        "req-model-recent",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "OpenAI",
        "gpt-5",
        100,
        50,
        0.4,
        0.4,
        recent_unix_secs(10),
    );
    let stale_row = sample_usage_row(
        "usage-model-stale",
        "req-model-stale",
        Some("user-1"),
        Some("key-1"),
        Some("primary"),
        "Anthropic",
        "claude-3-5-sonnet",
        60,
        20,
        0.2,
        0.2,
        recent_unix_secs(60 * 48),
    );

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        recent_row, stale_row,
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
        "{gateway_url}/api/admin/stats/leaderboard/models?metric=tokens&order=desc"
    )))
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["items"][0]["id"], "gpt-5");
    assert_eq!(payload["items"][0]["value"], 150);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_api_keys_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/api-keys").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-key-a",
            "req-key-a",
            Some("user-1"),
            Some("key-1"),
            Some("primary-key"),
            "OpenAI",
            "gpt-5",
            80,
            20,
            0.3,
            0.3,
            DAY_1_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-key-b",
            "req-key-b",
            Some("user-admin"),
            Some("key-admin"),
            Some("admin-key"),
            "OpenAI",
            "gpt-5",
            50,
            10,
            0.9,
            0.9,
            DAY_1_UNIX_SECS + 10,
        ),
    ]));

    let mut user_snapshot = sample_api_key_snapshot("key-1", "user-1", "primary-key");
    user_snapshot.username = "alice".to_string();
    let mut admin_snapshot = sample_api_key_snapshot("key-admin", "user-admin", "admin-key");
    admin_snapshot.username = "root".to_string();
    admin_snapshot.user_role = "admin".to_string();

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![
        (None, user_snapshot),
        (None, admin_snapshot),
    ]));
    let data_state = GatewayDataState::with_usage_reader_for_tests(usage_repository)
        .with_auth_api_key_reader(auth_repository);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/leaderboard/api-keys?start_date=2024-03-21&end_date=2024-03-21&metric=cost&order=desc&exclude_admin=true&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["items"][0]["id"], "key-1");
    assert_eq!(payload["items"][0]["name"], "primary-key");
    assert_eq!(payload["items"][0]["cost"], 0.3);
    assert_eq!(payload["items"][0]["value"], 0.3);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_api_keys_locally_without_auth_snapshot_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/api-keys").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-key-a",
            "req-key-a",
            Some("user-1"),
            Some("key-1"),
            Some("primary-key"),
            "OpenAI",
            "gpt-5",
            80,
            20,
            0.3,
            0.3,
            DAY_1_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-key-b",
            "req-key-b",
            Some("user-2"),
            Some("key-2"),
            Some("secondary-key"),
            "Anthropic",
            "claude-3-5-sonnet",
            40,
            10,
            0.1,
            0.1,
            DAY_1_UNIX_SECS + 10,
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
            "{gateway_url}/api/admin/stats/leaderboard/api-keys?start_date=2024-03-21&end_date=2024-03-21&metric=tokens&order=desc&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 2);
    assert_eq!(payload["items"][0]["id"], "key-1");
    assert_eq!(payload["items"][0]["name"], "primary-key");
    assert_eq!(payload["items"][0]["value"], 100);
    assert_eq!(payload["items"][1]["id"], "key-2");
    assert_eq!(payload["items"][1]["name"], "secondary-key");
    assert_eq!(payload["items"][1]["value"], 50);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_api_keys_with_auth_snapshot_single_lookup_fallback(
) {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/api-keys").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage_row(
        "usage-key-partial-list",
        "req-key-partial-list",
        Some("user-1"),
        Some("key-1"),
        Some("legacy-key"),
        "OpenAI",
        "gpt-5",
        80,
        20,
        0.3,
        0.3,
        DAY_1_UNIX_SECS,
    )]));
    let auth_repository = Arc::new(PartialListAuthApiKeyRepository {
        lookup: InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            None,
            sample_api_key_snapshot("key-1", "user-1", "fresh-key"),
        )]),
    });

    let data_state = GatewayDataState::with_usage_reader_for_tests(usage_repository)
        .with_auth_api_key_reader(auth_repository);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/leaderboard/api-keys?start_date=2024-03-21&end_date=2024-03-21&metric=cost&order=desc&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["items"][0]["id"], "key-1");
    assert_eq!(payload["items"][0]["name"], "fresh-key");
    assert_eq!(payload["items"][0]["value"], 0.3);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_api_keys_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/api-keys").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/leaderboard/api-keys?start_date=2024-03-21&end_date=2024-03-21&metric=tokens&order=desc&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["items"], serde_json::Value::Array(vec![]));
    assert_eq!(payload["total"], 0);
    assert_eq!(payload["metric"], "tokens");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_users_locally_with_trusted_admin_principal() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/users").await;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage_row(
            "usage-user-a",
            "req-user-a",
            Some("user-1"),
            Some("key-1"),
            Some("primary-key"),
            "OpenAI",
            "gpt-5",
            60,
            20,
            0.4,
            0.4,
            DAY_1_UNIX_SECS,
        ),
        sample_usage_row(
            "usage-user-b",
            "req-user-b",
            Some("user-admin"),
            Some("key-admin"),
            Some("admin-key"),
            "OpenAI",
            "gpt-5",
            100,
            40,
            1.2,
            1.2,
            DAY_1_UNIX_SECS + 15,
        ),
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed(vec![
        sample_user_summary("user-1", "alice", "user", true),
        sample_user_summary("user-admin", "root", "admin", true),
    ]));
    let data_state = GatewayDataState::with_usage_reader_for_tests(usage_repository)
        .with_user_reader(user_repository);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/leaderboard/users?start_date=2024-03-21&end_date=2024-03-21&metric=cost&order=desc&exclude_admin=true&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["items"][0]["id"], "user-1");
    assert_eq!(payload["items"][0]["name"], "alice");
    assert_eq!(payload["items"][0]["cost"], 0.4);
    assert_eq!(payload["items"][0]["value"], 0.4);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_users_locally_without_user_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/users").await;

    let mut usage_user = sample_usage_row(
        "usage-user-a-fallback",
        "req-user-a-fallback",
        Some("user-1"),
        Some("key-1"),
        Some("primary-key"),
        "OpenAI",
        "gpt-5",
        60,
        20,
        0.4,
        0.4,
        DAY_1_UNIX_SECS,
    );
    usage_user.username = Some("stale-alice".to_string());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        usage_user,
        sample_usage_row(
            "usage-user-b-fallback",
            "req-user-b-fallback",
            Some("user-admin"),
            Some("key-admin"),
            Some("admin-key"),
            "OpenAI",
            "gpt-5",
            100,
            40,
            1.2,
            1.2,
            DAY_1_UNIX_SECS + 15,
        ),
    ]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_usage_reader_for_tests(
                usage_repository,
            ))
            .with_auth_users_for_tests([
                sample_auth_user("user-1", "alice", "user", true),
                sample_auth_user("user-admin", "root", "admin", true),
            ]),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/leaderboard/users?start_date=2024-03-21&end_date=2024-03-21&metric=cost&order=desc&exclude_admin=true&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["items"][0]["id"], "user-1");
    assert_eq!(payload["items"][0]["name"], "alice");
    assert_eq!(payload["items"][0]["cost"], 0.4);
    assert_eq!(payload["items"][0]["value"], 0.4);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_users_without_legacy_username_fallback_when_user_reader_exists(
) {
    let (_upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/users").await;

    let mut usage_user = sample_usage_row(
        "usage-user-no-legacy-fallback",
        "req-user-no-legacy-fallback",
        Some("user-missing"),
        Some("key-1"),
        Some("primary-key"),
        "OpenAI",
        "gpt-5",
        60,
        20,
        0.4,
        0.4,
        DAY_1_UNIX_SECS,
    );
    usage_user.username = Some("stale-alice".to_string());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![usage_user]));
    let data_state = GatewayDataState::with_usage_reader_for_tests(usage_repository)
        .with_user_reader(Arc::new(InMemoryUserReadRepository::seed_auth_users(
            Vec::<StoredUserAuthRecord>::new(),
        )));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .without_auth_user_store_for_tests(),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/leaderboard/users?start_date=2024-03-21&end_date=2024-03-21&metric=cost&order=desc&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["items"][0]["id"], "user-missing");
    assert_eq!(payload["items"][0]["name"], "user-missing");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_stats_leaderboard_users_locally_without_usage_reader() {
    let (upstream_url, upstream_hits, upstream_handle) =
        start_stats_upstream("/api/admin/stats/leaderboard/users").await;

    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = admin_request(
        reqwest::Client::new().get(format!(
            "{gateway_url}/api/admin/stats/leaderboard/users?start_date=2024-03-21&end_date=2024-03-21&metric=cost&order=desc&exclude_admin=true&tz_offset_minutes=0"
        )),
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["items"], serde_json::Value::Array(vec![]));
    assert_eq!(payload["total"], 0);
    assert_eq!(payload["metric"], "cost");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
