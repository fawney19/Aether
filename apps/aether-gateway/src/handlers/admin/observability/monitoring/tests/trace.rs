use super::super::test_support::{
    request_context, sample_candidate, sample_endpoint, sample_key, sample_provider, sample_usage,
};
use super::local_monitoring_response;
use crate::AppState;
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use axum::body::to_bytes;
use serde_json::json;
use std::sync::Arc;

use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;

#[tokio::test]
async fn admin_monitoring_trace_request_returns_local_payload() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_candidate(
            "cand-unused",
            "request-1",
            0,
            RequestCandidateStatus::Pending,
            None,
            None,
            None,
        ),
        sample_candidate(
            "cand-used",
            "request-1",
            1,
            RequestCandidateStatus::Failed,
            Some(101),
            Some(33),
            Some(502),
        ),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let state = AppState::new()
        .expect("state should build")
        .with_decision_trace_data_readers_for_tests(request_candidates, provider_catalog);
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/trace/request-1?attempted_only=true",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["request_id"], json!("request-1"));
    assert_eq!(payload["total_candidates"], json!(1));
    assert_eq!(payload["final_status"], json!("failed"));
    assert_eq!(payload["candidates"][0]["id"], json!("cand-used"));
    assert_eq!(payload["candidates"][0]["provider_name"], json!("OpenAI"));
    assert_eq!(
        payload["candidates"][0]["provider_website"],
        json!("https://openai.com")
    );
    assert_eq!(
        payload["candidates"][0]["endpoint_name"],
        json!("openai:chat")
    );
    assert_eq!(payload["candidates"][0]["key_name"], json!("prod-key"));
    assert_eq!(payload["candidates"][0]["key_auth_type"], json!("api_key"));
    assert_eq!(payload["candidates"][0]["latency_ms"], json!(33));
    assert_eq!(payload["candidates"][0]["status_code"], json!(502));
}

#[tokio::test]
async fn admin_monitoring_trace_final_status_prefers_failed_over_stale_pending() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_candidate(
            "cand-stale-pending",
            "request-1",
            0,
            RequestCandidateStatus::Pending,
            None,
            None,
            None,
        ),
        sample_candidate(
            "cand-failed",
            "request-1",
            1,
            RequestCandidateStatus::Failed,
            Some(101),
            Some(33),
            Some(502),
        ),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let state = AppState::new()
        .expect("state should build")
        .with_decision_trace_data_readers_for_tests(request_candidates, provider_catalog);
    let context = request_context(http::Method::GET, "/api/admin/monitoring/trace/request-1");

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["final_status"], json!("failed"));
}

#[tokio::test]
async fn admin_monitoring_trace_request_keeps_format_conversion_disabled_candidates_visible() {
    let mut format_disabled_candidate = sample_candidate(
        "cand-format-disabled",
        "request-1",
        0,
        RequestCandidateStatus::Skipped,
        None,
        None,
        None,
    );
    format_disabled_candidate.skip_reason = Some("format_conversion_disabled".to_string());

    let mut visible_skipped_candidate = sample_candidate(
        "cand-visible-skipped",
        "request-1",
        1,
        RequestCandidateStatus::Skipped,
        None,
        None,
        None,
    );
    visible_skipped_candidate.skip_reason = Some("transport_unsupported".to_string());

    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        format_disabled_candidate,
        visible_skipped_candidate,
        sample_candidate(
            "cand-used",
            "request-1",
            2,
            RequestCandidateStatus::Failed,
            Some(101),
            Some(33),
            Some(502),
        ),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let state = AppState::new()
        .expect("state should build")
        .with_decision_trace_data_readers_for_tests(request_candidates, provider_catalog);
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/trace/request-1?attempted_only=false",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");

    assert_eq!(payload["total_candidates"], json!(3));
    assert_eq!(
        payload["candidates"]
            .as_array()
            .expect("candidates should be an array")
            .iter()
            .map(|item| item["id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["cand-format-disabled", "cand-visible-skipped", "cand-used"]
    );
    assert_eq!(
        payload["candidates"][0]["skip_reason"],
        json!("format_conversion_disabled")
    );
    assert_eq!(
        payload["candidates"][1]["skip_reason"],
        json!("transport_unsupported")
    );
}

#[tokio::test]
async fn admin_monitoring_trace_request_enriches_proxy_timing_from_usage_audit() {
    let mut candidate = sample_candidate(
        "cand-used",
        "request-1",
        1,
        RequestCandidateStatus::Success,
        Some(101),
        Some(33),
        Some(200),
    );
    candidate.extra_data = Some(json!({
        "proxy": {
            "node_id": "proxy-node-1",
            "node_name": "edge-1",
            "source": "provider"
        }
    }));

    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![candidate]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let mut usage = sample_usage(
        "request-1",
        "provider-1",
        "OpenAI",
        40,
        0.02,
        "completed",
        Some(200),
        100,
    );
    usage.candidate_id = Some("cand-used".to_string());
    usage.response_headers = Some(json!({
        "x-proxy-timing": "{\"connection_acquire_ms\":125,\"response_wait_ms\":475,\"ttfb_ms\":600}"
    }));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![usage]));
    let data_state =
        crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
            request_candidates,
            usage_repository,
        )
        .with_provider_catalog_reader(provider_catalog);
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(data_state);
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/trace/request-1?attempted_only=true",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(
        payload["candidates"][0]["extra_data"]["first_byte_time_ms"],
        json!(30)
    );
    assert_eq!(
        payload["candidates"][0]["extra_data"]["proxy"]["ttfb_ms"],
        json!(600)
    );
    assert_eq!(
        payload["candidates"][0]["extra_data"]["proxy"]["timing"]["connection_acquire_ms"],
        json!(125)
    );
    assert_eq!(
        payload["candidates"][0]["extra_data"]["proxy"]["timing"]["response_wait_ms"],
        json!(475)
    );
}

#[tokio::test]
async fn admin_monitoring_trace_provider_stats_returns_local_payload() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_candidate(
            "cand-1",
            "req-a",
            0,
            RequestCandidateStatus::Success,
            Some(101),
            Some(20),
            Some(200),
        ),
        sample_candidate(
            "cand-2",
            "req-b",
            0,
            RequestCandidateStatus::Failed,
            Some(201),
            Some(40),
            Some(502),
        ),
        sample_candidate(
            "cand-3",
            "req-c",
            0,
            RequestCandidateStatus::Cancelled,
            Some(301),
            Some(60),
            Some(499),
        ),
        sample_candidate(
            "cand-4",
            "req-d",
            0,
            RequestCandidateStatus::Available,
            None,
            None,
            None,
        ),
        sample_candidate(
            "cand-5",
            "req-e",
            0,
            RequestCandidateStatus::Unused,
            None,
            None,
            None,
        ),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_request_candidate_data_reader_for_tests(request_candidates);
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/trace/stats/provider/provider-1?limit=10",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["provider_id"], json!("provider-1"));
    assert_eq!(payload["total_attempts"], json!(5));
    assert_eq!(payload["success_count"], json!(1));
    assert_eq!(payload["failed_count"], json!(1));
    assert_eq!(payload["cancelled_count"], json!(1));
    assert_eq!(payload["skipped_count"], json!(0));
    assert_eq!(payload["pending_count"], json!(0));
    assert_eq!(payload["available_count"], json!(1));
    assert_eq!(payload["unused_count"], json!(1));
    assert_eq!(payload["failure_rate"], json!(50.0));
    assert_eq!(payload["avg_latency_ms"], json!(40.0));
}

#[tokio::test]
async fn admin_monitoring_trace_request_returns_contextual_not_found_payload() {
    let state = AppState::new().expect("state should build");
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/trace/provider-test-missing?attempted_only=false",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["detail"], json!("Request trace not found"));
    assert_eq!(payload["request_id"], json!("provider-test-missing"));
    assert_eq!(payload["attempted_only"], json!(false));
}
