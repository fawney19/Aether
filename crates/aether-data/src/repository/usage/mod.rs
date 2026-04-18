mod memory;
mod sql;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::usage::{
    StoredProviderApiKeyUsageSummary, StoredProviderUsageSummary, StoredProviderUsageWindow,
    StoredRequestUsageAudit, StoredUsageAuditAggregation, StoredUsageAuditSummary,
    StoredUsageBreakdownSummaryRow, StoredUsageCacheAffinityHitSummary,
    StoredUsageCacheAffinityIntervalRow, StoredUsageCacheHitSummary, StoredUsageCostSavingsSummary,
    StoredUsageDailySummary, StoredUsageDashboardDailyBreakdownRow,
    StoredUsageDashboardProviderCount, StoredUsageDashboardSummary,
    StoredUsageErrorDistributionRow, StoredUsageLeaderboardSummary,
    StoredUsagePerformancePercentilesRow, StoredUsageSettledCostSummary,
    StoredUsageTimeSeriesBucket, StoredUsageUserTotals, UpsertUsageRecord,
    UsageAuditAggregationGroupBy, UsageAuditAggregationQuery, UsageAuditKeywordSearchQuery,
    UsageAuditListQuery, UsageAuditSummaryQuery, UsageBreakdownGroupBy, UsageBreakdownSummaryQuery,
    UsageCacheAffinityHitSummaryQuery, UsageCacheAffinityIntervalGroupBy,
    UsageCacheAffinityIntervalQuery, UsageCacheHitSummaryQuery, UsageCostSavingsSummaryQuery,
    UsageDailyHeatmapQuery, UsageDashboardDailyBreakdownQuery, UsageDashboardProviderCountsQuery,
    UsageDashboardSummaryQuery, UsageErrorDistributionQuery, UsageLeaderboardGroupBy,
    UsageLeaderboardQuery, UsageMonitoringErrorCountQuery, UsageMonitoringErrorListQuery,
    UsagePerformancePercentilesQuery, UsageReadRepository, UsageRepository,
    UsageSettledCostSummaryQuery, UsageTimeSeriesGranularity, UsageTimeSeriesQuery,
    UsageWriteRepository,
};
pub use memory::InMemoryUsageReadRepository;
pub use sql::SqlxUsageReadRepository;

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ProviderApiKeyUsageContribution {
    pub key_id: String,
    pub request_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub total_response_time_ms: i64,
    pub last_used_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ProviderApiKeyUsageDelta {
    pub request_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub total_response_time_ms: i64,
    pub candidate_last_used_at_unix_secs: Option<u64>,
    pub removed_last_used_at_unix_secs: Option<u64>,
}

impl ProviderApiKeyUsageDelta {
    pub(crate) fn between(
        before: &ProviderApiKeyUsageContribution,
        after: &ProviderApiKeyUsageContribution,
    ) -> Self {
        Self {
            request_count: after.request_count - before.request_count,
            success_count: after.success_count - before.success_count,
            error_count: after.error_count - before.error_count,
            total_tokens: after.total_tokens - before.total_tokens,
            total_cost_usd: after.total_cost_usd - before.total_cost_usd,
            total_response_time_ms: after.total_response_time_ms - before.total_response_time_ms,
            candidate_last_used_at_unix_secs: after.last_used_at_unix_secs,
            removed_last_used_at_unix_secs: None,
        }
    }

    pub(crate) fn addition(after: &ProviderApiKeyUsageContribution) -> Self {
        Self {
            request_count: after.request_count,
            success_count: after.success_count,
            error_count: after.error_count,
            total_tokens: after.total_tokens,
            total_cost_usd: after.total_cost_usd,
            total_response_time_ms: after.total_response_time_ms,
            candidate_last_used_at_unix_secs: after.last_used_at_unix_secs,
            removed_last_used_at_unix_secs: None,
        }
    }

    pub(crate) fn removal(before: &ProviderApiKeyUsageContribution) -> Self {
        Self {
            request_count: -before.request_count,
            success_count: -before.success_count,
            error_count: -before.error_count,
            total_tokens: -before.total_tokens,
            total_cost_usd: -before.total_cost_usd,
            total_response_time_ms: -before.total_response_time_ms,
            candidate_last_used_at_unix_secs: None,
            removed_last_used_at_unix_secs: before.last_used_at_unix_secs,
        }
    }

    pub(crate) fn is_noop(&self) -> bool {
        self.request_count == 0
            && self.success_count == 0
            && self.error_count == 0
            && self.total_tokens == 0
            && self.total_cost_usd == 0.0
            && self.total_response_time_ms == 0
            && self.candidate_last_used_at_unix_secs.is_none()
            && self.removed_last_used_at_unix_secs.is_none()
    }
}

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

/// Clear legacy display-cache fields that still exist on `public.usage` for compatibility.
///
/// These values are no longer treated as authoritative read-model inputs; new writes should not
/// repopulate the deprecated mirror columns.
pub(crate) fn strip_deprecated_usage_display_fields(
    mut usage: UpsertUsageRecord,
) -> UpsertUsageRecord {
    usage.username = None;
    usage.api_key_name = None;
    usage
}

pub(crate) fn provider_api_key_usage_is_success(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> bool {
    matches!(
        status,
        "completed" | "success" | "ok" | "billed" | "settled"
    ) && status_code.is_none_or(|code| code < 400)
        && error_message.is_none_or(|value| value.trim().is_empty())
}

pub(crate) fn provider_api_key_usage_is_error(
    status: &str,
    status_code: Option<u16>,
    error_message: Option<&str>,
) -> bool {
    !matches!(status, "pending" | "streaming")
        && !provider_api_key_usage_is_success(status, status_code, error_message)
}

pub(crate) fn provider_api_key_usage_contribution(
    usage: &StoredRequestUsageAudit,
) -> Option<ProviderApiKeyUsageContribution> {
    let key_id = usage
        .provider_api_key_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let is_success = provider_api_key_usage_is_success(
        usage.status.as_str(),
        usage.status_code,
        usage.error_message.as_deref(),
    );
    let is_error = provider_api_key_usage_is_error(
        usage.status.as_str(),
        usage.status_code,
        usage.error_message.as_deref(),
    );

    Some(ProviderApiKeyUsageContribution {
        key_id,
        request_count: 1,
        success_count: i64::from(is_success),
        error_count: i64::from(is_error),
        total_tokens: i64::try_from(usage.total_tokens).unwrap_or(i64::MAX),
        total_cost_usd: if usage.total_cost_usd.is_finite() {
            usage.total_cost_usd.max(0.0)
        } else {
            0.0
        },
        total_response_time_ms: if is_success {
            usage
                .response_time_ms
                .and_then(|value| i64::try_from(value).ok())
                .unwrap_or_default()
        } else {
            0
        },
        last_used_at_unix_secs: Some(usage.created_at_unix_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        incoming_usage_can_recover_terminal_failure, provider_api_key_usage_contribution,
        provider_api_key_usage_is_error, provider_api_key_usage_is_success,
        strip_deprecated_usage_display_fields, usage_can_recover_terminal_failure,
        StoredRequestUsageAudit, UpsertUsageRecord,
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
            request_body_state: None,
            provider_request_body_state: None,
            response_body_state: None,
            client_response_body_state: None,
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

    #[test]
    fn provider_key_usage_success_requires_clean_terminal_success() {
        assert!(provider_api_key_usage_is_success(
            "completed",
            Some(200),
            None
        ));
        assert!(!provider_api_key_usage_is_success(
            "completed",
            Some(500),
            None
        ));
        assert!(!provider_api_key_usage_is_success(
            "completed",
            Some(200),
            Some("boom")
        ));
        assert!(!provider_api_key_usage_is_success(
            "streaming",
            Some(200),
            None
        ));
    }

    #[test]
    fn provider_key_usage_error_ignores_pending_states() {
        assert!(provider_api_key_usage_is_error(
            "failed",
            Some(500),
            Some("boom")
        ));
        assert!(provider_api_key_usage_is_error(
            "completed",
            Some(200),
            Some("boom")
        ));
        assert!(!provider_api_key_usage_is_error("pending", None, None));
        assert!(!provider_api_key_usage_is_error("streaming", None, None));
    }

    #[test]
    fn provider_key_usage_contribution_tracks_success_response_time() {
        let usage = StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "request-1".to_string(),
            None,
            None,
            None,
            None,
            "OpenAI".to_string(),
            "gpt-5".to_string(),
            None,
            Some("provider-1".to_string()),
            None,
            Some("provider-key-1".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            12,
            8,
            20,
            0.25,
            0.25,
            Some(200),
            None,
            None,
            Some(120),
            None,
            "completed".to_string(),
            "settled".to_string(),
            123,
            124,
            Some(125),
        )
        .expect("usage should build");

        let contribution =
            provider_api_key_usage_contribution(&usage).expect("contribution should exist");
        assert_eq!(contribution.key_id, "provider-key-1");
        assert_eq!(contribution.request_count, 1);
        assert_eq!(contribution.success_count, 1);
        assert_eq!(contribution.error_count, 0);
        assert_eq!(contribution.total_tokens, 20);
        assert_eq!(contribution.total_cost_usd, 0.25);
        assert_eq!(contribution.total_response_time_ms, 120);
        assert_eq!(contribution.last_used_at_unix_secs, Some(123));
    }
}
