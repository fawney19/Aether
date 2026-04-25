use aether_data_contracts::repository::usage::{
    parse_usage_body_ref, usage_body_ref, StoredUsageAuditAggregation, StoredUsageAuditSummary,
    StoredUsageBreakdownSummaryRow, StoredUsageCacheAffinityHitSummary,
    StoredUsageCacheAffinityIntervalRow, StoredUsageCacheHitSummary, StoredUsageCostSavingsSummary,
    StoredUsageDashboardDailyBreakdownRow, StoredUsageDashboardProviderCount,
    StoredUsageDashboardSummary, StoredUsageErrorDistributionRow, StoredUsageLeaderboardSummary,
    StoredUsagePerformancePercentilesRow, StoredUsageSettledCostSummary,
    StoredUsageTimeSeriesBucket, StoredUsageUserTotals, UsageAuditAggregationGroupBy,
    UsageAuditAggregationQuery, UsageAuditKeywordSearchQuery, UsageAuditSummaryQuery,
    UsageBodyCaptureState, UsageBodyField, UsageBreakdownGroupBy, UsageBreakdownSummaryQuery,
    UsageCacheAffinityHitSummaryQuery, UsageCacheAffinityIntervalGroupBy,
    UsageCacheAffinityIntervalQuery, UsageCacheHitSummaryQuery, UsageCostSavingsSummaryQuery,
    UsageDashboardDailyBreakdownQuery, UsageDashboardProviderCountsQuery,
    UsageDashboardSummaryQuery, UsageErrorDistributionQuery, UsageLeaderboardGroupBy,
    UsageLeaderboardQuery, UsageMonitoringErrorCountQuery, UsageMonitoringErrorListQuery,
    UsagePerformancePercentilesQuery, UsageSettledCostSummaryQuery, UsageTimeSeriesGranularity,
    UsageTimeSeriesQuery,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use futures_util::future::BoxFuture;
use futures_util::TryStreamExt;
use serde_json::Map;
use serde_json::Value;
use sqlx::{
    postgres::{PgArguments, PgRow},
    query::Query,
    PgPool, Postgres, QueryBuilder, Row,
};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use uuid::Uuid;

use super::{
    api_key_usage_contribution, incoming_usage_can_recover_terminal_failure,
    provider_api_key_usage_contribution, strip_deprecated_usage_display_fields, ApiKeyUsageDelta,
    ProviderApiKeyUsageDelta, StoredProviderApiKeyUsageSummary, StoredProviderUsageSummary,
    StoredRequestUsageAudit, StoredUsageDailySummary, UpsertUsageRecord, UsageAuditListQuery,
    UsageDailyHeatmapQuery, UsageReadRepository, UsageWriteRepository,
};
use crate::postgres::PostgresTransactionRunner;
use crate::{
    error::{postgres_error, SqlxResultExt},
    DataLayerError,
};

// Legacy inline body columns on public.usage are deprecated. Keep the threshold at zero so
// newly captured bodies always spill to usage_body_blobs and resolve through usage_http_audits.
const MAX_INLINE_USAGE_BODY_BYTES: usize = 0;
const MAX_SUPPORTED_UNIX_SECS: u64 = 253_402_300_799;
const FIND_USAGE_BODY_BLOB_BY_REF_SQL: &str =
    r#"SELECT payload_gzip FROM usage_body_blobs WHERE body_ref = $1 LIMIT 1"#;
const UPSERT_USAGE_BODY_BLOB_SQL: &str = include_str!("queries/upsert_usage_body_blob_sql.sql");
const DELETE_USAGE_BODY_BLOB_SQL: &str = include_str!("queries/delete_usage_body_blob_sql.sql");

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct AggregateRangeSplit {
    raw_leading: Option<(DateTime<Utc>, DateTime<Utc>)>,
    aggregate: Option<(DateTime<Utc>, DateTime<Utc>)>,
    raw_trailing: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

fn dashboard_non_empty_utc_range(
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    (start_utc < end_utc).then_some((start_utc, end_utc))
}

fn dashboard_unix_secs_to_utc(unix_secs: u64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(unix_secs.min(MAX_SUPPORTED_UNIX_SECS) as i64, 0)
        .expect("clamped unix timestamp should be valid")
}

fn dashboard_utc_to_unix_secs(value: DateTime<Utc>) -> u64 {
    value.timestamp().max(0) as u64
}

fn dashboard_utc_midnight(value: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::<Utc>::from_naive_utc_and_offset(
        value
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("midnight should be valid"),
        Utc,
    )
}

fn dashboard_next_utc_midnight(value: DateTime<Utc>) -> DateTime<Utc> {
    let midnight = dashboard_utc_midnight(value);
    if value == midnight {
        midnight
    } else {
        midnight + chrono::Duration::days(1)
    }
}

fn dashboard_utc_hour(value: DateTime<Utc>) -> DateTime<Utc> {
    let hour_unix_secs = value.timestamp().div_euclid(3600) * 3600;
    DateTime::<Utc>::from_timestamp(hour_unix_secs, 0).expect("hour-aligned timestamp is valid")
}

fn dashboard_next_utc_hour(value: DateTime<Utc>) -> DateTime<Utc> {
    let hour_utc = dashboard_utc_hour(value);
    if value == hour_utc {
        hour_utc
    } else {
        hour_utc + chrono::Duration::hours(1)
    }
}

fn split_dashboard_daily_aggregate_range(
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
    cutoff_utc: DateTime<Utc>,
) -> AggregateRangeSplit {
    if start_utc >= end_utc {
        return AggregateRangeSplit::default();
    }

    let aggregate_start = dashboard_next_utc_midnight(start_utc);
    let aggregate_end = dashboard_utc_midnight(end_utc).min(cutoff_utc);

    if aggregate_start < aggregate_end {
        let leading_end = aggregate_start.min(end_utc);
        AggregateRangeSplit {
            raw_leading: dashboard_non_empty_utc_range(start_utc, leading_end),
            aggregate: dashboard_non_empty_utc_range(aggregate_start, aggregate_end),
            raw_trailing: dashboard_non_empty_utc_range(aggregate_end, end_utc),
        }
    } else {
        AggregateRangeSplit {
            raw_leading: dashboard_non_empty_utc_range(start_utc, end_utc),
            aggregate: None,
            raw_trailing: None,
        }
    }
}

fn split_dashboard_hourly_aggregate_range(
    start_utc: DateTime<Utc>,
    end_utc: DateTime<Utc>,
    cutoff_utc: DateTime<Utc>,
) -> AggregateRangeSplit {
    if start_utc >= end_utc {
        return AggregateRangeSplit::default();
    }

    let aggregate_start = dashboard_next_utc_hour(start_utc);
    let aggregate_end = dashboard_utc_hour(end_utc).min(cutoff_utc);

    if aggregate_start < aggregate_end {
        let leading_end = aggregate_start.min(end_utc);
        AggregateRangeSplit {
            raw_leading: dashboard_non_empty_utc_range(start_utc, leading_end),
            aggregate: dashboard_non_empty_utc_range(aggregate_start, aggregate_end),
            raw_trailing: dashboard_non_empty_utc_range(aggregate_end, end_utc),
        }
    } else {
        AggregateRangeSplit {
            raw_leading: dashboard_non_empty_utc_range(start_utc, end_utc),
            aggregate: None,
            raw_trailing: None,
        }
    }
}

fn absorb_dashboard_summary(
    target: &mut StoredUsageDashboardSummary,
    part: &StoredUsageDashboardSummary,
) {
    target.total_requests = target.total_requests.saturating_add(part.total_requests);
    target.input_tokens = target.input_tokens.saturating_add(part.input_tokens);
    target.effective_input_tokens = target
        .effective_input_tokens
        .saturating_add(part.effective_input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(part.output_tokens);
    target.total_tokens = target.total_tokens.saturating_add(part.total_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .saturating_add(part.cache_creation_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(part.cache_read_tokens);
    target.total_input_context = target
        .total_input_context
        .saturating_add(part.total_input_context);
    target.cache_creation_cost_usd += part.cache_creation_cost_usd;
    target.cache_read_cost_usd += part.cache_read_cost_usd;
    target.total_cost_usd += part.total_cost_usd;
    target.actual_total_cost_usd += part.actual_total_cost_usd;
    target.error_requests = target.error_requests.saturating_add(part.error_requests);
    target.response_time_sum_ms += part.response_time_sum_ms;
    target.response_time_samples = target
        .response_time_samples
        .saturating_add(part.response_time_samples);
}

fn absorb_dashboard_provider_counts(
    target: &mut BTreeMap<String, u64>,
    rows: Vec<StoredUsageDashboardProviderCount>,
) {
    for row in rows {
        let entry = target.entry(row.provider_name).or_default();
        *entry = entry.saturating_add(row.request_count);
    }
}

fn finalize_dashboard_provider_counts(
    grouped: BTreeMap<String, u64>,
) -> Vec<StoredUsageDashboardProviderCount> {
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
    items
}

fn decode_dashboard_summary_row(
    row: &PgRow,
) -> Result<StoredUsageDashboardSummary, DataLayerError> {
    Ok(StoredUsageDashboardSummary {
        total_requests: row
            .try_get::<i64, _>("total_requests")
            .map_postgres_err()?
            .max(0) as u64,
        input_tokens: row
            .try_get::<i64, _>("input_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        effective_input_tokens: row
            .try_get::<i64, _>("effective_input_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        output_tokens: row
            .try_get::<i64, _>("output_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_tokens: row
            .try_get::<i64, _>("total_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_tokens: row
            .try_get::<i64, _>("cache_creation_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_read_tokens: row
            .try_get::<i64, _>("cache_read_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_input_context: row
            .try_get::<i64, _>("total_input_context")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_cost_usd: row
            .try_get::<f64, _>("cache_creation_cost_usd")
            .map_postgres_err()?,
        cache_read_cost_usd: row
            .try_get::<f64, _>("cache_read_cost_usd")
            .map_postgres_err()?,
        total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
        actual_total_cost_usd: row
            .try_get::<f64, _>("actual_total_cost_usd")
            .map_postgres_err()?,
        error_requests: row
            .try_get::<i64, _>("error_requests")
            .map_postgres_err()?
            .max(0) as u64,
        response_time_sum_ms: row
            .try_get::<f64, _>("response_time_sum_ms")
            .map_postgres_err()?,
        response_time_samples: row
            .try_get::<i64, _>("response_time_samples")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn finalize_dashboard_daily_breakdown_rows(
    mut items: Vec<StoredUsageDashboardDailyBreakdownRow>,
) -> Vec<StoredUsageDashboardDailyBreakdownRow> {
    items.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| {
                right
                    .total_cost_usd
                    .partial_cmp(&left.total_cost_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.model.cmp(&right.model))
            .then_with(|| left.provider.cmp(&right.provider))
    });
    items
}

fn decode_dashboard_daily_breakdown_row(
    row: &PgRow,
) -> Result<StoredUsageDashboardDailyBreakdownRow, DataLayerError> {
    Ok(StoredUsageDashboardDailyBreakdownRow {
        date: row.try_get::<String, _>("date").map_postgres_err()?,
        model: row.try_get::<String, _>("model").map_postgres_err()?,
        provider: row.try_get::<String, _>("provider").map_postgres_err()?,
        requests: row.try_get::<i64, _>("requests").map_postgres_err()?.max(0) as u64,
        total_tokens: row
            .try_get::<i64, _>("total_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
        response_time_sum_ms: row
            .try_get::<f64, _>("response_time_sum_ms")
            .map_postgres_err()?,
        response_time_samples: row
            .try_get::<i64, _>("response_time_samples")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn absorb_usage_breakdown_rows(
    target: &mut BTreeMap<String, StoredUsageBreakdownSummaryRow>,
    rows: Vec<StoredUsageBreakdownSummaryRow>,
) {
    for row in rows {
        let group_key = row.group_key.clone();
        let entry =
            target
                .entry(group_key.clone())
                .or_insert_with(|| StoredUsageBreakdownSummaryRow {
                    group_key,
                    ..Default::default()
                });
        entry.request_count = entry.request_count.saturating_add(row.request_count);
        entry.input_tokens = entry.input_tokens.saturating_add(row.input_tokens);
        entry.total_tokens = entry.total_tokens.saturating_add(row.total_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(row.output_tokens);
        entry.effective_input_tokens = entry
            .effective_input_tokens
            .saturating_add(row.effective_input_tokens);
        entry.total_input_context = entry
            .total_input_context
            .saturating_add(row.total_input_context);
        entry.cache_creation_tokens = entry
            .cache_creation_tokens
            .saturating_add(row.cache_creation_tokens);
        entry.cache_creation_ephemeral_5m_tokens = entry
            .cache_creation_ephemeral_5m_tokens
            .saturating_add(row.cache_creation_ephemeral_5m_tokens);
        entry.cache_creation_ephemeral_1h_tokens = entry
            .cache_creation_ephemeral_1h_tokens
            .saturating_add(row.cache_creation_ephemeral_1h_tokens);
        entry.cache_read_tokens = entry
            .cache_read_tokens
            .saturating_add(row.cache_read_tokens);
        entry.total_cost_usd += row.total_cost_usd;
        entry.actual_total_cost_usd += row.actual_total_cost_usd;
        entry.success_count = entry.success_count.saturating_add(row.success_count);
        entry.response_time_sum_ms += row.response_time_sum_ms;
        entry.response_time_samples = entry
            .response_time_samples
            .saturating_add(row.response_time_samples);
        entry.overall_response_time_sum_ms += row.overall_response_time_sum_ms;
        entry.overall_response_time_samples = entry
            .overall_response_time_samples
            .saturating_add(row.overall_response_time_samples);
    }
}

fn finalize_usage_breakdown_rows(
    grouped: BTreeMap<String, StoredUsageBreakdownSummaryRow>,
) -> Vec<StoredUsageBreakdownSummaryRow> {
    let mut items = grouped.into_values().collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .request_count
            .cmp(&left.request_count)
            .then_with(|| left.group_key.cmp(&right.group_key))
    });
    items
}

fn decode_usage_breakdown_summary_row(
    row: &PgRow,
) -> Result<StoredUsageBreakdownSummaryRow, DataLayerError> {
    Ok(StoredUsageBreakdownSummaryRow {
        group_key: row.try_get::<String, _>("group_key").map_postgres_err()?,
        request_count: row
            .try_get::<i64, _>("request_count")
            .map_postgres_err()?
            .max(0) as u64,
        input_tokens: row
            .try_get::<i64, _>("input_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_tokens: row
            .try_get::<i64, _>("total_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        output_tokens: row
            .try_get::<i64, _>("output_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        effective_input_tokens: row
            .try_get::<i64, _>("effective_input_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_input_context: row
            .try_get::<i64, _>("total_input_context")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_tokens: row
            .try_get::<i64, _>("cache_creation_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_ephemeral_5m_tokens: row
            .try_get::<i64, _>("cache_creation_ephemeral_5m_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_ephemeral_1h_tokens: row
            .try_get::<i64, _>("cache_creation_ephemeral_1h_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_read_tokens: row
            .try_get::<i64, _>("cache_read_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
        actual_total_cost_usd: row
            .try_get::<f64, _>("actual_total_cost_usd")
            .map_postgres_err()?,
        success_count: row
            .try_get::<i64, _>("success_count")
            .map_postgres_err()?
            .max(0) as u64,
        response_time_sum_ms: row
            .try_get::<f64, _>("response_time_sum_ms")
            .map_postgres_err()?,
        response_time_samples: row
            .try_get::<i64, _>("response_time_samples")
            .map_postgres_err()?
            .max(0) as u64,
        overall_response_time_sum_ms: row
            .try_get::<f64, _>("overall_response_time_sum_ms")
            .map_postgres_err()?,
        overall_response_time_samples: row
            .try_get::<i64, _>("overall_response_time_samples")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn absorb_usage_audit_summary(target: &mut StoredUsageAuditSummary, row: StoredUsageAuditSummary) {
    target.total_requests = target.total_requests.saturating_add(row.total_requests);
    target.input_tokens = target.input_tokens.saturating_add(row.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(row.output_tokens);
    target.recorded_total_tokens = target
        .recorded_total_tokens
        .saturating_add(row.recorded_total_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .saturating_add(row.cache_creation_tokens);
    target.cache_creation_ephemeral_5m_tokens = target
        .cache_creation_ephemeral_5m_tokens
        .saturating_add(row.cache_creation_ephemeral_5m_tokens);
    target.cache_creation_ephemeral_1h_tokens = target
        .cache_creation_ephemeral_1h_tokens
        .saturating_add(row.cache_creation_ephemeral_1h_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(row.cache_read_tokens);
    target.total_cost_usd += row.total_cost_usd;
    target.actual_total_cost_usd += row.actual_total_cost_usd;
    target.cache_creation_cost_usd += row.cache_creation_cost_usd;
    target.cache_read_cost_usd += row.cache_read_cost_usd;
    target.total_response_time_ms += row.total_response_time_ms;
    target.error_requests = target.error_requests.saturating_add(row.error_requests);
}

fn absorb_usage_cache_hit_summary(
    target: &mut StoredUsageCacheHitSummary,
    row: StoredUsageCacheHitSummary,
) {
    target.total_requests = target.total_requests.saturating_add(row.total_requests);
    target.cache_hit_requests = target
        .cache_hit_requests
        .saturating_add(row.cache_hit_requests);
}

fn decode_usage_cache_hit_summary_row(
    row: &PgRow,
) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
    Ok(StoredUsageCacheHitSummary {
        total_requests: row
            .try_get::<i64, _>("total_requests")
            .map_postgres_err()?
            .max(0) as u64,
        cache_hit_requests: row
            .try_get::<i64, _>("cache_hit_requests")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn absorb_usage_cache_affinity_hit_summary(
    target: &mut StoredUsageCacheAffinityHitSummary,
    row: StoredUsageCacheAffinityHitSummary,
) {
    target.total_requests = target.total_requests.saturating_add(row.total_requests);
    target.requests_with_cache_hit = target
        .requests_with_cache_hit
        .saturating_add(row.requests_with_cache_hit);
    target.input_tokens = target.input_tokens.saturating_add(row.input_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(row.cache_read_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .saturating_add(row.cache_creation_tokens);
    target.total_input_context = target
        .total_input_context
        .saturating_add(row.total_input_context);
    target.cache_read_cost_usd += row.cache_read_cost_usd;
    target.cache_creation_cost_usd += row.cache_creation_cost_usd;
}

fn decode_usage_cache_affinity_hit_summary_row(
    row: &PgRow,
) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
    Ok(StoredUsageCacheAffinityHitSummary {
        total_requests: row
            .try_get::<i64, _>("total_requests")
            .map_postgres_err()?
            .max(0) as u64,
        requests_with_cache_hit: row
            .try_get::<i64, _>("requests_with_cache_hit")
            .map_postgres_err()?
            .max(0) as u64,
        input_tokens: row
            .try_get::<i64, _>("input_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_read_tokens: row
            .try_get::<i64, _>("cache_read_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_tokens: row
            .try_get::<i64, _>("cache_creation_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_input_context: row
            .try_get::<i64, _>("total_input_context")
            .map_postgres_err()?
            .max(0) as u64,
        cache_read_cost_usd: row
            .try_get::<f64, _>("cache_read_cost_usd")
            .map_postgres_err()?,
        cache_creation_cost_usd: row
            .try_get::<f64, _>("cache_creation_cost_usd")
            .map_postgres_err()?,
    })
}

fn absorb_usage_settled_cost_summary(
    target: &mut StoredUsageSettledCostSummary,
    row: StoredUsageSettledCostSummary,
) {
    target.total_cost_usd += row.total_cost_usd;
    target.total_requests = target.total_requests.saturating_add(row.total_requests);
    target.input_tokens = target.input_tokens.saturating_add(row.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(row.output_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .saturating_add(row.cache_creation_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(row.cache_read_tokens);
    target.first_finalized_at_unix_secs = match (
        target.first_finalized_at_unix_secs,
        row.first_finalized_at_unix_secs,
    ) {
        (Some(existing), Some(candidate)) => Some(existing.min(candidate)),
        (None, candidate) => candidate,
        (existing, None) => existing,
    };
    target.last_finalized_at_unix_secs = match (
        target.last_finalized_at_unix_secs,
        row.last_finalized_at_unix_secs,
    ) {
        (Some(existing), Some(candidate)) => Some(existing.max(candidate)),
        (None, candidate) => candidate,
        (existing, None) => existing,
    };
}

fn decode_usage_settled_cost_row(
    row: &PgRow,
) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
    Ok(StoredUsageSettledCostSummary {
        total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
        total_requests: row
            .try_get::<i64, _>("total_requests")
            .map_postgres_err()?
            .max(0) as u64,
        input_tokens: row
            .try_get::<i64, _>("input_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        output_tokens: row
            .try_get::<i64, _>("output_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_tokens: row
            .try_get::<i64, _>("cache_creation_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_read_tokens: row
            .try_get::<i64, _>("cache_read_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        first_finalized_at_unix_secs: row
            .try_get::<Option<i64>, _>("first_finalized_at_unix_secs")
            .map_postgres_err()?
            .map(|value| value.max(0) as u64),
        last_finalized_at_unix_secs: row
            .try_get::<Option<i64>, _>("last_finalized_at_unix_secs")
            .map_postgres_err()?
            .map(|value| value.max(0) as u64),
    })
}

fn absorb_usage_cost_savings_summary(
    target: &mut StoredUsageCostSavingsSummary,
    row: StoredUsageCostSavingsSummary,
) {
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(row.cache_read_tokens);
    target.cache_read_cost_usd += row.cache_read_cost_usd;
    target.cache_creation_cost_usd += row.cache_creation_cost_usd;
    target.estimated_full_cost_usd += row.estimated_full_cost_usd;
}

fn decode_usage_cost_savings_row(
    row: &PgRow,
) -> Result<StoredUsageCostSavingsSummary, DataLayerError> {
    Ok(StoredUsageCostSavingsSummary {
        cache_read_tokens: row
            .try_get::<i64, _>("cache_read_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_read_cost_usd: row
            .try_get::<f64, _>("cache_read_cost_usd")
            .map_postgres_err()?,
        cache_creation_cost_usd: row
            .try_get::<f64, _>("cache_creation_cost_usd")
            .map_postgres_err()?,
        estimated_full_cost_usd: row
            .try_get::<f64, _>("estimated_full_cost_usd")
            .map_postgres_err()?,
    })
}

fn decode_usage_audit_summary_row(row: &PgRow) -> Result<StoredUsageAuditSummary, DataLayerError> {
    Ok(StoredUsageAuditSummary {
        total_requests: row
            .try_get::<i64, _>("total_requests")
            .map_postgres_err()?
            .max(0) as u64,
        input_tokens: row
            .try_get::<i64, _>("input_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        output_tokens: row
            .try_get::<i64, _>("output_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        recorded_total_tokens: row
            .try_get::<i64, _>("recorded_total_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_tokens: row
            .try_get::<i64, _>("cache_creation_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_ephemeral_5m_tokens: row
            .try_get::<i64, _>("cache_creation_ephemeral_5m_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_ephemeral_1h_tokens: row
            .try_get::<i64, _>("cache_creation_ephemeral_1h_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_read_tokens: row
            .try_get::<i64, _>("cache_read_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
        actual_total_cost_usd: row
            .try_get::<f64, _>("actual_total_cost_usd")
            .map_postgres_err()?,
        cache_creation_cost_usd: row
            .try_get::<f64, _>("cache_creation_cost_usd")
            .map_postgres_err()?,
        cache_read_cost_usd: row
            .try_get::<f64, _>("cache_read_cost_usd")
            .map_postgres_err()?,
        total_response_time_ms: row
            .try_get::<f64, _>("total_response_time_ms")
            .map_postgres_err()?,
        error_requests: row
            .try_get::<i64, _>("error_requests")
            .map_postgres_err()?
            .max(0) as u64,
    })
}

fn decode_usage_error_distribution_row(
    row: &PgRow,
) -> Result<StoredUsageErrorDistributionRow, DataLayerError> {
    Ok(StoredUsageErrorDistributionRow {
        date: row.try_get::<String, _>("date").map_postgres_err()?,
        error_category: row
            .try_get::<String, _>("error_category")
            .map_postgres_err()?,
        count: row.try_get::<i64, _>("count").map_postgres_err()?.max(0) as u64,
    })
}

fn absorb_usage_error_distribution_rows(
    target: &mut BTreeMap<(String, String), u64>,
    rows: Vec<StoredUsageErrorDistributionRow>,
) {
    for row in rows {
        let key = (row.date, row.error_category);
        let entry = target.entry(key).or_default();
        *entry = entry.saturating_add(row.count);
    }
}

fn finalize_usage_error_distribution_rows(
    grouped: BTreeMap<(String, String), u64>,
) -> Vec<StoredUsageErrorDistributionRow> {
    let mut items = grouped
        .into_iter()
        .map(
            |((date, error_category), count)| StoredUsageErrorDistributionRow {
                date,
                error_category,
                count,
            },
        )
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.error_category.cmp(&right.error_category))
    });
    items
}

fn decode_usage_performance_percentiles_row(
    row: &PgRow,
) -> Result<StoredUsagePerformancePercentilesRow, DataLayerError> {
    Ok(StoredUsagePerformancePercentilesRow {
        date: row.try_get::<String, _>("date").map_postgres_err()?,
        p50_response_time_ms: row
            .try_get::<Option<i64>, _>("p50_response_time_ms")
            .map_postgres_err()?
            .map(|value| value.max(0) as u64),
        p90_response_time_ms: row
            .try_get::<Option<i64>, _>("p90_response_time_ms")
            .map_postgres_err()?
            .map(|value| value.max(0) as u64),
        p99_response_time_ms: row
            .try_get::<Option<i64>, _>("p99_response_time_ms")
            .map_postgres_err()?
            .map(|value| value.max(0) as u64),
        p50_first_byte_time_ms: row
            .try_get::<Option<i64>, _>("p50_first_byte_time_ms")
            .map_postgres_err()?
            .map(|value| value.max(0) as u64),
        p90_first_byte_time_ms: row
            .try_get::<Option<i64>, _>("p90_first_byte_time_ms")
            .map_postgres_err()?
            .map(|value| value.max(0) as u64),
        p99_first_byte_time_ms: row
            .try_get::<Option<i64>, _>("p99_first_byte_time_ms")
            .map_postgres_err()?
            .map(|value| value.max(0) as u64),
    })
}

fn decode_usage_time_series_bucket_row(
    row: &PgRow,
) -> Result<StoredUsageTimeSeriesBucket, DataLayerError> {
    Ok(StoredUsageTimeSeriesBucket {
        bucket_key: row.try_get::<String, _>("bucket_key").map_postgres_err()?,
        total_requests: row
            .try_get::<i64, _>("total_requests")
            .map_postgres_err()?
            .max(0) as u64,
        input_tokens: row
            .try_get::<i64, _>("input_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        output_tokens: row
            .try_get::<i64, _>("output_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_creation_tokens: row
            .try_get::<i64, _>("cache_creation_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        cache_read_tokens: row
            .try_get::<i64, _>("cache_read_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
        total_response_time_ms: row
            .try_get::<f64, _>("total_response_time_ms")
            .map_postgres_err()?,
    })
}

fn absorb_usage_time_series_buckets(
    target: &mut BTreeMap<String, StoredUsageTimeSeriesBucket>,
    buckets: Vec<StoredUsageTimeSeriesBucket>,
) {
    for bucket in buckets {
        let entry = target.entry(bucket.bucket_key.clone()).or_insert_with(|| {
            StoredUsageTimeSeriesBucket {
                bucket_key: bucket.bucket_key.clone(),
                ..Default::default()
            }
        });
        entry.total_requests = entry.total_requests.saturating_add(bucket.total_requests);
        entry.input_tokens = entry.input_tokens.saturating_add(bucket.input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(bucket.output_tokens);
        entry.cache_creation_tokens = entry
            .cache_creation_tokens
            .saturating_add(bucket.cache_creation_tokens);
        entry.cache_read_tokens = entry
            .cache_read_tokens
            .saturating_add(bucket.cache_read_tokens);
        entry.total_cost_usd += bucket.total_cost_usd;
        entry.total_response_time_ms += bucket.total_response_time_ms;
    }
}

fn finalize_usage_time_series_buckets(
    grouped: BTreeMap<String, StoredUsageTimeSeriesBucket>,
) -> Vec<StoredUsageTimeSeriesBucket> {
    grouped.into_values().collect()
}

fn decode_usage_leaderboard_row(
    row: &PgRow,
) -> Result<StoredUsageLeaderboardSummary, DataLayerError> {
    Ok(StoredUsageLeaderboardSummary {
        group_key: row.try_get::<String, _>("group_key").map_postgres_err()?,
        legacy_name: row
            .try_get::<Option<String>, _>("legacy_name")
            .map_postgres_err()?,
        request_count: row
            .try_get::<i64, _>("request_count")
            .map_postgres_err()?
            .max(0) as u64,
        total_tokens: row
            .try_get::<i64, _>("total_tokens")
            .map_postgres_err()?
            .max(0) as u64,
        total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
    })
}

fn absorb_usage_leaderboard_rows(
    target: &mut BTreeMap<String, StoredUsageLeaderboardSummary>,
    rows: Vec<StoredUsageLeaderboardSummary>,
) {
    for row in rows {
        let entry =
            target
                .entry(row.group_key.clone())
                .or_insert_with(|| StoredUsageLeaderboardSummary {
                    group_key: row.group_key.clone(),
                    ..Default::default()
                });
        if entry.legacy_name.is_none() {
            entry.legacy_name = row.legacy_name.clone();
        }
        entry.request_count = entry.request_count.saturating_add(row.request_count);
        entry.total_tokens = entry.total_tokens.saturating_add(row.total_tokens);
        entry.total_cost_usd += row.total_cost_usd;
    }
}

fn finalize_usage_leaderboard_rows(
    grouped: BTreeMap<String, StoredUsageLeaderboardSummary>,
) -> Vec<StoredUsageLeaderboardSummary> {
    grouped.into_values().collect()
}

async fn fetch_usage_leaderboard_query<'q>(
    query: Query<'q, Postgres, PgArguments>,
    pool: &PgPool,
) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
    let mut rows = query.fetch(pool);
    let mut items = Vec::new();
    while let Some(row) = rows.try_next().await.map_postgres_err()? {
        items.push(decode_usage_leaderboard_row(&row)?);
    }
    Ok(items)
}
const RESET_STALE_VOID_USAGE_SQL: &str = include_str!("queries/reset_stale_void_usage_sql.sql");
const RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL: &str =
    include_str!("queries/reset_stale_void_usage_settlement_snapshot_sql.sql");
const LOCK_USAGE_REQUEST_ID_SQL: &str = include_str!("queries/lock_usage_request_id_sql.sql");
const UPSERT_USAGE_HTTP_AUDIT_SQL: &str = include_str!("queries/upsert_usage_http_audit_sql.sql");
const UPSERT_USAGE_ROUTING_SNAPSHOT_SQL: &str =
    include_str!("queries/upsert_usage_routing_snapshot_sql.sql");
const UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL: &str =
    include_str!("queries/upsert_usage_settlement_pricing_snapshot_sql.sql");

const FIND_BY_REQUEST_ID_SQL: &str = include_str!("queries/find_by_request_id_sql.sql");

const FIND_BY_ID_SQL: &str = include_str!("queries/find_by_id_sql.sql");

const SUMMARIZE_PROVIDER_USAGE_SINCE_SQL: &str =
    include_str!("queries/summarize_provider_usage_since_sql.sql");

const SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL: &str =
    include_str!("queries/summarize_total_tokens_by_api_key_ids_sql.sql");

const SUMMARIZE_USAGE_TOTALS_BY_USER_IDS_SQL: &str =
    include_str!("queries/summarize_usage_totals_by_user_ids_sql.sql");

const SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL: &str =
    include_str!("queries/summarize_usage_by_provider_api_key_ids_sql.sql");

const APPLY_API_KEY_USAGE_DELTA_SQL: &str =
    include_str!("queries/apply_api_key_usage_delta_sql.sql");

const RESET_API_KEY_USAGE_STATS_SQL: &str =
    include_str!("queries/reset_api_key_usage_stats_sql.sql");

const REBUILD_API_KEY_USAGE_STATS_SQL: &str =
    include_str!("queries/rebuild_api_key_usage_stats_sql.sql");

const APPLY_PROVIDER_API_KEY_USAGE_DELTA_SQL: &str =
    include_str!("queries/apply_provider_api_key_usage_delta_sql.sql");

const RESET_PROVIDER_API_KEY_USAGE_STATS_SQL: &str =
    include_str!("queries/reset_provider_api_key_usage_stats_sql.sql");

const REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL: &str =
    include_str!("queries/rebuild_provider_api_key_usage_stats_sql.sql");

const LIST_USAGE_AUDITS_PREFIX: &str = include_str!("queries/list_usage_audits_prefix.sql");

struct UsageAuditAggregationSqlFragments {
    filtered_extra_where: &'static str,
    group_key_expr: &'static str,
    display_name_expr: &'static str,
    secondary_name_expr: &'static str,
    aggregate_display_name_expr: &'static str,
    aggregate_secondary_name_expr: &'static str,
    avg_response_time_expr: &'static str,
    success_count_expr: &'static str,
}

fn usage_audit_aggregation_sql_fragments(
    group_by: UsageAuditAggregationGroupBy,
) -> UsageAuditAggregationSqlFragments {
    match group_by {
        UsageAuditAggregationGroupBy::Model => UsageAuditAggregationSqlFragments {
            filtered_extra_where: "",
            group_key_expr: "model",
            display_name_expr: "NULL::varchar",
            secondary_name_expr: "NULL::varchar",
            aggregate_display_name_expr: "NULL::varchar",
            aggregate_secondary_name_expr: "NULL::varchar",
            avg_response_time_expr: "NULL::DOUBLE PRECISION",
            success_count_expr: "NULL::BIGINT",
        },
        UsageAuditAggregationGroupBy::Provider => UsageAuditAggregationSqlFragments {
            filtered_extra_where: "",
            group_key_expr: "provider_group_key",
            display_name_expr: "provider_display_name",
            secondary_name_expr: "NULL::varchar",
            aggregate_display_name_expr:
                "COALESCE(MAX(NULLIF(display_name, 'Unknown')), 'Unknown')",
            aggregate_secondary_name_expr: "NULL::varchar",
            avg_response_time_expr: "AVG(response_time_ms::DOUBLE PRECISION)",
            success_count_expr: "COALESCE(SUM(success_flag), 0)::BIGINT",
        },
        UsageAuditAggregationGroupBy::ApiFormat => UsageAuditAggregationSqlFragments {
            filtered_extra_where: "",
            group_key_expr: "api_format_group_key",
            display_name_expr: "NULL::varchar",
            secondary_name_expr: "NULL::varchar",
            aggregate_display_name_expr: "NULL::varchar",
            aggregate_secondary_name_expr: "NULL::varchar",
            avg_response_time_expr: "AVG(response_time_ms::DOUBLE PRECISION)",
            success_count_expr: "NULL::BIGINT",
        },
        UsageAuditAggregationGroupBy::User => UsageAuditAggregationSqlFragments {
            filtered_extra_where: " AND \"usage\".user_id IS NOT NULL",
            group_key_expr: "user_id",
            display_name_expr: "NULL::varchar",
            secondary_name_expr: "NULL::varchar",
            aggregate_display_name_expr: "NULL::varchar",
            aggregate_secondary_name_expr: "NULL::varchar",
            avg_response_time_expr: "NULL::DOUBLE PRECISION",
            success_count_expr: "NULL::BIGINT",
        },
    }
}

struct UsageLeaderboardSqlFragments {
    filtered_extra_where: &'static str,
    group_key_expr: &'static str,
    legacy_name_expr: &'static str,
}

fn usage_leaderboard_sql_fragments(
    group_by: UsageLeaderboardGroupBy,
) -> UsageLeaderboardSqlFragments {
    match group_by {
        UsageLeaderboardGroupBy::Model => UsageLeaderboardSqlFragments {
            filtered_extra_where: "",
            group_key_expr: "\"usage\".model",
            legacy_name_expr: "NULL::varchar",
        },
        UsageLeaderboardGroupBy::User => UsageLeaderboardSqlFragments {
            filtered_extra_where: " AND \"usage\".user_id IS NOT NULL",
            group_key_expr: "\"usage\".user_id",
            legacy_name_expr: "NULLIF(BTRIM(\"usage\".username), '')",
        },
        UsageLeaderboardGroupBy::ApiKey => UsageLeaderboardSqlFragments {
            filtered_extra_where: " AND \"usage\".api_key_id IS NOT NULL",
            group_key_expr: "\"usage\".api_key_id",
            legacy_name_expr: "NULLIF(BTRIM(\"usage\".api_key_name), '')",
        },
    }
}

const LIST_RECENT_USAGE_AUDITS_PREFIX: &str =
    include_str!("queries/list_recent_usage_audits_prefix.sql");

const UPSERT_SQL: &str = include_str!("queries/upsert_sql.sql");

#[derive(Debug, Clone)]
pub struct SqlxUsageReadRepository {
    pool: PgPool,
    tx_runner: PostgresTransactionRunner,
}

impl SqlxUsageReadRepository {
    pub fn new(pool: PgPool) -> Self {
        let tx_runner = PostgresTransactionRunner::new(pool.clone());
        Self { pool, tx_runner }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn transaction_runner(&self) -> &PostgresTransactionRunner {
        &self.tx_runner
    }

    async fn read_stats_daily_cutoff_date(&self) -> Result<Option<DateTime<Utc>>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT cutoff_date
FROM stats_summary
ORDER BY updated_at DESC, created_at DESC
LIMIT 1
"#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;

        row.map(|row| {
            row.try_get::<DateTime<Utc>, _>("cutoff_date")
                .map_postgres_err()
        })
        .transpose()
    }

    async fn read_stats_hourly_cutoff(&self) -> Result<Option<DateTime<Utc>>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT MAX(hour_utc) AS latest_hour
FROM stats_hourly
WHERE is_complete IS TRUE
"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        let latest_hour = row
            .try_get::<Option<DateTime<Utc>>, _>("latest_hour")
            .map_postgres_err()?;
        Ok(latest_hour.map(|value| value + chrono::Duration::hours(1)))
    }

    async fn summarize_dashboard_usage_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(StoredUsageDashboardSummary::default());
        }

        let row = if let Some(user_id) = user_id {
            sqlx::query(
                r#"
SELECT
  COALESCE(SUM(total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(effective_input_tokens), 0)::BIGINT AS effective_input_tokens,
  COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(input_tokens + output_tokens), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(total_input_context), 0)::BIGINT AS total_input_context,
  COALESCE(SUM(cache_creation_cost), 0) AS cache_creation_cost_usd,
  COALESCE(SUM(cache_read_cost), 0) AS cache_read_cost_usd,
  COALESCE(SUM(total_cost), 0) AS total_cost_usd,
  COALESCE(SUM(actual_total_cost), 0) AS actual_total_cost_usd,
  COALESCE(SUM(error_requests), 0)::BIGINT AS error_requests,
  COALESCE(SUM(response_time_sum_ms), 0) AS response_time_sum_ms,
  COALESCE(SUM(response_time_samples), 0)::BIGINT AS response_time_samples
FROM stats_user_daily
WHERE user_id = $1
  AND date >= $2
  AND date < $3
"#,
            )
            .bind(user_id)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  COALESCE(SUM(total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(effective_input_tokens), 0)::BIGINT AS effective_input_tokens,
  COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(input_tokens + output_tokens), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(total_input_context), 0)::BIGINT AS total_input_context,
  COALESCE(SUM(cache_creation_cost), 0) AS cache_creation_cost_usd,
  COALESCE(SUM(cache_read_cost), 0) AS cache_read_cost_usd,
  COALESCE(SUM(total_cost), 0) AS total_cost_usd,
  COALESCE(SUM(actual_total_cost), 0) AS actual_total_cost_usd,
  COALESCE(SUM(error_requests), 0)::BIGINT AS error_requests,
  COALESCE(SUM(response_time_sum_ms), 0) AS response_time_sum_ms,
  COALESCE(SUM(response_time_samples), 0)::BIGINT AS response_time_samples
FROM stats_daily
WHERE date >= $1
  AND date < $2
"#,
            )
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
        };

        decode_dashboard_summary_row(&row)
    }

    async fn summarize_dashboard_usage_raw(
        &self,
        created_from_unix_secs: u64,
        created_until_unix_secs: u64,
        user_id: Option<&str>,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        if created_from_unix_secs >= created_until_unix_secs {
            return Ok(StoredUsageDashboardSummary::default());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  COUNT(*)::BIGINT AS total_requests,
  COALESCE(SUM(GREATEST(COALESCE("usage".input_tokens, 0), 0)), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(
    CASE
      WHEN GREATEST(COALESCE("usage".input_tokens, 0), 0) <= 0 THEN 0
      WHEN GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0) <= 0
      THEN GREATEST(COALESCE("usage".input_tokens, 0), 0)
      WHEN split_part(lower(COALESCE(COALESCE("usage".endpoint_api_format, "usage".api_format), '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN GREATEST(
        GREATEST(COALESCE("usage".input_tokens, 0), 0)
          - GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0),
        0
      )
      ELSE GREATEST(COALESCE("usage".input_tokens, 0), 0)
    END
  ), 0)::BIGINT AS effective_input_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".output_tokens, 0), 0)), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".total_tokens, 0), 0)), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(
    CASE
      WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
           AND (
             COALESCE("usage".cache_creation_input_tokens_5m, 0)
             + COALESCE("usage".cache_creation_input_tokens_1h, 0)
           ) > 0
      THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
         + COALESCE("usage".cache_creation_input_tokens_1h, 0)
      ELSE COALESCE("usage".cache_creation_input_tokens, 0)
    END
  ), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_read_tokens,
  COALESCE(SUM(
    CASE
      WHEN split_part(lower(COALESCE(COALESCE("usage".endpoint_api_format, "usage".api_format), '')), ':', 1)
           IN ('claude', 'anthropic')
      THEN GREATEST(COALESCE("usage".input_tokens, 0), 0)
         + CASE
             WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
                  AND (
                    COALESCE("usage".cache_creation_input_tokens_5m, 0)
                    + COALESCE("usage".cache_creation_input_tokens_1h, 0)
                  ) > 0
             THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
                + COALESCE("usage".cache_creation_input_tokens_1h, 0)
             ELSE COALESCE("usage".cache_creation_input_tokens, 0)
           END
         + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
      WHEN split_part(lower(COALESCE(COALESCE("usage".endpoint_api_format, "usage".api_format), '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN (
        CASE
          WHEN GREATEST(COALESCE("usage".input_tokens, 0), 0) <= 0 THEN 0
          WHEN GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0) <= 0
          THEN GREATEST(COALESCE("usage".input_tokens, 0), 0)
          ELSE GREATEST(
            GREATEST(COALESCE("usage".input_tokens, 0), 0)
              - GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0),
            0
          )
        END
      ) + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
      ELSE CASE
        WHEN (
          CASE
            WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
                 AND (
                   COALESCE("usage".cache_creation_input_tokens_5m, 0)
                   + COALESCE("usage".cache_creation_input_tokens_1h, 0)
                 ) > 0
            THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
               + COALESCE("usage".cache_creation_input_tokens_1h, 0)
            ELSE COALESCE("usage".cache_creation_input_tokens, 0)
          END
        ) > 0
        THEN GREATEST(COALESCE("usage".input_tokens, 0), 0)
           + (
             CASE
               WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
                    AND (
                      COALESCE("usage".cache_creation_input_tokens_5m, 0)
                      + COALESCE("usage".cache_creation_input_tokens_1h, 0)
                    ) > 0
               THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
                  + COALESCE("usage".cache_creation_input_tokens_1h, 0)
               ELSE COALESCE("usage".cache_creation_input_tokens, 0)
             END
           )
           + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
        ELSE GREATEST(COALESCE("usage".input_tokens, 0), 0)
           + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
      END
    END
  ), 0)::BIGINT AS total_input_context,
  COALESCE(SUM(COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_creation_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS actual_total_cost_usd,
  COALESCE(SUM(
    CASE
      WHEN COALESCE("usage".status_code, 0) >= 400
           OR lower(COALESCE("usage".status, '')) = 'failed'
      THEN 1
      ELSE 0
    END
  ), 0)::BIGINT AS error_requests,
  COALESCE(SUM(
    CASE
      WHEN "usage".response_time_ms IS NOT NULL
      THEN GREATEST(COALESCE("usage".response_time_ms, 0), 0)::DOUBLE PRECISION
      ELSE 0
    END
  ), 0) AS response_time_sum_ms,
  COALESCE(SUM(
    CASE
      WHEN "usage".response_time_ms IS NOT NULL THEN 1
      ELSE 0
    END
  ), 0)::BIGINT AS response_time_samples
FROM usage_billing_facts AS "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(created_until_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("\"usage\".status NOT IN ('pending', 'streaming')");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("\"usage\".provider_name NOT IN ('unknown', 'pending')");
        if let Some(user_id) = user_id {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        decode_dashboard_summary_row(&row)
    }

    async fn summarize_dashboard_provider_counts_from_hourly_aggregates(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
    ) -> Result<Vec<StoredUsageDashboardProviderCount>, DataLayerError> {
        if start_utc >= end_utc {
            return Ok(Vec::new());
        }

        let mut rows = sqlx::query(
            r#"
SELECT
  provider_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count
FROM stats_hourly_provider
WHERE hour_utc >= $1
  AND hour_utc < $2
GROUP BY provider_name
ORDER BY request_count DESC, provider_name ASC
"#,
        )
        .bind(start_utc)
        .bind(end_utc)
        .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(StoredUsageDashboardProviderCount {
                provider_name: row
                    .try_get::<String, _>("provider_name")
                    .map_postgres_err()?,
                request_count: row
                    .try_get::<i64, _>("request_count")
                    .map_postgres_err()?
                    .max(0) as u64,
            });
        }
        Ok(items)
    }

    async fn summarize_dashboard_provider_counts_raw(
        &self,
        created_from_unix_secs: u64,
        created_until_unix_secs: u64,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageDashboardProviderCount>, DataLayerError> {
        if created_from_unix_secs >= created_until_unix_secs {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  "usage".provider_name AS provider_name,
  COUNT(*)::BIGINT AS request_count
FROM usage_billing_facts AS "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(created_until_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("\"usage\".status NOT IN ('pending', 'streaming')");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("\"usage\".provider_name NOT IN ('unknown', 'pending')");
        if let Some(user_id) = user_id {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        builder.push(
            r#"
GROUP BY "usage".provider_name
ORDER BY request_count DESC, "usage".provider_name ASC
"#,
        );

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(StoredUsageDashboardProviderCount {
                provider_name: row
                    .try_get::<String, _>("provider_name")
                    .map_postgres_err()?,
                request_count: row
                    .try_get::<i64, _>("request_count")
                    .map_postgres_err()?
                    .max(0) as u64,
            });
        }
        Ok(items)
    }

    pub async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(FIND_BY_REQUEST_ID_SQL)
            .bind(request_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        let usage = row
            .as_ref()
            .map(|row| map_usage_row(row, true))
            .transpose()?;
        match usage {
            Some(usage) => self.hydrate_usage_body_refs(usage).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        let row = sqlx::query(FIND_BY_ID_SQL)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref()
            .map(|row| map_usage_row(row, false))
            .transpose()
    }

    pub async fn list_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = FIND_BY_ID_SQL.replacen(
            "WHERE \"usage\".id = $1\nLIMIT 1",
            "WHERE \"usage\".id = ANY($1::TEXT[])\nORDER BY \"usage\".created_at DESC, \"usage\".id ASC",
            1,
        );
        let mut rows = sqlx::query(&sql).bind(ids.to_vec()).fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row, false)?);
        }
        Ok(items)
    }

    pub async fn resolve_body_ref(&self, body_ref: &str) -> Result<Option<Value>, DataLayerError> {
        let blob_row = sqlx::query(FIND_USAGE_BODY_BLOB_BY_REF_SQL)
            .bind(body_ref)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        if let Some(row) = blob_row.as_ref() {
            let payload_gzip = row
                .try_get::<Vec<u8>, _>("payload_gzip")
                .map_postgres_err()?;
            return inflate_usage_json_value(&payload_gzip).map(Some);
        }
        let Some((request_id, field)) = parse_usage_body_ref(body_ref) else {
            return Ok(None);
        };
        let (inline_column, compressed_column) = usage_body_sql_columns(field);
        let row = sqlx::query(&format!(
            "SELECT {inline_column} AS inline_body, {compressed_column} AS compressed_body FROM \"usage\" WHERE request_id = $1 LIMIT 1"
        ))
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref()
            .map(|row| usage_json_column(row, "inline_body", "compressed_body", true))
            .transpose()
            .map(|value| value.and_then(|column| column.value))
    }

    async fn hydrate_usage_body_refs(
        &self,
        mut usage: StoredRequestUsageAudit,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        if usage.request_body.is_none() {
            usage.request_body = self
                .resolve_usage_body_ref(&usage, UsageBodyField::RequestBody)
                .await?;
        }
        if usage.provider_request_body.is_none() {
            usage.provider_request_body = self
                .resolve_usage_body_ref(&usage, UsageBodyField::ProviderRequestBody)
                .await?;
        }
        if usage.response_body.is_none() {
            usage.response_body = self
                .resolve_usage_body_ref(&usage, UsageBodyField::ResponseBody)
                .await?;
        }
        if usage.client_response_body.is_none() {
            usage.client_response_body = self
                .resolve_usage_body_ref(&usage, UsageBodyField::ClientResponseBody)
                .await?;
        }
        Ok(usage)
    }

    async fn resolve_usage_body_ref(
        &self,
        usage: &StoredRequestUsageAudit,
        field: UsageBodyField,
    ) -> Result<Option<Value>, DataLayerError> {
        let body_ref = usage.body_ref(field);
        match body_ref {
            Some(body_ref) => self.resolve_body_ref(body_ref).await,
            None => Ok(None),
        }
    }

    pub async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, DataLayerError> {
        let row = sqlx::query(SUMMARIZE_PROVIDER_USAGE_SINCE_SQL)
            .bind(provider_id)
            .bind(since_unix_secs as f64)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;

        Ok(StoredProviderUsageSummary {
            total_requests: row
                .try_get::<i64, _>("total_requests")
                .map_postgres_err()?
                .max(0) as u64,
            successful_requests: row
                .try_get::<i64, _>("successful_requests")
                .map_postgres_err()?
                .max(0) as u64,
            failed_requests: row
                .try_get::<i64, _>("failed_requests")
                .map_postgres_err()?
                .max(0) as u64,
            avg_response_time_ms: row
                .try_get::<f64, _>("avg_response_time_ms")
                .map_postgres_err()?,
            total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
        })
    }

    pub async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_USAGE_AUDITS_PREFIX);
        let mut has_where = false;

        if let Some(created_from_unix_secs) = query.created_from_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at >= TO_TIMESTAMP(")
                .push_bind(created_from_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(created_until_unix_secs) = query.created_until_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at < TO_TIMESTAMP(")
                .push_bind(created_until_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }
        if let Some(api_format) = query.api_format.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".api_format = ")
                .push_bind(api_format.to_string());
        }
        if let Some(statuses) = query.statuses.as_deref() {
            if !statuses.is_empty() {
                builder.push(if has_where { " AND " } else { " WHERE " });
                has_where = true;
                builder.push("\"usage\".status IN (");
                let mut separated = builder.separated(", ");
                for status in statuses {
                    separated.push_bind(status.to_string());
                }
                separated.push_unseparated(")");
            }
        }
        if let Some(is_stream) = query.is_stream {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("\"usage\".is_stream = ").push_bind(is_stream);
        }
        if query.error_only {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder.push(
                "(\"usage\".status = 'failed' \
OR COALESCE(\"usage\".status_code, 0) >= 400 \
OR (\"usage\".error_message IS NOT NULL AND BTRIM(\"usage\".error_message) <> ''))",
            );
        }

        if query.newest_first {
            builder.push(" ORDER BY \"usage\".created_at DESC, \"usage\".id ASC");
        } else {
            builder.push(" ORDER BY \"usage\".created_at ASC, \"usage\".request_id ASC");
        }
        if let Some(limit) = query.limit {
            builder.push(" LIMIT ").push_bind(limit as i64);
        }
        if let Some(offset) = query.offset {
            builder.push(" OFFSET ").push_bind(offset as i64);
        }
        let query = builder.build();
        let mut rows = query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row, false)?);
        }
        Ok(items)
    }

    pub async fn list_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_USAGE_AUDITS_PREFIX);
        let mut has_where = false;

        if let Some(created_from_unix_secs) = query.created_from_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at >= TO_TIMESTAMP(")
                .push_bind(created_from_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(created_until_unix_secs) = query.created_until_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at < TO_TIMESTAMP(")
                .push_bind(created_until_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }
        if let Some(api_format) = query.api_format.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".api_format = ")
                .push_bind(api_format.to_string());
        }
        if let Some(statuses) = query.statuses.as_deref() {
            if !statuses.is_empty() {
                builder.push(if has_where { " AND " } else { " WHERE " });
                has_where = true;
                builder.push("\"usage\".status IN (");
                let mut separated = builder.separated(", ");
                for status in statuses {
                    separated.push_bind(status.to_string());
                }
                separated.push_unseparated(")");
            }
        }
        if let Some(is_stream) = query.is_stream {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("\"usage\".is_stream = ").push_bind(is_stream);
        }
        if query.error_only {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push(
                "(\"usage\".status = 'failed' \
OR COALESCE(\"usage\".status_code, 0) >= 400 \
OR (\"usage\".error_message IS NOT NULL AND BTRIM(\"usage\".error_message) <> ''))",
            );
        }
        for (index, keyword) in query.keywords.iter().enumerate() {
            let keyword = keyword.trim();
            if keyword.is_empty() {
                continue;
            }
            let pattern = format!("%{}%", keyword.to_ascii_lowercase());
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("(");
            builder
                .push("LOWER(COALESCE(\"usage\".model, '')) LIKE ")
                .push_bind(pattern.clone());
            builder
                .push(" OR LOWER(COALESCE(\"usage\".provider_name, '')) LIKE ")
                .push_bind(pattern.clone());
            if query.auth_user_reader_available {
                let matched_user_ids = query
                    .matched_user_ids_by_keyword
                    .get(index)
                    .cloned()
                    .unwrap_or_default();
                if !matched_user_ids.is_empty() {
                    builder.push(" OR \"usage\".user_id IN (");
                    let mut separated = builder.separated(", ");
                    for user_id in matched_user_ids {
                        separated.push_bind(user_id);
                    }
                    separated.push_unseparated(")");
                }
            } else {
                builder
                    .push(" OR LOWER(COALESCE(\"usage\".username, '')) LIKE ")
                    .push_bind(pattern.clone());
            }
            if query.auth_api_key_reader_available {
                let matched_ids = query
                    .matched_api_key_ids_by_keyword
                    .get(index)
                    .cloned()
                    .unwrap_or_default();
                if !matched_ids.is_empty() {
                    builder.push(" OR \"usage\".api_key_id IN (");
                    let mut separated = builder.separated(", ");
                    for api_key_id in matched_ids {
                        separated.push_bind(api_key_id);
                    }
                    separated.push_unseparated(")");
                }
            } else {
                builder
                    .push(" OR LOWER(COALESCE(\"usage\".api_key_name, '')) LIKE ")
                    .push_bind(pattern);
            }
            builder.push(")");
        }
        if let Some(username_keyword) = query
            .username_keyword
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            builder.push(if has_where { " AND " } else { " WHERE " });
            if query.auth_user_reader_available {
                if query.matched_user_ids_for_username.is_empty() {
                    builder.push("FALSE");
                } else {
                    builder.push("\"usage\".user_id IN (");
                    let mut separated = builder.separated(", ");
                    for user_id in &query.matched_user_ids_for_username {
                        separated.push_bind(user_id.clone());
                    }
                    separated.push_unseparated(")");
                }
            } else {
                builder
                    .push("LOWER(COALESCE(\"usage\".username, '')) LIKE ")
                    .push_bind(format!("%{}%", username_keyword.to_ascii_lowercase()));
            }
        }

        if query.newest_first {
            builder.push(" ORDER BY \"usage\".created_at DESC, \"usage\".id ASC");
        } else {
            builder.push(" ORDER BY \"usage\".created_at ASC, \"usage\".request_id ASC");
        }
        if let Some(limit) = query.limit {
            builder.push(" LIMIT ").push_bind(limit as i64);
        }
        if let Some(offset) = query.offset {
            builder.push(" OFFSET ").push_bind(offset as i64);
        }

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row, false)?);
        }
        Ok(items)
    }

    pub async fn count_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<Postgres>::new(r#"SELECT COUNT(*)::BIGINT AS total FROM "usage""#);
        let mut has_where = false;

        if let Some(created_from_unix_secs) = query.created_from_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at >= TO_TIMESTAMP(")
                .push_bind(created_from_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(created_until_unix_secs) = query.created_until_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at < TO_TIMESTAMP(")
                .push_bind(created_until_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }
        if let Some(api_format) = query.api_format.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".api_format = ")
                .push_bind(api_format.to_string());
        }
        if let Some(statuses) = query.statuses.as_deref() {
            if !statuses.is_empty() {
                builder.push(if has_where { " AND " } else { " WHERE " });
                has_where = true;
                builder.push("\"usage\".status IN (");
                let mut separated = builder.separated(", ");
                for status in statuses {
                    separated.push_bind(status.to_string());
                }
                separated.push_unseparated(")");
            }
        }
        if let Some(is_stream) = query.is_stream {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("\"usage\".is_stream = ").push_bind(is_stream);
        }
        if query.error_only {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder.push(
                "(\"usage\".status = 'failed' \
OR COALESCE(\"usage\".status_code, 0) >= 400 \
OR (\"usage\".error_message IS NOT NULL AND BTRIM(\"usage\".error_message) <> ''))",
            );
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64)
    }

    pub async fn count_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<Postgres>::new(r#"SELECT COUNT(*)::BIGINT AS total FROM "usage""#);
        let mut has_where = false;

        if let Some(created_from_unix_secs) = query.created_from_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at >= TO_TIMESTAMP(")
                .push_bind(created_from_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(created_until_unix_secs) = query.created_until_unix_secs {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".created_at < TO_TIMESTAMP(")
                .push_bind(created_until_unix_secs as f64)
                .push("::double precision)");
        }
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }
        if let Some(api_format) = query.api_format.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".api_format = ")
                .push_bind(api_format.to_string());
        }
        if let Some(statuses) = query.statuses.as_deref() {
            if !statuses.is_empty() {
                builder.push(if has_where { " AND " } else { " WHERE " });
                has_where = true;
                builder.push("\"usage\".status IN (");
                let mut separated = builder.separated(", ");
                for status in statuses {
                    separated.push_bind(status.to_string());
                }
                separated.push_unseparated(")");
            }
        }
        if let Some(is_stream) = query.is_stream {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("\"usage\".is_stream = ").push_bind(is_stream);
        }
        if query.error_only {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push(
                "(\"usage\".status = 'failed' \
OR COALESCE(\"usage\".status_code, 0) >= 400 \
OR (\"usage\".error_message IS NOT NULL AND BTRIM(\"usage\".error_message) <> ''))",
            );
        }
        for (index, keyword) in query.keywords.iter().enumerate() {
            let keyword = keyword.trim();
            if keyword.is_empty() {
                continue;
            }
            let pattern = format!("%{}%", keyword.to_ascii_lowercase());
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder.push("(");
            builder
                .push("LOWER(COALESCE(\"usage\".model, '')) LIKE ")
                .push_bind(pattern.clone());
            builder
                .push(" OR LOWER(COALESCE(\"usage\".provider_name, '')) LIKE ")
                .push_bind(pattern.clone());
            if query.auth_user_reader_available {
                let matched_user_ids = query
                    .matched_user_ids_by_keyword
                    .get(index)
                    .cloned()
                    .unwrap_or_default();
                if !matched_user_ids.is_empty() {
                    builder.push(" OR \"usage\".user_id IN (");
                    let mut separated = builder.separated(", ");
                    for user_id in matched_user_ids {
                        separated.push_bind(user_id);
                    }
                    separated.push_unseparated(")");
                }
            } else {
                builder
                    .push(" OR LOWER(COALESCE(\"usage\".username, '')) LIKE ")
                    .push_bind(pattern.clone());
            }
            if query.auth_api_key_reader_available {
                let matched_ids = query
                    .matched_api_key_ids_by_keyword
                    .get(index)
                    .cloned()
                    .unwrap_or_default();
                if !matched_ids.is_empty() {
                    builder.push(" OR \"usage\".api_key_id IN (");
                    let mut separated = builder.separated(", ");
                    for api_key_id in matched_ids {
                        separated.push_bind(api_key_id);
                    }
                    separated.push_unseparated(")");
                }
            } else {
                builder
                    .push(" OR LOWER(COALESCE(\"usage\".api_key_name, '')) LIKE ")
                    .push_bind(pattern);
            }
            builder.push(")");
        }
        if let Some(username_keyword) = query
            .username_keyword
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            builder.push(if has_where { " AND " } else { " WHERE " });
            if query.auth_user_reader_available {
                if query.matched_user_ids_for_username.is_empty() {
                    builder.push("FALSE");
                } else {
                    builder.push("\"usage\".user_id IN (");
                    let mut separated = builder.separated(", ");
                    for user_id in &query.matched_user_ids_for_username {
                        separated.push_bind(user_id.clone());
                    }
                    separated.push_unseparated(")");
                }
            } else {
                builder
                    .push("LOWER(COALESCE(\"usage\".username, '')) LIKE ")
                    .push_bind(format!("%{}%", username_keyword.to_ascii_lowercase()));
            }
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64)
    }

    async fn summarize_usage_audits_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(StoredUsageAuditSummary::default());
        }

        let row = if let Some(user_id) = user_id {
            sqlx::query(
                r#"
SELECT
  COALESCE(SUM(total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(input_tokens + output_tokens), 0)::BIGINT AS recorded_total_tokens,
  COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(cache_creation_ephemeral_5m_tokens), 0)::BIGINT
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(cache_creation_ephemeral_1h_tokens), 0)::BIGINT
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(total_cost), 0) AS total_cost_usd,
  COALESCE(SUM(actual_total_cost), 0) AS actual_total_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0) AS cache_creation_cost_usd,
  COALESCE(SUM(cache_read_cost), 0) AS cache_read_cost_usd,
  COALESCE(SUM(response_time_sum_ms), 0) AS total_response_time_ms,
  COALESCE(SUM(error_requests), 0)::BIGINT AS error_requests
FROM stats_user_daily
WHERE user_id = $1
  AND date >= $2
  AND date < $3
"#,
            )
            .bind(user_id)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  COALESCE(SUM(total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(input_tokens + output_tokens), 0)::BIGINT AS recorded_total_tokens,
  COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(cache_creation_ephemeral_5m_tokens), 0)::BIGINT
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(cache_creation_ephemeral_1h_tokens), 0)::BIGINT
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(total_cost), 0) AS total_cost_usd,
  COALESCE(SUM(actual_total_cost), 0) AS actual_total_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0) AS cache_creation_cost_usd,
  COALESCE(SUM(cache_read_cost), 0) AS cache_read_cost_usd,
  COALESCE(SUM(response_time_sum_ms), 0) AS total_response_time_ms,
  COALESCE(SUM(error_requests), 0)::BIGINT AS error_requests
FROM stats_daily
WHERE date >= $1
  AND date < $2
"#,
            )
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
        };

        decode_usage_audit_summary_row(&row)
    }

    async fn summarize_usage_audits_raw(
        &self,
        created_from_unix_secs: u64,
        created_until_unix_secs: u64,
        user_id: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        if created_from_unix_secs >= created_until_unix_secs {
            return Ok(StoredUsageAuditSummary::default());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  COUNT(*)::BIGINT AS total_requests,
  COALESCE(SUM(GREATEST(COALESCE("usage".input_tokens, 0), 0)), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".output_tokens, 0), 0)), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".total_tokens, 0), 0)), 0)::BIGINT AS recorded_total_tokens,
  COALESCE(SUM(
    CASE
      WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
           AND (
             COALESCE("usage".cache_creation_input_tokens_5m, 0)
             + COALESCE("usage".cache_creation_input_tokens_1h, 0)
           ) > 0
      THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
         + COALESCE("usage".cache_creation_input_tokens_1h, 0)
      ELSE COALESCE("usage".cache_creation_input_tokens, 0)
    END
  ), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_creation_input_tokens_5m, 0), 0)), 0)::BIGINT
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_creation_input_tokens_1h, 0), 0)), 0)::BIGINT
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS actual_total_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_creation_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_read_cost_usd,
  COALESCE(SUM(GREATEST(COALESCE("usage".response_time_ms, 0), 0)::DOUBLE PRECISION), 0)
    AS total_response_time_ms,
  COALESCE(SUM(
    CASE
      WHEN COALESCE("usage".status_code, 0) >= 400 OR "usage".error_message IS NOT NULL THEN 1
      ELSE 0
    END
  ), 0)::BIGINT AS error_requests
FROM usage_billing_facts AS "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(created_until_unix_secs as f64)
            .push("::double precision)");
        if let Some(user_id) = user_id {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = provider_name {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = model {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        decode_usage_audit_summary_row(&row)
    }

    pub async fn summarize_usage_audits(
        &self,
        query: &UsageAuditSummaryQuery,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        if query.provider_name.is_some() || query.model.is_some() {
            return self
                .summarize_usage_audits_raw(
                    query.created_from_unix_secs,
                    query.created_until_unix_secs,
                    query.user_id.as_deref(),
                    query.provider_name.as_deref(),
                    query.model.as_deref(),
                )
                .await;
        }
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self
                .summarize_usage_audits_raw(
                    query.created_from_unix_secs,
                    query.created_until_unix_secs,
                    query.user_id.as_deref(),
                    None,
                    None,
                )
                .await;
        };

        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_usage_audits_raw(
                    query.created_from_unix_secs,
                    query.created_until_unix_secs,
                    query.user_id.as_deref(),
                    None,
                    None,
                )
                .await;
        };

        let mut summary = StoredUsageAuditSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_audit_summary(
                &mut summary,
                self.summarize_usage_audits_raw(
                    dashboard_utc_to_unix_secs(raw_start),
                    dashboard_utc_to_unix_secs(raw_end),
                    query.user_id.as_deref(),
                    None,
                    None,
                )
                .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_audit_summary(
                &mut summary,
                self.summarize_usage_audits_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                    query.user_id.as_deref(),
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_audit_summary(
                &mut summary,
                self.summarize_usage_audits_raw(
                    dashboard_utc_to_unix_secs(raw_start),
                    dashboard_utc_to_unix_secs(raw_end),
                    query.user_id.as_deref(),
                    None,
                    None,
                )
                .await?,
            );
        }

        Ok(summary)
    }

    async fn summarize_usage_cache_hit_summary_raw(
        &self,
        query: &UsageCacheHitSummaryQuery,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  COUNT(*)::BIGINT AS total_requests,
  COUNT(*) FILTER (
    WHERE GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0) > 0
  )::BIGINT AS cache_hit_requests
FROM usage_billing_facts AS "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        decode_usage_cache_hit_summary_row(&row)
    }

    async fn summarize_usage_cache_hit_summary_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  COALESCE(SUM(cache_hit_total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(cache_hit_requests), 0)::BIGINT AS cache_hit_requests
FROM stats_daily
WHERE date >= $1
  AND date < $2
"#,
        )
        .bind(start_day_utc)
        .bind(end_day_utc)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        decode_usage_cache_hit_summary_row(&row)
    }

    async fn summarize_usage_cache_hit_summary_from_hourly_aggregates(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  COALESCE(SUM(cache_hit_total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(cache_hit_requests), 0)::BIGINT AS cache_hit_requests
FROM stats_hourly
WHERE hour_utc >= $1
  AND hour_utc < $2
"#,
        )
        .bind(start_utc)
        .bind(end_utc)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        decode_usage_cache_hit_summary_row(&row)
    }

    async fn summarize_global_usage_cache_hit_summary_segment(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        if start_utc >= end_utc {
            return Ok(StoredUsageCacheHitSummary::default());
        }

        let Some(cutoff_utc) = self.read_stats_hourly_cutoff().await? else {
            return self
                .summarize_usage_cache_hit_summary_raw(&UsageCacheHitSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(start_utc),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(end_utc),
                    user_id: None,
                })
                .await;
        };

        let split = split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_usage_cache_hit_summary_raw(&UsageCacheHitSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(start_utc),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(end_utc),
                    user_id: None,
                })
                .await;
        };

        let mut summary = StoredUsageCacheHitSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_cache_hit_summary(
                &mut summary,
                self.summarize_usage_cache_hit_summary_raw(&UsageCacheHitSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    user_id: None,
                })
                .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_cache_hit_summary(
                &mut summary,
                self.summarize_usage_cache_hit_summary_from_hourly_aggregates(
                    aggregate_start,
                    aggregate_end,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_cache_hit_summary(
                &mut summary,
                self.summarize_usage_cache_hit_summary_raw(&UsageCacheHitSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    user_id: None,
                })
                .await?,
            );
        }

        Ok(summary)
    }

    pub async fn summarize_usage_cache_hit_summary(
        &self,
        query: &UsageCacheHitSummaryQuery,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        if query.user_id.is_some() {
            return self.summarize_usage_cache_hit_summary_raw(query).await;
        }

        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self
                .summarize_global_usage_cache_hit_summary_segment(start_utc, end_utc)
                .await;
        };

        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_global_usage_cache_hit_summary_segment(start_utc, end_utc)
                .await;
        };

        let mut summary = StoredUsageCacheHitSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_cache_hit_summary(
                &mut summary,
                self.summarize_global_usage_cache_hit_summary_segment(raw_start, raw_end)
                    .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_cache_hit_summary(
                &mut summary,
                self.summarize_usage_cache_hit_summary_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_cache_hit_summary(
                &mut summary,
                self.summarize_global_usage_cache_hit_summary_segment(raw_start, raw_end)
                    .await?,
            );
        }

        Ok(summary)
    }

    async fn summarize_usage_settled_cost_raw(
        &self,
        query: &UsageSettledCostSummaryQuery,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd,
  COUNT(*)::BIGINT AS total_requests,
  COALESCE(SUM(GREATEST(COALESCE("usage".input_tokens, 0), 0)), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".output_tokens, 0), 0)), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_creation_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_creation_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_read_tokens,
  MIN(CAST(EXTRACT(EPOCH FROM "usage".finalized_at) AS BIGINT))
    AS first_finalized_at_unix_secs,
  MAX(CAST(EXTRACT(EPOCH FROM "usage".finalized_at) AS BIGINT))
    AS last_finalized_at_unix_secs
FROM usage_billing_facts AS "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder.push("\"usage\".billing_status = 'settled'");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("COALESCE(CAST(\"usage\".total_cost_usd AS DOUBLE PRECISION), 0) > 0");

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        decode_usage_settled_cost_row(&row)
    }

    async fn summarize_usage_settled_cost_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        let row = if let Some(user_id) = user_id {
            sqlx::query(
                r#"
SELECT
  CAST(COALESCE(SUM(settled_total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd,
  COALESCE(SUM(settled_total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(settled_input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(settled_output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(settled_cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(settled_cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  MIN(settled_first_finalized_at_unix_secs) AS first_finalized_at_unix_secs,
  MAX(settled_last_finalized_at_unix_secs) AS last_finalized_at_unix_secs
FROM stats_user_daily
WHERE date >= $1
  AND date < $2
  AND user_id = $3
"#,
            )
            .bind(start_day_utc)
            .bind(end_day_utc)
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  CAST(COALESCE(SUM(settled_total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd,
  COALESCE(SUM(settled_total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(settled_input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(settled_output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(settled_cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(settled_cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  MIN(settled_first_finalized_at_unix_secs) AS first_finalized_at_unix_secs,
  MAX(settled_last_finalized_at_unix_secs) AS last_finalized_at_unix_secs
FROM stats_daily
WHERE date >= $1
  AND date < $2
"#,
            )
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
        };
        decode_usage_settled_cost_row(&row)
    }

    async fn summarize_usage_settled_cost_from_hourly_aggregates(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        let row = if let Some(user_id) = user_id {
            sqlx::query(
                r#"
SELECT
  CAST(COALESCE(SUM(settled_total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd,
  COALESCE(SUM(settled_total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(settled_input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(settled_output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(settled_cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(settled_cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  MIN(settled_first_finalized_at_unix_secs) AS first_finalized_at_unix_secs,
  MAX(settled_last_finalized_at_unix_secs) AS last_finalized_at_unix_secs
FROM stats_hourly_user
WHERE hour_utc >= $1
  AND hour_utc < $2
  AND user_id = $3
"#,
            )
            .bind(start_utc)
            .bind(end_utc)
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  CAST(COALESCE(SUM(settled_total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd,
  COALESCE(SUM(settled_total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(settled_input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(settled_output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(settled_cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(settled_cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  MIN(settled_first_finalized_at_unix_secs) AS first_finalized_at_unix_secs,
  MAX(settled_last_finalized_at_unix_secs) AS last_finalized_at_unix_secs
FROM stats_hourly
WHERE hour_utc >= $1
  AND hour_utc < $2
"#,
            )
            .bind(start_utc)
            .bind(end_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?
        };
        decode_usage_settled_cost_row(&row)
    }

    async fn summarize_usage_settled_cost_segment(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        if start_utc >= end_utc {
            return Ok(StoredUsageSettledCostSummary::default());
        }

        let Some(cutoff_utc) = self.read_stats_hourly_cutoff().await? else {
            return self
                .summarize_usage_settled_cost_raw(&UsageSettledCostSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(start_utc),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(end_utc),
                    user_id: user_id.map(ToOwned::to_owned),
                })
                .await;
        };

        let split = split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_usage_settled_cost_raw(&UsageSettledCostSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(start_utc),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(end_utc),
                    user_id: user_id.map(ToOwned::to_owned),
                })
                .await;
        };

        let mut summary = StoredUsageSettledCostSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_settled_cost_summary(
                &mut summary,
                self.summarize_usage_settled_cost_raw(&UsageSettledCostSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    user_id: user_id.map(ToOwned::to_owned),
                })
                .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_settled_cost_summary(
                &mut summary,
                self.summarize_usage_settled_cost_from_hourly_aggregates(
                    aggregate_start,
                    aggregate_end,
                    user_id,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_settled_cost_summary(
                &mut summary,
                self.summarize_usage_settled_cost_raw(&UsageSettledCostSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    user_id: user_id.map(ToOwned::to_owned),
                })
                .await?,
            );
        }

        Ok(summary)
    }

    pub async fn summarize_usage_settled_cost(
        &self,
        query: &UsageSettledCostSummaryQuery,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let user_id = query.user_id.as_deref();
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self
                .summarize_usage_settled_cost_segment(start_utc, end_utc, user_id)
                .await;
        };

        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_usage_settled_cost_segment(start_utc, end_utc, user_id)
                .await;
        };

        let mut summary = StoredUsageSettledCostSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_settled_cost_summary(
                &mut summary,
                self.summarize_usage_settled_cost_segment(raw_start, raw_end, user_id)
                    .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_settled_cost_summary(
                &mut summary,
                self.summarize_usage_settled_cost_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                    user_id,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_settled_cost_summary(
                &mut summary,
                self.summarize_usage_settled_cost_segment(raw_start, raw_end, user_id)
                    .await?,
            );
        }

        Ok(summary)
    }

    async fn summarize_usage_cache_affinity_hit_summary_raw(
        &self,
        query: &UsageCacheAffinityHitSummaryQuery,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  COUNT(*)::BIGINT AS total_requests,
  COALESCE(SUM(
    CASE
      WHEN GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0) > 0 THEN 1
      ELSE 0
    END
  ), 0)::BIGINT AS requests_with_cache_hit,
  COALESCE(SUM(GREATEST(COALESCE("usage".input_tokens, 0), 0)), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_read_tokens,
  COALESCE(SUM(
    CASE
      WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
           AND (
             COALESCE("usage".cache_creation_input_tokens_5m, 0)
             + COALESCE("usage".cache_creation_input_tokens_1h, 0)
           ) > 0
      THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
         + COALESCE("usage".cache_creation_input_tokens_1h, 0)
      ELSE COALESCE("usage".cache_creation_input_tokens, 0)
    END
  ), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(
    CASE
      WHEN split_part(lower(COALESCE(COALESCE("usage".endpoint_api_format, "usage".api_format), '')), ':', 1)
           IN ('claude', 'anthropic')
      THEN GREATEST(COALESCE("usage".input_tokens, 0), 0)
         + CASE
             WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
                  AND (
                    COALESCE("usage".cache_creation_input_tokens_5m, 0)
                    + COALESCE("usage".cache_creation_input_tokens_1h, 0)
                  ) > 0
             THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
                + COALESCE("usage".cache_creation_input_tokens_1h, 0)
             ELSE COALESCE("usage".cache_creation_input_tokens, 0)
           END
         + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
      WHEN split_part(lower(COALESCE(COALESCE("usage".endpoint_api_format, "usage".api_format), '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN (
        CASE
          WHEN GREATEST(COALESCE("usage".input_tokens, 0), 0) <= 0 THEN 0
          WHEN GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0) <= 0
          THEN GREATEST(COALESCE("usage".input_tokens, 0), 0)
          ELSE GREATEST(
            GREATEST(COALESCE("usage".input_tokens, 0), 0)
              - GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0),
            0
          )
        END
      ) + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
      ELSE CASE
        WHEN (
          CASE
            WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
                 AND (
                   COALESCE("usage".cache_creation_input_tokens_5m, 0)
                   + COALESCE("usage".cache_creation_input_tokens_1h, 0)
                 ) > 0
            THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
               + COALESCE("usage".cache_creation_input_tokens_1h, 0)
            ELSE COALESCE("usage".cache_creation_input_tokens, 0)
          END
        ) > 0
        THEN GREATEST(COALESCE("usage".input_tokens, 0), 0)
           + (
             CASE
               WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
                    AND (
                      COALESCE("usage".cache_creation_input_tokens_5m, 0)
                      + COALESCE("usage".cache_creation_input_tokens_1h, 0)
                    ) > 0
               THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
                  + COALESCE("usage".cache_creation_input_tokens_1h, 0)
               ELSE COALESCE("usage".cache_creation_input_tokens, 0)
             END
           )
           + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
        ELSE GREATEST(COALESCE("usage".input_tokens, 0), 0)
           + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
      END
    END
  ), 0)::BIGINT AS total_input_context,
  COALESCE(SUM(COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_creation_cost_usd
FROM usage_billing_facts AS "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder.push("\"usage\".status = 'completed'");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(api_key_id) = query.api_key_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".api_key_id = ")
                .push_bind(api_key_id.to_string());
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        decode_usage_cache_affinity_hit_summary_row(&row)
    }

    async fn summarize_usage_cache_affinity_hit_summary_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  COALESCE(SUM(completed_total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(completed_cache_hit_requests), 0)::BIGINT AS requests_with_cache_hit,
  COALESCE(SUM(completed_input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(completed_cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(completed_cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(completed_total_input_context), 0)::BIGINT AS total_input_context,
  CAST(COALESCE(SUM(completed_cache_read_cost), 0) AS DOUBLE PRECISION) AS cache_read_cost_usd,
  CAST(COALESCE(SUM(completed_cache_creation_cost), 0) AS DOUBLE PRECISION)
    AS cache_creation_cost_usd
FROM stats_daily
WHERE date >= $1
  AND date < $2
"#,
        )
        .bind(start_day_utc)
        .bind(end_day_utc)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        decode_usage_cache_affinity_hit_summary_row(&row)
    }

    async fn summarize_usage_cache_affinity_hit_summary_from_hourly_aggregates(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  COALESCE(SUM(completed_total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(completed_cache_hit_requests), 0)::BIGINT AS requests_with_cache_hit,
  COALESCE(SUM(completed_input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(completed_cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(completed_cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(completed_total_input_context), 0)::BIGINT AS total_input_context,
  CAST(COALESCE(SUM(completed_cache_read_cost), 0) AS DOUBLE PRECISION) AS cache_read_cost_usd,
  CAST(COALESCE(SUM(completed_cache_creation_cost), 0) AS DOUBLE PRECISION)
    AS cache_creation_cost_usd
FROM stats_hourly
WHERE hour_utc >= $1
  AND hour_utc < $2
"#,
        )
        .bind(start_utc)
        .bind(end_utc)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;
        decode_usage_cache_affinity_hit_summary_row(&row)
    }

    async fn summarize_global_usage_cache_affinity_hit_summary_segment(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        if start_utc >= end_utc {
            return Ok(StoredUsageCacheAffinityHitSummary::default());
        }

        let Some(cutoff_utc) = self.read_stats_hourly_cutoff().await? else {
            return self
                .summarize_usage_cache_affinity_hit_summary_raw(
                    &UsageCacheAffinityHitSummaryQuery {
                        created_from_unix_secs: dashboard_utc_to_unix_secs(start_utc),
                        created_until_unix_secs: dashboard_utc_to_unix_secs(end_utc),
                        user_id: None,
                        api_key_id: None,
                    },
                )
                .await;
        };

        let split = split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_usage_cache_affinity_hit_summary_raw(
                    &UsageCacheAffinityHitSummaryQuery {
                        created_from_unix_secs: dashboard_utc_to_unix_secs(start_utc),
                        created_until_unix_secs: dashboard_utc_to_unix_secs(end_utc),
                        user_id: None,
                        api_key_id: None,
                    },
                )
                .await;
        };

        let mut summary = StoredUsageCacheAffinityHitSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_cache_affinity_hit_summary(
                &mut summary,
                self.summarize_usage_cache_affinity_hit_summary_raw(
                    &UsageCacheAffinityHitSummaryQuery {
                        created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                        created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                        user_id: None,
                        api_key_id: None,
                    },
                )
                .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_cache_affinity_hit_summary(
                &mut summary,
                self.summarize_usage_cache_affinity_hit_summary_from_hourly_aggregates(
                    aggregate_start,
                    aggregate_end,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_cache_affinity_hit_summary(
                &mut summary,
                self.summarize_usage_cache_affinity_hit_summary_raw(
                    &UsageCacheAffinityHitSummaryQuery {
                        created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                        created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                        user_id: None,
                        api_key_id: None,
                    },
                )
                .await?,
            );
        }

        Ok(summary)
    }

    pub async fn summarize_usage_cache_affinity_hit_summary(
        &self,
        query: &UsageCacheAffinityHitSummaryQuery,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        if query.user_id.is_some() || query.api_key_id.is_some() {
            return self
                .summarize_usage_cache_affinity_hit_summary_raw(query)
                .await;
        }

        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self
                .summarize_global_usage_cache_affinity_hit_summary_segment(start_utc, end_utc)
                .await;
        };

        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_global_usage_cache_affinity_hit_summary_segment(start_utc, end_utc)
                .await;
        };

        let mut summary = StoredUsageCacheAffinityHitSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_cache_affinity_hit_summary(
                &mut summary,
                self.summarize_global_usage_cache_affinity_hit_summary_segment(raw_start, raw_end)
                    .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_cache_affinity_hit_summary(
                &mut summary,
                self.summarize_usage_cache_affinity_hit_summary_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_cache_affinity_hit_summary(
                &mut summary,
                self.summarize_global_usage_cache_affinity_hit_summary_segment(raw_start, raw_end)
                    .await?,
            );
        }

        Ok(summary)
    }

    pub async fn list_usage_cache_affinity_intervals(
        &self,
        query: &UsageCacheAffinityIntervalQuery,
    ) -> Result<Vec<StoredUsageCacheAffinityIntervalRow>, DataLayerError> {
        let group_column = match query.group_by {
            UsageCacheAffinityIntervalGroupBy::User => "\"usage\".user_id",
            UsageCacheAffinityIntervalGroupBy::ApiKey => "\"usage\".api_key_id",
        };
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
WITH filtered_usage AS (
  SELECT
    "#,
        );
        builder.push(group_column);
        builder.push(
            r#" AS group_id,
    "usage".username AS username,
    COALESCE("usage".model, '') AS model,
    "usage".created_at AS created_at,
    "usage".id AS usage_id
  FROM "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder.push("\"usage\".status = 'completed'");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push(group_column).push(" IS NOT NULL");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(api_key_id) = query.api_key_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".api_key_id = ")
                .push_bind(api_key_id.to_string());
        }
        builder.push(
            r#"
),
intervals AS (
  SELECT
    group_id,
    username,
    model,
    CAST(EXTRACT(EPOCH FROM created_at) AS BIGINT) AS created_at_unix_secs,
    usage_id,
    CAST((
      EXTRACT(EPOCH FROM (
        created_at - LAG(created_at) OVER (
          PARTITION BY group_id
          ORDER BY created_at ASC, usage_id ASC
        )
      )) / 60.0
    ) AS DOUBLE PRECISION) AS interval_minutes
  FROM filtered_usage
)
SELECT
  group_id,
  username,
  model,
  created_at_unix_secs,
  interval_minutes
FROM intervals
WHERE interval_minutes IS NOT NULL
ORDER BY created_at_unix_secs ASC, group_id ASC, usage_id ASC
"#,
        );

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(StoredUsageCacheAffinityIntervalRow {
                group_id: row.try_get::<String, _>("group_id").map_postgres_err()?,
                username: row
                    .try_get::<Option<String>, _>("username")
                    .map_postgres_err()?,
                model: row.try_get::<String, _>("model").map_postgres_err()?,
                created_at_unix_secs: row
                    .try_get::<i64, _>("created_at_unix_secs")
                    .map_postgres_err()?
                    .max(0) as u64,
                interval_minutes: row
                    .try_get::<f64, _>("interval_minutes")
                    .map_postgres_err()?,
            });
        }
        Ok(items)
    }

    pub async fn summarize_dashboard_usage(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self
                .summarize_dashboard_usage_raw(
                    query.created_from_unix_secs,
                    query.created_until_unix_secs,
                    query.user_id.as_deref(),
                )
                .await;
        };
        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_dashboard_usage_raw(
                    query.created_from_unix_secs,
                    query.created_until_unix_secs,
                    query.user_id.as_deref(),
                )
                .await;
        };

        let mut summary = StoredUsageDashboardSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            let raw = self
                .summarize_dashboard_usage_raw(
                    dashboard_utc_to_unix_secs(raw_start),
                    dashboard_utc_to_unix_secs(raw_end),
                    query.user_id.as_deref(),
                )
                .await?;
            absorb_dashboard_summary(&mut summary, &raw);
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            let aggregate = self
                .summarize_dashboard_usage_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                    query.user_id.as_deref(),
                )
                .await?;
            absorb_dashboard_summary(&mut summary, &aggregate);
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            let raw = self
                .summarize_dashboard_usage_raw(
                    dashboard_utc_to_unix_secs(raw_start),
                    dashboard_utc_to_unix_secs(raw_end),
                    query.user_id.as_deref(),
                )
                .await?;
            absorb_dashboard_summary(&mut summary, &raw);
        }

        Ok(summary)
    }

    async fn list_dashboard_daily_breakdown_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(Vec::new());
        }

        let sql = if user_id.is_some() {
            r#"
SELECT
  TO_CHAR(date, 'YYYY-MM-DD') AS date,
  model,
  provider_name AS provider,
  COALESCE(SUM(total_requests), 0)::BIGINT AS requests,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(total_cost), 0)::DOUBLE PRECISION AS total_cost_usd,
  COALESCE(SUM(response_time_sum_ms), 0) AS response_time_sum_ms,
  COALESCE(SUM(response_time_samples), 0)::BIGINT AS response_time_samples
FROM stats_user_daily_model_provider
WHERE user_id = $1
  AND date >= $2
  AND date < $3
GROUP BY date, model, provider_name
ORDER BY date ASC, total_cost_usd DESC, model ASC, provider_name ASC
"#
        } else {
            r#"
SELECT
  TO_CHAR(date, 'YYYY-MM-DD') AS date,
  model,
  provider_name AS provider,
  COALESCE(SUM(total_requests), 0)::BIGINT AS requests,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(total_cost), 0)::DOUBLE PRECISION AS total_cost_usd,
  COALESCE(SUM(response_time_sum_ms), 0) AS response_time_sum_ms,
  COALESCE(SUM(response_time_samples), 0)::BIGINT AS response_time_samples
FROM stats_daily_model_provider
WHERE date >= $1
  AND date < $2
GROUP BY date, model, provider_name
ORDER BY date ASC, total_cost_usd DESC, model ASC, provider_name ASC
"#
        };

        let mut rows = if let Some(user_id) = user_id {
            sqlx::query(sql)
                .bind(user_id)
                .bind(start_day_utc)
                .bind(end_day_utc)
                .fetch(&self.pool)
        } else {
            sqlx::query(sql)
                .bind(start_day_utc)
                .bind(end_day_utc)
                .fetch(&self.pool)
        };

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_dashboard_daily_breakdown_row(&row)?);
        }
        Ok(items)
    }

    async fn list_dashboard_daily_breakdown_raw(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  TO_CHAR(
    date_trunc('day', "usage".created_at + (
      "#,
        );
        builder.push_bind(query.tz_offset_minutes);
        builder.push(
            r#"::integer * INTERVAL '1 minute'
    )),
    'YYYY-MM-DD'
  ) AS date,
  "usage".model AS model,
  "usage".provider_name AS provider,
  COUNT(*)::BIGINT AS requests,
  COALESCE(SUM(GREATEST(COALESCE("usage".total_tokens, 0), 0)), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd,
  COALESCE(SUM(
    CASE
      WHEN "usage".response_time_ms IS NOT NULL
      THEN GREATEST(COALESCE("usage".response_time_ms, 0), 0)::DOUBLE PRECISION
      ELSE 0
    END
  ), 0) AS response_time_sum_ms,
  COALESCE(SUM(
    CASE
      WHEN "usage".response_time_ms IS NOT NULL THEN 1
      ELSE 0
    END
  ), 0)::BIGINT AS response_time_samples
FROM usage_billing_facts AS "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("\"usage\".status NOT IN ('pending', 'streaming')");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("\"usage\".provider_name NOT IN ('unknown', 'pending')");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        builder.push(
            r#"
GROUP BY date, "usage".model, "usage".provider_name
ORDER BY date ASC, total_cost_usd DESC, "usage".model ASC, "usage".provider_name ASC
"#,
        );

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_dashboard_daily_breakdown_row(&row)?);
        }
        Ok(items)
    }

    pub async fn list_dashboard_daily_breakdown(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        if query.tz_offset_minutes != 0 {
            return self.list_dashboard_daily_breakdown_raw(query).await;
        }

        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self.list_dashboard_daily_breakdown_raw(query).await;
        };
        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self.list_dashboard_daily_breakdown_raw(query).await;
        };

        let mut items = Vec::new();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            items.extend(
                self.list_dashboard_daily_breakdown_raw(&UsageDashboardDailyBreakdownQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    tz_offset_minutes: 0,
                    user_id: query.user_id.clone(),
                })
                .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            items.extend(
                self.list_dashboard_daily_breakdown_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                    query.user_id.as_deref(),
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            items.extend(
                self.list_dashboard_daily_breakdown_raw(&UsageDashboardDailyBreakdownQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    tz_offset_minutes: 0,
                    user_id: query.user_id.clone(),
                })
                .await?,
            );
        }

        Ok(finalize_dashboard_daily_breakdown_rows(items))
    }

    pub async fn summarize_dashboard_provider_counts(
        &self,
        query: &UsageDashboardProviderCountsQuery,
    ) -> Result<Vec<StoredUsageDashboardProviderCount>, DataLayerError> {
        if query.user_id.is_some() {
            return self
                .summarize_dashboard_provider_counts_raw(
                    query.created_from_unix_secs,
                    query.created_until_unix_secs,
                    query.user_id.as_deref(),
                )
                .await;
        }

        let Some(cutoff_utc) = self.read_stats_hourly_cutoff().await? else {
            return self
                .summarize_dashboard_provider_counts_raw(
                    query.created_from_unix_secs,
                    query.created_until_unix_secs,
                    None,
                )
                .await;
        };
        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let split = split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_dashboard_provider_counts_raw(
                    query.created_from_unix_secs,
                    query.created_until_unix_secs,
                    None,
                )
                .await;
        };

        let mut grouped = BTreeMap::<String, u64>::new();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            let raw = self
                .summarize_dashboard_provider_counts_raw(
                    dashboard_utc_to_unix_secs(raw_start),
                    dashboard_utc_to_unix_secs(raw_end),
                    None,
                )
                .await?;
            absorb_dashboard_provider_counts(&mut grouped, raw);
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            let aggregate = self
                .summarize_dashboard_provider_counts_from_hourly_aggregates(
                    aggregate_start,
                    aggregate_end,
                )
                .await?;
            absorb_dashboard_provider_counts(&mut grouped, aggregate);
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            let raw = self
                .summarize_dashboard_provider_counts_raw(
                    dashboard_utc_to_unix_secs(raw_start),
                    dashboard_utc_to_unix_secs(raw_end),
                    None,
                )
                .await?;
            absorb_dashboard_provider_counts(&mut grouped, raw);
        }

        Ok(finalize_dashboard_provider_counts(grouped))
    }

    async fn summarize_usage_breakdown_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        user_id: &str,
        group_by: UsageBreakdownGroupBy,
    ) -> Result<Vec<StoredUsageBreakdownSummaryRow>, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(Vec::new());
        }

        let (table_name, group_column) = match group_by {
            UsageBreakdownGroupBy::Model => ("stats_user_daily_model", "model"),
            UsageBreakdownGroupBy::Provider => ("stats_user_daily_provider", "provider_name"),
            UsageBreakdownGroupBy::ApiFormat => ("stats_user_daily_api_format", "api_format"),
        };
        let sql = format!(
            r#"
SELECT
  {group_column} AS group_key,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(effective_input_tokens), 0)::BIGINT AS effective_input_tokens,
  COALESCE(SUM(total_input_context), 0)::BIGINT AS total_input_context,
  COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(cache_creation_ephemeral_5m_tokens), 0)::BIGINT
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(cache_creation_ephemeral_1h_tokens), 0)::BIGINT
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(total_cost), 0)::DOUBLE PRECISION AS total_cost_usd,
  COALESCE(SUM(actual_total_cost), 0)::DOUBLE PRECISION AS actual_total_cost_usd,
  COALESCE(SUM(success_requests), 0)::BIGINT AS success_count,
  COALESCE(SUM(successful_response_time_sum_ms), 0) AS response_time_sum_ms,
  COALESCE(SUM(successful_response_time_samples), 0)::BIGINT AS response_time_samples,
  COALESCE(SUM(response_time_sum_ms), 0) AS overall_response_time_sum_ms,
  COALESCE(SUM(response_time_samples), 0)::BIGINT AS overall_response_time_samples
FROM {table_name}
WHERE user_id = $1
  AND date >= $2
  AND date < $3
GROUP BY {group_column}
ORDER BY request_count DESC, group_key ASC
"#,
        );

        let mut rows = sqlx::query(&sql)
            .bind(user_id)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_breakdown_summary_row(&row)?);
        }
        Ok(items)
    }

    async fn summarize_usage_breakdown_raw(
        &self,
        query: &UsageBreakdownSummaryQuery,
    ) -> Result<Vec<StoredUsageBreakdownSummaryRow>, DataLayerError> {
        let (group_key_expr, filtered_extra_where) = match query.group_by {
            UsageBreakdownGroupBy::Model => ("\"usage\".model", ""),
            UsageBreakdownGroupBy::Provider => ("\"usage\".provider_name", ""),
            UsageBreakdownGroupBy::ApiFormat => (
                "\"usage\".api_format",
                " AND \"usage\".api_format IS NOT NULL",
            ),
        };

        let mut builder = QueryBuilder::<Postgres>::new(&format!(
            r#"
WITH filtered_usage AS (
  SELECT
    {group_key_expr} AS group_key,
    GREATEST(COALESCE("usage".input_tokens, 0), 0) AS input_tokens,
    GREATEST(COALESCE("usage".output_tokens, 0), 0) AS output_tokens,
    GREATEST(COALESCE("usage".total_tokens, 0), 0) AS total_tokens,
    CASE
      WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
           AND (
             COALESCE("usage".cache_creation_input_tokens_5m, 0)
             + COALESCE("usage".cache_creation_input_tokens_1h, 0)
           ) > 0
      THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
         + COALESCE("usage".cache_creation_input_tokens_1h, 0)
      ELSE COALESCE("usage".cache_creation_input_tokens, 0)
    END AS cache_creation_tokens,
    GREATEST(COALESCE("usage".cache_creation_input_tokens_5m, 0), 0)
      AS cache_creation_ephemeral_5m_tokens,
    GREATEST(COALESCE("usage".cache_creation_input_tokens_1h, 0), 0)
      AS cache_creation_ephemeral_1h_tokens,
    GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0) AS cache_read_tokens,
    COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
    COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0) AS actual_total_cost_usd,
    CASE
      WHEN "usage".status <> 'failed'
           AND ("usage".status_code IS NULL OR "usage".status_code < 400)
           AND "usage".error_message IS NULL
      THEN 1
      ELSE 0
    END AS success_flag,
    CASE
      WHEN "usage".response_time_ms IS NOT NULL
      THEN GREATEST(COALESCE("usage".response_time_ms, 0), 0)::DOUBLE PRECISION
      ELSE 0
    END AS response_time_ms,
    CASE
      WHEN "usage".response_time_ms IS NOT NULL
      THEN 1
      ELSE 0
    END AS response_time_samples,
    CASE
      WHEN "usage".status <> 'failed'
           AND ("usage".status_code IS NULL OR "usage".status_code < 400)
           AND "usage".error_message IS NULL
           AND "usage".response_time_ms IS NOT NULL
      THEN GREATEST(COALESCE("usage".response_time_ms, 0), 0)::DOUBLE PRECISION
      ELSE 0
    END AS successful_response_time_ms,
    CASE
      WHEN "usage".status <> 'failed'
           AND ("usage".status_code IS NULL OR "usage".status_code < 400)
           AND "usage".error_message IS NULL
           AND "usage".response_time_ms IS NOT NULL
      THEN 1
      ELSE 0
    END AS successful_response_time_samples,
    COALESCE(COALESCE("usage".endpoint_api_format, "usage".api_format), '') AS normalized_api_format
  FROM usage_billing_facts AS "usage"
"#,
        ));
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("\"usage\".status NOT IN ('pending', 'streaming')");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder.push("\"usage\".provider_name NOT IN ('unknown', 'pending')");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        builder.push(filtered_extra_where);
        builder.push(
            r#"
),
normalized_usage AS (
  SELECT
    group_key,
    input_tokens,
    total_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost_usd,
    actual_total_cost_usd,
    success_flag,
    response_time_ms,
    response_time_samples,
    successful_response_time_ms,
    successful_response_time_samples,
    CASE
      WHEN input_tokens <= 0 THEN 0
      WHEN cache_read_tokens <= 0 THEN input_tokens
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN GREATEST(input_tokens - cache_read_tokens, 0)
      ELSE input_tokens
    END AS effective_input_tokens,
    CASE
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('claude', 'anthropic')
      THEN input_tokens + cache_creation_tokens + cache_read_tokens
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN (
        CASE
          WHEN input_tokens <= 0 THEN 0
          WHEN cache_read_tokens <= 0 THEN input_tokens
          WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
               IN ('openai', 'gemini', 'google')
          THEN GREATEST(input_tokens - cache_read_tokens, 0)
          ELSE input_tokens
        END
      ) + cache_read_tokens
      ELSE CASE
        WHEN cache_creation_tokens > 0
        THEN input_tokens + cache_creation_tokens + cache_read_tokens
        ELSE input_tokens + cache_read_tokens
      END
    END AS total_input_context
  FROM filtered_usage
)
SELECT
  group_key,
  COUNT(*)::BIGINT AS request_count,
  COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(effective_input_tokens), 0)::BIGINT AS effective_input_tokens,
  COALESCE(SUM(total_input_context), 0)::BIGINT AS total_input_context,
  COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(cache_creation_ephemeral_5m_tokens), 0)::BIGINT
    AS cache_creation_ephemeral_5m_tokens,
  COALESCE(SUM(cache_creation_ephemeral_1h_tokens), 0)::BIGINT
    AS cache_creation_ephemeral_1h_tokens,
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(total_cost_usd), 0) AS total_cost_usd,
  COALESCE(SUM(actual_total_cost_usd), 0) AS actual_total_cost_usd,
  COALESCE(SUM(success_flag), 0)::BIGINT AS success_count,
  COALESCE(SUM(successful_response_time_ms), 0) AS response_time_sum_ms,
  COALESCE(SUM(successful_response_time_samples), 0)::BIGINT AS response_time_samples,
  COALESCE(SUM(response_time_ms), 0) AS overall_response_time_sum_ms,
  COALESCE(SUM(response_time_samples), 0)::BIGINT AS overall_response_time_samples
FROM normalized_usage
GROUP BY group_key
ORDER BY request_count DESC, group_key ASC
"#,
        );

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_breakdown_summary_row(&row)?);
        }
        Ok(items)
    }

    pub async fn summarize_usage_breakdown(
        &self,
        query: &UsageBreakdownSummaryQuery,
    ) -> Result<Vec<StoredUsageBreakdownSummaryRow>, DataLayerError> {
        let Some(user_id) = query.user_id.as_deref() else {
            return self.summarize_usage_breakdown_raw(query).await;
        };
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self.summarize_usage_breakdown_raw(query).await;
        };

        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self.summarize_usage_breakdown_raw(query).await;
        };

        let mut grouped = BTreeMap::<String, StoredUsageBreakdownSummaryRow>::new();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            let raw = self
                .summarize_usage_breakdown_raw(&UsageBreakdownSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    user_id: Some(user_id.to_string()),
                    group_by: query.group_by,
                })
                .await?;
            absorb_usage_breakdown_rows(&mut grouped, raw);
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            let aggregate = self
                .summarize_usage_breakdown_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                    user_id,
                    query.group_by,
                )
                .await?;
            absorb_usage_breakdown_rows(&mut grouped, aggregate);
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            let raw = self
                .summarize_usage_breakdown_raw(&UsageBreakdownSummaryQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    user_id: Some(user_id.to_string()),
                    group_by: query.group_by,
                })
                .await?;
            absorb_usage_breakdown_rows(&mut grouped, raw);
        }

        Ok(finalize_usage_breakdown_rows(grouped))
    }

    pub async fn count_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorCountQuery,
    ) -> Result<u64, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT COUNT(*)::BIGINT AS total
FROM "usage"
WHERE "usage".created_at >= TO_TIMESTAMP($1::double precision)
  AND "usage".created_at < TO_TIMESTAMP($2::double precision)
  AND (
    lower(BTRIM(COALESCE("usage".status, ''))) IN ('failed', 'error')
    OR ("usage".error_category IS NOT NULL AND BTRIM("usage".error_category) <> '')
    OR (
      BTRIM(COALESCE("usage".status, '')) = ''
      AND (
        COALESCE("usage".status_code, 0) >= 400
        OR ("usage".error_message IS NOT NULL AND BTRIM("usage".error_message) <> '')
      )
    )
  )
"#,
        )
        .bind(query.created_from_unix_secs as f64)
        .bind(query.created_until_unix_secs as f64)
        .fetch_one(&self.pool)
        .await
        .map_postgres_err()?;

        Ok(row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64)
    }

    pub async fn list_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_USAGE_AUDITS_PREFIX);
        builder.push(
            r#"
WHERE "usage".created_at >= TO_TIMESTAMP(
"#,
        );
        builder
            .push_bind(query.created_from_unix_secs as f64)
            .push(
                r#"::double precision)
  AND "usage".created_at < TO_TIMESTAMP(
"#,
            )
            .push_bind(query.created_until_unix_secs as f64)
            .push(
                r#"::double precision)
  AND (
    lower(BTRIM(COALESCE("usage".status, ''))) IN ('failed', 'error')
    OR ("usage".error_category IS NOT NULL AND BTRIM("usage".error_category) <> '')
    OR (
      BTRIM(COALESCE("usage".status, '')) = ''
      AND (
        COALESCE("usage".status_code, 0) >= 400
        OR ("usage".error_message IS NOT NULL AND BTRIM("usage".error_message) <> '')
      )
    )
  )
ORDER BY "usage".created_at DESC, "usage".id ASC
"#,
            );
        if let Some(limit) = query.limit {
            builder.push(" LIMIT ").push_bind(limit as i64);
        }

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row, false)?);
        }
        Ok(items)
    }

    async fn summarize_usage_error_distribution_raw(
        &self,
        query: &UsageErrorDistributionQuery,
    ) -> Result<Vec<StoredUsageErrorDistributionRow>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  TO_CHAR(
    date_trunc('day', "usage".created_at + (
      "#,
        );
        builder.push_bind(query.tz_offset_minutes);
        builder.push(
            r#"::integer * INTERVAL '1 minute'
    )),
    'YYYY-MM-DD'
  ) AS date,
  "usage".error_category AS error_category,
  COUNT(*)::BIGINT AS count
FROM "usage"
WHERE "usage".created_at >= TO_TIMESTAMP("#,
        );
        builder.push_bind(query.created_from_unix_secs as f64);
        builder.push(
            r#"::double precision)
  AND "usage".created_at < TO_TIMESTAMP("#,
        );
        builder.push_bind(query.created_until_unix_secs as f64);
        builder.push(
            r#"::double precision)
  AND "usage".error_category IS NOT NULL
  AND BTRIM("usage".error_category) <> ''
GROUP BY date, "usage".error_category
ORDER BY date ASC, count DESC, "usage".error_category ASC
"#,
        );

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_error_distribution_row(&row)?);
        }
        Ok(items)
    }

    async fn summarize_usage_error_distribution_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
    ) -> Result<Vec<StoredUsageErrorDistributionRow>, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(Vec::new());
        }

        let mut rows = sqlx::query(
            r#"
SELECT
  TO_CHAR(date, 'YYYY-MM-DD') AS date,
  error_category,
  COALESCE(SUM(count), 0)::BIGINT AS count
FROM stats_daily_error
WHERE date >= $1
  AND date < $2
GROUP BY date, error_category
ORDER BY date ASC, count DESC, error_category ASC
"#,
        )
        .bind(start_day_utc)
        .bind(end_day_utc)
        .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_error_distribution_row(&row)?);
        }
        Ok(items)
    }

    pub async fn summarize_usage_error_distribution(
        &self,
        query: &UsageErrorDistributionQuery,
    ) -> Result<Vec<StoredUsageErrorDistributionRow>, DataLayerError> {
        if query.tz_offset_minutes != 0 {
            return self.summarize_usage_error_distribution_raw(query).await;
        }
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self.summarize_usage_error_distribution_raw(query).await;
        };

        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self.summarize_usage_error_distribution_raw(query).await;
        };

        let mut grouped = BTreeMap::<(String, String), u64>::new();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_error_distribution_rows(
                &mut grouped,
                self.summarize_usage_error_distribution_raw(&UsageErrorDistributionQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    tz_offset_minutes: 0,
                })
                .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_error_distribution_rows(
                &mut grouped,
                self.summarize_usage_error_distribution_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_error_distribution_rows(
                &mut grouped,
                self.summarize_usage_error_distribution_raw(&UsageErrorDistributionQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    tz_offset_minutes: 0,
                })
                .await?,
            );
        }

        Ok(finalize_usage_error_distribution_rows(grouped))
    }

    async fn summarize_usage_performance_percentiles_raw(
        &self,
        query: &UsagePerformancePercentilesQuery,
    ) -> Result<Vec<StoredUsagePerformancePercentilesRow>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
WITH filtered_usage AS (
  SELECT
    TO_CHAR(
      date_trunc('day', "usage".created_at + (
        "#,
        );
        builder.push_bind(query.tz_offset_minutes);
        builder.push(
            r#"::integer * INTERVAL '1 minute'
      )),
      'YYYY-MM-DD'
    ) AS date,
    "usage".response_time_ms AS response_time_ms,
    "usage".first_byte_time_ms AS first_byte_time_ms
  FROM "usage"
  WHERE "usage".created_at >= TO_TIMESTAMP("#,
        );
        builder.push_bind(query.created_from_unix_secs as f64);
        builder.push(
            r#"::double precision)
    AND "usage".created_at < TO_TIMESTAMP("#,
        );
        builder.push_bind(query.created_until_unix_secs as f64);
        builder.push(
            r#"::double precision)
    AND "usage".status = 'completed'
)
SELECT
  date,
  CASE
    WHEN COUNT(response_time_ms) >= 10
    THEN FLOOR(PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY response_time_ms))::BIGINT
    ELSE NULL
  END AS p50_response_time_ms,
  CASE
    WHEN COUNT(response_time_ms) >= 10
    THEN FLOOR(PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY response_time_ms))::BIGINT
    ELSE NULL
  END AS p90_response_time_ms,
  CASE
    WHEN COUNT(response_time_ms) >= 10
    THEN FLOOR(PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY response_time_ms))::BIGINT
    ELSE NULL
  END AS p99_response_time_ms,
  CASE
    WHEN COUNT(first_byte_time_ms) >= 10
    THEN FLOOR(PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY first_byte_time_ms))::BIGINT
    ELSE NULL
  END AS p50_first_byte_time_ms,
  CASE
    WHEN COUNT(first_byte_time_ms) >= 10
    THEN FLOOR(PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY first_byte_time_ms))::BIGINT
    ELSE NULL
  END AS p90_first_byte_time_ms,
  CASE
    WHEN COUNT(first_byte_time_ms) >= 10
    THEN FLOOR(PERCENTILE_CONT(0.99) WITHIN GROUP (ORDER BY first_byte_time_ms))::BIGINT
    ELSE NULL
  END AS p99_first_byte_time_ms
FROM filtered_usage
GROUP BY date
ORDER BY date ASC
"#,
        );

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_performance_percentiles_row(&row)?);
        }
        Ok(items)
    }

    async fn summarize_usage_performance_percentiles_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
    ) -> Result<Vec<StoredUsagePerformancePercentilesRow>, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(Vec::new());
        }

        let mut rows = sqlx::query(
            r#"
SELECT
  TO_CHAR(date, 'YYYY-MM-DD') AS date,
  CASE
    WHEN p50_response_time_ms IS NOT NULL THEN GREATEST(p50_response_time_ms, 0)::BIGINT
    ELSE NULL
  END AS p50_response_time_ms,
  CASE
    WHEN p90_response_time_ms IS NOT NULL THEN GREATEST(p90_response_time_ms, 0)::BIGINT
    ELSE NULL
  END AS p90_response_time_ms,
  CASE
    WHEN p99_response_time_ms IS NOT NULL THEN GREATEST(p99_response_time_ms, 0)::BIGINT
    ELSE NULL
  END AS p99_response_time_ms,
  CASE
    WHEN p50_first_byte_time_ms IS NOT NULL THEN GREATEST(p50_first_byte_time_ms, 0)::BIGINT
    ELSE NULL
  END AS p50_first_byte_time_ms,
  CASE
    WHEN p90_first_byte_time_ms IS NOT NULL THEN GREATEST(p90_first_byte_time_ms, 0)::BIGINT
    ELSE NULL
  END AS p90_first_byte_time_ms,
  CASE
    WHEN p99_first_byte_time_ms IS NOT NULL THEN GREATEST(p99_first_byte_time_ms, 0)::BIGINT
    ELSE NULL
  END AS p99_first_byte_time_ms
FROM stats_daily
WHERE date >= $1
  AND date < $2
ORDER BY date ASC
"#,
        )
        .bind(start_day_utc)
        .bind(end_day_utc)
        .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_performance_percentiles_row(&row)?);
        }
        Ok(items)
    }

    pub async fn summarize_usage_performance_percentiles(
        &self,
        query: &UsagePerformancePercentilesQuery,
    ) -> Result<Vec<StoredUsagePerformancePercentilesRow>, DataLayerError> {
        if query.tz_offset_minutes != 0 {
            return self
                .summarize_usage_performance_percentiles_raw(query)
                .await;
        }
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self
                .summarize_usage_performance_percentiles_raw(query)
                .await;
        };

        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_usage_performance_percentiles_raw(query)
                .await;
        };

        let mut items = Vec::new();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            items.extend(
                self.summarize_usage_performance_percentiles_raw(
                    &UsagePerformancePercentilesQuery {
                        created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                        created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                        tz_offset_minutes: 0,
                    },
                )
                .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            items.extend(
                self.summarize_usage_performance_percentiles_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            items.extend(
                self.summarize_usage_performance_percentiles_raw(
                    &UsagePerformancePercentilesQuery {
                        created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                        created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                        tz_offset_minutes: 0,
                    },
                )
                .await?,
            );
        }
        items.sort_by(|left, right| left.date.cmp(&right.date));
        Ok(items)
    }

    async fn summarize_usage_cost_savings_raw_from_range(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
        user_id: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<StoredUsageCostSavingsSummary, DataLayerError> {
        if start_utc >= end_utc {
            return Ok(StoredUsageCostSavingsSummary::default());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST("usage".cache_read_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_read_cost_usd,
  COALESCE(SUM(COALESCE(CAST("usage".cache_creation_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS cache_creation_cost_usd,
  COALESCE(SUM(
    COALESCE(
      CAST("usage".output_price_per_1m AS DOUBLE PRECISION),
      0
    ) * GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)::DOUBLE PRECISION / 1000000.0
  ), 0) AS estimated_full_cost_usd
FROM usage_billing_facts AS "usage"
"#,
        );
        builder
            .push(" WHERE \"usage\".created_at >= ")
            .push_bind(start_utc);
        builder
            .push(" AND \"usage\".created_at < ")
            .push_bind(end_utc);
        if let Some(user_id) = user_id {
            builder.push(" AND ");
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = provider_name {
            builder.push(" AND ");
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = model {
            builder.push(" AND ");
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        decode_usage_cost_savings_row(&row)
    }

    async fn summarize_usage_cost_savings_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        user_id: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<StoredUsageCostSavingsSummary, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(StoredUsageCostSavingsSummary::default());
        }

        let row = match (user_id, provider_name, model) {
            (None, None, None) => sqlx::query(
                r#"
SELECT
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(cache_read_cost), 0)::DOUBLE PRECISION AS cache_read_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0)::DOUBLE PRECISION AS cache_creation_cost_usd,
  COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION AS estimated_full_cost_usd
FROM stats_daily_cost_savings
WHERE date >= $1
  AND date < $2
"#,
            )
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
            (None, Some(provider_name), None) => sqlx::query(
                r#"
SELECT
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(cache_read_cost), 0)::DOUBLE PRECISION AS cache_read_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0)::DOUBLE PRECISION AS cache_creation_cost_usd,
  COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION AS estimated_full_cost_usd
FROM stats_daily_cost_savings_provider
WHERE provider_name = $1
  AND date >= $2
  AND date < $3
"#,
            )
            .bind(provider_name)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
            (None, None, Some(model)) => sqlx::query(
                r#"
SELECT
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(cache_read_cost), 0)::DOUBLE PRECISION AS cache_read_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0)::DOUBLE PRECISION AS cache_creation_cost_usd,
  COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION AS estimated_full_cost_usd
FROM stats_daily_cost_savings_model
WHERE model = $1
  AND date >= $2
  AND date < $3
"#,
            )
            .bind(model)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
            (None, Some(provider_name), Some(model)) => sqlx::query(
                r#"
SELECT
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(cache_read_cost), 0)::DOUBLE PRECISION AS cache_read_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0)::DOUBLE PRECISION AS cache_creation_cost_usd,
  COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION AS estimated_full_cost_usd
FROM stats_daily_cost_savings_model_provider
WHERE provider_name = $1
  AND model = $2
  AND date >= $3
  AND date < $4
"#,
            )
            .bind(provider_name)
            .bind(model)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
            (Some(user_id), None, None) => sqlx::query(
                r#"
SELECT
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(cache_read_cost), 0)::DOUBLE PRECISION AS cache_read_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0)::DOUBLE PRECISION AS cache_creation_cost_usd,
  COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION AS estimated_full_cost_usd
FROM stats_user_daily_cost_savings
WHERE user_id = $1
  AND date >= $2
  AND date < $3
"#,
            )
            .bind(user_id)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
            (Some(user_id), Some(provider_name), None) => sqlx::query(
                r#"
SELECT
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(cache_read_cost), 0)::DOUBLE PRECISION AS cache_read_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0)::DOUBLE PRECISION AS cache_creation_cost_usd,
  COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION AS estimated_full_cost_usd
FROM stats_user_daily_cost_savings_provider
WHERE user_id = $1
  AND provider_name = $2
  AND date >= $3
  AND date < $4
"#,
            )
            .bind(user_id)
            .bind(provider_name)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
            (Some(user_id), None, Some(model)) => sqlx::query(
                r#"
SELECT
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(cache_read_cost), 0)::DOUBLE PRECISION AS cache_read_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0)::DOUBLE PRECISION AS cache_creation_cost_usd,
  COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION AS estimated_full_cost_usd
FROM stats_user_daily_cost_savings_model
WHERE user_id = $1
  AND model = $2
  AND date >= $3
  AND date < $4
"#,
            )
            .bind(user_id)
            .bind(model)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
            (Some(user_id), Some(provider_name), Some(model)) => sqlx::query(
                r#"
SELECT
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(cache_read_cost), 0)::DOUBLE PRECISION AS cache_read_cost_usd,
  COALESCE(SUM(cache_creation_cost), 0)::DOUBLE PRECISION AS cache_creation_cost_usd,
  COALESCE(SUM(estimated_full_cost), 0)::DOUBLE PRECISION AS estimated_full_cost_usd
FROM stats_user_daily_cost_savings_model_provider
WHERE user_id = $1
  AND provider_name = $2
  AND model = $3
  AND date >= $4
  AND date < $5
"#,
            )
            .bind(user_id)
            .bind(provider_name)
            .bind(model)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?,
        };

        decode_usage_cost_savings_row(&row)
    }

    pub async fn summarize_usage_cost_savings(
        &self,
        query: &UsageCostSavingsSummaryQuery,
    ) -> Result<StoredUsageCostSavingsSummary, DataLayerError> {
        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let user_id = query.user_id.as_deref();
        let provider_name = query.provider_name.as_deref();
        let model = query.model.as_deref();

        if start_utc >= end_utc {
            return Ok(StoredUsageCostSavingsSummary::default());
        }

        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self
                .summarize_usage_cost_savings_raw_from_range(
                    start_utc,
                    end_utc,
                    user_id,
                    provider_name,
                    model,
                )
                .await;
        };

        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_usage_cost_savings_raw_from_range(
                    start_utc,
                    end_utc,
                    user_id,
                    provider_name,
                    model,
                )
                .await;
        };

        let mut summary = StoredUsageCostSavingsSummary::default();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_cost_savings_summary(
                &mut summary,
                self.summarize_usage_cost_savings_raw_from_range(
                    raw_start,
                    raw_end,
                    user_id,
                    provider_name,
                    model,
                )
                .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            absorb_usage_cost_savings_summary(
                &mut summary,
                self.summarize_usage_cost_savings_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                    user_id,
                    provider_name,
                    model,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_cost_savings_summary(
                &mut summary,
                self.summarize_usage_cost_savings_raw_from_range(
                    raw_start,
                    raw_end,
                    user_id,
                    provider_name,
                    model,
                )
                .await?,
            );
        }
        Ok(summary)
    }

    async fn summarize_usage_time_series_raw(
        &self,
        query: &UsageTimeSeriesQuery,
        excluded_only: bool,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        match query.granularity {
            UsageTimeSeriesGranularity::Day => {
                builder
                    .push("TO_CHAR(date_trunc('day', \"usage\".created_at + (")
                    .push_bind(query.tz_offset_minutes)
                    .push("::integer * INTERVAL '1 minute')), 'YYYY-MM-DD') AS bucket_key");
            }
            UsageTimeSeriesGranularity::Hour => {
                builder
                    .push("TO_CHAR(date_trunc('hour', \"usage\".created_at + (")
                    .push_bind(query.tz_offset_minutes)
                    .push("::integer * INTERVAL '1 minute')), 'YYYY-MM-DD\"T\"HH24:00:00+00:00') AS bucket_key");
            }
        }
        builder.push(
            r#",
  COUNT(*)::BIGINT AS total_requests,
  COALESCE(SUM(GREATEST(COALESCE("usage".input_tokens, 0), 0)), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".output_tokens, 0), 0)), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_creation_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_creation_tokens,
  COALESCE(SUM(GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)), 0)::BIGINT
    AS cache_read_tokens,
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd,
  COALESCE(SUM(GREATEST(COALESCE("usage".response_time_ms, 0), 0)::DOUBLE PRECISION), 0)
    AS total_response_time_ms
FROM usage_billing_facts AS "usage"
"#,
        );
        let mut has_where = false;

        builder.push(if has_where { " AND " } else { " WHERE " });
        has_where = true;
        builder
            .push("\"usage\".created_at >= TO_TIMESTAMP(")
            .push_bind(query.created_from_unix_secs as f64)
            .push("::double precision)");
        builder.push(if has_where { " AND " } else { " WHERE " });
        builder
            .push("\"usage\".created_at < TO_TIMESTAMP(")
            .push_bind(query.created_until_unix_secs as f64)
            .push("::double precision)");
        if let Some(user_id) = query.user_id.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        if let Some(provider_name) = query.provider_name.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            has_where = true;
            builder
                .push("\"usage\".provider_name = ")
                .push_bind(provider_name.to_string());
        }
        if let Some(model) = query.model.as_deref() {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder
                .push("\"usage\".model = ")
                .push_bind(model.to_string());
        }
        if excluded_only {
            builder.push(if has_where { " AND " } else { " WHERE " });
            builder.push(
                r#"(
  "usage".status IS NULL
  OR "usage".status IN ('pending', 'streaming')
  OR "usage".provider_name IS NULL
  OR "usage".provider_name IN ('unknown', 'pending')
)"#,
            );
        }
        builder.push(" GROUP BY bucket_key ORDER BY bucket_key ASC");

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_time_series_bucket_row(&row)?);
        }
        Ok(items)
    }

    async fn summarize_usage_time_series_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(Vec::new());
        }

        let rows = if let Some(user_id) = user_id {
            sqlx::query(
                r#"
SELECT
  TO_CHAR(date, 'YYYY-MM-DD') AS bucket_key,
  total_requests::BIGINT AS total_requests,
  input_tokens::BIGINT AS input_tokens,
  output_tokens::BIGINT AS output_tokens,
  cache_creation_tokens::BIGINT AS cache_creation_tokens,
  cache_read_tokens::BIGINT AS cache_read_tokens,
  CAST(total_cost AS DOUBLE PRECISION) AS total_cost_usd,
  CAST(response_time_sum_ms AS DOUBLE PRECISION) AS total_response_time_ms
FROM stats_user_daily
WHERE user_id = $1
  AND date >= $2
  AND date < $3
ORDER BY date ASC
"#,
            )
            .bind(user_id)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?
        } else {
            sqlx::query(
                r#"
SELECT
  TO_CHAR(date, 'YYYY-MM-DD') AS bucket_key,
  total_requests::BIGINT AS total_requests,
  input_tokens::BIGINT AS input_tokens,
  output_tokens::BIGINT AS output_tokens,
  cache_creation_tokens::BIGINT AS cache_creation_tokens,
  cache_read_tokens::BIGINT AS cache_read_tokens,
  CAST(total_cost AS DOUBLE PRECISION) AS total_cost_usd,
  CAST(response_time_sum_ms AS DOUBLE PRECISION) AS total_response_time_ms
FROM stats_daily
WHERE date >= $1
  AND date < $2
ORDER BY date ASC
"#,
            )
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?
        };
        let mut items = Vec::new();
        for row in rows {
            items.push(decode_usage_time_series_bucket_row(&row)?);
        }
        Ok(items)
    }

    async fn summarize_usage_time_series_from_hourly_aggregates(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
        granularity: UsageTimeSeriesGranularity,
        tz_offset_minutes: i32,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        if start_utc >= end_utc {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new("SELECT ");
        match granularity {
            UsageTimeSeriesGranularity::Day => {
                builder
                    .push("TO_CHAR(date_trunc('day', hour_utc + (")
                    .push_bind(tz_offset_minutes)
                    .push("::integer * INTERVAL '1 minute')), 'YYYY-MM-DD') AS bucket_key");
            }
            UsageTimeSeriesGranularity::Hour => {
                builder
                    .push("TO_CHAR(date_trunc('hour', hour_utc + (")
                    .push_bind(tz_offset_minutes)
                    .push("::integer * INTERVAL '1 minute')), 'YYYY-MM-DD\"T\"HH24:00:00+00:00') AS bucket_key");
            }
        }
        builder.push(
            r#",
  COALESCE(SUM(total_requests), 0)::BIGINT AS total_requests,
  COALESCE(SUM(input_tokens), 0)::BIGINT AS input_tokens,
  COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
  COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
  COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
  COALESCE(SUM(CAST(total_cost AS DOUBLE PRECISION)), 0) AS total_cost_usd,
  COALESCE(SUM(CAST(response_time_sum_ms AS DOUBLE PRECISION)), 0) AS total_response_time_ms
FROM stats_hourly
WHERE is_complete IS TRUE
  AND hour_utc >= "#,
        );
        builder.push_bind(start_utc).push(
            r#"
  AND hour_utc < "#,
        );
        builder
            .push_bind(end_utc)
            .push("\nGROUP BY bucket_key ORDER BY bucket_key ASC");

        let mut rows = builder.build().fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_time_series_bucket_row(&row)?);
        }
        Ok(items)
    }

    pub async fn summarize_usage_time_series(
        &self,
        query: &UsageTimeSeriesQuery,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        if query.provider_name.is_some() || query.model.is_some() {
            return self.summarize_usage_time_series_raw(query, false).await;
        }

        if query.granularity == UsageTimeSeriesGranularity::Day && query.tz_offset_minutes == 0 {
            if let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? {
                let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
                let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
                let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
                if let Some((aggregate_start, aggregate_end)) = split.aggregate {
                    let mut grouped = BTreeMap::<String, StoredUsageTimeSeriesBucket>::new();
                    if let Some((raw_start, raw_end)) = split.raw_leading {
                        absorb_usage_time_series_buckets(
                            &mut grouped,
                            self.summarize_usage_time_series_raw(
                                &UsageTimeSeriesQuery {
                                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                                    granularity: UsageTimeSeriesGranularity::Day,
                                    tz_offset_minutes: 0,
                                    user_id: query.user_id.clone(),
                                    provider_name: None,
                                    model: None,
                                },
                                false,
                            )
                            .await?,
                        );
                    }
                    absorb_usage_time_series_buckets(
                        &mut grouped,
                        self.summarize_usage_time_series_from_daily_aggregates(
                            aggregate_start,
                            aggregate_end,
                            query.user_id.as_deref(),
                        )
                        .await?,
                    );
                    absorb_usage_time_series_buckets(
                        &mut grouped,
                        self.summarize_usage_time_series_raw(
                            &UsageTimeSeriesQuery {
                                created_from_unix_secs: dashboard_utc_to_unix_secs(aggregate_start),
                                created_until_unix_secs: dashboard_utc_to_unix_secs(aggregate_end),
                                granularity: UsageTimeSeriesGranularity::Day,
                                tz_offset_minutes: 0,
                                user_id: query.user_id.clone(),
                                provider_name: None,
                                model: None,
                            },
                            true,
                        )
                        .await?,
                    );
                    if let Some((raw_start, raw_end)) = split.raw_trailing {
                        absorb_usage_time_series_buckets(
                            &mut grouped,
                            self.summarize_usage_time_series_raw(
                                &UsageTimeSeriesQuery {
                                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                                    granularity: UsageTimeSeriesGranularity::Day,
                                    tz_offset_minutes: 0,
                                    user_id: query.user_id.clone(),
                                    provider_name: None,
                                    model: None,
                                },
                                false,
                            )
                            .await?,
                        );
                    }
                    return Ok(finalize_usage_time_series_buckets(grouped));
                }
            }
        }

        if query.user_id.is_none() && query.tz_offset_minutes % 60 == 0 {
            if let Some(cutoff_utc) = self.read_stats_hourly_cutoff().await? {
                let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
                let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
                let split = split_dashboard_hourly_aggregate_range(start_utc, end_utc, cutoff_utc);
                if let Some((aggregate_start, aggregate_end)) = split.aggregate {
                    let mut grouped = BTreeMap::<String, StoredUsageTimeSeriesBucket>::new();
                    if let Some((raw_start, raw_end)) = split.raw_leading {
                        absorb_usage_time_series_buckets(
                            &mut grouped,
                            self.summarize_usage_time_series_raw(
                                &UsageTimeSeriesQuery {
                                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                                    granularity: query.granularity,
                                    tz_offset_minutes: query.tz_offset_minutes,
                                    user_id: None,
                                    provider_name: None,
                                    model: None,
                                },
                                false,
                            )
                            .await?,
                        );
                    }
                    absorb_usage_time_series_buckets(
                        &mut grouped,
                        self.summarize_usage_time_series_from_hourly_aggregates(
                            aggregate_start,
                            aggregate_end,
                            query.granularity,
                            query.tz_offset_minutes,
                        )
                        .await?,
                    );
                    absorb_usage_time_series_buckets(
                        &mut grouped,
                        self.summarize_usage_time_series_raw(
                            &UsageTimeSeriesQuery {
                                created_from_unix_secs: dashboard_utc_to_unix_secs(aggregate_start),
                                created_until_unix_secs: dashboard_utc_to_unix_secs(aggregate_end),
                                granularity: query.granularity,
                                tz_offset_minutes: query.tz_offset_minutes,
                                user_id: None,
                                provider_name: None,
                                model: None,
                            },
                            true,
                        )
                        .await?,
                    );
                    if let Some((raw_start, raw_end)) = split.raw_trailing {
                        absorb_usage_time_series_buckets(
                            &mut grouped,
                            self.summarize_usage_time_series_raw(
                                &UsageTimeSeriesQuery {
                                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                                    granularity: query.granularity,
                                    tz_offset_minutes: query.tz_offset_minutes,
                                    user_id: None,
                                    provider_name: None,
                                    model: None,
                                },
                                false,
                            )
                            .await?,
                        );
                    }
                    return Ok(finalize_usage_time_series_buckets(grouped));
                }
            }
        }

        self.summarize_usage_time_series_raw(query, false).await
    }

    async fn summarize_usage_leaderboard_raw(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
        let fragments = usage_leaderboard_sql_fragments(query.group_by);
        let sql = format!(
            r#"
SELECT
  {group_key_expr} AS group_key,
  MAX({legacy_name_expr}) AS legacy_name,
  COUNT(*)::BIGINT AS request_count,
  COALESCE(SUM(
    GREATEST(COALESCE("usage".input_tokens, 0), 0)
    + GREATEST(COALESCE("usage".output_tokens, 0), 0)
    + GREATEST(COALESCE("usage".cache_creation_input_tokens, 0), 0)
    + GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0)
  ), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0)), 0)
    AS total_cost_usd
FROM usage_billing_facts AS "usage"
WHERE "usage".created_at >= TO_TIMESTAMP($1::double precision)
  AND "usage".created_at < TO_TIMESTAMP($2::double precision)
  AND "usage".status NOT IN ('pending', 'streaming')
  AND "usage".provider_name NOT IN ('unknown', 'pending')
  {filtered_extra_where}
  AND ($3::varchar IS NULL OR "usage".user_id = $3)
  AND ($4::varchar IS NULL OR "usage".provider_name = $4)
  AND ($5::varchar IS NULL OR "usage".model = $5)
GROUP BY group_key
ORDER BY group_key ASC
"#,
            group_key_expr = fragments.group_key_expr,
            legacy_name_expr = fragments.legacy_name_expr,
            filtered_extra_where = fragments.filtered_extra_where,
        );
        let mut rows = sqlx::query(&sql)
            .bind(query.created_from_unix_secs as f64)
            .bind(query.created_until_unix_secs as f64)
            .bind(query.user_id.as_deref())
            .bind(query.provider_name.as_deref())
            .bind(query.model.as_deref())
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(decode_usage_leaderboard_row(&row)?);
        }
        Ok(items)
    }

    async fn summarize_usage_leaderboard_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        query: &UsageLeaderboardQuery,
    ) -> Result<Option<Vec<StoredUsageLeaderboardSummary>>, DataLayerError> {
        let items = match query.group_by {
            UsageLeaderboardGroupBy::Model => {
                let mut builder = if let Some(user_id) = query.user_id.as_deref() {
                    if let Some(provider_name) = query.provider_name.as_deref() {
                        let mut builder = QueryBuilder::<Postgres>::new(
                            r#"
SELECT
  model AS group_key,
  NULL::varchar AS legacy_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_user_daily_model_provider
WHERE date >=
"#,
                        );
                        builder
                            .push_bind(start_day_utc)
                            .push(" AND date < ")
                            .push_bind(end_day_utc)
                            .push(" AND user_id = ")
                            .push_bind(user_id.to_string())
                            .push(" AND provider_name = ")
                            .push_bind(provider_name.to_string());
                        if let Some(model) = query.model.as_deref() {
                            builder.push(" AND model = ").push_bind(model.to_string());
                        }
                        builder.push(" GROUP BY model ORDER BY model ASC");
                        builder
                    } else {
                        let mut builder = QueryBuilder::<Postgres>::new(
                            r#"
SELECT
  model AS group_key,
  NULL::varchar AS legacy_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_user_daily_model
WHERE date >=
"#,
                        );
                        builder
                            .push_bind(start_day_utc)
                            .push(" AND date < ")
                            .push_bind(end_day_utc)
                            .push(" AND user_id = ")
                            .push_bind(user_id.to_string());
                        if let Some(model) = query.model.as_deref() {
                            builder.push(" AND model = ").push_bind(model.to_string());
                        }
                        builder.push(" GROUP BY model ORDER BY model ASC");
                        builder
                    }
                } else if let Some(provider_name) = query.provider_name.as_deref() {
                    let mut builder = QueryBuilder::<Postgres>::new(
                        r#"
SELECT
  model AS group_key,
  NULL::varchar AS legacy_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_daily_model_provider
WHERE date >=
"#,
                    );
                    builder
                        .push_bind(start_day_utc)
                        .push(" AND date < ")
                        .push_bind(end_day_utc)
                        .push(" AND provider_name = ")
                        .push_bind(provider_name.to_string());
                    if let Some(model) = query.model.as_deref() {
                        builder.push(" AND model = ").push_bind(model.to_string());
                    }
                    builder.push(" GROUP BY model ORDER BY model ASC");
                    builder
                } else {
                    let mut builder = QueryBuilder::<Postgres>::new(
                        r#"
SELECT
  model AS group_key,
  NULL::varchar AS legacy_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(
    SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens),
    0
  )::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_daily_model
WHERE date >=
"#,
                    );
                    builder
                        .push_bind(start_day_utc)
                        .push(" AND date < ")
                        .push_bind(end_day_utc);
                    if let Some(model) = query.model.as_deref() {
                        builder.push(" AND model = ").push_bind(model.to_string());
                    }
                    builder.push(" GROUP BY model ORDER BY model ASC");
                    builder
                };
                fetch_usage_leaderboard_query(builder.build(), &self.pool).await?
            }
            UsageLeaderboardGroupBy::User => {
                let mut builder = if query.provider_name.is_some() && query.model.is_some() {
                    let mut builder = QueryBuilder::<Postgres>::new(
                        r#"
SELECT
  user_id AS group_key,
  MAX(NULLIF(BTRIM(username), '')) AS legacy_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_user_daily_model_provider
WHERE date >=
"#,
                    );
                    builder
                        .push_bind(start_day_utc)
                        .push(" AND date < ")
                        .push_bind(end_day_utc)
                        .push(" AND provider_name = ")
                        .push_bind(query.provider_name.as_deref().unwrap().to_string())
                        .push(" AND model = ")
                        .push_bind(query.model.as_deref().unwrap().to_string());
                    if let Some(user_id) = query.user_id.as_deref() {
                        builder
                            .push(" AND user_id = ")
                            .push_bind(user_id.to_string());
                    }
                    builder.push(" GROUP BY user_id ORDER BY user_id ASC");
                    builder
                } else if let Some(provider_name) = query.provider_name.as_deref() {
                    let mut builder = QueryBuilder::<Postgres>::new(
                        r#"
SELECT
  user_id AS group_key,
  MAX(NULLIF(BTRIM(username), '')) AS legacy_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_user_daily_provider
WHERE date >=
"#,
                    );
                    builder
                        .push_bind(start_day_utc)
                        .push(" AND date < ")
                        .push_bind(end_day_utc)
                        .push(" AND provider_name = ")
                        .push_bind(provider_name.to_string());
                    if let Some(user_id) = query.user_id.as_deref() {
                        builder
                            .push(" AND user_id = ")
                            .push_bind(user_id.to_string());
                    }
                    builder.push(" GROUP BY user_id ORDER BY user_id ASC");
                    builder
                } else if let Some(model) = query.model.as_deref() {
                    let mut builder = QueryBuilder::<Postgres>::new(
                        r#"
SELECT
  user_id AS group_key,
  MAX(NULLIF(BTRIM(username), '')) AS legacy_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_user_daily_model
WHERE date >=
"#,
                    );
                    builder
                        .push_bind(start_day_utc)
                        .push(" AND date < ")
                        .push_bind(end_day_utc)
                        .push(" AND model = ")
                        .push_bind(model.to_string());
                    if let Some(user_id) = query.user_id.as_deref() {
                        builder
                            .push(" AND user_id = ")
                            .push_bind(user_id.to_string());
                    }
                    builder.push(" GROUP BY user_id ORDER BY user_id ASC");
                    builder
                } else {
                    let mut builder = QueryBuilder::<Postgres>::new(
                        r#"
SELECT
  user_id AS group_key,
  MAX(NULLIF(BTRIM(username), '')) AS legacy_name,
  COALESCE(SUM(total_requests), 0)::BIGINT AS request_count,
  COALESCE(
    SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens),
    0
  )::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_user_daily
WHERE date >=
"#,
                    );
                    builder
                        .push_bind(start_day_utc)
                        .push(" AND date < ")
                        .push_bind(end_day_utc)
                        .push(" AND user_id IS NOT NULL");
                    if let Some(user_id) = query.user_id.as_deref() {
                        builder
                            .push(" AND user_id = ")
                            .push_bind(user_id.to_string());
                    }
                    builder.push(" GROUP BY user_id ORDER BY user_id ASC");
                    builder
                };
                fetch_usage_leaderboard_query(builder.build(), &self.pool).await?
            }
            UsageLeaderboardGroupBy::ApiKey => {
                if query.provider_name.is_some() || query.model.is_some() {
                    return Ok(None);
                }
                let mut builder = QueryBuilder::<Postgres>::new(
                    r#"
SELECT
  stats_daily_api_key.api_key_id AS group_key,
  COALESCE(
    MAX(NULLIF(BTRIM(stats_daily_api_key.api_key_name), '')),
    MAX(NULLIF(BTRIM(api_keys.name), ''))
  ) AS legacy_name,
  COALESCE(SUM(stats_daily_api_key.total_requests), 0)::BIGINT AS request_count,
  COALESCE(
    SUM(
      stats_daily_api_key.input_tokens
      + stats_daily_api_key.output_tokens
      + stats_daily_api_key.cache_creation_tokens
      + stats_daily_api_key.cache_read_tokens
    ),
    0
  )::BIGINT AS total_tokens,
  CAST(COALESCE(SUM(stats_daily_api_key.total_cost), 0) AS DOUBLE PRECISION) AS total_cost_usd
FROM stats_daily_api_key
LEFT JOIN api_keys ON api_keys.id = stats_daily_api_key.api_key_id
WHERE stats_daily_api_key.date >=
"#,
                );
                builder
                    .push_bind(start_day_utc)
                    .push(" AND stats_daily_api_key.date < ")
                    .push_bind(end_day_utc)
                    .push(" AND stats_daily_api_key.api_key_id IS NOT NULL");
                if let Some(user_id) = query.user_id.as_deref() {
                    builder
                        .push(" AND api_keys.user_id = ")
                        .push_bind(user_id.to_string());
                }
                builder.push(
                    " GROUP BY stats_daily_api_key.api_key_id ORDER BY stats_daily_api_key.api_key_id ASC",
                );
                fetch_usage_leaderboard_query(builder.build(), &self.pool).await?
            }
        };
        Ok(Some(items))
    }

    pub async fn summarize_usage_leaderboard(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self.summarize_usage_leaderboard_raw(query).await;
        };

        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = dashboard_unix_secs_to_utc(query.created_until_unix_secs);
        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let mut grouped = BTreeMap::new();

        if let Some((raw_start, raw_end)) = split.raw_leading {
            absorb_usage_leaderboard_rows(
                &mut grouped,
                self.summarize_usage_leaderboard_raw(&UsageLeaderboardQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    ..query.clone()
                })
                .await?,
            );
        }

        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            if let Some(rows) = self
                .summarize_usage_leaderboard_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                    query,
                )
                .await?
            {
                absorb_usage_leaderboard_rows(&mut grouped, rows);
            } else {
                absorb_usage_leaderboard_rows(
                    &mut grouped,
                    self.summarize_usage_leaderboard_raw(&UsageLeaderboardQuery {
                        created_from_unix_secs: dashboard_utc_to_unix_secs(aggregate_start),
                        created_until_unix_secs: dashboard_utc_to_unix_secs(aggregate_end),
                        ..query.clone()
                    })
                    .await?,
                );
            }
        }

        if let Some((raw_start, raw_end)) = split.raw_trailing {
            absorb_usage_leaderboard_rows(
                &mut grouped,
                self.summarize_usage_leaderboard_raw(&UsageLeaderboardQuery {
                    created_from_unix_secs: dashboard_utc_to_unix_secs(raw_start),
                    created_until_unix_secs: dashboard_utc_to_unix_secs(raw_end),
                    ..query.clone()
                })
                .await?,
            );
        }

        Ok(finalize_usage_leaderboard_rows(grouped))
    }

    pub async fn aggregate_usage_audits(
        &self,
        query: &UsageAuditAggregationQuery,
    ) -> Result<Vec<StoredUsageAuditAggregation>, DataLayerError> {
        let fragments = usage_audit_aggregation_sql_fragments(query.group_by);
        let sql = format!(
            r#"
WITH filtered_usage AS (
  SELECT
    "usage".model AS model,
    "usage".user_id AS user_id,
    COALESCE("usage".provider_id, 'unknown') AS provider_group_key,
    CASE
      WHEN BTRIM(COALESCE("usage".provider_name, '')) = ''
        OR "usage".provider_name IN ('unknown', 'pending')
      THEN 'Unknown'
      ELSE "usage".provider_name
    END AS provider_display_name,
    COALESCE("usage".api_format, 'unknown') AS api_format_group_key,
    GREATEST(COALESCE("usage".input_tokens, 0), 0) AS input_tokens,
    GREATEST(COALESCE("usage".output_tokens, 0), 0) AS output_tokens,
    GREATEST(COALESCE("usage".total_tokens, 0), 0) AS total_tokens,
    CASE
      WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
           AND (
             COALESCE("usage".cache_creation_input_tokens_5m, 0)
             + COALESCE("usage".cache_creation_input_tokens_1h, 0)
           ) > 0
      THEN COALESCE("usage".cache_creation_input_tokens_5m, 0)
         + COALESCE("usage".cache_creation_input_tokens_1h, 0)
      ELSE COALESCE("usage".cache_creation_input_tokens, 0)
    END AS cache_creation_tokens,
    GREATEST(COALESCE("usage".cache_creation_input_tokens_5m, 0), 0)
      AS cache_creation_ephemeral_5m_tokens,
    GREATEST(COALESCE("usage".cache_creation_input_tokens_1h, 0), 0)
      AS cache_creation_ephemeral_1h_tokens,
    GREATEST(COALESCE("usage".cache_read_input_tokens, 0), 0) AS cache_read_tokens,
    COALESCE("usage".endpoint_api_format, "usage".api_format) AS normalized_api_format,
    COALESCE(CAST("usage".total_cost_usd AS DOUBLE PRECISION), 0) AS total_cost_usd,
    COALESCE(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION), 0) AS actual_total_cost_usd,
    GREATEST(COALESCE("usage".response_time_ms, 0), 0) AS response_time_ms,
    CASE
      WHEN "usage".status IN ('completed', 'success', 'ok', 'billed', 'settled')
           AND ("usage".status_code IS NULL OR "usage".status_code < 400)
      THEN 1
      ELSE 0
    END AS success_flag
  FROM usage_billing_facts AS "usage"
  WHERE "usage".created_at >= TO_TIMESTAMP($1::double precision)
    AND "usage".created_at < TO_TIMESTAMP($2::double precision)
    AND "usage".status NOT IN ('pending', 'streaming')
    {filtered_extra_where}
),
normalized_usage AS (
  SELECT
    {group_key_expr} AS group_key,
    {display_name_expr} AS display_name,
    {secondary_name_expr} AS secondary_name,
    total_tokens,
    output_tokens,
    cache_creation_tokens,
    cache_creation_ephemeral_5m_tokens,
    cache_creation_ephemeral_1h_tokens,
    cache_read_tokens,
    total_cost_usd,
    actual_total_cost_usd,
    response_time_ms,
    success_flag,
    CASE
      WHEN input_tokens <= 0 THEN 0
      WHEN cache_read_tokens <= 0 THEN input_tokens
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN GREATEST(input_tokens - cache_read_tokens, 0)
      ELSE input_tokens
    END AS effective_input_tokens,
    CASE
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('claude', 'anthropic')
      THEN input_tokens + cache_creation_tokens + cache_read_tokens
      WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
           IN ('openai', 'gemini', 'google')
      THEN (
        CASE
          WHEN input_tokens <= 0 THEN 0
          WHEN cache_read_tokens <= 0 THEN input_tokens
          WHEN split_part(lower(COALESCE(normalized_api_format, '')), ':', 1)
               IN ('openai', 'gemini', 'google')
          THEN GREATEST(input_tokens - cache_read_tokens, 0)
          ELSE input_tokens
        END
      ) + cache_read_tokens
      ELSE CASE
        WHEN cache_creation_tokens > 0
        THEN input_tokens + cache_creation_tokens + cache_read_tokens
        ELSE input_tokens + cache_read_tokens
      END
    END AS total_input_context
  FROM filtered_usage
),
aggregated_usage AS (
  SELECT
    group_key,
    {aggregate_display_name_expr} AS display_name,
    {aggregate_secondary_name_expr} AS secondary_name,
    COUNT(*)::BIGINT AS request_count,
    COALESCE(SUM(total_tokens), 0)::BIGINT AS total_tokens,
    COALESCE(SUM(output_tokens), 0)::BIGINT AS output_tokens,
    COALESCE(SUM(effective_input_tokens), 0)::BIGINT AS effective_input_tokens,
    COALESCE(SUM(total_input_context), 0)::BIGINT AS total_input_context,
    COALESCE(SUM(cache_creation_tokens), 0)::BIGINT AS cache_creation_tokens,
    COALESCE(SUM(cache_creation_ephemeral_5m_tokens), 0)::BIGINT
      AS cache_creation_ephemeral_5m_tokens,
    COALESCE(SUM(cache_creation_ephemeral_1h_tokens), 0)::BIGINT
      AS cache_creation_ephemeral_1h_tokens,
    COALESCE(SUM(cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
    COALESCE(SUM(total_cost_usd), 0) AS total_cost_usd,
    COALESCE(SUM(actual_total_cost_usd), 0) AS actual_total_cost_usd,
    {avg_response_time_expr} AS avg_response_time_ms,
    {success_count_expr} AS success_count
  FROM normalized_usage
  GROUP BY group_key
)
SELECT
  group_key,
  display_name,
  secondary_name,
  request_count,
  total_tokens,
  output_tokens,
  effective_input_tokens,
  total_input_context,
  cache_creation_tokens,
  cache_creation_ephemeral_5m_tokens,
  cache_creation_ephemeral_1h_tokens,
  cache_read_tokens,
  total_cost_usd,
  actual_total_cost_usd,
  avg_response_time_ms,
  success_count
FROM aggregated_usage
ORDER BY request_count DESC, group_key ASC
LIMIT $3
"#,
            filtered_extra_where = fragments.filtered_extra_where,
            group_key_expr = fragments.group_key_expr,
            display_name_expr = fragments.display_name_expr,
            secondary_name_expr = fragments.secondary_name_expr,
            aggregate_display_name_expr = fragments.aggregate_display_name_expr,
            aggregate_secondary_name_expr = fragments.aggregate_secondary_name_expr,
            avg_response_time_expr = fragments.avg_response_time_expr,
            success_count_expr = fragments.success_count_expr,
        );

        let mut rows = sqlx::query(&sql)
            .bind(query.created_from_unix_secs as f64)
            .bind(query.created_until_unix_secs as f64)
            .bind(i64::try_from(query.limit).map_err(|_| {
                DataLayerError::InvalidInput(format!(
                    "invalid usage aggregation limit: {}",
                    query.limit
                ))
            })?)
            .fetch(&self.pool);

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(StoredUsageAuditAggregation {
                group_key: row.try_get::<String, _>("group_key").map_postgres_err()?,
                display_name: row
                    .try_get::<Option<String>, _>("display_name")
                    .map_postgres_err()?,
                secondary_name: row
                    .try_get::<Option<String>, _>("secondary_name")
                    .map_postgres_err()?,
                request_count: row
                    .try_get::<i64, _>("request_count")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_tokens: row
                    .try_get::<i64, _>("total_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                output_tokens: row
                    .try_get::<i64, _>("output_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                effective_input_tokens: row
                    .try_get::<i64, _>("effective_input_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_input_context: row
                    .try_get::<i64, _>("total_input_context")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_creation_tokens: row
                    .try_get::<i64, _>("cache_creation_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_creation_ephemeral_5m_tokens: row
                    .try_get::<i64, _>("cache_creation_ephemeral_5m_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_creation_ephemeral_1h_tokens: row
                    .try_get::<i64, _>("cache_creation_ephemeral_1h_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                cache_read_tokens: row
                    .try_get::<i64, _>("cache_read_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_cost_usd: row.try_get::<f64, _>("total_cost_usd").map_postgres_err()?,
                actual_total_cost_usd: row
                    .try_get::<f64, _>("actual_total_cost_usd")
                    .map_postgres_err()?,
                avg_response_time_ms: row
                    .try_get::<Option<f64>, _>("avg_response_time_ms")
                    .map_postgres_err()?,
                success_count: row
                    .try_get::<Option<i64>, _>("success_count")
                    .map_postgres_err()?
                    .map(|value| value.max(0) as u64),
            });
        }
        Ok(items)
    }

    async fn summarize_usage_daily_heatmap_raw_from_range(
        &self,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        if start_utc >= end_utc {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            r#"SELECT
  DATE("usage".created_at) AS day,
  COUNT(*)::BIGINT AS requests,
  COALESCE(SUM("usage".input_tokens + "usage".output_tokens
    + CASE
        WHEN COALESCE("usage".cache_creation_input_tokens, 0) = 0
             AND (COALESCE("usage".cache_creation_input_tokens_5m, 0) + COALESCE("usage".cache_creation_input_tokens_1h, 0)) > 0
        THEN COALESCE("usage".cache_creation_input_tokens_5m, 0) + COALESCE("usage".cache_creation_input_tokens_1h, 0)
        ELSE COALESCE("usage".cache_creation_input_tokens, 0)
      END
    + COALESCE("usage".cache_read_input_tokens, 0)), 0)::BIGINT AS total_tokens,
  COALESCE(SUM(CAST("usage".total_cost_usd AS DOUBLE PRECISION)), 0) AS total_cost_usd,
  COALESCE(SUM(CAST("usage".actual_total_cost_usd AS DOUBLE PRECISION)), 0) AS actual_total_cost_usd
FROM usage_billing_facts AS "usage"
WHERE "usage".created_at >= $1
  AND "usage".created_at < $2
  AND "usage".status NOT IN ('pending', 'streaming')
  AND "usage".provider_name NOT IN ('unknown', 'pending')"#,
        );
        let bind_index = 3;
        if user_id.is_some() {
            sql.push_str(&format!(" AND \"usage\".user_id = ${bind_index}"));
        }
        sql.push_str(" GROUP BY day ORDER BY day ASC");

        let mut q = sqlx::query(&sql).bind(start_utc).bind(end_utc);
        if let Some(user_id) = user_id {
            q = q.bind(user_id.to_string());
        }

        let mut rows = q.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let day: chrono::NaiveDate = row.try_get("day").map_postgres_err()?;
            let requests: i64 = row.try_get("requests").map_postgres_err()?;
            let total_tokens: i64 = row.try_get("total_tokens").map_postgres_err()?;
            let total_cost_usd: f64 = row.try_get("total_cost_usd").map_postgres_err()?;
            let actual_total_cost_usd: f64 =
                row.try_get("actual_total_cost_usd").map_postgres_err()?;
            items.push(StoredUsageDailySummary {
                date: day.to_string(),
                requests: requests.max(0) as u64,
                total_tokens: total_tokens.max(0) as u64,
                total_cost_usd,
                actual_total_cost_usd,
            });
        }
        Ok(items)
    }

    async fn summarize_usage_daily_heatmap_from_daily_aggregates(
        &self,
        start_day_utc: DateTime<Utc>,
        end_day_utc: DateTime<Utc>,
        user_id: Option<&str>,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        if start_day_utc >= end_day_utc {
            return Ok(Vec::new());
        }

        let mut rows = if let Some(user_id) = user_id {
            sqlx::query(
                r#"
SELECT
  date,
  total_requests,
  input_tokens,
  output_tokens,
  cache_creation_tokens,
  cache_read_tokens,
  total_cost,
  COALESCE(actual_total_cost, 0) AS actual_total_cost
FROM stats_user_daily
WHERE user_id = $1
  AND date >= $2
  AND date < $3
ORDER BY date ASC
"#,
            )
            .bind(user_id)
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch(&self.pool)
        } else {
            sqlx::query(
                r#"
SELECT
  date,
  total_requests,
  input_tokens,
  output_tokens,
  cache_creation_tokens,
  cache_read_tokens,
  total_cost,
  actual_total_cost
FROM stats_daily
WHERE date >= $1
  AND date < $2
ORDER BY date ASC
"#,
            )
            .bind(start_day_utc)
            .bind(end_day_utc)
            .fetch(&self.pool)
        };

        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let date = row.try_get::<DateTime<Utc>, _>("date").map_postgres_err()?;
            let requests = row.try_get::<i64, _>("total_requests").map_postgres_err()?;
            let input_tokens = row.try_get::<i64, _>("input_tokens").map_postgres_err()?;
            let output_tokens = row.try_get::<i64, _>("output_tokens").map_postgres_err()?;
            let cache_creation_tokens = row
                .try_get::<i64, _>("cache_creation_tokens")
                .map_postgres_err()?;
            let cache_read_tokens = row
                .try_get::<i64, _>("cache_read_tokens")
                .map_postgres_err()?;
            let total_cost_usd = row.try_get::<f64, _>("total_cost").map_postgres_err()?;
            let actual_total_cost_usd = row
                .try_get::<f64, _>("actual_total_cost")
                .map_postgres_err()?;
            items.push(StoredUsageDailySummary {
                date: date.date_naive().to_string(),
                requests: requests.max(0) as u64,
                total_tokens: input_tokens
                    .saturating_add(output_tokens)
                    .saturating_add(cache_creation_tokens)
                    .saturating_add(cache_read_tokens)
                    .max(0) as u64,
                total_cost_usd,
                actual_total_cost_usd,
            });
        }

        Ok(items)
    }

    pub async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        let start_utc = dashboard_unix_secs_to_utc(query.created_from_unix_secs);
        let end_utc = Utc::now() + chrono::Duration::seconds(1);
        let user_id = query.user_id.as_deref();

        let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? else {
            return self
                .summarize_usage_daily_heatmap_raw_from_range(start_utc, end_utc, user_id)
                .await;
        };

        let split = split_dashboard_daily_aggregate_range(start_utc, end_utc, cutoff_utc);
        let Some(_) = split.aggregate else {
            return self
                .summarize_usage_daily_heatmap_raw_from_range(start_utc, end_utc, user_id)
                .await;
        };

        let mut items = Vec::new();
        if let Some((raw_start, raw_end)) = split.raw_leading {
            items.extend(
                self.summarize_usage_daily_heatmap_raw_from_range(raw_start, raw_end, user_id)
                    .await?,
            );
        }
        if let Some((aggregate_start, aggregate_end)) = split.aggregate {
            items.extend(
                self.summarize_usage_daily_heatmap_from_daily_aggregates(
                    aggregate_start,
                    aggregate_end,
                    user_id,
                )
                .await?,
            );
        }
        if let Some((raw_start, raw_end)) = split.raw_trailing {
            items.extend(
                self.summarize_usage_daily_heatmap_raw_from_range(raw_start, raw_end, user_id)
                    .await?,
            );
        }
        items.sort_by(|left, right| left.date.cmp(&right.date));
        Ok(items)
    }

    pub async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_RECENT_USAGE_AUDITS_PREFIX);
        if let Some(user_id) = user_id {
            builder
                .push(" WHERE \"usage\".user_id = ")
                .push_bind(user_id.to_string());
        }
        builder
            .push(" ORDER BY \"usage\".created_at DESC, \"usage\".id ASC LIMIT ")
            .push_bind(i64::try_from(limit).map_err(|_| {
                DataLayerError::InvalidInput(format!("invalid recent usage limit: {limit}"))
            })?);
        let query = builder.build();
        let mut rows = query.fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_usage_row(&row, false)?);
        }
        Ok(items)
    }

    pub async fn summarize_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }

        let mut totals = std::collections::BTreeMap::new();
        if let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? {
            let mut aggregate_rows = sqlx::query(
                r#"
SELECT
  api_key_id,
  COALESCE(
    SUM(input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens),
    0
  )::BIGINT AS total_tokens
FROM stats_daily_api_key
WHERE api_key_id = ANY($1::TEXT[])
  AND date < $2
GROUP BY api_key_id
ORDER BY api_key_id ASC
"#,
            )
            .bind(api_key_ids)
            .bind(cutoff_utc)
            .fetch(&self.pool);

            while let Some(row) = aggregate_rows.try_next().await.map_postgres_err()? {
                let api_key_id: String = row.try_get("api_key_id").map_postgres_err()?;
                let total_tokens = row
                    .try_get::<i64, _>("total_tokens")
                    .map_postgres_err()?
                    .max(0) as u64;
                totals.insert(api_key_id, total_tokens);
            }

            let mut builder = QueryBuilder::<Postgres>::new(
                r#"
SELECT
  api_key_id,
  COALESCE(
    SUM(
      COALESCE(
        total_tokens,
        COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)
      )
    ),
    0
  ) AS total_tokens
FROM usage_billing_facts AS "usage"
WHERE api_key_id = ANY(
"#,
            );
            builder
                .push_bind(api_key_ids)
                .push("::TEXT[]) AND created_at >= ")
                .push_bind(cutoff_utc)
                .push(
                    r#"
GROUP BY api_key_id
ORDER BY api_key_id ASC
"#,
                );

            let mut raw_rows = builder.build().fetch(&self.pool);
            while let Some(row) = raw_rows.try_next().await.map_postgres_err()? {
                let api_key_id: String = row.try_get("api_key_id").map_postgres_err()?;
                let total_tokens = row
                    .try_get::<i64, _>("total_tokens")
                    .map_postgres_err()?
                    .max(0) as u64;
                let entry = totals.entry(api_key_id).or_default();
                *entry = entry.saturating_add(total_tokens);
            }
            return Ok(totals);
        }

        let mut rows = sqlx::query(SUMMARIZE_TOTAL_TOKENS_BY_API_KEY_IDS_SQL)
            .bind(api_key_ids)
            .fetch(&self.pool);
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let api_key_id: String = row.try_get("api_key_id").map_postgres_err()?;
            let total_tokens = row
                .try_get::<i64, _>("total_tokens")
                .map_postgres_err()?
                .max(0) as u64;
            totals.insert(api_key_id, total_tokens);
        }
        Ok(totals)
    }

    pub async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUsageUserTotals>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut totals = std::collections::BTreeMap::<String, StoredUsageUserTotals>::new();
        if let Some(cutoff_utc) = self.read_stats_daily_cutoff_date().await? {
            let mut aggregate_rows = sqlx::query(
                r#"
SELECT
  user_id,
  COALESCE(all_time_requests, 0)::BIGINT AS request_count,
  COALESCE(
    all_time_input_tokens
      + all_time_output_tokens
      + all_time_cache_creation_tokens
      + all_time_cache_read_tokens,
    0
  )::BIGINT AS total_tokens
FROM stats_user_summary
WHERE user_id = ANY($1::TEXT[])
ORDER BY user_id ASC
"#,
            )
            .bind(user_ids)
            .fetch(&self.pool);

            while let Some(row) = aggregate_rows.try_next().await.map_postgres_err()? {
                let user_id = row.try_get::<String, _>("user_id").map_postgres_err()?;
                let request_count = row
                    .try_get::<i64, _>("request_count")
                    .map_postgres_err()?
                    .max(0) as u64;
                let total_tokens = row
                    .try_get::<i64, _>("total_tokens")
                    .map_postgres_err()?
                    .max(0) as u64;
                totals.insert(
                    user_id.clone(),
                    StoredUsageUserTotals {
                        user_id,
                        request_count,
                        total_tokens,
                    },
                );
            }

            let mut builder = QueryBuilder::<Postgres>::new(
                r#"
SELECT
  "usage".user_id,
  COUNT(*)::BIGINT AS request_count,
  COALESCE(SUM(GREATEST(COALESCE("usage".total_tokens, 0), 0)), 0)::BIGINT AS total_tokens
FROM usage_billing_facts AS "usage"
WHERE "usage".user_id = ANY(
"#,
            );
            builder
                .push_bind(user_ids)
                .push("::TEXT[])")
                .push(" AND \"usage\".created_at >= ")
                .push_bind(cutoff_utc)
                .push(
                    r#"
  AND "usage".status NOT IN ('pending', 'streaming')
  AND "usage".provider_name NOT IN ('unknown', 'pending')
GROUP BY "usage".user_id
ORDER BY "usage".user_id ASC
"#,
                );

            let mut raw_rows = builder.build().fetch(&self.pool);
            while let Some(row) = raw_rows.try_next().await.map_postgres_err()? {
                let user_id = row.try_get::<String, _>("user_id").map_postgres_err()?;
                let request_count = row
                    .try_get::<i64, _>("request_count")
                    .map_postgres_err()?
                    .max(0) as u64;
                let total_tokens = row
                    .try_get::<i64, _>("total_tokens")
                    .map_postgres_err()?
                    .max(0) as u64;
                let entry =
                    totals
                        .entry(user_id.clone())
                        .or_insert_with(|| StoredUsageUserTotals {
                            user_id,
                            request_count: 0,
                            total_tokens: 0,
                        });
                entry.request_count = entry.request_count.saturating_add(request_count);
                entry.total_tokens = entry.total_tokens.saturating_add(total_tokens);
            }

            return Ok(totals.into_values().collect());
        }

        let mut rows = sqlx::query(SUMMARIZE_USAGE_TOTALS_BY_USER_IDS_SQL)
            .bind(user_ids)
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(StoredUsageUserTotals {
                user_id: row.try_get::<String, _>("user_id").map_postgres_err()?,
                request_count: row
                    .try_get::<i64, _>("request_count")
                    .map_postgres_err()?
                    .max(0) as u64,
                total_tokens: row
                    .try_get::<i64, _>("total_tokens")
                    .map_postgres_err()?
                    .max(0) as u64,
            });
        }
        Ok(items)
    }

    pub async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, StoredProviderApiKeyUsageSummary>, DataLayerError>
    {
        if provider_api_key_ids.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }

        let mut rows = sqlx::query(SUMMARIZE_USAGE_BY_PROVIDER_API_KEY_IDS_SQL)
            .bind(provider_api_key_ids)
            .fetch(&self.pool);

        let mut summaries = std::collections::BTreeMap::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            let provider_api_key_id: String =
                row.try_get("provider_api_key_id").map_postgres_err()?;
            let request_count = row
                .try_get::<i64, _>("request_count")
                .map_postgres_err()?
                .try_into()
                .map_err(|_| {
                    DataLayerError::UnexpectedValue(
                        "usage.request_count aggregate is negative".to_string(),
                    )
                })?;
            let total_tokens = row
                .try_get::<i64, _>("total_tokens")
                .map_postgres_err()?
                .try_into()
                .map_err(|_| {
                    DataLayerError::UnexpectedValue(
                        "usage.total_tokens aggregate is negative".to_string(),
                    )
                })?;
            let total_cost_usd: f64 = row.try_get("total_cost_usd").map_postgres_err()?;
            if !total_cost_usd.is_finite() {
                return Err(DataLayerError::UnexpectedValue(
                    "usage.total_cost_usd aggregate is not finite".to_string(),
                ));
            }
            let last_used_at_unix_secs = row
                .try_get::<Option<i64>, _>("last_used_at_unix_secs")
                .map_postgres_err()?
                .map(|value| {
                    value.try_into().map_err(|_| {
                        DataLayerError::UnexpectedValue(
                            "usage.last_used_at_unix_secs aggregate is negative".to_string(),
                        )
                    })
                })
                .transpose()?;

            summaries.insert(
                provider_api_key_id.clone(),
                StoredProviderApiKeyUsageSummary {
                    provider_api_key_id,
                    request_count,
                    total_tokens,
                    total_cost_usd,
                    last_used_at_unix_secs,
                },
            );
        }

        Ok(summaries)
    }

    pub async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        usage.validate()?;
        let usage = strip_deprecated_usage_display_fields(usage);
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    lock_usage_request_id_in_tx(tx, &usage.request_id).await?;

                    if incoming_usage_can_recover_terminal_failure(
                        usage.status.as_str(),
                        usage.billing_status.as_str(),
                    ) {
                        sqlx::query(RESET_STALE_VOID_USAGE_SQL)
                            .bind(&usage.request_id)
                            .execute(&mut **tx)
                            .await
                            .map_postgres_err()?;
                        sqlx::query(RESET_STALE_VOID_USAGE_SETTLEMENT_SNAPSHOT_SQL)
                            .bind(&usage.request_id)
                            .execute(&mut **tx)
                            .await
                            .map_postgres_err()?;
                    }

                    let previous_usage =
                        find_usage_by_request_id_in_tx(tx, &usage.request_id).await?;

                    let request_headers_json = json_bind_text(usage.request_headers.as_ref())?;
                    let request_body_storage =
                        prepare_usage_body_storage(usage.request_body.as_ref())?;
                    let provider_request_headers_json =
                        json_bind_text(usage.provider_request_headers.as_ref())?;
                    let provider_request_body_storage =
                        prepare_usage_body_storage(usage.provider_request_body.as_ref())?;
                    let response_headers_json = json_bind_text(usage.response_headers.as_ref())?;
                    let response_body_storage =
                        prepare_usage_body_storage(usage.response_body.as_ref())?;
                    let client_response_headers_json =
                        json_bind_text(usage.client_response_headers.as_ref())?;
                    let client_response_body_storage =
                        prepare_usage_body_storage(usage.client_response_body.as_ref())?;
                    let http_audit_refs = UsageHttpAuditRefs {
                        request_body_ref: resolved_write_usage_body_ref(
                            usage.request_body_ref.as_deref(),
                            &usage.request_id,
                            UsageBodyField::RequestBody,
                            request_body_storage.has_detached_blob(),
                            None,
                        ),
                        provider_request_body_ref: resolved_write_usage_body_ref(
                            usage.provider_request_body_ref.as_deref(),
                            &usage.request_id,
                            UsageBodyField::ProviderRequestBody,
                            provider_request_body_storage.has_detached_blob(),
                            None,
                        ),
                        response_body_ref: resolved_write_usage_body_ref(
                            usage.response_body_ref.as_deref(),
                            &usage.request_id,
                            UsageBodyField::ResponseBody,
                            response_body_storage.has_detached_blob(),
                            None,
                        ),
                        client_response_body_ref: resolved_write_usage_body_ref(
                            usage.client_response_body_ref.as_deref(),
                            &usage.request_id,
                            UsageBodyField::ClientResponseBody,
                            client_response_body_storage.has_detached_blob(),
                            None,
                        ),
                    };
                    let http_audit_states = UsageHttpAuditStates {
                        request_body_state: usage.request_body_state,
                        provider_request_body_state: usage.provider_request_body_state,
                        response_body_state: usage.response_body_state,
                        client_response_body_state: usage.client_response_body_state,
                    };
                    let request_metadata_value = prepare_request_metadata_for_body_storage(
                        usage.request_metadata.clone(),
                        [
                            (
                                UsageBodyField::RequestBody,
                                &request_body_storage,
                                usage.request_body.as_ref(),
                                usage.request_body_ref.as_deref(),
                            ),
                            (
                                UsageBodyField::ProviderRequestBody,
                                &provider_request_body_storage,
                                usage.provider_request_body.as_ref(),
                                usage.provider_request_body_ref.as_deref(),
                            ),
                            (
                                UsageBodyField::ResponseBody,
                                &response_body_storage,
                                usage.response_body.as_ref(),
                                usage.response_body_ref.as_deref(),
                            ),
                            (
                                UsageBodyField::ClientResponseBody,
                                &client_response_body_storage,
                                usage.client_response_body.as_ref(),
                                usage.client_response_body_ref.as_deref(),
                            ),
                        ],
                    );
                    let http_audit_capture_mode = usage_http_audit_capture_mode(
                        &http_audit_refs,
                        [
                            usage.request_body.as_ref(),
                            usage.provider_request_body.as_ref(),
                            usage.response_body.as_ref(),
                            usage.client_response_body.as_ref(),
                        ],
                    );
                    let routing_snapshot =
                        usage_routing_snapshot_from_usage(&usage, request_metadata_value.as_ref());
                    let settlement_pricing_snapshot = usage_settlement_pricing_snapshot_from_usage(
                        &usage,
                        request_metadata_value.as_ref(),
                    )?;
                    let request_metadata_json = json_bind_text(request_metadata_value.as_ref())?;
                    let _row = sqlx::query(UPSERT_SQL)
                        .bind(Uuid::new_v4().to_string())
                        .bind(&usage.request_id)
                        .bind(&usage.user_id)
                        .bind(&usage.api_key_id)
                        .bind(&usage.username)
                        .bind(&usage.api_key_name)
                        .bind(&usage.provider_name)
                        .bind(&usage.model)
                        .bind(&usage.target_model)
                        .bind(&usage.provider_id)
                        .bind(&usage.provider_endpoint_id)
                        .bind(&usage.provider_api_key_id)
                        .bind(&usage.request_type)
                        .bind(&usage.api_format)
                        .bind(&usage.api_family)
                        .bind(&usage.endpoint_kind)
                        .bind(&usage.endpoint_api_format)
                        .bind(&usage.provider_api_family)
                        .bind(&usage.provider_endpoint_kind)
                        .bind(usage.has_format_conversion)
                        .bind(usage.is_stream)
                        .bind(usage.input_tokens.map(to_i32).transpose()?)
                        .bind(usage.output_tokens.map(to_i32).transpose()?)
                        .bind(usage.total_tokens.map(to_i32).transpose()?)
                        .bind(usage.cache_creation_input_tokens.map(to_i32).transpose()?)
                        .bind(
                            usage
                                .cache_creation_ephemeral_5m_input_tokens
                                .map(to_i32)
                                .transpose()?,
                        )
                        .bind(
                            usage
                                .cache_creation_ephemeral_1h_input_tokens
                                .map(to_i32)
                                .transpose()?,
                        )
                        .bind(usage.cache_read_input_tokens.map(to_i32).transpose()?)
                        .bind(usage.cache_creation_cost_usd)
                        .bind(usage.cache_read_cost_usd)
                        .bind(None::<f64>)
                        .bind(usage.total_cost_usd)
                        .bind(usage.actual_total_cost_usd)
                        .bind(usage.status_code.map(i32::from))
                        .bind(&usage.error_message)
                        .bind(&usage.error_category)
                        .bind(usage.response_time_ms.map(to_i32).transpose()?)
                        .bind(usage.first_byte_time_ms.map(to_i32).transpose()?)
                        .bind(&usage.status)
                        .bind(&usage.billing_status)
                        .bind(None::<String>)
                        .bind(&request_body_storage.inline_json)
                        .bind(None::<Vec<u8>>)
                        .bind(None::<String>)
                        .bind(&provider_request_body_storage.inline_json)
                        .bind(None::<Vec<u8>>)
                        .bind(None::<String>)
                        .bind(&response_body_storage.inline_json)
                        .bind(None::<Vec<u8>>)
                        .bind(None::<String>)
                        .bind(&client_response_body_storage.inline_json)
                        .bind(None::<Vec<u8>>)
                        .bind(&request_metadata_json)
                        .bind(usage.finalized_at_unix_secs.map(|value| value as f64))
                        .bind(usage.created_at_unix_ms.map(|value| value as f64))
                        .bind(request_body_storage.has_detached_blob())
                        .bind(provider_request_body_storage.has_detached_blob())
                        .bind(response_body_storage.has_detached_blob())
                        .bind(client_response_body_storage.has_detached_blob())
                        .fetch_one(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    sync_usage_body_blob_storage(
                        &mut **tx,
                        &usage.request_id,
                        UsageBodyField::RequestBody,
                        usage.request_body.as_ref(),
                        &request_body_storage,
                    )
                    .await?;
                    sync_usage_body_blob_storage(
                        &mut **tx,
                        &usage.request_id,
                        UsageBodyField::ProviderRequestBody,
                        usage.provider_request_body.as_ref(),
                        &provider_request_body_storage,
                    )
                    .await?;
                    sync_usage_body_blob_storage(
                        &mut **tx,
                        &usage.request_id,
                        UsageBodyField::ResponseBody,
                        usage.response_body.as_ref(),
                        &response_body_storage,
                    )
                    .await?;
                    sync_usage_body_blob_storage(
                        &mut **tx,
                        &usage.request_id,
                        UsageBodyField::ClientResponseBody,
                        usage.client_response_body.as_ref(),
                        &client_response_body_storage,
                    )
                    .await?;
                    let http_audit_headers = UsageHttpAuditHeaders {
                        request_headers_json: request_headers_json.as_deref(),
                        provider_request_headers_json: provider_request_headers_json.as_deref(),
                        response_headers_json: response_headers_json.as_deref(),
                        client_response_headers_json: client_response_headers_json.as_deref(),
                    };
                    sync_usage_http_audit_storage(
                        &mut **tx,
                        &usage.request_id,
                        &http_audit_headers,
                        &http_audit_refs,
                        &http_audit_states,
                        http_audit_capture_mode,
                    )
                    .await?;
                    sync_usage_routing_snapshot_storage(
                        &mut **tx,
                        &usage.request_id,
                        &routing_snapshot,
                    )
                    .await?;
                    sync_usage_settlement_pricing_snapshot_storage(
                        &mut **tx,
                        &usage.request_id,
                        &settlement_pricing_snapshot,
                    )
                    .await?;

                    let mut stored = find_usage_by_request_id_in_tx(tx, &usage.request_id)
                        .await?
                        .ok_or_else(|| {
                            DataLayerError::UnexpectedValue(format!(
                                "usage row missing after upsert: {}",
                                usage.request_id
                            ))
                        })?;
                    if request_body_storage.has_detached_blob() {
                        stored.request_body = usage.request_body.clone();
                    }
                    stored.request_headers = usage.request_headers.clone();
                    stored.provider_request_headers = usage.provider_request_headers.clone();
                    if provider_request_body_storage.has_detached_blob() {
                        stored.provider_request_body = usage.provider_request_body.clone();
                    }
                    stored.response_headers = usage.response_headers.clone();
                    if response_body_storage.has_detached_blob() {
                        stored.response_body = usage.response_body.clone();
                    }
                    stored.client_response_headers = usage.client_response_headers.clone();
                    if client_response_body_storage.has_detached_blob() {
                        stored.client_response_body = usage.client_response_body.clone();
                    }
                    stored.request_body_ref = resolved_write_usage_body_ref(
                        usage.request_body_ref.as_deref(),
                        &usage.request_id,
                        UsageBodyField::RequestBody,
                        request_body_storage.has_detached_blob(),
                        http_audit_refs.request_body_ref.as_deref(),
                    );
                    stored.provider_request_body_ref = resolved_write_usage_body_ref(
                        usage.provider_request_body_ref.as_deref(),
                        &usage.request_id,
                        UsageBodyField::ProviderRequestBody,
                        provider_request_body_storage.has_detached_blob(),
                        http_audit_refs.provider_request_body_ref.as_deref(),
                    );
                    stored.response_body_ref = resolved_write_usage_body_ref(
                        usage.response_body_ref.as_deref(),
                        &usage.request_id,
                        UsageBodyField::ResponseBody,
                        response_body_storage.has_detached_blob(),
                        http_audit_refs.response_body_ref.as_deref(),
                    );
                    stored.client_response_body_ref = resolved_write_usage_body_ref(
                        usage.client_response_body_ref.as_deref(),
                        &usage.request_id,
                        UsageBodyField::ClientResponseBody,
                        client_response_body_storage.has_detached_blob(),
                        http_audit_refs.client_response_body_ref.as_deref(),
                    );
                    stored.request_body_state =
                        usage.request_body_state.or(stored.request_body_state);
                    stored.provider_request_body_state = usage
                        .provider_request_body_state
                        .or(stored.provider_request_body_state);
                    stored.response_body_state =
                        usage.response_body_state.or(stored.response_body_state);
                    stored.client_response_body_state = usage
                        .client_response_body_state
                        .or(stored.client_response_body_state);
                    stored.candidate_id = routing_snapshot.candidate_id.clone();
                    stored.candidate_index = routing_snapshot.candidate_index;
                    stored.key_name = routing_snapshot.key_name.clone();
                    stored.planner_kind = routing_snapshot.planner_kind.clone();
                    stored.route_family = routing_snapshot.route_family.clone();
                    stored.route_kind = routing_snapshot.route_kind.clone();
                    stored.execution_path = routing_snapshot.execution_path.clone();
                    stored.local_execution_runtime_miss_reason =
                        routing_snapshot.local_execution_runtime_miss_reason.clone();
                    stored.output_price_per_1m = settlement_pricing_snapshot.output_price_per_1m;
                    stored.request_metadata = request_metadata_value;

                    let before_api_key_contribution =
                        previous_usage.as_ref().and_then(api_key_usage_contribution);
                    let after_api_key_contribution = api_key_usage_contribution(&stored);
                    match (
                        before_api_key_contribution.as_ref(),
                        after_api_key_contribution.as_ref(),
                    ) {
                        (Some(before), Some(after)) if before.api_key_id == after.api_key_id => {
                            let delta = ApiKeyUsageDelta::between(before, after);
                            apply_api_key_usage_delta_in_tx(tx, before.api_key_id.as_str(), &delta)
                                .await?;
                        }
                        _ => {
                            if let Some(before) = before_api_key_contribution.as_ref() {
                                let delta = ApiKeyUsageDelta::removal(before);
                                apply_api_key_usage_delta_in_tx(
                                    tx,
                                    before.api_key_id.as_str(),
                                    &delta,
                                )
                                .await?;
                            }
                            if let Some(after) = after_api_key_contribution.as_ref() {
                                let delta = ApiKeyUsageDelta::addition(after);
                                apply_api_key_usage_delta_in_tx(
                                    tx,
                                    after.api_key_id.as_str(),
                                    &delta,
                                )
                                .await?;
                            }
                        }
                    }

                    let before_provider_contribution = previous_usage
                        .as_ref()
                        .and_then(provider_api_key_usage_contribution);
                    let after_provider_contribution = provider_api_key_usage_contribution(&stored);
                    match (
                        before_provider_contribution.as_ref(),
                        after_provider_contribution.as_ref(),
                    ) {
                        (Some(before), Some(after)) if before.key_id == after.key_id => {
                            let delta = ProviderApiKeyUsageDelta::between(before, after);
                            apply_provider_api_key_usage_delta_in_tx(
                                tx,
                                before.key_id.as_str(),
                                &delta,
                            )
                            .await?;
                        }
                        _ => {
                            if let Some(before) = before_provider_contribution.as_ref() {
                                let delta = ProviderApiKeyUsageDelta::removal(before);
                                apply_provider_api_key_usage_delta_in_tx(
                                    tx,
                                    before.key_id.as_str(),
                                    &delta,
                                )
                                .await?;
                            }
                            if let Some(after) = after_provider_contribution.as_ref() {
                                let delta = ProviderApiKeyUsageDelta::addition(after);
                                apply_provider_api_key_usage_delta_in_tx(
                                    tx,
                                    after.key_id.as_str(),
                                    &delta,
                                )
                                .await?;
                            }
                        }
                    }
                    Ok(stored)
                }) as BoxFuture<'_, Result<StoredRequestUsageAudit, DataLayerError>>
            })
            .await
    }

    pub async fn rebuild_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    sqlx::query(RESET_API_KEY_USAGE_STATS_SQL)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let rows_affected = sqlx::query(REBUILD_API_KEY_USAGE_STATS_SQL)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?
                        .rows_affected();
                    Ok(rows_affected)
                }) as BoxFuture<'_, Result<u64, DataLayerError>>
            })
            .await
    }

    pub async fn rebuild_provider_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        self.tx_runner
            .run_read_write(|tx| {
                Box::pin(async move {
                    sqlx::query(RESET_PROVIDER_API_KEY_USAGE_STATS_SQL)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?;
                    let rows_affected = sqlx::query(REBUILD_PROVIDER_API_KEY_USAGE_STATS_SQL)
                        .execute(&mut **tx)
                        .await
                        .map_postgres_err()?
                        .rows_affected();
                    Ok(rows_affected)
                }) as BoxFuture<'_, Result<u64, DataLayerError>>
            })
            .await
    }
}

#[async_trait]
impl UsageReadRepository for SqlxUsageReadRepository {
    async fn find_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        Self::find_by_id(self, id).await
    }

    async fn list_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_by_ids(self, ids).await
    }

    async fn find_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        Self::find_by_request_id(self, request_id).await
    }

    async fn resolve_body_ref(&self, body_ref: &str) -> Result<Option<Value>, DataLayerError> {
        Self::resolve_body_ref(self, body_ref).await
    }

    async fn list_usage_audits(
        &self,
        query: &UsageAuditListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_usage_audits(self, query).await
    }

    async fn count_usage_audits(&self, query: &UsageAuditListQuery) -> Result<u64, DataLayerError> {
        Self::count_usage_audits(self, query).await
    }

    async fn list_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_usage_audits_by_keyword_search(self, query).await
    }

    async fn count_usage_audits_by_keyword_search(
        &self,
        query: &UsageAuditKeywordSearchQuery,
    ) -> Result<u64, DataLayerError> {
        Self::count_usage_audits_by_keyword_search(self, query).await
    }

    async fn aggregate_usage_audits(
        &self,
        query: &UsageAuditAggregationQuery,
    ) -> Result<Vec<StoredUsageAuditAggregation>, DataLayerError> {
        Self::aggregate_usage_audits(self, query).await
    }

    async fn summarize_usage_audits(
        &self,
        query: &UsageAuditSummaryQuery,
    ) -> Result<StoredUsageAuditSummary, DataLayerError> {
        Self::summarize_usage_audits(self, query).await
    }

    async fn summarize_usage_cache_hit_summary(
        &self,
        query: &UsageCacheHitSummaryQuery,
    ) -> Result<StoredUsageCacheHitSummary, DataLayerError> {
        Self::summarize_usage_cache_hit_summary(self, query).await
    }

    async fn summarize_usage_settled_cost(
        &self,
        query: &UsageSettledCostSummaryQuery,
    ) -> Result<StoredUsageSettledCostSummary, DataLayerError> {
        Self::summarize_usage_settled_cost(self, query).await
    }

    async fn summarize_usage_cache_affinity_hit_summary(
        &self,
        query: &UsageCacheAffinityHitSummaryQuery,
    ) -> Result<StoredUsageCacheAffinityHitSummary, DataLayerError> {
        Self::summarize_usage_cache_affinity_hit_summary(self, query).await
    }

    async fn list_usage_cache_affinity_intervals(
        &self,
        query: &UsageCacheAffinityIntervalQuery,
    ) -> Result<Vec<StoredUsageCacheAffinityIntervalRow>, DataLayerError> {
        Self::list_usage_cache_affinity_intervals(self, query).await
    }

    async fn summarize_dashboard_usage(
        &self,
        query: &UsageDashboardSummaryQuery,
    ) -> Result<StoredUsageDashboardSummary, DataLayerError> {
        Self::summarize_dashboard_usage(self, query).await
    }

    async fn list_dashboard_daily_breakdown(
        &self,
        query: &UsageDashboardDailyBreakdownQuery,
    ) -> Result<Vec<StoredUsageDashboardDailyBreakdownRow>, DataLayerError> {
        Self::list_dashboard_daily_breakdown(self, query).await
    }

    async fn summarize_dashboard_provider_counts(
        &self,
        query: &UsageDashboardProviderCountsQuery,
    ) -> Result<Vec<StoredUsageDashboardProviderCount>, DataLayerError> {
        Self::summarize_dashboard_provider_counts(self, query).await
    }

    async fn summarize_usage_breakdown(
        &self,
        query: &UsageBreakdownSummaryQuery,
    ) -> Result<Vec<StoredUsageBreakdownSummaryRow>, DataLayerError> {
        Self::summarize_usage_breakdown(self, query).await
    }

    async fn count_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorCountQuery,
    ) -> Result<u64, DataLayerError> {
        Self::count_monitoring_usage_errors(self, query).await
    }

    async fn list_monitoring_usage_errors(
        &self,
        query: &UsageMonitoringErrorListQuery,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_monitoring_usage_errors(self, query).await
    }

    async fn summarize_usage_error_distribution(
        &self,
        query: &UsageErrorDistributionQuery,
    ) -> Result<Vec<StoredUsageErrorDistributionRow>, DataLayerError> {
        Self::summarize_usage_error_distribution(self, query).await
    }

    async fn summarize_usage_performance_percentiles(
        &self,
        query: &UsagePerformancePercentilesQuery,
    ) -> Result<Vec<StoredUsagePerformancePercentilesRow>, DataLayerError> {
        Self::summarize_usage_performance_percentiles(self, query).await
    }

    async fn summarize_usage_cost_savings(
        &self,
        query: &UsageCostSavingsSummaryQuery,
    ) -> Result<StoredUsageCostSavingsSummary, DataLayerError> {
        Self::summarize_usage_cost_savings(self, query).await
    }

    async fn summarize_usage_time_series(
        &self,
        query: &UsageTimeSeriesQuery,
    ) -> Result<Vec<StoredUsageTimeSeriesBucket>, DataLayerError> {
        Self::summarize_usage_time_series(self, query).await
    }

    async fn summarize_usage_leaderboard(
        &self,
        query: &UsageLeaderboardQuery,
    ) -> Result<Vec<StoredUsageLeaderboardSummary>, DataLayerError> {
        Self::summarize_usage_leaderboard(self, query).await
    }

    async fn list_recent_usage_audits(
        &self,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredRequestUsageAudit>, DataLayerError> {
        Self::list_recent_usage_audits(self, user_id, limit).await
    }

    async fn summarize_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, u64>, DataLayerError> {
        Self::summarize_total_tokens_by_api_key_ids(self, api_key_ids).await
    }

    async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUsageUserTotals>, DataLayerError> {
        Self::summarize_usage_totals_by_user_ids(self, user_ids).await
    }

    async fn summarize_usage_by_provider_api_key_ids(
        &self,
        provider_api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, StoredProviderApiKeyUsageSummary>, DataLayerError>
    {
        Self::summarize_usage_by_provider_api_key_ids(self, provider_api_key_ids).await
    }

    async fn summarize_provider_usage_since(
        &self,
        provider_id: &str,
        since_unix_secs: u64,
    ) -> Result<StoredProviderUsageSummary, DataLayerError> {
        Self::summarize_provider_usage_since(self, provider_id, since_unix_secs).await
    }

    async fn summarize_usage_daily_heatmap(
        &self,
        query: &UsageDailyHeatmapQuery,
    ) -> Result<Vec<StoredUsageDailySummary>, DataLayerError> {
        Self::summarize_usage_daily_heatmap(self, query).await
    }
}

#[async_trait]
impl UsageWriteRepository for SqlxUsageReadRepository {
    async fn upsert(
        &self,
        usage: UpsertUsageRecord,
    ) -> Result<StoredRequestUsageAudit, DataLayerError> {
        Self::upsert(self, usage).await
    }

    async fn rebuild_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        Self::rebuild_api_key_usage_stats(self).await
    }

    async fn rebuild_provider_api_key_usage_stats(&self) -> Result<u64, DataLayerError> {
        Self::rebuild_provider_api_key_usage_stats(self).await
    }
}

async fn find_usage_by_request_id_in_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    request_id: &str,
) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
    sqlx::query(FIND_BY_REQUEST_ID_SQL)
        .bind(request_id)
        .fetch_optional(&mut **tx)
        .await
        .map_postgres_err()?
        .map(|row| map_usage_row(&row, false))
        .transpose()
}

async fn lock_usage_request_id_in_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    request_id: &str,
) -> Result<(), DataLayerError> {
    sqlx::query(LOCK_USAGE_REQUEST_ID_SQL)
        .bind(request_id)
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    Ok(())
}

async fn apply_api_key_usage_delta_in_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    api_key_id: &str,
    delta: &ApiKeyUsageDelta,
) -> Result<(), DataLayerError> {
    if api_key_id.trim().is_empty() {
        return Ok(());
    }
    if delta.is_noop() {
        return Ok(());
    }

    let total_cost_usd_delta = if delta.total_cost_usd.is_finite() {
        delta.total_cost_usd
    } else {
        0.0
    };

    sqlx::query(APPLY_API_KEY_USAGE_DELTA_SQL)
        .bind(api_key_id)
        .bind(i32::try_from(delta.total_requests).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "api_keys.total_requests delta exceeds i32: {}",
                delta.total_requests
            ))
        })?)
        .bind(delta.total_tokens)
        .bind(total_cost_usd_delta)
        .bind(
            delta
                .candidate_last_used_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(
            delta
                .removed_last_used_at_unix_secs
                .map(|value| value as f64),
        )
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    Ok(())
}

async fn apply_provider_api_key_usage_delta_in_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    key_id: &str,
    delta: &ProviderApiKeyUsageDelta,
) -> Result<(), DataLayerError> {
    if key_id.trim().is_empty() {
        return Ok(());
    }
    if delta.is_noop() {
        return Ok(());
    }

    let total_cost_usd_delta = if delta.total_cost_usd.is_finite() {
        delta.total_cost_usd
    } else {
        0.0
    };

    sqlx::query(APPLY_PROVIDER_API_KEY_USAGE_DELTA_SQL)
        .bind(key_id)
        .bind(i32::try_from(delta.request_count).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "provider_api_keys.request_count delta exceeds i32: {}",
                delta.request_count
            ))
        })?)
        .bind(i32::try_from(delta.success_count).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "provider_api_keys.success_count delta exceeds i32: {}",
                delta.success_count
            ))
        })?)
        .bind(i32::try_from(delta.error_count).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "provider_api_keys.error_count delta exceeds i32: {}",
                delta.error_count
            ))
        })?)
        .bind(delta.total_tokens)
        .bind(total_cost_usd_delta)
        .bind(i32::try_from(delta.total_response_time_ms).map_err(|_| {
            DataLayerError::UnexpectedValue(format!(
                "provider_api_keys.total_response_time_ms delta exceeds i32: {}",
                delta.total_response_time_ms
            ))
        })?)
        .bind(
            delta
                .candidate_last_used_at_unix_secs
                .map(|value| value as f64),
        )
        .bind(
            delta
                .removed_last_used_at_unix_secs
                .map(|value| value as f64),
        )
        .execute(&mut **tx)
        .await
        .map_postgres_err()?;
    Ok(())
}

// Build the usage read model from the split storage layout.
//
// Query projections already prefer the newer audit/snapshot owners and only fall back to
// deprecated `public.usage` mirror columns for historical rows that predate the split schema.
//
// Some read paths intentionally project only the core usage fields. For the newer adjunct
// audit/snapshot columns, treat a missing projection the same as SQL NULL so older callers and
// partial rollouts do not fail the whole read.
fn row_try_get_optional<T>(
    row: &sqlx::postgres::PgRow,
    column: &str,
) -> Result<Option<T>, DataLayerError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    match row.try_get::<Option<T>, _>(column) {
        Ok(value) => Ok(value),
        Err(sqlx::Error::ColumnNotFound(_)) => Ok(None),
        Err(error) => Err(postgres_error(error)),
    }
}

fn map_usage_row(
    row: &sqlx::postgres::PgRow,
    resolve_compressed_bodies: bool,
) -> Result<StoredRequestUsageAudit, DataLayerError> {
    let mut usage = StoredRequestUsageAudit::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("request_id").map_postgres_err()?,
        row.try_get("user_id").map_postgres_err()?,
        row.try_get("api_key_id").map_postgres_err()?,
        row.try_get("username").map_postgres_err()?,
        row.try_get("api_key_name").map_postgres_err()?,
        row.try_get("provider_name").map_postgres_err()?,
        row.try_get("model").map_postgres_err()?,
        row.try_get("target_model").map_postgres_err()?,
        row.try_get("provider_id").map_postgres_err()?,
        row.try_get("provider_endpoint_id").map_postgres_err()?,
        row.try_get("provider_api_key_id").map_postgres_err()?,
        row.try_get("request_type").map_postgres_err()?,
        row.try_get("api_format").map_postgres_err()?,
        row.try_get("api_family").map_postgres_err()?,
        row.try_get("endpoint_kind").map_postgres_err()?,
        row.try_get("endpoint_api_format").map_postgres_err()?,
        row.try_get("provider_api_family").map_postgres_err()?,
        row.try_get("provider_endpoint_kind").map_postgres_err()?,
        row.try_get("has_format_conversion").map_postgres_err()?,
        row.try_get("is_stream").map_postgres_err()?,
        row.try_get("input_tokens").map_postgres_err()?,
        row.try_get("output_tokens").map_postgres_err()?,
        row.try_get("total_tokens").map_postgres_err()?,
        row.try_get("total_cost_usd").map_postgres_err()?,
        row.try_get("actual_total_cost_usd").map_postgres_err()?,
        row.try_get("status_code").map_postgres_err()?,
        row.try_get("error_message").map_postgres_err()?,
        row.try_get("error_category").map_postgres_err()?,
        row.try_get("response_time_ms").map_postgres_err()?,
        row.try_get("first_byte_time_ms").map_postgres_err()?,
        row.try_get("status").map_postgres_err()?,
        row.try_get("billing_status").map_postgres_err()?,
        row.try_get("created_at_unix_ms").map_postgres_err()?,
        row.try_get("updated_at_unix_secs").map_postgres_err()?,
        row.try_get("finalized_at_unix_secs").map_postgres_err()?,
    )?;
    usage.cache_creation_input_tokens = row
        .try_get::<Option<i32>, _>("cache_creation_input_tokens")
        .map_postgres_err()?
        .map(|value| to_u64(value, "usage.cache_creation_input_tokens"))
        .transpose()?
        .unwrap_or_default();
    usage.cache_creation_ephemeral_5m_input_tokens = row
        .try_get::<Option<i32>, _>("cache_creation_ephemeral_5m_input_tokens")
        .map_postgres_err()?
        .map(|value| to_u64(value, "usage.cache_creation_ephemeral_5m_input_tokens"))
        .transpose()?
        .unwrap_or_default();
    usage.cache_creation_ephemeral_1h_input_tokens = row
        .try_get::<Option<i32>, _>("cache_creation_ephemeral_1h_input_tokens")
        .map_postgres_err()?
        .map(|value| to_u64(value, "usage.cache_creation_ephemeral_1h_input_tokens"))
        .transpose()?
        .unwrap_or_default();
    usage.cache_read_input_tokens = row
        .try_get::<Option<i32>, _>("cache_read_input_tokens")
        .map_postgres_err()?
        .map(|value| to_u64(value, "usage.cache_read_input_tokens"))
        .transpose()?
        .unwrap_or_default();
    usage.cache_creation_cost_usd = row
        .try_get::<f64, _>("cache_creation_cost_usd")
        .map_postgres_err()?;
    usage.cache_read_cost_usd = row
        .try_get::<f64, _>("cache_read_cost_usd")
        .map_postgres_err()?;
    usage.output_price_per_1m = row.try_get("output_price_per_1m").map_postgres_err()?;
    usage.request_headers = row.try_get("request_headers").map_postgres_err()?;
    let request_body = usage_json_column(
        row,
        "request_body",
        "request_body_compressed",
        resolve_compressed_bodies,
    )?;
    usage.provider_request_headers = row.try_get("provider_request_headers").map_postgres_err()?;
    let provider_request_body = usage_json_column(
        row,
        "provider_request_body",
        "provider_request_body_compressed",
        resolve_compressed_bodies,
    )?;
    usage.response_headers = row.try_get("response_headers").map_postgres_err()?;
    let response_body = usage_json_column(
        row,
        "response_body",
        "response_body_compressed",
        resolve_compressed_bodies,
    )?;
    usage.client_response_headers = row.try_get("client_response_headers").map_postgres_err()?;
    let client_response_body = usage_json_column(
        row,
        "client_response_body",
        "client_response_body_compressed",
        resolve_compressed_bodies,
    )?;
    let request_metadata: Option<Value> = row.try_get("request_metadata").map_postgres_err()?;
    let http_audit_refs = UsageHttpAuditRefs {
        request_body_ref: row_try_get_optional(row, "http_request_body_ref")?,
        provider_request_body_ref: row_try_get_optional(row, "http_provider_request_body_ref")?,
        response_body_ref: row_try_get_optional(row, "http_response_body_ref")?,
        client_response_body_ref: row_try_get_optional(row, "http_client_response_body_ref")?,
    };
    let http_audit_states = UsageHttpAuditStates {
        request_body_state: row_try_get_optional::<String>(row, "http_request_body_state")?
            .as_deref()
            .and_then(parse_usage_body_capture_state),
        provider_request_body_state: row_try_get_optional::<String>(
            row,
            "http_provider_request_body_state",
        )?
        .as_deref()
        .and_then(parse_usage_body_capture_state),
        response_body_state: row_try_get_optional::<String>(row, "http_response_body_state")?
            .as_deref()
            .and_then(parse_usage_body_capture_state),
        client_response_body_state: row_try_get_optional::<String>(
            row,
            "http_client_response_body_state",
        )?
        .as_deref()
        .and_then(parse_usage_body_capture_state),
    };
    let routing_snapshot = usage_routing_snapshot_from_row(row)?;
    let settlement_pricing_snapshot = usage_settlement_pricing_snapshot_from_row(row)?;
    usage.request_body = request_body.value;
    usage.provider_request_body = provider_request_body.value;
    usage.response_body = response_body.value;
    usage.client_response_body = client_response_body.value;
    let request_metadata_object = request_metadata.as_ref().and_then(Value::as_object);
    usage.request_body_ref = resolved_read_usage_body_ref(
        None,
        request_metadata_object,
        &usage.request_id,
        UsageBodyField::RequestBody,
        request_body.has_compressed_storage,
        http_audit_refs.request_body_ref.as_deref(),
    );
    usage.provider_request_body_ref = resolved_read_usage_body_ref(
        None,
        request_metadata_object,
        &usage.request_id,
        UsageBodyField::ProviderRequestBody,
        provider_request_body.has_compressed_storage,
        http_audit_refs.provider_request_body_ref.as_deref(),
    );
    usage.response_body_ref = resolved_read_usage_body_ref(
        None,
        request_metadata_object,
        &usage.request_id,
        UsageBodyField::ResponseBody,
        response_body.has_compressed_storage,
        http_audit_refs.response_body_ref.as_deref(),
    );
    usage.client_response_body_ref = resolved_read_usage_body_ref(
        None,
        request_metadata_object,
        &usage.request_id,
        UsageBodyField::ClientResponseBody,
        client_response_body.has_compressed_storage,
        http_audit_refs.client_response_body_ref.as_deref(),
    );
    usage.request_body_state = http_audit_states.request_body_state;
    usage.provider_request_body_state = http_audit_states.provider_request_body_state;
    usage.response_body_state = http_audit_states.response_body_state;
    usage.client_response_body_state = http_audit_states.client_response_body_state;
    usage.candidate_id = routing_snapshot.candidate_id.clone();
    usage.candidate_index = routing_snapshot.candidate_index;
    usage.key_name = routing_snapshot.key_name.clone();
    usage.planner_kind = routing_snapshot.planner_kind.clone();
    usage.route_family = routing_snapshot.route_family.clone();
    usage.route_kind = routing_snapshot.route_kind.clone();
    usage.execution_path = routing_snapshot.execution_path.clone();
    usage.local_execution_runtime_miss_reason =
        routing_snapshot.local_execution_runtime_miss_reason.clone();
    usage.request_metadata = attach_usage_settlement_pricing_snapshot_metadata(
        request_metadata,
        &settlement_pricing_snapshot,
    );
    Ok(usage)
}

fn to_i32(value: u64) -> Result<i32, DataLayerError> {
    i32::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("invalid usage integer value: {value}"))
    })
}

fn to_u64(value: i32, field_name: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value)
        .map_err(|_| DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageBodyStorage {
    inline_json: Option<String>,
    detached_blob_bytes: Option<Vec<u8>>,
}

impl UsageBodyStorage {
    fn has_detached_blob(&self) -> bool {
        self.detached_blob_bytes.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct UsageBodyColumn {
    value: Option<Value>,
    has_compressed_storage: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageHttpAuditRefs {
    request_body_ref: Option<String>,
    provider_request_body_ref: Option<String>,
    response_body_ref: Option<String>,
    client_response_body_ref: Option<String>,
}

impl UsageHttpAuditRefs {
    fn any_present(&self) -> bool {
        self.request_body_ref.is_some()
            || self.provider_request_body_ref.is_some()
            || self.response_body_ref.is_some()
            || self.client_response_body_ref.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageHttpAuditStates {
    request_body_state: Option<UsageBodyCaptureState>,
    provider_request_body_state: Option<UsageBodyCaptureState>,
    response_body_state: Option<UsageBodyCaptureState>,
    client_response_body_state: Option<UsageBodyCaptureState>,
}

impl UsageHttpAuditStates {
    fn any_present(&self) -> bool {
        self.request_body_state.is_some()
            || self.provider_request_body_state.is_some()
            || self.response_body_state.is_some()
            || self.client_response_body_state.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageHttpAuditHeaders<'a> {
    request_headers_json: Option<&'a str>,
    provider_request_headers_json: Option<&'a str>,
    response_headers_json: Option<&'a str>,
    client_response_headers_json: Option<&'a str>,
}

impl UsageHttpAuditHeaders<'_> {
    fn any_present(&self) -> bool {
        self.request_headers_json.is_some()
            || self.provider_request_headers_json.is_some()
            || self.response_headers_json.is_some()
            || self.client_response_headers_json.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UsageRoutingSnapshot {
    candidate_id: Option<String>,
    candidate_index: Option<u64>,
    key_name: Option<String>,
    planner_kind: Option<String>,
    route_family: Option<String>,
    route_kind: Option<String>,
    execution_path: Option<String>,
    local_execution_runtime_miss_reason: Option<String>,
    selected_provider_id: Option<String>,
    selected_endpoint_id: Option<String>,
    selected_provider_api_key_id: Option<String>,
    has_format_conversion: Option<bool>,
}

impl UsageRoutingSnapshot {
    fn has_metadata_fields(&self) -> bool {
        self.candidate_id.is_some()
            || self.candidate_index.is_some()
            || self.key_name.is_some()
            || self.planner_kind.is_some()
            || self.route_family.is_some()
            || self.route_kind.is_some()
            || self.execution_path.is_some()
            || self.local_execution_runtime_miss_reason.is_some()
    }

    fn any_present(&self) -> bool {
        self.has_metadata_fields()
            || self.selected_provider_id.is_some()
            || self.selected_endpoint_id.is_some()
            || self.selected_provider_api_key_id.is_some()
            || self.has_format_conversion.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct UsageSettlementPricingSnapshot {
    billing_status: Option<String>,
    billing_snapshot_schema_version: Option<String>,
    billing_snapshot_status: Option<String>,
    settlement_snapshot_schema_version: Option<String>,
    settlement_snapshot: Option<Value>,
    billing_dimensions: Option<Value>,
    billing_input_tokens: Option<i64>,
    billing_effective_input_tokens: Option<i64>,
    billing_output_tokens: Option<i64>,
    billing_cache_creation_tokens: Option<i64>,
    billing_cache_creation_5m_tokens: Option<i64>,
    billing_cache_creation_1h_tokens: Option<i64>,
    billing_cache_read_tokens: Option<i64>,
    billing_total_input_context: Option<i64>,
    billing_cache_creation_cost_usd: Option<f64>,
    billing_cache_read_cost_usd: Option<f64>,
    billing_total_cost_usd: Option<f64>,
    billing_actual_total_cost_usd: Option<f64>,
    billing_pricing_source: Option<String>,
    billing_rule_id: Option<String>,
    billing_rule_version: Option<String>,
    rate_multiplier: Option<f64>,
    is_free_tier: Option<bool>,
    input_price_per_1m: Option<f64>,
    output_price_per_1m: Option<f64>,
    cache_creation_price_per_1m: Option<f64>,
    cache_read_price_per_1m: Option<f64>,
    price_per_request: Option<f64>,
}

impl UsageSettlementPricingSnapshot {
    fn any_present(&self) -> bool {
        self.billing_status.is_some()
            || self.billing_snapshot_schema_version.is_some()
            || self.billing_snapshot_status.is_some()
            || self.settlement_snapshot_schema_version.is_some()
            || self.settlement_snapshot.is_some()
            || self.billing_dimensions.is_some()
            || self.billing_input_tokens.is_some()
            || self.billing_effective_input_tokens.is_some()
            || self.billing_output_tokens.is_some()
            || self.billing_cache_creation_tokens.is_some()
            || self.billing_cache_creation_5m_tokens.is_some()
            || self.billing_cache_creation_1h_tokens.is_some()
            || self.billing_cache_read_tokens.is_some()
            || self.billing_total_input_context.is_some()
            || self.billing_cache_creation_cost_usd.is_some()
            || self.billing_cache_read_cost_usd.is_some()
            || self.billing_total_cost_usd.is_some()
            || self.billing_actual_total_cost_usd.is_some()
            || self.billing_pricing_source.is_some()
            || self.billing_rule_id.is_some()
            || self.billing_rule_version.is_some()
            || self.rate_multiplier.is_some()
            || self.is_free_tier.is_some()
            || self.input_price_per_1m.is_some()
            || self.output_price_per_1m.is_some()
            || self.cache_creation_price_per_1m.is_some()
            || self.cache_read_price_per_1m.is_some()
            || self.price_per_request.is_some()
    }
}

fn prepare_usage_body_storage(value: Option<&Value>) -> Result<UsageBodyStorage, DataLayerError> {
    let Some(value) = value else {
        return Ok(UsageBodyStorage {
            inline_json: None,
            detached_blob_bytes: None,
        });
    };
    let bytes = serde_json::to_vec(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to serialize usage json: {err}"))
    })?;
    if bytes.len() == MAX_INLINE_USAGE_BODY_BYTES {
        return Ok(UsageBodyStorage {
            inline_json: Some(String::from_utf8(bytes).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "failed to encode inline usage body as utf-8: {err}"
                ))
            })?),
            detached_blob_bytes: None,
        });
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(&bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to compress usage json: {err}"))
    })?;
    let detached_blob_bytes = encoder.finish().map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to finish usage json compression: {err}"))
    })?;
    Ok(UsageBodyStorage {
        inline_json: None,
        detached_blob_bytes: Some(detached_blob_bytes),
    })
}

fn json_bind_text(value: Option<&Value>) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!("failed to serialize usage json: {err}"))
            })
        })
        .transpose()
}

fn usage_body_capture_state_bind_text(
    value: Option<UsageBodyCaptureState>,
) -> Option<&'static str> {
    value.map(UsageBodyCaptureState::as_str)
}

fn parse_usage_body_capture_state(value: &str) -> Option<UsageBodyCaptureState> {
    match value.trim() {
        "none" => Some(UsageBodyCaptureState::None),
        "inline" => Some(UsageBodyCaptureState::Inline),
        "reference" => Some(UsageBodyCaptureState::Reference),
        "truncated" => Some(UsageBodyCaptureState::Truncated),
        "disabled" => Some(UsageBodyCaptureState::Disabled),
        "unavailable" => Some(UsageBodyCaptureState::Unavailable),
        _ => None,
    }
}

#[cfg(test)]
fn usage_http_audit_body_refs(metadata: Option<&Value>) -> UsageHttpAuditRefs {
    let object = metadata.and_then(Value::as_object);
    UsageHttpAuditRefs {
        request_body_ref: metadata_ref_value(object, "request_body_ref"),
        provider_request_body_ref: metadata_ref_value(object, "provider_request_body_ref"),
        response_body_ref: metadata_ref_value(object, "response_body_ref"),
        client_response_body_ref: metadata_ref_value(object, "client_response_body_ref"),
    }
}

fn resolved_read_usage_body_ref(
    explicit_ref: Option<&str>,
    metadata: Option<&serde_json::Map<String, Value>>,
    request_id: &str,
    field: UsageBodyField,
    has_compressed_storage: bool,
    http_audit_ref: Option<&str>,
) -> Option<String> {
    explicit_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            http_audit_ref
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| has_compressed_storage.then(|| usage_body_ref(request_id, field)))
        .or_else(|| metadata_usage_body_ref_value(metadata, request_id, field))
}

fn resolved_write_usage_body_ref(
    explicit_ref: Option<&str>,
    request_id: &str,
    field: UsageBodyField,
    has_compressed_storage: bool,
    http_audit_ref: Option<&str>,
) -> Option<String> {
    explicit_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| has_compressed_storage.then(|| usage_body_ref(request_id, field)))
        .or_else(|| {
            http_audit_ref
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn metadata_ref_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<String> {
    metadata
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn metadata_usage_body_ref_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    request_id: &str,
    field: UsageBodyField,
) -> Option<String> {
    metadata_ref_value(metadata, field.as_ref_key())
        .and_then(|value| parse_usage_body_ref(&value))
        .filter(|(parsed_request_id, parsed_field)| {
            parsed_request_id == request_id && *parsed_field == field
        })
        .map(|(parsed_request_id, parsed_field)| usage_body_ref(&parsed_request_id, parsed_field))
}

fn metadata_number_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<f64> {
    metadata
        .and_then(|object| object.get(key))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn metadata_u64_value(metadata: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<u64> {
    metadata.and_then(|object| {
        object.get(key).and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        })
    })
}

fn metadata_bool_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<bool> {
    metadata
        .and_then(|object| object.get(key))
        .and_then(Value::as_bool)
}

fn billing_snapshot_object(
    metadata: Option<&serde_json::Map<String, Value>>,
) -> Option<&serde_json::Map<String, Value>> {
    metadata
        .and_then(|object| object.get("billing_snapshot"))
        .and_then(Value::as_object)
}

fn billing_snapshot_string_value(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<String> {
    billing_snapshot_object(metadata)
        .and_then(|snapshot| snapshot.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn billing_snapshot_resolved_number(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<f64> {
    billing_snapshot_object(metadata)
        .and_then(|snapshot| snapshot.get("resolved_variables"))
        .and_then(Value::as_object)
        .and_then(|variables| variables.get(key))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn settlement_snapshot_object(
    metadata: Option<&serde_json::Map<String, Value>>,
) -> Option<&serde_json::Map<String, Value>> {
    metadata
        .and_then(|object| object.get("settlement_snapshot"))
        .and_then(Value::as_object)
}

fn settlement_snapshot_schema_version(
    metadata: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    metadata_ref_value(metadata, "settlement_snapshot_schema_version").or_else(|| {
        settlement_snapshot_object(metadata)
            .and_then(|snapshot| snapshot.get("schema_version"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn settlement_snapshot_value(metadata: Option<&serde_json::Map<String, Value>>) -> Option<Value> {
    metadata
        .and_then(|object| object.get("settlement_snapshot"))
        .cloned()
}

fn settlement_snapshot_child_value<'a>(
    metadata: Option<&'a serde_json::Map<String, Value>>,
    child: &str,
) -> Option<&'a Value> {
    settlement_snapshot_object(metadata).and_then(|snapshot| snapshot.get(child))
}

fn settlement_snapshot_child_object<'a>(
    metadata: Option<&'a serde_json::Map<String, Value>>,
    child: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    settlement_snapshot_child_value(metadata, child).and_then(Value::as_object)
}

fn metadata_or_snapshot_dimensions(
    metadata: Option<&serde_json::Map<String, Value>>,
) -> Option<Value> {
    metadata
        .and_then(|object| object.get("billing_dimensions"))
        .cloned()
        .or_else(|| settlement_snapshot_child_value(metadata, "resolved_dimensions").cloned())
        .or_else(|| {
            billing_snapshot_object(metadata)
                .and_then(|snapshot| snapshot.get("resolved_dimensions"))
                .cloned()
        })
}

fn json_i64_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
}

fn billing_dimension_i64(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<i64> {
    metadata_or_snapshot_dimensions(metadata)
        .and_then(|dimensions| dimensions.get(key).and_then(json_i64_value))
        .filter(|value| *value >= 0)
}

fn settlement_snapshot_number(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<f64> {
    settlement_snapshot_object(metadata)
        .and_then(|snapshot| snapshot.get(key))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn billing_snapshot_number(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<f64> {
    billing_snapshot_object(metadata)
        .and_then(|snapshot| snapshot.get(key))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn settlement_cost_breakdown_number(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<f64> {
    settlement_snapshot_child_object(metadata, "cost_breakdown")
        .or_else(|| {
            billing_snapshot_object(metadata)
                .and_then(|snapshot| snapshot.get("cost_breakdown"))
                .and_then(Value::as_object)
        })
        .and_then(|breakdown| breakdown.get(key))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn settlement_cache_creation_cost(
    metadata: Option<&serde_json::Map<String, Value>>,
) -> Option<f64> {
    let keys = [
        "cache_creation_uncategorized_cost",
        "cache_creation_ephemeral_5m_cost",
        "cache_creation_ephemeral_1h_cost",
        "cache_creation_cost",
    ];
    let mut found = false;
    let total = keys.into_iter().fold(0.0, |sum, key| {
        if let Some(value) = settlement_cost_breakdown_number(metadata, key) {
            found = true;
            sum + value
        } else {
            sum
        }
    });
    found.then_some(total)
}

fn settlement_snapshot_nested_string(
    metadata: Option<&serde_json::Map<String, Value>>,
    child: &str,
    key: &str,
) -> Option<String> {
    settlement_snapshot_child_object(metadata, child)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn billing_snapshot_string_field(
    metadata: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Option<String> {
    billing_snapshot_object(metadata)
        .and_then(|snapshot| snapshot.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn usage_http_audit_capture_mode(
    refs: &UsageHttpAuditRefs,
    body_values: [Option<&Value>; 4],
) -> &'static str {
    if refs.any_present() {
        return "ref_backed";
    }
    if body_values.iter().any(Option::is_some) {
        return "inline_legacy";
    }
    "none"
}

fn usage_routing_snapshot_from_usage(
    usage: &UpsertUsageRecord,
    metadata: Option<&Value>,
) -> UsageRoutingSnapshot {
    let object = metadata.and_then(Value::as_object);
    let mut snapshot = UsageRoutingSnapshot {
        candidate_id: usage
            .candidate_id
            .clone()
            .or_else(|| metadata_ref_value(object, "candidate_id")),
        candidate_index: usage
            .candidate_index
            .or_else(|| metadata_u64_value(object, "candidate_index")),
        key_name: usage
            .key_name
            .clone()
            .or_else(|| metadata_ref_value(object, "key_name")),
        planner_kind: usage
            .planner_kind
            .clone()
            .or_else(|| metadata_ref_value(object, "planner_kind")),
        route_family: usage
            .route_family
            .clone()
            .or_else(|| metadata_ref_value(object, "route_family")),
        route_kind: usage
            .route_kind
            .clone()
            .or_else(|| metadata_ref_value(object, "route_kind")),
        execution_path: usage
            .execution_path
            .clone()
            .or_else(|| metadata_ref_value(object, "execution_path")),
        local_execution_runtime_miss_reason: usage
            .local_execution_runtime_miss_reason
            .clone()
            .or_else(|| metadata_ref_value(object, "local_execution_runtime_miss_reason")),
        selected_provider_id: None,
        selected_endpoint_id: None,
        selected_provider_api_key_id: None,
        has_format_conversion: None,
    };
    if !snapshot.has_metadata_fields() {
        return snapshot;
    }

    snapshot.selected_provider_id = usage.provider_id.clone();
    snapshot.selected_endpoint_id = usage.provider_endpoint_id.clone();
    snapshot.selected_provider_api_key_id = usage.provider_api_key_id.clone();
    snapshot.has_format_conversion = usage.has_format_conversion;
    snapshot
}

fn usage_optional_i64(value: Option<u64>, field_name: &str) -> Result<Option<i64>, DataLayerError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!(
                    "usage {field_name} exceeds bigint: {value}"
                ))
            })
        })
        .transpose()
}

fn usage_cache_creation_tokens_from_parts(
    uncategorized: Option<i64>,
    ephemeral_5m: Option<i64>,
    ephemeral_1h: Option<i64>,
) -> Option<i64> {
    let categorized = ephemeral_5m
        .unwrap_or_default()
        .saturating_add(ephemeral_1h.unwrap_or_default());
    match uncategorized {
        Some(0) if categorized > 0 => Some(categorized),
        Some(value) => Some(value),
        None if categorized > 0 => Some(categorized),
        None => None,
    }
}

fn usage_normalized_api_family(usage: &UpsertUsageRecord) -> String {
    usage
        .endpoint_api_format
        .as_deref()
        .or(usage.api_format.as_deref())
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn usage_effective_input_tokens(
    input_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    api_family: &str,
) -> Option<i64> {
    let input_tokens = input_tokens?;
    let cache_read_tokens = cache_read_tokens.unwrap_or_default();
    if matches!(api_family, "openai" | "gemini" | "google")
        && input_tokens > 0
        && cache_read_tokens > 0
    {
        return Some(input_tokens.saturating_sub(cache_read_tokens));
    }
    Some(input_tokens)
}

fn usage_total_input_context(
    input_tokens: Option<i64>,
    effective_input_tokens: Option<i64>,
    cache_creation_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    api_family: &str,
) -> Option<i64> {
    if input_tokens.is_none()
        && effective_input_tokens.is_none()
        && cache_creation_tokens.is_none()
        && cache_read_tokens.is_none()
    {
        return None;
    }

    let input_tokens = input_tokens.unwrap_or_default();
    let effective_input_tokens = effective_input_tokens.unwrap_or(input_tokens);
    let cache_creation_tokens = cache_creation_tokens.unwrap_or_default();
    let cache_read_tokens = cache_read_tokens.unwrap_or_default();
    match api_family {
        "claude" | "anthropic" => Some(
            input_tokens
                .saturating_add(cache_creation_tokens)
                .saturating_add(cache_read_tokens),
        ),
        "openai" | "gemini" | "google" => {
            Some(effective_input_tokens.saturating_add(cache_read_tokens))
        }
        _ => Some(
            input_tokens
                .saturating_add(cache_creation_tokens)
                .saturating_add(cache_read_tokens),
        ),
    }
}

fn usage_settlement_pricing_snapshot_from_usage(
    usage: &UpsertUsageRecord,
    metadata: Option<&Value>,
) -> Result<UsageSettlementPricingSnapshot, DataLayerError> {
    let object = metadata.and_then(Value::as_object);
    let billing_dimensions = metadata_or_snapshot_dimensions(object);
    let has_billing_dimensions = billing_dimensions.is_some();
    let usage_input_tokens = usage_optional_i64(usage.input_tokens, "input_tokens")?;
    let usage_output_tokens = usage_optional_i64(usage.output_tokens, "output_tokens")?;
    let usage_cache_creation_uncategorized_tokens = usage_optional_i64(
        usage.cache_creation_input_tokens,
        "cache_creation_input_tokens",
    )?;
    let usage_cache_creation_5m_tokens = usage_optional_i64(
        usage.cache_creation_ephemeral_5m_input_tokens,
        "cache_creation_ephemeral_5m_input_tokens",
    )?;
    let usage_cache_creation_1h_tokens = usage_optional_i64(
        usage.cache_creation_ephemeral_1h_input_tokens,
        "cache_creation_ephemeral_1h_input_tokens",
    )?;
    let usage_cache_read_tokens =
        usage_optional_i64(usage.cache_read_input_tokens, "cache_read_input_tokens")?;
    let usage_cache_creation_tokens = usage_cache_creation_tokens_from_parts(
        usage_cache_creation_uncategorized_tokens,
        usage_cache_creation_5m_tokens,
        usage_cache_creation_1h_tokens,
    );
    let billing_cache_creation_tokens = billing_dimension_i64(object, "cache_creation_tokens")
        .or_else(|| {
            usage_cache_creation_tokens_from_parts(
                billing_dimension_i64(object, "cache_creation_uncategorized_tokens"),
                billing_dimension_i64(object, "cache_creation_ephemeral_5m_tokens"),
                billing_dimension_i64(object, "cache_creation_ephemeral_1h_tokens"),
            )
        })
        .or(usage_cache_creation_tokens);
    let billing_cache_creation_5m_tokens =
        billing_dimension_i64(object, "cache_creation_ephemeral_5m_tokens")
            .or(usage_cache_creation_5m_tokens);
    let billing_cache_creation_1h_tokens =
        billing_dimension_i64(object, "cache_creation_ephemeral_1h_tokens")
            .or(usage_cache_creation_1h_tokens);
    let billing_input_tokens = billing_dimension_i64(object, "input_tokens").or(usage_input_tokens);
    let billing_output_tokens =
        billing_dimension_i64(object, "output_tokens").or(usage_output_tokens);
    let billing_cache_read_tokens =
        billing_dimension_i64(object, "cache_read_tokens").or(usage_cache_read_tokens);
    let api_family = usage_normalized_api_family(usage);
    let billing_effective_input_tokens = billing_dimension_i64(object, "effective_input_tokens")
        .or_else(|| {
            has_billing_dimensions
                .then(|| billing_dimension_i64(object, "input_tokens"))
                .flatten()
        })
        .or_else(|| {
            usage_effective_input_tokens(
                billing_input_tokens,
                billing_cache_read_tokens,
                api_family.as_str(),
            )
        });
    let billing_total_input_context =
        billing_dimension_i64(object, "total_input_context").or_else(|| {
            usage_total_input_context(
                billing_input_tokens,
                billing_effective_input_tokens,
                billing_cache_creation_tokens,
                billing_cache_read_tokens,
                api_family.as_str(),
            )
        });
    let snapshot = UsageSettlementPricingSnapshot {
        billing_status: Some(usage.billing_status.clone()),
        billing_snapshot_schema_version: metadata_ref_value(
            object,
            "billing_snapshot_schema_version",
        )
        .or_else(|| billing_snapshot_string_value(object, "schema_version")),
        billing_snapshot_status: metadata_ref_value(object, "billing_snapshot_status")
            .or_else(|| billing_snapshot_string_value(object, "status")),
        settlement_snapshot_schema_version: settlement_snapshot_schema_version(object),
        settlement_snapshot: settlement_snapshot_value(object),
        billing_dimensions,
        billing_input_tokens,
        billing_effective_input_tokens,
        billing_output_tokens,
        billing_cache_creation_tokens,
        billing_cache_creation_5m_tokens,
        billing_cache_creation_1h_tokens,
        billing_cache_read_tokens,
        billing_total_input_context,
        billing_cache_creation_cost_usd: settlement_cache_creation_cost(object)
            .or(usage.cache_creation_cost_usd),
        billing_cache_read_cost_usd: settlement_cost_breakdown_number(object, "cache_read_cost")
            .or(usage.cache_read_cost_usd),
        billing_total_cost_usd: settlement_snapshot_number(object, "total_cost")
            .or_else(|| billing_snapshot_number(object, "total_cost"))
            .or(usage.total_cost_usd),
        billing_actual_total_cost_usd: settlement_snapshot_number(object, "actual_total_cost")
            .or(usage.actual_total_cost_usd),
        billing_pricing_source: settlement_snapshot_nested_string(
            object,
            "pricing_snapshot",
            "pricing_source",
        ),
        billing_rule_id: settlement_snapshot_nested_string(
            object,
            "billing_plan_snapshot",
            "rule_id",
        )
        .or_else(|| billing_snapshot_string_field(object, "rule_id")),
        billing_rule_version: settlement_snapshot_nested_string(
            object,
            "billing_plan_snapshot",
            "rule_version",
        ),
        rate_multiplier: metadata_number_value(object, "rate_multiplier"),
        is_free_tier: metadata_bool_value(object, "is_free_tier"),
        input_price_per_1m: metadata_number_value(object, "input_price_per_1m")
            .or_else(|| billing_snapshot_resolved_number(object, "input_price_per_1m")),
        output_price_per_1m: metadata_number_value(object, "output_price_per_1m")
            .or_else(|| billing_snapshot_resolved_number(object, "output_price_per_1m"))
            .or(usage.output_price_per_1m),
        cache_creation_price_per_1m: metadata_number_value(object, "cache_creation_price_per_1m")
            .or_else(|| billing_snapshot_resolved_number(object, "cache_creation_price_per_1m")),
        cache_read_price_per_1m: metadata_number_value(object, "cache_read_price_per_1m")
            .or_else(|| billing_snapshot_resolved_number(object, "cache_read_price_per_1m")),
        price_per_request: metadata_number_value(object, "price_per_request")
            .or_else(|| billing_snapshot_resolved_number(object, "price_per_request")),
    };
    Ok(if snapshot.any_present() {
        snapshot
    } else {
        UsageSettlementPricingSnapshot::default()
    })
}

// Decode deprecated inline/compressed body columns from `public.usage`.
//
// New writes keep these columns empty by forcing body storage through `usage_body_blobs` and
// `usage_http_audits`; this helper exists only so older rows remain readable without backfill.
fn usage_json_column(
    row: &sqlx::postgres::PgRow,
    inline_column: &str,
    compressed_column: &str,
    resolve_compressed: bool,
) -> Result<UsageBodyColumn, DataLayerError> {
    let inline = row
        .try_get::<Option<Value>, _>(inline_column)
        .map_postgres_err()?;
    if inline.is_some() {
        return Ok(UsageBodyColumn {
            value: inline,
            has_compressed_storage: false,
        });
    }
    let compressed = row
        .try_get::<Option<Vec<u8>>, _>(compressed_column)
        .map_postgres_err()?;
    let has_compressed_storage = compressed.is_some();
    let value = if resolve_compressed {
        compressed
            .map(|bytes| inflate_usage_json_value(&bytes))
            .transpose()?
    } else {
        None
    };
    Ok(UsageBodyColumn {
        value,
        has_compressed_storage,
    })
}

fn inflate_usage_json_value(bytes: &[u8]) -> Result<Value, DataLayerError> {
    let mut decoder = GzDecoder::new(bytes);
    let mut json_bytes = Vec::new();
    decoder.read_to_end(&mut json_bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to decompress usage json: {err}"))
    })?;
    serde_json::from_slice(&json_bytes).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to parse decompressed usage json: {err}"))
    })
}

#[cfg(test)]
fn attach_compressed_body_refs(
    request_id: &str,
    metadata: Option<Value>,
    has_request_body_compressed: bool,
    has_provider_request_body_compressed: bool,
    has_response_body_compressed: bool,
    has_client_response_body_compressed: bool,
) -> Option<Value> {
    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => return Some(value),
        None => Map::new(),
    };
    maybe_insert_usage_body_ref(
        &mut metadata,
        "request_body_ref",
        request_id,
        "request_body",
        has_request_body_compressed,
    );
    maybe_insert_usage_body_ref(
        &mut metadata,
        "provider_request_body_ref",
        request_id,
        "provider_request_body",
        has_provider_request_body_compressed,
    );
    maybe_insert_usage_body_ref(
        &mut metadata,
        "response_body_ref",
        request_id,
        "response_body",
        has_response_body_compressed,
    );
    maybe_insert_usage_body_ref(
        &mut metadata,
        "client_response_body_ref",
        request_id,
        "client_response_body",
        has_client_response_body_compressed,
    );
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

#[cfg(test)]
fn attach_usage_http_audit_body_refs(
    metadata: Option<Value>,
    refs: &UsageHttpAuditRefs,
) -> Option<Value> {
    if !refs.any_present() {
        return metadata;
    }

    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => return Some(value),
        None => Map::new(),
    };
    maybe_insert_string_value(
        &mut metadata,
        "request_body_ref",
        refs.request_body_ref.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "provider_request_body_ref",
        refs.provider_request_body_ref.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "response_body_ref",
        refs.response_body_ref.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "client_response_body_ref",
        refs.client_response_body_ref.as_deref(),
    );
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

#[cfg(test)]
fn attach_usage_routing_snapshot_metadata(
    metadata: Option<Value>,
    snapshot: &UsageRoutingSnapshot,
) -> Option<Value> {
    if !snapshot.has_metadata_fields() {
        return metadata;
    }

    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => return Some(value),
        None => Map::new(),
    };
    maybe_insert_string_value(
        &mut metadata,
        "candidate_id",
        snapshot.candidate_id.as_deref(),
    );
    maybe_insert_string_value(&mut metadata, "key_name", snapshot.key_name.as_deref());
    maybe_insert_string_value(
        &mut metadata,
        "planner_kind",
        snapshot.planner_kind.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "route_family",
        snapshot.route_family.as_deref(),
    );
    maybe_insert_string_value(&mut metadata, "route_kind", snapshot.route_kind.as_deref());
    maybe_insert_string_value(
        &mut metadata,
        "execution_path",
        snapshot.execution_path.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "local_execution_runtime_miss_reason",
        snapshot.local_execution_runtime_miss_reason.as_deref(),
    );
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

fn prepare_request_metadata_for_body_storage<const N: usize>(
    metadata: Option<Value>,
    body_fields: [(
        UsageBodyField,
        &UsageBodyStorage,
        Option<&Value>,
        Option<&str>,
    ); N],
) -> Option<Value> {
    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => {
            let mut object = Map::new();
            object.insert("request_metadata".to_string(), value);
            object
        }
        None => Map::new(),
    };
    let should_replace = !metadata.is_empty()
        || body_fields.iter().any(|(_, storage, value, explicit_ref)| {
            storage.has_detached_blob() || value.is_some() || explicit_ref.is_some()
        });
    if !should_replace {
        return None;
    }

    for (field, storage, value, explicit_ref) in body_fields {
        if storage.has_detached_blob() || value.is_some() || explicit_ref.is_some() {
            let ref_key = field.as_ref_key();
            metadata.remove(ref_key);
        }
    }

    Some(Value::Object(metadata))
}

async fn sync_usage_body_blob_storage<'e, E>(
    executor: E,
    request_id: &str,
    field: UsageBodyField,
    value: Option<&Value>,
    storage: &UsageBodyStorage,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let body_ref = usage_body_ref(request_id, field);
    if let Some(payload_gzip) = storage.detached_blob_bytes.as_ref() {
        sqlx::query(UPSERT_USAGE_BODY_BLOB_SQL)
            .bind(&body_ref)
            .bind(request_id)
            .bind(field.as_storage_field())
            .bind(payload_gzip)
            .execute(executor)
            .await
            .map_postgres_err()?;
        return Ok(());
    }

    if value.is_some() {
        sqlx::query(DELETE_USAGE_BODY_BLOB_SQL)
            .bind(&body_ref)
            .execute(executor)
            .await
            .map_postgres_err()?;
    }

    Ok(())
}

async fn sync_usage_http_audit_storage<'e, E>(
    executor: E,
    request_id: &str,
    headers: &UsageHttpAuditHeaders<'_>,
    refs: &UsageHttpAuditRefs,
    states: &UsageHttpAuditStates,
    body_capture_mode: &str,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    if !headers.any_present()
        && !refs.any_present()
        && !states.any_present()
        && body_capture_mode == "none"
    {
        return Ok(());
    }

    sqlx::query(UPSERT_USAGE_HTTP_AUDIT_SQL)
        .bind(request_id)
        .bind(headers.request_headers_json)
        .bind(headers.provider_request_headers_json)
        .bind(headers.response_headers_json)
        .bind(headers.client_response_headers_json)
        .bind(refs.request_body_ref.as_deref())
        .bind(refs.provider_request_body_ref.as_deref())
        .bind(refs.response_body_ref.as_deref())
        .bind(refs.client_response_body_ref.as_deref())
        .bind(usage_body_capture_state_bind_text(
            states.request_body_state,
        ))
        .bind(usage_body_capture_state_bind_text(
            states.provider_request_body_state,
        ))
        .bind(usage_body_capture_state_bind_text(
            states.response_body_state,
        ))
        .bind(usage_body_capture_state_bind_text(
            states.client_response_body_state,
        ))
        .bind(body_capture_mode)
        .execute(executor)
        .await
        .map_postgres_err()?;

    Ok(())
}

async fn sync_usage_routing_snapshot_storage<'e, E>(
    executor: E,
    request_id: &str,
    snapshot: &UsageRoutingSnapshot,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    if !snapshot.any_present() {
        return Ok(());
    }

    sqlx::query(UPSERT_USAGE_ROUTING_SNAPSHOT_SQL)
        .bind(request_id)
        .bind(snapshot.candidate_id.as_deref())
        .bind(snapshot.candidate_index.map(to_i32).transpose()?)
        .bind(snapshot.key_name.as_deref())
        .bind(snapshot.planner_kind.as_deref())
        .bind(snapshot.route_family.as_deref())
        .bind(snapshot.route_kind.as_deref())
        .bind(snapshot.execution_path.as_deref())
        .bind(snapshot.local_execution_runtime_miss_reason.as_deref())
        .bind(snapshot.selected_provider_id.as_deref())
        .bind(snapshot.selected_endpoint_id.as_deref())
        .bind(snapshot.selected_provider_api_key_id.as_deref())
        .bind(snapshot.has_format_conversion)
        .execute(executor)
        .await
        .map_postgres_err()?;

    Ok(())
}

async fn sync_usage_settlement_pricing_snapshot_storage<'e, E>(
    executor: E,
    request_id: &str,
    snapshot: &UsageSettlementPricingSnapshot,
) -> Result<(), DataLayerError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    if !snapshot.any_present() {
        return Ok(());
    }

    sqlx::query(UPSERT_USAGE_SETTLEMENT_PRICING_SNAPSHOT_SQL)
        .bind(request_id)
        .bind(snapshot.billing_status.as_deref().unwrap_or("pending"))
        .bind(snapshot.billing_snapshot_schema_version.as_deref())
        .bind(snapshot.billing_snapshot_status.as_deref())
        .bind(snapshot.settlement_snapshot_schema_version.as_deref())
        .bind(snapshot.settlement_snapshot.as_ref())
        .bind(snapshot.billing_dimensions.as_ref())
        .bind(snapshot.billing_input_tokens)
        .bind(snapshot.billing_effective_input_tokens)
        .bind(snapshot.billing_output_tokens)
        .bind(snapshot.billing_cache_creation_tokens)
        .bind(snapshot.billing_cache_creation_5m_tokens)
        .bind(snapshot.billing_cache_creation_1h_tokens)
        .bind(snapshot.billing_cache_read_tokens)
        .bind(snapshot.billing_total_input_context)
        .bind(snapshot.billing_cache_creation_cost_usd)
        .bind(snapshot.billing_cache_read_cost_usd)
        .bind(snapshot.billing_total_cost_usd)
        .bind(snapshot.billing_actual_total_cost_usd)
        .bind(snapshot.billing_pricing_source.as_deref())
        .bind(snapshot.billing_rule_id.as_deref())
        .bind(snapshot.billing_rule_version.as_deref())
        .bind(snapshot.rate_multiplier)
        .bind(snapshot.is_free_tier)
        .bind(snapshot.input_price_per_1m)
        .bind(snapshot.output_price_per_1m)
        .bind(snapshot.cache_creation_price_per_1m)
        .bind(snapshot.cache_read_price_per_1m)
        .bind(snapshot.price_per_request)
        .execute(executor)
        .await
        .map_postgres_err()?;

    Ok(())
}

#[cfg(test)]
fn maybe_insert_usage_body_ref(
    metadata: &mut Map<String, Value>,
    key: &str,
    request_id: &str,
    field: &str,
    should_insert: bool,
) {
    if !should_insert || metadata.contains_key(key) {
        return;
    }
    metadata.insert(
        key.to_string(),
        Value::String(usage_body_ref(
            request_id,
            UsageBodyField::from_storage_field(field).expect("known usage body field"),
        )),
    );
}

fn maybe_insert_string_value(metadata: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    if metadata.contains_key(key) {
        return;
    }
    metadata.insert(key.to_string(), Value::String(value.to_string()));
}

fn maybe_insert_number_value(metadata: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return;
    };
    if metadata.contains_key(key) {
        return;
    }
    let Some(number) = serde_json::Number::from_f64(value) else {
        return;
    };
    metadata.insert(key.to_string(), Value::Number(number));
}

fn maybe_insert_bool_value(metadata: &mut Map<String, Value>, key: &str, value: Option<bool>) {
    let Some(value) = value else {
        return;
    };
    if metadata.contains_key(key) {
        return;
    }
    metadata.insert(key.to_string(), Value::Bool(value));
}

fn usage_routing_snapshot_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<UsageRoutingSnapshot, DataLayerError> {
    Ok(UsageRoutingSnapshot {
        candidate_id: row_try_get_optional(row, "routing_candidate_id")?,
        candidate_index: row_try_get_optional::<i32>(row, "routing_candidate_index")?
            .map(|value| to_u64(value, "usage_routing_snapshots.candidate_index"))
            .transpose()?,
        key_name: row_try_get_optional(row, "routing_key_name")?,
        planner_kind: row_try_get_optional(row, "routing_planner_kind")?,
        route_family: row_try_get_optional(row, "routing_route_family")?,
        route_kind: row_try_get_optional(row, "routing_route_kind")?,
        execution_path: row_try_get_optional(row, "routing_execution_path")?,
        local_execution_runtime_miss_reason: row_try_get_optional(
            row,
            "routing_local_execution_runtime_miss_reason",
        )?,
        selected_provider_id: None,
        selected_endpoint_id: None,
        selected_provider_api_key_id: None,
        has_format_conversion: None,
    })
}

fn usage_settlement_pricing_snapshot_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<UsageSettlementPricingSnapshot, DataLayerError> {
    Ok(UsageSettlementPricingSnapshot {
        billing_status: None,
        billing_snapshot_schema_version: row_try_get_optional(
            row,
            "settlement_billing_snapshot_schema_version",
        )?,
        billing_snapshot_status: row_try_get_optional(row, "settlement_billing_snapshot_status")?,
        settlement_snapshot_schema_version: row_try_get_optional(
            row,
            "settlement_snapshot_schema_version",
        )?,
        settlement_snapshot: row_try_get_optional(row, "settlement_snapshot")?,
        billing_dimensions: row_try_get_optional(row, "settlement_billing_dimensions")?,
        billing_input_tokens: row_try_get_optional(row, "settlement_billing_input_tokens")?,
        billing_effective_input_tokens: row_try_get_optional(
            row,
            "settlement_billing_effective_input_tokens",
        )?,
        billing_output_tokens: row_try_get_optional(row, "settlement_billing_output_tokens")?,
        billing_cache_creation_tokens: row_try_get_optional(
            row,
            "settlement_billing_cache_creation_tokens",
        )?,
        billing_cache_creation_5m_tokens: row_try_get_optional(
            row,
            "settlement_billing_cache_creation_5m_tokens",
        )?,
        billing_cache_creation_1h_tokens: row_try_get_optional(
            row,
            "settlement_billing_cache_creation_1h_tokens",
        )?,
        billing_cache_read_tokens: row_try_get_optional(
            row,
            "settlement_billing_cache_read_tokens",
        )?,
        billing_total_input_context: row_try_get_optional(
            row,
            "settlement_billing_total_input_context",
        )?,
        billing_cache_creation_cost_usd: row_try_get_optional(
            row,
            "settlement_billing_cache_creation_cost_usd",
        )?,
        billing_cache_read_cost_usd: row_try_get_optional(
            row,
            "settlement_billing_cache_read_cost_usd",
        )?,
        billing_total_cost_usd: row_try_get_optional(row, "settlement_billing_total_cost_usd")?,
        billing_actual_total_cost_usd: row_try_get_optional(
            row,
            "settlement_billing_actual_total_cost_usd",
        )?,
        billing_pricing_source: row_try_get_optional(row, "settlement_billing_pricing_source")?,
        billing_rule_id: row_try_get_optional(row, "settlement_billing_rule_id")?,
        billing_rule_version: row_try_get_optional(row, "settlement_billing_rule_version")?,
        rate_multiplier: row_try_get_optional(row, "settlement_rate_multiplier")?,
        is_free_tier: row_try_get_optional(row, "settlement_is_free_tier")?,
        input_price_per_1m: row_try_get_optional(row, "settlement_input_price_per_1m")?,
        output_price_per_1m: row_try_get_optional(row, "settlement_output_price_per_1m")?,
        cache_creation_price_per_1m: row_try_get_optional(
            row,
            "settlement_cache_creation_price_per_1m",
        )?,
        cache_read_price_per_1m: row_try_get_optional(row, "settlement_cache_read_price_per_1m")?,
        price_per_request: row_try_get_optional(row, "settlement_price_per_request")?,
    })
}

fn attach_usage_settlement_pricing_snapshot_metadata(
    metadata: Option<Value>,
    snapshot: &UsageSettlementPricingSnapshot,
) -> Option<Value> {
    if !snapshot.any_present() {
        return metadata;
    }

    let mut metadata = match metadata {
        Some(Value::Object(object)) => object,
        Some(value) => return Some(value),
        None => Map::new(),
    };
    maybe_insert_string_value(
        &mut metadata,
        "billing_snapshot_schema_version",
        snapshot.billing_snapshot_schema_version.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "billing_snapshot_status",
        snapshot.billing_snapshot_status.as_deref(),
    );
    maybe_insert_string_value(
        &mut metadata,
        "settlement_snapshot_schema_version",
        snapshot.settlement_snapshot_schema_version.as_deref(),
    );
    if !metadata.contains_key("settlement_snapshot") {
        if let Some(value) = snapshot.settlement_snapshot.clone() {
            metadata.insert("settlement_snapshot".to_string(), value);
        }
    }
    if !metadata.contains_key("billing_dimensions") {
        if let Some(value) = snapshot.billing_dimensions.clone() {
            metadata.insert("billing_dimensions".to_string(), value);
        }
    }
    maybe_insert_number_value(&mut metadata, "rate_multiplier", snapshot.rate_multiplier);
    maybe_insert_bool_value(&mut metadata, "is_free_tier", snapshot.is_free_tier);
    maybe_insert_number_value(
        &mut metadata,
        "input_price_per_1m",
        snapshot.input_price_per_1m,
    );
    maybe_insert_number_value(
        &mut metadata,
        "output_price_per_1m",
        snapshot.output_price_per_1m,
    );
    maybe_insert_number_value(
        &mut metadata,
        "cache_creation_price_per_1m",
        snapshot.cache_creation_price_per_1m,
    );
    maybe_insert_number_value(
        &mut metadata,
        "cache_read_price_per_1m",
        snapshot.cache_read_price_per_1m,
    );
    maybe_insert_number_value(
        &mut metadata,
        "price_per_request",
        snapshot.price_per_request,
    );
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

fn usage_body_sql_columns(field: UsageBodyField) -> (&'static str, &'static str) {
    match field {
        UsageBodyField::RequestBody => ("request_body", "request_body_compressed"),
        UsageBodyField::ProviderRequestBody => {
            ("provider_request_body", "provider_request_body_compressed")
        }
        UsageBodyField::ResponseBody => ("response_body", "response_body_compressed"),
        UsageBodyField::ClientResponseBody => {
            ("client_response_body", "client_response_body_compressed")
        }
    }
}

#[cfg(test)]
mod tests;
