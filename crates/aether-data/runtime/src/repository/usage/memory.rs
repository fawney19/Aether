use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::RwLock;

use aether_ai_formats::UPSTREAM_IS_STREAM_KEY;
use aether_data_contracts::repository::usage::{
    parse_usage_body_ref, usage_body_ref, StoredUsageAuditAggregation, StoredUsageAuditSummary,
    StoredUsageBreakdownSummaryRow, StoredUsageCacheAffinityHitSummary,
    StoredUsageCacheAffinityIntervalRow, StoredUsageCacheHitSummary, StoredUsageCostSavingsSummary,
    StoredUsageDashboardDailyBreakdownRow, StoredUsageDashboardProviderCount,
    StoredUsageDashboardSummary, StoredUsageErrorDistributionRow, StoredUsageLeaderboardSummary,
    StoredUsagePerformancePercentilesRow, StoredUsageProviderPerformance,
    StoredUsageProviderPerformanceProviderRow, StoredUsageProviderPerformanceSummary,
    StoredUsageProviderPerformanceTimelineRow, StoredUsageSettledCostSummary,
    StoredUsageTimeSeriesBucket, StoredUsageUserTotals, UsageAuditAggregationGroupBy,
    UsageAuditAggregationQuery, UsageAuditKeywordSearchQuery, UsageAuditSummaryQuery,
    UsageBodyCaptureState, UsageBodyField, UsageBreakdownGroupBy, UsageBreakdownSummaryQuery,
    UsageCacheAffinityHitSummaryQuery, UsageCacheAffinityIntervalGroupBy,
    UsageCacheAffinityIntervalQuery, UsageCacheHitSummaryQuery, UsageCostSavingsSummaryQuery,
    UsageDashboardDailyBreakdownQuery, UsageDashboardProviderCountsQuery,
    UsageDashboardSummaryQuery, UsageErrorDistributionQuery, UsageLeaderboardGroupBy,
    UsageLeaderboardQuery, UsageMonitoringErrorCountQuery, UsageMonitoringErrorListQuery,
    UsagePerformancePercentilesQuery, UsageProviderPerformanceQuery, UsageSettledCostSummaryQuery,
    UsageTimeSeriesGranularity, UsageTimeSeriesQuery, PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY,
    PROVIDER_REASONING_EFFORT_METADATA_KEY, PROVIDER_SERVICE_TIER_METADATA_KEY,
    REQUESTED_REASONING_EFFORT_METADATA_KEY,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

use super::{
    api_key_usage_contribution, provider_api_key_usage_contribution,
    strip_deprecated_usage_display_fields, usage_can_recover_terminal_failure,
    usage_request_metadata_client_family, ApiKeyUsageContribution, ApiKeyUsageDelta,
    ProviderApiKeyUsageContribution, ProviderApiKeyUsageDelta, ProviderApiKeyWindowUsageRequest,
    StoredProviderApiKeyUsageSummary, StoredProviderApiKeyWindowUsageSummary,
    StoredProviderUsageSummary, StoredProviderUsageWindow, StoredRequestUsageAudit,
    StoredUsageDailySummary, UpsertUsageRecord, UsageAuditListQuery, UsageDailyHeatmapQuery,
    UsageReadRepository, UsageWriteRepository,
};
use crate::repository::auth::InMemoryAuthApiKeySnapshotRepository;
use crate::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryUsageReadRepository {
    by_request_id: RwLock<BTreeMap<String, StoredRequestUsageAudit>>,
    detached_bodies: RwLock<BTreeMap<String, Value>>,
    provider_usage_windows: RwLock<Vec<StoredProviderUsageWindow>>,
    auth_api_keys: Option<Arc<InMemoryAuthApiKeySnapshotRepository>>,
    provider_catalog: Option<Arc<InMemoryProviderCatalogReadRepository>>,
}

impl InMemoryUsageReadRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredRequestUsageAudit>,
    {
        let mut by_request_id = BTreeMap::new();
        for mut item in items {
            hydrate_legacy_body_refs(&mut item);
            hydrate_client_family(&mut item);
            by_request_id.insert(item.request_id.clone(), item);
        }
        Self {
            by_request_id: RwLock::new(by_request_id),
            detached_bodies: RwLock::new(BTreeMap::new()),
            provider_usage_windows: RwLock::new(Vec::new()),
            auth_api_keys: None,
            provider_catalog: None,
        }
    }

    pub fn seed_with_detached_bodies<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredRequestUsageAudit>,
    {
        let mut by_request_id = BTreeMap::new();
        let mut detached_bodies = BTreeMap::new();
        for mut item in items {
            hydrate_legacy_body_refs(&mut item);
            hydrate_client_family(&mut item);
            let request_id = item.request_id.clone();
            if let Some(body_ref) = detach_usage_body(
                &request_id,
                &mut item.request_body,
                &mut detached_bodies,
                UsageBodyField::RequestBody,
            ) {
                item.request_body_ref = Some(body_ref);
            }
            if let Some(body_ref) = detach_usage_body(
                &request_id,
                &mut item.provider_request_body,
                &mut detached_bodies,
                UsageBodyField::ProviderRequestBody,
            ) {
                item.provider_request_body_ref = Some(body_ref);
            }
            if let Some(body_ref) = detach_usage_body(
                &request_id,
                &mut item.response_body,
                &mut detached_bodies,
                UsageBodyField::ResponseBody,
            ) {
                item.response_body_ref = Some(body_ref);
            }
            if let Some(body_ref) = detach_usage_body(
                &request_id,
                &mut item.client_response_body,
                &mut detached_bodies,
                UsageBodyField::ClientResponseBody,
            ) {
                item.client_response_body_ref = Some(body_ref);
            }
            by_request_id.insert(request_id, item);
        }
        Self {
            by_request_id: RwLock::new(by_request_id),
            detached_bodies: RwLock::new(detached_bodies),
            provider_usage_windows: RwLock::new(Vec::new()),
            auth_api_keys: None,
            provider_catalog: None,
        }
    }

    pub fn with_provider_usage_windows<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredProviderUsageWindow>,
    {
        Self {
            by_request_id: self.by_request_id,
            detached_bodies: self.detached_bodies,
            provider_usage_windows: RwLock::new(items.into_iter().collect()),
            auth_api_keys: self.auth_api_keys,
            provider_catalog: self.provider_catalog,
        }
    }

    pub fn with_auth_api_key_repository(
        mut self,
        repository: Arc<InMemoryAuthApiKeySnapshotRepository>,
    ) -> Self {
        self.auth_api_keys = Some(repository);
        self
    }

    pub fn with_provider_catalog_repository(
        mut self,
        repository: Arc<InMemoryProviderCatalogReadRepository>,
    ) -> Self {
        self.provider_catalog = Some(repository);
        self
    }
}

fn usage_status_is_finalized(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

fn usage_status_is_lifecycle(status: &str) -> bool {
    matches!(status, "pending" | "streaming")
}

fn merge_usage_timing(existing: Option<u64>, incoming: Option<u64>) -> Option<u64> {
    match incoming {
        Some(0) | None => existing.or(incoming),
        Some(value) => Some(value),
    }
}

fn accumulate_provider_api_key_usage_contribution(
    aggregates: &mut BTreeMap<String, ProviderApiKeyUsageContribution>,
    contribution: ProviderApiKeyUsageContribution,
) {
    let entry = aggregates
        .entry(contribution.key_id.clone())
        .or_insert_with(|| ProviderApiKeyUsageContribution {
            key_id: contribution.key_id.clone(),
            ..ProviderApiKeyUsageContribution::default()
        });
    entry.request_count = entry
        .request_count
        .saturating_add(contribution.request_count);
    entry.success_count = entry
        .success_count
        .saturating_add(contribution.success_count);
    entry.error_count = entry.error_count.saturating_add(contribution.error_count);
    entry.total_tokens = entry.total_tokens.saturating_add(contribution.total_tokens);
    entry.total_cost_usd += contribution.total_cost_usd;
    entry.total_response_time_ms = entry
        .total_response_time_ms
        .saturating_add(contribution.total_response_time_ms);
    entry.last_used_at_unix_secs = match (
        entry.last_used_at_unix_secs,
        contribution.last_used_at_unix_secs,
    ) {
        (Some(existing), Some(candidate)) => Some(existing.max(candidate)),
        (None, Some(candidate)) => Some(candidate),
        (existing, None) => existing,
    };
}

fn accumulate_api_key_usage_contribution(
    aggregates: &mut BTreeMap<String, ApiKeyUsageContribution>,
    contribution: ApiKeyUsageContribution,
) {
    let entry = aggregates
        .entry(contribution.api_key_id.clone())
        .or_insert_with(|| ApiKeyUsageContribution {
            api_key_id: contribution.api_key_id.clone(),
            ..ApiKeyUsageContribution::default()
        });
    entry.total_requests = entry
        .total_requests
        .saturating_add(contribution.total_requests);
    entry.total_tokens = entry.total_tokens.saturating_add(contribution.total_tokens);
    entry.total_cost_usd += contribution.total_cost_usd;
    entry.last_used_at_unix_secs = match (
        entry.last_used_at_unix_secs,
        contribution.last_used_at_unix_secs,
    ) {
        (Some(existing), Some(candidate)) => Some(existing.max(candidate)),
        (None, Some(candidate)) => Some(candidate),
        (existing, None) => existing,
    };
}

fn usage_audit_client_family(item: &StoredRequestUsageAudit) -> Option<&str> {
    item.client_family
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| usage_request_metadata_client_family(item.request_metadata.as_ref()))
}

fn usage_admin_unknown_label(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "unknown" | "unknow"
    )
}

fn usage_has_admin_unknown_model_or_provider(item: &StoredRequestUsageAudit) -> bool {
    usage_admin_unknown_label(&item.model) || usage_admin_unknown_label(&item.provider_name)
}

fn usage_matches_list_query(item: &StoredRequestUsageAudit, query: &UsageAuditListQuery) -> bool {
    // The field is historically named `created_at_unix_ms`, but usage audit rows
    // across gateway handlers, SQL repositories and tests are stored as epoch seconds.
    if let Some(created_from_unix_secs) = query.created_from_unix_secs {
        if item.created_at_unix_ms < created_from_unix_secs {
            return false;
        }
    }
    if let Some(created_until_unix_secs) = query.created_until_unix_secs {
        if item.created_at_unix_ms >= created_until_unix_secs {
            return false;
        }
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        if item.provider_name != provider_name {
            return false;
        }
    }
    if let Some(model) = query.model.as_deref() {
        if item.model != model {
            return false;
        }
    }
    if let Some(api_format) = query.api_format.as_deref() {
        if item.api_format.as_deref() != Some(api_format) {
            return false;
        }
    }
    if let Some(client_family) = query
        .client_family
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !usage_audit_client_family(item)
            .is_some_and(|value| value.eq_ignore_ascii_case(client_family))
        {
            return false;
        }
    }
    if query.exclude_unknown_model_or_provider && usage_has_admin_unknown_model_or_provider(item) {
        return false;
    }
    if let Some(statuses) = query.statuses.as_ref() {
        if !statuses.iter().any(|status| status == &item.status) {
            return false;
        }
    }
    if item
        .status_code
        .is_some_and(|status_code| query.exclude_status_codes.contains(&status_code))
    {
        return false;
    }
    if let Some(is_stream) = query.is_stream {
        if item.is_stream != is_stream {
            return false;
        }
    }
    if query.error_only
        && item.status != "failed"
        && item.status_code.unwrap_or_default() < 400
        && item
            .error_message
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        return false;
    }

    true
}

fn usage_matches_keyword_search_query(
    item: &StoredRequestUsageAudit,
    query: &UsageAuditKeywordSearchQuery,
) -> bool {
    if let Some(created_from_unix_secs) = query.created_from_unix_secs {
        if item.created_at_unix_ms < created_from_unix_secs {
            return false;
        }
    }
    if let Some(created_until_unix_secs) = query.created_until_unix_secs {
        if item.created_at_unix_ms >= created_until_unix_secs {
            return false;
        }
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        if item.provider_name != provider_name {
            return false;
        }
    }
    if let Some(model) = query.model.as_deref() {
        if item.model != model {
            return false;
        }
    }
    if let Some(api_format) = query.api_format.as_deref() {
        if item.api_format.as_deref() != Some(api_format) {
            return false;
        }
    }
    if let Some(client_family) = query
        .client_family
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !usage_audit_client_family(item)
            .is_some_and(|value| value.eq_ignore_ascii_case(client_family))
        {
            return false;
        }
    }
    if query.exclude_unknown_model_or_provider && usage_has_admin_unknown_model_or_provider(item) {
        return false;
    }
    if let Some(statuses) = query.statuses.as_ref() {
        if !statuses.iter().any(|status| status == &item.status) {
            return false;
        }
    }
    if item
        .status_code
        .is_some_and(|status_code| query.exclude_status_codes.contains(&status_code))
    {
        return false;
    }
    if let Some(is_stream) = query.is_stream {
        if item.is_stream != is_stream {
            return false;
        }
    }
    if query.error_only
        && item.status != "failed"
        && item.status_code.unwrap_or_default() < 400
        && item
            .error_message
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    {
        return false;
    }

    let model = item.model.to_ascii_lowercase();
    let provider = item.provider_name.to_ascii_lowercase();
    let legacy_username = item
        .username
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let legacy_api_key_name = item
        .api_key_name
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    query.keywords.iter().enumerate().all(|(index, keyword)| {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            return true;
        }
        if model.contains(keyword) || provider.contains(keyword) {
            return true;
        }
        if query.auth_user_reader_available {
            let mut matched_user_ids = query
                .matched_user_ids_by_keyword
                .get(index)
                .into_iter()
                .flatten();
            if item
                .user_id
                .as_ref()
                .is_some_and(|user_id| matched_user_ids.any(|candidate| candidate == user_id))
            {
                return true;
            }
        } else if legacy_username.contains(keyword) {
            return true;
        }
        if query.auth_api_key_reader_available {
            let mut matched_ids = query
                .matched_api_key_ids_by_keyword
                .get(index)
                .into_iter()
                .flatten();
            item.api_key_id
                .as_ref()
                .is_some_and(|api_key_id| matched_ids.any(|candidate| candidate == api_key_id))
        } else {
            legacy_api_key_name.contains(keyword)
        }
    }) && match query.username_keyword.as_deref().map(str::trim) {
        Some(username_keyword) if !username_keyword.is_empty() => {
            let username_keyword = username_keyword.to_ascii_lowercase();
            if query.auth_user_reader_available {
                item.user_id.as_ref().is_some_and(|user_id| {
                    query
                        .matched_user_ids_for_username
                        .iter()
                        .any(|candidate| candidate == user_id)
                })
            } else {
                legacy_username.contains(&username_keyword)
            }
        }
        _ => true,
    }
}

fn usage_matches_summary_query(
    item: &StoredRequestUsageAudit,
    query: &UsageAuditSummaryQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        if item.provider_name != provider_name {
            return false;
        }
    }
    if let Some(model) = query.model.as_deref() {
        if item.model != model {
            return false;
        }
    }
    true
}

fn usage_matches_time_series_query(
    item: &StoredRequestUsageAudit,
    query: &UsageTimeSeriesQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        if item.provider_name != provider_name {
            return false;
        }
    }
    if let Some(model) = query.model.as_deref() {
        if item.model != model {
            return false;
        }
    }
    true
}

fn usage_matches_dashboard_summary_query(
    item: &StoredRequestUsageAudit,
    query: &UsageDashboardSummaryQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
        || matches!(item.status.as_str(), "pending" | "streaming")
        || matches!(item.provider_name.as_str(), "unknown" | "pending")
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    true
}

fn usage_matches_dashboard_daily_breakdown_query(
    item: &StoredRequestUsageAudit,
    query: &UsageDashboardDailyBreakdownQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
        || matches!(item.status.as_str(), "pending" | "streaming")
        || matches!(item.provider_name.as_str(), "unknown" | "pending")
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    true
}

fn usage_matches_dashboard_provider_counts_query(
    item: &StoredRequestUsageAudit,
    query: &UsageDashboardProviderCountsQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
        || matches!(item.status.as_str(), "pending" | "streaming")
        || matches!(item.provider_name.as_str(), "unknown" | "pending")
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    true
}

fn usage_matches_breakdown_summary_query(
    item: &StoredRequestUsageAudit,
    query: &UsageBreakdownSummaryQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
        || matches!(item.status.as_str(), "pending" | "streaming")
        || matches!(item.provider_name.as_str(), "unknown" | "pending")
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        if item.provider_name != provider_name {
            return false;
        }
    }
    if let Some(model) = query.model.as_deref() {
        if item.model != model {
            return false;
        }
    }
    if let Some(api_format) = query.api_format.as_deref() {
        if item.api_format.as_deref() != Some(api_format) {
            return false;
        }
    }
    if item
        .status_code
        .is_some_and(|status_code| query.exclude_status_codes.contains(&status_code))
    {
        return false;
    }
    match query.group_by {
        UsageBreakdownGroupBy::Model | UsageBreakdownGroupBy::Provider => true,
        UsageBreakdownGroupBy::ApiFormat => item.api_format.is_some(),
    }
}

fn usage_is_monitoring_error(item: &StoredRequestUsageAudit) -> bool {
    let status = item.status.trim();
    status.eq_ignore_ascii_case("error")
        || status.eq_ignore_ascii_case("failed")
        || item
            .error_category
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        || (status.is_empty()
            && (item.status_code.unwrap_or_default() >= 400
                || item
                    .error_message
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty())))
}

fn usage_matches_monitoring_error_count_query(
    item: &StoredRequestUsageAudit,
    query: &UsageMonitoringErrorCountQuery,
) -> bool {
    item.created_at_unix_ms >= query.created_from_unix_secs
        && item.created_at_unix_ms < query.created_until_unix_secs
        && usage_is_monitoring_error(item)
}

fn usage_matches_monitoring_error_list_query(
    item: &StoredRequestUsageAudit,
    query: &UsageMonitoringErrorListQuery,
) -> bool {
    item.created_at_unix_ms >= query.created_from_unix_secs
        && item.created_at_unix_ms < query.created_until_unix_secs
        && usage_is_monitoring_error(item)
}

fn usage_matches_error_distribution_query(
    item: &StoredRequestUsageAudit,
    query: &UsageErrorDistributionQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
    {
        return false;
    }
    item.error_category
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn usage_matches_performance_percentiles_query(
    item: &StoredRequestUsageAudit,
    query: &UsagePerformancePercentilesQuery,
) -> bool {
    item.created_at_unix_ms >= query.created_from_unix_secs
        && item.created_at_unix_ms < query.created_until_unix_secs
        && item.status == "completed"
}

fn usage_reserved_provider_label(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "unknown" | "unknow" | "pending"
    )
}

fn usage_provider_performance_identity(item: &StoredRequestUsageAudit) -> Option<(String, String)> {
    let provider_id = item.provider_id.as_deref()?.trim();
    if provider_id.is_empty() || usage_reserved_provider_label(provider_id) {
        return None;
    }
    let provider_name = item.provider_name.trim();
    if usage_reserved_provider_label(provider_name) {
        return None;
    }
    let display_name = if provider_name.is_empty() {
        provider_id
    } else {
        provider_name
    };
    Some((provider_id.to_string(), display_name.to_string()))
}

fn usage_matches_provider_performance_query(
    item: &StoredRequestUsageAudit,
    query: &UsageProviderPerformanceQuery,
) -> Option<(String, String)> {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
        || matches!(item.status.as_str(), "pending" | "streaming")
    {
        return None;
    }
    if let Some(provider_id) = query.provider_id.as_deref() {
        if item.provider_id.as_deref() != Some(provider_id) {
            return None;
        }
    }
    if let Some(model) = query.model.as_deref() {
        if item.model != model {
            return None;
        }
    }
    if let Some(api_format) = query.api_format.as_deref() {
        if item.api_format.as_deref() != Some(api_format) {
            return None;
        }
    }
    if let Some(endpoint_kind) = query.endpoint_kind.as_deref() {
        if item.endpoint_kind.as_deref() != Some(endpoint_kind) {
            return None;
        }
    }
    if let Some(is_stream) = query.is_stream {
        if item.is_stream != is_stream {
            return None;
        }
    }
    if let Some(has_format_conversion) = query.has_format_conversion {
        if item.has_format_conversion != has_format_conversion {
            return None;
        }
    }
    usage_provider_performance_identity(item)
}

fn usage_matches_cost_savings_query(
    item: &StoredRequestUsageAudit,
    query: &UsageCostSavingsSummaryQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        if item.provider_name != provider_name {
            return false;
        }
    }
    if let Some(model) = query.model.as_deref() {
        if item.model != model {
            return false;
        }
    }
    true
}

fn usage_matches_cache_affinity_hit_summary_query(
    item: &StoredRequestUsageAudit,
    query: &UsageCacheAffinityHitSummaryQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
        || item.status != "completed"
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    if let Some(api_key_id) = query.api_key_id.as_deref() {
        if item.api_key_id.as_deref() != Some(api_key_id) {
            return false;
        }
    }
    true
}

fn usage_matches_cache_affinity_interval_query(
    item: &StoredRequestUsageAudit,
    query: &UsageCacheAffinityIntervalQuery,
) -> Option<String> {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
        || item.status != "completed"
    {
        return None;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return None;
        }
    }
    if let Some(api_key_id) = query.api_key_id.as_deref() {
        if item.api_key_id.as_deref() != Some(api_key_id) {
            return None;
        }
    }
    match query.group_by {
        UsageCacheAffinityIntervalGroupBy::User => item.user_id.clone(),
        UsageCacheAffinityIntervalGroupBy::ApiKey => item.api_key_id.clone(),
    }
}

fn usage_dashboard_local_date(
    item: &StoredRequestUsageAudit,
    tz_offset_minutes: i32,
) -> Option<String> {
    let timestamp =
        chrono::DateTime::<Utc>::from_timestamp(i64::try_from(item.created_at_unix_ms).ok()?, 0)?;
    let local =
        timestamp.checked_add_signed(chrono::Duration::minutes(i64::from(tz_offset_minutes)))?;
    Some(local.date_naive().to_string())
}

fn usage_percentile_cont(values: &mut [u64], percentile: f64) -> Option<u64> {
    if values.len() < 10 {
        return None;
    }
    values.sort_unstable();
    let position = percentile * (values.len().saturating_sub(1)) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let lower_value = values[lower] as f64;
    let upper_value = values[upper] as f64;
    Some((lower_value + (upper_value - lower_value) * (position - lower as f64)).trunc() as u64)
}

fn usage_time_series_bucket_key(
    item: &StoredRequestUsageAudit,
    granularity: UsageTimeSeriesGranularity,
    tz_offset_minutes: i32,
) -> Option<String> {
    let timestamp =
        chrono::DateTime::<Utc>::from_timestamp(i64::try_from(item.created_at_unix_ms).ok()?, 0)?;
    let local =
        timestamp.checked_add_signed(chrono::Duration::minutes(i64::from(tz_offset_minutes)))?;
    Some(match granularity {
        UsageTimeSeriesGranularity::Day => local.date_naive().to_string(),
        UsageTimeSeriesGranularity::Hour => local.format("%Y-%m-%dT%H:00:00+00:00").to_string(),
    })
}

fn usage_matches_leaderboard_query(
    item: &StoredRequestUsageAudit,
    query: &UsageLeaderboardQuery,
) -> bool {
    if item.created_at_unix_ms < query.created_from_unix_secs
        || item.created_at_unix_ms >= query.created_until_unix_secs
        || matches!(item.status.as_str(), "pending" | "streaming")
        || matches!(item.provider_name.as_str(), "unknown" | "pending")
    {
        return false;
    }
    if let Some(user_id) = query.user_id.as_deref() {
        if item.user_id.as_deref() != Some(user_id) {
            return false;
        }
    }
    if let Some(provider_name) = query.provider_name.as_deref() {
        if item.provider_name != provider_name {
            return false;
        }
    }
    if let Some(model) = query.model.as_deref() {
        if item.model != model {
            return false;
        }
    }
    true
}

fn sort_usage_items(items: &mut [StoredRequestUsageAudit], newest_first: bool) {
    items.sort_by(|left, right| {
        let created_order = if newest_first {
            right.created_at_unix_ms.cmp(&left.created_at_unix_ms)
        } else {
            left.created_at_unix_ms.cmp(&right.created_at_unix_ms)
        };
        if newest_first {
            created_order.then_with(|| left.id.cmp(&right.id))
        } else {
            created_order.then_with(|| left.request_id.cmp(&right.request_id))
        }
    });
}

fn usage_cache_creation_tokens(item: &StoredRequestUsageAudit) -> u64 {
    let classified = item
        .cache_creation_ephemeral_5m_input_tokens
        .saturating_add(item.cache_creation_ephemeral_1h_input_tokens);
    if item.cache_creation_input_tokens == 0 && classified > 0 {
        classified
    } else {
        item.cache_creation_input_tokens
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UsageApiFamily {
    OpenAi,
    Claude,
    Gemini,
    Unknown,
}

fn usage_api_family(api_format: Option<&str>) -> UsageApiFamily {
    let Some(api_format) = api_format else {
        return UsageApiFamily::Unknown;
    };
    let family = api_format
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match family.as_str() {
        "openai" => UsageApiFamily::OpenAi,
        "claude" | "anthropic" => UsageApiFamily::Claude,
        "gemini" | "google" => UsageApiFamily::Gemini,
        _ => UsageApiFamily::Unknown,
    }
}

fn normalize_usage_input_tokens(
    api_format: Option<&str>,
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> i64 {
    if input_tokens <= 0 {
        return input_tokens.max(0);
    }
    if cache_creation_tokens <= 0 && cache_read_tokens <= 0 {
        return input_tokens;
    }

    match usage_api_family(api_format) {
        UsageApiFamily::OpenAi => input_tokens
            .saturating_sub(cache_creation_tokens.max(0))
            .saturating_sub(cache_read_tokens.max(0))
            .max(0),
        UsageApiFamily::Gemini => (input_tokens - cache_read_tokens).max(0),
        UsageApiFamily::Claude | UsageApiFamily::Unknown => input_tokens,
    }
}

fn normalize_usage_total_input_context(
    api_format: Option<&str>,
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> i64 {
    let normalized_input_tokens = input_tokens.max(0);
    let normalized_cache_creation_tokens = cache_creation_tokens.max(0);
    let normalized_cache_read_tokens = cache_read_tokens.max(0);

    let fresh_input_tokens = match usage_api_family(api_format) {
        UsageApiFamily::Claude => {
            normalized_input_tokens.saturating_add(normalized_cache_creation_tokens)
        }
        UsageApiFamily::OpenAi => normalize_usage_input_tokens(
            api_format,
            normalized_input_tokens,
            normalized_cache_creation_tokens,
            normalized_cache_read_tokens,
        )
        .saturating_add(normalized_cache_creation_tokens),
        UsageApiFamily::Gemini => normalize_usage_input_tokens(
            api_format,
            normalized_input_tokens,
            0,
            normalized_cache_read_tokens,
        ),
        UsageApiFamily::Unknown => {
            if normalized_cache_creation_tokens > 0 {
                normalized_input_tokens.saturating_add(normalized_cache_creation_tokens)
            } else {
                normalized_input_tokens
            }
        }
    };

    fresh_input_tokens.saturating_add(normalized_cache_read_tokens)
}

fn usage_total_input_context(item: &StoredRequestUsageAudit) -> u64 {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref());
    let input_tokens = i64::try_from(item.input_tokens).unwrap_or(i64::MAX);
    let cache_creation_tokens =
        i64::try_from(usage_cache_creation_tokens(item)).unwrap_or(i64::MAX);
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_usage_total_input_context(
        api_format,
        input_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    ) as u64
}

fn usage_effective_input_tokens(item: &StoredRequestUsageAudit) -> u64 {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref());
    let input_tokens = i64::try_from(item.input_tokens).unwrap_or(i64::MAX);
    let cache_creation_tokens =
        i64::try_from(usage_cache_creation_tokens(item)).unwrap_or(i64::MAX);
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_usage_input_tokens(
        api_format,
        input_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    ) as u64
}

fn usage_total_tokens(item: &StoredRequestUsageAudit) -> u64 {
    usage_effective_input_tokens(item)
        .saturating_add(item.output_tokens)
        .saturating_add(usage_cache_creation_tokens(item))
        .saturating_add(item.cache_read_input_tokens)
}

fn usage_is_success(item: &StoredRequestUsageAudit) -> bool {
    matches!(
        item.status.as_str(),
        "completed" | "success" | "ok" | "billed" | "settled"
    ) && item.status_code.is_none_or(|code| code < 400)
}

fn usage_output_tps_uses_generation_time(item: &StoredRequestUsageAudit) -> bool {
    item.request_metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(UPSTREAM_IS_STREAM_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(item.is_stream)
}

fn usage_output_tps_duration_ms(item: &StoredRequestUsageAudit) -> Option<u64> {
    let response_time_ms = item.response_time_ms?;
    if response_time_ms == 0 {
        return None;
    }

    if !usage_output_tps_uses_generation_time(item) {
        return Some(response_time_ms);
    }

    let first_byte_time_ms = item.first_byte_time_ms?;
    if first_byte_time_ms >= response_time_ms {
        return None;
    }
    Some(response_time_ms - first_byte_time_ms)
}

fn usage_provider_display_name(item: &StoredRequestUsageAudit) -> Option<String> {
    let provider_name = item.provider_name.trim();
    if provider_name.is_empty() || usage_reserved_provider_label(provider_name) {
        None
    } else {
        Some(provider_name.to_string())
    }
}

fn usage_provider_id(item: &StoredRequestUsageAudit) -> Option<String> {
    let provider_id = item.provider_id.as_deref()?.trim();
    if provider_id.is_empty() || usage_reserved_provider_label(provider_id) {
        None
    } else {
        Some(provider_id.to_string())
    }
}

fn usage_provider_aggregation_identity(
    item: &StoredRequestUsageAudit,
) -> Option<(String, Option<String>, String)> {
    let display_name = usage_provider_display_name(item);
    if let Some(provider_id) = usage_provider_id(item) {
        return Some((provider_id, display_name, "provider_id".to_string()));
    }
    let display_name = display_name?;
    Some((
        display_name.clone(),
        Some(display_name),
        "legacy_name".to_string(),
    ))
}

#[async_trait]
impl UsageReadRepository for InMemoryUsageReadRepository {
    async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        Ok(self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .find(|item| item.id == id)
            .cloned())
    }

    async fn list_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let requested_ids = ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        Ok(self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| requested_ids.contains(&item.id))
            .cloned()
            .collect())
    }

    async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        Ok(self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .get(request_id)
            .cloned())
    }

    async fn resolve_body_ref(&self, body_ref: &str) -> Result<Option<Value>, DataLayerError> {
        if let Some(value) = self
            .detached_bodies
            .read()
            .expect("usage repository lock")
            .get(body_ref)
            .cloned()
        {
            return Ok(Some(value));
        }
        let Some((request_id, field)) = parse_usage_body_ref(body_ref) else {
            return Ok(None);
        };
        let usage = self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .get(&request_id)
            .cloned();
        Ok(usage.and_then(|usage| match field {
            UsageBodyField::RequestBody => usage.request_body,
            UsageBodyField::ProviderRequestBody => usage.provider_request_body,
            UsageBodyField::ResponseBody => usage.response_body,
            UsageBodyField::ClientResponseBody => usage.client_response_body,
        }))
    }

    async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut items: Vec<_> = self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| usage_matches_list_query(item, query))
            .cloned()
            .collect();
        sort_usage_items(&mut items, query.newest_first);
        if let Some(offset) = query.offset {
            if offset >= items.len() {
                items.clear();
            } else {
                items.drain(..offset);
            }
        }
        if let Some(limit) = query.limit {
            items.truncate(limit);
        }
        Ok(items)
    }

    async fn list_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut items: Vec<_> = self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| usage_matches_keyword_search_query(item, query))
            .cloned()
            .collect();
        sort_usage_items(&mut items, query.newest_first);
        if let Some(offset) = query.offset {
            if offset >= items.len() {
                items.clear();
            } else {
                items.drain(..offset);
            }
        }
        if let Some(limit) = query.limit {
            items.truncate(limit);
        }
        Ok(items)
    }

    async fn count_usage_audits(&self, query: &UsageAuditListQuery) -> Result<u64, DataLayerError> {
        Ok(self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| usage_matches_list_query(item, query))
            .count() as u64)
    }

    async fn count_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<u64, DataLayerError> {
        Ok(self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| usage_matches_keyword_search_query(item, query))
            .count() as u64)
    }

    async fn aggregate_usage_audits(
        &self,
        query: &UsageAuditAggregationQuery,
    ) -> Result<Vec<StoredUsageAuditAggregation>, DataLayerError> {
        #[derive(Default)]
        struct AggregateBucket {
            display_name: Option<String>,
            secondary_name: Option<String>,
            request_count: u64,
            total_tokens: u64,
            output_tokens: u64,
            effective_input_tokens: u64,
            total_input_context: u64,
            cache_creation_tokens: u64,
            cache_creation_ephemeral_5m_tokens: u64,
            cache_creation_ephemeral_1h_tokens: u64,
            cache_read_tokens: u64,
            total_cost_usd: f64,
            actual_total_cost_usd: f64,
            response_time_ms_sum: u64,
            success_count: u64,
        }

        let mut grouped: BTreeMap<String, AggregateBucket> = BTreeMap::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if item.created_at_unix_ms < query.created_from_unix_secs
                || item.created_at_unix_ms >= query.created_until_unix_secs
                || matches!(item.status.as_str(), "pending" | "streaming")
                || (query.exclude_reserved_provider_labels
                    && usage_provider_aggregation_identity(item).is_none())
            {
                continue;
            }

            let provider_identity =
                if matches!(query.group_by, UsageAuditAggregationGroupBy::Provider) {
                    match usage_provider_aggregation_identity(item) {
                        Some(value) => Some(value),
                        None => continue,
                    }
                } else {
                    None
                };

            let group_key = match query.group_by {
                UsageAuditAggregationGroupBy::Model => item.model.clone(),
                UsageAuditAggregationGroupBy::Provider => provider_identity
                    .as_ref()
                    .expect("provider identity is set for provider aggregation")
                    .0
                    .clone(),
                UsageAuditAggregationGroupBy::ApiFormat => item
                    .api_format
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                UsageAuditAggregationGroupBy::User => match item.user_id.clone() {
                    Some(value) => value,
                    None => continue,
                },
            };
            let bucket = grouped.entry(group_key).or_default();
            if matches!(query.group_by, UsageAuditAggregationGroupBy::Provider)
                && (bucket.display_name.is_none()
                    || bucket.display_name.as_deref() == Some("Unknown"))
            {
                bucket.display_name = provider_identity
                    .as_ref()
                    .and_then(|(_, display_name, _)| display_name.clone());
            }
            if matches!(query.group_by, UsageAuditAggregationGroupBy::Provider)
                && (bucket.secondary_name.is_none()
                    || bucket.secondary_name.as_deref() == Some("legacy_name"))
            {
                bucket.secondary_name = provider_identity
                    .as_ref()
                    .map(|(_, _, identity_source)| identity_source.clone());
            }
            bucket.request_count = bucket.request_count.saturating_add(1);
            bucket.total_tokens = bucket.total_tokens.saturating_add(item.total_tokens);
            bucket.output_tokens = bucket.output_tokens.saturating_add(item.output_tokens);
            bucket.effective_input_tokens = bucket
                .effective_input_tokens
                .saturating_add(usage_effective_input_tokens(item));
            bucket.total_input_context = bucket
                .total_input_context
                .saturating_add(usage_total_input_context(item));
            bucket.cache_creation_tokens = bucket
                .cache_creation_tokens
                .saturating_add(usage_cache_creation_tokens(item));
            bucket.cache_creation_ephemeral_5m_tokens = bucket
                .cache_creation_ephemeral_5m_tokens
                .saturating_add(item.cache_creation_ephemeral_5m_input_tokens);
            bucket.cache_creation_ephemeral_1h_tokens = bucket
                .cache_creation_ephemeral_1h_tokens
                .saturating_add(item.cache_creation_ephemeral_1h_input_tokens);
            bucket.cache_read_tokens = bucket
                .cache_read_tokens
                .saturating_add(item.cache_read_input_tokens);
            bucket.total_cost_usd += item.total_cost_usd;
            bucket.actual_total_cost_usd += item.actual_total_cost_usd;
            bucket.response_time_ms_sum = bucket
                .response_time_ms_sum
                .saturating_add(item.response_time_ms.unwrap_or_default());
            bucket.success_count = bucket
                .success_count
                .saturating_add(if usage_is_success(item) { 1 } else { 0 });
        }

        let mut items = grouped
            .into_iter()
            .map(|(group_key, bucket)| StoredUsageAuditAggregation {
                group_key,
                display_name: bucket.display_name,
                secondary_name: bucket.secondary_name,
                request_count: bucket.request_count,
                total_tokens: bucket.total_tokens,
                output_tokens: bucket.output_tokens,
                effective_input_tokens: bucket.effective_input_tokens,
                total_input_context: bucket.total_input_context,
                cache_creation_tokens: bucket.cache_creation_tokens,
                cache_creation_ephemeral_5m_tokens: bucket.cache_creation_ephemeral_5m_tokens,
                cache_creation_ephemeral_1h_tokens: bucket.cache_creation_ephemeral_1h_tokens,
                cache_read_tokens: bucket.cache_read_tokens,
                total_cost_usd: bucket.total_cost_usd,
                actual_total_cost_usd: bucket.actual_total_cost_usd,
                avg_response_time_ms: match query.group_by {
                    UsageAuditAggregationGroupBy::Provider
                    | UsageAuditAggregationGroupBy::ApiFormat => {
                        Some(if bucket.request_count == 0 {
                            0.0
                        } else {
                            bucket.response_time_ms_sum as f64 / bucket.request_count as f64
                        })
                    }
                    _ => None,
                },
                success_count: match query.group_by {
                    UsageAuditAggregationGroupBy::Provider => Some(bucket.success_count),
                    _ => None,
                },
            })
            .collect::<Vec<_>>();

        items.sort_by(|left, right| {
            right
                .request_count
                .cmp(&left.request_count)
                .then_with(|| left.group_key.cmp(&right.group_key))
        });
        items.truncate(query.limit);
        Ok(items)
    }

    async fn summarize_usage_audits(
        &self,
        query: &UsageAuditSummaryQuery,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        let mut summary = StoredUsageAuditSummary::default();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_summary_query(item, query) {
                continue;
            }
            summary.total_requests = summary.total_requests.saturating_add(1);
            summary.input_tokens = summary.input_tokens.saturating_add(item.input_tokens);
            summary.output_tokens = summary.output_tokens.saturating_add(item.output_tokens);
            summary.recorded_total_tokens = summary
                .recorded_total_tokens
                .saturating_add(item.total_tokens);
            summary.cache_creation_tokens = summary
                .cache_creation_tokens
                .saturating_add(usage_cache_creation_tokens(item));
            summary.cache_creation_ephemeral_5m_tokens = summary
                .cache_creation_ephemeral_5m_tokens
                .saturating_add(item.cache_creation_ephemeral_5m_input_tokens);
            summary.cache_creation_ephemeral_1h_tokens = summary
                .cache_creation_ephemeral_1h_tokens
                .saturating_add(item.cache_creation_ephemeral_1h_input_tokens);
            summary.cache_read_tokens = summary
                .cache_read_tokens
                .saturating_add(item.cache_read_input_tokens);
            summary.total_cost_usd += item.total_cost_usd;
            summary.actual_total_cost_usd += item.actual_total_cost_usd;
            summary.cache_creation_cost_usd += item.cache_creation_cost_usd;
            summary.cache_read_cost_usd += item.cache_read_cost_usd;
            summary.total_response_time_ms += item.response_time_ms.unwrap_or(0) as f64;
            if item.status_code.is_some_and(|value| value >= 400) || item.error_message.is_some() {
                summary.error_requests = summary.error_requests.saturating_add(1);
            }
        }
        Ok(summary)
    }

    async fn summarize_usage_cache_hit_summary(
        &self,
        query: &UsageCacheHitSummaryQuery,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        let mut summary = StoredUsageCacheHitSummary::default();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if item.created_at_unix_ms < query.created_from_unix_secs
                || item.created_at_unix_ms >= query.created_until_unix_secs
            {
                continue;
            }
            if let Some(user_id) = query.user_id.as_deref() {
                if item.user_id.as_deref() != Some(user_id) {
                    continue;
                }
            }

            summary.total_requests = summary.total_requests.saturating_add(1);
            if item.cache_read_input_tokens > 0 {
                summary.cache_hit_requests = summary.cache_hit_requests.saturating_add(1);
            }
        }
        Ok(summary)
    }

    async fn summarize_usage_settled_cost(
        &self,
        query: &UsageSettledCostSummaryQuery,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        let mut summary = StoredUsageSettledCostSummary::default();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if item.created_at_unix_ms < query.created_from_unix_secs
                || item.created_at_unix_ms >= query.created_until_unix_secs
            {
                continue;
            }
            if let Some(user_id) = query.user_id.as_deref() {
                if item.user_id.as_deref() != Some(user_id) {
                    continue;
                }
            }
            if let Some(api_key_id) = query.api_key_id.as_deref() {
                if item.api_key_id.as_deref() != Some(api_key_id) {
                    continue;
                }
            }
            if item.billing_status != "settled" || item.total_cost_usd <= 0.0 {
                continue;
            }

            summary.total_cost_usd += item.total_cost_usd;
            summary.total_requests = summary.total_requests.saturating_add(1);
            summary.input_tokens = summary.input_tokens.saturating_add(item.input_tokens);
            summary.output_tokens = summary.output_tokens.saturating_add(item.output_tokens);
            summary.cache_creation_tokens = summary
                .cache_creation_tokens
                .saturating_add(item.cache_creation_input_tokens);
            summary.cache_read_tokens = summary
                .cache_read_tokens
                .saturating_add(item.cache_read_input_tokens);
            if let Some(finalized_at_unix_secs) = item.finalized_at_unix_secs {
                summary.first_finalized_at_unix_secs = Some(
                    summary
                        .first_finalized_at_unix_secs
                        .map(|value| value.min(finalized_at_unix_secs))
                        .unwrap_or(finalized_at_unix_secs),
                );
                summary.last_finalized_at_unix_secs = Some(
                    summary
                        .last_finalized_at_unix_secs
                        .map(|value| value.max(finalized_at_unix_secs))
                        .unwrap_or(finalized_at_unix_secs),
                );
            }
        }
        Ok(summary)
    }

    async fn summarize_dashboard_usage(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        let mut summary = StoredUsageDashboardSummary::default();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_dashboard_summary_query(item, query) {
                continue;
            }
            summary.total_requests = summary.total_requests.saturating_add(1);
            summary.input_tokens = summary.input_tokens.saturating_add(item.input_tokens);
            summary.effective_input_tokens = summary
                .effective_input_tokens
                .saturating_add(usage_effective_input_tokens(item));
            summary.output_tokens = summary.output_tokens.saturating_add(item.output_tokens);
            summary.total_tokens = summary
                .total_tokens
                .saturating_add(usage_total_tokens(item));
            summary.cache_creation_tokens = summary
                .cache_creation_tokens
                .saturating_add(usage_cache_creation_tokens(item));
            summary.cache_read_tokens = summary
                .cache_read_tokens
                .saturating_add(item.cache_read_input_tokens);
            summary.total_input_context = summary
                .total_input_context
                .saturating_add(usage_total_input_context(item));
            summary.cache_creation_cost_usd += item.cache_creation_cost_usd;
            summary.cache_read_cost_usd += item.cache_read_cost_usd;
            summary.total_cost_usd += item.total_cost_usd;
            summary.actual_total_cost_usd += item.actual_total_cost_usd;
            if item.status_code.is_some_and(|value| value >= 400)
                || item.status.eq_ignore_ascii_case("failed")
            {
                summary.error_requests = summary.error_requests.saturating_add(1);
            }
            if let Some(response_time_ms) = item.response_time_ms {
                summary.response_time_sum_ms += response_time_ms as f64;
                summary.response_time_samples = summary.response_time_samples.saturating_add(1);
            }
        }
        Ok(summary)
    }

    async fn list_dashboard_daily_breakdown(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        #[derive(Default)]
        struct DashboardDailyBucket {
            requests: u64,
            total_tokens: u64,
            total_cost_usd: f64,
            response_time_sum_ms: f64,
            response_time_samples: u64,
        }

        let mut grouped = BTreeMap::<(String, String, String), DashboardDailyBucket>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_dashboard_daily_breakdown_query(item, query) {
                continue;
            }
            let Some(date) = usage_dashboard_local_date(item, query.tz_offset_minutes) else {
                continue;
            };
            let key = (date, item.model.clone(), item.provider_name.clone());
            let bucket = grouped.entry(key).or_default();
            bucket.requests = bucket.requests.saturating_add(1);
            bucket.total_tokens = bucket.total_tokens.saturating_add(item.total_tokens);
            bucket.total_cost_usd += item.total_cost_usd;
            if let Some(response_time_ms) = item.response_time_ms {
                bucket.response_time_sum_ms += response_time_ms as f64;
                bucket.response_time_samples = bucket.response_time_samples.saturating_add(1);
            }
        }

        Ok(grouped
            .into_iter()
            .map(
                |((date, model, provider), bucket)| StoredUsageDashboardDailyBreakdownRow {
                    date,
                    model,
                    provider,
                    requests: bucket.requests,
                    total_tokens: bucket.total_tokens,
                    total_cost_usd: bucket.total_cost_usd,
                    response_time_sum_ms: bucket.response_time_sum_ms,
                    response_time_samples: bucket.response_time_samples,
                },
            )
            .collect())
    }

    async fn summarize_dashboard_provider_counts(
        &self,
        query: &UsageDashboardProviderCountsQuery,
    ) -> Result<Vec<StoredUsageDashboardProviderCount>, DataLayerError> {
        let mut grouped = BTreeMap::<String, u64>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_dashboard_provider_counts_query(item, query) {
                continue;
            }
            *grouped.entry(item.provider_name.clone()).or_default() += 1;
        }

        let mut items = grouped
            .into_iter()
            .map(
                |(provider_name, request_count)| StoredUsageDashboardProviderCount {
                    provider_name,
                    request_count,
                },
            )
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .request_count
                .cmp(&left.request_count)
                .then_with(|| left.provider_name.cmp(&right.provider_name))
        });
        Ok(items)
    }

    async fn summarize_usage_breakdown(
        &self,
        query: &UsageBreakdownSummaryQuery,
    ) -> Result<Vec<StoredUsageBreakdownSummaryRow>, DataLayerError> {
        #[derive(Default)]
        struct BreakdownBucket {
            request_count: u64,
            input_tokens: u64,
            total_tokens: u64,
            output_tokens: u64,
            effective_input_tokens: u64,
            total_input_context: u64,
            cache_creation_tokens: u64,
            cache_creation_ephemeral_5m_tokens: u64,
            cache_creation_ephemeral_1h_tokens: u64,
            cache_read_tokens: u64,
            total_cost_usd: f64,
            actual_total_cost_usd: f64,
            success_count: u64,
            response_time_sum_ms: f64,
            response_time_samples: u64,
            overall_response_time_sum_ms: f64,
            overall_response_time_samples: u64,
        }

        let mut grouped = BTreeMap::<String, BreakdownBucket>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_breakdown_summary_query(item, query) {
                continue;
            }
            let group_key = match query.group_by {
                UsageBreakdownGroupBy::Model => item.model.clone(),
                UsageBreakdownGroupBy::Provider => item.provider_name.clone(),
                UsageBreakdownGroupBy::ApiFormat => item.api_format.clone().unwrap_or_default(),
            };
            let bucket = grouped.entry(group_key).or_default();
            let is_success = item.status != "failed"
                && item.status_code.is_none_or(|status| status < 400)
                && item.error_message.is_none();

            bucket.request_count = bucket.request_count.saturating_add(1);
            bucket.input_tokens = bucket.input_tokens.saturating_add(item.input_tokens);
            bucket.total_tokens = bucket.total_tokens.saturating_add(item.total_tokens);
            bucket.output_tokens = bucket.output_tokens.saturating_add(item.output_tokens);
            bucket.effective_input_tokens = bucket
                .effective_input_tokens
                .saturating_add(usage_effective_input_tokens(item));
            bucket.total_input_context = bucket
                .total_input_context
                .saturating_add(usage_total_input_context(item));
            bucket.cache_creation_tokens = bucket
                .cache_creation_tokens
                .saturating_add(usage_cache_creation_tokens(item));
            bucket.cache_creation_ephemeral_5m_tokens = bucket
                .cache_creation_ephemeral_5m_tokens
                .saturating_add(item.cache_creation_ephemeral_5m_input_tokens);
            bucket.cache_creation_ephemeral_1h_tokens = bucket
                .cache_creation_ephemeral_1h_tokens
                .saturating_add(item.cache_creation_ephemeral_1h_input_tokens);
            bucket.cache_read_tokens = bucket
                .cache_read_tokens
                .saturating_add(item.cache_read_input_tokens);
            bucket.total_cost_usd += item.total_cost_usd;
            bucket.actual_total_cost_usd += item.actual_total_cost_usd;
            if let Some(response_time_ms) = item.response_time_ms {
                bucket.overall_response_time_sum_ms += response_time_ms as f64;
                bucket.overall_response_time_samples =
                    bucket.overall_response_time_samples.saturating_add(1);
            }
            if is_success {
                bucket.success_count = bucket.success_count.saturating_add(1);
                if let Some(response_time_ms) = item.response_time_ms {
                    bucket.response_time_sum_ms += response_time_ms as f64;
                    bucket.response_time_samples = bucket.response_time_samples.saturating_add(1);
                }
            }
        }

        let mut items = grouped
            .into_iter()
            .map(|(group_key, bucket)| StoredUsageBreakdownSummaryRow {
                group_key,
                request_count: bucket.request_count,
                input_tokens: bucket.input_tokens,
                total_tokens: bucket.total_tokens,
                output_tokens: bucket.output_tokens,
                effective_input_tokens: bucket.effective_input_tokens,
                total_input_context: bucket.total_input_context,
                cache_creation_tokens: bucket.cache_creation_tokens,
                cache_creation_ephemeral_5m_tokens: bucket.cache_creation_ephemeral_5m_tokens,
                cache_creation_ephemeral_1h_tokens: bucket.cache_creation_ephemeral_1h_tokens,
                cache_read_tokens: bucket.cache_read_tokens,
                total_cost_usd: bucket.total_cost_usd,
                actual_total_cost_usd: bucket.actual_total_cost_usd,
                success_count: bucket.success_count,
                response_time_sum_ms: bucket.response_time_sum_ms,
                response_time_samples: bucket.response_time_samples,
                overall_response_time_sum_ms: bucket.overall_response_time_sum_ms,
                overall_response_time_samples: bucket.overall_response_time_samples,
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .request_count
                .cmp(&left.request_count)
                .then_with(|| left.group_key.cmp(&right.group_key))
        });
        Ok(items)
    }

    async fn count_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorCountQuery,
    ) -> Result<u64, DataLayerError> {
        Ok(self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| usage_matches_monitoring_error_count_query(item, query))
            .count() as u64)
    }

    async fn list_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut items: Vec<_> = self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| usage_matches_monitoring_error_list_query(item, query))
            .cloned()
            .collect();
        sort_usage_items(&mut items, true);
        if let Some(limit) = query.limit {
            items.truncate(limit);
        }
        Ok(items)
    }

    async fn summarize_usage_error_distribution(
        &self,
        query: &UsageErrorDistributionQuery,
    ) -> Result<Vec<StoredUsageErrorDistributionRow>, DataLayerError> {
        let mut grouped = BTreeMap::<(String, String), u64>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_error_distribution_query(item, query) {
                continue;
            }
            let Some(date) = usage_dashboard_local_date(item, query.tz_offset_minutes) else {
                continue;
            };
            let Some(category) = item.error_category.clone() else {
                continue;
            };
            *grouped.entry((date, category)).or_default() += 1;
        }

        Ok(grouped
            .into_iter()
            .map(
                |((date, error_category), count)| StoredUsageErrorDistributionRow {
                    date,
                    error_category,
                    count,
                },
            )
            .collect())
    }

    async fn summarize_usage_performance_percentiles(
        &self,
        query: &UsagePerformancePercentilesQuery,
    ) -> Result<Vec<StoredUsagePerformancePercentilesRow>, DataLayerError> {
        let mut grouped = BTreeMap::<String, (Vec<u64>, Vec<u64>)>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_performance_percentiles_query(item, query) {
                continue;
            }
            let Some(date) = usage_dashboard_local_date(item, query.tz_offset_minutes) else {
                continue;
            };
            let entry = grouped.entry(date).or_default();
            if let Some(response_time_ms) = item.response_time_ms {
                entry.0.push(response_time_ms);
            }
            if let Some(first_byte_time_ms) = item.first_byte_time_ms {
                entry.1.push(first_byte_time_ms);
            }
        }

        Ok(grouped
            .into_iter()
            .map(|(date, (mut response_times, mut first_byte_times))| {
                StoredUsagePerformancePercentilesRow {
                    date,
                    p50_response_time_ms: usage_percentile_cont(&mut response_times, 0.5),
                    p90_response_time_ms: usage_percentile_cont(&mut response_times, 0.9),
                    p99_response_time_ms: usage_percentile_cont(&mut response_times, 0.99),
                    p50_first_byte_time_ms: usage_percentile_cont(&mut first_byte_times, 0.5),
                    p90_first_byte_time_ms: usage_percentile_cont(&mut first_byte_times, 0.9),
                    p99_first_byte_time_ms: usage_percentile_cont(&mut first_byte_times, 0.99),
                }
            })
            .collect())
    }

    async fn summarize_usage_provider_performance(
        &self,
        query: &UsageProviderPerformanceQuery,
    ) -> Result<StoredUsageProviderPerformance, DataLayerError> {
        #[derive(Default)]
        struct ProviderPerformanceBucket {
            provider: String,
            request_count: u64,
            success_count: u64,
            output_tokens: u64,
            tps_output_tokens: u64,
            tps_response_time_ms_sum: u64,
            tps_sample_count: u64,
            first_byte_time_ms_sum: u64,
            first_byte_sample_count: u64,
            response_time_ms_sum: u64,
            response_time_sample_count: u64,
            response_times: Vec<u64>,
            first_byte_times: Vec<u64>,
            slow_request_count: u64,
        }

        impl ProviderPerformanceBucket {
            fn add(&mut self, item: &StoredRequestUsageAudit, slow_threshold_ms: u64) {
                self.request_count = self.request_count.saturating_add(1);
                self.output_tokens = self.output_tokens.saturating_add(item.output_tokens);
                if item
                    .response_time_ms
                    .is_some_and(|value| value >= slow_threshold_ms)
                {
                    self.slow_request_count = self.slow_request_count.saturating_add(1);
                }
                if !usage_is_success(item) {
                    return;
                }

                self.success_count = self.success_count.saturating_add(1);
                if let Some(response_time_ms) = item.response_time_ms {
                    self.response_time_ms_sum =
                        self.response_time_ms_sum.saturating_add(response_time_ms);
                    self.response_time_sample_count =
                        self.response_time_sample_count.saturating_add(1);
                    self.response_times.push(response_time_ms);
                    if let Some(output_tps_duration_ms) = usage_output_tps_duration_ms(item) {
                        if item.output_tokens > 0 {
                            self.tps_output_tokens =
                                self.tps_output_tokens.saturating_add(item.output_tokens);
                            self.tps_response_time_ms_sum = self
                                .tps_response_time_ms_sum
                                .saturating_add(output_tps_duration_ms);
                            self.tps_sample_count = self.tps_sample_count.saturating_add(1);
                        }
                    }
                }
                if let Some(first_byte_time_ms) = item.first_byte_time_ms {
                    self.first_byte_time_ms_sum = self
                        .first_byte_time_ms_sum
                        .saturating_add(first_byte_time_ms);
                    self.first_byte_sample_count = self.first_byte_sample_count.saturating_add(1);
                    self.first_byte_times.push(first_byte_time_ms);
                }
            }
        }

        fn avg(sum: u64, samples: u64) -> Option<f64> {
            if samples == 0 {
                None
            } else {
                Some(sum as f64 / samples as f64)
            }
        }

        fn avg_tps(tokens: u64, response_time_ms_sum: u64) -> Option<f64> {
            if response_time_ms_sum == 0 {
                None
            } else {
                Some(tokens as f64 * 1000.0 / response_time_ms_sum as f64)
            }
        }

        let usage = self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();

        let mut grouped = BTreeMap::<String, ProviderPerformanceBucket>::new();
        let mut summary_bucket = ProviderPerformanceBucket::default();
        for item in &usage {
            let Some((provider_id, provider)) =
                usage_matches_provider_performance_query(item, query)
            else {
                continue;
            };
            summary_bucket.add(item, query.slow_threshold_ms);
            let bucket = grouped.entry(provider_id).or_default();
            if bucket.provider.is_empty() {
                bucket.provider = provider;
            }
            bucket.add(item, query.slow_threshold_ms);
        }

        let mut summary_response_times = summary_bucket.response_times.clone();
        let mut summary_first_byte_times = summary_bucket.first_byte_times.clone();
        let summary = StoredUsageProviderPerformanceSummary {
            request_count: summary_bucket.request_count,
            success_count: summary_bucket.success_count,
            avg_output_tps: avg_tps(
                summary_bucket.tps_output_tokens,
                summary_bucket.tps_response_time_ms_sum,
            ),
            avg_first_byte_time_ms: avg(
                summary_bucket.first_byte_time_ms_sum,
                summary_bucket.first_byte_sample_count,
            ),
            avg_response_time_ms: avg(
                summary_bucket.response_time_ms_sum,
                summary_bucket.response_time_sample_count,
            ),
            p90_response_time_ms: usage_percentile_cont(&mut summary_response_times, 0.9),
            p99_response_time_ms: usage_percentile_cont(&mut summary_response_times, 0.99),
            p90_first_byte_time_ms: usage_percentile_cont(&mut summary_first_byte_times, 0.9),
            p99_first_byte_time_ms: usage_percentile_cont(&mut summary_first_byte_times, 0.99),
            tps_sample_count: summary_bucket.tps_sample_count,
            response_time_sample_count: summary_bucket.response_time_sample_count,
            first_byte_sample_count: summary_bucket.first_byte_sample_count,
            slow_request_count: summary_bucket.slow_request_count,
        };

        let mut providers = grouped
            .into_iter()
            .map(|(provider_id, mut bucket)| {
                let p90_response_time_ms = usage_percentile_cont(&mut bucket.response_times, 0.9);
                let p99_response_time_ms = usage_percentile_cont(&mut bucket.response_times, 0.99);
                let p90_first_byte_time_ms =
                    usage_percentile_cont(&mut bucket.first_byte_times, 0.9);
                let p99_first_byte_time_ms =
                    usage_percentile_cont(&mut bucket.first_byte_times, 0.99);
                StoredUsageProviderPerformanceProviderRow {
                    provider_id,
                    provider: bucket.provider,
                    request_count: bucket.request_count,
                    success_count: bucket.success_count,
                    output_tokens: bucket.output_tokens,
                    avg_output_tps: avg_tps(
                        bucket.tps_output_tokens,
                        bucket.tps_response_time_ms_sum,
                    ),
                    avg_first_byte_time_ms: avg(
                        bucket.first_byte_time_ms_sum,
                        bucket.first_byte_sample_count,
                    ),
                    avg_response_time_ms: avg(
                        bucket.response_time_ms_sum,
                        bucket.response_time_sample_count,
                    ),
                    p90_response_time_ms,
                    p99_response_time_ms,
                    p90_first_byte_time_ms,
                    p99_first_byte_time_ms,
                    tps_sample_count: bucket.tps_sample_count,
                    response_time_sample_count: bucket.response_time_sample_count,
                    first_byte_sample_count: bucket.first_byte_sample_count,
                    slow_request_count: bucket.slow_request_count,
                }
            })
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| {
            right
                .request_count
                .cmp(&left.request_count)
                .then_with(|| left.provider_id.cmp(&right.provider_id))
        });
        providers.truncate(query.limit.max(1));

        let timeline = if query.include_timeline {
            let top_provider_ids = providers
                .iter()
                .map(|row| row.provider_id.clone())
                .collect::<Vec<_>>();
            let mut timeline_grouped =
                BTreeMap::<(String, String), ProviderPerformanceBucket>::new();
            for item in &usage {
                let Some((provider_id, provider)) =
                    usage_matches_provider_performance_query(item, query)
                else {
                    continue;
                };
                if !top_provider_ids.iter().any(|value| value == &provider_id) {
                    continue;
                }
                let Some(bucket_key) =
                    usage_time_series_bucket_key(item, query.granularity, query.tz_offset_minutes)
                else {
                    continue;
                };
                let bucket = timeline_grouped
                    .entry((bucket_key, provider_id))
                    .or_default();
                if bucket.provider.is_empty() {
                    bucket.provider = provider;
                }
                bucket.add(item, query.slow_threshold_ms);
            }

            timeline_grouped
                .into_iter()
                .map(
                    |((date, provider_id), bucket)| StoredUsageProviderPerformanceTimelineRow {
                        date,
                        provider_id,
                        provider: bucket.provider,
                        request_count: bucket.request_count,
                        success_count: bucket.success_count,
                        output_tokens: bucket.output_tokens,
                        avg_output_tps: avg_tps(
                            bucket.tps_output_tokens,
                            bucket.tps_response_time_ms_sum,
                        ),
                        avg_first_byte_time_ms: avg(
                            bucket.first_byte_time_ms_sum,
                            bucket.first_byte_sample_count,
                        ),
                        avg_response_time_ms: avg(
                            bucket.response_time_ms_sum,
                            bucket.response_time_sample_count,
                        ),
                        slow_request_count: bucket.slow_request_count,
                    },
                )
                .collect()
        } else {
            Vec::new()
        };

        Ok(StoredUsageProviderPerformance {
            summary,
            providers,
            timeline,
        })
    }

    async fn summarize_usage_cost_savings(
        &self,
        query: &UsageCostSavingsSummaryQuery,
    ) -> Result<StoredUsageCostSavingsSummary, DataLayerError> {
        let mut summary = StoredUsageCostSavingsSummary::default();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_cost_savings_query(item, query) {
                continue;
            }
            summary.cache_read_tokens = summary
                .cache_read_tokens
                .saturating_add(item.cache_read_input_tokens);
            summary.cache_read_cost_usd += item.cache_read_cost_usd;
            summary.cache_creation_cost_usd += item.cache_creation_cost_usd;
            summary.estimated_full_cost_usd += item.settlement_input_price_per_1m().unwrap_or(0.0)
                * item.cache_read_input_tokens as f64
                / 1_000_000.0;
        }
        Ok(summary)
    }

    async fn summarize_usage_cache_affinity_hit_summary(
        &self,
        query: &UsageCacheAffinityHitSummaryQuery,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        let mut summary = StoredUsageCacheAffinityHitSummary::default();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_cache_affinity_hit_summary_query(item, query) {
                continue;
            }
            summary.total_requests = summary.total_requests.saturating_add(1);
            summary.input_tokens = summary.input_tokens.saturating_add(item.input_tokens);
            summary.cache_read_tokens = summary
                .cache_read_tokens
                .saturating_add(item.cache_read_input_tokens);
            summary.cache_creation_tokens = summary
                .cache_creation_tokens
                .saturating_add(usage_cache_creation_tokens(item));
            summary.total_input_context = summary
                .total_input_context
                .saturating_add(usage_total_input_context(item));
            summary.cache_read_cost_usd += item.cache_read_cost_usd;
            summary.cache_creation_cost_usd += item.cache_creation_cost_usd;
            if item.cache_read_input_tokens > 0 {
                summary.requests_with_cache_hit = summary.requests_with_cache_hit.saturating_add(1);
            }
        }
        Ok(summary)
    }

    async fn list_usage_cache_affinity_intervals(
        &self,
        query: &UsageCacheAffinityIntervalQuery,
    ) -> Result<Vec<StoredUsageCacheAffinityIntervalRow>, DataLayerError> {
        let mut items: Vec<_> = self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter_map(|item| {
                usage_matches_cache_affinity_interval_query(item, query)
                    .map(|group_id| (group_id, item.clone()))
            })
            .collect();
        items.sort_by(|left, right| {
            left.1
                .created_at_unix_ms
                .cmp(&right.1.created_at_unix_ms)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });

        let mut previous_created_at_by_group = BTreeMap::<String, u64>::new();
        let mut rows = Vec::new();
        for (group_id, item) in items {
            let previous =
                previous_created_at_by_group.insert(group_id.clone(), item.created_at_unix_ms);
            let Some(previous_created_at_unix_secs) = previous else {
                continue;
            };
            rows.push(StoredUsageCacheAffinityIntervalRow {
                group_id,
                username: item.username.clone(),
                model: item.model.clone(),
                created_at_unix_secs: item.created_at_unix_ms,
                interval_minutes: item
                    .created_at_unix_ms
                    .saturating_sub(previous_created_at_unix_secs)
                    as f64
                    / 60.0,
            });
        }
        Ok(rows)
    }

    async fn summarize_usage_time_series(
        &self,
        query: &UsageTimeSeriesQuery,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        let mut buckets = BTreeMap::<String, StoredUsageTimeSeriesBucket>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_time_series_query(item, query) {
                continue;
            }
            let Some(bucket_key) =
                usage_time_series_bucket_key(item, query.granularity, query.tz_offset_minutes)
            else {
                continue;
            };
            let bucket =
                buckets
                    .entry(bucket_key.clone())
                    .or_insert_with(|| StoredUsageTimeSeriesBucket {
                        bucket_key,
                        ..Default::default()
                    });
            bucket.total_requests = bucket.total_requests.saturating_add(1);
            bucket.input_tokens = bucket.input_tokens.saturating_add(item.input_tokens);
            bucket.output_tokens = bucket.output_tokens.saturating_add(item.output_tokens);
            bucket.cache_creation_tokens = bucket
                .cache_creation_tokens
                .saturating_add(item.cache_creation_input_tokens);
            bucket.cache_read_tokens = bucket
                .cache_read_tokens
                .saturating_add(item.cache_read_input_tokens);
            bucket.total_cost_usd += item.total_cost_usd;
            bucket.total_response_time_ms += item.response_time_ms.unwrap_or(0) as f64;
        }
        Ok(buckets.into_values().collect())
    }

    async fn summarize_usage_leaderboard(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
        let mut grouped = BTreeMap::<String, StoredUsageLeaderboardSummary>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if !usage_matches_leaderboard_query(item, query) {
                continue;
            }
            let (group_key, legacy_name) = match query.group_by {
                UsageLeaderboardGroupBy::Model => (item.model.clone(), None),
                UsageLeaderboardGroupBy::User => match item.user_id.clone() {
                    Some(user_id) => (user_id, item.username.clone()),
                    None => continue,
                },
                UsageLeaderboardGroupBy::ApiKey => match item.api_key_id.clone() {
                    Some(api_key_id) => (api_key_id, item.api_key_name.clone()),
                    None => continue,
                },
            };
            let entry =
                grouped
                    .entry(group_key.clone())
                    .or_insert_with(|| StoredUsageLeaderboardSummary {
                        group_key,
                        legacy_name: legacy_name.clone(),
                        ..Default::default()
                    });
            if entry.legacy_name.is_none() {
                entry.legacy_name = legacy_name;
            }
            entry.request_count = entry.request_count.saturating_add(1);
            entry.total_tokens = entry.total_tokens.saturating_add(usage_total_tokens(item));
            entry.total_cost_usd += item.total_cost_usd;
        }
        Ok(grouped.into_values().collect())
    }

    async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut items: Vec<_> = self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| match user_id {
                Some(user_id) => item.user_id.as_deref() == Some(user_id),
                None => true,
            })
            .cloned()
            .collect();
        items.sort_by(|left, right| {
            right
                .created_at_unix_ms
                .cmp(&left.created_at_unix_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        items.truncate(limit);
        Ok(items)
    }

    async fn summarize_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<BTreeMap<String, u64>, DataLayerError> {
        let api_key_id_set = api_key_ids.iter().map(String::as_str).collect::<Vec<_>>();
        let mut totals = BTreeMap::<String, u64>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            let Some(api_key_id) = item.api_key_id.as_deref() else {
                continue;
            };
            if !api_key_id_set.contains(&api_key_id) {
                continue;
            }
            let entry = totals.entry(api_key_id.to_string()).or_insert(0);
            *entry = (*entry).saturating_add(item.total_tokens);
        }
        Ok(totals)
    }

    async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUsageUserTotals>, DataLayerError> {
        let user_id_set = user_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let mut totals = BTreeMap::<String, StoredUsageUserTotals>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            if matches!(item.status.as_str(), "pending" | "streaming")
                || matches!(item.provider_name.as_str(), "unknown" | "pending")
            {
                continue;
            }
            let Some(user_id) = item.user_id.as_deref() else {
                continue;
            };
            if !user_id_set.contains(user_id) {
                continue;
            }

            let entry =
                totals
                    .entry(user_id.to_string())
                    .or_insert_with(|| StoredUsageUserTotals {
                        user_id: user_id.to_string(),
                        ..Default::default()
                    });
            entry.request_count = entry.request_count.saturating_add(1);
            entry.total_tokens = entry.total_tokens.saturating_add(item.total_tokens);
        }
        Ok(totals.into_values().collect())
    }

    async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<BTreeMap<String, StoredProviderApiKeyUsageSummary>, DataLayerError> {
        let provider_api_key_id_set = provider_api_key_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let mut summaries = BTreeMap::<String, StoredProviderApiKeyUsageSummary>::new();
        for item in self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
        {
            let Some(provider_api_key_id) = item.provider_api_key_id.as_deref() else {
                continue;
            };
            if !provider_api_key_id_set.contains(&provider_api_key_id) {
                continue;
            }
            let entry = summaries
                .entry(provider_api_key_id.to_string())
                .or_insert_with(|| StoredProviderApiKeyUsageSummary {
                    provider_api_key_id: provider_api_key_id.to_string(),
                    ..StoredProviderApiKeyUsageSummary::default()
                });
            entry.request_count = entry.request_count.saturating_add(1);
            entry.total_tokens = entry.total_tokens.saturating_add(item.total_tokens);
            entry.total_cost_usd += item.total_cost_usd;
            entry.last_used_at_unix_secs = Some(
                entry
                    .last_used_at_unix_secs
                    .unwrap_or_default()
                    .max(item.created_at_unix_ms),
            );
        }
        Ok(summaries)
    }

    async fn summarize_usage_by_provider_api_key_windows(
        &self,
        requests: &[ProviderApiKeyWindowUsageRequest],
    ) -> Result<Vec<StoredProviderApiKeyWindowUsageSummary>, DataLayerError> {
        let usage = self.by_request_id.read().expect("usage repository lock");
        let mut summaries = Vec::with_capacity(requests.len());

        for request in requests {
            let provider_api_key_id = request.provider_api_key_id.trim();
            if provider_api_key_id.is_empty() {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage provider_api_key_id cannot be empty".to_string(),
                ));
            }
            let window_code = request.window_code.trim();
            if window_code.is_empty() {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage window_code cannot be empty".to_string(),
                ));
            }
            if request.start_unix_secs >= request.end_unix_secs {
                return Err(DataLayerError::InvalidInput(
                    "provider api key window usage range must be non-empty".to_string(),
                ));
            }

            let mut summary = StoredProviderApiKeyWindowUsageSummary {
                provider_api_key_id: provider_api_key_id.to_string(),
                window_code: window_code.to_string(),
                ..StoredProviderApiKeyWindowUsageSummary::default()
            };

            for item in usage.values() {
                if item.provider_api_key_id.as_deref() != Some(provider_api_key_id) {
                    continue;
                }
                if item.created_at_unix_ms < request.start_unix_secs
                    || item.created_at_unix_ms >= request.end_unix_secs
                {
                    continue;
                }

                summary.request_count = summary.request_count.saturating_add(1);
                summary.total_tokens = summary.total_tokens.saturating_add(item.total_tokens);
                summary.total_cost_usd += item.total_cost_usd;
            }

            summaries.push(summary);
        }

        Ok(summaries)
    }

    async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, DataLayerError> {
        let windows = self
            .provider_usage_windows
            .read()
            .expect("provider usage repository lock");

        let mut summary = StoredProviderUsageSummary::default();
        let mut response_time_samples = 0u64;
        for window in windows.iter().filter(|window| {
            window.provider_id == provider_id && window.window_start_unix_secs >= since_unix_secs
        }) {
            summary.total_requests = summary.total_requests.saturating_add(window.total_requests);
            summary.successful_requests = summary
                .successful_requests
                .saturating_add(window.successful_requests);
            summary.failed_requests = summary
                .failed_requests
                .saturating_add(window.failed_requests);
            summary.total_cost_usd += window.total_cost_usd;
            summary.avg_response_time_ms += window.avg_response_time_ms;
            response_time_samples = response_time_samples.saturating_add(1);
        }

        if response_time_samples > 0 {
            summary.avg_response_time_ms /= response_time_samples as f64;
        }

        Ok(summary)
    }

    async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        let items = self.by_request_id.read().expect("usage repository lock");
        let mut daily = BTreeMap::<String, (u64, u64, f64, f64)>::new();
        for item in items.values() {
            if item.created_at_unix_ms < query.created_from_unix_secs {
                continue;
            }
            if let Some(user_id) = &query.user_id {
                if item.user_id.as_deref() != Some(user_id) {
                    continue;
                }
            }
            if item.status == "pending" || item.status == "streaming" {
                continue;
            }
            if item.provider_name.eq_ignore_ascii_case("unknown")
                || item.provider_name.eq_ignore_ascii_case("pending")
            {
                continue;
            }
            let ts = i64::try_from(item.created_at_unix_ms).unwrap_or_default();
            let Some(dt) = chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0) else {
                continue;
            };
            let date_key = dt.date_naive().to_string();
            let entry = daily.entry(date_key).or_insert((0, 0, 0.0, 0.0));
            entry.0 += 1;
            let cache_creation = if item.cache_creation_input_tokens == 0
                && (item.cache_creation_ephemeral_5m_input_tokens
                    + item.cache_creation_ephemeral_1h_input_tokens)
                    > 0
            {
                item.cache_creation_ephemeral_5m_input_tokens
                    + item.cache_creation_ephemeral_1h_input_tokens
            } else {
                item.cache_creation_input_tokens
            };
            entry.1 += item.input_tokens
                + item.output_tokens
                + cache_creation
                + item.cache_read_input_tokens;
            entry.2 += item.total_cost_usd;
            entry.3 += item.actual_total_cost_usd;
        }
        let mut result: Vec<_> = daily
            .into_iter()
            .map(
                |(date, (requests, total_tokens, total_cost_usd, actual_total_cost_usd))| {
                    StoredUsageDailySummary {
                        date,
                        requests,
                        total_tokens,
                        total_cost_usd,
                        actual_total_cost_usd,
                    }
                },
            )
            .collect();
        result.sort_by(|a, b| a.date.cmp(&b.date));
        Ok(result)
    }
}

fn detach_usage_body(
    request_id: &str,
    body: &mut Option<Value>,
    detached_bodies: &mut BTreeMap<String, Value>,
    field: UsageBodyField,
) -> Option<String> {
    let value = body.take()?;
    let body_ref = usage_body_ref(request_id, field);
    detached_bodies.insert(body_ref.clone(), value);
    Some(body_ref)
}

fn usage_body_ref_from_metadata(
    metadata: Option<&Value>,
    request_id: &str,
    field: UsageBodyField,
) -> Option<String> {
    metadata
        .and_then(Value::as_object)
        .and_then(|object| object.get(field.as_ref_key()))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(parse_usage_body_ref)
        .filter(|(parsed_request_id, parsed_field)| {
            parsed_request_id == request_id && *parsed_field == field
        })
        .map(|(parsed_request_id, parsed_field)| usage_body_ref(&parsed_request_id, parsed_field))
}

fn hydrate_legacy_body_refs(item: &mut StoredRequestUsageAudit) {
    if item.request_body_ref.is_none() {
        item.request_body_ref = usage_body_ref_from_metadata(
            item.request_metadata.as_ref(),
            &item.request_id,
            UsageBodyField::RequestBody,
        );
    }
    if item.provider_request_body_ref.is_none() {
        item.provider_request_body_ref = usage_body_ref_from_metadata(
            item.request_metadata.as_ref(),
            &item.request_id,
            UsageBodyField::ProviderRequestBody,
        );
    }
    if item.response_body_ref.is_none() {
        item.response_body_ref = usage_body_ref_from_metadata(
            item.request_metadata.as_ref(),
            &item.request_id,
            UsageBodyField::ResponseBody,
        );
    }
    if item.client_response_body_ref.is_none() {
        item.client_response_body_ref = usage_body_ref_from_metadata(
            item.request_metadata.as_ref(),
            &item.request_id,
            UsageBodyField::ClientResponseBody,
        );
    }
}

fn hydrate_client_family(item: &mut StoredRequestUsageAudit) {
    if item.client_family.is_none() {
        item.client_family = usage_request_metadata_client_family(item.request_metadata.as_ref())
            .map(ToOwned::to_owned);
    }
}

fn persisted_usage_body_ref(
    incoming_ref: Option<&str>,
    incoming_body: Option<&Value>,
    incoming_state: Option<UsageBodyCaptureState>,
    _metadata: Option<&Value>,
    existing: Option<&StoredRequestUsageAudit>,
    field: UsageBodyField,
) -> Option<String> {
    if incoming_state == Some(UsageBodyCaptureState::None) {
        return None;
    }
    if incoming_body.is_some() {
        return None;
    }
    incoming_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            existing.and_then(|existing| match field {
                UsageBodyField::RequestBody => existing.request_body_ref.clone(),
                UsageBodyField::ProviderRequestBody => existing.provider_request_body_ref.clone(),
                UsageBodyField::ResponseBody => existing.response_body_ref.clone(),
                UsageBodyField::ClientResponseBody => existing.client_response_body_ref.clone(),
            })
        })
}

fn request_body_capture_replaces_derived_facts(
    request_body: Option<&Value>,
    request_body_state: Option<UsageBodyCaptureState>,
) -> bool {
    if request_body_state.is_some() {
        return true;
    }
    let Some(request_body) = request_body else {
        return false;
    };
    !request_body.as_object().is_some_and(|body| {
        body.get("truncated").and_then(Value::as_bool) == Some(true)
            && body.get("reason").and_then(Value::as_str) == Some("body_capture_limit_exceeded")
    })
}

fn clear_request_body_facts(
    metadata: Option<&Value>,
    clear_client_request_body_facts: bool,
    clear_provider_request_body_facts: bool,
) -> Value {
    let mut metadata = metadata
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if clear_client_request_body_facts {
        metadata.remove(REQUESTED_REASONING_EFFORT_METADATA_KEY);
        metadata.remove("request_body_ref");
    }
    if clear_provider_request_body_facts {
        metadata.remove(PROVIDER_REASONING_EFFORT_METADATA_KEY);
        metadata.remove(PROVIDER_SERVICE_TIER_METADATA_KEY);
        metadata.remove(PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY);
        metadata.remove("provider_request_body_ref");
    }
    Value::Object(metadata)
}

fn retain_previous_request_audit_metadata(
    metadata: Option<&Value>,
    preserve_client_request_body_facts: bool,
) -> Value {
    let Some(metadata) = metadata.and_then(Value::as_object) else {
        return Value::Object(serde_json::Map::new());
    };
    let mut retained = serde_json::Map::new();
    for key in [
        "trace_id",
        "client_ip",
        "user_agent",
        "client_family",
        "client_requested_stream",
        "client_session_affinity",
        "api_key_is_standalone",
        "request_path",
        "request_query_string",
        "request_path_and_query",
    ] {
        if let Some(value) = metadata.get(key) {
            retained.insert(key.to_string(), value.clone());
        }
    }
    if preserve_client_request_body_facts {
        for key in [REQUESTED_REASONING_EFFORT_METADATA_KEY, "request_body_ref"] {
            if let Some(value) = metadata.get(key) {
                retained.insert(key.to_string(), value.clone());
            }
        }
    }
    Value::Object(retained)
}

fn merge_usage_status_code(
    existing: Option<&StoredRequestUsageAudit>,
    incoming_status: &str,
    incoming_status_code: Option<u16>,
) -> Option<u16> {
    if existing.is_some_and(|existing| {
        existing.status == "streaming"
            && incoming_status == "streaming"
            && incoming_status_code.is_none()
    }) {
        return existing.and_then(|existing| existing.status_code);
    }
    incoming_status_code
}

#[async_trait]
impl UsageWriteRepository for InMemoryUsageReadRepository {
    async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        usage.validate()?;
        let usage = strip_deprecated_usage_display_fields(usage);
        let mut by_request_id = self.by_request_id.write().expect("usage repository lock");
        let existing = by_request_id.get(&usage.request_id).cloned();

        let created_at_unix_ms = by_request_id
            .get(&usage.request_id)
            .map(|existing| existing.created_at_unix_ms)
            .or(usage.created_at_unix_ms)
            .unwrap_or(usage.updated_at_unix_secs);

        let total_tokens = usage
            .total_tokens
            .or_else(|| {
                Some(
                    usage.input_tokens.unwrap_or_default()
                        + usage.output_tokens.unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        if existing.as_ref().is_some_and(|existing| {
            let can_recover = usage_can_recover_terminal_failure(
                existing.status.as_str(),
                existing.billing_status.as_str(),
                usage.status.as_str(),
                usage.billing_status.as_str(),
            );
            let finalized_lifecycle_regression = usage_status_is_finalized(&existing.status)
                && usage_status_is_lifecycle(&usage.status);
            let completed_terminal_failure_recovery = existing.billing_status == "void"
                && matches!(existing.status.as_str(), "failed" | "cancelled")
                && usage.status == "completed";
            (finalized_lifecycle_regression || completed_terminal_failure_recovery) && !can_recover
        }) {
            return Ok(existing.expect("existing usage should be present").clone());
        }
        if existing.as_ref().is_some_and(|existing| {
            existing.billing_status == "pending"
                && existing.status == "streaming"
                && usage.status == "pending"
        }) {
            return Ok(existing.expect("existing usage should be present").clone());
        }

        let replace_client_request_body_facts = request_body_capture_replaces_derived_facts(
            usage.request_body.as_ref(),
            usage.request_body_state,
        );
        let replace_provider_request_body_facts = request_body_capture_replaces_derived_facts(
            usage.provider_request_body.as_ref(),
            usage.provider_request_body_state,
        );
        let clear_request_body = usage.request_body_state == Some(UsageBodyCaptureState::None);
        let clear_provider_request_body =
            usage.provider_request_body_state == Some(UsageBodyCaptureState::None);
        let clear_response_body = usage.response_body_state == Some(UsageBodyCaptureState::None);
        let clear_client_response_body =
            usage.client_response_body_state == Some(UsageBodyCaptureState::None);
        let replace_routing_snapshot = usage_status_is_finalized(&usage.status);
        let mut incoming_request_metadata = usage.request_metadata.clone();
        if incoming_request_metadata.is_some()
            && (clear_request_body || clear_provider_request_body)
        {
            incoming_request_metadata = Some(clear_request_body_facts(
                incoming_request_metadata.as_ref(),
                clear_request_body,
                clear_provider_request_body,
            ));
        }
        let request_metadata = incoming_request_metadata.or_else(|| {
            if replace_routing_snapshot {
                Some(retain_previous_request_audit_metadata(
                    existing
                        .as_ref()
                        .and_then(|existing| existing.request_metadata.as_ref()),
                    !replace_client_request_body_facts,
                ))
            } else if replace_client_request_body_facts || replace_provider_request_body_facts {
                Some(clear_request_body_facts(
                    existing
                        .as_ref()
                        .and_then(|existing| existing.request_metadata.as_ref()),
                    replace_client_request_body_facts,
                    replace_provider_request_body_facts,
                ))
            } else {
                existing
                    .as_ref()
                    .and_then(|existing| existing.request_metadata.clone())
            }
        });
        let request_body_ref = persisted_usage_body_ref(
            usage.request_body_ref.as_deref(),
            usage.request_body.as_ref(),
            usage.request_body_state,
            request_metadata.as_ref(),
            existing.as_ref(),
            UsageBodyField::RequestBody,
        );
        let provider_request_body_ref = persisted_usage_body_ref(
            usage.provider_request_body_ref.as_deref(),
            usage.provider_request_body.as_ref(),
            usage.provider_request_body_state,
            request_metadata.as_ref(),
            existing.as_ref(),
            UsageBodyField::ProviderRequestBody,
        );
        let response_body_ref = persisted_usage_body_ref(
            usage.response_body_ref.as_deref(),
            usage.response_body.as_ref(),
            usage.response_body_state,
            request_metadata.as_ref(),
            existing.as_ref(),
            UsageBodyField::ResponseBody,
        );
        let client_response_body_ref = persisted_usage_body_ref(
            usage.client_response_body_ref.as_deref(),
            usage.client_response_body.as_ref(),
            usage.client_response_body_state,
            request_metadata.as_ref(),
            existing.as_ref(),
            UsageBodyField::ClientResponseBody,
        );
        if clear_request_body
            || clear_provider_request_body
            || clear_response_body
            || clear_client_response_body
        {
            let mut detached_bodies = self.detached_bodies.write().expect("usage repository lock");
            for (clear, field) in [
                (clear_request_body, UsageBodyField::RequestBody),
                (
                    clear_provider_request_body,
                    UsageBodyField::ProviderRequestBody,
                ),
                (clear_response_body, UsageBodyField::ResponseBody),
                (
                    clear_client_response_body,
                    UsageBodyField::ClientResponseBody,
                ),
            ] {
                if clear {
                    detached_bodies.remove(&usage_body_ref(&usage.request_id, field));
                }
            }
        }
        let stored = StoredRequestUsageAudit {
            id: existing
                .as_ref()
                .map(|existing| existing.id.clone())
                .unwrap_or_else(|| format!("usage-{}", usage.request_id)),
            request_id: usage.request_id.clone(),
            user_id: usage.user_id,
            api_key_id: usage.api_key_id,
            username: existing
                .as_ref()
                .and_then(|existing| existing.username.clone()),
            api_key_name: existing
                .as_ref()
                .and_then(|existing| existing.api_key_name.clone()),
            provider_name: usage.provider_name,
            model: usage.model,
            target_model: usage.target_model,
            provider_id: usage.provider_id,
            provider_endpoint_id: usage.provider_endpoint_id,
            provider_api_key_id: usage.provider_api_key_id,
            request_type: usage.request_type,
            api_format: usage.api_format,
            api_family: usage.api_family,
            endpoint_kind: usage.endpoint_kind,
            endpoint_api_format: usage.endpoint_api_format,
            provider_api_family: usage.provider_api_family,
            provider_endpoint_kind: usage.provider_endpoint_kind,
            has_format_conversion: usage.has_format_conversion.unwrap_or(false),
            is_stream: usage.is_stream.unwrap_or(false),
            input_tokens: usage.input_tokens.unwrap_or_default(),
            output_tokens: usage.output_tokens.unwrap_or_default(),
            total_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens.unwrap_or_else(|| {
                existing
                    .as_ref()
                    .map(|existing| existing.cache_creation_input_tokens)
                    .unwrap_or_default()
            }),
            cache_creation_ephemeral_5m_input_tokens: usage
                .cache_creation_ephemeral_5m_input_tokens
                .unwrap_or_else(|| {
                    existing
                        .as_ref()
                        .map(|existing| existing.cache_creation_ephemeral_5m_input_tokens)
                        .unwrap_or_default()
                }),
            cache_creation_ephemeral_1h_input_tokens: usage
                .cache_creation_ephemeral_1h_input_tokens
                .unwrap_or_else(|| {
                    existing
                        .as_ref()
                        .map(|existing| existing.cache_creation_ephemeral_1h_input_tokens)
                        .unwrap_or_default()
                }),
            cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or_else(|| {
                existing
                    .as_ref()
                    .map(|existing| existing.cache_read_input_tokens)
                    .unwrap_or_default()
            }),
            cache_creation_cost_usd: usage.cache_creation_cost_usd.unwrap_or_else(|| {
                existing
                    .as_ref()
                    .map(|existing| existing.cache_creation_cost_usd)
                    .unwrap_or_default()
            }),
            cache_read_cost_usd: usage.cache_read_cost_usd.unwrap_or_else(|| {
                existing
                    .as_ref()
                    .map(|existing| existing.cache_read_cost_usd)
                    .unwrap_or_default()
            }),
            output_price_per_1m: existing
                .as_ref()
                .and_then(|existing| existing.output_price_per_1m),
            total_cost_usd: usage.total_cost_usd.unwrap_or_else(|| {
                existing
                    .as_ref()
                    .map(|existing| existing.total_cost_usd)
                    .unwrap_or_default()
            }),
            actual_total_cost_usd: usage.actual_total_cost_usd.unwrap_or_else(|| {
                existing
                    .as_ref()
                    .map(|existing| existing.actual_total_cost_usd)
                    .unwrap_or_default()
            }),
            status_code: merge_usage_status_code(
                existing.as_ref(),
                usage.status.as_str(),
                usage.status_code,
            ),
            error_message: usage.error_message,
            error_category: usage.error_category,
            response_time_ms: merge_usage_timing(
                existing
                    .as_ref()
                    .and_then(|existing| existing.response_time_ms),
                usage.response_time_ms,
            ),
            first_byte_time_ms: merge_usage_timing(
                existing
                    .as_ref()
                    .and_then(|existing| existing.first_byte_time_ms),
                usage.first_byte_time_ms,
            ),
            status: usage.status,
            billing_status: usage.billing_status,
            request_headers: usage.request_headers.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|existing| existing.request_headers.clone())
            }),
            request_body: if clear_request_body {
                None
            } else {
                usage.request_body.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.request_body.clone())
                })
            },
            request_body_ref,
            request_body_state: usage.request_body_state.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|existing| existing.request_body_state)
            }),
            provider_request_headers: usage.provider_request_headers.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|existing| existing.provider_request_headers.clone())
            }),
            provider_request_body: if clear_provider_request_body {
                None
            } else {
                usage.provider_request_body.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.provider_request_body.clone())
                })
            },
            provider_request_body_ref,
            provider_request_body_state: usage.provider_request_body_state.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|existing| existing.provider_request_body_state)
            }),
            response_headers: usage.response_headers.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|existing| existing.response_headers.clone())
            }),
            response_body: if clear_response_body {
                None
            } else {
                usage.response_body.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.response_body.clone())
                })
            },
            response_body_ref,
            response_body_state: usage.response_body_state.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|existing| existing.response_body_state)
            }),
            client_response_headers: usage.client_response_headers.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|existing| existing.client_response_headers.clone())
            }),
            client_response_body: if clear_client_response_body {
                None
            } else {
                usage.client_response_body.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.client_response_body.clone())
                })
            },
            client_response_body_ref,
            client_response_body_state: usage.client_response_body_state.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|existing| existing.client_response_body_state)
            }),
            candidate_id: if replace_routing_snapshot {
                usage.candidate_id
            } else {
                usage.candidate_id.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.routing_candidate_id().map(ToOwned::to_owned))
                })
            },
            candidate_index: if replace_routing_snapshot {
                usage.candidate_index
            } else {
                usage.candidate_index.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.routing_candidate_index())
                })
            },
            key_name: if replace_routing_snapshot {
                usage.key_name
            } else {
                usage.key_name.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.routing_key_name().map(ToOwned::to_owned))
                })
            },
            planner_kind: if replace_routing_snapshot {
                usage.planner_kind
            } else {
                usage.planner_kind.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.routing_planner_kind().map(ToOwned::to_owned))
                })
            },
            route_family: if replace_routing_snapshot {
                usage.route_family
            } else {
                usage.route_family.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.routing_route_family().map(ToOwned::to_owned))
                })
            },
            route_kind: if replace_routing_snapshot {
                usage.route_kind
            } else {
                usage.route_kind.or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.routing_route_kind().map(ToOwned::to_owned))
                })
            },
            execution_path: if replace_routing_snapshot {
                usage.execution_path
            } else {
                usage.execution_path.or_else(|| {
                    existing.as_ref().and_then(|existing| {
                        existing.routing_execution_path().map(ToOwned::to_owned)
                    })
                })
            },
            local_execution_runtime_miss_reason: if replace_routing_snapshot {
                usage.local_execution_runtime_miss_reason
            } else {
                usage.local_execution_runtime_miss_reason.or_else(|| {
                    existing.as_ref().and_then(|existing| {
                        existing
                            .routing_local_execution_runtime_miss_reason()
                            .map(ToOwned::to_owned)
                    })
                })
            },
            client_family: usage_request_metadata_client_family(request_metadata.as_ref())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    existing
                        .as_ref()
                        .and_then(|existing| existing.client_family.clone())
                }),
            request_metadata,
            created_at_unix_ms,
            updated_at_unix_secs: usage.updated_at_unix_secs,
            finalized_at_unix_secs: usage.finalized_at_unix_secs,
        };

        by_request_id.insert(stored.request_id.clone(), stored.clone());
        if let Some(auth_api_keys) = self.auth_api_keys.as_ref() {
            let before_contribution = existing.as_ref().and_then(api_key_usage_contribution);
            let after_contribution = api_key_usage_contribution(&stored);

            match (before_contribution.as_ref(), after_contribution.as_ref()) {
                (Some(before), Some(after)) if before.api_key_id == after.api_key_id => {
                    let delta = ApiKeyUsageDelta::between(before, after);
                    auth_api_keys.apply_usage_stats_delta(before.api_key_id.as_str(), &delta, None);
                }
                _ => {
                    if let Some(before) = before_contribution.as_ref() {
                        let delta = ApiKeyUsageDelta::removal(before);
                        let recomputed_last_used_at_unix_secs = by_request_id
                            .values()
                            .filter_map(|item| {
                                item.api_key_id
                                    .as_deref()
                                    .filter(|api_key_id| *api_key_id == before.api_key_id.as_str())
                                    .map(|_| item.created_at_unix_ms)
                            })
                            .max();
                        auth_api_keys.apply_usage_stats_delta(
                            before.api_key_id.as_str(),
                            &delta,
                            recomputed_last_used_at_unix_secs,
                        );
                    }
                    if let Some(after) = after_contribution.as_ref() {
                        let delta = ApiKeyUsageDelta::addition(after);
                        auth_api_keys.apply_usage_stats_delta(
                            after.api_key_id.as_str(),
                            &delta,
                            None,
                        );
                    }
                }
            }
        }
        if let Some(provider_catalog) = self.provider_catalog.as_ref() {
            let before_contribution = existing
                .as_ref()
                .and_then(provider_api_key_usage_contribution);
            let after_contribution = provider_api_key_usage_contribution(&stored);

            match (before_contribution.as_ref(), after_contribution.as_ref()) {
                (Some(before), Some(after)) if before.key_id == after.key_id => {
                    let delta = ProviderApiKeyUsageDelta::between(before, after);
                    provider_catalog.apply_usage_stats_delta(before.key_id.as_str(), &delta, None);
                }
                _ => {
                    if let Some(before) = before_contribution.as_ref() {
                        let delta = ProviderApiKeyUsageDelta::removal(before);
                        let recomputed_last_used_at_unix_secs = by_request_id
                            .values()
                            .filter_map(|item| {
                                item.provider_api_key_id
                                    .as_deref()
                                    .filter(|key_id| *key_id == before.key_id.as_str())
                                    .map(|_| item.created_at_unix_ms)
                            })
                            .max();
                        provider_catalog.apply_usage_stats_delta(
                            before.key_id.as_str(),
                            &delta,
                            recomputed_last_used_at_unix_secs,
                        );
                    }
                    if let Some(after) = after_contribution.as_ref() {
                        let delta = ProviderApiKeyUsageDelta::addition(after);
                        provider_catalog.apply_usage_stats_delta(
                            after.key_id.as_str(),
                            &delta,
                            None,
                        );
                    }
                }
            }
        }
        Ok(stored)
    }

    async fn rebuild_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        let Some(auth_api_keys) = self.auth_api_keys.as_ref() else {
            return Ok(0);
        };

        let by_request_id = self.by_request_id.read().expect("usage repository lock");
        let mut aggregates = BTreeMap::new();
        for usage in by_request_id.values() {
            let Some(contribution) = api_key_usage_contribution(usage) else {
                continue;
            };
            accumulate_api_key_usage_contribution(&mut aggregates, contribution);
        }
        auth_api_keys.rebuild_usage_stats(&aggregates);
        Ok(aggregates.len() as u64)
    }

    async fn rebuild_provider_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        let Some(provider_catalog) = self.provider_catalog.as_ref() else {
            return Ok(0);
        };

        let by_request_id = self.by_request_id.read().expect("usage repository lock");
        let mut aggregates = BTreeMap::new();
        for usage in by_request_id.values() {
            let Some(contribution) = provider_api_key_usage_contribution(usage) else {
                continue;
            };
            accumulate_provider_api_key_usage_contribution(&mut aggregates, contribution);
        }
        provider_catalog.rebuild_usage_stats(&aggregates);
        Ok(aggregates.len() as u64)
    }
}

#[cfg(test)]
mod tests;
