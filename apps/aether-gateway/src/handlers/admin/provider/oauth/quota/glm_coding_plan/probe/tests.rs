use super::{
    handle_probe_result, usage_query_for_cycle_window, GlmCodingPlanProbeParser,
    ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::provider::oauth::quota::glm_coding_plan::RefreshOutcome;
use crate::handlers::admin::provider::oauth::quota::shared::build_quota_snapshot_payload;
use aether_contracts::{ExecutionResult, ResponseBody};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_pool_core::{
    run_pool_scheduler, PoolCandidateFacts, PoolCandidateInput, PoolRuntimeState,
    PoolSchedulingConfig, POOL_ACCOUNT_EXHAUSTED_SKIP_REASON,
};
use aether_provider_pool::{ProviderPoolMemberInput, ProviderPoolService};
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
fn http_200_non_blocking_business_error_only_records_warning() {
    // Given
    let mut outcome = empty_outcome();
    let result = response_with_json(
        200,
        json!({
            "code": 1302,
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
fn quota_business_errors_materialize_an_exhausted_scheduling_snapshot() {
    for (code, status_code, body) in [
        (
            1308,
            200,
            json!({
                "code": 1308,
                "msg": "GLM Coding Plan is unavailable",
                "success": false
            }),
        ),
        (
            1309,
            429,
            json!({
                "type": "error",
                "error": {
                    "type": "rate_limit_error",
                    "code": "1309",
                    "message": "GLM Coding Plan is unavailable"
                }
            }),
        ),
        (
            1310,
            429,
            json!({
                "type": "error",
                "error": {
                    "type": "rate_limit_error",
                    "code": "1310",
                    "message": "GLM Coding Plan is unavailable"
                }
            }),
        ),
        (
            1313,
            429,
            json!({
                "type": "error",
                "error": {
                    "type": "rate_limit_error",
                    "code": "1313",
                    "message": "GLM Coding Plan is unavailable"
                }
            }),
        ),
    ] {
        // Given a GLM quota probe blocked by an upstream quota or plan limit.
        let mut outcome = empty_outcome();
        outcome
            .metadata
            .insert("updated_at".to_string(), json!(1_234));
        let result = response_with_json(status_code, body);

        // When the probe result is converted into the persisted quota contract.
        handle_probe_result(
            "quota_limit",
            GlmCodingPlanProbeParser::QuotaLimit,
            ProviderQuotaExecutionOutcome::Response(result),
            1_234,
            &mut outcome,
        );
        let metadata_update = json!({ "glm_coding_plan": outcome.metadata });
        let quota_snapshot =
            build_quota_snapshot_payload("glm_coding_plan", None, Some(&metadata_update))
                .expect("quota business error should materialize a snapshot");
        let mut key = StoredProviderCatalogKey::new(
            "key-glm".to_string(),
            "provider-glm".to_string(),
            "GLM key".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.status_snapshot = Some(json!({ "quota": quota_snapshot }));
        let key_context = ProviderPoolService::with_builtin_adapters()
            .adapter("glm_coding_plan")
            .member_signals(&ProviderPoolMemberInput {
                provider_type: "glm_coding_plan",
                key: &key,
                auth_config: None,
            });

        // Then a pool configured to skip exhaustion excludes the key without poisoning it.
        let scheduled = run_pool_scheduler(
            vec![PoolCandidateInput {
                candidate: key.id.clone(),
                facts: PoolCandidateFacts {
                    provider_id: key.provider_id.clone(),
                    endpoint_id: "endpoint-glm".to_string(),
                    model_id: "model-glm".to_string(),
                    selected_provider_model_name: "glm-4.5".to_string(),
                    provider_api_format: "openai:chat".to_string(),
                    key_id: key.id.clone(),
                    key_internal_priority: 0,
                },
                pool_config: Some(PoolSchedulingConfig {
                    scheduling_presets: Vec::new(),
                    lru_enabled: true,
                    skip_exhausted_accounts: true,
                    cost_limit_per_key_tokens: None,
                }),
                key_context,
            }],
            &BTreeMap::<String, PoolRuntimeState>::new(),
            "glm-quota-test",
        );
        assert!(scheduled.candidates.is_empty());
        assert_eq!(scheduled.skipped_candidates.len(), 1);
        assert_eq!(
            scheduled.skipped_candidates[0].skip_reason, POOL_ACCOUNT_EXHAUSTED_SKIP_REASON,
            "business code {code} should block configured pool scheduling"
        );
        assert_eq!(outcome.oauth_invalid_at_unix_secs, None);
        assert_eq!(outcome.oauth_invalid_reason, None);
    }
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
