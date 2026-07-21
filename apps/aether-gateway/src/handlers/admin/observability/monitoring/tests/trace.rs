use super::super::test_support::{
    request_context, sample_candidate, sample_endpoint, sample_key, sample_provider, sample_usage,
};
use super::local_monitoring_response;
use crate::data::GatewayDataState;
use crate::AppState;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data_contracts::repository::{
    candidates::RequestCandidateStatus, usage::UsageBodyCaptureState,
};
use axum::body::to_bytes;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
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
async fn admin_monitoring_trace_request_resolves_usage_id_to_header_trace_id() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_candidate(
            "cand-used",
            "trace-1",
            0,
            RequestCandidateStatus::Success,
            Some(101),
            Some(33),
            Some(200),
        ),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let mut usage = sample_usage(
        "usage-request-1",
        "provider-1",
        "OpenAI",
        40,
        0.02,
        "completed",
        Some(200),
        100,
    );
    usage.id = "usage-row-1".to_string();
    usage.candidate_id = Some("cand-used".to_string());
    usage.request_headers = Some(json!({
        "x-trace-id": "trace-1"
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
        "/api/admin/monitoring/trace/usage-row-1?attempted_only=true",
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
    assert_eq!(payload["request_id"], json!("trace-1"));
    assert_eq!(payload["candidates"][0]["id"], json!("cand-used"));
    assert_eq!(
        payload["candidates"][0]["extra_data"]["first_byte_time_ms"],
        json!(30)
    );
}

#[tokio::test]
async fn admin_monitoring_trace_request_resolves_usage_request_id_to_metadata_trace_id() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_candidate(
            "cand-used",
            "trace-2",
            0,
            RequestCandidateStatus::Success,
            Some(101),
            Some(33),
            Some(200),
        ),
    ]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let mut usage = sample_usage(
        "usage-request-2",
        "provider-1",
        "OpenAI",
        40,
        0.02,
        "completed",
        Some(200),
        100,
    );
    usage.candidate_id = Some("cand-used".to_string());
    usage.request_metadata = Some(json!({
        "trace_id": "trace-2"
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
        "/api/admin/monitoring/trace/usage-request-2",
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
    assert_eq!(payload["request_id"], json!("trace-2"));
    assert_eq!(payload["candidates"][0]["id"], json!("cand-used"));
}

#[tokio::test]
async fn admin_monitoring_trace_request_falls_back_to_usage_routing_snapshot() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let mut usage = sample_usage(
        "request-usage-snapshot",
        "provider-1",
        "OpenAI",
        0,
        0.0,
        "failed",
        Some(503),
        100,
    );
    usage.candidate_id = Some("routing-cand-1".to_string());
    usage.candidate_index = Some(0);
    usage.planner_kind = Some("openai_responses_stream".to_string());
    usage.execution_path = Some("local_execution_runtime_miss".to_string());
    usage.local_execution_runtime_miss_reason = Some("no_local_stream_plans".to_string());
    usage.api_format = Some("openai:responses".to_string());
    usage.endpoint_api_format = Some("openai:responses".to_string());
    usage.provider_api_key_id = Some("provider-key-1".to_string());
    usage.error_message = Some("no local stream plans".to_string());
    usage.response_time_ms = Some(45);

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
        "/api/admin/monitoring/trace/request-usage-snapshot?attempted_only=true",
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
    assert_eq!(payload["request_id"], json!("request-usage-snapshot"));
    assert_eq!(payload["total_candidates"], json!(1));
    assert_eq!(payload["final_status"], json!("failed"));
    assert_eq!(payload["candidates"][0]["id"], json!("routing-cand-1"));
    assert_eq!(payload["candidates"][0]["status"], json!("failed"));
    assert_eq!(
        payload["candidates"][0]["error_type"],
        json!("no_local_stream_plans")
    );
    assert_eq!(
        payload["candidates"][0]["extra_data"]["source"],
        json!("usage_routing_snapshot")
    );
    assert_eq!(
        payload["candidates"][0]["extra_data"]["execution_path"],
        json!("local_execution_runtime_miss")
    );
}

#[tokio::test]
async fn admin_monitoring_trace_request_returns_oauth_account_label_from_auth_config() {
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![
        sample_candidate(
            "cand-used",
            "request-1",
            0,
            RequestCandidateStatus::Failed,
            Some(101),
            Some(33),
            Some(502),
        ),
    ]));
    let auth_config = json!({
        "provider_type": "codex",
        "email": "codex_alice@example.com",
        "plan_type": "plus",
        "refresh_token": "rt-test"
    })
    .to_string();
    let oauth_key = sample_key()
        .with_transport_fields(
            None,
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
                .expect("placeholder should encrypt"),
            Some(
                encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, &auth_config)
                    .expect("auth config should encrypt"),
            ),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport fields should build");
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![oauth_key],
    ));
    let data_state = GatewayDataState::with_decision_trace_readers_for_tests(
        request_candidates,
        provider_catalog,
    )
    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(data_state);
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
    assert_eq!(
        payload["candidates"][0]["key_account_label"],
        json!("codex_alice@example.com")
    );
    assert_eq!(
        payload["candidates"][0]["key_oauth_plan_type"],
        json!("plus")
    );
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
    assert!(payload["candidates"][0]["extra_data"]
        .get("upstream_response")
        .is_none());
}

#[tokio::test]
async fn admin_monitoring_trace_request_exposes_request_path_from_usage_audit() {
    let mut candidate = sample_candidate(
        "cand-used",
        "request-1",
        0,
        RequestCandidateStatus::Failed,
        Some(101),
        Some(33),
        Some(403),
    );
    candidate.extra_data = Some(json!({
        "client_api_format": "gemini:generate_content"
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
        "failed",
        Some(403),
        100,
    );
    usage.candidate_id = Some("cand-used".to_string());
    usage.request_metadata = Some(json!({
        "request_path": "/v1beta/models/gemini-2.5-pro:generateContent",
        "request_query_string": "alt=sse"
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

    assert_eq!(
        payload["request_path"],
        json!("/v1beta/models/gemini-2.5-pro:generateContent")
    );
    assert_eq!(payload["request_query_string"], json!("alt=sse"));
    assert_eq!(
        payload["request_path_and_query"],
        json!("/v1beta/models/gemini-2.5-pro:generateContent?alt=sse")
    );
    assert_eq!(
        payload["candidates"][0]["extra_data"]["request_path_and_query"],
        json!("/v1beta/models/gemini-2.5-pro:generateContent?alt=sse")
    );
}

#[tokio::test]
async fn admin_monitoring_trace_request_exposes_failed_candidate_upstream_response_boundary() {
    let mut candidate = sample_candidate(
        "cand-used",
        "request-1",
        0,
        RequestCandidateStatus::Failed,
        Some(101),
        Some(33),
        Some(302),
    );
    candidate.extra_data = Some(json!({
        "cache_1h": true,
        "upstream_response": {
            "status_code": 302,
            "body": {
                "error": {
                    "message": "redirect blocked"
                }
            }
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
        "failed",
        Some(302),
        100,
    );
    usage.candidate_id = Some("cand-used".to_string());
    usage.response_headers = Some(json!({
        "location": "/",
        "content-type": "text/html"
    }));
    usage.client_response_headers = Some(json!({
        "content-type": "application/json",
        "x-aether-upstream-status": "302"
    }));
    usage.client_response_body = Some(json!({
        "error": {
            "type": "execution_runtime_non_success_status",
            "message": "execution runtime stream returned non-success status 302",
            "upstream_status": 302,
            "location": "/"
        }
    }));
    usage.request_metadata = Some(json!({
        "client_response_status_code": 502
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
    let extra = &payload["candidates"][0]["extra_data"];
    assert_eq!(extra["upstream_response"]["status_code"], json!(302));
    assert_eq!(
        extra["upstream_response"]["headers"]["location"],
        json!("/")
    );
    assert_eq!(
        extra["upstream_response"]["body"]["error"]["message"],
        json!("redirect blocked")
    );
    assert!(extra.get("client_response").is_none());
    assert!(extra.get("provider_response").is_none());
}

#[tokio::test]
async fn admin_monitoring_trace_request_prefers_ref_backed_usage_response_body() {
    let mut candidate = sample_candidate(
        "cand-used",
        "request-ref-body",
        0,
        RequestCandidateStatus::Failed,
        Some(101),
        Some(33),
        Some(400),
    );
    candidate.extra_data = Some(json!({
        "upstream_response": {
            "status_code": 400,
            "headers": {
                "content-type": "text/event-stream",
                "x-request-id": "stale-request-like-body"
            },
            "body": {
                "model": "gpt-5.6-sol",
                "input": [{"role": "user", "content": "request prompt"}]
            }
        }
    }));

    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![candidate]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let mut usage = sample_usage(
        "request-ref-body",
        "provider-1",
        "OpenAI",
        0,
        0.0,
        "failed",
        Some(400),
        100,
    );
    usage.candidate_id = Some("cand-used".to_string());
    usage.response_headers = Some(json!({
        "content-type": "application/json",
        "x-request-id": "req_usage-cyber-risk-demo"
    }));
    usage.response_body = Some(json!({
        "error": {
            "type": "invalid_request",
            "message": "This content was flagged for possible cybersecurity risk.",
            "code": 400
        }
    }));
    usage.response_body_state = Some(UsageBodyCaptureState::Reference);
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed_with_detached_bodies(
        vec![usage],
    ));
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
        "/api/admin/monitoring/trace/request-ref-body",
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
    let upstream_response = &payload["candidates"][0]["extra_data"]["upstream_response"];
    assert_eq!(
        upstream_response["headers"],
        json!({
            "content-type": "application/json",
            "x-request-id": "req_usage-cyber-risk-demo"
        })
    );
    assert_eq!(
        upstream_response["body"]["error"],
        json!({
            "type": "invalid_request",
            "message": "This content was flagged for possible cybersecurity risk.",
            "code": 400
        })
    );
    assert!(upstream_response["body"].get("input").is_none());
    assert_eq!(
        upstream_response["body_ref"],
        json!("usage://request/request-ref-body/response_body")
    );
}

#[tokio::test]
async fn admin_monitoring_trace_request_decodes_connect_json_response_body_refs() {
    let mut candidate = sample_candidate(
        "cand-used",
        "request-connect",
        0,
        RequestCandidateStatus::Failed,
        Some(101),
        Some(33),
        Some(429),
    );
    candidate.extra_data = Some(json!({"cache_1h": true}));

    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![candidate]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let mut usage = sample_usage(
        "request-connect",
        "provider-1",
        "Windsurf",
        0,
        0.0,
        "failed",
        Some(429),
        100,
    );
    usage.candidate_id = Some("cand-used".to_string());
    usage.response_headers = Some(json!({
        "content-type": "application/connect+json"
    }));
    let mut framed = Vec::new();
    framed.push(2);
    let payload = br#"{"error":{"code":"resource_exhausted","message":"quota exhausted"}}"#;
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(payload);
    usage.response_body = Some(json!(BASE64_STANDARD.encode(framed)));
    usage.response_body_ref = Some("usage://request/request-connect/response_body".to_string());
    usage.response_body_state = Some(UsageBodyCaptureState::Inline);
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
        "/api/admin/monitoring/trace/request-connect",
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
    let upstream_response = &payload["candidates"][0]["extra_data"]["upstream_response"];
    assert_eq!(upstream_response["status_code"], json!(429));
    assert_eq!(
        upstream_response["body"]["error"]["code"],
        json!("resource_exhausted")
    );
    assert_eq!(
        upstream_response["body"]["error"]["message"],
        json!("quota exhausted")
    );
    assert_eq!(
        upstream_response["body_ref"],
        json!("usage://request/request-connect/response_body")
    );
    assert_eq!(upstream_response["body_state"], json!("inline"));
}

#[tokio::test]
async fn admin_monitoring_trace_request_exposes_structured_ranking_metadata() {
    let mut candidate = sample_candidate(
        "cand-used",
        "request-1",
        0,
        RequestCandidateStatus::Success,
        Some(101),
        Some(33),
        Some(200),
    );
    candidate.extra_data = Some(json!({
        "ranking_mode": "CacheAffinity",
        "priority_mode": "Provider",
        "ranking_index": 0,
        "priority_slot": 7,
        "promoted_by": "cached_affinity",
        "demoted_by": "cross_format",
        "client_api_format": "openai:responses"
    }));

    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::seed(vec![candidate]));
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

    assert_eq!(
        payload["candidates"][0]["ranking"],
        json!({
            "mode": "CacheAffinity",
            "priority_mode": "Provider",
            "index": 0,
            "priority_slot": 7,
            "promoted_by": "cached_affinity",
            "demoted_by": "cross_format"
        })
    );
    assert_eq!(
        payload["candidates"][0]["extra_data"]["ranking_mode"],
        json!("CacheAffinity")
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
