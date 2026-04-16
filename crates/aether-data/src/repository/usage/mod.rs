mod memory;
mod sql;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::usage::{
    StoredProviderApiKeyUsageSummary, StoredProviderUsageSummary, StoredProviderUsageWindow,
    StoredRequestUsageAudit, StoredUsageAuditAggregation, StoredUsageAuditSummary,
    StoredUsageDailySummary, StoredUsageLeaderboardSummary, StoredUsageTimeSeriesBucket,
    UpsertUsageRecord, UsageAuditAggregationGroupBy, UsageAuditAggregationQuery,
    UsageAuditListQuery, UsageAuditSummaryQuery, UsageDailyHeatmapQuery, UsageLeaderboardGroupBy,
    UsageLeaderboardQuery, UsageReadRepository, UsageRepository, UsageTimeSeriesGranularity,
    UsageTimeSeriesQuery, UsageWriteRepository,
};
pub use memory::InMemoryUsageReadRepository;
pub use sql::SqlxUsageReadRepository;

pub(crate) fn incoming_usage_can_recover_terminal_failure(
    incoming_status: &str,
    incoming_billing_status: &str,
) -> bool {
    incoming_billing_status == "pending"
        && matches!(incoming_status, "pending" | "streaming" | "completed")
}

pub(crate) fn usage_can_recover_terminal_failure(
    existing_status: &str,
    existing_billing_status: &str,
    incoming_status: &str,
    incoming_billing_status: &str,
) -> bool {
    existing_billing_status == "void"
        && matches!(existing_status, "failed" | "cancelled")
        && incoming_usage_can_recover_terminal_failure(incoming_status, incoming_billing_status)
}

pub(crate) fn strip_deprecated_usage_display_fields(
    mut usage: UpsertUsageRecord,
) -> UpsertUsageRecord {
    usage.username = None;
    usage.api_key_name = None;
    usage
}

#[cfg(test)]
mod tests {
    use super::{
        incoming_usage_can_recover_terminal_failure, strip_deprecated_usage_display_fields,
        usage_can_recover_terminal_failure, UpsertUsageRecord,
    };

    #[test]
    fn strip_deprecated_usage_display_fields_clears_legacy_display_columns() {
        let usage = strip_deprecated_usage_display_fields(UpsertUsageRecord {
            request_id: "req-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("alice".to_string()),
            api_key_name: Some("default".to_string()),
            provider_name: "OpenAI".to_string(),
            model: "gpt-5".to_string(),
            target_model: None,
            provider_id: None,
            provider_endpoint_id: None,
            provider_api_key_id: None,
            request_type: Some("chat".to_string()),
            api_format: Some("openai:chat".to_string()),
            api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_api_format: Some("openai:chat".to_string()),
            provider_api_family: Some("openai".to_string()),
            provider_endpoint_kind: Some("chat".to_string()),
            has_format_conversion: Some(false),
            is_stream: Some(false),
            input_tokens: Some(10),
            output_tokens: Some(20),
            total_tokens: Some(30),
            cache_creation_input_tokens: None,
            cache_creation_ephemeral_5m_input_tokens: None,
            cache_creation_ephemeral_1h_input_tokens: None,
            cache_read_input_tokens: None,
            cache_creation_cost_usd: None,
            cache_read_cost_usd: None,
            output_price_per_1m: None,
            total_cost_usd: Some(0.25),
            actual_total_cost_usd: Some(0.15),
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(120),
            first_byte_time_ms: Some(40),
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            request_headers: None,
            request_body: None,
            request_body_ref: None,
            provider_request_headers: None,
            provider_request_body: None,
            provider_request_body_ref: None,
            response_headers: None,
            response_body: None,
            response_body_ref: None,
            client_response_headers: None,
            client_response_body: None,
            client_response_body_ref: None,
            candidate_id: None,
            candidate_index: None,
            key_name: None,
            planner_kind: None,
            route_family: None,
            route_kind: None,
            execution_path: None,
            local_execution_runtime_miss_reason: None,
            request_metadata: None,
            finalized_at_unix_secs: None,
            created_at_unix_ms: Some(100),
            updated_at_unix_secs: 101,
        });

        assert_eq!(usage.user_id.as_deref(), Some("user-1"));
        assert_eq!(usage.api_key_id.as_deref(), Some("key-1"));
        assert_eq!(usage.username, None);
        assert_eq!(usage.api_key_name, None);
        assert_eq!(usage.provider_name, "OpenAI");
        assert_eq!(usage.model, "gpt-5");
    }

    #[test]
    fn incoming_usage_recovery_only_applies_to_pending_lifecycle_states() {
        assert!(incoming_usage_can_recover_terminal_failure(
            "completed",
            "pending"
        ));
        assert!(incoming_usage_can_recover_terminal_failure(
            "streaming",
            "pending"
        ));
        assert!(!incoming_usage_can_recover_terminal_failure(
            "failed", "void"
        ));
        assert!(!incoming_usage_can_recover_terminal_failure(
            "completed",
            "settled"
        ));
    }

    #[test]
    fn usage_recovery_requires_void_failure_to_be_followed_by_pending_lifecycle_state() {
        assert!(usage_can_recover_terminal_failure(
            "failed",
            "void",
            "completed",
            "pending"
        ));
        assert!(usage_can_recover_terminal_failure(
            "cancelled",
            "void",
            "streaming",
            "pending"
        ));
        assert!(!usage_can_recover_terminal_failure(
            "completed",
            "pending",
            "completed",
            "pending"
        ));
        assert!(!usage_can_recover_terminal_failure(
            "failed", "void", "failed", "void"
        ));
    }
}
