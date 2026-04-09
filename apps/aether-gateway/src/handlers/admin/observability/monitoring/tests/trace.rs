use super::super::test_support::{
    request_context, sample_candidate, sample_endpoint, sample_key, sample_provider,
};
use super::local_monitoring_response;
use crate::AppState;
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use axum::body::to_bytes;
use serde_json::json;
use std::sync::Arc;

use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;

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
