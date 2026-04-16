use super::AdminAppState;
use crate::GatewayError;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(
        &self,
        endpoint_ids: &[String],
        since_unix_secs: u64,
        until_unix_secs: u64,
        segments: u32,
    ) -> Result<
        Vec<aether_data_contracts::repository::candidates::PublicHealthTimelineBucket>,
        GatewayError,
    > {
        self.app
            .aggregate_finalized_request_candidate_timeline_by_endpoint_ids_since(
                endpoint_ids,
                since_unix_secs,
                until_unix_secs,
                segments,
            )
            .await
    }

    pub(crate) async fn read_recent_request_candidates(
        &self,
        limit: usize,
    ) -> Result<
        Vec<aether_data_contracts::repository::candidates::StoredRequestCandidate>,
        GatewayError,
    > {
        self.app.read_recent_request_candidates(limit).await
    }

    pub(crate) async fn list_usage_audits(
        &self,
        query: &aether_data_contracts::repository::usage::UsageAuditListQuery,
    ) -> Result<Vec<aether_data_contracts::repository::usage::StoredRequestUsageAudit>, GatewayError>
    {
        self.app.list_usage_audits(query).await
    }

    pub(crate) async fn count_usage_audits(
        &self,
        query: &aether_data_contracts::repository::usage::UsageAuditListQuery,
    ) -> Result<u64, GatewayError> {
        self.app.count_usage_audits(query).await
    }

    pub(crate) async fn aggregate_usage_audits(
        &self,
        query: &aether_data_contracts::repository::usage::UsageAuditAggregationQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageAuditAggregation>,
        GatewayError,
    > {
        self.app.aggregate_usage_audits(query).await
    }

    pub(crate) async fn summarize_usage_audits(
        &self,
        query: &aether_data_contracts::repository::usage::UsageAuditSummaryQuery,
    ) -> Result<aether_data_contracts::repository::usage::StoredUsageAuditSummary, GatewayError>
    {
        self.app.summarize_usage_audits(query).await
    }

    pub(crate) async fn summarize_usage_time_series(
        &self,
        query: &aether_data_contracts::repository::usage::UsageTimeSeriesQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageTimeSeriesBucket>,
        GatewayError,
    > {
        self.app.summarize_usage_time_series(query).await
    }

    pub(crate) async fn summarize_usage_leaderboard(
        &self,
        query: &aether_data_contracts::repository::usage::UsageLeaderboardQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::usage::StoredUsageLeaderboardSummary>,
        GatewayError,
    > {
        self.app.summarize_usage_leaderboard(query).await
    }

    pub(crate) async fn summarize_usage_daily_heatmap(
        &self,
        query: &aether_data_contracts::repository::usage::UsageDailyHeatmapQuery,
    ) -> Result<Vec<aether_data_contracts::repository::usage::StoredUsageDailySummary>, GatewayError>
    {
        self.app.summarize_usage_daily_heatmap(query).await
    }

    pub(crate) async fn find_request_usage_by_id(
        &self,
        usage_id: &str,
    ) -> Result<
        Option<aether_data_contracts::repository::usage::StoredRequestUsageAudit>,
        GatewayError,
    > {
        self.app
            .data
            .find_request_usage_by_id(usage_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn resolve_request_usage_body_ref(
        &self,
        body_ref: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        self.app
            .data
            .resolve_request_usage_body_ref(body_ref)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_admin_usage_for_range(
        &self,
        time_range: &crate::handlers::admin::observability::AdminStatsTimeRange,
        filters: &crate::handlers::admin::observability::AdminStatsUsageFilter,
    ) -> Result<Vec<aether_data_contracts::repository::usage::StoredRequestUsageAudit>, GatewayError>
    {
        crate::handlers::admin::observability::list_usage_for_range(self, time_range, filters).await
    }

    pub(crate) async fn list_admin_usage_for_optional_range(
        &self,
        time_range: Option<&crate::handlers::admin::observability::AdminStatsTimeRange>,
        filters: &crate::handlers::admin::observability::AdminStatsUsageFilter,
    ) -> Result<Vec<aether_data_contracts::repository::usage::StoredRequestUsageAudit>, GatewayError>
    {
        crate::handlers::admin::observability::list_usage_for_optional_range(
            self, time_range, filters,
        )
        .await
    }

    pub(crate) async fn build_api_format_health_monitor_payload(
        &self,
        lookback_hours: u64,
        per_format_limit: usize,
        options: crate::handlers::public::ApiFormatHealthMonitorOptions,
    ) -> Option<serde_json::Value> {
        crate::handlers::public::build_api_format_health_monitor_payload(
            self.app,
            lookback_hours,
            per_format_limit,
            options,
        )
        .await
    }

    pub(crate) async fn execute_execution_runtime_sync_plan(
        &self,
        trace_id: Option<&str>,
        plan: &aether_contracts::ExecutionPlan,
    ) -> Result<aether_contracts::ExecutionResult, GatewayError> {
        crate::execution_runtime::execute_execution_runtime_sync_plan(self.app, trace_id, plan)
            .await
    }
}
