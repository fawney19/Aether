use aether_data_contracts::repository::usage::{
    StoredUsageDailySummary, StoredUsageDashboardDailyBreakdownRow, StoredUsageDashboardSummary,
    StoredUsageUserTotals, UsageDailyHeatmapQuery, UsageDashboardDailyBreakdownQuery,
    UsageDashboardSummaryQuery, UsageReadRepository,
};

use super::InMemoryUsageReadRepository;
use crate::driver::mysql::MysqlPool;
use crate::DataLayerError;

pub use aether_data_mysql::MysqlUsageWriteRepository;

#[derive(Debug, Clone)]
pub struct MysqlUsageReadRepository {
    storage: aether_data_mysql::MysqlUsageStorage,
}

impl MysqlUsageReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self {
            storage: aether_data_mysql::MysqlUsageStorage::new(pool),
        }
    }

    async fn materialize_read_model(&self) -> Result<InMemoryUsageReadRepository, DataLayerError> {
        Ok(InMemoryUsageReadRepository::seed(
            self.storage.load_usage_records().await?,
        ))
    }

    async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        self.storage.summarize_usage_daily_heatmap(query).await
    }

    async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUsageUserTotals>, DataLayerError> {
        self.storage
            .summarize_usage_totals_by_user_ids(user_ids)
            .await
    }

    async fn summarize_dashboard_usage(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        if let Some(summary) = self
            .storage
            .summarize_dashboard_usage_from_daily_aggregates(query)
            .await?
        {
            return Ok(summary);
        }
        let repository = self.materialize_read_model().await?;
        repository.summarize_dashboard_usage(query).await
    }

    async fn list_dashboard_daily_breakdown(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        let rows = self
            .storage
            .list_dashboard_daily_breakdown_from_daily_aggregates(query)
            .await?;
        if !rows.is_empty() {
            return Ok(rows);
        }
        let repository = self.materialize_read_model().await?;
        repository.list_dashboard_daily_breakdown(query).await
    }
}

impl_materialized_usage_read_repository!(MysqlUsageReadRepository);
