use crate::{AppState, GatewayError};
use aether_data_contracts::repository::{candidates, usage};
use usage::{StoredUsageDailySummary, UsageDailyHeatmapQuery};

impl AppState {
    #[allow(dead_code)]
    pub(crate) async fn rebuild_provider_api_key_usage_stats(&self) -> Result<u64, GatewayError> {
        self.data
            .rebuild_provider_api_key_usage_stats()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn read_request_candidates_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Vec<candidates::StoredRequestCandidate>, GatewayError> {
        self.data
            .list_request_candidates_by_request_id(request_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn read_request_candidates_by_provider_id(
        &self,
        provider_id: &str,
        limit: usize,
    ) -> Result<Vec<candidates::StoredRequestCandidate>, GatewayError> {
        self.data
            .list_request_candidates_by_provider_id(provider_id, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<usage::StoredProviderUsageSummary, GatewayError> {
        self.data
            .summarize_provider_usage_since(provider_id, since_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_request_usage_by_id(
        &self,
        usage_id: &str,
    ) -> Result<Option<usage::StoredRequestUsageAudit>, GatewayError> {
        self.data
            .find_request_usage_by_id(usage_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_request_usage_by_ids(
        &self,
        usage_ids: &[String],
    ) -> Result<Vec<usage::StoredRequestUsageAudit>, GatewayError> {
        self.data
            .list_request_usage_by_ids(usage_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_usage_audits(
        &self,
        query: &usage::UsageAuditListQuery,
    ) -> Result<Vec<usage::StoredRequestUsageAudit>, GatewayError> {
        self.data
            .list_usage_audits(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_usage_audits(
        &self,
        query: &usage::UsageAuditListQuery,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_usage_audits(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_usage_audits_by_keyword_search(
        &self,
        query: &usage::UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<usage::StoredRequestUsageAudit>, GatewayError> {
        self.data
            .list_usage_audits_by_keyword_search(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_usage_audits_by_keyword_search(
        &self,
        query: &usage::UsageAuditKeywordSearchQuery,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_usage_audits_by_keyword_search(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn aggregate_usage_audits(
        &self,
        query: &usage::UsageAuditAggregationQuery,
    ) -> Result<Vec<usage::StoredUsageAuditAggregation>, GatewayError> {
        self.data
            .aggregate_usage_audits(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_audits(
        &self,
        query: &usage::UsageAuditSummaryQuery,
    ) -> Result<usage::StoredUsageAuditSummary, GatewayError> {
        self.data
            .summarize_usage_audits(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_cache_hit_summary(
        &self,
        query: &usage::UsageCacheHitSummaryQuery,
    ) -> Result<usage::StoredUsageCacheHitSummary, GatewayError> {
        self.data
            .summarize_usage_cache_hit_summary(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_settled_cost(
        &self,
        query: &usage::UsageSettledCostSummaryQuery,
    ) -> Result<usage::StoredUsageSettledCostSummary, GatewayError> {
        self.data
            .summarize_usage_settled_cost(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_cache_affinity_hit_summary(
        &self,
        query: &usage::UsageCacheAffinityHitSummaryQuery,
    ) -> Result<usage::StoredUsageCacheAffinityHitSummary, GatewayError> {
        self.data
            .summarize_usage_cache_affinity_hit_summary(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_usage_cache_affinity_intervals(
        &self,
        query: &usage::UsageCacheAffinityIntervalQuery,
    ) -> Result<Vec<usage::StoredUsageCacheAffinityIntervalRow>, GatewayError> {
        self.data
            .list_usage_cache_affinity_intervals(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_dashboard_usage(
        &self,
        query: &usage::UsageDashboardSummaryQuery,
    ) -> Result<usage::StoredUsageDashboardSummary, GatewayError> {
        self.data
            .summarize_dashboard_usage(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_dashboard_daily_breakdown(
        &self,
        query: &usage::UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<usage::StoredUsageDashboardDailyBreakdownRow>, GatewayError> {
        self.data
            .list_dashboard_daily_breakdown(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_dashboard_provider_counts(
        &self,
        query: &usage::UsageDashboardProviderCountsQuery,
    ) -> Result<Vec<usage::StoredUsageDashboardProviderCount>, GatewayError> {
        self.data
            .summarize_dashboard_provider_counts(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_breakdown(
        &self,
        query: &usage::UsageBreakdownSummaryQuery,
    ) -> Result<Vec<usage::StoredUsageBreakdownSummaryRow>, GatewayError> {
        self.data
            .summarize_usage_breakdown(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_monitoring_usage_errors(
        &self,
        query: &usage::UsageMonitoringErrorCountQuery,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_monitoring_usage_errors(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_monitoring_usage_errors(
        &self,
        query: &usage::UsageMonitoringErrorListQuery,
    ) -> Result<Vec<usage::StoredRequestUsageAudit>, GatewayError> {
        self.data
            .list_monitoring_usage_errors(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_error_distribution(
        &self,
        query: &usage::UsageErrorDistributionQuery,
    ) -> Result<Vec<usage::StoredUsageErrorDistributionRow>, GatewayError> {
        self.data
            .summarize_usage_error_distribution(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_performance_percentiles(
        &self,
        query: &usage::UsagePerformancePercentilesQuery,
    ) -> Result<Vec<usage::StoredUsagePerformancePercentilesRow>, GatewayError> {
        self.data
            .summarize_usage_performance_percentiles(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_cost_savings(
        &self,
        query: &usage::UsageCostSavingsSummaryQuery,
    ) -> Result<usage::StoredUsageCostSavingsSummary, GatewayError> {
        self.data
            .summarize_usage_cost_savings(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_time_series(
        &self,
        query: &usage::UsageTimeSeriesQuery,
    ) -> Result<Vec<usage::StoredUsageTimeSeriesBucket>, GatewayError> {
        self.data
            .summarize_usage_time_series(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_leaderboard(
        &self,
        query: &usage::UsageLeaderboardQuery,
    ) -> Result<Vec<usage::StoredUsageLeaderboardSummary>, GatewayError> {
        self.data
            .summarize_usage_leaderboard(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, GatewayError> {
        self.data
            .summarize_usage_daily_heatmap(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<usage::StoredRequestUsageAudit>, GatewayError> {
        self.data
            .list_recent_usage_audits(user_id, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, u64>, GatewayError> {
        self.data
            .summarize_usage_total_tokens_by_api_key_ids(api_key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<usage::StoredUsageUserTotals>, GatewayError> {
        self.data
            .summarize_usage_totals_by_user_ids(user_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<
        std::collections::BTreeMap<String, usage::StoredProviderApiKeyUsageSummary>,
        GatewayError,
    > {
        self.data
            .summarize_usage_by_provider_api_key_ids(provider_api_key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::users::StoredUserSummary>, GatewayError> {
        self.data
            .list_users_by_ids(user_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}
