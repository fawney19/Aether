macro_rules! impl_materialized_usage_read_repository {
    ($repository:ty) => {
        #[async_trait::async_trait]
        impl $crate::repository::usage::UsageReadRepository for $repository {
            async fn find_by_id(
                &self,
                id: &str,
            ) -> Result<
                Option<$crate::repository::usage::StoredRequestUsageAudit>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::find_by_id(&repository, id).await
            }

            async fn list_by_ids(
                &self,
                ids: &[String],
            ) -> Result<
                Vec<$crate::repository::usage::StoredRequestUsageAudit>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::list_by_ids(&repository, ids).await
            }

            async fn find_by_request_id(
                &self,
                request_id: &str,
            ) -> Result<
                Option<$crate::repository::usage::StoredRequestUsageAudit>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::find_by_request_id(&repository, request_id).await
            }

            async fn resolve_body_ref(
                &self,
                body_ref: &str,
            ) -> Result<Option<serde_json::Value>, $crate::DataLayerError> {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::resolve_body_ref(&repository, body_ref).await
            }

            async fn list_usage_audits(
                &self,
                query: &$crate::repository::usage::UsageAuditListQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredRequestUsageAudit>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::list_usage_audits(&repository, query).await
            }

            async fn count_usage_audits(
                &self,
                query: &$crate::repository::usage::UsageAuditListQuery,
            ) -> Result<u64, $crate::DataLayerError> {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::count_usage_audits(&repository, query).await
            }

            async fn list_usage_audits_by_keyword_search(
                &self,
                query: &$crate::repository::usage::UsageAuditKeywordSearchQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredRequestUsageAudit>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::list_usage_audits_by_keyword_search(&repository, query).await
            }

            async fn count_usage_audits_by_keyword_search(
                &self,
                query: &$crate::repository::usage::UsageAuditKeywordSearchQuery,
            ) -> Result<u64, $crate::DataLayerError> {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::count_usage_audits_by_keyword_search(&repository, query).await
            }

            async fn aggregate_usage_audits(
                &self,
                query: &$crate::repository::usage::UsageAuditAggregationQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageAuditAggregation>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::aggregate_usage_audits(&repository, query).await
            }

            async fn summarize_usage_audits(
                &self,
                query: &$crate::repository::usage::UsageAuditSummaryQuery,
            ) -> Result<
                $crate::repository::usage::StoredUsageAuditSummary,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_audits(&repository, query).await
            }

            async fn summarize_usage_totals_by_user_ids(
                &self,
                user_ids: &[String],
            ) -> Result<Vec<$crate::repository::usage::StoredUsageUserTotals>, $crate::DataLayerError>
            {
                <$repository>::summarize_usage_totals_by_user_ids(self, user_ids).await
            }

            async fn summarize_usage_cache_hit_summary(
                &self,
                query: &$crate::repository::usage::UsageCacheHitSummaryQuery,
            ) -> Result<
                $crate::repository::usage::StoredUsageCacheHitSummary,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_cache_hit_summary(&repository, query).await
            }

            async fn summarize_usage_settled_cost(
                &self,
                query: &$crate::repository::usage::UsageSettledCostSummaryQuery,
            ) -> Result<
                $crate::repository::usage::StoredUsageSettledCostSummary,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_settled_cost(&repository, query).await
            }

            async fn summarize_usage_cache_affinity_hit_summary(
                &self,
                query: &$crate::repository::usage::UsageCacheAffinityHitSummaryQuery,
            ) -> Result<
                $crate::repository::usage::StoredUsageCacheAffinityHitSummary,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_cache_affinity_hit_summary(&repository, query).await
            }

            async fn list_usage_cache_affinity_intervals(
                &self,
                query: &$crate::repository::usage::UsageCacheAffinityIntervalQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageCacheAffinityIntervalRow>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::list_usage_cache_affinity_intervals(&repository, query).await
            }

            async fn summarize_dashboard_usage(
                &self,
                query: &$crate::repository::usage::UsageDashboardSummaryQuery,
            ) -> Result<
                $crate::repository::usage::StoredUsageDashboardSummary,
                $crate::DataLayerError,
            > {
                <$repository>::summarize_dashboard_usage(self, query).await
            }

            async fn list_dashboard_daily_breakdown(
                &self,
                query: &$crate::repository::usage::UsageDashboardDailyBreakdownQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageDashboardDailyBreakdownRow>,
                $crate::DataLayerError,
            > {
                <$repository>::list_dashboard_daily_breakdown(self, query).await
            }

            async fn summarize_dashboard_provider_counts(
                &self,
                query: &$crate::repository::usage::UsageDashboardProviderCountsQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageDashboardProviderCount>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_dashboard_provider_counts(&repository, query).await
            }

            async fn summarize_usage_breakdown(
                &self,
                query: &$crate::repository::usage::UsageBreakdownSummaryQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageBreakdownSummaryRow>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_breakdown(&repository, query).await
            }

            async fn count_monitoring_usage_errors(
                &self,
                query: &$crate::repository::usage::UsageMonitoringErrorCountQuery,
            ) -> Result<u64, $crate::DataLayerError> {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::count_monitoring_usage_errors(&repository, query).await
            }

            async fn list_monitoring_usage_errors(
                &self,
                query: &$crate::repository::usage::UsageMonitoringErrorListQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredRequestUsageAudit>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::list_monitoring_usage_errors(&repository, query).await
            }

            async fn summarize_usage_error_distribution(
                &self,
                query: &$crate::repository::usage::UsageErrorDistributionQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageErrorDistributionRow>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_error_distribution(&repository, query).await
            }

            async fn summarize_usage_performance_percentiles(
                &self,
                query: &$crate::repository::usage::UsagePerformancePercentilesQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsagePerformancePercentilesRow>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_performance_percentiles(&repository, query).await
            }

            async fn summarize_usage_provider_performance(
                &self,
                query: &$crate::repository::usage::UsageProviderPerformanceQuery,
            ) -> Result<
                $crate::repository::usage::StoredUsageProviderPerformance,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_provider_performance(&repository, query).await
            }

            async fn summarize_usage_cost_savings(
                &self,
                query: &$crate::repository::usage::UsageCostSavingsSummaryQuery,
            ) -> Result<
                $crate::repository::usage::StoredUsageCostSavingsSummary,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_cost_savings(&repository, query).await
            }

            async fn summarize_usage_time_series(
                &self,
                query: &$crate::repository::usage::UsageTimeSeriesQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageTimeSeriesBucket>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_time_series(&repository, query).await
            }

            async fn summarize_usage_leaderboard(
                &self,
                query: &$crate::repository::usage::UsageLeaderboardQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageLeaderboardSummary>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_leaderboard(&repository, query).await
            }

            async fn list_recent_usage_audits(
                &self,
                user_id: Option<&str>,
                limit: usize,
            ) -> Result<
                Vec<$crate::repository::usage::StoredRequestUsageAudit>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::list_recent_usage_audits(&repository, user_id, limit).await
            }

            async fn summarize_total_tokens_by_api_key_ids(
                &self,
                api_key_ids: &[String],
            ) -> Result<std::collections::BTreeMap<String, u64>, $crate::DataLayerError> {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_total_tokens_by_api_key_ids(&repository, api_key_ids).await
            }

            async fn summarize_usage_by_provider_api_key_ids(
                &self,
                provider_api_key_ids: &[String],
            ) -> Result<
                std::collections::BTreeMap<
                    String,
                    $crate::repository::usage::StoredProviderApiKeyUsageSummary,
                >,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_by_provider_api_key_ids(&repository, provider_api_key_ids).await
            }

            async fn summarize_usage_by_provider_api_key_windows(
                &self,
                requests: &[$crate::repository::usage::ProviderApiKeyWindowUsageRequest],
            ) -> Result<
                Vec<$crate::repository::usage::StoredProviderApiKeyWindowUsageSummary>,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_usage_by_provider_api_key_windows(&repository, requests).await
            }

            async fn summarize_provider_usage_since(
                &self,
                provider_id: &str,
                since_unix_secs: u64,
            ) -> Result<
                $crate::repository::usage::StoredProviderUsageSummary,
                $crate::DataLayerError,
            > {
                let repository = self.materialize_read_model().await?;
                <$crate::repository::usage::InMemoryUsageReadRepository as $crate::repository::usage::UsageReadRepository>::summarize_provider_usage_since(&repository, provider_id, since_unix_secs).await
            }

            async fn summarize_usage_daily_heatmap(
                &self,
                query: &$crate::repository::usage::UsageDailyHeatmapQuery,
            ) -> Result<
                Vec<$crate::repository::usage::StoredUsageDailySummary>,
                $crate::DataLayerError,
            > {
                <$repository>::summarize_usage_daily_heatmap(self, query).await
            }
        }
    };
}

mod memory;
mod mysql;
mod postgres;
mod sqlite;

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::usage::{
    usage_request_metadata_client_family, ApiKeyLastUsedDelta, ManagementTokenCounterDelta,
    PendingUsageCleanupSummary, ProviderApiKeyWindowUsageRequest, ProxyNodeCounterDelta,
    StoredProviderApiKeyUsageSummary, StoredProviderApiKeyWindowUsageSummary,
    StoredProviderUsageSummary, StoredProviderUsageWindow, StoredRequestUsageAudit,
    StoredUsageAuditAggregation, StoredUsageAuditSummary, StoredUsageBreakdownSummaryRow,
    StoredUsageCacheAffinityHitSummary, StoredUsageCacheAffinityIntervalRow,
    StoredUsageCacheHitSummary, StoredUsageCostSavingsSummary, StoredUsageDailySummary,
    StoredUsageDashboardDailyBreakdownRow, StoredUsageDashboardProviderCount,
    StoredUsageDashboardSummary, StoredUsageErrorDistributionRow, StoredUsageLeaderboardSummary,
    StoredUsagePerformancePercentilesRow, StoredUsageProviderPerformance,
    StoredUsageProviderPerformanceProviderRow, StoredUsageProviderPerformanceSummary,
    StoredUsageProviderPerformanceTimelineRow, StoredUsageSettledCostSummary,
    StoredUsageTimeSeriesBucket, StoredUsageUserTotals, UpsertUsageRecord,
    UsageAuditAggregationGroupBy, UsageAuditAggregationQuery, UsageAuditKeywordSearchQuery,
    UsageAuditListQuery, UsageAuditSummaryQuery, UsageBreakdownGroupBy, UsageBreakdownSummaryQuery,
    UsageCacheAffinityHitSummaryQuery, UsageCacheAffinityIntervalGroupBy,
    UsageCacheAffinityIntervalQuery, UsageCacheHitSummaryQuery, UsageCleanupPreviewCounts,
    UsageCleanupSummary, UsageCleanupWindow, UsageCostSavingsSummaryQuery,
    UsageCounterFlushSummary, UsageCounterHealthSnapshot, UsageDailyHeatmapQuery,
    UsageDashboardDailyBreakdownQuery, UsageDashboardProviderCountsQuery,
    UsageDashboardSummaryQuery, UsageErrorDistributionQuery, UsageLeaderboardGroupBy,
    UsageLeaderboardQuery, UsageMonitoringErrorCountQuery, UsageMonitoringErrorListQuery,
    UsagePerformancePercentilesQuery, UsageProviderPerformanceQuery, UsageReadRepository,
    UsageRepository, UsageSettledCostSummaryQuery, UsageTimeSeriesGranularity,
    UsageTimeSeriesQuery, UsageWriteRepository,
};
pub mod cleanup {
    pub use super::postgres::cleanup::*;
}
pub use memory::InMemoryUsageReadRepository;
pub use mysql::{MysqlUsageReadRepository, MysqlUsageWriteRepository};
pub use postgres::SqlxUsageReadRepository;
pub use sqlite::{SqliteUsageReadRepository, SqliteUsageWriteRepository};

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ApiKeyUsageContribution {
    pub api_key_id: String,
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub last_used_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ApiKeyUsageDelta {
    pub total_requests: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub candidate_last_used_at_unix_secs: Option<u64>,
    pub removed_last_used_at_unix_secs: Option<u64>,
}

impl ApiKeyUsageDelta {
    pub(crate) fn between(
        before: &ApiKeyUsageContribution,
        after: &ApiKeyUsageContribution,
    ) -> Self {
        Self {
            total_requests: after.total_requests - before.total_requests,
            total_tokens: after.total_tokens - before.total_tokens,
            total_cost_usd: after.total_cost_usd - before.total_cost_usd,
            candidate_last_used_at_unix_secs: newer_last_used_at(
                before.last_used_at_unix_secs,
                after.last_used_at_unix_secs,
            ),
            removed_last_used_at_unix_secs: None,
        }
    }

    pub(crate) fn addition(after: &ApiKeyUsageContribution) -> Self {
        Self {
            total_requests: after.total_requests,
            total_tokens: after.total_tokens,
            total_cost_usd: after.total_cost_usd,
            candidate_last_used_at_unix_secs: after.last_used_at_unix_secs,
            removed_last_used_at_unix_secs: None,
        }
    }

    pub(crate) fn removal(before: &ApiKeyUsageContribution) -> Self {
        Self {
            total_requests: -before.total_requests,
            total_tokens: -before.total_tokens,
            total_cost_usd: -before.total_cost_usd,
            candidate_last_used_at_unix_secs: None,
            removed_last_used_at_unix_secs: before.last_used_at_unix_secs,
        }
    }

    pub(crate) fn is_noop(&self) -> bool {
        self.total_requests == 0
            && self.total_tokens == 0
            && self.total_cost_usd == 0.0
            && self.candidate_last_used_at_unix_secs.is_none()
            && self.removed_last_used_at_unix_secs.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ModelUsageContribution {
    pub model: String,
    pub request_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ModelUsageDelta {
    pub request_count: i64,
}

impl ModelUsageDelta {
    pub(crate) fn between(before: &ModelUsageContribution, after: &ModelUsageContribution) -> Self {
        Self {
            request_count: after.request_count - before.request_count,
        }
    }

    pub(crate) fn addition(after: &ModelUsageContribution) -> Self {
        Self {
            request_count: after.request_count,
        }
    }

    pub(crate) fn removal(before: &ModelUsageContribution) -> Self {
        Self {
            request_count: -before.request_count,
        }
    }

    pub(crate) fn is_noop(&self) -> bool {
        self.request_count == 0
    }
}

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
    pub usage_created_at_unix_secs: Option<u64>,
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
    pub usage_created_at_unix_secs: Option<u64>,
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
            candidate_last_used_at_unix_secs: newer_last_used_at(
                before.last_used_at_unix_secs,
                after.last_used_at_unix_secs,
            ),
            removed_last_used_at_unix_secs: None,
            usage_created_at_unix_secs: after.usage_created_at_unix_secs,
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
            usage_created_at_unix_secs: after.usage_created_at_unix_secs,
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
            usage_created_at_unix_secs: before.usage_created_at_unix_secs,
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
        // Late pending placeholders are not authoritative enough to reopen a void terminal row;
        // they can otherwise regress a real failure back to pending when background writes race.
        && matches!(incoming_status, "streaming" | "completed")
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
    let is_in_flight = matches!(usage.status.as_str(), "pending" | "streaming");
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
        total_tokens: if is_in_flight {
            0
        } else {
            i64::try_from(usage.total_tokens).unwrap_or(i64::MAX)
        },
        total_cost_usd: if is_in_flight {
            0.0
        } else if usage.total_cost_usd.is_finite() {
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
        usage_created_at_unix_secs: Some(usage.created_at_unix_ms),
    })
}

pub(crate) fn model_usage_contribution(
    usage: &StoredRequestUsageAudit,
) -> Option<ModelUsageContribution> {
    if matches!(usage.status.as_str(), "pending" | "streaming") {
        return None;
    }
    let model = usage.model.trim();
    if model.is_empty() {
        return None;
    }

    Some(ModelUsageContribution {
        model: model.to_string(),
        request_count: 1,
    })
}

pub(crate) fn api_key_usage_contribution(
    usage: &StoredRequestUsageAudit,
) -> Option<ApiKeyUsageContribution> {
    if matches!(usage.status.as_str(), "pending" | "streaming") {
        return None;
    }
    let api_key_id = usage
        .api_key_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();

    Some(ApiKeyUsageContribution {
        api_key_id,
        total_requests: 1,
        total_tokens: i64::try_from(usage.total_tokens).unwrap_or(i64::MAX),
        total_cost_usd: if usage.total_cost_usd.is_finite() {
            usage.total_cost_usd.max(0.0)
        } else {
            0.0
        },
        last_used_at_unix_secs: Some(usage.created_at_unix_ms),
    })
}

fn newer_last_used_at(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    match (before, after) {
        (Some(before), Some(after)) if after > before => Some(after),
        (None, Some(after)) => Some(after),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        api_key_usage_contribution, incoming_usage_can_recover_terminal_failure,
        model_usage_contribution, provider_api_key_usage_contribution,
        provider_api_key_usage_is_error, provider_api_key_usage_is_success,
        strip_deprecated_usage_display_fields, usage_can_recover_terminal_failure,
        ApiKeyUsageDelta, ModelUsageDelta, ProviderApiKeyUsageDelta, StoredRequestUsageAudit,
        UpsertUsageRecord,
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
    fn incoming_usage_recovery_requires_streaming_or_completed_state() {
        assert!(incoming_usage_can_recover_terminal_failure(
            "completed",
            "pending"
        ));
        assert!(incoming_usage_can_recover_terminal_failure(
            "streaming",
            "pending"
        ));
        assert!(!incoming_usage_can_recover_terminal_failure(
            "pending", "pending"
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
    fn usage_recovery_requires_void_failure_to_be_followed_by_streaming_or_completed_state() {
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
            "failed", "void", "pending", "pending"
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

    #[test]
    fn api_key_usage_contribution_tracks_request_totals() {
        let usage = StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "request-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            None,
            None,
            "OpenAI".to_string(),
            "gpt-5".to_string(),
            None,
            Some("provider-1".to_string()),
            None,
            None,
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

        let contribution = api_key_usage_contribution(&usage).expect("contribution should exist");
        assert_eq!(contribution.api_key_id, "api-key-1");
        assert_eq!(contribution.total_requests, 1);
        assert_eq!(contribution.total_tokens, 20);
        assert_eq!(contribution.total_cost_usd, 0.25);
        assert_eq!(contribution.last_used_at_unix_secs, Some(123));

        let mut streaming = usage.clone();
        streaming.status = "streaming".to_string();
        assert!(api_key_usage_contribution(&streaming).is_none());

        let mut pending = usage;
        pending.status = "pending".to_string();
        assert!(api_key_usage_contribution(&pending).is_none());
    }

    #[test]
    fn provider_api_key_usage_contribution_counts_in_flight_requests_once() {
        let usage = StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "request-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
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

        assert!(provider_api_key_usage_contribution(&usage).is_some());

        let mut streaming = usage.clone();
        streaming.status = "streaming".to_string();
        let streaming_contribution =
            provider_api_key_usage_contribution(&streaming).expect("streaming should count");
        assert_eq!(streaming_contribution.request_count, 1);
        assert_eq!(streaming_contribution.success_count, 0);
        assert_eq!(streaming_contribution.error_count, 0);
        assert_eq!(streaming_contribution.total_tokens, 0);
        assert_eq!(streaming_contribution.total_cost_usd, 0.0);
        assert_eq!(streaming_contribution.total_response_time_ms, 0);

        let mut pending = usage.clone();
        pending.status = "pending".to_string();
        let pending_contribution =
            provider_api_key_usage_contribution(&pending).expect("pending should count");
        assert_eq!(pending_contribution.request_count, 1);
        assert_eq!(pending_contribution.success_count, 0);
        assert_eq!(pending_contribution.error_count, 0);
        assert_eq!(pending_contribution.total_tokens, 0);
        assert_eq!(pending_contribution.total_cost_usd, 0.0);
        assert_eq!(pending_contribution.total_response_time_ms, 0);

        let terminal_contribution =
            provider_api_key_usage_contribution(&usage).expect("terminal should count");
        let delta =
            ProviderApiKeyUsageDelta::between(&pending_contribution, &terminal_contribution);
        assert_eq!(delta.request_count, 0);
        assert_eq!(delta.success_count, 1);
        assert_eq!(delta.error_count, 0);
        assert_eq!(delta.total_tokens, 20);
        assert_eq!(delta.total_cost_usd, 0.25);
        assert_eq!(delta.total_response_time_ms, 120);
    }

    #[test]
    fn usage_delta_between_does_not_emit_duplicate_last_used_candidate() {
        let api_key_contribution = super::ApiKeyUsageContribution {
            api_key_id: "api-key-1".to_string(),
            total_requests: 1,
            total_tokens: 20,
            total_cost_usd: 0.25,
            last_used_at_unix_secs: Some(123),
        };
        assert!(ApiKeyUsageDelta::between(&api_key_contribution, &api_key_contribution).is_noop());

        let provider_contribution = super::ProviderApiKeyUsageContribution {
            key_id: "provider-key-1".to_string(),
            request_count: 1,
            success_count: 1,
            error_count: 0,
            total_tokens: 20,
            total_cost_usd: 0.25,
            total_response_time_ms: 120,
            last_used_at_unix_secs: Some(123),
            usage_created_at_unix_secs: Some(123),
        };
        assert!(
            ProviderApiKeyUsageDelta::between(&provider_contribution, &provider_contribution,)
                .is_noop()
        );
    }

    #[test]
    fn model_usage_contribution_tracks_terminal_requests_only() {
        let completed = StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "request-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            None,
            None,
            "OpenAI".to_string(),
            " gpt-5.5 ".to_string(),
            None,
            Some("provider-1".to_string()),
            None,
            None,
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
            model_usage_contribution(&completed).expect("completed usage should count");
        assert_eq!(contribution.model, "gpt-5.5");
        assert_eq!(contribution.request_count, 1);

        let mut streaming = completed.clone();
        streaming.status = "streaming".to_string();
        assert!(model_usage_contribution(&streaming).is_none());

        let mut pending = completed;
        pending.status = "pending".to_string();
        assert!(model_usage_contribution(&pending).is_none());
    }

    #[test]
    fn model_usage_delta_handles_model_changes() {
        let before = super::ModelUsageContribution {
            model: "gpt-5.4".to_string(),
            request_count: 1,
        };
        let after = super::ModelUsageContribution {
            model: "gpt-5.5".to_string(),
            request_count: 1,
        };

        assert_eq!(ModelUsageDelta::removal(&before).request_count, -1);
        assert_eq!(ModelUsageDelta::addition(&after).request_count, 1);
        assert!(ModelUsageDelta::between(&before, &before).is_noop());
    }
}
