use aether_contracts::{ExecutionPlan, ExecutionResult};
use serde_json::Value;

use crate::orchestration::{
    resolve_local_failover_analysis_for_attempt, LocalFailoverAnalysis, LocalFailoverDecision,
};
use crate::AppState;

fn sync_plan_kind_disables_local_candidate_failover(plan_kind: &str) -> bool {
    matches!(
        plan_kind,
        "openai_video_delete_sync" | "openai_video_cancel_sync" | "gemini_video_cancel_sync"
    )
}

pub(crate) async fn should_retry_next_local_candidate_sync(
    state: &AppState,
    plan: &ExecutionPlan,
    plan_kind: &str,
    report_context: Option<&serde_json::Value>,
    result: &ExecutionResult,
    response_text: Option<&str>,
) -> bool {
    matches!(
        analyze_local_candidate_failover_sync(
            state,
            plan,
            plan_kind,
            report_context,
            result,
            response_text,
        )
        .await
        .decision,
        LocalFailoverDecision::RetryNextCandidate
    )
}

pub(crate) async fn analyze_local_candidate_failover_sync(
    state: &AppState,
    plan: &ExecutionPlan,
    plan_kind: &str,
    report_context: Option<&serde_json::Value>,
    result: &ExecutionResult,
    response_text: Option<&str>,
) -> LocalFailoverAnalysis {
    if sync_plan_kind_disables_local_candidate_failover(plan_kind) {
        return LocalFailoverAnalysis::use_default();
    }

    resolve_local_failover_analysis_for_attempt(
        state,
        plan,
        report_context,
        result.status_code,
        response_text,
    )
    .await
}

pub(crate) async fn should_stop_local_candidate_failover_sync(
    state: &AppState,
    plan: &ExecutionPlan,
    plan_kind: &str,
    report_context: Option<&serde_json::Value>,
    result: &ExecutionResult,
    response_text: Option<&str>,
) -> bool {
    matches!(
        analyze_local_candidate_failover_sync(
            state,
            plan,
            plan_kind,
            report_context,
            result,
            response_text,
        )
        .await,
        LocalFailoverAnalysis {
            decision: LocalFailoverDecision::StopLocalFailover,
            ..
        }
    )
}

pub(crate) fn should_fallback_to_control_sync(
    plan_kind: &str,
    result: &ExecutionResult,
    body_json: Option<&serde_json::Value>,
    has_body_bytes: bool,
    explicit_finalize: bool,
    mapped_error_finalize: bool,
) -> bool {
    if explicit_finalize
        && matches!(
            plan_kind,
            "openai_video_delete_sync" | "openai_video_cancel_sync" | "gemini_video_cancel_sync"
        )
    {
        return false;
    }

    if !matches!(
        plan_kind,
        "openai_video_create_sync"
            | "openai_video_remix_sync"
            | "gemini_video_create_sync"
            | "openai_chat_sync"
            | "openai_responses_sync"
            | "openai_responses_compact_sync"
            | "claude_chat_sync"
            | "gemini_chat_sync"
            | "claude_cli_sync"
            | "gemini_cli_sync"
    ) {
        return false;
    }

    if explicit_finalize {
        return result.status_code < 400 && body_json.is_none() && !has_body_bytes;
    }

    if mapped_error_finalize {
        return false;
    }

    if result.status_code >= 400 {
        return true;
    }

    let Some(body_json) = body_json else {
        return true;
    };

    body_json.get("error").is_some()
}

pub(crate) fn should_finalize_sync_response(report_kind: Option<&str>) -> bool {
    report_kind.is_some_and(|kind| kind.ends_with("_finalize"))
}

pub(crate) fn resolve_core_sync_error_finalize_report_kind(
    plan_kind: &str,
    result: &ExecutionResult,
    body_json: Option<&serde_json::Value>,
) -> Option<String> {
    let has_embedded_error = body_json.is_some_and(|value| value.get("error").is_some());
    if result.status_code < 400 && !has_embedded_error {
        return None;
    }

    let report_kind = match plan_kind {
        "openai_chat_sync" => "openai_chat_sync_finalize",
        "openai_responses_sync" => "openai_responses_sync_finalize",
        "openai_responses_compact_sync" => "openai_responses_compact_sync_finalize",
        "claude_chat_sync" => "claude_chat_sync_finalize",
        "gemini_chat_sync" => "gemini_chat_sync_finalize",
        "claude_cli_sync" => "claude_cli_sync_finalize",
        "gemini_cli_sync" => "gemini_cli_sync_finalize",
        _ => return None,
    };

    Some(report_kind.to_string())
}

pub(crate) async fn should_retry_next_local_candidate_stream(
    state: &AppState,
    plan: &ExecutionPlan,
    _plan_kind: &str,
    report_context: Option<&serde_json::Value>,
    status_code: u16,
    response_text: Option<&str>,
) -> bool {
    matches!(
        resolve_local_candidate_failover_analysis_stream(
            state,
            plan,
            report_context,
            status_code,
            response_text,
        )
        .await,
        LocalFailoverAnalysis {
            decision: LocalFailoverDecision::RetryNextCandidate,
            ..
        }
    )
}

pub(crate) async fn should_stop_local_candidate_failover_stream(
    state: &AppState,
    plan: &ExecutionPlan,
    _plan_kind: &str,
    report_context: Option<&serde_json::Value>,
    status_code: u16,
    response_text: Option<&str>,
) -> bool {
    matches!(
        resolve_local_candidate_failover_analysis_stream(
            state,
            plan,
            report_context,
            status_code,
            response_text,
        )
        .await,
        LocalFailoverAnalysis {
            decision: LocalFailoverDecision::StopLocalFailover,
            ..
        }
    )
}

pub(crate) async fn resolve_local_candidate_failover_analysis_stream(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    status_code: u16,
    response_text: Option<&str>,
) -> LocalFailoverAnalysis {
    resolve_local_failover_analysis_for_attempt(
        state,
        plan,
        report_context,
        status_code,
        response_text,
    )
    .await
}

pub(crate) async fn resolve_local_candidate_failover_decision_stream(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    status_code: u16,
    response_text: Option<&str>,
) -> LocalFailoverDecision {
    resolve_local_candidate_failover_analysis_stream(
        state,
        plan,
        report_context,
        status_code,
        response_text,
    )
    .await
    .decision
}

pub(crate) fn local_failover_response_text(
    body_json: Option<&serde_json::Value>,
    body_bytes: &[u8],
    fallback_text: Option<&str>,
) -> Option<String> {
    if let Some(body_json) = body_json {
        return serde_json::to_string(body_json).ok();
    }
    if !body_bytes.is_empty() {
        return Some(String::from_utf8_lossy(body_bytes).into_owned());
    }
    fallback_text
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn should_fallback_to_control_stream(
    plan_kind: &str,
    status_code: u16,
    mapped_error_finalize: bool,
) -> bool {
    if mapped_error_finalize {
        return false;
    }

    matches!(
        plan_kind,
        "openai_chat_stream"
            | "claude_chat_stream"
            | "gemini_chat_stream"
            | "openai_responses_stream"
            | "openai_responses_compact_stream"
            | "claude_cli_stream"
            | "gemini_cli_stream"
    ) && status_code >= 400
}

pub(crate) fn resolve_core_stream_error_finalize_report_kind(
    plan_kind: &str,
    status_code: u16,
) -> Option<String> {
    if status_code < 400 {
        return None;
    }

    let report_kind = match plan_kind {
        "openai_chat_stream" => "openai_chat_sync_finalize",
        "claude_chat_stream" => "claude_chat_sync_finalize",
        "gemini_chat_stream" => "gemini_chat_sync_finalize",
        "openai_responses_stream" => "openai_responses_sync_finalize",
        "openai_responses_compact_stream" => "openai_responses_compact_sync_finalize",
        "claude_cli_stream" => "claude_cli_sync_finalize",
        "gemini_cli_stream" => "gemini_cli_sync_finalize",
        _ => return None,
    };

    Some(report_kind.to_string())
}

pub(crate) fn resolve_core_stream_direct_finalize_report_kind(plan_kind: &str) -> Option<String> {
    let report_kind = match plan_kind {
        "openai_chat_stream" => "openai_chat_sync_finalize",
        "openai_image_stream" => "openai_image_sync_finalize",
        "claude_chat_stream" => "claude_chat_sync_finalize",
        "gemini_chat_stream" => "gemini_chat_sync_finalize",
        "openai_responses_stream" => "openai_responses_sync_finalize",
        "openai_responses_compact_stream" => "openai_responses_compact_sync_finalize",
        "claude_cli_stream" => "claude_cli_sync_finalize",
        "gemini_cli_stream" => "gemini_cli_sync_finalize",
        _ => return None,
    };

    Some(report_kind.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use aether_contracts::ExecutionResult;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };

    use super::{
        resolve_core_stream_error_finalize_report_kind,
        resolve_core_sync_error_finalize_report_kind, should_fallback_to_control_stream,
        should_fallback_to_control_sync, should_retry_next_local_candidate_stream,
        should_retry_next_local_candidate_sync, should_stop_local_candidate_failover_stream,
        should_stop_local_candidate_failover_sync,
    };
    use crate::data::GatewayDataState;
    use crate::orchestration::{
        resolve_local_failover_policy, LocalFailoverPolicy, LocalFailoverRegexRule,
    };
    use crate::AppState;

    fn sample_plan() -> aether_contracts::ExecutionPlan {
        aether_contracts::ExecutionPlan {
            request_id: "req-1".to_string(),
            candidate_id: Some("cand-1".to_string()),
            provider_name: Some("provider-1".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: aether_contracts::RequestBody::from_json(serde_json::json!({"model":"gpt-5"})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            tls_profile: None,
            timeouts: None,
        }
    }

    fn sample_provider(config: Option<serde_json::Value>) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "provider-1".to_string(),
            Some("https://provider.example".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(true, false, false, None, Some(3), None, None, None, config)
    }

    fn sample_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.provider.example".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:chat"])),
            "plain-upstream-key".to_string(),
            None,
            None,
            Some(serde_json::json!({"openai:chat": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    fn build_state_with_provider_config(config: Option<serde_json::Value>) -> AppState {
        let provider_catalog = InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider(config)],
            vec![sample_endpoint()],
            vec![sample_key()],
        );
        let data_state = GatewayDataState::with_provider_transport_reader_for_tests(
            std::sync::Arc::new(provider_catalog),
            "development-key",
        );
        AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data_state)
    }

    #[test]
    fn sync_failover_marks_chat_errors() {
        let result = ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code: 502,
            headers: Default::default(),
            body: None,
            telemetry: None,
            error: None,
        };

        assert!(should_fallback_to_control_sync(
            "openai_chat_sync",
            &result,
            None,
            false,
            false,
            false,
        ));
        assert_eq!(
            resolve_core_sync_error_finalize_report_kind("openai_chat_sync", &result, None),
            Some("openai_chat_sync_finalize".to_string())
        );
    }

    #[test]
    fn stream_failover_marks_chat_errors() {
        assert!(should_fallback_to_control_stream(
            "openai_chat_stream",
            502,
            false,
        ));
        assert_eq!(
            resolve_core_stream_error_finalize_report_kind("openai_chat_stream", 502),
            Some("openai_chat_sync_finalize".to_string())
        );
    }

    #[tokio::test]
    async fn sync_retry_next_candidate_requires_local_candidate_context() {
        let result = ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code: 502,
            headers: Default::default(),
            body: None,
            telemetry: None,
            error: None,
        };
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(None);
        let plan = sample_plan();

        assert!(
            should_retry_next_local_candidate_sync(
                &state,
                &plan,
                "openai_chat_sync",
                Some(&local_report_context),
                &result,
                None,
            )
            .await
        );
        assert!(
            should_retry_next_local_candidate_sync(
                &state,
                &plan,
                "claude_cli_sync",
                Some(&local_report_context),
                &result,
                None,
            )
            .await
        );
        assert!(
            !should_retry_next_local_candidate_sync(
                &state,
                &plan,
                "openai_chat_sync",
                None,
                &result,
                None,
            )
            .await
        );
        assert!(
            !should_retry_next_local_candidate_sync(
                &state,
                &plan,
                "claude_chat_sync",
                None,
                &result,
                None,
            )
            .await
        );
    }

    #[tokio::test]
    async fn sync_retry_next_candidate_treats_rate_limit_as_retryable() {
        let result = ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code: 429,
            headers: Default::default(),
            body: None,
            telemetry: None,
            error: None,
        };
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(None);
        let plan = sample_plan();

        assert!(
            should_retry_next_local_candidate_sync(
                &state,
                &plan,
                "openai_chat_sync",
                Some(&local_report_context),
                &result,
                None,
            )
            .await
        );
    }

    #[tokio::test]
    async fn sync_retry_next_candidate_treats_client_error_as_failover_by_default() {
        let result = ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code: 401,
            headers: Default::default(),
            body: None,
            telemetry: None,
            error: None,
        };
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(None);
        let plan = sample_plan();

        assert!(
            should_retry_next_local_candidate_sync(
                &state,
                &plan,
                "openai_chat_sync",
                Some(&local_report_context),
                &result,
                Some("{\"error\":{\"message\":\"invalid auth token\"}}"),
            )
            .await
        );
    }

    #[tokio::test]
    async fn sync_retry_next_candidate_skips_video_follow_up_plan_kinds() {
        let result = ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code: 404,
            headers: Default::default(),
            body: None,
            telemetry: None,
            error: None,
        };
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(None);
        let plan = sample_plan();

        for plan_kind in [
            "openai_video_delete_sync",
            "openai_video_cancel_sync",
            "gemini_video_cancel_sync",
        ] {
            assert!(
                !should_retry_next_local_candidate_sync(
                    &state,
                    &plan,
                    plan_kind,
                    Some(&local_report_context),
                    &result,
                    None,
                )
                .await,
                "{plan_kind} should not retry local failover candidates"
            );
            assert!(
                !should_stop_local_candidate_failover_sync(
                    &state,
                    &plan,
                    plan_kind,
                    Some(&local_report_context),
                    &result,
                    None,
                )
                .await,
                "{plan_kind} should not use local failover stop decisions"
            );
        }
    }

    #[tokio::test]
    async fn stream_retry_next_candidate_requires_local_candidate_context() {
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(None);
        let plan = sample_plan();

        assert!(
            should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "openai_chat_stream",
                Some(&local_report_context),
                502,
                None,
            )
            .await
        );
        assert!(
            should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "gemini_cli_stream",
                Some(&local_report_context),
                502,
                None,
            )
            .await
        );
        assert!(
            !should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "openai_chat_stream",
                None,
                502,
                None,
            )
            .await
        );
        assert!(
            !should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "claude_chat_stream",
                None,
                502,
                None,
            )
            .await
        );
    }

    #[tokio::test]
    async fn stream_retry_next_candidate_treats_rate_limit_as_retryable() {
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(None);
        let plan = sample_plan();

        assert!(
            should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "openai_chat_stream",
                Some(&local_report_context),
                429,
                None,
            )
            .await
        );
    }

    #[tokio::test]
    async fn stream_retry_next_candidate_treats_client_error_as_failover_by_default() {
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(None);
        let plan = sample_plan();

        assert!(
            should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "openai_chat_stream",
                Some(&local_report_context),
                403,
                Some("{\"error\":{\"message\":\"invalid auth token\"}}"),
            )
            .await
        );
    }

    #[test]
    fn resolve_local_failover_policy_reads_provider_rules() {
        let state = build_state_with_provider_config(Some(serde_json::json!({
            "failover_rules": {
                "max_retries": 1,
                "stop_on_status_codes": [503],
                "continue_on_status_codes": [409, 429]
            }
        })));
        let plan = sample_plan();
        let runtime = tokio::runtime::Runtime::new().expect("runtime should build");

        let policy = runtime.block_on(resolve_local_failover_policy(&state, &plan, None));
        assert_eq!(
            policy,
            LocalFailoverPolicy {
                max_retries: Some(1),
                stop_status_codes: [503].into_iter().collect(),
                continue_status_codes: [409, 429].into_iter().collect(),
                success_failover_patterns: Vec::new(),
                error_stop_patterns: Vec::new(),
            }
        );
    }

    #[tokio::test]
    async fn local_failover_policy_can_stop_retryable_statuses_and_continue_non_retryable_statuses()
    {
        let state = build_state_with_provider_config(Some(serde_json::json!({
            "failover_rules": {
                "max_retries": 2,
                "stop_on_status_codes": [503],
                "continue_on_status_codes": [409]
            }
        })));
        let plan = sample_plan();
        let first_candidate = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let third_candidate = serde_json::json!({
            "candidate_index": 2,
            "retry_index": 0,
        });

        assert!(
            !should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "openai_chat_stream",
                Some(&first_candidate),
                503,
                None,
            )
            .await
        );
        assert!(
            should_stop_local_candidate_failover_stream(
                &state,
                &plan,
                "openai_chat_stream",
                Some(&first_candidate),
                503,
                None,
            )
            .await
        );
        assert!(
            should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "openai_chat_stream",
                Some(&first_candidate),
                409,
                None,
            )
            .await
        );
        assert!(
            should_retry_next_local_candidate_stream(
                &state,
                &plan,
                "openai_chat_stream",
                Some(&third_candidate),
                429,
                None,
            )
            .await
        );
    }

    #[test]
    fn resolve_local_failover_policy_reads_regex_rules() {
        let state = build_state_with_provider_config(Some(serde_json::json!({
            "failover_rules": {
                "success_failover_patterns": [
                    {"pattern": "relay:.*格式错误"}
                ],
                "error_stop_patterns": [
                    {"pattern": "content_policy_violation", "status_codes": [400, 403]}
                ]
            }
        })));
        let plan = sample_plan();
        let runtime = tokio::runtime::Runtime::new().expect("runtime should build");

        let policy = runtime.block_on(resolve_local_failover_policy(&state, &plan, None));
        assert_eq!(
            policy.success_failover_patterns,
            vec![LocalFailoverRegexRule {
                pattern: "relay:.*格式错误".to_string(),
                status_codes: BTreeSet::new(),
            }]
        );
        assert_eq!(
            policy.error_stop_patterns,
            vec![LocalFailoverRegexRule {
                pattern: "content_policy_violation".to_string(),
                status_codes: [400, 403].into_iter().collect(),
            }]
        );
    }

    #[tokio::test]
    async fn success_failover_pattern_can_retry_sync_candidate() {
        let result = ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code: 200,
            headers: Default::default(),
            body: None,
            telemetry: None,
            error: None,
        };
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(Some(serde_json::json!({
            "failover_rules": {
                "success_failover_patterns": [
                    {"pattern": "relay:.*格式错误"}
                ]
            }
        })));
        let plan = sample_plan();

        assert!(
            should_retry_next_local_candidate_sync(
                &state,
                &plan,
                "openai_chat_sync",
                Some(&local_report_context),
                &result,
                Some("{\"error\":\"relay: 返回格式错误\"}"),
            )
            .await
        );
    }

    #[tokio::test]
    async fn error_stop_pattern_can_stop_sync_failover() {
        let result = ExecutionResult {
            request_id: "req-1".to_string(),
            candidate_id: None,
            status_code: 400,
            headers: Default::default(),
            body: None,
            telemetry: None,
            error: None,
        };
        let local_report_context = serde_json::json!({
            "candidate_index": 0,
            "retry_index": 0,
        });
        let state = build_state_with_provider_config(Some(serde_json::json!({
            "failover_rules": {
                "error_stop_patterns": [
                    {"pattern": "content_policy_violation", "status_codes": [400]}
                ]
            }
        })));
        let plan = sample_plan();

        assert!(
            should_stop_local_candidate_failover_sync(
                &state,
                &plan,
                "openai_chat_sync",
                Some(&local_report_context),
                &result,
                Some("{\"error\":\"content_policy_violation\"}"),
            )
            .await
        );
        assert!(
            !should_retry_next_local_candidate_sync(
                &state,
                &plan,
                "openai_chat_sync",
                Some(&local_report_context),
                &result,
                Some("{\"error\":\"content_policy_violation\"}"),
            )
            .await
        );
    }
}
