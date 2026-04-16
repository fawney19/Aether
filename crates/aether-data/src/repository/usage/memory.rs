use std::collections::BTreeMap;
use std::sync::RwLock;

use aether_data_contracts::repository::usage::{
    parse_usage_body_ref, usage_body_ref, StoredUsageAuditAggregation, StoredUsageAuditSummary,
    StoredUsageLeaderboardSummary, StoredUsageTimeSeriesBucket, UsageAuditAggregationGroupBy,
    UsageAuditAggregationQuery, UsageAuditSummaryQuery, UsageBodyField, UsageLeaderboardGroupBy,
    UsageLeaderboardQuery, UsageTimeSeriesGranularity, UsageTimeSeriesQuery,
};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

use super::{
    strip_deprecated_usage_display_fields, usage_can_recover_terminal_failure,
    StoredProviderApiKeyUsageSummary, StoredProviderUsageSummary, StoredProviderUsageWindow,
    StoredRequestUsageAudit, StoredUsageDailySummary, UpsertUsageRecord, UsageAuditListQuery,
    UsageDailyHeatmapQuery, UsageReadRepository, UsageWriteRepository,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryUsageReadRepository {
    by_request_id: RwLock<BTreeMap<String, StoredRequestUsageAudit>>,
    detached_bodies: RwLock<BTreeMap<String, Value>>,
    provider_usage_windows: RwLock<Vec<StoredProviderUsageWindow>>,
}

impl InMemoryUsageReadRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredRequestUsageAudit>,
    {
        let mut by_request_id = BTreeMap::new();
        for mut item in items {
            hydrate_legacy_body_refs(&mut item);
            by_request_id.insert(item.request_id.clone(), item);
        }
        Self {
            by_request_id: RwLock::new(by_request_id),
            detached_bodies: RwLock::new(BTreeMap::new()),
            provider_usage_windows: RwLock::new(Vec::new()),
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
        }
    }
}

fn usage_status_is_finalized(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

fn usage_status_is_lifecycle(status: &str) -> bool {
    matches!(status, "pending" | "streaming")
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
    if let Some(statuses) = query.statuses.as_ref() {
        if !statuses.iter().any(|status| status == &item.status) {
            return false;
        }
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
    cache_read_tokens: i64,
) -> i64 {
    if input_tokens <= 0 {
        return input_tokens.max(0);
    }
    if cache_read_tokens <= 0 {
        return input_tokens;
    }

    match usage_api_family(api_format) {
        UsageApiFamily::OpenAi | UsageApiFamily::Gemini => {
            (input_tokens - cache_read_tokens).max(0)
        }
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
        UsageApiFamily::OpenAi | UsageApiFamily::Gemini => normalize_usage_input_tokens(
            api_format,
            normalized_input_tokens,
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
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_usage_input_tokens(api_format, input_tokens, cache_read_tokens) as u64
}

fn usage_is_success(item: &StoredRequestUsageAudit) -> bool {
    matches!(
        item.status.as_str(),
        "completed" | "success" | "ok" | "billed" | "settled"
    ) && item.status_code.is_none_or(|code| code < 400)
}

fn usage_provider_display_name(item: &StoredRequestUsageAudit) -> Option<String> {
    let provider_name = item.provider_name.trim();
    if provider_name.is_empty() || matches!(provider_name, "unknown" | "pending") {
        None
    } else {
        Some(item.provider_name.clone())
    }
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

    async fn count_usage_audits(&self, query: &UsageAuditListQuery) -> Result<u64, DataLayerError> {
        Ok(self
            .by_request_id
            .read()
            .expect("usage repository lock")
            .values()
            .filter(|item| usage_matches_list_query(item, query))
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
            {
                continue;
            }

            let group_key = match query.group_by {
                UsageAuditAggregationGroupBy::Model => item.model.clone(),
                UsageAuditAggregationGroupBy::Provider => item
                    .provider_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
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
                bucket.display_name =
                    usage_provider_display_name(item).or(Some("Unknown".to_string()));
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
            entry.total_tokens = entry.total_tokens.saturating_add(
                item.input_tokens
                    .saturating_add(item.output_tokens)
                    .saturating_add(item.cache_creation_input_tokens)
                    .saturating_add(item.cache_read_input_tokens),
            );
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
            if query.admin_mode {
                if item.status == "pending" || item.status == "streaming" {
                    continue;
                }
            } else if item.billing_status != "settled" || item.total_cost_usd <= 0.0 {
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

fn persisted_usage_body_ref(
    incoming_ref: Option<&str>,
    incoming_body: Option<&Value>,
    _metadata: Option<&Value>,
    existing: Option<&StoredRequestUsageAudit>,
    field: UsageBodyField,
) -> Option<String> {
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

#[async_trait]
impl UsageWriteRepository for InMemoryUsageReadRepository {
    async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        usage.validate()?;
        let usage = strip_deprecated_usage_display_fields(usage);
        let mut by_request_id = self.by_request_id.write().expect("usage repository lock");

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
        let existing = by_request_id.get(&usage.request_id);
        if existing.is_some_and(|existing| {
            usage_status_is_finalized(existing.status.as_str())
                && usage_status_is_lifecycle(usage.status.as_str())
                && !usage_can_recover_terminal_failure(
                    existing.status.as_str(),
                    existing.billing_status.as_str(),
                    usage.status.as_str(),
                    usage.billing_status.as_str(),
                )
        }) {
            return Ok(existing.expect("existing usage should be present").clone());
        }
        if existing.is_some_and(|existing| {
            existing.billing_status == "pending"
                && existing.status == "streaming"
                && usage.status == "pending"
        }) {
            return Ok(existing.expect("existing usage should be present").clone());
        }

        let request_metadata = usage
            .request_metadata
            .clone()
            .or_else(|| existing.and_then(|existing| existing.request_metadata.clone()));
        let request_body_ref = persisted_usage_body_ref(
            usage.request_body_ref.as_deref(),
            usage.request_body.as_ref(),
            request_metadata.as_ref(),
            existing,
            UsageBodyField::RequestBody,
        );
        let provider_request_body_ref = persisted_usage_body_ref(
            usage.provider_request_body_ref.as_deref(),
            usage.provider_request_body.as_ref(),
            request_metadata.as_ref(),
            existing,
            UsageBodyField::ProviderRequestBody,
        );
        let response_body_ref = persisted_usage_body_ref(
            usage.response_body_ref.as_deref(),
            usage.response_body.as_ref(),
            request_metadata.as_ref(),
            existing,
            UsageBodyField::ResponseBody,
        );
        let client_response_body_ref = persisted_usage_body_ref(
            usage.client_response_body_ref.as_deref(),
            usage.client_response_body.as_ref(),
            request_metadata.as_ref(),
            existing,
            UsageBodyField::ClientResponseBody,
        );
        let stored = StoredRequestUsageAudit {
            id: existing
                .map(|existing| existing.id.clone())
                .unwrap_or_else(|| format!("usage-{}", usage.request_id)),
            request_id: usage.request_id.clone(),
            user_id: usage.user_id,
            api_key_id: usage.api_key_id,
            username: existing.and_then(|existing| existing.username.clone()),
            api_key_name: existing.and_then(|existing| existing.api_key_name.clone()),
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
                    .map(|existing| existing.cache_creation_input_tokens)
                    .unwrap_or_default()
            }),
            cache_creation_ephemeral_5m_input_tokens: usage
                .cache_creation_ephemeral_5m_input_tokens
                .unwrap_or_else(|| {
                    existing
                        .map(|existing| existing.cache_creation_ephemeral_5m_input_tokens)
                        .unwrap_or_default()
                }),
            cache_creation_ephemeral_1h_input_tokens: usage
                .cache_creation_ephemeral_1h_input_tokens
                .unwrap_or_else(|| {
                    existing
                        .map(|existing| existing.cache_creation_ephemeral_1h_input_tokens)
                        .unwrap_or_default()
                }),
            cache_read_input_tokens: usage.cache_read_input_tokens.unwrap_or_else(|| {
                existing
                    .map(|existing| existing.cache_read_input_tokens)
                    .unwrap_or_default()
            }),
            cache_creation_cost_usd: usage.cache_creation_cost_usd.unwrap_or_else(|| {
                existing
                    .map(|existing| existing.cache_creation_cost_usd)
                    .unwrap_or_default()
            }),
            cache_read_cost_usd: usage.cache_read_cost_usd.unwrap_or_else(|| {
                existing
                    .map(|existing| existing.cache_read_cost_usd)
                    .unwrap_or_default()
            }),
            output_price_per_1m: existing.and_then(|existing| existing.output_price_per_1m),
            total_cost_usd: usage.total_cost_usd.unwrap_or_else(|| {
                existing
                    .map(|existing| existing.total_cost_usd)
                    .unwrap_or_default()
            }),
            actual_total_cost_usd: usage.actual_total_cost_usd.unwrap_or_else(|| {
                existing
                    .map(|existing| existing.actual_total_cost_usd)
                    .unwrap_or_default()
            }),
            status_code: usage.status_code,
            error_message: usage.error_message,
            error_category: usage.error_category,
            response_time_ms: usage.response_time_ms,
            first_byte_time_ms: usage.first_byte_time_ms,
            status: usage.status,
            billing_status: usage.billing_status,
            request_headers: usage
                .request_headers
                .or_else(|| existing.and_then(|existing| existing.request_headers.clone())),
            request_body: usage
                .request_body
                .or_else(|| existing.and_then(|existing| existing.request_body.clone())),
            request_body_ref,
            provider_request_headers: usage.provider_request_headers.or_else(|| {
                existing.and_then(|existing| existing.provider_request_headers.clone())
            }),
            provider_request_body: usage
                .provider_request_body
                .or_else(|| existing.and_then(|existing| existing.provider_request_body.clone())),
            provider_request_body_ref,
            response_headers: usage
                .response_headers
                .or_else(|| existing.and_then(|existing| existing.response_headers.clone())),
            response_body: usage
                .response_body
                .or_else(|| existing.and_then(|existing| existing.response_body.clone())),
            response_body_ref,
            client_response_headers: usage
                .client_response_headers
                .or_else(|| existing.and_then(|existing| existing.client_response_headers.clone())),
            client_response_body: usage
                .client_response_body
                .or_else(|| existing.and_then(|existing| existing.client_response_body.clone())),
            client_response_body_ref,
            candidate_id: usage.candidate_id.or_else(|| {
                existing.and_then(|existing| existing.routing_candidate_id().map(ToOwned::to_owned))
            }),
            candidate_index: usage
                .candidate_index
                .or_else(|| existing.and_then(|existing| existing.routing_candidate_index())),
            key_name: usage.key_name.or_else(|| {
                existing.and_then(|existing| existing.routing_key_name().map(ToOwned::to_owned))
            }),
            planner_kind: usage.planner_kind.or_else(|| {
                existing.and_then(|existing| existing.routing_planner_kind().map(ToOwned::to_owned))
            }),
            route_family: usage.route_family.or_else(|| {
                existing.and_then(|existing| existing.routing_route_family().map(ToOwned::to_owned))
            }),
            route_kind: usage.route_kind.or_else(|| {
                existing.and_then(|existing| existing.routing_route_kind().map(ToOwned::to_owned))
            }),
            execution_path: usage.execution_path.or_else(|| {
                existing
                    .and_then(|existing| existing.routing_execution_path().map(ToOwned::to_owned))
            }),
            local_execution_runtime_miss_reason: usage.local_execution_runtime_miss_reason.or_else(
                || {
                    existing.and_then(|existing| {
                        existing
                            .routing_local_execution_runtime_miss_reason()
                            .map(ToOwned::to_owned)
                    })
                },
            ),
            request_metadata,
            created_at_unix_ms,
            updated_at_unix_secs: usage.updated_at_unix_secs,
            finalized_at_unix_secs: usage.finalized_at_unix_secs,
        };

        by_request_id.insert(stored.request_id.clone(), stored.clone());
        Ok(stored)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryUsageReadRepository;
    use crate::repository::usage::{
        StoredProviderUsageWindow, StoredRequestUsageAudit, UpsertUsageRecord, UsageReadRepository,
        UsageWriteRepository,
    };
    use aether_data_contracts::repository::usage::{usage_body_ref, UsageBodyField};
    use serde_json::json;

    fn sample_usage(request_id: &str, created_at_unix_ms: i64) -> StoredRequestUsageAudit {
        StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            request_id.to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            "OpenAI".to_string(),
            "gpt-4.1".to_string(),
            Some("gpt-4.1-mini".to_string()),
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
            false,
            100,
            50,
            150,
            0.12,
            0.18,
            Some(200),
            None,
            None,
            Some(420),
            Some(120),
            "completed".to_string(),
            "settled".to_string(),
            created_at_unix_ms,
            created_at_unix_ms + 1,
            Some(created_at_unix_ms + 2),
        )
        .expect("usage should build")
    }

    #[tokio::test]
    async fn finds_usage_by_request_id() {
        let repository = InMemoryUsageReadRepository::seed(vec![
            sample_usage("req-1", 100),
            sample_usage("req-2", 200),
        ]);

        let usage = repository
            .find_by_request_id("req-2")
            .await
            .expect("find should succeed")
            .expect("usage should exist");

        assert_eq!(usage.request_id, "req-2");
        assert_eq!(usage.total_tokens, 150);
    }

    #[tokio::test]
    async fn stale_pending_update_does_not_regress_finalized_usage() {
        let repository = InMemoryUsageReadRepository::default();
        repository
            .upsert(UpsertUsageRecord {
                request_id: "req-finalized-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("api-key-1".to_string()),
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(false),
                input_tokens: Some(3),
                output_tokens: Some(5),
                total_tokens: Some(8),
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: Some(200),
                error_message: None,
                error_category: None,
                response_time_ms: Some(45),
                first_byte_time_ms: None,
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
                finalized_at_unix_secs: Some(101),
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 101,
            })
            .await
            .expect("completed usage should upsert");

        repository
            .upsert(UpsertUsageRecord {
                request_id: "req-finalized-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("api-key-1".to_string()),
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(false),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: None,
                error_message: None,
                error_category: None,
                response_time_ms: None,
                first_byte_time_ms: None,
                status: "pending".to_string(),
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
                updated_at_unix_secs: 102,
            })
            .await
            .expect("stale pending usage should upsert");

        let stored = repository
            .find_by_request_id("req-finalized-1")
            .await
            .expect("usage lookup should succeed")
            .expect("usage should exist");
        assert_eq!(stored.status, "completed");
        assert_eq!(stored.status_code, Some(200));
        assert_eq!(stored.total_tokens, 8);
        assert_eq!(stored.finalized_at_unix_secs, Some(101));
    }

    #[tokio::test]
    async fn upsert_allows_streaming_recovery_after_void_failure() {
        let repository = InMemoryUsageReadRepository::default();
        repository
            .upsert(UpsertUsageRecord {
                request_id: "req-recover-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("api-key-1".to_string()),
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(false),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: Some(0.0),
                actual_total_cost_usd: Some(0.0),
                status_code: Some(503),
                error_message: Some("provider timeout".to_string()),
                error_category: Some("provider_error".to_string()),
                response_time_ms: Some(90),
                first_byte_time_ms: None,
                status: "failed".to_string(),
                billing_status: "void".to_string(),
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
                finalized_at_unix_secs: Some(101),
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 101,
            })
            .await
            .expect("failed usage should upsert");

        repository
            .upsert(UpsertUsageRecord {
                request_id: "req-recover-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("api-key-1".to_string()),
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: Some("gpt-5-mini".to_string()),
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(true),
                is_stream: Some(true),
                input_tokens: Some(10),
                output_tokens: None,
                total_tokens: None,
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: None,
                error_message: None,
                error_category: None,
                response_time_ms: Some(45),
                first_byte_time_ms: Some(12),
                status: "streaming".to_string(),
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
                candidate_id: Some("cand-1".to_string()),
                candidate_index: Some(1),
                key_name: Some("primary".to_string()),
                planner_kind: Some("claude_cli_sync".to_string()),
                route_family: Some("claude".to_string()),
                route_kind: Some("cli".to_string()),
                execution_path: Some("remote".to_string()),
                local_execution_runtime_miss_reason: None,
                request_metadata: Some(json!({
                    "trace_id": "trace-recovered"
                })),
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 102,
            })
            .await
            .expect("recovery usage should upsert");

        let stored = repository
            .find_by_request_id("req-recover-1")
            .await
            .expect("usage lookup should succeed")
            .expect("usage should exist");
        assert_eq!(stored.status, "streaming");
        assert_eq!(stored.billing_status, "pending");
        assert_eq!(stored.status_code, None);
        assert_eq!(stored.error_message, None);
        assert_eq!(stored.finalized_at_unix_secs, None);
        assert_eq!(
            stored.request_metadata,
            Some(json!({ "trace_id": "trace-recovered" }))
        );
        assert_eq!(stored.total_tokens, 10);
    }

    #[tokio::test]
    async fn stale_pending_update_does_not_regress_streaming_usage() {
        let repository = InMemoryUsageReadRepository::default();
        repository
            .upsert(UpsertUsageRecord {
                request_id: "req-streaming-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("api-key-1".to_string()),
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: Some("gpt-5-upstream".to_string()),
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(true),
                input_tokens: Some(10),
                output_tokens: Some(2),
                total_tokens: Some(12),
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: Some(0.0),
                actual_total_cost_usd: Some(0.0),
                status_code: Some(200),
                error_message: None,
                error_category: None,
                response_time_ms: Some(45),
                first_byte_time_ms: Some(12),
                status: "streaming".to_string(),
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
                candidate_id: Some("cand-1".to_string()),
                candidate_index: Some(1),
                key_name: Some("primary".to_string()),
                planner_kind: Some("claude_cli_sync".to_string()),
                route_family: Some("claude".to_string()),
                route_kind: Some("cli".to_string()),
                execution_path: Some("remote".to_string()),
                local_execution_runtime_miss_reason: None,
                request_metadata: Some(json!({
                    "trace_id": "trace-streaming"
                })),
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 101,
            })
            .await
            .expect("streaming usage should upsert");

        repository
            .upsert(UpsertUsageRecord {
                request_id: "req-streaming-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("api-key-1".to_string()),
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(true),
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: None,
                error_message: None,
                error_category: None,
                response_time_ms: None,
                first_byte_time_ms: None,
                status: "pending".to_string(),
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
                updated_at_unix_secs: 102,
            })
            .await
            .expect("stale pending usage should upsert");

        let stored = repository
            .find_by_request_id("req-streaming-1")
            .await
            .expect("usage lookup should succeed")
            .expect("usage should exist");
        assert_eq!(stored.status, "streaming");
        assert_eq!(stored.status_code, Some(200));
        assert_eq!(stored.first_byte_time_ms, Some(12));
        assert_eq!(stored.response_time_ms, Some(45));
        assert_eq!(stored.target_model.as_deref(), Some("gpt-5-upstream"));
        assert_eq!(stored.total_tokens, 12);
    }

    #[tokio::test]
    async fn seed_hydrates_legacy_body_ref_metadata_into_typed_fields() {
        let repository = InMemoryUsageReadRepository::seed(vec![StoredRequestUsageAudit {
            request_metadata: Some(json!({
                "request_body_ref": "usage://request/req-legacy/request_body"
            })),
            ..sample_usage("req-legacy", 100)
        }]);

        let usage = repository
            .find_by_request_id("req-legacy")
            .await
            .expect("find should succeed")
            .expect("usage should exist");

        assert_eq!(
            usage.body_ref(UsageBodyField::RequestBody),
            Some("usage://request/req-legacy/request_body")
        );
        assert_eq!(
            usage.request_metadata,
            Some(json!({
                "request_body_ref": "usage://request/req-legacy/request_body"
            }))
        );
    }

    #[tokio::test]
    async fn seed_ignores_invalid_or_mismatched_legacy_body_ref_metadata() {
        let repository = InMemoryUsageReadRepository::seed(vec![
            StoredRequestUsageAudit {
                request_metadata: Some(json!({
                    "request_body_ref": "blob://legacy-request"
                })),
                ..sample_usage("req-invalid-legacy", 100)
            },
            StoredRequestUsageAudit {
                request_metadata: Some(json!({
                    "request_body_ref": "usage://request/req-other/request_body"
                })),
                ..sample_usage("req-mismatched-legacy", 200)
            },
        ]);

        let invalid = repository
            .find_by_request_id("req-invalid-legacy")
            .await
            .expect("find should succeed")
            .expect("usage should exist");
        let mismatched = repository
            .find_by_request_id("req-mismatched-legacy")
            .await
            .expect("find should succeed")
            .expect("usage should exist");

        assert_eq!(invalid.body_ref(UsageBodyField::RequestBody), None);
        assert_eq!(mismatched.body_ref(UsageBodyField::RequestBody), None);
    }

    #[tokio::test]
    async fn detached_body_seed_moves_large_payloads_behind_usage_refs() {
        let mut usage = sample_usage("req-detached", 100);
        usage.request_body = Some(json!({
            "model": "gpt-4.1",
            "messages": [{"role": "user", "content": "hello"}]
        }));
        usage.provider_request_body = Some(json!({
            "model": "gpt-4.1-mini",
            "stream": false
        }));

        let repository = InMemoryUsageReadRepository::seed_with_detached_bodies(vec![usage]);

        let stored = repository
            .find_by_request_id("req-detached")
            .await
            .expect("find should succeed")
            .expect("usage should exist");

        assert!(stored.request_body.is_none());
        assert!(stored.provider_request_body.is_none());
        assert_eq!(
            stored.body_ref(UsageBodyField::RequestBody),
            Some("usage://request/req-detached/request_body")
        );
        assert_eq!(
            stored.body_ref(UsageBodyField::ProviderRequestBody),
            Some("usage://request/req-detached/provider_request_body")
        );
        assert_eq!(stored.request_metadata, None);
        assert_eq!(
            repository
                .resolve_body_ref(&usage_body_ref("req-detached", UsageBodyField::RequestBody))
                .await
                .expect("body ref should resolve"),
            Some(json!({
                "model": "gpt-4.1",
                "messages": [{"role": "user", "content": "hello"}]
            }))
        );
        assert_eq!(
            repository
                .resolve_body_ref(&usage_body_ref(
                    "req-detached",
                    UsageBodyField::ProviderRequestBody
                ))
                .await
                .expect("provider request body ref should resolve"),
            Some(json!({
                "model": "gpt-4.1-mini",
                "stream": false
            }))
        );
    }

    #[tokio::test]
    async fn upsert_writes_usage_record() {
        let repository = InMemoryUsageReadRepository::default();
        let stored = repository
            .upsert(UpsertUsageRecord {
                request_id: "req-upsert-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: Some("gpt-5-mini".to_string()),
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(false),
                is_stream: Some(true),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: None,
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
                response_time_ms: Some(300),
                first_byte_time_ms: Some(120),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                request_headers: Some(json!({"authorization": "Bearer test"})),
                request_body: Some(json!({"model": "gpt-5"})),
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
            })
            .await
            .expect("upsert should succeed");

        assert_eq!(stored.request_id, "req-upsert-1");
        assert_eq!(stored.total_tokens, 30);
        assert_eq!(stored.total_cost_usd, 0.25);
        assert_eq!(stored.actual_total_cost_usd, 0.15);
        assert_eq!(
            repository
                .find_by_request_id("req-upsert-1")
                .await
                .expect("find should succeed")
                .expect("usage should exist")
                .model,
            "gpt-5"
        );
    }

    #[tokio::test]
    async fn upsert_defaults_created_at_to_second_timestamp() {
        let repository = InMemoryUsageReadRepository::default();
        let stored = repository
            .upsert(UpsertUsageRecord {
                request_id: "req-upsert-ms-default".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: None,
                provider_endpoint_id: None,
                provider_api_key_id: None,
                request_type: None,
                api_format: None,
                api_family: None,
                endpoint_kind: None,
                endpoint_api_format: None,
                provider_api_family: None,
                provider_endpoint_kind: None,
                has_format_conversion: None,
                is_stream: None,
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: None,
                actual_total_cost_usd: None,
                status_code: None,
                error_message: None,
                error_category: None,
                response_time_ms: None,
                first_byte_time_ms: None,
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
                created_at_unix_ms: None,
                updated_at_unix_secs: 101,
            })
            .await
            .expect("upsert should succeed");

        assert_eq!(stored.created_at_unix_ms, 101);
    }

    #[tokio::test]
    async fn upsert_does_not_backfill_legacy_output_price_from_request_metadata() {
        let repository = InMemoryUsageReadRepository::default();
        let stored = repository
            .upsert(UpsertUsageRecord {
                request_id: "req-upsert-price-metadata".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
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
                total_tokens: None,
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
                response_time_ms: Some(300),
                first_byte_time_ms: Some(120),
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
                request_metadata: Some(json!({
                    "output_price_per_1m": 15.0
                })),
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 101,
            })
            .await
            .expect("upsert should succeed");

        assert_eq!(stored.output_price_per_1m, None);
        assert_eq!(stored.settlement_output_price_per_1m(), Some(15.0));
    }

    #[tokio::test]
    async fn upsert_does_not_backfill_typed_body_refs_from_request_metadata() {
        let repository = InMemoryUsageReadRepository::default();
        let stored = repository
            .upsert(UpsertUsageRecord {
                request_id: "req-upsert-body-ref-metadata".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
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
                total_tokens: None,
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
                response_time_ms: Some(300),
                first_byte_time_ms: Some(120),
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
                request_metadata: Some(json!({
                    "request_body_ref": "usage://request/req-upsert-body-ref-metadata/request_body"
                })),
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 101,
            })
            .await
            .expect("upsert should succeed");

        assert_eq!(stored.request_body_ref, None);
        assert_eq!(
            stored.request_metadata,
            Some(json!({
                "request_body_ref": "usage://request/req-upsert-body-ref-metadata/request_body"
            }))
        );
    }

    #[tokio::test]
    async fn upsert_keeps_typed_routing_fields_out_of_request_metadata() {
        let repository = InMemoryUsageReadRepository::default();
        let stored = repository
            .upsert(UpsertUsageRecord {
                request_id: "req-upsert-routing-metadata".to_string(),
                user_id: None,
                api_key_id: None,
                username: None,
                api_key_name: None,
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                target_model: None,
                provider_id: Some("provider-1".to_string()),
                provider_endpoint_id: Some("endpoint-1".to_string()),
                provider_api_key_id: Some("provider-key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                provider_api_family: Some("openai".to_string()),
                provider_endpoint_kind: Some("chat".to_string()),
                has_format_conversion: Some(true),
                is_stream: Some(false),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: None,
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: Some(0.25),
                actual_total_cost_usd: Some(0.15),
                status_code: Some(503),
                error_message: None,
                error_category: None,
                response_time_ms: Some(300),
                first_byte_time_ms: Some(120),
                status: "failed".to_string(),
                billing_status: "void".to_string(),
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
                candidate_id: Some("cand-typed".to_string()),
                candidate_index: Some(2),
                key_name: Some("primary".to_string()),
                planner_kind: Some("claude_cli_sync".to_string()),
                route_family: Some("claude".to_string()),
                route_kind: Some("cli".to_string()),
                execution_path: Some("local_execution_runtime_miss".to_string()),
                local_execution_runtime_miss_reason: Some("all_candidates_skipped".to_string()),
                request_metadata: Some(json!({
                    "trace_id": "trace-1"
                })),
                finalized_at_unix_secs: None,
                created_at_unix_ms: Some(100),
                updated_at_unix_secs: 101,
            })
            .await
            .expect("upsert should succeed");

        assert_eq!(
            stored.request_metadata,
            Some(json!({ "trace_id": "trace-1" }))
        );
        assert_eq!(stored.routing_candidate_id(), Some("cand-typed"));
        assert_eq!(stored.routing_candidate_index(), Some(2));
        assert_eq!(stored.routing_key_name(), Some("primary"));
        assert_eq!(stored.routing_planner_kind(), Some("claude_cli_sync"));
        assert_eq!(stored.routing_route_family(), Some("claude"));
        assert_eq!(stored.routing_route_kind(), Some("cli"));
        assert_eq!(
            stored.routing_execution_path(),
            Some("local_execution_runtime_miss")
        );
        assert_eq!(
            stored.routing_local_execution_runtime_miss_reason(),
            Some("all_candidates_skipped")
        );
    }

    #[tokio::test]
    async fn upsert_does_not_persist_legacy_display_columns_for_new_rows() {
        let repository = InMemoryUsageReadRepository::default();
        let stored = repository
            .upsert(UpsertUsageRecord {
                request_id: "req-upsert-display-columns".to_string(),
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
                total_tokens: None,
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
                response_time_ms: Some(300),
                first_byte_time_ms: Some(120),
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
            })
            .await
            .expect("upsert should succeed");

        assert_eq!(stored.username, None);
        assert_eq!(stored.api_key_name, None);
    }

    #[tokio::test]
    async fn upsert_preserves_existing_legacy_display_columns_when_new_write_omits_them() {
        let repository = InMemoryUsageReadRepository::seed(vec![StoredRequestUsageAudit {
            id: "usage-req-existing-display-columns".to_string(),
            request_id: "req-existing-display-columns".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            username: Some("legacy-alice".to_string()),
            api_key_name: Some("legacy-default".to_string()),
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
            has_format_conversion: false,
            is_stream: false,
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cache_creation_input_tokens: 0,
            cache_creation_ephemeral_5m_input_tokens: 0,
            cache_creation_ephemeral_1h_input_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_cost_usd: 0.0,
            cache_read_cost_usd: 0.0,
            output_price_per_1m: None,
            total_cost_usd: 0.25,
            actual_total_cost_usd: 0.15,
            status_code: Some(200),
            error_message: None,
            error_category: None,
            response_time_ms: Some(300),
            first_byte_time_ms: Some(120),
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
            created_at_unix_ms: 100,
            updated_at_unix_secs: 101,
            finalized_at_unix_secs: None,
        }]);
        let stored = repository
            .upsert(UpsertUsageRecord {
                request_id: "req-existing-display-columns".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                username: Some("fresh-alice".to_string()),
                api_key_name: Some("fresh-default".to_string()),
                provider_name: "OpenAI".to_string(),
                model: "gpt-5-mini".to_string(),
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
                input_tokens: Some(30),
                output_tokens: Some(40),
                total_tokens: None,
                cache_creation_input_tokens: None,
                cache_creation_ephemeral_5m_input_tokens: None,
                cache_creation_ephemeral_1h_input_tokens: None,
                cache_read_input_tokens: None,
                cache_creation_cost_usd: None,
                cache_read_cost_usd: None,
                output_price_per_1m: None,
                total_cost_usd: Some(0.45),
                actual_total_cost_usd: Some(0.30),
                status_code: Some(200),
                error_message: None,
                error_category: None,
                response_time_ms: Some(200),
                first_byte_time_ms: Some(80),
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
                updated_at_unix_secs: 102,
            })
            .await
            .expect("upsert should succeed");

        assert_eq!(stored.username.as_deref(), Some("legacy-alice"));
        assert_eq!(stored.api_key_name.as_deref(), Some("legacy-default"));
        assert_eq!(stored.model, "gpt-5-mini");
    }

    #[tokio::test]
    async fn summarizes_provider_usage_windows_since_timestamp() {
        let repository = InMemoryUsageReadRepository::default().with_provider_usage_windows(vec![
            StoredProviderUsageWindow::new(
                "provider-1".to_string(),
                1_700_000_000,
                10,
                9,
                1,
                120.0,
                1.25,
            )
            .expect("window should build"),
            StoredProviderUsageWindow::new(
                "provider-1".to_string(),
                1_700_003_600,
                6,
                5,
                1,
                180.0,
                0.75,
            )
            .expect("window should build"),
            StoredProviderUsageWindow::new(
                "provider-2".to_string(),
                1_700_003_600,
                99,
                99,
                0,
                50.0,
                5.0,
            )
            .expect("window should build"),
        ]);

        let summary = repository
            .summarize_provider_usage_since("provider-1", 1_700_000_100)
            .await
            .expect("summary should succeed");

        assert_eq!(summary.total_requests, 6);
        assert_eq!(summary.successful_requests, 5);
        assert_eq!(summary.failed_requests, 1);
        assert_eq!(summary.avg_response_time_ms, 180.0);
        assert_eq!(summary.total_cost_usd, 0.75);
    }

    #[tokio::test]
    async fn summarizes_usage_by_provider_api_key_ids() {
        let repository = InMemoryUsageReadRepository::seed(vec![
            sample_usage("req-1", 1_711_000_000),
            sample_usage("req-2", 1_711_000_250),
        ]);

        let usage = repository
            .summarize_usage_by_provider_api_key_ids(&["provider-key-1".to_string()])
            .await
            .expect("summary should succeed");
        let item = usage
            .get("provider-key-1")
            .expect("provider key summary should exist");

        assert_eq!(item.request_count, 2);
        assert_eq!(item.total_tokens, 300);
        assert_eq!(item.total_cost_usd, 0.24);
        assert_eq!(item.last_used_at_unix_secs, Some(1_711_000_250));
    }

    #[tokio::test]
    async fn list_usage_audits_applies_second_based_time_filters() {
        let repository = InMemoryUsageReadRepository::seed(vec![
            sample_usage("req-1", 1),
            sample_usage("req-2", 2),
            sample_usage("req-3", 3),
        ]);

        let items = repository
            .list_usage_audits(&crate::repository::usage::UsageAuditListQuery {
                created_from_unix_secs: Some(2),
                created_until_unix_secs: Some(3),
                ..Default::default()
            })
            .await
            .expect("list should succeed");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].request_id, "req-2");
    }

    #[tokio::test]
    async fn summarizes_provider_api_key_last_used_at_in_seconds() {
        let repository = InMemoryUsageReadRepository::seed(vec![
            sample_usage("req-1", 1_999),
            sample_usage("req-2", 2_500),
        ]);

        let summary = repository
            .summarize_usage_by_provider_api_key_ids(&["provider-key-1".to_string()])
            .await
            .expect("summary should succeed");

        let usage = summary
            .get("provider-key-1")
            .expect("provider key summary should exist");
        assert_eq!(usage.request_count, 2);
        assert_eq!(usage.last_used_at_unix_secs, Some(2_500));
    }
}
