use super::super::cache_config::ADMIN_MONITORING_REDIS_REQUIRED_DETAIL;
use super::super::test_support::{request_context, sample_key, sample_provider, sample_usage};
use super::local_monitoring_response;
use crate::AppState;
use aether_admin::observability::monitoring::{match_admin_monitoring_route, AdminMonitoringRoute};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use axum::body::to_bytes;
use serde_json::json;
use std::sync::Arc;

#[test]
fn admin_monitoring_matches_typical_routes() {
    assert_eq!(
        match_admin_monitoring_route(&http::Method::GET, "/api/admin/monitoring/audit-logs"),
        Some(AdminMonitoringRoute::AuditLogs)
    );
    assert_eq!(
        match_admin_monitoring_route(&http::Method::GET, "/api/admin/monitoring/trace/request-1"),
        Some(AdminMonitoringRoute::TraceRequest)
    );
    assert_eq!(
        match_admin_monitoring_route(&http::Method::GET, "/api/admin/monitoring/cache/stats"),
        Some(AdminMonitoringRoute::CacheStats)
    );
    assert_eq!(
        match_admin_monitoring_route(
            &http::Method::GET,
            "/api/admin/monitoring/resilience-status"
        ),
        Some(AdminMonitoringRoute::ResilienceStatus)
    );
    assert_eq!(
        match_admin_monitoring_route(
            &http::Method::GET,
            "/api/admin/monitoring/user-behavior/user-1"
        ),
        Some(AdminMonitoringRoute::UserBehavior)
    );
    assert_eq!(
        match_admin_monitoring_route(
            &http::Method::GET,
            "/api/admin/monitoring/trace/stats/provider/provider-1"
        ),
        Some(AdminMonitoringRoute::TraceProviderStats)
    );
}

#[test]
fn admin_monitoring_matches_cache_delete_shapes_and_trailing_slashes() {
    assert_eq!(
        match_admin_monitoring_route(&http::Method::DELETE, "/api/admin/monitoring/cache/"),
        Some(AdminMonitoringRoute::CacheFlush)
    );
    assert_eq!(
        match_admin_monitoring_route(
            &http::Method::DELETE,
            "/api/admin/monitoring/cache/model-mapping/provider/provider-1/model-1"
        ),
        Some(AdminMonitoringRoute::CacheModelMappingDeleteProvider)
    );
    assert_eq!(
        match_admin_monitoring_route(
            &http::Method::DELETE,
            "/api/admin/monitoring/cache/affinity/a/b/c/d"
        ),
        Some(AdminMonitoringRoute::CacheAffinityDelete)
    );
}

#[tokio::test]
async fn admin_monitoring_model_mapping_delete_requires_redis_without_runtime_or_test_entries() {
    let state = AppState::new().expect("state should build");
    let context = request_context(
        http::Method::DELETE,
        "/api/admin/monitoring/cache/model-mapping",
    );
    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("monitoring route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(
        payload,
        json!({ "detail": ADMIN_MONITORING_REDIS_REQUIRED_DETAIL })
    );
}

#[tokio::test]
async fn admin_monitoring_user_behavior_returns_empty_local_payload_without_postgres() {
    let state = AppState::new().expect("state should build");
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/user-behavior/user-123?days=30",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("user behavior route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["user_id"], json!("user-123"));
    assert_eq!(payload["period_days"], json!(30));
    assert_eq!(payload["event_counts"], json!({}));
    assert_eq!(payload["failed_requests"], json!(0));
    assert_eq!(payload["success_requests"], json!(0));
    assert_eq!(payload["success_rate"], json!(0.0));
    assert_eq!(payload["suspicious_activities"], json!(0));
    assert!(payload["analysis_time"].as_str().is_some());
}

#[tokio::test]
async fn admin_monitoring_audit_logs_returns_empty_local_payload_without_postgres() {
    let state = AppState::new().expect("state should build");
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/audit-logs?username=alice&event_type=login_failed&days=14&limit=20&offset=5",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("monitoring route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["items"], json!([]));
    assert_eq!(payload["meta"]["total"], json!(0));
    assert_eq!(payload["meta"]["limit"], json!(20));
    assert_eq!(payload["meta"]["offset"], json!(5));
    assert_eq!(payload["meta"]["count"], json!(0));
    assert_eq!(payload["filters"]["username"], json!("alice"));
    assert_eq!(payload["filters"]["event_type"], json!("login_failed"));
    assert_eq!(payload["filters"]["days"], json!(14));
}

#[tokio::test]
async fn admin_monitoring_suspicious_activities_returns_empty_local_payload_without_postgres() {
    let state = AppState::new().expect("state should build");
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/suspicious-activities?hours=48",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("monitoring route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["activities"], json!([]));
    assert_eq!(payload["count"], json!(0));
    assert_eq!(payload["time_range_hours"], json!(48));
}

#[tokio::test]
async fn admin_monitoring_resilience_status_returns_local_payload() {
    let now = chrono::Utc::now().timestamp();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![],
        vec![sample_key().with_health_fields(
            Some(json!({
                "openai:chat": {
                    "health_score": 0.25,
                    "consecutive_failures": 3,
                    "last_failure_at": "2026-03-30T12:00:00+00:00"
                }
            })),
            Some(json!({
                "openai:chat": {
                    "open": true
                }
            })),
        )],
    ));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage(
            "request-recent-failed",
            "provider-1",
            "OpenAI",
            10,
            0.10,
            "failed",
            Some(502),
            now - 120,
        ),
        sample_usage(
            "request-old-failed",
            "provider-1",
            "OpenAI",
            12,
            0.15,
            "failed",
            Some(500),
            now - 172_800,
        ),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                provider_catalog,
                usage_repository,
            ),
        );
    let context = request_context(http::Method::GET, "/api/admin/monitoring/resilience-status");

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["health_score"], json!(78));
    assert_eq!(payload["status"], json!("degraded"));
    assert_eq!(payload["error_statistics"]["total_errors"], json!(1));
    assert_eq!(
        payload["error_statistics"]["open_circuit_breakers"],
        json!(1)
    );
    assert_eq!(
        payload["error_statistics"]["circuit_breakers"]["provider-key-1"]["state"],
        json!("open")
    );
    assert_eq!(payload["recent_errors"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        payload["recent_errors"][0]["error_id"],
        json!("usage-request-recent-failed")
    );
    let recommendations = payload["recommendations"]
        .as_array()
        .expect("recommendations should be array");
    assert!(recommendations.iter().any(|item| item
        .as_str()
        .is_some_and(|value| value.contains("prod-key"))));
    assert!(payload["timestamp"].as_str().is_some());
}

#[tokio::test]
async fn admin_monitoring_cache_stats_returns_local_payload() {
    let now = chrono::Utc::now().timestamp();
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage(
            "request-cache-hit",
            "provider-1",
            "OpenAI",
            20,
            0.20,
            "success",
            Some(200),
            now - 60,
        )
        .with_cache_input_tokens(10, 5),
        sample_usage(
            "request-cache-miss",
            "provider-1",
            "OpenAI",
            15,
            0.10,
            "success",
            Some(200),
            now - 120,
        ),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_usage_reader_for_tests(usage_repository)
                .with_system_config_values_for_tests([
                    ("scheduling_mode".to_string(), json!("cache_affinity")),
                    ("provider_priority_mode".to_string(), json!("provider")),
                ]),
        );
    let context = request_context(http::Method::GET, "/api/admin/monitoring/cache/stats");

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["scheduler"], json!("cache_aware"));
    assert_eq!(payload["data"]["total_affinities"], json!(0));
    assert_eq!(payload["data"]["cache_hits"], json!(1));
    assert_eq!(payload["data"]["cache_misses"], json!(1));
    assert_eq!(payload["data"]["cache_hit_rate"], json!(0.5));
    assert_eq!(
        payload["data"]["scheduler_metrics"]["scheduling_mode"],
        json!("cache_affinity")
    );
    assert_eq!(
        payload["data"]["affinity_stats"]["storage_type"],
        json!("memory")
    );
    assert_eq!(
        payload["data"]["affinity_stats"]["config"]["default_ttl"],
        json!(300)
    );
}
