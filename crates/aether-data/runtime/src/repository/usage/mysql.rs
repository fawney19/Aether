use std::collections::BTreeMap;

use aether_data_contracts::repository::usage::{
    ProviderApiKeyWindowUsageRequest, StoredProviderApiKeyUsageSummary,
    StoredProviderApiKeyWindowUsageSummary, StoredProviderUsageSummary, StoredRequestUsageAudit,
    StoredUsageAuditAggregation, StoredUsageAuditSummary, StoredUsageBreakdownSummaryRow,
    StoredUsageCacheAffinityHitSummary, StoredUsageCacheAffinityIntervalRow,
    StoredUsageCacheHitSummary, StoredUsageCostSavingsSummary, StoredUsageDailyActualCostRollup,
    StoredUsageDailySummary, StoredUsageDashboardDailyBreakdownRow,
    StoredUsageDashboardProviderCount, StoredUsageDashboardSummary,
    StoredUsageErrorDistributionRow, StoredUsageLeaderboardSummary,
    StoredUsagePerformancePercentilesRow, StoredUsageProviderPerformance,
    StoredUsageSettledCostSummary, StoredUsageTimeSeriesBucket, StoredUsageUserTotals,
    UsageAuditAggregationQuery, UsageAuditKeywordSearchQuery, UsageAuditListQuery,
    UsageAuditSummaryQuery, UsageBreakdownSummaryQuery, UsageCacheAffinityHitSummaryQuery,
    UsageCacheAffinityIntervalQuery, UsageCacheHitSummaryQuery, UsageCostSavingsSummaryQuery,
    UsageDailyActualCostRollupQuery, UsageDailyHeatmapQuery, UsageDashboardDailyBreakdownQuery,
    UsageDashboardProviderCountsQuery, UsageDashboardSummaryQuery, UsageErrorDistributionQuery,
    UsageLeaderboardQuery, UsageMonitoringErrorCountQuery, UsageMonitoringErrorListQuery,
    UsagePerformancePercentilesQuery, UsageProviderPerformanceQuery, UsageReadRepository,
    UsageSettledCostSummaryQuery, UsageTimeSeriesQuery,
};

use super::InMemoryUsageReadRepository;
use crate::driver::mysql::MysqlPool;
use crate::DataLayerError;

pub use aether_data_mysql::MysqlUsageWriteRepository;
use aether_data_mysql::{MysqlUsageReadFilter, MysqlUsageStorage};

#[derive(Debug, Clone)]
pub struct MysqlUsageReadRepository {
    storage: MysqlUsageStorage,
}

impl MysqlUsageReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self {
            storage: MysqlUsageStorage::new(pool),
        }
    }

    async fn materialize_read_model(
        &self,
        filter: MysqlUsageReadFilter,
    ) -> Result<InMemoryUsageReadRepository, DataLayerError> {
        Ok(InMemoryUsageReadRepository::seed(
            self.storage.load_usage_records_in_range(&filter).await?,
        ))
    }

    fn range(created_from_unix_secs: u64, created_until_unix_secs: u64) -> MysqlUsageReadFilter {
        MysqlUsageReadFilter::new(created_from_unix_secs, created_until_unix_secs)
    }
}

#[async_trait::async_trait]
impl UsageReadRepository for MysqlUsageReadRepository {
    async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        self.storage.find_by_id(id).await
    }

    async fn list_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        self.storage.list_by_ids(ids).await
    }

    async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        self.storage.find_by_request_id(request_id).await
    }

    async fn resolve_body_ref(
        &self,
        body_ref: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        self.storage.resolve_body_ref(body_ref).await
    }

    async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        self.storage.list_usage_audits(query).await
    }

    async fn count_usage_audits(&self, query: &UsageAuditListQuery) -> Result<u64, DataLayerError> {
        self.storage.count_usage_audits(query).await
    }

    async fn list_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        self.storage
            .list_usage_audits_by_keyword_search(query)
            .await
    }

    async fn count_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<u64, DataLayerError> {
        self.storage
            .count_usage_audits_by_keyword_search(query)
            .await
    }

    async fn aggregate_usage_audits(
        &self,
        query: &UsageAuditAggregationQuery,
    ) -> Result<Vec<StoredUsageAuditAggregation>, DataLayerError> {
        let repository = self
            .materialize_read_model(Self::range(
                query.created_from_unix_secs,
                query.created_until_unix_secs,
            ))
            .await?;
        repository.aggregate_usage_audits(query).await
    }

    async fn summarize_usage_audits(
        &self,
        query: &UsageAuditSummaryQuery,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .with_provider_name(query.provider_name.as_deref())
            .with_model(query.model.as_deref());
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_usage_audits(query).await
    }

    async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUsageUserTotals>, DataLayerError> {
        self.storage
            .summarize_usage_totals_by_user_ids(user_ids)
            .await
    }

    async fn summarize_usage_cache_hit_summary(
        &self,
        query: &UsageCacheHitSummaryQuery,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref());
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_usage_cache_hit_summary(query).await
    }

    async fn summarize_usage_settled_cost(
        &self,
        query: &UsageSettledCostSummaryQuery,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .with_api_key_id(query.api_key_id.as_deref());
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_usage_settled_cost(query).await
    }

    async fn summarize_usage_daily_actual_cost_rollups(
        &self,
        query: &UsageDailyActualCostRollupQuery,
    ) -> Result<Vec<StoredUsageDailyActualCostRollup>, DataLayerError> {
        self.storage
            .summarize_usage_daily_actual_cost_rollups(query)
            .await
    }

    async fn summarize_usage_cache_affinity_hit_summary(
        &self,
        query: &UsageCacheAffinityHitSummaryQuery,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .with_api_key_id(query.api_key_id.as_deref())
            .completed_only();
        let repository = self.materialize_read_model(filter).await?;
        repository
            .summarize_usage_cache_affinity_hit_summary(query)
            .await
    }

    async fn list_usage_cache_affinity_intervals(
        &self,
        query: &UsageCacheAffinityIntervalQuery,
    ) -> Result<Vec<StoredUsageCacheAffinityIntervalRow>, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .with_api_key_id(query.api_key_id.as_deref())
            .completed_only();
        let repository = self.materialize_read_model(filter).await?;
        repository.list_usage_cache_affinity_intervals(query).await
    }

    async fn summarize_dashboard_usage(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(StoredUsageDashboardSummary::default());
        }
        if let Some(summary) = self
            .storage
            .summarize_dashboard_usage_from_daily_aggregates(query)
            .await?
        {
            return Ok(summary);
        }
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .finalized_only();
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_dashboard_usage(query).await
    }

    async fn list_dashboard_daily_breakdown(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        if query.created_from_unix_secs >= query.created_until_unix_secs {
            return Ok(Vec::new());
        }
        let rows = self
            .storage
            .list_dashboard_daily_breakdown_from_daily_aggregates(query)
            .await?;
        if !rows.is_empty() {
            return Ok(rows);
        }
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .finalized_only();
        let repository = self.materialize_read_model(filter).await?;
        repository.list_dashboard_daily_breakdown(query).await
    }

    async fn summarize_dashboard_provider_counts(
        &self,
        query: &UsageDashboardProviderCountsQuery,
    ) -> Result<Vec<StoredUsageDashboardProviderCount>, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .finalized_only();
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_dashboard_provider_counts(query).await
    }

    async fn summarize_usage_breakdown(
        &self,
        query: &UsageBreakdownSummaryQuery,
    ) -> Result<Vec<StoredUsageBreakdownSummaryRow>, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .with_provider_name(query.provider_name.as_deref())
            .with_model(query.model.as_deref())
            .with_api_format(query.api_format.as_deref())
            .finalized_only();
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_usage_breakdown(query).await
    }

    async fn count_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorCountQuery,
    ) -> Result<u64, DataLayerError> {
        self.storage.count_monitoring_usage_errors(query).await
    }

    async fn list_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        self.storage.list_monitoring_usage_errors(query).await
    }

    async fn summarize_usage_error_distribution(
        &self,
        query: &UsageErrorDistributionQuery,
    ) -> Result<Vec<StoredUsageErrorDistributionRow>, DataLayerError> {
        let repository = self
            .materialize_read_model(Self::range(
                query.created_from_unix_secs,
                query.created_until_unix_secs,
            ))
            .await?;
        repository.summarize_usage_error_distribution(query).await
    }

    async fn summarize_usage_performance_percentiles(
        &self,
        query: &UsagePerformancePercentilesQuery,
    ) -> Result<Vec<StoredUsagePerformancePercentilesRow>, DataLayerError> {
        let repository = self
            .materialize_read_model(
                Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
                    .completed_only(),
            )
            .await?;
        repository
            .summarize_usage_performance_percentiles(query)
            .await
    }

    async fn summarize_usage_provider_performance(
        &self,
        query: &UsageProviderPerformanceQuery,
    ) -> Result<StoredUsageProviderPerformance, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_provider_id(query.provider_id.as_deref())
            .with_model(query.model.as_deref())
            .with_api_format(query.api_format.as_deref())
            .with_endpoint_kind(query.endpoint_kind.as_deref())
            .with_is_stream(query.is_stream)
            .with_has_format_conversion(query.has_format_conversion)
            .finalized_only();
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_usage_provider_performance(query).await
    }

    async fn summarize_usage_cost_savings(
        &self,
        query: &UsageCostSavingsSummaryQuery,
    ) -> Result<StoredUsageCostSavingsSummary, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .with_provider_name(query.provider_name.as_deref())
            .with_model(query.model.as_deref());
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_usage_cost_savings(query).await
    }

    async fn summarize_usage_time_series(
        &self,
        query: &UsageTimeSeriesQuery,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .with_provider_name(query.provider_name.as_deref())
            .with_model(query.model.as_deref());
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_usage_time_series(query).await
    }

    async fn summarize_usage_leaderboard(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
        let filter = Self::range(query.created_from_unix_secs, query.created_until_unix_secs)
            .with_user_id(query.user_id.as_deref())
            .with_provider_name(query.provider_name.as_deref())
            .with_model(query.model.as_deref())
            .finalized_only();
        let repository = self.materialize_read_model(filter).await?;
        repository.summarize_usage_leaderboard(query).await
    }

    async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        self.storage.list_recent_usage_audits(user_id, limit).await
    }

    async fn summarize_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<BTreeMap<String, u64>, DataLayerError> {
        let repository = InMemoryUsageReadRepository::seed(
            self.storage
                .load_usage_records_by_api_key_ids(api_key_ids)
                .await?,
        );
        repository
            .summarize_total_tokens_by_api_key_ids(api_key_ids)
            .await
    }

    async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<BTreeMap<String, StoredProviderApiKeyUsageSummary>, DataLayerError> {
        let repository = InMemoryUsageReadRepository::seed(
            self.storage
                .load_usage_records_by_provider_api_key_ids(provider_api_key_ids)
                .await?,
        );
        repository
            .summarize_usage_by_provider_api_key_ids(provider_api_key_ids)
            .await
    }

    async fn summarize_usage_by_provider_api_key_windows(
        &self,
        requests: &[ProviderApiKeyWindowUsageRequest],
    ) -> Result<Vec<StoredProviderApiKeyWindowUsageSummary>, DataLayerError> {
        let repository = InMemoryUsageReadRepository::seed(
            self.storage
                .load_usage_records_by_provider_api_key_windows(requests)
                .await?,
        );
        repository
            .summarize_usage_by_provider_api_key_windows(requests)
            .await
    }

    async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, DataLayerError> {
        let repository = InMemoryUsageReadRepository::seed(
            self.storage
                .load_usage_records_for_provider_since(provider_id, since_unix_secs)
                .await?,
        );
        repository
            .summarize_provider_usage_since(provider_id, since_unix_secs)
            .await
    }

    async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        self.storage.summarize_usage_daily_heatmap(query).await
    }

    async fn read_usage_counter_health(
        &self,
    ) -> Result<aether_data_contracts::repository::usage::UsageCounterHealthSnapshot, DataLayerError>
    {
        self.storage.read_usage_counter_health().await
    }

    async fn read_usage_counter_pending_health(
        &self,
    ) -> Result<
        aether_data_contracts::repository::usage::UsageCounterPendingHealthSnapshot,
        DataLayerError,
    > {
        self.storage.read_usage_counter_pending_health().await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mysql_usage_reads_do_not_restore_the_unconditional_full_table_loader() {
        let source = include_str!("mysql.rs");
        let forbidden = ["load_usage_", "records()"].concat();
        assert!(!source.contains(&forbidden));
        assert!(source.contains("load_usage_records_in_range"));
        assert!(source.contains("MysqlUsageReadFilter::new"));
    }
}
