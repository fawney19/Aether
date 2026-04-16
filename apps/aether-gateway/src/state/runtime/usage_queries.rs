use crate::{AppState, GatewayError};
use aether_data_contracts::repository::{candidates, usage};
use usage::{StoredUsageDailySummary, UsageDailyHeatmapQuery};

impl AppState {
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
