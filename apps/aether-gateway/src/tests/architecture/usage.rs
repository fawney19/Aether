use super::*;

#[test]
fn usage_runtime_paths_depend_on_shared_crates_not_app_runtime_shims() {
    for path in [
        "apps/aether-gateway/src/usage/runtime.rs",
        "apps/aether-gateway/src/usage/worker.rs",
        "apps/aether-gateway/src/async_task/runtime.rs",
    ] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("aether_usage_runtime"),
            "{path} should depend on aether_usage_runtime"
        );
        assert!(
            !source.contains("wallet_runtime"),
            "{path} should not depend on wallet_runtime"
        );
    }

    for path in ["apps/aether-gateway/src/async_task/runtime.rs"] {
        let source = read_workspace_file(path);
        assert!(
            source.contains("aether_billing"),
            "{path} should depend on aether_billing"
        );
        assert!(
            !source.contains("billing_runtime::enrich_usage_event_with_billing"),
            "{path} should not depend on billing_runtime compat re-export"
        );
        assert!(
            !source.contains("settlement_runtime::settle_usage_if_needed"),
            "{path} should not depend on settlement_runtime compat re-export"
        );
    }

    let usage_runtime = read_workspace_file("apps/aether-gateway/src/usage/runtime.rs");
    assert!(
        !usage_runtime.contains("GatewayDataState"),
        "usage/runtime.rs should not own GatewayDataState integration impls anymore"
    );
    assert!(
        !usage_runtime.contains("UsageBillingEventEnricher"),
        "usage/runtime.rs should not own UsageBillingEventEnricher impl anymore"
    );
    assert!(
        !usage_runtime.contains("UsageRuntimeAccess"),
        "usage/runtime.rs should not own UsageRuntimeAccess impl anymore"
    );

    let usage_worker = read_workspace_file("apps/aether-gateway/src/usage/worker.rs");
    let runtime_usage_worker = usage_worker
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(usage_worker.as_str());
    assert!(
        !runtime_usage_worker.contains("GatewayDataState"),
        "usage/worker.rs runtime path should not own GatewayDataState integration impls anymore"
    );
    assert!(
        !runtime_usage_worker.contains("UsageRecordWriter"),
        "usage/worker.rs runtime path should not own UsageRecordWriter impl anymore"
    );

    let integrations = read_workspace_file("apps/aether-gateway/src/data/state/integrations.rs");
    for pattern in [
        "UsageBillingEventEnricher for GatewayDataState",
        "UsageRuntimeAccess for GatewayDataState",
        "UsageRecordWriter for GatewayDataState",
        "UsageSettlementWriter for GatewayDataState",
    ] {
        assert!(
            integrations.contains(pattern),
            "data/state/integrations.rs should centralize {pattern}"
        );
    }

    let usage_reporting_context =
        read_workspace_file("apps/aether-gateway/src/usage/reporting/context.rs");
    assert!(
        usage_reporting_context.contains("aether_usage_runtime"),
        "usage/reporting/context.rs should depend on aether_usage_runtime"
    );
    assert!(
        usage_reporting_context.contains("resolve_video_task_report_lookup"),
        "usage/reporting/context.rs should depend on shared video task report lookup helper"
    );
    for pattern in [
        "build_locally_actionable_report_context_from_video_task",
        "report_context_is_locally_actionable",
    ] {
        assert!(
            usage_reporting_context.contains(pattern),
            "usage/reporting/context.rs should depend on shared usage helper {pattern}"
        );
    }
    assert!(
        usage_reporting_context.contains("VideoTaskReportLookup::TaskIdOrExternal"),
        "usage/reporting/context.rs should keep app-local external task fallback orchestration"
    );
    for pattern in [
        "build_locally_actionable_report_context_from_request_candidate",
        "read_request_candidates_by_request_id(",
        "resolve_locally_actionable_report_context_from_request_candidates(",
    ] {
        assert!(
            !usage_reporting_context.contains(pattern),
            "usage/reporting/context.rs should not own request-candidate resolver details {pattern}"
        );
    }
    for pattern in [
        "context\n        .get(\"local_task_id\")",
        "context\n        .get(\"local_short_id\")",
        "context\n        .get(\"task_id\")",
        "VideoTaskLookupKey::ShortId(short_id)",
        "fn insert_missing_string_value(",
        "fn insert_missing_optional_string_value(",
        "fn has_non_empty_str(",
        "fn has_u64(",
    ] {
        assert!(
            !usage_reporting_context.contains(pattern),
            "usage/reporting/context.rs should not own video task report lookup parsing {pattern}"
        );
    }

    let usage_reporting_mod = read_workspace_file("apps/aether-gateway/src/usage/reporting/mod.rs");
    assert!(
        usage_reporting_mod.contains("aether_usage_runtime"),
        "usage/reporting/mod.rs should depend on aether_usage_runtime"
    );
    for pattern in [
        "is_local_ai_sync_report_kind",
        "is_local_ai_stream_report_kind",
        "sync_report_represents_failure",
        "extract_gemini_file_mapping_entries",
        "gemini_file_mapping_cache_key",
        "normalize_gemini_file_name",
        "report_request_id",
        "should_handle_local_sync_report",
        "should_handle_local_stream_report",
        "GEMINI_FILE_MAPPING_TTL_SECONDS",
    ] {
        assert!(
            usage_reporting_mod.contains(pattern),
            "usage/reporting/mod.rs should depend on shared usage helper {pattern}"
        );
    }
    for pattern in [
        "fn is_local_ai_sync_report_kind(",
        "fn is_local_ai_stream_report_kind(",
        "fn sync_report_represents_failure(",
        "fn extract_gemini_file_mapping_entries(",
        "fn maybe_push_local_gemini_file_mapping_entry(",
        "fn extract_sync_report_body_json(",
        "fn content_type_starts_with(",
        "fn normalize_file_name(",
        "const GEMINI_FILE_MAPPING_TTL_SECONDS",
        "const GEMINI_FILE_MAPPING_CACHE_PREFIX",
        "fn gemini_file_mapping_cache_key(",
        "fn report_request_id(",
        "fn should_handle_local_sync_report(",
        "fn should_handle_local_stream_report(",
        "\"openai_video_delete_sync_success\" && payload.status_code == 404",
    ] {
        assert!(
            !usage_reporting_mod.contains(pattern),
            "usage/reporting/mod.rs should not own local report classification logic {pattern}"
        );
    }
}

#[test]
fn handlers_do_not_depend_on_raw_state_records() {
    assert_no_sensitive_log_patterns(
        "src/handlers",
        &[
            "StoredUserSessionRecord",
            "StoredUserPreferenceRecord",
            "AdminPaymentCallbackRecord",
        ],
    );

    let state_types = read_workspace_file("apps/aether-gateway/src/state/types.rs");
    for pattern in [
        "pub(crate) struct GatewayUserSessionView",
        "pub(crate) struct GatewayUserPreferenceView",
        "pub(crate) struct GatewayAdminPaymentCallbackView",
    ] {
        assert!(
            state_types.contains(pattern),
            "state/types.rs should keep handler-facing state view {pattern}"
        );
    }
}
