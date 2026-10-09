use super::InMemoryUsageReadRepository;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::usage::*;
use aether_data_contracts::repository::users::StoredUserSummary;
use aether_data_contracts::DataLayerError;
use chrono::{DateTime, TimeZone, Timelike, Utc};
use std::collections::{BTreeMap, BTreeSet};

fn metadata<'a>(row: &'a StoredRequestUsageAudit, group: &str, field: &str) -> Option<&'a str> {
    row.request_metadata
        .as_ref()?
        .get(group)?
        .get(field)?
        .as_str()
}
fn standalone(row: &StoredRequestUsageAudit, keys: &BTreeMap<String, bool>) -> Option<bool> {
    row.api_key_id
        .as_ref()
        .and_then(|id| keys.get(id).copied())
        .or_else(|| {
            row.request_metadata
                .as_ref()?
                .get("analytics_attribution")?
                .get("is_standalone")?
                .as_bool()
        })
        .or_else(|| {
            row.request_metadata
                .as_ref()?
                .get("api_key_is_standalone")?
                .as_bool()
        })
        .or_else(|| row.api_key_id.is_none().then_some(false))
}
pub(super) fn actor<'a>(
    row: &'a StoredRequestUsageAudit,
    keys: &BTreeMap<String, bool>,
) -> Option<&'a str> {
    (standalone(row, keys) == Some(false))
        .then_some(row.user_id.as_deref())
        .flatten()
}
pub(super) fn attribution(
    row: &StoredRequestUsageAudit,
    keys: &BTreeMap<String, bool>,
) -> &'static str {
    match (row.user_id.as_ref(), standalone(row, keys)) {
        (Some(_), Some(false)) => "employee",
        (Some(_), Some(true)) => "standalone",
        _ => "unknown",
    }
}
fn available(row: &StoredRequestUsageAudit, key: &str) -> bool {
    row.request_metadata
        .as_ref()
        .and_then(|metadata| metadata.get(key))
        .and_then(serde_json::Value::as_bool)
        != Some(false)
}
/// 提供商明细分组的展示名：分组键依旧是 provider_id（保证与 PostgreSQL 实现一致），
/// 只是把展示标签换成使用记录里的提供商名称快照，名称缺失或为历史占位值时回退到 provider_id。
/// provider_id 为空说明无法归属，保持 None 让前端显示“未归属提供商”。
fn provider_display_label(
    rows: &[&StoredRequestUsageAudit],
    group_id: Option<&str>,
) -> Option<String> {
    let id = group_id?;
    rows.iter()
        .map(|row| row.provider_name.trim())
        .filter(|name| {
            !name.is_empty()
                && !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "unknown" | "unknow" | "pending"
                )
        })
        .max()
        .map(str::to_owned)
        .or_else(|| Some(id.to_owned()))
}
// The legacy audit contract stores epoch seconds despite its historical field name.
fn usage_started_ms(row: &StoredRequestUsageAudit) -> u64 {
    row.created_at_unix_ms.saturating_mul(1000)
}

fn timestamp(ms: u64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms as i64)
        .unwrap_or_default()
        .to_rfc3339()
}
fn amount_units(value: &Option<String>) -> Option<i128> {
    let (whole, fraction) = value.as_ref()?.split_once('.')?;
    if fraction.len() > 8 {
        return None;
    }
    let sign = if whole.starts_with('-') { -1 } else { 1 };
    Some(
        sign * (whole.trim_start_matches('-').parse::<i128>().ok()? * 100_000_000
            + fraction.parse::<i128>().ok()? * 10_i128.pow((8 - fraction.len()) as u32)),
    )
}

fn apply_allocations(
    metrics: &mut UsageAnalyticsMetrics,
    rows: &[&StoredRequestUsageAudit],
    allocations: &BTreeMap<String, UsageAnalyticsAllocation>,
) {
    let selected = rows
        .iter()
        .filter_map(|row| allocations.get(&row.request_id))
        .collect::<Vec<_>>();
    metrics.allocation_available_count = selected
        .iter()
        .filter(|allocation| allocation.complete)
        .count() as u64;
    let sum = |selected: &[&UsageAnalyticsAllocation],
               read: fn(&UsageAnalyticsAllocation) -> &Option<String>| {
        let values = selected
            .iter()
            .filter_map(|allocation| amount_units(read(allocation)))
            .collect::<Vec<_>>();
        if values.is_empty() {
            return None;
        }
        let sum: i128 = values.iter().sum();
        Some(format!(
            "{}{}.{:08}",
            if sum < 0 { "-" } else { "" },
            sum.abs() / 100_000_000,
            sum.abs() % 100_000_000
        ))
    };
    metrics.quota_covered_amount = sum(&selected, |a| &a.quota_covered_amount);
    metrics.wallet_consumed_amount = sum(&selected, |a| &a.wallet_consumed_amount);
    metrics.wallet_debit_amount = sum(&selected, |a| &a.wallet_debit_amount);
    metrics.wallet_recharge_debit_amount = sum(&selected, |a| &a.wallet_recharge_debit_amount);
    metrics.wallet_gift_debit_amount = sum(&selected, |a| &a.wallet_gift_debit_amount);
    metrics.wallet_overdraft_amount = sum(&selected, |a| &a.wallet_overdraft_amount);
    let priced = rows
        .iter()
        .filter(|row| {
            available(row, USAGE_AVAILABLE_METADATA_KEY)
                && available(row, USAGE_PRICING_AVAILABLE_METADATA_KEY)
        })
        .filter_map(|row| allocations.get(&row.request_id))
        .collect::<Vec<_>>();
    metrics.cache_read_cost_amount = sum(&priced, |a| &a.cache_read_cost_amount);
    metrics.cache_creation_cost_amount = sum(&priced, |a| &a.cache_creation_cost_amount);
    metrics.cache_estimated_full_cost_amount =
        sum(&priced, |a| &a.cache_estimated_full_cost_amount);
    metrics.cache_pricing_available_count = priced
        .iter()
        .filter(|allocation| allocation.cache_estimated_full_cost_amount.is_some())
        .count() as u64;
}
fn decimal_sum(
    rows: &[&StoredRequestUsageAudit],
    value: impl Fn(&StoredRequestUsageAudit) -> Option<f64>,
) -> Option<String> {
    let amounts = rows
        .iter()
        .filter(|row| {
            available(row, USAGE_PRICING_AVAILABLE_METADATA_KEY) && row.billing_status == "settled"
        })
        .filter_map(|row| value(row))
        .filter(|amount| amount.is_finite())
        .map(|amount| (amount * 100_000_000.0).round() as i128)
        .collect::<Vec<_>>();
    if amounts.is_empty() {
        None
    } else {
        let sum: i128 = amounts.iter().sum();
        Some(format!(
            "{}{}.{:08}",
            if sum < 0 { "-" } else { "" },
            sum.abs() / 100_000_000,
            sum.abs() % 100_000_000
        ))
    }
}
fn metrics(
    rows: &[&StoredRequestUsageAudit],
    slow: u64,
    keys: &BTreeMap<String, bool>,
) -> UsageAnalyticsMetrics {
    let mut metrics = UsageAnalyticsMetrics::default();
    let mut latencies = Vec::new();
    let mut first_bytes = Vec::new();
    let mut users = BTreeSet::new();
    for row in rows {
        metrics.request_count += 1;
        match row.status.as_str() {
            "completed" => metrics.successful_request_count += 1,
            "failed" => metrics.failed_request_count += 1,
            "cancelled" => metrics.cancelled_request_count += 1,
            _ => metrics.in_flight_request_count += 1,
        }
        if available(row, USAGE_AVAILABLE_METADATA_KEY) {
            metrics.usage_available_count += 1;
            match metadata(row, "analytics_measurement", "source") {
                Some("reported") => metrics.reported_usage_count += 1,
                Some("estimated") => metrics.estimated_usage_count += 1,
                Some("mixed") => metrics.mixed_usage_count += 1,
                _ => metrics.unknown_usage_count += 1,
            }
            metrics.input_tokens += row.input_tokens;
            metrics.output_tokens += row.output_tokens;
            metrics.total_tokens += row.total_tokens;
            metrics.cache_read_input_tokens += row.cache_read_input_tokens;
            metrics.cache_creation_input_tokens += row.cache_creation_input_tokens;
        } else {
            metrics.unknown_usage_count += 1;
        }
        if row.billing_status == "settled" {
            metrics.settled_count += 1;
            if available(row, USAGE_PRICING_AVAILABLE_METADATA_KEY) {
                metrics.pricing_available_count += 1;
            }
        }
        if let Some(actor) = actor(row, keys) {
            users.insert(actor);
            metrics.trusted_attribution_count += 1;
        }
        if let Some(value) = row.response_time_ms {
            latencies.push(value);
            metrics.latency_sum_ms += value as f64;
            if value >= slow {
                metrics.slow_request_count += 1;
            }
        }
        if let Some(value) = row.first_byte_time_ms {
            first_bytes.push(value);
            metrics.first_byte_sum_ms += value as f64;
        }
        let upstream_is_stream = row
            .request_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("upstream_is_stream"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(row.is_stream);
        if upstream_is_stream
            && available(row, USAGE_AVAILABLE_METADATA_KEY)
            && row.output_tokens > 0
        {
            if let (Some(duration), Some(first_byte)) =
                (row.response_time_ms, row.first_byte_time_ms)
            {
                if duration > first_byte {
                    metrics.output_tps_sample_count += 1;
                    metrics.output_tps_sum +=
                        row.output_tokens as f64 * 1000.0 / (duration - first_byte) as f64;
                }
            }
        }
        if row.status == "failed"
            && metadata(row, "analytics_failure", "origin")
                .is_some_and(|origin| origin != "unknown")
        {
            metrics.classified_failure_count += 1;
        }
    }
    latencies.sort_unstable();
    let percentile = |fraction: f64| {
        if latencies.is_empty() {
            return None;
        }
        let rank = (latencies.len() - 1) as f64 * fraction;
        let lower = rank.floor() as usize;
        let upper = rank.ceil() as usize;
        Some(latencies[lower] as f64 + (latencies[upper] - latencies[lower]) as f64 * rank.fract())
    };
    metrics.latency_sample_count = latencies.len() as u64;
    metrics.latency_p50_ms = percentile(0.5);
    metrics.latency_p95_ms = percentile(0.95);
    metrics.latency_p99_ms = percentile(0.99);
    metrics.latency_p90_ms = percentile(0.9);
    first_bytes.sort_unstable();
    let first_percentile = |fraction: f64| {
        if first_bytes.is_empty() {
            return None;
        }
        let rank = (first_bytes.len() - 1) as f64 * fraction;
        let lower = rank.floor() as usize;
        let upper = rank.ceil() as usize;
        Some(
            first_bytes[lower] as f64
                + (first_bytes[upper] - first_bytes[lower]) as f64 * rank.fract(),
        )
    };
    metrics.first_byte_sample_count = first_bytes.len() as u64;
    metrics.first_byte_p90_ms = first_percentile(0.9);
    metrics.first_byte_p99_ms = first_percentile(0.99);
    metrics.usage_active_users = users.len() as u64;
    metrics.rated_amount = decimal_sum(rows, |row| Some(row.total_cost_usd));
    metrics.billable_amount = decimal_sum(rows, |row| row.billing_cost());
    metrics
}

fn dashboard_total_metrics(
    rows: &[&StoredRequestUsageAudit],
    allocations: &BTreeMap<String, UsageAnalyticsAllocation>,
) -> UsageAnalyticsMetrics {
    let mut metrics = UsageAnalyticsMetrics {
        request_count: rows.len() as u64,
        billable_amount: decimal_sum(rows, |row| row.billing_cost()),
        ..Default::default()
    };
    for row in rows {
        if available(row, USAGE_AVAILABLE_METADATA_KEY) {
            metrics.usage_available_count += 1;
            metrics.total_tokens += row.total_tokens;
        }
        if row.billing_status == "settled" {
            metrics.settled_count += 1;
            if available(row, USAGE_PRICING_AVAILABLE_METADATA_KEY) {
                metrics.pricing_available_count += 1;
            }
        }
        if allocations
            .get(&row.request_id)
            .is_some_and(|allocation| allocation.complete)
        {
            metrics.allocation_available_count += 1;
        }
    }
    metrics
}
fn matches(
    row: &StoredRequestUsageAudit,
    query: &UsageAnalyticsQuery,
    keys: &BTreeMap<String, bool>,
) -> bool {
    usage_started_ms(row) >= query.from_unix_ms
        && usage_started_ms(row) < query.to_unix_ms
        && metadata(row, "analytics_attribution", "record_kind") != Some("session")
        && query
            .actor_user_id
            .as_deref()
            .is_none_or(|value| actor(row, keys) == Some(value))
        && query
            .credential_owner_id
            .as_deref()
            .is_none_or(|value| row.user_id.as_deref() == Some(value))
        && query
            .attribution_kind
            .as_deref()
            .is_none_or(|value| attribution(row, keys) == value)
        && query
            .api_key_id
            .as_deref()
            .is_none_or(|value| row.api_key_id.as_deref() == Some(value))
        && query
            .model
            .as_deref()
            .is_none_or(|value| row.model == value)
        && query
            .provider_id
            .as_deref()
            .is_none_or(|value| row.provider_id.as_deref() == Some(value))
        && query
            .api_format
            .as_deref()
            .is_none_or(|value| row.api_format.as_deref() == Some(value))
        && query
            .endpoint_kind
            .as_deref()
            .is_none_or(|value| row.endpoint_kind.as_deref() == Some(value))
        && query
            .request_type
            .as_deref()
            .is_none_or(|value| row.request_type.as_deref() == Some(value))
        && query
            .status
            .as_deref()
            .is_none_or(|value| row.status == value)
        && query.is_stream.is_none_or(|value| row.is_stream == value)
        && query
            .has_format_conversion
            .is_none_or(|value| row.has_format_conversion == value)
}

impl InMemoryUsageReadRepository {
    pub fn with_analytics_allocations(
        self,
        allocations: impl IntoIterator<Item = UsageAnalyticsAllocation>,
    ) -> Self {
        *self
            .analytics_allocations
            .write()
            .expect("analytics allocations lock") = allocations
            .into_iter()
            .map(|allocation| (allocation.request_id.clone(), allocation))
            .collect();
        self
    }
    pub fn with_analytics_users(self, users: impl IntoIterator<Item = StoredUserSummary>) -> Self {
        *self.analytics_users.write().expect("analytics roster lock") = users.into_iter().collect();
        self
    }
    pub fn with_analytics_candidates(
        self,
        candidates: impl IntoIterator<Item = StoredRequestCandidate>,
    ) -> Self {
        *self
            .analytics_candidates
            .write()
            .expect("analytics candidates lock") = candidates.into_iter().collect();
        self
    }

    pub(super) fn analytics_query(
        &self,
        query: &UsageAnalyticsQuery,
    ) -> Result<StoredUsageAnalytics, DataLayerError> {
        query.validate()?;
        let keys = self.analytics_key_flags();
        let rows = self
            .by_request_id
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("usage lock poisoned".into()))?;
        let users = self
            .analytics_users
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("users lock poisoned".into()))?;
        let filtered = rows
            .values()
            .filter(|row| matches(row, query, &keys))
            .collect::<Vec<_>>();
        let allocations = self
            .analytics_allocations
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("allocation lock poisoned".into()))?;
        let metrics = |rows: &[&StoredRequestUsageAudit], slow| {
            let mut result = metrics(rows, slow, &keys);
            apply_allocations(&mut result, rows, &allocations);
            if query.view == UsageAnalyticsView::DashboardCharts {
                result.pricing_available_count = rows
                    .iter()
                    .filter(|row| {
                        row.billing_status == "settled"
                            && available(row, USAGE_PRICING_AVAILABLE_METADATA_KEY)
                            && row.billing_cost().is_some()
                    })
                    .count() as u64;
                if rows.is_empty() {
                    result.billable_amount = Some("0.00000000".into());
                    result.rated_amount = Some("0.00000000".into());
                }
            }
            result
        };
        let mut summary = metrics(&filtered, query.slow_threshold_ms.unwrap_or(5000));
        summary.enabled_users = users
            .iter()
            .filter(|user| user.is_active && !user.is_deleted)
            .count() as u64;
        let mut result = StoredUsageAnalytics {
            total: summary.request_count,
            summary,
            generated_at: Utc::now().to_rfc3339(),
            read_revision: format!(
                "memory:{}:{}",
                rows.len(),
                rows.values()
                    .map(|row| row.updated_at_unix_secs)
                    .max()
                    .unwrap_or(0)
            ),
            ..Default::default()
        };
        let tz = query
            .timezone
            .parse::<chrono_tz::Tz>()
            .expect("validated timezone");
        match query.view {
            UsageAnalyticsView::Summary => {}
            UsageAnalyticsView::Consumption => {
                let mut sorted = filtered.clone();
                sorted.sort_by(|left, right| {
                    let order = left.created_at_unix_ms.cmp(&right.created_at_unix_ms);
                    (if query.descending {
                        order.reverse()
                    } else {
                        order
                    })
                    .then(left.request_id.cmp(&right.request_id))
                });
                result.consumption = sorted
                    .into_iter()
                    .skip(query.offset as usize)
                    .take(query.limit as usize)
                    .map(|row| {
                        let metric = metrics(&[row], 5000);
                        UsageAnalyticsConsumption {
                            id: row.id.clone(),
                            request_id: row.request_id.clone(),
                            started_at: timestamp(usage_started_ms(row)),
                            user_id: actor(row, &keys).map(str::to_owned),
                            credential_owner_id: row.user_id.clone(),
                            model: row.model.clone(),
                            provider: Some(row.provider_name.clone()),
                            provider_id: row.provider_id.clone(),
                            api_key_id: row.api_key_id.clone(),
                            status: row.status.clone(),
                            settlement_status: row.billing_status.clone(),
                            attribution_kind: attribution(row, &keys).into(),
                            attribution_source: match attribution(row, &keys) {
                                "employee" => "user_account",
                                "standalone" => "standalone_key",
                                _ => "unknown",
                            }
                            .into(),
                            rated_amount: metric.rated_amount,
                            billable_amount: metric.billable_amount,
                            quota_covered_amount: metric.quota_covered_amount,
                            wallet_consumed_amount: metric.wallet_consumed_amount,
                            wallet_debit_amount: metric.wallet_debit_amount,
                        }
                    })
                    .collect();
            }
            UsageAnalyticsView::Users => {
                let mut roster = users
                    .iter()
                    .filter(|user| {
                        !user.is_deleted
                            && query
                                .user_is_active
                                .is_none_or(|value| user.is_active == value)
                            && query
                                .actor_user_id
                                .as_ref()
                                .or(query.credential_owner_id.as_ref())
                                .is_none_or(|value| user.id == *value)
                            && query.search.as_ref().is_none_or(|search| {
                                user.username
                                    .to_lowercase()
                                    .contains(&search.to_lowercase())
                                    || user.email.as_ref().is_some_and(|email| {
                                        email.to_lowercase().contains(&search.to_lowercase())
                                    })
                            })
                    })
                    .filter_map(|user| {
                        let usage = filtered
                            .iter()
                            .copied()
                            .filter(|row| {
                                if query.actor_user_id.is_some()
                                    || query.attribution_kind.as_deref() == Some("employee")
                                {
                                    actor(row, &keys) == Some(user.id.as_str())
                                } else {
                                    row.user_id.as_ref() == Some(&user.id)
                                }
                            })
                            .collect::<Vec<_>>();
                        if query
                            .has_usage
                            .is_some_and(|value| value == usage.is_empty())
                        {
                            return None;
                        }
                        let days = usage
                            .iter()
                            .map(|row| {
                                DateTime::<Utc>::from_timestamp_millis(usage_started_ms(row) as i64)
                                    .unwrap_or_default()
                                    .with_timezone(&tz)
                                    .date_naive()
                            })
                            .collect::<BTreeSet<_>>();
                        Some(UsageAnalyticsUser {
                            user_id: user.id.clone(),
                            username: user.username.clone(),
                            email: user.email.clone(),
                            is_active: user.is_active,
                            last_used_at: usage
                                .iter()
                                .map(|row| usage_started_ms(row))
                                .max()
                                .map(timestamp),
                            active_days: days.len() as u64,
                            metrics: metrics(&usage, query.slow_threshold_ms.unwrap_or(5000)),
                            // This adapter has no wallet/payment read model. Do
                            // not invent zero balances or zero credited orders.
                            finance: None,
                        })
                    })
                    .collect::<Vec<_>>();
                roster.sort_by(|a, b| {
                    let order = match query.sort {
                        UsageAnalyticsSort::Requests => {
                            a.metrics.request_count.cmp(&b.metrics.request_count)
                        }
                        UsageAnalyticsSort::Tokens => {
                            a.metrics.total_tokens.cmp(&b.metrics.total_tokens)
                        }
                        UsageAnalyticsSort::ActiveDays => a.active_days.cmp(&b.active_days),
                        UsageAnalyticsSort::Username => a.username.cmp(&b.username),
                        UsageAnalyticsSort::LastUsed | UsageAnalyticsSort::StartedAt => {
                            a.last_used_at.cmp(&b.last_used_at)
                        }
                        UsageAnalyticsSort::BillableAmount => {
                            amount_units(&a.metrics.billable_amount)
                                .cmp(&amount_units(&b.metrics.billable_amount))
                        }
                    };
                    (if query.descending {
                        order.reverse()
                    } else {
                        order
                    })
                    .then(a.user_id.cmp(&b.user_id))
                });
                result.total = roster.len() as u64;
                let selected_ids = roster
                    .iter()
                    .map(|user| user.user_id.as_str())
                    .collect::<BTreeSet<_>>();
                let selected_usage = filtered
                    .iter()
                    .copied()
                    .filter(|row| {
                        let subject = if query.actor_user_id.is_some()
                            || query.attribution_kind.as_deref() == Some("employee")
                        {
                            actor(row, &keys)
                        } else {
                            row.user_id.as_deref()
                        };
                        subject.is_some_and(|id| selected_ids.contains(id))
                    })
                    .collect::<Vec<_>>();
                result.summary = metrics(&selected_usage, query.slow_threshold_ms.unwrap_or(5000));
                result.summary.enabled_users =
                    roster.iter().filter(|user| user.is_active).count() as u64;
                result.user_summary = Some(UsageAnalyticsUserSummary {
                    user_count: result.total,
                    active_user_count: roster
                        .iter()
                        .filter(|user| user.metrics.request_count > 0)
                        .count() as u64,
                    metrics: result.summary.clone(),
                });
                result.users = roster
                    .into_iter()
                    .skip(query.offset as usize)
                    .take(query.limit as usize)
                    .collect();
            }
            UsageAnalyticsView::Timeseries
            | UsageAnalyticsView::Performance
            | UsageAnalyticsView::DashboardCharts
            | UsageAnalyticsView::Breakdown => {
                let mut groups = BTreeMap::<Option<String>, Vec<&StoredRequestUsageAudit>>::new();
                for row in filtered.iter().copied() {
                    let group = if query.view != UsageAnalyticsView::Breakdown {
                        let local =
                            DateTime::<Utc>::from_timestamp_millis(usage_started_ms(row) as i64)
                                .unwrap_or_default()
                                .with_timezone(&tz);
                        let bucket = match query.granularity {
                            UsageAnalyticsGranularity::Hour => local
                                .with_timezone(&Utc)
                                .with_minute(0)
                                .and_then(|d| d.with_second(0))
                                .and_then(|d| d.with_nanosecond(0))
                                .map(|d| d.with_timezone(&Utc)),
                            UsageAnalyticsGranularity::Day => Some(
                                UsageDashboardAnalyticsQuery {
                                    timezone: query.timezone.clone(),
                                }
                                .today_start(local.with_timezone(&Utc))?,
                            ),
                        };
                        bucket.map(|d| d.to_rfc3339())
                    } else {
                        match query.group_by {
                            UsageAnalyticsGroupBy::Model => Some(row.model.clone()),
                            UsageAnalyticsGroupBy::Provider => row.provider_id.clone(),
                            UsageAnalyticsGroupBy::ApiKey => row.api_key_id.clone(),
                            UsageAnalyticsGroupBy::Attribution => {
                                Some(attribution(row, &keys).into())
                            }
                            UsageAnalyticsGroupBy::ApiFormat => row.api_format.clone(),
                            UsageAnalyticsGroupBy::RequestType => row.request_type.clone(),
                        }
                    };
                    groups.entry(group).or_default().push(row);
                }
                // 提供商明细分组需要单独解析展示名，其余分组仍然用分组键本身作为标签。
                let provider_breakdown = query.view == UsageAnalyticsView::Breakdown
                    && query.group_by == UsageAnalyticsGroupBy::Provider;
                let mut grouped = groups
                    .into_iter()
                    .map(|(id, rows)| {
                        let mut metrics = metrics(&rows, query.slow_threshold_ms.unwrap_or(5000));
                        if query.view == UsageAnalyticsView::DashboardCharts {
                            metrics.unique_providers = Some(
                                rows.iter()
                                    .filter_map(|row| {
                                        row.provider_id
                                            .as_deref()
                                            .filter(|id| !id.is_empty())
                                            .or_else(|| {
                                                (!matches!(
                                                    row.provider_name.as_str(),
                                                    "" | "unknown" | "pending"
                                                ))
                                                .then_some(row.provider_name.as_str())
                                            })
                                    })
                                    .collect::<BTreeSet<_>>()
                                    .len() as u64,
                            );
                        }
                        UsageAnalyticsRow {
                            label: if provider_breakdown {
                                provider_display_label(&rows, id.as_deref())
                            } else {
                                id.clone()
                            },
                            bucket_start: (query.view != UsageAnalyticsView::Breakdown)
                                .then(|| id.clone())
                                .flatten(),
                            id,
                            metrics,
                        }
                    })
                    .collect::<Vec<_>>();
                if query.view == UsageAnalyticsView::Breakdown {
                    grouped.sort_by(|a, b| {
                        let order = match query.sort {
                            UsageAnalyticsSort::BillableAmount => {
                                amount_units(&a.metrics.billable_amount)
                                    .cmp(&amount_units(&b.metrics.billable_amount))
                            }
                            UsageAnalyticsSort::Tokens => {
                                a.metrics.total_tokens.cmp(&b.metrics.total_tokens)
                            }
                            _ => a.metrics.request_count.cmp(&b.metrics.request_count),
                        };
                        (if query.descending {
                            order.reverse()
                        } else {
                            order
                        })
                        .then(a.id.cmp(&b.id))
                    });
                }
                result.total = grouped.len() as u64;
                result.rows = if query.view == UsageAnalyticsView::Breakdown {
                    grouped
                        .into_iter()
                        .skip(query.offset as usize)
                        .take(query.limit as usize)
                        .collect()
                } else {
                    grouped
                };
            }
        }
        if matches!(
            query.view,
            UsageAnalyticsView::Timeseries
                | UsageAnalyticsView::Performance
                | UsageAnalyticsView::DashboardCharts
        ) {
            fill_usage_analytics_timeseries(query, &mut result.rows);
            result.total = result.rows.len() as u64;
        }
        if query.view == UsageAnalyticsView::DashboardCharts {
            let mut providers = BTreeMap::<Option<String>, Vec<&StoredRequestUsageAudit>>::new();
            let mut models = BTreeMap::<(String, String), Vec<&StoredRequestUsageAudit>>::new();
            for row in &filtered {
                let at = DateTime::<Utc>::from_timestamp_millis(usage_started_ms(row) as i64)
                    .expect("usage timestamp");
                let bucket = if query.granularity == UsageAnalyticsGranularity::Hour {
                    at.with_minute(0)
                        .and_then(|value| value.with_second(0))
                        .and_then(|value| value.with_nanosecond(0))
                        .expect("hour")
                } else {
                    UsageDashboardAnalyticsQuery {
                        timezone: query.timezone.clone(),
                    }
                    .today_start(at)?
                };
                providers
                    .entry(
                        row.provider_id
                            .clone()
                            .filter(|id| !id.is_empty())
                            .or_else(|| {
                                (!row.provider_name.is_empty()).then(|| row.provider_name.clone())
                            }),
                    )
                    .or_default()
                    .push(row);
                models
                    .entry((bucket.to_rfc3339(), row.model.clone()))
                    .or_default()
                    .push(row);
            }
            if providers.len() > USAGE_DASHBOARD_CHART_ROW_LIMIT
                || models.len() > USAGE_DASHBOARD_CHART_ROW_LIMIT
            {
                return Err(DataLayerError::InvalidInput(
                    "dashboard chart exceeds 10000 groups; narrow the range".into(),
                ));
            }
            result.provider_rows = providers
                .into_iter()
                .map(|(id, rows)| UsageAnalyticsRow {
                    label: rows.first().map(|row| row.provider_name.clone()),
                    id,
                    bucket_start: None,
                    metrics: metrics(&rows, query.slow_threshold_ms.unwrap_or(5000)),
                })
                .collect();
            result.model_rows = models
                .into_iter()
                .map(|((bucket, model), rows)| UsageAnalyticsRow {
                    id: Some(model.clone()),
                    label: Some(model),
                    bucket_start: Some(bucket),
                    metrics: metrics(&rows, query.slow_threshold_ms.unwrap_or(5000)),
                })
                .collect();
        }
        if query.view == UsageAnalyticsView::Performance {
            let mut providers = BTreeMap::<Option<String>, Vec<&StoredRequestUsageAudit>>::new();
            let mut models = BTreeMap::<String, Vec<&StoredRequestUsageAudit>>::new();
            let mut timeline =
                BTreeMap::<(String, Option<String>), Vec<&StoredRequestUsageAudit>>::new();
            let mut errors = BTreeMap::<String, u64>::new();
            for row in filtered {
                models.entry(row.model.clone()).or_default().push(row);
                let date = DateTime::<Utc>::from_timestamp_millis(usage_started_ms(row) as i64)
                    .unwrap_or_default();
                let bucket = if query.granularity == UsageAnalyticsGranularity::Hour {
                    date.with_minute(0)
                        .and_then(|value| value.with_second(0))
                        .and_then(|value| value.with_nanosecond(0))
                        .unwrap_or(date)
                } else {
                    tz.from_local_datetime(
                        &date
                            .with_timezone(&tz)
                            .date_naive()
                            .and_hms_opt(0, 0, 0)
                            .unwrap(),
                    )
                    .earliest()
                    .map(|date| date.with_timezone(&Utc))
                    .unwrap_or(date)
                };
                timeline
                    .entry((bucket.to_rfc3339(), row.provider_id.clone()))
                    .or_default()
                    .push(row);
                providers
                    .entry(row.provider_id.clone())
                    .or_default()
                    .push(row);
                if row.status == "failed" {
                    *errors
                        .entry(
                            metadata(row, "analytics_failure", "reason")
                                .or(row.error_category.as_deref())
                                .unwrap_or("unknown")
                                .into(),
                        )
                        .or_default() += 1;
                }
            }
            result.provider_rows = providers
                .into_iter()
                .map(|(id, rows)| UsageAnalyticsRow {
                    label: rows.first().map(|row| row.provider_name.clone()),
                    id,
                    bucket_start: None,
                    metrics: metrics(&rows, query.slow_threshold_ms.unwrap_or(5000)),
                })
                .collect();
            result.model_rows = models
                .into_iter()
                .map(|(model, rows)| UsageAnalyticsRow {
                    label: Some(model.clone()),
                    id: Some(model),
                    bucket_start: None,
                    metrics: metrics(&rows, query.slow_threshold_ms.unwrap_or(5000)),
                })
                .collect();
            result.model_rows.sort_by(|left, right| {
                right
                    .metrics
                    .request_count
                    .cmp(&left.metrics.request_count)
                    .then(left.id.cmp(&right.id))
            });
            result.errors = errors
                .into_iter()
                .map(|(reason, count)| UsageAnalyticsErrorCount { reason, count })
                .collect();
            result.provider_timeline_rows = timeline
                .into_iter()
                .map(|((bucket, id), rows)| UsageAnalyticsRow {
                    label: rows.first().map(|row| row.provider_name.clone()),
                    id,
                    bucket_start: Some(bucket),
                    metrics: metrics(&rows, query.slow_threshold_ms.unwrap_or(5000)),
                })
                .collect();
        }
        Ok(result)
    }

    pub(super) fn dashboard_analytics_query(
        &self,
        query: &UsageDashboardAnalyticsQuery,
    ) -> Result<StoredUsageDashboardAnalytics, DataLayerError> {
        query.validate()?;
        let keys = self.analytics_key_flags();
        let rows = self
            .by_request_id
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("usage lock poisoned".into()))?;
        let users = self
            .analytics_users
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("users lock poisoned".into()))?;
        let allocations = self
            .analytics_allocations
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("allocation lock poisoned".into()))?;
        let now = Utc::now();
        let today_from = query.today_start(now)?;
        let to = now.timestamp_millis().max(0) as u64;
        let total_from = rows
            .values()
            .map(usage_started_ms)
            .filter(|started| *started < to)
            .min();
        let revision = format!(
            "memory:{}:{}",
            rows.len(),
            rows.values()
                .map(|row| row.updated_at_unix_secs)
                .max()
                .unwrap_or(0)
        );
        let snapshot = |from, detailed| {
            let range = UsageAnalyticsQuery {
                from_unix_ms: from,
                to_unix_ms: to,
                timezone: query.timezone.clone(),
                limit: 1,
                ..Default::default()
            };
            let selected = rows
                .values()
                .filter(|row| matches(row, &range, &keys))
                .collect::<Vec<_>>();
            let mut summary = if detailed {
                let mut summary = metrics(&selected, 5000, &keys);
                apply_allocations(&mut summary, &selected, &allocations);
                summary
            } else {
                dashboard_total_metrics(&selected, &allocations)
            };
            if summary.request_count == 0 {
                summary.rated_amount = Some("0.00000000".into());
                summary.billable_amount = Some("0.00000000".into());
            }
            summary.enabled_users = users
                .iter()
                .filter(|user| user.is_active && !user.is_deleted)
                .count() as u64;
            StoredUsageAnalytics {
                total: summary.request_count,
                summary,
                read_revision: revision.clone(),
                generated_at: now.to_rfc3339(),
                ..Default::default()
            }
        };
        Ok(StoredUsageDashboardAnalytics {
            today: snapshot(today_from.timestamp_millis().max(0) as u64, true),
            total: snapshot(total_from.unwrap_or(to), false),
            today_from: today_from.to_rfc3339(),
            total_from: total_from.map(timestamp),
            to: now.to_rfc3339(),
            history_complete: None,
        })
    }

    pub(super) fn health_observations(
        &self,
        query: &HealthObservationQuery,
    ) -> Result<HealthObservationSummary, DataLayerError> {
        if query.from_unix_ms >= query.to_unix_ms
            || query.to_unix_ms - query.from_unix_ms > 31 * 86_400_000
            || query.segments == 0
            || query.segments > 96
        {
            return Err(DataLayerError::InvalidInput(
                "invalid health observation window".into(),
            ));
        }
        let rows = self
            .by_request_id
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("usage lock poisoned".into()))?;
        let candidates = self
            .analytics_candidates
            .read()
            .map_err(|_| DataLayerError::UnexpectedValue("candidate lock poisoned".into()))?;
        let width = (query.to_unix_ms - query.from_unix_ms).div_ceil(query.segments as u64);
        let timeline = || {
            (0..query.segments)
                .filter_map(|i| {
                    let from = query.from_unix_ms + i as u64 * width;
                    (from < query.to_unix_ms).then(|| HealthObservationBucket {
                        from_unix_ms: from,
                        to_unix_ms: (from + width).min(query.to_unix_ms),
                        metrics: Default::default(),
                    })
                })
                .collect::<Vec<_>>()
        };
        let mut result = HealthObservationSummary {
            timeline: timeline(),
            ..Default::default()
        };
        let mut objects = BTreeMap::<String, HealthObservationObject>::new();
        let mut observe =
            |object: Option<String>, time: u64, update: &dyn Fn(&mut HealthObservationMetrics)| {
                let Some(value) = object.filter(|value| {
                    query
                        .object_values
                        .as_ref()
                        .is_none_or(|allowed| allowed.contains(value))
                }) else {
                    return;
                };
                if time < query.from_unix_ms || time >= query.to_unix_ms {
                    return;
                }
                let index = ((time - query.from_unix_ms) / width) as usize;
                let object =
                    objects
                        .entry(value.clone())
                        .or_insert_with(|| HealthObservationObject {
                            object_value: value,
                            metrics: Default::default(),
                            timeline: timeline(),
                        });
                update(&mut result.overall);
                update(&mut object.metrics);
                update(&mut result.timeline[index].metrics);
                update(&mut object.timeline[index].metrics);
            };
        for row in rows
            .values()
            .filter(|row| metadata(row, "analytics_attribution", "record_kind") != Some("session"))
        {
            let object = match query.object_kind {
                HealthObservationObjectKind::ApiFormat => row.api_format.clone(),
                HealthObservationObjectKind::Model => Some(row.model.clone()),
                HealthObservationObjectKind::Provider => row.provider_id.clone(),
            };
            observe(object, usage_started_ms(row), &|m| {
                m.request_count += 1;
                m.last_request_at_unix_ms = Some(
                    m.last_request_at_unix_ms
                        .unwrap_or(0)
                        .max(usage_started_ms(row)),
                );
                if let Some(latency) = row.response_time_ms {
                    m.latency_sample_count += 1;
                    m.latency_sum_ms += latency as f64;
                }
                let origin = metadata(row, "analytics_failure", "origin");
                let excluded = matches!(row.status.as_str(), "failed" | "cancelled")
                    && ((row.status == "cancelled" && origin == Some("client"))
                        || (origin == Some("client")
                            && (matches!(
                                metadata(row, "analytics_failure", "stage"),
                                Some("authentication" | "admission")
                            ) || matches!(
                                metadata(row, "analytics_failure", "reason"),
                                Some(
                                    "invalid_input"
                                        | "invalid_credentials"
                                        | "quota_exceeded"
                                        | "policy_rejection"
                                )
                            ))));
                if excluded {
                    m.excluded_count += 1;
                }
                match row.status.as_str() {
                    "completed" => {
                        m.succeeded_count += 1;
                        m.service_succeeded_count += 1;
                    }
                    "cancelled" | "failed" => {
                        if row.status == "cancelled" {
                            m.cancelled_count += 1;
                        } else {
                            m.failed_count += 1;
                        }
                        if !excluded {
                            if matches!(origin, Some("gateway" | "upstream" | "transport")) {
                                m.service_failed_count += 1;
                            } else {
                                m.unknown_failure_count += 1;
                            }
                        }
                    }
                    _ => m.in_progress_count += 1,
                }
            });
        }
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.status.is_attempted(candidate.started_at_unix_ms))
        {
            let row = rows.get(&candidate.request_id);
            let object = match query.object_kind {
                HealthObservationObjectKind::ApiFormat => row.and_then(|r| r.api_format.clone()),
                HealthObservationObjectKind::Model => row.map(|r| r.model.clone()),
                HealthObservationObjectKind::Provider => candidate.provider_id.clone(),
            };
            observe(
                object,
                candidate.created_at_unix_ms,
                &|m| match candidate.status {
                    RequestCandidateStatus::Success => m.attempt_succeeded_count += 1,
                    RequestCandidateStatus::Failed => m.attempt_failed_count += 1,
                    RequestCandidateStatus::Cancelled => m.attempt_cancelled_count += 1,
                    _ => m.attempt_in_progress_count += 1,
                },
            );
        }
        result.objects = objects.into_values().collect();
        Ok(result)
    }
}
