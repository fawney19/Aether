use super::{
    handle_probe_result, usage_query_for_cycle_window, GlmCodingPlanProbeParser,
    ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::provider::oauth::quota::glm_coding_plan::RefreshOutcome;
use aether_contracts::{ExecutionResult, ResponseBody};
use serde_json::json;
use std::collections::BTreeMap;

fn response_with_json(status_code: u16, body: serde_json::Value) -> ExecutionResult {
    ExecutionResult {
        request_id: "glm-coding-plan:test".to_string(),
        candidate_id: None,
        status_code,
        headers: BTreeMap::new(),
        body: Some(ResponseBody {
            json_body: Some(body),
            body_bytes_b64: None,
        }),
        telemetry: None,
        error: None,
    }
}

fn empty_outcome() -> RefreshOutcome {
    RefreshOutcome {
        metadata: serde_json::Map::new(),
        parsed_probe_count: 0,
        oauth_invalid_at_unix_secs: None,
        oauth_invalid_reason: None,
    }
}

#[test]
fn builds_usage_query_from_provider_cycle_in_beijing_time() {
    let query =
        usage_query_for_cycle_window(Some(1_782_372_879), 5 * 60 * 60, 1_782_355_388, false)
            .expect("query should build");

    assert_eq!(
        query,
        "startTime=2026-06-25+10%3A34%3A39&endTime=2026-06-25+10%3A43%3A08"
    );
}

#[test]
fn builds_rolling_usage_query_only_when_fallback_is_allowed() {
    assert!(usage_query_for_cycle_window(None, 5 * 60 * 60, 1_782_355_388, false).is_none());

    let query = usage_query_for_cycle_window(None, 5 * 60 * 60, 1_782_355_388, true)
        .expect("fallback query should build");

    assert_eq!(
        query,
        "startTime=2026-06-25+05%3A43%3A08&endTime=2026-06-25+10%3A43%3A08"
    );
}

#[test]
fn http_200_non_auth_business_error_only_records_warning() {
    // Given
    let mut outcome = empty_outcome();
    let result = response_with_json(
        200,
        json!({
            "code": 1308,
            "msg": "rate limit reached",
            "success": false
        }),
    );

    // When
    handle_probe_result(
        "quota_limit",
        GlmCodingPlanProbeParser::QuotaLimit,
        ProviderQuotaExecutionOutcome::Response(result),
        1_234,
        &mut outcome,
    );

    // Then
    assert_eq!(outcome.oauth_invalid_at_unix_secs, None);
    assert_eq!(outcome.oauth_invalid_reason, None);
    assert_eq!(
        outcome.metadata["probe_warnings"][0]["probe"],
        "quota_limit"
    );
}

#[test]
fn http_200_code_401_marks_candidate_invalid() {
    // Given
    let mut outcome = empty_outcome();
    let result = response_with_json(
        200,
        json!({
            "code": "401",
            "msg": "token expired or incorrect",
            "success": false
        }),
    );

    // When
    handle_probe_result(
        "quota_limit",
        GlmCodingPlanProbeParser::QuotaLimit,
        ProviderQuotaExecutionOutcome::Response(result),
        1_234,
        &mut outcome,
    );

    // Then
    assert_eq!(outcome.oauth_invalid_at_unix_secs, Some(1_234));
    assert_eq!(
        outcome.oauth_invalid_reason.as_deref(),
        Some("token expired or incorrect")
    );
}

#[test]
fn bare_http_401_marks_candidate_invalid() {
    // Given
    let mut outcome = empty_outcome();
    let result = response_with_json(401, json!({ "message": "unauthorized" }));

    // When
    handle_probe_result(
        "quota_limit",
        GlmCodingPlanProbeParser::QuotaLimit,
        ProviderQuotaExecutionOutcome::Response(result),
        1_234,
        &mut outcome,
    );

    // Then
    assert_eq!(outcome.oauth_invalid_at_unix_secs, Some(1_234));
}

#[test]
fn bare_http_403_does_not_mark_candidate_invalid() {
    // Given
    let mut outcome = empty_outcome();
    let result = response_with_json(
        403,
        json!({
            "code": 1220,
            "msg": "API permission denied",
            "success": false
        }),
    );

    // When
    handle_probe_result(
        "quota_limit",
        GlmCodingPlanProbeParser::QuotaLimit,
        ProviderQuotaExecutionOutcome::Response(result),
        1_234,
        &mut outcome,
    );

    // Then
    assert_eq!(outcome.oauth_invalid_at_unix_secs, None);
    assert_eq!(outcome.oauth_invalid_reason, None);
    assert_eq!(
        outcome.metadata["probe_warnings"][0]["probe"],
        "quota_limit"
    );
}
