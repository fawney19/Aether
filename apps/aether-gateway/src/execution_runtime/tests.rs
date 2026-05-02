use aether_contracts::{ExecutionPlan, RequestBody};
use axum::http::Request;
use base64::Engine as _;
use serde_json::json;

use crate::ai_serving::api::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_openai_responses_stream_plan_from_decision,
    build_openai_responses_sync_plan_from_decision, build_passthrough_sync_plan_from_decision,
    build_standard_stream_plan_from_decision, build_standard_sync_plan_from_decision,
    AiExecutionDecision,
};
use crate::execution_runtime::submission::{
    build_best_effort_local_core_error_body, has_nested_error,
    resolve_core_error_background_report_kind, resolve_core_success_background_report_kind,
    resolve_local_core_error_response_body_json,
};
use crate::execution_runtime::{
    resolve_local_sync_error_background_report_kind,
    resolve_local_sync_success_background_report_kind,
};
use crate::executor::{
    should_bypass_execution_runtime_decision, should_bypass_execution_runtime_plan,
};
use crate::usage::GatewaySyncReportRequest;

fn test_parts() -> http::request::Parts {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .body(())
        .expect("request");
    let (parts, _) = request.into_parts();
    parts
}

fn missing_exact_provider_request_payload(decision_kind: &str) -> AiExecutionDecision {
    AiExecutionDecision {
        action: "execution_runtime".to_string(),
        decision_kind: Some(decision_kind.to_string()),
        execution_strategy: Some("local_same_format".to_string()),
        conversion_mode: Some("none".to_string()),
        request_id: Some("req_123".to_string()),
        candidate_id: Some("cand_123".to_string()),
        provider_name: Some("provider".to_string()),
        provider_id: Some("provider_id".to_string()),
        endpoint_id: Some("endpoint_id".to_string()),
        key_id: Some("key_id".to_string()),
        upstream_base_url: Some("https://example.com".to_string()),
        upstream_url: Some("https://example.com/v1/messages".to_string()),
        provider_request_method: None,
        auth_header: Some("authorization".to_string()),
        auth_value: Some("Bearer test".to_string()),
        provider_api_format: Some("anthropic".to_string()),
        client_api_format: Some("openai".to_string()),
        provider_contract: Some("anthropic".to_string()),
        client_contract: Some("openai".to_string()),
        model_name: Some("model".to_string()),
        mapped_model: Some("model".to_string()),
        prompt_cache_key: None,
        extra_headers: Default::default(),
        provider_request_headers: Default::default(),
        provider_request_body: None,
        provider_request_body_base64: None,
        content_type: Some("application/json".to_string()),
        proxy: None,
        tls_profile: None,
        timeouts: None,
        upstream_is_stream: false,
        report_kind: Some("openai_chat_sync_success".to_string()),
        report_context: Some(json!({})),
        auth_context: None,
    }
}

fn core_finalize_payload(
    report_kind: &str,
    client_api_format: &str,
    provider_api_format: &str,
    status_code: u16,
    body_json: serde_json::Value,
) -> GatewaySyncReportRequest {
    GatewaySyncReportRequest {
        trace_id: "trace-core-error-123".to_string(),
        report_kind: report_kind.to_string(),
        report_context: Some(json!({
            "client_api_format": client_api_format,
            "provider_api_format": provider_api_format,
        })),
        status_code,
        headers: Default::default(),
        body_json: Some(body_json),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    }
}

#[test]
fn resolve_core_error_background_report_kind_maps_all_core_finalize_kinds() {
    let cases = [
        ("openai_chat_sync_finalize", Some("openai_chat_sync_error")),
        ("claude_chat_sync_finalize", Some("claude_chat_sync_error")),
        ("gemini_chat_sync_finalize", Some("gemini_chat_sync_error")),
        (
            "openai_responses_sync_finalize",
            Some("openai_responses_sync_error"),
        ),
        (
            "openai_responses_compact_sync_finalize",
            Some("openai_responses_compact_sync_error"),
        ),
        (
            "openai_cli_sync_finalize",
            Some("openai_responses_sync_error"),
        ),
        (
            "openai_compact_sync_finalize",
            Some("openai_responses_compact_sync_error"),
        ),
        ("claude_cli_sync_finalize", Some("claude_cli_sync_error")),
        ("gemini_cli_sync_finalize", Some("gemini_cli_sync_error")),
        ("openai_video_create_sync_finalize", None),
        ("gemini_video_cancel_sync_finalize", None),
        ("unknown_finalize_kind", None),
    ];

    for (report_kind, expected) in cases {
        assert_eq!(
            resolve_core_error_background_report_kind(report_kind),
            expected.map(str::to_string),
            "unexpected mapping for {report_kind}"
        );
    }
}

#[test]
fn resolve_core_success_background_report_kind_maps_all_core_finalize_kinds() {
    let cases = [
        (
            "openai_chat_sync_finalize",
            Some("openai_chat_sync_success"),
        ),
        (
            "claude_chat_sync_finalize",
            Some("claude_chat_sync_success"),
        ),
        (
            "gemini_chat_sync_finalize",
            Some("gemini_chat_sync_success"),
        ),
        (
            "openai_responses_sync_finalize",
            Some("openai_responses_sync_success"),
        ),
        (
            "openai_responses_compact_sync_finalize",
            Some("openai_responses_compact_sync_success"),
        ),
        (
            "openai_cli_sync_finalize",
            Some("openai_responses_sync_success"),
        ),
        (
            "openai_compact_sync_finalize",
            Some("openai_responses_compact_sync_success"),
        ),
        ("claude_cli_sync_finalize", Some("claude_cli_sync_success")),
        ("gemini_cli_sync_finalize", Some("gemini_cli_sync_success")),
        ("openai_video_create_sync_finalize", None),
        ("gemini_video_cancel_sync_finalize", None),
        ("unknown_finalize_kind", None),
    ];

    for (report_kind, expected) in cases {
        assert_eq!(
            resolve_core_success_background_report_kind(report_kind),
            expected.map(str::to_string),
            "unexpected mapping for {report_kind}"
        );
    }
}

#[test]
fn build_best_effort_local_core_error_body_converts_gemini_chat_error_to_openai_chat() {
    let payload = core_finalize_payload(
        "openai_chat_sync_finalize",
        "openai:chat",
        "gemini:generate_content",
        429,
        json!({
            "error": {
                "message": "rate limited",
                "status": "RESOURCE_EXHAUSTED",
                "code": 429
            }
        }),
    );

    let converted = build_best_effort_local_core_error_body(
        &payload,
        payload.body_json.as_ref().expect("body_json should exist"),
    )
    .expect("conversion should not error")
    .expect("conversion should produce a client error body");

    assert_eq!(
        converted,
        json!({
            "error": {
                "message": "rate limited",
                "type": "rate_limit_error",
                "code": "429"
            }
        })
    );
}

#[test]
fn build_best_effort_local_core_error_body_converts_claude_cli_error_to_openai_responses() {
    let payload = core_finalize_payload(
        "openai_responses_sync_finalize",
        "openai:responses",
        "claude:messages",
        401,
        json!({
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "invalid auth token"
            }
        }),
    );

    let converted = build_best_effort_local_core_error_body(
        &payload,
        payload.body_json.as_ref().expect("body_json should exist"),
    )
    .expect("conversion should not error")
    .expect("conversion should produce a client error body");

    assert_eq!(
        converted,
        json!({
            "error": {
                "message": "invalid auth token",
                "type": "authentication_error"
            }
        })
    );
}

#[test]
fn has_nested_error_ignores_null_error_fields() {
    assert!(!has_nested_error(&json!({
        "id": "resp_completed_123",
        "object": "response",
        "status": "completed",
        "error": null,
        "output": []
    })));
    assert!(has_nested_error(&json!({
        "id": "resp_failed_123",
        "object": "response",
        "status": "failed",
        "error": {"message": "quota reached"}
    })));
}

#[test]
fn build_best_effort_local_core_error_body_converts_sync_errors_across_standard_families() {
    let cases = vec![
        (
            "claude chat -> openai chat",
            core_finalize_payload(
                "openai_chat_sync_finalize",
                "openai:chat",
                "claude:messages",
                429,
                json!({
                    "type": "error",
                    "error": {
                        "type": "rate_limit_error",
                        "message": "slow down"
                    }
                }),
            ),
            json!({
                "error": {
                    "message": "slow down",
                    "type": "rate_limit_error"
                }
            }),
        ),
        (
            "openai chat -> claude chat",
            core_finalize_payload(
                "claude_chat_sync_finalize",
                "claude:messages",
                "openai:chat",
                404,
                json!({
                    "error": {
                        "message": "missing model",
                        "type": "not_found_error",
                        "code": "model_missing"
                    }
                }),
            ),
            json!({
                "type": "error",
                "error": {
                    "message": "missing model",
                    "type": "not_found_error",
                    "code": "model_missing"
                }
            }),
        ),
        (
            "openai chat -> gemini chat",
            core_finalize_payload(
                "gemini_chat_sync_finalize",
                "gemini:generate_content",
                "openai:chat",
                401,
                json!({
                    "error": {
                        "message": "bad auth",
                        "type": "authentication_error"
                    }
                }),
            ),
            json!({
                "error": {
                    "code": 401,
                    "message": "bad auth",
                    "status": "UNAUTHENTICATED"
                }
            }),
        ),
        (
            "gemini cli -> openai responses",
            core_finalize_payload(
                "openai_responses_sync_finalize",
                "openai:responses",
                "gemini:generate_content",
                429,
                json!({
                    "error": {
                        "message": "quota hit",
                        "status": "RESOURCE_EXHAUSTED",
                        "code": 429
                    }
                }),
            ),
            json!({
                "error": {
                    "message": "quota hit",
                    "type": "rate_limit_error",
                    "code": "429"
                }
            }),
        ),
        (
            "gemini cli -> claude cli",
            core_finalize_payload(
                "claude_cli_sync_finalize",
                "claude:messages",
                "gemini:generate_content",
                503,
                json!({
                    "error": {
                        "message": "backend busy",
                        "status": "UNAVAILABLE"
                    }
                }),
            ),
            json!({
                "type": "error",
                "error": {
                    "message": "backend busy",
                    "type": "api_error",
                    "code": "UNAVAILABLE"
                }
            }),
        ),
        (
            "claude cli -> gemini cli",
            core_finalize_payload(
                "gemini_cli_sync_finalize",
                "gemini:generate_content",
                "claude:messages",
                404,
                json!({
                    "type": "error",
                    "error": {
                        "type": "not_found_error",
                        "message": "resource missing"
                    }
                }),
            ),
            json!({
                "error": {
                    "code": 404,
                    "message": "resource missing",
                    "status": "NOT_FOUND"
                }
            }),
        ),
    ];

    for (label, payload, expected) in cases {
        let converted = build_best_effort_local_core_error_body(
            &payload,
            payload.body_json.as_ref().expect("body_json should exist"),
        )
        .expect("conversion should not error")
        .expect("conversion should produce a client error body");

        assert_eq!(converted, expected, "unexpected conversion for {label}");
    }
}

#[test]
fn resolve_local_core_error_response_body_json_parses_body_base64_json_for_cross_format_cli_error()
{
    let mut payload = core_finalize_payload(
        "claude_cli_sync_finalize",
        "claude:messages",
        "openai:responses",
        401,
        json!({}),
    );
    payload.body_json = None;
    payload.body_base64 = Some(
        base64::engine::general_purpose::STANDARD
            .encode(r#"{"error":{"message":"invalid auth token","type":"authentication_error"}}"#),
    );

    let resolved = resolve_local_core_error_response_body_json(&payload)
        .expect("resolution should not error")
        .expect("resolution should produce a client error body");

    assert_eq!(
        resolved,
        json!({
            "type": "error",
            "error": {
                "message": "invalid auth token",
                "type": "authentication_error"
            }
        })
    );
}

#[test]
fn resolve_local_core_error_response_body_json_builds_client_error_from_plain_text_body() {
    let mut payload = core_finalize_payload(
        "claude_cli_sync_finalize",
        "claude:messages",
        "openai:responses",
        400,
        json!({}),
    );
    payload.body_json = None;
    payload.body_base64 =
        Some(base64::engine::general_purpose::STANDARD.encode("invalid model for this endpoint"));

    let resolved = resolve_local_core_error_response_body_json(&payload)
        .expect("resolution should not error")
        .expect("resolution should produce a client error body");

    assert_eq!(
        resolved,
        json!({
            "type": "error",
            "error": {
                "message": "invalid model for this endpoint",
                "type": "invalid_request_error"
            }
        })
    );
}

#[test]
fn resolve_local_sync_success_background_report_kind_maps_video_finalize_kinds() {
    let cases = [
        (
            "openai_video_delete_sync_finalize",
            Some("openai_video_delete_sync_success"),
        ),
        (
            "openai_video_cancel_sync_finalize",
            Some("openai_video_cancel_sync_success"),
        ),
        (
            "gemini_video_cancel_sync_finalize",
            Some("gemini_video_cancel_sync_success"),
        ),
        ("openai_video_create_sync_finalize", None),
        ("unknown_finalize_kind", None),
    ];

    for (report_kind, expected) in cases {
        assert_eq!(
            resolve_local_sync_success_background_report_kind(report_kind),
            expected,
            "unexpected mapping for {report_kind}"
        );
    }
}

#[test]
fn resolve_local_sync_error_background_report_kind_maps_video_finalize_kinds() {
    let cases = [
        (
            "openai_video_create_sync_finalize",
            Some("openai_video_create_sync_error"),
        ),
        (
            "openai_video_remix_sync_finalize",
            Some("openai_video_remix_sync_error"),
        ),
        (
            "gemini_video_create_sync_finalize",
            Some("gemini_video_create_sync_error"),
        ),
        (
            "openai_video_delete_sync_finalize",
            Some("openai_video_delete_sync_error"),
        ),
        (
            "openai_video_cancel_sync_finalize",
            Some("openai_video_cancel_sync_error"),
        ),
        (
            "gemini_video_cancel_sync_finalize",
            Some("gemini_video_cancel_sync_error"),
        ),
        ("unknown_finalize_kind", None),
    ];

    for (report_kind, expected) in cases {
        assert_eq!(
            resolve_local_sync_error_background_report_kind(report_kind),
            expected,
            "unexpected mapping for {report_kind}"
        );
    }
}

#[test]
fn generic_decision_builders_require_exact_provider_request() {
    let parts = test_parts();
    let body_json = json!({"messages":[{"role":"user","content":"hi"}]});

    assert!(build_openai_responses_stream_plan_from_decision(
        &parts,
        &body_json,
        missing_exact_provider_request_payload("openai_responses_stream"),
        false,
    )
    .expect("builder should not error")
    .is_none());
    assert!(build_openai_responses_stream_plan_from_decision(
        &parts,
        &body_json,
        missing_exact_provider_request_payload("openai_responses_compact_stream"),
        true,
    )
    .expect("builder should not error")
    .is_none());
    assert!(build_standard_stream_plan_from_decision(
        &parts,
        &body_json,
        missing_exact_provider_request_payload("claude_chat_stream"),
        false,
    )
    .expect("builder should not error")
    .is_none());
    assert!(build_gemini_stream_plan_from_decision(
        &parts,
        &body_json,
        missing_exact_provider_request_payload("gemini_chat_stream"),
    )
    .expect("builder should not error")
    .is_none());
    assert!(build_openai_responses_sync_plan_from_decision(
        &parts,
        &body_json,
        missing_exact_provider_request_payload("openai_responses_sync"),
        false,
    )
    .expect("builder should not error")
    .is_none());
    assert!(build_standard_sync_plan_from_decision(
        &parts,
        &body_json,
        missing_exact_provider_request_payload("claude_chat_sync"),
    )
    .expect("builder should not error")
    .is_none());
    assert!(build_gemini_sync_plan_from_decision(
        &parts,
        &body_json,
        missing_exact_provider_request_payload("gemini_chat_sync"),
    )
    .expect("builder should not error")
    .is_none());
}

#[test]
fn passthrough_sync_plan_uses_provider_request_method_override() {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/videos/task-123/cancel")
        .body(())
        .expect("request");
    let (parts, _) = request.into_parts();

    let mut payload = missing_exact_provider_request_payload("openai_video_cancel_sync");
    payload.provider_name = Some("openai".to_string());
    payload.provider_api_format = Some("openai:video".to_string());
    payload.client_api_format = Some("openai:video".to_string());
    payload.model_name = Some("sora-2".to_string());
    payload.upstream_url = Some("https://api.openai.example/v1/videos/ext-123".to_string());
    payload.provider_request_method = Some("DELETE".to_string());
    payload.provider_request_headers = [(
        "authorization".to_string(),
        "Bearer upstream-key".to_string(),
    )]
    .into_iter()
    .collect();

    let plan_and_report = build_passthrough_sync_plan_from_decision(&parts, payload)
        .expect("builder should not error")
        .expect("plan should be built");

    assert_eq!(plan_and_report.plan.method, "DELETE");
    assert_eq!(
        plan_and_report.plan.url,
        "https://api.openai.example/v1/videos/ext-123"
    );
}

#[test]
fn openai_responses_sync_plan_injects_auth_header_when_exact_headers_omit_it() {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .body(())
        .expect("request");
    let (parts, _) = request.into_parts();

    let mut payload = missing_exact_provider_request_payload("openai_responses_sync");
    payload.provider_name = Some("openai".to_string());
    payload.provider_api_format = Some("openai:responses".to_string());
    payload.client_api_format = Some("openai:responses".to_string());
    payload.model_name = Some("gpt-5".to_string());
    payload.upstream_url = Some("https://chatgpt.com/backend-api/codex/responses".to_string());
    payload.provider_request_headers =
        [("content-type".to_string(), "application/json".to_string())]
            .into_iter()
            .collect();
    payload.provider_request_body = Some(json!({"model":"gpt-5"}));

    let plan_and_report =
        build_openai_responses_sync_plan_from_decision(&parts, &json!({}), payload, false)
            .expect("builder should not error")
            .expect("plan should be built");

    assert_eq!(
        plan_and_report
            .plan
            .headers
            .get("authorization")
            .map(String::as_str),
        Some("Bearer test")
    );
}

#[test]
fn passthrough_sync_plan_uses_raw_body_bytes_when_decision_provides_base64_body() {
    let request = Request::builder()
        .method("POST")
        .uri("/upload/v1beta/files?uploadType=resumable")
        .body(())
        .expect("request");
    let (parts, _) = request.into_parts();

    let mut payload = missing_exact_provider_request_payload("gemini_files_upload");
    payload.provider_name = Some("gemini".to_string());
    payload.provider_api_format = Some("gemini:files".to_string());
    payload.client_api_format = Some("gemini:files".to_string());
    payload.provider_request_method = Some("POST".to_string());
    payload.upstream_url = Some(
        "https://generativelanguage.googleapis.com/upload/v1beta/files?uploadType=resumable"
            .to_string(),
    );
    payload.provider_request_headers = [
        (
            "content-type".to_string(),
            "application/octet-stream".to_string(),
        ),
        ("x-goog-api-key".to_string(), "upstream-key".to_string()),
    ]
    .into_iter()
    .collect();
    payload.provider_request_body_base64 = Some("dXBsb2FkLWJ5dGVz".to_string());

    let plan_and_report = build_passthrough_sync_plan_from_decision(&parts, payload)
        .expect("builder should not error")
        .expect("plan should be built");

    assert_eq!(plan_and_report.plan.method, "POST");
    assert_eq!(
        plan_and_report.plan.url,
        "https://generativelanguage.googleapis.com/upload/v1beta/files?uploadType=resumable"
    );
    assert_eq!(
        plan_and_report.plan.body.body_bytes_b64.as_deref(),
        Some("dXBsb2FkLWJ5dGVz")
    );
    assert_eq!(plan_and_report.plan.body.json_body, None);
    assert_eq!(
        plan_and_report
            .plan
            .headers
            .get("content-type")
            .map(String::as_str),
        Some("application/octet-stream")
    );
}

#[test]
fn openai_responses_stream_plan_injects_auth_header_when_exact_headers_omit_it() {
    let request = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .body(())
        .expect("request");
    let (parts, _) = request.into_parts();

    let mut payload = missing_exact_provider_request_payload("openai_responses_stream");
    payload.provider_name = Some("openai".to_string());
    payload.provider_api_format = Some("openai:responses".to_string());
    payload.client_api_format = Some("openai:responses".to_string());
    payload.model_name = Some("gpt-5".to_string());
    payload.upstream_url = Some("https://chatgpt.com/backend-api/codex/responses".to_string());
    payload.provider_request_headers =
        [("content-type".to_string(), "application/json".to_string())]
            .into_iter()
            .collect();
    payload.provider_request_body = Some(json!({"model":"gpt-5","stream":true}));

    let plan_and_report =
        build_openai_responses_stream_plan_from_decision(&parts, &json!({}), payload, false)
            .expect("builder should not error")
            .expect("plan should be built");

    assert_eq!(
        plan_and_report
            .plan
            .headers
            .get("authorization")
            .map(String::as_str),
        Some("Bearer test")
    );
    assert_eq!(
        plan_and_report
            .plan
            .headers
            .get("accept")
            .map(String::as_str),
        Some("text/event-stream")
    );
}

#[test]
fn bypasses_execution_runtime_for_codex_backendapi_variant() {
    let mut payload = missing_exact_provider_request_payload("openai_responses_stream");
    payload.provider_api_format = Some("openai:responses".to_string());
    payload.client_api_format = Some("openai:responses".to_string());
    payload.upstream_url = Some("https://chatgpt.com/backendapi/codex/responses".to_string());

    assert!(should_bypass_execution_runtime_decision(&payload));
}

#[test]
fn bypasses_execution_runtime_for_codex_plan_variant() {
    let plan = ExecutionPlan {
        request_id: "req-123".to_string(),
        candidate_id: None,
        provider_name: Some("codex".to_string()),
        provider_id: "provider-123".to_string(),
        endpoint_id: "endpoint-123".to_string(),
        key_id: "key-123".to_string(),
        method: "POST".to_string(),
        url: "https://chatgpt.com/backendapi/codex/responses".to_string(),
        headers: Default::default(),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(json!({"model":"gpt-5.4"})),
        stream: true,
        client_api_format: "openai:responses".to_string(),
        provider_api_format: "openai:responses".to_string(),
        model_name: Some("gpt-5.4".to_string()),
        proxy: None,
        tls_profile: None,
        timeouts: None,
    };

    assert!(should_bypass_execution_runtime_plan(&plan));
}
