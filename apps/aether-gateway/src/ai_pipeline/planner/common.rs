use axum::body::Bytes;

pub(crate) use crate::ai_pipeline::contracts::{
    CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_STREAM_PLAN_KIND,
    CLAUDE_CLI_SYNC_PLAN_KIND, EXECUTION_RUNTIME_STREAM_ACTION,
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, EXECUTION_RUNTIME_SYNC_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_CHAT_STREAM_PLAN_KIND,
    GEMINI_CHAT_SYNC_PLAN_KIND, GEMINI_CLI_STREAM_PLAN_KIND, GEMINI_CLI_SYNC_PLAN_KIND,
    GEMINI_FILES_DELETE_PLAN_KIND, GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND,
    GEMINI_FILES_LIST_PLAN_KIND, GEMINI_FILES_UPLOAD_PLAN_KIND, GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_CHAT_STREAM_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND,
    OPENAI_CLI_STREAM_PLAN_KIND, OPENAI_CLI_SYNC_PLAN_KIND, OPENAI_COMPACT_STREAM_PLAN_KIND,
    OPENAI_COMPACT_SYNC_PLAN_KIND, OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CONTENT_PLAN_KIND, OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND, OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::ai_pipeline::GatewayControlDecision;
use crate::ai_pipeline::{
    extract_gemini_model_from_path as extract_gemini_model_from_path_impl,
    force_upstream_streaming_for_provider as force_upstream_streaming_for_provider_impl,
    is_json_request, parse_direct_request_body as parse_direct_request_body_impl,
};
use crate::LocalExecutionRuntimeMissDiagnostic;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestedModelFamily {
    Standard,
    Gemini,
}

pub(crate) fn parse_direct_request_body(
    parts: &http::request::Parts,
    body_bytes: &Bytes,
) -> Option<(serde_json::Value, Option<String>)> {
    parse_direct_request_body_impl(is_json_request(&parts.headers), body_bytes.as_ref())
}

pub(crate) fn force_upstream_streaming_for_provider(
    provider_type: &str,
    provider_api_format: &str,
) -> bool {
    force_upstream_streaming_for_provider_impl(provider_type, provider_api_format)
}

pub(crate) fn extract_standard_requested_model(body_json: &serde_json::Value) -> Option<String> {
    body_json
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn extract_requested_model_from_request(
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    family: RequestedModelFamily,
) -> Option<String> {
    match family {
        RequestedModelFamily::Standard => extract_standard_requested_model(body_json),
        RequestedModelFamily::Gemini => extract_gemini_model_from_path_impl(parts.uri.path()),
    }
}

pub(crate) fn build_local_runtime_miss_diagnostic(
    decision: &GatewayControlDecision,
    plan_kind: &str,
    requested_model: Option<&str>,
    reason: &str,
) -> LocalExecutionRuntimeMissDiagnostic {
    LocalExecutionRuntimeMissDiagnostic {
        reason: reason.to_string(),
        route_family: decision.route_family.clone(),
        route_kind: decision.route_kind.clone(),
        public_path: Some(decision.public_path.clone()),
        plan_kind: Some(plan_kind.to_string()),
        requested_model: requested_model.map(ToOwned::to_owned),
        candidate_count: None,
        skipped_candidate_count: None,
        skip_reasons: std::collections::BTreeMap::new(),
    }
}

pub(crate) fn apply_local_candidate_evaluation_progress(
    diagnostic: &mut LocalExecutionRuntimeMissDiagnostic,
    candidate_count: usize,
) {
    diagnostic.candidate_count = Some(candidate_count);
    diagnostic.reason = if candidate_count == 0 {
        "candidate_list_empty".to_string()
    } else {
        "candidate_evaluation_incomplete".to_string()
    };
}

pub(crate) fn apply_local_candidate_terminal_plan_reason(
    diagnostic: &mut LocalExecutionRuntimeMissDiagnostic,
    no_plan_reason: &'static str,
) {
    let candidate_count = diagnostic.candidate_count.unwrap_or(0);
    let skipped_candidate_count = diagnostic.skipped_candidate_count.unwrap_or(0);
    diagnostic.reason = if candidate_count == 0 {
        "candidate_list_empty".to_string()
    } else if skipped_candidate_count >= candidate_count
        && diagnostic.skip_reasons.len() == 1
        && diagnostic
            .skip_reasons
            .get("api_key_concurrency_limit_reached")
            .copied()
            .unwrap_or(0)
            > 0
    {
        "api_key_concurrency_limit_reached".to_string()
    } else if skipped_candidate_count >= candidate_count {
        "all_candidates_skipped".to_string()
    } else {
        no_plan_reason.to_string()
    };
}

#[cfg(test)]
mod tests {
    use super::{
        apply_local_candidate_evaluation_progress, apply_local_candidate_terminal_plan_reason,
        build_local_runtime_miss_diagnostic, extract_requested_model_from_request,
        extract_standard_requested_model, force_upstream_streaming_for_provider,
        RequestedModelFamily,
    };
    use axum::http::Request;
    use serde_json::json;

    #[test]
    fn forces_streaming_for_codex_openai_cli() {
        assert!(force_upstream_streaming_for_provider("codex", "openai:cli"));
    }

    #[test]
    fn does_not_force_streaming_for_compact_or_other_provider_types() {
        assert!(!force_upstream_streaming_for_provider(
            "codex",
            "openai:compact"
        ));
        assert!(!force_upstream_streaming_for_provider(
            "openai",
            "openai:cli"
        ));
    }

    #[test]
    fn extracts_standard_requested_model_from_request_body() {
        let requested_model =
            extract_standard_requested_model(&json!({ "model": " claude-sonnet-4 " }));

        assert_eq!(requested_model.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn request_family_helper_delegates_standard_model_extraction() {
        let request = Request::builder()
            .uri("https://example.test/v1/chat/completions")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();

        let requested_model = extract_requested_model_from_request(
            &parts,
            &json!({ "model": " claude-sonnet-4 " }),
            RequestedModelFamily::Standard,
        );

        assert_eq!(requested_model.as_deref(), Some("claude-sonnet-4"));
    }

    #[test]
    fn extracts_gemini_requested_model_from_request_path() {
        let request = Request::builder()
            .uri("https://example.test/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();

        let requested_model =
            extract_requested_model_from_request(&parts, &json!({}), RequestedModelFamily::Gemini);

        assert_eq!(requested_model.as_deref(), Some("gemini-2.5-pro"));
    }

    #[test]
    fn candidate_evaluation_progress_sets_candidate_count_and_reason() {
        let mut diagnostic = build_local_runtime_miss_diagnostic(
            &crate::ai_pipeline::GatewayControlDecision::synthetic(
                "/v1/test",
                Some("passthrough".to_string()),
                Some("ai".to_string()),
                Some("chat".to_string()),
                Some("test#sync".to_string()),
            ),
            "test_plan",
            Some("test-model"),
            "seed",
        );

        apply_local_candidate_evaluation_progress(&mut diagnostic, 0);
        assert_eq!(diagnostic.candidate_count, Some(0));
        assert_eq!(diagnostic.reason, "candidate_list_empty");

        apply_local_candidate_evaluation_progress(&mut diagnostic, 3);
        assert_eq!(diagnostic.candidate_count, Some(3));
        assert_eq!(diagnostic.reason, "candidate_evaluation_incomplete");
    }

    #[test]
    fn candidate_terminal_reason_prefers_empty_then_skipped_then_fallback() {
        let mut diagnostic = build_local_runtime_miss_diagnostic(
            &crate::ai_pipeline::GatewayControlDecision::synthetic(
                "/v1/test",
                Some("passthrough".to_string()),
                Some("ai".to_string()),
                Some("chat".to_string()),
                Some("test#sync".to_string()),
            ),
            "test_plan",
            Some("test-model"),
            "seed",
        );

        apply_local_candidate_terminal_plan_reason(&mut diagnostic, "no_local_sync_plans");
        assert_eq!(diagnostic.reason, "candidate_list_empty");

        diagnostic.candidate_count = Some(2);
        diagnostic.skipped_candidate_count = Some(2);
        apply_local_candidate_terminal_plan_reason(&mut diagnostic, "no_local_sync_plans");
        assert_eq!(diagnostic.reason, "all_candidates_skipped");

        diagnostic.skip_reasons = std::collections::BTreeMap::from([(
            "api_key_concurrency_limit_reached".to_string(),
            2,
        )]);
        apply_local_candidate_terminal_plan_reason(&mut diagnostic, "no_local_sync_plans");
        assert_eq!(diagnostic.reason, "api_key_concurrency_limit_reached");

        diagnostic.skipped_candidate_count = Some(1);
        diagnostic.skip_reasons.clear();
        apply_local_candidate_terminal_plan_reason(&mut diagnostic, "no_local_sync_plans");
        assert_eq!(diagnostic.reason, "no_local_sync_plans");
    }
}
